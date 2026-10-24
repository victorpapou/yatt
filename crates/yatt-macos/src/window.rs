//! The main window, drawn with AppKit. [`MainWindow::render`] diffs a [`Snapshot`]
//! against the views it already has: cards are kept per task id, updated in place
//! and moved with animations, so reorders and column moves stay smooth.
//!
//! Layout: a header with the timer, then the board split into three columns by
//! full-height 1 px dividers. A click on a card opens the task modal
//! (`modal.rs`); minimal mode shrinks the window to the timer alone.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

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
    NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTextAlignment, NSTextField,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use objc2_quartz_core::{
    CAMediaTiming, CAMediaTimingFunction, CATransition, kCAMediaTimingFunctionEaseOut,
    kCATransitionFade, kCATransitionFromTop, kCATransitionPush,
};
use yatt_core::{Status, TaskId};

use crate::classes::{
    ActionTarget, CardMsg, CardView, ColumnView, DropMsg, Fill, FillView, FlippedView, RootView,
    WindowDelegate, editing_text,
};
use crate::modal::Modal;
use crate::model::{CardSnap, Snapshot, TimerSnap, UiEvent, format_clock, format_duration};
use crate::motion;

pub type Sink = Rc<dyn Fn(UiEvent)>;

const HEADER_H: f64 = 112.0;
const PAD: f64 = 16.0;
const CARD_GAP: f64 = 8.0;
const CARD_H: f64 = 60.0;
const CARD_INSET: f64 = 12.0;
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
    cards: RefCell<HashMap<TaskId, Card>>,
    /// The card being dragged, hidden in its column until the drag ends.
    dragging: Cell<Option<TaskId>>,
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
    status: Status,
    view: Retained<ColumnView>,
    tint: Retained<FillView>,
    title: Retained<NSTextField>,
    count: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    doc: Retained<FlippedView>,
    /// Card order as last rendered (or as a drop just changed it).
    order: RefCell<Vec<TaskId>>,
    /// Where a dragged card would land, as an index among the visible cards.
    gap: Cell<Option<usize>>,
    highlighted: Cell<bool>,
}

struct Card {
    view: Retained<CardView>,
    title: Retained<NSTextField>,
    meta: Retained<NSTextField>,
    snap: CardSnap,
    status: Status,
    removing: bool,
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

        let columns = Status::ALL.map(|status| Column::new(mtm, status));
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
            cards: RefCell::new(HashMap::new()),
            dragging: Cell::new(None),
            modal,
            minimal: Cell::new(false),
            saved_frame: Cell::new(NSRect::ZERO),
            last: RefCell::new(None),
            _retained: RefCell::new(retained),
        });
        Ui::wire(&ui);

        let delegate = WindowDelegate::new(mtm, move || sink(UiEvent::Quit));
        ui.window
            .setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));
        ui._retained
            .borrow_mut()
            .push(Retained::into_super(Retained::into_super(delegate)));

        ui.layout(false);
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
                    ui.layout(false);
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
        for (i, column) in this.columns.iter().enumerate() {
            let weak = weak.clone();
            *column.view.ivars().on_drop.borrow_mut() = Some(Box::new(move |msg| {
                weak.upgrade().is_some_and(|ui| ui.on_drop(i, msg))
            }));
        }
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

    fn layout(&self, animate: bool) {
        let bounds = self.root.bounds();
        let size = bounds.size;
        self.header.view.setFrame(NSRect::new(
            NSPoint::ZERO,
            NSSize::new(
                size.width,
                if self.minimal.get() {
                    size.height
                } else {
                    HEADER_H
                },
            ),
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
        let col_w = (size.width / 3.0).floor();
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
                    NSPoint::new(CARD_INSET, top),
                    NSSize::new(w - 2.0 * CARD_INSET, 24.0),
                ));
                top += 34.0;
            }
            c.scroll.setFrame(NSRect::new(
                NSPoint::new(CARD_INSET - 4.0, top),
                NSSize::new(w - 2.0 * CARD_INSET + 8.0, (board_h - top - 1.0).max(40.0)),
            ));
            if i < 2 {
                self.dividers[i].setFrame(NSRect::new(
                    NSPoint::new(x + w - 0.5, 1.0),
                    NSSize::new(1.0, board_h),
                ));
            }
            self.layout_column(i, animate);
        }
    }

    /// Stacks a column's cards top to bottom, leaving room for a dragged card when needed.
    fn layout_column(&self, index: usize, animate: bool) {
        self.layout_column_with(index, animate, motion::MOVE_SECS);
    }

    fn layout_column_with(&self, index: usize, animate: bool, secs: f64) {
        let c = &self.columns[index];
        let cards = self.cards.borrow();
        let clip = c.scroll.contentView().bounds().size;
        let width = clip.width - 8.0;
        let mut y = 4.0;
        let mut frames = Vec::new();
        let visible = self.visible_order(index);
        for (i, id) in visible.iter().enumerate() {
            if c.gap.get() == Some(i) {
                y += CARD_H + CARD_GAP;
            }
            let Some(card) = cards.get(id) else { continue };
            frames.push((
                card.view.clone(),
                NSRect::new(NSPoint::new(4.0, y), NSSize::new(width, CARD_H)),
            ));
            y += CARD_H + CARD_GAP;
        }
        if c.gap.get() == Some(visible.len()) {
            y += CARD_H + CARD_GAP;
        }
        c.doc
            .setFrameSize(NSSize::new(clip.width, y.max(clip.height)));
        c.count
            .setStringValue(&NSString::from_str(&c.order.borrow().len().to_string()));
        drop(cards);
        animate_frames(frames, animate, secs);
    }

    /// The column's cards without the one being dragged.
    fn visible_order(&self, index: usize) -> Vec<TaskId> {
        let dragging = self.dragging.get();
        self.columns[index]
            .order
            .borrow()
            .iter()
            .copied()
            .filter(|id| Some(*id) != dragging)
            .collect()
    }

    /// Index among the visible cards for a point in the column view's coordinates.
    fn drop_index(&self, index: usize, p: NSPoint) -> usize {
        let c = &self.columns[index];
        let p = c.doc.convertPoint_fromView(p, Some(&c.view));
        let visible = self.visible_order(index);
        let slot = ((p.y - 4.0 + CARD_GAP / 2.0) / (CARD_H + CARD_GAP)).floor();
        (slot.max(0.0) as usize).min(visible.len())
    }

    /// Top of a card in its column's current order.
    fn card_y(&self, index: usize, id: TaskId) -> f64 {
        let pos = self.columns[index]
            .order
            .borrow()
            .iter()
            .position(|x| *x == id)
            .unwrap_or(0);
        4.0 + pos as f64 * (CARD_H + CARD_GAP)
    }

    fn column_of(&self, id: TaskId) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.order.borrow().contains(&id))
    }

    // ---- drag and drop ----

    fn on_card(&self, id: TaskId, msg: CardMsg) {
        match msg {
            CardMsg::Click => {
                self.window.makeFirstResponder(Some(&self.root));
                (self.sink)(UiEvent::ToggleExpand(id));
            }
            CardMsg::DragStarted => {
                self.dragging.set(Some(id));
                if let Some(card) = self.cards.borrow().get(&id) {
                    card.view.setAlphaValue(0.0);
                }
                for i in 0..3 {
                    self.layout_column(i, true);
                }
            }
            CardMsg::DragEnded => {
                // After a drop the card already sits in its new place; otherwise it comes back.
                self.dragging.set(None);
                if let Some(card) = self.cards.borrow().get(&id) {
                    let view = card.view.clone();
                    animate(true, motion::APPEAR_SECS, move || {
                        view.animator().setAlphaValue(1.0)
                    });
                }
                for (i, c) in self.columns.iter().enumerate() {
                    c.gap.set(None);
                    self.set_highlight(i, false);
                    self.layout_column(i, true);
                }
            }
        }
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

    fn on_drop(&self, index: usize, msg: DropMsg) -> bool {
        let Some(id) = self.dragging.get() else {
            return false;
        };
        let c = &self.columns[index];
        match msg {
            DropMsg::Over(p) => {
                let at = self.drop_index(index, p);
                for i in 0..3 {
                    self.set_highlight(i, i == index);
                }
                if c.gap.get() != Some(at) {
                    c.gap.set(Some(at));
                    self.layout_column_with(index, true, motion::GAP_SECS);
                }
                true
            }
            DropMsg::Exit => {
                self.set_highlight(index, false);
                c.gap.set(None);
                self.layout_column_with(index, true, motion::GAP_SECS);
                true
            }
            DropMsg::Drop(p) => {
                let at = self.drop_index(index, p);
                self.set_highlight(index, false);
                c.gap.set(None);
                // Put the card where the gap was right away, so the new snapshot finds it in place.
                for col in &self.columns {
                    col.order.borrow_mut().retain(|x| *x != id);
                }
                {
                    let mut order = c.order.borrow_mut();
                    let at = at.min(order.len());
                    order.insert(at, id);
                }
                if let Some(card) = self.cards.borrow().get(&id) {
                    let y = self.card_y(index, id);
                    if !unsafe { card.view.superview() }.is_some_and(|s| same(&s, &c.doc)) {
                        card.view.removeFromSuperview();
                        c.doc.addSubview(&card.view);
                    }
                    let width = c.scroll.contentView().bounds().size.width - 8.0;
                    card.view.setFrame(NSRect::new(
                        NSPoint::new(4.0, y),
                        NSSize::new(width, CARD_H),
                    ));
                    card.view.setAlphaValue(1.0);
                }
                self.dragging.set(None);
                for i in 0..3 {
                    self.layout_column(i, true);
                }
                self.dragging.set(Some(id));
                (self.sink)(UiEvent::Move {
                    id,
                    status: c.status,
                    index: at,
                });
                true
            }
        }
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
        self.layout(false);
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

        let mut seen = Vec::new();
        for (index, column) in snap.columns.iter().enumerate() {
            let c = &self.columns[index];
            c.title.setStringValue(&NSString::from_str(&column.title));
            *c.order.borrow_mut() = column.cards.iter().map(|c| c.id).collect();
            for card_snap in &column.cards {
                seen.push(card_snap.id);
                self.upsert_card(index, card_snap);
            }
        }

        let gone: Vec<TaskId> = self
            .cards
            .borrow()
            .iter()
            .filter(|(id, card)| !seen.contains(id) && !card.removing)
            .map(|(id, _)| *id)
            .collect();

        self.sync_modal(snap);
        for id in gone {
            self.remove_card(id);
        }

        if self.dragging.get().is_none_or(|id| !seen.contains(&id)) {
            for i in 0..3 {
                self.layout_column(i, true);
            }
        }
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
                if let Some(ui) = weak.upgrade()
                    && let Some(card) = ui.cards.borrow().get(&id)
                {
                    card.view.setAlphaValue(1.0);
                }
            });
            self.window.makeFirstResponder(Some(&self.root));
        }
        if let Some((card, status)) = want.and_then(find) {
            let from = self
                .card_rect_in_root(card.id)
                .unwrap_or_else(|| Modal::target_frame(self.root.bounds()));
            if let Some(view) = self.cards.borrow().get(&card.id) {
                view.view.setAlphaValue(0.0);
            }
            self.modal.open(&card, status, from, self.root.bounds());
        }
    }

    /// Where a card is on screen, in the root view's coordinates, if it is laid out.
    fn card_rect_in_root(&self, id: TaskId) -> Option<NSRect> {
        let index = self.column_of(id)?;
        let c = &self.columns[index];
        let width = c.scroll.contentView().bounds().size.width - 8.0;
        let rect = NSRect::new(
            NSPoint::new(4.0, self.card_y(index, id)),
            NSSize::new(width, CARD_H),
        );
        Some(self.root.convertRect_fromView(rect, Some(&c.doc)))
    }

    fn upsert_card(self: &Rc<Self>, index: usize, snap: &CardSnap) {
        let status = self.columns[index].status;
        let doc = &self.columns[index].doc;
        let mut cards = self.cards.borrow_mut();
        let card = match cards.get_mut(&snap.id) {
            Some(card) => {
                card.removing = false;
                // Moved to another column: keep its spot on screen, then animate to the new place.
                let parent = unsafe { card.view.superview() };
                if !parent.as_ref().is_some_and(|s| same(s, doc)) {
                    let frame = match parent {
                        Some(old) => doc.convertRect_fromView(card.view.frame(), Some(&old)),
                        None => card.view.frame(),
                    };
                    card.view.removeFromSuperview();
                    doc.addSubview(&card.view);
                    card.view.setFrame(frame);
                }
                card
            }
            None => {
                let card = Card::new(self.mtm, snap, Rc::downgrade(self));
                let y = self.card_y(index, snap.id);
                let width = self.columns[index].scroll.contentView().bounds().size.width - 8.0;
                card.view.setFrame(NSRect::new(
                    NSPoint::new(4.0, y - 6.0),
                    NSSize::new(width, CARD_H),
                ));
                card.view.setAlphaValue(0.0);
                doc.addSubview(&card.view);
                let view = card.view.clone();
                animate(true, motion::APPEAR_SECS, move || {
                    view.animator().setAlphaValue(1.0)
                });
                cards.entry(snap.id).or_insert(card)
            }
        };
        card.update(snap, status);
    }

    fn remove_card(self: &Rc<Self>, id: TaskId) {
        let view = {
            let mut cards = self.cards.borrow_mut();
            let Some(card) = cards.get_mut(&id) else {
                return;
            };
            card.removing = true;
            card.view.clone()
        };
        let done_view = view.clone();
        let weak = Rc::downgrade(self);
        animate_with_completion(
            motion::APPEAR_SECS,
            move || view.animator().setAlphaValue(0.0),
            move || {
                done_view.removeFromSuperview();
                // A card that came back during the fade (undo) stays.
                if let Some(ui) = weak.upgrade() {
                    let mut cards = ui.cards.borrow_mut();
                    if cards.get(&id).is_some_and(|c| c.removing) {
                        cards.remove(&id);
                    }
                }
            },
        );
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
    fn new(mtm: MainThreadMarker, status: Status) -> Self {
        let view = ColumnView::new(mtm, NSRect::ZERO);
        let tint = FillView::new(mtm, Fill::Tint);
        tint.setAlphaValue(0.0);
        view.addSubview(&tint);
        let title = label(mtm, "", 13.0, unsafe { NSFontWeightBold });
        let count = label(mtm, "0", 12.0, unsafe { NSFontWeightRegular });
        count.setAlignment(NSTextAlignment::Right);
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let scroll = NSScrollView::new(mtm);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let doc = FlippedView::new(mtm, NSRect::ZERO);
        scroll.setDocumentView(Some(&doc));
        view.addSubview(&title);
        view.addSubview(&count);
        view.addSubview(&scroll);
        Self {
            status,
            view,
            tint,
            title,
            count,
            scroll,
            doc,
            order: RefCell::new(Vec::new()),
            gap: Cell::new(None),
            highlighted: Cell::new(false),
        }
    }
}

impl Card {
    fn new(mtm: MainThreadMarker, snap: &CardSnap, ui: Weak<Ui>) -> Self {
        let view = CardView::new(
            mtm,
            snap.id,
            NSRect::new(NSPoint::ZERO, NSSize::new(200.0, CARD_H)),
        );
        view.setWantsLayer(true);
        *view.ivars().on_msg.borrow_mut() = Some(Box::new(move |id, msg| {
            if let Some(ui) = ui.upgrade() {
                ui.on_card(id, msg);
            }
        }));
        let title = label(mtm, &snap.title, 13.0, unsafe { NSFontWeightMedium });
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        title.setFrame(NSRect::new(
            NSPoint::new(12.0, 10.0),
            NSSize::new(176.0, 18.0),
        ));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let meta = label(mtm, "", 11.0, unsafe { NSFontWeightRegular });
        meta.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0,
            unsafe { NSFontWeightRegular },
        )));
        meta.setFrame(NSRect::new(
            NSPoint::new(12.0, 33.0),
            NSSize::new(176.0, 16.0),
        ));
        meta.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.addSubview(&title);
        view.addSubview(&meta);
        Self {
            view,
            title,
            meta,
            snap: snap.clone(),
            status: Status::Backlog,
            removing: false,
        }
    }

    fn update(&mut self, snap: &CardSnap, status: Status) {
        self.title.setStringValue(&NSString::from_str(&snap.title));
        let notes = if snap.notes.trim().is_empty() {
            ""
        } else {
            "  ·  ✎"
        };
        let meta = format!(
            "{}{}  ·  сегодня {}  ·  🍅 {}{}",
            if snap.tracking { "● " } else { "" },
            format_duration(snap.total_ms),
            format_duration(snap.today_ms),
            snap.pomodoros,
            notes
        );
        self.meta.setStringValue(&NSString::from_str(&meta));
        if self.view.ivars().tracking.get() != snap.tracking {
            self.view.ivars().tracking.set(snap.tracking);
            self.view.setNeedsDisplay(true);
        }
        let color = if snap.tracking {
            NSColor::controlAccentColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.meta.setTextColor(Some(&color));
        self.snap = snap.clone();
        self.status = status;
    }
}

// ---- helpers ----

fn same(a: &objc2_app_kit::NSView, b: &objc2_app_kit::NSView) -> bool {
    std::ptr::eq(a, b)
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

/// Moves views to their frames. With reduced motion they jump and cross-fade instead.
fn animate_frames(frames: Vec<(Retained<CardView>, NSRect)>, animated: bool, secs: f64) {
    let moved: Vec<_> = frames
        .into_iter()
        .filter(|(v, f)| v.frame() != *f)
        .collect();
    if moved.is_empty() {
        return;
    }
    if !animated {
        for (v, f) in &moved {
            v.setFrame(*f);
        }
        return;
    }
    if motion::reduce_motion() {
        for (v, f) in &moved {
            v.setAlphaValue(0.0);
            v.setFrame(*f);
        }
        animate(true, secs, move || {
            for (v, _) in &moved {
                v.animator().setAlphaValue(1.0);
            }
        });
        return;
    }
    animate(true, secs, move || {
        for (v, f) in &moved {
            v.animator().setFrame(*f);
        }
    });
}
