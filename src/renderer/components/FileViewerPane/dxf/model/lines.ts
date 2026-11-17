// Linetypes and lineweights on the drawing-model path.
//
// Linetypes. A pattern is in drawing units, scaled by the entity's linetype
// scale (the model's `lt_scale` is already the entity's own scale × $LTSCALE,
// so it is used as it is; $CELTSCALE only sets the scale new objects get, so
// it never changes how an existing drawing looks). Lines are drawn with
// non-scaling strokes, so a dash array is in screen pixels: the viewer sets
// `--k2-ppu` (screen px per drawing unit) on the drawing as it zooms, and each
// dashed element's array is a CSS expression over it, so a zoom restyles the
// dashes without re-rendering the drawing. A placement sets `--k2i-s` (its
// scale) for its block's entities; a nested placement multiplies its outer
// one's (two levels exactly; deeper ones use the outermost and their own).
//
// A pattern too fine to see draws continuous, as CAD apps do: when its
// shortest dash or gap is under a screen pixel, every gap closes. A dot draws
// one lineweight long. A pattern runs along a whole path, so a polyline's
// pattern carries on across its vertices.
//
// Complex linetypes draw their dashes only. Not drawn yet: the text and the
// shapes they place along the line.
//
// Lineweights are screen pixels at a fixed 96 dpi, at least 1 px, and don't
// follow the zoom (AutoCAD's model-space display). With $LWDISPLAY off or
// absent everything draws at 1 px, as AutoCAD shows it. Default (and anything
// unresolved) is $LWDEFAULT, else 0.25 mm; ByLayer is the layer's weight;
// ByBlock inside a block is "insert", the placement's.

import type { ModelHeader, ModelLayer, ModelLinetype, ModelStyle } from './types'

/** Screen px per drawing unit, set on the drawing's <svg> by the viewer. */
export const PX_PER_UNIT = '--k2-ppu'
const UNIT = '--k2-u'
const SCALE = '--k2i-s'
const SCALE0 = '--k2i-s0'
const DASH = '--k2i-d'
const WIDTH = '--k2i-w'

const DPI = 96
const MM_PER_IN = 25.4
const DEFAULT_MM = 0.25

const n = (v: number): string => String(Number(v.toPrecision(6)))

export type Run = { len: number; dot: boolean }
/** Alternating dash and gap lengths (drawing units), starting with a dash, even in count. */
export type Pattern = { runs: Run[]; min: number }

export function patternOf(lt: ModelLinetype | undefined): Pattern | null {
  if (!lt || !Array.isArray(lt.dashes)) return null
  const runs: Run[] = []
  for (const e of lt.dashes) {
    if (typeof e !== 'number' || !Number.isFinite(e)) continue
    const gap = e < 0
    const len = Math.abs(e)
    if (!runs.length && gap) runs.push({ len: 0, dot: false })
    const gapNext = runs.length % 2 === 1
    if (runs.length && gapNext !== gap) {
      // Same kind as the run before: join them.
      const prev = runs[runs.length - 1]
      runs[runs.length - 1] = { len: prev.len + len, dot: prev.dot && len === 0 }
    } else runs.push({ len, dot: !gap && len === 0 })
  }
  if (runs.length % 2 === 1) runs.push({ len: 0, dot: false })
  const gaps = runs.filter((_, i) => i % 2 === 1)
  if (!gaps.some((g) => g.len > 0)) return null
  const lens = runs.map((r) => r.len).filter((l) => l > 0)
  return { runs, min: Math.min(...lens) }
}

/**
 * The CSS dash array for a pattern. `unit` is an expression for screen px per
 * pattern unit; `dot` is a dot's length (CSS). Gaps close when the shortest
 * run is under 1 px.
 */
function dashArray(p: Pattern, unit: string, dot: string): string {
  const open = `clamp(0, (${n(p.min)} * ${unit} - 1) * 1000, 1)`
  return p.runs
    .map((r, i) => {
      if (i % 2 === 0) return r.dot ? dot : r.len ? `calc(${n(r.len)} * ${unit} * 1px)` : '0px'
      return r.len ? `calc(${n(r.len)} * ${unit} * 1px * ${open})` : '0px'
    })
    .join(' ')
}

export type LineCtx = {
  settings: NonNullable<ModelHeader['settings']>
  linetypes: Map<string, ModelLinetype>
  layers: Map<string, ModelLayer>
}

export function lineCtx(header: ModelHeader): LineCtx {
  return {
    settings: header.settings ?? {},
    linetypes: new Map((header.linetypes ?? []).map((l) => [l.name.toUpperCase(), l])),
    layers: new Map(header.layers.map((l) => [l.name, l])),
  }
}

/** A weight in mm as screen px (at least 1), or null when lineweights are off (everything 1 px). */
export function lineweightPx(mm: number | undefined, settings: LineCtx['settings']): number | null {
  if (!settings.lwdisplay) return null
  const fallback = typeof settings.lwdefault_mm === 'number' && settings.lwdefault_mm >= 0 ? settings.lwdefault_mm : DEFAULT_MM
  const w = typeof mm === 'number' && Number.isFinite(mm) && mm >= 0 ? mm : fallback
  return Math.max(1, Math.round(((w / MM_PER_IN) * DPI) * 100) / 100)
}

/** A concrete weight (mm), ByLayer (-1) through the layer; Default, ByBlock at top level and unknowns stay unresolved. */
function weightMm(lw: number, ctx: LineCtx, layer: string | undefined): number | undefined {
  if (lw >= 0) return lw
  const l = lw === -1 && layer ? ctx.layers.get(layer) : undefined
  return typeof l?.lw_mm === 'number' && l.lw_mm >= 0 ? l.lw_mm : undefined
}

function linetype(name: string, ctx: LineCtx, layer: string | undefined): ModelLinetype | undefined {
  const up = name.toUpperCase()
  if (up === 'BYLAYER' && layer) {
    const l = ctx.layers.get(layer)
    return l?.ltype ? ctx.linetypes.get(l.ltype.toUpperCase()) : undefined
  }
  return ctx.linetypes.get(up)
}

const widthOf = (s: ModelStyle, ctx: LineCtx, layer: string | undefined): string | null => {
  if (!ctx.settings.lwdisplay) return null
  if (s.lw_mm === 'insert') return `var(${WIDTH}, 1px)`
  const px = lineweightPx(weightMm(s.lw_mm, ctx, layer), ctx.settings)
  return px === null ? null : `${n(px)}px`
}

/** An entity's dash array and width, as style properties (empty for a plain 1 px line). */
export function strokeOf(s: ModelStyle | undefined, ctx: LineCtx, layer?: string): Record<string, string> {
  const out: Record<string, string> = {}
  if (!s) return out
  const width = widthOf(s, ctx, layer)
  if (width) out.strokeWidth = width
  if (s.ltype === 'insert') {
    out.strokeDasharray = `var(${DASH}, none)`
    return out
  }
  const p = patternOf(linetype(s.ltype, ctx, layer))
  if (!p) return out
  const scale = typeof s.lt_scale === 'number' && s.lt_scale > 0 ? s.lt_scale : 1
  out[UNIT] = `calc(var(${PX_PER_UNIT}, 0) * var(${SCALE}, 1) * ${n(scale)})`
  out.strokeDasharray = dashArray(p, `var(${UNIT})`, width ?? '1px')
  return out
}

/** A placement's uniform scale (the square root of its matrix's area scale). */
const scaleOf = (m: readonly number[]): number => Math.sqrt(Math.abs(m[0] * m[3] - m[1] * m[2])) || 1

/**
 * What a placement hands its block's entities: its scale, its pattern (worked
 * out at its own scale) and its weight. A value that is itself "insert" (a
 * nested placement) hands nothing, so the outer one's flows through.
 */
export function insertLineVars(s: ModelStyle | undefined, ctx: LineCtx, m: readonly number[], nested: boolean, layer?: string): Record<string, string> {
  const k = scaleOf(m)
  const v: Record<string, string> = nested ? { [SCALE]: `calc(var(${SCALE0}, 1) * ${n(k)})` } : { [SCALE]: n(k), [SCALE0]: n(k) }
  if (!s) return v
  const width = widthOf(s, ctx, layer)
  if (width && s.lw_mm !== 'insert') v[WIDTH] = width
  if (s.ltype !== 'insert') {
    const p = patternOf(linetype(s.ltype, ctx, layer))
    const scale = typeof s.lt_scale === 'number' && s.lt_scale > 0 ? s.lt_scale : 1
    v[DASH] = p ? dashArray(p, `(var(${PX_PER_UNIT}, 0) * var(${SCALE}, 1) * ${n(scale)})`, width ?? '1px') : 'none'
  }
  return v
}

/** Whether any linetype in the header has a pattern (the viewer skips the zoom property otherwise). */
export const hasPatterns = (header: ModelHeader): boolean => (header.linetypes ?? []).some((l) => patternOf(l) !== null)

/** Sets screen px per drawing unit on the drawing for a view box in a pane (the box is "meet"-fitted). */
export function setPxPerUnit(svg: SVGSVGElement, pane: Element, v: { w: number; h: number }): void {
  const r = pane.getBoundingClientRect()
  if (!r.width || !r.height || !v.w || !v.h) return
  const value = n(1 / Math.max(v.w / r.width, v.h / r.height))
  if (svg.style.getPropertyValue(PX_PER_UNIT) !== value) svg.style.setProperty(PX_PER_UNIT, value)
}
