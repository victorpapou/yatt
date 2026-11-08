// Notes editor for yatt's task modal. Runs in a WKWebView; talks to the app
// through window.webkit.messageHandlers.yatt and window.yatt.
import { Editor, Extension } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'
import StarterKit from '@tiptap/starter-kit'
import { Placeholder } from '@tiptap/extensions'
import { Markdown } from '@tiptap/markdown'
import Image from '@tiptap/extension-image'
import { TaskList, TaskItem } from '@tiptap/extension-list'

const css = `
:root { color-scheme: light dark; }
html, body { width: 100%; height: 100%; margin: 0; overflow: hidden; background: transparent; }
body {
  font: 14px/1.5 -apple-system, BlinkMacSystemFont, sans-serif;
  color: CanvasText;
  -webkit-font-smoothing: antialiased;
}
/* The page fills the panel's body edge to edge, so the scroller sits at the
   panel's edge and text scrolls away under the rules; the panel's margins are
   the text's own padding, inside what scrolls. The page keeps the open
   panel's width (--page) while the panel grows round it, so the notes never
   reflow during the morph. */
#editor { width: 100%; height: 100%; overflow-x: hidden; overflow-y: auto; }
.ProseMirror {
  box-sizing: border-box; width: var(--page, 100%); min-height: 100%; outline: none;
  padding: 20px 24px 24px;
}
.ProseMirror > :first-child { margin-top: 0; }
.ProseMirror p { margin: 0 0 8px; }
.ProseMirror h1 { font-size: 20px; margin: 12px 0 6px; }
.ProseMirror h2 { font-size: 17px; margin: 12px 0 6px; }
.ProseMirror h3 { font-size: 15px; margin: 10px 0 4px; }
.ProseMirror ul, .ProseMirror ol { padding-left: 22px; margin: 0 0 8px; }
/* Checklists («- [ ]» / «- [x]»): a real checkbox in front of each line, ticked
   items struck through and dimmed. These lines are the task's subtasks. */
.ProseMirror ul[data-type="taskList"] { list-style: none; padding-left: 2px; }
.ProseMirror ul[data-type="taskList"] li { display: flex; align-items: flex-start; gap: 8px; margin: 2px 0; }
.ProseMirror ul[data-type="taskList"] li > label { flex: 0 0 auto; margin-top: 3px; user-select: none; }
.ProseMirror ul[data-type="taskList"] li > div { flex: 1 1 auto; min-width: 0; }
.ProseMirror ul[data-type="taskList"] li > div > p { margin: 0; }
.ProseMirror ul[data-type="taskList"] li[data-checked="true"] > div {
  text-decoration: line-through; color: color-mix(in srgb, CanvasText 45%, transparent);
}
.ProseMirror ul[data-type="taskList"] input[type="checkbox"] {
  width: 15px; height: 15px; margin: 0; accent-color: AccentColor; cursor: pointer;
}
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
.ProseMirror .tag {
  border-radius: 10px; padding: 1px 7px;
  background: color-mix(in srgb, CanvasText 12%, transparent);
}
.ProseMirror p.is-editor-empty:first-child::before {
  content: attr(data-placeholder); float: left; height: 0; pointer-events: none;
  color: color-mix(in srgb, CanvasText 35%, transparent);
}
`
const style = document.createElement('style')
style.textContent = css
document.head.appendChild(style)

// «#тег» in the text is a tag: drawn in place as a capsule (the text stays as
// typed). The colours come from the app (`setTagColors`).
let tagColors = {}
const HashTags = Extension.create({
  name: 'hashtags',
  addProseMirrorPlugins() {
    return [new Plugin({
      key: new PluginKey('hashtags'),
      props: {
        decorations(state) {
          const decos = []
          state.doc.descendants((node, pos, parent) => {
            if (!node.isText) return
            if (parent && parent.type.name === 'codeBlock') return
            if (node.marks.some((m) => m.type.name === 'code')) return
            const re = /(^|[\s(])#([\p{L}\p{N}_-]+)/gu
            let m
            while ((m = re.exec(node.text))) {
              const start = pos + m.index + m[1].length
              const end = start + 1 + m[2].length
              const c = tagColors[m[2]]
              decos.push(Decoration.inline(start, end, { class: 'tag', style: c ? 'background:' + c : '' }))
            }
          })
          return DecorationSet.create(state.doc, decos)
        },
      },
    })]
  },
})

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
    TaskList,
    HashTags,
    TaskItem.configure({ nested: true }),
    Image.configure({ allowBase64: true }),
    Placeholder.configure({ placeholder: 'Заметки… Markdown: # заголовок, - список, - [ ] подзадача, **жирный**, `код`' }),
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
  setTagColors(map) {
    tagColors = map || {}
    editor.view.dispatch(editor.state.tr.setMeta('hashtags', true))
  },
  flush() { if (timer) flush() },
  // The caret at the end, without scrolling the notes as the panel opens.
  focus() { editor.commands.focus('end', { scrollIntoView: false }) },
  // ⌘Z and ⌘⇧Z reach the app's menu first; it hands them back here.
  undo() { editor.commands.undo() },
  redo() { editor.commands.redo() },
}
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { e.preventDefault(); flush(); post('close') }
})
post('ready')
