// Task lists (build-editors.md E2): the checkbox toggles `[ ]` and `[x]` in
// the text by an ordinary transaction, so it's one undoable edit, and in a
// live doc it travels like any other keystroke.

import type { EditorView } from '@codemirror/view'

// Quote marks, indent, a bullet or a number, then the box.
const TASK = /^((?:\s*>)*\s*(?:[-+*]|\d{1,9}[.)])\s+\[)([ xX])\]/

/** Is the task on the line holding `pos` done? null when that line has no task. */
export function taskDone(view: EditorView, pos: number): boolean | null {
  const m = TASK.exec(view.state.doc.lineAt(pos).text)
  return m ? m[2] !== ' ' : null
}

/** Tick or untick the task on the line holding `pos`. False when there's none or the file is read only. */
export function toggleTask(view: EditorView, pos: number): boolean {
  if (view.state.readOnly) return false
  const line = view.state.doc.lineAt(pos)
  const m = TASK.exec(line.text)
  if (!m) return false
  const at = line.from + m[1].length
  view.dispatch({ changes: { from: at, to: at + 1, insert: m[2] === ' ' ? 'x' : ' ' }, userEvent: 'input.toggle-task' })
  return true
}
