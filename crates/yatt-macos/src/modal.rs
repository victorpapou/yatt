//! The task modal. A click on a card morphs it into a fixed-size panel over a
//! dimmed board: title, stats, the markdown notes editor and the latest time
//! entries. Closing morphs it back into the card. The panel's frame rides a
//! spring ([`physics::MORPH`]), so the morph lands with a little give.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;

use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSButton, NSColor, NSControlSize, NSFont, NSFontWeightRegular,
    NSFontWeightSemibold, NSLineBreakMode, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer, ns_string};
use objc2_quartz_core::CACurrentMediaTime;
use yatt_core::{Status, TaskId};

use crate::classes::{ActionTarget, Fill, FillView, FlippedView};
use crate::editor::{EditorMsg, NotesEditor};
use crate::model::{CardSnap, UiEvent, format_duration};
use crate::motion;
use crate::physics::{self, Body};
use crate::window::{Sink, animate};

const ENTRY_LINES: usize = 4;

pub struct Modal {
    pub dim: Retained<FillView>,
    pub panel: Retained<FillView>,
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
    /// Last title and notes known to match the store, to tell edits from echoes.
    title_known: Rc<RefCell<String>>,
    notes_known: Rc<RefCell<String>>,
    morph: Rc<Morph>,
    _retained: Vec<Retained<AnyObject>>,
}

/// Springs the panel's frame towards a target rect: origin and size each ride a body.
struct Morph {
    panel: Retained<FillView>,
    origin: Cell<Body>,
    size: Cell<Body>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    last_tick: Cell<f64>,
    done: RefCell<Option<Box<dyn Fn()>>>,
}

impl Morph {
    fn jump(&self, r: NSRect) {
        let mut o = Body::at(r.origin.x, r.origin.y);
        let mut s = Body::at(r.size.width, r.size.height);
        o.spring = physics::MORPH;
        s.spring = physics::MORPH;
        self.origin.set(o);
        self.size.set(s);
        self.panel.setFrame(r);
    }

    /// Springs to `r`, then runs `done` (dropping any earlier one unrun).
    fn to(self: &Rc<Self>, r: NSRect, done: Option<Box<dyn Fn()>>) {
        let (mut o, mut s) = (self.origin.get(), self.size.get());
        o.target(r.origin.x, r.origin.y, 0.0);
        s.target(r.size.width, r.size.height, 0.0);
        self.origin.set(o);
        self.size.set(s);
        *self.done.borrow_mut() = done;
        if motion::reduce_motion() {
            self.jump(r);
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
        self.panel.setFrame(NSRect::new(
            NSPoint::new(o.x, o.y),
            NSSize::new(s.x.max(1.0), s.y.max(1.0)),
        ));
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
        let notes_known = Rc::new(RefCell::new(String::new()));
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
        let content = FlippedView::new(mtm, NSRect::ZERO);
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setMasksToBounds(true);
            layer.setCornerRadius(16.0);
        }
        content.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        panel.addSubview(&content);

        let title = NSTextField::textFieldWithString(ns_string!(""), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(20.0, unsafe {
            NSFontWeightSemibold
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
            NotesEditor::new(mtm, move |msg| {
                let Some(id) = open.get() else { return };
                match msg {
                    EditorMsg::Notes(md) => {
                        if md != *known.borrow() {
                            *known.borrow_mut() = md.clone();
                            sink(UiEvent::SetNotes(id, md));
                        }
                    }
                    EditorMsg::Close => sink(UiEvent::ToggleExpand(id)),
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
            morph,
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
            self.morph.jump(frame);
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
    pub fn open(&self, snap: &CardSnap, status: Status, from: NSRect, bounds: NSRect) {
        self.open.set(Some(snap.id));
        *self.title_known.borrow_mut() = snap.title.clone();
        *self.notes_known.borrow_mut() = snap.notes.clone();
        self.title.setStringValue(&NSString::from_str(&snap.title));
        self.editor.set_markdown(&snap.notes);
        self.update(snap, status);

        // Lay the content out at full size, then shrink the panel onto the card: the
        // content keeps its layout and is revealed as the panel grows.
        let target = Self::target_frame(bounds);
        self.panel.setFrame(target);
        self.content
            .setFrame(NSRect::new(NSPoint::ZERO, target.size));
        self.layout(bounds);
        self.morph.jump(from);
        self.panel.setAlphaValue(1.0);
        self.content.setAlphaValue(0.0);
        self.dim.setAlphaValue(0.0);
        self.dim.setHidden(false);
        self.panel.setHidden(false);
        self.morph.to(target, None);
        let (content, dim) = (self.content.clone(), self.dim.clone());
        animate(true, motion::MODAL_OPEN_SECS, move || {
            content.animator().setAlphaValue(1.0);
            dim.animator().setAlphaValue(1.0);
        });
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(Some(&self.editor.view));
        }
        self.editor.focus();
    }

    /// Morphs back into `to` (the card's frame), or fades out when the card is gone.
    pub fn close(&self, to: Option<NSRect>, done: impl Fn() + 'static) {
        self.editor.flush();
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(None);
        }
        self.open.set(None);
        let (panel, content, dim) = (self.panel.clone(), self.content.clone(), self.dim.clone());
        let gone = to.is_none();
        animate(true, motion::MODAL_CLOSE_SECS, move || {
            if gone {
                panel.animator().setAlphaValue(0.0);
            }
            content.animator().setAlphaValue(0.0);
            dim.animator().setAlphaValue(0.0);
        });
        let (panel, dim) = (self.panel.clone(), self.dim.clone());
        let to = to.unwrap_or_else(|| self.panel.frame());
        self.morph.to(
            to,
            Some(Box::new(move || {
                panel.setHidden(true);
                panel.setAlphaValue(1.0);
                dim.setHidden(true);
                done();
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
        if *self.notes_known.borrow() != snap.notes {
            *self.notes_known.borrow_mut() = snap.notes.clone();
            self.editor.set_markdown(&snap.notes);
        }
        let stats = format!(
            "{}{}  ·  сегодня {}  ·  🍅 {}",
            if snap.tracking { "● " } else { "" },
            format_duration(snap.total_ms),
            format_duration(snap.today_ms),
            snap.pomodoros
        );
        self.stats.setStringValue(&NSString::from_str(&stats));
        let color = if snap.tracking {
            NSColor::controlAccentColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.stats.setTextColor(Some(&color));
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
