// The drawing from the model, as SVG in drawing coordinates (the group flips
// Y once). Every entity element carries its handle (`data-h`). Block
// definitions are drawn once, in <defs>, and each INSERT places one with a
// <use> (one per array cell), handing the block its own colour, alpha,
// layer state, linetype, lineweight and scale for the definition's "insert"
// values. Lines keep their width in screen pixels at any zoom (1 px, or their
// lineweight when the drawing shows lineweights); dashes follow the zoom
// (lines.ts).

import { useId } from 'react'
import type { TextMetrics } from '../text-metrics'
import { arcD, bulgeD, ellipsePoints, fmt, num, pointsOf, polyD, vertsOf, xy } from './geom'
import { HatchShape } from './hatch'
import { hasPatterns, insertLineVars, lineCtx, strokeOf, type LineCtx } from './lines'
import { insertVars, paintOf, paintProps, type Paint } from './paint'
import { MtextShape, TextShape } from './text'
import type { ModelHeader, ModelPage, ModelRow, XY } from './types'

const HAIR = 'non-scaling-stroke'

export type ModelScene = {
  header: ModelHeader
  rows: ModelRow[]
  blocks: ReadonlyMap<string, ModelPage>
  /** How far an XLINE or RAY reaches: well past the drawing. */
  reach: number
  /** The finest line spacing a dense hatch is tiled at, in drawing units. */
  minSpacing: number
}

/** `nested`: drawing a block definition, where a placement is inside another. */
type Ctx = { scene: ModelScene; m: TextMetrics; idp: string; layers: Map<string, { on: boolean; frozen: boolean }>; lines: LineCtx; nested: boolean }

const blockId = (idp: string, handle: string): string => `${idp}-b-${handle.replace(/^@h:/, '')}`

function pointD(p: XY, mode: number, size: number): string {
  const s = size / 2
  const [x, y] = p
  const parts: string[] = []
  const shape = mode & 7
  if (shape === 0) parts.push(`M${fmt(x)} ${fmt(y)} l0.0001 0`)
  if (shape === 2) parts.push(`M${fmt(x - s)} ${fmt(y)} H${fmt(x + s)} M${fmt(x)} ${fmt(y - s)} V${fmt(y + s)}`)
  if (shape === 3) parts.push(`M${fmt(x - s)} ${fmt(y - s)} L${fmt(x + s)} ${fmt(y + s)} M${fmt(x - s)} ${fmt(y + s)} L${fmt(x + s)} ${fmt(y - s)}`)
  if (shape === 4) parts.push(`M${fmt(x)} ${fmt(y)} V${fmt(y + s)}`)
  if (mode & 32) parts.push(arcD(p, s, 0, 2 * Math.PI))
  if (mode & 64) parts.push(`M${fmt(x - s)} ${fmt(y - s)} H${fmt(x + s)} V${fmt(y + s)} H${fmt(x - s)} Z`)
  return parts.join(' ')
}

/** A leader's arrowhead: the model's size and kind (a size of its own when the model gives only `true`). */
function arrowOf(v: unknown, insunits: number | undefined): { kind: string; size: number } | null {
  if (v === true) return { kind: 'closed_filled', size: insunits === 1 ? 0.18 : 2.5 }
  if (!v || typeof v !== 'object') return null
  const a = v as { shown?: unknown; kind?: unknown; size?: unknown }
  if (a.shown === false || a.kind === 'none') return null
  return { kind: typeof a.kind === 'string' ? a.kind : 'closed_filled', size: num(a.size, insunits === 1 ? 0.18 : 2.5) }
}

function Arrowhead({ at, toward, kind, size, paint }: { at: XY; toward: XY; kind: string; size: number; paint: Paint }): React.JSX.Element {
  const len = Math.hypot(toward[0] - at[0], toward[1] - at[1]) || 1
  const ux = (toward[0] - at[0]) / len
  const uy = (toward[1] - at[1]) / len
  const w = size / 6
  const l: XY = [at[0] + ux * size - uy * w, at[1] + uy * size + ux * w]
  const rr: XY = [at[0] + ux * size + uy * w, at[1] + uy * size - ux * w]
  const p = (q: XY) => `${fmt(q[0])} ${fmt(q[1])}`
  const common = { 'data-part': 'arrow', 'data-kind': kind, vectorEffect: HAIR }
  if (kind === 'dot') return <circle {...common} cx={at[0]} cy={at[1]} r={size / 2} {...paintProps(paint)} {...paintProps(paint, 'fill')} />
  if (kind === 'open' || kind === 'open30' || kind === 'open90') return <path {...common} d={`M${p(l)} L${p(at)} L${p(rr)}`} {...paintProps(paint)} />
  if (kind === 'tick' || kind === 'oblique' || kind === 'architectural_tick')
    return <path {...common} d={`M${fmt(at[0] - (ux - uy) * size * 0.5)} ${fmt(at[1] - (uy + ux) * size * 0.5)} L${fmt(at[0] + (ux - uy) * size * 0.5)} ${fmt(at[1] + (uy + ux) * size * 0.5)}`} {...paintProps(paint)} />
  const filled = kind !== 'closed' && kind !== 'closed_blank'
  return <path {...common} d={`M${p(at)} L${p(l)} L${p(rr)} Z`} {...paintProps(paint)} {...(filled ? paintProps(paint, 'fill') : {})} />
}

function Insert({ r, c }: { r: ModelRow; c: Ctx }): React.JSX.Element | null {
  const g = r.geom ?? {}
  const block = typeof g.block === 'string' ? g.block : null
  const m = Array.isArray(g.m) && g.m.length === 6 ? (g.m as number[]) : null
  const layer = c.layers.get(r.layer)
  // A placement on a frozen layer hides its whole block; on an off layer, only the block's layer-0 entities.
  if (r.visible !== 'insert' && layer?.frozen) return null
  if (r.visible === false && layer && layer.on) return null
  const on = r.visible === 'insert' ? 'insert' : (layer?.on ?? true)
  const vars = { ...insertVars(r.style, on), ...(m ? insertLineVars(r.style, c.lines, m, c.nested, r.layer) : {}) } as React.CSSProperties
  const uses: React.JSX.Element[] = []
  if (block && m && c.scene.blocks.has(block)) {
    const arr = (g.array ?? null) as { cols?: number; rows?: number; dx?: number; dy?: number } | null
    const cols = Math.max(1, num(arr?.cols, 1))
    const rows = Math.max(1, num(arr?.rows, 1))
    const sx = Math.hypot(m[0], m[1]) || 1
    const sy = Math.hypot(m[2], m[3]) || 1
    for (let i = 0; i < rows && uses.length < 400; i++) {
      for (let j = 0; j < cols && uses.length < 400; j++) {
        // An array cell steps in the placement's turned frame, before its scale.
        const off = i || j ? ` translate(${fmt((j * num(arr?.dx)) / sx)} ${fmt((i * num(arr?.dy)) / sy)})` : ''
        uses.push(<use key={`${i}-${j}`} href={`#${blockId(c.idp, block)}`} data-h={r.h} data-block={block} transform={`matrix(${m.map(fmt).join(' ')})${off}`} style={vars} />)
      }
    }
  }
  const attribs = Array.isArray(g.attribs) ? (g.attribs as Record<string, unknown>[]) : []
  const shown = attribs.filter((a) => a.visible !== false && a.text && typeof a.text === 'object')
  if (!uses.length && !shown.length) return null
  return (
    <>
      {uses}
      {shown.map((a, i) => (
        <TextShape key={`a${i}`} h={String(a.h ?? r.h)} g={a.text as Record<string, unknown>} style={r.style} visible={r.visible} m={c.m} />
      ))}
    </>
  )
}

function Row({ r, c }: { r: ModelRow; c: Ctx }): React.JSX.Element | null {
  if (!r.drawn || !r.geom) return null
  if (r.visible === false && r.kind !== 'INSERT') return null
  const g = r.geom
  const paint = paintOf(r.style, r.visible)
  // `plain`: points, fills and frames (no linetype, 1 px); `line`: with the row's linetype and weight.
  const pp = paintProps(paint)
  const plain = { 'data-h': r.h, vectorEffect: HAIR, ...pp }
  const strokes = strokeOf(r.style, c.lines, r.layer)
  const line = Object.keys(strokes).length ? { ...plain, style: { ...pp.style, ...strokes } as React.CSSProperties } : plain
  const path = (d: string) => (d ? <path {...line} d={d} /> : null)
  const plainPath = (d: string) => (d ? <path {...plain} d={d} /> : null)
  switch (r.kind) {
    case 'LINE': {
      const a = xy(g.p1)
      const b = xy(g.p2)
      return a && b ? <line {...line} x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} /> : null
    }
    case 'XLINE':
    case 'RAY': {
      const p = xy(g.point)
      const d = xy(g.dir)
      if (!p || !d) return null
      const L = c.scene.reach
      const a: XY = g.ray === true ? p : [p[0] - d[0] * L, p[1] - d[1] * L]
      return <line {...line} x1={a[0]} y1={a[1]} x2={p[0] + d[0] * L} y2={p[1] + d[1] * L} />
    }
    case 'POINT': {
      const p = xy(g.p)
      const size = num(g.pdsize, num(c.scene.header.settings?.pdsize))
      const mode = num(g.pdmode, num(c.scene.header.settings?.pdmode))
      return p ? plainPath(pointD(p, mode, size > 0 ? size : c.scene.minSpacing * 5)) : null
    }
    case 'CIRCLE': {
      const p = xy(g.centre)
      return p && num(g.r) > 0 ? <circle {...line} cx={p[0]} cy={p[1]} r={num(g.r)} /> : null
    }
    case 'ARC': {
      if (Array.isArray(g.path)) return path(polyD(pointsOf(g.path), false))
      const p = xy(g.centre)
      return p ? path(arcD(p, num(g.r), num(g.a0), num(g.a1))) : null
    }
    case 'ELLIPSE': {
      const p = xy(g.centre)
      const major = xy(g.major)
      if (!p || !major) return null
      const e = ellipsePoints(p, major, num(g.ratio, 1), num(g.p0), num(g.p1, 2 * Math.PI))
      return path(polyD(e.pts, e.full))
    }
    case 'LWPOLYLINE':
    case 'POLYLINE':
      return path(bulgeD(vertsOf(g.verts), g.closed === true))
    case 'SPLINE':
      return path(polyD(pointsOf(g.path), g.closed === true))
    case 'TEXT':
    case 'ATTRIB':
    case 'ATTDEF':
      return <TextShape h={r.h} g={g} style={r.style} visible={r.visible} m={c.m} />
    case 'MTEXT':
      return <MtextShape h={r.h} g={g} style={r.style} visible={r.visible} m={c.m} />
    case 'INSERT':
      return <Insert r={r} c={c} />
    case 'DIMENSION': {
      const block = typeof g.block === 'string' ? g.block : null
      const m = Array.isArray(g.m) && g.m.length === 6 ? (g.m as number[]) : [1, 0, 0, 1, 0, 0]
      if (!block || !c.scene.blocks.has(block)) return null
      const vars = { ...insertVars(r.style, true), ...insertLineVars(r.style, c.lines, m, c.nested, r.layer) } as React.CSSProperties
      return <use href={`#${blockId(c.idp, block)}`} data-h={r.h} data-block={block} transform={`matrix(${m.map(fmt).join(' ')})`} style={vars} />
    }
    case 'SOLID':
    case 'TRACE': {
      const q = pointsOf(g.quad)
      if (q.length < 3) return null
      return g.filled === false ? path(polyD(q, true)) : <path {...plain} d={polyD(q, true)} {...paintProps(paint, 'fill')} />
    }
    case '3DFACE': {
      const q = pointsOf(g.quad)
      const hidden = num(g.hidden_edges)
      const d = q.map((p, i) => ((hidden >> i) & 1 ? '' : `M${fmt(p[0])} ${fmt(p[1])} L${fmt(q[(i + 1) % q.length][0])} ${fmt(q[(i + 1) % q.length][1])}`)).join(' ')
      return path(d.trim())
    }
    case 'HATCH':
      return <HatchShape h={r.h} g={g} paint={paint} id={`${c.idp}-p-${r.h.replace(/^@h:/, '')}`} minSpacing={c.scene.minSpacing} />
    case 'LEADER': {
      const pts = pointsOf(g.path)
      if (pts.length < 2) return null
      const head = arrowOf(g.arrow, c.scene.header.units?.insunits)
      return (
        <g data-h={r.h}>
          <path {...line} data-h={undefined} d={polyD(pts, false)} />
          {head ? <Arrowhead at={pts[0]} toward={pts[1]} kind={head.kind} size={head.size} paint={paint} /> : null}
        </g>
      )
    }
    case 'MULTILEADER':
    case 'MLEADER': {
      // Each leader's lines run from their arrow; the last point meets the
      // landing, which carries on to the text. An arrowhead sits at each
      // line's start. The text is one line at `at`, placed by its attachment.
      const leaders = Array.isArray(g.leaders) ? (g.leaders as Record<string, unknown>[]) : []
      const head = arrowOf(g.arrow, c.scene.header.units?.insunits)
      const lines = leaders.flatMap((l) => {
        const landing = xy(l.landing)
        return (Array.isArray(l.lines) ? (l.lines as unknown[]) : []).map((p) => {
          const pts = pointsOf(p)
          const last = pts[pts.length - 1]
          return landing && last && (last[0] !== landing[0] || last[1] !== landing[1]) ? [...pts, landing] : pts
        })
      }).filter((pts) => pts.length >= 2)
      const text = mleaderText(g)
      if (!lines.length && !text) return null
      return (
        <g data-h={r.h}>
          {lines.length ? <path {...line} data-h={undefined} d={lines.map((pts) => polyD(pts, false)).join(' ')} /> : null}
          {head ? lines.map((pts, i) => <Arrowhead key={i} at={pts[0]} toward={pts[1]} kind={head.kind} size={head.size} paint={paint} />) : null}
          {text ? <TextShape h={r.h} g={text} style={r.style} visible={r.visible} m={c.m} /> : null}
        </g>
      )
    }
    case 'WIPEOUT': {
      const mask = pointsOf(g.mask)
      // Covers what is under it in the viewer's own background.
      return mask.length >= 3 ? <path data-h={r.h} d={polyD(mask, g.closed !== false)} stroke="none" style={{ fill: 'var(--color-bg)' }} /> : null
    }
    case 'IMAGE': {
      // Images aren't loaded yet: the frame, marked across, and its clip boundary when it has one.
      const q = pointsOf(g.corners)
      if (q.length < 4) return null
      const clip = pointsOf(g.clip)
      return plainPath(`${polyD(q, true)} M${fmt(q[0][0])} ${fmt(q[0][1])} L${fmt(q[2][0])} ${fmt(q[2][1])} M${fmt(q[1][0])} ${fmt(q[1][1])} L${fmt(q[3][0])} ${fmt(q[3][1])}${clip.length >= 3 ? ` ${polyD(clip, true)}` : ''}`)
    }
    default:
      return null
  }
}

/** A multileader's text as a one-line text shape: its plain text at `at`,
 *  aligned by its attachment (1 top left … 9 bottom right), or null. */
export function mleaderText(g: Record<string, unknown>): Record<string, unknown> | null {
  const plain = typeof g.plain === 'string' ? g.plain.replace(/\s*\n\s*/g, ' ').trim() : ''
  const at = xy(g.at)
  if (!plain || !at) return null
  const a = Math.min(9, Math.max(1, Math.round(num(g.attachment, 1)) || 1))
  const h = ['left', 'center', 'right'][(a - 1) % 3]
  const v = ['top', 'middle', 'bottom'][Math.floor((a - 1) / 3)]
  return { plain, anchor: at, height: num(g.height, 1) || 1, align: { h, v } }
}

export function ModelShapes({ scene, m }: { scene: ModelScene; m: TextMetrics }): React.JSX.Element {
  const idp = `k2m${useId().replace(/[^a-zA-Z0-9]/g, '')}`
  const layers = new Map(scene.header.layers.map((l) => [l.name, { on: l.on, frozen: l.frozen }]))
  const c: Ctx = { scene, m, idp, layers, lines: lineCtx(scene.header), nested: false }
  const inBlock: Ctx = { ...c, nested: true }
  return (
    <g transform="scale(1 -1)" fill="none" strokeWidth={1} style={{ fontFamily: 'var(--font-ui)' }} data-testid="dxf-shapes" data-source="model" data-dashed={hasPatterns(scene.header) ? '' : undefined}>
      <defs>
        {[...scene.blocks].map(([h, page]) => (
          <g key={h} id={blockId(idp, h)} data-block={h}>
            {page.entities.map((r) => (
              <Row key={r.h} r={r} c={inBlock} />
            ))}
          </g>
        ))}
      </defs>
      {scene.rows.map((r) => (
        <Row key={r.h} r={r} c={c} />
      ))}
    </g>
  )
}
