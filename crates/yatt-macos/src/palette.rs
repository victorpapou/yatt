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
            // ...and the chosen one carries a tick, no ring or border round any.
            let ring = objc2_quartz_core::CAShapeLayer::new();
            ring.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(D, D)));
            ring.setFillColor(None);
            ring.setStrokeColor(Some(&NSColor::whiteColor().CGColor()));
            ring.setLineWidth(2.2);
            unsafe { ring.setLineCap(objc2_quartz_core::kCALineCapRound) };
            ring.setLineJoin(unsafe { objc2_quartz_core::kCALineJoinRound });
            let path = objc2_core_graphics::CGMutablePath::new();
            unsafe {
                objc2_core_graphics::CGMutablePath::move_to_point(Some(&path), std::ptr::null(), D * 0.32, D * 0.50);
                objc2_core_graphics::CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), D * 0.44, D * 0.38);
                objc2_core_graphics::CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), D * 0.69, D * 0.64);
            }
            ring.setPath(Some(&path));
            ring.setHidden(true);
            layer.addSublayer(&ring);
            *this.ivars().ring.borrow_mut() = Some(Retained::into_super(ring));
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
