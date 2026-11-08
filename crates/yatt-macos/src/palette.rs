//! The new-task capsule's colour: a small popover off the palette button, a
//! grid of round swatches (Freeform's), the chosen one ringed.

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
            layer.setCornerRadius(D / 2.0);
            layer.setBackgroundColor(Some(&style::swatch(hue).CGColor()));
            // The white ring inside the colour, shown on the chosen one.
            let ring = CALayer::new();
            ring.setFrame(NSRect::new(
                NSPoint::new(4.0, 4.0),
                NSSize::new(D - 8.0, D - 8.0),
            ));
            ring.setCornerRadius((D - 8.0) / 2.0);
            ring.setBorderWidth(2.5);
            ring.setBorderColor(Some(&NSColor::whiteColor().CGColor()));
            ring.setHidden(true);
            layer.addSublayer(&ring);
            *this.ivars().ring.borrow_mut() = Some(ring);
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
}

impl Palette {
    /// `on_pick` gets the chosen colour.
    pub fn new(mtm: MainThreadMarker, on_pick: impl Fn(usize) + 'static) -> Self {
        let on_pick = Rc::new(on_pick);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(true);
        let rows = HUE_SWATCH_ORDER.len().div_ceil(PER_ROW);
        let w = 2.0 * PAD + PER_ROW as f64 * D + (PER_ROW as f64 - 1.0) * GAP;
        let h = 2.0 * PAD + rows as f64 * D + (rows as f64 - 1.0) * GAP;
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
                pick(hue);
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
        let vc = NSViewController::new(mtm);
        vc.setView(&content);
        popover.setContentViewController(Some(&vc));
        popover.setContentSize(NSSize::new(w, h));
        Self { popover, swatches }
    }

    /// Rings the chosen colour.
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
