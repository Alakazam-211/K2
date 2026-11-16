// The / menu in Write mode (build-editors.md E3; editors-notes.md §2): type
// "/" on an empty line and a list of blocks opens under it; keep typing to
// filter ("/ta" leaves Task list and Table); ↑ ↓ choose, Enter or Tab puts the
// block's markdown in place of the "/…", Esc closes it and leaves what you
// typed. Moving off the line, or deleting the "/", closes it too.
//
// The state is a CodeMirror field and the keys a keymap of the view, so they
// only ever act inside this editor: they never reach the Thread composer's
// own / menu, a different component. The list itself is
// components/Editor/tools/SlashMenu (it reads this field).

import { EditorSelection, Prec, StateEffect, StateField, type EditorState, type Extension, type TransactionSpec } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'

export interface SlashBlock {
  id: string
  label: string
  /** The markdown it types, shown as the hint. */
  hint: string
  /** Other words it answers to. */
  words: string[]
}

export const SLASH_BLOCKS: readonly SlashBlock[] = [
  { id: 'heading', label: 'Heading', hint: '#', words: ['title', 'h1', 'h2', 'h3'] },
  { id: 'bullet', label: 'Bulleted list', hint: '-', words: ['ul', 'unordered', 'bullet'] },
  { id: 'task', label: 'Task list', hint: '[ ]', words: ['todo', 'checkbox', 'check'] },
  { id: 'quote', label: 'Quote', hint: '>', words: ['blockquote', 'cite'] },
  { id: 'code', label: 'Code block', hint: '```', words: ['fence', 'snippet', 'pre'] },
  { id: 'table', label: 'Table', hint: '|', words: ['grid', 'columns'] },
  { id: 'divider', label: 'Divider', hint: '---', words: ['hr', 'rule', 'line', 'separator'] },
  { id: 'image', label: 'Image', hint: '![', words: ['img', 'picture', 'photo'] },
]

export function filterBlocks(query: string): SlashBlock[] {
  const q = query.trim().toLowerCase()
  if (!q) return [...SLASH_BLOCKS]
  return SLASH_BLOCKS.filter((b) => b.label.toLowerCase().includes(q) || b.words.some((w) => w.startsWith(q)))
}

export interface SlashState {
  /** Where the "/" is (its line's start). */
  from: number
  /** What's typed after the "/". */
  query: string
  /** The chosen row among the filtered blocks. */
  active: number
}

interface Field {
  open: SlashState | null
  /** The start of a line where Esc closed it: it stays closed while the caret is on that line. */
  dismissed: number | null
}

export const closeSlash = StateEffect.define<null>()
const setActive = StateEffect.define<number>()

function onLine(state: EditorState, from: number): boolean {
  const sel = state.selection.main
  const line = state.doc.lineAt(from)
  return line.from === from && sel.empty && sel.head > from && sel.head <= line.to && line.text.startsWith('/') && !line.text.includes('\n')
}

const slashField = StateField.define<Field>({
  create: () => ({ open: null, dismissed: null }),
  update(f, tr) {
    let { open, dismissed } = f
    if (tr.docChanged) {
      if (open) open = { ...open, from: tr.changes.mapPos(open.from, -1) }
      if (dismissed !== null) dismissed = tr.changes.mapPos(dismissed, -1)
    }
    const state = tr.state
    const head = state.selection.main.head
    if (dismissed !== null && state.doc.lineAt(head).from !== dismissed) dismissed = null
    for (const e of tr.effects) {
      if (e.is(closeSlash)) {
        if (open) dismissed = open.from
        open = null
      } else if (e.is(setActive) && open) open = { ...open, active: e.value }
    }
    if (open) {
      if (!onLine(state, open.from)) open = null
      else {
        const query = state.doc.lineAt(open.from).text.slice(1)
        if (query !== open.query) open = { ...open, query, active: 0 }
      }
    } else if (tr.docChanged && tr.isUserEvent('input.type')) {
      const line = state.doc.lineAt(head)
      const sel = state.selection.main
      if (sel.empty && line.text === '/' && head === line.to && dismissed !== line.from) open = { from: line.from, query: '', active: 0 }
    }
    return open === f.open && dismissed === f.dismissed ? f : { open, dismissed }
  },
})

export const slashState = (state: EditorState): SlashState | null => state.field(slashField, false)?.open ?? null

/** The markdown a block puts in place of the "/…" line, and where the caret goes in it. */
function blockText(state: EditorState, from: number, id: string): { insert: string; caret: number } {
  switch (id) {
    case 'heading':
      return { insert: '## ', caret: 3 }
    case 'bullet':
      return { insert: '- ', caret: 2 }
    case 'task':
      return { insert: '- [ ] ', caret: 6 }
    case 'quote':
      return { insert: '> ', caret: 2 }
    case 'code':
      return { insert: '```\n\n```', caret: 4 }
    case 'table': {
      const insert = '| Column | Column |\n| --- | --- |\n|  |  |'
      return { insert, caret: insert.lastIndexOf('|  |') + 2 }
    }
    case 'divider': {
      // "---" under a paragraph would make it a heading: keep a blank line before it.
      const line = state.doc.lineAt(from)
      const above = line.number > 1 ? state.doc.line(line.number - 1).text : ''
      const insert = `${above.trim() ? '\n' : ''}---\n`
      return { insert, caret: insert.length }
    }
    case 'image':
      return { insert: '![]()', caret: 2 }
    default:
      return { insert: '', caret: 0 }
  }
}

/** Put the block in place of the "/…" (one edit of yours). */
export function insertBlockSpec(state: EditorState, s: SlashState, id: string): TransactionSpec {
  const line = state.doc.lineAt(s.from)
  const { insert, caret } = blockText(state, s.from, id)
  return { changes: { from: line.from, to: line.to, insert }, selection: EditorSelection.cursor(line.from + caret), effects: closeSlash.of(null), userEvent: 'input.slash', scrollIntoView: true }
}

function move(view: EditorView, by: number): boolean {
  const s = slashState(view.state)
  if (!s) return false
  const n = filterBlocks(s.query).length
  if (!n) return true
  view.dispatch({ effects: setActive.of((s.active + by + n) % n) })
  return true
}

function pick(view: EditorView): boolean {
  const s = slashState(view.state)
  if (!s) return false
  const block = filterBlocks(s.query)[s.active]
  if (!block) return false
  view.dispatch(insertBlockSpec(view.state, s, block.id))
  return true
}

/** The field, its keys (ahead of every other keymap) and the editor's combobox attributes. */
export function slashExtension(ids = 'ed-slash'): Extension {
  return [
    slashField,
    Prec.highest(
      keymap.of([
        { key: 'ArrowDown', run: (v) => move(v, 1) },
        { key: 'ArrowUp', run: (v) => move(v, -1) },
        { key: 'Enter', run: pick },
        { key: 'Tab', run: pick },
        { key: 'Escape', run: (v) => (slashState(v.state) ? (v.dispatch({ effects: closeSlash.of(null) }), true) : false), stopPropagation: true },
      ]),
    ),
    EditorView.contentAttributes.compute([slashField], (state) => {
      const s = slashState(state)
      if (!s) return {}
      const block = filterBlocks(s.query)[s.active]
      return { 'aria-haspopup': 'listbox', 'aria-expanded': 'true', 'aria-controls': `${ids}-menu`, ...(block ? { 'aria-activedescendant': `${ids}-${block.id}` } : {}) }
    }),
  ]
}

/** Pick a block from the list with the pointer. */
export function pickBlock(view: EditorView, id: string): void {
  const s = slashState(view.state)
  if (s) view.dispatch(insertBlockSpec(view.state, s, id))
  view.focus()
}
