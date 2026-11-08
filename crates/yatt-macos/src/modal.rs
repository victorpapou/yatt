//! The task inspector: the open task in a native inspector pane on the right of
//! the board (a split-view item, as in Calendar or Finder), so the board stays in
//! view and clickable. Title and stats under a header in the card's colour, the
//! markdown notes editor and the latest time entries. Opening and closing is the
//! split item's own collapse animation; the content fades with it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSFontWeightMedium, NSFontWeightRegular,
    NSLineBreakMode, NSMenu, NSMenuItem, NSPasteboard,
    NSPasteboardTypeString, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use yatt_core::{Status, TaskId};

use crate::classes::{ActionTarget, Fill, FillView, FlippedView, PanelView};
use crate::editor::{EditorMsg, NotesEditor};
use crate::model::{CardSnap, UiEvent, format_duration};
use crate::picker::{Current, Picker};
use crate::style;
use crate::window::Sink;

/// The time entries' line at the bottom.
const FOOTER_H: f64 = 44.0;
const PANEL_PAD: f64 = 20.0;

pub struct Modal {
    /// The inspector's content view.
    pub panel: Retained<PanelView>,
    material: Retained<objc2_app_kit::NSVisualEffectView>,
    title: Retained<NSTextField>,
    /// Covers the header; a click on it starts editing the title.
    rename: Retained<NSButton>,
    stats: Retained<NSTextField>,
    /// The subtasks the checklist lines of the notes make, as last sent.
    subs_known: Rc<RefCell<HashMap<TaskId, Vec<(String, bool)>>>>,
    /// The tags the #tags of the notes make, as last sent.
    tags_known: Rc<RefCell<HashMap<TaskId, Vec<String>>>>,
    /// The tag colours last given to the editor.
    colors_sent: RefCell<String>,
    /// The tags, as capsules; Enter or a comma adds one.
    tags: Rc<crate::tags::Tags>,
    rules: [Retained<FillView>; 2],
    editor: NotesEditor,
    editor_clip: Retained<FlippedView>,
    /// The time entries' one line, and every entry, as written.
    times: Retained<NSButton>,
    all_entries: Rc<RefCell<Vec<String>>>,
    look: Retained<NSButton>,
    picker: Rc<Picker>,
    /// The open task's colour.
    hue: Cell<usize>,
    /// The height of the titlebar with the toolbar: where the header starts.
    top: Cell<f64>,
    /// The task on screen, shared with the callbacks.
    open: Rc<Cell<Option<TaskId>>>,
    /// Last title known to match the store, to tell edits from echoes.
    title_known: Rc<RefCell<String>>,
    sink: Sink,
    /// Notes known to match the store, per task: a late flush from the editor may
    /// belong to a task that is no longer open.
    notes_known: Rc<RefCell<HashMap<TaskId, String>>>,
    /// The editor's keys for the tasks it has shown.
    keys: Rc<RefCell<HashMap<String, TaskId>>>,
    _retained: Vec<Retained<AnyObject>>,
}

impl Modal {
    pub fn new(mtm: MainThreadMarker, sink: &Sink) -> Self {
        let open: Rc<Cell<Option<TaskId>>> = Rc::new(Cell::new(None));
        let title_known = Rc::new(RefCell::new(String::new()));
        let notes_known: Rc<RefCell<HashMap<TaskId, String>>> = Rc::default();
        let keys: Rc<RefCell<HashMap<String, TaskId>>> = Rc::default();
        let mut retained: Vec<Retained<AnyObject>> = Vec::new();

        let panel = PanelView::new(mtm);
        // The system's own material behind the pane, as an inspector has it.
        let material = objc2_app_kit::NSVisualEffectView::new(mtm);
        material.setMaterial(objc2_app_kit::NSVisualEffectMaterial::Sidebar);
        material.setBlendingMode(objc2_app_kit::NSVisualEffectBlendingMode::BehindWindow);
        material.setState(objc2_app_kit::NSVisualEffectState::FollowsWindowActiveState);
        material.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        panel.addSubview(&material);
        let content = &panel;
        let title = NSTextField::textFieldWithString(ns_string!(""), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(20.0, unsafe {
            NSFontWeightMedium
        })));
        // `textFieldWithString` makes a read-only label; the title must be typed into.
        title.setEditable(true);
        title.setSelectable(true);
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
                crate::eventlog::log(format!("title action: {text:?} known {:?}", known.borrow()));
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

        let subs_known: Rc<RefCell<HashMap<TaskId, Vec<(String, bool)>>>> = Rc::default();
        let tags_known: Rc<RefCell<HashMap<TaskId, Vec<String>>>> = Rc::default();
        let tags = crate::tags::Tags::new(mtm);

        let editor = {
            let (sink, open, known) = (sink.clone(), open.clone(), notes_known.clone());
            let (keys, subs_known, tags_known) =
                (keys.clone(), subs_known.clone(), tags_known.clone());
            NotesEditor::new(mtm, move |msg| match msg {
                EditorMsg::Notes { key, md } => {
                    // Filed under the task the notes were typed into, open or not.
                    let Some(id) = keys.borrow().get(&key).copied() else {
                        return;
                    };
                    if known.borrow().get(&id) != Some(&md) {
                        known.borrow_mut().insert(id, md.clone());
                        // The checklist lines are the task's subtasks.
                        let list = checklist(&md);
                        if subs_known.borrow().get(&id) != Some(&list) {
                            subs_known.borrow_mut().insert(id, list.clone());
                            sink(UiEvent::SetSubtasks { id, subtasks: list });
                        }
                        // The #tags are its tags.
                        let tags = hashtags(&md);
                        if tags_known.borrow().get(&id) != Some(&tags) {
                            tags_known.borrow_mut().insert(id, tags.clone());
                            sink(UiEvent::SetTags { id, tags });
                        }
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
        // WKWebView is hosted by WebKit and can paint outside the panel's
        // animated bounds. Keep it in a flipped, layer-backed viewport so its
        // frame uses the same top-left coordinates as the panel content.
        let editor_clip = FlippedView::new(mtm, NSRect::ZERO);
        editor_clip.setWantsLayer(true);
        if let Some(layer) = editor_clip.layer() {
            layer.setMasksToBounds(true);
        }
        editor.view.setWantsLayer(true);
        if let Some(layer) = editor.view.layer() {
            layer.setMasksToBounds(true);
        }
        editor_clip.addSubview(&editor.view);

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
            // No image of its own: AppKit stretched the button's image layer
            // out of shape. The icon is a bitmap under it (`look_icon`), drawn
            // like the cards' glyphs, aspect kept at any size.
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    ns_string!(""),
                    Some(&target),
                    Some(ActionTarget::sel()),
                    mtm,
                )
            };
            b.setBordered(false);
            b.setTransparent(true);
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

        // The whole header is a way into the title: a click anywhere on it puts
        // the caret there with the text selected, ready to be typed over.
        let rename = {
            let field = ObjcWeak::from_retained(&title);
            let target = ActionTarget::new(mtm, move || {
                if let Some(field) = field.load() {
                    if let Some(window) = field.window() {
                        window.makeFirstResponder(Some(&field));
                    }
                    unsafe { field.selectText(None) };
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
            b.setTransparent(true);
            b.setToolTip(Some(ns_string!("Переименовать")));
            retained.push(Retained::into_super(Retained::into_super(target)));
            b
        };

        let rules = [
            FillView::new(mtm, Fill::Separator),
            FillView::new(mtm, Fill::Separator),
        ];
        let views: [&NSView; 9] = [
            &rename,
            &title,
            &stats,
            tags.as_view(),
            &look,
            &rules[0],
            &editor_clip,
            &rules[1],
            &times,
        ];
        for v in views {
            content.addSubview(v);
        }


        Self {
            panel,
            material,
            title,
            stats,
            subs_known,
            tags_known,
            colors_sent: RefCell::new(String::new()),
            tags,
            rules,
            editor,
            editor_clip,
            times,
            all_entries,
            look,
            picker,
            hue: Cell::new(0),
            top: Cell::new(52.0),
            open,
            title_known,
            sink: sink.clone(),
            rename,
            notes_known,
            keys,
            _retained: retained,
        }
    }

    /// Sends the notes still waiting to be saved, then calls `done`.
    pub fn flush_then(&self, done: impl FnOnce() + 'static) {
        self.editor.flush_then(done);
    }

    /// Undo or redo in the notes editor.
    pub fn editor_undo(&self, redo: bool) {
        self.editor.undo(redo);
    }

    /// Development aid: types `title` over the open task's title and `notes` into
    /// its notes, leaving both still being edited.
    pub fn debug_type(&self, title: &str, notes: &str) {
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(Some(&self.title));
        }
        if let Some(field) = self.title.currentEditor() {
            field.setString(&NSString::from_str(title));
        }
        self.editor.debug_type(notes);
    }

    pub fn debug_print_editor(&self) {
        self.editor.debug_print_html();
    }

    /// Development aid: the panel's frame, angle (always 0), how open it is
    /// (0 or 1) and its alphas (dim and card text are gone: 0).
    pub fn debug_state(&self) -> (NSRect, f64, f64, [f64; 4]) {
        let open = if self.open.get().is_some() { 1.0 } else { 0.0 };
        (
            self.panel.frame(),
            0.0,
            open,
            [0.0, self.panel.alphaValue(), self.panel.alphaValue(), 0.0],
        )
    }

    /// Development aid: where the editor sits in the panel, on stderr.
    pub fn debug_editor_geometry(&self) {
        let f = self
            .editor
            .view
            .convertRect_toView(self.editor.view.bounds(), Some(&self.panel));
        eprintln!(
            "editor view in panel: x {:.1} y {:.1} w {:.1} h {:.1}; panel w {:.1} h {:.1}",
            f.origin.x,
            f.origin.y,
            f.size.width,
            f.size.height,
            self.panel.frame().size.width,
            self.panel.frame().size.height
        );
    }

    /// Lets the tags' relayout reach the pane (set once the window exists).
    pub fn on_subtasks_resize(&self, f: impl Fn() + Clone + 'static) {
        *self.tags.on_resize.borrow_mut() = Some(Box::new(f));
    }

    /// The tags' capsules in their (possibly new) colours.
    pub fn refresh_tag_colors(&self) {
        self.tags.refresh();
    }

    pub fn open_task(&self) -> Option<TaskId> {
        self.open.get()
    }

    /// Lays the content out in the panel's current size; the header starts `top`
    /// down, below the toolbar.
    pub fn layout(&self, top: f64) {
        self.top.set(top);
        self.material.setFrame(self.panel.bounds());
        let size = self.panel.bounds().size;
        let (w, h) = (size.width, size.height);
        let pad = PANEL_PAD;
        let iw = (w - 2.0 * pad).max(0.0);
        let ty = top + 16.0;
        let tags_y = ty + 56.0;
        let tags_h = self.tags.layout(w);
        self.tags.view.setFrame(NSRect::new(
            NSPoint::new(0.0, tags_y),
            NSSize::new(w, tags_h),
        ));
        // The header is as tall as its title, stats and tags (rows of them wrap).
        let header_h = (tags_y + tags_h + 14.0) - top;
        self.rename.setFrame(NSRect::new(
            NSPoint::new(0.0, top),
            NSSize::new(w, header_h),
        ));
        self.title.setFrame(NSRect::new(
            NSPoint::new(pad, ty),
            NSSize::new(iw, 28.0),
        ));
        // The look button is only the picker's anchor, at the pane's top right
        // (its real button is in the toolbar's inspector section).
        self.look.setFrame(NSRect::new(
            NSPoint::new(w - pad - 28.0, top - 40.0),
            NSSize::new(28.0, 28.0),
        ));
        self.stats.setFrame(NSRect::new(
            NSPoint::new(pad, ty + 32.0),
            NSSize::new(iw, 16.0),
        ));
        self.rules[0].setFrame(NSRect::new(
            NSPoint::new(0.0, top + header_h),
            NSSize::new(w, 1.0),
        ));
        let editor_top = top + header_h + 1.0;
        let editor_size = NSSize::new(w, (h - editor_top - FOOTER_H).max(0.0));
        self.editor_clip
            .setFrame(NSRect::new(NSPoint::new(0.0, editor_top), editor_size));
        self.editor
            .view
            .setFrame(NSRect::new(NSPoint::ZERO, editor_size));
        self.editor.set_page_width(w);
        self.rules[1].setFrame(NSRect::new(
            NSPoint::new(0.0, h - FOOTER_H),
            NSSize::new(w, 1.0),
        ));
        self.times.setFrame(NSRect::new(
            NSPoint::new(pad, h - FOOTER_H + 12.0),
            NSSize::new(iw, 20.0),
        ));
    }

    /// Shows `snap` in the inspector (the pane itself opens in the window).
    pub fn open(&self, snap: &CardSnap, status: Status) {
        self.open.set(Some(snap.id));
        *self.title_known.borrow_mut() = snap.title.clone();
        let key = snap.id.to_string();
        self.keys.borrow_mut().insert(key.clone(), snap.id);
        self.notes_known
            .borrow_mut()
            .insert(snap.id, snap.notes.clone());
        self.title.setStringValue(&NSString::from_str(&snap.title));
        // Subtasks made before the checklist lived in the notes join them as
        // checklist lines (once; the notes are then the one place for them).
        let mut notes = snap.notes.clone();
        if !snap.subtasks.is_empty() && checklist(&notes).is_empty() {
            if !notes.trim().is_empty() {
                notes.push_str("\n\n");
            }
            let lines: Vec<String> = snap
                .subtasks
                .iter()
                .map(|(t, d)| format!("- [{}] {t}", if *d { "x" } else { " " }))
                .collect();
            notes.push_str(&lines.join("\n"));
            self.notes_known.borrow_mut().insert(snap.id, notes.clone());
            (self.sink)(UiEvent::SetNotes(snap.id, notes.clone()));
        }
        // Tags set before they lived in the notes join them as #tags (once).
        let present = hashtags(&notes);
        let missing: Vec<&String> = snap.tags.iter().filter(|t| !present.contains(t)).collect();
        if !missing.is_empty() {
            if !notes.trim().is_empty() {
                notes.push_str("\n\n");
            }
            let line: Vec<String> = missing.iter().map(|t| format!("#{t}")).collect();
            notes.push_str(&line.join(" "));
            self.notes_known.borrow_mut().insert(snap.id, notes.clone());
            (self.sink)(UiEvent::SetNotes(snap.id, notes.clone()));
        }
        self.tags_known
            .borrow_mut()
            .insert(snap.id, hashtags(&notes));
        self.subs_known
            .borrow_mut()
            .insert(snap.id, checklist(&notes));
        self.editor.set_markdown(&notes, &key);
        self.update(snap, status);
        self.layout(self.top.get());
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(Some(&self.editor.view));
        }
        self.editor.focus();
    }

    /// Sends the title field's text as a rename if it differs from the stored one.
    fn commit_title(&self) {
        let Some(id) = self.open.get() else { return };
        let text = self.title.stringValue().to_string().trim().to_string();
        crate::eventlog::log(format!("commit_title on close: {text:?}"));
        if !text.is_empty() && text != *self.title_known.borrow() {
            *self.title_known.borrow_mut() = text.clone();
            (self.sink)(UiEvent::Rename(id, text));
        }
    }

    /// Saves what is typed and lets go of the open task (the pane itself closes
    /// in the window, or the next task is shown in it).
    pub fn close(&self) {
        self.editor.flush();
        // Another card or the close button comes before the field's end-editing
        // action runs: commit the typed title here, while the task is still open.
        self.commit_title();
        self.picker.close();
        if let Some(window) = self.panel.window() {
            window.makeFirstResponder(None);
        }
        self.open.set(None);
    }

    /// The window turned light or dark: the header in the new colours (the next
    /// `update` sets the ink).
    pub fn restyle(&self) {}

    /// The palette button in the toolbar: the look and status popover.
    pub fn toggle_picker(&self) {
        self.picker.open(&self.look);
    }

    /// The trash button in the toolbar.
    pub fn delete_open(&self) {
        if let Some(id) = self.open.get() {
            (self.sink)(UiEvent::Delete(id));
        }
    }

    /// The close button in the toolbar.
    pub fn close_open(&self) {
        if let Some(id) = self.open.get() {
            (self.sink)(UiEvent::ToggleExpand(id));
        }
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
        self.tags.set(&snap.tags);
        // The capsules of the #tags in the notes, in their colours.
        let colors: Vec<String> = hashtags(&snap.notes)
            .iter()
            .map(|t| format!("{}:{}", json_str(t), json_str(&style::tag_css(t))))
            .collect();
        let json = format!("{{{}}}", colors.join(","));
        if *self.colors_sent.borrow() != json {
            self.editor.set_tag_colors(&json);
            *self.colors_sent.borrow_mut() = json;
        }
        let stats = format!(
            "{}{}  ·  сегодня {}  ·  🍅 {}",
            if snap.tracking { "● " } else { "" },
            format_duration(snap.total_ms),
            format_duration(snap.today_ms),
            snap.pomodoros
        );
        self.stats.setStringValue(&NSString::from_str(&stats));
        let hue = style::hue_of(snap.id);
        self.hue.set(hue);
        self.title.setTextColor(Some(&NSColor::labelColor()));
        self.stats.setTextColor(Some(&NSColor::secondaryLabelColor()));
        self.picker.show_task(Current {
            id: Some(snap.id),
            color: snap.color,
            icon: snap.icon.clone(),
            status: Some(status),
        });
        // The notes sit on the window's own background, in its ordinary ink.
        self.editor.set_ink(&style::editor_ink_css());
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

/// A string as a JSON literal.
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The #tags written in Markdown notes, once each, in order of appearance: a `#`
/// at the start of a word followed by letters, digits, `_` or `-` (so `# Title`
/// is a heading, not a tag); code (fenced or inline) is left out.
pub(crate) fn hashtags(md: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // Without `inline code` spans.
        let mut text = String::new();
        let mut in_code = false;
        for c in line.chars() {
            if c == '`' {
                in_code = !in_code;
            } else if !in_code {
                text.push(c);
            }
        }
        let chars: Vec<char> = text.chars().collect();
        for i in 0..chars.len() {
            if chars[i] != '#' || (i > 0 && !(chars[i - 1].is_whitespace() || chars[i - 1] == '(')) {
                continue;
            }
            let tag: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_alphanumeric() || **c == '_' || **c == '-')
                .collect();
            if !tag.is_empty() && !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out
}

/// The checklist lines of Markdown notes (`- [ ] title`, `- [x] title`, at any
/// indent, also with `*`): the task's subtasks, title and whether it is ticked.
pub(crate) fn checklist(md: &str) -> Vec<(String, bool)> {
    md.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            let rest = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
            let (done, title) = if let Some(r) = rest.strip_prefix("[ ] ") {
                (false, r)
            } else if let Some(r) = rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
                (true, r)
            } else {
                return None;
            };
            let title = title.trim();
            (!title.is_empty()).then(|| (title.to_string(), done))
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::{checklist, hashtags};

    #[test]
    fn checklist_lines_are_the_subtasks() {
        let md = "# План\n\n- [x] Макеты\n- [ ] Сверстать\n  - [ ] вложенная\n* [X] Готово\n- обычный пункт\n- [ ] \nтекст";
        assert_eq!(
            checklist(md),
            vec![
                ("Макеты".to_string(), true),
                ("Сверстать".to_string(), false),
                ("вложенная".to_string(), false),
                ("Готово".to_string(), true),
            ]
        );
    }

    #[test]
    fn hashtags_are_words_after_a_hash() {
        let md = "# Заголовок\nпро #дизайн и #motion, ещё #дизайн\n`#код` и (#в-скобках)\n```\n#вблоке\n```\nссылка a#b";
        assert_eq!(hashtags(md), vec!["дизайн", "motion", "в-скобках"]);
    }

    #[test]
    fn two_tasks_keep_their_own_checklists() {
        let a = "- [x] a1\n- [ ] a2";
        let b = "заметка без подзадач";
        assert_eq!(checklist(a).len(), 2);
        assert!(checklist(b).is_empty());
    }
}
