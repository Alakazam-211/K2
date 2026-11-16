// The formatting toolbar's commands (build-editors.md E3; editors-notes.md §2):
// Bold, Italic, Code, Link, Heading and List, as pure transactions on the
// markdown text, so they work the same in a plain file and in a live doc
// (where each is one edit of yours, like typing). formatState says what the
// caret is in, which the toolbar shows as on (aria-pressed). The / menu's
// blocks are lib/editor/slash.

import { EditorSelection, type ChangeSpec, type EditorState, type TransactionSpec } from '@codemirror/state'
import { syntaxTree } from '@codemirror/language'
import type { SyntaxNode } from '@lezer/common'

export const FORMAT_IDS = ['bold', 'italic', 'code', 'link', 'heading', 'list'] as const
export type FormatId = (typeof FORMAT_IDS)[number]
export type FormatState = Record<FormatId, boolean>

export const FORMAT_NAMES: Record<FormatId, string> = { bold: 'Bold', italic: 'Italic', code: 'Code', link: 'Link', heading: 'Heading', list: 'List' }

/** Each command's key: CodeMirror's name for it, the glyphs the tooltip shows, and aria-keyshortcuts. */
export const FORMAT_KEYS: Record<FormatId, { key: string; label: string; aria: string }> = {
  bold: { key: 'Mod-b', label: '⌘B', aria: 'Meta+B' },
  italic: { key: 'Mod-i', label: '⌘I', aria: 'Meta+I' },
  // ⌘E is Write | Read: Code takes ⇧⌘C.
  code: { key: 'Mod-Shift-c', label: '⇧⌘C', aria: 'Meta+Shift+C' },
  link: { key: 'Mod-k', label: '⌘K', aria: 'Meta+K' },
  heading: { key: 'Mod-Alt-2', label: '⌥⌘2', aria: 'Meta+Alt+2' },
  list: { key: 'Mod-Shift-8', label: '⇧⌘8', aria: 'Meta+Shift+8' },
}

const SPAN: Record<'bold' | 'italic' | 'code', { node: string; mark: string; marker: string }> = {
  bold: { node: 'StrongEmphasis', mark: 'EmphasisMark', marker: '**' },
  italic: { node: 'Emphasis', mark: 'EmphasisMark', marker: '*' },
  code: { node: 'InlineCode', mark: 'CodeMark', marker: '`' },
}

const ATX = /^( {0,3})(#{1,6})([ \t]+|$)/
const BULLET = /^(\s*)([-+*])[ \t]+/
const ITEM = /^\s*([-+*]|\d{1,9}[.)])([ \t]|$)/

/** The innermost node named `name` holding the whole selection, or null. */
function enclosing(state: EditorState, name: string): SyntaxNode | null {
  const { from, to } = state.selection.main
  const tree = syntaxTree(state)
  for (const side of [1, -1] as const) {
    for (let n: SyntaxNode | null = tree.resolveInner(from, side); n; n = n.parent) {
      if (n.name === name && n.from <= from && n.to >= to) return n
    }
  }
  return null
}

export function formatState(state: EditorState): FormatState {
  const line = state.doc.lineAt(state.selection.main.head)
  const inSetext = (() => {
    for (let n: SyntaxNode | null = syntaxTree(state).resolveInner(line.from, 1); n; n = n.parent) if (/^SetextHeading/.test(n.name)) return true
    return false
  })()
  return {
    bold: !!enclosing(state, 'StrongEmphasis'),
    italic: !!enclosing(state, 'Emphasis'),
    code: !!enclosing(state, 'InlineCode'),
    link: !!enclosing(state, 'Link'),
    heading: ATX.test(line.text) || inSetext,
    list: ITEM.test(line.text),
  }
}

function toggleSpan(state: EditorState, id: 'bold' | 'italic' | 'code'): TransactionSpec {
  const { node, mark, marker } = SPAN[id]
  const found = enclosing(state, node)
  if (found) {
    const marks: SyntaxNode[] = []
    for (let c = found.firstChild; c; c = c.nextSibling) if (c.name === mark) marks.push(c)
    if (marks.length >= 2) {
      const a = marks[0]
      const b = marks[marks.length - 1]
      return { changes: [{ from: a.from, to: a.to }, { from: b.from, to: b.to }], userEvent: 'input.format' }
    }
  }
  const { from, to } = state.selection.main
  const len = marker.length
  if (from === to) return { changes: { from, insert: marker + marker }, selection: EditorSelection.cursor(from + len), userEvent: 'input.format' }
  return { changes: [{ from, insert: marker }, { from: to, insert: marker }], selection: EditorSelection.single(from + len, to + len), userEvent: 'input.format' }
}

function toggleLink(state: EditorState): TransactionSpec {
  const link = enclosing(state, 'Link')
  if (link) {
    const marks: SyntaxNode[] = []
    for (let c = link.firstChild; c; c = c.nextSibling) if (c.name === 'LinkMark') marks.push(c)
    if (marks.length >= 2) return { changes: { from: link.from, to: link.to, insert: state.sliceDoc(marks[0].to, marks[1].from) }, userEvent: 'input.format' }
  }
  const { from, to } = state.selection.main
  const words = state.sliceDoc(from, to)
  const insert = `[${words}](url)`
  const url = from + words.length + 3
  return { changes: { from, to, insert }, selection: from === to ? EditorSelection.cursor(from + 1) : EditorSelection.single(url, url + 3), userEvent: 'input.format' }
}

function toggleHeading(state: EditorState): TransactionSpec {
  const line = state.doc.lineAt(state.selection.main.head)
  const m = ATX.exec(line.text)
  if (m && m[2].length === 2) return { changes: { from: line.from + m[1].length, to: line.from + m[0].length }, userEvent: 'input.format' }
  if (m) return { changes: { from: line.from + m[1].length, to: line.from + m[0].length, insert: '## ' }, userEvent: 'input.format' }
  return { changes: { from: line.from, insert: '## ' }, userEvent: 'input.format' }
}

function toggleList(state: EditorState): TransactionSpec {
  const { from, to } = state.selection.main
  const first = state.doc.lineAt(from).number
  const last = state.doc.lineAt(to).number
  const lines = []
  for (let n = first; n <= last; n++) lines.push(state.doc.line(n))
  const filled = lines.filter((l) => l.text.trim() !== '')
  const all = filled.length > 0 && filled.every((l) => BULLET.test(l.text))
  const changes = filled.flatMap((l): ChangeSpec[] => {
    const m = BULLET.exec(l.text)
    if (all && m) return [{ from: l.from + m[1].length, to: l.from + m[0].length }]
    if (!all && !m) return [{ from: l.from + (/^\s*/.exec(l.text)?.[0].length ?? 0), insert: '- ' }]
    return []
  })
  if (!changes.length && !filled.length) {
    const line = state.doc.lineAt(from)
    return { changes: { from: line.from, insert: '- ' }, userEvent: 'input.format' }
  }
  return { changes, userEvent: 'input.format' }
}

/** What a toolbar button (or its key) does to the text; null when the file is read only. */
export function formatSpec(state: EditorState, id: FormatId): TransactionSpec | null {
  if (state.readOnly) return null
  if (id === 'bold' || id === 'italic' || id === 'code') return toggleSpan(state, id)
  if (id === 'link') return toggleLink(state)
  if (id === 'heading') return toggleHeading(state)
  return toggleList(state)
}
