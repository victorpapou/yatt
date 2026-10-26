//! The markdown notes editor: Tiptap (bundled from `editor/`, see its package.json)
//! running in a WKWebView. Content goes in with `yatt.setMarkdown`, comes back as
//! `notes:<markdown>` messages, debounced in the page.
//!
//! Those messages arrive late (the debounce, and the flush when the panel closes),
//! when another task may already be open. So each load first posts `key:<task>`
//! through the same channel; messages keep their order, and every notes message is
//! filed under the key posted before it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSNumber, NSObjectNSKeyValueCoding, NSRect, NSString, ns_string};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

use crate::classes::ScriptHandler;

/// Built with `npm run build` in crates/yatt-macos/editor.
const BUNDLE: &str = include_str!("../editor/dist/editor.js");

pub enum EditorMsg {
    /// The notes of the task loaded under `key`.
    Notes { key: String, md: String },
    /// Esc pressed inside the editor.
    Close,
}

pub struct NotesEditor {
    pub view: Retained<WKWebView>,
    ready: Rc<Cell<bool>>,
    /// Content (and its key) to load once the page is ready.
    pending: Rc<RefCell<Option<(String, String)>>>,
    /// Text colour (CSS) to apply once the page is ready.
    ink: Rc<RefCell<Option<String>>>,
    /// The page's width (points), to set once the page is ready.
    page_width: Rc<Cell<f64>>,
    _handler: Retained<ScriptHandler>,
}

impl NotesEditor {
    pub fn new(mtm: MainThreadMarker, on_msg: impl Fn(EditorMsg) + 'static) -> Self {
        let ready = Rc::new(Cell::new(false));
        let pending: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
        let ink: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let page_width = Rc::new(Cell::new(0.0));
        // The key the next notes messages belong to, as posted from the page.
        let key = Rc::new(RefCell::new(String::new()));
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config)
        };
        let handler = {
            let (ready, pending, view) = (ready.clone(), pending.clone(), view.clone());
            let (ink, page_width) = (ink.clone(), page_width.clone());
            ScriptHandler::new(mtm, move |msg| {
                if msg == "ready" {
                    ready.set(true);
                    if page_width.get() > 0.0 {
                        eval(&view, &page_width_js(page_width.get()));
                    }
                    if let Some(css) = ink.borrow().as_ref() {
                        eval(&view, &set_ink_js(css));
                    }
                    if let Some((md, k)) = pending.borrow_mut().take() {
                        eval(&view, &set_markdown_js(&md, &k));
                    }
                } else if msg == "close" {
                    on_msg(EditorMsg::Close);
                } else if let Some(k) = msg.strip_prefix("key:") {
                    *key.borrow_mut() = k.to_string();
                } else if let Some(md) = msg.strip_prefix("notes:") {
                    on_msg(EditorMsg::Notes {
                        key: key.borrow().clone(),
                        md: md.to_string(),
                    });
                }
            })
        };
        unsafe {
            config.userContentController().addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*handler),
                ns_string!("yatt"),
            );
            // Let the panel behind show through.
            view.setValue_forKey(
                Some(&NSNumber::new_bool(false)),
                ns_string!("drawsBackground"),
            );
        }
        let html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
             <body><div id=\"editor\"></div><script>{BUNDLE}</script></body></html>"
        );
        unsafe { view.loadHTMLString_baseURL(&NSString::from_str(&html), None) };
        Self {
            view,
            ready,
            pending,
            ink,
            page_width,
            _handler: handler,
        }
    }

    /// Loads `md`; the notes that come back from now on belong to `key`.
    pub fn set_markdown(&self, md: &str, key: &str) {
        if self.ready.get() {
            eval(&self.view, &set_markdown_js(md, key));
        } else {
            *self.pending.borrow_mut() = Some((md.to_string(), key.to_string()));
        }
    }

    /// Draws the notes in `css` (a colour) on a light page, to sit on a card's tint.
    pub fn set_ink(&self, css: &str) {
        *self.ink.borrow_mut() = Some(css.to_string());
        if self.ready.get() {
            eval(&self.view, &set_ink_js(css));
        }
    }

    /// Lays the page out `w` points wide whatever the view's width, so the notes
    /// keep their lines while the panel grows round them.
    pub fn set_page_width(&self, w: f64) {
        if self.page_width.replace(w) != w && self.ready.get() {
            eval(&self.view, &page_width_js(w));
        }
    }

    /// Sends any edit still waiting for the debounce.
    pub fn flush(&self) {
        if self.ready.get() {
            eval(&self.view, "window.yatt.flush()");
        }
    }

    /// The editor's own undo or redo (ProseMirror's history), for ⌘Z and ⌘⇧Z,
    /// which the app's menu takes before the page can see them.
    pub fn undo(&self, redo: bool) {
        eval(
            &self.view,
            if redo {
                "window.yatt.redo()"
            } else {
                "window.yatt.undo()"
            },
        );
    }

    pub fn focus(&self) {
        if self.ready.get() {
            eval(&self.view, "window.yatt.focus()");
        }
    }
}

impl NotesEditor {
    /// Development aid: prints what the editor currently renders.
    pub fn debug_print_html(&self) {
        self.debug_eval("document.querySelector('.ProseMirror')?.innerHTML ?? 'no editor'");
    }

    /// Development aid: evaluates `js` in the page and prints its result.
    pub fn debug_eval(&self, js: &str) {
        let block = block2::RcBlock::new(
            |result: *mut objc2::runtime::AnyObject, _err: *mut objc2_foundation::NSError| {
                let text = unsafe { result.as_ref() }
                    .and_then(|r| r.downcast_ref::<NSString>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "<no result>".into());
                eprintln!("editor: {text}");
            },
        );
        unsafe {
            self.view
                .evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&block))
        };
    }
}

fn set_ink_js(css: &str) -> String {
    // The body's ink, and the placeholder and quiet parts in it, in the panel's
    // own appearance (system colours there follow the colour scheme).
    let rule = js_string(&format!(
        ".ProseMirror {{ color: {css}; }} \
         .ProseMirror p.is-editor-empty:first-child::before {{ \
         color: color-mix(in srgb, {css} 45%, transparent); }}"
    ));
    let scheme = if crate::style::dark() {
        "dark"
    } else {
        "light"
    };
    format!(
        "document.documentElement.style.colorScheme = '{scheme}';\
         (document.getElementById('ink') || document.head.appendChild(\
         Object.assign(document.createElement('style'), {{ id: 'ink' }}))).textContent = {rule};"
    )
}

fn page_width_js(w: f64) -> String {
    format!("document.documentElement.style.setProperty('--page', '{w}px')")
}

fn eval(view: &WKWebView, js: &str) {
    unsafe { view.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
}

/// Flushes edits still waiting for the debounce (they go out under the old key),
/// posts the new key, then loads the notes.
fn set_markdown_js(md: &str, key: &str) -> String {
    format!(
        "window.yatt.flush();\
         window.webkit.messageHandlers.yatt.postMessage({});\
         window.yatt.setMarkdown({})",
        js_string(&format!("key:{key}")),
        js_string(md)
    )
}

/// A JavaScript string literal for any text.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::js_string;

    #[test]
    fn escapes_for_javascript() {
        assert_eq!(
            js_string("a\"b\\c\n</script>"),
            "\"a\\\"b\\\\c\\n\\u003c/script>\""
        );
    }
}
