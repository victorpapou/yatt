//! The pomodoro's lengths, in a native popover off the 🍅 button (hold it):
//! work and break minutes with steppers, and three presets.

use std::cell::Cell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSButtonType, NSControlStateValueOff, NSControlStateValueOn, NSFont,
    NSFontWeightRegular, NSPopover, NSPopoverBehavior, NSStepper, NSTextField, NSView,
    NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};

use crate::classes::{ActionTarget, FlippedView};
use crate::model::{TimerSnap, UiEvent};
use crate::window::Sink;

/// Work / break minutes one click away.
pub const PRESETS: [(u32, u32); 3] = [(25, 5), (40, 10), (50, 15)];

/// All the pomodoro's settings at once: work, break, long break, every N,
/// auto-start breaks, auto-start work.
type Prefs = (u32, u32, u32, u32, bool, bool);

pub struct Durations {
    popover: Retained<NSPopover>,
    rows: Vec<(Retained<NSStepper>, Retained<NSTextField>)>,
    checks: Vec<Retained<NSButton>>,
    now: Rc<Cell<Prefs>>,
    _retained: Vec<Retained<AnyObject>>,
}

fn minutes(n: u32) -> Retained<NSString> {
    NSString::from_str(&format!("{n} мин"))
}

impl Durations {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Self {
        let (w, h) = (236.0, 236.0);
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
        let now: Rc<Cell<Prefs>> = Rc::new(Cell::new((25, 5, 15, 4, true, false)));
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let send = {
            let sink = sink.clone();
            let now = now.clone();
            move |p: Prefs| {
                let old = now.get();
                now.set(p);
                if (p.0, p.1) != (old.0, old.1) {
                    sink(UiEvent::SetDurations {
                        work_min: p.0,
                        break_min: p.1,
                    });
                }
                if (p.2, p.3, p.4, p.5) != (old.2, old.3, old.4, old.5) {
                    sink(UiEvent::SetCycle {
                        long_min: p.2,
                        every: p.3,
                        auto_breaks: p.4,
                        auto_work: p.5,
                    });
                }
            }
        };
        let row = |y: f64, title: &str, min: f64, max: f64, step: f64| {
            let name = NSTextField::labelWithString(&NSString::from_str(title), mtm);
            name.setFrame(NSRect::new(
                NSPoint::new(12.0, y + 3.0),
                NSSize::new(96.0, 18.0),
            ));
            let value = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            value.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                13.0,
                unsafe { NSFontWeightRegular },
            )));
            value.setFrame(NSRect::new(
                NSPoint::new(112.0, y + 3.0),
                NSSize::new(70.0, 18.0),
            ));
            let stepper = NSStepper::new(mtm);
            stepper.setMinValue(min);
            stepper.setMaxValue(max);
            stepper.setIncrement(step);
            stepper.setFrame(NSRect::new(
                NSPoint::new(w - 32.0, y),
                NSSize::new(19.0, 24.0),
            ));
            view.addSubview(&name);
            view.addSubview(&value);
            view.addSubview(&stepper);
            (stepper, value)
        };
        let rows = vec![
            row(12.0, "Работа", 1.0, 240.0, 5.0),
            row(42.0, "Перерыв", 1.0, 120.0, 5.0),
            row(72.0, "Длинный перерыв", 1.0, 120.0, 5.0),
            row(102.0, "Длинный каждые", 2.0, 12.0, 1.0),
        ];
        for (i, (stepper, _)) in rows.iter().enumerate() {
            let (send, field) = (send.clone(), stepper.clone());
            let now = now.clone();
            let t = ActionTarget::new(mtm, move || {
                let v = field.intValue().max(1) as u32;
                let mut p = now.get();
                match i {
                    0 => p.0 = v,
                    1 => p.1 = v,
                    2 => p.2 = v,
                    _ => p.3 = v,
                }
                send(p);
            });
            unsafe {
                stepper.setTarget(Some(&t));
                stepper.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(t)));
        }
        let mut checks = Vec::new();
        for (i, title) in ["Перерыв стартует сам", "Работа стартует сама"]
            .into_iter()
            .enumerate()
        {
            let (send, now) = (send.clone(), now.clone());
            let cell: Rc<std::cell::RefCell<Option<Retained<NSButton>>>> = Rc::default();
            let c2 = cell.clone();
            let t = ActionTarget::new(mtm, move || {
                let on = c2
                    .borrow()
                    .as_ref()
                    .is_some_and(|b| b.state() == NSControlStateValueOn);
                let mut p = now.get();
                if i == 0 {
                    p.4 = on;
                } else {
                    p.5 = on;
                }
                send(p);
            });
            let b = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&t),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setButtonType(NSButtonType::Switch);
            b.setFrame(NSRect::new(
                NSPoint::new(12.0, 136.0 + i as f64 * 26.0),
                NSSize::new(w - 24.0, 20.0),
            ));
            *cell.borrow_mut() = Some(b.clone());
            view.addSubview(&b);
            checks.push(b);
            retained.push(Retained::into_super(Retained::into_super(t)));
        }
        let bw = (w - 24.0 - 2.0 * 6.0) / 3.0;
        for (i, preset) in PRESETS.into_iter().enumerate() {
            let (send, now) = (send.clone(), now.clone());
            let t = ActionTarget::new(mtm, move || {
                let mut p = now.get();
                p.0 = preset.0;
                p.1 = preset.1;
                send(p);
            });
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&format!("{}/{}", preset.0, preset.1)),
                    Some(&t),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setFrame(NSRect::new(
                NSPoint::new(12.0 + i as f64 * (bw + 6.0), 196.0),
                NSSize::new(bw, 28.0),
            ));
            view.addSubview(&b);
            retained.push(Retained::into_super(Retained::into_super(t)));
        }
        let controller = NSViewController::new(mtm);
        controller.setView(&view);
        let popover = NSPopover::new(mtm);
        popover.setContentViewController(Some(&controller));
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(NSSize::new(w, h));
        Self {
            popover,
            rows,
            checks,
            now,
            _retained: retained,
        }
    }

    /// The settings now in force (from the snapshot).
    pub fn show_values(&self, t: &TimerSnap) {
        self.now.set((
            t.work_min,
            t.break_min,
            t.long_min,
            t.every,
            t.auto_breaks,
            t.auto_work,
        ));
        for (i, v) in [t.work_min, t.break_min, t.long_min, t.every]
            .into_iter()
            .enumerate()
        {
            self.rows[i].0.setIntValue(v as i32);
            let text = if i == 3 {
                NSString::from_str(&format!("{v}-й"))
            } else {
                minutes(v)
            };
            self.rows[i].1.setStringValue(&text);
        }
        for (b, on) in self.checks.iter().zip([t.auto_breaks, t.auto_work]) {
            b.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    pub fn open(&self, anchor: &NSView) {
        self.popover.showRelativeToRect_ofView_preferredEdge(
            anchor.bounds(),
            anchor,
            NSRectEdge::MaxY,
        );
    }
}
