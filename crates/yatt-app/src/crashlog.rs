//! An always-on log at `~/Library/Logs/Yatt/yatt.log`: launch steps, panics with a
//! full backtrace, and uncaught Objective-C exceptions with their call stack. A
//! bundled .app has no terminal, so this file is what a user sends when it dies.

use std::backtrace::Backtrace;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;

static PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn path() -> Option<&'static PathBuf> {
    PATH.get()
}

/// Appends one timestamped line. Never panics: logging must not be what kills the app.
pub fn log(line: impl AsRef<str>) {
    let Some(path) = PATH.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{} {}", jiff::Timestamp::now(), line.as_ref());
    }
}

/// Opens the log (starting a fresh one past 1 MB, keeping the last as `.old`),
/// records the system, and installs the panic and exception hooks.
pub fn init() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let dir = home.join("Library/Logs/Yatt");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("yatt.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = std::fs::rename(&file, dir.join("yatt.log.old"));
    }
    let _ = PATH.set(file);

    log(format!(
        "---- yatt {} started, macOS {}, pid {}",
        env!("CARGO_PKG_VERSION"),
        os_version(),
        std::process::id()
    ));

    std::panic::set_hook(Box::new(|info| {
        log(format!("PANIC {info}\n{}", Backtrace::force_capture()));
    }));
    unsafe { NSSetUncaughtExceptionHandler(Some(on_exception)) };
}

fn os_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "?".into(), |s| s.trim().to_string())
}

unsafe extern "C" {
    fn NSSetUncaughtExceptionHandler(handler: Option<extern "C-unwind" fn(*mut AnyObject)>);
}

/// Called by Foundation for an Objective-C exception nobody caught.
extern "C-unwind" fn on_exception(exception: *mut AnyObject) {
    let text = unsafe {
        let Some(e) = Retained::retain(exception) else {
            return;
        };
        let describe = |sel: &dyn Fn() -> Option<Retained<AnyObject>>| -> String {
            sel().map_or_else(String::new, |o| {
                let d: Option<Retained<objc2_foundation::NSString>> = msg_send![&*o, description];
                d.map(|s| s.to_string()).unwrap_or_default()
            })
        };
        let name = describe(&|| msg_send![&*e, name]);
        let reason = describe(&|| msg_send![&*e, reason]);
        let stack = describe(&|| msg_send![&*e, callStackSymbols]);
        format!("OBJC EXCEPTION {name}: {reason}\n{stack}")
    };
    log(text);
}
