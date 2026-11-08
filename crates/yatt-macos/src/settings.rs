//! The settings window: a native tab-view controller with a toolbar of tabs, as
//! the system's own settings windows have. One tab so far, «Задачи».

use std::rc::Rc;

use objc2::{MainThreadMarker, MainThreadOnly};
use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSButton, NSControlStateValueOn, NSImage, NSNormalWindowLevel, NSPopUpButton,
    NSTabViewController, NSTabViewControllerTabStyle, NSTabViewItem, NSTextAlignment, NSTextField,
    NSViewController, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSCopying, NSPoint, NSRect, NSSize, NSString, NSUserDefaults, ns_string};

use crate::board::HUE_NAMES;
use crate::classes::{ActionTarget, FlippedView};

const COLOR_KEY: &str = "defaultTaskColor";
const OPEN_KEY: &str = "openTaskIn";
const RECOLOR_KEY: &str = "recolorByTag";

/// A new card's colour unless one is picked: yellow (an index into the palette).
pub const DEFAULT_COLOR: usize = 8;

/// The colour new tasks get.
pub fn default_color() -> usize {
    let d = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str(COLOR_KEY);
    if d.objectForKey(&key).is_some() {
        (d.integerForKey(&key).max(0) as usize).min(HUE_NAMES.len() - 1)
    } else {
        DEFAULT_COLOR
    }
}

pub fn set_default_color(c: usize) {
    NSUserDefaults::standardUserDefaults().setInteger_forKey(c as isize, &NSString::from_str(COLOR_KEY));
}

/// Whether a task opens in a window of its own instead of the inspector.
pub fn open_in_window() -> bool {
    NSUserDefaults::standardUserDefaults().integerForKey(&NSString::from_str(OPEN_KEY)) == 1
}

pub fn set_open_in_window(on: bool) {
    NSUserDefaults::standardUserDefaults()
        .setInteger_forKey(on as isize, &NSString::from_str(OPEN_KEY));
}

/// Whether cards take the colour of their first tag.
pub fn recolor_by_tag() -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(RECOLOR_KEY))
}

pub fn set_recolor_by_tag(on: bool) {
    NSUserDefaults::standardUserDefaults().setBool_forKey(on, &NSString::from_str(RECOLOR_KEY));
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    _retained: Vec<Retained<AnyObject>>,
}

fn label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setAlignment(NSTextAlignment::Right);
    l.setFrame(frame);
    l
}

impl SettingsWindow {
    /// `changed` is called after any setting changed (the board reads them again).
    pub fn new(mtm: MainThreadMarker, changed: Rc<dyn Fn()>) -> Self {
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let view = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(520.0, 190.0)));

        // Default colour of a new task.
        view.addSubview(&label(
            mtm,
            "Цвет новой задачи:",
            NSRect::new(NSPoint::new(20.0, 28.0), NSSize::new(190.0, 20.0)),
        ));
        let colors = unsafe {
            NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                NSRect::new(NSPoint::new(222.0, 24.0), NSSize::new(220.0, 26.0)),
                false,
            )
        };
        let names: Vec<Retained<NSString>> = HUE_NAMES.iter().map(|n| NSString::from_str(n)).collect();
        colors.addItemsWithTitles(&NSArray::from_retained_slice(&names));
        colors.selectItemAtIndex(default_color() as isize);
        {
            let (slot, changed) = (ObjcWeak::from_retained(&colors), changed.clone());
            let t = ActionTarget::new(mtm, move || {
                if let Some(p) = slot.load() {
                    set_default_color(p.indexOfSelectedItem().max(0) as usize);
                    changed();
                }
            });
            unsafe {
                colors.setTarget(Some(&t));
                colors.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(t)));
        }
        view.addSubview(&colors);

        // Where a task opens.
        view.addSubview(&label(
            mtm,
            "Открывать задачу:",
            NSRect::new(NSPoint::new(20.0, 70.0), NSSize::new(190.0, 20.0)),
        ));
        let open = unsafe {
            NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                NSRect::new(NSPoint::new(222.0, 66.0), NSSize::new(220.0, 26.0)),
                false,
            )
        };
        open.addItemsWithTitles(&NSArray::from_retained_slice(&[
            ns_string!("В инспекторе").copy(),
            ns_string!("В отдельном окне").copy(),
        ]));
        open.selectItemAtIndex(open_in_window() as isize);
        {
            let (slot, changed) = (ObjcWeak::from_retained(&open), changed.clone());
            let t = ActionTarget::new(mtm, move || {
                if let Some(p) = slot.load() {
                    set_open_in_window(p.indexOfSelectedItem() == 1);
                    changed();
                }
            });
            unsafe {
                open.setTarget(Some(&t));
                open.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(t)));
        }
        view.addSubview(&open);

        // Cards in the colour of their first tag.
        let recolor_slot: Rc<std::cell::RefCell<Option<ObjcWeak<NSButton>>>> = Rc::default();
        let t = {
            let (slot, changed) = (recolor_slot.clone(), changed.clone());
            ActionTarget::new(mtm, move || {
                if let Some(b) = slot.borrow().as_ref().and_then(|w| w.load()) {
                    set_recolor_by_tag(b.state() == NSControlStateValueOn);
                    changed();
                }
            })
        };
        let recolor = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Красить карточку в цвет первого тега"),
                Some(&t),
                Some(ActionTarget::sel()),
                mtm,
            )
        };
        recolor.setState(if recolor_by_tag() { NSControlStateValueOn } else { 0 });
        recolor.setFrame(NSRect::new(NSPoint::new(222.0, 112.0), NSSize::new(280.0, 22.0)));
        *recolor_slot.borrow_mut() = Some(ObjcWeak::from_retained(&recolor));
        view.addSubview(&recolor);
        retained.push(Retained::into_super(Retained::into_super(t)));

        let vc = NSViewController::new(mtm);
        vc.setView(&view);
        vc.setTitle(Some(ns_string!("Задачи")));
        vc.setPreferredContentSize(NSSize::new(520.0, 170.0));
        let tab = NSTabViewItem::tabViewItemWithViewController(&vc);
        tab.setLabel(ns_string!("Задачи"));
        tab.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("checklist"),
                Some(ns_string!("Задачи")),
            )
            .as_deref(),
        );
        let tabs = NSTabViewController::new(mtm);
        tabs.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
        tabs.addTabViewItem(&tab);
        let window = NSWindow::windowWithContentViewController(&tabs);
        window.setTitle(ns_string!("Задачи"));
        window.setStyleMask(NSWindowStyleMask::Titled | NSWindowStyleMask::Closable);
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSNormalWindowLevel);
        window.center();
        retained.push(Retained::<AnyObject>::from(Retained::into_super(Retained::into_super(vc))));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(Retained::into_super(tabs))));
        retained.push(Retained::<AnyObject>::from(Retained::into_super(tab)));
        Self {
            window,
            _retained: retained,
        }
    }

    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
    }

    /// Development aid: saves the settings window as a PNG.
    pub fn capture_png(&self, path: &str) -> bool {
        let Some(content) = self.window.contentView() else {
            return false;
        };
        let view = unsafe { content.superview() }.unwrap_or(content);
        let bounds = view.bounds();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
            return false;
        };
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = objc2_foundation::NSDictionary::new();
        let Some(data) = (unsafe {
            rep.representationUsingType_properties(objc2_app_kit::NSBitmapImageFileType::PNG, &props)
        }) else {
            return false;
        };
        data.writeToFile_atomically(&NSString::from_str(path), true)
    }
}
