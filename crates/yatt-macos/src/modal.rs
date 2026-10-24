//! The task modal. A click on a card morphs it into a fixed-size panel over a
//! dimmed board: title, stats, the markdown notes editor and the latest time
//! entries. Closing morphs it back into the card.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSButton, NSColor, NSControlSize, NSFont, NSFontWeightRegular,
    NSFontWeightSemibold, NSLineBreakMode, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use yatt_core::{Status, TaskId};

use crate::classes::{ActionTarget, Fill, FillView, FlippedView};
use crate::editor::{EditorMsg, NotesEditor};
use crate::model::{CardSnap, UiEvent, format_duration};
use crate::motion;
use crate::window::{Sink, animate, animate_with_completion};

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
    _retained: Vec<Retained<AnyObject>>,
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

        Self {
            dim,
            panel,
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
            self.panel.setFrame(frame);
        }
        let (w, h) = (frame.size.width, frame.size.height);
        self.content
            .setFrame(NSRect::new(NSPoint::ZERO, frame.size));
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
        self.layout(bounds);

        let target = Self::target_frame(bounds);
        self.panel.setFrame(from);
        self.content
            .setFrame(NSRect::new(NSPoint::ZERO, target.size));
        self.content.setAlphaValue(0.0);
        self.dim.setAlphaValue(0.0);
        self.dim.setHidden(false);
        self.panel.setHidden(false);
        let reduce = motion::reduce_motion();
        if reduce {
            self.panel.setFrame(target);
        }
        let (panel, content, dim) = (self.panel.clone(), self.content.clone(), self.dim.clone());
        animate(true, motion::MODAL_OPEN_SECS, move || {
            if !reduce {
                panel.animator().setFrame(target);
            }
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
        let (panel2, dim2) = (self.panel.clone(), self.dim.clone());
        let reduce = motion::reduce_motion();
        animate_with_completion(
            motion::MODAL_CLOSE_SECS,
            move || {
                match to {
                    Some(rect) if !reduce => panel.animator().setFrame(rect),
                    _ => panel.animator().setAlphaValue(0.0),
                }
                content.animator().setAlphaValue(0.0);
                dim.animator().setAlphaValue(0.0);
            },
            move || {
                panel2.setHidden(true);
                panel2.setAlphaValue(1.0);
                dim2.setHidden(true);
                done();
            },
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
