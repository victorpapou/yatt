//! The pomodoro's lengths, in the menu under the 🍅's chevron in the toolbar: the
//! usual work/break pairs, the lengths one by one, and «Настройки помидора…»
//! (the settings' own tab). The menu and the settings follow each other.

use std::cell::Cell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem};
use objc2_foundation::{NSString, ns_string};
use objc2::MainThreadOnly;

use crate::classes::ActionTarget;
use crate::model::{TimerSnap, UiEvent};
use crate::window::Sink;

/// Work / break minutes one click away.
pub const PRESETS: [(u32, u32); 3] = [(25, 5), (40, 10), (50, 15)];
const WORK: [u32; 7] = [15, 20, 25, 30, 45, 50, 60];
const BREAK: [u32; 4] = [3, 5, 10, 15];
const LONG: [u32; 4] = [10, 15, 20, 30];

/// Work, break, long break, every N, auto-start breaks, auto-start work.
type Prefs = (u32, u32, u32, u32, bool, bool);

pub struct Durations {
    now: Rc<Cell<Prefs>>,
    menu: Retained<NSMenu>,
    presets: Vec<(Retained<NSMenuItem>, (u32, u32))>,
    work: Vec<(Retained<NSMenuItem>, u32)>,
    breaks: Vec<(Retained<NSMenuItem>, u32)>,
    long: Vec<(Retained<NSMenuItem>, u32)>,
    _retained: Vec<Retained<AnyObject>>,
}

fn state(on: bool) -> isize {
    if on { NSControlStateValueOn } else { NSControlStateValueOff }
}

impl Durations {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Self {
        let menu = NSMenu::new(mtm);
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        // The lengths in force (work, break, long), to send the others unchanged.
        let now: Rc<Cell<Prefs>> = Rc::new(Cell::new((25, 5, 15, 4, true, false)));
        let item = |title: &str, f: Box<dyn Fn()>, retained: &mut Vec<Retained<AnyObject>>| {
            let t = ActionTarget::new(mtm, move || f());
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(ActionTarget::sel()),
                    ns_string!(""),
                )
            };
            unsafe { it.setTarget(Some(&t)) };
            retained.push(Retained::into_super(Retained::into_super(t)));
            it
        };
        let send_work_break = {
            let sink = sink.clone();
            move |w: u32, b: u32| {
                sink(UiEvent::SetDurations {
                    work_min: w,
                    break_min: b,
                })
            }
        };
        let mut presets = Vec::new();
        for (w, b) in PRESETS {
            let send = send_work_break.clone();
            let it = item(
                &format!("{w} мин работы, {b} мин перерыва"),
                Box::new(move || send(w, b)),
                &mut retained,
            );
            menu.addItem(&it);
            presets.push((it, (w, b)));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let sub = |title: &str, menu: &NSMenu| -> Retained<NSMenu> {
            let parent = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    None,
                    ns_string!(""),
                )
            };
            let sub = NSMenu::new(mtm);
            parent.setSubmenu(Some(&sub));
            menu.addItem(&parent);
            sub
        };
        let work_menu = sub("Работа", &menu);
        let mut work = Vec::new();
        for m in WORK {
            let (send, now) = (send_work_break.clone(), now.clone());
            let it = item(
                &format!("{m} мин"),
                Box::new(move || send(m, now.get().1)),
                &mut retained,
            );
            work_menu.addItem(&it);
            work.push((it, m));
        }
        let break_menu = sub("Перерыв", &menu);
        let mut breaks = Vec::new();
        for m in BREAK {
            let (send, now) = (send_work_break.clone(), now.clone());
            let it = item(
                &format!("{m} мин"),
                Box::new(move || send(now.get().0, m)),
                &mut retained,
            );
            break_menu.addItem(&it);
            breaks.push((it, m));
        }
        let long_menu = sub("Длинный перерыв", &menu);
        let mut long = Vec::new();
        for m in LONG {
            let (sink, now) = (sink.clone(), now.clone());
            let it = item(
                &format!("{m} мин"),
                Box::new(move || {
                    let p = now.get();
                    sink(UiEvent::SetCycle {
                        long_min: m,
                        every: p.3,
                        auto_breaks: p.4,
                        auto_work: p.5,
                    });
                }),
                &mut retained,
            );
            long_menu.addItem(&it);
            long.push((it, m));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let open = {
            let sink = sink.clone();
            item(
                "Настройки помидора…",
                Box::new(move || sink(UiEvent::OpenSettings(2))),
                &mut retained,
            )
        };
        menu.addItem(&open);
        // Kept for `show_values`.
        Self {
            now,
            menu,
            presets,
            work,
            breaks,
            long,
            _retained: retained,
        }
    }

    /// The menu under the 🍅's chevron.
    pub fn menu(&self) -> &NSMenu {
        &self.menu
    }

    /// The settings now in force (from the snapshot): ticks the lengths.
    pub fn show_values(&self, t: &TimerSnap) {
        self.now.set((t.work_min, t.break_min, t.long_min, t.every, t.auto_breaks, t.auto_work));
        for (it, (w, b)) in &self.presets {
            it.setState(state((*w, *b) == (t.work_min, t.break_min)));
        }
        for (it, m) in &self.work {
            it.setState(state(*m == t.work_min));
        }
        for (it, m) in &self.breaks {
            it.setState(state(*m == t.break_min));
        }
        for (it, m) in &self.long {
            it.setState(state(*m == t.long_min));
        }
    }
}
