//! The main window, drawn with AppKit. [`MainWindow::render`] diffs a [`Snapshot`]
//! against the views it already has: cards are kept per task id, updated in place
//! and moved with animations, so reorders, column moves and expansion stay smooth.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::{RcBlock, StackBlock};
use objc2::rc::Retained;
use objc2::rc::Weak as ObjcWeak;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSEvent, NSEventModifierFlags, NSFont,
    NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
    NSLineBreakMode, NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTextAlignment,
    NSTextField, NSTextView, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use objc2_quartz_core::{
    CAMediaTiming, CAMediaTimingFunction, CATransition, kCAMediaTimingFunctionEaseOut,
    kCATransitionFade, kCATransitionFromTop, kCATransitionPush,
};
use yatt_core::{Status, TaskId};

use crate::classes::{
    ActionTarget, CardMsg, CardView, ColumnView, DropMsg, FlippedView, NotesDelegate, RootView,
    WindowDelegate, editing_text,
};
use crate::model::{CardSnap, Snapshot, TimerSnap, UiEvent, format_clock, format_duration};
use crate::motion;

pub type Sink = Rc<dyn Fn(UiEvent)>;

const HEADER_H: f64 = 112.0;
const PAD: f64 = 16.0;
const COLUMN_GAP: f64 = 12.0;
const CARD_GAP: f64 = 8.0;
const CARD_COLLAPSED_H: f64 = 60.0;
const ENTRY_LINE_H: f64 = 16.0;
const DIGIT_W: f64 = 28.0;
const COLON_W: f64 = 14.0;

pub struct MainWindow {
    ui: Rc<Ui>,
}

struct Ui {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    root: Retained<RootView>,
    sink: Sink,
    header: Header,
    columns: [Column; 3],
    new_task: Retained<NSTextField>,
    cards: RefCell<HashMap<TaskId, Card>>,
    /// The card being dragged, hidden in its column until the drag ends.
    dragging: Cell<Option<TaskId>>,
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
    progress: Retained<NSProgressIndicator>,
}

struct Column {
    status: Status,
    view: Retained<ColumnView>,
    title: Retained<NSTextField>,
    count: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    doc: Retained<FlippedView>,
    /// Card order as last rendered (or as a drop just changed it).
    order: RefCell<Vec<TaskId>>,
    /// Where a dragged card would land, as an index among the visible cards.
    gap: Cell<Option<usize>>,
}

struct Card {
    view: Retained<CardView>,
    title: Retained<NSTextField>,
    meta: Retained<NSTextField>,
    expanded: Option<Expanded>,
    snap: CardSnap,
    height: f64,
    removing: bool,
}

struct Expanded {
    title_field: Retained<NSTextField>,
    notes_scroll: Retained<NSScrollView>,
    notes: Retained<NSTextView>,
    entries: Retained<NSTextField>,
    done: Retained<NSButton>,
    delete: Retained<NSButton>,
    _retained: Vec<Retained<AnyObject>>,
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
            root.addSubview(&c.view);
        }
        columns[0].view.addSubview(&new_task);

        let ui = Rc::new(Ui {
            mtm,
            window,
            root,
            sink: sink.clone(),
            header,
            columns,
            new_task,
            cards: RefCell::new(HashMap::new()),
            dragging: Cell::new(None),
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

    /// Draws the window's content into a PNG, without screen-recording permission.
    /// Layer styling (card backgrounds, blur) does not show up; text and controls do.
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

    /// cmd-N: put the cursor into the new-task field.
    pub fn focus_new_task(&self) {
        self.ui.window.makeFirstResponder(Some(&self.ui.new_task));
    }

    /// cmd-Z while typing undoes the typing, not the board. Returns true when it did.
    pub fn text_undo(&self, redo: bool) -> bool {
        if !editing_text(&self.ui.window) {
            return false;
        }
        let Some(responder) = self.ui.window.firstResponder() else {
            return false;
        };
        if let Some(manager) = responder.undoManager() {
            if redo {
                manager.redo();
            } else {
                manager.undo();
            }
        }
        true
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
        for (i, column) in this.columns.iter().enumerate() {
            let weak = weak.clone();
            *column.view.ivars().on_drop.borrow_mut() = Some(Box::new(move |msg| {
                weak.upgrade().is_some_and(|ui| ui.on_drop(i, msg))
            }));
        }
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
            // Escape collapses the open card.
            53 => {
                let open = self.last.borrow().as_ref().and_then(|s| s.expanded);
                if let Some(id) = open {
                    (self.sink)(UiEvent::ToggleExpand(id));
                }
                open.is_some()
            }
            _ => false,
        }
    }

    // ---- layout ----

    fn layout(&self, animate: bool) {
        let size = self.root.bounds().size;
        self.header.view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(size.width, HEADER_H),
        ));
        self.header.layout(size.width);

        let col_w = ((size.width - 2.0 * PAD - 2.0 * COLUMN_GAP) / 3.0).floor();
        let col_h = (size.height - HEADER_H - PAD).max(100.0);
        for (i, c) in self.columns.iter().enumerate() {
            let x = PAD + i as f64 * (col_w + COLUMN_GAP);
            c.view.setFrame(NSRect::new(
                NSPoint::new(x, HEADER_H),
                NSSize::new(col_w, col_h),
            ));
            c.title.setFrame(NSRect::new(
                NSPoint::new(14.0, 10.0),
                NSSize::new(col_w - 70.0, 20.0),
            ));
            c.count.setFrame(NSRect::new(
                NSPoint::new(col_w - 54.0, 10.0),
                NSSize::new(40.0, 20.0),
            ));
            let mut top = 38.0;
            if i == 0 {
                self.new_task.setFrame(NSRect::new(
                    NSPoint::new(10.0, top),
                    NSSize::new(col_w - 20.0, 24.0),
                ));
                top += 32.0;
            }
            c.scroll.setFrame(NSRect::new(
                NSPoint::new(8.0, top),
                NSSize::new(col_w - 16.0, (col_h - top - 8.0).max(40.0)),
            ));
            self.layout_column(i, animate);
        }
    }

    /// Stacks a column's cards top to bottom, leaving room for a dragged card when needed.
    fn layout_column(&self, index: usize, animate: bool) {
        let c = &self.columns[index];
        let cards = self.cards.borrow();
        let clip = c.scroll.contentView().bounds().size;
        let width = clip.width - 4.0;
        let gap_h = self
            .dragging
            .get()
            .and_then(|id| cards.get(&id))
            .map_or(CARD_COLLAPSED_H, |card| card.height)
            + CARD_GAP;
        let mut y = 2.0;
        let mut frames = Vec::new();
        let visible = self.visible_order(index);
        for (i, id) in visible.iter().enumerate() {
            if c.gap.get() == Some(i) {
                y += gap_h;
            }
            let Some(card) = cards.get(id) else { continue };
            frames.push((
                card.view.clone(),
                NSRect::new(NSPoint::new(2.0, y), NSSize::new(width, card.height)),
            ));
            y += card.height + CARD_GAP;
        }
        if c.gap.get() == Some(visible.len()) {
            y += gap_h;
        }
        c.doc
            .setFrameSize(NSSize::new(clip.width, y.max(clip.height)));
        c.count
            .setStringValue(&NSString::from_str(&c.order.borrow().len().to_string()));
        drop(cards);
        animate_frames(frames, animate, motion::MOVE_SECS);
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
        let cards = self.cards.borrow();
        let visible = self.visible_order(index);
        let mut y = 2.0;
        for (i, id) in visible.iter().enumerate() {
            let h = cards.get(id).map_or(CARD_COLLAPSED_H, |c| c.height);
            if p.y < y + h / 2.0 {
                return i;
            }
            y += h + CARD_GAP;
        }
        visible.len()
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
                    animate(true, motion::APPEAR_SECS, || {
                        card.view.animator().setAlphaValue(1.0)
                    });
                }
                for (i, c) in self.columns.iter().enumerate() {
                    c.gap.set(None);
                    set_highlight(&c.view, false);
                    self.layout_column(i, true);
                }
            }
        }
    }

    fn on_drop(&self, index: usize, msg: DropMsg) -> bool {
        let Some(id) = self.dragging.get() else {
            return false;
        };
        let c = &self.columns[index];
        match msg {
            DropMsg::Over(p) => {
                let at = self.drop_index(index, p);
                set_highlight(&c.view, true);
                if c.gap.get() != Some(at) {
                    c.gap.set(Some(at));
                    self.layout_column_gap(index);
                }
                true
            }
            DropMsg::Exit => {
                set_highlight(&c.view, false);
                c.gap.set(None);
                self.layout_column_gap(index);
                true
            }
            DropMsg::Drop(p) => {
                let at = self.drop_index(index, p);
                set_highlight(&c.view, false);
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
                    let width = c.scroll.contentView().bounds().size.width - 4.0;
                    card.view.setFrame(NSRect::new(
                        NSPoint::new(2.0, y),
                        NSSize::new(width, card.height),
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

    fn layout_column_gap(&self, index: usize) {
        // Gap changes animate faster than regular moves so the cards follow the cursor.
        let c = &self.columns[index];
        let cards = self.cards.borrow();
        let width = c.scroll.contentView().bounds().size.width - 4.0;
        let gap_h = self
            .dragging
            .get()
            .and_then(|id| cards.get(&id))
            .map_or(CARD_COLLAPSED_H, |card| card.height)
            + CARD_GAP;
        let mut y = 2.0;
        let mut frames = Vec::new();
        for (i, id) in self.visible_order(index).iter().enumerate() {
            if c.gap.get() == Some(i) {
                y += gap_h;
            }
            let Some(card) = cards.get(id) else { continue };
            frames.push((
                card.view.clone(),
                NSRect::new(NSPoint::new(2.0, y), NSSize::new(width, card.height)),
            ));
            y += card.height + CARD_GAP;
        }
        drop(cards);
        animate_frames(frames, true, motion::GAP_SECS);
    }

    /// Top of a card in its column's current order.
    fn card_y(&self, index: usize, id: TaskId) -> f64 {
        let cards = self.cards.borrow();
        let mut y = 2.0;
        for x in self.columns[index].order.borrow().iter() {
            if *x == id {
                break;
            }
            y += cards.get(x).map_or(CARD_COLLAPSED_H, |c| c.height) + CARD_GAP;
        }
        y
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
            for card_snap in &column.cards {
                seen.push(card_snap.id);
                let expanded = snap.expanded == Some(card_snap.id);
                self.upsert_card(index, card_snap, expanded);
            }
            *c.order.borrow_mut() = column.cards.iter().map(|c| c.id).collect();
        }

        let gone: Vec<TaskId> = self
            .cards
            .borrow()
            .iter()
            .filter(|(id, card)| !seen.contains(id) && !card.removing)
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.remove_card(id);
        }

        if self.dragging.get().is_none() || !seen.contains(&self.dragging.get().unwrap()) {
            for i in 0..3 {
                self.layout_column(i, true);
            }
        }
        *self.last.borrow_mut() = Some(snap.clone());
    }

    fn upsert_card(self: &Rc<Self>, index: usize, snap: &CardSnap, expanded: bool) {
        let doc = &self.columns[index].doc;
        let mut cards = self.cards.borrow_mut();
        let card = match cards.get_mut(&snap.id) {
            Some(card) => {
                card.removing = false;
                // Moved to another column: keep its spot on screen, then animate to the new place.
                if !unsafe { card.view.superview() }.is_some_and(|s| same(&s, doc)) {
                    let frame = match unsafe { card.view.superview() } {
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
                let y = {
                    let order = self.columns[index].order.borrow();
                    let pos = order.iter().position(|x| *x == snap.id).unwrap_or(0);
                    order[..pos]
                        .iter()
                        .map(|x| cards.get(x).map_or(CARD_COLLAPSED_H, |c| c.height) + CARD_GAP)
                        .sum::<f64>()
                };
                let width = self.columns[index].scroll.contentView().bounds().size.width - 4.0;
                card.view.setFrame(NSRect::new(
                    NSPoint::new(2.0, y - 6.0),
                    NSSize::new(width, card.height),
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
        card.update(self, snap, expanded, self.columns[index].status);
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
            b.setControlSize(objc2_app_kit::NSControlSize::Large);
            view.addSubview(&b);
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };
        let primary = button("Старт", UiEvent::StartPause);
        primary.setBezelColor(Some(&NSColor::controlAccentColor()));
        primary.setKeyEquivalent(ns_string!(""));
        let stopwatch = button("Секундомер", UiEvent::StartStopwatch);
        let skip = button("Пропустить", UiEvent::Skip);
        let stop = button("Стоп", UiEvent::Stop);

        let progress = NSProgressIndicator::new(mtm);
        progress.setStyle(NSProgressIndicatorStyle::Bar);
        progress.setIndeterminate(false);
        progress.setMinValue(0.0);
        progress.setMaxValue(1.0);
        progress.setControlSize(objc2_app_kit::NSControlSize::Small);
        view.addSubview(&progress);

        Self {
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
            progress,
        }
    }

    fn layout(&self, width: f64) {
        let top = 34.0;
        let digits_w: f64 = self
            .digits
            .borrow()
            .iter()
            .map(|d| d.frame().size.width)
            .sum();
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
            NSPoint::new(PAD, HEADER_H - 14.0),
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
        self.layout(self.view.frame().size.width);
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
        let rebuild = digits.len() != chars.len();
        if rebuild {
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
        view.setWantsLayer(true);
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
            title,
            count,
            scroll,
            doc,
            order: RefCell::new(Vec::new()),
            gap: Cell::new(None),
        }
    }
}

impl Card {
    fn new(mtm: MainThreadMarker, snap: &CardSnap, ui: Weak<Ui>) -> Self {
        let view = CardView::new(
            mtm,
            snap.id,
            NSRect::new(NSPoint::ZERO, NSSize::new(200.0, CARD_COLLAPSED_H)),
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
        meta.setTextColor(Some(&NSColor::secondaryLabelColor()));
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
            expanded: None,
            snap: snap.clone(),
            height: CARD_COLLAPSED_H,
            removing: false,
        }
    }

    fn update(&mut self, ui: &Ui, snap: &CardSnap, expanded: bool, status: Status) {
        self.title.setStringValue(&NSString::from_str(&snap.title));
        let meta = format!(
            "{}{}  ·  сегодня {}  ·  🍅 {}",
            if snap.tracking { "● " } else { "" },
            format_duration(snap.total_ms),
            format_duration(snap.today_ms),
            snap.pomodoros
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

        match (expanded, self.expanded.is_some()) {
            (true, false) => self.expanded = Some(Expanded::new(ui, &self.view, snap)),
            (false, true) => {
                if let Some(e) = self.expanded.take() {
                    e.remove();
                }
            }
            _ => {}
        }
        self.title.setHidden(self.expanded.is_some());
        if let Some(e) = &self.expanded {
            e.update(&ui.window, snap, status);
        }
        self.height = match &self.expanded {
            Some(e) => e.height(),
            None => CARD_COLLAPSED_H,
        };
        self.snap = snap.clone();
    }
}

impl Expanded {
    fn new(ui: &Ui, card: &CardView, snap: &CardSnap) -> Self {
        let mtm = ui.mtm;
        let id = snap.id;
        let width = card.frame().size.width - 24.0;
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();
        let fixed_top =
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin;

        let title_field = NSTextField::textFieldWithString(&NSString::from_str(&snap.title), mtm);
        title_field.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe {
            NSFontWeightMedium
        })));
        title_field.setFrame(NSRect::new(
            NSPoint::new(10.0, 6.0),
            NSSize::new(width + 4.0, 24.0),
        ));
        title_field.setAutoresizingMask(fixed_top);
        if let Some(cell) = title_field.cell() {
            cell.setSendsActionOnEndEditing(true);
        }
        {
            let sink = ui.sink.clone();
            let field = ObjcWeak::from_retained(&title_field);
            let original = RefCell::new(snap.title.clone());
            let target = ActionTarget::new(mtm, move || {
                let Some(field) = field.load() else { return };
                let title = field.stringValue().to_string();
                let title = title.trim().to_string();
                if !title.is_empty() && title != *original.borrow() {
                    *original.borrow_mut() = title.clone();
                    sink(UiEvent::Rename(id, title));
                }
            });
            unsafe {
                title_field.setTarget(Some(&target));
                title_field.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(target)));
        }

        let notes_scroll = NSTextView::scrollableTextView(mtm);
        notes_scroll.setFrame(NSRect::new(
            NSPoint::new(12.0, 58.0),
            NSSize::new(width, 72.0),
        ));
        notes_scroll.setAutoresizingMask(fixed_top);
        notes_scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
        let notes = notes_scroll
            .documentView()
            .and_then(|v| v.downcast::<NSTextView>().ok())
            .expect("scrollable text view");
        notes.setRichText(false);
        notes.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        notes.setString(&NSString::from_str(&snap.notes));
        {
            let sink = ui.sink.clone();
            let original = RefCell::new(snap.notes.clone());
            let delegate = NotesDelegate::new(mtm, move |text| {
                if text != *original.borrow() {
                    *original.borrow_mut() = text.clone();
                    sink(UiEvent::SetNotes(id, text));
                }
            });
            notes.setDelegate(Some(delegate.as_delegate()));
            retained.push(Retained::into_super(Retained::into_super(delegate)));
        }

        let entries = label(mtm, "", 11.0, unsafe { NSFontWeightRegular });
        entries.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0,
            unsafe { NSFontWeightRegular },
        )));
        entries.setTextColor(Some(&NSColor::secondaryLabelColor()));
        entries.setAutoresizingMask(fixed_top);
        entries.setMaximumNumberOfLines(0);

        let mut button = |title: &str, event: UiEvent| {
            let sink = ui.sink.clone();
            let target = ActionTarget::new(mtm, move || sink(event.clone()));
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setControlSize(objc2_app_kit::NSControlSize::Small);
            b.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };
        let done = button(
            "Готово",
            UiEvent::Move {
                id,
                status: Status::Done,
                index: 0,
            },
        );
        let delete = button("Удалить", UiEvent::Delete(id));

        let views: [&NSView; 5] = [&title_field, &notes_scroll, &entries, &done, &delete];
        for v in views {
            v.setAlphaValue(0.0);
            card.addSubview(v);
        }
        animate(true, motion::EXPAND_SECS, || {
            for v in views {
                v.animator().setAlphaValue(1.0);
            }
        });

        Self {
            title_field,
            notes_scroll,
            notes,
            entries,
            done,
            delete,
            _retained: retained,
        }
    }

    fn entries_top() -> f64 {
        140.0
    }

    fn entries_height(&self) -> f64 {
        let lines = self
            .entries
            .stringValue()
            .to_string()
            .lines()
            .count()
            .max(1);
        lines as f64 * ENTRY_LINE_H
    }

    fn height(&self) -> f64 {
        Self::entries_top() + self.entries_height() + 8.0 + 24.0 + 12.0
    }

    fn update(&self, window: &NSWindow, snap: &CardSnap, status: Status) {
        // Never overwrite what the user is typing.
        if self.title_field.currentEditor().is_none() {
            self.title_field
                .setStringValue(&NSString::from_str(&snap.title));
        }
        let notes_focused = window.firstResponder().is_some_and(|r| {
            std::ptr::eq(
                &*r as *const _ as *const AnyObject,
                &*self.notes as *const _ as *const AnyObject,
            )
        });
        if !notes_focused && self.notes.string().to_string() != snap.notes {
            self.notes.setString(&NSString::from_str(&snap.notes));
        }
        let text = if snap.entries.is_empty() {
            "Записей времени пока нет".to_string()
        } else {
            snap.entries.join("\n")
        };
        self.entries.setStringValue(&NSString::from_str(&text));
        let width = self.notes_scroll.frame().size.width;
        let eh = self.entries_height();
        self.entries.setFrame(NSRect::new(
            NSPoint::new(12.0, Self::entries_top()),
            NSSize::new(width, eh),
        ));
        let y = Self::entries_top() + eh + 8.0;
        // A finished task has nothing to finish.
        let mut x = 10.0;
        self.done.setHidden(status == Status::Done);
        if status != Status::Done {
            let w = self.done.fittingSize().width.max(70.0);
            self.done
                .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 24.0)));
            x += w + 6.0;
        }
        let w = self.delete.fittingSize().width.max(70.0);
        self.delete
            .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 24.0)));
    }

    fn remove(self) {
        let views: Vec<Retained<NSView>> = vec![
            Retained::into_super(Retained::into_super(self.title_field)),
            Retained::into_super(self.notes_scroll),
            Retained::into_super(Retained::into_super(self.entries)),
            Retained::into_super(Retained::into_super(self.done)),
            Retained::into_super(Retained::into_super(self.delete)),
        ];
        let retained = self._retained;
        let fade = views.clone();
        animate_with_completion(
            motion::APPEAR_SECS * 0.6,
            move || {
                for v in &fade {
                    v.animator().setAlphaValue(0.0);
                }
            },
            move || {
                for v in &views {
                    v.removeFromSuperview();
                }
                let _keep = &retained;
            },
        );
    }
}

// ---- helpers ----

fn same(a: &NSView, b: &NSView) -> bool {
    std::ptr::eq(a, b)
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, weight: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    l
}

fn set_highlight(view: &ColumnView, on: bool) {
    if view.ivars().highlight.get() != on {
        view.ivars().highlight.set(on);
        view.setNeedsDisplay(true);
    }
}

fn timing() -> Retained<CAMediaTimingFunction> {
    let [a, b, c, d] = motion::SPRING_CURVE;
    CAMediaTimingFunction::functionWithControlPoints(a, b, c, d)
}

/// Runs `changes` inside an animation group (or right away when `animated` is false
/// or the system asks for reduced motion).
fn animate(animated: bool, secs: f64, changes: impl Fn()) {
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

fn animate_with_completion(secs: f64, changes: impl Fn(), done: impl Fn() + 'static) {
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
