// A HATCH from the model: its boundary loops as one even-odd path (islands
// cut out), then
//   - solid: filled;
//   - pattern with its lines: the model's lines, already clipped and dashed;
//   - dense (too many lines to send): the pattern tiled inside the boundary
//     (an SVG pattern of the definition lines, filling the loops), with the
//     line spacing kept to at least a fine screen hatch so it reads as a
//     pattern rather than a solid;
//   - gradient: filled in its colour, as a flat fill.

import { bulgeD, ellipsePoints, fmt, num, pointsOf, polyD, vertsOf, xy } from './geom'
import { paintProps, type Paint } from './paint'
import type { XY } from './types'

type Edge = Record<string, unknown>

function edgeD(e: Edge, first: boolean): string {
  const line = pointsOf(e.line)
  if (line.length === 2) return `${first ? `M${fmt(line[0][0])} ${fmt(line[0][1])} ` : ''}L${fmt(line[1][0])} ${fmt(line[1][1])}`
  const arc = (e.arc ?? null) as Record<string, unknown> | null
  if (arc) {
    const c = xy(arc.c ?? arc.centre)
    if (c) {
      // Sampled in the edge's own direction, so a clockwise edge joins its neighbours.
      const r = num(arc.r)
      const a0 = num(arc.a0)
      let span = num(arc.a1) - a0
      if (arc.ccw === false) while (span >= 0) span -= 2 * Math.PI
      else while (span <= 0) span += 2 * Math.PI
      const n = Math.max(8, Math.ceil((Math.abs(span) / (2 * Math.PI)) * 72))
      const pts: XY[] = Array.from({ length: n + 1 }, (_, i) => [c[0] + r * Math.cos(a0 + (span * i) / n), c[1] + r * Math.sin(a0 + (span * i) / n)])
      return polyD(pts, false).replace(/^M/, first ? 'M' : 'L')
    }
  }
  const el = (e.ellipse ?? null) as Record<string, unknown> | null
  if (el) {
    const c = xy(el.c ?? el.centre)
    const major = xy(el.major)
    if (c && major) return polyD(ellipsePoints(c, major, num(el.ratio, 1), num(el.p0), num(el.p1, 2 * Math.PI)).pts, false).replace(/^M/, first ? 'M' : 'L')
  }
  const sp = e.spline
  const path = pointsOf(Array.isArray(sp) ? sp : (sp as { path?: unknown } | undefined)?.path)
  if (path.length) return polyD(path, false).replace(/^M/, first ? 'M' : 'L')
  return ''
}

/** The loops as one path: polylines with bulges, or edge lists. */
export function loopsD(loops: unknown): string {
  if (!Array.isArray(loops)) return ''
  return loops
    .map((l: Record<string, unknown>) => {
      const poly = l.poly as { verts?: unknown; closed?: unknown } | undefined
      if (poly) return bulgeD(vertsOf(poly.verts), true)
      if (Array.isArray(l.edges)) return `${(l.edges as Edge[]).map((e, i) => edgeD(e, i === 0)).join(' ')} Z`
      return ''
    })
    .filter(Boolean)
    .join(' ')
}

type Def = { angle?: number; base?: unknown; offset?: unknown; dashes?: unknown }

export function HatchShape(p: { h: string; g: Record<string, unknown>; paint: Paint; id: string; minSpacing: number }): React.JSX.Element | null {
  const { g, paint } = p
  const d = loopsD(g.loops)
  if (!d) return null
  const pattern = (g.pattern ?? null) as { lines?: unknown; dense?: boolean; defs?: Def[] } | null
  if (g.fill === 'solid' || g.fill === 'gradient' || !pattern) {
    return <path data-h={p.h} d={d} fillRule="evenodd" stroke="none" {...paintProps(paint, 'fill')} />
  }
  if (Array.isArray(pattern.lines)) {
    const segs = (pattern.lines as unknown[]).filter((s): s is number[] => Array.isArray(s) && s.length === 4)
    const lines = segs.map((s) => `M${fmt(s[0])} ${fmt(s[1])} L${fmt(s[2])} ${fmt(s[3])}`).join(' ')
    return (
      <g data-h={p.h}>
        <path data-part="lines" d={lines} {...paintProps(paint)} vectorEffect="non-scaling-stroke" />
      </g>
    )
  }
  // Dense: tile each definition line's family, at its angle and spacing, inside the boundary.
  const defs = (pattern.defs ?? []).filter((x) => x && typeof x === 'object')
  const def = defs[0] ?? { angle: 0, offset: [0, 1] }
  const off = xy(def.offset) ?? ([0, 1] as XY)
  const a = num(def.angle)
  // The spacing is the offset across the line's direction.
  const across = Math.abs(-Math.sin(a) * off[0] + Math.cos(a) * off[1]) || Math.hypot(off[0], off[1])
  const spacing = Math.max(across, p.minSpacing)
  const base = xy(def.base) ?? ([0, 0] as XY)
  const colour = paint.stroke
  return (
    <>
      <defs>
        <pattern
          id={p.id}
          patternUnits="userSpaceOnUse"
          width={spacing}
          height={spacing}
          patternTransform={`translate(${fmt(base[0])} ${fmt(base[1])}) rotate(${fmt((a * 180) / Math.PI)})`}
          data-spacing={spacing}
          data-floor={spacing > across ? 'true' : 'false'}
        >
          <line x1={0} y1={0} x2={spacing} y2={0} strokeWidth={spacing * 0.18} style={{ stroke: colour }} />
        </pattern>
      </defs>
      <path data-h={p.h} d={d} fill={`url(#${p.id})`} fillRule="evenodd" stroke="none" style={paint.opacity !== undefined ? { opacity: paint.opacity } : undefined} />
    </>
  )
}
