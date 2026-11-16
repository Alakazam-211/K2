// Where the labels on the picture go, so they never touch (live-scene-v2-
// notes.md: "labels never touch each other or the compass, chips, key
// plate or ⓘ … moved with a 6 px gap"; "each tag and callout is measured;
// one that would leave the picture flips to the box's other edge, else is
// clamped"). Measured sizes in, positions out, in one pass by priority:
// the change under way first, then holds, your pick, notes, others'
// selections. Pure: the overlay measures, this decides.
//
// - A tag sits above its box's top-left; else flips to the box's top-right;
//   else goes below it; else slides up or down (then sideways) until free.
// - A point label (a selection on its front edge) hangs centred under its
//   point, else above it, else slides.
// - An edge chip (something off the picture) sits on its frame's edge and
//   slides along it.
// - A callout (a refusal) hangs centred under its box, far enough down for
//   its line, else above it, else slides; it goes first.
// Every candidate is clamped inside its frame (less `pad`). When nothing is
// free, the least-overlapping spot is used and the label says `touching`.

export interface Rect {
  x0: number
  y0: number
  x1: number
  y1: number
}

export type LabelKind = 'tag' | 'point' | 'edge' | 'callout'
export type Side = 'left' | 'right' | 'top' | 'bottom'

export interface LabelReq {
  id: string
  kind: LabelKind
  /** A tag: the box it labels. A point or edge label: its point (x0 = x1, y0 = y1). */
  anchor: Rect
  w: number
  h: number
  /** Lower goes first and keeps its spot. */
  prio: number
  /** An edge label's edge. */
  side?: Side
  /** Its own frame (an edge chip uses the stage; tags the picture). */
  frame?: Rect
}

export interface Placed {
  id: string
  x: number
  y: number
  w: number
  h: number
  /** No free spot was found: it touches something. */
  touching: boolean
}

/** The gap between any two labels, and between a label and the chrome. */
export const LABEL_GAP = 6
const PAD = 4
const SWEEP = 40
/** How far a callout sits from its box: room for the line to it. */
export const CALLOUT_DROP = 28

export const grow = (r: Rect, g: number): Rect => ({ x0: r.x0 - g, y0: r.y0 - g, x1: r.x1 + g, y1: r.y1 + g })
/** Overlapping (edges that only meet don't). */
export const hit = (a: Rect, b: Rect): boolean => a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
const area = (a: Rect, b: Rect): number => Math.max(0, Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0)) * Math.max(0, Math.min(a.y1, b.y1) - Math.max(a.y0, b.y0))
const rectAt = (x: number, y: number, w: number, h: number): Rect => ({ x0: x, y0: y, x1: x + w, y1: y + h })

/** Pairs closer than `gap`: label index pairs, and [index, 'obstacle']. For tests and the shots' MEASURE. */
export function touchingPairs(labels: readonly Rect[], obstacles: readonly Rect[], gap = LABEL_GAP): [number, number | 'obstacle'][] {
  const out: [number, number | 'obstacle'][] = []
  labels.forEach((a, i) => {
    for (let j = i + 1; j < labels.length; j++) if (hit(grow(a, gap), labels[j])) out.push([i, j])
    if (obstacles.some((o) => hit(grow(a, gap), o))) out.push([i, 'obstacle'])
  })
  return out
}

/** The spots to try, best first (before clamping). */
function candidates(r: LabelReq, inner: Rect, gap: number): { x: number; y: number }[] {
  const { anchor: a, w, h } = r
  const base: { x: number; y: number }[] = []
  if (r.kind === 'tag') base.push({ x: a.x0, y: a.y0 - gap - h }, { x: a.x1 - w, y: a.y0 - gap - h }, { x: a.x0, y: a.y1 + gap })
  else if (r.kind === 'callout') base.push({ x: (a.x0 + a.x1) / 2 - w / 2, y: a.y1 + CALLOUT_DROP }, { x: (a.x0 + a.x1) / 2 - w / 2, y: a.y0 - CALLOUT_DROP - h })
  else if (r.kind === 'point') base.push({ x: a.x0 - w / 2, y: a.y0 + gap }, { x: a.x0 - w / 2, y: a.y0 - gap - h })
  else {
    const s = r.side ?? 'left'
    base.push(s === 'left' ? { x: inner.x0, y: a.y0 - h / 2 } : s === 'right' ? { x: inner.x1 - w, y: a.y0 - h / 2 } : s === 'top' ? { x: a.x0 - w / 2, y: inner.y0 } : { x: a.x0 - w / 2, y: inner.y1 - h })
  }
  const out = base.slice()
  const o = base[0]
  const along = r.kind === 'edge' && (r.side === 'top' || r.side === 'bottom') ? 'x' : 'y'
  const step = (along === 'x' ? w : h) + gap
  for (let k = 1; k <= SWEEP; k++)
    for (const s of [1, -1]) out.push(along === 'x' ? { x: o.x + (s * k * step) / 3, y: o.y } : { x: o.x, y: o.y + (s * k * step) / 3 })
  if (r.kind !== 'edge')
    for (const dx of [w + gap, -(w + gap)])
      for (let k = 0; k <= SWEEP; k++) for (const s of [1, -1]) out.push({ x: o.x + dx, y: o.y + (s * k * step) / 3 })
  return out
}

/** Place every label, highest priority first. `frame` is the picture; `pad` keeps labels off its very edge. */
export function placeLabels(reqs: readonly LabelReq[], o: { frame: Rect; obstacles?: readonly Rect[]; gap?: number; pad?: number }): Placed[] {
  const gap = o.gap ?? LABEL_GAP
  const pad = o.pad ?? PAD
  const obstacles = o.obstacles ?? []
  const taken: Rect[] = []
  const placed = new Map<string, Placed>()
  const order = reqs.map((r, i) => ({ r, i })).sort((a, b) => a.r.prio - b.r.prio || a.i - b.i)
  for (const { r } of order) {
    const f = r.frame ?? o.frame
    const inner = { x0: f.x0 + pad, y0: f.y0 + pad, x1: f.x1 - pad, y1: f.y1 - pad }
    const clamp = (c: { x: number; y: number }): { x: number; y: number } => ({
      x: Math.max(inner.x0, Math.min(inner.x1 - r.w, c.x)),
      y: Math.max(inner.y0, Math.min(inner.y1 - r.h, c.y)),
    })
    const others = taken.concat(obstacles)
    let best: { x: number; y: number } | null = null
    let least = { at: null as { x: number; y: number } | null, cost: Infinity }
    const cands = candidates(r, inner, gap)
    // First the spots that fit as they are (a flip beats a clamp), then the clamped ones.
    for (const fits of [true, false]) {
      for (const c0 of cands) {
        const c = clamp(c0)
        if (fits !== (c.x === c0.x && c.y === c0.y)) continue
        const g = grow(rectAt(c.x, c.y, r.w, r.h), gap)
        const cost = others.reduce((s, b) => s + area(g, b), 0)
        if (cost === 0) {
          best = c
          break
        }
        if (cost < least.cost) least = { at: c, cost }
      }
      if (best) break
    }
    const at = best ?? least.at ?? clamp({ x: r.anchor.x0, y: r.anchor.y0 })
    const p: Placed = { id: r.id, x: at.x, y: at.y, w: r.w, h: r.h, touching: !best }
    taken.push(rectAt(p.x, p.y, p.w, p.h))
    placed.set(r.id, p)
  }
  return reqs.map((r) => placed.get(r.id)!)
}
