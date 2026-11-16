// @vitest-environment jsdom
// Write mode's keystroke budget (build-editors.md §6): on a 10,000-line file,
// a keystroke's update stays near 8 ms. jsdom has no layout and a slow DOM, so
// the whole-view number overstates a browser's; the decoration-only number is
// the part Write adds. Both print as MEASURE lines; the asserts are loose so a
// busy runner doesn't fail them. The real check is the WebKit pass on z3mbpZ.

import { describe, expect, it } from 'vitest'
import { EditorSelection, EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { ensureSyntaxTree, syntaxHighlighting, HighlightStyle } from '@codemirror/language'
import { FOLLOW_APP_HIGHLIGHT } from '../theme'
import { gfmMarkdown } from './language'
import { writeMode } from '.'
import { buildInline } from './inline'

/** A 10,000-line file with every construct Write draws. */
export function bigMarkdown(lines = 10_000): string {
  const out: string[] = ['# Studio handbook', '']
  for (let s = 1; out.length < lines; s++) {
    out.push(`## ${s}. Section ${s}`, '')
    out.push(`A draft moves through the **brief**, the *outline* and \`two passes\` before [a person](docs/people.md) reads it. Every pass leaves a note.`, '')
    out.push('- Step one', '  - A nested step', '- [ ] Check the sources', '- [x] Write the outline', '1. First', '2. Second', '')
    out.push('> Quill never publishes on its own.', '')
    out.push('| Setting | Default |', '|---|---|', '| `tone` | plain |', '| maxWords | 1800 |', '')
    out.push('```ts', 'export const config = { tone: "plain", maxWords: 1800 }', '```', '')
    out.push('![Outline](img/outline.png)', '', '---', '')
  }
  return out.slice(0, lines).join('\n')
}

const pct = (xs: number[], p: number): number => {
  const s = [...xs].sort((a, b) => a - b)
  return Math.round(s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] * 100) / 100
}

function timeTyping(withWrite: boolean, keys = 200): { p50: number; p95: number; max: number; drawn: number } {
  const doc = bigMarkdown()
  const mid = doc.indexOf('A draft moves', Math.floor(doc.length / 2))
  const state = EditorState.create({
    doc,
    selection: EditorSelection.cursor(mid),
    extensions: [gfmMarkdown(), syntaxHighlighting(HighlightStyle.define(FOLLOW_APP_HIGHLIGHT)), withWrite ? writeMode('write', { filePath: '/ws/handbook.md' }) : []],
  })
  ensureSyntaxTree(state, doc.length, 10_000)
  const view = new EditorView({ state, parent: document.body })
  view.dispatch({ effects: EditorView.scrollIntoView(mid, { y: 'center' }) })
  // The caret's part of the file is the drawn part.
  expect(view.viewport.from <= mid && mid <= view.viewport.to).toBe(true)
  if (withWrite) view.dispatch({ effects: view.state.facet(EditorView.focusChangeEffect).map((f) => f(view.state, true)!).filter(Boolean) })
  const times: number[] = []
  const drawn = view.contentDOM.querySelectorAll('.cm-line').length
  for (let i = 0; i < keys; i++) {
    const at = view.state.selection.main.head
    const t0 = performance.now()
    view.dispatch({ changes: { from: at, insert: i % 7 === 6 ? ' ' : 'x' }, selection: { anchor: at + 1 }, userEvent: 'input.type' })
    times.push(performance.now() - t0)
  }
  view.destroy()
  // Warm-up: the first keystrokes compile and allocate.
  const warm = times.slice(20)
  return { p50: pct(warm, 50), p95: pct(warm, 95), max: Math.round(Math.max(...warm) * 100) / 100, drawn }
}

describe('keystroke to update on a 10,000-line file (budget 8 ms)', () => {
  it('measures the whole view update with and without Write', () => {
    const plain = timeTyping(false)
    const write = timeTyping(true)
    console.log(`MEASURE keystroke view-update 10k lines jsdom: source p50=${plain.p50}ms p95=${plain.p95}ms; write p50=${write.p50}ms p95=${write.p95}ms max=${write.max}ms (${write.drawn} lines drawn)`)
    expect(write.p95).toBeLessThan(100)
  })

  it('measures what Write adds per keystroke: the state update (parse, reveal, block map) and the visible decorations', () => {
    const doc = bigMarkdown()
    const mid = doc.indexOf('A draft moves', Math.floor(doc.length / 2))
    let state = EditorState.create({ doc, selection: EditorSelection.cursor(mid), extensions: [writeMode('write', { filePath: '/ws/handbook.md' })] })
    ensureSyntaxTree(state, doc.length, 10_000)
    // About a screen around the caret: 60 lines.
    const span = (s: EditorState): [number, number] => {
      const l = s.doc.lineAt(s.selection.main.head).number
      return [s.doc.line(Math.max(1, l - 30)).from, s.doc.line(Math.min(s.doc.lines, l + 30)).to]
    }
    const tState: number[] = []
    const tDeco: number[] = []
    for (let i = 0; i < 220; i++) {
      const at = state.selection.main.head
      const t0 = performance.now()
      state = state.update({ changes: { from: at, insert: 'x' }, selection: { anchor: at + 1 }, userEvent: 'input.type' }).state
      const t1 = performance.now()
      buildInline(state, span(state), false)
      const t2 = performance.now()
      tState.push(t1 - t0)
      tDeco.push(t2 - t1)
    }
    const total = tState.map((t, i) => t + tDeco[i]).slice(20)
    console.log(`MEASURE keystroke write-only 10k lines: state p50=${pct(tState.slice(20), 50)}ms p95=${pct(tState.slice(20), 95)}ms; decorations p50=${pct(tDeco.slice(20), 50)}ms p95=${pct(tDeco.slice(20), 95)}ms; total p50=${pct(total, 50)}ms p95=${pct(total, 95)}ms`)
    expect(pct(total, 95)).toBeLessThan(40)
  })
})
