// Thread view's floating objects, merged (Rosson 2026-11-14: the name's
// bubble "grows, and intersects with the other glass bubbles", as on the
// phone). The name's object (the plate) may grow under the objects beside it
// (ThreadViewIdentity); where it comes within MERGE_K of one, the two become
// one object:
//
// - Glass: the K2 Companion's merged header glass (src/zen/glassMerge.ts).
//   Each piece is a signed distance field (a rounded box from its DOM box);
//   the plate joins a neighbour with a smooth minimum (Inigo Quilez's smin)
//   within MERGE_K, and neighbours keep their own outlines among themselves.
//   One frosted layer and its rim, cut by alpha masks drawn here, sit under
//   the pieces, which drop their own glass meanwhile (ThreadFloatMerge.tsx).
//   The rim's colours stay in the theme: the masks carry shape only, full
//   strength, and each layer's fill is a token (--material-edge,
//   --color-frost-sheen, --color-shadow-soft).
// - Terminal: nothing to melt. Pieces that close snap into one segmented
//   bar: one chamfered outline round the lot (`joinBar`), the gaps between
//   them filled, and one hairline between neighbours instead of two borders.
//   The plate snaps out under every object it joins, so the bar's ends line
//   up with theirs.
//
// Everything here is in the layer's own CSS px. Pure apart from
// `renderMergeMasks` (canvas).

/** How close (px) the plate and a neighbour come before they merge (Glass)
 *  or join (Terminal). The phone's MERGE_K. */
export const MERGE_K = 16
/** Glass: a moving header (a window resize, a pane drag) shows each piece's
 *  own glass, and the masks are drawn once it has been still this long.
 *  Never per frame. */
export const MERGE_SETTLE_MS = 160
/** The mask's resolution is the screen's: CSS px x app zoom x device pixel
 *  ratio, capped here. */
export const MERGE_MAX_SCALE = 4

/** An axis-aligned box. */
export type Box = { x0: number; y0: number; x1: number; y1: number }
/** A rounded box: centre, half size, corner radius. */
export type GlassShape = { cx: number; cy: number; hx: number; hy: number; r: number }

const w = (b: Box): number => b.x1 - b.x0
const h = (b: Box): number => b.y1 - b.y0

/** A rounded box from a box and its corner radius (a pill's 9999px is half
 *  its short side). */
export function shapeFromBox(b: Box, radius: number): GlassShape {
  const hx = w(b) / 2
  const hy = h(b) / 2
  return { cx: b.x0 + hx, cy: b.y0 + hy, hx, hy, r: Math.max(0, Math.min(radius, hx, hy)) }
}

/** Signed distance to a rounded box (negative inside). */
export function sdBox(px: number, py: number, s: GlassShape): number {
  const r = Math.min(s.r, s.hx, s.hy)
  const qx = Math.abs(px - s.cx) - s.hx + r
  const qy = Math.abs(py - s.cy) - s.hy + r
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r
}

/** Smooth minimum: within `k` the two shapes join with a rounded neck;
 *  `k <= 0` is a plain union. */
export function smin(a: number, b: number, k: number): number {
  if (k <= 0) return Math.min(a, b)
  const t = Math.max(0, Math.min(1, 0.5 + (0.5 * (b - a)) / k))
  return b + (a - b) * t - k * t * (1 - t)
}

/** The merged outline's distance at (x, y): the plate flows into the
 *  others; the others meet with a plain min. */
export function mergedDistance(x: number, y: number, others: GlassShape[], plate: GlassShape | null, k = MERGE_K): number {
  let d = Infinity
  for (const s of others) d = Math.min(d, sdBox(x, y, s))
  return plate ? smin(sdBox(x, y, plate), d, k) : d
}

/** The gap between two boxes (0 when they touch or overlap). */
export function boxGap(a: Box, b: Box): number {
  const dx = Math.max(0, a.x0 - b.x1, b.x0 - a.x1)
  const dy = Math.max(0, a.y0 - b.y1, b.y0 - a.y1)
  return Math.hypot(dx, dy)
}

/** Glass: the neighbours the plate flows into, by index: those whose box
 *  comes within `k` of the plate's. The rest keep their own glass (the
 *  smooth minimum leaves a shape that far away exactly as it was). */
export function mergeNeighbours(plate: Box, others: Box[], k = MERGE_K): number[] {
  const out: number[] = []
  others.forEach((b, i) => {
    if (boxGap(plate, b) < k) out.push(i)
  })
  return out
}

/** The boxes' bounds, grown by `pad` and snapped out to whole px. */
export function boundsOf(boxes: Box[], pad = 0): Box {
  return {
    x0: Math.floor(Math.min(...boxes.map((b) => b.x0)) - pad),
    y0: Math.floor(Math.min(...boxes.map((b) => b.y0)) - pad),
    x1: Math.ceil(Math.max(...boxes.map((b) => b.x1)) + pad),
    y1: Math.ceil(Math.max(...boxes.map((b) => b.y1)) + pad),
  }
}

/** A drop shadow under the merged outline (Veil's frost has one): the
 *  box-shadow's blur radius and its downward offset, in px. */
export type MergeShadow = { blur: number; y: number }

export type MergeMasks = { fill: string; edge: string; sheen: string; shadow: string | null }

/** How far outside the shapes a mask can reach: the neck's bulge (k/4) and
 *  the shadow's tail. */
export function maskPad(k: number, shadow: MergeShadow | null): number {
  return Math.ceil(Math.max(k / 4 + 2, shadow ? shadow.blur * 1.5 + shadow.y + 1 : 0))
}

/**
 * The masks as PNG data URLs, `width` x `height` CSS px (shapes in the same
 * space) at `scale` device px per CSS px. Null where a canvas is missing
 * (jsdom, a locked-down webview): the pieces then keep their own glass.
 *
 * - fill: the merged outline (the frosted layer);
 * - edge: a band `edge` px wide just inside it (`.zen-glass`'s 0.5px
 *   border, Veil's 1px ring);
 * - sheen: the 1px light catch along its top-facing edges
 *   (`inset 0 1px 0`);
 * - shadow: a soft drop outside it, when asked for.
 */
export function renderMergeMasks(o: {
  width: number
  height: number
  scale: number
  plate: GlassShape
  others: GlassShape[]
  k?: number
  edge?: number
  shadow?: MergeShadow | null
}): MergeMasks | null {
  const { width, height, plate, others } = o
  const k = o.k ?? MERGE_K
  const edgeW = o.edge ?? 0.5
  const shadow = o.shadow ?? null
  if (typeof document === 'undefined' || width <= 0 || height <= 0) return null
  const scale = Math.max(1, Math.min(MERGE_MAX_SCALE, o.scale))
  const cw = Math.round(width * scale)
  const ch = Math.round(height * scale)
  const canvases = [0, 1, 2, 3].map(() => document.createElement('canvas'))
  const ctxs: CanvasRenderingContext2D[] = []
  try {
    for (const c of canvases) {
      c.width = cw
      c.height = ch
      const ctx = c.getContext('2d')
      if (!ctx) return null
      ctxs.push(ctx)
    }
  } catch {
    return null
  }
  const imgs = ctxs.map((c) => c.createImageData(cw, ch))
  const [fd, ed, sd, hd] = imgs.map((i) => i.data)
  const dist = (x: number, y: number): number => mergedDistance(x, y, others, plate, k)
  // Coverage of a device pixel whose centre is `d` CSS px outside the edge.
  const cov = (d: number): number => Math.max(0, Math.min(1, 0.5 - d * scale))
  // The shadow: a Gaussian falloff (logistic approximation) of the outline
  // moved down by its offset, only outside the outline (a box-shadow never
  // paints under its box).
  const sigma = shadow ? Math.max(0.5, shadow.blur / 2) : 1
  for (let j = 0; j < ch; j++) {
    const y = (j + 0.5) / scale
    for (let i = 0; i < cw; i++) {
      const x = (i + 0.5) / scale
      const d = dist(x, y)
      const a = cov(d)
      const p = (j * cw + i) * 4
      fd[p + 3] = a * 255
      if (shadow && a < 1) {
        const ds = dist(x, y - shadow.y)
        const s = 1 / (1 + Math.exp((1.702 * ds) / sigma))
        hd[p + 3] = s * (1 - a) * 255
      }
      if (d > 1.5 || d < -(edgeW + 2)) continue
      const inner = cov(d + edgeW)
      ed[p + 3] = Math.max(0, a - inner) * 255
      const above = cov(dist(x, y - 1) + edgeW)
      sd[p + 3] = Math.max(0, inner - above) * 255
    }
  }
  imgs.forEach((img, n) => ctxs[n].putImageData(img, 0, 0))
  const url = (n: number): string => canvases[n].toDataURL()
  return { fill: url(0), edge: url(1), sheen: url(2), shadow: shadow ? url(3) : null }
}

// ── Terminal: the joined bar ────────────────────────────────────────────

/** Terminal's answer: the joined pieces (indices into `others`), the boxes
 *  whose union is the bar (the plate snapped out under what it joins, the
 *  pieces and the bridges over the gaps between them), the hairlines inside
 *  it (one where two pieces meet), and its outline as an SVG path. */
export type JoinBar = { joined: number[]; plate: Box; rects: Box[]; dividers: Box[]; path: string; bounds: Box }

/** In one row: they overlap by at least half the shorter one's height. */
function sameRow(a: Box, b: Box): boolean {
  const overlap = Math.min(a.y1, b.y1) - Math.max(a.y0, b.y0)
  return overlap >= 0.5 * Math.min(h(a), h(b))
}

/** `b` stands on `p`: above it, its bottom closer than `k` to p's top. */
function standsOn(b: Box, p: Box, k: number): boolean {
  return !sameRow(b, p) && b.y0 < p.y0 && p.y0 - b.y1 < k
}

function hGap(a: Box, b: Box): number {
  return Math.max(a.x0 - b.x1, b.x0 - a.x1)
}

/**
 * Terminal: which pieces join the plate, and the bar they make. A piece in
 * the plate's row joins across a gap under `k`; a piece standing on the
 * plate (the stacked header: the objects' row above the name) joins when
 * the plate comes closer than `k` to it sideways, and the plate snaps out
 * under it. A piece next to a joined one (the same row, under `k` apart)
 * joins too, so a side's objects join together. Null when nothing is close.
 */
export function joinBar(plate: Box, others: Box[], { k = MERGE_K, cut = 0 }: { k?: number; cut?: number } = {}): JoinBar | null {
  const p: Box = { ...plate }
  const joined = new Set<number>()
  const stands = (i: number): boolean => standsOn(others[i], p, k)
  const join = (i: number): void => {
    joined.add(i)
    if (stands(i)) {
      p.x0 = Math.min(p.x0, others[i].x0)
      p.x1 = Math.max(p.x1, others[i].x1)
    }
  }
  for (let changed = true; changed; ) {
    changed = false
    others.forEach((b, i) => {
      if (joined.has(i)) return
      const near =
        ((sameRow(b, p) || standsOn(b, p, k)) && hGap(b, p) < k) ||
        [...joined].some((j) => sameRow(b, others[j]) && hGap(b, others[j]) < k)
      if (near) {
        join(i)
        changed = true
      }
    })
  }
  if (joined.size === 0) return null
  const ids = [...joined].sort((a, b) => a - b)
  const rects: Box[] = [p, ...ids.map((i) => others[i])]
  const dividers: Box[] = []
  // Rows of joined pieces (the plate counts in its own row), left to right:
  // the gap between neighbours is filled, with one hairline at its middle.
  const rows: Box[][] = []
  for (const b of [p, ...ids.map((i) => others[i])]) {
    const row = rows.find((r) => sameRow(r[0], b))
    if (row) row.push(b)
    else rows.push([b])
  }
  for (const row of rows) {
    row.sort((a, b) => a.x0 - b.x0)
    for (let n = 1; n < row.length; n++) {
      const a = row[n - 1]
      const c = row[n]
      const gap = c.x0 - a.x1
      if (gap >= k) continue
      const y0 = Math.max(a.y0, c.y0)
      const y1 = Math.min(a.y1, c.y1)
      if (gap > 0) rects.push({ x0: a.x1, y0, x1: c.x0, y1 })
      const mid = gap > 0 ? (a.x1 + c.x0) / 2 : c.x0 + 0.5
      dividers.push({ x0: mid - 0.5, y0, x1: mid + 0.5, y1 })
    }
  }
  // Pieces standing on the plate: the gap under them filled, and one
  // hairline along their bottom edge, across each run of them.
  const standing = ids.filter((i) => standsOn(others[i], p, k)).map((i) => others[i])
  for (const b of standing) if (p.y0 > b.y1) rects.push({ x0: b.x0, y0: b.y1, x1: b.x1, y1: p.y0 })
  standing.sort((a, b) => a.x0 - b.x0)
  for (let n = 0; n < standing.length; ) {
    let m = n
    while (m + 1 < standing.length && standing[m + 1].x0 - standing[m].x1 < k && sameRow(standing[m], standing[m + 1])) m++
    const yb = Math.min(...standing.slice(n, m + 1).map((b) => b.y1))
    dividers.push({ x0: standing[n].x0, y0: yb - 1, x1: standing[m].x1, y1: yb })
    n = m + 1
  }
  return { joined: ids, plate: p, rects, dividers, path: unionPath(rects, cut), bounds: boundsOf(rects) }
}

type Pt = { x: number; y: number }

/** The outline of a union of boxes as an SVG path: one loop per piece of
 *  outline, clockwise, outer corners cut by `cut` px (never more than half
 *  an edge), inner corners square. */
export function unionPath(rects: Box[], cut = 0): string {
  return unionLoops(rects)
    .map((loop) => chamferLoop(loop, cut))
    .map((pts) => 'M' + pts.map((q) => `${r2(q.x)} ${r2(q.y)}`).join('L') + 'Z')
    .join('')
}

const r2 = (n: number): number => Math.round(n * 100) / 100

/** The corners of each loop of the union's outline, clockwise on screen. */
export function unionLoops(rects: Box[]): Pt[][] {
  const live = rects.filter((b) => w(b) > 0 && h(b) > 0)
  if (live.length === 0) return []
  const xs = [...new Set(live.flatMap((b) => [b.x0, b.x1]))].sort((a, b) => a - b)
  const ys = [...new Set(live.flatMap((b) => [b.y0, b.y1]))].sort((a, b) => a - b)
  const nx = xs.length - 1
  const ny = ys.length - 1
  const inside = (i: number, j: number): boolean => {
    if (i < 0 || j < 0 || i >= nx || j >= ny) return false
    const cx = (xs[i] + xs[i + 1]) / 2
    const cy = (ys[j] + ys[j + 1]) / 2
    return live.some((b) => cx > b.x0 && cx < b.x1 && cy > b.y0 && cy < b.y1)
  }
  // Each covered cell's sides that face an empty cell, directed so the
  // inside is on the right (clockwise on screen, y down). Grid indices.
  type E = { a: [number, number]; b: [number, number] }
  const edges: E[] = []
  for (let i = 0; i < nx; i++) {
    for (let j = 0; j < ny; j++) {
      if (!inside(i, j)) continue
      if (!inside(i, j - 1)) edges.push({ a: [i, j], b: [i + 1, j] })
      if (!inside(i + 1, j)) edges.push({ a: [i + 1, j], b: [i + 1, j + 1] })
      if (!inside(i, j + 1)) edges.push({ a: [i + 1, j + 1], b: [i, j + 1] })
      if (!inside(i - 1, j)) edges.push({ a: [i, j + 1], b: [i, j] })
    }
  }
  const key = (p: [number, number]): string => `${p[0]},${p[1]}`
  const from = new Map<string, E[]>()
  for (const e of edges) from.set(key(e.a), [...(from.get(key(e.a)) ?? []), e])
  const used = new Set<E>()
  const loops: Pt[][] = []
  for (const start of edges) {
    if (used.has(start)) continue
    const pts: [number, number][] = []
    let e: E | undefined = start
    while (e && !used.has(e)) {
      used.add(e)
      pts.push(e.a)
      const nexts: E[] = (from.get(key(e.b)) ?? []).filter((n) => !used.has(n))
      // Where two cells touch at a corner only, turn right: keeps loops simple.
      const prev: E = e
      const dir = (q: E): [number, number] => [Math.sign(q.b[0] - q.a[0]), Math.sign(q.b[1] - q.a[1])]
      const [dx, dy] = dir(prev)
      e = nexts.find((n) => {
        const [ex, ey] = dir(n)
        return dx * ey - dy * ex > 0
      }) ?? nexts[0]
    }
    // Drop the points in the middle of a straight run.
    const corners = pts.filter((q, n) => {
      const a = pts[(n - 1 + pts.length) % pts.length]
      const b = pts[(n + 1) % pts.length]
      return (q[0] - a[0]) * (b[1] - q[1]) - (q[1] - a[1]) * (b[0] - q[0]) !== 0
    })
    if (corners.length >= 4) loops.push(corners.map(([i, j]) => ({ x: xs[i], y: ys[j] })))
  }
  return loops
}

/** Cuts each outer corner of a clockwise loop (a right turn on screen). */
export function chamferLoop(loop: Pt[], cut: number): Pt[] {
  if (cut <= 0) return loop
  const out: Pt[] = []
  const n = loop.length
  for (let m = 0; m < n; m++) {
    const a = loop[(m - 1 + n) % n]
    const q = loop[m]
    const b = loop[(m + 1) % n]
    const inX = q.x - a.x
    const inY = q.y - a.y
    const outX = b.x - q.x
    const outY = b.y - q.y
    const lin = Math.hypot(inX, inY)
    const lout = Math.hypot(outX, outY)
    const convex = inX * outY - inY * outX > 0
    if (!convex) {
      out.push(q)
      continue
    }
    const c = Math.min(cut, lin / 2, lout / 2)
    out.push({ x: q.x - (inX / lin) * c, y: q.y - (inY / lin) * c }, { x: q.x + (outX / lout) * c, y: q.y + (outY / lout) * c })
  }
  return out
}
