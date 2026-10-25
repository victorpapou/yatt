//! The task modal. A click on a card morphs it into a fixed-size panel over a
//! dimmed board: title, stats, the markdown notes editor and the latest time
//! entries. Closing morphs it back into the card.
//!
//! Seamless: the panel starts as the card itself, on the same frame and angle, in
//! the same material and colour, showing the card's own text, and swaps places with
//! the card in one frame. Its frame and rotation ride one spring
//! ([`physics::MORPH`]); only once it has settled does the card's text hand over to
//! the panel's controls. Closing runs the same backwards and hands back to the card
//! when the panel lies exactly on it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;

use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSButton, NSColor, NSFont, NSFontWeightMedium,
    NSFontWeightRegular, NSImage, NSImageSymbolConfiguration, NSLineBreakMode, NSMenu, NSMenuItem,
    NSPasteboard, NSPasteboardTypeString, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer, ns_string};
use objc2_quartz_core::{CACornerMask, CACurrentMediaTime};
use yatt_core::{Status, TaskId};

use crate::board::{Face, card_face, set_meta, set_title, time_text};
use crate::classes::{ActionTarget, Fill, FillView, FlippedView};
use crate::editor::{EditorMsg, NotesEditor};
use crate::model::{CardSnap, UiEvent, format_duration};
use crate::motion;
use crate::physics::{self, Body};
use crate::picker::{Current, Picker};
use crate::style;
use crate::window::{Sink, animate};

/// The card's parts that change as it opens into the panel.
struct Parts {
    content: Retained<FlippedView>,
    card_box: Retained<FlippedView>,
    face: Retained<FillView>,
    sheet: Retained<FillView>,
}

/// How open the panel looks at `openness` (0 = the card, 1 = the panel), for a
/// panel `size` big. It is the card all the way: its coloured face stays on top
/// and eases from the card's height to the header's, and below it the same card
/// shows its white body, growing downward as the card grows (none of it at card
/// size). The title, the editor and the rest stay put at the card's top left and
/// show as it grows round them; the card's own text gives way over the first
/// half, the panel's controls come in over the second, so nothing pops.
fn blend(parts: &Parts, openness: f64, size: NSSize) {
    let ease = |a: f64, b: f64| {
        let t = ((openness - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    parts.card_box.setAlphaValue(1.0 - ease(0.15, 0.55));
    parts.content.setAlphaValue(ease(0.45, 0.95));
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let head = (style::CARD_H + (HEADER_H - style::CARD_H) * ease(0.1, 0.7)).min(h);
    motion::quietly(|| {
        parts
            .face
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(w, head)));
        parts.sheet.setFrame(NSRect::new(
            NSPoint::new(0.0, head),
            NSSize::new(w, h - head),
        ));
        // The face rounds only its top corners once the body shows under it.
        if let Some(layer) = parts.face.layer() {
            layer.setMaskedCorners(if h - head > 0.5 {
                CACornerMask::LayerMinXMinYCorner | CACornerMask::LayerMaxXMinYCorner
            } else {
                CACornerMask::LayerMinXMinYCorner
                    | CACornerMask::LayerMaxXMinYCorner
                    | CACornerMask::LayerMinXMaxYCorner
                    | CACornerMask::LayerMaxXMaxYCorner
            });
        }
    });
}

/// The panel's header: title and stats, in the card's colour.
const HEADER_H: f64 = 80.0;

pub struct Modal {
    pub dim: Retained<FillView>,
    pub panel: Retained<FillView>,
    /// The card's material, so the panel is the card grown big: blur and tint.
    face: Retained<FillView>,
    /// White over the card's tint below the header, once the panel stands open:
    /// only the header keeps the card's colour.
    sheet: Retained<FillView>,
    /// The card's own text, shown while the panel is (becoming) the card.
    card_box: Retained<FlippedView>,
    card: RefCell<Option<Face>>,
    /// The panel's controls, shown once it has settled open.
    content: Retained<FlippedView>,
    title: Retained<NSTextField>,
    stats: Retained<NSTextField>,
    rules: [Retained<FillView>; 2],
    editor: NotesEditor,
    /// The time entries' one line, and every entry, as written.
    times: Retained<NSButton>,
    all_entries: Rc<RefCell<Vec<String>>>,
    look: Retained<NSButton>,
    picker: Rc<Picker>,
    /// The card's own edge round the panel (outline and inner glow).
    edges: style::Edges,
    /// The task on screen, shared with the callbacks.
    open: Rc<Cell<Option<TaskId>>>,
    /// Last title known to match the store, to tell edits from echoes.
    title_known: Rc<RefCell<String>>,
    /// Notes known to match the store, per task: a late flush from the editor may
    /// belong to a task that is no longer open.
    notes_known: Rc<RefCell<HashMap<TaskId, String>>>,
    /// The editor's keys for the tasks it has shown.
    keys: Rc<RefCell<HashMap<String, TaskId>>>,
    morph: Rc<Morph>,
    _retained: Vec<Retained<AnyObject>>,
}

/// Springs the panel's frame towards a target rect: origin (with the rotation, so a
/// card in a fan unturns as it grows and turns back as it lands) and size each
/// ride a body.
/// Told how far along a morph is (0 to 1) and the panel's frame, unturned.
type ProgressFn = Box<dyn Fn(f64, NSRect)>;

struct Morph {
    panel: Retained<FillView>,
    origin: Cell<Body>,
    size: Cell<Body>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<f64>,
    done: RefCell<Option<Box<dyn Fn()>>>,
    /// Told each frame how far along the current morph is, 0 at its start and 1
    /// at its end, measured on the width.
    on_progress: RefCell<Option<ProgressFn>>,
    from_w: Cell<f64>,
    to_w: Cell<f64>,
}

impl Morph {
    fn jump(&self, r: NSRect, deg: f64) {
        let mut o = Body::at(r.origin.x, r.origin.y);
        o.r = deg;
        o.tr = deg;
        let mut s = Body::at(r.size.width, r.size.height);
        o.spring = physics::MORPH;
        s.spring = physics::MORPH;
        // The panel eases to its angle; only cards hang and swing.
        o.swings = false;
        s.swings = false;
        self.origin.set(o);
        self.size.set(s);
        self.place(o, s);
    }

    fn place(&self, o: Body, s: Body) {
        self.panel.setFrameCenterRotation(0.0);
        self.panel.setFrame(NSRect::new(
            NSPoint::new(o.x, o.y),
            NSSize::new(s.x.max(1.0), s.y.max(1.0)),
        ));
        self.panel.setFrameCenterRotation(o.r);
    }

    /// Springs to `r`, then runs `done` (dropping any earlier one unrun).
    fn to(self: &Rc<Self>, r: NSRect, deg: f64, done: Option<Box<dyn Fn()>>) {
        let (mut o, mut s) = (self.origin.get(), self.size.get());
        // Growing: a little jelly; shrinking back: lighter.
        let spring = if r.size.width >= s.x {
            physics::MORPH
        } else {
            physics::MORPH_BACK
        };
        o.spring = spring;
        s.spring = spring;
        self.from_w.set(s.x);
        self.to_w.set(r.size.width);
        o.target(r.origin.x, r.origin.y, deg);
        s.target(r.size.width, r.size.height, 0.0);
        self.origin.set(o);
        self.size.set(s);
        *self.done.borrow_mut() = done;
        if motion::reduce_motion() {
            self.jump(r, deg);
            if let Some(f) = self.on_progress.borrow().as_ref() {
                f(1.0, r);
            }
            self.finish();
            return;
        }
        if self.timer.borrow().is_some() {
            return;
        }
        self.last_tick.set(CACurrentMediaTime());
        let weak = Rc::downgrade(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(m) = weak.upgrade() {
                m.tick();
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
        let (mut o, mut s) = (self.origin.get(), self.size.get());
        o.step(dt);
        s.step(dt);
        let rest = o.at_rest() && s.at_rest();
        if rest {
            o.settle();
            s.settle();
        }
        self.origin.set(o);
        self.size.set(s);
        self.place(o, s);
        if let Some(f) = self.on_progress.borrow().as_ref() {
            let span = self.to_w.get() - self.from_w.get();
            let p = if span.abs() < 1.0 {
                1.0
            } else {
                ((s.x - self.from_w.get()) / span).clamp(0.0, 1.0)
            };
            let frame = NSRect::new(NSPoint::new(o.x, o.y), NSSize::new(s.x, s.y));
            f(if rest { 1.0 } else { p }, frame);
        }
        if rest {
            if let Some(t) = self.timer.borrow_mut().take() {
                t.invalidate();
            }
            self.finish();
        }
    }

    fn finish(&self) {
        let done = self.done.borrow_mut().take();
        if let Some(f) = done {
            f();
        }
    }
}

impl Modal {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Self {
        let open: Rc<Cell<Option<TaskId>>> = Rc::new(Cell::new(None));
        let title_known = Rc::new(RefCell::new(String::new()));
        let notes_known: Rc<RefCell<HashMap<TaskId, String>>> = Rc::default();
        let keys: Rc<RefCell<HashMap<String, TaskId>>> = Rc::default();
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();

        let dim = FillView::new(mtm, Fill::Dim);
        dim.setHidden(true);
        {
            let (sink, open) = (sink.clone(), open.clone());
            *dim.ivars().on_click.borrow_mut() = Some(Box::new(move || {
                if let Some(id) = open.get() {
                    sink(UiEvent::ToggleExpand(id));
                }
            }));
        }

        let panel = FillView::new(mtm, Fill::Panel);
        panel.setHidden(true);
        // Clicks on the panel stay on it instead of reaching the backdrop.
        *panel.ivars().on_click.borrow_mut() = Some(Box::new(|| {}));
        let fill_mask = objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable;
        let glass = style::card_blur(mtm);
        let face = FillView::new(
            mtm,
            Fill::Card {
                hue: 0,
                tracking: false,
                selected: false,
            },
        );
        let content = FlippedView::new(mtm, NSRect::ZERO);
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setMasksToBounds(true);
            style::round(&layer, style::CARD_RADIUS);
        }
        // The card's own edge, so the panel is the card grown big.
        let edges = style::Edges::new(mtm);
        let size = panel.frame().size;
        edges.follow(size.width, size.height);
        panel.addSubview(&edges.outer);
        glass.setAutoresizingMask(fill_mask);
        panel.addSubview(&glass);
        // The card's coloured face on top and its white body below; `blend`
        // sizes both as the card opens.
        let sheet = FillView::new(mtm, Fill::Sheet);
        panel.addSubview(&sheet);
        panel.addSubview(&face);
        panel.addSubview(&edges.inner);
        content.setAutoresizingMask(fill_mask);
        panel.addSubview(&content);
        // Pinned to the top-left at card size: when the panel is card-sized it
        // lies exactly where the card's own text does.
        let card_box = FlippedView::new(
            mtm,
            NSRect::new(NSPoint::ZERO, NSSize::new(style::CARD_W, style::CARD_H)),
        );
        panel.addSubview(&card_box);

        let title = NSTextField::textFieldWithString(ns_string!(""), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(20.0, unsafe {
            NSFontWeightMedium
        })));
        title.setBordered(false);
        title.setBezeled(false);
        title.setDrawsBackground(false);
        title.setFocusRingType(objc2_app_kit::NSFocusRingType::None);
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        if let Some(cell) = title.cell() {
            cell.setSendsActionOnEndEditing(true);
        }
        {
            let (sink, open, known) = (sink.clone(), open.clone(), title_known.clone());
            let field = ObjcWeak::from_retained(&title);
            let target = ActionTarget::new(mtm, move || {
                let (Some(field), Some(id)) = (field.load(), open.get()) else {
                    return;
                };
                let text = field.stringValue().to_string().trim().to_string();
                if !text.is_empty() && text != *known.borrow() {
                    *known.borrow_mut() = text.clone();
                    sink(UiEvent::Rename(id, text));
                }
            });
            unsafe {
                title.setTarget(Some(&target));
                title.setAction(Some(ActionTarget::sel()));
            }
            retained.push(Retained::into_super(Retained::into_super(target)));
        }

        let stats = small_label(mtm);

        let editor = {
            let (sink, open, known) = (sink.clone(), open.clone(), notes_known.clone());
            let keys = keys.clone();
            NotesEditor::new(mtm, move |msg| match msg {
                EditorMsg::Notes { key, md } => {
                    // Filed under the task the notes were typed into, open or not.
                    let Some(id) = keys.borrow().get(&key).copied() else {
                        return;
                    };
                    if known.borrow().get(&id) != Some(&md) {
                        known.borrow_mut().insert(id, md.clone());
                        sink(UiEvent::SetNotes(id, md));
                    }
                }
                EditorMsg::Close => {
                    if let Some(id) = open.get() {
                        sink(UiEvent::ToggleExpand(id));
                    }
                }
            })
        };

        // The look and status, in a popover off a small button in the header.
        let picker = Rc::new(Picker::new(mtm, sink));
        let look = {
            let picker = picker.clone();
            let slot: Rc<RefCell<Option<Retained<NSButton>>>> = Rc::default();
            let anchor = slot.clone();
            let target = ActionTarget::new(mtm, move || {
                if let Some(b) = anchor.borrow().as_ref() {
                    picker.open(b);
                }
            });
            // The toolbar's icon size and weight, crisp at any scale.
            let config =
                NSImageSymbolConfiguration::configurationWithPointSize_weight(15.0, unsafe {
                    NSFontWeightMedium
                });
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("paintpalette"),
                Some(ns_string!("Цвет, иконка, статус")),
            )
            .and_then(|i| i.imageWithSymbolConfiguration(&config))
            .expect("symbol");
            let b = unsafe {
                NSButton::buttonWithImage_target_action(
                    &image,
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setBordered(false);
            b.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
            b.setImageScaling(objc2_app_kit::NSImageScaling::ScaleNone);
            b.setToolTip(Some(ns_string!("Цвет, иконка, статус")));
            *slot.borrow_mut() = Some(b.clone());
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };
        // The time entries: one quiet line; a click lists them to copy, one or
        // all, exactly as written. A menu, so it scrolls however many there are.
        let all_entries: Rc<RefCell<Vec<String>>> = Rc::default();
        let times = {
            let entries = all_entries.clone();
            let slot: Rc<RefCell<Option<Retained<NSButton>>>> = Rc::default();
            let anchor = slot.clone();
            let target = ActionTarget::new(mtm, move || {
                let list = entries.borrow().clone();
                if let Some(b) = anchor.borrow().as_ref() {
                    copy_menu(mtm, b, &list);
                }
            });
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    ns_string!(""),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setBordered(false);
            b.setAlignment(objc2_app_kit::NSTextAlignment::Left);
            b.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                12.0,
                unsafe { NSFontWeightRegular },
            )));
            b.setToolTip(Some(ns_string!("Скопировать время")));
            *slot.borrow_mut() = Some(b.clone());
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };

        let rules = [
            FillView::new(mtm, Fill::Separator),
            FillView::new(mtm, Fill::Separator),
        ];
        let views: [&NSView; 7] = [
            &title,
            &stats,
            &look,
            &rules[0],
            &editor.view,
            &rules[1],
            &times,
        ];
        for v in views {
            content.addSubview(v);
        }

        let morph = Rc::new(Morph {
            panel: panel.clone(),
            origin: Cell::new(Body::at(0.0, 0.0)),
            size: Cell::new(Body::at(0.0, 0.0)),
            timer: RefCell::new(None),
            last_tick: Cell::new(0.0),
            done: RefCell::new(None),
            on_progress: RefCell::new(None),
            from_w: Cell::new(0.0),
            to_w: Cell::new(0.0),
        });

        Self {
            dim,
            panel,
            face,
            sheet,
            card_box,
            card: RefCell::new(None),
            morph,
            keys,
            content,
            title,
            stats,
            rules,
            editor,
            times,
            all_entries,
            look,
            picker,
            edges,
            open,
            title_known,
            notes_known,
            _retained: retained,
        }
    }

    fn parts(&self) -> Parts {
        Parts {
            content: self.content.clone(),
            card_box: self.card_box.clone(),
            face: self.face.clone(),
            sheet: self.sheet.clone(),
        }
    }

    /// Undo or redo in the notes editor.
    pub fn editor_undo(&self, redo: bool) {
        self.editor.undo(redo);
    }

    pub fn debug_print_editor(&self) {
        self.editor.debug_print_html();
    }

    pub fn open_task(&self) -> Option<TaskId> {
        self.open.get()
    }

    /// Where the panel sits when open, centred in `bounds`.
    pub fn target_frame(bounds: NSRect) -> NSRect {
        let w = motion::MODAL_SIZE.0.min(bounds.size.width - 40.0);
        let h = motion::MODAL_SIZE.1.min(bounds.size.height - 60.0);
        NSRect::new(
            NSPoint::new(
                ((bounds.size.width - w) / 2.0).round(),
                ((bounds.size.height - h) / 2.0).round(),
            ),
            NSSize::new(w, h),
        )
    }

    /// Lays out the content for the panel's open size.
    pub fn layout(&self, bounds: NSRect) {
        self.dim.setFrame(bounds);
        let frame = Self::target_frame(bounds);
        if self.open.get().is_some() {
            self.morph.jump(frame, 0.0);
        }
        let (w, h) = (frame.size.width, frame.size.height);
        let pad = 24.0;
        let iw = w - 2.0 * pad;
        self.title.setFrame(NSRect::new(
            NSPoint::new(pad - 2.0, 20.0),
            NSSize::new(iw + 4.0 - 40.0, 28.0),
        ));
        self.look.setFrame(NSRect::new(
            NSPoint::new(w - pad - 28.0, 20.0),
            NSSize::new(28.0, 28.0),
        ));
        self.stats
            .setFrame(NSRect::new(NSPoint::new(pad, 52.0), NSSize::new(iw, 16.0)));
        self.rules[0].setFrame(NSRect::new(NSPoint::new(0.0, 80.0), NSSize::new(w, 1.0)));
        if self.open.get().is_some() {
            blend(&self.parts(), 1.0, NSSize::new(w, h));
        }
        let bottom_h = 44.0;
        self.editor.view.setFrame(NSRect::new(
            NSPoint::new(pad - 4.0, 92.0),
            NSSize::new(iw + 8.0, h - 92.0 - bottom_h - 8.0),
        ));
        self.rules[1].setFrame(NSRect::new(
            NSPoint::new(0.0, h - bottom_h),
            NSSize::new(w, 1.0),
        ));
        self.times.setFrame(NSRect::new(
            NSPoint::new(pad - 2.0, h - bottom_h + 12.0),
            NSSize::new(iw, 20.0),
        ));
    }

    /// Morphs from `from` (the card, in the dim view's coordinates) to the centred panel.
    /// `deg` is the card's rotation (in a fan); the panel turns upright as it grows.
    pub fn open(&self, snap: &CardSnap, status: Status, from: NSRect, deg: f64, bounds: NSRect) {
        self.open.set(Some(snap.id));
        *self.title_known.borrow_mut() = snap.title.clone();
        let key = snap.id.to_string();
        self.keys.borrow_mut().insert(key.clone(), snap.id);
        self.notes_known
            .borrow_mut()
            .insert(snap.id, snap.notes.clone());
        self.title.setStringValue(&NSString::from_str(&snap.title));
        self.editor.set_markdown(&snap.notes, &key);
        self.update(snap, status);
        self.show_card(snap);

        // Lay the controls out at full size; they wait, hidden, until the panel has
        // grown. The panel starts as the card: same frame, angle, colour and text.
        let target = Self::target_frame(bounds);
        self.panel.setFrame(target);
        self.content
            .setFrame(NSRect::new(NSPoint::ZERO, target.size));
        self.layout(bounds);
        self.content.setAlphaValue(0.0);
        self.card_box.setAlphaValue(1.0);
        self.panel.setAlphaValue(1.0);
        self.dim.setAlphaValue(0.0);
        motion::quietly(|| {
            self.morph.jump(from, deg);
            self.dim.setHidden(false);
            self.panel.setHidden(false);
        });
        let dim = self.dim.clone();
        animate(true, motion::MODAL_OPEN_SECS, move || {
            dim.animator().setAlphaValue(1.0)
        });
        // Grown and still: the card's text hands over to the panel's controls.
        // One element all the way: as the panel grows, the card's text fades out
        // over the first half and the panel's controls and white body fade in
        // over the second, driven by the morph itself.
        self.content.setBoundsOrigin(NSPoint::ZERO);
        let parts = self.parts();
        blend(&parts, 0.0, from.size);
        *self.morph.on_progress.borrow_mut() = Some(Box::new(move |p, frame| {
            blend(&parts, p, frame.size);
        }));
        self.morph.to(target, 0.0, None);
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(Some(&self.editor.view));
        }
        self.editor.focus();
    }

    /// Puts the card's own text (as built for the board) in the panel's corner.
    fn show_card(&self, snap: &CardSnap) {
        if let Some(old) = self.card.borrow_mut().take() {
            old.content.removeFromSuperview();
        }
        let mtm = MainThreadMarker::new().expect("main thread");
        let face = card_face(mtm, snap.id, &snap.title, style::hue_of(snap.id));
        set_meta(&face.time, &time_text(snap));
        self.card_box.addSubview(&face.content);
        *self.card.borrow_mut() = Some(face);
    }

    /// Morphs back into `to` (the card's frame), or fades out when the card is gone.
    /// Lands on `to` turned by `deg`, the card's place and angle.
    pub fn close(&self, to: Option<NSRect>, deg: f64, done: impl Fn() + 'static) {
        self.editor.flush();
        self.picker.close();
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(None);
        }
        self.open.set(None);
        // The controls hand back to the card's text first, then the panel shrinks
        // with that text on it all the way.
        let (panel, dim) = (self.panel.clone(), self.dim.clone());
        let gone = to.is_none();
        // The same blend backwards: the card closes round its contents.
        let parts = self.parts();
        *self.morph.on_progress.borrow_mut() = Some(Box::new(move |p, frame| {
            blend(&parts, 1.0 - p, frame.size);
        }));
        animate(true, motion::MODAL_CLOSE_SECS, move || {
            if gone {
                panel.animator().setAlphaValue(0.0);
            }
            dim.animator().setAlphaValue(0.0);
        });
        let (panel, dim) = (self.panel.clone(), self.dim.clone());
        let to = to.unwrap_or_else(|| self.panel.frame());
        self.morph.to(
            to,
            deg,
            // On the card's exact pixels: the card takes over in the same frame.
            Some(Box::new(move || {
                motion::quietly(|| {
                    panel.setHidden(true);
                    panel.setAlphaValue(1.0);
                    dim.setHidden(true);
                    done();
                });
            })),
        );
    }

    /// The window turned light or dark: the panel's material in the new colours
    /// (the next `update` sets the ink).
    pub fn restyle(&self) {
        self.face.repaint();
        self.sheet.repaint();
        self.edges.restyle();
    }

    /// Refreshes what the open task shows, without touching what is being typed.
    pub fn update(&self, snap: &CardSnap, status: Status) {
        if self.title.currentEditor().is_none() && *self.title_known.borrow() != snap.title {
            self.title.setStringValue(&NSString::from_str(&snap.title));
        }
        *self.title_known.borrow_mut() = snap.title.clone();
        // Notes changed elsewhere (undo): reload the editor.
        if self.notes_known.borrow().get(&snap.id) != Some(&snap.notes) {
            self.notes_known
                .borrow_mut()
                .insert(snap.id, snap.notes.clone());
            self.keys.borrow_mut().insert(snap.id.to_string(), snap.id);
            self.editor.set_markdown(&snap.notes, &snap.id.to_string());
        }
        if let Some(face) = self.card.borrow().as_ref() {
            set_title(&face.title, &snap.title);
            set_meta(&face.time, &time_text(snap));
        }
        let stats = format!(
            "{}{}  ·  сегодня {}  ·  🍅 {}",
            if snap.tracking { "● " } else { "" },
            format_duration(snap.total_ms),
            format_duration(snap.today_ms),
            snap.pomodoros
        );
        self.stats.setStringValue(&NSString::from_str(&stats));
        // The card's own colours, so nothing changes as it grows into the panel.
        let hue = style::hue_of(snap.id);
        self.face.set_fill(Fill::Card {
            hue,
            tracking: snap.tracking,
            selected: false,
        });
        let ink = style::text(hue);
        self.title.setTextColor(Some(&ink));
        self.stats
            .setTextColor(Some(&ink.colorWithAlphaComponent(0.75)));
        self.look
            .setContentTintColor(Some(&ink.colorWithAlphaComponent(0.75)));
        self.picker.show_task(Current {
            id: Some(snap.id),
            color: snap.color,
            icon: snap.icon.clone(),
            status: Some(status),
        });
        self.editor.set_ink(&style::text_css(hue));
        let line = match snap.entries.len() {
            0 => "Записей времени пока нет".to_string(),
            n => format!(
                "{}  ·  {}  ⌄",
                snap.entries[0],
                if n == 1 {
                    "1 запись".to_string()
                } else {
                    format!("{n} {}", plural(n, "запись", "записи", "записей"))
                }
            ),
        };
        self.times.setTitle(&NSString::from_str(&line));
        self.times
            .setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        *self.all_entries.borrow_mut() = snap.entries.clone();
    }
}

fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match (n % 10, n % 100) {
        (1, x) if x != 11 => one,
        (2..=4, x) if !(12..=14).contains(&x) => few,
        _ => many,
    }
}

/// The time entries to copy, under `anchor`: all of them, or any one, each
/// exactly as written.
fn copy_menu(mtm: MainThreadMarker, anchor: &NSButton, entries: &[String]) {
    if entries.is_empty() {
        return;
    }
    let menu = NSMenu::new(mtm);
    let mut targets = Vec::new();
    let mut item = |title: &str, text: String| {
        let t = ActionTarget::new(mtm, move || {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            unsafe { pb.setString_forType(&NSString::from_str(&text), NSPasteboardTypeString) };
        });
        let it = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(ActionTarget::sel()),
                ns_string!(""),
            )
        };
        unsafe { it.setTarget(Some(&t)) };
        menu.addItem(&it);
        targets.push(t);
    };
    item("Скопировать всё", entries.join("\n"));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    for e in entries {
        item(e, e.clone());
    }
    let at = NSPoint::new(0.0, anchor.bounds().size.height + 4.0);
    menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(anchor));
    drop(targets);
}

fn small_label(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(ns_string!(""), mtm);
    l.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
        12.0,
        unsafe { NSFontWeightRegular },
    )));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l
}
