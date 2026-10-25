//! The timer in the system's menu bar: a pomodoro's time left as a small pie (a
//! template image, so it follows the menu bar's light or dark look), the
//! stopwatch's minutes as text, a still dimmed pie when idle.
//! A click shows a menu: the exact time, the tasks being worked on (a click
//! opens one's panel), then open the window, pause or resume, stop.

use std::cell::RefCell;
use std::rc::Rc;

use yatt_core::TaskId;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBezierPath, NSCellImagePosition, NSColor, NSFont, NSFontWeightRegular, NSImage, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use yatt_platform::{MenuBar, MenuBarLook};

use crate::classes::ActionTarget;

const SIDE: f64 = 16.0;

/// The disc in whole degrees, the label, dimmed, paused.
type Shown = (Option<i64>, Option<String>, bool, bool);

/// A task in the menu: id, title, colour, time so far.
pub struct MenuTask {
    pub id: TaskId,
    pub title: String,
    pub hue: usize,
    pub time: String,
}

pub struct StatusBar {
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    pause: Retained<NSMenuItem>,
    /// The timer to the second, at the top.
    clock: Retained<NSMenuItem>,
    /// The task rows under it, and what they show.
    tasks: RefCell<Vec<(TaskId, String, Retained<NSMenuItem>)>>,
    task_targets: RefCell<Vec<Retained<AnyObject>>>,
    open_task: Rc<dyn Fn(TaskId)>,
    /// What is drawn now: the disc in whole degrees, the label, dimmed, paused.
    shown: RefCell<Option<Shown>>,
    _targets: Vec<Retained<AnyObject>>,
}

impl StatusBar {
    /// `open` brings the window forward; `pause` and `stop` drive the timer.
    pub fn new(
        mtm: MainThreadMarker,
        open: impl Fn() + 'static,
        pause: impl Fn() + 'static,
        stop: impl Fn() + 'static,
        open_task: impl Fn(TaskId) + 'static,
    ) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        let mut targets: Vec<Retained<AnyObject>> = Vec::new();
        let mut add = |title: &str, f: Box<dyn Fn()>| {
            let t = ActionTarget::new(mtm, f);
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(ActionTarget::sel()),
                    ns_string!(""),
                )
            };
            unsafe { it.setTarget(Some(&t)) };
            menu.addItem(&it);
            targets.push(Retained::into_super(Retained::into_super(t)));
            it
        };
        let clock = NSMenuItem::new(mtm);
        clock.setEnabled(false);
        menu.addItem(&clock);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add("Открыть yatt", Box::new(open));
        let pause_item = add("Пауза", Box::new(pause));
        add("Стоп", Box::new(stop));
        item.setMenu(Some(&menu));
        if let Some(b) = item.button(mtm) {
            b.setImagePosition(NSCellImagePosition::ImageLeft);
            b.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                0.0,
                unsafe { NSFontWeightRegular },
            )));
        }
        Self {
            item,
            menu,
            clock,
            tasks: RefCell::new(Vec::new()),
            task_targets: RefCell::new(Vec::new()),
            open_task: Rc::new(open_task),
            pause: pause_item,
            shown: RefCell::new(None),
            _targets: targets,
        }
    }
}

impl StatusBar {
    /// The menu's top: the timer to the second, then the tasks it counts for
    /// (or those «В работе»). Rows are rebuilt only when the tasks change;
    /// otherwise only their times are set again.
    pub fn show_tasks(&self, clock: &str, tasks: &[MenuTask]) {
        let mtm = MainThreadMarker::new().expect("main thread");
        let clock = NSString::from_str(clock);
        if self.clock.title() != clock {
            self.clock.setTitle(&clock);
        }
        let same = {
            let rows = self.tasks.borrow();
            rows.len() == tasks.len()
                && rows
                    .iter()
                    .zip(tasks)
                    .all(|(r, t)| r.0 == t.id && r.1 == t.title)
        };
        if same {
            for ((_, _, item), t) in self.tasks.borrow().iter().zip(tasks) {
                let title = NSString::from_str(&row_title(t));
                if item.title() != title {
                    item.setTitle(&title);
                }
            }
            return;
        }
        for (_, _, item) in self.tasks.borrow_mut().drain(..) {
            self.menu.removeItem(&item);
        }
        self.task_targets.borrow_mut().clear();
        // Right under the clock and its separator.
        let at = self.menu.indexOfItem(&self.clock) + 2;
        let mut rows = Vec::new();
        for (k, t) in tasks.iter().enumerate() {
            let (open, id) = (self.open_task.clone(), t.id);
            let target = ActionTarget::new(mtm, move || open(id));
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&row_title(t)),
                    Some(ActionTarget::sel()),
                    ns_string!(""),
                )
            };
            unsafe { item.setTarget(Some(&target)) };
            item.setImage(dot(t.hue).as_deref());
            self.menu.insertItem_atIndex(&item, at + k as isize);
            self.task_targets
                .borrow_mut()
                .push(Retained::into_super(Retained::into_super(target)));
            rows.push((t.id, t.title.clone(), item));
        }
        *self.tasks.borrow_mut() = rows;
    }
}

fn row_title(t: &MenuTask) -> String {
    format!("{}  ·  {}", t.title, t.time)
}

/// The card's colour as a small dot.
fn dot(hue: usize) -> Option<Retained<NSImage>> {
    let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        ns_string!("circle.fill"),
        None,
    )?;
    let config = objc2_app_kit::NSImageSymbolConfiguration::configurationWithPaletteColors(
        &objc2_foundation::NSArray::from_retained_slice(&[crate::style::swatch(hue)]),
    );
    base.imageWithSymbolConfiguration(&config)
}

/// The pie: a wedge `fill` of the way round clockwise from the top over a
/// faint full disc (the time gone), no outline; all fainter when `dimmed`.
fn disc(fill: f64, dimmed: bool) -> Retained<NSImage> {
    let draw = RcBlock::new(move |_rect: NSRect| -> Bool {
        let alpha = if dimmed { 0.45 } else { 1.0 };
        let c = NSPoint::new(SIDE / 2.0, SIDE / 2.0);
        let r = SIDE / 2.0 - 1.5;
        NSColor::blackColor()
            .colorWithAlphaComponent(0.2 * alpha)
            .set();
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(c.x - r, c.y - r),
            NSSize::new(2.0 * r, 2.0 * r),
        ))
        .fill();
        if fill > 0.0 {
            NSColor::blackColor().colorWithAlphaComponent(alpha).set();
            let wedge = NSBezierPath::bezierPath();
            wedge.moveToPoint(c);
            // y up: the top is 90°, clockwise goes down from there.
            wedge.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                c,
                r,
                90.0,
                90.0 - 360.0 * fill.min(1.0),
                true,
            );
            wedge.closePath();
            wedge.fill();
        }
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(SIDE, SIDE), false, &draw);
    image.setTemplate(true);
    image
}

impl MenuBar for StatusBar {
    fn show(&self, look: &MenuBarLook) {
        // Drawn again only when something visible changed: a degree of the
        // disc, the minutes, the state.
        let key = (
            look.disc.map(|d| (d * 360.0).round() as i64),
            look.label.clone(),
            look.dimmed,
            look.paused,
        );
        if self.shown.borrow().as_ref() == Some(&key) {
            return;
        }
        let mtm = MainThreadMarker::new().expect("main thread");
        if let Some(b) = self.item.button(mtm) {
            b.setImage(look.disc.map(|d| disc(d, look.dimmed)).as_deref());
            let gap = if look.disc.is_some() { " " } else { "" };
            let title = look
                .label
                .as_deref()
                .map(|l| format!("{gap}{l}"))
                .unwrap_or_default();
            b.setTitle(&NSString::from_str(&title));
        }
        self.pause.setTitle(&NSString::from_str(if look.paused {
            "Продолжить"
        } else {
            "Пауза"
        }));
        *self.shown.borrow_mut() = Some(key);
    }
}
