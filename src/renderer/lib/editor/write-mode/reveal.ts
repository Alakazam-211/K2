// Which lines show their marks in Write mode (build-editors.md E2; the notes'
// per-line Pick). Every line a selection touches is revealed, whole, so a
// line's width changes once when the caret arrives and once when it leaves,
// never as the caret walks through it. Nothing is revealed while the editor
// is unfocused, in Read, or (frozen) while an IME composition is open: a
// replaced range that appears or vanishes under a composition breaks it
// (§6). The reveal thaws a moment after compositionend, once CodeMirror has
// flushed the composed text.

import { EditorState, StateEffect, StateField, type Extension, type Transaction } from '@codemirror/state'
import { EditorView, ViewPlugin } from '@codemirror/view'

/** A run of whole lines: `from` is a line start, `to` a line end. */
export interface LineSpan {
  from: number
  to: number
}

export interface RevealState {
  lines: readonly LineSpan[]
  focused: boolean
  composing: boolean
}

export const setComposing = StateEffect.define<boolean>()
const setFocused = StateEffect.define<boolean>()

/** The lines the selection touches, merged and in order. */
export function selectionLines(state: EditorState): LineSpan[] {
  const out: LineSpan[] = []
  const ranges = [...state.selection.ranges].sort((a, b) => a.from - b.from)
  for (const r of ranges) {
    const from = state.doc.lineAt(r.from).from
    const to = state.doc.lineAt(r.to).to
    const last = out[out.length - 1]
    if (last && from <= last.to + 1) last.to = Math.max(last.to, to)
    else out.push({ from, to })
  }
  return out
}

const sameSpans = (a: readonly LineSpan[], b: readonly LineSpan[]): boolean =>
  a.length === b.length && a.every((s, i) => s.from === b[i].from && s.to === b[i].to)

function mapSpans(spans: readonly LineSpan[], tr: Transaction): LineSpan[] {
  const doc = tr.state.doc
  return spans.map((s) => ({ from: doc.lineAt(tr.changes.mapPos(s.from, -1)).from, to: doc.lineAt(tr.changes.mapPos(s.to, 1)).to }))
}

export const revealField = StateField.define<RevealState>({
  create: () => ({ lines: [], focused: false, composing: false }),
  update(v, tr) {
    let { focused, composing } = v
    for (const e of tr.effects) {
      if (e.is(setFocused)) focused = e.value
      else if (e.is(setComposing)) composing = e.value
    }
    // A composed keystroke can reach the state before compositionstart's effect does.
    if (tr.isUserEvent('input.type.compose')) composing = true
    let lines: readonly LineSpan[] = tr.docChanged ? mapSpans(v.lines, tr) : v.lines
    if (!composing) lines = focused ? selectionLines(tr.state) : []
    if (sameSpans(lines, v.lines)) lines = v.lines
    if (lines === v.lines && focused === v.focused && composing === v.composing) return v
    return { lines, focused, composing }
  },
})

/** Is the line holding `pos` revealed? */
export function revealedAt(lines: readonly LineSpan[], pos: number): boolean {
  for (const s of lines) if (pos >= s.from && pos <= s.to) return true
  return false
}

/** Does any revealed line fall in [from, to]? */
export function revealedIn(lines: readonly LineSpan[], from: number, to: number): boolean {
  for (const s of lines) if (s.from <= to && s.to >= from) return true
  return false
}

/** How long after compositionend the reveal thaws (CodeMirror flushes within 50 ms). */
export const THAW_MS = 80

const compositionGuard = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | null = null
    destroy(): void {
      if (this.timer) clearTimeout(this.timer)
    }
  },
  {
    eventHandlers: {
      compositionstart(_e, view) {
        if (this.timer) clearTimeout(this.timer)
        this.timer = null
        // An effect only: no change, no selection, so the DOM under the IME is untouched.
        if (!view.state.field(revealField).composing) view.dispatch({ effects: setComposing.of(true) })
      },
      compositionend(_e, view) {
        if (this.timer) clearTimeout(this.timer)
        this.timer = setTimeout(() => {
          this.timer = null
          if (!view.composing && view.state.field(revealField).composing) view.dispatch({ effects: setComposing.of(false) })
        }, THAW_MS)
      },
    },
  },
)

/** The reveal for Write: the field, focus tracking and the composition guard. */
export function revealExtension(): Extension {
  return [revealField, EditorView.focusChangeEffect.of((_s, focusing) => setFocused.of(focusing)), compositionGuard]
}

/** Read: the field with nothing ever revealed. */
export const readReveal: Extension = revealField
