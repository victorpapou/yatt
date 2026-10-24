//! The cards, laid out and moved like the reference clip (docs/reference/card-motion.md).
//!
//! Every card lives on one canvas above the three columns and is driven by a spring
//! ([`crate::physics`]) stepped by a display-rate timer while anything moves:
//!
//! - At rest each column is one stack: the top card is frosted glass with an
//!   "ещё N" badge, the rest peek out around it.
//! - Hovering a stack fans it out like a swatch book; the card under the cursor turns
//!   to glass.
//! - Dragging a card lifts it as glass with its content blurred and the target column
//!   written on it, tilting with its speed. Shift-drag takes the whole stack: the other
//!   cards trail behind on softer springs. Dropping makes the card(s) the top of the
//!   target stack.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua,
    NSAutoresizingMaskOptions, NSColor, NSFont, NSFontWeightBold, NSFontWeightSemibold, NSImage,
    NSImageView, NSLineBreakMode, NSTextAlignment, NSTextField, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_core_image::CIFilter;
use objc2_foundation::{
    NSArray, NSNumber, NSObjectNSKeyValueCoding, NSPoint, NSRect, NSSize, NSString, NSTimer,
    ns_string,
};
use objc2_quartz_core::CACurrentMediaTime;
use yatt_core::{Status, TaskId};

use crate::classes::{CanvasView, CardMsg, CardView, Fill, FillView, FlippedView};
use crate::model::{CardSnap, ColumnSnap, UiEvent, format_duration};
use crate::motion;
use crate::physics::{self, Body};
use crate::style::{self, CARD_H, CARD_W};
use crate::window::{Sink, animate};

/// The canvas is flipped, so AppKit's positive rotation already reads clockwise,
/// like the physics.
const ROT_SIGN: f64 = 1.0;
/// Pixels the mouse must travel before a press becomes a drag.
const DRAG_THRESHOLD: f64 = 4.0;
/// Where a trailing card rests relative to the one ahead of it.
const TRAIN_OFFSET: (f64, f64) = (-7.0, 5.0);
/// Content blur on a lifted card, in points.
const LIFT_BLUR: f64 = 5.0;

/// Synthetic pointer input for demos, in canvas coordinates.
#[derive(Debug, Clone, Copy)]
pub enum DemoPointer {
    Hover(Option<(f64, f64)>),
    Down {
        id: TaskId,
        at: (f64, f64),
        shift: bool,
    },
    Drag(TaskId, (f64, f64)),
    Up(TaskId, (f64, f64)),
}

/// Geometry of one column on the canvas.
#[derive(Clone, Copy)]
pub struct ColumnGeom {
    /// The whole column, for drop targeting.
    pub rect: NSRect,
    /// Top-left of the stack's top card at rest.
    pub anchor: NSPoint,
    /// Highest y a fanned card may reach.
    pub top_limit: f64,
}

#[derive(Clone, PartialEq)]
enum Look {
    Solid,
    Glass,
    /// Being dragged: glass, blurred content, the target column written on it.
    Lifted(String),
}

struct Card {
    view: Retained<CardView>,
    face: Retained<FillView>,
    glass: Retained<NSVisualEffectView>,
    frost: Retained<FillView>,
    content: Retained<FlippedView>,
    icon: Retained<NSImageView>,
    dots: Retained<NSTextField>,
    time: Retained<NSTextField>,
    title: Retained<NSTextField>,
    badge_bg: Retained<FillView>,
    badge: Retained<NSTextField>,
    label: Retained<NSTextField>,
    hue: usize,
    body: Body,
    look: Look,
    badge_text: String,
    z: f64,
    removing: bool,
}

struct Press {
    id: TaskId,
    start: NSPoint,
    shift: bool,
}

struct Drag {
    /// Lead first, then the cards that trail it.
    ids: Vec<TaskId>,
    grab: (f64, f64),
    cursor: (f64, f64),
    target: Option<usize>,
}

pub struct Board {
    mtm: MainThreadMarker,
    pub canvas: Retained<CanvasView>,
    sink: Sink,
    statuses: [Status; 3],
    titles: RefCell<[String; 3]>,
    geom: RefCell<[ColumnGeom; 3]>,
    order: RefCell<[Vec<TaskId>; 3]>,
    cards: RefCell<HashMap<TaskId, Card>>,
    /// The column whose stack is fanned out, and the card under the cursor in it.
    hover: Cell<Option<usize>>,
    hover_card: Cell<Option<TaskId>>,
    press: RefCell<Option<Press>>,
    drag: RefCell<Option<Drag>>,
    /// The card standing in for the open task modal, kept invisible.
    hidden: Cell<Option<TaskId>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<f64>,
    /// A demo script drives the pointer; real mouse movement is ignored meanwhile.
    demo: Cell<bool>,
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
        let geom = ColumnGeom {
            rect: NSRect::ZERO,
            anchor: NSPoint::ZERO,
            top_limit: 0.0,
        };
        let board = Rc::new(Self {
            mtm,
            canvas,
            sink,
            statuses: Status::ALL,
            titles: RefCell::new(Default::default()),
            geom: RefCell::new([geom; 3]),
            order: RefCell::new(Default::default()),
            cards: RefCell::new(HashMap::new()),
            hover: Cell::new(None),
            hover_card: Cell::new(None),
            press: RefCell::new(None),
            drag: RefCell::new(None),
            hidden: Cell::new(None),
            timer: RefCell::new(None),
            last_tick: Cell::new(0.0),
            demo: Cell::new(false),
            on_target: Box::new(on_target),
        });
        let weak = Rc::downgrade(&board);
        *board.canvas.ivars().on_hover.borrow_mut() = Some(Box::new(move |p| {
            if let Some(b) = weak.upgrade()
                && !b.demo.get()
            {
                b.on_hover(p);
            }
        }));
        board
    }

    pub fn set_geometry(self: &Rc<Self>, geom: [ColumnGeom; 3]) {
        *self.geom.borrow_mut() = geom;
        self.retarget();
        self.kick();
    }

    /// Brings cards in line with a snapshot: new ones fade in at their spot,
    /// gone ones fade out, the rest spring to their new places.
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
                    let card = self.make_card(snap);
                    let mut card = card;
                    // Appear where the stack is, so it settles into place.
                    let a = self.geom.borrow()[ci].anchor;
                    card.body = Body::at(a.x, a.y - 8.0);
                    card.view.setAlphaValue(0.0);
                    let view = card.view.clone();
                    animate(true, motion::APPEAR_SECS, move || {
                        view.animator().setAlphaValue(1.0)
                    });
                    self.cards.borrow_mut().insert(snap.id, card);
                }
                if let Some(card) = self.cards.borrow_mut().get_mut(&snap.id) {
                    card.removing = false;
                    card.update(snap, badge);
                }
            }
        }
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

    /// The card's frame on the canvas, unrotated.
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

    pub fn demo_anchor(&self, column: usize) -> (f64, f64) {
        let a = self.geom.borrow()[column].anchor;
        (a.x, a.y)
    }

    pub fn demo_pointer(self: &Rc<Self>, input: DemoPointer) {
        let to_window =
            |(x, y): (f64, f64)| self.canvas.convertPoint_toView(NSPoint::new(x, y), None);
        self.demo.set(true);
        match input {
            DemoPointer::Hover(p) => self.on_hover(p.map(|(x, y)| NSPoint::new(x, y))),
            DemoPointer::Down { id, at, shift } => self.on_card(
                id,
                CardMsg::Down {
                    at: to_window(at),
                    shift,
                },
            ),
            DemoPointer::Drag(id, at) => self.on_card(id, CardMsg::Dragged(to_window(at))),
            DemoPointer::Up(id, at) => self.on_card(id, CardMsg::Up(to_window(at))),
        }
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

    // ---- cards ----

    fn make_card(self: &Rc<Self>, snap: &CardSnap) -> Card {
        let mtm = self.mtm;
        let hue = style::hue_of(snap.id);
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(CARD_W, CARD_H));
        let view = CardView::new(mtm, snap.id, bounds);
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setShadowColor(Some(&NSColor::blackColor().CGColor()));
            layer.setShadowOpacity(0.12);
            layer.setShadowRadius(6.0);
            layer.setShadowOffset(NSSize::new(0.0, -2.0));
        }
        let weak: Weak<Self> = Rc::downgrade(self);
        *view.ivars().on_msg.borrow_mut() = Some(Box::new(move |id, msg| {
            if let Some(b) = weak.upgrade() {
                b.on_card(id, msg);
            }
        }));
        let fill_mask = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;

        let face = FillView::new(
            mtm,
            Fill::Card {
                hue,
                tracking: false,
            },
        );
        face.setFrame(bounds);
        face.setAutoresizingMask(fill_mask);

        // Real blur of whatever is behind the card inside the window.
        let glass = NSVisualEffectView::new(mtm);
        glass.setFrame(bounds);
        glass.setAutoresizingMask(fill_mask);
        glass.setMaterial(NSVisualEffectMaterial::Popover);
        glass.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        glass.setState(NSVisualEffectState::Active);
        // The clip's glass is light frosted in any system appearance.
        glass.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).as_deref(),
        );
        glass.setWantsLayer(true);
        if let Some(layer) = glass.layer() {
            layer.setCornerRadius(style::CARD_RADIUS);
            layer.setMasksToBounds(true);
        }
        glass.setAlphaValue(0.0);
        let frost = FillView::new(mtm, Fill::Frost);
        frost.setFrame(bounds);
        frost.setAutoresizingMask(fill_mask);
        frost.setAlphaValue(0.0);

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
        let title = text_label(mtm, &snap.title, 13.5, unsafe { NSFontWeightBold });
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        title.setFrame(NSRect::new(
            NSPoint::new(14.0, CARD_H - 29.0),
            NSSize::new(CARD_W - 28.0, 18.0),
        ));
        let badge_bg = FillView::new(mtm, Fill::Badge);
        let badge = text_label(mtm, "", 11.0, unsafe { NSFontWeightSemibold });
        badge.setAlignment(NSTextAlignment::Center);
        for v in [
            &*icon as &objc2_app_kit::NSView,
            &dots,
            &time,
            &title,
            &badge_bg,
            &badge,
        ] {
            content.addSubview(v);
        }

        let label = text_label(mtm, "", 22.0, unsafe { NSFontWeightSemibold });
        label.setAlignment(NSTextAlignment::Center);
        label.setTextColor(Some(&style::glass_text()));
        label.setFrame(NSRect::new(
            NSPoint::new(8.0, (CARD_H - 30.0) / 2.0),
            NSSize::new(CARD_W - 16.0, 30.0),
        ));
        label.setAlphaValue(0.0);

        for v in [
            &*face as &objc2_app_kit::NSView,
            &glass,
            &frost,
            &content,
            &label,
        ] {
            view.addSubview(v);
        }
        self.canvas.addSubview(&view);

        let mut card = Card {
            view,
            face,
            glass,
            frost,
            content,
            icon,
            dots,
            time,
            title,
            badge_bg,
            badge,
            label,
            hue,
            body: Body::at(0.0, 0.0),
            look: Look::Glass,
            badge_text: String::new(),
            z: 0.0,
            removing: false,
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
        let geom = *self.geom.borrow();
        let drag_ids = self.dragged();
        let hover = self.hover.get();
        let hover_card = self.hover_card.get();
        let order = self.order.borrow().clone();
        let mut cards = self.cards.borrow_mut();
        let (w, h) = (CARD_W, CARD_H);

        for (ci, ids) in order.iter().enumerate() {
            let ids: Vec<TaskId> = ids
                .iter()
                .copied()
                .filter(|id| !drag_ids.contains(id))
                .collect();
            let n = ids.len();
            let a = geom[ci].anchor;
            let fanned = hover == Some(ci) && n > 1;
            if fanned {
                let angles = physics::fan_angles(n);
                // Keep the fan's top below the column header: the top card reaches about
                // w·sin45 + h·cos45 above the pivot.
                let reach = w * 0.71 + h * 0.71;
                let pivot = (
                    a.x + 0.08 * w,
                    (a.y + h + 0.25 * h).max(geom[ci].top_limit + reach + 8.0),
                );
                for (i, id) in ids.iter().enumerate() {
                    let Some(card) = cards.get_mut(id) else {
                        continue;
                    };
                    let deg = angles[i];
                    let (x, y) = physics::fan_origin(pivot, w, h, deg);
                    card.body.target(x, y, deg);
                    card.body.spring = physics::PILE;
                    let hovered = hover_card == Some(*id);
                    card.z = if hovered { 500.0 } else { (n - i) as f64 };
                    card.set_look(if hovered { Look::Glass } else { Look::Solid }, true);
                    card.set_badge("");
                }
            } else {
                for (i, id) in ids.iter().enumerate() {
                    let Some(card) = cards.get_mut(id) else {
                        continue;
                    };
                    let (fx, fy) = physics::pile_offset(i);
                    card.body.target(a.x + fx * w, a.y + fy * h, 0.0);
                    card.body.spring = physics::PILE;
                    card.z = (n - i) as f64;
                    let top_of_stack = i == 0 && n > 1;
                    card.set_look(
                        if top_of_stack {
                            Look::Glass
                        } else {
                            Look::Solid
                        },
                        true,
                    );
                    let badge = card.badge_text.clone();
                    card.set_badge(if i == 0 { &badge } else { "" });
                }
            }
        }

        if let Some(drag) = self.drag.borrow().as_ref() {
            let label = drag
                .target
                .map(|c| self.titles.borrow()[c].clone())
                .unwrap_or_default();
            for (i, id) in drag.ids.iter().enumerate() {
                let Some(card) = cards.get_mut(id) else {
                    continue;
                };
                card.z = 1000.0 - i as f64;
                card.set_badge("");
                if i == 0 {
                    card.body.spring = physics::LEAD;
                    card.set_look(Look::Lifted(label.clone()), true);
                } else {
                    card.body.spring = physics::follower(i);
                    card.set_look(Look::Solid, true);
                }
            }
        }

        for card in cards.values() {
            if let Some(layer) = card.view.layer() {
                layer.setZPosition(card.z);
            }
        }
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

    fn tick(&self) {
        let now = CACurrentMediaTime();
        let dt = (now - self.last_tick.replace(now)).clamp(0.0, 1.0 / 30.0);
        let reduce = motion::reduce_motion();
        let mut cards = self.cards.borrow_mut();

        // The lead chases the cursor; each trailing card chases the one ahead.
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
                let max = if i == 0 {
                    physics::LEAD_MAX_TILT
                } else {
                    physics::FOLLOWER_MAX_TILT
                };
                card.body.target(tx, ty, physics::tilt(card.body.vx, max));
                ahead = Some((card.body.x, card.body.y));
            }
        }

        let mut moving = self.drag.borrow().is_some();
        for card in cards.values_mut() {
            if reduce && self.drag.borrow().is_none() {
                card.body.settle();
            } else {
                card.body.step(dt);
            }
            if card.body.at_rest() {
                card.body.settle();
            } else {
                moving = true;
            }
            card.apply_frame();
        }
        drop(cards);
        if !moving && let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
    }

    // ---- input ----

    fn on_hover(self: &Rc<Self>, p: Option<NSPoint>) {
        if self.drag.borrow().is_some() {
            return;
        }
        let (col, card) = match p {
            None => (None, None),
            Some(p) => self.hit(p),
        };
        if col != self.hover.get() || card != self.hover_card.get() {
            self.hover.set(col);
            self.hover_card.set(card);
            self.retarget();
            self.kick();
        }
    }

    /// Which stack the point is over (its cards' bounds, or its rest spot), and the
    /// topmost card under it.
    fn hit(&self, p: NSPoint) -> (Option<usize>, Option<TaskId>) {
        let cards = self.cards.borrow();
        let order = self.order.borrow();
        let geom = *self.geom.borrow();
        for (ci, ids) in order.iter().enumerate() {
            if ids.is_empty() {
                continue;
            }
            let mut bounds = NSRect::new(geom[ci].anchor, NSSize::new(CARD_W, CARD_H));
            let mut top: Option<(f64, TaskId)> = None;
            for id in ids {
                let Some(c) = cards.get(id) else { continue };
                bounds = union(bounds, c.view.frame());
                if c.contains(p) && top.is_none_or(|(z, _)| c.z > z) {
                    top = Some((c.z, *id));
                }
            }
            let bounds = inset(bounds, -14.0);
            if contains(bounds, p) {
                return (Some(ci), top.map(|(_, id)| id));
            }
        }
        (None, None)
    }

    fn on_card(self: &Rc<Self>, id: TaskId, msg: CardMsg) {
        let to_canvas = |p: NSPoint| self.canvas.convertPoint_fromView(p, None);
        match msg {
            CardMsg::Down { at, shift } => {
                *self.press.borrow_mut() = Some(Press {
                    id,
                    start: to_canvas(at),
                    shift,
                });
            }
            CardMsg::Dragged(at) => {
                let p = to_canvas(at);
                if self.drag.borrow().is_none() {
                    let Some(press) = self.press.borrow_mut().take() else {
                        return;
                    };
                    if (p.x - press.start.x).hypot(p.y - press.start.y) < DRAG_THRESHOLD {
                        *self.press.borrow_mut() = Some(press);
                        return;
                    }
                    self.begin_drag(press, p);
                }
                self.drag_to(p);
            }
            CardMsg::Up(at) => {
                if self.drag.borrow().is_some() {
                    self.drop_at(to_canvas(at));
                } else if self.press.borrow_mut().take().is_some() {
                    (self.sink)(UiEvent::ToggleExpand(id));
                }
            }
        }
    }

    fn begin_drag(self: &Rc<Self>, press: Press, p: NSPoint) {
        let Some(col) = self.column_of(press.id) else {
            return;
        };
        let mut ids = vec![press.id];
        if press.shift {
            ids.extend(
                self.order.borrow()[col]
                    .iter()
                    .copied()
                    .filter(|x| *x != press.id),
            );
        }
        let Some(body) = self.cards.borrow().get(&press.id).map(|c| c.body) else {
            return;
        };
        *self.drag.borrow_mut() = Some(Drag {
            ids,
            grab: (press.start.x - body.x, press.start.y - body.y),
            cursor: (p.x, p.y),
            target: Some(col),
        });
    }

    fn drag_to(self: &Rc<Self>, p: NSPoint) {
        let target = self
            .geom
            .borrow()
            .iter()
            .position(|g| p.x >= g.rect.origin.x && p.x < g.rect.origin.x + g.rect.size.width);
        let changed = {
            let mut drag = self.drag.borrow_mut();
            let Some(drag) = drag.as_mut() else { return };
            drag.cursor = (p.x, p.y);
            let changed = drag.target != target;
            drag.target = target;
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
        self.hover.set(None);
        self.hover_card.set(None);
        if let Some(col) = drag.target {
            // Reorder right away so the cards spring to their new stack before the
            // snapshot comes back.
            {
                let mut order = self.order.borrow_mut();
                for o in order.iter_mut() {
                    o.retain(|id| !drag.ids.contains(id));
                }
                for (i, id) in drag.ids.iter().enumerate() {
                    order[col].insert(i, *id);
                }
            }
            for (i, id) in drag.ids.iter().enumerate() {
                (self.sink)(UiEvent::Move {
                    id: *id,
                    status: self.statuses[col],
                    index: i,
                });
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
        self.face.set_fill(Fill::Card {
            hue: self.hue,
            tracking: snap.tracking,
        });
        self.badge_text = badge;
    }

    fn set_badge(&self, text: &str) {
        let show = !text.is_empty() && self.look != Look::Solid;
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

    /// Switches between the pastel face and frosted glass, cross-fading when `animated`.
    fn set_look(&mut self, look: Look, animated: bool) {
        if self.look == look {
            return;
        }
        let glass = look != Look::Solid;
        let lifted = matches!(look, Look::Lifted(_));
        if let Look::Lifted(text) = &look {
            self.label.setStringValue(&NSString::from_str(text));
        }
        let text_color = if glass {
            style::glass_text()
        } else {
            style::text(self.hue)
        };
        for l in [&self.dots, &self.time, &self.title, &self.badge] {
            l.setTextColor(Some(&text_color));
        }
        self.icon.setContentTintColor(Some(&text_color));
        self.time.setAlphaValue(0.75);
        let filters: Retained<NSArray<CIFilter>> = if lifted {
            blur_filter().map_or_else(NSArray::new, |f| NSArray::from_retained_slice(&[f]))
        } else {
            NSArray::new()
        };
        self.content.setContentFilters(&filters);

        let (face, glass_a, label) = (
            if glass { 0.0 } else { 1.0 },
            if glass { 1.0 } else { 0.0 },
            if lifted { 1.0 } else { 0.0 },
        );
        let content = if lifted { 0.45 } else { 1.0 };
        let views = (
            self.face.clone(),
            self.glass.clone(),
            self.frost.clone(),
            self.label.clone(),
            self.content.clone(),
        );
        animate(animated, motion::GLASS_SECS, move || {
            views.0.animator().setAlphaValue(face);
            views.1.animator().setAlphaValue(glass_a);
            views.2.animator().setAlphaValue(glass_a);
            views.3.animator().setAlphaValue(label);
            views.4.animator().setAlphaValue(content);
        });
        self.look = look;
    }

    fn apply_frame(&self) {
        let b = &self.body;
        self.view.setFrameCenterRotation(0.0);
        self.view.setFrame(NSRect::new(
            NSPoint::new(b.x, b.y),
            NSSize::new(CARD_W, CARD_H),
        ));
        self.view.setFrameCenterRotation(ROT_SIGN * b.r);
    }

    /// Point-in-card test that respects the card's rotation.
    fn contains(&self, p: NSPoint) -> bool {
        let b = &self.body;
        let (cx, cy) = (b.x + CARD_W / 2.0, b.y + CARD_H / 2.0);
        let t = (-b.r).to_radians();
        let (dx, dy) = (p.x - cx, p.y - cy);
        let (lx, ly) = (dx * t.cos() - dy * t.sin(), dx * t.sin() + dy * t.cos());
        lx.abs() <= CARD_W / 2.0 && ly.abs() <= CARD_H / 2.0
    }
}

fn text_label(mtm: MainThreadMarker, text: &str, size: f64, weight: f64) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    l
}

fn blur_filter() -> Option<Retained<CIFilter>> {
    let filter = unsafe { CIFilter::filterWithName(ns_string!("CIGaussianBlur")) }?;
    unsafe {
        filter.setDefaults();
        filter.setValue_forKey(
            Some(&NSNumber::new_f64(LIFT_BLUR)),
            ns_string!("inputRadius"),
        );
    }
    Some(filter)
}

fn union(a: NSRect, b: NSRect) -> NSRect {
    let x0 = a.origin.x.min(b.origin.x);
    let y0 = a.origin.y.min(b.origin.y);
    let x1 = (a.origin.x + a.size.width).max(b.origin.x + b.size.width);
    let y1 = (a.origin.y + a.size.height).max(b.origin.y + b.size.height);
    NSRect::new(NSPoint::new(x0, y0), NSSize::new(x1 - x0, y1 - y0))
}

fn inset(r: NSRect, d: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + d, r.origin.y + d),
        NSSize::new(r.size.width - 2.0 * d, r.size.height - 2.0 * d),
    )
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x
        && p.x <= r.origin.x + r.size.width
        && p.y >= r.origin.y
        && p.y <= r.origin.y + r.size.height
}
