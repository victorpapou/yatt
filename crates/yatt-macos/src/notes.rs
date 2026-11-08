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
use objc2_app_kit::NSStringDrawing;
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSKernAttributeName, NSMutableParagraphStyle, NSObliquenessAttributeName,
    NSParagraphStyleAttributeName, NSScrollView, NSStrikethroughStyleAttributeName, NSTextDelegate,
    NSTextView, NSTextViewDelegate, NSView,
};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSNotification, NSNumber, NSPoint, NSRange, NSRect,
    NSSize, NSString, NSTimer,
};

use objc2_quartz_core::{
    CABasicAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction, CAShapeLayer,
    kCALineCapRound, kCAMediaTimingFunctionEaseOut,
};
use objc2_foundation::ns_string;

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
    text: RefCell<Option<Retained<NotesTextView>>>,
    boxes: RefCell<Vec<BoxLayer>>,
    /// Decoded images by their URL (None: not decodable).
    images: RefCell<HashMap<String, Option<Retained<objc2_app_kit::NSImage>>>>,
    image_layers: RefCell<Vec<Retained<CALayer>>>,
    bullet_layers: RefCell<Vec<Retained<CALayer>>>,
    /// The images shown: where their text starts, the URL and the size they take.
    image_slots: RefCell<Vec<(usize, String, f64, f64)>>,
    /// Where each shown checkbox is, and the index of its state character.
    hits: RefCell<Vec<(NSRect, usize)>>,
    /// The checkbox clicked last (for a Shift-click range).
    last_box: Cell<Option<usize>>,
    /// Our own edit in progress: the guard of the atomic markers stands aside.
    internal: Cell<bool>,
}

/// One checkbox's layers: ring, accent fill and the tick drawn as a path.
pub(crate) struct BoxLayer {
    pub root: Retained<CALayer>,
    pub fill: Retained<CALayer>,
    pub ring: Retained<CAShapeLayer>,
    pub tick: Retained<CAShapeLayer>,
    pub checked: Cell<bool>,
}

pub(crate) const BOX: f64 = 16.0;
/// What a collapsed `- ` takes in the line.
const BULLET_W: f64 = 18.0;
/// The height of a checklist line.
const BOX_LINE: f64 = 20.0;
/// What a collapsed `- [ ] ` takes in the line.
const BOX_W: f64 = 24.0;

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

        #[unsafe(method(textViewDidChangeSelection:))]
        fn selection_changed(&self, _note: &NSNotification) {
            let st = &self.ivars().state;
            if !st.applying.get() {
                restyle(st);
            }
        }

        #[unsafe(method(textView:willChangeSelectionFromCharacterRange:toCharacterRange:))]
        fn will_change_selection(&self, view: &NSTextView, old: NSRange, new: NSRange) -> NSRange {
            if new.length > 0 {
                return new;
            }
            // The checkbox is one atom: the caret stands before it or after it.
            for (start, end) in box_markers(&view.string().to_string()) {
                if new.location > start && new.location < end {
                    let at = if new.location < old.location { start } else { end };
                    return NSRange::new(at, 0);
                }
            }
            new
        }

        #[unsafe(method(textView:shouldChangeTextInRange:replacementString:))]
        fn should_change(
            &self,
            view: &NSTextView,
            range: NSRange,
            _with: Option<&NSString>,
        ) -> Bool {
            if self.ivars().state.internal.get() {
                return Bool::YES;
            }
            // An edit that touches part of a checkbox takes all of it (a Backspace
            // after the circle turns the line plain).
            let (rs, re) = (range.location, range.location + range.length);
            for (start, end) in box_markers(&view.string().to_string()) {
                let touches = if range.length == 0 {
                    false
                } else {
                    rs < end && re > start
                };
                let contains = rs <= start && re >= end;
                if touches && !contains {
                    let (from, to) = (rs.min(start), re.max(end));
                    let with = _with.map(|s| s.to_string()).unwrap_or_default();
                    replace_range(view, NSRange::new(from, to - from), &with);
                    return Bool::NO;
                }
            }
            Bool::YES
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


pub struct TextIvars {
    on_layout: RefCell<Option<Box<dyn Fn()>>>,
    on_click: RefCell<Option<Box<dyn Fn(NSPoint, bool) -> bool>>>,
}

define_class!(
    // SAFETY: NSTextView has no extra subclassing requirements; no Drop impl.
    #[unsafe(super(NSTextView, objc2_app_kit::NSText, NSView, objc2_app_kit::NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TextIvars]
    pub struct NotesTextView;

    impl NotesTextView {
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };
            if let Some(f) = self.ivars().on_layout.borrow().as_ref() {
                f();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let hit = self
                .ivars()
                .on_click
                .borrow()
                .as_ref()
                .is_some_and(|f| f(p, event.modifierFlags().contains(objc2_app_kit::NSEventModifierFlags::Shift)));
            if !hit {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }
    }
);

impl NotesTextView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TextIvars {
            on_layout: RefCell::default(),
            on_click: RefCell::default(),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

pub struct NotesEditor {
    pub view: Retained<NSScrollView>,
    text: Retained<NotesTextView>,
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
            boxes: RefCell::default(),
            images: RefCell::default(),
            image_layers: RefCell::default(),
            bullet_layers: RefCell::default(),
            image_slots: RefCell::default(),
            hits: RefCell::default(),
            last_box: Cell::new(None),
            internal: Cell::new(false),
        });
        let scroll = NSScrollView::new(mtm);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let text = NotesTextView::new(
            mtm,
            NSRect::new(NSPoint::ZERO, NSSize::new(300.0, 300.0)),
        );
        text.setWantsLayer(true);
        crate::classes::enable_inline_prediction(&text);
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
        {
            let weak = Rc::downgrade(&state);
            *text.ivars().on_layout.borrow_mut() = Some(Box::new(move || {
                if let Some(st) = weak.upgrade() {
                    place_boxes(&st);
                }
            }));
            let weak = Rc::downgrade(&state);
            *text.ivars().on_click.borrow_mut() = Some(Box::new(move |p, shift| {
                weak.upgrade().is_some_and(|st| click_box(&st, p, shift))
            }));
        }
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

    /// Development aid: clicks the centre of checkbox `k`, as the mouse would.
    pub fn debug_click_box(&self, k: usize, shift: bool) {
        let hit = self.state.hits.borrow().get(k).map(|(r, _)| *r);
        if let Some(r) = hit {
            let c = NSPoint::new(r.origin.x + r.size.width / 2.0, r.origin.y + r.size.height / 2.0);
            eprintln!("editor: click box {k} at {:.0},{:.0}: {}", c.x, c.y, click_box(&self.state, c, shift));
        } else {
            eprintln!("editor: no box {k} ({} boxes)", self.state.hits.borrow().len());
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
    // The caret's line shows its raw `- [ ] `; the others collapse it to a box.
    let sel = text.selectedRange();
    let caret_line = line_range(&string, sel.location);
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
    let mut slots: Vec<(usize, String, f64, f64)> = Vec::new();
    for sp in parse(&string) {
        match sp.kind {
            Kind::Marker => unsafe { set(NSForegroundColorAttributeName, &dim, sp.start, sp.end) },
            Kind::Hide => collapse(&storage, &string, sp.start, sp.end, 0.0),
            Kind::Bullet => collapse(&storage, &string, sp.start, sp.end, BULLET_W),
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
                // Always a circle, whoever has the caret: the marker is collapsed to
                // the width of the checkbox drawn in its place.
                collapse(&storage, &string, sp.start, sp.end, BOX_W);
                // A line of its own fixed height: the checkbox sits in the middle.
                let lp = NSMutableParagraphStyle::new();
                lp.setLineSpacing(3.0);
                lp.setParagraphSpacing(4.0);
                lp.setMinimumLineHeight(BOX_LINE);
                lp.setMaximumLineHeight(BOX_LINE);
                let (ls, le) = line_range(&string, sp.start);
                unsafe { set(NSParagraphStyleAttributeName, &lp, ls, (le + 1).min(total)) };
            }
            Kind::Image => {
                let raw = utf16_slice(&string, sp.start, sp.end);
                let on_caret = sel.location >= sp.start && sel.location <= sp.end;
                let shown = (!on_caret)
                    .then(|| url_of(&raw))
                    .flatten()
                    .and_then(|url| image_size(st, &text, &url).map(|(w, h)| (url, w, h)));
                if let Some((url, w, h)) = shown {
                    // The text collapses; its line is as tall as the picture, which
                    // is drawn over it.
                    let tiny = NSFont::systemFontOfSize(1.0);
                    let clear = NSColor::clearColor();
                    let p = NSMutableParagraphStyle::new();
                    p.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByClipping);
                    p.setMinimumLineHeight(h + 8.0);
                    p.setMaximumLineHeight(h + 8.0);
                    p.setParagraphSpacing(4.0);
                    unsafe {
                        set(NSFontAttributeName, &tiny, sp.start, sp.end);
                        set(NSForegroundColorAttributeName, &clear, sp.start, sp.end);
                        set(NSParagraphStyleAttributeName, &p, sp.start, sp.end);
                    }
                    slots.push((sp.start, url, w, h));
                } else {
                    let f = NSFont::systemFontOfSize(6.0);
                    let c = ink.colorWithAlphaComponent(0.25);
                    unsafe {
                        set(NSFontAttributeName, &f, sp.start, sp.end);
                        set(NSForegroundColorAttributeName, &c, sp.start, sp.end);
                    }
                }
            }
            Kind::Tag(name) => {
                // Only the text takes the tag's colour (medium), no fill.
                let (r, g, b) = colors.get(&name).copied().unwrap_or_else(|| {
                    let c = style::swatch(style::tag_hue(&name));
                    let (r, g, b, _) = rgba(&c);
                    (r, g, b)
                });
                let c = NSColor::colorWithSRGBRed_green_blue_alpha(
                    r as f64 / 255.0,
                    g as f64 / 255.0,
                    b as f64 / 255.0,
                    1.0,
                );
                let f = NSFont::systemFontOfSize_weight(BODY, unsafe {
                    objc2_app_kit::NSFontWeightMedium
                });
                unsafe {
                    set(NSForegroundColorAttributeName, &c, sp.start, sp.end);
                    set(NSFontAttributeName, &f, sp.start, sp.end);
                }
            }
        }
    }
    storage.endEditing();
    *st.image_slots.borrow_mut() = slots;
    drop(colors);
    place_boxes(st);
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
    let replace = |range: NSRange, with: &str| replace_range(view, range, with);
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


/// Collapses the marker [start, end) to `width` points (0: gone) while keeping the
/// body font on it, so a line with nothing else on it keeps the height of a line and
/// the caret its size. Each character is kerned back by its own advance.
fn collapse(
    storage: &objc2_app_kit::NSTextStorage,
    string: &str,
    start: usize,
    end: usize,
    width: f64,
) {
    let slice = utf16_slice(string, start, end);
    let font = NSFont::systemFontOfSize(BODY);
    let keys = unsafe { [NSFontAttributeName] };
    let vals: [Retained<AnyObject>; 1] = [Retained::into_super(Retained::into_super(font))];
    let attrs = NSDictionary::from_retained_objects(&keys, &vals);
    let chars: Vec<char> = slice.chars().collect();
    let mut at = start;
    for (i, c) in chars.iter().enumerate() {
        let adv = unsafe { NSString::from_str(&c.to_string()).sizeWithAttributes(Some(&attrs)) }.width;
        let keep = if i + 1 == chars.len() { width } else { 0.0 };
        let kern = NSNumber::new_f64(keep - adv);
        let len = c.len_utf16();
        unsafe {
            storage.addAttribute_value_range(NSKernAttributeName, &kern, NSRange::new(at, len));
            storage.addAttribute_value_range(
                NSForegroundColorAttributeName,
                &NSColor::clearColor(),
                NSRange::new(at, len),
            );
        }
        at += len;
    }
}

/// The checkbox markers of the text: (start, end) in UTF-16 units.
fn box_markers(string: &str) -> Vec<(usize, usize)> {
    parse(string)
        .into_iter()
        .filter(|s| matches!(s.kind, Kind::Box { .. } | Kind::Hide | Kind::Bullet))
        .map(|s| (s.start, s.end))
        .collect()
}

/// The UTF-16 range (start, end) of the line holding `loc`.
fn line_range(string: &str, loc: usize) -> (usize, usize) {
    let units: Vec<u16> = string.encode_utf16().collect();
    let loc = loc.min(units.len());
    let start = units[..loc].iter().rposition(|u| *u == b'\n' as u16).map_or(0, |i| i + 1);
    let end = units[loc..]
        .iter()
        .position(|u| *u == b'\n' as u16)
        .map_or(units.len(), |i| loc + i);
    (start, end)
}

/// Lays the checkbox layers over the collapsed markers.
fn place_boxes(st: &Rc<State>) {
    let Some(text) = st.text.borrow().clone() else {
        return;
    };
    let (Some(lm), Some(container)) = (unsafe { text.layoutManager() }, unsafe { text.textContainer() })
    else {
        return;
    };
    let string = text.string().to_string();
    let caret = line_range(&string, text.selectedRange().location);
    let boxes: Vec<(usize, bool)> = parse(&string)
        .into_iter()
        .filter_map(|s| match s.kind {
            Kind::Box { checked } => Some((s.start, checked)),
            _ => None,
        })
        .collect();
    let origin = text.textContainerOrigin();
    let Some(layer) = text.layer() else { return };
    let ink = st.ink.borrow().clone();
    let mut layers = st.boxes.borrow_mut();
    while layers.len() < boxes.len() {
        layers.push(make_box());
        layer.addSublayer(&layers.last().unwrap().root);
    }
    let mut hits = Vec::new();
    unsafe { lm.ensureLayoutForTextContainer(&container) };
    for (k, bl) in layers.iter().enumerate() {
        let Some((start, checked)) = boxes.get(k).copied() else {
            bl.root.setHidden(true);
            continue;
        };
        let glyphs = unsafe {
            lm.glyphRangeForCharacterRange_actualCharacterRange(
                NSRange::new(start, 6),
                std::ptr::null_mut(),
            )
        };
        let used = unsafe {
            lm.lineFragmentUsedRectForGlyphAtIndex_effectiveRange(
                glyphs.location,
                std::ptr::null_mut(),
            )
        };
        let rect = lm.boundingRectForGlyphRange_inTextContainer(glyphs, &container);
        let frame = NSRect::new(
            NSPoint::new(
                (origin.x + rect.origin.x).round(),
                (origin.y + used.origin.y + (BOX_LINE - BOX) / 2.0).round(),
            ),
            NSSize::new(BOX, BOX),
        );
        bl.root.setHidden(false);
        crate::motion::quietly(|| bl.root.setFrame(frame));
        bl.ring.setStrokeColor(Some(&ink.colorWithAlphaComponent(0.4).CGColor()));
        bl.fill
            .setBackgroundColor(Some(&NSColor::controlAccentColor().CGColor()));
        if bl.checked.replace(checked) != checked {
            animate_box(bl, checked);
        } else {
            crate::motion::quietly(|| {
                bl.fill.setOpacity(if checked { 1.0 } else { 0.0 });
                bl.tick.setStrokeEnd(if checked { 1.0 } else { 0.0 });
            });
        }
        hits.push((frame, start + 3));
    }
    *st.hits.borrow_mut() = hits;
    drop(layers);
    place_images(st, &text, &lm, &container, origin);
    place_bullets(st, &text, &lm, origin, &string);
}

pub(crate) fn make_box() -> BoxLayer {
    let root = CALayer::new();
    root.setBounds(NSRect::new(NSPoint::ZERO, NSSize::new(BOX, BOX)));
    let fill = CALayer::new();
    fill.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(BOX, BOX)));
    fill.setCornerRadius(BOX / 2.0);
    fill.setOpacity(0.0);
    let ring = CAShapeLayer::new();
    ring.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(BOX, BOX)));
    ring.setFillColor(None);
    ring.setLineWidth(1.5);
    let circle = CGMutablePath::new();
    unsafe {
        CGMutablePath::add_ellipse_in_rect(
            Some(&circle),
            std::ptr::null(),
            NSRect::new(NSPoint::new(0.75, 0.75), NSSize::new(BOX - 1.5, BOX - 1.5)),
        );
    }
    ring.setPath(Some(&circle));
    let tick = CAShapeLayer::new();
    tick.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(BOX, BOX)));
    tick.setFillColor(None);
    tick.setStrokeColor(Some(&NSColor::whiteColor().CGColor()));
    tick.setLineWidth(1.7);
    unsafe { tick.setLineCap(kCALineCapRound) };
    tick.setLineJoin(unsafe { objc2_quartz_core::kCALineJoinRound });
    let path = CGMutablePath::new();
    unsafe {
        CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 4.6, 8.4);
        CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), 7.0, 10.8);
        CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), 11.6, 5.6);
    }
    tick.setPath(Some(&path));
    tick.setStrokeEnd(0.0);
    root.addSublayer(&ring);
    root.addSublayer(&fill);
    root.addSublayer(&tick);
    BoxLayer {
        root,
        fill,
        ring,
        tick,
        checked: Cell::new(false),
    }
}

/// The fill comes in, and the tick runs along its path (0.25 s, ease-out); or back.
pub(crate) fn animate_box(bl: &BoxLayer, checked: bool) {
    let timing = CAMediaTimingFunction::functionWithName(unsafe { kCAMediaTimingFunctionEaseOut });
    let (from_end, to_end) = if checked { (0.0, 1.0) } else { (1.0, 0.0) };
    let (from_op, to_op) = if checked { (0.0, 1.0) } else { (1.0, 0.0) };
    crate::motion::quietly(|| {
        bl.tick.setStrokeEnd(to_end);
        bl.fill.setOpacity(to_op as f32);
    });
    let a = unsafe { CABasicAnimation::animationWithKeyPath(Some(ns_string!("strokeEnd"))) };
    unsafe {
        a.setFromValue(Some(&NSNumber::new_f64(from_end)));
        a.setToValue(Some(&NSNumber::new_f64(to_end)));
    }
    a.setDuration(0.25);
    a.setTimingFunction(Some(&timing));
    bl.tick.addAnimation_forKey(&a, Some(ns_string!("tick")));
    let b = unsafe { CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity"))) };
    unsafe {
        b.setFromValue(Some(&NSNumber::new_f64(from_op)));
        b.setToValue(Some(&NSNumber::new_f64(to_op)));
    }
    b.setDuration(0.2);
    b.setTimingFunction(Some(&timing));
    bl.fill.addAnimation_forKey(&b, Some(ns_string!("fill")));
}

/// A click on a checkbox ticks or unticks its line (an edit of the text, so undoable).
/// With Shift, every checkbox from the last one clicked to this one takes the new
/// state, in one edit.
fn click_box(st: &Rc<State>, p: NSPoint, shift: bool) -> bool {
    let hit = st.hits.borrow().iter().position(|(r, _)| {
        p.x >= r.origin.x - 4.0
            && p.x <= r.origin.x + r.size.width + 4.0
            && p.y >= r.origin.y - 4.0
            && p.y <= r.origin.y + r.size.height + 4.0
    });
    let (Some(k), Some(text)) = (hit, st.text.borrow().clone()) else {
        return false;
    };
    toggle_boxes(st, &text, k, shift);
    true
}

/// Ticks or unticks checkbox `k` (and, with `shift`, the ones between it and the
/// last one clicked) as one edit.
fn toggle_boxes(st: &Rc<State>, text: &NSTextView, k: usize, shift: bool) {
    let idxs: Vec<usize> = st.hits.borrow().iter().map(|(_, i)| *i).collect();
    let Some(&idx) = idxs.get(k) else { return };
    let units: Vec<u16> = text.string().to_string().encode_utf16().collect();
    let checked_at = |i: usize| units.get(i).is_some_and(|u| *u != b' ' as u16);
    let target = !checked_at(idx);
    let from = if shift { st.last_box.get().unwrap_or(k) } else { k };
    st.last_box.set(Some(k));
    let (lo, hi) = (from.min(k), from.max(k));
    let (first, last) = (idxs[lo], idxs[hi]);
    let mut new: Vec<u16> = units[first..=last].to_vec();
    for &i in &idxs[lo..=hi] {
        new[i - first] = if target { b'x' as u16 } else { b' ' as u16 };
    }
    let changed = new != units[first..=last];
    if changed {
        st.internal.set(true);
        replace_range(text, NSRange::new(first, last - first + 1), &String::from_utf16_lossy(&new));
        st.internal.set(false);
    }
}

/// Replaces `range` with `with` as an edit the user made (undo, change notifications).
fn replace_range(view: &NSTextView, range: NSRange, with: &str) {
    let ns = NSString::from_str(with);
    if view.shouldChangeTextInRange_replacementString(range, Some(&ns)) {
        if let Some(storage) = unsafe { view.textStorage() } {
            storage.replaceCharactersInRange_withString(range, &ns);
        }
        view.didChangeText();
    }
}

/// The UTF-16 slice [s, e) of `string`.
fn utf16_slice(string: &str, s: usize, e: usize) -> String {
    let units: Vec<u16> = string.encode_utf16().collect();
    String::from_utf16_lossy(&units[s.min(units.len())..e.min(units.len())])
}

/// `![alt](url)` to its url.
fn url_of(raw: &str) -> Option<String> {
    let i = raw.find("](")? + 2;
    let j = raw.rfind(')')?;
    (j > i).then(|| raw[i..j].to_string())
}

/// The image of a `data:` URL (base64, or percent-encoded text such as SVG).
fn decode_image(url: &str) -> Option<Retained<objc2_app_kit::NSImage>> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let data = if meta.contains(";base64") {
        objc2_foundation::NSData::initWithBase64EncodedString_options(
            objc2_foundation::NSData::alloc(),
            &NSString::from_str(payload),
            objc2_foundation::NSDataBase64DecodingOptions::IgnoreUnknownCharacters,
        )?
    } else {
        let text = NSString::from_str(payload).stringByRemovingPercentEncoding()?;
        objc2_foundation::NSData::with_bytes(text.to_string().as_bytes())
    };
    objc2_app_kit::NSImage::initWithData(objc2_app_kit::NSImage::alloc(), &data)
}

/// The size an image takes in the text's width, if it can be shown.
fn image_size(st: &Rc<State>, text: &NSTextView, url: &str) -> Option<(f64, f64)> {
    let mut cache = st.images.borrow_mut();
    let img = cache
        .entry(url.to_string())
        .or_insert_with(|| decode_image(url))
        .clone()?;
    let natural = img.size();
    if natural.width <= 0.0 || natural.height <= 0.0 {
        return None;
    }
    let avail = {
        let w = text.frame().size.width - 2.0 * text.textContainerInset().width - 10.0;
        if w > 40.0 { w } else { 300.0 }
    };
    let w = natural.width.min(avail);
    let h = (w * natural.height / natural.width).min(360.0);
    Some((w, h))
}

/// The pictures over their collapsed lines.
fn place_images(
    st: &Rc<State>,
    text: &NSTextView,
    lm: &objc2_app_kit::NSLayoutManager,
    _container: &objc2_app_kit::NSTextContainer,
    origin: NSPoint,
) {
    let Some(layer) = text.layer() else { return };
    let slots = st.image_slots.borrow().clone();
    let mut layers = st.image_layers.borrow_mut();
    while layers.len() < slots.len() {
        let l = CALayer::new();
        l.setCornerRadius(12.0);
        l.setMasksToBounds(true);
        layer.addSublayer(&l);
        layers.push(l);
    }
    for (k, l) in layers.iter().enumerate() {
        let Some((start, url, w, h)) = slots.get(k) else {
            l.setHidden(true);
            continue;
        };
        let glyphs = unsafe {
            lm.glyphRangeForCharacterRange_actualCharacterRange(
                NSRange::new(*start, 1),
                std::ptr::null_mut(),
            )
        };
        let used = unsafe {
            lm.lineFragmentUsedRectForGlyphAtIndex_effectiveRange(
                glyphs.location,
                std::ptr::null_mut(),
            )
        };
        let frame = NSRect::new(
            NSPoint::new(origin.x + 5.0, (origin.y + used.origin.y + 4.0).round()),
            NSSize::new(*w, *h),
        );
        let img = st.images.borrow().get(url).cloned().flatten();
        l.setHidden(false);
        crate::motion::quietly(|| {
            l.setFrame(frame);
            if let Some(img) = img {
                unsafe { l.setContents(Some(&img)) };
            }
        });
    }
}

/// The bullets of list items, drawn in the room their `- ` leaves.
fn place_bullets(
    st: &Rc<State>,
    text: &NSTextView,
    lm: &objc2_app_kit::NSLayoutManager,
    origin: NSPoint,
    string: &str,
) {
    let Some(layer) = text.layer() else { return };
    let starts: Vec<usize> = parse(string)
        .into_iter()
        .filter(|s| matches!(s.kind, Kind::Bullet))
        .map(|s| s.start)
        .collect();
    let ink = st.ink.borrow().colorWithAlphaComponent(0.8);
    let mut layers = st.bullet_layers.borrow_mut();
    while layers.len() < starts.len() {
        let l = CALayer::new();
        l.setCornerRadius(2.5);
        layer.addSublayer(&l);
        layers.push(l);
    }
    for (k, l) in layers.iter().enumerate() {
        let Some(start) = starts.get(k).copied() else {
            l.setHidden(true);
            continue;
        };
        let glyphs = unsafe {
            lm.glyphRangeForCharacterRange_actualCharacterRange(
                NSRange::new(start, 1),
                std::ptr::null_mut(),
            )
        };
        let used = unsafe {
            lm.lineFragmentUsedRectForGlyphAtIndex_effectiveRange(
                glyphs.location,
                std::ptr::null_mut(),
            )
        };
        let rect = unsafe { lm.boundingRectForGlyphRange_inTextContainer(glyphs, &text.textContainer().unwrap()) };
        let frame = NSRect::new(
            NSPoint::new(
                (origin.x + rect.origin.x + 5.0).round(),
                (origin.y + used.origin.y + (BOX_LINE - 5.0) / 2.0).round(),
            ),
            NSSize::new(5.0, 5.0),
        );
        l.setHidden(false);
        crate::motion::quietly(|| {
            l.setFrame(frame);
            l.setBackgroundColor(Some(&ink.CGColor()));
        });
    }
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
            Self::Native(e) => Retained::into_super(Retained::into_super(Retained::into_super(e.text.clone()))),
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
    pub fn debug_click_box(&self, k: usize, shift: bool) {
        if let Self::Native(e) = self {
            e.debug_click_box(k, shift);
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
