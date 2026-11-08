//! The time editor: a popover listing every tracked interval of a task, each with
//! its start and end in native date pickers, a button to delete it, and one to
//! add an interval by hand. Every change goes out as a UI event (so it is a step
//! of undo, and saved).

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSFont,
    NSPopover, NSPopoverBehavior, NSScrollView, NSTextField, NSView,
    NSViewController,
};
use objc2_foundation::{ns_string, NSDate, NSRectEdge, NSPoint, NSRect, NSSize, NSString};

use crate::classes::{ActionTarget, FlippedView};
use crate::model::{LogEntry, UiEvent};
use crate::window::Sink;
use yatt_core::TaskId;

const W: f64 = 440.0;
const ROW_H: f64 = 30.0;
const HEAD_H: f64 = 40.0;
const MAX_ROWS: usize = 9;

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
    state: Rc<RefCell<State>>,
}

fn date(ms: i64) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(ms as f64 / 1000.0)
}

fn ms_of(d: &NSDate) -> i64 {
    (d.timeIntervalSince1970() * 1000.0).round() as i64
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn label(mtm: MainThreadMarker, text: &str, secondary: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(12.0, unsafe {
        objc2_app_kit::NSFontWeightRegular
    })));
    if secondary {
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    l
}

fn picker(mtm: MainThreadMarker, ms: i64, enabled: bool) -> Retained<NSDatePicker> {
    let p = NSDatePicker::new(mtm);
    p.setDatePickerStyle(NSDatePickerStyle::TextField);
    p.setDatePickerElements(
        NSDatePickerElementFlags::YearMonthDay | NSDatePickerElementFlags::HourMinuteSecond,
    );
    p.setDateValue(&date(ms));
    p.setControlSize(objc2_app_kit::NSControlSize::Small);
    p.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(12.0, unsafe {
        objc2_app_kit::NSFontWeightRegular
    })));
    p.setEnabled(enabled);
    p
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
        Rc::new(Self {
            mtm,
            sink: sink.clone(),
            popover,
            state: Rc::new(RefCell::new(State {
                task: None,
                shown: Vec::new(),
                targets: Vec::new(),
            })),
        })
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

    /// The log changed under an open editor: the rows follow.
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

    fn build(self: &Rc<Self>, task: TaskId, log: &[LogEntry]) {
        let mtm = self.mtm;
        let rows = log.len().min(MAX_ROWS).max(1);
        let list_h = rows as f64 * ROW_H + 8.0;
        let h = HEAD_H + list_h;
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(W, h)),
        );
        let mut targets: Vec<Retained<ActionTarget>> = Vec::new();

        // Header: the total, and the button that adds an interval.
        let total: i64 = log
            .iter()
            .filter_map(|e| e.end_ms.map(|end| end - e.start_ms))
            .sum();
        let head = label(mtm, &format!("Всего  {}", duration(total)), false);
        head.setFrame(NSRect::new(NSPoint::new(12.0, h - HEAD_H + 11.0), NSSize::new(200.0, 18.0)));
        content.addSubview(&head);
        {
            let sink = self.sink.clone();
            let t = ActionTarget::new(mtm, move || {
                let end = now_ms();
                sink(UiEvent::AddEntry { task, start_ms: end - 30 * 60_000, end_ms: end });
            });
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    ns_string!("Добавить запись"),
                    Some(&t),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setControlSize(objc2_app_kit::NSControlSize::Small);
            b.setFrame(NSRect::new(
                NSPoint::new(W - 12.0 - 140.0, h - HEAD_H + 7.0),
                NSSize::new(140.0, 24.0),
            ));
            content.addSubview(&b);
            targets.push(t);
        }

        // The rows, newest first, scrolling past nine.
        let doc = FlippedView::new(
            mtm,
            NSRect::new(NSPoint::ZERO, NSSize::new(W, log.len() as f64 * ROW_H + 8.0)),
        );
        if log.is_empty() {
            let l = label(mtm, "Записей времени пока нет", true);
            l.setFrame(NSRect::new(NSPoint::new(12.0, 6.0), NSSize::new(W - 24.0, 18.0)));
            doc.addSubview(&l);
        }
        for (i, e) in log.iter().enumerate() {
            let y = 4.0 + i as f64 * ROW_H;
            let id = e.id.clone();
            let finished = e.end_ms.is_some();
            let ps = picker(mtm, e.start_ms, finished);
            ps.setFrame(NSRect::new(NSPoint::new(12.0, y + 2.0), NSSize::new(160.0, 24.0)));
            let pe = picker(mtm, e.end_ms.unwrap_or_else(now_ms), finished);
            pe.setFrame(NSRect::new(NSPoint::new(184.0, y + 2.0), NSSize::new(160.0, 24.0)));
            if finished {
                // Either picker: both values go out together.
                let (ws, we) = (
                    objc2::rc::Weak::from_retained(&ps),
                    objc2::rc::Weak::from_retained(&pe),
                );
                let sink = self.sink.clone();
                let tid = id.clone();
                let send = Rc::new(move || {
                    if let (Some(a), Some(b)) = (ws.load(), we.load()) {
                        sink(UiEvent::EditEntry {
                            id: tid.clone(),
                            start_ms: ms_of(&a.dateValue()),
                            end_ms: ms_of(&b.dateValue()),
                        });
                    }
                });
                for p in [&ps, &pe] {
                    let send = send.clone();
                    let t = ActionTarget::new(mtm, move || send());
                    unsafe {
                        p.setTarget(Some(&*(Retained::as_ptr(&t) as *const AnyObject)));
                        p.setAction(Some(ActionTarget::sel()));
                    }
                    targets.push(t);
                }
            }
            doc.addSubview(&ps);
            doc.addSubview(&pe);
            let dur = label(
                mtm,
                &match e.end_ms {
                    Some(end) => duration(end - e.start_ms),
                    None => "идёт".to_string(),
                },
                true,
            );
            dur.setFrame(NSRect::new(NSPoint::new(352.0, y + 6.0), NSSize::new(50.0, 18.0)));
            doc.addSubview(&dur);
            if finished {
                let sink = self.sink.clone();
                let t = ActionTarget::new(mtm, move || sink(UiEvent::DeleteEntry(id.clone())));
                let b = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        ns_string!("✕"),
                        Some(&t),
                        Some(ActionTarget::sel()),
                        mtm,
                    )
                };
                b.setBordered(false);
                b.setToolTip(Some(ns_string!("Удалить запись")));
                b.setFrame(NSRect::new(NSPoint::new(W - 32.0, y + 3.0), NSSize::new(24.0, 24.0)));
                doc.addSubview(&b);
                targets.push(t);
            }
        }
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(W, list_h)),
        );
        scroll.setHasVerticalScroller(log.len() > MAX_ROWS);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&doc));
        content.addSubview(&scroll);

        let vc = NSViewController::new(mtm);
        vc.setView(&content);
        self.popover.setContentViewController(Some(&vc));
        self.popover.setContentSize(NSSize::new(W, h));
        let mut st = self.state.borrow_mut();
        st.task = Some(task);
        st.shown = log.to_vec();
        st.targets = targets;
    }
}
