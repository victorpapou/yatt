//! The task in a window of its own, when that is how the settings want it: a
//! native window with a toolbar holding the pane's buttons (look, trash, close)
//! and the same pane as its content, the one the inspector shows otherwise.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSImage, NSNormalWindowLevel, NSToolbar, NSToolbarDisplayMode,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSView, NSWindow, NSWindowStyleMask,
    NSWindowToolbarStyle,
};
use objc2_foundation::{NSCopying, NSPoint, NSRect, NSSize, NSString, ns_string};

use crate::classes::{ActionTarget, ToolbarDelegate, WindowDelegate};

pub struct TaskWindow {
    window: Retained<NSWindow>,
    _retained: Vec<Retained<AnyObject>>,
}

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}

impl TaskWindow {
    /// `on_look`, `on_trash` and `on_close` are the toolbar buttons; `on_closed`
    /// the window's own red button.
    pub fn new(
        mtm: MainThreadMarker,
        on_look: Rc<dyn Fn()>,
        on_trash: Rc<dyn Fn()>,
        on_close: Rc<dyn Fn()>,
        on_closed: impl Fn() + 'static,
    ) -> Self {
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(520.0, 640.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSNormalWindowLevel);
        window.setMinSize(NSSize::new(380.0, 360.0));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
        window.setFrameAutosaveName(ns_string!("yatt.task"));

        let mut items: Vec<Retained<NSToolbarItem>> = Vec::new();
        for (id, sym, tip, f) in [
            ("yatt.win.look", "paintpalette", "Цвет, иконка, статус", on_look),
            ("yatt.win.trash", "trash", "Удалить (⌘Z вернёт)", on_trash),
            ("yatt.win.close", "xmark", "Закрыть (esc)", on_close),
        ] {
            let item = NSToolbarItem::initWithItemIdentifier(
                NSToolbarItem::alloc(mtm),
                &NSString::from_str(id),
            );
            item.setLabel(&NSString::from_str(tip));
            item.setToolTip(Some(&NSString::from_str(tip)));
            item.setImage(symbol(sym, tip).as_deref());
            let t = ActionTarget::new(mtm, move || f());
            unsafe {
                item.setTarget(Some(&t));
                item.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(t)));
            items.push(item);
        }
        let flexible = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
        let order: Vec<Retained<NSString>> = vec![
            flexible,
            items[0].itemIdentifier(),
            items[1].itemIdentifier(),
            items[2].itemIdentifier(),
        ];
        let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), ns_string!("yatt.task.toolbar"));
        let delegate = ToolbarDelegate::new(mtm, items, order, Vec::new());
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.setAllowsUserCustomization(false);
        retained.push(Retained::into_super(Retained::into_super(delegate)));
        let closed = WindowDelegate::new(mtm, on_closed);
        window.setDelegate(Some(ProtocolObject::from_ref(&*closed)));
        retained.push(Retained::into_super(Retained::into_super(closed)));
        window.setToolbar(Some(&toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        retained.push(Retained::<AnyObject>::from(Retained::into_super(toolbar)));
        Self {
            window,
            _retained: retained,
        }
    }

    /// Shows the pane in the window.
    pub fn attach(&self, pane: &NSView) {
        unsafe {
            if pane.superview().is_some() {
                pane.removeFromSuperview();
            }
        }
        self.window.setContentView(Some(pane));
        self.window.makeKeyAndOrderFront(None);
    }

    /// Takes the pane out and hides the window; `blank` stands in as its content.
    pub fn detach(&self, blank: &NSView) {
        self.window.orderOut(None);
        self.window.setContentView(Some(blank));
    }

    /// Development aid: saves the task window as a PNG.
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

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    /// How far the toolbar reaches into the content, where the pane starts.
    pub fn top_inset(&self) -> f64 {
        let frame = self.window.frame().size.height;
        let layout = self.window.contentLayoutRect().size.height;
        let inset = frame - layout;
        if inset > 20.0 && inset < 100.0 { inset } else { 52.0 }
    }
}
