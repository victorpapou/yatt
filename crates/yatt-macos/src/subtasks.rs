//! The inspector's subtasks: a checkbox and a title per row, and a last empty
//! row to add the next one. Edits are sent as the whole list.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFocusRingType, NSFont,
    NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};

use crate::classes::{ActionTarget, FlippedView};

pub const ROW_H: f64 = 28.0;
const PAD: f64 = 20.0;

struct Row {
    check: Retained<NSButton>,
    field: Retained<NSTextField>,
    /// Keeps the row's action targets alive.
    _targets: Vec<Retained<AnyObject>>,
}

pub struct Subtasks {
    pub view: Retained<FlippedView>,
    rows: Rc<RefCell<Vec<Row>>>,
    /// The list as last sent or received, to tell edits from echoes.
    known: Rc<RefCell<Vec<(String, bool)>>>,
    width: Cell<f64>,
    on_change: Rc<dyn Fn(Vec<(String, bool)>)>,
    /// Called when the number of rows changed (the pane lays itself out again).
    pub on_resize: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    mtm: MainThreadMarker,
}

impl Subtasks {
    pub fn new(mtm: MainThreadMarker, on_change: impl Fn(Vec<(String, bool)>) + 'static) -> Rc<Self> {
        let this = Rc::new(Self {
            view: FlippedView::new(mtm, NSRect::ZERO),
            rows: Rc::default(),
            known: Rc::default(),
            width: Cell::new(300.0),
            on_change: Rc::new(on_change),
            on_resize: Rc::default(),
            mtm,
        });
        this.rebuild(&[]);
        this
    }

    /// Height of the list with its add row.
    pub fn height(&self) -> f64 {
        self.rows.borrow().len() as f64 * ROW_H
    }

    pub fn layout(&self, width: f64) {
        self.width.set(width);
        for (i, r) in self.rows.borrow().iter().enumerate() {
            let y = i as f64 * ROW_H;
            r.check.setFrame(NSRect::new(
                NSPoint::new(PAD - 2.0, y + 4.0),
                NSSize::new(20.0, 20.0),
            ));
            r.field.setFrame(NSRect::new(
                NSPoint::new(PAD + 22.0, y + 4.0),
                NSSize::new((width - PAD * 2.0 - 22.0).max(0.0), 20.0),
            ));
        }
    }

    fn editing(&self) -> bool {
        self.rows.borrow().iter().any(|r| r.field.currentEditor().is_some())
    }

    /// Shows `items` (unless they are what is already shown, or one is being typed).
    pub fn set(self: &Rc<Self>, items: &[(String, bool)]) {
        if *self.known.borrow() == items {
            return;
        }
        if self.editing() {
            return;
        }
        self.rebuild(items);
    }

    /// The list as typed: rows with a title, the add row left out when empty.
    fn collect(&self) -> Vec<(String, bool)> {
        self.rows
            .borrow()
            .iter()
            .filter_map(|r| {
                let t = r.field.stringValue().to_string().trim().to_string();
                (!t.is_empty()).then(|| (t, r.check.state() == NSControlStateValueOn))
            })
            .collect()
    }

    fn commit(self: &Rc<Self>, focus_add_row: bool) {
        let now = self.collect();
        let changed = now != *self.known.borrow();
        if changed {
            *self.known.borrow_mut() = now.clone();
            (self.on_change)(now.clone());
        }
        // Rows whose title was cleared go; a filled add row gets a new one after it.
        let shape_changed = {
            let rows = self.rows.borrow();
            let filled = rows
                .iter()
                .filter(|r| !r.field.stringValue().to_string().trim().is_empty())
                .count();
            filled + 1 != rows.len() || rows.last().is_some_and(|r| !r.field.stringValue().to_string().is_empty())
        };
        if shape_changed {
            self.rebuild(&now);
            if focus_add_row
                && let Some(last) = self.rows.borrow().last()
                && let Some(window) = last.field.window()
            {
                window.makeFirstResponder(Some(&last.field));
            }
        }
    }

    fn rebuild(self: &Rc<Self>, items: &[(String, bool)]) {
        // Taken out first: removing a field that is being edited ends its editing,
        // and its action would look at the rows again.
        let old: Vec<Row> = std::mem::take(&mut *self.rows.borrow_mut());
        for r in &old {
            unsafe {
                r.field.setTarget(None);
                r.check.setTarget(None);
            }
        }
        for r in old {
            r.check.removeFromSuperview();
            r.field.removeFromSuperview();
        }
        *self.known.borrow_mut() = items.to_vec();
        let mut rows = Vec::new();
        for i in 0..=items.len() {
            let (title, done) = items.get(i).cloned().unwrap_or_default();
            let mut targets: Vec<Retained<AnyObject>> = Vec::new();
            let weak = Rc::downgrade(self);
            let t_check = ActionTarget::new(self.mtm, {
                let weak = weak.clone();
                move || {
                    if let Some(s) = weak.upgrade() {
                        s.commit(false);
                    }
                }
            });
            let check = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    ns_string!(""),
                    Some(&t_check),
                    Some(ActionTarget::sel()),
                    self.mtm,
                )
            };
            check.setState(if done { NSControlStateValueOn } else { NSControlStateValueOff });
            targets.push(Retained::into_super(Retained::into_super(t_check)));
            let field = NSTextField::textFieldWithString(&NSString::from_str(&title), self.mtm);
            field.setEditable(true);
            field.setSelectable(true);
            field.setBordered(false);
            field.setBezeled(false);
            field.setDrawsBackground(false);
            field.setFocusRingType(NSFocusRingType::None);
            field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
            field.setPlaceholderString(Some(ns_string!("Подзадача")));
            if done {
                field.setTextColor(Some(&NSColor::secondaryLabelColor()));
            }
            if let Some(cell) = field.cell() {
                cell.setSendsActionOnEndEditing(true);
            }
            let t_field = ActionTarget::new(self.mtm, move || {
                if let Some(s) = weak.upgrade() {
                    s.commit(true);
                }
            });
            unsafe {
                field.setTarget(Some(&t_field));
                field.setAction(Some(ActionTarget::sel()));
            }
            targets.push(Retained::into_super(Retained::into_super(t_field)));
            self.view.addSubview(&check);
            self.view.addSubview(&field);
            rows.push(Row {
                check,
                field,
                _targets: targets,
            });
        }
        *self.rows.borrow_mut() = rows;
        self.layout(self.width.get());
        if let Some(f) = self.on_resize.borrow().as_ref() {
            f();
        }
    }

    /// Sends what is typed in a row still being edited.
    pub fn flush(self: &Rc<Self>) {
        self.commit(false);
    }

    /// The view, for adding to the pane.
    pub fn as_view(&self) -> &NSView {
        &self.view
    }
}
