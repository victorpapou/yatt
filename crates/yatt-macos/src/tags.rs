//! The inspector's tags: small rounded capsules in a wrapping row, then a field
//! to type the next one (Enter or a comma adds it). Clicking a capsule takes the
//! tag off. Edits are sent as the whole list.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSButton, NSColor, NSFocusRingType, NSFont, NSFontWeightMedium, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};

use crate::classes::{ActionTarget, FlippedView};
use crate::style;

const CAPSULE_H: f64 = 20.0;
/// Horizontal padding of a capsule's text: a little, not a plate.
const CAPSULE_PAD: f64 = 6.0;
const GAP: f64 = 4.0;
const ROW_GAP: f64 = 4.0;
const FIELD_MIN_W: f64 = 64.0;
const PAD: f64 = 20.0;

struct Capsule {
    button: Retained<NSButton>,
    _target: Retained<AnyObject>,
}

pub struct Tags {
    pub view: Retained<FlippedView>,
    capsules: RefCell<Vec<Capsule>>,
    field: Retained<NSTextField>,
    known: Rc<RefCell<Vec<String>>>,
    on_change: Rc<dyn Fn(Vec<String>)>,
    /// The width and height the row was last laid out for.
    laid: Cell<(f64, f64)>,
    /// Called when the row's height changed (the pane lays itself out again).
    pub on_resize: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    _targets: RefCell<Vec<Retained<AnyObject>>>,
    mtm: MainThreadMarker,
}

impl Tags {
    pub fn new(mtm: MainThreadMarker, on_change: impl Fn(Vec<String>) + 'static) -> Rc<Self> {
        let field = NSTextField::textFieldWithString(ns_string!(""), mtm);
        field.setEditable(true);
        field.setSelectable(true);
        field.setBordered(false);
        field.setBezeled(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        field.setPlaceholderString(Some(ns_string!("Тег")));
        if let Some(cell) = field.cell() {
            cell.setSendsActionOnEndEditing(true);
        }
        let this = Rc::new(Self {
            view: FlippedView::new(mtm, NSRect::ZERO),
            capsules: RefCell::default(),
            field,
            known: Rc::default(),
            on_change: Rc::new(on_change),
            laid: Cell::new((0.0, CAPSULE_H)),
            on_resize: Rc::default(),
            _targets: RefCell::default(),
            mtm,
        });
        this.view.addSubview(&this.field);
        let weak = Rc::downgrade(&this);
        let target = ActionTarget::new(mtm, move || {
            if let Some(t) = weak.upgrade() {
                t.add_typed();
            }
        });
        unsafe {
            this.field.setTarget(Some(&target));
            this.field.setAction(Some(ActionTarget::sel()));
        }
        this._targets
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(target)));
        this
    }

    pub fn as_view(&self) -> &objc2_app_kit::NSView {
        &self.view
    }

    /// The row's height for its last width.
    pub fn height(&self) -> f64 {
        self.laid.get().1
    }

    fn send(&self, tags: Vec<String>) {
        *self.known.borrow_mut() = tags.clone();
        (self.on_change)(tags);
    }

    /// Takes what is typed in the field as new tags (a comma ends one).
    fn add_typed(self: &Rc<Self>) {
        let typed = self.field.stringValue().to_string();
        let mut tags = self.known.borrow().clone();
        for part in typed.split(',') {
            let t = part.trim().to_string();
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        }
        self.field.setStringValue(ns_string!(""));
        if tags != *self.known.borrow() {
            self.send(tags.clone());
            self.rebuild(&tags);
        }
    }

    /// Draws the capsules again (a tag's colour was changed in the settings).
    pub fn refresh(self: &Rc<Self>) {
        let tags = self.known.borrow().clone();
        self.rebuild(&tags);
    }

    /// Flushes a tag still being typed (the pane is closing).
    pub fn flush(self: &Rc<Self>) {
        if !self.field.stringValue().to_string().trim().is_empty() {
            self.add_typed();
        }
    }

    /// Shows `tags` (unless they are what is already shown, or one is being typed).
    pub fn set(self: &Rc<Self>, tags: &[String]) {
        if *self.known.borrow() == tags || self.field.currentEditor().is_some() {
            return;
        }
        *self.known.borrow_mut() = tags.to_vec();
        self.rebuild(tags);
    }

    fn rebuild(self: &Rc<Self>, tags: &[String]) {
        for c in self.capsules.borrow_mut().drain(..) {
            unsafe { c.button.setTarget(None) };
            c.button.removeFromSuperview();
        }
        let mut caps = Vec::new();
        for tag in tags {
            let weak = Rc::downgrade(self);
            let name = tag.clone();
            let target = ActionTarget::new(self.mtm, move || {
                if let Some(t) = weak.upgrade() {
                    let mut now = t.known.borrow().clone();
                    now.retain(|x| *x != name);
                    t.send(now.clone());
                    t.rebuild(&now);
                }
            });
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(tag),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    self.mtm,
                )
            };
            b.setBordered(false);
            b.setFont(Some(&NSFont::systemFontOfSize_weight(11.5, unsafe {
                NSFontWeightMedium
            })));
            b.setToolTip(Some(ns_string!("Убрать тег")));
            b.setWantsLayer(true);
            if let Some(layer) = b.layer() {
                layer.setCornerRadius(CAPSULE_H / 2.0);
                let hue = style::tag_hue(tag);
                layer.setBackgroundColor(Some(
                    &style::swatch(hue).colorWithAlphaComponent(0.28).CGColor(),
                ));
            }
            b.setContentTintColor(Some(&NSColor::labelColor()));
            self.view.addSubview(&b);
            caps.push(Capsule {
                button: b,
                _target: Retained::into_super(Retained::into_super(target)),
            });
        }
        *self.capsules.borrow_mut() = caps;
        self.layout(self.laid.get().0);
        if let Some(f) = self.on_resize.borrow().as_ref() {
            f();
        }
    }

    /// Lays the capsules and the field out in rows `width` wide; returns the height.
    pub fn layout(&self, width: f64) -> f64 {
        let inner = (width - 2.0 * PAD).max(40.0);
        let (mut x, mut y) = (0.0f64, 0.0f64);
        for c in self.capsules.borrow().iter() {
            let tw = c.button.fittingSize().width.ceil();
            let w = (tw - 8.0).max(0.0) + 2.0 * CAPSULE_PAD;
            if x > 0.0 && x + w > inner {
                x = 0.0;
                y += CAPSULE_H + ROW_GAP;
            }
            c.button.setFrame(NSRect::new(
                NSPoint::new(PAD + x, y),
                NSSize::new(w, CAPSULE_H),
            ));
            x += w + GAP;
        }
        let fw = (inner - x).max(0.0);
        if x > 0.0 && fw < FIELD_MIN_W {
            x = 0.0;
            y += CAPSULE_H + ROW_GAP;
        }
        self.field.setFrame(NSRect::new(
            NSPoint::new(PAD + x, y + 1.0),
            NSSize::new((inner - x).max(FIELD_MIN_W.min(inner)), CAPSULE_H - 2.0),
        ));
        let h = y + CAPSULE_H;
        self.laid.set((width, h));
        h
    }
}
