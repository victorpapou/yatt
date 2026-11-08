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
    capsules: RefCell<Vec<Retained<NSTextField>>>,
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
            let l = NSTextField::labelWithString(&NSString::from_str(tag), self.mtm);
            l.setFont(Some(&NSFont::systemFontOfSize_weight(11.5, unsafe {
                NSFontWeightMedium
            })));
            l.setTextColor(Some(&NSColor::labelColor()));
            l.setAlignment(objc2_app_kit::NSTextAlignment::Center);
            l.setWantsLayer(true);
            if let Some(layer) = l.layer() {
                layer.setCornerRadius(CAPSULE_H / 2.0);
                layer.setBackgroundColor(Some(
                    &style::swatch(style::tag_hue(tag))
                        .colorWithAlphaComponent(0.28)
                        .CGColor(),
                ));
            }
            self.view.addSubview(&l);
            caps.push(l);
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
            let w = c.fittingSize().width.ceil() - 4.0 + 2.0 * CAPSULE_PAD;
            if x > 0.0 && x + w > inner {
                x = 0.0;
                y += CAPSULE_H + ROW_GAP;
            }
            c.setFrame(NSRect::new(
                NSPoint::new(PAD + x, y + 1.0),
                NSSize::new(w.max(0.0), CAPSULE_H - 2.0),
            ));
            x += w + GAP;
        }
        let h = if caps.is_empty() { 0.0 } else { y + CAPSULE_H };
        self.laid.set((width, h));
        h
    }
}
