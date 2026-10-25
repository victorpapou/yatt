# Shortcuts

Who gets a key depends on where the focus is: the board (nothing is being
typed), the new-task capsule (a text field), or the notes editor in the task
panel (Tiptap in a web view). Menu key equivalents run first in AppKit, so the
Edit menu sends its commands down the responder chain: a text field or the
editor takes them before the board ever sees them.

| Shortcut | Board | Capsule | Notes editor |
|---|---|---|---|
| ⌘N | Focus the capsule | — (already there) | Focus the capsule |
| ↩ | — | New task in «Бэклог» | New line / list item |
| ⌘↩ | — | New task in «В работе» | — |
| Space | Start / pause the timer | Types a space | Types a space |
| ⌘Z / ⌘⇧Z | Undo / redo on the board (a group move, paste, cut or delete is one step) | Undo / redo the typing | Undo / redo in the notes (the editor's own history) |
| ⌘C / ⌘X / ⌘V | Copy / cut / paste the selected cards (pasted text makes one card per line) | Text | Text |
| ⌘A | Select every card in the focused column (or the board) | Select the text | Select the notes |
| ⌫ / ⌦ | Delete the selected cards (one undo step) | Delete characters | Delete characters |
| Fingers or wheel down / up over a stack (either scrolling setting) | Fan it out / fold it, following the fingers (140 pt for all the way; a fling goes at once; let go part way and it opens or folds, whichever is nearer, after 0.25 s) | — | — |
| Click on a card (list, stack or fan) | Open it | — | — |
| A task in the menu bar's menu | Brings yatt forward and opens the task | the same | the same |
| Esc | Clear the selection; fold the fans | End editing | Close the task panel |
| ⌘. | Stop the timer | Stop the timer | Stop the timer |
| ⌘] | Skip the phase | Skip the phase | Skip the phase |
| Hold 🍅 in the toolbar (0.4 s) | Pomodoro lengths: work / break, presets 25/5, 40/10, 50/15 | the same | the same |
| ⌘B / ⌘I / ⌘K, Tab, ↩ in lists | — | — | The editor's own (bold, italic, link, indent, new item); the app menu has none of them |

Conflicts found and how they are resolved:

- ⌘Z / ⌘⇧Z in the notes editor: the menu took them before the page, and the
  plain `undo:` it then sent did not reach the editor's own history. Now, with
  the editor focused, the app calls the editor's undo and redo directly
  (`NotesEditor::undo`).
- Space: it is not a menu key equivalent, only the window's own key handler,
  which never sees keys a text field or the editor takes.
- ⌫ / ⌦ act on cards only when the board has the keys and something is
  selected.
- No menu item uses ⌘B, ⌘I, ⌘K or ⌘↩, so the editor and the capsule keep them.
