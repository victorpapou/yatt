//! The Objective-C subclasses the window needs. Each one only forwards AppKit
//! callbacks to a Rust closure; the logic lives in `window.rs`.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    sel,
};
use objc2_app_kit::{
    NSColor, NSEvent, NSEventModifierFlags, NSResponder, NSTrackingArea, NSTrackingAreaOptions,
    NSView, NSWindow, NSWindowDelegate,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSString};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController};
use yatt_core::TaskId;

pub type KeyHandler = Box<dyn Fn(&NSEvent) -> bool>;
pub type CardHandler = Box<dyn Fn(TaskId, CardMsg)>;
pub type HoverHandler = Box<dyn Fn(Option<NSPoint>)>;

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

pub struct RootIvars {
    pub on_layout: RefCell<Option<Box<dyn Fn()>>>,
    /// Esc anywhere in the window that nothing else handled (`cancelOperation:`).
    pub on_cancel: RefCell<Option<Box<dyn Fn()>>>,
    /// Returns true when it handled the key.
    pub on_key: RefCell<Option<KeyHandler>>,
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
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RootIvars {
            on_layout: RefCell::new(None),
            on_cancel: RefCell::new(None),
            on_key: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// What a [`FillView`] paints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// A card's pastel face; `tracking` adds an accent rim.
    Card { hue: usize, tracking: bool },
    /// The light frosting over a glass card's blur.
    Frost,
    /// The "N скрыто" capsule.
    Badge,
    /// A 1 px divider line.
    Separator,
    /// The light tint over the column a card is dragged over.
    Tint,
    /// The backdrop behind the task modal.
    Dim,
    /// The modal panel itself.
    Panel,
}

pub struct FillIvars {
    pub fill: Cell<Fill>,
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
                Fill::Frost => NSColor::whiteColor().colorWithAlphaComponent(0.42),
                Fill::Badge => NSColor::whiteColor().colorWithAlphaComponent(0.55),
                Fill::Separator => NSColor::separatorColor(),
                Fill::Tint => NSColor::labelColor().colorWithAlphaComponent(0.035),
                Fill::Dim => NSColor::blackColor().colorWithAlphaComponent(0.28),
                Fill::Panel => NSColor::windowBackgroundColor(),
            };
            layer.setBackgroundColor(Some(&color.CGColor()));
            match fill {
                Fill::Card { tracking, .. } => {
                    layer.setCornerRadius(crate::style::CARD_RADIUS);
                    layer.setBorderWidth(if tracking { 2.0 } else { 1.0 });
                    let rim = if tracking {
                        NSColor::controlAccentColor()
                    } else {
                        NSColor::whiteColor().colorWithAlphaComponent(0.45)
                    };
                    layer.setBorderColor(Some(&rim.CGColor()));
                }
                Fill::Frost => {
                    layer.setCornerRadius(crate::style::CARD_RADIUS);
                    layer.setBorderWidth(1.0);
                    layer.setBorderColor(Some(
                        &NSColor::whiteColor().colorWithAlphaComponent(0.7).CGColor(),
                    ));
                }
                Fill::Badge => layer.setCornerRadius(9.0),
                _ => {}
            }
            if fill == Fill::Panel {
                layer.setCornerRadius(16.0);
                layer.setBorderWidth(0.5);
                layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
                layer.setShadowColor(Some(&NSColor::shadowColor().CGColor()));
                layer.setShadowOpacity(0.3);
                layer.setShadowRadius(24.0);
                layer.setShadowOffset(objc2_foundation::NSSize::new(0.0, -8.0));
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
    pub fn set_fill(&self, fill: Fill) {
        if self.ivars().fill.replace(fill) != fill {
            self.setNeedsDisplay(true);
        }
    }

    pub fn new(mtm: MainThreadMarker, fill: Fill) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FillIvars {
            fill: Cell::new(fill),
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

/// Raw mouse input on a card, in the window's coordinates. The board decides
/// what is a click and what is a drag.
pub enum CardMsg {
    Down { at: NSPoint, shift: bool },
    Dragged(NSPoint),
    Up(NSPoint),
}

pub struct CardIvars {
    pub id: TaskId,
    pub on_msg: RefCell<Option<CardHandler>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = CardIvars]
    pub struct CardView;

    impl CardView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let shift = event.modifierFlags().contains(NSEventModifierFlags::Shift);
            self.send(CardMsg::Down {
                at: event.locationInWindow(),
                shift,
            });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send(CardMsg::Dragged(event.locationInWindow()));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send(CardMsg::Up(event.locationInWindow()));
        }
    }
);

impl CardView {
    pub fn new(mtm: MainThreadMarker, id: TaskId, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CardIvars {
            id,
            on_msg: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn send(&self, msg: CardMsg) {
        if let Some(f) = self.ivars().on_msg.borrow().as_ref() {
            f(self.ivars().id, msg);
        }
    }
}

/// Where the cards live. Tracks the mouse over the whole board so stacks can fan
/// out on hover; reports `None` when the mouse leaves.
pub struct CanvasIvars {
    pub on_hover: RefCell<Option<HoverHandler>>,
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
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            self.send(Some(p));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.send(None);
        }

        // Only the cards take clicks; empty board passes them to the columns below.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.filter(|v| !std::ptr::eq(&**v, &**self))
        }
    }
);

impl CanvasView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CanvasIvars {
            on_hover: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn send(&self, p: Option<NSPoint>) {
        if let Some(f) = self.ivars().on_hover.borrow().as_ref() {
            f(p);
        }
    }
}

/// The window's current first responder is a text editor (a field being edited or a text view).
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
