//! The task's look and status, in a native popover off the panel's header: the
//! card colours, the status, the icons, and delete. Small, and every row grows
//! with its list (more colours or icons are more cells in the same grid).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSImage, NSPopover, NSPopoverBehavior, NSSegmentedControl, NSView, NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};
use yatt_core::{Status, TaskId};

use crate::classes::{ActionTarget, FlippedView};
use crate::model::UiEvent;
use crate::style;
use crate::window::Sink;

const CELL: f64 = 34.0;
const PER_ROW: usize = 6;
const PAD: f64 = 12.0;

/// What the open task looks like now: its colour and icon as chosen (`None`:
/// from its id) and its status.
#[derive(Default, Clone)]
pub struct Current {
    pub id: Option<TaskId>,
    pub color: Option<u8>,
    pub icon: Option<String>,
    pub status: Option<Status>,
}

/// What a cell sends for the open task.
type EventFn = dyn Fn(&Current) -> Option<UiEvent>;

pub struct Picker {
    popover: Retained<NSPopover>,
    swatches: Vec<Retained<NSButton>>,
    icons: Vec<Retained<NSButton>>,
    status: Retained<NSSegmentedControl>,
    current: Rc<RefCell<Current>>,
    _retained: Vec<Retained<AnyObject>>,
}

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
}

fn cell_button(
    mtm: MainThreadMarker,
    at: NSPoint,
    target: &ActionTarget,
    image: Option<&NSImage>,
) -> Retained<NSButton> {
    let b = unsafe {
        NSButton::buttonWithImage_target_action(
            image.expect("symbol"),
            Some(target),
            Some(ActionTarget::sel()),
            mtm,
        )
    };
    b.setBordered(false);
    b.setFrame(NSRect::new(at, NSSize::new(CELL, CELL)));
    b
}

impl Picker {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Self {
        let current: Rc<RefCell<Current>> = Rc::default();
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let width = PAD * 2.0 + CELL * PER_ROW as f64;
        let rows = |n: usize| n.div_ceil(PER_ROW) as f64 * CELL;
        let icons_y = PAD + rows(style::HUE_COUNT) + 44.0;
        let height = icons_y + rows(style::ICONS.len()) + 44.0;
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(width, height)));
        let at = |i: usize, y0: f64| {
            NSPoint::new(
                PAD + (i % PER_ROW) as f64 * CELL,
                y0 + (i / PER_ROW) as f64 * CELL,
            )
        };
        let send = |f: Rc<EventFn>| {
            let (sink, current) = (sink.clone(), current.clone());
            ActionTarget::new(mtm, move || {
                let cur = current.borrow().clone();
                if let Some(e) = f(&cur) {
                    sink(e);
                }
            })
        };

        let mut swatches = Vec::new();
        for hue in 0..style::HUE_COUNT {
            let t = send(Rc::new(move |c: &Current| {
                Some(UiEvent::SetLook {
                    id: c.id?,
                    color: Some(hue as u8),
                    icon: c.icon.clone(),
                })
            }));
            let b = cell_button(mtm, at(hue, PAD), &t, symbol("circle.fill").as_deref());
            b.setContentTintColor(Some(&style::swatch(hue)));
            b.setToolTip(Some(&NSString::from_str("Цвет")));
            view.addSubview(&b);
            swatches.push(b);
            retained.push(Retained::into_super(Retained::into_super(t)));
        }

        // The status, as the three columns.
        let labels = Status::ALL.map(|s| NSString::from_str(crate::model::status_label(s)));
        let refs: Vec<&NSString> = labels.iter().map(|l| &**l).collect();
        let status_target = {
            let (sink, current) = (sink.clone(), current.clone());
            let control: Rc<Cell<Option<Retained<NSSegmentedControl>>>> = Rc::default();
            let slot = control.clone();
            let t = ActionTarget::new(mtm, move || {
                let (Some(c), Some(id)) = (slot.take(), current.borrow().id) else {
                    return;
                };
                let i = c.selectedSegment();
                slot.set(Some(c));
                if let Some(&status) = Status::ALL.get(i as usize) {
                    current.borrow_mut().status = Some(status);
                    sink(UiEvent::Move {
                        id,
                        status,
                        index: 0,
                    });
                }
            });
            (t, control)
        };
        let status = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &objc2_foundation::NSArray::from_slice(&refs),
                objc2_app_kit::NSSegmentSwitchTracking::SelectOne,
                Some(&status_target.0),
                Some(ActionTarget::sel()),
                mtm,
            )
        };
        status_target.1.set(Some(status.clone()));
        status.setFrame(NSRect::new(
            NSPoint::new(PAD, PAD + rows(style::HUE_COUNT) + 10.0),
            NSSize::new(width - 2.0 * PAD, 24.0),
        ));
        view.addSubview(&status);
        retained.push(Retained::into_super(Retained::into_super(status_target.0)));

        let mut icons = Vec::new();
        for (i, name) in style::ICONS.iter().enumerate() {
            let name = *name;
            let t = send(Rc::new(move |c: &Current| {
                Some(UiEvent::SetLook {
                    id: c.id?,
                    color: c.color,
                    icon: Some(name.to_string()),
                })
            }));
            let b = cell_button(mtm, at(i, icons_y), &t, symbol(name).as_deref());
            view.addSubview(&b);
            icons.push(b);
            retained.push(Retained::into_super(Retained::into_super(t)));
        }

        let popover = NSPopover::new(mtm);
        let delete = {
            let popover = popover.clone();
            let (sink, current) = (sink.clone(), current.clone());
            ActionTarget::new(mtm, move || {
                popover.close();
                if let Some(id) = current.borrow().id {
                    sink(UiEvent::Delete(id));
                }
            })
        };
        let del = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Удалить задачу"),
                Some(&delete),
                Some(ActionTarget::sel()),
                mtm,
            )
        };
        del.setBordered(false);
        del.setContentTintColor(Some(&objc2_app_kit::NSColor::systemRedColor()));
        del.setFrame(NSRect::new(
            NSPoint::new(PAD, height - 36.0),
            NSSize::new(width - 2.0 * PAD, 24.0),
        ));
        view.addSubview(&del);
        retained.push(Retained::into_super(Retained::into_super(delete)));

        let controller = NSViewController::new(mtm);
        controller.setView(&view);
        popover.setContentViewController(Some(&controller));
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(width, height));
        Self {
            popover,
            swatches,
            icons,
            status,
            current,
            _retained: retained,
        }
    }

    /// The open task changed (or its look did): marks its colour, icon and status.
    pub fn show_task(&self, c: Current) {
        let hue = c.id.map(style::hue_of);
        let glyph = c.id.map(style::glyph_of);
        for (i, b) in self.swatches.iter().enumerate() {
            let on = hue == Some(i);
            b.setImage(
                symbol(if on {
                    "checkmark.circle.fill"
                } else {
                    "circle.fill"
                })
                .as_deref(),
            );
        }
        for (b, name) in self.icons.iter().zip(style::ICONS) {
            let tint = if glyph == Some(name) {
                objc2_app_kit::NSColor::controlAccentColor()
            } else {
                objc2_app_kit::NSColor::secondaryLabelColor()
            };
            b.setContentTintColor(Some(&tint));
        }
        if let Some(i) = c
            .status
            .and_then(|s| Status::ALL.iter().position(|x| *x == s))
        {
            self.status.setSelectedSegment(i as isize);
        }
        *self.current.borrow_mut() = c;
    }

    /// Opens below `anchor`, as a native popover.
    pub fn open(&self, anchor: &NSView) {
        if self.popover.isShown() {
            self.popover.close();
            return;
        }
        self.popover.showRelativeToRect_ofView_preferredEdge(
            anchor.bounds(),
            anchor,
            NSRectEdge::MaxY,
        );
    }

    pub fn close(&self) {
        if self.popover.isShown() {
            self.popover.close();
        }
    }
}
