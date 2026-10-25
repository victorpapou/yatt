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
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSButton, NSColor, NSControlSize, NSFont, NSFontWeightMedium,
    NSFontWeightRegular, NSLineBreakMode, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer, ns_string};
use objc2_quartz_core::CACurrentMediaTime;
use yatt_core::{Status, TaskId};

use crate::board::{Face, card_face, set_meta, set_title, time_text};
use crate::classes::{ActionTarget, Fill, FillView, FlippedView};
use crate::editor::{EditorMsg, NotesEditor};
use crate::model::{CardSnap, UiEvent, format_duration};
use crate::motion;
use crate::physics::{self, Body};
use crate::style;
use crate::window::{Sink, animate};

const ENTRY_LINES: usize = 4;

pub struct Modal {
    pub dim: Retained<FillView>,
    pub panel: Retained<FillView>,
    /// The card's material, so the panel is the card grown big: blur and tint.
    face: Retained<FillView>,
    /// The card's own text, shown while the panel is (becoming) the card.
    card_box: Retained<FlippedView>,
    card: RefCell<Option<Face>>,
    /// The panel's controls, shown once it has settled open.
    content: Retained<FlippedView>,
    title: Retained<NSTextField>,
    stats: Retained<NSTextField>,
    rules: [Retained<FillView>; 2],
    editor: NotesEditor,
    entries: Retained<NSTextField>,
    done: Retained<NSButton>,
    delete: Retained<NSButton>,
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
struct Morph {
    panel: Retained<FillView>,
    origin: Cell<Body>,
    size: Cell<Body>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<f64>,
    done: RefCell<Option<Box<dyn Fn()>>>,
}

impl Morph {
    fn jump(&self, r: NSRect, deg: f64) {
        let mut o = Body::at(r.origin.x, r.origin.y);
        o.r = deg;
        o.tr = deg;
        let mut s = Body::at(r.size.width, r.size.height);
        o.spring = physics::MORPH;
        s.spring = physics::MORPH;
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
        o.target(r.origin.x, r.origin.y, deg);
        s.target(r.size.width, r.size.height, 0.0);
        self.origin.set(o);
        self.size.set(s);
        *self.done.borrow_mut() = done;
        if motion::reduce_motion() {
            self.jump(r, deg);
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
        for v in [&*glass as &NSView, &face] {
            v.setAutoresizingMask(fill_mask);
            panel.addSubview(v);
        }
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
        let entries = small_label(mtm);
        entries.setMaximumNumberOfLines(ENTRY_LINES as isize);

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

        let mut button = |label: &str, events: Vec<Box<dyn Fn(TaskId) -> UiEvent>>| {
            let (sink, open) = (sink.clone(), open.clone());
            let target = ActionTarget::new(mtm, move || {
                if let Some(id) = open.get() {
                    for e in &events {
                        sink(e(id));
                    }
                }
            });
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(label),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setControlSize(NSControlSize::Regular);
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };
        let done = button(
            "Готово",
            vec![
                Box::new(UiEvent::ToggleExpand),
                Box::new(|id| UiEvent::Move {
                    id,
                    status: Status::Done,
                    index: 0,
                }),
            ],
        );
        let delete = button("Удалить", vec![Box::new(UiEvent::Delete)]);

        let rules = [
            FillView::new(mtm, Fill::Separator),
            FillView::new(mtm, Fill::Separator),
        ];
        let views: [&NSView; 8] = [
            &title,
            &stats,
            &rules[0],
            &editor.view,
            &rules[1],
            &entries,
            &done,
            &delete,
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
        });

        Self {
            dim,
            panel,
            face,
            card_box,
            card: RefCell::new(None),
            morph,
            keys,
            content,
            title,
            stats,
            rules,
            editor,
            entries,
            done,
            delete,
            open,
            title_known,
            notes_known,
            _retained: retained,
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
            NSSize::new(iw + 4.0, 28.0),
        ));
        self.stats
            .setFrame(NSRect::new(NSPoint::new(pad, 52.0), NSSize::new(iw, 16.0)));
        self.rules[0].setFrame(NSRect::new(NSPoint::new(0.0, 80.0), NSSize::new(w, 1.0)));
        let bottom_h = 16.0 * ENTRY_LINES as f64 + 64.0;
        self.editor.view.setFrame(NSRect::new(
            NSPoint::new(pad - 4.0, 92.0),
            NSSize::new(iw + 8.0, h - 92.0 - bottom_h - 8.0),
        ));
        self.rules[1].setFrame(NSRect::new(
            NSPoint::new(0.0, h - bottom_h),
            NSSize::new(w, 1.0),
        ));
        self.entries.setFrame(NSRect::new(
            NSPoint::new(pad, h - bottom_h + 12.0),
            NSSize::new(iw, 16.0 * ENTRY_LINES as f64),
        ));
        let y = h - 44.0;
        let dw = self.done.fittingSize().width.max(84.0);
        let delw = self.delete.fittingSize().width.max(84.0);
        self.delete.setFrame(NSRect::new(
            NSPoint::new(w - pad - delw, y),
            NSSize::new(delw, 28.0),
        ));
        self.done.setFrame(NSRect::new(
            NSPoint::new(w - pad - delw - 8.0 - dw, y),
            NSSize::new(dw, 28.0),
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
        let (content, card_box) = (self.content.clone(), self.card_box.clone());
        self.morph.to(
            target,
            0.0,
            Some(Box::new(move || {
                let (content, card_box) = (content.clone(), card_box.clone());
                animate(true, motion::SOFTEN_SECS, move || {
                    content.animator().setAlphaValue(1.0);
                    card_box.animator().setAlphaValue(0.0);
                });
            })),
        );
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
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(None);
        }
        self.open.set(None);
        // The controls hand back to the card's text first, then the panel shrinks
        // with that text on it all the way.
        let (panel, content, dim, card_box) = (
            self.panel.clone(),
            self.content.clone(),
            self.dim.clone(),
            self.card_box.clone(),
        );
        let gone = to.is_none();
        animate(true, motion::SOFTEN_SECS, move || {
            content.animator().setAlphaValue(0.0);
            card_box.animator().setAlphaValue(1.0);
        });
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
        self.entries
            .setTextColor(Some(&ink.colorWithAlphaComponent(0.75)));
        self.editor.set_ink(&style::text_css(hue));
        let entries = if snap.entries.is_empty() {
            "Записей времени пока нет".to_string()
        } else {
            snap.entries
                .iter()
                .take(ENTRY_LINES)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        };
        self.entries.setStringValue(&NSString::from_str(&entries));
        self.done.setHidden(status == Status::Done);
    }
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
