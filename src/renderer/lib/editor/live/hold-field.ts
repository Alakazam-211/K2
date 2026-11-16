// Holds on the text, in CodeMirror (build-editors.md E4b): one field keeps
// each hold as a range of the doc, mapped through every edit (typing at either
// end stays inside: a hold never blocks a person). From it:
//   - a layer under the text paints the soft tint in the holder's presence
//     colour and a 3px bar left of the column (the gutter's edge in code);
//   - the label ("Atlas is rewriting Setup", with Message Atlas) is a block of
//     its own above the range, so it never sits on a line of text;
//   - while your caret is inside, one block under its line says you can
//     still type there.
// The words are lib/editor/live/holds.ts.

import { EditorState, StateEffect, StateField, type Extension, type Range } from '@codemirror/state'
import { BlockType, Decoration, EditorView, RectangleMarker, WidgetType, layer, type DecorationSet } from '@codemirror/view'
import { holdHint, holdLabel, type Hold } from './holds'

export interface HeldRange extends Hold {
  from: number
  to: number
}

/** Every hold now (lines resolved against the doc as it is). */
export const setHolds = StateEffect.define<Hold[]>()

function resolve(state: EditorState, h: Hold): HeldRange | null {
  const n = state.doc.lines
  if (h.first > n) return null
  return { ...h, from: state.doc.line(h.first).from, to: state.doc.line(Math.min(h.last, n)).to }
}

export const holdField = StateField.define<HeldRange[]>({
  create: () => [],
  update(holds, tr) {
    let next = holds
    if (tr.docChanged) next = next.map((h) => ({ ...h, from: tr.changes.mapPos(h.from, -1), to: Math.max(tr.changes.mapPos(h.from, -1), tr.changes.mapPos(h.to, 1)) }))
    for (const e of tr.effects) if (e.is(setHolds)) next = e.value.flatMap((h) => resolve(tr.state, h) ?? [])
    return next
  },
})

export const holdsIn = (state: EditorState): HeldRange[] => state.field(holdField, false) ?? []
export const heldAt = (state: EditorState, pos: number): HeldRange | null => holdsIn(state).find((h) => pos >= h.from && pos <= h.to) ?? null

const SVG = 'http://www.w3.org/2000/svg'
function icon(paths: string[]): SVGSVGElement {
  const s = document.createElementNS(SVG, 'svg')
  for (const [k, v] of Object.entries({ viewBox: '0 0 24 24', width: '13', height: '13', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.75', 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true', focusable: 'false' })) s.setAttribute(k, v)
  for (const d of paths) {
    const p = document.createElementNS(SVG, 'path')
    p.setAttribute('d', d)
    s.appendChild(p)
  }
  return s
}
/** The app's message and info marks (lucide's, at K2's stroke). */
const messageIcon = (): SVGSVGElement => icon(['M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z'])
const infoIcon = (): SVGSVGElement => icon(['M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z', 'M12 16v-4', 'M12 8h.01'])

class LabelWidget extends WidgetType {
  constructor(
    readonly hold: HeldRange,
    readonly text: string,
    readonly onMessage: (h: Hold) => void,
  ) {
    super()
  }
  eq(o: LabelWidget): boolean {
    return o.text === this.text && o.hold.claim === this.hold.claim && o.hold.slot === this.hold.slot && o.hold.first === this.hold.first && o.hold.last === this.hold.last
  }
  toDOM(): HTMLElement {
    const row = document.createElement('div')
    row.className = 'ed-hold-row'
    const lab = document.createElement('span')
    lab.className = `ed-hold-label ed-pc-${this.hold.slot}`
    lab.dataset.k2Chamfer = 'chip'
    lab.dataset.testid = 'ed-hold-label'
    lab.title = `${this.text} (${this.hold.first === this.hold.last ? `line ${this.hold.first}` : `lines ${this.hold.first}–${this.hold.last}`}). You can still type there.`
    const av = document.createElement('span')
    av.className = 'ed-hold-av'
    av.dataset.k2Chamfer = 'surface'
    av.dataset.k2Avatar = ''
    av.setAttribute('aria-hidden', 'true')
    av.textContent = (this.hold.name.trim()[0] ?? '?').toUpperCase()
    const words = document.createElement('span')
    words.textContent = this.text
    const msg = document.createElement('button')
    msg.type = 'button'
    msg.className = 'ed-hold-msg'
    msg.dataset.k2Focus = ''
    msg.setAttribute('aria-label', `Message ${this.hold.name}`)
    msg.title = `Message ${this.hold.name}`
    msg.appendChild(messageIcon())
    msg.addEventListener('mousedown', (e) => e.preventDefault())
    msg.addEventListener('click', (e) => {
      e.preventDefault()
      this.onMessage(this.hold)
    })
    lab.append(av, words, msg)
    row.appendChild(lab)
    return row
  }
  ignoreEvent(): boolean {
    return true
  }
}

class HintWidget extends WidgetType {
  constructor(
    readonly hold: HeldRange,
    readonly label: string,
  ) {
    super()
  }
  eq(o: HintWidget): boolean {
    return o.label === this.label && o.hold.name === this.hold.name
  }
  toDOM(): HTMLElement {
    const el = document.createElement('div')
    el.className = 'ed-hold-hint'
    el.dataset.k2Chamfer = 'surface'
    el.dataset.testid = 'ed-hold-hint'
    el.setAttribute('role', 'note')
    const b = document.createElement('b')
    b.textContent = `${this.label}.`
    const rest = holdHint(this.hold, null).slice(holdLabel(this.hold, null).length + 1)
    const words = document.createElement('span')
    words.append(b, rest)
    el.append(infoIcon(), words)
    return el
  }
  ignoreEvent(): boolean {
    return true
  }
}

export interface HoldsOptions {
  onMessage: (h: Hold) => void
  /** The section a hold's first line is in ("Setup"), or null (code). */
  section: (state: EditorState, line: number) => string | null
}

function decorations(state: EditorState, o: HoldsOptions): DecorationSet {
  const holds = holdsIn(state)
  if (!holds.length) return Decoration.none
  const ranges: Range<Decoration>[] = []
  const head = state.selection.main.head
  for (const h of holds) {
    const label = holdLabel(h, o.section(state, h.first))
    ranges.push(Decoration.widget({ widget: new LabelWidget(h, label, o.onMessage), block: true, side: -1 }).range(h.from))
    if (!state.readOnly && head >= h.from && head <= h.to) ranges.push(Decoration.widget({ widget: new HintWidget(h, label), block: true, side: 1 }).range(state.doc.lineAt(head).to))
  }
  return Decoration.set(ranges, true)
}

/** The tint and bar under the text, from the column's left edge (or the code's). */
const tintLayer = layer({
  above: false,
  class: 'ed-hold-layer',
  update: (u) => u.docChanged || u.viewportChanged || u.geometryChanged || u.transactions.some((tr) => tr.effects.some((e) => e.is(setHolds))),
  markers(view) {
    const holds = holdsIn(view.state)
    if (!holds.length) return []
    const sc = view.scrollDOM.getBoundingClientRect()
    const base = { left: sc.left - view.scrollDOM.scrollLeft, top: sc.top - view.scrollDOM.scrollTop }
    const content = view.contentDOM.getBoundingClientRect()
    const gutters = view.dom.querySelector('.cm-gutters')
    const code = !!gutters && (gutters as HTMLElement).offsetWidth > 0
    const x = content.left - base.left
    const right = Math.min(content.right - base.left + (code ? 0 : 10), view.scrollDOM.scrollLeft + view.scrollDOM.clientWidth)
    const left = code ? x : x - 17
    const out: RectangleMarker[] = []
    const { from: vFrom, to: vTo } = view.viewport
    for (const h of holds) {
      if (h.to < vFrom || h.from > vTo) continue
      const first = view.lineBlockAt(h.from)
      const text = Array.isArray(first.type) ? (first.type.find((b) => b.type === BlockType.Text) ?? first) : first
      const top = view.documentTop + text.top - base.top
      const bottom = view.documentTop + view.lineBlockAt(h.to).bottom - base.top
      out.push(new RectangleMarker(`ed-hold-tint ed-pc-${h.slot}`, left, top - 4, right - left, bottom - top + 8))
      out.push(new RectangleMarker(`ed-hold-bar ed-pc-${h.slot}`, left, top - 4, 3, bottom - top + 8))
    }
    return out
  },
})

/** The hold field, its label and hint blocks, and the tint layer. */
export function holdsExtension(o: HoldsOptions): Extension {
  return [holdField, EditorView.decorations.compute([holdField, 'selection', EditorState.readOnly], (s) => decorations(s, o)), tintLayer]
}
