//! The Objective-C subclasses the window needs. Each one only forwards AppKit
//! callbacks to a Rust closure; the logic lives in `window.rs`.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send, sel,
};
use objc2_app_kit::{
    NSColor, NSControl, NSControlTextEditingDelegate, NSEvent, NSEventModifierFlags, NSResponder,
    NSTextFieldDelegate, NSTextView, NSToolbarDelegate, NSToolbarItem, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowDelegate,
};
use objc2_foundation::{NSArray, NSNotification, NSPoint, NSRect, NSString};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController};

pub type KeyHandler = Box<dyn Fn(&NSEvent) -> bool>;
pub type CanvasHandler = Box<dyn Fn(CanvasMsg)>;

// ---- target/action ----

pub struct ActionIvars {
    f: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActionIvars]
    pub struct ActionTarget;

    impl ActionTarget {
        #[unsafe(method(fire:))]
        fn fire(&self, _sender: Option<&NSObject>) {
            (self.ivars().f)();
        }
    }

    unsafe impl NSObjectProtocol for ActionTarget {}
);

impl ActionTarget {
    pub fn new(mtm: MainThreadMarker, f: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionIvars { f: Box::new(f) });
        unsafe { msg_send![super(this), init] }
    }

    pub fn sel() -> objc2::runtime::Sel {
        sel!(fire:)
    }
}

// ---- window delegate ----

pub struct WindowIvars {
    on_close: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = WindowIvars]
    pub struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            (self.ivars().on_close)();
        }
    }
);

impl WindowDelegate {
    pub fn new(mtm: MainThreadMarker, on_close: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowIvars {
            on_close: Box::new(on_close),
        });
        unsafe { msg_send![super(this), init] }
    }
}

// ---- toolbar ----

pub struct ToolbarIvars {
    items: Vec<Retained<NSToolbarItem>>,
    /// Identifiers in order, the system's spaces included.
    order: Vec<Retained<NSString>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ToolbarIvars]
    /// Serves the window's toolbar its items, in a fixed order.
    pub struct ToolbarDelegate;

    unsafe impl NSObjectProtocol for ToolbarDelegate {}

    unsafe impl NSToolbarDelegate for ToolbarDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for(
            &self,
            _toolbar: &objc2_app_kit::NSToolbar,
            id: &NSString,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.ivars()
                .items
                .iter()
                .find(|item| *item.itemIdentifier() == *id)
                .cloned()
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &objc2_app_kit::NSToolbar) -> Retained<NSArray<NSString>> {
            NSArray::from_retained_slice(&self.ivars().order)
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &objc2_app_kit::NSToolbar) -> Retained<NSArray<NSString>> {
            NSArray::from_retained_slice(&self.ivars().order)
        }
    }
);

impl ToolbarDelegate {
    pub fn new(
        mtm: MainThreadMarker,
        items: Vec<Retained<NSToolbarItem>>,
        order: Vec<Retained<NSString>>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ToolbarIvars { items, order });
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // SAFETY: NSToolbarItem has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSToolbarItem, NSObject))]
    #[thread_kind = MainThreadOnly]
    /// A toolbar item for plain content: it asks the toolbar for no glass bubble.
    /// `glassBehavior` is AppKit's own private, read-only switch for that on
    /// macOS 26 and later; `hidesSharedBackground` is not there on 27.2.
    pub struct PlainToolbarItem;

    impl PlainToolbarItem {
        #[unsafe(method(glassBehavior))]
        fn glass_behavior(&self) -> usize {
            // 0: no glass. Any other value (1–3 tried) gives the item a bubble.
            0
        }
    }
);

impl PlainToolbarItem {
    /// A new item, as the plain toolbar item it is to everyone else.
    pub fn item(mtm: MainThreadMarker, id: &NSString) -> Retained<NSToolbarItem> {
        let this = Self::alloc(mtm).set_ivars(());
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithItemIdentifier: id] };
        Retained::into_super(this)
    }
}

// ---- views ----

/// A flipped plain view: y grows downwards, like the layouts here.
#[derive(Default)]
pub struct NoIvars;

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = NoIvars]
    pub struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NoIvars);
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// The standard edit commands as they reach the board: from the Edit menu (or
/// its shortcuts) when no text field is editing, and Delete/Backspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCmd {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Delete,
}

pub type EditHandler = Box<dyn Fn(EditCmd)>;

pub struct RootIvars {
    pub on_layout: RefCell<Option<Box<dyn Fn()>>>,
    /// Copy, cut, paste, select all and delete for the board's selection.
    pub on_edit: RefCell<Option<EditHandler>>,
    /// Esc anywhere in the window that nothing else handled (`cancelOperation:`).
    pub on_cancel: RefCell<Option<Box<dyn Fn()>>>,
    /// Returns true when it handled the key.
    pub on_key: RefCell<Option<KeyHandler>>,
    /// The window turned light or dark.
    pub on_appearance: RefCell<Option<Box<dyn Fn()>>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RootIvars]
    pub struct RootView;

    impl RootView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn did_change_appearance(&self) {
            if let Some(f) = self.ivars().on_appearance.borrow().as_ref() {
                f();
            }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, _old: objc2_foundation::NSSize) {
            if let Some(f) = self.ivars().on_layout.borrow().as_ref() {
                f();
            }
        }

        // The Edit menu's actions travel the responder chain: a text field that is
        // editing takes them first; otherwise they land here, for the board.
        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            self.edit(EditCmd::Copy);
        }

        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            self.edit(EditCmd::Cut);
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            self.edit(EditCmd::Paste);
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            self.edit(EditCmd::SelectAll);
        }

        #[unsafe(method(delete:))]
        fn delete(&self, _sender: Option<&AnyObject>) {
            self.edit(EditCmd::Delete);
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            if let Some(f) = self.ivars().on_cancel.borrow().as_ref() {
                f();
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled = self
                .ivars()
                .on_key
                .borrow()
                .as_ref()
                .is_some_and(|f| f(event));
            if !handled {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }
    }
);

impl RootView {
    fn edit(&self, cmd: EditCmd) {
        if let Some(f) = self.ivars().on_edit.borrow().as_ref() {
            f(cmd);
        }
    }

    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RootIvars {
            on_layout: RefCell::new(None),
            on_edit: RefCell::new(None),
            on_cancel: RefCell::new(None),
            on_key: RefCell::new(None),
            on_appearance: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// What a [`FillView`] paints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// A card's tint; `selected` adds an accent ring.
    Card {
        hue: usize,
        tracking: bool,
        selected: bool,
    },
    /// A column's header pill, tinted in the column's colour.
    Chip(usize),
    /// A 1 px divider line.
    Separator,
    /// The faint tint, in the column's own colour, over the column a card is
    /// dragged over.
    Tint(usize),
    /// The backdrop behind the task modal.
    Dim,
    /// The modal panel itself.
    Panel,
    /// The task panel's white body under its tinted header: rounded at the
    /// bottom only.
    Sheet,
}

pub struct FillIvars {
    pub fill: Cell<Fill>,
    /// A card's corners shrink with the card: its radius is CARD_RADIUS × this.
    pub corner_scale: Cell<f64>,
    pub on_click: RefCell<Option<Box<dyn Fn()>>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = FillIvars]
    pub struct FillView;

    impl FillView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            let fill = self.ivars().fill.get();
            let color = match fill {
                Fill::Card { hue, .. } => crate::style::fill(hue),
                Fill::Chip(i) => crate::style::column_color(i).colorWithAlphaComponent(0.14),
                Fill::Separator => NSColor::separatorColor(),
                Fill::Tint(i) => crate::style::column_color(i).colorWithAlphaComponent(0.045),
                Fill::Dim => NSColor::blackColor().colorWithAlphaComponent(0.28),
                // The panel's look comes from the card material inside it.
                Fill::Panel => NSColor::clearColor(),
                Fill::Sheet => NSColor::textBackgroundColor(),
            };
            layer.setBackgroundColor(Some(&color.CGColor()));
            match fill {
                Fill::Card { .. } => {
                    crate::style::round(
                        &layer,
                        crate::style::CARD_RADIUS * self.ivars().corner_scale.get(),
                    );
                    // No stroke or shadow: the card's edge, and the selection ring
                    // outside it, are layers of their own (`style::Edges`). A
                    // running timer shows as ● in the text.
                    layer.setBorderWidth(0.0);
                }
                Fill::Chip(_) => crate::style::round(&layer, 9.0),
                Fill::Sheet => {
                    crate::style::round(&layer, crate::style::CARD_RADIUS);
                    // The flipped view's layer counts y down: the bottom corners.
                    layer.setMaskedCorners(
                        objc2_quartz_core::CACornerMask::LayerMinXMaxYCorner
                            | objc2_quartz_core::CACornerMask::LayerMaxXMaxYCorner,
                    );
                }
                _ => {}
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            match self.ivars().on_click.borrow().as_ref() {
                Some(f) => f(),
                // Panels swallow clicks so they do not reach the board below.
                None if self.ivars().fill.get() == Fill::Panel => {}
                None => unsafe { msg_send![super(self), mouseDown: event] },
            }
        }
    }
);

impl FillView {
    /// A card's tint at `scale` of its size: the corners go with it.
    pub fn set_corner_scale(&self, scale: f64) {
        self.ivars().corner_scale.set(scale);
        if let Some(layer) = self.layer() {
            layer.setCornerRadius(crate::style::CARD_RADIUS * scale);
        }
    }

    /// Draws again in the current colours (the appearance changed), easing over.
    pub fn repaint(&self) {
        crate::motion::soften(self);
        self.setNeedsDisplay(true);
    }

    pub fn set_fill(&self, fill: Fill) {
        if self.ivars().fill.replace(fill) != fill {
            // Selection rings, tints and fills ease over instead of jumping.
            crate::motion::soften(self);
            self.setNeedsDisplay(true);
        }
    }

    pub fn new(mtm: MainThreadMarker, fill: Fill) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FillIvars {
            fill: Cell::new(fill),
            corner_scale: Cell::new(1.0),
            on_click: RefCell::new(None),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        view.setWantsLayer(true);
        view
    }
}

// ---- WebKit bridge ----

pub struct ScriptIvars {
    on_message: Box<dyn Fn(String)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ScriptIvars]
    pub struct ScriptHandler;

    unsafe impl NSObjectProtocol for ScriptHandler {}

    unsafe impl WKScriptMessageHandler for ScriptHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            if let Ok(text) = body.downcast::<NSString>() {
                (self.ivars().on_message)(text.to_string());
            }
        }
    }
);

impl ScriptHandler {
    pub fn new(mtm: MainThreadMarker, on_message: impl Fn(String) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ScriptIvars {
            on_message: Box::new(on_message),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// Pointer input on the board canvas, in the canvas's coordinates. The board
/// decides what is a click, a drag or a scroll.
pub enum CanvasMsg {
    Down {
        at: NSPoint,
        shift: bool,
        cmd: bool,
        /// 2 for the second click of a double-click.
        clicks: isize,
    },
    Dragged(NSPoint),
    Up(NSPoint),
    Scroll(Scroll),
    /// The pointer moved over the canvas, or left it (`None`).
    Hover(Option<NSPoint>),
}

#[derive(Debug, Clone)]
pub struct Scroll {
    /// The trackpad or wheel event itself, for handing on to a column's scroll view.
    pub event: Option<Retained<NSEvent>>,
    pub at: NSPoint,
    /// How far the fingers (or the wheel) move down, in points (negative: up),
    /// the same whatever the natural-scrolling setting.
    pub down: f64,
}

pub struct CanvasIvars {
    pub on_msg: RefCell<Option<CanvasHandler>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = CanvasIvars]
    pub struct CanvasView;

    impl CanvasView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        // The canvas takes every click inside it; the cards are only pictures.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            // The columns' scrollers keep their own clicks; everything else is ours.
            hit.map(|v| {
                if v.isKindOfClass(objc2_app_kit::NSScroller::class()) {
                    v
                } else {
                    Retained::into_super(self.retain())
                }
            })
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            for area in self.trackingAreas().iter() {
                self.removeTrackingArea(&area);
            }
            let options = NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveInKeyWindow
                | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    NSRect::ZERO,
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            unsafe { msg_send![super(self), updateTrackingAreas] }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            // Over something drawn above the canvas (the capsule, a chip, the
            // panel) the pointer is that view's: it keeps its own cursor.
            let on_top = self.window().and_then(|w| w.contentView()).and_then(|content| {
                let frame = unsafe { content.superview() }.unwrap_or(content.clone());
                frame.hitTest(frame.convertPoint_fromView(event.locationInWindow(), None))
            });
            let ours = on_top.is_some_and(|v| std::ptr::eq::<NSView>(&*v, self.as_ref()));
            self.send(CanvasMsg::Hover(ours.then(|| self.local(event))));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.send(CanvasMsg::Hover(None));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let flags = event.modifierFlags();
            self.send(CanvasMsg::Down {
                at: self.local(event),
                shift: flags.contains(NSEventModifierFlags::Shift),
                cmd: flags.contains(NSEventModifierFlags::Command),
                clicks: event.clickCount(),
            });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send(CanvasMsg::Dragged(self.local(event)));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send(CanvasMsg::Up(self.local(event)));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let k = if event.hasPreciseScrollingDeltas() { 1.0 } else { 16.0 };
            self.send(CanvasMsg::Scroll(Scroll {
                event: Some(event.retain()),
                at: self.local(event),
                down: crate::physics::gesture_down(
                    event.scrollingDeltaY() * k,
                    event.isDirectionInvertedFromDevice(),
                ),
            }));
        }
    }
);

impl CanvasView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CanvasIvars {
            on_msg: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn local(&self, event: &NSEvent) -> NSPoint {
        self.convertPoint_fromView(event.locationInWindow(), None)
    }

    fn send(&self, msg: CanvasMsg) {
        if let Some(f) = self.ivars().on_msg.borrow().as_ref() {
            f(msg);
        }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = NoIvars]
    /// A flipped view that never takes clicks: cards being dragged ride on it.
    pub struct OverlayView;

    impl OverlayView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

impl OverlayView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NoIvars);
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = NoIvars]
    /// A flipped view that takes clicks only on its subviews; the rest falls through.
    pub struct PassView;

    impl PassView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.filter(|v| !std::ptr::eq::<NSView>(&**v, &**self))
        }
    }
);

impl PassView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NoIvars);
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

// ---- text field delegate ----

pub struct FieldIvars {
    /// Gets each editing command (`insertNewline:` …); returns true when it handled it.
    on_command: Box<dyn Fn(Sel) -> bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = FieldIvars]
    pub struct FieldDelegate;

    unsafe impl NSObjectProtocol for FieldDelegate {}

    unsafe impl NSControlTextEditingDelegate for FieldDelegate {
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _view: &NSTextView, sel: Sel) -> bool {
            (self.ivars().on_command)(sel)
        }
    }

    unsafe impl NSTextFieldDelegate for FieldDelegate {}
);

impl FieldDelegate {
    pub fn new(
        mtm: MainThreadMarker,
        on_command: impl Fn(Sel) -> bool + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FieldIvars {
            on_command: Box::new(on_command),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// The window's current first responder is a text editor (a field being edited or a text view).
/// Whether the first responder is inside a web view (the notes editor).
pub fn editing_in_web_view(window: &NSWindow) -> bool {
    let Some(r) = window.firstResponder() else {
        return false;
    };
    let mut view = r.downcast::<NSView>().ok();
    while let Some(v) = view {
        if v.isKindOfClass(objc2_web_kit::WKWebView::class()) {
            return true;
        }
        view = unsafe { v.superview() };
    }
    false
}

pub fn editing_text(window: &NSWindow) -> bool {
    let Some(r) = window.firstResponder() else {
        return false;
    };
    if r.isKindOfClass(objc2_app_kit::NSText::class()) {
        return true;
    }
    // Inside a web view the first responder is one of WebKit's private subviews.
    let mut view = r.downcast::<NSView>().ok();
    while let Some(v) = view {
        if v.isKindOfClass(objc2_web_kit::WKWebView::class()) {
            return true;
        }
        view = unsafe { v.superview() };
    }
    false
}
