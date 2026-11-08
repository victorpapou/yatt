//! The notes editor, native: an NSTextView holding the Markdown text, styled live
//! (headings, emphasis, code, #tags, checklist lines) by `mdspans`. Same API as the
//! web editor in `editor.rs`: the notes come back as `EditorMsg::Notes` under the key
//! of the task they were typed into, debounced; ⌘Z is the text view's own undo.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSMutableParagraphStyle, NSObliquenessAttributeName,
    NSParagraphStyleAttributeName, NSScrollView, NSStrikethroughStyleAttributeName, NSTextDelegate,
    NSTextView, NSTextViewDelegate, NSView,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSNotification, NSNumber, NSPoint, NSRange, NSRect,
    NSSize, NSString, NSTimer,
};

use crate::editor::EditorMsg;
use crate::mdspans::{Kind, parse};
use crate::style;

const BODY: f64 = 14.0;
const DEBOUNCE: f64 = 0.4;

pub struct State {
    key: RefCell<String>,
    on_msg: Box<dyn Fn(EditorMsg)>,
    /// A change not sent yet.
    dirty: Cell<bool>,
    /// Loading text from outside: not an edit.
    applying: Cell<bool>,
    ink: RefCell<Retained<NSColor>>,
    tag_colors: RefCell<HashMap<String, (u8, u8, u8)>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    text: RefCell<Option<Retained<NSTextView>>>,
}

pub struct DelegateIvars {
    state: Rc<State>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    pub struct NotesDelegate;

    impl NotesDelegate {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _note: &NSNotification) {
            let st = &self.ivars().state;
            if st.applying.get() {
                return;
            }
            restyle(st);
            st.dirty.set(true);
            schedule(st);
        }

        #[unsafe(method(textView:doCommandBySelector:))]
        fn do_command(&self, view: &NSTextView, sel: Sel) -> Bool {
            if sel == sel!(insertNewline:) {
                return Bool::new(continue_list(view));
            }
            if sel == sel!(cancelOperation:) {
                send_notes(&self.ivars().state);
                (self.ivars().state.on_msg)(EditorMsg::Close);
                return Bool::YES;
            }
            Bool::NO
        }
    }

    unsafe impl NSObjectProtocol for NotesDelegate {}
    unsafe impl NSTextDelegate for NotesDelegate {}
    unsafe impl NSTextViewDelegate for NotesDelegate {}
);

pub struct NotesEditor {
    pub view: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    state: Rc<State>,
    _delegate: Retained<NotesDelegate>,
}

impl NotesEditor {
    pub fn new(mtm: MainThreadMarker, on_msg: impl Fn(EditorMsg) + 'static) -> Self {
        let state = Rc::new(State {
            key: RefCell::default(),
            on_msg: Box::new(on_msg),
            dirty: Cell::new(false),
            applying: Cell::new(false),
            ink: RefCell::new(NSColor::labelColor()),
            tag_colors: RefCell::default(),
            timer: RefCell::default(),
            text: RefCell::default(),
        });
        let scroll = NSScrollView::new(mtm);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(300.0, 300.0)),
        );
        text.setDrawsBackground(false);
        text.setRichText(true);
        text.setImportsGraphics(false);
        text.setAllowsUndo(true);
        text.setUsesFindBar(true);
        text.setAutomaticQuoteSubstitutionEnabled(false);
        text.setAutomaticDashSubstitutionEnabled(false);
        text.setAutomaticTextReplacementEnabled(false);
        text.setAutomaticSpellingCorrectionEnabled(false);
        text.setContinuousSpellCheckingEnabled(true);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        text.setTextContainerInset(NSSize::new(20.0, 12.0));
        text.setMinSize(NSSize::new(0.0, 0.0));
        text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        text.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        let delegate = {
            let this = NotesDelegate::alloc(mtm).set_ivars(DelegateIvars {
                state: state.clone(),
            });
            let this: Retained<NotesDelegate> = unsafe { msg_send![super(this), init] };
            this
        };
        text.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        scroll.setDocumentView(Some(&text));
        *state.text.borrow_mut() = Some(text.clone());
        Self {
            view: scroll,
            text,
            state,
            _delegate: delegate,
        }
    }

    /// What takes the keyboard when the pane opens.
    pub fn responder(&self) -> &NSView {
        &self.text
    }

    /// Loads `md`; the notes that come back from now on belong to `key`.
    pub fn set_markdown(&self, md: &str, key: &str) {
        // Edits waiting go out under the old key first.
        if self.state.dirty.get() {
            send_notes(&self.state);
        }
        *self.state.key.borrow_mut() = key.to_string();
        let same = self.text.string().to_string() == md;
        if !same {
            self.state.applying.set(true);
            self.text.setString(&NSString::from_str(md));
            self.state.applying.set(false);
            if let Some(um) = self.text.undoManager() {
                um.removeAllActions();
            }
        }
        restyle(&self.state);
    }

    /// The colours of the tags written in the notes (JSON: name to "r,g,b").
    pub fn set_tag_colors(&self, json: &str) {
        let map = parse_colors(json);
        if *self.state.tag_colors.borrow() != map {
            *self.state.tag_colors.borrow_mut() = map;
            restyle(&self.state);
        }
    }

    /// Draws the notes in `css` (`#rrggbb`).
    pub fn set_ink(&self, css: &str) {
        if let Some(c) = hex_color(css) {
            *self.state.ink.borrow_mut() = c;
            restyle(&self.state);
        }
    }

    /// The text view lays itself out to the pane's width.
    pub fn set_page_width(&self, _w: f64) {}

    pub fn flush(&self) {
        if self.state.dirty.get() {
            send_notes(&self.state);
        }
    }

    pub fn flush_then(&self, done: impl FnOnce() + 'static) {
        self.flush();
        done();
    }

    pub fn undo(&self, redo: bool) {
        if let Some(um) = self.text.undoManager() {
            if redo {
                um.redo();
            } else {
                um.undo();
            }
        }
    }

    pub fn focus(&self) {
        let end = self.text.string().length();
        self.text.setSelectedRange(NSRange::new(end, 0));
        if let Some(w) = self.text.window() {
            w.makeFirstResponder(Some(&self.text));
        }
    }

    /// Development aid: what the editor holds.
    pub fn debug_print_html(&self) {
        eprintln!("editor: {}", self.text.string());
    }

    /// Development aid: types `text` at the end of the notes.
    pub fn debug_type(&self, text: &str) {
        let end = self.text.string().length();
        self.text.setSelectedRange(NSRange::new(end, 0));
        unsafe { self.text.insertText(&NSString::from_str(text)) };
    }
}

/// Posts the notes now (the key they were typed under).
fn send_notes(st: &Rc<State>) {
    if let Some(t) = st.timer.borrow_mut().take() {
        t.invalidate();
    }
    if !st.dirty.replace(false) {
        return;
    }
    let md = st
        .text
        .borrow()
        .as_ref()
        .map(|t| t.string().to_string())
        .unwrap_or_default();
    (st.on_msg)(EditorMsg::Notes {
        key: st.key.borrow().clone(),
        md,
    });
}

/// Posts the notes after the debounce.
fn schedule(st: &Rc<State>) {
    if let Some(t) = st.timer.borrow_mut().take() {
        t.invalidate();
    }
    let weak = Rc::downgrade(st);
    let block = block2::RcBlock::new(move |_t: std::ptr::NonNull<NSTimer>| {
        if let Some(st) = weak.upgrade() {
            send_notes(&st);
        }
    });
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(DEBOUNCE, false, &block)
    };
    *st.timer.borrow_mut() = Some(timer);
}

fn ns_range(start: usize, end: usize) -> NSRange {
    NSRange::new(start, end - start)
}

/// Styles the whole text from its Markdown. Attributes only: the text, the
/// selection and the undo history are left alone.
fn restyle(st: &Rc<State>) {
    let Some(text) = st.text.borrow().clone() else {
        return;
    };
    let Some(storage) = (unsafe { text.textStorage() }) else {
        return;
    };
    let string = storage.string().to_string();
    let total = storage.length();
    let ink = st.ink.borrow().clone();
    let dim = ink.colorWithAlphaComponent(0.35);
    let colors = st.tag_colors.borrow();
    let para = NSMutableParagraphStyle::new();
    para.setLineSpacing(3.0);
    para.setParagraphSpacing(4.0);
    let body = NSFont::systemFontOfSize(BODY);
    storage.beginEditing();
    let full = NSRange::new(0, total);
    unsafe {
        storage.setAttributes_range(None, full);
        storage.addAttribute_value_range(NSFontAttributeName, &body, full);
        storage.addAttribute_value_range(NSForegroundColorAttributeName, &ink, full);
        storage.addAttribute_value_range(NSParagraphStyleAttributeName, &para, full);
    }
    let set = |name: &NSAttributedStringKey, value: &AnyObject, s: usize, e: usize| {
        if e > s && e <= total {
            unsafe { storage.addAttribute_value_range(name, value, ns_range(s, e)) };
        }
    };
    for sp in parse(&string) {
        match sp.kind {
            Kind::Marker => unsafe { set(NSForegroundColorAttributeName, &dim, sp.start, sp.end) },
            Kind::Heading(n) => {
                let size = [BODY + 8.0, BODY + 4.0, BODY + 2.0][(n as usize - 1).min(2)];
                let f = NSFont::systemFontOfSize_weight(size, unsafe {
                    objc2_app_kit::NSFontWeightSemibold
                });
                unsafe { set(NSFontAttributeName, &f, sp.start, sp.end) };
            }
            Kind::Bold => {
                let f = NSFont::systemFontOfSize_weight(BODY, unsafe {
                    objc2_app_kit::NSFontWeightSemibold
                });
                unsafe { set(NSFontAttributeName, &f, sp.start, sp.end) };
            }
            Kind::Italic => {
                let v = NSNumber::new_f64(0.2);
                unsafe { set(NSObliquenessAttributeName, &v, sp.start, sp.end) };
            }
            Kind::Strike => {
                let v = NSNumber::new_isize(1);
                unsafe { set(NSStrikethroughStyleAttributeName, &v, sp.start, sp.end) };
            }
            Kind::Code | Kind::CodeBlock => {
                let f = NSFont::monospacedSystemFontOfSize_weight(BODY - 1.0, unsafe {
                    objc2_app_kit::NSFontWeightRegular
                });
                let bg = ink.colorWithAlphaComponent(0.08);
                unsafe {
                    set(NSFontAttributeName, &f, sp.start, sp.end);
                    set(NSBackgroundColorAttributeName, &bg, sp.start, sp.end);
                }
            }
            Kind::Quote => {
                let c = ink.colorWithAlphaComponent(0.7);
                unsafe { set(NSForegroundColorAttributeName, &c, sp.start, sp.end) };
            }
            Kind::Done => {
                let c = ink.colorWithAlphaComponent(0.45);
                let v = NSNumber::new_isize(1);
                unsafe {
                    set(NSForegroundColorAttributeName, &c, sp.start, sp.end);
                    set(NSStrikethroughStyleAttributeName, &v, sp.start, sp.end);
                }
            }
            Kind::Box { .. } => {
                // The marker, muted; the circle checkbox is drawn over it (later).
                unsafe { set(NSForegroundColorAttributeName, &dim, sp.start, sp.end) };
            }
            Kind::Image => {
                let f = NSFont::systemFontOfSize(6.0);
                let c = ink.colorWithAlphaComponent(0.25);
                unsafe {
                    set(NSFontAttributeName, &f, sp.start, sp.end);
                    set(NSForegroundColorAttributeName, &c, sp.start, sp.end);
                }
            }
            Kind::Tag(name) => {
                let (r, g, b) = colors.get(&name).copied().unwrap_or_else(|| {
                    let c = style::swatch(style::tag_hue(&name));
                    let (r, g, b, _) = rgba(&c);
                    (r, g, b)
                });
                let bg = NSColor::colorWithSRGBRed_green_blue_alpha(
                    r as f64 / 255.0,
                    g as f64 / 255.0,
                    b as f64 / 255.0,
                    1.0,
                );
                let white = NSColor::whiteColor();
                let f = NSFont::systemFontOfSize_weight(BODY - 1.0, unsafe {
                    objc2_app_kit::NSFontWeightMedium
                });
                unsafe {
                    set(NSBackgroundColorAttributeName, &bg, sp.start, sp.end);
                    set(NSForegroundColorAttributeName, &white, sp.start, sp.end);
                    set(NSFontAttributeName, &f, sp.start, sp.end);
                }
            }
        }
    }
    storage.endEditing();
    // What is typed next starts plain, not in the style of the text before it.
    let keys: [&NSAttributedStringKey; 3] = unsafe {
        [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let vals: [Retained<AnyObject>; 3] = [
        Retained::into_super(Retained::into_super(body.clone())),
        Retained::into_super(Retained::into_super(ink.clone())),
        Retained::into_super(Retained::into_super(Retained::into_super(para.clone()))),
    ];
    let attrs = NSDictionary::from_retained_objects(&keys, &vals);
    unsafe { text.setTypingAttributes(&attrs) };
}

fn rgba(c: &NSColor) -> (u8, u8, u8, u8) {
    let c = c
        .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
        .unwrap_or_else(|| objc2::Message::retain(c));
    let q = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    (q(c.redComponent()), q(c.greenComponent()), q(c.blueComponent()), q(c.alphaComponent()))
}

/// Enter on a list line starts the next item (or ends the list on an empty one).
fn continue_list(view: &NSTextView) -> bool {
    let string = view.string().to_string();
    let sel = view.selectedRange();
    if sel.length > 0 {
        return false;
    }
    // The line the caret is on, in UTF-16 units.
    let units: Vec<u16> = string.encode_utf16().collect();
    let caret = sel.location.min(units.len());
    let start = units[..caret].iter().rposition(|u| *u == b'\n' as u16).map_or(0, |i| i + 1);
    let end = units[caret..]
        .iter()
        .position(|u| *u == b'\n' as u16)
        .map_or(units.len(), |i| caret + i);
    let line = String::from_utf16_lossy(&units[start..end]);
    let Some((marker, rest)) = list_marker(&line) else {
        return false;
    };
    if caret < end {
        return false;
    }
    let replace = |range: NSRange, with: &str| {
        let ns = NSString::from_str(with);
        if view.shouldChangeTextInRange_replacementString(range, Some(&ns)) {
            if let Some(storage) = unsafe { view.textStorage() } {
                storage.replaceCharactersInRange_withString(range, &ns);
            }
            view.didChangeText();
        }
    };
    if rest.trim().is_empty() {
        // An empty item: the marker goes, the list ends.
        replace(NSRange::new(start, end - start), "");
    } else {
        replace(NSRange::new(caret, 0), &format!("\n{marker}"));
    }
    true
}

/// A list line's marker as the next line starts it, and the text after it.
fn list_marker(line: &str) -> Option<(String, String)> {
    let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let body = &line[indent.len()..];
    for m in ["- [ ] ", "- [x] ", "- [X] "] {
        if let Some(rest) = body.strip_prefix(m) {
            return Some((format!("{indent}- [ ] "), rest.to_string()));
        }
    }
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = body.strip_prefix(m) {
            return Some((format!("{indent}{m}"), rest.to_string()));
        }
    }
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(rest) = body[digits.len()..].strip_prefix(". ") {
            let n: u64 = digits.parse().unwrap_or(0);
            return Some((format!("{indent}{}. ", n + 1), rest.to_string()));
        }
    }
    None
}

fn hex_color(css: &str) -> Option<Retained<NSColor>> {
    let h = css.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(NSColor::colorWithSRGBRed_green_blue_alpha(
        ((v >> 16) & 255) as f64 / 255.0,
        ((v >> 8) & 255) as f64 / 255.0,
        (v & 255) as f64 / 255.0,
        1.0,
    ))
}

/// `{"name":"r,g,b",…}` as a map.
fn parse_colors(json: &str) -> HashMap<String, (u8, u8, u8)> {
    let mut strings = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in json.chars() {
        if in_str {
            if esc {
                cur.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                strings.push(std::mem::take(&mut cur));
                in_str = false;
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_str = true;
        }
    }
    let mut map = HashMap::new();
    for pair in strings.chunks(2) {
        if let [name, rgb] = pair {
            let n: Vec<u8> = rgb.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let [r, g, b] = n[..] {
                map.insert(name.clone(), (r, g, b));
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_markers_continue() {
        assert_eq!(list_marker("- a"), Some(("- ".into(), "a".into())));
        assert_eq!(list_marker("  - [x] done"), Some(("  - [ ] ".into(), "done".into())));
        assert_eq!(list_marker("3. c"), Some(("4. ".into(), "c".into())));
        assert_eq!(list_marker("plain"), None);
    }

    #[test]
    fn colours_parse() {
        let m = parse_colors("{\"a\":\"1,2,3\",\"б\":\"255,0,9\"}");
        assert_eq!(m["a"], (1, 2, 3));
        assert_eq!(m["б"], (255, 0, 9));
    }
}

/// The notes editor in use: the native one, or (YATT_WEB_EDITOR=1) the web one.
pub enum AnyEditor {
    Native(NotesEditor),
    Web(crate::editor::NotesEditor),
}

impl AnyEditor {
    pub fn new(mtm: MainThreadMarker, on_msg: impl Fn(EditorMsg) + 'static) -> Self {
        if std::env::var_os("YATT_WEB_EDITOR").is_some() {
            Self::Web(crate::editor::NotesEditor::new(mtm, on_msg))
        } else {
            Self::Native(NotesEditor::new(mtm, on_msg))
        }
    }

    /// The view that goes into the pane.
    pub fn view(&self) -> Retained<NSView> {
        match self {
            Self::Native(e) => Retained::into_super(e.view.clone()),
            Self::Web(e) => Retained::into_super(e.view.clone()),
        }
    }

    /// What takes the keyboard.
    pub fn responder(&self) -> Retained<NSView> {
        match self {
            Self::Native(e) => Retained::into_super(Retained::into_super(e.text.clone())),
            Self::Web(e) => Retained::into_super(e.view.clone()),
        }
    }

    pub fn set_markdown(&self, md: &str, key: &str) {
        match self {
            Self::Native(e) => e.set_markdown(md, key),
            Self::Web(e) => e.set_markdown(md, key),
        }
    }
    pub fn set_tag_colors(&self, json: &str) {
        match self {
            Self::Native(e) => e.set_tag_colors(json),
            Self::Web(e) => e.set_tag_colors(json),
        }
    }
    pub fn set_ink(&self, css: &str) {
        match self {
            Self::Native(e) => e.set_ink(css),
            Self::Web(e) => e.set_ink(css),
        }
    }
    pub fn set_page_width(&self, w: f64) {
        match self {
            Self::Native(e) => e.set_page_width(w),
            Self::Web(e) => e.set_page_width(w),
        }
    }
    pub fn flush(&self) {
        match self {
            Self::Native(e) => e.flush(),
            Self::Web(e) => e.flush(),
        }
    }
    pub fn flush_then(&self, done: impl FnOnce() + 'static) {
        match self {
            Self::Native(e) => e.flush_then(done),
            Self::Web(e) => e.flush_then(done),
        }
    }
    pub fn undo(&self, redo: bool) {
        match self {
            Self::Native(e) => e.undo(redo),
            Self::Web(e) => e.undo(redo),
        }
    }
    pub fn focus(&self) {
        match self {
            Self::Native(e) => e.focus(),
            Self::Web(e) => e.focus(),
        }
    }
    pub fn debug_print_html(&self) {
        match self {
            Self::Native(e) => e.debug_print_html(),
            Self::Web(e) => e.debug_print_html(),
        }
    }
    pub fn debug_type(&self, text: &str) {
        match self {
            Self::Native(e) => e.debug_type(text),
            Self::Web(e) => e.debug_type(text),
        }
    }
}
