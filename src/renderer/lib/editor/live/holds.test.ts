// @vitest-environment jsdom

// Holds on a live text from the kit's claim events (build-editors.md E4b;
// editors-notes.md §2 Holds, v3.1, v3.2). Pinned here:
//   - claim_started on lines becomes a hold with the holder's name, kind and
//     slot, its lines, its note; claim_ended removes it; yours aren't drawn;
//   - an older host's claim (no actor) takes the name from the roster;
//   - the label: "<name> is <note>" for an -ing note, else "<name> is working
//     on <section>"; the line under your caret says you can still type;
//   - a holder who left the file loses their holds;
//   - the CodeMirror field: lines become a range that maps through edits
//     (typing at either end stays inside), the label sits above the range as
//     its own block (never over text), and the line under the caret shows only
//     while your caret is inside the range.

import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { holdHint, holdLabel, holdsReducer, pruneHolds, type Hold } from './holds'
import { heldAt, holdsExtension, holdsIn, setHolds } from './hold-field'

const DOC = ['# Harbor', '', 'Intro.', '', '## Setup', '', '1. Clone it.', '2. Open it.', '', '## Configuration', '', 'A table.'].join('\n')
const people = [
  { id: 'me', name: 'You', kind: 'person' as const, self: true },
  { id: 'atlas', name: 'Atlas', kind: 'agent' as const },
  { id: 'sam', name: 'Sam', kind: 'person' as const },
]
const ctx = { self: 'me', people, slot: (id: string) => (id === 'atlas' ? 1 : 2) as 1 | 2 }
const started = (who: string, first: number, last: number, o: Record<string, unknown> = {}) => ({ kind: 'claim_started', who, claim: `c-${who}`, lines: { root: 'source', first, last }, version: 3, note: null, hold: true, ...o })

describe('holdsReducer', () => {
  it('turns claim_started into a hold and claim_ended removes it', () => {
    let h = holdsReducer([], started('atlas', 5, 8, { note: 'rewriting Setup', actor: { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' } }), ctx)
    expect(h).toEqual([{ claim: 'c-atlas', who: 'atlas', name: 'Atlas', agent: true, slot: 1, first: 5, last: 8, note: 'rewriting Setup', hold: true }])
    h = holdsReducer(h, { kind: 'claim_ended', who: 'atlas', claim: 'c-atlas', how: 'released', version: 4 }, ctx)
    expect(h).toEqual([])
  })

  it('takes the name from the roster without an actor, and leaves your own claims out', () => {
    const h = holdsReducer([], started('sam', 10, 12), ctx)
    expect(h[0]).toMatchObject({ who: 'sam', name: 'Sam', agent: false, slot: 2, note: null })
    expect(holdsReducer(h, started('me', 1, 2), ctx)).toBe(h)
  })

  it('replaces a claim taken again (new lines) and ignores other kinds', () => {
    const a = holdsReducer([], started('atlas', 5, 8), ctx)
    const b = holdsReducer(a, started('atlas', 5, 9), ctx)
    expect(b).toHaveLength(1)
    expect(b[0].last).toBe(9)
    expect(holdsReducer(b, { kind: 'cursor', who: 'atlas' }, ctx)).toBe(b)
  })

  it('drops the holds of someone who left', () => {
    const h = holdsReducer(holdsReducer([], started('atlas', 5, 8), ctx), started('sam', 10, 12), ctx)
    expect(pruneHolds(h, ['me', 'sam']).map((x) => x.who)).toEqual(['sam'])
    expect(pruneHolds(h, ['me', 'sam', 'atlas'])).toBe(h)
  })
})

describe('the words', () => {
  const hold: Hold = { claim: 'c', who: 'atlas', name: 'Atlas', agent: true, slot: 1, first: 5, last: 8, note: 'rewriting Setup', hold: true }
  it('says what the holder is doing', () => {
    expect(holdLabel(hold, 'Setup')).toBe('Atlas is rewriting Setup')
    expect(holdLabel({ ...hold, note: 'Restructure' }, 'Setup')).toBe('Atlas is working on Setup')
    expect(holdLabel({ ...hold, note: null }, null)).toBe('Atlas is working on lines 5–8')
  })
  it('tells you that you can still type', () => {
    expect(holdHint(hold, 'Setup')).toBe('Atlas is rewriting Setup. You can still type here; your words go in now, and Atlas sees them and keeps them.')
  })
})

describe('the hold field', () => {
  const hold: Hold = { claim: 'c-atlas', who: 'atlas', name: 'Atlas', agent: true, slot: 1, first: 5, last: 8, note: 'rewriting Setup', hold: true }
  const make = (sel = 0) => {
    const state = EditorState.create({ doc: DOC, selection: { anchor: sel }, extensions: holdsExtension({ onMessage: () => undefined, section: () => 'Setup' }) })
    return state.update({ effects: setHolds.of([hold]) }).state
  }

  it('holds lines first..last as a range, mapped through edits at either end', () => {
    let s = make()
    const [h] = holdsIn(s)
    expect(s.sliceDoc(h.from, h.to)).toBe('## Setup\n\n1. Clone it.\n2. Open it.')
    s = s.update({ changes: { from: h.from, insert: 'X' } }).state
    s = s.update({ changes: { from: holdsIn(s)[0].to, insert: 'Y' } }).state
    const [m] = holdsIn(s)
    expect(s.sliceDoc(m.from, m.to)).toBe('X## Setup\n\n1. Clone it.\n2. Open it.Y')
    expect(heldAt(s, m.from + 3)?.who).toBe('atlas')
    expect(heldAt(s, 0)).toBeNull()
  })

  it('draws the label as its own block above the range, and the line under your caret only inside it', () => {
    const view = new EditorView({ state: make(DOC.indexOf('Clone')) })
    const label = view.dom.querySelector('[data-testid="ed-hold-label"]') as HTMLElement
    expect(label.textContent).toContain('Atlas is rewriting Setup')
    expect(label.closest('.cm-line')).toBeNull()
    expect(label.querySelector('button')?.getAttribute('aria-label')).toBe('Message Atlas')
    expect(view.dom.querySelector('[data-testid="ed-hold-hint"]')?.textContent).toBe('Atlas is rewriting Setup. You can still type here; your words go in now, and Atlas sees them and keeps them.')
    view.dispatch({ selection: { anchor: DOC.indexOf('A table') } })
    expect(view.dom.querySelector('[data-testid="ed-hold-hint"]')).toBeNull()
    view.dispatch({ effects: setHolds.of([]) })
    expect(view.dom.querySelector('[data-testid="ed-hold-label"]')).toBeNull()
    view.destroy()
  })
})
