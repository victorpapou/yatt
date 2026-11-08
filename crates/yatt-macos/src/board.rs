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
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
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

use crate::classes::{
    CanvasMsg, CanvasView, Fill, FillView, FlippedView, OverlayView, Scroll, Touch,
};
use crate::model::{COLS, CardSnap, ColumnSnap, UiEvent, format_duration, moves_into};
use crate::motion;
use crate::physics::{self, Body};
use crate::style::{self, CARD_H, CARD_W};
use crate::window::{Sink, animate};

/// Pixels the pointer (or fingers) must travel before a press becomes a drag.
const DRAG_THRESHOLD: f64 = 4.0;
/// Cards of a folded stack that draw: the top one and two slivers. The rest fade
/// out as they fall into the pile, and leave the render tree once in it, until
/// the stack opens again.
const STACK_SHOWN: usize = 3;
/// Over this much of a stack's openness, next to folded, the cards that draw
/// only when it is open fade in or out, and the cards behind the top one let
/// their text show or fade: nothing a stack shows appears or goes at once.
const STACK_FADE: f64 = 0.3;
/// The shortest a stack card fades in or out, in seconds, however fast the fan moves.
const STACK_FADE_MIN: f64 = 0.22;
/// How far beyond a column's visible area list rows stay drawn, in screens:
/// enough that a fast scroll never shows an empty slot, few enough glass views.
const ROW_OVERSCAN: f64 = 0.5;
/// The longest a landing group keeps its flight order, in case a card never
/// comes to rest.
const LANDING_MAX_SECS: f64 = 2.0;
/// Room a fan keeps from the canvas's sides, when it has room.
const FAN_MARGIN: f64 = 8.0;
/// How far a fan may stick out past its column on each side before it shrinks.
const FAN_PEEK: f64 = 36.0;
/// A fanned card under the pointer grows a little, keeping its place in the order.
const HOVER_SCALE: f64 = 1.03;
/// The new-task field's text inset from the capsule's left edge.
const FIELD_TEXT_X: f64 = 20.0;
/// Content blur on a lifted card, in points.
const LIFT_BLUR: f64 = 5.0;
/// List layout: first slot's top below the toolbar (and the column chips, which
/// float over the cards), and the gap between cards.
const LIST_TOP: f64 = 42.0;
/// The overlay's layer z on the board: under the bottom fade (20) and the
/// new-task capsule (30), over them while a card grows out of the capsule.
pub const OVERLAY_Z: f64 = 10.0;
const OVERLAY_Z_LIFTED: f64 = 40.0;
const LIST_GAP: f64 = 10.0;
const PITCH: f64 = CARD_H + LIST_GAP;
/// Room left under the last card for the new-task field floating over the board.
const LIST_BOTTOM: f64 = 96.0;
/// What the new-task capsule takes at the board's bottom (with its gap): a
/// stack centres in the column above it.
const CAPSULE_ROOM: f64 = 76.0;
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

/// Told how far a list has scrolled (points).
pub type ScrollHandler = Box<dyn Fn(f64)>;

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
    /// The group titles over the cards, while the board is grouped.
    heads: RefCell<Vec<Head>>,
}

/// A group's title strip: the full width of its column, a row of its own. Bare at
/// rest; a blurred band with a hairline under it while it holds under the toolbar.
struct Head {
    strip: Retained<FlippedView>,
    material: Retained<objc2_app_kit::NSVisualEffectView>,
    line: Retained<FillView>,
    label: Retained<objc2_app_kit::NSTextField>,
}

/// How the cards of each column are grouped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Grouping {
    #[default]
    None,
    Tag,
    Color,
}

/// The height of a group's title row in a list.
const HEAD_H: f64 = 26.0;
/// How far under the toolbar's bottom edge a sticky group title holds: none, flush.
const CHIP_STICK: f64 = 0.0;

/// Names of the card colours, by palette index.
pub(crate) const HUE_NAMES: [&str; 15] = [
    "Синий",
    "Розовый",
    "Фиолетовый",
    "Лаймовый",
    "Мятный",
    "Серый",
    "Коралловый",
    "Оранжевый",
    "Жёлтый",
    "Шалфей",
    "Бирюзовый",
    "Голубой",
    "Бежевый",
    "Индиго",
    "Пурпурный",
]; 

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    List,
    Stack { fanned: bool },
}

/// A stack held by the fingers on the trackpad: the openness they took it at,
/// how far they have moved since, and how fast.
#[derive(Clone, Copy, Debug)]
struct Held {
    start: f64,
    down: f64,
    swipe: physics::Swipe,
}

/// A card's place in a stack, both ways: folded on the pile and laid out on
/// the fan, with the stack's centre and the card's size and visibility each
/// way. Where it is headed at any moment lies between them at the stack's
/// openness ([`physics::Opening`]); it falls onto that place ([`Body::fall_to`]).
#[derive(Clone, Copy, Debug)]
struct StackPlace {
    col: usize,
    pile: (f64, f64, f64),
    fan: (f64, f64, f64),
    /// The stack's centre, folded and fanned, less half a card (the frame of
    /// `Body::x, y`).
    pile_centre: (f64, f64),
    fan_centre: (f64, f64),
    pile_scale: f64,
    fan_scale: f64,
    /// Whether it draws on the pile, and on the fan.
    on_pile: bool,
    on_fan: bool,
    /// Its text shows on the pile (only the top card's does).
    pile_text: bool,
}

impl StackPlace {
    /// Where it is headed at openness `q`, and the stack's centre then.
    fn at(&self, q: f64) -> ((f64, f64, f64), (f64, f64)) {
        let (cx, cy, _) = physics::fan_mix(
            (self.pile_centre.0, self.pile_centre.1, 0.0),
            (self.fan_centre.0, self.fan_centre.1, 0.0),
            q,
        );
        (physics::fan_mix(self.pile, self.fan, q), (cx, cy))
    }

    fn scale(&self, q: f64) -> f64 {
        self.pile_scale + (self.fan_scale - self.pile_scale) * q.clamp(0.0, 1.0)
    }

    /// How opaque it is at openness `q`: what draws only open fades in over the
    /// first stretch of the opening, what draws only folded fades out over it.
    fn fade(&self, q: f64) -> f64 {
        let f = |on: bool| if on { 1.0 } else { 0.0 };
        let t = (q / STACK_FADE).clamp(0.0, 1.0);
        f(self.on_pile) + (f(self.on_fan) - f(self.on_pile)) * t
    }

    /// How much of its text shows at openness `q`.
    fn text(&self, q: f64) -> f64 {
        if self.pile_text {
            1.0
        } else {
            (q / STACK_FADE).clamp(0.0, 1.0)
        }
    }
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
    time: Retained<NSTextField>,
    title: Retained<NSTextField>,
    label: Retained<NSTextField>,
    /// "3 задачи" under the label while leading a group drag.
    count: Retained<NSTextField>,
    hue: usize,
    glyph: &'static str,
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
    /// In a stack: its place there, both ways ([`StackPlace`]).
    stack: Option<StackPlace>,
    /// The openness its stack place was last taken at: as the openness
    /// moves, the card rides along with its place exactly, and only the
    /// openness itself springs (no second overshoot of the card's own).
    stack_q: Option<f64>,
    /// The view's opacity as a stack sets it, and the share of its text that
    /// shows; set only when they change, so other fades are left alone.
    fade: f64,
    text: f64,
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
    /// How far the toolbar reaches over the board (it runs under it, blurred).
    top: Cell<f64>,
    sink: Sink,
    cols: [Column; COLS],
    grouping: Cell<Grouping>,
    /// The scroll view round all the columns (they scroll sideways in it).
    outer: RefCell<Option<Retained<NSScrollView>>>,
    /// Per column, per slot: the title of the group that starts there.
    group_starts: RefCell<[Vec<Option<String>>; COLS]>,
    col_titles: RefCell<[String; COLS]>,
    /// Scroll observers of the columns, kept for as long as the board.
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    statuses: [Status; COLS],
    titles: RefCell<[String; COLS]>,
    geom: Cell<[ColumnGeom; COLS]>,
    height: Cell<f64>,
    order: RefCell<[Vec<TaskId>; COLS]>,
    cards: RefCell<HashMap<TaskId, Card>>,
    /// `YATT_DUMP`: the glass's layer tree has been printed once.
    dumped: Cell<bool>,
    started: Cell<f64>,
    layouts: Cell<[Layout; COLS]>,
    /// How open each column's stack should be (0 folded, 1 fanned; between
    /// while a scroll drives it), and when that scroll last moved it (media
    /// time).
    open_p: Cell<[f64; COLS]>,
    /// How open each column's stack is: falling toward `open_p`, or held by
    /// the fingers on the trackpad (`held`).
    opening: Cell<[physics::Opening; COLS]>,
    held: Cell<[Option<Held>; COLS]>,
    scrubbed: Cell<[f64; COLS]>,
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
    /// Development aid: stepped by hand at a fixed timestep, for frame-by-frame
    /// comparison with the reference clip.
    manual: Cell<bool>,
    fixed_dt: Cell<Option<f64>>,
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
    /// Tells the window which column to tint while dragging.
    on_target: Box<dyn Fn(Option<usize>)>,
    /// Told how far the furthest-scrolled list has gone up under the toolbar
    /// (points; 0 at the top or rubber-banding past it), on every scroll.
    pub on_scrolled: RefCell<Option<ScrollHandler>>,
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
        let cols = std::array::from_fn::<_, COLS, _>(|_| {
            let scroll = NSScrollView::new(mtm);
            scroll.setDrawsBackground(false);
            scroll.setHasVerticalScroller(true);
            scroll.setHasHorizontalScroller(false);
            scroll.setAutohidesScrollers(true);
            scroll.setScrollerStyle(NSScrollerStyle::Overlay);
            // Rubber band only when the list is taller than the column.
            scroll.setVerticalScrollElasticity(NSScrollElasticity::Automatic);
            scroll.setHorizontalScrollElasticity(NSScrollElasticity::None);
            scroll.contentView().setDrawsBackground(false);
            // The system keeps the list clear of the toolbar (a top content
            // inset, the scroller starting below it) and softens the edge where
            // the rows go under it.
            scroll.setAutomaticallyAdjustsContentInsets(true);
            let doc = FlippedView::new(mtm, NSRect::ZERO);
            scroll.setDocumentView(Some(&doc));
            // The scroll view's home is a split-view item of the window (see
            // `scroll_view`); the canvas lies over it and forwards the scrolls.
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            Column {
                scroll,
                doc,
                heads: RefCell::new(Vec::new()),
            }
        });
        let board = Rc::new(Self {
            mtm,
            canvas,
            overlay,
            overlay_dy: Cell::new(0.0),
            top: Cell::new(0.0),
            sink,
            cols,
            grouping: Cell::new(Grouping::None),
            outer: RefCell::new(None),
            group_starts: RefCell::new(Default::default()),
            col_titles: RefCell::new(Default::default()),
            observers: RefCell::new(Vec::new()),
            statuses: Status::ALL,
            titles: RefCell::new(Default::default()),
            geom: Cell::new(Default::default()),
            height: Cell::new(0.0),
            order: RefCell::new(Default::default()),
            cards: RefCell::new(HashMap::new()),
            dumped: Cell::new(false),
            started: Cell::new(CACurrentMediaTime()),
            layouts: Cell::new([Layout::List; COLS]),
            open_p: Cell::new([0.0; COLS]),
            opening: Cell::new(Default::default()),
            held: Cell::new([None; COLS]),
            scrubbed: Cell::new([0.0; COLS]),
            selected: RefCell::new(HashSet::new()),
            anchor: Cell::new(None),
            press: RefCell::new(None),
            drag: RefCell::new(None),
            hover: Cell::new(None),
            stacking: RefCell::new(Default::default()),
            restack_due: Cell::new(false),
            fan_clicked: Cell::new(None),
            landing: RefCell::new(None),
            manual: Cell::new(false),
            fixed_dt: Cell::new(None),
            hidden: Cell::new(None),
            spawn: Cell::new(None),
            timer: RefCell::new(None),
            last_tick: Cell::new(0.0),
            perf: RefCell::new(std::env::var_os("YATT_PERF").map(|_| Vec::new())),
            on_target: Box::new(on_target),
            on_scrolled: RefCell::new(None),
        });
        // Scrolling (momentum and rubber band included) shows the rows coming near.
        for (ci, col) in board.cols.iter().enumerate() {
            let clip = col.scroll.contentView();
            clip.setPostsBoundsChangedNotifications(true);
            let weak = Rc::downgrade(&board);
            let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
                if let Some(b) = weak.upgrade() {
                    b.cull_lists();
                    b.report_scroll();
                    // The group titles stick under the column's chip as it scrolls.
                    if b.layouts.get()[ci] == Layout::List {
                        b.place_heads(ci, b.grouping.get() != Grouping::None);
                    }
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
    /// Puts the overlay (flying and dragged cards) over the capsule while a new
    /// card grows out of it, under the capsule and its fade otherwise.
    fn lift_overlay(&self, over_capsule: bool) {
        if let Some(layer) = self.overlay.layer() {
            let z = if over_capsule {
                OVERLAY_Z_LIFTED
            } else {
                OVERLAY_Z
            };
            if layer.zPosition() != z {
                layer.setZPosition(z);
            }
        }
    }

    /// How far the furthest-scrolled list has gone up (stacks do not scroll),
    /// to whoever listens.
    pub fn report_scroll(&self) {
        let up = self
            .cols
            .iter()
            .zip(self.layouts.get())
            .filter(|(_, l)| *l == Layout::List)
            .map(|(c, _)| c.scroll.contentView().bounds().origin.y + c.scroll.contentInsets().top)
            .fold(0.0_f64, f64::max);
        if let Some(f) = self.on_scrolled.borrow().as_ref() {
            f(up);
        }
    }

    /// A column's native scroll view, for the window to put into the column's
    /// split-view item.
    /// The board's horizontal scroll view, to pass sideways scrolls to.
    pub fn set_outer_scroll(&self, s: Retained<NSScrollView>) {
        *self.outer.borrow_mut() = Some(s);
    }

    pub fn scroll_view(&self, ci: usize) -> &NSScrollView {
        &self.cols[ci].scroll
    }

    /// Where the list starts: below the toolbar and the chips.
    fn list_top(&self) -> f64 {
        LIST_TOP + self.top.get()
    }

    /// Where the first row sits in a list's own (document) coordinates: the
    /// scroll view's automatic top inset already keeps the list below the toolbar,
    /// so only the chips' room is left.
    fn doc_top(&self) -> f64 {
        // Grouped, the first title sits right under the toolbar (its row is the
        // gap); ungrouped, the first card keeps its breathing room.
        if self.grouping.get() == Grouping::None {
            HEAD_H + 8.0
        } else {
            HEAD_H
        }
    }

    /// `top`: how far the toolbar reaches over the board, which runs under it.
    pub fn set_geometry(
        self: &Rc<Self>,
        geom: [ColumnGeom; COLS],
        height: f64,
        overlay_dy: f64,
        top: f64,
    ) {
        self.top.set(top);
        self.geom.set(geom);
        self.height.set(height);
        self.overlay_dy.set(overlay_dy);
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
    /// The window turned light or dark: every card in the new colours.
    pub fn restyle(&self) {
        for card in self.cards.borrow_mut().values_mut() {
            card.restyle();
        }
    }

    /// Development aid: scrolls a list column to `y` (document coordinates).
    pub fn demo_scroll_to(&self, ci: usize, y: f64) {
        let scroll = &self.cols[ci].scroll;
        let clip = scroll.contentView();
        // As the user's scroll would be: held within what the clip view allows.
        let want = NSRect::new(NSPoint::new(0.0, y), clip.bounds().size);
        let at = clip.constrainBoundsRect(want).origin;
        eprintln!("scroll: asked {y}, allowed {}", at.y);
        clip.scrollToPoint(at);
        scroll.reflectScrolledClipView(&clip);
    }

    pub fn grouping(&self) -> Grouping {
        self.grouping.get()
    }

    /// Groups the columns' cards (or not) from now on; the caller renders again.
    pub fn set_grouping(&self, g: Grouping) {
        self.grouping.set(g);
    }

    /// The columns with their cards clustered by group, in the order groups first
    /// appear, and the title of the group that starts at each slot.
    fn grouped(&self, columns: &[ColumnSnap; COLS]) -> ([ColumnSnap; COLS], [Vec<Option<String>>; COLS]) {
        let mode = self.grouping.get();
        let mut starts: [Vec<Option<String>>; COLS] = Default::default();
        let out = std::array::from_fn(|ci| {
            let col = &columns[ci];
            if mode == Grouping::None {
                starts[ci] = vec![None; col.cards.len()];
                return col.clone();
            }
            let key = |c: &CardSnap| -> String {
                match mode {
                    Grouping::Tag => c.tags.first().cloned().unwrap_or_else(|| "Без тега".into()),
                    _ => HUE_NAMES[style::hue_of(c.id) % HUE_NAMES.len()].to_string(),
                }
            };
            let mut keys: Vec<String> = Vec::new();
            for c in &col.cards {
                let k = key(c);
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
            let mut cards = Vec::with_capacity(col.cards.len());
            for k in &keys {
                for (i, c) in col.cards.iter().filter(|c| key(c) == *k).enumerate() {
                    starts[ci].push((i == 0).then(|| k.clone()));
                    cards.push(c.clone());
                }
            }
            ColumnSnap {
                cards,
                ..col.clone()
            }
        });
        (out, starts)
    }

    /// A title strip (blur band, hairline, grey semibold label), hidden.
    fn new_head(&self, doc: &FlippedView) -> Head {
        let strip = FlippedView::new(self.mtm, NSRect::ZERO);
        let material = objc2_app_kit::NSVisualEffectView::new(self.mtm);
        material.setMaterial(objc2_app_kit::NSVisualEffectMaterial::HeaderView);
        material.setBlendingMode(objc2_app_kit::NSVisualEffectBlendingMode::WithinWindow);
        material.setState(objc2_app_kit::NSVisualEffectState::Active);
        material.setAlphaValue(0.0);
        let line = FillView::new(self.mtm, Fill::Separator);
        line.setAlphaValue(0.0);
        let label = objc2_app_kit::NSTextField::labelWithString(
            objc2_foundation::ns_string!(""),
            self.mtm,
        );
        label.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize_weight(12.0, unsafe {
            objc2_app_kit::NSFontWeightSemibold
        })));
        label.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        strip.addSubview(&material);
        strip.addSubview(&line);
        strip.addSubview(&label);
        strip.setHidden(true);
        doc.addSubview(&strip);
        Head { strip, material, line, label }
    }

    /// Puts a column's titles in one row of headers: its own name first, then
    /// (grouped) each group's. All look alike and behave alike: the one in view
    /// holds under the toolbar, and the next, coming up from below, pushes it up
    /// and takes its place (as in Finder).
    fn place_heads(&self, ci: usize, show: bool) {
        let col = &self.cols[ci];
        let mut heads = col.heads.borrow_mut();
        let is_list = self.layouts.get()[ci] == Layout::List;
        let groups: Vec<(usize, String)> = if show && is_list {
            self.group_starts.borrow()[ci]
                .iter()
                .enumerate()
                .filter_map(|(i, g)| g.clone().map(|g| (i, g)))
                .collect()
        } else {
            Vec::new()
        };
        // (natural top, title) in order down the list.
        let mut items: Vec<(f64, String)> = Vec::new();
        if is_list {
            items.push((0.0, self.col_titles.borrow()[ci].clone()));
        }
        for (i, g) in &groups {
            items.push((self.list_y(ci, *i) - HEAD_H, g.clone()));
        }
        while heads.len() < items.len() {
            let h = self.new_head(&col.doc);
            heads.push(h);
            self.stacking.borrow_mut().clear();
        }
        let x = self.list_x(ci);
        let width = col.doc.frame().size.width;
        // The top edge of what is seen below the toolbar, in document coordinates.
        let seen_top = col.scroll.contentView().bounds().origin.y
            + col.scroll.contentInsets().top
            + CHIP_STICK;
        for (k, h) in heads.iter().enumerate() {
            let Some((natural, title)) = items.get(k) else {
                h.strip.setHidden(true);
                continue;
            };
            h.label
                .setStringValue(&objc2_foundation::NSString::from_str(title));
            let mut y = natural.max(seen_top);
            if let Some((next, _)) = items.get(k + 1) {
                y = y.min(next - HEAD_H);
            }
            let y = y.max(*natural);
            // The blurred band and its hairline only while it holds over cards
            // going under it.
            let want = if y > natural + 0.5 { 1.0 } else { 0.0 };
            if (h.material.alphaValue() - want).abs() > 0.01 {
                let (m, l) = (h.material.clone(), h.line.clone());
                animate(true, 0.15, move || {
                    m.animator().setAlphaValue(want);
                    l.animator().setAlphaValue(want);
                });
            }
            h.strip
                .setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(width, HEAD_H)));
            h.material
                .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, HEAD_H)));
            h.line.setFrame(NSRect::new(
                NSPoint::new(0.0, HEAD_H - 1.0),
                NSSize::new(width, 1.0),
            ));
            h.label.setFrame(NSRect::new(
                NSPoint::new(x - 2.0, HEAD_H - 20.0),
                NSSize::new((width - x).max(0.0), 16.0),
            ));
            if h.strip.isHidden() {
                h.strip.setAlphaValue(0.0);
                h.strip.setHidden(false);
                let s2 = h.strip.clone();
                animate(true, 0.4, move || s2.animator().setAlphaValue(1.0));
            }
            // Later titles lie over earlier ones, so the one coming up covers.
            let on_top = col.doc.subviews().lastObject();
            if on_top.as_deref().map(|v| v as *const _ as usize)
                != Some(&**h.strip as *const _ as usize)
            {
                col.doc.addSubview(&h.strip);
            }
        }
    }

    /// The top of slot `i` of a list column: the group titles above it count.
    fn list_y(&self, ci: usize, i: usize) -> f64 {
        let starts = self.group_starts.borrow();
        let s = &starts[ci];
        let heads = s.iter().take(i + 1).filter(|g| g.is_some()).count();
        self.doc_top() + i as f64 * PITCH + heads as f64 * HEAD_H
    }

    pub fn render(self: &Rc<Self>, columns: &[ColumnSnap; COLS]) {
        for col in columns {
            for snap in &col.cards {
                style::set_look(snap.id, snap.color, snap.icon.as_deref());
                style::set_first_tag(snap.id, snap.tags.first().map(String::as_str));
            }
        }
        let (grouped, starts) = self.grouped(columns);
        *self.group_starts.borrow_mut() = starts;
        for (i, c) in columns.iter().enumerate() {
            self.col_titles.borrow_mut()[i] = c.title.clone();
        }
        let columns = &grouped;
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
        for col in columns {
            for snap in &col.cards {
                style::set_look(snap.id, snap.color, snap.icon.as_deref());
                style::set_first_tag(snap.id, snap.tags.first().map(String::as_str));
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
                        self.lift_overlay(true);
                        // On the capsule before it is ever drawn: a new view
                        // stands at the origin until its first frame is applied.
                        motion::quietly(|| card.apply_frame(self.overlay_dy.get()));
                        self.overlay.addSubview(&card.view);
                    } else {
                        let (x, y) = self.slot(ci, i);
                        card.body = Body::at(x, y - 8.0);
                        card.space = self.home_of(ci);
                        card.home = card.space;
                        motion::quietly(|| card.apply_frame(self.overlay_dy.get()));
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
            c.view.setAlphaValue(c.fade);
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

    /// Development aid: from now on the board moves only when stepped by hand
    /// (`demo_step`), a fixed timestep per call, so frames are exactly
    /// reproducible.
    pub fn demo_manual(&self) {
        self.manual.set(true);
        if let Some(t) = self.timer.borrow_mut().take() {
            t.invalidate();
        }
    }

    /// Development aid: one fixed step of `dt` seconds.
    pub fn demo_step(self: &Rc<Self>, dt: f64) {
        self.fixed_dt.set(Some(dt));
        self.tick();
    }

    /// Development aid: a card's centre (canvas coordinates), angle and scale.
    pub fn demo_card_state(&self, id: TaskId) -> Option<(f64, f64, f64, f64)> {
        let cards = self.cards.borrow();
        let c = cards.get(&id)?;
        let (ox, oy) = self.offset(c.space);
        Some((
            c.body.x + ox + CARD_W / 2.0,
            c.body.y + oy + CARD_H / 2.0,
            c.body.r,
            c.scale,
        ))
    }

    /// Development aid: a scroll of `down` points over `at` (canvas coordinates).
    pub fn demo_scroll_at(self: &Rc<Self>, at: (f64, f64), down: f64) {
        self.on_scroll(Scroll {
            event: None,
            at: NSPoint::new(at.0, at.1),
            down,
            touch: Touch::Wheel,
        });
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
        self.layouts.set([layout; COLS]);
        self.open_p.set([0.0; COLS]);
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
            Layout::List => (self.list_x(ci), self.list_y(ci, i)),
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

    /// Top-left of a stack's top card: the pile centred in the column both ways.
    fn stack_anchor(&self, ci: usize) -> (f64, f64) {
        let g = self.geom.get()[ci];
        let pile = CARD_H + physics::pile(2).0;
        (
            physics::centred_left(g.x, g.w, CARD_W),
            physics::centred_top(self.list_top(), self.height.get() - CAPSULE_ROOM, pile),
        )
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
        style::tint_glass(&glass, hue);
        glass.setFrame(bounds);
        glass.setAutoresizingMask(fill_mask);

        let Face {
            content,
            icon,
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
        for v in [&*time as &NSView, &title] {
            v.setLayerContentsRedrawPolicy(policy);
        }

        if std::env::var_os("YATT_DUMP").is_some() {
            eprintln!("card view tree (bottom to top):");
            style::dump_tree(&view, 0);
        }
        let parts = [glass.clone(), Retained::into_super(face.clone())];
        let mut card = Card {
            view,
            face,
            content,
            icon,
            time,
            title,
            label,
            count,
            hue,
            glyph: style::glyph_of(snap.id),
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
            stack: None,
            stack_q: None,
            fade: 1.0,
            text: 1.0,
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
        // Only the cards laid out in a stack below keep a stack place.
        for card in cards.values_mut() {
            card.stack = None;
            card.stack_q = None;
        }

        for (ci, ids) in order.iter().enumerate() {
            let ids: Vec<TaskId> = ids
                .iter()
                .copied()
                .filter(|id| !drag_ids.contains(id))
                .collect();
            let n = ids.len();
            match layouts[ci] {
                Layout::Stack { .. } => {
                    self.place_heads(ci, false);
                    // Every card of a stack has two places: on the pile and on the
                    // fan. It is headed for the point between them at the stack's
                    // openness, which falls toward the openness the stack should
                    // have (`tick`), and falls onto it round the stack's centre.
                    let a = self.stack_anchor(ci);
                    // The swatch fan of the reference, newest card on top, shrunk
                    // as a whole (angles unchanged) to fit its column and the board's
                    // height (sticking out past the column a little first), and
                    // centred in the column both ways below the chips.
                    let g = self.geom.get()[ci];
                    let tall = self.height.get() - self.list_top() - CAPSULE_ROOM;
                    let room = (
                        g.w - 2.0 * FAN_MARGIN + 2.0 * FAN_PEEK,
                        tall - 2.0 * FAN_MARGIN,
                    );
                    let (shown_n, scale) = if n > 1 {
                        physics::fan_fit(n, (w, h), room)
                    } else {
                        (0, 1.0)
                    };
                    let (x0, y0, x1, y1) = physics::fan_bounds(shown_n.max(1), (w, h));
                    let centre = (
                        g.x + g.w / 2.0 - (x0 + x1) / 2.0 * scale,
                        self.list_top() + tall / 2.0 - (y0 + y1) / 2.0 * scale,
                    );
                    let q = self.opening.get()[ci].at;
                    let goal = self.open_goal(ci);
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let (py, ps) = physics::pile(i);
                        let pile = (a.0, a.1 + py, 0.0);
                        let on_fan = i < shown_n;
                        let fan = if on_fan {
                            let (dx, dy, deg) = physics::fan(i, shown_n, w);
                            // Each card strays a little from its place, its own way.
                            let (turn, out, round) = physics::fan_jitter(seed_of(*id));
                            let (an, r) = (dy.atan2(dx) + round.to_radians(), dx.hypot(dy) + out);
                            (
                                centre.0 + r * an.cos() * scale - w / 2.0,
                                centre.1 + r * an.sin() * scale - h / 2.0,
                                deg + turn,
                            )
                        } else {
                            pile
                        };
                        let place = StackPlace {
                            col: ci,
                            pile,
                            fan,
                            pile_centre: a,
                            fan_centre: (centre.0 - w / 2.0, centre.1 - h / 2.0),
                            pile_scale: ps,
                            fan_scale: if on_fan {
                                scale * if hover == Some(*id) { HOVER_SCALE } else { 1.0 }
                            } else {
                                ps
                            },
                            on_pile: i < STACK_SHOWN,
                            on_fan,
                            pile_text: i == 0,
                        };
                        let (to, c) = place.at(q);
                        self.send_home(card, Space::Canvas, to);
                        card.body.fall_to(to, c);
                        card.body.folding = goal < q;
                        card.stack = Some(place);
                        card.stack_q = Some(q);
                        // Out of the render tree once it lies hidden in the pile,
                        // faded out on its way there.
                        if place.fade(goal) == 0.0 {
                            parked.insert(*id);
                        }
                        // Newer cards on top, as on the stack.
                        card.z = (n - i) as f64;
                        card.target_scale = place.scale(q);
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
                    let heads = self.group_starts.borrow()[ci].iter().filter(|g| g.is_some()).count();
                    // A short list must not scroll at all (card flush under the toolbar,
                    // its group title stuck over it): the document is then no taller
                    // than what shows below the toolbar, less one more inset's worth.
                    let inset = self.cols[ci].scroll.contentInsets().top.max(self.top.get());
                    let doc_h = (self.doc_top()
                        + slots as f64 * PITCH
                        + heads as f64 * HEAD_H
                        + LIST_BOTTOM)
                        .max(self.height.get() - 2.0 * inset);
                    // A list that fits does not scroll at all, not even elastically; one
                    // that overflows rubber-bands as usual.
                    let content_h = self.doc_top()
                        + slots as f64 * PITCH
                        + heads as f64 * HEAD_H
                        + LIST_BOTTOM;
                    let overflows = content_h > self.height.get() - inset;
                    self.cols[ci].scroll.setVerticalScrollElasticity(if overflows {
                        NSScrollElasticity::Automatic
                    } else {
                        NSScrollElasticity::None
                    });
                    let doc = &self.cols[ci].doc;
                    if doc.frame().size.height != doc_h
                        || doc.frame().size.width != self.geom.get()[ci].w
                    {
                        motion::quietly(|| {
                            doc.setFrameSize(NSSize::new(self.geom.get()[ci].w, doc_h))
                        });
                    }
                    self.place_heads(ci, true);
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let slot = match gap {
                            Some((at, k)) if i >= at => i + k,
                            _ => i,
                        };
                        let y = self.list_y(ci, slot);
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
        let hidden = self.hidden.get();
        for (id, card) in cards.iter_mut() {
            if !followers.contains(id) {
                card.set_light(false);
            }
            // Only the carried card (the head of a group) casts the lift shadow.
            if lead != Some(*id) {
                card.set_lifted(false);
            }
            card.set_parked(parked.contains(id), at_once.contains(id));
            match card.stack {
                Some(place) => {
                    let q = self.opening.get()[place.col].at;
                    // Its fade follows in `tick`, over at least STACK_FADE_MIN:
                    // set here, a flick (one retarget per scroll event) blinked
                    // the cards past the pile out.
                    if hidden != Some(*id) && motion::reduce_motion() {
                        card.set_fade(place.fade(q));
                    }
                    card.set_text(place.text(q));
                }
                None if hidden != Some(*id) => card.set_fade(1.0),
                None => {}
            }
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
            let mut views: Vec<Retained<NSView>> = Vec::new();
            views.extend(
                sorted
                    .iter()
                    .filter(|(_, c)| c.space == space)
                    .map(|(_, c)| Retained::into_super(c.view.clone())),
            );
            // A list column's group titles lie over its cards (they stick, the
            // cards go under them).
            if let Space::Doc(ci) = space {
                views.extend(
                    self.cols[ci]
                        .heads
                        .borrow()
                        .iter()
                        .map(|h| Retained::into_super(h.strip.clone())),
                );
            }
            let parent = self.parent(space);
            motion::quietly(|| parent.setSubviews(&NSArray::from_retained_slice(&views)));
        }
        *self.stacking.borrow_mut() = key;
    }

    // ---- motion ----

    /// Starts the frame timer if it is not running.
    fn kick(self: &Rc<Self>) {
        // Stepped by hand (the comparison harness): no timer of its own.
        if self.manual.get() || self.timer.borrow().is_some() {
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
        // A part-open fan left alone opens or folds, whichever is nearer.
        let mut part = false;
        let held = self.held.get();
        for (ci, p) in self.open_p.get().into_iter().enumerate() {
            if p > 0.0 && p < 1.0 && held[ci].is_none() {
                if now - self.scrubbed.get()[ci] > physics::FAN_SNAP_IDLE {
                    self.set_fanned(ci, physics::fan_snap(p));
                } else {
                    part = true;
                }
            }
        }
        let dt = match self.fixed_dt.take() {
            Some(dt) => dt,
            None => (now - self.last_tick.replace(now)).clamp(0.0, 1.0 / 30.0),
        };
        let reduce = motion::reduce_motion();
        self.autoscroll(dt);
        let dragging = self.drag.borrow().is_some();
        let overlay_dy = self.overlay_dy.get();
        let mut cards = self.cards.borrow_mut();

        // The system glass makes and remakes its rounded layers as it
        // displays, with circular corners: keep them continuous. A few layers
        // per card, and a property set to the value it has costs nothing.
        for card in cards.values() {
            if let Some(layer) = card.parts[0].layer() {
                style::continuous_layers(&layer);
            }
        }
        if std::env::var_os("YATT_DUMP").is_some()
            && now - self.started.get() > 2.0
            && !self.dumped.replace(true)
        {
            if let Some(layer) = cards.values().next().and_then(|c| c.parts[0].layer()) {
                eprintln!("glass layers after first display:");
                style::dump_layers(&layer, 0);
            }
        }

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

        // Each stack's openness falls toward what it should be, and every card
        // of the stack is headed for its place at that openness.
        let mut opening = self.opening.get();
        let mut opened = [false; COLS];
        for (ci, o) in opening.iter_mut().enumerate() {
            // Held: the fingers set it (`on_scroll`); the cards follow.
            if held[ci].is_some() {
                opened[ci] = true;
                continue;
            }
            let goal = self.open_goal(ci);
            if o.at != goal || o.v != 0.0 {
                if reduce {
                    *o = physics::Opening {
                        at: goal,
                        ..Default::default()
                    };
                } else {
                    o.step(goal, dt);
                }
                opened[ci] = true;
            }
        }
        self.opening.set(opening);
        let hidden = self.hidden.get();
        let mut fading = false;
        for (id, card) in cards.iter_mut() {
            let Some(place) = card.stack else { continue };
            let q = opening[place.col].at;
            // A card the fan hides or shows fades over at least STACK_FADE_MIN,
            // however fast the swipe: with the openness alone a quick fold
            // blinked the cards past the pile out.
            let fade = {
                let to = place.fade(q);
                let step = dt / STACK_FADE_MIN;
                if reduce || hidden == Some(*id) || card.removing {
                    to
                } else {
                    card.fade + (to - card.fade).clamp(-step, step)
                }
            };
            if !opened[place.col] && (fade - card.fade).abs() < 1e-3 {
                continue;
            }
            fading |= (fade - place.fade(q)).abs() >= 1e-3;
            let (to, c) = place.at(q);
            // Ride with the openness: the card moves by exactly as much as its
            // place did, so what is left to fall is only its own offset (a new
            // place, a drop), never a lag behind the fan.
            if let Some(q0) = card.stack_q {
                let (from, _) = place.at(q0);
                card.body.x += to.0 - from.0;
                card.body.y += to.1 - from.1;
                card.body.r += to.2 - from.2;
            }
            card.stack_q = Some(q);
            card.body.fall_to(to, c);
            card.body.folding = self.open_goal(place.col) < q;
            card.target_scale = place.scale(q);
            if hidden != Some(*id) {
                card.set_fade(fade);
            }
            card.set_text(place.text(q));
        }

        let mut moving = dragging || part || fading || opened.contains(&true);
        let drag_ids = self.dragged();
        let started = now;
        for card in cards.values_mut() {
            // Parked and arrived under the stack (faded out on its way in): now
            // it can leave the tree.
            if card.parked.get()
                && !card.view.isHidden()
                && card.body.at_rest()
                && (card.stack.is_none() || card.fade < 0.01)
            {
                card.view.setHidden(true);
            }
            // Out of sight (a culled row, or a card hidden in a stack or under a
            // dragged group): it lands at once and draws when it shows again.
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
        // While a card is still the capsule growing into a card, it rides
        // above the capsule; otherwise the capsule and the fade under it stand
        // over every card (see `OVERLAY_Z`).
        self.lift_overlay(cards.values().any(|c| c.grow.is_some()));
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
        let dump_pending = std::env::var_os("YATT_DUMP").is_some() && !self.dumped.get();
        if !moving
            && !dump_pending
            && let Some(timer) = self.timer.borrow_mut().take()
        {
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
        } else if y < self.list_top() + AUTOSCROLL_EDGE {
            -(self.list_top() + AUTOSCROLL_EDGE - y) / AUTOSCROLL_EDGE
        } else {
            return;
        };
        let scroll = &self.cols[ci].scroll;
        let clip = scroll.contentView();
        let max = (self.cols[ci].doc.frame().size.height - clip.bounds().size.height).max(0.0);
        let from = self.scrolled(ci);
        let min = -scroll.contentInsets().top;
        let to = (from + speed.clamp(-1.0, 1.0) * AUTOSCROLL_SPEED * dt).clamp(min, max);
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
            CanvasMsg::Context { at, view } => self.context_menu(at, &view),
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

    /// Right-click on a card: «Удалить» for it, or for the whole selection when
    /// the card is part of it. Undoable (⌘Z) like any delete.
    fn context_menu(self: &Rc<Self>, at: NSPoint, view: &NSView) {
        let Some(id) = self.hit(at) else { return };
        if !self.selected.borrow().contains(&id) {
            self.selected.borrow_mut().clear();
            self.selected.borrow_mut().insert(id);
            self.anchor.set(Some(id));
            self.retarget();
        }
        let mut ids: Vec<TaskId> = self.selected.borrow().iter().copied().collect();
        ids.sort();
        let single = ids.len() == 1;
        let pressed = id;
        let mtm = MainThreadMarker::new().expect("main thread");
        let title = if ids.len() > 1 {
            format!("Удалить ({})", ids.len())
        } else {
            "Удалить".to_string()
        };
        let sink = self.sink.clone();
        let weak = Rc::downgrade(self);
        let target = crate::classes::ActionTarget::new(mtm, move || {
            let deletes = ids.iter().copied().map(UiEvent::Delete).collect();
            sink(UiEvent::Group(deletes));
            if let Some(b) = weak.upgrade() {
                b.clear_selection();
            }
        });
        let item = unsafe {
            objc2_app_kit::NSMenuItem::initWithTitle_action_keyEquivalent(
                objc2_app_kit::NSMenuItem::alloc(mtm),
                &objc2_foundation::NSString::from_str(&title),
                Some(crate::classes::ActionTarget::sel()),
                &objc2_foundation::NSString::from_str(""),
            )
        };
        unsafe { item.setTarget(Some(&target)) };
        let menu = objc2_app_kit::NSMenu::new(mtm);
        // «Открыть в отдельном окне» for the one card pressed (not for a selection).
        let open_target = {
            let sink = self.sink.clone();
            crate::classes::ActionTarget::new(mtm, move || sink(UiEvent::OpenInWindow(pressed)))
        };
        if single {
            let open_item = unsafe {
                objc2_app_kit::NSMenuItem::initWithTitle_action_keyEquivalent(
                    objc2_app_kit::NSMenuItem::alloc(mtm),
                    &objc2_foundation::NSString::from_str("Открыть в отдельном окне"),
                    Some(crate::classes::ActionTarget::sel()),
                    &objc2_foundation::NSString::from_str(""),
                )
            };
            unsafe { open_item.setTarget(Some(&open_target)) };
            menu.addItem(&open_item);
            menu.addItem(&objc2_app_kit::NSMenuItem::separatorItem(mtm));
        }
        menu.addItem(&item);
        // Blocks until the menu closes, so the target outlives the click.
        menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(view));
        drop(target);
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
    /// A plain click: opens the card (a stack's or a fan's too: the fan is only a
    /// way of looking at the stack), or clears the selection on empty board.
    fn click(self: &Rc<Self>, id: Option<TaskId>, clicks: isize) {
        let Some(id) = id else {
            self.clear_selection();
            return;
        };
        self.selected.borrow_mut().clear();
        self.anchor.set(Some(id));
        self.retarget();
        // The second click of a double-click takes the task to a window of its own
        // (the first already opened it as usual: no waiting for a second click).
        if clicks >= 2 {
            (self.sink)(UiEvent::OpenInWindow(id));
        } else {
            (self.sink)(UiEvent::ToggleExpand(id));
        }
    }

    /// How open column `ci`'s stack should be: fanned 1, folded 0, part open
    /// while a scroll drives it.
    fn open_goal(&self, ci: usize) -> f64 {
        match self.layouts.get()[ci] {
            Layout::Stack { fanned: true } => 1.0,
            Layout::Stack { fanned: false } => self.open_p.get()[ci].clamp(0.0, 1.0),
            Layout::List => 0.0,
        }
    }

    /// Fans out a stack (`open`) or folds it back, one column on its own; any
    /// number of stacks can stand fanned at once.
    fn set_fanned(self: &Rc<Self>, ci: usize, open: bool) {
        let mut layouts = self.layouts.get();
        let want = Layout::Stack { fanned: open };
        let mut p = self.open_p.get();
        let part = p[ci] > 0.0 && p[ci] < 1.0;
        p[ci] = if open { 1.0 } else { 0.0 };
        self.open_p.set(p);
        if !matches!(layouts[ci], Layout::Stack { .. }) || (layouts[ci] == want && !part) {
            return;
        }
        if open && self.order.borrow()[ci].len() < 2 {
            return;
        }
        layouts[ci] = want;
        self.layouts.set(layouts);
        if open {
            self.fan_clicked.set(Some(CACurrentMediaTime()));
        }
        self.retarget();
        self.kick();
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

    /// The pointer over the canvas: an open hand over a card (it can be taken),
    /// closed while carrying one, the arrow elsewhere.
    fn show_cursor(&self, over: Option<TaskId>) {
        let cursor = if self.drag.borrow().is_some() {
            NSCursor::closedHandCursor()
        } else if over.is_some() {
            NSCursor::openHandCursor()
        } else {
            NSCursor::arrowCursor()
        };
        cursor.set();
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
        self.open_p.set([0.0; COLS]);
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
        // A sideways scroll belongs to the board's own horizontal scroll view.
        if let Some(event) = event
            && event.scrollingDeltaX().abs() > event.scrollingDeltaY().abs()
            && let Some(outer) = self.outer.borrow().as_ref()
        {
            outer.scrollWheel(event);
            return;
        }
        if let (Some(ci), Some(event)) = (ci, event)
            && self.layouts.get()[ci] == Layout::List
        {
            self.cols[ci].scroll.scrollWheel(event);
        }
    }

    /// Scrolling is native: a column's scroll view gets the events. A two-finger
    /// swipe that starts sideways over a card drags it instead; until that is
    /// decided the events are held, then replayed to the scroll view if it scrolls.
    /// Trackpad and wheel scrolling: over a list it is the column's own (its
    /// native scroll view); over a stack, scrolling down fans it out and
    /// scrolling up folds it back.
    fn on_scroll(self: &Rc<Self>, s: Scroll) {
        // The fingers lifted: every stack they held is let go, wherever the
        // pointer is now.
        if s.touch == Touch::Release {
            self.let_go();
        }
        let col = self.column_at(s.at.x);
        if let Some(ci) = col
            && matches!(self.layouts.get()[ci], Layout::Stack { .. })
        {
            if self.order.borrow()[ci].len() < 2 {
                return;
            }
            match s.touch {
                // On the trackpad the stack is in the fingers: it opens one
                // for one with them, stretches past its ends, and a stack
                // still moving is caught where it is.
                Touch::Hold => self.hold(ci, s.down),
                // After the fingers lift the fall carries it on; the system's
                // momentum steps are not the fingers.
                Touch::Release | Touch::Coast => {}
                // A mouse wheel: each step moves how open it should be; a
                // fling goes all the way.
                Touch::Wheel => self.wheel(ci, s.down),
            }
            return;
        }
        self.forward(col, s.event.as_deref());
    }

    /// The fingers move a stack `down` points.
    fn hold(self: &Rc<Self>, ci: usize, down: f64) {
        let now = CACurrentMediaTime();
        let mut opening = self.opening.get();
        let mut held = self.held.get();
        let h = held[ci].get_or_insert(Held {
            start: opening[ci].at,
            down: 0.0,
            swipe: physics::Swipe::default(),
        });
        h.down += down;
        h.swipe.feed(down, now);
        let at = physics::fan_pull(h.start, h.down);
        opening[ci] = physics::Opening {
            at,
            v: h.swipe.speed(now) / physics::FAN_SCRUB,
            past: 0.0,
        };
        self.held.set(held);
        self.opening.set(opening);
        let mut layouts = self.layouts.get();
        layouts[ci] = Layout::Stack { fanned: false };
        self.layouts.set(layouts);
        let mut ps = self.open_p.get();
        ps[ci] = at.clamp(0.0, 1.0);
        self.open_p.set(ps);
        let mut t = self.scrubbed.get();
        t[ci] = now;
        self.scrubbed.set(t);
        self.retarget();
        self.kick();
    }

    /// Lets go every held stack: each goes open or folded by where it is and
    /// how fast the fingers threw it, and falls there from that speed.
    fn let_go(self: &Rc<Self>) {
        let now = CACurrentMediaTime();
        let mut held = self.held.get();
        for ci in 0..COLS {
            let Some(h) = held[ci].take() else { continue };
            self.held.set(held);
            let mut opening = self.opening.get();
            let (open, v) =
                physics::fan_release(opening[ci].at, h.swipe.speed(now) / physics::FAN_SCRUB);
            opening[ci].v = v;
            opening[ci].past = if open { physics::fan_recoil(v) } else { 0.0 };
            self.opening.set(opening);
            self.set_fanned(ci, open);
        }
        self.held.set(held);
        self.retarget();
        self.kick();
    }

    /// A mouse wheel step of `down` points over a stack.
    fn wheel(self: &Rc<Self>, ci: usize, down: f64) {
        if down == 0.0 {
            return;
        }
        let fanned = self.layouts.get()[ci] == (Layout::Stack { fanned: true });
        let from = if fanned { 1.0 } else { self.open_p.get()[ci] };
        let p = physics::fan_scrub(from, down);
        if p >= 1.0 {
            self.set_fanned(ci, true);
        } else if p <= 0.0 {
            self.set_fanned(ci, false);
        } else {
            let mut layouts = self.layouts.get();
            layouts[ci] = Layout::Stack { fanned: false };
            self.layouts.set(layouts);
            let mut ps = self.open_p.get();
            ps[ci] = p;
            self.open_p.set(ps);
            let mut at = self.scrubbed.get();
            at[ci] = CACurrentMediaTime();
            self.scrubbed.set(at);
            self.retarget();
            self.kick();
        }
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
                    (0..n)
                        .take_while(|&i| centre + LIST_GAP / 2.0 >= self.list_y(ci, i) + PITCH)
                        .count()
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
        // Let go at speed, the cards were carrying a lot of it (a chain on stiff
        // springs): bound it so they settle in with a little give instead of
        // flying past their places and crawling back.
        if let Ok(mut cards) = self.cards.try_borrow_mut() {
            for id in &drag.ids {
                if let Some(c) = cards.get_mut(id) {
                    c.body.limit_speed(physics::RELEASE_SPEED);
                }
            }
        }
        self.retarget();
        self.kick();
    }
}

impl Card {
    fn update(&mut self, snap: &CardSnap) {
        // A new colour or icon: the tint, the ink and the glyph follow.
        let hue = style::hue_of(snap.id);
        let glyph = style::glyph_of(snap.id);
        if hue != self.hue || glyph != self.glyph {
            self.hue = hue;
            self.glyph = glyph;
            self.restyle();
        }
        set_title(&self.title, &snap.title);
        set_meta(&self.time, &time_text(snap));
        self.tracking = snap.tracking;
        self.refill();
    }

    /// Ink, glyph and edge in the current colours (a new colour, icon or
    /// appearance).
    fn restyle(&mut self) {
        let (hue, glyph) = (self.hue, self.glyph);
        {
            let ink = style::text(hue);
            self.title.setTextColor(Some(&ink));
            self.time
                .setTextColor(Some(&ink.colorWithAlphaComponent(0.5)));
            refresh_title(&self.title);
            refresh_meta(&self.time);
            self.label.setTextColor(Some(&ink));
            self.count
                .setTextColor(Some(&ink.colorWithAlphaComponent(0.7)));
            let mtm = MainThreadMarker::new().expect("main thread");
            let icon = style::glyph_view(mtm, glyph, hue);
            icon.setFrame(self.icon.frame());
            self.content.replaceSubview_with(&self.icon, &icon);
            self.icon = icon;
        }
        self.edges.restyle();
        style::tint_glass(&self.parts[0], hue);
        self.face.repaint();
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
            (false, _) => (0.0, self.text),
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
        for v in [&*self.icon as &NSView, &self.time, &self.face] {
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
            self.view.setAlphaValue(self.fade);
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

    /// Shows all of the card's text.
    fn set_muted(&mut self, on: bool) {
        self.set_text(if on { 0.0 } else { 1.0 });
    }

    /// Shows this share of the card's text: a card behind the top of a stack
    /// shows none, and lets it show as the stack opens, with the openness.
    fn set_text(&mut self, a: f64) {
        if (self.text - a).abs() < 1e-3 {
            return;
        }
        self.text = a;
        self.muted = a <= 0.0;
        if self.look == Look::Solid {
            self.content.setAlphaValue(a);
        }
    }

    /// The view's opacity as its stack sets it. Only changes are applied, so a
    /// card fading in or out on its own (new, removed) is left to that fade.
    fn set_fade(&mut self, a: f64) {
        if self.removing || (self.fade - a).abs() < 1e-3 {
            return;
        }
        self.fade = a;
        self.view.setAlphaValue(a);
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
            for v in [&*self.icon as &NSView, &self.time, &self.face] {
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
    pub time: Retained<NSTextField>,
    pub title: Retained<NSTextField>,
}

/// A task's fixed random seed (its id's random tail).
fn seed_of(id: TaskId) -> u64 {
    let s = id.0.as_u128();
    (s ^ (s >> 64)) as u64
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
        NSSize::new(22.0, 22.0),
    ));

    // No "···": nothing on a card that does not do something.

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

    for v in [&*icon as &NSView, &time, &title] {
        content.addSubview(v);
    }
    Face {
        content,
        icon,
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
    fit_title(label);
}

/// A long title wraps by words onto a second line (cut with an ellipsis after
/// that), growing up from the same bottom edge; the meta line above it moves up
/// by the same. One-line titles sit exactly where they always did.
fn fit_title(label: &NSTextField) {
    if let Some(cell) = label.cell() {
        cell.setWraps(true);
        cell.setUsesSingleLineMode(false);
        cell.setTruncatesLastVisibleLine(true);
    }
    label.setMaximumNumberOfLines(2);
    label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    let width = CARD_W - 2.0 * TEXT_X;
    label.setPreferredMaxLayoutWidth(width);
    style::refresh_tracked(label, TITLE_KERN);
    let h = label.intrinsicContentSize().height;
    let two = h > 24.0;
    let lift = if two { 18.0 } else { 0.0 };
    label.setFrame(NSRect::new(
        NSPoint::new(TEXT_X, TITLE_Y - lift),
        NSSize::new(width, 20.0 + lift),
    ));
    if let Some(parent) = unsafe { label.superview() } {
        for v in parent.subviews().iter() {
            if let Ok(meta) = v.downcast::<NSTextField>()
                && &*meta as *const NSTextField != label as *const NSTextField
            {
                let f = meta.frame();
                meta.setFrame(NSRect::new(
                    NSPoint::new(f.origin.x, META_Y - lift * 0.8),
                    f.size,
                ));
            }
        }
    }
}

/// A card's meta line, tracked.
pub(crate) fn set_meta(label: &NSTextField, text: &str) {
    style::set_tracked(label, text, META_KERN);
}

pub(crate) fn refresh_title(label: &NSTextField) {
    style::refresh_tracked(label, TITLE_KERN);
}

pub(crate) fn refresh_meta(label: &NSTextField) {
    style::refresh_tracked(label, META_KERN);
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

/// A card as the canvas draws it (glass, tint, edges, glyph, time, title), for the
/// settings' preview: the same parts in the same order, only without a board.
pub(crate) struct CardPreview {
    pub view: Retained<FlippedView>,
    glass: Retained<NSView>,
    face: Retained<FillView>,
    content: RefCell<Retained<FlippedView>>,
    id: TaskId,
    title: String,
    mtm: MainThreadMarker,
}

impl CardPreview {
    pub fn new(mtm: MainThreadMarker, title: &str, hue: usize) -> Self {
        let id = TaskId::new();
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(CARD_W, CARD_H));
        let view = FlippedView::new(mtm, bounds);
        view.setAutoresizesSubviews(false);
        view.setWantsLayer(true);
        let face = FillView::new(
            mtm,
            Fill::Card {
                hue,
                tracking: false,
                selected: false,
            },
        );
        face.setFrame(bounds);
        let glass = style::card_blur(mtm);
        style::tint_glass(&glass, hue);
        glass.setFrame(bounds);
        let content = Self::face_for(mtm, id, title, hue);
        let edges = style::Edges::new(mtm);
        for v in [&*edges.lift, &edges.ring, &edges.outer, &glass, &face, &edges.inner, &content] {
            view.addSubview(v);
        }
        Self {
            view,
            glass,
            face,
            content: RefCell::new(content),
            id,
            title: title.to_string(),
            mtm,
        }
    }

    fn face_for(mtm: MainThreadMarker, id: TaskId, title: &str, hue: usize) -> Retained<FlippedView> {
        let Face { content, time, .. } = card_face(mtm, id, title, hue);
        set_meta(&time, "0:00:00");
        content
    }

    /// Eases to the card's colour (text and glyph included).
    pub fn set_hue(&self, hue: usize, eased: bool) {
        if eased {
            crate::motion::soften(&self.glass);
        }
        style::tint_glass(&self.glass, hue);
        self.face.set_fill(Fill::Card {
            hue,
            tracking: false,
            selected: false,
        });
        let new = Self::face_for(self.mtm, self.id, &self.title, hue);
        let old = self.content.replace(new.clone());
        self.view.replaceSubview_with(&old, &new);
    }
}
