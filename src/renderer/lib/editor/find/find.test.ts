// @vitest-environment jsdom
// Find and replace in the file editors (build-editors.md E3; editors-notes.md
// §2 Find, v3.2): the count reads "3 of 12", Previous and Next wrap, Aa, W and
// .* change what matches, Replace replaces the match you're on and moves to
// the next, and Replace all is ONE edit of yours, so one Undo reverses it
// (CodeMirror's history here; the live doc's server undo in
// components/LiveEditor/test/editor-replace-all.test.ts). A held range takes a
// replace like any typing: holds never refuse a person.

import { describe, expect, it } from 'vitest'
import { EditorSelection, EditorState, type Transaction } from '@codemirror/state'
import { history, undo } from '@codemirror/commands'
import { holdField, holdsIn, setHolds } from '@/lib/editor/live/hold-field'
import { search, setSearchQuery } from '@codemirror/search'
import { countWords, findCount, findStep, replaceAllSpec, replaceOneSpec, toQuery, type FindSpec } from './find'
import { findMarks, matchMarks, showMatchesEffect } from './find-view'

const DOC = ['# Harbor', '', 'A draft, then a draft, then the Draft.', '', 'drafts and redraft', 'draft'].join('\n')
const spec = (search: string, o: Partial<FindSpec> = {}): FindSpec => ({ search, caseSensitive: false, wholeWord: false, regexp: false, replace: '', ...o })
const at = (doc: string, needle: string, nth = 0): number => {
  let i = -1
  for (let n = 0; n <= nth; n++) i = doc.indexOf(needle, i + 1)
  return i
}
const state = (doc = DOC, sel?: [number, number]): EditorState =>
  EditorState.create({ doc, selection: sel ? EditorSelection.single(sel[0], sel[1]) : undefined, extensions: [history()] })

describe('the count', () => {
  it('counts every match and says which one the selection is on: "3 of 6"', () => {
    const third = at(DOC, 'Draft')
    const c = findCount(state(DOC, [third, third + 5]), spec('draft'))
    expect(c).toEqual({ total: 6, current: 3, invalid: false, capped: false, empty: false })
    expect(countWords(c)).toBe('3 of 6')
  })

  it('with the selection off every match it says how many: "6 matches"', () => {
    const c = findCount(state(DOC, [0, 0]), spec('draft'))
    expect(c.current).toBe(0)
    expect(countWords(c)).toBe('6 matches')
    expect(countWords(findCount(state('one draft'), spec('draft')))).toBe('1 match')
  })

  it('"No matches" when nothing matches, and an empty field counts nothing', () => {
    expect(countWords(findCount(state(), spec('deadline')))).toBe('No matches')
    const empty = findCount(state(), spec(''))
    expect(empty.total).toBe(0)
    expect(countWords(empty)).toBe('')
  })

  it('Aa matches case; W whole words only; .* a regular expression', () => {
    expect(findCount(state(), spec('Draft', { caseSensitive: true })).total).toBe(1)
    // "drafts" and "redraft" are not whole words "draft".
    expect(findCount(state(), spec('draft', { wholeWord: true })).total).toBe(4)
    expect(findCount(state(), spec('(re)?draft', { regexp: true })).total).toBe(6)
    expect(findCount(state(), spec('^draft$', { regexp: true })).total).toBe(1)
  })

  it('a bad regular expression says so, and matches nothing', () => {
    const c = findCount(state(), spec('(draft', { regexp: true }))
    expect(c).toMatchObject({ total: 0, invalid: true })
    expect(countWords(c)).toBe('Not a valid pattern')
  })

  it('without .* the text is literal: a backslash-n is two characters, not a line break', () => {
    expect(findCount(state('a\\nb\nc'), spec('\\n')).total).toBe(1)
  })

  it('counts past 10,000 say so instead of counting for ever', () => {
    const c = findCount(state('a '.repeat(10_050)), spec('a'))
    expect(c.capped).toBe(true)
    expect(countWords(c)).toBe('10,000+ matches')
  })
})

describe('Previous and Next', () => {
  it('Next goes to the first match after the caret, then wraps to the top', () => {
    const s = state(DOC, [at(DOC, 'redraft'), at(DOC, 'redraft')])
    const next = findStep(s, spec('draft'), 1)!
    expect(next.from).toBe(at(DOC, 'redraft') + 2)
    const last = DOC.lastIndexOf('draft')
    const s2 = state(DOC, [last, last + 5])
    expect(findStep(s2, spec('draft'), 1)!.from).toBe(at(DOC, 'draft'))
  })

  it('Previous goes back, and wraps to the end', () => {
    const first = at(DOC, 'draft')
    expect(findStep(state(DOC, [first, first + 5]), spec('draft'), -1)!.from).toBe(DOC.lastIndexOf('draft'))
  })

  it('no match, no step', () => {
    expect(findStep(state(), spec('deadline'), 1)).toBeNull()
  })
})

describe('Replace', () => {
  it('replaces the match the selection is on and selects the next', () => {
    const first = at(DOC, 'draft')
    const s = state(DOC, [first, first + 5])
    const tr = s.update(replaceOneSpec(s, spec('draft', { replace: 'sketch' }))!)
    expect(tr.state.doc.toString()).toContain('A sketch, then a draft')
    const sel = tr.state.selection.main
    expect(tr.state.sliceDoc(sel.from, sel.to)).toBe('draft')
    expect(sel.from).toBe(at(tr.state.doc.toString(), 'draft'))
  })

  it('off a match, Replace only moves to the next one (nothing changes yet)', () => {
    const s = state(DOC, [0, 0])
    const tr = s.update(replaceOneSpec(s, spec('draft', { replace: 'sketch' }))!)
    expect(tr.docChanged).toBe(false)
    expect(tr.state.selection.main.from).toBe(at(DOC, 'draft'))
  })

  it('.* replacements use $1 groups', () => {
    const s = state('Ln 12, Col 4', [0, 0])
    const tr = s.update(replaceAllSpec(s, spec('(\\d+)', { regexp: true, replace: '[$1]' }))!)
    expect(tr.state.doc.toString()).toBe('Ln [12], Col [4]')
  })

  it('Replace all is one transaction, one undo step, separate from the typing just before it', () => {
    let s = state('draft', [5, 5])
    // You type, then at once Replace all: one Undo takes back only the replace.
    s = s.update({ changes: { from: 5, insert: ' draft' }, selection: { anchor: 11 }, userEvent: 'input.type' }).state
    const before = s.doc.toString()
    const tr: Transaction = s.update(replaceAllSpec(s, spec('draft', { replace: 'sketch' }))!)
    expect(tr.state.doc.toString()).toBe('sketch sketch')
    expect(tr.isUserEvent('input.replace.all')).toBe(true)
    s = tr.state
    let undone = s
    undo({ state: s, dispatch: (t) => void (undone = t.state) })
    expect(undone.doc.toString()).toBe(before)
  })

  it('Replace all in a read-only file does nothing', () => {
    const s = EditorState.create({ doc: DOC, extensions: [EditorState.readOnly.of(true)] })
    expect(replaceAllSpec(s, spec('draft', { replace: 'x' }))).toBeNull()
    expect(replaceOneSpec(s, spec('draft', { replace: 'x' }))).toBeNull()
  })

  it('a held range takes the replace (holds never refuse a person), and the hold keeps its lines', () => {
    const doc = ['# Harbor', '', '## Setup', '', '1. Clone the draft.', '2. Open the draft.', '', '## Notes', 'A draft.'].join('\n')
    let s = EditorState.create({ doc, extensions: [holdField] })
    s = s.update({ effects: setHolds.of([{ who: 'atlas', claim: 'c1', name: 'Atlas', agent: true, slot: 1, first: 3, last: 6, at: 0, note: 'rewriting Setup', hold: true }]) }).state
    const tr = s.update(replaceAllSpec(s, spec('draft', { replace: 'sketch' }))!)
    expect(tr.state.doc.toString().match(/sketch/g)).toHaveLength(3)
    const [h] = holdsIn(tr.state)
    expect(tr.state.doc.lineAt(h.from).number).toBe(3)
    expect(tr.state.doc.lineAt(h.to).number).toBe(6)
    expect(tr.state.sliceDoc(h.from, h.to)).toContain('2. Open the sketch.')
  })
})

describe('the marks', () => {
  it('Write splits the view into several ranges: each match is marked once, in order (the builder never throws)', () => {
    let s = EditorState.create({ doc: DOC, extensions: [findMarks, search()] })
    s = s.update({ effects: [setSearchQuery.of(toQuery(spec('draft'))), showMatchesEffect(true)] }).state
    const set = matchMarks(s, [{ from: 0, to: 12 }, { from: 14, to: 30 }, { from: 31, to: DOC.length }])
    const seen: number[] = []
    set.between(0, DOC.length, (from) => void seen.push(from))
    expect(seen).toHaveLength(6)
    expect([...seen].sort((a, b) => a - b)).toEqual(seen)
  })

  it('no marks while the bar is closed', () => {
    let s = EditorState.create({ doc: DOC, extensions: [findMarks, search()] })
    s = s.update({ effects: setSearchQuery.of(toQuery(spec('draft'))) }).state
    expect(matchMarks(s, [{ from: 0, to: DOC.length }]).size).toBe(0)
  })
})
