//! `YATT_EVENTLOG=1`: every mouse and scroll event the app gets, where it went,
//! and what the board decided, appended to `/tmp/yatt-events.log`, for chasing
//! input bugs on a machine we cannot watch.

use std::io::Write;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSWindow};

const PATH: &str = "/tmp/yatt-events.log";

pub fn enabled() -> bool {
    thread_local! {
        static ON: bool = std::env::var_os("YATT_EVENTLOG").is_some_and(|v| v != "0");
    }
    ON.with(|on| *on)
}

/// Appends one line, stamped with the time since boot in seconds.
pub fn log(line: impl AsRef<str>) {
    if !enabled() {
        return;
    }
    let t = objc2_quartz_core::CACurrentMediaTime();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PATH)
    {
        let _ = writeln!(f, "{t:.3} {}", line.as_ref());
    }
}

/// The class name of an object, for the log.
pub fn class_of(obj: Option<&AnyObject>) -> String {
    obj.map_or_else(
        || "none".into(),
        |o| o.class().name().to_string_lossy().into_owned(),
    )
}

/// Logs every mouse and scroll event as the app receives it, before AppKit hands
/// it to a view: which window, where, which view it hits, who is first responder.
pub fn install() -> Option<Retained<AnyObject>> {
    if !enabled() {
        return None;
    }
    log("---- yatt started, event log on ----");
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp
        | NSEventMask::ScrollWheel
        | NSEventMask::Gesture
        | NSEventMask::Pressure;
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        let e = unsafe { event.as_ref() };
        let mtm = objc2::MainThreadMarker::new().expect("events arrive on the main thread");
        let window: Option<Retained<NSWindow>> = e.window(mtm);
        let (hit, responder, key) = match window.as_deref() {
            Some(w) => {
                let hit = w.contentView().and_then(|content| {
                    let sup = unsafe { content.superview() }.unwrap_or(content.clone());
                    let p = sup.convertPoint_fromView(e.locationInWindow(), None);
                    sup.hitTest(p)
                });
                (
                    class_of(hit.as_deref().map(|v| v.as_ref())),
                    class_of(w.firstResponder().as_deref().map(|r| r.as_ref())),
                    w.isKeyWindow(),
                )
            }
            None => ("no window".into(), "none".into(), false),
        };
        let p = e.locationInWindow();
        // Clicks belong to mouse events, phases to scroll and gesture events only.
        let detail = match e.r#type() {
            objc2_app_kit::NSEventType::LeftMouseDown
            | objc2_app_kit::NSEventType::LeftMouseDragged
            | objc2_app_kit::NSEventType::LeftMouseUp => format!("clicks {}", e.clickCount()),
            objc2_app_kit::NSEventType::ScrollWheel => {
                format!("phase {:?} momentum {:?}", e.phase(), e.momentumPhase())
            }
            _ => String::new(),
        };
        log(format!(
            "event {:?} at ({:.0}, {:.0}) {detail} key window {key} hit {hit} first responder {responder}",
            e.r#type(),
            p.x,
            p.y,
        ));
        event.as_ptr()
    });
    unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) }
}
