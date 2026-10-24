//! The Objective-C subclasses the window needs. Each one only forwards AppKit
//! callbacks to a Rust closure; the logic lives in `window.rs`.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    sel,
};
use objc2_app_kit::{
    NSColor, NSDragOperation, NSDraggingContext, NSDraggingDestination, NSDraggingInfo,
    NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent, NSImage, NSPasteboardTypeString,
    NSResponder, NSTextDelegate, NSTextView, NSTextViewDelegate, NSView, NSWindow,
    NSWindowDelegate,
};
use objc2_foundation::{NSArray, NSNotification, NSPoint, NSRect, NSString};
use yatt_core::TaskId;

pub type KeyHandler = Box<dyn Fn(&NSEvent) -> bool>;
pub type CardHandler = Box<dyn Fn(TaskId, CardMsg)>;
pub type DropHandler = Box<dyn Fn(DropMsg) -> bool>;

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

// ---- notes text view delegate ----

pub struct NotesIvars {
    on_end: Box<dyn Fn(String)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = NotesIvars]
    pub struct NotesDelegate;

    unsafe impl NSObjectProtocol for NotesDelegate {}

    unsafe impl NSTextDelegate for NotesDelegate {
        #[unsafe(method(textDidEndEditing:))]
        fn text_did_end_editing(&self, notification: &NSNotification) {
            let Some(obj) = notification.object() else {
                return;
            };
            if let Ok(view) = obj.downcast::<NSTextView>() {
                (self.ivars().on_end)(view.string().to_string());
            }
        }
    }

    unsafe impl NSTextViewDelegate for NotesDelegate {}
);

impl NotesDelegate {
    pub fn new(mtm: MainThreadMarker, on_end: impl Fn(String) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NotesIvars {
            on_end: Box::new(on_end),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn as_delegate(&self) -> &ProtocolObject<dyn NSTextViewDelegate> {
        ProtocolObject::from_ref(self)
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
            on_key: RefCell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// What a card reports to its owner.
pub enum CardMsg {
    Click,
    DragStarted,
    DragEnded,
}

pub struct CardIvars {
    pub id: TaskId,
    /// Accruing time: drawn with an accent border.
    pub tracking: Cell<bool>,
    pub on_msg: RefCell<Option<CardHandler>>,
    down_at: Cell<Option<NSPoint>>,
    dragging: Cell<bool>,
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

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        // Runs with the view's appearance current, so system colors resolve for dark or light.
        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            let tracking = self.ivars().tracking.get();
            layer.setCornerRadius(10.0);
            layer.setBackgroundColor(Some(&NSColor::controlBackgroundColor().CGColor()));
            layer.setBorderWidth(if tracking { 1.5 } else { 0.5 });
            let border = if tracking {
                NSColor::controlAccentColor()
            } else {
                NSColor::separatorColor()
            };
            layer.setBorderColor(Some(&border.CGColor()));
            layer.setShadowColor(Some(&NSColor::shadowColor().CGColor()));
            layer.setShadowOpacity(0.12);
            layer.setShadowRadius(3.0);
            layer.setShadowOffset(objc2_foundation::NSSize::new(0.0, -1.0));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().down_at.set(Some(event.locationInWindow()));
            self.ivars().dragging.set(false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let iv = self.ivars();
            let (Some(start), false) = (iv.down_at.get(), iv.dragging.get()) else {
                return;
            };
            let p = event.locationInWindow();
            if (p.x - start.x).hypot(p.y - start.y) < 4.0 {
                return;
            }
            iv.dragging.set(true);
            self.begin_drag(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            let iv = self.ivars();
            if iv.down_at.take().is_some() && !iv.dragging.get() {
                self.send(CardMsg::Click);
            }
        }
    }

    unsafe impl NSObjectProtocol for CardView {}

    unsafe impl NSDraggingSource for CardView {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Move
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn dragging_ended(
            &self,
            _session: &NSDraggingSession,
            _point: NSPoint,
            _operation: NSDragOperation,
        ) {
            self.ivars().dragging.set(false);
            self.send(CardMsg::DragEnded);
        }
    }
);

impl CardView {
    pub fn new(mtm: MainThreadMarker, id: TaskId, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CardIvars {
            id,
            tracking: Cell::new(false),
            on_msg: RefCell::new(None),
            down_at: Cell::new(None),
            dragging: Cell::new(false),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn send(&self, msg: CardMsg) {
        if let Some(f) = self.ivars().on_msg.borrow().as_ref() {
            f(self.ivars().id, msg);
        }
    }

    /// Starts a system drag with a snapshot of the card under the cursor.
    fn begin_drag(&self, event: &NSEvent) {
        let bounds = self.bounds();
        let image = NSImage::initWithSize(NSImage::alloc(), bounds.size);
        if let Some(rep) = self.bitmapImageRepForCachingDisplayInRect(bounds) {
            self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
            image.addRepresentation(&rep);
        }
        let id = NSString::from_str(&self.ivars().id.to_string());
        let item = NSDraggingItem::initWithPasteboardWriter(
            NSDraggingItem::alloc(),
            ProtocolObject::from_ref(&*id),
        );
        unsafe { item.setDraggingFrame_contents(bounds, Some(&image)) };
        self.send(CardMsg::DragStarted);
        self.beginDraggingSessionWithItems_event_source(
            &NSArray::from_retained_slice(&[item]),
            event,
            ProtocolObject::from_ref(self),
        );
    }
}

/// What a column reports while something is dragged over it. Points are in the column's coordinates.
pub enum DropMsg {
    Over(NSPoint),
    Exit,
    Drop(NSPoint),
}

pub struct ColumnIvars {
    /// A card is dragged over this column.
    pub highlight: Cell<bool>,
    pub on_drop: RefCell<Option<DropHandler>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ColumnIvars]
    pub struct ColumnView;

    impl ColumnView {
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
            layer.setCornerRadius(12.0);
            let fill = if self.ivars().highlight.get() {
                NSColor::tertiaryLabelColor()
            } else {
                NSColor::quaternaryLabelColor()
            };
            layer.setBackgroundColor(Some(&fill.colorWithAlphaComponent(0.35).CGColor()));
        }
    }

    unsafe impl NSObjectProtocol for ColumnView {}

    unsafe impl NSDraggingDestination for ColumnView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.over(info)
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.over(info)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.send(DropMsg::Exit);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let p = self.convertPoint_fromView(info.draggingLocation(), None);
            self.send(DropMsg::Drop(p))
        }
    }
);

impl ColumnView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ColumnIvars {
            highlight: Cell::new(false),
            on_drop: RefCell::new(None),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeString }]));
        view
    }

    fn over(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        let p = self.convertPoint_fromView(info.draggingLocation(), None);
        if self.send(DropMsg::Over(p)) {
            NSDragOperation::Move
        } else {
            NSDragOperation::None
        }
    }

    fn send(&self, msg: DropMsg) -> bool {
        self.ivars()
            .on_drop
            .borrow()
            .as_ref()
            .is_some_and(|f| f(msg))
    }
}

/// The window's current first responder is a text editor (a field being edited or a text view).
pub fn editing_text(window: &NSWindow) -> bool {
    window
        .firstResponder()
        .is_some_and(|r| r.isKindOfClass(objc2_app_kit::NSText::class()))
}
