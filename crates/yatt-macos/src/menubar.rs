//! The timer in the system's menu bar: a pomodoro's time left as a small pie (a
//! template image, so it follows the menu bar's light or dark look), the
//! stopwatch's minutes as text, a still dimmed pie when idle.
//! A click shows a short menu: open the window, pause or resume, stop.

use std::cell::RefCell;

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

pub struct StatusBar {
    item: Retained<NSStatusItem>,
    pause: Retained<NSMenuItem>,
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
            pause: pause_item,
            shown: RefCell::new(None),
            _targets: targets,
        }
    }
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
