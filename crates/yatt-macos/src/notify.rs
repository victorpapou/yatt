//! A system notification with the "Glass" sound, for the end of a phase.
//! Goes through `osascript`, so it needs no bundle or notification entitlement.

use std::process::{Command, Stdio};

pub struct MacNotifier;

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl yatt_platform::Notifier for MacNotifier {
    fn notify(&self, title: &str, body: &str) {
        let script = format!(
            "display notification {} with title {} sound name \"Glass\"",
            quoted(body),
            quoted(title)
        );
        // Fire and forget: a missing notification must never stall the timer.
        let _ = Command::new("osascript")
            .args(["-e", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
