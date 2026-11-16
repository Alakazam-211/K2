// Presence on a live text, in CodeMirror (build-editors.md E4b): the kit
// draws each other person's caret and selection (components/LiveEditor); this
// adds what K2 draws around them:
//   - a layer above the text with each caret's name flag in the right margin
//     and a dashed hairline back to the caret (lib/editor/live/flags.ts places
//     them; the kit's inline names are hidden by the host's CSS). With no room
//     in the margin, a name tag just above the caret instead, for 2 s after it
//     moves and while the pointer is over it (TagClock); it fades, and never
//     takes the pointer;
//   - a watch that sorts people above and below the view for the off-screen
//     band, names the heading at the top of the view (your view address), and
//     tells the host when you scroll or type (that stops following).
// The host answers through a stable object, so the extension is made once.

import { StateEffect, type Extension } from '@codemirror/state'
import { EditorView, ViewPlugin, layer, type LayerMarker, type ViewUpdate } from '@codemirror/view'
import { placeFlags, TagClock, type Flag, type FlagCaret } from './flags'
import { bandCentre, offscreen, sameOffscreen, type Offscreen } from './offscreen'
import type { Slot } from './presence-slots'

export interface PresencePerson {
  id: string
  name: string
  agent: boolean
  slot: Slot
  self: boolean
  away: boolean
  /** Their caret's line (1-based), or null. */
  line: number | null
}

export interface PresenceHost {
  people(): readonly PresencePerson[]
  following(): string | null
  /** Who is above and below the view, and the column's centre (from the editor's left). */
  onOffscreen(o: Offscreen & { centre: number }): void
  /** The first line in view (for your view address). */
  onTopLine(line: number): void
  /** You scrolled, clicked or typed in the text. */
  onUserMove(): void
}

/** Redraw the flags and measure again (the people changed). */
export const presenceChanged = StateEffect.define<null>()

const SVG = 'http://www.w3.org/2000/svg'
/** K2's agent mark (the Agents icon, lucide's `bot` squared). */
function agentGlyph(): SVGSVGElement {
  const s = document.createElementNS(SVG, 'svg')
  for (const [k, v] of Object.entries({ viewBox: '0 0 24 24', width: '12', height: '12', fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'square', 'aria-hidden': 'true', focusable: 'false' })) s.setAttribute(k, v)
  for (const d of ['M12 8V4H8', 'M4 8h16v12H4z', 'M2 14h2', 'M20 14h2', 'M15 13v2', 'M9 13v2']) {
    const p = document.createElementNS(SVG, 'path')
    p.setAttribute('d', d)
    s.appendChild(p)
  }
  return s
}

class FlagMarker implements LayerMarker {
  constructor(
    readonly f: Flag,
    readonly slot: Slot,
  ) {}
  eq(o: LayerMarker): boolean {
    return o instanceof FlagMarker && o.slot === this.slot && o.f.id === this.f.id && o.f.label === this.f.label && o.f.left === this.f.left && o.f.top === this.f.top && o.f.agent === this.f.agent
  }
  draw(): HTMLElement {
    const el = document.createElement('span')
    el.className = `ed-flag ed-pc-${this.slot}${this.f.mini ? ' ed-flag-mini' : ''}`
    el.dataset.who = this.f.id
    el.dataset.testid = 'ed-flag'
    el.dataset.k2Chamfer = 'chip'
    el.title = this.f.name
    el.setAttribute('aria-hidden', 'true')
    el.style.left = `${this.f.left}px`
    el.style.top = `${this.f.top}px`
    el.style.width = `${this.f.width}px`
    if (this.f.agent && !this.f.mini) el.appendChild(agentGlyph())
    el.appendChild(document.createTextNode(this.f.label))
    return el
  }
}

/** The fallback name tag over a caret. Kept drawn while hidden, so showing and hiding fade in place. */
class TagMarker implements LayerMarker {
  constructor(
    readonly f: Flag,
    readonly slot: Slot,
    readonly shown: boolean,
  ) {}
  eq(o: LayerMarker): boolean {
    return o instanceof TagMarker && o.slot === this.slot && o.shown === this.shown && o.f.id === this.f.id && o.f.label === this.f.label && o.f.left === this.f.left && o.f.top === this.f.top && o.f.width === this.f.width
  }
  draw(): HTMLElement {
    const el = document.createElement('span')
    el.dataset.who = this.f.id
    el.dataset.testid = 'ed-caret-tag'
    el.dataset.k2Chamfer = 'chip'
    el.setAttribute('aria-hidden', 'true')
    if (this.f.agent) el.appendChild(agentGlyph())
    el.appendChild(document.createTextNode(this.f.label))
    this.place(el)
    return el
  }
  update(dom: HTMLElement, prev: LayerMarker): boolean {
    if (!(prev instanceof TagMarker) || prev.f.id !== this.f.id || prev.f.label !== this.f.label || prev.f.agent !== this.f.agent) return false
    this.place(dom)
    return true
  }
  private place(el: HTMLElement): void {
    el.className = `ed-caret-tag ed-pc-${this.slot}${this.shown ? ' ed-caret-tag-shown' : ''}`
    el.style.left = `${this.f.left}px`
    el.style.top = `${this.f.top}px`
    el.style.maxWidth = `${this.f.width}px`
  }
}

class LineMarker implements LayerMarker {
  constructor(
    readonly f: Flag,
    readonly slot: Slot,
  ) {}
  eq(o: LayerMarker): boolean {
    const a = (o as LineMarker).f?.line
    const b = this.f.line
    return o instanceof LineMarker && o.slot === this.slot && !!a && !!b && a.x0 === b.x0 && a.x1 === b.x1 && a.y === b.y
  }
  draw(): HTMLElement {
    const line = this.f.line ?? { x0: 0, x1: 0, y: 0 }
    const el = document.createElement('span')
    el.className = `ed-fline ed-pc-${this.slot}`
    el.dataset.who = this.f.id
    el.style.left = `${line.x0}px`
    el.style.top = `${line.y}px`
    el.style.width = `${Math.max(0, line.x1 - line.x0)}px`
    return el
  }
}

let canvas: CanvasRenderingContext2D | null | undefined
/** A flag's drawn width: 12px semibold words, its padding, the agent mark. */
function flagWidth(view: EditorView, label: string, agent: boolean): number {
  if (canvas === undefined) {
    try {
      canvas = typeof document !== 'undefined' ? document.createElement('canvas').getContext('2d') : null
    } catch {
      canvas = null
    }
  }
  const pad = 5 + 7 + (agent ? 16 : 0)
  if (!canvas) return Math.ceil(label.length * 7.2) + pad
  const face = getComputedStyle(view.dom).getPropertyValue('--font-ui').trim() || 'system-ui, sans-serif'
  canvas.font = `600 12px ${face}`
  return Math.ceil(canvas.measureText(label).width) + pad + 2
}

/** The layer's origin, as CodeMirror's own layers take it. */
const baseOf = (view: EditorView): { left: number; top: number } => {
  const r = view.scrollDOM.getBoundingClientRect()
  return { left: r.left - view.scrollDOM.scrollLeft, top: r.top - view.scrollDOM.scrollTop }
}

function flagMarkers(view: EditorView, host: PresenceHost, clock: TagClock, now: number): LayerMarker[] {
  const people = new Map(host.people().map((p) => [p.id, p]))
  const base = baseOf(view)
  const carets: FlagCaret[] = []
  const seen = new Set<string>()
  for (const el of view.contentDOM.querySelectorAll<HTMLElement>('.cm-mpc-caret[data-mpc-caret]')) {
    const id = (el.dataset.mpcCaret ?? '').replace(/#\d+$/, '')
    const p = people.get(id)
    // One flag a person (their most active caret: the kit draws it first), none for away people.
    if (!p || p.self || p.away || seen.has(id)) continue
    seen.add(id)
    // Where the caret is now: a new place (a move or typing) restarts its tag's time.
    try {
      clock.observe(id, view.posAtDOM(el), now)
    } catch {
      // Not in the document this instant (a redraw): no move to note.
    }
    const r = el.getBoundingClientRect()
    carets.push({ id, name: p.name, agent: p.agent, x: r.left - base.left, top: r.top - base.top, bottom: r.bottom - base.top })
  }
  if (!carets.length) return []
  const sc = view.scrollDOM
  const content = view.contentDOM.getBoundingClientRect()
  // Write and Read have a reading column; Source and code have the text's own right edge.
  const column = view.dom.classList.contains('cm-md') ? content.right - base.left : null
  // Each line's box once (a caret's band looks only at the lines it crosses).
  const blocks = [...view.contentDOM.querySelectorAll<HTMLElement>('.cm-line, .ed-hold-label, .ed-hold-hint')].map((b) => ({ b, br: b.getBoundingClientRect() }))
  const textRight = (top: number, bottom: number): number => {
    let right = -Infinity
    for (const { b, br } of blocks) {
      if (br.bottom - base.top <= top || br.top - base.top >= bottom) continue
      if (!b.classList.contains('cm-line')) {
        right = Math.max(right, br.right - base.left)
        continue
      }
      const range = document.createRange()
      range.selectNodeContents(b)
      for (const g of range.getClientRects()) if (g.width > 0 && g.bottom - base.top > top && g.top - base.top < bottom) right = Math.max(right, g.right - base.left)
    }
    return right
  }
  const flags = placeFlags(carets, {
    column,
    clip: { left: sc.scrollLeft, right: sc.scrollLeft + sc.clientWidth, top: sc.scrollTop, bottom: sc.scrollTop + sc.clientHeight },
    textRight,
    width: (label, agent) => flagWidth(view, label, agent),
  })
  return flags.flatMap((f) => {
    const slot = people.get(f.id)?.slot ?? 1
    if (f.tag) return [new TagMarker(f, slot, clock.visible(f.id, now))]
    return [new LineMarker(f, slot), new FlagMarker(f, slot)]
  })
}

const touched = (u: ViewUpdate): boolean => u.docChanged || u.viewportChanged || u.geometryChanged || u.selectionSet || u.transactions.length > 0

/** The flags layer, the off-screen watch and your moves. */
export function presenceExtension(host: PresenceHost, now: () => number = Date.now): Extension {
  const clock = new TagClock()
  let expiry: ReturnType<typeof setTimeout> | null = null
  const flagLayer = layer({
    above: true,
    class: 'ed-flag-layer',
    update: touched,
    markers: (view) => {
      const t = now()
      const out = flagMarkers(view, host, clock, t)
      // A tag that is showing goes when its time is up: redraw then.
      if (expiry) clearTimeout(expiry)
      const next = clock.nextExpiry(t)
      expiry = next === null ? null : setTimeout(() => view.dispatch({ effects: presenceChanged.of(null) }), next - t + 16)
      return out
    },
    destroy: () => void (expiry && clearTimeout(expiry)),
  })
  const watch = ViewPlugin.fromClass(
    class {
      private last: Offscreen & { centre: number } = { above: [], below: [], centre: -1 }
      private topLine = 0
      private frame = 0
      // A scroll moves the flags with the text; one redraw a frame puts away any it took out of view.
      private readonly onScroll = (): void => {
        if (this.frame) return
        this.frame = requestAnimationFrame(() => {
          this.frame = 0
          this.view.dispatch({ effects: presenceChanged.of(null) })
        })
      }
      private readonly onMove = (e: Event): void => {
        // A modifier on its own isn't a move (⌘ before ⌘E, ⇧ before a capital), nor is Message on a hold.
        if (e instanceof KeyboardEvent && ['Shift', 'Meta', 'Control', 'Alt', 'CapsLock'].includes(e.key)) return
        if (e.target instanceof Element && e.target.closest('.ed-hold-label')) return
        host.onUserMove()
      }
      // The pointer over a caret (within a few px of its bar) or its selection keeps that tag up.
      private readonly onPointer = (e: PointerEvent): void => {
        let who: string | null = null
        const t = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-mpc-caret]') : null
        if (t) who = (t.dataset.mpcCaret ?? '').replace(/#\d+$/, '')
        else
          for (const c of this.view.contentDOM.querySelectorAll<HTMLElement>('.cm-mpc-caret[data-mpc-caret]')) {
            const r = c.getBoundingClientRect()
            if (e.clientX >= r.left - 4 && e.clientX <= r.right + 4 && e.clientY >= r.top && e.clientY <= r.bottom) {
              who = (c.dataset.mpcCaret ?? '').replace(/#\d+$/, '')
              break
            }
          }
        if (clock.hover(who)) this.view.dispatch({ effects: presenceChanged.of(null) })
      }
      private readonly onLeave = (): void => {
        if (clock.hover(null)) this.view.dispatch({ effects: presenceChanged.of(null) })
      }
      constructor(readonly view: EditorView) {
        view.contentDOM.addEventListener('pointermove', this.onPointer, { passive: true })
        view.contentDOM.addEventListener('pointerleave', this.onLeave, { passive: true })
        view.scrollDOM.addEventListener('scroll', this.onScroll, { passive: true })
        for (const t of ['wheel', 'touchmove', 'mousedown']) view.scrollDOM.addEventListener(t, this.onMove, { passive: true })
        view.dom.addEventListener('keydown', this.onMove, true)
        this.measure()
      }
      update(u: ViewUpdate): void {
        if (touched(u)) this.measure()
      }
      measure(): void {
        this.view.requestMeasure({
          key: this,
          read: (view) => {
            const sc = view.scrollDOM.getBoundingClientRect()
            const top = sc.top - view.documentTop
            const bottom = top + view.scrollDOM.clientHeight
            const doc = view.state.doc
            const placed = host.people().map((p) => {
              if (p.line === null || p.line > doc.lines) return { id: p.id, top: null, bottom: null, self: p.self, away: p.away }
              const b = view.lineBlockAt(doc.line(p.line).from)
              return { id: p.id, top: b.top, bottom: b.bottom, self: p.self, away: p.away }
            })
            const content = view.contentDOM.getBoundingClientRect()
            const column = view.dom.classList.contains('cm-md') ? { left: content.left - sc.left, right: content.right - sc.left } : null
            const first = doc.lineAt(view.lineBlockAtHeight(Math.max(0, top)).from).number
            return { o: { ...offscreen(placed, { top, bottom }, { following: host.following() }), centre: bandCentre(column, view.scrollDOM.clientWidth) }, first }
          },
          write: ({ o, first }) => {
            if (!sameOffscreen(o, this.last) || o.centre !== this.last.centre) {
              this.last = o
              host.onOffscreen(o)
            }
            if (first !== this.topLine) {
              this.topLine = first
              host.onTopLine(first)
            }
          },
        })
      }
      destroy(): void {
        if (this.frame) cancelAnimationFrame(this.frame)
        this.view.contentDOM.removeEventListener('pointermove', this.onPointer)
        this.view.contentDOM.removeEventListener('pointerleave', this.onLeave)
        this.view.scrollDOM.removeEventListener('scroll', this.onScroll)
        for (const t of ['wheel', 'touchmove', 'mousedown']) this.view.scrollDOM.removeEventListener(t, this.onMove)
        this.view.dom.removeEventListener('keydown', this.onMove, true)
      }
    },
  )
  return [flagLayer, watch]
}
