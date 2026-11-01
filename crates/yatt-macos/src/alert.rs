//! A plain system alert, for the problems the user has to hear about.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication};
use objc2_foundation::NSString;

/// Shows a modal alert and returns when it is dismissed. Safe to call before the
/// GPUI app has started, which is when a database that will not open is found.
pub fn alert(mtm: MainThreadMarker, title: &str, text: &str) {
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    alert.runModal();
}
