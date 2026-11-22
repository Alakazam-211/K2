// The kept pair's marks (notes v3.1, v3.2): after a merge kept both sides of
// a clash, each side's lines get a quiet warn tint and a bar in the margin,
// and its first line a tag in the right margin, where the name flags sit:
// "Kept both · yours" and "Kept both · saved outside K2". The marks move with
// edits (mapped through every change) and go when the notice does.
//
// Added to whatever editor view is mounted, without touching its setup
// (StateEffect.appendConfig), so the plain editor and E2's Write mode get the
// same marks.

import { StateEffect, StateField, type EditorState, type Extension, type Range } from '@codemirror/state'
import { Decoration, EditorView, WidgetType, type DecorationSet } from '@codemirror/view'
import type { KeptPair } from './merge3'
import { PRODUCT_NAME } from '@/lib/brand'

export const KEPT_YOURS = 'Kept both · yours'
export const KEPT_THEIRS = `Kept both · saved outside ${PRODUCT_NAME}`

class KeptTag extends WidgetType {
  constructor(readonly label: string) {
    super()
  }
  eq(other: KeptTag): boolean {
    return other.label === this.label
  }
  toDOM(): HTMLElement {
    const el = document.createElement('span')
    el.className = 'ed-kept-tag'
    el.setAttribute('data-k2-chamfer', 'chip')
    el.textContent = this.label
    return el
  }
  ignoreEvent(): boolean {
    return true
  }
}

/** The same tag on its own row above the side (no right margin: Source,
 *  code, a narrow tab), so it never covers text and the gutter stays aligned.
 *  CSS shows one form or the other. */
class KeptRow extends KeptTag {
  toDOM(): HTMLElement {
    const row = document.createElement('div')
    row.className = 'ed-kept-row'
    row.setAttribute('aria-hidden', 'true')
    row.appendChild(super.toDOM())
    return row
  }
}

export const setKeptMarks = StateEffect.define<KeptPair[]>()

function build(state: EditorState, pairs: KeptPair[]): DecorationSet {
  const out: Range<Decoration>[] = []
  const lines = state.doc.lines
  const side = (from: number, to: number, who: 'yours' | 'theirs'): void => {
    for (let n = from; n < to && n < lines; n++) {
      const line = state.doc.line(n + 1)
      out.push(Decoration.line({ class: 'ed-kept', attributes: { 'data-kept': who } }).range(line.from))
      if (n !== from) continue
      // The side's first line carries the tag: in the margin, or on a row above.
      const label = who === 'yours' ? KEPT_YOURS : KEPT_THEIRS
      out.push(Decoration.widget({ widget: new KeptRow(label), block: true, side: -1 }).range(line.from))
      out.push(Decoration.widget({ widget: new KeptTag(label), side: 1 }).range(line.from))
    }
  }
  for (const p of pairs) {
    side(p.yours.from, p.yours.to, 'yours')
    side(p.theirs.from, p.theirs.to, 'theirs')
  }
  return Decoration.set(out, true)
}

const keptField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    let next = deco.map(tr.changes)
    for (const e of tr.effects) if (e.is(setKeptMarks)) next = build(tr.state, e.value)
    return next
  },
  provide: (f) => EditorView.decorations.from(f),
})

export const keptMarks: Extension = keptField

/** Show `pairs` (merged-text line ranges) on `view`, adding the field first if it's new to the view. */
export function applyKeptMarks(view: EditorView, pairs: KeptPair[]): void {
  const has = view.state.field(keptField, false) !== undefined
  if (!has && pairs.length === 0) return
  // A field added by a transaction starts at create(); it hears the next one.
  if (!has) view.dispatch({ effects: StateEffect.appendConfig.of(keptField) })
  view.dispatch({ effects: setKeptMarks.of(pairs) })
}

/** The marks' lines right now (tests). */
export function keptLines(view: EditorView): { line: number; who: string }[] {
  const out: { line: number; who: string }[] = []
  const set = view.state.field(keptField, false)
  if (!set) return out
  set.between(0, view.state.doc.length, (from, _to, d) => {
    const who = d.spec.attributes?.['data-kept']
    if (who) out.push({ line: view.state.doc.lineAt(from).number - 1, who })
  })
  return out
}
