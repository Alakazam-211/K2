// Find and replace for the file editors (build-editors.md E3; editors-notes.md
// §2 Find, v3.2): one query (the field, Aa, W and .*) over CodeMirror's own
// search (`SearchQuery`), on whichever view the frame has mounted (Write,
// Read, Source, code, a live doc). Pure: each function reads an EditorState
// and returns what to dispatch. The bar is components/Editor/find/FindBar.
//
// Replace all is ONE transaction marked `isolateHistory`, so it is one undo
// step of yours: CodeMirror's history keeps it apart from the typing around
// it, and the live editor's binding starts a fresh undo group for it
// (components/LiveEditor/binding.ts), so the server's ⌘Z reverses it whole.
// A held range takes it like any typing (holds never refuse a person).

import { EditorSelection, type EditorState, type TransactionSpec } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { isolateHistory } from '@codemirror/commands'
import { SearchQuery } from '@codemirror/search'

export interface FindSpec {
  search: string
  caseSensitive: boolean
  wholeWord: boolean
  regexp: boolean
  replace: string
}

export const EMPTY_FIND: FindSpec = { search: '', caseSensitive: false, wholeWord: false, regexp: false, replace: '' }

/** Past this many the count stops ("10,000+ matches"). */
export const COUNT_LIMIT = 10_000

export interface FindCount {
  total: number
  /** Which match the selection is on (1-based), 0 when it's on none. */
  current: number
  /** A regular expression that doesn't parse. */
  invalid: boolean
  /** More than COUNT_LIMIT. */
  capped: boolean
  /** Nothing typed yet: no count at all. */
  empty: boolean
}

/** What you type is what's found (literal), unless .* is on. */
export const toQuery = (s: FindSpec): SearchQuery => new SearchQuery({ search: s.search, caseSensitive: s.caseSensitive, wholeWord: s.wholeWord, regexp: s.regexp, literal: !s.regexp, replace: s.replace })

export function findCount(state: EditorState, s: FindSpec): FindCount {
  const q = toQuery(s)
  if (!s.search) return { total: 0, current: 0, invalid: false, capped: false, empty: true }
  if (!q.valid) return { total: 0, current: 0, invalid: true, capped: false, empty: false }
  const { from, to } = state.selection.main
  const cursor = q.getCursor(state)
  let total = 0
  let current = 0
  for (let r = cursor.next(); !r.done; r = cursor.next()) {
    if (total === COUNT_LIMIT) return { total, current, invalid: false, capped: true, empty: false }
    total++
    if (r.value.from === from && r.value.to === to) current = total
  }
  return { total, current, invalid: false, capped: false, empty: false }
}

/** The count in words: "3 of 12", "12 matches", "No matches". Empty for an empty field. */
export function countWords(c: FindCount): string {
  if (c.empty) return ''
  if (c.invalid) return 'Not a valid pattern'
  if (c.capped) return `${COUNT_LIMIT.toLocaleString('en-US')}+ matches`
  if (c.total === 0) return 'No matches'
  if (c.current) return `${c.current} of ${c.total}`
  return `${c.total} ${c.total === 1 ? 'match' : 'matches'}`
}

/** The next (1) or previous (-1) match from the selection, wrapping; null when there's none. */
export function findStep(state: EditorState, s: FindSpec, dir: 1 | -1): { from: number; to: number } | null {
  const q = toQuery(s)
  if (!s.search || !q.valid) return null
  const { from, to } = state.selection.main
  if (dir === 1) {
    const c = q.getCursor(state, to)
    for (let r = c.next(); !r.done; r = c.next()) if (r.value.from !== from || r.value.to !== to) return r.value
    const w = q.getCursor(state, 0, Math.min(state.doc.length, to))
    const r = w.next()
    return r.done ? null : r.value
  }
  let last: { from: number; to: number } | null = null
  const c = q.getCursor(state, 0, from)
  for (let r = c.next(); !r.done; r = c.next()) if (r.value.to <= from || r.value.from < from) last = r.value
  if (last && !(last.from === from && last.to === to)) return last
  const w = q.getCursor(state, from)
  for (let r = w.next(); !r.done; r = w.next()) if (r.value.from !== from || r.value.to !== to) last = r.value
  return last
}

/** Select a match and bring it into view, as a search step (not an edit). */
export const selectMatch = (m: { from: number; to: number }): TransactionSpec => ({
  selection: EditorSelection.single(m.from, m.to),
  effects: EditorView.scrollIntoView(m.from, { y: 'center' }),
  userEvent: 'select.search',
})

/** The first match at or after `pos` (wrapping): where typing in the field lands. */
export function firstFrom(state: EditorState, s: FindSpec, pos: number): { from: number; to: number } | null {
  const q = toQuery(s)
  if (!s.search || !q.valid) return null
  const c = q.getCursor(state, pos)
  const r = c.next()
  if (!r.done) return r.value
  const w = q.getCursor(state).next()
  return w.done ? null : w.value
}

/** The replacement text for one match (a regular expression's $1 groups filled in). */
function replacementFor(state: EditorState, s: FindSpec, m: { from: number; to: number }): string {
  if (!s.regexp) return s.replace
  const re = new RegExp(s.search, s.caseSensitive ? 'u' : 'iu')
  const found = re.exec(state.sliceDoc(m.from, m.to))
  if (!found) return s.replace
  return s.replace.replace(/\$([$&]|\d+)/g, (all, i: string) => (i === '$' ? '$' : i === '&' ? found[0] : (found[Number(i)] ?? all)))
}

/** Replace the match the selection is on and select the next; off a match, only move to the next. */
export function replaceOneSpec(state: EditorState, s: FindSpec): TransactionSpec | null {
  if (state.readOnly) return null
  const q = toQuery(s)
  if (!s.search || !q.valid) return null
  const { from, to } = state.selection.main
  const c = q.getCursor(state, from)
  const r = c.next()
  const on = !r.done && r.value.from === from && r.value.to === to
  if (!on) {
    const next = findStep(state, s, 1)
    return next ? selectMatch(next) : null
  }
  const insert = replacementFor(state, s, { from, to })
  const changes = state.changes({ from, to, insert })
  const after = state.update({ changes }).state
  const next = firstFrom(after, s, from + insert.length)
  return {
    changes,
    ...(next ? { selection: EditorSelection.single(next.from, next.to), effects: EditorView.scrollIntoView(next.from, { y: 'center' }) } : { selection: EditorSelection.cursor(from + insert.length) }),
    userEvent: 'input.replace',
    annotations: isolateHistory.of('full'),
  }
}

/** Every match replaced, as one edit of yours (one undo step); null when nothing matches. */
export function replaceAllSpec(state: EditorState, s: FindSpec): TransactionSpec | null {
  if (state.readOnly) return null
  const q = toQuery(s)
  if (!s.search || !q.valid) return null
  const changes: { from: number; to: number; insert: string }[] = []
  const c = q.getCursor(state)
  for (let r = c.next(); !r.done; r = c.next()) changes.push({ from: r.value.from, to: r.value.to, insert: replacementFor(state, s, r.value) })
  if (!changes.length) return null
  return {
    changes,
    effects: EditorView.announce.of(`Replaced ${changes.length} ${changes.length === 1 ? 'match' : 'matches'}.`),
    userEvent: 'input.replace.all',
    annotations: isolateHistory.of('full'),
  }
}
