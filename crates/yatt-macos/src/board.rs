//! The cards, laid out and moved on springs ([`crate::physics`]) stepped by a
//! display-rate timer while anything moves.
//!
//! - Each column is a plain list by default, scrolling on its own. A toggle in the
//!   column header collapses it into a stack like the reference clip
//!   (docs/reference/card-motion.md): the top card carries an "ещё N" badge, the
//!   next ones peek out below it as slivers. A click fans the stack out into a
//!   rosette whose spread grows with the count; the card under the pointer lifts.
//!   Dragging the folded stack takes it whole.
//! - Every card is tinted glass: a blur of what is behind, the card's colour over
//!   it with a light sheen on top, a hairline rim and a soft white glow.
//! - Click opens a card, ⌘-click toggles it in the selection, ⇧-click selects a
//!   range within the column.
//! - Dragging one card is a plain drag: it follows the pointer and the list makes
//!   room. Dragging a selection blurs the pressed card's content and writes the
//!   target column on it; the other selected cards trail it on softer springs,
//!   each smaller than the one ahead.
//! - A two-finger swipe across a card drags it (or the selection) the same way;
//!   a vertical swipe scrolls the column.
//!
//! The canvas takes all pointer input and hit-tests the cards itself, so a card
//! can move onto the overlay above the column headers while it is dragged.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAutoresizingMaskOptions, NSColor, NSFont, NSFontWeightBold,
    NSFontWeightMedium, NSFontWeightSemibold, NSImage, NSImageView, NSLineBreakMode,
    NSTextAlignment, NSTextField, NSView,
};
use objc2_core_image::CIFilter;
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString, NSTimer};
use objc2_quartz_core::CACurrentMediaTime;
use yatt_core::{Status, TaskId};

use crate::classes::{
    CanvasMsg, CanvasView, Fill, FillView, FlippedView, OverlayView, Scroll, SheenView, Swipe,
};
use crate::model::{CardSnap, ColumnSnap, UiEvent, format_duration, moves_into};
use crate::motion;
use crate::physics::{self, Body};
use crate::style::{self, CARD_H, CARD_W};
use crate::window::{Sink, animate};

/// Pixels the pointer (or fingers) must travel before a press becomes a drag.
const DRAG_THRESHOLD: f64 = 4.0;
/// Where a trailing card rests relative to the one ahead of it: up and to the right.
const TRAIN_OFFSET: (f64, f64) = (20.0, -35.0);
/// A fanned card under the pointer grows a little, keeping its place in the order.
const HOVER_SCALE: f64 = 1.03;
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
    /// A two-finger trackpad swipe step: finger travel since the last step.
    Swipe {
        at: (f64, f64),
        finger: (f64, f64),
        phase: DemoPhase,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoPhase {
    Began,
    Changed,
    Ended,
}

/// One column's horizontal extent on the canvas.
#[derive(Clone, Copy, Default)]
pub struct ColumnGeom {
    pub x: f64,
    pub w: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    List,
    Stack { fanned: bool },
}

#[derive(Clone, PartialEq)]
enum Look {
    Solid,
    /// Leading a dragged selection: content blurred, the target column written on it.
    Lifted(String),
}

struct Card {
    view: Retained<FlippedView>,
    face: Retained<FillView>,
    content: Retained<FlippedView>,
    icon: Retained<NSImageView>,
    dots: Retained<NSTextField>,
    time: Retained<NSTextField>,
    title: Retained<NSTextField>,
    badge_bg: Retained<FillView>,
    badge: Retained<NSTextField>,
    label: Retained<NSTextField>,
    hue: usize,
    tracking: bool,
    selected: bool,
    body: Body,
    look: Look,
    badge_text: String,
    z: f64,
    removing: bool,
    /// On the overlay above the column headers instead of the clipped canvas.
    lifted: bool,
    /// Flying in from the new-task field.
    flying: bool,
    /// Drawn scale about the centre, easing towards `target_scale`.
    scale: f64,
    target_scale: f64,
    /// Text hidden: behind the top card of a stack.
    muted: bool,
}

struct Press {
    id: Option<TaskId>,
    start: NSPoint,
    /// No modifiers: a click opens the card (or fans its stack).
    plain: bool,
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
}

/// A two-finger trackpad gesture over the canvas.
#[derive(Clone, Copy)]
enum Gesture {
    Idle,
    /// Not sure yet whether it scrolls or drags.
    Deciding {
        at: NSPoint,
        finger: (f64, f64),
        scroll: f64,
    },
    Scrolling(Option<usize>),
    Dragging((f64, f64)),
    /// Momentum after a drag: ignored.
    Spent,
}

pub struct Board {
    mtm: MainThreadMarker,
    pub canvas: Retained<CanvasView>,
    /// Above the column headers; dragged cards ride here.
    pub overlay: Retained<OverlayView>,
    /// The canvas's top edge in overlay coordinates.
    overlay_dy: Cell<f64>,
    sink: Sink,
    statuses: [Status; 3],
    titles: RefCell<[String; 3]>,
    geom: Cell<[ColumnGeom; 3]>,
    height: Cell<f64>,
    order: RefCell<[Vec<TaskId>; 3]>,
    cards: RefCell<HashMap<TaskId, Card>>,
    layouts: Cell<[Layout; 3]>,
    scroll: Cell<[f64; 3]>,
    /// Selected cards, and the one a ⇧-click range starts from.
    selected: RefCell<HashSet<TaskId>>,
    anchor: Cell<Option<TaskId>>,
    press: RefCell<Option<Press>>,
    drag: RefCell<Option<Drag>>,
    gesture: Cell<Gesture>,
    /// The card order last applied to the views, and which of them were lifted.
    stacking: RefCell<(Vec<TaskId>, Vec<bool>)>,
    /// The card under the pointer, for the glass highlight in a fanned stack.
    hover: Cell<Option<TaskId>>,
    /// The card standing in for the open task modal, kept invisible.
    hidden: Cell<Option<TaskId>>,
    /// A task just typed into the new-task field: its card starts here (canvas coordinates).
    spawn: Cell<Option<(NSPoint, Status)>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<f64>,
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
        let board = Rc::new(Self {
            mtm,
            canvas,
            overlay,
            overlay_dy: Cell::new(0.0),
            sink,
            statuses: Status::ALL,
            titles: RefCell::new(Default::default()),
            geom: Cell::new(Default::default()),
            height: Cell::new(0.0),
            order: RefCell::new(Default::default()),
            cards: RefCell::new(HashMap::new()),
            layouts: Cell::new([Layout::List; 3]),
            scroll: Cell::new([0.0; 3]),
            selected: RefCell::new(HashSet::new()),
            anchor: Cell::new(None),
            press: RefCell::new(None),
            drag: RefCell::new(None),
            gesture: Cell::new(Gesture::Idle),
            hover: Cell::new(None),
            stacking: RefCell::new(Default::default()),
            hidden: Cell::new(None),
            spawn: Cell::new(None),
            timer: RefCell::new(None),
            last_tick: Cell::new(0.0),
            on_target: Box::new(on_target),
        });
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
        for ci in 0..3 {
            self.scroll_column(ci, 0.0);
        }
        self.retarget();
        // A resize moves the columns under the cards: put the cards there now
        // instead of letting them spring after.
        let dragged = self.dragged();
        for (id, card) in self.cards.borrow_mut().iter_mut() {
            if !dragged.contains(id) && !card.flying {
                card.body.settle();
                card.apply_frame(overlay_dy);
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
            let n = col.cards.len();
            for (i, snap) in col.cards.iter().enumerate() {
                seen.insert(snap.id);
                let badge = if i == 0 && n > 1 {
                    format!("ещё {}", n - 1)
                } else {
                    String::new()
                };
                let exists = self.cards.borrow().contains_key(&snap.id);
                if !exists {
                    let mut card = self.make_card(snap);
                    let spawn = self.spawn.get().filter(|(_, s)| *s == col.status);
                    if let (Some((p, _)), 0) = (spawn, i) {
                        self.spawn.set(None);
                        card.body = Body::at(p.x - CARD_W / 2.0, p.y - CARD_H / 2.0);
                        card.flying = true;
                        card.lift(&self.overlay);
                    } else {
                        let (x, y) = self.slot(ci, i);
                        card.body = Body::at(x, y - 8.0);
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
                    card.update(snap, badge);
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
        for ci in 0..3 {
            self.scroll_column(ci, 0.0);
        }
        self.retarget();
        self.kick();
    }

    /// The next new card of `status` starts at `p` (canvas coordinates) and flies to its slot.
    pub fn spawn_from(&self, p: NSPoint, status: Status) {
        self.spawn.set(Some((p, status)));
    }

    /// The card's frame on the canvas, unrotated, where it is headed.
    pub fn card_rect(&self, id: TaskId) -> Option<NSRect> {
        self.cards.borrow().get(&id).map(|c| {
            NSRect::new(
                NSPoint::new(c.body.tx, c.body.ty),
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
        self.scroll.set([0.0; 3]);
        self.retarget();
        self.kick();
    }

    /// Esc: drops the selection and folds fanned stacks. Returns true when there was any.
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
            },
            DemoPointer::Drag(at) => CanvasMsg::Dragged(pt(at)),
            DemoPointer::Up(at) => CanvasMsg::Up(pt(at)),
            DemoPointer::Hover(at) => CanvasMsg::Hover(at.map(pt)),
            DemoPointer::Swipe { at, finger, phase } => CanvasMsg::Scroll(Scroll {
                at: pt(at),
                finger,
                scroll: -finger.1,
                swipe: match phase {
                    DemoPhase::Began => Swipe::Began,
                    DemoPhase::Changed => Swipe::Changed,
                    DemoPhase::Ended => Swipe::Ended,
                },
            }),
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
            Layout::List => (
                self.list_x(ci),
                LIST_TOP + i as f64 * PITCH - self.scroll.get()[ci],
            ),
            Layout::Stack { .. } => {
                let a = self.stack_anchor(ci);
                (a.0, a.1 + physics::pile(i).0)
            }
        }
    }

    fn list_x(&self, ci: usize) -> f64 {
        let g = self.geom.get()[ci];
        (g.x + (g.w - CARD_W) / 2.0).round()
    }

    /// Top-left of a stack's top card, centred in the column.
    fn stack_anchor(&self, ci: usize) -> (f64, f64) {
        let g = self.geom.get()[ci];
        ((g.x + (g.w - CARD_W) / 2.0).round(), LIST_TOP + 10.0)
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
        if let Some(layer) = view.layer() {
            layer.setShadowColor(Some(&NSColor::blackColor().CGColor()));
            // A soft, wide, faint grey shadow.
            layer.setShadowOpacity(0.1);
            layer.setShadowRadius(12.0);
            layer.setShadowOffset(NSSize::new(0.0, -2.0));
        }
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
        let sheen = SheenView::new(mtm, style::CARD_RADIUS);
        sheen.setFrame(bounds);
        sheen.setAutoresizingMask(fill_mask);

        let content = FlippedView::new(mtm, bounds);
        content.setAutoresizingMask(fill_mask);
        content.setWantsLayer(true);
        content.setLayerUsesCoreImageFilters(true);

        let icon = NSImageView::new(mtm);
        icon.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(style::glyph_of(snap.id)),
                None,
            )
            .as_deref(),
        );
        icon.setFrame(NSRect::new(
            NSPoint::new(14.0, 13.0),
            NSSize::new(18.0, 18.0),
        ));
        let dots = text_label(mtm, "···", 13.0, unsafe { NSFontWeightBold });
        dots.setAlignment(NSTextAlignment::Right);
        dots.setFrame(NSRect::new(
            NSPoint::new(CARD_W - 44.0, 9.0),
            NSSize::new(30.0, 18.0),
        ));
        let time = text_label(mtm, "", 11.0, unsafe { NSFontWeightSemibold });
        time.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0,
            unsafe { NSFontWeightSemibold },
        )));
        time.setFrame(NSRect::new(
            NSPoint::new(14.0, CARD_H - 44.0),
            NSSize::new(CARD_W - 28.0, 15.0),
        ));
        let title = text_label(mtm, &snap.title, 14.0, unsafe { NSFontWeightSemibold });
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        title.setFrame(NSRect::new(
            NSPoint::new(14.0, CARD_H - 29.0),
            NSSize::new(CARD_W - 28.0, 18.0),
        ));
        let badge_bg = FillView::new(mtm, Fill::Badge);
        let badge = text_label(mtm, "", 11.0, unsafe { NSFontWeightMedium });
        badge.setAlignment(NSTextAlignment::Center);
        for v in [&*icon as &NSView, &dots, &time, &title, &badge_bg, &badge] {
            content.addSubview(v);
        }

        let label = text_label(mtm, "", 22.0, unsafe { NSFontWeightSemibold });
        label.setAlignment(NSTextAlignment::Center);
        label.setTextColor(Some(&style::text(hue)));
        label.setFrame(NSRect::new(
            NSPoint::new(8.0, (CARD_H - 30.0) / 2.0),
            NSSize::new(CARD_W - 16.0, 30.0),
        ));
        label.setAlphaValue(0.0);

        for v in [&*glass as &NSView, &face, &sheen, &content, &label] {
            view.addSubview(v);
        }
        self.canvas.addSubview(&view);

        let mut card = Card {
            view,
            face,
            content,
            icon,
            dots,
            time,
            title,
            badge_bg,
            badge,
            label,
            hue,
            tracking: false,
            selected: false,
            body: Body::at(0.0, 0.0),
            look: Look::Lifted(String::new()),
            badge_text: String::new(),
            z: 0.0,
            removing: false,
            lifted: false,
            flying: false,
            scale: 1.0,
            target_scale: 1.0,
            muted: false,
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
        let scroll = self.scroll.get();
        let selected = self.selected.borrow();
        let hover = self.hover.get();
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
                    // A rosette round the stack's centre, kept clear of the canvas top.
                    let a = self.stack_anchor(ci);
                    let reach = physics::fan_radius(n, w) + 0.5 * (w * w + h * h).sqrt();
                    let centre = (a.0 + w / 2.0, (a.1 + h / 2.0).max(reach + LIST_TOP));
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let (dx, dy, deg) = physics::fan(i, n, w);
                        card.body
                            .target(centre.0 + dx - w / 2.0, centre.1 + dy - h / 2.0, deg);
                        card.body.spring = physics::PILE;
                        card.set_muted(false);
                        card.z = (n - i) as f64;
                        card.target_scale = if hover == Some(*id) { HOVER_SCALE } else { 1.0 };
                        card.set_look(Look::Solid, true);
                        card.set_badge("");
                    }
                }
                Layout::Stack { .. } => {
                    let a = self.stack_anchor(ci);
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let (dy, scale) = physics::pile(i);
                        card.body.target(a.0, a.1 + dy, 0.0);
                        // Only the top card's text; the rest show as coloured slivers.
                        card.set_muted(i > 0);
                        card.body.spring = physics::PILE;
                        card.z = (n - i) as f64;
                        card.target_scale = scale;
                        card.set_look(Look::Solid, true);
                        let badge = card.badge_text.clone();
                        card.set_badge(if i == 0 { &badge } else { "" });
                    }
                }
                Layout::List => {
                    // Make room where the dragged cards would land.
                    let gap = match drag.as_ref() {
                        Some(d) if d.target == Some(ci) => Some((d.index, d.ids.len())),
                        _ => None,
                    };
                    let x = self.list_x(ci);
                    for (i, id) in ids.iter().enumerate() {
                        let Some(card) = cards.get_mut(id) else {
                            continue;
                        };
                        let slot = match gap {
                            Some((at, k)) if i >= at => i + k,
                            _ => i,
                        };
                        card.body
                            .target(x, LIST_TOP + slot as f64 * PITCH - scroll[ci], 0.0);
                        card.set_muted(false);
                        card.body.spring = physics::SLOT;
                        card.z = 1.0;
                        card.target_scale = 1.0;
                        card.set_look(Look::Solid, true);
                        card.set_badge("");
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
                card.z = 1000.0 - i as f64;
                card.set_muted(false);
                card.target_scale = if drag.group {
                    physics::trail_scale(i)
                } else {
                    1.0
                };
                card.set_badge("");
                if i == 0 {
                    card.body.spring = physics::LEAD;
                    let look = if drag.group {
                        Look::Lifted(label.clone())
                    } else {
                        Look::Solid
                    };
                    card.set_look(look, true);
                } else {
                    card.body.spring = physics::follower(i);
                    card.set_look(Look::Solid, true);
                }
            }
        }

        for (id, card) in cards.iter_mut() {
            if card.flying {
                card.body.spring = physics::FLY;
                card.z = 900.0;
            }
            card.set_selected(selected.contains(id));
        }
        self.restack(&cards);
    }

    /// Orders the canvas's and the overlay's subviews by each card's `z`. Layer
    /// zPosition alone does not hold once a view has been moved between them.
    fn restack(&self, cards: &HashMap<TaskId, Card>) {
        let mut sorted: Vec<(&TaskId, &Card)> = cards.iter().collect();
        sorted.sort_by(|a, b| a.1.z.total_cmp(&b.1.z).then(a.0.cmp(b.0)));
        let order: Vec<TaskId> = sorted.iter().map(|(id, _)| **id).collect();
        let lifted: Vec<bool> = sorted.iter().map(|(_, c)| c.lifted).collect();
        let key = (order, lifted);
        if *self.stacking.borrow() == key {
            return;
        }
        for (parent, on_overlay) in [(&*self.canvas as &NSView, false), (&self.overlay, true)] {
            let views: Vec<Retained<NSView>> = sorted
                .iter()
                .filter(|(_, c)| c.lifted == on_overlay)
                .map(|(_, c)| Retained::into_super(c.view.clone()))
                .collect();
            parent.setSubviews(&NSArray::from_retained_slice(&views));
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

        if let Some(drag) = self.drag.borrow().as_ref() {
            let mut ahead: Option<(f64, f64)> = None;
            for (i, id) in drag.ids.iter().enumerate() {
                let Some(card) = cards.get_mut(id) else {
                    continue;
                };
                let (tx, ty) = match ahead {
                    None => (drag.cursor.0 - drag.grab.0, drag.cursor.1 - drag.grab.1),
                    Some((x, y)) => (x + TRAIN_OFFSET.0, y + TRAIN_OFFSET.1),
                };
                if drag.group {
                    // The lead chases the pointer; each trailing card chases the one ahead.
                    let max = if i == 0 {
                        physics::LEAD_MAX_TILT
                    } else {
                        physics::FOLLOWER_MAX_TILT
                    };
                    card.body.target(tx, ty, physics::tilt(card.body.vx, max));
                } else {
                    // A single card sticks to the pointer.
                    card.body.target(tx, ty, 0.0);
                    card.body.settle();
                }
                ahead = Some((card.body.x, card.body.y));
            }
        }

        let mut moving = dragging;
        let drag_ids = self.dragged();
        for card in cards.values_mut() {
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
        // Landed cards go back under the column headers.
        for (id, card) in cards.iter_mut() {
            if card.lifted && !card.flying && !drag_ids.contains(id) && card.body.at_rest() {
                card.land(&self.canvas);
                card.apply_frame(overlay_dy);
            }
        }
        self.restack(&cards);
        drop(cards);
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
        if self.scroll_column(ci, speed.clamp(-1.0, 1.0) * AUTOSCROLL_SPEED * dt) {
            let cursor = self.drag.borrow().as_ref().map(|d| d.cursor);
            if let Some((x, y)) = cursor {
                self.drag_to(NSPoint::new(x, y));
            }
        }
    }

    /// Scrolls a list column by `delta` (clamped), moving its cards with it right away.
    /// Returns true when it moved.
    fn scroll_column(&self, ci: usize, delta: f64) -> bool {
        let mut scroll = self.scroll.get();
        if self.layouts.get()[ci] != Layout::List {
            scroll[ci] = 0.0;
            self.scroll.set(scroll);
            return false;
        }
        let n = self.order.borrow()[ci].len();
        let max = (LIST_TOP + n as f64 * PITCH + LIST_BOTTOM - self.height.get()).max(0.0);
        let new = (scroll[ci] + delta).clamp(0.0, max);
        let d = new - scroll[ci];
        scroll[ci] = new;
        self.scroll.set(scroll);
        if d == 0.0 {
            return false;
        }
        let drag_ids = self.dragged();
        let overlay_dy = self.overlay_dy.get();
        let mut cards = self.cards.borrow_mut();
        for id in &self.order.borrow()[ci] {
            if drag_ids.contains(id) {
                continue;
            }
            if let Some(card) = cards.get_mut(id) {
                card.body.y -= d;
                card.body.ty -= d;
                card.apply_frame(overlay_dy);
            }
        }
        true
    }

    // ---- input ----

    fn on_msg(self: &Rc<Self>, msg: CanvasMsg) {
        match msg {
            CanvasMsg::Down { at, shift, cmd } => self.on_down(at, shift, cmd),
            CanvasMsg::Dragged(p) => {
                if self.drag.borrow().is_none() {
                    let Some(press) = self.press.borrow_mut().take() else {
                        return;
                    };
                    let far = (p.x - press.start.x).hypot(p.y - press.start.y) >= DRAG_THRESHOLD;
                    match press.id {
                        Some(id) if far => self.begin_drag(id, press.start, p),
                        _ => {
                            *self.press.borrow_mut() = Some(press);
                            return;
                        }
                    }
                }
                self.drag_to(p);
            }
            CanvasMsg::Up(p) => {
                if self.drag.borrow().is_some() {
                    self.drop_at(p);
                } else if let Some(press) = self.press.borrow_mut().take()
                    && press.plain
                {
                    self.click(press.id);
                }
            }
            CanvasMsg::Scroll(s) => self.on_scroll(s),
            CanvasMsg::Hover(p) => {
                let id = p.and_then(|p| self.hit(p));
                if self.drag.borrow().is_none() && self.hover.replace(id) != id {
                    self.retarget();
                    self.kick();
                }
            }
        }
    }

    fn on_down(self: &Rc<Self>, at: NSPoint, shift: bool, cmd: bool) {
        let id = self.hit(at);
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
    fn click(self: &Rc<Self>, id: Option<TaskId>) {
        let Some(id) = id else {
            self.clear_selection();
            return;
        };
        let col = self.column_of(id);
        let mut layouts = self.layouts.get();
        if let Some(ci) = col
            && layouts[ci] == (Layout::Stack { fanned: false })
            && self.order.borrow()[ci].len() > 1
        {
            layouts[ci] = Layout::Stack { fanned: true };
            self.layouts.set(layouts);
            self.retarget();
            self.kick();
            return;
        }
        self.selected.borrow_mut().clear();
        self.anchor.set(Some(id));
        self.retarget();
        (self.sink)(UiEvent::ToggleExpand(id));
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
            .filter(|(_, c)| !c.removing && c.contains(p))
            .max_by(|a, b| a.1.z.total_cmp(&b.1.z))
            .map(|(id, _)| *id)
    }

    fn on_scroll(self: &Rc<Self>, s: Scroll) {
        let col = self.column_at(s.at.x);
        match (s.swipe, self.gesture.get()) {
            (Swipe::None, _) => {
                if let Some(ci) = col {
                    self.scroll_column(ci, s.scroll);
                }
            }
            (Swipe::Began, _) => {
                self.gesture.set(Gesture::Deciding {
                    at: s.at,
                    finger: s.finger,
                    scroll: s.scroll,
                });
            }
            (Swipe::Changed, Gesture::Deciding { at, finger, scroll }) => {
                let finger = (finger.0 + s.finger.0, finger.1 + s.finger.1);
                let scroll = scroll + s.scroll;
                if finger.0.hypot(finger.1) < DRAG_THRESHOLD * 1.5 {
                    self.gesture.set(Gesture::Deciding { at, finger, scroll });
                    return;
                }
                let sideways = finger.0.abs() > finger.1.abs();
                match self.hit(at) {
                    Some(id) if sideways => {
                        let cursor = (at.x + finger.0, at.y + finger.1);
                        self.begin_drag(id, at, NSPoint::new(cursor.0, cursor.1));
                        self.drag_to(NSPoint::new(cursor.0, cursor.1));
                        self.gesture.set(Gesture::Dragging(cursor));
                    }
                    _ => {
                        let ci = self.column_at(at.x);
                        if let Some(ci) = ci {
                            self.scroll_column(ci, scroll);
                        }
                        self.gesture.set(Gesture::Scrolling(ci));
                    }
                }
            }
            (Swipe::Changed | Swipe::Coasting, Gesture::Scrolling(ci)) => {
                if let Some(ci) = ci {
                    self.scroll_column(ci, s.scroll);
                }
            }
            (Swipe::Changed, Gesture::Dragging(c)) => {
                let cursor = (c.0 + s.finger.0, c.1 + s.finger.1);
                self.drag_to(NSPoint::new(cursor.0, cursor.1));
                self.gesture.set(Gesture::Dragging(cursor));
            }
            (Swipe::Ended, Gesture::Dragging(c)) => {
                self.drop_at(NSPoint::new(c.0, c.1));
                self.gesture.set(Gesture::Spent);
            }
            (Swipe::Ended, Gesture::Deciding { .. }) => self.gesture.set(Gesture::Idle),
            (Swipe::Coasting, Gesture::Idle) => {
                if let Some(ci) = col {
                    self.scroll_column(ci, s.scroll);
                }
            }
            _ => {}
        }
    }

    fn begin_drag(self: &Rc<Self>, id: TaskId, start: NSPoint, p: NSPoint) {
        let Some(col) = self.column_of(id) else {
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
        let Some(body) = self.cards.borrow().get(&id).map(|c| c.body) else {
            return;
        };
        {
            let mut cards = self.cards.borrow_mut();
            for (i, x) in ids.iter().enumerate() {
                if let Some(card) = cards.get_mut(x) {
                    card.lift(&self.overlay);
                    card.set_raised(i == 0);
                }
            }
        }
        *self.press.borrow_mut() = None;
        let group = ids.len() > 1;
        *self.drag.borrow_mut() = Some(Drag {
            ids,
            group,
            grab: (start.x - body.x, start.y - body.y),
            cursor: (p.x, p.y),
            target: Some(col),
            index: 0,
        });
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
                    let centre = p.y - drag.grab.1 + CARD_H / 2.0 + self.scroll.get()[ci];
                    physics::insert_index(centre + LIST_GAP / 2.0, LIST_TOP, PITCH, n)
                }
                _ => 0,
            };
            let changed = drag.target != target;
            drag.target = target;
            drag.index = index;
            changed
        };
        if changed {
            (self.on_target)(target);
        }
        self.retarget();
        self.kick();
    }

    fn drop_at(self: &Rc<Self>, p: NSPoint) {
        self.drag_to(p);
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        (self.on_target)(None);
        {
            let mut cards = self.cards.borrow_mut();
            for id in &drag.ids {
                if let Some(card) = cards.get_mut(id) {
                    card.set_raised(false);
                }
            }
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
            for e in moves_into(&before, &ids, drag.index, self.statuses[col]) {
                (self.sink)(e);
            }
        }
        self.retarget();
        self.kick();
    }
}

impl Card {
    fn update(&mut self, snap: &CardSnap, badge: String) {
        self.title.setStringValue(&NSString::from_str(&snap.title));
        let notes = if snap.notes.trim().is_empty() {
            ""
        } else {
            "  ·  ✎"
        };
        let time = format!(
            "{}{}{}",
            if snap.tracking { "● " } else { "" },
            format_duration(snap.total_ms),
            notes
        );
        self.time.setStringValue(&NSString::from_str(&time));
        self.tracking = snap.tracking;
        self.refill();
        self.badge_text = badge;
    }

    fn set_selected(&mut self, on: bool) {
        if self.selected != on {
            self.selected = on;
            self.refill();
        }
    }

    fn refill(&self) {
        self.face.set_fill(Fill::Card {
            hue: self.hue,
            tracking: self.tracking,
            selected: self.selected,
        });
    }

    /// Moves the card onto the overlay, above the column headers.
    fn lift(&mut self, overlay: &OverlayView) {
        if !self.lifted {
            self.lifted = true;
            self.view.removeFromSuperview();
            overlay.addSubview(&self.view);
        }
    }

    /// Back onto the canvas, under the headers and clipped to it.
    fn land(&mut self, canvas: &CanvasView) {
        if self.lifted {
            self.lifted = false;
            self.view.removeFromSuperview();
            canvas.addSubview(&self.view);
        }
    }

    /// A deeper shadow while held.
    fn set_raised(&self, on: bool) {
        if let Some(layer) = self.view.layer() {
            layer.setShadowOpacity(if on { 0.18 } else { 0.1 });
            layer.setShadowRadius(if on { 16.0 } else { 12.0 });
            layer.setShadowOffset(NSSize::new(0.0, if on { -8.0 } else { -2.0 }));
        }
    }

    fn set_badge(&self, text: &str) {
        let show = !text.is_empty();
        self.badge.setStringValue(&NSString::from_str(text));
        let w = self.badge.fittingSize().width + 18.0;
        let frame = NSRect::new(NSPoint::new((CARD_W - w) / 2.0, 9.0), NSSize::new(w, 18.0));
        self.badge_bg.setFrame(frame);
        self.badge.setFrame(NSRect::new(
            NSPoint::new(frame.origin.x, 10.0),
            NSSize::new(w, 16.0),
        ));
        self.badge.setHidden(!show);
        self.badge_bg.setHidden(!show);
    }

    /// Blurs the content and shows the target label while leading a drag,
    /// cross-fading when `animated`.
    fn set_look(&mut self, look: Look, animated: bool) {
        if self.look == look {
            return;
        }
        let lifted = matches!(look, Look::Lifted(_));
        if let Look::Lifted(text) = &look {
            self.label.setStringValue(&NSString::from_str(text));
        }
        let ink = style::text(self.hue);
        // Dark tones of the hue at reduced opacity, as in the reference.
        self.title
            .setTextColor(Some(&ink.colorWithAlphaComponent(0.85)));
        self.time
            .setTextColor(Some(&ink.colorWithAlphaComponent(0.6)));
        self.badge
            .setTextColor(Some(&ink.colorWithAlphaComponent(0.6)));
        self.dots
            .setTextColor(Some(&ink.colorWithAlphaComponent(0.35)));
        self.icon
            .setContentTintColor(Some(&ink.colorWithAlphaComponent(0.85)));
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
        let views = (self.label.clone(), self.content.clone());
        animate(animated, motion::GLASS_SECS, move || {
            views.0.animator().setAlphaValue(label);
            views.1.animator().setAlphaValue(content);
        });
        self.look = look;
    }

    /// Hides the card's text (a card behind the top of a stack).
    fn set_muted(&mut self, on: bool) {
        if self.muted == on {
            return;
        }
        self.muted = on;
        let (content, alpha) = (self.content.clone(), if on { 0.0 } else { 1.0 });
        animate(true, motion::GLASS_SECS, move || {
            content.animator().setAlphaValue(alpha)
        });
    }

    /// Places the view; on the overlay the canvas's offset is added.
    fn apply_frame(&self, overlay_dy: f64) {
        let b = &self.body;
        let dy = if self.lifted { overlay_dy } else { 0.0 };
        let s = self.scale;
        let (w, h) = (CARD_W * s, CARD_H * s);
        self.view.setFrameCenterRotation(0.0);
        self.view.setFrame(NSRect::new(
            NSPoint::new(b.x + (CARD_W - w) / 2.0, b.y + dy + (CARD_H - h) / 2.0),
            NSSize::new(w, h),
        ));
        // Bounds stay card-sized, so the content scales with the frame.
        self.view.setBoundsSize(NSSize::new(CARD_W, CARD_H));
        // The canvas is flipped, so AppKit's positive rotation already reads clockwise.
        self.view.setFrameCenterRotation(b.r);
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

fn text_label(mtm: MainThreadMarker, text: &str, size: f64, weight: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    l
}
