//! The pomodoro's lengths, in a native popover off the 🍅 button (hold it):
//! work and break minutes with steppers, and three presets.

use std::cell::Cell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSFont, NSFontWeightRegular, NSPopover, NSPopoverBehavior, NSStepper, NSTextField,
    NSView, NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};

use crate::classes::{ActionTarget, FlippedView};
use crate::model::UiEvent;
use crate::window::Sink;

/// Work / break minutes one click away.
pub const PRESETS: [(u32, u32); 3] = [(25, 5), (40, 10), (50, 15)];

pub struct Durations {
    popover: Retained<NSPopover>,
    work: (Retained<NSStepper>, Retained<NSTextField>),
    rest: (Retained<NSStepper>, Retained<NSTextField>),
    now: Rc<Cell<(u32, u32)>>,
    _retained: Vec<Retained<AnyObject>>,
}

fn minutes(n: u32) -> Retained<NSString> {
    NSString::from_str(&format!("{n} мин"))
}

impl Durations {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Self {
        let (w, h) = (236.0, 128.0);
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
        let now: Rc<Cell<(u32, u32)>> = Rc::new(Cell::new((25, 5)));
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let send = {
            let sink = sink.clone();
            let now = now.clone();
            move |(work_min, break_min): (u32, u32)| {
                now.set((work_min, break_min));
                sink(UiEvent::SetDurations {
                    work_min,
                    break_min,
                });
            }
        };
        let row = |y: f64, title: &str, max: f64| {
            let name = NSTextField::labelWithString(&NSString::from_str(title), mtm);
            name.setFrame(NSRect::new(
                NSPoint::new(12.0, y + 3.0),
                NSSize::new(80.0, 18.0),
            ));
            let value = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            value.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                13.0,
                unsafe { NSFontWeightRegular },
            )));
            value.setFrame(NSRect::new(
                NSPoint::new(100.0, y + 3.0),
                NSSize::new(80.0, 18.0),
            ));
            let stepper = NSStepper::new(mtm);
            stepper.setMinValue(1.0);
            stepper.setMaxValue(max);
            stepper.setIncrement(5.0);
            stepper.setFrame(NSRect::new(
                NSPoint::new(w - 32.0, y),
                NSSize::new(19.0, 24.0),
            ));
            view.addSubview(&name);
            view.addSubview(&value);
            view.addSubview(&stepper);
            (stepper, value)
        };
        let work = row(12.0, "Работа", 240.0);
        let rest = row(42.0, "Перерыв", 120.0);
        for (i, (stepper, _)) in [&work, &rest].into_iter().enumerate() {
            let (send, field) = (send.clone(), stepper.clone());
            let now = now.clone();
            let t = ActionTarget::new(mtm, move || {
                let v = field.intValue().max(1) as u32;
                let (a, b) = now.get();
                send(if i == 0 { (v, b) } else { (a, v) });
            });
            unsafe {
                stepper.setTarget(Some(&t));
                stepper.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(t)));
        }
        let bw = (w - 24.0 - 2.0 * 6.0) / 3.0;
        for (i, preset) in PRESETS.into_iter().enumerate() {
            let send = send.clone();
            let t = ActionTarget::new(mtm, move || send(preset));
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&format!("{}/{}", preset.0, preset.1)),
                    Some(&t),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setFrame(NSRect::new(
                NSPoint::new(12.0 + i as f64 * (bw + 6.0), 80.0),
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
            work,
            rest,
            now,
            _retained: retained,
        }
    }

    /// The lengths now set (from the snapshot).
    pub fn show_values(&self, work_min: u32, break_min: u32) {
        self.now.set((work_min, break_min));
        self.work.0.setIntValue(work_min as i32);
        self.work.1.setStringValue(&minutes(work_min));
        self.rest.0.setIntValue(break_min as i32);
        self.rest.1.setStringValue(&minutes(break_min));
    }

    pub fn open(&self, anchor: &NSView) {
        self.popover.showRelativeToRect_ofView_preferredEdge(
            anchor.bounds(),
            anchor,
            NSRectEdge::MaxY,
        );
    }
}
