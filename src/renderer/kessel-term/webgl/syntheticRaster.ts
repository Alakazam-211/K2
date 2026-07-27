// Procedural rasterization of synthetic glyphs for the WebGL atlas —
// the Canvas2D counterpart of syntheticGlyphs.ts's CSS paint stage.
//
// The DOM painter replaces box-drawing (U+2500–257F), block-element
// (U+2580–259F) and sextant (U+1FB00–1FB3B) font glyphs with painted
// geometry (stacked gradient layers); this module gives the WebGL
// atlas the same treatment: the glyph is FILLED procedurally into the
// atlas page instead of fillText'd, so stacked blocks tile seamlessly
// and TUI borders stay continuous at every zoom — identical defect
// coverage to the DOM path, same shared spec + device-px rect math
// (syntheticGlyphSpec / glyphMetrics / resolveGlyphRects).
//
// Everything rasterizes WHITE (shades at fractional alpha via
// globalAlpha): the atlas alpha channel is the coverage mask and the
// shader tints per instance, so one entry serves every color — and
// `v_color.a * s.a` composes shade alpha with dim exactly like the
// DOM's colorWithAlpha × opacity stack.
//
// Atlas coordinates are already device pixels, so metrics resolve
// with dpr = 1 against the device cell box — the same rects the DOM
// path would round to, without the CSS-px round trip.
//
// Pure math (dash segmentation, arc stroke geometry) is separated
// from the two ctx-touching functions so it unit-tests in node
// (Canvas2D is unavailable there; the atlas itself is stubbed in
// tests via GlyphSource).

import {
  glyphMetrics,
  resolveGlyphRects,
  syntheticGlyphSpec,
  type DeviceRect,
  type GlyphMetrics,
  type SyntheticGlyph,
} from '../syntheticGlyphs'

/** Synthetic spec for an atlas cluster, or null ⇒ font path. Only a
 *  BARE single-code-point cluster qualifies: expandRow folds
 *  zero-width followers (variation selectors, combining marks) into
 *  the cluster text, and a decorated cluster is no longer the plain
 *  geometry char — the font owns it, same as the DOM path (which
 *  checks codePointAt(0) on per-CHAR cells, where the split already
 *  happened). */
export function syntheticSpecForCluster(text: string): SyntheticGlyph | null {
  const cp = text.codePointAt(0) ?? 0
  const spec = syntheticGlyphSpec(cp)
  if (!spec) return null
  return String.fromCodePoint(cp) === text ? spec : null
}

/** Dashed-line stroke as explicit device-px segment rects. The DOM
 *  paints dashes as a repeating gradient; a canvas fill needs the
 *  segments themselves. Same pattern contract: `dash` segments per
 *  cell, ~60% ink each, phase-locked to the cell edge (segment i
 *  starts at round(i·span/dash)) so adjacent dashed cells continue
 *  the pattern seamlessly. */
export function dashSegments(
  spec: Extract<SyntheticGlyph, { kind: 'lines' }>,
  m: GlyphMetrics,
): DeviceRect[] {
  const horizontal = spec.arms[0] !== 0
  const heavy = (horizontal ? spec.arms[0] : spec.arms[1]) === 2
  const t = heavy ? m.ht : m.lt
  const band = horizontal ? (heavy ? m.hha : m.ha) : (heavy ? m.vha : m.va)
  const span = horizontal ? m.Wd : m.Hd
  const count = spec.dash as number
  const seg = span / count
  const on = Math.max(1, Math.round(seg * 0.6))
  const out: DeviceRect[] = []
  for (let i = 0; i < count; i++) {
    const a = Math.round(i * seg)
    const b = Math.min(a + on, span)
    if (b <= a) continue
    out.push(
      horizontal
        ? { x0: a, y0: band, x1: b, y1: band + t }
        : { x0: band, y0: a, x1: band + t, y1: b },
    )
  }
  return out
}

/** Stroke geometry for the rounded arcs ╭╮╰╯, in device px relative
 *  to the cell origin. The path is moveTo(start) → arcTo(corner, end,
 *  radius) → lineTo(end): two straight tails reaching the cell-edge
 *  midbands (so the arc joins neighboring ─/│ seamlessly, same as the
 *  DOM child div's borders) around one rounded corner. Coordinates
 *  are STROKE CENTERS (canvas stroking is center-aligned); the
 *  half-width overshoot at the edges is clipped by the atlas slot
 *  clip, mirroring the DOM's overflow:hidden. */
export interface ArcStroke {
  startX: number
  startY: number
  cornerX: number
  cornerY: number
  endX: number
  endY: number
  radius: number
  lineWidth: number
}

export function arcStroke(
  corner: 'tl' | 'tr' | 'br' | 'bl',
  m: GlyphMetrics,
): ArcStroke {
  // Stroke-center lines of the light vertical/horizontal bands.
  const cx = m.va + m.lt / 2
  const cy = m.ha + m.lt / 2
  // Tangent legs run from the corner to the two cell edges the arc's
  // arms exit through; arcTo needs radius ≤ both legs. Matching the
  // DOM's radius = min(box W, box H) — the largest turn the cell
  // affords.
  const legRight = m.Wd - cx
  const legLeft = cx
  const legDown = m.Hd - cy
  const legUp = cy
  switch (corner) {
    case 'tl': // ╭ bottom edge → curve → right edge
      return {
        startX: cx, startY: m.Hd,
        cornerX: cx, cornerY: cy,
        endX: m.Wd, endY: cy,
        radius: Math.min(legDown, legRight),
        lineWidth: m.lt,
      }
    case 'tr': // ╮ left edge → curve → bottom edge
      return {
        startX: 0, startY: cy,
        cornerX: cx, cornerY: cy,
        endX: cx, endY: m.Hd,
        radius: Math.min(legLeft, legDown),
        lineWidth: m.lt,
      }
    case 'br': // ╯ left edge → curve → top edge
      return {
        startX: 0, startY: cy,
        cornerX: cx, cornerY: cy,
        endX: cx, endY: 0,
        radius: Math.min(legLeft, legUp),
        lineWidth: m.lt,
      }
    case 'bl': // ╰ top edge → curve → right edge
      return {
        startX: cx, startY: 0,
        cornerX: cx, cornerY: cy,
        endX: m.Wd, endY: cy,
        radius: Math.min(legUp, legRight),
        lineWidth: m.lt,
      }
  }
}

/** Rasterize one synthetic glyph into a device cell box (w, h) at
 *  origin (x, y). Two consumers, one geometry:
 *  - WebGL atlas (defaults): white-on-transparent coverage mask,
 *    tinted per instance by the shader; caller clips to the slot.
 *  - DOM per-row canvas: pass the cell's resolved ink color and its
 *    dim/opacity as `alpha` — the strokes rasterize pre-colored
 *    (solid fillRects, NOT CSS gradients: WebKit has no solid-fill
 *    fast path for gradients, which was the grid-scroll cost —
 *    docs/learnings/LEARNINGS-dom-grids.md). Shade specs (`spec.alpha`) compose
 *    multiplicatively with `alpha`, mirroring the shader's
 *    coverage × instance-alpha and the DOM's rgba × opacity. */
export function drawSyntheticGlyph(
  ctx: CanvasRenderingContext2D,
  spec: SyntheticGlyph,
  x: number,
  y: number,
  w: number,
  h: number,
  ink: string = '#ffffff',
  alpha: number = 1,
): void {
  const m = glyphMetrics(w, h, 1)
  ctx.fillStyle = ink
  if (spec.kind === 'arc') {
    const a = arcStroke(spec.corner, m)
    ctx.strokeStyle = ink
    if (alpha !== 1) ctx.globalAlpha = alpha
    ctx.lineWidth = a.lineWidth
    ctx.beginPath()
    ctx.moveTo(x + a.startX, y + a.startY)
    ctx.arcTo(x + a.cornerX, y + a.cornerY, x + a.endX, y + a.endY, a.radius)
    ctx.lineTo(x + a.endX, y + a.endY)
    ctx.stroke()
    ctx.globalAlpha = 1
    return
  }
  const specAlpha =
    spec.kind === 'rects' && spec.alpha !== undefined ? spec.alpha : 1
  const combined = alpha * specAlpha
  if (combined !== 1) ctx.globalAlpha = combined
  const rects =
    spec.kind === 'lines' && spec.dash
      ? dashSegments(spec, m)
      : resolveGlyphRects(spec, m)
  for (const r of rects) {
    ctx.fillRect(x + r.x0, y + r.y0, r.x1 - r.x0, r.y1 - r.y0)
  }
  ctx.globalAlpha = 1
}
