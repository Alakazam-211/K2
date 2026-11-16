// Set a live editor's text as one edit of yours (Undo their save): only the
// part that differs changes, so carets and others' places away from it stay,
// and the server's per-person undo (⌘Z) reverses it in one step.

import type { EditorView } from '@codemirror/view'

/** The one change that turns `from` into `to`: common start and end kept. */
export function minimalChange(from: string, to: string): { from: number; to: number; insert: string } | null {
  if (from === to) return null
  let a = 0
  const max = Math.min(from.length, to.length)
  while (a < max && from.charCodeAt(a) === to.charCodeAt(a)) a++
  let b = 0
  while (b < max - a && from.charCodeAt(from.length - 1 - b) === to.charCodeAt(to.length - 1 - b)) b++
  return { from: a, to: from.length - b, insert: to.slice(a, to.length - b) }
}

export function replaceText(view: EditorView, text: string): boolean {
  const c = minimalChange(view.state.doc.toString(), text)
  if (!c) return false
  view.dispatch({ changes: c, userEvent: 'input.replace' })
  return true
}
