//! The task's look and status, in a native popover off the panel's header: the
//! card colours, the status, the icons, and delete. Small, and every row grows
//! with its list (more colours or icons are more cells in the same grid).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSImage, NSPopover, NSPopoverBehavior, NSSearchField, NSSegmentedControl,
    NSTextField, NSView, NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};
use yatt_core::{Status, TaskId};

use crate::classes::{ActionTarget, FlippedView};
use crate::model::UiEvent;
use crate::style;
use crate::window::Sink;

const CELL: f64 = 32.0;
const PER_ROW: usize = 8;
const SWATCH_CELL: f64 = 28.0;
const SWATCH_PER_ROW: usize = 10;
const PAD: f64 = 15.0;
const STATUS_Y: f64 = PAD + SWATCH_CELL * 2.0 + 18.0;
const SEARCH_Y: f64 = STATUS_Y + 42.0;
const SYMBOLS_Y: f64 = SEARCH_Y + 48.0;
const ICONS_Y: f64 = SYMBOLS_Y + 22.0;
const HUE_SWATCH_ORDER: [usize; style::HUE_COUNT] =
    [6, 7, 8, 3, 4, 10, 11, 0, 13, 2, 14, 1, 5, 9, 12];

fn icon_at(slot: usize, y0: f64) -> NSPoint {
    let inner_width = PAD * 2.0 + SWATCH_CELL * SWATCH_PER_ROW as f64 - 2.0 * PAD;
    let step = (inner_width - CELL) / (PER_ROW - 1) as f64;
    NSPoint::new(
        PAD + (slot % PER_ROW) as f64 * step,
        y0 + (slot / PER_ROW) as f64 * CELL,
    )
}

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
    icon_names: Vec<&'static str>,
    status: Retained<NSSegmentedControl>,
    search: Retained<NSSearchField>,
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
        let width = PAD * 2.0 + SWATCH_CELL * SWATCH_PER_ROW as f64;
        let rows = |n: usize| n.div_ceil(PER_ROW) as f64 * CELL;
        // Match the native symbol chooser's compact 310×468 pt footprint.
        let height = ICONS_Y + rows(style::ICONS.len()) + 75.0;
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
        for (slot, hue) in HUE_SWATCH_ORDER.iter().copied().enumerate() {
            let t = send(Rc::new(move |c: &Current| {
                Some(UiEvent::SetLook {
                    id: c.id?,
                    color: Some(hue as u8),
                    icon: c.icon.clone(),
                })
            }));
            let b = cell_button(
                mtm,
                at(
                    slot % SWATCH_PER_ROW,
                    PAD + (slot / SWATCH_PER_ROW) as f64 * SWATCH_CELL,
                ),
                &t,
                symbol("circle.fill").as_deref(),
            );
            b.setFrame(NSRect::new(
                NSPoint::new(
                    PAD + (slot % SWATCH_PER_ROW) as f64 * SWATCH_CELL,
                    PAD + (slot / SWATCH_PER_ROW) as f64 * SWATCH_CELL,
                ),
                NSSize::new(SWATCH_CELL, SWATCH_CELL),
            ));
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
            NSPoint::new(PAD, STATUS_Y),
            NSSize::new(width - 2.0 * PAD, 28.0),
        ));
        view.addSubview(&status);
        retained.push(Retained::into_super(Retained::into_super(status_target.0)));

        let symbols_label = NSTextField::labelWithString(&NSString::from_str("SYMBOLS"), mtm);
        symbols_label.setFrame(NSRect::new(
            NSPoint::new(PAD + 6.0, SYMBOLS_Y),
            NSSize::new(width - 2.0 * PAD, 18.0),
        ));
        symbols_label.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        view.addSubview(&symbols_label);

        let mut icons = Vec::new();
        let mut icon_names = Vec::new();
        for name in style::ICONS.iter() {
            let name = *name;
            let Some(image) = symbol(name) else {
                continue;
            };
            let slot = icons.len();
            let t = send(Rc::new(move |c: &Current| {
                Some(UiEvent::SetLook {
                    id: c.id?,
                    color: c.color,
                    icon: Some(name.to_string()),
                })
            }));
            let b = cell_button(mtm, icon_at(slot, ICONS_Y), &t, Some(&image));
            b.setToolTip(Some(&NSString::from_str(name)));
            view.addSubview(&b);
            icons.push(b);
            icon_names.push(name);
            retained.push(Retained::into_super(Retained::into_super(t)));
        }

        let search = NSSearchField::new(mtm);
        search.setFrame(NSRect::new(
            NSPoint::new(PAD, SEARCH_Y),
            NSSize::new(width - 2.0 * PAD, 34.0),
        ));
        search.setPlaceholderString(Some(&NSString::from_str("Search Symbols")));
        search.setSendsSearchStringImmediately(true);
        view.addSubview(&search);
        let search_target = {
            let search = search.clone();
            let icons = icons.clone();
            let names = icon_names.clone();
            ActionTarget::new(mtm, move || {
                let query = search.stringValue().to_string().to_lowercase();
                let mut visible = 0;
                for (button, name) in icons.iter().zip(&names) {
                    let matches = query.is_empty() || name.to_lowercase().contains(&query);
                    button.setHidden(!matches);
                    if matches {
                        button.setFrame(NSRect::new(
                            icon_at(visible, ICONS_Y),
                            NSSize::new(CELL, CELL),
                        ));
                        visible += 1;
                    }
                }
            })
        };
        unsafe {
            let target: &AnyObject = &search_target;
            search.setTarget(Some(target));
            search.setAction(Some(ActionTarget::sel()));
        }
        retained.push(Retained::into_super(Retained::into_super(search_target)));

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
            NSPoint::new(PAD, height - 42.0),
            NSSize::new(width - 2.0 * PAD, 30.0),
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
            icon_names,
            status,
            search,
            current,
            _retained: retained,
        }
    }

    /// The open task changed (or its look did): marks its colour, icon and status.
    pub fn show_task(&self, c: Current) {
        let hue = c.id.map(style::hue_of);
        let glyph = c.id.map(style::glyph_of);
        for (slot, b) in self.swatches.iter().enumerate() {
            let color = HUE_SWATCH_ORDER[slot];
            let on = hue == Some(color);
            b.setContentTintColor(Some(&style::swatch(color)));
            b.setImage(
                symbol(if on {
                    "checkmark.circle.fill"
                } else {
                    "circle.fill"
                })
                .as_deref(),
            );
        }
        for (i, (b, name)) in self.icons.iter().zip(&self.icon_names).enumerate() {
            let tint = if glyph == Some(*name) {
                objc2_app_kit::NSColor::controlAccentColor()
            } else {
                objc2_app_kit::NSColor::secondaryLabelColor()
            };
            b.setContentTintColor(Some(&tint));
            b.setFrame(NSRect::new(icon_at(i, ICONS_Y), NSSize::new(CELL, CELL)));
        }
        self.search.setStringValue(&NSString::from_str(""));
        for b in &self.icons {
            b.setHidden(false);
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
