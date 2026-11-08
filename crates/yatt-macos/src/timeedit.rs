//! The time editor: a popover listing every tracked interval of a task as a grid
//! (day, start, end, length, delete), with the total and a button that adds an
//! interval by hand. Every change goes out as a UI event, so it is a step of undo,
//! and saved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSControlSize, NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle,
    NSFont, NSGridCellPlacement, NSGridView, NSImage, NSImageSymbolConfiguration, NSPopover,
    NSPopoverBehavior, NSScrollView, NSTextField, NSView, NSViewController,
};
use objc2_foundation::{
    NSArray, NSCalendar, NSDate, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSTimer,
};

use crate::classes::ActionTarget;
use crate::model::{LogEntry, UiEvent};
use crate::window::Sink;
use yatt_core::TaskId;

const PAD: f64 = 12.0;
const HEAD_H: f64 = 28.0;
const GAP: f64 = 8.0;
const COL_GAP: f64 = 12.0;
const ROW_GAP: f64 = 6.0;
const MAX_ROWS: usize = 9;
/// The date pickers' own inner margin, so the rows start where the header does.
const PICKER_INSET: f64 = 8.5;

struct State {
    task: Option<TaskId>,
    shown: Vec<LogEntry>,
    /// Targets of the controls in the rows, kept alive with them.
    targets: Vec<Retained<ActionTarget>>,
}

pub struct TimeEditor {
    mtm: MainThreadMarker,
    sink: Sink,
    popover: Retained<NSPopover>,
    controller: Retained<NSViewController>,
    state: Rc<RefCell<State>>,
}

fn date(ms: i64) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(ms as f64 / 1000.0)
}

fn ms_of(d: &NSDate) -> i64 {
    (d.timeIntervalSince1970() * 1000.0).round() as i64
}

/// The start of the (local) day `ms` falls in.
fn day_start(ms: i64) -> i64 {
    ms_of(&NSCalendar::currentCalendar().startOfDayForDate(&date(ms)))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn mono() -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(13.0, unsafe { objc2_app_kit::NSFontWeightRegular })
}

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&mono()));
    l
}

fn picker(mtm: MainThreadMarker, ms: i64, elements: NSDatePickerElementFlags, enabled: bool) -> Retained<NSDatePicker> {
    let p = NSDatePicker::new(mtm);
    p.setDatePickerStyle(NSDatePickerStyle::TextField);
    p.setDatePickerElements(elements);
    p.setDateValue(&date(ms));
    p.setFont(Some(&mono()));
    p.setEnabled(enabled);
    p.setBezeled(false);
    p.setDrawsBackground(false);
    p
}

fn symbol_button(mtm: MainThreadMarker, name: &str, tip: &str, target: &ActionTarget) -> Retained<NSButton> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(tip)),
    )
    .and_then(|i| {
        let c = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
            13.0,
            unsafe { objc2_app_kit::NSFontWeightRegular },
            objc2_app_kit::NSImageSymbolScale::Medium,
        );
        i.imageWithSymbolConfiguration(&c)
    });
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(""),
            Some(target),
            Some(ActionTarget::sel()),
            mtm,
        )
    };
    b.setImage(image.as_deref());
    b.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
    b.setBordered(false);
    b.setToolTip(Some(&NSString::from_str(tip)));
    b.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    b
}

/// "1:23:45".
fn duration(ms: i64) -> String {
    crate::model::format_duration(ms.max(0))
}

impl TimeEditor {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Rc<Self> {
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(true);
        let controller = NSViewController::new(mtm);
        controller.setView(&NSView::new(mtm));
        popover.setContentViewController(Some(&controller));
        Rc::new(Self {
            mtm,
            sink: sink.clone(),
            popover,
            controller,
            state: Rc::new(RefCell::new(State {
                task: None,
                shown: Vec::new(),
                targets: Vec::new(),
            })),
        })
    }

    /// Development aid: the editor's content as a PNG.
    pub fn debug_png(self: &Rc<Self>, task: TaskId, log: &[LogEntry], path: &str) -> bool {
        self.build(task, log);
        let view = self.controller.view();
        let b = view.bounds();
        // Controls draw only in a window: a throwaway one holds the content.
        let win = unsafe {
            objc2_app_kit::NSWindow::initWithContentRect_styleMask_backing_defer(
                objc2_app_kit::NSWindow::alloc(self.mtm),
                b,
                objc2_app_kit::NSWindowStyleMask::Borderless,
                objc2_app_kit::NSBackingStoreType::Buffered,
                false,
            )
        };
        win.setContentView(Some(&view));
        view.layoutSubtreeIfNeeded();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(b) else { return false };
        view.cacheDisplayInRect_toBitmapImageRep(b, &rep);
        let props = objc2_foundation::NSDictionary::new();
        let Some(data) = (unsafe {
            rep.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, &props)
        }) else {
            return false;
        };
        data.writeToFile_atomically(&NSString::from_str(path), true)
    }

    /// Opens the editor under `anchor`.
    pub fn show(self: &Rc<Self>, anchor: &NSView, task: TaskId, log: &[LogEntry]) {
        self.build(task, log);
        self.popover.showRelativeToRect_ofView_preferredEdge(
            NSRect::new(NSPoint::ZERO, anchor.bounds().size),
            anchor,
            NSRectEdge::MaxY,
        );
    }

    /// The log changed under an open editor: the rows follow, the popover resizing
    /// with them.
    pub fn refresh(self: &Rc<Self>, task: TaskId, log: &[LogEntry]) {
        if !self.popover.isShown() {
            return;
        }
        {
            let st = self.state.borrow();
            if st.task == Some(task) && st.shown == log {
                return;
            }
        }
        self.build(task, log);
    }

    /// Builds the rows again a moment later (an edit that was refused snaps back).
    fn rebuild_soon(self: &Rc<Self>, task: TaskId) {
        let weak = Rc::downgrade(self);
        let block = block2::RcBlock::new(move |_t: std::ptr::NonNull<NSTimer>| {
            if let Some(me) = weak.upgrade() {
                let log = me.state.borrow().shown.clone();
                me.build(task, &log);
            }
        });
        unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.02, false, &block);
        }
    }

    fn build(self: &Rc<Self>, task: TaskId, log: &[LogEntry]) {
        let mtm = self.mtm;
        let mut targets: Vec<Retained<ActionTarget>> = Vec::new();

        // The grid: day, start, end, length, delete.
        let mut rows: Vec<Retained<NSArray<NSView>>> = Vec::new();
        for e in log {
            let id = e.id.clone();
            let finished = e.end_ms.is_some();
            let (start, end) = (Rc::new(Cell::new(e.start_ms)), Rc::new(Cell::new(e.end_ms.unwrap_or_else(now_ms))));
            let day = picker(mtm, e.start_ms, NSDatePickerElementFlags::YearMonthDay, finished);
            let ps = picker(mtm, e.start_ms, NSDatePickerElementFlags::HourMinute, finished);
            let pe = picker(mtm, end.get(), NSDatePickerElementFlags::HourMinute, finished);
            if finished {
                let send = {
                    let (sink, id, start, end) = (self.sink.clone(), id.clone(), start.clone(), end.clone());
                    let me = Rc::downgrade(self);
                    move || {
                        let refused = end.get() <= start.get();
                        sink(UiEvent::EditEntry { id: id.clone(), start_ms: start.get(), end_ms: end.get() });
                        if refused {
                            if let Some(me) = me.upgrade() {
                                me.rebuild_soon(task);
                            }
                        }
                    }
                };
                let send = Rc::new(send);
                let wire = |p: &NSDatePicker, f: Box<dyn Fn()>, targets: &mut Vec<Retained<ActionTarget>>| {
                    let t = ActionTarget::new(mtm, f);
                    unsafe {
                        p.setTarget(Some(&*(Retained::as_ptr(&t) as *const AnyObject)));
                        p.setAction(Some(ActionTarget::sel()));
                    }
                    targets.push(t);
                };
                // The day moves both ends together.
                {
                    let (d, start, end, send) = (objc2::rc::Weak::from_retained(&day), start.clone(), end.clone(), send.clone());
                    wire(&day, Box::new(move || {
                        if let Some(d) = d.load() {
                            let by = day_start(ms_of(&d.dateValue())) - day_start(start.get());
                            start.set(start.get() + by);
                            end.set(end.get() + by);
                            send();
                        }
                    }), &mut targets);
                }
                // A time of day, on the day the entry already has.
                for (p, cell, is_end) in [(&ps, &start, false), (&pe, &end, true)] {
                    let (w, cell, start, send) = (objc2::rc::Weak::from_retained(p), cell.clone(), start.clone(), send.clone());
                    wire(p, Box::new(move || {
                        if let Some(p) = w.load() {
                            let v = ms_of(&p.dateValue());
                            let tod = v - day_start(v);
                            let mut at = day_start(cell.get()) + tod;
                            if is_end && at <= start.get() {
                                at += 86_400_000;
                            }
                            cell.set(at);
                            send();
                        }
                    }), &mut targets);
                }
            }
            let len = label(
                mtm,
                &match e.end_ms {
                    Some(end) => duration(end - e.start_ms),
                    None => "идёт".to_string(),
                },
            );
            len.setTextColor(Some(&NSColor::secondaryLabelColor()));
            len.setAlignment(objc2_app_kit::NSTextAlignment::Right);
            let del: Retained<NSView> = if finished {
                let sink = self.sink.clone();
                let t = ActionTarget::new(mtm, move || sink(UiEvent::DeleteEntry(id.clone())));
                let b = symbol_button(mtm, "trash", "Удалить запись", &t);
                targets.push(t);
                Retained::into_super(Retained::into_super(b))
            } else {
                NSView::new(mtm)
            };
            let cells: [Retained<NSView>; 5] = [
                Retained::into_super(Retained::into_super(day)),
                Retained::into_super(Retained::into_super(ps)),
                Retained::into_super(Retained::into_super(pe)),
                Retained::into_super(Retained::into_super(len)),
                del,
            ];
            rows.push(NSArray::from_retained_slice(&cells));
        }
        let grid = (!rows.is_empty()).then(|| {
            let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
            grid.setColumnSpacing(COL_GAP);
            grid.setRowSpacing(ROW_GAP);
            grid.columnAtIndex(3).setXPlacement(NSGridCellPlacement::Trailing);
            grid.columnAtIndex(4).setXPlacement(NSGridCellPlacement::Trailing);
            grid.setRowAlignment(objc2_app_kit::NSGridRowAlignment::FirstBaseline);
            let size = grid.fittingSize();
            grid.setFrame(NSRect::new(NSPoint::ZERO, size));
            (grid, size)
        });
        let grid_w = grid.as_ref().map_or(0.0, |(_, s)| s.width);
        let list_w = if grid.is_some() { grid_w - PICKER_INSET } else { 240.0 };
        let w = list_w + 2.0 * PAD;
        let row_h = grid.as_ref().map_or(0.0, |(_, s)| s.height / log.len().max(1) as f64);
        let shown_rows = log.len().min(MAX_ROWS);
        let list_h = if log.is_empty() { 22.0 } else { (shown_rows as f64 * row_h).ceil() + 2.0 };
        let h = PAD + HEAD_H + GAP + list_h + PAD;

        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(w, h)),
        );
        // The header: the total on the left, the add button on the right, on the
        // same edges as the rows.
        let total: i64 = log
            .iter()
            .filter_map(|e| e.end_ms.map(|end| end - e.start_ms))
            .sum();
        let head = label(mtm, &format!("Всего  {}", duration(total)));
        head.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(13.0, unsafe {
            objc2_app_kit::NSFontWeightSemibold
        })));
        let hs = head.fittingSize();
        head.setFrame(NSRect::new(
            NSPoint::new(PAD, h - PAD - HEAD_H + (HEAD_H - hs.height) / 2.0),
            hs,
        ));
        content.addSubview(&head);
        {
            let sink = self.sink.clone();
            let t = ActionTarget::new(mtm, move || {
                let end = now_ms();
                sink(UiEvent::AddEntry { task, start_ms: end - 30 * 60_000, end_ms: end });
            });
            let b = symbol_button(mtm, "plus", "Добавить запись", &t);
            b.setFrame(NSRect::new(
                NSPoint::new(w - PAD - 22.0, h - PAD - HEAD_H),
                NSSize::new(22.0, HEAD_H),
            ));
            content.addSubview(&b);
            targets.push(t);
        }
        // The rows; a scroll view past nine of them.
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(NSPoint::new(PAD, PAD), NSSize::new(list_w, list_h)),
        );
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(log.len() > MAX_ROWS);
        scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        match &grid {
            Some((g, size)) => {
                // Flipped, so the rows run from the top.
                let doc = crate::classes::FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(list_w, size.height)));
                g.setFrame(NSRect::new(NSPoint::new(-PICKER_INSET, 0.0), *size));
                doc.addSubview(g);
                scroll.setDocumentView(Some(&doc));
            }
            None => {
                let l = label(mtm, "Записей времени пока нет");
                l.setTextColor(Some(&NSColor::secondaryLabelColor()));
                let doc = crate::classes::FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(list_w, list_h)));
                l.setFrame(NSRect::new(NSPoint::new(0.0, 2.0), NSSize::new(list_w, 18.0)));
                doc.addSubview(&l);
                scroll.setDocumentView(Some(&doc));
            }
        }
        content.addSubview(&scroll);

        // The popover follows the content's size, easing over (it animates).
        self.controller.setView(&content);
        self.popover.setContentSize(NSSize::new(w, h));
        let mut st = self.state.borrow_mut();
        st.task = Some(task);
        st.shown = log.to_vec();
        st.targets = targets;
    }
}
