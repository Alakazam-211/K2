// @vitest-environment jsdom
// The kept pair's marks on a mounted editor: each side's lines tinted, a tag
// on each side's first line, moved by edits, gone when cleared.

import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { afterEach, describe, expect, it } from 'vitest'
import { applyKeptMarks, KEPT_THEIRS, KEPT_YOURS, keptLines } from './kept-marks'
import { merge3 } from './merge3'

let view: EditorView | null = null
afterEach(() => {
  view?.destroy()
  view = null
})

describe('kept marks', () => {
  const m = merge3('A\nB\nC\n', 'A\nB theirs\nC\n', 'A\nB mine\nB mine 2\nC\n')

  it('marks the pair the merge kept, yours then theirs, with a tag on each', () => {
    view = new EditorView({ state: EditorState.create({ doc: m.text }), parent: document.body })
    applyKeptMarks(view, m.kept)
    expect(keptLines(view)).toEqual([
      { line: 1, who: 'yours' },
      { line: 2, who: 'yours' },
      { line: 3, who: 'theirs' },
    ])
    // In the margin (inline, on the line) and on a row above (for no margin); CSS shows one.
    expect([...view.dom.querySelectorAll('.cm-line > .ed-kept-tag')].map((t) => t.textContent)).toEqual([KEPT_YOURS, KEPT_THEIRS])
    expect([...view.dom.querySelectorAll('.ed-kept-row > .ed-kept-tag')].map((t) => t.textContent)).toEqual([KEPT_YOURS, KEPT_THEIRS])
    expect(view.dom.querySelectorAll('.cm-line.ed-kept')).toHaveLength(3)
  })

  it('the marks move with an edit above them, and clear', () => {
    view = new EditorView({ state: EditorState.create({ doc: m.text }), parent: document.body })
    applyKeptMarks(view, m.kept)
    view.dispatch({ changes: { from: 0, insert: 'New first line\n' } })
    expect(keptLines(view).map((k) => k.line)).toEqual([2, 3, 4])
    applyKeptMarks(view, [])
    expect(keptLines(view)).toEqual([])
  })

  it('nothing kept on a view that never had marks: the view is left alone', () => {
    view = new EditorView({ state: EditorState.create({ doc: 'x' }), parent: document.body })
    const before = view.state
    applyKeptMarks(view, [])
    expect(view.state).toBe(before)
  })
})
