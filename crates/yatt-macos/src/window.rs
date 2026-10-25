//! The main window, drawn with AppKit. [`MainWindow::render`] diffs a [`Snapshot`]
//! against the views it already has: cards are kept per task id, updated in place
//! and moved with animations, so reorders and column moves stay smooth.
//!
//! Layout: the timer row in a native toolbar unified with the titlebar (a
//! progress ring, the digits, the phase, the controls), then the board split into
//! three columns by
//! full-height 1 px dividers. Each column's title and count sit in a glass chip in
//! the column's colour, floating over the cards; one header button switches the
//! whole board between lists and stacks.
//! The new-task field floats over the bottom of the board as a glass capsule.
//! A click on a card opens the task modal (`modal.rs`); minimal mode hides the
//! toolbar and shrinks the window to the timer row alone, at a larger size.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::{RcBlock, StackBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::NSStringDrawing;
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSControlSize, NSEvent,
    NSEventModifierFlags, NSEventType, NSFloatingWindowLevel, NSFocusRingType, NSFont,
    NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSLineBreakMode,
    NSNormalWindowLevel, NSPasteboard, NSPasteboardTypeString, NSTextAlignment, NSTextField,
    NSToolbar, NSToolbarDisplayMode, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarSpaceItemIdentifier, NSView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
    NSWindowTitleVisibility, NSWindowToolbarStyle,
};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{NSArray, NSNumber, NSObjectNSKeyValueCoding, NSValue};
use objc2_foundation::{NSCopying, NSPoint, NSRect, NSSize, NSString, ns_string};
use objc2_quartz_core::{
    CABasicAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction, CAShapeLayer,
    CASpringAnimation, NSValueCATransform3DAdditions, kCALineCapRound,
};
use yatt_core::{Status, TaskId};

pub use crate::board::DemoPointer;
use crate::board::{Board, ColumnGeom};

/// A real mouse event for the demo script to send through the window.
#[derive(Debug, Clone, Copy)]
pub enum DemoMouse {
    Down,
    Drag,
    Up,
}
use crate::classes::{
    ActionTarget, EditCmd, FieldDelegate, Fill, FillView, FlippedView, PassView, PlainToolbarItem,
    RootView, ToolbarDelegate, WindowDelegate, editing_in_web_view, editing_text,
};
use crate::modal::Modal;
use crate::model::{Snapshot, TimerSnap, UiEvent, format_clock};
use crate::motion;
use crate::style;

pub type Sink = Rc<dyn Fn(UiEvent)>;

/// The toolbar's timer item: the ring and five digits.
const TIMER_ITEM_W: f64 = 112.0;
/// Height of the timer row in the toolbar.
const TOOLBAR_H: f64 = 40.0;
/// The progress ring: diameter and stroke, in the toolbar and in minimal mode.
const RING_D: f64 = 22.0;
const RING_D_MINI: f64 = 34.0;
const RING_LINE: f64 = 3.0;
const PAD: f64 = 16.0;
/// Column chips: height and distance from the top of the board.
const CHIP_H: f64 = 20.0;
const CHIP_TOP: f64 = 12.0;
/// The new-task capsule.
const CAPSULE_H: f64 = 44.0;
const CAPSULE_MAX_W: f64 = 460.0;
const CAPSULE_BOTTOM: f64 = 22.0;
/// The clock's digits in minimal mode: font size, the step from one digit to the
/// next and from a colon on (tabular figures, tracked tight), and height.
const DIGITS_MINI: (f64, f64, f64, f64) = (46.0, 26.0, 12.0, 56.0);
/// The same in the toolbar.
const DIGITS_BAR: (f64, f64, f64, f64) = (24.0, 14.0, 7.0, 32.0);
/// Each digit's label is this much wider than its step, so the glyph is never
/// clipped by the field's inset; neighbouring labels overlap, glyphs don't.
const DIGIT_PAD: f64 = 6.0;
/// Minimal mode: just the timer.
const MINI_SIZE: (f64, f64) = (300.0, 136.0);

pub struct MainWindow {
    ui: Rc<Ui>,
}

struct Ui {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    root: Retained<RootView>,
    sink: Sink,
    header: Header,
    toolbar: Retained<NSToolbar>,
    /// The toolbar's one item, holding the header's view outside minimal mode.
    timer_item: Retained<NSToolbarItem>,
    /// Height of the titlebar with the toolbar: where the board starts.
    top_inset: Cell<f64>,
    /// Everything below the header; hidden in minimal mode.
    board: Retained<FlippedView>,
    header_rule: Retained<FillView>,
    dividers: [Retained<FillView>; 2],
    columns: [Column; 3],
    /// Holds the column chips above the cards; clicks elsewhere fall through.
    heads: Retained<PassView>,
    capsule: Retained<NSView>,
    new_task: Retained<NSTextField>,
    cards: Rc<Board>,
    modal: Modal,
    minimal: Cell<bool>,
    /// The window frame to go back to when leaving minimal mode.
    saved_frame: Cell<NSRect>,
    last: RefCell<Option<Snapshot>>,
    /// Copied cards, `(title, notes)`, and the text put on the pasteboard with them.
    clipboard: RefCell<(Vec<(String, String)>, String)>,
    // Kept alive for as long as the window: AppKit holds targets and delegates weakly.
    _retained: RefCell<Vec<Retained<AnyObject>>>,
}

struct Header {
    view: Retained<FlippedView>,
    digits_box: Retained<FlippedView>,
    digits: RefCell<Vec<Retained<NSTextField>>>,
    shown_clock: RefCell<String>,
    phase: Retained<NSTextField>,
    tasks: Retained<NSTextField>,
    primary: Retained<NSButton>,
    stopwatch: Retained<NSButton>,
    skip: Retained<NSButton>,
    stop: Retained<NSButton>,
    mini: Retained<NSButton>,
    /// Lists or stacks, for the whole board.
    stack: Retained<NSButton>,
    ring: Ring,
    /// The same controls as native toolbar items.
    bar: Bar,
    minimal: Cell<bool>,
}

struct Column {
    view: Retained<FlippedView>,
    tint: Retained<FillView>,
    chip: Retained<FillView>,
    title: Retained<NSTextField>,
    count: Retained<NSTextField>,
    highlighted: Cell<bool>,
}

impl MainWindow {
    pub fn new(mtm: MainThreadMarker, sink: Sink) -> Self {
        // Room for three columns of cards, no more; the system keeps the size the
        // window had last time.
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(980.0, 640.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("yatt"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setMinSize(NSSize::new(760.0, 480.0));

        // Opaque and theme-aware: pure white in light mode, the system's dark
        // text background in dark mode.
        window.setOpaque(true);
        window.setBackgroundColor(Some(&NSColor::textBackgroundColor()));
        let root = RootView::new(mtm, frame);
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        window.setContentView(Some(&root));

        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        retained.extend(crate::eventlog::install());
        let header = Header::new(mtm, &sink, &mut retained);
        // The timer row lives in a native toolbar, unified with the titlebar.
        let timer_item = PlainToolbarItem::item(mtm, ns_string!("yatt.timer"));
        timer_item.setView(Some(&header.view));
        // Plain text in the toolbar: no bubble (see `PlainToolbarItem`).
        timer_item.setBordered(false);
        #[allow(deprecated)]
        {
            timer_item.setMinSize(NSSize::new(TIMER_ITEM_W, TOOLBAR_H));
            timer_item.setMaxSize(NSSize::new(TIMER_ITEM_W, TOOLBAR_H));
        }
        let toolbar =
            NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), ns_string!("yatt.toolbar"));
        // [ring, digits, phase] … [Старт] [Секундомер] [Пропустить] [Стоп] [▤ ⤡]
        let bar = &header.bar;
        let items: Vec<Retained<NSToolbarItem>> = vec![
            timer_item.clone(),
            bar.start.clone(),
            bar.stopwatch.clone(),
            bar.skip.clone(),
            bar.stop.clone(),
            bar.stack.clone(),
            bar.mini.clone(),
        ];
        // The toolbar gives neighbouring items one shared bubble; a fixed space
        // between two items gives each its own.
        let id = |item: &Retained<NSToolbarItem>| item.itemIdentifier();
        let flexible = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
        let space = unsafe { NSToolbarSpaceItemIdentifier }.copy();
        let order: Vec<Retained<NSString>> = vec![
            id(&items[0]),
            flexible,
            id(&items[1]),
            space.clone(),
            id(&items[2]),
            space.clone(),
            id(&items[3]),
            space.clone(),
            id(&items[4]),
            space,
            id(&items[5]),
            id(&items[6]),
        ];
        let toolbar_delegate = ToolbarDelegate::new(mtm, items, order);
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_delegate)));
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.setAllowsUserCustomization(false);
        window.setToolbar(Some(&toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        retained.push(Retained::into_super(Retained::into_super(toolbar_delegate)));

        let board = FlippedView::new(mtm, NSRect::ZERO);
        root.addSubview(&board);
        let header_rule = FillView::new(mtm, Fill::Separator);
        board.addSubview(&header_rule);

        let columns = [0, 1, 2].map(|i| Column::new(mtm, i));
        for c in &columns {
            board.addSubview(&c.view);
        }
        let dividers = [
            FillView::new(mtm, Fill::Separator),
            FillView::new(mtm, Fill::Separator),
        ];
        for d in &dividers {
            board.addSubview(d);
        }
        // The board tints the column under a dragged card through a weak handle,
        // filled in once `Ui` exists.
        let tint_target: Rc<RefCell<Option<std::rc::Weak<Ui>>>> = Rc::new(RefCell::new(None));
        let cards = {
            let tint_target = tint_target.clone();
            Board::new(mtm, sink.clone(), move |col| {
                if let Some(ui) = tint_target.borrow().as_ref().and_then(|w| w.upgrade()) {
                    for i in 0..3 {
                        ui.set_highlight(i, Some(i) == col);
                    }
                }
            })
        };
        board.addSubview(&cards.canvas);
        let heads = PassView::new(mtm);
        for c in &columns {
            heads.addSubview(&c.chip);
        }
        board.addSubview(&heads);

        // The new-task capsule floats over the cards, under the dragged ones.
        let (capsule, capsule_content) = style::glass(mtm, CAPSULE_H / 2.0);
        let new_task = NSTextField::textFieldWithString(ns_string!(""), mtm);
        new_task.setPlaceholderString(Some(ns_string!(
            "Новая задача  ·  ↩ в бэклог  ·  ⌘↩ в работу"
        )));
        new_task.setBordered(false);
        new_task.setBezeled(false);
        new_task.setDrawsBackground(false);
        new_task.setFocusRingType(NSFocusRingType::None);
        new_task.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        capsule_content.addSubview(&new_task);
        board.addSubview(&capsule);
        board.addSubview(&cards.overlay);

        let modal = Modal::new(mtm, &sink);
        root.addSubview(&modal.dim);
        root.addSubview(&modal.panel);

        let ui = Rc::new(Ui {
            mtm,
            window,
            root,
            sink: sink.clone(),
            header,
            toolbar,
            timer_item,
            top_inset: Cell::new(52.0),
            board,
            header_rule,
            dividers,
            columns,
            heads,
            capsule,
            new_task,
            cards,
            modal,
            minimal: Cell::new(false),
            saved_frame: Cell::new(NSRect::ZERO),
            last: RefCell::new(None),
            clipboard: RefCell::default(),
            _retained: RefCell::new(retained),
        });
        *tint_target.borrow_mut() = Some(Rc::downgrade(&ui));
        Ui::wire(&ui);

        let delegate = WindowDelegate::new(mtm, move || sink(UiEvent::Quit));
        ui.window
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        ui._retained
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(delegate)));

        ui.layout();
        ui.window.center();
        ui.window.makeKeyAndOrderFront(None);
        ui.window.makeFirstResponder(Some(&ui.root));
        Self { ui }
    }

    pub fn render(&self, snap: &Snapshot) {
        self.ui.render(snap);
    }

    /// cmd-N: put the cursor into the new-task field.
    pub fn focus_new_task(&self) {
        if self.ui.minimal.get() {
            self.ui.toggle_minimal();
        }
        self.ui.window.makeFirstResponder(Some(&self.ui.new_task));
    }

    /// cmd-shift-M: the timer alone in a small floating window, or back to the board.
    pub fn toggle_minimal(&self) {
        self.ui.toggle_minimal();
    }

    pub fn is_minimal(&self) -> bool {
        self.ui.minimal.get()
    }

    /// cmd-Z while typing undoes the typing, not the board. Returns true when it did.
    pub fn text_undo(&self, redo: bool) -> bool {
        if !editing_text(&self.ui.window) {
            return false;
        }
        // The notes editor keeps its own history.
        if self.ui.modal.open_task().is_some() && editing_in_web_view(&self.ui.window) {
            self.ui.modal.editor_undo(redo);
            return true;
        }
        let action = if redo { sel!(redo:) } else { sel!(undo:) };
        unsafe {
            NSApplication::sharedApplication(self.ui.mtm).sendAction_to_from(action, None, None);
        }
        true
    }

    /// Development aid: synthetic pointer input for the demo script, in canvas
    /// coordinates (y down from the top of the cards area).
    pub fn demo_pointer(&self, input: DemoPointer) {
        self.ui.cards.demo_pointer(input);
    }

    /// Development aid: the centre of a card, in canvas coordinates.
    pub fn demo_center(&self, id: TaskId) -> Option<(f64, f64)> {
        self.ui.cards.demo_center(id)
    }

    /// Development aid: the horizontal centre of a column, in canvas coordinates.
    pub fn demo_column_x(&self, column: usize) -> f64 {
        self.ui.cards.demo_column_x(column)
    }

    /// Development aid: resizes the window's content, keeping its top-left corner.
    pub fn demo_resize(&self, w: f64, h: f64) {
        let f = self.ui.window.frame();
        let content = self.ui.window.contentRectForFrameRect(f);
        let frame = self.ui.window.frameRectForContentRect(NSRect::new(
            NSPoint::new(content.origin.x, content.origin.y + content.size.height - h),
            NSSize::new(w, h),
        ));
        self.ui.window.setFrame_display(frame, true);
    }

    /// Development aid: a real click at `at` (canvas coordinates), sent through the
    /// window like a mouse click, so it goes through hit-testing. `clicks` is the
    /// click count the event carries.
    pub fn demo_real_click(&self, at: (f64, f64), clicks: isize) {
        let canvas = &self.ui.cards.canvas;
        let p = canvas.convertPoint_toView(NSPoint::new(at.0, at.1), None);
        let window = &self.ui.window;
        let number = window.windowNumber();
        for kind in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                p,
                NSEventModifierFlags::empty(),
                objc2_quartz_core::CACurrentMediaTime(),
                number,
                None,
                0,
                clicks,
                1.0,
            );
            if let Some(event) = event {
                // Through the application, as the system delivers them.
                NSApplication::sharedApplication(self.ui.mtm).sendEvent(&event);
            }
        }
    }

    /// Development aid: scrolls a column's list to `y`.
    pub fn demo_scroll(&self, column: usize, y: f64) {
        self.ui.cards.demo_scroll(column, y);
    }

    /// Development aid: where a card's view is drawn, its centre in canvas
    /// coordinates.
    pub fn demo_view_center(&self, id: TaskId) -> Option<(f64, f64)> {
        self.ui.cards.demo_view_center(id)
    }

    /// Development aid: selects exactly these cards.
    pub fn demo_select(&self, ids: &[TaskId]) {
        self.ui.cards.demo_select(ids);
    }

    /// Development aid: one real mouse event of `kind` at `at` (canvas
    /// coordinates), sent through the window like the system's.
    pub fn demo_real_mouse(&self, kind: DemoMouse, at: (f64, f64)) {
        let canvas = &self.ui.cards.canvas;
        let p = canvas.convertPoint_toView(NSPoint::new(at.0, at.1), None);
        let window = &self.ui.window;
        let kind = match kind {
            DemoMouse::Down => NSEventType::LeftMouseDown,
            DemoMouse::Drag => NSEventType::LeftMouseDragged,
            DemoMouse::Up => NSEventType::LeftMouseUp,
        };
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            p,
            NSEventModifierFlags::empty(),
            objc2_quartz_core::CACurrentMediaTime(),
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        );
        if let Some(event) = event {
            NSApplication::sharedApplication(self.ui.mtm).sendEvent(&event);
        }
    }

    /// Development aid: true while a stack is fanned out.
    pub fn demo_is_fanned(&self) -> bool {
        self.ui.cards.is_fanned()
    }

    /// Development aid: the board's list/stack toggle.
    pub fn demo_toggle_stack(&self) {
        self.ui.toggle_stack();
    }

    /// Development aid: types a task into the new-task field and submits it.
    pub fn demo_new_task(&self, title: &str, in_progress: bool) {
        self.ui.new_task.setStringValue(&NSString::from_str(title));
        self.ui.submit_new_task(in_progress);
    }

    /// Development aid: prints the notes editor's rendered HTML to stderr.
    pub fn debug_print_editor(&self) {
        self.ui.modal.debug_print_editor();
    }

    /// Draws the window's content into a PNG, without screen-recording permission.
    /// Web content (the notes editor) does not show up; native views do.
    pub fn capture_png(&self, path: &str) -> bool {
        let Some(content) = self.ui.window.contentView() else {
            return false;
        };
        // The frame view holds the toolbar too.
        let view = unsafe { content.superview() }.unwrap_or(content);
        let bounds = view.bounds();
        let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
            return false;
        };
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = objc2_foundation::NSDictionary::new();
        let Some(data) = (unsafe {
            rep.representationUsingType_properties(
                objc2_app_kit::NSBitmapImageFileType::PNG,
                &props,
            )
        }) else {
            return false;
        };
        data.writeToFile_atomically(&NSString::from_str(path), true)
    }
}

impl Ui {
    /// Hooks AppKit callbacks (resize, keys, drops) up to this object.
    fn wire(this: &Rc<Self>) {
        let weak = Rc::downgrade(this);
        *this.root.ivars().on_layout.borrow_mut() = Some(Box::new({
            let weak = weak.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    ui.layout();
                }
            }
        }));
        *this.root.ivars().on_key.borrow_mut() = Some(Box::new({
            let weak = weak.clone();
            move |event: &NSEvent| weak.upgrade().is_some_and(|ui| ui.key_down(event))
        }));
        *this.root.ivars().on_edit.borrow_mut() = Some(Box::new({
            let weak = weak.clone();
            move |cmd| {
                if let Some(ui) = weak.upgrade() {
                    ui.edit(cmd);
                }
            }
        }));
        *this.root.ivars().on_cancel.borrow_mut() = Some(Box::new({
            let weak = weak.clone();
            move || {
                let Some(ui) = weak.upgrade() else { return };
                if let Some(id) = ui.modal.open_task() {
                    (ui.sink)(UiEvent::ToggleExpand(id));
                } else {
                    ui.cards.clear_selection();
                }
            }
        }));
        let target = ActionTarget::new(this.mtm, {
            let weak = weak.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    ui.toggle_minimal();
                }
            }
        });
        unsafe {
            this.header.mini.setTarget(Some(&target));
            this.header.mini.setAction(Some(ActionTarget::sel()));
            this.header.bar.mini.setTarget(Some(&target));
            this.header.bar.mini.setAction(Some(ActionTarget::sel()));
        }
        this._retained
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(target)));
        let target = ActionTarget::new(this.mtm, {
            let weak = weak.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    ui.toggle_stack();
                }
            }
        });
        unsafe {
            this.header.stack.setTarget(Some(&target));
            this.header.stack.setAction(Some(ActionTarget::sel()));
            this.header.bar.stack.setTarget(Some(&target));
            this.header.bar.stack.setAction(Some(ActionTarget::sel()));
        }
        this._retained
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(target)));

        // Return files the task into the backlog, ⌘Return straight into "В работе".
        let delegate = FieldDelegate::new(this.mtm, {
            let weak = weak.clone();
            move |command| {
                let Some(ui) = weak.upgrade() else {
                    return false;
                };
                let app = NSApplication::sharedApplication(ui.mtm);
                let Some(event) = app.currentEvent() else {
                    return false;
                };
                let is_return = matches!(event.keyCode(), 36 | 76);
                if !is_return || (command != sel!(insertNewline:) && command != sel!(noop:)) {
                    return false;
                }
                let cmd = event
                    .modifierFlags()
                    .contains(NSEventModifierFlags::Command);
                ui.submit_new_task(cmd);
                true
            }
        });
        unsafe {
            this.new_task
                .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        this._retained
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(delegate)));
    }

    fn submit_new_task(&self, in_progress: bool) {
        let title = self.new_task.stringValue().to_string();
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        let status = if in_progress {
            Status::InProgress
        } else {
            Status::Backlog
        };
        // The capsule itself, typed text and all, becomes the card and flies to its
        // column; the field is left empty behind it.
        let at = self
            .cards
            .canvas
            .convertRect_fromView(self.capsule.frame(), Some(&self.board));
        self.cards.spawn_from(at, status);
        (self.sink)(UiEvent::NewTask {
            title: title.to_string(),
            status,
        });
        self.new_task.setStringValue(ns_string!(""));
    }

    fn toggle_stack(&self) {
        self.cards.toggle_stack();
        self.header.update_stack_icon(self.cards.is_stacked());
    }

    fn key_down(&self, event: &NSEvent) -> bool {
        let mods = event.modifierFlags()
            & (NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option);
        if !mods.is_empty() {
            return false;
        }
        match event.keyCode() {
            // Space
            49 => {
                (self.sink)(UiEvent::StartPause);
                true
            }
            // Backspace and forward delete remove the selected cards.
            51 | 117 if !self.cards.selection().is_empty() => {
                self.edit(EditCmd::Delete);
                true
            }
            _ => false,
        }
    }

    /// Copy, cut, paste, select all and delete on the board's selection. Copied
    /// cards also go on the pasteboard as text, one title a line; pasting text
    /// copied elsewhere makes a card of each line.
    fn edit(&self, cmd: EditCmd) {
        let selection = self.cards.selection();
        let status = Status::ALL[self.cards.focused_column().unwrap_or(0)];
        let pasteboard = NSPasteboard::generalPasteboard();
        match cmd {
            EditCmd::Copy | EditCmd::Cut => {
                if selection.is_empty() {
                    return;
                }
                let items: Vec<(String, String)> = {
                    let last = self.last.borrow();
                    let cards: Vec<&crate::model::CardSnap> = last
                        .iter()
                        .flat_map(|s| s.columns.iter())
                        .flat_map(|c| c.cards.iter())
                        .collect();
                    selection
                        .iter()
                        .filter_map(|id| cards.iter().find(|c| c.id == *id))
                        .map(|c| (c.title.clone(), c.notes.clone()))
                        .collect()
                };
                let text = items
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                pasteboard.clearContents();
                unsafe {
                    pasteboard.setString_forType(&NSString::from_str(&text), NSPasteboardTypeString)
                };
                *self.clipboard.borrow_mut() = (items, text);
                if cmd == EditCmd::Cut {
                    let deletes = selection.into_iter().map(UiEvent::Delete).collect();
                    (self.sink)(UiEvent::Group(deletes));
                }
            }
            EditCmd::Paste => {
                let text = unsafe { pasteboard.stringForType(NSPasteboardTypeString) }
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let clipboard = self.clipboard.borrow();
                let items = if !clipboard.0.is_empty() && clipboard.1 == text {
                    clipboard.0.clone()
                } else {
                    text.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(|l| (l.to_string(), String::new()))
                        .collect()
                };
                if !items.is_empty() {
                    (self.sink)(UiEvent::Paste { items, status });
                }
            }
            EditCmd::SelectAll => self.cards.select_all(self.cards.focused_column()),
            EditCmd::Delete => {
                let deletes = selection.into_iter().map(UiEvent::Delete).collect();
                (self.sink)(UiEvent::Group(deletes));
                self.cards.clear_selection();
            }
        }
    }

    // ---- layout ----

    fn layout(&self) {
        let bounds = self.root.bounds();
        let size = bounds.size;
        self.modal.layout(bounds);
        if self.minimal.get() {
            self.header.view.setFrame(NSRect::new(NSPoint::ZERO, size));
            self.header.layout(size.width, size.height);
            return;
        }
        let row = self.header.view.frame().size;
        self.header.layout(row.width, row.height);

        // The board starts where the toolbar ends (the content runs under it).
        // `contentLayoutRect` lags behind while the window resizes; between
        // toolbar changes the inset stays the same, so an odd reading is ignored.
        let inset = size.height - self.window.contentLayoutRect().size.height;
        if inset > 20.0 && inset < TOOLBAR_H + 40.0 {
            self.top_inset.set(inset);
        }
        let top = self.top_inset.get();
        let board_h = (size.height - top).max(100.0);
        self.board.setFrame(NSRect::new(
            NSPoint::new(0.0, top),
            NSSize::new(size.width, board_h),
        ));
        self.header_rule
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(size.width, 1.0)));
        // The cards run under the column chips, so the canvas starts right below the rule.
        let canvas_h = board_h - 1.0;
        self.cards.canvas.setFrame(NSRect::new(
            NSPoint::new(0.0, 1.0),
            NSSize::new(size.width, canvas_h),
        ));
        let full = NSRect::new(NSPoint::ZERO, NSSize::new(size.width, board_h));
        self.cards.overlay.setFrame(full);
        self.heads.setFrame(full);
        let cw = (size.width - 80.0).min(CAPSULE_MAX_W);
        self.capsule.setFrame(NSRect::new(
            NSPoint::new(
                ((size.width - cw) / 2.0).round(),
                board_h - CAPSULE_BOTTOM - CAPSULE_H,
            ),
            NSSize::new(cw, CAPSULE_H),
        ));
        let field_h = 20.0;
        self.new_task.setFrame(NSRect::new(
            NSPoint::new(20.0, ((CAPSULE_H - field_h) / 2.0).round()),
            NSSize::new(cw - 40.0, field_h),
        ));
        let col_w = (size.width / 3.0).floor();
        let mut geom = [ColumnGeom::default(); 3];
        for (i, c) in self.columns.iter().enumerate() {
            let x = i as f64 * col_w;
            let w = if i == 2 { size.width - x } else { col_w };
            c.view.setFrame(NSRect::new(
                NSPoint::new(x, 1.0),
                NSSize::new(w, board_h - 1.0),
            ));
            c.tint.setFrame(c.view.bounds());
            c.layout_chip(x + PAD);
            if i < 2 {
                self.dividers[i].setFrame(NSRect::new(
                    NSPoint::new(x + w - 0.5, 1.0),
                    NSSize::new(1.0, board_h),
                ));
            }
            geom[i] = ColumnGeom { x, w };
        }
        self.cards.set_geometry(geom, canvas_h, 1.0);
    }

    fn set_highlight(&self, index: usize, on: bool) {
        let c = &self.columns[index];
        if c.highlighted.replace(on) == on {
            return;
        }
        let tint = c.tint.clone();
        let alpha = if on { 1.0 } else { 0.0 };
        animate(true, motion::COLUMN_TINT_SECS, move || {
            tint.animator().setAlphaValue(alpha)
        });
    }

    // ---- minimal mode ----

    fn toggle_minimal(&self) {
        let to_minimal = !self.minimal.get();
        if to_minimal {
            if let Some(id) = self.modal.open_task() {
                (self.sink)(UiEvent::ToggleExpand(id));
            }
            let frame = self.window.frame();
            self.saved_frame.set(frame);
            self.minimal.set(true);
            self.header.minimal.set(true);
            self.board.setHidden(true);
            // The timer row leaves the toolbar for the window itself.
            self.toolbar.setVisible(false);
            self.timer_item.setView(None);
            self.header.view.removeFromSuperview();
            self.root.addSubview_positioned_relativeTo(
                &self.header.view,
                NSWindowOrderingMode::Below,
                Some(&self.modal.dim),
            );
            self.header.reset_clock();
            self.window
                .setMinSize(NSSize::new(MINI_SIZE.0, MINI_SIZE.1));
            // Keep the top-left corner where it is.
            let target = NSRect::new(
                NSPoint::new(
                    frame.origin.x,
                    frame.origin.y + frame.size.height - MINI_SIZE.1,
                ),
                NSSize::new(MINI_SIZE.0, MINI_SIZE.1),
            );
            self.window
                .setFrame_display_animate(target, true, !motion::reduce_motion());
            self.window.setLevel(NSFloatingWindowLevel);
        } else {
            self.minimal.set(false);
            self.header.minimal.set(false);
            self.header.view.removeFromSuperview();
            self.timer_item.setView(Some(&self.header.view));
            self.toolbar.setVisible(true);
            self.header.reset_clock();
            self.window.setLevel(NSNormalWindowLevel);
            self.window.setFrame_display_animate(
                self.saved_frame.get(),
                true,
                !motion::reduce_motion(),
            );
            self.window.setMinSize(NSSize::new(760.0, 480.0));
            self.board.setAlphaValue(0.0);
            self.board.setHidden(false);
            let board = self.board.clone();
            animate(true, motion::APPEAR_SECS, move || {
                board.animator().setAlphaValue(1.0)
            });
        }
        self.header.update_mini_icon();
        self.layout();
        if let Some(snap) = self.last.borrow().as_ref() {
            self.header.update(&snap.timer);
        }
    }

    // ---- render ----

    fn render(self: &Rc<Self>, snap: &Snapshot) {
        if self.last.borrow().as_ref() == Some(snap) {
            return;
        }
        self.header.update(&snap.timer);
        for (c, col) in self.columns.iter().zip(&snap.columns) {
            c.title.setStringValue(&NSString::from_str(&col.title));
            let count = col.cards.len().to_string();
            if c.count.stringValue().to_string() != count {
                motion::soften(&c.chip);
                c.count.setStringValue(&NSString::from_str(&count));
            }
            c.layout_chip(c.view.frame().origin.x + PAD);
        }
        self.cards.render(&snap.columns);
        self.sync_modal(snap);
        *self.last.borrow_mut() = Some(snap.clone());
    }

    /// Opens, closes or refreshes the modal to match `snap.expanded`.
    fn sync_modal(self: &Rc<Self>, snap: &Snapshot) {
        let open = self.modal.open_task();
        let want = snap.expanded;
        let find = |id: TaskId| {
            snap.columns.iter().find_map(|c| {
                c.cards
                    .iter()
                    .find(|card| card.id == id)
                    .map(|card| (card.clone(), c.status))
            })
        };
        if open == want {
            if let Some((card, status)) = want.and_then(find) {
                self.modal.update(&card, status);
            }
            return;
        }
        if let Some(id) = open {
            let to = self.card_rect_in_root(id);
            // The panel shrinks back onto the card with the card's text on it; the
            // card itself shows again when the panel lies exactly on it.
            let weak = Rc::downgrade(self);
            self.modal.close(to, self.cards.card_angle(id), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.cards.set_hidden(None);
                }
            });
            self.window.makeFirstResponder(Some(&self.root));
        }
        if let Some((card, status)) = want.and_then(find) {
            let from = self
                .card_rect_in_root(card.id)
                .unwrap_or_else(|| Modal::target_frame(self.root.bounds()));
            self.cards.set_hidden(Some(card.id));
            let deg = self.cards.card_angle(card.id);
            self.modal
                .open(&card, status, from, deg, self.root.bounds());
        }
    }

    /// Where a card is on screen, in the root view's coordinates.
    fn card_rect_in_root(&self, id: TaskId) -> Option<NSRect> {
        let rect = self.cards.card_rect(id)?;
        Some(
            self.root
                .convertRect_fromView(rect, Some(&self.cards.canvas)),
        )
    }
}

impl Header {
    fn new(mtm: MainThreadMarker, sink: &Sink, retained: &mut Vec<Retained<AnyObject>>) -> Self {
        let view = FlippedView::new(mtm, NSRect::ZERO);
        let digits_box = FlippedView::new(mtm, NSRect::ZERO);
        view.addSubview(&digits_box);

        let phase = label(mtm, "", 15.0, unsafe { NSFontWeightSemibold });
        let tasks = label(mtm, "", 12.0, unsafe { NSFontWeightRegular });
        tasks.setTextColor(Some(&NSColor::secondaryLabelColor()));
        tasks.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        view.addSubview(&phase);
        view.addSubview(&tasks);

        // Each control is both a button (minimal mode) and a native toolbar item of
        // its own, so the toolbar gives it its own bubble.
        let mut items: Vec<Retained<NSToolbarItem>> = Vec::new();
        let mut button = |title: &str, id: &str, event: UiEvent| {
            let sink = sink.clone();
            let target = ActionTarget::new(mtm, move || sink(event.clone()));
            let item = NSToolbarItem::initWithItemIdentifier(
                NSToolbarItem::alloc(mtm),
                &NSString::from_str(id),
            );
            item.setTitle(&NSString::from_str(title));
            item.setLabel(&NSString::from_str(title));
            unsafe {
                item.setTarget(Some(&target));
                item.setAction(Some(ActionTarget::sel()));
            }
            items.push(item);
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setBezelStyle(NSBezelStyle::Push);
            b.setControlSize(NSControlSize::Regular);
            view.addSubview(&b);
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };
        let primary = button("Старт", "yatt.start", UiEvent::StartPause);
        primary.setBezelColor(Some(&NSColor::controlAccentColor()));
        let stopwatch = button("Секундомер", "yatt.stopwatch", UiEvent::StartStopwatch);
        let skip = button("Пропустить", "yatt.skip", UiEvent::Skip);
        let stop = button("Стоп", "yatt.stop", UiEvent::Stop);
        let [start_item, stopwatch_item, skip_item, stop_item]: [Retained<NSToolbarItem>; 4] =
            items.try_into().expect("four control items");
        // Icons, not words: ▶ starts the stopwatch; the main button's icon follows
        // the timer (`primary_icon`).
        stopwatch_item.setTitle(ns_string!(""));
        stopwatch_item.setImage(symbol("play.fill", "Секундомер").as_deref());
        stopwatch_item.setToolTip(Some(ns_string!("Секундомер")));
        stopwatch.setTitle(ns_string!(""));
        stopwatch.setImage(symbol("play.fill", "Секундомер").as_deref());
        start_item.setTitle(ns_string!(""));
        // The view switches: two icons side by side, which the toolbar puts in one
        // shared bubble. Their targets are set in `Ui::wire`.
        let icon_item = |id: &str| {
            NSToolbarItem::initWithItemIdentifier(
                NSToolbarItem::alloc(mtm),
                &NSString::from_str(id),
            )
        };
        let stack_item = icon_item("yatt.stack");
        let mini_item = icon_item("yatt.mini");

        // Target and action are set in `Ui::wire`: minimal mode is window state, not an event.
        let mini =
            unsafe { NSButton::buttonWithTitle_target_action(ns_string!(""), None, None, mtm) };
        mini.setBordered(false);
        mini.setToolTip(Some(ns_string!("Только таймер (⇧⌘M)")));
        view.addSubview(&mini);
        // Target and action are set in `Ui::wire`: the layout is board state, not an event.
        let stack =
            unsafe { NSButton::buttonWithTitle_target_action(ns_string!(""), None, None, mtm) };
        stack.setBordered(false);
        stack.setWantsLayer(true);
        view.addSubview(&stack);

        let ring = Ring::new(mtm);
        view.addSubview(&ring.view);

        let header = Self {
            view,
            digits_box,
            digits: RefCell::new(Vec::new()),
            shown_clock: RefCell::new(String::new()),
            phase,
            tasks,
            primary,
            stopwatch,
            skip,
            stop,
            mini,
            stack,
            ring,
            bar: Bar {
                start: start_item,
                stopwatch: stopwatch_item,
                skip: skip_item,
                stop: stop_item,
                stack: stack_item,
                mini: mini_item,
            },
            minimal: Cell::new(false),
        };
        header.update_mini_icon();
        header.update_stack_icon(false);
        header
    }

    fn update_stack_icon(&self, stacked: bool) {
        motion::soften(&self.stack);
        let (symbol, tip) = if stacked {
            ("list.bullet", "Списками")
        } else {
            ("square.stack", "Стопками")
        };
        let tip = NSString::from_str(tip);
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&tip),
        );
        self.stack.setImage(image.as_deref());
        self.stack.setToolTip(Some(&tip));
        self.bar.stack.setImage(image.as_deref());
        self.bar.stack.setToolTip(Some(&tip));
        self.bar.stack.setLabel(&tip);
    }

    fn update_mini_icon(&self) {
        let (symbol, tip) = if self.minimal.get() {
            ("arrow.up.left.and.arrow.down.right", "Показать доску (⇧⌘M)")
        } else {
            ("arrow.down.right.and.arrow.up.left", "Только таймер (⇧⌘M)")
        };
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(tip)),
        );
        self.mini.setImage(image.as_deref());
        self.mini.setToolTip(Some(&NSString::from_str(tip)));
        self.bar.mini.setImage(image.as_deref());
        self.bar.mini.setToolTip(Some(&NSString::from_str(tip)));
        self.bar.mini.setLabel(&NSString::from_str(tip));
    }

    fn layout(&self, width: f64, height: f64) {
        let digits_w: f64 = self
            .digits
            .borrow()
            .iter()
            .map(|d| d.frame().size.width - DIGIT_PAD)
            .sum();
        let minimal = self.minimal.get();
        for b in [&self.stopwatch, &self.skip, &self.stop] {
            if minimal {
                b.setHidden(true);
            }
        }
        // Only the timer: the ring and the digits.
        self.phase.setHidden(true);
        self.tasks.setHidden(true);
        self.stack.setHidden(minimal);
        if minimal {
            self.primary.setHidden(false);
            self.mini.setHidden(false);
            // Under the traffic lights: the ring, then the big digits, the phase
            // under them, the main button on the right.
            let top = 30.0;
            let (_, _, _, dh) = DIGITS_MINI;
            self.ring.view.setFrame(NSRect::new(
                NSPoint::new(PAD, top + (dh - RING_D_MINI) / 2.0),
                NSSize::new(RING_D_MINI, RING_D_MINI),
            ));
            self.ring.layout();
            let dx = PAD + RING_D_MINI + 10.0;
            self.digits_box.setFrame(NSRect::new(
                NSPoint::new(dx, top),
                NSSize::new(digits_w.max(1.0), dh),
            ));
            self.phase
                .setFont(Some(&NSFont::systemFontOfSize_weight(15.0, unsafe {
                    NSFontWeightSemibold
                })));
            self.phase.setFrame(NSRect::new(
                NSPoint::new(PAD + 2.0, top + dh + 2.0),
                NSSize::new(width - 2.0 * PAD - 110.0, 20.0),
            ));
            self.mini.setFrame(NSRect::new(
                NSPoint::new(width - 34.0, 4.0),
                NSSize::new(24.0, 22.0),
            ));
            let w = self.primary.fittingSize().width.max(88.0);
            self.primary.setFrame(NSRect::new(
                NSPoint::new(width - PAD - w, top + (dh - 28.0) / 2.0),
                NSSize::new(w, 28.0),
            ));
            return;
        }
        // One toolbar row, everything on its centre line: ring, digits, phase over
        // the tasks, then the controls on the right.
        let mid = height / 2.0;
        let (_, _, _, dh) = DIGITS_BAR;
        self.ring.view.setFrame(NSRect::new(
            NSPoint::new(0.0, mid - RING_D / 2.0),
            NSSize::new(RING_D, RING_D),
        ));
        self.ring.layout();
        let dx = RING_D + 8.0;
        self.digits_box.setFrame(NSRect::new(
            NSPoint::new(dx, mid - dh / 2.0),
            NSSize::new(digits_w.max(1.0), dh),
        ));
        // The controls are toolbar items of their own here.
        for b in [
            &self.primary,
            &self.stopwatch,
            &self.skip,
            &self.stop,
            &self.stack,
            &self.mini,
        ] {
            b.setHidden(true);
        }
        let text_x = dx + digits_w + 12.0;
        let text_w = (width - text_x - 12.0).max(40.0);
        self.phase
            .setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe {
                NSFontWeightSemibold
            })));
        self.phase.setFrame(NSRect::new(
            NSPoint::new(text_x, mid - 17.0),
            NSSize::new(text_w, 17.0),
        ));
        self.tasks.setFrame(NSRect::new(
            NSPoint::new(text_x, mid + 1.0),
            NSSize::new(text_w, 15.0),
        ));
    }

    /// Drops the clock's digits, so the next update builds them at the size of
    /// the current mode.
    fn reset_clock(&self) {
        for d in self.digits.borrow_mut().drain(..) {
            d.removeFromSuperview();
        }
        self.shown_clock.borrow_mut().clear();
    }

    fn update(&self, t: &TimerSnap) {
        self.set_clock(&format_clock(t.clock_ms));
        self.phase
            .setStringValue(&NSString::from_str(&t.phase_label));
        self.tasks
            .setStringValue(&NSString::from_str(&t.tasks_label));
        // The main button: a tomato to start a pomodoro, then pause and play.
        let (icon, tip) = primary_icon(t);
        self.primary.setTitle(ns_string!(""));
        self.primary.setImage(icon.as_deref());
        self.primary.setToolTip(Some(&NSString::from_str(tip)));
        // The phase and the tasks it counts for are no longer shown; they stay a
        // hover away.
        let summary = format!("{} · {}", t.phase_label, t.tasks_label);
        self.view.setToolTip(Some(&NSString::from_str(&summary)));
        self.stopwatch.setHidden(!t.can_start_stopwatch);
        self.skip.setHidden(!t.can_skip);
        self.stop.setHidden(!t.can_stop);
        self.bar.start.setImage(icon.as_deref());
        self.bar.start.setLabel(&NSString::from_str(tip));
        self.bar.start.setToolTip(Some(&NSString::from_str(tip)));
        self.bar.stopwatch.setHidden(!t.can_start_stopwatch);
        self.bar.skip.setHidden(!t.can_skip);
        self.bar.stop.setHidden(!t.can_stop);
        self.ring.set(t.progress as f64);
        let size = self.view.frame().size;
        self.layout(size.width, size.height);
    }

    /// One label per character; a changed digit rolls in from the top.
    fn set_clock(&self, clock: &str) {
        let mut shown = self.shown_clock.borrow_mut();
        if *shown == clock {
            return;
        }
        let mtm = MainThreadMarker::new().expect("main thread");
        let mut digits = self.digits.borrow_mut();
        let chars: Vec<char> = clock.chars().collect();
        if digits.len() != chars.len() {
            for d in digits.drain(..) {
                d.removeFromSuperview();
            }
            let (size, digit_w, colon_w, h) = if self.minimal.get() {
                DIGITS_MINI
            } else {
                DIGITS_BAR
            };
            let mut x = 0.0;
            for ch in &chars {
                let w = if ch.is_ascii_digit() {
                    digit_w
                } else {
                    colon_w
                };
                let d = label(mtm, &ch.to_string(), size, unsafe { NSFontWeightMedium });
                d.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                    size,
                    unsafe { NSFontWeightMedium },
                )));
                d.setAlignment(NSTextAlignment::Center);
                d.setWantsLayer(true);
                d.setLayerUsesCoreImageFilters(true);
                d.setFrame(NSRect::new(
                    NSPoint::new(x - DIGIT_PAD / 2.0, 0.0),
                    NSSize::new(w + DIGIT_PAD, h),
                ));
                self.digits_box.addSubview(&d);
                digits.push(d);
                x += w;
            }
        } else {
            let old: Vec<char> = shown.chars().collect();
            let reduce = motion::reduce_motion();
            for (i, ch) in chars.iter().enumerate() {
                if old.get(i) == Some(ch) {
                    continue;
                }
                let d = &digits[i];
                // The system's numeric transition: the old glyph shrinks, blurs and
                // fades away upwards on a ghost copy, the new one grows in from
                // below, sharpening. Only this digit moves.
                let ghost = label(
                    mtm,
                    &old.get(i).map(|c| c.to_string()).unwrap_or_default(),
                    1.0,
                    0.0,
                );
                ghost.setFont(d.font().as_deref());
                ghost.setAlignment(NSTextAlignment::Center);
                ghost.setFrame(d.frame());
                ghost.setWantsLayer(true);
                ghost.setLayerUsesCoreImageFilters(true);
                self.digits_box.addSubview(&ghost);
                let (w, h) = (d.frame().size.width, d.frame().size.height);
                if let Some(layer) = ghost.layer() {
                    numeric_morph(&layer, (w, h), DigitPose::REST, DigitPose::OUT, reduce);
                }
                if let Some(layer) = d.layer() {
                    numeric_morph(&layer, (w, h), DigitPose::IN, DigitPose::REST, reduce);
                }
                let block = RcBlock::new(move |_t: NonNull<objc2_foundation::NSTimer>| {
                    ghost.removeFromSuperview();
                });
                unsafe {
                    objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                        motion::DIGIT_SECS + 0.15,
                        false,
                        &block,
                    );
                }
                d.setStringValue(&NSString::from_str(&ch.to_string()));
            }
        }
        *shown = clock.to_string();
    }
}

/// A digit's look in the numeric transition: scale, vertical offset (in its
/// height), blur radius (pt) and opacity.
#[derive(Clone, Copy)]
struct DigitPose {
    scale: f64,
    rise: f64,
    blur: f64,
    alpha: f32,
}

impl DigitPose {
    const REST: Self = Self {
        scale: 1.0,
        rise: 0.0,
        blur: 0.0,
        alpha: 1.0,
    };
    const IN: Self = Self {
        scale: 0.8,
        rise: 0.22,
        blur: 4.0,
        alpha: 0.0,
    };
    const OUT: Self = Self {
        scale: 0.8,
        rise: -0.22,
        blur: 4.0,
        alpha: 0.0,
    };

    /// About the glyph's centre (a view's layer scales about its corner).
    fn transform(self, (w, h): (f64, f64)) -> objc2_quartz_core::CATransform3D {
        let s = self.scale;
        objc2_quartz_core::CATransform3D {
            m11: s,
            m12: 0.0,
            m13: 0.0,
            m14: 0.0,
            m21: 0.0,
            m22: s,
            m23: 0.0,
            m24: 0.0,
            m31: 0.0,
            m32: 0.0,
            m33: 1.0,
            m34: 0.0,
            m41: w * (1.0 - s) / 2.0,
            m42: h * (1.0 - s) / 2.0 + self.rise * h,
            m43: 0.0,
            m44: 1.0,
        }
    }
}

/// Animates a digit's layer from one pose to another: the scale and offset on a
/// critically damped spring (no bounce), opacity and blur alongside. The layer
/// ends in `to` (kept for a pose other than rest, for a ghost on its way out).
fn numeric_morph(layer: &CALayer, size: (f64, f64), from: DigitPose, to: DigitPose, reduce: bool) {
    let keep = |a: &CABasicAnimation| {
        if to.alpha < 1.0 {
            a.setFillMode(unsafe { objc2_quartz_core::kCAFillModeForwards });
            a.setRemovedOnCompletion(false);
        }
    };
    let opacity = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
    unsafe {
        opacity.setFromValue(Some(&NSNumber::new_f32(from.alpha)));
        opacity.setToValue(Some(&NSNumber::new_f32(to.alpha)));
    }
    opacity.setDuration(motion::DIGIT_SECS);
    keep(&opacity);
    layer.addAnimation_forKey(&opacity, Some(ns_string!("digit.opacity")));
    if reduce {
        return;
    }
    let spring = CASpringAnimation::animationWithKeyPath(Some(ns_string!("transform")));
    spring.setMass(1.0);
    spring.setStiffness(DIGIT_STIFFNESS);
    spring.setDamping(2.0 * DIGIT_STIFFNESS.sqrt());
    unsafe {
        spring.setFromValue(Some(&NSValue::valueWithCATransform3D(from.transform(size))));
        spring.setToValue(Some(&NSValue::valueWithCATransform3D(to.transform(size))));
    }
    spring.setDuration(spring.settlingDuration());
    keep(&spring);
    layer.addAnimation_forKey(&spring, Some(ns_string!("digit.transform")));
    // The blur runs on a filter named on the layer for the length of the change.
    if let Some(blur) = style::gaussian(0.0) {
        unsafe { blur.setValue_forKey(Some(ns_string!("blur")), ns_string!("name")) };
        let filters =
            NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(blur))]);
        unsafe { layer.setFilters(Some(&filters)) };
        let radius =
            CABasicAnimation::animationWithKeyPath(Some(ns_string!("filters.blur.inputRadius")));
        unsafe {
            radius.setFromValue(Some(&NSNumber::new_f64(from.blur)));
            radius.setToValue(Some(&NSNumber::new_f64(to.blur)));
        }
        radius.setDuration(motion::DIGIT_SECS);
        keep(&radius);
        layer.addAnimation_forKey(&radius, Some(ns_string!("digit.blur")));
    }
}

/// Stiffness of the digits' spring: settled in about 0.3 s.
const DIGIT_STIFFNESS: f64 = 300.0;

/// The toolbar's control items, each its own bubble; the two view switches,
/// side by side, share one.
struct Bar {
    start: Retained<NSToolbarItem>,
    stopwatch: Retained<NSToolbarItem>,
    skip: Retained<NSToolbarItem>,
    stop: Retained<NSToolbarItem>,
    stack: Retained<NSToolbarItem>,
    mini: Retained<NSToolbarItem>,
}

/// The phase's progress as a ring: a faint track and an accent arc that grows
/// clockwise from the top. Its layers are its own, so the arc eases over.
struct Ring {
    view: Retained<FlippedView>,
    track: Retained<CAShapeLayer>,
    arc: Retained<CAShapeLayer>,
}

impl Ring {
    fn new(mtm: MainThreadMarker) -> Self {
        let view = FlippedView::new(mtm, NSRect::ZERO);
        view.setWantsLayer(true);
        let track = CAShapeLayer::new();
        let arc = CAShapeLayer::new();
        for (layer, color) in [
            (&track, NSColor::labelColor().colorWithAlphaComponent(0.12)),
            (&arc, NSColor::controlAccentColor()),
        ] {
            layer.setFillColor(None);
            layer.setStrokeColor(Some(&color.CGColor()));
            layer.setLineWidth(RING_LINE);
            layer.setLineCap(unsafe { kCALineCapRound });
            if let Some(host) = view.layer() {
                host.addSublayer(layer);
            }
        }
        arc.setStrokeEnd(0.0);
        Self { view, track, arc }
    }

    /// Fits the circle to the view's current size.
    fn layout(&self) {
        let d = self.view.frame().size.width;
        let path = CGMutablePath::new();
        let r = (d - RING_LINE) / 2.0;
        // The view is flipped, so going from −90° with growing angles runs
        // clockwise from the top.
        unsafe {
            CGMutablePath::add_arc(
                Some(&path),
                std::ptr::null(),
                d / 2.0,
                d / 2.0,
                r.max(0.0),
                -std::f64::consts::FRAC_PI_2,
                3.0 * std::f64::consts::FRAC_PI_2,
                false,
            );
        }
        motion::quietly(|| {
            for layer in [&self.track, &self.arc] {
                layer.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(d, d)));
                layer.setPath(Some(&path));
            }
        });
    }

    fn set(&self, progress: f64) {
        self.arc.setStrokeEnd(progress.clamp(0.0, 1.0));
    }
}

impl Column {
    fn new(mtm: MainThreadMarker, index: usize) -> Self {
        let view = FlippedView::new(mtm, NSRect::ZERO);
        let tint = FillView::new(mtm, Fill::Tint(index));
        tint.setAlphaValue(0.0);
        view.addSubview(&tint);
        let color = style::column_color(index);
        // A flat pill tinted in the column's colour, with the text in that colour.
        let chip = FillView::new(mtm, Fill::Chip(index));
        let title = label(mtm, "", 11.5, unsafe { NSFontWeightSemibold });
        title.setTextColor(Some(&color));
        let count = label(mtm, "0", 11.0, unsafe { NSFontWeightMedium });
        count.setTextColor(Some(&color.colorWithAlphaComponent(0.7)));
        chip.addSubview(&title);
        chip.addSubview(&count);
        Self {
            view,
            tint,
            chip,
            title,
            count,
            highlighted: Cell::new(false),
        }
    }

    /// Sizes the chip to its title and count, at `x` on the board.
    fn layout_chip(&self, x: f64) {
        let (tw, cw) = (
            self.title.fittingSize().width.ceil(),
            self.count.fittingSize().width.ceil(),
        );
        let (pad, gap, text_h) = (9.0, 5.0, 14.0);
        self.chip.setFrame(NSRect::new(
            NSPoint::new(x, CHIP_TOP),
            NSSize::new(pad + tw + gap + cw + pad, CHIP_H),
        ));
        let y = ((CHIP_H - text_h) / 2.0).round();
        self.title
            .setFrame(NSRect::new(NSPoint::new(pad, y), NSSize::new(tw, text_h)));
        self.count.setFrame(NSRect::new(
            NSPoint::new(pad + tw + gap, y),
            NSSize::new(cw, text_h),
        ));
    }
}

// ---- helpers ----

/// An SF Symbol by name.
fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}

/// The main button's icon and tooltip: a red tomato to start a pomodoro, pause
/// while running, play to resume.
fn primary_icon(t: &TimerSnap) -> (Option<Retained<NSImage>>, &'static str) {
    if t.running {
        (symbol("pause.fill", "Пауза"), "Пауза")
    } else if t.can_start_stopwatch {
        (Some(tomato()), "Помидор")
    } else {
        (symbol("play.fill", "Продолжить"), "Продолжить")
    }
}

/// A small red tomato with a green stem, drawn from the emoji in full colour
/// (not a template image, so the toolbar keeps its colours).
fn tomato() -> Retained<NSImage> {
    thread_local! {
        static TOMATO: Retained<NSImage> = {
            let size = NSSize::new(18.0, 18.0);
            let block = block2::RcBlock::new(move |rect: NSRect| -> objc2::runtime::Bool {
                let font = NSFont::systemFontOfSize(14.0);
                let attrs = objc2_foundation::NSDictionary::from_retained_objects(
                    &[unsafe { objc2_app_kit::NSFontAttributeName }],
                    &[Retained::into_super(Retained::into_super(font))],
                );
                let text = ns_string!("🍅");
                let fit = unsafe { text.sizeWithAttributes(Some(&attrs)) };
                let at = NSPoint::new(
                    rect.origin.x + (rect.size.width - fit.width) / 2.0,
                    rect.origin.y + (rect.size.height - fit.height) / 2.0,
                );
                unsafe { text.drawAtPoint_withAttributes(at, Some(&attrs)) };
                objc2::runtime::Bool::YES
            });
            let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &block);
            image.setTemplate(false);
            image
        };
    }
    TOMATO.with(|t| t.clone())
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, weight: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    l
}

fn timing() -> Retained<CAMediaTimingFunction> {
    let [a, b, c, d] = motion::SPRING_CURVE;
    CAMediaTimingFunction::functionWithControlPoints(a, b, c, d)
}

/// Runs `changes` inside an animation group (or right away when `animated` is false).
pub(crate) fn animate(animated: bool, secs: f64, changes: impl Fn()) {
    if !animated {
        changes();
        return;
    }
    // The changes block runs before runAnimationGroup returns, so it may borrow.
    let changes = &changes;
    let block = StackBlock::new(move |ctx: NonNull<NSAnimationContext>| {
        let ctx = unsafe { ctx.as_ref() };
        ctx.setDuration(secs);
        ctx.setTimingFunction(Some(&timing()));
        ctx.setAllowsImplicitAnimation(true);
        changes();
    });
    NSAnimationContext::runAnimationGroup(&block);
}

pub(crate) fn animate_with_completion(secs: f64, changes: impl Fn(), done: impl Fn() + 'static) {
    let changes = &changes;
    let block = StackBlock::new(move |ctx: NonNull<NSAnimationContext>| {
        let ctx = unsafe { ctx.as_ref() };
        ctx.setDuration(secs);
        ctx.setTimingFunction(Some(&timing()));
        changes();
    });
    let done = RcBlock::new(done);
    NSAnimationContext::runAnimationGroup_completionHandler(&block, Some(&done));
}
