//! The main window, drawn with AppKit. [`MainWindow::render`] diffs a [`Snapshot`]
//! against the views it already has: cards are kept per task id, updated in place
//! and moved with animations, so reorders and column moves stay smooth.
//!
//! Layout: a header with the timer, then the board split into three columns by
//! full-height 1 px dividers. A click on a card opens the task modal
//! (`modal.rs`); minimal mode shrinks the window to the timer alone.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::{RcBlock, StackBlock};
use objc2::rc::Retained;
use objc2::rc::Weak as ObjcWeak;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSControlSize, NSEvent,
    NSEventModifierFlags, NSFloatingWindowLevel, NSFont, NSFontWeightBold, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSLineBreakMode, NSNormalWindowLevel,
    NSProgressIndicator, NSProgressIndicatorStyle, NSTextAlignment, NSTextField,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use objc2_quartz_core::{
    CAMediaTiming, CAMediaTimingFunction, CATransition, kCAMediaTimingFunctionEaseOut,
    kCATransitionFade, kCATransitionFromTop, kCATransitionPush,
};
use yatt_core::{Status, TaskId};

pub use crate::board::DemoPointer;
use crate::board::{Board, ColumnGeom};
use crate::classes::{
    ActionTarget, Fill, FillView, FlippedView, RootView, WindowDelegate, editing_text,
};
use crate::modal::Modal;
use crate::model::{Snapshot, TimerSnap, UiEvent, format_clock};
use crate::motion;
use crate::style::{CARD_H, CARD_W};

pub type Sink = Rc<dyn Fn(UiEvent)>;

const HEADER_H: f64 = 112.0;
const PAD: f64 = 16.0;
const FIELD_INSET: f64 = 12.0;
const DIGIT_W: f64 = 28.0;
const COLON_W: f64 = 14.0;
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
    /// Everything below the header; hidden in minimal mode.
    board: Retained<FlippedView>,
    header_rule: Retained<FillView>,
    dividers: [Retained<FillView>; 2],
    columns: [Column; 3],
    new_task: Retained<NSTextField>,
    cards: Rc<Board>,
    modal: Modal,
    minimal: Cell<bool>,
    /// The window frame to go back to when leaving minimal mode.
    saved_frame: Cell<NSRect>,
    last: RefCell<Option<Snapshot>>,
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
    progress: Retained<NSProgressIndicator>,
    minimal: Cell<bool>,
}

struct Column {
    view: Retained<FlippedView>,
    tint: Retained<FillView>,
    title: Retained<NSTextField>,
    count: Retained<NSTextField>,
    highlighted: Cell<bool>,
}

impl MainWindow {
    pub fn new(mtm: MainThreadMarker, sink: Sink) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1100.0, 720.0));
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

        let effect = NSVisualEffectView::new(mtm);
        effect.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::FollowsWindowActiveState);
        window.setContentView(Some(&effect));

        let root = RootView::new(mtm, effect.bounds());
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        effect.addSubview(&root);

        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let header = Header::new(mtm, &sink, &mut retained);
        root.addSubview(&header.view);

        let board = FlippedView::new(mtm, NSRect::ZERO);
        root.addSubview(&board);
        let header_rule = FillView::new(mtm, Fill::Separator);
        board.addSubview(&header_rule);

        let new_task = NSTextField::textFieldWithString(ns_string!(""), mtm);
        new_task.setPlaceholderString(Some(ns_string!("Новая задача")));
        new_task.setBezelStyle(objc2_app_kit::NSTextFieldBezelStyle::RoundedBezel);
        {
            let sink = sink.clone();
            let field = ObjcWeak::from_retained(&new_task);
            let target = ActionTarget::new(mtm, move || {
                let Some(field) = field.load() else { return };
                let title = field.stringValue().to_string();
                let title = title.trim();
                if !title.is_empty() {
                    sink(UiEvent::NewTask(title.to_string()));
                    field.setStringValue(ns_string!(""));
                }
            });
            unsafe {
                new_task.setTarget(Some(&target));
                new_task.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(target)));
        }

        let columns = Status::ALL.map(|_| Column::new(mtm));
        for c in &columns {
            board.addSubview(&c.view);
        }
        columns[0].view.addSubview(&new_task);
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

        let modal = Modal::new(mtm, &sink);
        root.addSubview(&modal.dim);
        root.addSubview(&modal.panel);

        let ui = Rc::new(Ui {
            mtm,
            window,
            root,
            sink: sink.clone(),
            header,
            board,
            header_rule,
            dividers,
            columns,
            new_task,
            cards,
            modal,
            minimal: Cell::new(false),
            saved_frame: Cell::new(NSRect::ZERO),
            last: RefCell::new(None),
            _retained: RefCell::new(retained),
        });
        *tint_target.borrow_mut() = Some(Rc::downgrade(&ui));
        *tint_target.borrow_mut() = Some(Rc::downgrade(&ui));
        Ui::wire(&ui);

        let delegate = WindowDelegate::new(mtm, move || sink(UiEvent::Quit));
        ui.window
            .setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));
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
        let action = if redo { sel!(redo:) } else { sel!(undo:) };
        unsafe {
            NSApplication::sharedApplication(self.ui.mtm).sendAction_to_from(action, None, None);
        }
        true
    }

    /// Development aid: synthetic pointer input for the demo script, in board
    /// coordinates (y down from the top of the board).
    pub fn demo_pointer(&self, input: DemoPointer) {
        self.ui.cards.demo_pointer(input);
    }

    /// Development aid: where a column's stack sits, in board coordinates.
    pub fn demo_anchor(&self, column: usize) -> (f64, f64) {
        self.ui.cards.demo_anchor(column)
    }

    /// Development aid: prints the notes editor's rendered HTML to stderr.
    pub fn debug_print_editor(&self) {
        self.ui.modal.debug_print_editor();
    }

    /// Draws the window's content into a PNG, without screen-recording permission.
    /// Web content (the notes editor) does not show up; native views do.
    pub fn capture_png(&self, path: &str) -> bool {
        let Some(view) = self.ui.window.contentView() else {
            return false;
        };
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
        *this.root.ivars().on_cancel.borrow_mut() = Some(Box::new({
            let weak = weak.clone();
            move || {
                if let Some(ui) = weak.upgrade()
                    && let Some(id) = ui.modal.open_task()
                {
                    (ui.sink)(UiEvent::ToggleExpand(id));
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
        }
        this._retained
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(target)));
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
            _ => false,
        }
    }

    // ---- layout ----

    fn layout(&self) {
        let bounds = self.root.bounds();
        let size = bounds.size;
        let header_h = if self.minimal.get() {
            size.height
        } else {
            HEADER_H
        };
        self.header.view.setFrame(NSRect::new(
            NSPoint::ZERO,
            NSSize::new(size.width, header_h),
        ));
        self.header.layout(size.width, size.height);
        self.modal.layout(bounds);
        if self.minimal.get() {
            return;
        }

        let board_h = (size.height - HEADER_H).max(100.0);
        self.board.setFrame(NSRect::new(
            NSPoint::new(0.0, HEADER_H),
            NSSize::new(size.width, board_h),
        ));
        self.header_rule
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(size.width, 1.0)));
        self.cards
            .canvas
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(size.width, board_h)));
        let col_w = (size.width / 3.0).floor();
        let mut geom = [ColumnGeom {
            rect: NSRect::ZERO,
            anchor: NSPoint::ZERO,
            top_limit: 0.0,
        }; 3];
        for (i, c) in self.columns.iter().enumerate() {
            let x = i as f64 * col_w;
            let w = if i == 2 { size.width - x } else { col_w };
            c.view.setFrame(NSRect::new(
                NSPoint::new(x, 1.0),
                NSSize::new(w, board_h - 1.0),
            ));
            c.tint.setFrame(c.view.bounds());
            c.title.setFrame(NSRect::new(
                NSPoint::new(PAD + 2.0, 14.0),
                NSSize::new(w - 90.0, 20.0),
            ));
            c.count.setFrame(NSRect::new(
                NSPoint::new(w - PAD - 42.0, 14.0),
                NSSize::new(40.0, 20.0),
            ));
            let mut top = 44.0;
            if i == 0 {
                self.new_task.setFrame(NSRect::new(
                    NSPoint::new(FIELD_INSET, top),
                    NSSize::new(w - 2.0 * FIELD_INSET, 24.0),
                ));
                top += 34.0;
            }
            if i < 2 {
                self.dividers[i].setFrame(NSRect::new(
                    NSPoint::new(x + w - 0.5, 1.0),
                    NSSize::new(1.0, board_h),
                ));
            }
            // A pile spans −0.10W … 1.22W around its top card: centre that in the column.
            geom[i] = ColumnGeom {
                rect: NSRect::new(NSPoint::new(x, 0.0), NSSize::new(w, board_h)),
                anchor: NSPoint::new(
                    (x + w / 2.0 - 0.56 * CARD_W).round(),
                    (top + 0.23 * CARD_H + 10.0).round(),
                ),
                top_limit: top,
            };
        }
        self.cards.set_geometry(geom);
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
            c.count
                .setStringValue(&NSString::from_str(&col.cards.len().to_string()));
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
            let weak = Rc::downgrade(self);
            self.modal.close(to, move || {
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
            self.modal.open(&card, status, from, self.root.bounds());
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

        let mut button = |title: &str, event: UiEvent| {
            let sink = sink.clone();
            let target = ActionTarget::new(mtm, move || sink(event.clone()));
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setBezelStyle(NSBezelStyle::Push);
            b.setControlSize(NSControlSize::Large);
            view.addSubview(&b);
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };
        let primary = button("Старт", UiEvent::StartPause);
        primary.setBezelColor(Some(&NSColor::controlAccentColor()));
        let stopwatch = button("Секундомер", UiEvent::StartStopwatch);
        let skip = button("Пропустить", UiEvent::Skip);
        let stop = button("Стоп", UiEvent::Stop);

        // Target and action are set in `Ui::wire`: minimal mode is window state, not an event.
        let mini =
            unsafe { NSButton::buttonWithTitle_target_action(ns_string!(""), None, None, mtm) };
        mini.setBordered(false);
        mini.setToolTip(Some(ns_string!("Только таймер (⇧⌘M)")));
        view.addSubview(&mini);

        let progress = NSProgressIndicator::new(mtm);
        progress.setStyle(NSProgressIndicatorStyle::Bar);
        progress.setIndeterminate(false);
        progress.setMinValue(0.0);
        progress.setMaxValue(1.0);
        progress.setControlSize(NSControlSize::Small);
        view.addSubview(&progress);

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
            progress,
            minimal: Cell::new(false),
        };
        header.update_mini_icon();
        header
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
    }

    fn layout(&self, width: f64, height: f64) {
        let digits_w: f64 = self
            .digits
            .borrow()
            .iter()
            .map(|d| d.frame().size.width)
            .sum();
        let minimal = self.minimal.get();
        for b in [&self.stopwatch, &self.skip, &self.stop] {
            if minimal {
                b.setHidden(true);
            }
        }
        self.tasks.setHidden(minimal);
        self.mini.setFrame(NSRect::new(
            NSPoint::new(width - 34.0, 4.0),
            NSSize::new(24.0, 22.0),
        ));
        if minimal {
            let top = 30.0;
            self.digits_box.setFrame(NSRect::new(
                NSPoint::new(PAD, top),
                NSSize::new(digits_w.max(1.0), 56.0),
            ));
            self.phase.setFrame(NSRect::new(
                NSPoint::new(PAD + 2.0, top + 58.0),
                NSSize::new(width - 2.0 * PAD - 110.0, 20.0),
            ));
            let w = self.primary.fittingSize().width.max(96.0);
            self.primary.setFrame(NSRect::new(
                NSPoint::new(width - PAD - w, top + 12.0),
                NSSize::new(w, 32.0),
            ));
            self.progress.setFrame(NSRect::new(
                NSPoint::new(PAD, height - 12.0),
                NSSize::new(width - 2.0 * PAD, 6.0),
            ));
            return;
        }
        let top = 34.0;
        self.digits_box.setFrame(NSRect::new(
            NSPoint::new(PAD + 4.0, top),
            NSSize::new(digits_w.max(1.0), 56.0),
        ));
        let text_x = PAD + 4.0 + digits_w + 18.0;
        let buttons = [&self.primary, &self.stopwatch, &self.skip, &self.stop];
        let mut x = width - PAD;
        for b in buttons.iter().rev() {
            if b.isHidden() {
                continue;
            }
            let w = b.fittingSize().width.max(96.0);
            x -= w;
            b.setFrame(NSRect::new(
                NSPoint::new(x, top + 12.0),
                NSSize::new(w, 32.0),
            ));
            x -= 8.0;
        }
        let text_w = (x - text_x - 8.0).max(60.0);
        self.phase.setFrame(NSRect::new(
            NSPoint::new(text_x, top + 10.0),
            NSSize::new(text_w, 20.0),
        ));
        self.tasks.setFrame(NSRect::new(
            NSPoint::new(text_x, top + 32.0),
            NSSize::new(text_w, 16.0),
        ));
        self.progress.setFrame(NSRect::new(
            NSPoint::new(PAD, HEADER_H - 16.0),
            NSSize::new(width - 2.0 * PAD, 6.0),
        ));
    }

    fn update(&self, t: &TimerSnap) {
        self.set_clock(&format_clock(t.clock_ms));
        self.phase
            .setStringValue(&NSString::from_str(&t.phase_label));
        self.tasks
            .setStringValue(&NSString::from_str(&t.tasks_label));
        self.primary.setTitle(&NSString::from_str(&t.primary_label));
        self.stopwatch.setHidden(!t.can_start_stopwatch);
        self.skip.setHidden(!t.can_skip);
        self.stop.setHidden(!t.can_stop);
        self.progress.setDoubleValue(t.progress as f64);
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
            let mut x = 0.0;
            for ch in &chars {
                let w = if ch.is_ascii_digit() {
                    DIGIT_W
                } else {
                    COLON_W
                };
                let d = label(mtm, &ch.to_string(), 46.0, unsafe { NSFontWeightMedium });
                d.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                    46.0,
                    unsafe { NSFontWeightMedium },
                )));
                d.setAlignment(NSTextAlignment::Center);
                d.setWantsLayer(true);
                d.setFrame(NSRect::new(NSPoint::new(x, 0.0), NSSize::new(w, 56.0)));
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
                if let Some(layer) = d.layer() {
                    let t = CATransition::animation();
                    unsafe {
                        if reduce {
                            t.setType(kCATransitionFade);
                        } else {
                            t.setType(kCATransitionPush);
                            t.setSubtype(Some(kCATransitionFromTop));
                        }
                        t.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(
                            kCAMediaTimingFunctionEaseOut,
                        )));
                    }
                    t.setDuration(motion::DIGIT_SECS);
                    layer.addAnimation_forKey(&t, Some(ns_string!("roll")));
                }
                d.setStringValue(&NSString::from_str(&ch.to_string()));
            }
        }
        *shown = clock.to_string();
    }
}

impl Column {
    fn new(mtm: MainThreadMarker) -> Self {
        let view = FlippedView::new(mtm, NSRect::ZERO);
        let tint = FillView::new(mtm, Fill::Tint);
        tint.setAlphaValue(0.0);
        view.addSubview(&tint);
        let title = label(mtm, "", 13.0, unsafe { NSFontWeightBold });
        let count = label(mtm, "0", 12.0, unsafe { NSFontWeightRegular });
        count.setAlignment(NSTextAlignment::Right);
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        view.addSubview(&title);
        view.addSubview(&count);
        Self {
            view,
            tint,
            title,
            count,
            highlighted: Cell::new(false),
        }
    }
}

// ---- helpers ----

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
