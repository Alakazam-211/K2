// Write mode's block decorations (build-editors.md E2): rendered tables and
// image figures. CodeMirror takes block widgets only from a state field, so
// these live here, not in the inline plugin. The set is mapped through each
// edit and rebuilt only for the top-level blocks the edit touched (before and
// after it, so opening or closing a fence redraws everything it swallowed),
// for text the parser has just reached, and for tables the reveal moved in
// or out of. A table shows its text while a revealed line is inside it.

import { type EditorState, type Range, StateField, type Transaction } from '@codemirror/state'
import { Decoration, type DecorationSet, EditorView } from '@codemirror/view'
import { syntaxTree } from '@codemirror/language'
import type { Tree } from '@lezer/common'
import { revealField, revealedIn, type LineSpan } from './reveal'
import { ImageWidget } from './widgets'
import { TableWidget, tableModel } from './table'

interface Blocks {
  decos: DecorationSet
  tree: Tree
  reveal: readonly LineSpan[]
}

/** [from, to] widened to whole top-level blocks of `tree`. */
function widen(tree: Tree, from: number, to: number): [number, number] {
  const top = (pos: number, side: -1 | 1): { from: number; to: number } | null => {
    let n = tree.resolve(pos, side)
    while (n.parent && n.parent.parent) n = n.parent
    return n.parent ? n : null
  }
  const a = top(from, 1) ?? top(from, -1)
  const b = top(to, -1) ?? top(to, 1)
  return [Math.min(from, a?.from ?? from), Math.max(to, b?.to ?? to)]
}

/** The block decorations for nodes starting in [from, to). */
export function buildBlocks(state: EditorState, from: number, to: number, reveal: readonly LineSpan[]): Range<Decoration>[] {
  const out: Range<Decoration>[] = []
  const doc = state.doc
  syntaxTree(state).iterate({
    from,
    to,
    enter: (n) => {
      if (n.from < from) return n.name === 'Document' || n.to > from ? undefined : false
      if (n.from >= to) return false
      if (n.name === 'Table') {
        const a = doc.lineAt(n.from).from
        const b = doc.lineAt(n.to).to
        if (!revealedIn(reveal, a, b)) out.push(Decoration.replace({ widget: new TableWidget(tableModel(state, n.node)), block: true }).range(a, b))
        return false
      }
      if (n.name === 'Image') {
        const url = n.node.getChild('URL')
        const marks = n.node.getChildren('LinkMark')
        const alt = marks.length > 1 ? state.sliceDoc(marks[0].to, marks[1].from) : ''
        if (url) out.push(Decoration.widget({ widget: new ImageWidget(state.sliceDoc(url.from, url.to), alt), block: true, side: -1 }).range(doc.lineAt(n.from).from))
        return false
      }
      if (n.name === 'FencedCode' || n.name === 'CodeBlock' || n.name === 'HTMLBlock') return false
      return undefined
    },
  })
  return out
}

function rebuild(state: EditorState, decos: DecorationSet, spans: Array<[number, number]>, reveal: readonly LineSpan[]): DecorationSet {
  spans.sort((x, y) => x[0] - y[0])
  const merged: Array<[number, number]> = []
  for (const s of spans) {
    const last = merged[merged.length - 1]
    if (last && s[0] <= last[1] + 1) last[1] = Math.max(last[1], s[1])
    else merged.push([s[0], s[1]])
  }
  for (const [from, to] of merged) {
    decos = decos.update({ filterFrom: from, filterTo: to, filter: () => false, add: buildBlocks(state, from, to + 1, reveal), sort: true })
  }
  return decos
}

function update(v: Blocks, tr: Transaction): Blocks {
  const state = tr.state
  const tree = syntaxTree(state)
  const reveal = state.field(revealField).lines
  if (!tr.docChanged && tree === v.tree && reveal === v.reveal) return v
  const len = state.doc.length
  const clamp = (n: number): number => Math.max(0, Math.min(len, n))
  const decos = tr.docChanged ? v.decos.map(tr.changes) : v.decos
  const dirty: Array<[number, number]> = []
  if (tr.docChanged) {
    const old = syntaxTree(tr.startState)
    tr.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
      const [oa, ob] = widen(old, fromA, toA)
      const [na, nb] = widen(tree, fromB, toB)
      dirty.push([clamp(Math.min(na, tr.changes.mapPos(oa, -1))), clamp(Math.max(nb, tr.changes.mapPos(ob, 1)))])
    })
  }
  // The parser reached more of the file.
  const parsed = tr.docChanged ? tr.changes.mapPos(v.tree.length, -1) : v.tree.length
  if (tree.length > parsed) dirty.push(widen(tree, clamp(parsed), clamp(tree.length)).map(clamp) as [number, number])
  // Tables the reveal entered or left.
  if (reveal !== v.reveal) {
    for (const s of v.reveal) {
      const a = tr.docChanged ? tr.changes.mapPos(s.from, -1) : s.from
      const b = tr.docChanged ? tr.changes.mapPos(s.to, 1) : s.to
      dirty.push(widen(tree, clamp(a), clamp(b)))
    }
    for (const s of reveal) dirty.push(widen(tree, s.from, s.to))
  }
  return { decos: dirty.length ? rebuild(state, decos, dirty, reveal) : decos, tree, reveal }
}

export const blockField = StateField.define<Blocks>({
  create(state) {
    const reveal = state.field(revealField).lines
    return { decos: Decoration.set(buildBlocks(state, 0, state.doc.length + 1, reveal), true), tree: syntaxTree(state), reveal }
  },
  update,
  provide: (f) => EditorView.decorations.from(f, (v) => v.decos),
})
