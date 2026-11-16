// The find bar's CodeMirror half (build-editors.md E3): the query lives in
// CodeMirror's search state (setSearchQuery), and the frame marks the matches
// itself while the bar is open, in the find tokens (lib/editor/theme.ts:
// .cm-searchMatch, the current one -selected). CodeMirror's own highlighter
// draws only with its panel open, which never happens here; ours runs at the
// highest precedence and the frame mounts it first, so a match's mark is the
// innermost element and never splits a code chip or a link in Write.

import { Prec, RangeSetBuilder, StateEffect, StateField, type Extension } from '@codemirror/state'
import { Decoration, ViewPlugin, type DecorationSet, type EditorView, type ViewUpdate } from '@codemirror/view'
import { getSearchQuery, setSearchQuery } from '@codemirror/search'
import { toQuery, type FindSpec } from './find'

const setShown = StateEffect.define<boolean>()
const shownField = StateField.define<boolean>({
  create: () => false,
  update: (v, tr) => tr.effects.reduce((acc, e) => (e.is(setShown) ? e.value : acc), v),
})

const match = Decoration.mark({ class: 'cm-searchMatch' })
const current = Decoration.mark({ class: 'cm-searchMatch cm-searchMatch-selected' })

function marks(view: EditorView): DecorationSet {
  if (!view.state.field(shownField, false)) return Decoration.none
  const q = getSearchQuery(view.state)
  if (!q.valid) return Decoration.none
  const b = new RangeSetBuilder<Decoration>()
  const sel = view.state.selection.main
  for (const { from, to } of view.visibleRanges) {
    const c = q.getCursor(view.state, Math.max(0, from - 250), Math.min(view.state.doc.length, to + 250))
    let last = -1
    for (let r = c.next(); !r.done; r = c.next()) {
      if (r.value.from < last || r.value.from === r.value.to) continue
      last = r.value.to
      b.add(r.value.from, r.value.to, r.value.from === sel.from && r.value.to === sel.to ? current : match)
    }
  }
  return b.finish()
}

const highlighter = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet
    constructor(view: EditorView) {
      this.decorations = marks(view)
    }
    update(u: ViewUpdate): void {
      const changed = u.startState.field(shownField, false) !== u.state.field(shownField, false) || getSearchQuery(u.startState) !== getSearchQuery(u.state)
      if (changed || u.docChanged || u.selectionSet || u.viewportChanged) this.decorations = marks(u.view)
    }
  },
  { decorations: (v) => v.decorations },
)

/** The shown flag and the marks (mount before Write's extension). */
export const findMarks: Extension = [shownField, Prec.highest(highlighter)]

export const matchesShown = (view: EditorView): boolean => view.state.field(shownField, false) ?? false

export function showMatches(view: EditorView, spec: FindSpec): void {
  view.dispatch({ effects: [setSearchQuery.of(toQuery(spec)), setShown.of(true)] })
}

export function hideMatches(view: EditorView): void {
  if (matchesShown(view)) view.dispatch({ effects: setShown.of(false) })
}
