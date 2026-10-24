//! The markdown notes editor: Tiptap (bundled from `editor/`, see its package.json)
//! running in a WKWebView. Content goes in with `yatt.setMarkdown`, comes back as
//! `notes:<markdown>` messages, debounced in the page.

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
    Notes(String),
    /// Esc pressed inside the editor.
    Close,
}

pub struct NotesEditor {
    pub view: Retained<WKWebView>,
    ready: Rc<Cell<bool>>,
    /// Content to load once the page is ready.
    pending: Rc<RefCell<Option<String>>>,
    _handler: Retained<ScriptHandler>,
}

impl NotesEditor {
    pub fn new(mtm: MainThreadMarker, on_msg: impl Fn(EditorMsg) + 'static) -> Self {
        let ready = Rc::new(Cell::new(false));
        let pending: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config)
        };
        let handler = {
            let (ready, pending, view) = (ready.clone(), pending.clone(), view.clone());
            ScriptHandler::new(mtm, move |msg| {
                if msg == "ready" {
                    ready.set(true);
                    if let Some(md) = pending.borrow_mut().take() {
                        eval(&view, &set_markdown_js(&md));
                    }
                } else if msg == "close" {
                    on_msg(EditorMsg::Close);
                } else if let Some(md) = msg.strip_prefix("notes:") {
                    on_msg(EditorMsg::Notes(md.to_string()));
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
            _handler: handler,
        }
    }

    pub fn set_markdown(&self, md: &str) {
        if self.ready.get() {
            eval(&self.view, &set_markdown_js(md));
        } else {
            *self.pending.borrow_mut() = Some(md.to_string());
        }
    }

    /// Sends any edit still waiting for the debounce.
    pub fn flush(&self) {
        if self.ready.get() {
            eval(&self.view, "window.yatt.flush()");
        }
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
        let block = block2::RcBlock::new(
            |result: *mut objc2::runtime::AnyObject, _err: *mut objc2_foundation::NSError| {
                let text = unsafe { result.as_ref() }
                    .and_then(|r| r.downcast_ref::<NSString>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "<no result>".into());
                eprintln!("editor html: {text}");
            },
        );
        unsafe {
            self.view.evaluateJavaScript_completionHandler(
                ns_string!("document.querySelector('.ProseMirror')?.innerHTML ?? 'no editor'"),
                Some(&block),
            )
        };
    }
}

fn eval(view: &WKWebView, js: &str) {
    unsafe { view.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
}

fn set_markdown_js(md: &str) -> String {
    format!("window.yatt.setMarkdown({})", js_string(md))
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
