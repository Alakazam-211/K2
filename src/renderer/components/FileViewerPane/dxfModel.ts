// DXF → a small SVG-ready model. Loaded lazily by DxfView (with
// dxf-parser), so neither lands in the start-up chunk.
//
// Everything is kept in DXF coordinates (Y up). The view flips the whole
// drawing once with `scale(1,-1)`; arcs therefore use SVG sweep-flag 1 for
// DXF's counter-clockwise, and text flips itself back so it reads.
// Blocks (INSERT, DIMENSION) become nested <g transform> groups.

import DxfParser from 'dxf-parser'
import type {
  IArcEntity,
  ICircleEntity,
  IDimensionEntity,
  IDxf,
  IEllipseEntity,
  IEntity,
  IInsertEntity,
  ILineEntity,
  ILwpolylineEntity,
  IMtextEntity,
  IPoint,
  IPolylineEntity,
  ISplineEntity,
  ITextEntity,
} from 'dxf-parser'

/** Affine 2D: x' = a·x + c·y + e, y' = b·x + d·y + f (SVG `matrix()`). */
export type Mat = [number, number, number, number, number, number]

export type DxfNode =
  | { k: 'line'; x1: number; y1: number; x2: number; y2: number; c: string }
  | { k: 'circle'; cx: number; cy: number; r: number; c: string }
  | { k: 'path'; d: string; c: string; fill?: boolean }
  | {
      k: 'text'
      x: number
      y: number
      /** Cap height (font-size), DXF units. */
      h: number
      /** Degrees, counter-clockwise. */
      rot: number
      lines: string[]
      anchor: 'start' | 'middle' | 'end'
      /** First baseline, down from the anchor, in the text's own frame. */
      y0: number
      /** Line pitch for MTEXT. */
      lh: number
      c: string
    }
  | { k: 'group'; m: Mat; children: DxfNode[] }

export type Bounds = { minX: number; minY: number; maxX: number; maxY: number }

export type DxfDrawing = {
  nodes: DxfNode[]
  bounds: Bounds | null
  /** Entities drawn (block contents count once per insert). */
  drawn: number
  /** Model-space entities not drawn, by DXF type. */
  skipped: Record<string, number>
  skippedTotal: number
  /** Layer names that drew something (for the stats line). */
  layers: number
}

/** The theme foreground: ACI 7 is "white on black, black on white". */
export const FG = 'currentColor'
const MAX_NODES = 250_000
const MAX_DEPTH = 12
const MAX_ARRAY = 400

export function isBinaryDxf(text: string): boolean {
  return text.startsWith('AutoCAD Binary DXF')
}

// ── Colours ──

/** dxf-parser resolves ACI 1–255 to RGB (`color`); ACI 7 is white. */
function hex(rgb: number | undefined): string {
  if (typeof rgb !== 'number' || !Number.isFinite(rgb)) return FG
  // Pure white/black would vanish on one theme or the other.
  if (rgb === 0xffffff || rgb === 0) return FG
  return `#${rgb.toString(16).padStart(6, '0')}`
}

type LayerInfo = { c: string; hidden: boolean }

// ── Matrices ──

const IDENTITY: Mat = [1, 0, 0, 1, 0, 0]

function mul(p: Mat, q: Mat): Mat {
  return [
    p[0] * q[0] + p[2] * q[1],
    p[1] * q[0] + p[3] * q[1],
    p[0] * q[2] + p[2] * q[3],
    p[1] * q[2] + p[3] * q[3],
    p[0] * q[4] + p[2] * q[5] + p[4],
    p[1] * q[4] + p[3] * q[5] + p[5],
  ]
}
const translate = (x: number, y: number): Mat => [1, 0, 0, 1, x, y]
const scale = (sx: number, sy: number): Mat => [sx, 0, 0, sy, 0, 0]
function rotate(deg: number): Mat {
  const r = (deg * Math.PI) / 180
  const c = Math.cos(r)
  const s = Math.sin(r)
  return [c, s, -s, c, 0, 0]
}

// ── Geometry helpers ──

const n = (v: unknown, d = 0): number => (typeof v === 'number' && Number.isFinite(v) ? v : d)
const fmt = (v: number): string => {
  const r = Math.round(v * 1e4) / 1e4
  return Object.is(r, -0) ? '0' : String(r)
}

/** SVG arc from a polyline bulge (tan of a quarter of the included angle). */
function bulgeArc(x1: number, y1: number, x2: number, y2: number, bulge: number): string {
  const chord = Math.hypot(x2 - x1, y2 - y1)
  if (chord === 0) return ''
  const theta = 4 * Math.atan(Math.abs(bulge))
  const r = chord / (2 * Math.sin(theta / 2))
  const large = theta > Math.PI ? 1 : 0
  const sweep = bulge > 0 ? 1 : 0
  return ` A${fmt(r)} ${fmt(r)} 0 ${large} ${sweep} ${fmt(x2)} ${fmt(y2)}`
}

/** Points along a bulge segment, for the bounds. */
function bulgePoints(x1: number, y1: number, x2: number, y2: number, bulge: number): [number, number][] {
  const chord = Math.hypot(x2 - x1, y2 - y1)
  if (chord === 0 || bulge === 0) return []
  // Signed included angle; the centre sits left of the chord for CCW.
  const theta = 4 * Math.atan(bulge)
  const r = Math.abs(chord / (2 * Math.sin(theta / 2)))
  const d = chord / 2 / Math.tan(theta / 2)
  const cx = (x1 + x2) / 2 - ((y2 - y1) / chord) * d
  const cy = (y1 + y2) / 2 + ((x2 - x1) / chord) * d
  const a0 = Math.atan2(y1 - cy, x1 - cx)
  const out: [number, number][] = []
  for (let i = 1; i < 8; i++) {
    const a = a0 + (theta * i) / 8
    out.push([cx + r * Math.cos(a), cy + r * Math.sin(a)])
  }
  return out
}

/** Uniform B-spline evaluation (de Boor), weights ignored. */
function sampleSpline(e: ISplineEntity): [number, number][] {
  const cps = e.controlPoints ?? []
  const p = n(e.degreeOfSplineCurve, 3)
  const knots = e.knotValues ?? []
  if (cps.length >= 2 && p >= 1 && knots.length === cps.length + p + 1) {
    const lo = knots[p]
    const hi = knots[cps.length]
    if (hi > lo) {
      const steps = Math.min(2000, Math.max(32, cps.length * 10))
      const out: [number, number][] = []
      for (let s = 0; s <= steps; s++) {
        const t = s === steps ? hi - 1e-9 * (hi - lo) : lo + ((hi - lo) * s) / steps
        let k = p
        while (k < cps.length - 1 && knots[k + 1] <= t) k++
        const d = Array.from({ length: p + 1 }, (_, j) => [n(cps[j + k - p].x), n(cps[j + k - p].y)])
        for (let r = 1; r <= p; r++) {
          for (let j = p; j >= r; j--) {
            const i = j + k - p
            const den = knots[i + p - r + 1] - knots[i]
            const a = den === 0 ? 0 : (t - knots[i]) / den
            d[j] = [(1 - a) * d[j - 1][0] + a * d[j][0], (1 - a) * d[j - 1][1] + a * d[j][1]]
          }
        }
        out.push([d[p][0], d[p][1]])
      }
      return out
    }
  }
  const pts = e.fitPoints && e.fitPoints.length >= 2 ? e.fitPoints : cps
  return pts.map((q) => [n(q.x), n(q.y)])
}

/** Strip MTEXT inline formatting; `\P` splits lines. */
export function mtextLines(raw: string): string[] {
  let s = raw
    .replace(/\\\\/g, '\u0000')
    .replace(/\\U\+([0-9a-fA-F]{4})/g, (_, h: string) => String.fromCharCode(parseInt(h, 16)))
    .replace(/\\S([^;^/#]*)[\^/#]([^;]*);/g, '$1/$2')
    .replace(/\\[ACcFfHhQqTtWwp][^;\\]*;/g, '')
    .replace(/\\[LlOoKkNX]/g, '')
    .replace(/\\~/g, ' ')
    .replace(/[{}]/g, '')
    .replace(/\\P/g, '\n')
  s = decodeSpecials(s).replace(/\u0000/g, '\\')
  return s.split('\n')
}

/** `%%c` ⌀, `%%d` °, `%%p` ±, `%%u`/`%%o` toggles dropped. */
function decodeSpecials(s: string): string {
  return s
    .replace(/%%[cC]/g, '⌀')
    .replace(/%%[dD]/g, '°')
    .replace(/%%[pP]/g, '±')
    .replace(/%%[uUoOkK]/g, '')
    .replace(/%%%/g, '%')
    .replace(/\\U\+([0-9a-fA-F]{4})/g, (_, h: string) => String.fromCharCode(parseInt(h, 16)))
}

// ── Model-space census (dxf-parser drops types it has no handler for) ──

const CHILD_TYPES = new Set(['VERTEX', 'SEQEND', 'ATTRIB', 'ENDSEC'])

/** Top-level ENTITIES by type, straight from the group codes. */
export function censusEntities(text: string): Record<string, number> {
  const lines = text.split(/\r?\n/)
  const out: Record<string, number> = {}
  let inEntities = false
  for (let i = 0; i + 1 < lines.length; i += 2) {
    if (lines[i].trim() !== '0') continue
    const v = lines[i + 1].trim()
    if (v === 'SECTION') {
      inEntities = lines[i + 2]?.trim() === '2' && lines[i + 3]?.trim() === 'ENTITIES'
      continue
    }
    if (v === 'ENDSEC') {
      if (inEntities) break
      continue
    }
    if (!inEntities || CHILD_TYPES.has(v)) continue
    out[v] = (out[v] ?? 0) + 1
  }
  return out
}

// ── Build ──

type Ctx = {
  m: Mat
  depth: number
  /** Colour for ByBlock (ACI 0) entities: the INSERT's own. */
  blockColor: string
  /** Layer "0" inside a block takes the INSERT's layer. */
  insertLayer: string | null
}

export function buildDrawing(text: string): DxfDrawing {
  if (isBinaryDxf(text)) throw new Error('Binary DXF is not supported. Save it as ASCII DXF.')
  const parser = new DxfParser()
  let dxf: IDxf | null
  try {
    dxf = parser.parseSync(text)
  } catch (e) {
    throw new Error(`This DXF could not be read: ${e instanceof Error ? e.message : String(e)}`)
  }
  if (!dxf) throw new Error('This DXF could not be read.')
  return drawingFromDxf(dxf, censusEntities(text))
}

export function drawingFromDxf(dxf: IDxf, census: Record<string, number> = {}): DxfDrawing {
  const layers = new Map<string, LayerInfo>()
  for (const [name, l] of Object.entries(dxf.tables?.layer?.layers ?? {})) {
    layers.set(name, { c: hex(l.color), hidden: l.visible === false || l.frozen === true })
  }
  const blocks = dxf.blocks ?? {}
  const bounds: Bounds = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity }
  const usedLayers = new Set<string>()
  const skipped: Record<string, number> = {}
  let drawn = 0
  let nodeCount = 0

  const add = (m: Mat, x: number, y: number) => {
    const X = m[0] * x + m[2] * y + m[4]
    const Y = m[1] * x + m[3] * y + m[5]
    if (!Number.isFinite(X) || !Number.isFinite(Y)) return
    if (X < bounds.minX) bounds.minX = X
    if (X > bounds.maxX) bounds.maxX = X
    if (Y < bounds.minY) bounds.minY = Y
    if (Y > bounds.maxY) bounds.maxY = Y
  }

  const layerOf = (e: IEntity, ctx: Ctx) =>
    (e.layer === '0' || !e.layer) && ctx.insertLayer ? ctx.insertLayer : (e.layer ?? '0')

  const colorOf = (e: IEntity, ctx: Ctx): string => {
    const idx = e.colorIndex
    if (idx === 0) return ctx.blockColor
    if (typeof idx === 'number' && idx > 0 && idx < 256) return hex(e.color)
    return layers.get(layerOf(e, ctx))?.c ?? FG
  }

  const skip = (type: string, top: boolean) => {
    if (top) skipped[type] = (skipped[type] ?? 0) + 1
  }

  function polyPath(
    verts: { x: number; y: number; bulge?: number }[],
    closed: boolean,
    ctx: Ctx,
    mirrorX = false,
  ): string {
    const pts = verts.map((v) => ({
      x: mirrorX ? -n(v.x) : n(v.x),
      y: n(v.y),
      b: (mirrorX ? -1 : 1) * n(v.bulge),
    }))
    if (pts.length < 2) return ''
    let d = `M${fmt(pts[0].x)} ${fmt(pts[0].y)}`
    const segs = closed ? pts.length : pts.length - 1
    for (let i = 0; i < segs; i++) {
      const a = pts[i]
      const b = pts[(i + 1) % pts.length]
      add(ctx.m, a.x, a.y)
      if (a.b) {
        d += bulgeArc(a.x, a.y, b.x, b.y, a.b) || ` L${fmt(b.x)} ${fmt(b.y)}`
        for (const [x, y] of bulgePoints(a.x, a.y, b.x, b.y, a.b)) add(ctx.m, x, y)
      } else {
        d += ` L${fmt(b.x)} ${fmt(b.y)}`
      }
      add(ctx.m, b.x, b.y)
    }
    return closed ? `${d} Z` : d
  }

  function arcNode(cx: number, cy: number, r: number, a0: number, a1: number, c: string, ctx: Ctx): DxfNode {
    let span = a1 - a0
    while (span <= 0) span += 2 * Math.PI
    while (span > 2 * Math.PI) span -= 2 * Math.PI
    const sx = cx + r * Math.cos(a0)
    const sy = cy + r * Math.sin(a0)
    const ex = cx + r * Math.cos(a0 + span)
    const ey = cy + r * Math.sin(a0 + span)
    for (let i = 0; i <= 16; i++) {
      const a = a0 + (span * i) / 16
      add(ctx.m, cx + r * Math.cos(a), cy + r * Math.sin(a))
    }
    const large = span > Math.PI ? 1 : 0
    return {
      k: 'path',
      d: `M${fmt(sx)} ${fmt(sy)} A${fmt(r)} ${fmt(r)} 0 ${large} 1 ${fmt(ex)} ${fmt(ey)}`,
      c,
    }
  }

  function textNode(
    x: number,
    y: number,
    h: number,
    rot: number,
    lines: string[],
    anchor: 'start' | 'middle' | 'end',
    y0: number,
    lh: number,
    c: string,
    ctx: Ctx,
  ): DxfNode | null {
    const visible = lines.filter((l) => l.length > 0)
    if (!visible.length) return null
    // Rough extent for the fit: 0.6em per character.
    const w = Math.max(...lines.map((l) => l.length)) * h * 0.6
    const r = (rot * Math.PI) / 180
    const dx = anchor === 'start' ? 0 : anchor === 'middle' ? -w / 2 : -w
    for (const [px, py] of [
      [dx, -y0 + h],
      [dx + w, -y0 + h],
      [dx, -y0 - lh * (lines.length - 1)],
      [dx + w, -y0 - lh * (lines.length - 1)],
    ]) {
      add(ctx.m, x + px * Math.cos(r) - py * Math.sin(r), y + px * Math.sin(r) + py * Math.cos(r))
    }
    return { k: 'text', x, y, h, rot, lines, anchor, y0, lh, c }
  }

  function entity(e: IEntity, ctx: Ctx, top: boolean): DxfNode | null {
    if (nodeCount >= MAX_NODES) {
      skip(e.type, top)
      return null
    }
    if (e.visible === false || e.inPaperSpace) return null
    const layer = layerOf(e, ctx)
    if (layers.get(layer)?.hidden) return null
    const c = colorOf(e, ctx)
    let node: DxfNode | null = null

    switch (e.type) {
      case 'LINE': {
        const v = (e as ILineEntity).vertices ?? []
        if (v.length >= 2) {
          node = { k: 'line', x1: n(v[0].x), y1: n(v[0].y), x2: n(v[1].x), y2: n(v[1].y), c }
          add(ctx.m, node.x1, node.y1)
          add(ctx.m, node.x2, node.y2)
        }
        break
      }
      case 'CIRCLE': {
        const ce = e as ICircleEntity & { extrusionDirectionZ?: number }
        const sign = n(ce.extrusionDirectionZ, 1) < 0 ? -1 : 1
        const cx = sign * n(ce.center?.x)
        const cy = n(ce.center?.y)
        const r = Math.abs(n(ce.radius))
        if (r > 0) {
          node = { k: 'circle', cx, cy, r, c }
          for (let i = 0; i < 8; i++) {
            const a = (i * Math.PI) / 4
            add(ctx.m, cx + r * Math.cos(a), cy + r * Math.sin(a))
          }
        }
        break
      }
      case 'ARC': {
        const a = e as IArcEntity
        const r = Math.abs(n(a.radius))
        if (r > 0) {
          // OCS with -Z extrusion (mirrored arcs): x flips, angles reflect.
          if (n(a.extrusionDirectionZ, 1) < 0) {
            node = arcNode(
              -n(a.center?.x),
              n(a.center?.y),
              r,
              Math.PI - n(a.endAngle),
              Math.PI - n(a.startAngle),
              c,
              ctx,
            )
          } else {
            node = arcNode(n(a.center?.x), n(a.center?.y), r, n(a.startAngle), n(a.endAngle), c, ctx)
          }
        }
        break
      }
      case 'LWPOLYLINE': {
        const p = e as ILwpolylineEntity
        const d = polyPath(p.vertices ?? [], Boolean(p.shape), ctx, n(p.extrusionDirectionZ, 1) < 0)
        if (d) node = { k: 'path', d, c }
        break
      }
      case 'POLYLINE': {
        const p = e as IPolylineEntity
        if (p.isPolyfaceMesh || p.is3dPolygonMesh) break
        const verts = (p.vertices ?? []).filter((v) => !v.splineControlPoint)
        const d = polyPath(verts, Boolean(p.shape), ctx, n(p.extrusionDirection?.z, 1) < 0)
        if (d) node = { k: 'path', d, c }
        break
      }
      case 'ELLIPSE': {
        const el = e as IEllipseEntity
        const cx = n(el.center?.x)
        const cy = n(el.center?.y)
        const mx = n(el.majorAxisEndPoint?.x)
        const my = n(el.majorAxisEndPoint?.y)
        const ratio = n(el.axisRatio, 1)
        let t0 = n(el.startAngle, 0)
        let t1 = n(el.endAngle, 2 * Math.PI)
        while (t1 <= t0) t1 += 2 * Math.PI
        const full = t1 - t0 >= 2 * Math.PI - 1e-6
        const steps = Math.max(8, Math.ceil((72 * (t1 - t0)) / (2 * Math.PI)))
        let d = ''
        for (let i = 0; i <= steps; i++) {
          const t = t0 + ((t1 - t0) * i) / steps
          const x = cx + mx * Math.cos(t) - ratio * my * Math.sin(t)
          const y = cy + my * Math.cos(t) + ratio * mx * Math.sin(t)
          add(ctx.m, x, y)
          d += `${i ? ' L' : 'M'}${fmt(x)} ${fmt(y)}`
        }
        if (mx || my) node = { k: 'path', d: full ? `${d} Z` : d, c }
        break
      }
      case 'SPLINE': {
        const s = e as ISplineEntity
        const pts = sampleSpline(s)
        if (pts.length >= 2) {
          let d = ''
          pts.forEach(([x, y], i) => {
            add(ctx.m, x, y)
            d += `${i ? ' L' : 'M'}${fmt(x)} ${fmt(y)}`
          })
          node = { k: 'path', d: s.closed ? `${d} Z` : d, c }
        }
        break
      }
      case 'SOLID':
      case '3DFACE': {
        const pts = ((e as unknown as { points?: IPoint[]; vertices?: IPoint[] }).points ??
          (e as unknown as { vertices?: IPoint[] }).vertices ??
          []) as IPoint[]
        if (pts.length >= 3) {
          // SOLID corners run 1-2-4-3.
          const order = e.type === 'SOLID' && pts.length === 4 ? [pts[0], pts[1], pts[3], pts[2]] : pts
          const d = polyPath(order, true, ctx)
          if (d) node = { k: 'path', d, c, fill: e.type === 'SOLID' }
        }
        break
      }
      case 'TEXT': {
        const t = e as ITextEntity
        const h = n(t.textHeight, 1) || 1
        const aligned = (t.halign || t.valign) && t.endPoint
        const at = aligned ? t.endPoint : t.startPoint
        const anchor = t.halign === 1 || t.halign === 4 ? 'middle' : t.halign === 2 ? 'end' : 'start'
        const y0 = t.valign === 3 ? h : t.valign === 2 ? h / 2 : t.valign === 1 ? -0.2 * h : 0
        node = textNode(n(at?.x), n(at?.y), h, n(t.rotation), [decodeSpecials(t.text ?? '')], anchor, y0, h, c, ctx)
        break
      }
      case 'MTEXT': {
        const t = e as IMtextEntity
        const h = n(t.height, 1) || 1
        const lines = mtextLines(t.text ?? '')
        const lh = (h * 5) / 3
        const ap = Math.min(9, Math.max(1, n(t.attachmentPoint, 1)))
        const col = (ap - 1) % 3
        const row = Math.floor((ap - 1) / 3)
        const anchor = col === 0 ? 'start' : col === 1 ? 'middle' : 'end'
        const span = h + lh * (lines.length - 1)
        const y0 = row === 0 ? h : row === 1 ? h - span / 2 : h - span
        const rot = t.directionVector
          ? (Math.atan2(n(t.directionVector.y), n(t.directionVector.x)) * 180) / Math.PI
          : n(t.rotation)
        node = textNode(n(t.position?.x), n(t.position?.y), h, rot, lines, anchor, y0, lh, c, ctx)
        break
      }
      case 'INSERT': {
        const ins = e as IInsertEntity
        const block = blocks[ins.name]
        if (!block || ctx.depth >= MAX_DEPTH) break
        const base = block.position ?? { x: 0, y: 0, z: 0 }
        const cols = Math.max(1, n(ins.columnCount, 1))
        const rows = Math.max(1, n(ins.rowCount, 1))
        const children: DxfNode[] = []
        let copies = 0
        for (let r = 0; r < rows && copies < MAX_ARRAY; r++) {
          for (let k = 0; k < cols && copies < MAX_ARRAY; k++, copies++) {
            let m = mul(translate(n(ins.position?.x), n(ins.position?.y)), rotate(n(ins.rotation)))
            m = mul(m, translate(k * n(ins.columnSpacing), r * n(ins.rowSpacing)))
            m = mul(m, scale(n(ins.xScale, 1) || 1, n(ins.yScale, 1) || 1))
            m = mul(m, translate(-n(base.x), -n(base.y)))
            const g = blockGroup(block.entities ?? [], m, ctx, c, layer)
            if (g) children.push(g)
          }
        }
        if (children.length) node = children.length === 1 ? children[0] : { k: 'group', m: IDENTITY, children }
        break
      }
      case 'DIMENSION': {
        // The dimension's look lives in an anonymous block, already in WCS.
        const block = blocks[(e as IDimensionEntity).block]
        if (!block || ctx.depth >= MAX_DEPTH) break
        node = blockGroup(block.entities ?? [], IDENTITY, ctx, c, layer)
        break
      }
      default:
        break
    }

    if (node) {
      drawn++
      nodeCount++
      usedLayers.add(layer)
    } else if (top) {
      skip(e.type, top)
    }
    return node
  }

  function blockGroup(ents: IEntity[], local: Mat, ctx: Ctx, color: string, layer: string): DxfNode | null {
    const inner: Ctx = { m: mul(ctx.m, local), depth: ctx.depth + 1, blockColor: color, insertLayer: layer }
    const children: DxfNode[] = []
    for (const child of ents) {
      const nd = entity(child, inner, false)
      if (nd) children.push(nd)
    }
    return children.length ? { k: 'group', m: local, children } : null
  }

  const root: Ctx = { m: IDENTITY, depth: 0, blockColor: FG, insertLayer: null }
  const nodes: DxfNode[] = []
  const parsedByType: Record<string, number> = {}
  for (const e of dxf.entities ?? []) {
    parsedByType[e.type] = (parsedByType[e.type] ?? 0) + 1
    const nd = entity(e, root, true)
    if (nd) nodes.push(nd)
  }
  // Types dxf-parser has no handler for (HATCH, LEADER, …) never reach
  // `entities`: the census counts them.
  for (const [type, count] of Object.entries(census)) {
    const missing = count - (parsedByType[type] ?? 0)
    if (missing > 0) skipped[type] = (skipped[type] ?? 0) + missing
  }
  const skippedTotal = Object.values(skipped).reduce((a, b) => a + b, 0)
  const has = Number.isFinite(bounds.minX) && Number.isFinite(bounds.minY)
  return { nodes, bounds: has ? bounds : null, drawn, skipped, skippedTotal, layers: usedLayers.size }
}
