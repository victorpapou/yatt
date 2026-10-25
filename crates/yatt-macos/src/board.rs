//! The cards, laid out and moved on springs ([`crate::physics`]) stepped by a
//! display-rate timer while anything moves.
//!
//! - Each column is a plain list by default in its own native scroll view (rubber
//!   band, momentum, scroller), the cards in its document view. A header button
//!   collapses the board into stacks like the reference clip
//!   (docs/reference/card-motion.md): the top card shows its text, the next
//!   ones peek out below it as slivers. A click fans the stack out into a
//!   rosette whose spread grows with the count; the card under the pointer lifts.
//!   Dragging the folded stack takes it whole.
//! - Every card is tinted glass: a blur of what is behind, the card's colour over
//!   it, a 1 px dark edge shadow (no stroke) and a soft white glow.
//! - Click opens a card, ⌘-click toggles it in the selection, ⇧-click selects a
//!   range within the column.
//! - Dragging one card is a plain drag: it follows the pointer and the list makes
//!   room. Dragging a selection blurs the pressed card's content and writes the
//!   target column and the count on it; the other selected cards follow right
//!   under it on stiff springs, almost at once.
//! - A two-finger swipe across a card drags it (or the selection) the same way;
//!   a vertical swipe scrolls the column.
//!
//! The canvas takes all pointer input and hit-tests the cards itself, handing
//! scroll gestures on to the column's scroll view. Stacks and fans sit on the canvas
//! (a fan is wider than its column). A card that changes home (dragged, moved to
//! another column, or switching between list and stack) rides the overlay above
//! everything and lands in its new home once it comes to rest, so it is never
//! clipped on the way.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAutoresizingMaskOptions, NSCursor, NSEvent, NSFont,
    NSFontWeightMedium, NSLineBreakMode, NSScrollElasticity, NSScrollView, NSScrollerStyle,
    NSTextAlignment, NSTextField, NSView, NSViewBoundsDidChangeNotification,
    NSViewLayerContentsRedrawPolicy, NSWindowOrderingMode,
};
use objc2_core_image::CIFilter;
use objc2_foundation::{
    NSArray, NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use objc2_quartz_core::CACurrentMediaTime;
use yatt_core::{Status, TaskId};

use crate::classes::{CanvasMsg, CanvasView, Fill, FillView, FlippedView, OverlayView, Scroll};
use crate::model::{CardSnap, ColumnSnap, UiEvent, format_duration, moves_into};
use crate::motion;
use crate::physics::{self, Body};
use crate::style::{self, CARD_H, CARD_W};
use crate::window::{Sink, animate};

/// Pixels the pointer (or fingers) must travel before a press becomes a drag.
const DRAG_THRESHOLD: f64 = 4.0;
/// Cards of a folded stack that draw: the top one and two slivers. The rest fade
/// out and leave the render tree until the stack fans or unfolds.
const STACK_SHOWN: usize = 3;
/// How far beyond a column's visible area list rows stay drawn, in screens:
/// enough that a fast scroll never shows an empty slot, few enough glass views.
const ROW_OVERSCAN: f64 = 0.5;
/// The longest a landing group keeps its flight order, in case a card never
/// comes to rest.
const LANDING_MAX_SECS: f64 = 2.0;
/// Room a fan keeps from the canvas's sides, when it has room.
const FAN_MARGIN: f64 = 8.0;
/// The most a click on a fan waits for a second one before folding, in seconds.
const FAN_FOLD_DELAY: f64 = 0.22;
/// A fanned card under the pointer grows a little, keeping its place in the order.
const HOVER_SCALE: f64 = 1.03;
/// The new-task field's text inset from the capsule's left edge.
const FIELD_TEXT_X: f64 = 20.0;
/// Content blur on a lifted card, in points.
const LIFT_BLUR: f64 = 5.0;
/// List layout: first slot's top (below the column chips, which float over the
/// cards), and the gap between cards.
const LIST_TOP: f64 = 52.0;
const LIST_GAP: f64 = 10.0;
const PITCH: f64 = CARD_H + LIST_GAP;
/// Room left under the last card for the new-task field floating over the board.
const LIST_BOTTOM: f64 = 96.0;
/// Dragging this close to the top or bottom edge scrolls the column.
const AUTOSCROLL_EDGE: f64 = 56.0;
const AUTOSCROLL_SPEED: f64 = 700.0;

/// Synthetic pointer input for demos, in canvas coordinates.
#[derive(Debug, Clone, Copy)]
pub enum DemoPointer {
    Down {
        at: (f64, f64),
        shift: bool,
        cmd: bool,
    },
    Drag((f64, f64)),
    Up((f64, f64)),
    Hover(Option<(f64, f64)>),
}

/// One column's horizontal extent on the canvas.
#[derive(Clone, Copy, Default)]
pub struct ColumnGeom {
    pub x: f64,
    pub w: f64,
}

/// Which view a card's view sits in; its body is in that view's coordinates (the
/// overlay's are the canvas's).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum Space {
    Canvas,
    Doc(usize),
    Overlay,
}

/// A column's native scroll view and the document view its list cards live in.
struct Column {
    scroll: Retained<NSScrollView>,
    doc: Retained<FlippedView>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    List,
    Stack { fanned: bool },
}

#[derive(Clone, PartialEq)]
enum Look {
    Solid,
    /// Leading a dragged group: content blurred, the target column written on it
    /// and, under it, how many cards are coming along.
    Lifted {
        column: String,
        count: usize,
    },
}

struct Card {
    view: Retained<FlippedView>,
    face: Retained<FillView>,
    content: Retained<FlippedView>,
    icon: Retained<NSView>,
    dots: Retained<NSTextField>,
    time: Retained<NSTextField>,
    title: Retained<NSTextField>,
    label: Retained<NSTextField>,
    /// "3 задачи" under the label while leading a group drag.
    count: Retained<NSTextField>,
    hue: usize,
    tracking: bool,
    selected: bool,
    body: Body,
    look: Look,
    z: f64,
    removing: bool,
    /// Where the view is now, and where it belongs once it comes to rest.
    space: Space,
    home: Space,
    /// Flying in from the new-task field.
    flying: bool,
    /// Drawn scale about the centre, easing towards `target_scale`.
    scale: f64,
    target_scale: f64,
    /// Text hidden: behind the top card of a stack.
    muted: bool,
    /// Riding under the lead of a group drag: no squash and stretch.
    light: bool,
    /// Carried: its lift shadow shows.
    lifted: bool,
    /// Holds still until then (a fan opening as a wave), in media time.
    hold_until: f64,
    /// The blur and the tint, sized to the card by hand while it grows.
    parts: [Retained<NSView>; 2],
    /// Growing from the new-task capsule: progress 0…1 and the capsule's size.
    grow: Option<(f64, NSSize)>,
    /// Faded out and out of the render tree; shared with the fade's completion.
    parked: Rc<Cell<bool>>,
    /// A list row far outside its column's visible area: hidden at once, no
    /// fade, until it scrolls near.
    culled: bool,
    /// The geometry last applied to the view; a card at rest is not touched again.
    placed: Cell<[f64; 8]>,
    /// The size the edge layers were last laid out for.
    shadow_size: Cell<(f64, f64)>,
    /// The scale the corners were last cut for.
    corner_scale: Cell<f64>,
    edges: style::Edges,
}

struct Press {
    id: Option<TaskId>,
    start: NSPoint,
    /// No modifiers: a click opens the card (or fans its stack).
    plain: bool,
    clicks: isize,
}

struct Drag {
    /// Lead first, then the cards that trail it.
    ids: Vec<TaskId>,
    /// More than one card: glass lead and a train of followers.
    group: bool,
    grab: (f64, f64),
    cursor: (f64, f64),
    target: Option<usize>,
    /// Where the cards go in the target column, counted without them.
    index: usize,
    /// The lead, held where it was grabbed.
    grip: physics::Grip,
}

/// A dropped group on its way into its column, all of it at once: it keeps the
/// stacking order it flew in until the last card is down.
struct Landing {
    /// Lead first, in the order they flew.
    ids: Vec<TaskId>,
    start: f64,
}

pub struct Board {
    mtm: MainThreadMarker,
    pub canvas: Retained<CanvasView>,
    /// Above the column headers; dragged cards ride here.
    pub overlay: Retained<OverlayView>,
    /// The canvas's top edge in overlay coordinates.
    overlay_dy: Cell<f64>,
    sink: Sink,
    cols: [Column; 3],
    /// Scroll observers of the columns, kept for as long as the board.
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    statuses: [Status; 3],
    titles: RefCell<[String; 3]>,
    geom: Cell<[ColumnGeom; 3]>,
    height: Cell<f64>,
    order: RefCell<[Vec<TaskId>; 3]>,
    cards: RefCell<HashMap<TaskId, Card>>,
    layouts: Cell<[Layout; 3]>,
    /// Selected cards, and the one a ⇧-click range starts from.
    selected: RefCell<HashSet<TaskId>>,
    anchor: Cell<Option<TaskId>>,
    press: RefCell<Option<Press>>,
    drag: RefCell<Option<Drag>>,
    /// The card order last applied to the views, and where each sat.
    stacking: RefCell<Vec<(TaskId, Space)>>,
    /// A card changed parent since the views were last ordered.
    restack_due: Cell<bool>,
    /// A dropped group flying in, one card after another.
    landing: RefCell<Option<Landing>>,
    /// `YATT_PERF`: when a stack was clicked open, until its first frame.
    fan_clicked: Cell<Option<f64>>,
    /// The card under the pointer, for the glass highlight in a fanned stack.
    hover: Cell<Option<TaskId>>,
    /// The card standing in for the open task modal, kept invisible.
    hidden: Cell<Option<TaskId>>,
    /// A task just typed into the new-task field: its card starts as the field's
    /// capsule, at this frame (canvas coordinates).
    spawn: Cell<Option<(NSRect, Status)>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<f64>,
    /// `YATT_PERF`: frame intervals and tick times (seconds) during a drag.
    perf: RefCell<Option<Vec<(f64, f64)>>>,
    /// Folds a fan after a single click, unless a double-click follows.
    unfan_timer: RefCell<Option<Retained<NSTimer>>>,
    /// When a fan last folded, to let the rest of a double-click pass.
    folded_at: Cell<f64>,
    /// Tells the window which column to tint while dragging.
    on_target: Box<dyn Fn(Option<usize>)>,
}

impl Board {
    pub fn new(
        mtm: MainThreadMarker,
        sink: Sink,
        on_target: impl Fn(Option<usize>) + 'static,
    ) -> Rc<Self> {
        let canvas = CanvasView::new(mtm);
        canvas.setClipsToBounds(true);
        let overlay = OverlayView::new(mtm);
        overlay.setClipsToBounds(false);
        let cols = [0, 1, 2].map(|_| {
            let scroll = NSScrollView::new(mtm);
            scroll.setDrawsBackground(false);
            scroll.setHasVerticalScroller(true);
            scroll.setHasHorizontalScroller(false);
            scroll.setAutohidesScrollers(true);
            scroll.setScrollerStyle(NSScrollerStyle::Overlay);
            scroll.setVerticalScrollElasticity(NSScrollElasticity::Allowed);
            scroll.setHorizontalScrollElasticity(NSScrollElasticity::None);
            scroll.contentView().setDrawsBackground(false);
            let doc = FlippedView::new(mtm, NSRect::ZERO);
            scroll.setDocumentView(Some(&doc));
            canvas.addSubview(&scroll);
            Column { scroll, doc }
        });
        let board = Rc::new(Self {
            mtm,
            canvas,
            overlay,
            overlay_dy: Cell::new(0.0),
            sink,
            cols,
            observers: RefCell::new(Vec::new()),
            statuses: Status::ALL,
            titles: RefCell::new(Default::default()),
            geom: Cell::new(Default::default()),
            height: Cell::new(0.0),
            order: RefCell::new(Default::default()),
            cards: RefCell::new(HashMap::new()),
            layouts: Cell::new([Layout::List; 3]),
            selected: RefCell::new(HashSet::new()),
            anchor: Cell::new(None),
            press: RefCell::new(None),
            drag: RefCell::new(None),
            hover: Cell::new(None),
            stacking: RefCell::new(Default::default()),
            restack_due: Cell::new(false),
            fan_clicked: Cell::new(None),
            landing: RefCell::new(None),
            hidden: Cell::new(None),
            spawn: Cell::new(None),
            timer: RefCell::new(None),
            last_tick: Cell::new(0.0),
            perf: RefCell::new(std::env::var_os("YATT_PERF").map(|_| Vec::new())),
            unfan_timer: RefCell::new(None),
            folded_at: Cell::new(f64::NEG_INFINITY),
            on_target: Box::new(on_target),
        });
        // Scrolling (momentum and rubber band included) shows the rows coming near.
        for col in &board.cols {
            let clip = col.scroll.contentView();
            clip.setPostsBoundsChangedNotifications(true);
            let weak = Rc::downgrade(&board);
            let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
                if let Some(b) = weak.upgrade() {
                    b.cull_lists();
                }
            });
            let token = unsafe {
                NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                    Some(NSViewBoundsDidChangeNotification),
                    Some(&clip),
                    None,
                    &block,
                )
            };
            board.observers.borrow_mut().push(token);
        }
        let weak = Rc::downgrade(&board);
        *board.canvas.ivars().on_msg.borrow_mut() = Some(Box::new(move |msg| {
            if let Some(b) = weak.upgrade() {
                b.on_msg(msg);
            }
        }));
        board
    }

    /// Column extents, the canvas height, and where the canvas's top is on the overlay.
    pub fn set_geometry(self: &Rc<Self>, geom: [ColumnGeom; 3], height: f64, overlay_dy: f64) {
        self.geom.set(geom);
        self.height.set(height);
        self.overlay_dy.set(overlay_dy);
        for (c, g) in self.cols.iter().zip(geom) {
            c.scroll.setFrame(NSRect::new(
                NSPoint::new(g.x, 0.0),
                NSSize::new(g.w, height),
            ));
        }
        self.retarget();
        // A resize moves the columns under the cards: put the cards there now
        // instead of letting them spring after.
        let dragged = self.dragged();
        for (id, card) in self.cards.borrow_mut().iter_mut() {
            if !dragged.contains(id) && !card.flying {
                card.body.settle();
                card.apply_frame(overlay_dy);
                if card.space == Space::Overlay {
                    land(card, self);
                }
            }
        }
        self.kick();
    }

    /// Brings cards in line with a snapshot: new ones fade in at their spot (or fly
    /// in from the new-task field), gone ones fade out, the rest spring to their places.
    pub fn render(self: &Rc<Self>, columns: &[ColumnSnap; 3]) {
        let dragging: HashSet<TaskId> = self.dragged().into_iter().collect();
        let mut seen = HashSet::new();
        {
            let mut titles = self.titles.borrow_mut();
            let mut order = self.order.borrow_mut();
            for (i, col) in columns.iter().enumerate() {
                titles[i] = col.title.clone();
                order[i] = col.cards.iter().map(|c| c.id).collect();
            }
        }
        for (ci, col) in columns.iter().enumerate() {
            for (i, snap) in col.cards.iter().enumerate() {
                seen.insert(snap.id);
                let exists = self.cards.borrow().contains_key(&snap.id);
                if !exists {
                    let mut card = self.make_card(snap);
                    let spawn = self.spawn.get().filter(|(_, s)| *s == col.status);
                    if let (Some((r, _)), 0) = (spawn, i) {
                        // Born as the new-task capsule, typed text and all; it grows
                        // into the card on the way to its slot.
                        self.spawn.set(None);
                        card.body = Body::at(r.origin.x, r.origin.y);
                        card.flying = true;
                        card.grow = Some((0.0, r.size));
                        card.space = Space::Overlay;
                        self.overlay.addSubview(&card.view);
                    } else {
                        let (x, y) = self.slot(ci, i);
                        card.body = Body::at(x, y - 8.0);
                        card.space = self.home_of(ci);
                        card.home = card.space;
                        self.parent(card.space).addSubview(&card.view);
                        card.view.setAlphaValue(0.0);
                        let view = card.view.clone();
                        animate(true, motion::APPEAR_SECS, move || {
                            view.animator().setAlphaValue(1.0)
                        });
                    }
                    self.cards.borrow_mut().insert(snap.id, card);
                }
                if let Some(card) = self.cards.borrow_mut().get_mut(&snap.id) {
                    card.removing = false;
                    card.update(snap);
                }
            }
        }
        self.selected.borrow_mut().retain(|id| seen.contains(id));
        let gone: Vec<TaskId> = self
            .cards
            .borrow()
            .iter()
            .filter(|(id, c)| !seen.contains(id) && !c.removing && !dragging.contains(id))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.remove_card(id);
        }
        self.retarget();
        self.kick();
    }

    /// The next new card of `status` starts as the new-task capsule at `capsule`
    /// (canvas coordinates) and grows into a card as it flies to its slot.
    pub fn spawn_from(&self, capsule: NSRect, status: Status) {
        self.spawn.set(Some((capsule, status)));
    }

    /// The card's frame on the canvas, unrotated, where it is headed.
    pub fn card_rect(&self, id: TaskId) -> Option<NSRect> {
        self.cards.borrow().get(&id).map(|c| {
            let (ox, oy) = self.offset(c.space);
            NSRect::new(
                NSPoint::new(c.body.tx + ox, c.body.ty + oy),
                NSSize::new(CARD_W, CARD_H),
            )
        })
    }

    /// Keeps a card invisible while the task modal stands in for it.
    pub fn set_hidden(&self, id: Option<TaskId>) {
        let old = self.hidden.replace(id);
        let cards = self.cards.borrow();
        if let Some(c) = old.and_then(|id| cards.get(&id)) {
            c.view.setAlphaValue(1.0);
        }
        if let Some(c) = id.and_then(|id| cards.get(&id)) {
            c.view.setAlphaValue(0.0);
        }
    }

    /// The card's rotation where it rests, in degrees.
    pub fn card_angle(&self, id: TaskId) -> f64 {
        self.cards.borrow().get(&id).map_or(0.0, |c| c.body.tr)
    }

    /// Development aid: scrolls a column's list to `y`.
    pub fn demo_scroll(&self, column: usize, y: f64) {
        let scroll = &self.cols[column].scroll;
        scroll.contentView().scrollToPoint(NSPoint::new(0.0, y));
        scroll.reflectScrolledClipView(&scroll.contentView());
    }

    /// Development aid: where a card's view is drawn, its centre in canvas
    /// coordinates.
    pub fn demo_view_center(&self, id: TaskId) -> Option<(f64, f64)> {
        let cards = self.cards.borrow();
        let card = cards.get(&id)?;
        let f = card.view.frame();
        let p = self.canvas.convertPoint_fromView(
            NSPoint::new(
                f.origin.x + f.size.width / 2.0,
                f.origin.y + f.size.height / 2.0,
            ),
            unsafe { card.view.superview() }.as_deref(),
        );
        Some((p.x, p.y))
    }

    /// Development aid: every card whose tint, backdrop or inner edge is not
    /// exactly the card's own size (a layer that would show round it as a halo).
    pub fn demo_misfit_parts(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (id, card) in self.cards.borrow().iter() {
            if card.view.isHidden() || card.grow.is_some() {
                continue;
            }
            let b = card.view.bounds().size;
            let parts: [(&str, &NSView); 3] = [
                ("backdrop", &card.parts[0]),
                ("tint", &card.face),
                ("inner edge", &card.edges.inner),
            ];
            for (name, v) in parts {
                let f = v.frame();
                if f.origin.x.abs() > 0.5
                    || f.origin.y.abs() > 0.5
                    || (f.size.width - b.width).abs() > 0.5
                    || (f.size.height - b.height).abs() > 0.5
                {
                    out.push(format!("{id:?} {name}: {f:?} in a card of {b:?}"));
                }
            }
        }
        out
    }

    /// Development aid: the lowest opacity among a column's drawn cards (the
    /// card and its text), and how many are drawn.
    pub fn demo_column_opacity(&self, column: usize) -> (f64, usize) {
        let cards = self.cards.borrow();
        let mut low = 1.0f64;
        let mut drawn = 0;
        for id in &self.order.borrow()[column] {
            if let Some(c) = cards.get(id)
                && !c.view.isHidden()
            {
                drawn += 1;
                low = low.min(c.view.alphaValue());
                if !c.muted {
                    low = low.min(c.content.alphaValue());
                }
            }
        }
        (low, drawn)
    }

    /// Development aid: selects exactly these cards.
    pub fn demo_select(self: &Rc<Self>, ids: &[TaskId]) {
        *self.selected.borrow_mut() = ids.iter().copied().collect();
        self.retarget();
        self.kick();
    }

    /// True while some stack is fanned out.
    pub fn is_fanned(&self) -> bool {
        self.layouts.get().contains(&Layout::Stack { fanned: true })
    }

    pub fn is_stacked(&self) -> bool {
        self.layouts.get()[0] != Layout::List
    }

    /// Switches the whole board between lists and collapsed stacks.
    pub fn toggle_stack(self: &Rc<Self>) {
        let layout = if self.is_stacked() {
            Layout::List
        } else {
            Layout::Stack { fanned: false }
        };
        self.layouts.set([layout; 3]);
        self.retarget();
        self.kick();
    }

    /// Esc: drops the selection and folds fanned stacks. Returns true when there was any.
    /// The selected cards in board order: column by column, top to bottom.
    pub fn selection(&self) -> Vec<TaskId> {
        let selected = self.selected.borrow();
        self.order
            .borrow()
            .iter()
            .flatten()
            .filter(|id| selected.contains(id))
            .copied()
            .collect()
    }

    /// The column the selection (or the last card clicked) is in.
    pub fn focused_column(&self) -> Option<usize> {
        self.selection()
            .first()
            .copied()
            .or(self.anchor.get())
            .and_then(|id| self.column_of(id))
    }

    /// Selects every card in `column`, or on the whole board.
    pub fn select_all(self: &Rc<Self>, column: Option<usize>) {
        let order = self.order.borrow().clone();
        *self.selected.borrow_mut() = match column {
            Some(c) => order[c].iter().copied().collect(),
            None => order.iter().flatten().copied().collect(),
        };
        self.retarget();
        self.kick();
    }

    pub fn clear_selection(self: &Rc<Self>) -> bool {
        let had = !self.selected.borrow().is_empty() || self.unfan();
        self.selected.borrow_mut().clear();
        self.retarget();
        self.kick();
        had
    }

    pub fn demo_center(&self, id: TaskId) -> Option<(f64, f64)> {
        self.card_rect(id).map(|r| {
            (
                r.origin.x + r.size.width / 2.0,
                r.origin.y + r.size.height / 2.0,
            )
        })
    }

    pub fn demo_column_x(&self, column: usize) -> f64 {
        let g = self.geom.get()[column];
        g.x + g.w / 2.0
    }

    pub fn demo_pointer(self: &Rc<Self>, input: DemoPointer) {
        let pt = |(x, y): (f64, f64)| NSPoint::new(x, y);
        let msg = match input {
            DemoPointer::Down { at, shift, cmd } => CanvasMsg::Down {
                at: pt(at),
                shift,
                cmd,
                clicks: 1,
            },
            DemoPointer::Drag(at) => CanvasMsg::Dragged(pt(at)),
            DemoPointer::Up(at) => CanvasMsg::Up(pt(at)),
            DemoPointer::Hover(at) => CanvasMsg::Hover(at.map(pt)),
        };
        self.on_msg(msg);
    }

    fn dragged(&self) -> Vec<TaskId> {
        self.drag
            .borrow()
            .as_ref()
            .map(|d| d.ids.clone())
            .unwrap_or_default()
    }

    fn column_of(&self, id: TaskId) -> Option<usize> {
        self.order.borrow().iter().position(|o| o.contains(&id))
    }

    fn column_at(&self, x: f64) -> Option<usize> {
        self.geom
            .get()
            .iter()
            .position(|g| x >= g.x && x < g.x + g.w)
    }

    /// Where the `i`-th card of a column sits, ignoring drags.
    fn slot(&self, ci: usize, i: usize) -> (f64, f64) {
        match self.layouts.get()[ci] {
            Layout::List => (self.list_x(ci), LIST_TOP + i as f64 * PITCH),
            Layout::Stack { .. } => {
                let a = self.stack_anchor(ci);
                (a.0, a.1 + physics::pile(i).0)
            }
        }
    }

    /// A list card's left edge in its column's document view.
    fn list_x(&self, ci: usize) -> f64 {
        physics::centred_left(0.0, self.geom.get()[ci].w, CARD_W)
    }

    /// Where a column's cards live: its scroll view's document as a list, the
    /// canvas as a stack.
    fn home_of(&self, ci: usize) -> Space {
        match self.layouts.get()[ci] {
            Layout::List => Space::Doc(ci),
            Layout::Stack { .. } => Space::Canvas,
        }
    }

    fn parent(&self, space: Space) -> &NSView {
        match space {
            Space::Canvas => &self.canvas,
            Space::Doc(ci) => &self.cols[ci].doc,
            Space::Overlay => &self.overlay,
        }
    }

    /// Where a space's origin is on the canvas (the overlay shares the canvas's).
    fn offset(&self, space: Space) -> (f64, f64) {
        match space {
            Space::Doc(ci) => {
                let p = self
                    .canvas
                    .convertPoint_fromView(NSPoint::ZERO, Some(&self.cols[ci].doc));
                (p.x, p.y)
            }
            _ => (0.0, 0.0),
        }
    }

    /// A column's scroll offset (negative or past the end while rubber-banding).
    fn scrolled(&self, ci: usize) -> f64 {
        self.cols[ci].scroll.contentView().bounds().origin.y
    }

    /// Top-left of a stack's top card, centred in the column.
    fn stack_anchor(&self, ci: usize) -> (f64, f64) {
        let g = self.geom.get()[ci];
        (physics::centred_left(g.x, g.w, CARD_W), LIST_TOP + 10.0)
    }

    // ---- cards ----

    fn make_card(self: &Rc<Self>, snap: &CardSnap) -> Card {
        let mtm = self.mtm;
        let hue = style::hue_of(snap.id);
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(CARD_W, CARD_H));
        let view = FlippedView::new(mtm, bounds);
        // Scaling shrinks the frame and keeps card-sized bounds; the parts must not
        // follow the frame, or they would shrink twice.
        view.setAutoresizesSubviews(false);
        view.setWantsLayer(true);
        let fill_mask = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;

        let face = FillView::new(
            mtm,
            Fill::Card {
                hue,
                tracking: false,
                selected: false,
            },
        );
        face.setFrame(bounds);
        face.setAutoresizingMask(fill_mask);

        // Real blur of whatever is behind the card inside the window.
        let glass = style::card_blur(mtm);
        glass.setFrame(bounds);
        glass.setAutoresizingMask(fill_mask);

        let Face {
            content,
            icon,
            dots,
            time,
            title,
        } = card_face(mtm, snap.id, &snap.title, hue);
        content.setAutoresizingMask(fill_mask);

        // While leading a group drag: the target column, and how many cards come along.
        let label = text_label(mtm, "", 22.0, unsafe { NSFontWeightMedium });
        label.setAlignment(NSTextAlignment::Center);
        label.setTextColor(Some(&style::text(hue)));
        label.setAlphaValue(0.0);
        let count = text_label(mtm, "", 13.0, unsafe { NSFontWeightMedium });
        count.setAlignment(NSTextAlignment::Center);
        count.setTextColor(Some(&style::text(hue).colorWithAlphaComponent(0.7)));
        count.setAlphaValue(0.0);

        // The edge: the outline under everything, the white inner shadow over the
        // tint and under the text.
        let edges = style::Edges::new(mtm);
        for v in [
            &*edges.lift,
            &edges.ring,
            &edges.outer,
            &glass,
            &face,
            &edges.inner,
            &content,
            &label,
            &count,
        ] {
            view.addSubview(v);
        }
        // Squash, stretch and scale change the parts' sizes every frame while a
        // card moves; redrawing the tint and the text for a few percent would cost
        // a full redisplay per card per frame, so their pixels stretch instead and
        // they redraw only when their content changes.
        let policy = NSViewLayerContentsRedrawPolicy::OnSetNeedsDisplay;
        for v in [&*face as &NSView, &content, &label, &count] {
            v.setLayerContentsRedrawPolicy(policy);
        }
        // The text only: the glyph is a layer of its own, whose picture AppKit
        // must not take over (it would drop it).
        for v in [&*time as &NSView, &title, &dots] {
            v.setLayerContentsRedrawPolicy(policy);
        }

        let parts = [glass.clone(), Retained::into_super(face.clone())];
        let mut card = Card {
            view,
            face,
            content,
            icon,
            dots,
            time,
            title,
            label,
            count,
            hue,
            tracking: false,
            selected: false,
            body: Body::at(0.0, 0.0),
            look: Look::Lifted {
                column: String::new(),
                count: 0,
            },
            z: 0.0,
            removing: false,
            space: Space::Canvas,
            home: Space::Canvas,
            flying: false,
            scale: 1.0,
            target_scale: 1.0,
            muted: false,
            light: false,
            lifted: false,
            hold_until: 0.0,
            parts,
            grow: None,
            shadow_size: Cell::new((0.0, 0.0)),
            corner_scale: Cell::new(f64::NAN),
            placed: Cell::new([f64::NAN; 8]),
            parked: Rc::new(Cell::new(false)),
            culled: false,
            edges,
        };
        card.set_look(Look::Solid, false);
        card
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
        crate::window::animate_with_completion(
            motion::APPEAR_SECS,
            move || view.animator().setAlphaValue(0.0),
            move || {
                if let Some(b) = weak.upgrade() {
                    let mut cards = b.cards.borrow_mut();
                    if cards.get(&id).is_some_and(|c| c.removing) {
                        done_view.removeFromSuperview();
                        cards.remove(&id);
                    }
                }
            },
        );
    }

    // ---- layout ----

    /// Recomputes every card's target, springiness, look and stacking order.
    fn retarget(&self) {
        let drag = self.drag.borrow();
        let drag_ids = drag.as_ref().map(|d| d.ids.clone()).unwrap_or_default();
        let order = self.order.borrow().clone();
        let layouts = self.layouts.get();
        let selected = self.selected.borrow();
        let hover = self.hover.get();
        // Cards that need not draw: deep in a folded stack, or far down a dragged
        // group's tail, right under the cards in front.
        let mut parked: HashSet<TaskId> = HashSet::new();
        let mut at_once: HashSet<TaskId> = HashSet::new();
        let mut cards = self.cards.borrow_mut();
        let (w, h) = (CARD_W, CARD_H);

        for (ci, ids) in order.iter().enumerate() {
            let ids: Vec<TaskId> = ids
                .iter()
                .copied()
                .filter(|id| !drag_ids.contains(id))
                .collect();
            let n = ids.len();
            match layouts[ci] {
                Layout::Stack { fanned } if fanned && n > 1 => {
                    // The swatch fan of the reference, newest card on top, shrunk
                    // as a whole (angles unchanged) to fit its column and the board's
                    // height, and centred on the column below the chips.
                    let g = self.geom.get()[ci];
                    let room = (
                        g.w - 2.0 * FAN_MARGIN,
                        self.height.get() - LIST_TOP - 2.0 * FAN_MARGIN,
                    );
                    let (shown_n, scale) = physics::fan_fit(n, (w, h), room);
                    let (x0, y0, x1, _) = physics::fan_bounds(shown_n, (w, h));
                    let centre = (
                        g.x + g.w / 2.0 - (x0 + x1) / 2.0 * scale,
                        LIST_TOP + FAN_MARGIN - y0 * scale,
                    );
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        // Twelve cards at most; the rest wait, hidden, on the stack.
                        let shown = i < shown_n;
                        let (dx, dy, deg) = if shown {
                            let (dx, dy, deg) = physics::fan(i, shown_n, w);
                            (dx * scale, dy * scale, deg)
                        } else {
                            parked.insert(*id);
                            (0.0, 0.0, 0.0)
                        };
                        let at = (centre.0 + dx - w / 2.0, centre.1 + dy - h / 2.0, deg);
                        // A new place: the cards set off as a short wave.
                        let moved =
                            (card.body.tx - at.0).abs() > 2.0 || (card.body.ty - at.1).abs() > 2.0;
                        self.send_home(card, Space::Canvas, at);
                        if moved && shown {
                            card.hold_until = CACurrentMediaTime() + physics::fan_wave(i, shown_n);
                        }
                        card.body.spring = physics::FAN_OPEN;
                        card.set_muted(false);
                        // Newer cards on top, as on the stack.
                        card.z = if shown { (n - i) as f64 } else { 0.0 };
                        card.target_scale =
                            scale * if hover == Some(*id) { HOVER_SCALE } else { 1.0 };
                        card.set_look(Look::Solid, true);
                    }
                }
                Layout::Stack { .. } => {
                    let a = self.stack_anchor(ci);
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let (dy, scale) = physics::pile(i);
                        self.send_home(card, Space::Canvas, (a.0, a.1 + dy, 0.0));
                        // Only the top card's text; the rest show as coloured slivers.
                        card.set_muted(i > 0);
                        if i >= STACK_SHOWN {
                            parked.insert(*id);
                        }
                        card.body.spring = physics::FAN_FOLD;
                        card.z = (n - i) as f64;
                        card.target_scale = scale;
                        card.set_look(Look::Solid, true);
                    }
                }
                Layout::List => {
                    // Make room where the dragged cards would land.
                    let gap = match drag.as_ref() {
                        // One card's room, however many come: they land as a stack.
                        Some(d) if d.target == Some(ci) => Some((d.index, 1)),
                        _ => None,
                    };
                    let x = self.list_x(ci);
                    // The document is as tall as the list (room for the gap included).
                    let slots = n + gap.map_or(0, |(_, k)| k);
                    let doc_h =
                        (LIST_TOP + slots as f64 * PITCH + LIST_BOTTOM).max(self.height.get());
                    let doc = &self.cols[ci].doc;
                    if doc.frame().size.height != doc_h
                        || doc.frame().size.width != self.geom.get()[ci].w
                    {
                        motion::quietly(|| {
                            doc.setFrameSize(NSSize::new(self.geom.get()[ci].w, doc_h))
                        });
                    }
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let slot = match gap {
                            Some((at, k)) if i >= at => i + k,
                            _ => i,
                        };
                        let y = LIST_TOP + slot as f64 * PITCH;
                        self.send_home(card, Space::Doc(ci), (x, y, 0.0));
                        // Rows far out of sight jump to their slot: only what can be
                        // seen animates, however long the list.
                        if card.space == Space::Doc(ci)
                            && !self.row_near(ci, y)
                            && !self.row_near(ci, card.body.y)
                        {
                            card.body.settle();
                        }
                        card.set_muted(false);
                        // Arriving cards land (and swing a little); the ones already
                        // there just glide over to make room.
                        card.body.spring = if card.space == Space::Doc(ci) {
                            physics::SHIFT
                        } else {
                            physics::SLOT
                        };
                        card.z = 1.0;
                        card.target_scale = 1.0;
                        card.set_look(Look::Solid, true);
                    }
                }
            }
        }

        if let Some(drag) = drag.as_ref() {
            let label = drag
                .target
                .map(|c| self.titles.borrow()[c].clone())
                .unwrap_or_default();
            for (i, id) in drag.ids.iter().enumerate() {
                let Some(card) = cards.get_mut(id) else {
                    continue;
                };
                card.z = physics::group_z(i);
                card.set_muted(false);
                card.target_scale = if i == 0 { 1.0 } else { physics::trail(i).1 };
                if i == 0 {
                    card.set_lifted(true);
                    card.body.spring = physics::LEAD;
                    let look = if drag.group {
                        Look::Lifted {
                            column: label.clone(),
                            count: drag.ids.len(),
                        }
                    } else {
                        Look::Solid
                    };
                    card.set_look(look, true);
                } else {
                    // Followers replay the lead's path in `tick`.
                    card.set_look(Look::Solid, true);
                    // Under the lead, the lead's blur is the whole group's.
                    card.set_light(true);
                }
            }
        }

        let followers: HashSet<TaskId> = drag
            .as_ref()
            .map(|d| d.ids.iter().skip(1).copied().collect())
            .unwrap_or_default();
        if let Some(d) = drag.as_ref() {
            parked.extend(d.ids.iter().skip(1 + physics::TRAIL_SHOWN).copied());
            // These never come to rest while dragged: out of the tree at once.
            at_once.extend(d.ids.iter().skip(1 + physics::TRAIL_SHOWN).copied());
        }
        // A landing group keeps the stacking order it flew in until the last of
        // its cards is down.
        if let Some(l) = self.landing.borrow().as_ref() {
            for (k, id) in l.ids.iter().enumerate() {
                if let Some(card) = cards.get_mut(id) {
                    card.z = physics::group_z(k);
                }
            }
        }
        let lead = drag.as_ref().and_then(|d| d.ids.first().copied());
        for (id, card) in cards.iter_mut() {
            if !followers.contains(id) {
                card.set_light(false);
            }
            // Only the carried card (the head of a group) casts the lift shadow.
            if lead != Some(*id) {
                card.set_lifted(false);
            }
            card.set_parked(parked.contains(id), at_once.contains(id));
            if card.flying {
                card.body.spring = physics::FLY;
                card.z = 900.0;
            }
            // The selection ring shows while choosing, not while dragging.
            card.set_selected(physics::ring_shown(selected.contains(id), drag.is_some()));
        }
        self.restack(&cards);
        drop(cards);
        self.cull_lists();
    }

    /// Hides list rows more than half a screen above or below their column's
    /// visible area and shows the ones that come near: a long list costs what one screen
    /// of it does. Runs after layout changes and on every scroll.
    fn cull_lists(&self) {
        thread_local! {
            // `YATT_NO_CULL=1`: draw every row, to measure what culling saves.
            static OFF: bool = std::env::var_os("YATT_NO_CULL").is_some();
        }
        if OFF.with(|off| *off) {
            return;
        }
        let Ok(mut cards) = self.cards.try_borrow_mut() else {
            return;
        };
        let layouts = self.layouts.get();
        let drag_ids = self.dragged();
        let overlay_dy = self.overlay_dy.get();
        for (ci, layout) in layouts.iter().enumerate() {
            let list = *layout == Layout::List;
            let near = |y: f64| self.row_near(ci, y);
            for id in self.order.borrow()[ci].iter() {
                let Some(card) = cards.get_mut(id) else {
                    continue;
                };
                let far = list
                    && card.space == Space::Doc(ci)
                    && !drag_ids.contains(id)
                    && !near(card.body.y)
                    && !near(card.body.ty);
                if card.set_culled(far) && !far {
                    card.apply_frame(overlay_dy);
                }
            }
        }
    }

    /// Whether a row at `y` in column `ci`'s list is within [`ROW_OVERSCAN`] of
    /// what the column shows.
    fn row_near(&self, ci: usize, y: f64) -> bool {
        let clip = self.cols[ci].scroll.contentView().bounds();
        let overscan = ROW_OVERSCAN * clip.size.height;
        y + CARD_H >= clip.origin.y - overscan && y <= clip.origin.y + clip.size.height + overscan
    }

    /// Points a card at its place `(x, y, degrees)` in `home`'s coordinates. A card
    /// away from home flies there on the overlay (in canvas coordinates) and lands
    /// once it is at rest.
    fn send_home(&self, card: &mut Card, home: Space, (x, y, deg): (f64, f64, f64)) {
        card.home = home;
        if card.space != home && card.space != Space::Overlay {
            relocate(card, Space::Overlay, self);
        }
        let (ox, oy) = if card.space == home {
            (0.0, 0.0)
        } else {
            self.offset(home)
        };
        card.body.target(x + ox, y + oy, deg);
    }

    /// Orders each parent's subviews by the cards' `z`. Layer zPosition alone does
    /// not hold once a view has moved between parents.
    fn restack(&self, cards: &HashMap<TaskId, Card>) {
        let mut sorted: Vec<(&TaskId, &Card)> = cards.iter().collect();
        sorted.sort_by(|a, b| a.1.z.total_cmp(&b.1.z).then(a.0.cmp(b.0)));
        let key: Vec<(TaskId, Space)> = sorted.iter().map(|(id, c)| (**id, c.space)).collect();
        if *self.stacking.borrow() == key {
            return;
        }
        let spaces = [
            Space::Canvas,
            Space::Doc(0),
            Space::Doc(1),
            Space::Doc(2),
            Space::Overlay,
        ];
        for space in spaces {
            // The columns' scroll views stay at the bottom of the canvas.
            let mut views: Vec<Retained<NSView>> = if space == Space::Canvas {
                self.cols
                    .iter()
                    .map(|c| Retained::into_super(c.scroll.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            views.extend(
                sorted
                    .iter()
                    .filter(|(_, c)| c.space == space)
                    .map(|(_, c)| Retained::into_super(c.view.clone())),
            );
            let parent = self.parent(space);
            motion::quietly(|| parent.setSubviews(&NSArray::from_retained_slice(&views)));
        }
        *self.stacking.borrow_mut() = key;
    }

    // ---- motion ----

    /// Starts the frame timer if it is not running.
    fn kick(self: &Rc<Self>) {
        if self.timer.borrow().is_some() {
            return;
        }
        self.last_tick.set(CACurrentMediaTime());
        let weak = Rc::downgrade(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(b) = weak.upgrade() {
                b.tick();
            }
        });
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0 / 120.0, true, &block)
        };
        *self.timer.borrow_mut() = Some(timer);
    }

    fn tick(self: &Rc<Self>) {
        let now = CACurrentMediaTime();
        let dt = (now - self.last_tick.replace(now)).clamp(0.0, 1.0 / 30.0);
        let reduce = motion::reduce_motion();
        self.autoscroll(dt);
        let dragging = self.drag.borrow().is_some();
        let overlay_dy = self.overlay_dy.get();
        let mut cards = self.cards.borrow_mut();

        if let Some(drag) = self.drag.borrow_mut().as_mut() {
            // The lead chases the pointer; every other card of the group chases the
            // lead itself on a very stiff spring, so it stays exactly under it.
            // The lead is held at the point it was grabbed and turns about it as
            // it is moved (`physics::Grip`); each follower hangs off the card
            // ahead of it, a chain on strings, its angle swinging on its own
            // (`Body::step`).
            let pointer = (drag.cursor.0 - drag.grab.0, drag.cursor.1 - drag.grab.1);
            if let Some(lead) = drag.ids.first().and_then(|id| cards.get_mut(id)) {
                let s = lead.scale;
                drag.grip.step(drag.cursor, dt, (CARD_W * s, CARD_H * s));
                let (cx, cy) = drag.grip.centre();
                let (x, y, r) = (cx - CARD_W / 2.0, cy - CARD_H / 2.0, drag.grip.theta);
                let spring = lead.body.spring;
                lead.body = Body {
                    spring,
                    r,
                    tr: r,
                    ..Body::at(x, y)
                };
            }
            let at: Vec<Option<(f64, f64)>> = drag
                .ids
                .iter()
                .map(|id| cards.get(id).map(|c| (c.body.x, c.body.y)))
                .collect();
            let chain = physics::chain_spring(drag.ids.len());
            for (i, id) in drag.ids.iter().enumerate() {
                let Some(card) = cards.get_mut(id) else {
                    continue;
                };
                if i == 0 {
                    continue;
                }
                let (tx, ty) = at[i - 1].unwrap_or(pointer);
                card.body.spring = chain;
                card.body.target(tx, ty + physics::trail(i).0, 0.0);
            }
        }

        let mut moving = dragging;
        let drag_ids = self.dragged();
        let started = now;
        for card in cards.values_mut() {
            // A culled row is out of sight: it lands at once and draws when it
            // comes near.
            // Parked and arrived under the stack: now it can leave the tree.
            if card.parked.get() && !card.view.isHidden() && card.body.at_rest() {
                card.view.setHidden(true);
            }
            // Out of sight (a culled row, or a card hidden in a stack or under a
            // dragged group): it lands at once and draws when it shows again.
            // Waiting for its turn in a fan's wave.
            if card.hold_until > now {
                moving = true;
                continue;
            }
            if card.culled || (card.parked.get() && card.view.isHidden()) {
                card.body.settle();
                card.scale = card.target_scale;
                continue;
            }
            // A card at rest on its place, at its size, has nothing to step. It may
            // still have been put somewhere new (a single dragged card is set right
            // on the pointer, at rest there), so its frame is applied; that returns
            // at once when nothing changed.
            if card.grow.is_none()
                && card.scale == card.target_scale
                && card.body.x == card.body.tx
                && card.body.y == card.body.ty
                && card.body.r == card.body.tr
                && card.body.vx == 0.0
                && card.body.vy == 0.0
                && card.body.vr == 0.0
            {
                card.apply_frame(overlay_dy);
                continue;
            }
            if reduce && !dragging {
                card.body.settle();
            } else {
                card.body.step(dt);
            }
            let ease = if reduce {
                1.0
            } else {
                1.0 - (-dt * 16.0).exp()
            };
            card.scale += (card.target_scale - card.scale) * ease;
            if let Some((t, from)) = card.grow {
                // Slower than the scale, and with no overshoot: a smooth morph.
                let t = if reduce {
                    1.0
                } else {
                    t + (1.0 - t) * (1.0 - (-dt * 7.0).exp())
                };
                if t > 0.995 {
                    card.end_grow();
                } else {
                    card.grow = Some((t, from));
                    moving = true;
                }
            }
            if (card.scale - card.target_scale).abs() < 0.002 {
                card.scale = card.target_scale;
            } else {
                moving = true;
            }
            if card.body.at_rest() {
                card.body.settle();
                card.flying = false;
            } else {
                moving = true;
            }
            card.apply_frame(overlay_dy);
        }
        let mut land_now = false;
        // A dropped group lands together; it keeps its stacking order until the
        // last of its cards is down in its column.
        if let Some(l) = self.landing.borrow().as_ref() {
            let down = l
                .ids
                .iter()
                .all(|id| cards.get(id).is_none_or(|c| c.space != Space::Overlay));
            if down || now - l.start > LANDING_MAX_SECS {
                land_now = true;
            } else {
                moving = true;
            }
        }
        // Cards that came to rest away from home land there, but only once every
        // card on its way to the same home has: one that landed early would drop
        // under those still on the overlay and show behind cards it should cover.
        let busy: HashSet<Space> = cards
            .iter()
            .filter(|(id, c)| {
                c.space == Space::Overlay
                    && (c.flying || drag_ids.contains(id) || !c.body.at_rest())
            })
            .map(|(_, c)| c.home)
            .collect();
        for (id, card) in cards.iter_mut() {
            if card.space == Space::Overlay
                && !card.flying
                && !drag_ids.contains(id)
                && card.body.at_rest()
                && !busy.contains(&card.home)
            {
                land(card, self);
                card.apply_frame(overlay_dy);
            }
        }
        if self.restack_due.replace(false) {
            self.restack(&cards);
        }
        drop(cards);
        if land_now {
            *self.landing.borrow_mut() = None;
            self.retarget();
        }
        if let Some(t0) = self.fan_clicked.take()
            && self.perf.borrow().is_some()
        {
            eprintln!(
                "perf: stack click to its first fan frame: {:.1} ms",
                (CACurrentMediaTime() - t0) * 1000.0
            );
        }
        if dragging && let Some(perf) = self.perf.borrow_mut().as_mut() {
            perf.push((dt, CACurrentMediaTime() - started));
        }
        if !moving && let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
    }

    /// While dragging near the top or bottom edge of a list, scroll it.
    fn autoscroll(self: &Rc<Self>, dt: f64) {
        let Some((ci, y)) = self
            .drag
            .borrow()
            .as_ref()
            .and_then(|d| d.target.map(|t| (t, d.cursor.1)))
        else {
            return;
        };
        if self.layouts.get()[ci] != Layout::List {
            return;
        }
        let h = self.height.get();
        let speed = if y > h - AUTOSCROLL_EDGE {
            (y - (h - AUTOSCROLL_EDGE)) / AUTOSCROLL_EDGE
        } else if y < LIST_TOP + AUTOSCROLL_EDGE {
            -(LIST_TOP + AUTOSCROLL_EDGE - y) / AUTOSCROLL_EDGE
        } else {
            return;
        };
        let scroll = &self.cols[ci].scroll;
        let clip = scroll.contentView();
        let max = (self.cols[ci].doc.frame().size.height - clip.bounds().size.height).max(0.0);
        let from = self.scrolled(ci);
        let to = (from + speed.clamp(-1.0, 1.0) * AUTOSCROLL_SPEED * dt).clamp(0.0, max);
        if to != from {
            clip.scrollToPoint(NSPoint::new(0.0, to));
            scroll.reflectScrolledClipView(&clip);
            let cursor = self.drag.borrow().as_ref().map(|d| d.cursor);
            if let Some((x, y)) = cursor {
                self.drag_to(NSPoint::new(x, y));
            }
        }
    }

    // ---- input ----

    fn on_msg(self: &Rc<Self>, msg: CanvasMsg) {
        match msg {
            CanvasMsg::Down {
                at,
                shift,
                cmd,
                clicks,
            } => self.on_down(at, shift, cmd, clicks),
            CanvasMsg::Dragged(p) => {
                if self.drag.borrow().is_none() {
                    let Some(press) = self.press.borrow_mut().take() else {
                        crate::eventlog::log("board: dragged, but no press: not dragging");
                        return;
                    };
                    let d = (p.x - press.start.x).hypot(p.y - press.start.y);
                    let far = d >= DRAG_THRESHOLD;
                    match press.id {
                        Some(id) if far => {
                            crate::eventlog::log(format!("board: drag starts with {id:?}"));
                            self.begin_drag(id, press.start, p)
                        }
                        _ => {
                            crate::eventlog::log(format!(
                                "board: dragged {d:.1} pt, {}",
                                if press.id.is_none() {
                                    "pressed on no card"
                                } else {
                                    "below the threshold"
                                }
                            ));
                            *self.press.borrow_mut() = Some(press);
                            return;
                        }
                    }
                }
                self.drag_to(p);
            }
            CanvasMsg::Up(p) => {
                crate::eventlog::log(format!(
                    "board: up at ({:.0}, {:.0}), {}",
                    p.x,
                    p.y,
                    if self.drag.borrow().is_some() {
                        "drop"
                    } else {
                        "click"
                    }
                ));
                if self.drag.borrow().is_some() {
                    self.drop_at(p);
                } else if let Some(press) = self.press.borrow_mut().take()
                    && press.plain
                {
                    self.click(press.id, press.clicks);
                }
            }
            CanvasMsg::Scroll(s) => self.on_scroll(s),
            CanvasMsg::Hover(p) => {
                let id = p.and_then(|p| self.hit(p));
                if p.is_some() {
                    self.show_cursor(id);
                }
                if self.drag.borrow().is_none() && self.hover.replace(id) != id {
                    self.retarget();
                    self.kick();
                }
            }
        }
    }

    fn on_down(self: &Rc<Self>, at: NSPoint, shift: bool, cmd: bool, clicks: isize) {
        let id = self.hit(at);
        if crate::eventlog::enabled() {
            let what = id.and_then(|id| {
                let cards = self.cards.borrow();
                let c = cards.get(&id)?;
                Some(format!(
                    "card {id:?} in {:?} at ({:.0}, {:.0}) scale {:.2} hidden {}",
                    c.space,
                    c.body.x,
                    c.body.y,
                    c.scale,
                    c.view.isHidden()
                ))
            });
            crate::eventlog::log(format!(
                "board: down at ({:.0}, {:.0}) clicks {clicks} shift {shift} cmd {cmd} on {}; layouts {:?}",
                at.x,
                at.y,
                what.unwrap_or_else(|| "no card".into()),
                self.layouts.get()
            ));
        }
        if let Some(id) = id {
            if cmd {
                let mut sel = self.selected.borrow_mut();
                if !sel.remove(&id) {
                    sel.insert(id);
                }
                self.anchor.set(Some(id));
            } else if shift {
                self.select_range(id);
            }
            if cmd || shift {
                self.retarget();
            }
        }
        *self.press.borrow_mut() = Some(Press {
            id,
            start: at,
            plain: !cmd && !shift,
            clicks,
        });
    }

    /// ⇧-click: everything between the anchor and `id` in the anchor's column.
    fn select_range(&self, id: TaskId) {
        let order = self.order.borrow();
        let col = self.column_of(id);
        let range = self.anchor.get().and_then(|a| {
            let ids = &order[col?];
            let (i, j) = (
                ids.iter().position(|x| *x == a)?,
                ids.iter().position(|x| *x == id)?,
            );
            Some(ids[i.min(j)..=i.max(j)].to_vec())
        });
        let mut sel = self.selected.borrow_mut();
        match range {
            Some(ids) => sel.extend(ids),
            None => {
                sel.insert(id);
                self.anchor.set(Some(id));
            }
        }
    }

    /// A plain click: fan a folded stack, open a card, or clear on empty board.
    fn click(self: &Rc<Self>, id: Option<TaskId>, clicks: isize) {
        let Some(id) = id else {
            // On empty board a click folds a fan at once and clears the selection.
            self.cancel_unfan();
            if self.unfan() {
                self.folded_at.set(CACurrentMediaTime());
                self.retarget();
                self.kick();
            }
            self.clear_selection();
            return;
        };
        let col = self.column_of(id);
        let mut layouts = self.layouts.get();
        // In a fanned stack a click folds it back and a double-click opens the card.
        // The first click of a double waits out the double-click interval, so the
        // fan stays put for the second one.
        if layouts.contains(&Layout::Stack { fanned: true }) {
            // The click that fanned the stack out and the next one also add up to a
            // double-click; only a double-click made on the fan opens the card.
            let first_on_fan = self.unfan_timer.borrow().is_some();
            if physics::fan_click_opens(clicks, first_on_fan) {
                self.cancel_unfan();
                self.selected.borrow_mut().clear();
                self.retarget();
                (self.sink)(UiEvent::ToggleExpand(id));
            } else {
                self.unfan_soon();
            }
            return;
        }
        // The second click of a double whose first folded the fan: done already.
        if clicks >= 2
            && CACurrentMediaTime() - self.folded_at.get() < NSEvent::doubleClickInterval()
        {
            return;
        }
        if let Some(ci) = col
            && layouts[ci] == (Layout::Stack { fanned: false })
            && self.order.borrow()[ci].len() > 1
        {
            layouts[ci] = Layout::Stack { fanned: true };
            self.layouts.set(layouts);
            self.fan_clicked.set(Some(CACurrentMediaTime()));
            self.retarget();
            self.kick();
            return;
        }
        self.selected.borrow_mut().clear();
        self.anchor.set(Some(id));
        self.retarget();
        (self.sink)(UiEvent::ToggleExpand(id));
    }

    /// `YATT_PERF`: prints the frame times of the drag that just ended.
    fn report_perf(&self, cards: usize) {
        let Some(perf) = self.perf.borrow_mut().as_mut().map(std::mem::take) else {
            return;
        };
        if perf.len() < 2 {
            return;
        }
        let mut gaps: Vec<f64> = perf.iter().skip(1).map(|p| p.0 * 1000.0).collect();
        gaps.sort_by(f64::total_cmp);
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        let p50 = gaps[gaps.len() / 2];
        let p95 = gaps[(gaps.len() * 95 / 100).min(gaps.len() - 1)];
        let late = gaps.iter().filter(|g| **g > 1000.0 / 55.0).count();
        let tick = perf.iter().map(|p| p.1 * 1000.0).sum::<f64>() / perf.len() as f64;
        eprintln!(
            "perf: drag of {cards} cards ({}): {} frames, interval mean {mean:.1} ms, \
             p50 {p50:.1} ms, p95 {p95:.1} ms, {late} over 18 ms; tick {tick:.2} ms",
            if style::real_glass() { "glass" } else { "blur" },
            perf.len()
        );
    }

    /// The pointer over the canvas: a pointing hand over a fanned card (a click
    /// picks it), an open hand over any other card (it can be taken), closed while
    /// carrying one, the arrow elsewhere.
    fn show_cursor(&self, over: Option<TaskId>) {
        let cursor = if self.drag.borrow().is_some() {
            NSCursor::closedHandCursor()
        } else if let Some(id) = over {
            let fanned = self
                .column_of(id)
                .is_some_and(|c| self.layouts.get()[c] == (Layout::Stack { fanned: true }));
            if fanned {
                NSCursor::pointingHandCursor()
            } else {
                NSCursor::openHandCursor()
            }
        } else {
            NSCursor::arrowCursor()
        };
        cursor.set();
    }

    /// Folds the fan unless a second click comes quickly: a double-click on the fan
    /// opens the card. The wait is short whatever the system's double-click
    /// interval (often 0.5–0.8 s), or the fold would feel like it missed the click.
    fn unfan_soon(self: &Rc<Self>) {
        self.cancel_unfan();
        let weak = Rc::downgrade(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(b) = weak.upgrade() {
                b.unfan_timer.borrow_mut().take();
                if b.unfan() {
                    b.folded_at.set(CACurrentMediaTime());
                    b.retarget();
                    b.kick();
                }
            }
        });
        let interval = NSEvent::doubleClickInterval().min(FAN_FOLD_DELAY);
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(interval, false, &block)
        };
        *self.unfan_timer.borrow_mut() = Some(timer);
    }

    fn cancel_unfan(&self) {
        if let Some(t) = self.unfan_timer.borrow_mut().take() {
            t.invalidate();
        }
    }

    /// Folds fanned stacks back. Returns true when one was fanned.
    fn unfan(&self) -> bool {
        let mut layouts = self.layouts.get();
        let mut any = false;
        for l in &mut layouts {
            if *l == (Layout::Stack { fanned: true }) {
                *l = Layout::Stack { fanned: false };
                any = true;
            }
        }
        self.layouts.set(layouts);
        any
    }

    /// The topmost card under the point.
    fn hit(&self, p: NSPoint) -> Option<TaskId> {
        self.cards
            .borrow()
            .iter()
            .filter(|(_, c)| {
                let (ox, oy) = self.offset(c.space);
                !c.removing && c.contains(NSPoint::new(p.x - ox, p.y - oy))
            })
            .max_by(|a, b| a.1.z.total_cmp(&b.1.z))
            .map(|(id, _)| *id)
    }

    /// Hands a trackpad or wheel event to a column's native scroll view.
    fn forward(&self, ci: Option<usize>, event: Option<&NSEvent>) {
        if let (Some(ci), Some(event)) = (ci, event)
            && self.layouts.get()[ci] == Layout::List
        {
            self.cols[ci].scroll.scrollWheel(event);
        }
    }

    /// Scrolling is native: a column's scroll view gets the events. A two-finger
    /// swipe that starts sideways over a card drags it instead; until that is
    /// decided the events are held, then replayed to the scroll view if it scrolls.
    /// Trackpad and wheel scrolling is the column's own: every event goes to its
    /// native scroll view. (Cards are dragged with the pointer only.)
    fn on_scroll(self: &Rc<Self>, s: Scroll) {
        self.forward(self.column_at(s.at.x), s.event.as_deref());
    }

    fn begin_drag(self: &Rc<Self>, id: TaskId, start: NSPoint, p: NSPoint) {
        let Some(col) = self.column_of(id) else {
            crate::eventlog::log(format!("board: drag refused: {id:?} is in no column"));
            return;
        };
        let mut ids = vec![id];
        let selected = self.selected.borrow().clone();
        if self.layouts.get()[col] == (Layout::Stack { fanned: false }) {
            // A folded stack goes as a whole.
            ids.extend(self.order.borrow()[col].iter().filter(|x| **x != id));
        } else if selected.contains(&id) && selected.len() > 1 {
            for o in self.order.borrow().iter() {
                ids.extend(o.iter().filter(|x| **x != id && selected.contains(x)));
            }
        }
        {
            let mut cards = self.cards.borrow_mut();
            for x in &ids {
                if let Some(card) = cards.get_mut(x) {
                    relocate(card, Space::Overlay, self);
                }
            }
        }
        let Some(body) = self.cards.borrow().get(&id).map(|c| c.body) else {
            return;
        };
        *self.press.borrow_mut() = None;
        let group = ids.len() > 1;
        // Held where it was grabbed: that point, from the card's centre, in the
        // card's own unturned frame.
        let centre = (body.x + CARD_W / 2.0, body.y + CARD_H / 2.0);
        let r = physics::turn((start.x - centre.0, start.y - centre.1), -body.r);
        *self.drag.borrow_mut() = Some(Drag {
            ids,
            group,
            grab: (start.x - body.x, start.y - body.y),
            cursor: (p.x, p.y),
            target: Some(col),
            index: 0,
            grip: physics::Grip::new(r, (start.x, start.y), body.r),
        });
        // Rings off, followers stacked, the tail hidden: right away, not at the
        // first change of target.
        self.retarget();
        self.kick();
    }

    fn drag_to(self: &Rc<Self>, p: NSPoint) {
        let target = self.column_at(p.x);
        let changed = {
            let mut drag = self.drag.borrow_mut();
            let Some(drag) = drag.as_mut() else { return };
            drag.cursor = (p.x, p.y);
            let index = match target {
                Some(ci) if self.layouts.get()[ci] == Layout::List => {
                    let n = self.order.borrow()[ci]
                        .iter()
                        .filter(|id| !drag.ids.contains(id))
                        .count();
                    let centre = p.y - drag.grab.1 + CARD_H / 2.0 - self.offset(Space::Doc(ci)).1;
                    physics::insert_index(centre + LIST_GAP / 2.0, LIST_TOP, PITCH, n)
                }
                _ => 0,
            };
            let changed = drag.target != target;
            let moved = changed || drag.index != index;
            NSCursor::closedHandCursor().set();
            drag.target = target;
            drag.index = index;
            (changed, moved)
        };
        let (changed, moved) = changed;
        if changed {
            (self.on_target)(target);
        }
        // The cards chase the pointer in `tick`; the board's layout only changes
        // when the drop target does.
        if moved {
            self.retarget();
        }
        self.kick();
    }

    fn drop_at(self: &Rc<Self>, p: NSPoint) {
        self.drag_to(p);
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        (self.on_target)(None);
        self.report_perf(drag.ids.len());
        // Released over the card it carried: ready to take it again.
        NSCursor::openHandCursor().set();
        if drag.group && drag.target.is_some() {
            *self.landing.borrow_mut() = Some(Landing {
                ids: drag.ids.clone(),
                start: CACurrentMediaTime(),
            });
        }
        if let Some(col) = drag.target {
            let before = self.order.borrow()[col].clone();
            // The cards land in the order they had on the board, whichever one led.
            let flat: Vec<TaskId> = self.order.borrow().iter().flatten().copied().collect();
            let mut ids = drag.ids.clone();
            ids.sort_by_key(|id| flat.iter().position(|x| x == id));
            // Reorder right away so the cards spring to their new places before the
            // snapshot comes back.
            {
                let mut order = self.order.borrow_mut();
                for o in order.iter_mut() {
                    o.retain(|id| !ids.contains(id));
                }
                let at = drag.index.min(order[col].len());
                for (i, id) in ids.iter().enumerate() {
                    order[col].insert(at + i, *id);
                }
            }
            // One undo step for the whole group.
            let moves = moves_into(&before, &ids, drag.index, self.statuses[col]);
            if !moves.is_empty() {
                (self.sink)(UiEvent::Group(moves));
            }
        }
        self.retarget();
        self.kick();
    }
}

impl Card {
    fn update(&mut self, snap: &CardSnap) {
        set_title(&self.title, &snap.title);
        set_meta(&self.time, &time_text(snap));
        self.tracking = snap.tracking;
        self.refill();
    }

    fn set_selected(&mut self, on: bool) {
        if self.selected != on {
            self.selected = on;
            self.edges.set_selected(on);
        }
    }

    fn refill(&self) {
        self.face.set_fill(Fill::Card {
            hue: self.hue,
            tracking: self.tracking,
            selected: self.selected,
        });
    }

    /// Blurs the content and shows the target label while leading a drag,
    /// cross-fading when `animated`.
    fn set_look(&mut self, look: Look, animated: bool) {
        if self.look == look {
            return;
        }
        let lifted = matches!(look, Look::Lifted { .. });
        if let Look::Lifted { column, count } = &look {
            self.label.setStringValue(&NSString::from_str(column));
            self.count
                .setStringValue(&NSString::from_str(&tasks_count(*count)));
        }
        let filters: Retained<NSArray<CIFilter>> = if lifted {
            style::gaussian(LIFT_BLUR)
                .map_or_else(NSArray::new, |f| NSArray::from_retained_slice(&[f]))
        } else {
            NSArray::new()
        };
        self.content.setContentFilters(&filters);
        let (label, content) = match (lifted, self.muted) {
            (true, _) => (1.0, 0.45),
            (false, true) => (0.0, 0.0),
            (false, false) => (0.0, 1.0),
        };
        let views = (self.label.clone(), self.content.clone(), self.count.clone());
        animate(animated, motion::GLASS_SECS, move || {
            views.0.animator().setAlphaValue(label);
            views.1.animator().setAlphaValue(content);
            views.2.animator().setAlphaValue(label);
        });
        self.look = look;
    }

    /// Finishes the capsule-to-card morph: everything back to card size.
    fn end_grow(&mut self) {
        self.grow = None;
        self.placed.set([f64::NAN; 8]);
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(CARD_W, CARD_H));
        for part in &self.parts {
            part.setFrame(bounds);
        }
        self.content.setFrame(bounds);
        for v in [&*self.icon as &NSView, &self.time, &self.dots, &self.face] {
            v.setAlphaValue(1.0);
        }
    }

    /// A drawn follower of a group drag keeps its full material (glass and all;
    /// only three are ever drawn) but rides without squash and stretch.
    fn set_light(&mut self, on: bool) {
        if self.light == on {
            return;
        }
        self.light = on;
    }

    /// Fades the whole card out and takes it out of the render tree (no blur, no
    /// edges, nothing to composite), or brings it back.
    ///
    /// Never by fading: a card that leaves the render tree first travels to
    /// where it is covered (under a stack's top cards) and goes when it comes to
    /// rest there (`tick`), or at once (`at_once`) when it is already covered;
    /// one that comes back is whole from its first frame, still covered, and
    /// travels out from there.
    fn set_parked(&mut self, on: bool, at_once: bool) {
        if self.parked.get() == on {
            if on && at_once && !self.view.isHidden() {
                self.view.setHidden(true);
            }
            return;
        }
        self.parked.set(on);
        if on {
            if at_once {
                self.view.setHidden(true);
            }
        } else {
            self.view.setAlphaValue(1.0);
            self.view.setHidden(self.culled);
        }
    }

    /// Takes a far off-screen list row out of the render tree, or puts it back;
    /// it is out of sight either way, so there is nothing to fade.
    /// Returns whether it changed.
    fn set_culled(&mut self, on: bool) -> bool {
        if self.culled == on {
            return false;
        }
        self.culled = on;
        self.view.setHidden(on || self.parked.get());
        // Out of sight a row lets go of its glass, so even the system's layout
        // passes skip it; it gets it back, under the tint, when it comes near.
        let glass = &self.parts[0];
        if on {
            glass.removeFromSuperview();
        } else if unsafe { glass.superview() }.is_none() {
            self.view.addSubview_positioned_relativeTo(
                glass,
                NSWindowOrderingMode::Below,
                Some(&self.face),
            );
        }
        // Whatever moved while it was hidden is applied when it shows.
        self.placed.set([f64::NAN; 8]);
        true
    }

    /// The carried card casts a soft shadow; set down, it lets go of it.
    fn set_lifted(&mut self, on: bool) {
        if self.lifted != on {
            self.lifted = on;
            self.edges.set_lifted(on);
        }
    }

    /// Hides the card's text (a card behind the top of a stack).
    fn set_muted(&mut self, on: bool) {
        if self.muted == on {
            return;
        }
        self.muted = on;
        // At once: it changes while the card lies under the stack's top card,
        // as it starts to travel out or arrives back; no fade in the transition.
        self.content.setAlphaValue(if on { 0.0 } else { 1.0 });
    }

    /// Places the view; on the overlay the canvas's offset is added.
    fn apply_frame(&self, overlay_dy: f64) {
        let b = &self.body;
        let dy = if self.space == Space::Overlay {
            overlay_dy
        } else {
            0.0
        };
        if let Some((t, from)) = self.grow {
            self.placed.set([f64::NAN; 8]);
            // Capsule → card: the frame grows, the typed text (now the title) slides
            // to its place, the colour and the rest of the card fade in.
            let lerp = |a: f64, b: f64| a + (b - a) * t;
            let (w, h) = (lerp(from.width, CARD_W), lerp(from.height, CARD_H));
            self.view.setFrameCenterRotation(0.0);
            self.view
                .setFrame(NSRect::new(NSPoint::new(b.x, b.y + dy), NSSize::new(w, h)));
            self.view.setBoundsSize(NSSize::new(w, h));
            for part in &self.parts {
                part.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w, h)));
            }
            self.cut_shadows(w, h, self.scale);
            // Where the title sits so that it covers the text in the field.
            let start = (
                FIELD_TEXT_X - TEXT_X,
                (from.height - 20.0) / 2.0 + 1.0 - TITLE_Y,
            );
            self.content.setFrame(NSRect::new(
                NSPoint::new(lerp(start.0, 0.0), lerp(start.1, 0.0)),
                NSSize::new(CARD_W, CARD_H),
            ));
            self.content.setBoundsSize(NSSize::new(CARD_W, CARD_H));
            for v in [&*self.icon as &NSView, &self.time, &self.dots, &self.face] {
                v.setAlphaValue(t);
            }
            return;
        }
        let s = self.scale;
        let space = match self.space {
            Space::Canvas => 0.0,
            Space::Doc(i) => 1.0 + i as f64,
            Space::Overlay => 9.0,
        };
        let key = [
            b.x,
            b.y + dy,
            b.r,
            s,
            b.vx,
            b.vy,
            space,
            f64::from(self.light),
        ];
        if self.placed.replace(key) == key {
            return;
        }
        let (sx, sy) = (1.0, 1.0);
        let (w, h) = (CARD_W * s * sx, CARD_H * s * sy);
        self.view.setFrameCenterRotation(0.0);
        self.view.setFrame(NSRect::new(
            NSPoint::new(b.x + (CARD_W - w) / 2.0, b.y + dy + (CARD_H - h) / 2.0),
            NSSize::new(w, h),
        ));
        // Scale by sizing the parts, not the card's bounds: the blur layer ignores
        // bounds scaling and would draw full-size from the card's top-left corner.
        // Only the text scales, through the content view's own bounds.
        let size = NSSize::new(w, h);
        self.view.setBoundsSize(size);
        let local = NSRect::new(NSPoint::ZERO, size);
        for part in &self.parts {
            part.setFrame(local);
        }
        self.cut_shadows(w, h, self.scale);
        self.content.setFrame(local);
        self.content.setBoundsSize(NSSize::new(CARD_W, CARD_H));
        // The column label a touch above the middle, the count under it.
        self.label.setFrame(NSRect::new(
            NSPoint::new(8.0 * s, h / 2.0 - 24.0 * s),
            NSSize::new(w - 16.0 * s, 30.0 * s),
        ));
        self.count.setFrame(NSRect::new(
            NSPoint::new(8.0 * s, h / 2.0 + 6.0 * s),
            NSSize::new(w - 16.0 * s, 16.0 * s),
        ));
        // The canvas is flipped, so AppKit's positive rotation already reads clockwise.
        self.view.setFrameCenterRotation(b.r);
    }

    /// Fits the edge layers to a `w` × `h` card, when its size has changed.
    /// Its corners too: a smaller card has a smaller radius everywhere (tint,
    /// backdrop, both edges, the selection ring), as if the whole card scaled.
    /// The corners follow the card's scale, not its squash, so they hold still
    /// while it moves.
    fn cut_shadows(&self, w: f64, h: f64, scale: f64) {
        let scale_changed = self.corner_scale.replace(scale) != scale;
        if self.shadow_size.replace((w, h)) != (w, h) || scale_changed {
            self.edges.layout_scaled(w, h, scale);
        }
        if scale_changed {
            self.face.set_corner_scale(scale);
            style::set_blur_corners(&self.parts[0], scale);
        }
    }

    /// Point-in-card test that respects the card's rotation.
    fn contains(&self, p: NSPoint) -> bool {
        let b = &self.body;
        let (cx, cy) = (b.x + CARD_W / 2.0, b.y + CARD_H / 2.0);
        let t = (-b.r).to_radians();
        let (dx, dy) = (p.x - cx, p.y - cy);
        let (lx, ly) = (dx * t.cos() - dy * t.sin(), dx * t.sin() + dy * t.cos());
        lx.abs() <= self.scale * CARD_W / 2.0 && ly.abs() <= self.scale * CARD_H / 2.0
    }
}

/// Moves a card's view to another parent, keeping it where it is on screen.
fn relocate(card: &mut Card, to: Space, board: &Board) {
    if card.space == to {
        return;
    }
    let (ax, ay) = board.offset(card.space);
    let (bx, by) = board.offset(to);
    let (dx, dy) = (ax - bx, ay - by);
    let b = &mut card.body;
    b.x += dx;
    b.tx += dx;
    b.y += dy;
    b.ty += dy;
    let parent = board.parent(to);
    motion::quietly(|| {
        card.view.removeFromSuperview();
        parent.addSubview(&card.view);
    });
    card.space = to;
    board.restack_due.set(true);
    card.apply_frame(board.overlay_dy.get());
}

/// A card at rest on the overlay goes into its home view.
fn land(card: &mut Card, board: &Board) {
    let home = card.home;
    relocate(card, home, board);
}

/// Card text layout: left inset, and where the meta line and the title sit.
pub(crate) const TEXT_X: f64 = 14.0;
pub(crate) const META_Y: f64 = CARD_H - 47.0;
pub(crate) const TITLE_Y: f64 = CARD_H - 31.0;

/// A card's text: glyph top-left, `···` top-right, then the meta line over a bold
/// title near the bottom, all in dark tones of the card's hue. The board's cards
/// and the task panel's opening and closing face are built from the same.
pub(crate) struct Face {
    pub content: Retained<FlippedView>,
    pub icon: Retained<NSView>,
    pub dots: Retained<NSTextField>,
    pub time: Retained<NSTextField>,
    pub title: Retained<NSTextField>,
}

pub(crate) fn card_face(mtm: MainThreadMarker, id: TaskId, title: &str, hue: usize) -> Face {
    let ink = style::text(hue);
    let content = FlippedView::new(mtm, NSRect::new(NSPoint::ZERO, NSSize::new(CARD_W, CARD_H)));
    content.setWantsLayer(true);
    content.setLayerUsesCoreImageFilters(true);

    // The glyph as a cached bitmap on a plain layer: an image view would
    // rasterise the vector symbol again on every layer update, for every card.
    let icon = style::glyph_view(mtm, style::glyph_of(id), hue);
    icon.setFrame(NSRect::new(
        NSPoint::new(TEXT_X, 14.0),
        NSSize::new(16.0, 16.0),
    ));

    let dots = text_label(mtm, "···", 13.0, unsafe { NSFontWeightMedium });
    dots.setAlignment(NSTextAlignment::Right);
    dots.setFrame(NSRect::new(
        NSPoint::new(CARD_W - 44.0, 10.0),
        NSSize::new(30.0, 18.0),
    ));
    dots.setTextColor(Some(&ink.colorWithAlphaComponent(0.3)));

    // Meta: small, medium weight, tabular digits, opened up a touch and well
    // below the title in contrast, so the two never compete.
    let time = text_label(mtm, "", 11.0, unsafe { NSFontWeightMedium });
    time.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
        11.0,
        unsafe { NSFontWeightMedium },
    )));
    time.setFrame(NSRect::new(
        NSPoint::new(TEXT_X, META_Y),
        NSSize::new(CARD_W - 2.0 * TEXT_X, 14.0),
    ));
    time.setTextColor(Some(&ink.colorWithAlphaComponent(0.5)));

    // Title: two steps up in size, semibold, tracked in a little as display text
    // is, nearly full ink.
    let title_label = text_label(mtm, "", 15.0, unsafe { NSFontWeightMedium });
    title_label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    title_label.setFrame(NSRect::new(
        NSPoint::new(TEXT_X, TITLE_Y),
        NSSize::new(CARD_W - 2.0 * TEXT_X, 20.0),
    ));
    title_label.setTextColor(Some(&ink.colorWithAlphaComponent(0.92)));
    set_title(&title_label, title);
    let title = title_label;

    for v in [&*icon as &NSView, &dots, &time, &title] {
        content.addSubview(v);
    }
    Face {
        content,
        icon,
        dots,
        time,
        title,
    }
}

/// Tracking of a card's title and of its meta line, in points.
const TITLE_KERN: f64 = -0.2;
const META_KERN: f64 = 0.15;

/// A card's title, tracked.
pub(crate) fn set_title(label: &NSTextField, text: &str) {
    style::set_tracked(label, text, TITLE_KERN);
}

/// A card's meta line, tracked.
pub(crate) fn set_meta(label: &NSTextField, text: &str) {
    style::set_tracked(label, text, META_KERN);
}

/// The card's meta line: tracked time, a dot while it runs, a pencil if it has notes.
pub(crate) fn time_text(snap: &CardSnap) -> String {
    let notes = if snap.notes.trim().is_empty() {
        ""
    } else {
        "  ·  ✎"
    };
    format!(
        "{}{}{}",
        if snap.tracking { "● " } else { "" },
        format_duration(snap.total_ms),
        notes
    )
}

/// "1 задача", "3 задачи", "5 задач".
fn tasks_count(n: usize) -> String {
    let word = match (n % 10, n % 100) {
        (1, r) if r != 11 => "задача",
        (2..=4, r) if !(12..=14).contains(&r) => "задачи",
        _ => "задач",
    };
    format!("{n} {word}")
}

fn text_label(mtm: MainThreadMarker, text: &str, size: f64, weight: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    l
}
