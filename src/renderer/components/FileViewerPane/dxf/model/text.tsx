// Text from the model. The model places it (the anchor its alignment names,
// both points for aligned and fit text, codes already turned into runs); the
// viewer measures it with the font it draws in:
//   - a height is the cap height, so the font size is height ÷ the font's cap ratio;
//   - aligned and fit text run from the first point to the second (aligned
//     also scales its height so its natural width fills the run);
//   - MTEXT wraps at its reference width, word by word, each run keeping its
//     bold, italic, colour, height and decoration.

import type { TextMetrics } from '../text-metrics'
import { fmt, num, xy } from './geom'
import { paintProps, paintOf, rgbOf } from './paint'
import type { ModelStyle, RGB, XY } from './types'

export type Run = {
  text: string
  bold?: boolean
  italic?: boolean
  underline?: boolean
  overline?: boolean
  strike?: boolean
  rgb?: RGB | null
  height?: number
  font?: string
  stack?: unknown
}

const runsOf = (v: unknown): Run[] =>
  Array.isArray(v) ? v.filter((r): r is Run => typeof (r as Run)?.text === 'string') : []

const deg = (rad: number): string => fmt((rad * 180) / Math.PI)

/** Where the first baseline sits below the anchor, by vertical alignment (cap height h). */
const baselineBelow = (v: string, h: number): number => (v === 'top' ? h : v === 'middle' ? h / 2 : v === 'bottom' ? -0.2 * h : 0)

function decoration(r: Run): string | undefined {
  const d = [r.underline && 'underline', r.overline && 'overline', r.strike && 'line-through'].filter(Boolean)
  return d.length ? d.join(' ') : undefined
}

function runProps(r: Run, cap: number): Record<string, unknown> {
  const p: Record<string, unknown> = {}
  if (r.bold) p.fontWeight = 700
  if (r.italic) p.fontStyle = 'italic'
  if (r.rgb) p.fill = rgbOf(r.rgb)
  if (typeof r.height === 'number' && r.height > 0) p.fontSize = r.height / cap
  const dec = decoration(r)
  if (dec) p.textDecoration = dec
  return p
}

/** TEXT, ATTRIB, and attribute text on an INSERT. */
export function TextShape({ h, g, style, visible, m }: { h: string; g: Record<string, unknown>; style?: ModelStyle; visible: boolean | 'insert'; m: TextMetrics }): React.JSX.Element | null {
  const height = num(g.height, 1) || 1
  const runs = runsOf(g.runs)
  const plain = typeof g.plain === 'string' ? g.plain : runs.map((r) => r.text).join('')
  if (!plain.trim()) return null
  const align = (g.align ?? {}) as { h?: string; v?: string }
  const ah = align.h ?? 'left'
  const width = num(g.width, 1) || 1
  const from = xy(g.from)
  const to = xy(g.to)
  let at = xy(g.anchor) ?? [0, 0]
  let rot = num(g.rotation)
  let cap = height
  let len: number | undefined
  if ((ah === 'aligned' || ah === 'fit') && from && to) {
    at = from
    len = Math.hypot(to[0] - from[0], to[1] - from[1])
    rot = Math.atan2(to[1] - from[1], to[0] - from[0])
    if (ah === 'aligned') {
      const natural = m.advance(plain) * width
      if (natural > 0) cap = (len / natural) * m.cap
    }
  }
  const anchor = ah === 'center' || ah === 'middle' ? 'middle' : ah === 'right' ? 'end' : 'start'
  const y0 = len !== undefined ? 0 : ah === 'middle' ? cap / 2 : baselineBelow(align.v ?? 'baseline', cap)
  const t = [`translate(${fmt(at[0])} ${fmt(at[1])}) scale(1 -1)`]
  if (rot) t.push(`rotate(${fmt(-Number(deg(rot)))})`)
  if (num(g.oblique)) t.push(`skewX(${fmt(-Number(deg(num(g.oblique))))})`)
  if (len === undefined && width !== 1) t.push(`scale(${fmt(width)} 1)`)
  if (g.backwards === true) t.push('scale(-1 1)')
  if (g.upside_down === true) t.push('scale(1 -1)')
  const paint = paintOf(style, visible)
  return (
    <text data-h={h} transform={t.join(' ')} fontSize={cap / m.cap} textAnchor={anchor} stroke="none" {...paintProps(paint, 'fill')}>
      <tspan x={0} y={y0} {...(len !== undefined ? { textLength: len, lengthAdjust: 'spacingAndGlyphs' } : {})}>
        {runs.length > 1 || (runs[0] && decoration(runs[0]))
          ? runs.map((r, i) => (
              <tspan key={i} {...runProps(r, m.cap)}>
                {r.text}
              </tspan>
            ))
          : plain}
      </tspan>
    </text>
  )
}

/** Runs into lines no wider than `width` (0: no wrapping), breaking between words. */
export function wrapRuns(runs: readonly Run[], width: number, size: number, advance: (s: string) => number, sizeOf: (r: Run) => number = () => size): Run[][] {
  const lines: { run: Run; from: Run }[][] = [[]]
  let w = 0
  for (const r of runs) {
    const fs = sizeOf(r)
    // Words with their trailing spaces; a no-break space is part of its word.
    for (const tok of r.text.match(/[^ ]+ *| +/g) ?? []) {
      if (width > 0 && lines[lines.length - 1].length && w + advance(tok.trimEnd()) * fs > width + 1e-9) {
        lines.push([])
        w = 0
      }
      const cur = lines[lines.length - 1]
      const last = cur[cur.length - 1]
      if (last?.from === r) last.run.text += tok
      else cur.push({ run: { ...r, text: tok }, from: r })
      w += advance(tok) * fs
    }
  }
  return lines.map((l) => l.map((x) => x.run))
}

type Paragraph = { align?: string | null; runs?: unknown }

export function MtextShape({ h, g, style, visible, m }: { h: string; g: Record<string, unknown>; style?: ModelStyle; visible: boolean | 'insert'; m: TextMetrics }): React.JSX.Element | null {
  const height = num(g.height, 1) || 1
  const at = xy(g.anchor) ?? ([0, 0] as XY)
  const attach = (g.attach ?? {}) as { h?: string; v?: string }
  const base = height / m.cap
  const sizeOf = (r: Run) => (typeof r.height === 'number' && r.height > 0 ? r.height / m.cap : base)
  const refWidth = num(g.ref_width)
  const paras = Array.isArray(g.paragraphs) ? (g.paragraphs as Paragraph[]) : []
  const lines = paras.flatMap((p) => wrapRuns(runsOf(p.runs), refWidth, base, m.advance, sizeOf))
  if (!lines.some((l) => l.some((r) => r.text.trim()))) return null
  // Each line steps down by its tallest run (5/3 of the cap height, times the spacing factor).
  const spacing = num(g.line_spacing, 1) || 1
  const tallest = lines.map((l) => Math.max(height, ...l.map((r) => (typeof r.height === 'number' && r.height > 0 ? r.height : height))))
  const ys: number[] = []
  tallest.forEach((t, i) => ys.push(i ? ys[i - 1] + ((t * 5) / 3) * spacing : t))
  const span = ys[ys.length - 1]
  const v = attach.v ?? 'top'
  const shift = v === 'top' ? 0 : v === 'middle' ? -span / 2 : -span
  const ah = attach.h ?? 'left'
  const anchor = ah === 'center' ? 'middle' : ah === 'right' ? 'end' : 'start'
  const rot = num(g.rotation)
  const paint = paintOf(style, visible)
  return (
    <text
      data-h={h}
      transform={`translate(${fmt(at[0])} ${fmt(at[1])}) scale(1 -1)${rot ? ` rotate(${fmt(-(rot * 180) / Math.PI)})` : ''}`}
      fontSize={base}
      textAnchor={anchor}
      stroke="none"
      {...paintProps(paint, 'fill')}
    >
      {lines.map((line, i) => (
        <tspan key={i} x={0} y={ys[i] + shift}>
          {line.map((r, j) => (
            <tspan key={j} {...runProps(r, m.cap)}>
              {r.text}
            </tspan>
          ))}
        </tspan>
      ))}
    </text>
  )
}
