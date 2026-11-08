//! The inspector's tags: small rounded capsules in a wrapping row, the tags the
//! #tags of the notes make (the notes are the one place to write them).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont, NSFontWeightMedium, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::classes::FlippedView;
use crate::style;

const CAPSULE_H: f64 = 20.0;
/// Horizontal padding of a capsule's text: a little, not a plate.
const CAPSULE_PAD: f64 = 6.0;
const GAP: f64 = 4.0;
const ROW_GAP: f64 = 4.0;
const PAD: f64 = 20.0;

pub struct Tags {
    pub view: Retained<FlippedView>,
    capsules: RefCell<Vec<Retained<FlippedView>>>,
    known: Rc<RefCell<Vec<String>>>,
    /// The width and height the row was last laid out for.
    laid: Cell<(f64, f64)>,
    /// Called when the row's height changed (the pane lays itself out again).
    pub on_resize: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    mtm: MainThreadMarker,
}

impl Tags {
    pub fn new(mtm: MainThreadMarker) -> Rc<Self> {
        Rc::new(Self {
            view: FlippedView::new(mtm, NSRect::ZERO),
            capsules: RefCell::default(),
            known: Rc::default(),
            laid: Cell::new((0.0, 0.0)),
            on_resize: Rc::default(),
            mtm,
        })
    }

    pub fn as_view(&self) -> &objc2_app_kit::NSView {
        &self.view
    }

    /// Shows `tags`.
    pub fn set(self: &Rc<Self>, tags: &[String]) {
        if *self.known.borrow() == tags {
            return;
        }
        *self.known.borrow_mut() = tags.to_vec();
        self.rebuild();
    }

    /// Draws the capsules again (a tag's colour was changed in the settings).
    pub fn refresh(self: &Rc<Self>) {
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        for c in self.capsules.borrow_mut().drain(..) {
            c.removeFromSuperview();
        }
        let mut caps = Vec::new();
        for tag in self.known.borrow().iter() {
            let b = badge(self.mtm, tag);
            self.view.addSubview(&b);
            caps.push(b);
        }
        *self.capsules.borrow_mut() = caps;
        self.layout(self.laid.get().0);
        if let Some(f) = self.on_resize.borrow().as_ref() {
            f();
        }
    }

    /// Lays the capsules out in rows `width` wide; returns the height.
    pub fn layout(&self, width: f64) -> f64 {
        let inner = (width - 2.0 * PAD).max(40.0);
        let (mut x, mut y) = (0.0f64, 0.0f64);
        let caps = self.capsules.borrow();
        for c in caps.iter() {
            let w = c.frame().size.width;
            if x > 0.0 && x + w > inner {
                x = 0.0;
                y += CAPSULE_H + ROW_GAP;
            }
            c.setFrame(NSRect::new(
                NSPoint::new(PAD + 2.0 + x, y),
                NSSize::new(w, CAPSULE_H),
            ));
            x += w + GAP;
        }
        let h = if caps.is_empty() { 0.0 } else { y + CAPSULE_H };
        self.laid.set((width, h));
        h
    }
}

/// A tag's badge, one look everywhere (the editor's `.tag`): a solid fill of the
/// tag's colour, white text, medium, centred; the size of it.
pub fn badge(mtm: MainThreadMarker, name: &str) -> Retained<FlippedView> {
    let hue = style::tag_hue(name);
    let label = NSTextField::labelWithString(&NSString::from_str(name), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize_weight(12.5, unsafe {
        NSFontWeightMedium
    })));
    label.setTextColor(Some(&NSColor::whiteColor()));
    let size = label.fittingSize();
    let w = (size.width.ceil() - 4.0 + 2.0 * CAPSULE_PAD).max(0.0).min(240.0);
    let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(w, CAPSULE_H)));
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(8.0);
        layer.setBackgroundColor(Some(
            &style::swatch(hue).CGColor(),
        ));
    }
    let h = size.height.ceil();
    label.setFrame(NSRect::new(
        NSPoint::new(CAPSULE_PAD - 2.0, ((CAPSULE_H - h) / 2.0).round()),
        NSSize::new(size.width.ceil(), h),
    ));
    view.addSubview(&label);
    view
}
