// Notes editor for yatt's task modal. Runs in a WKWebView; talks to the app
// through window.webkit.messageHandlers.yatt and window.yatt.
import { Editor } from '@tiptap/core'
import StarterKit from '@tiptap/starter-kit'
import { Placeholder } from '@tiptap/extensions'
import { Markdown } from '@tiptap/markdown'
import Image from '@tiptap/extension-image'

const css = `
:root { color-scheme: light dark; }
html, body { margin: 0; background: transparent; }
body {
  font: 14px/1.5 -apple-system, BlinkMacSystemFont, sans-serif;
  color: CanvasText;
  -webkit-font-smoothing: antialiased;
}
.ProseMirror { outline: none; min-height: 100vh; padding: 2px 2px 24px; }
.ProseMirror > :first-child { margin-top: 0; }
.ProseMirror p { margin: 0 0 8px; }
.ProseMirror h1 { font-size: 20px; margin: 12px 0 6px; }
.ProseMirror h2 { font-size: 17px; margin: 12px 0 6px; }
.ProseMirror h3 { font-size: 15px; margin: 10px 0 4px; }
.ProseMirror ul, .ProseMirror ol { padding-left: 22px; margin: 0 0 8px; }
.ProseMirror code {
  font: 12.5px ui-monospace, SFMono-Regular, Menlo, monospace;
  background: color-mix(in srgb, CanvasText 8%, transparent);
  border-radius: 4px; padding: 1px 4px;
}
.ProseMirror pre {
  background: color-mix(in srgb, CanvasText 6%, transparent);
  border-radius: 8px; padding: 10px 12px; overflow-x: auto;
}
.ProseMirror pre code { background: none; padding: 0; }
.ProseMirror blockquote {
  margin: 0 0 8px; padding-left: 12px;
  border-left: 3px solid color-mix(in srgb, CanvasText 20%, transparent);
  color: color-mix(in srgb, CanvasText 70%, transparent);
}
.ProseMirror a { color: AccentColor; }
.ProseMirror img {
  display: block; max-width: 100%; height: auto; margin: 4px 0 10px;
  border-radius: 12px;
}
.ProseMirror img.ProseMirror-selectednode { outline: 2px solid AccentColor; }
.ProseMirror hr { border: none; border-top: 1px solid color-mix(in srgb, CanvasText 15%, transparent); }
.ProseMirror p.is-editor-empty:first-child::before {
  content: attr(data-placeholder); float: left; height: 0; pointer-events: none;
  color: color-mix(in srgb, CanvasText 35%, transparent);
}
`
const style = document.createElement('style')
style.textContent = css
document.head.appendChild(style)

const post = (msg) => window.webkit?.messageHandlers?.yatt?.postMessage(msg)

let timer = null
let applying = false
const flush = () => {
  clearTimeout(timer)
  timer = null
  post('notes:' + editor.getMarkdown())
}

const editor = new Editor({
  element: document.getElementById('editor'),
  extensions: [
    StarterKit,
    Markdown,
    Image.configure({ allowBase64: true }),
    Placeholder.configure({ placeholder: 'Заметки… Markdown: # заголовок, - список, **жирный**, `код`' }),
  ],
  content: '',
  onUpdate: () => {
    if (applying) return
    clearTimeout(timer)
    timer = setTimeout(flush, 400)
  },
  onBlur: () => { if (timer) flush() },
})

window.yatt = {
  setMarkdown(md) {
    applying = true
    editor.commands.setContent(md || '', { contentType: 'markdown', emitUpdate: false })
    applying = false
  },
  flush() { if (timer) flush() },
  focus() { editor.commands.focus('end') },
}
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { e.preventDefault(); flush(); post('close') }
})
post('ready')
