//! The new-task capsule's colour: a small popover off the palette button, a
//! grid of round swatches (Freeform's), the chosen one ticked. The one colour
//! picker of the app: the capsule, the settings and the tags use it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSEvent, NSPopover, NSPopoverBehavior, NSView, NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize};
use objc2_quartz_core::CALayer;

use crate::picker::HUE_SWATCH_ORDER;
use crate::style;

const D: f64 = 28.0;
const GAP: f64 = 12.0;
const PAD: f64 = 16.0;
const PER_ROW: usize = 5;

pub struct SwatchIvars {
    hue: usize,
    on_pick: Box<dyn Fn()>,
    selected: Cell<bool>,
    ring: RefCell<Option<Retained<CALayer>>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SwatchIvars]
    pub struct SwatchView;

    impl SwatchView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            (self.ivars().on_pick)();
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }
    }

    unsafe impl NSObjectProtocol for SwatchView {}
);

impl SwatchView {
    fn new(mtm: MainThreadMarker, hue: usize, on_pick: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SwatchIvars {
            hue,
            on_pick: Box::new(on_pick),
            selected: Cell::new(false),
            ring: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::ZERO, NSSize::new(D, D))]
        };
        this.setWantsLayer(true);
        if let Some(layer) = this.layer() {
            // The colour, a little inside the cell...
            let dot = CALayer::new();
            dot.setFrame(NSRect::new(NSPoint::new(3.0, 3.0), NSSize::new(D - 6.0, D - 6.0)));
            dot.setCornerRadius((D - 6.0) / 2.0);
            dot.setBackgroundColor(Some(&style::swatch(hue).CGColor()));
            layer.addSublayer(&dot);
            // ...and the chosen one carries the SF Symbol checkmark (semibold), dark on
            // a light colour and white on a dark one, by the colour's luminance.
            let (r, g, b) = style::swatch_rgb(hue);
            let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let ink = if luminance > 0.6 {
                NSColor::colorWithSRGBRed_green_blue_alpha(0.08, 0.08, 0.09, 1.0)
            } else {
                NSColor::whiteColor()
            };
            let tick = CALayer::new();
            tick.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(D, D)));
            tick.setContentsGravity(unsafe { objc2_quartz_core::kCAGravityCenter });
            tick.setContentsScale(2.0);
            if let Some(image) = objc2_app_kit::NSImage::imageWithSystemSymbolName_accessibilityDescription(
                objc2_foundation::ns_string!("checkmark"),
                None,
            ) {
                let weight = objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    9.0,
                    unsafe { objc2_app_kit::NSFontWeightSemibold },
                );
                let colour = objc2_app_kit::NSImageSymbolConfiguration::configurationWithPaletteColors(
                    &objc2_foundation::NSArray::from_retained_slice(&[ink]),
                );
                let config = weight.configurationByApplyingConfiguration(&colour);
                if let Some(img) = image.imageWithSymbolConfiguration(&config) {
                    let size = img.size();
                    let mut proposed = NSRect::new(NSPoint::ZERO, NSSize::new(size.width * 2.0, size.height * 2.0));
                    if let Some(cg) = unsafe {
                        img.CGImageForProposedRect_context_hints(&mut proposed, None, None)
                    } {
                        unsafe { tick.setContents(Some(&*(&*cg as *const _ as *const objc2::runtime::AnyObject))) };
                    }
                }
            }
            tick.setHidden(true);
            layer.addSublayer(&tick);
            *this.ivars().ring.borrow_mut() = Some(tick);
        }
        this
    }

    fn select(&self, on: bool) {
        self.ivars().selected.set(on);
        if let Some(ring) = self.ivars().ring.borrow().as_ref() {
            ring.setHidden(!on);
        }
    }
}

pub struct Palette {
    popover: Retained<NSPopover>,
    swatches: Vec<(usize, Retained<SwatchView>)>,
    _retained: Vec<Retained<objc2::runtime::AnyObject>>,
}

impl Palette {
    /// `on_pick` gets the chosen colour.
    pub fn new(mtm: MainThreadMarker, on_pick: impl Fn(usize) + 'static) -> Self {
        Self::build(mtm, move |p| {
            if let Some(h) = p {
                on_pick(h)
            }
        }, false)
    }

    /// For a tag: the same grid, with «Автоматически» (`None`) and «Случайный»
    /// (a colour drawn now) under it.
    pub fn for_tag(mtm: MainThreadMarker, on_pick: impl Fn(Option<usize>) + 'static) -> Self {
        Self::build(mtm, on_pick, true)
    }

    fn build(mtm: MainThreadMarker, on_pick: impl Fn(Option<usize>) + 'static, extras: bool) -> Self {
        let on_pick: Rc<dyn Fn(Option<usize>)> = Rc::new(on_pick);
        let mut retained: Vec<Retained<objc2::runtime::AnyObject>> = Vec::new();
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(true);
        let rows = HUE_SWATCH_ORDER.len().div_ceil(PER_ROW);
        let w = 2.0 * PAD + PER_ROW as f64 * D + (PER_ROW as f64 - 1.0) * GAP;
        let extra_h = if extras { 40.0 } else { 0.0 };
        let h = 2.0 * PAD + rows as f64 * D + (rows as f64 - 1.0) * GAP + extra_h;
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(w, h)),
        );
        let mut swatches = Vec::new();
        for (slot, hue) in HUE_SWATCH_ORDER.iter().copied().enumerate() {
            let (col, row) = (slot % PER_ROW, slot / PER_ROW);
            let pop = popover.clone();
            let pick = on_pick.clone();
            let v = SwatchView::new(mtm, hue, move || {
                pick(Some(hue));
                pop.close();
            });
            // Rows from the top (the content view is not flipped).
            v.setFrame(NSRect::new(
                NSPoint::new(
                    PAD + col as f64 * (D + GAP),
                    h - PAD - D - row as f64 * (D + GAP),
                ),
                NSSize::new(D, D),
            ));
            content.addSubview(&v);
            swatches.push((hue, v));
        }
        if extras {
            let mut x = PAD;
            let bw = (w - 2.0 * PAD - GAP) / 2.0;
            for (title, random) in [("Автоматически", false), ("Случайный", true)] {
                let (pop, pick) = (popover.clone(), on_pick.clone());
                let target = crate::classes::ActionTarget::new(mtm, move || {
                    if random {
                        let n = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.subsec_nanos() as usize);
                        pick(Some(HUE_SWATCH_ORDER[n % HUE_SWATCH_ORDER.len()]));
                    } else {
                        pick(None);
                    }
                    pop.close();
                });
                let b = unsafe {
                    objc2_app_kit::NSButton::buttonWithTitle_target_action(
                        &objc2_foundation::NSString::from_str(title),
                        Some(&target),
                        Some(crate::classes::ActionTarget::sel()),
                        mtm,
                    )
                };
                b.setFrame(NSRect::new(NSPoint::new(x, PAD - 4.0), NSSize::new(bw, 28.0)));
                content.addSubview(&b);
                retained.push(Retained::into_super(Retained::into_super(target)));
                x += bw + GAP;
            }
        }
        let vc = NSViewController::new(mtm);
        vc.setView(&content);
        popover.setContentViewController(Some(&vc));
        popover.setContentSize(NSSize::new(w, h));
        Self {
            popover,
            swatches,
            _retained: retained,
        }
    }

    /// Development aid: the grid with `selected` ticked, as a PNG.
    pub fn debug_png(&self, mtm: MainThreadMarker, selected: usize, path: &str) -> bool {
        self.show_state(selected);
        let Some(vc) = self.popover.contentViewController() else { return false };
        let view = vc.view();
        let b = view.bounds();
        let win = unsafe {
            objc2_app_kit::NSWindow::initWithContentRect_styleMask_backing_defer(
                objc2_app_kit::NSWindow::alloc(mtm),
                b,
                objc2_app_kit::NSWindowStyleMask::Borderless,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
            )
        };
        win.setContentView(Some(&view));
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(b) else { return false };
        view.cacheDisplayInRect_toBitmapImageRep(b, &rep);
        let props = objc2_foundation::NSDictionary::new();
        let Some(data) = (unsafe {
            rep.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, &props)
        }) else {
            return false;
        };
        data.writeToFile_atomically(&objc2_foundation::NSString::from_str(path), true)
    }

    /// Ticks the chosen colour.
    pub fn show_state(&self, selected: usize) {
        for (hue, v) in &self.swatches {
            v.select(*hue == selected);
        }
    }

    /// Opens the popover off `anchor`.
    pub fn toggle(&self, anchor: &NSView) {
        if self.popover.isShown() {
            self.popover.close();
        } else {
            self.popover
                .showRelativeToRect_ofView_preferredEdge(anchor.bounds(), anchor, NSRectEdge::MaxY);
        }
    }
}
