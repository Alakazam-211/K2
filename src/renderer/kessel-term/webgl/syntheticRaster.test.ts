// WebGL synthetic-raster geometry contract (syntheticRaster.ts).
//
// Only the pure parts run here — drawSyntheticGlyph needs Canvas2D,
// which the node test env lacks (the atlas is stubbed via
// GlyphSource in packFrame.test.ts). What we pin instead: (1) which
// clusters take the synthetic path (bare single code point only —
// decorated clusters belong to the font, same as the DOM split);
// (2) dash segments reproduce the DOM's repeating-gradient contract
// as explicit rects (phase-locked to the cell edge, ~60% ink, right
// band, clamped in-cell); (3) arc strokes land their tangent
// endpoints on the light-band stroke centers at the cell edges so
// ╭╮╰╯ join neighboring ─/│ without steps.

import { describe, expect, it } from 'vitest'
import {
  glyphMetrics,
  type SyntheticGlyph,
} from '../syntheticGlyphs'
import {
  arcStroke,
  dashSegments,
  drawSyntheticGlyph,
  syntheticSpecForCluster,
} from './syntheticRaster'

// Atlas metrics resolve at dpr = 1 (atlas coordinates ARE device
// px). 9×20 keeps the numbers readable while exercising rounding.
const M = glyphMetrics(9, 20, 1)

function linesSpec(text: string): Extract<SyntheticGlyph, { kind: 'lines' }> {
  const spec = syntheticSpecForCluster(text)
  if (!spec || spec.kind !== 'lines') throw new Error(`not a lines spec: ${text}`)
  return spec
}

describe('syntheticSpecForCluster', () => {
  it('resolves bare box-drawing code points', () => {
    expect(syntheticSpecForCluster('─')).toMatchObject({ kind: 'lines' })
    expect(syntheticSpecForCluster('█')).toMatchObject({ kind: 'rects' })
    expect(syntheticSpecForCluster('╔')).toMatchObject({ kind: 'double' })
    expect(syntheticSpecForCluster('╭')).toMatchObject({ kind: 'arc', corner: 'tl' })
  })

  it('resolves astral sextants (codePointAt, not charCodeAt)', () => {
    const spec = syntheticSpecForCluster('\u{1FB00}')
    expect(spec).toMatchObject({ kind: 'rects' })
  })

  it('returns null for ordinary text', () => {
    expect(syntheticSpecForCluster('A')).toBeNull()
    expect(syntheticSpecForCluster('┌'.repeat(2))).toBeNull()
  })

  it('returns null for excluded diagonals (font path, like the DOM)', () => {
    expect(syntheticSpecForCluster('╱')).toBeNull()
    expect(syntheticSpecForCluster('╲')).toBeNull()
    expect(syntheticSpecForCluster('╳')).toBeNull()
  })

  it('returns null when the cluster carries zero-width followers', () => {
    expect(syntheticSpecForCluster('─︎')).toBeNull() // variation selector
    expect(syntheticSpecForCluster('─́')).toBeNull() // combining mark
  })
})

describe('dashSegments', () => {
  it('light horizontal triple dash ┄: 3 rects in the light h-band', () => {
    const segs = dashSegments(linesSpec('┄'), M)
    expect(segs).toHaveLength(3)
    const seg = M.Wd / 3
    const on = Math.max(1, Math.round(seg * 0.6))
    segs.forEach((r, i) => {
      expect(r.y0).toBe(M.ha)
      expect(r.y1).toBe(M.ha + M.lt)
      expect(r.x0).toBe(Math.round(i * seg))
      expect(r.x1 - r.x0).toBe(on)
      expect(r.x0).toBeGreaterThanOrEqual(0)
      expect(r.x1).toBeLessThanOrEqual(M.Wd)
    })
  })

  it('heavy vertical quadruple dash ┋: 4 rects in the heavy v-band, clamped to Hd', () => {
    const segs = dashSegments(linesSpec('┋'), M)
    expect(segs).toHaveLength(4)
    const seg = M.Hd / 4
    const on = Math.max(1, Math.round(seg * 0.6))
    segs.forEach((r, i) => {
      expect(r.x0).toBe(M.vha)
      expect(r.x1).toBe(M.vha + M.ht)
      const a = Math.round(i * seg)
      expect(r.y0).toBe(a)
      expect(r.y1).toBe(Math.min(a + on, M.Hd))
    })
  })

  it('double dash ╌ / ╎: 2 segments, phase-locked to the cell edge', () => {
    const h = dashSegments(linesSpec('╌'), M)
    expect(h).toHaveLength(2)
    expect(h[0].x0).toBe(0) // segment 0 starts at the cell edge —
    // adjacent dashed cells continue the pattern seamlessly
    expect(h[1].x0).toBe(Math.round(M.Wd / 2))

    const v = dashSegments(linesSpec('╎'), M)
    expect(v).toHaveLength(2)
    expect(v[0].y0).toBe(0)
    expect(v[0].x0).toBe(M.va)
    expect(v[0].x1).toBe(M.va + M.lt)
  })

  it('every segment has positive ink even at tiny cells', () => {
    const tiny = glyphMetrics(3, 5, 1)
    for (const ch of ['┄', '┈', '╏']) {
      for (const r of dashSegments(linesSpec(ch), tiny)) {
        expect(r.x1).toBeGreaterThan(r.x0)
        expect(r.y1).toBeGreaterThan(r.y0)
      }
    }
  })
})

describe('arcStroke', () => {
  // Stroke centers of the light bands — where the arc must meet
  // neighboring ─ / │ strokes.
  const cx = M.va + M.lt / 2
  const cy = M.ha + M.lt / 2

  it('tl ╭: bottom edge → corner → right edge', () => {
    const a = arcStroke('tl', M)
    expect([a.startX, a.startY]).toEqual([cx, M.Hd])
    expect([a.cornerX, a.cornerY]).toEqual([cx, cy])
    expect([a.endX, a.endY]).toEqual([M.Wd, cy])
    expect(a.radius).toBe(Math.min(M.Hd - cy, M.Wd - cx))
  })

  it('tr ╮: left edge → corner → bottom edge', () => {
    const a = arcStroke('tr', M)
    expect([a.startX, a.startY]).toEqual([0, cy])
    expect([a.endX, a.endY]).toEqual([cx, M.Hd])
    expect(a.radius).toBe(Math.min(cx, M.Hd - cy))
  })

  it('br ╯: left edge → corner → top edge', () => {
    const a = arcStroke('br', M)
    expect([a.startX, a.startY]).toEqual([0, cy])
    expect([a.endX, a.endY]).toEqual([cx, 0])
    expect(a.radius).toBe(Math.min(cx, cy))
  })

  it('bl ╰: top edge → corner → right edge', () => {
    const a = arcStroke('bl', M)
    expect([a.startX, a.startY]).toEqual([cx, 0])
    expect([a.endX, a.endY]).toEqual([M.Wd, cy])
    expect(a.radius).toBe(Math.min(cy, M.Wd - cx))
  })

  it('all corners share the band centers, light stroke width, radius > 0', () => {
    for (const c of ['tl', 'tr', 'br', 'bl'] as const) {
      const a = arcStroke(c, M)
      expect([a.cornerX, a.cornerY]).toEqual([cx, cy])
      expect(a.lineWidth).toBe(M.lt)
      expect(a.radius).toBeGreaterThan(0)
    }
  })
})

describe('drawSyntheticGlyph — ink/alpha contract (recording stub ctx)', () => {
  // drawSyntheticGlyph only touches a handful of ctx members, so a
  // recording stub exercises the real draw path in node — what the
  // jsdom row tests can't (no Canvas2D there).
  interface Fill {
    x: number
    y: number
    w: number
    h: number
    alpha: number
    style: string | CanvasGradient | CanvasPattern
  }
  function stubCtx() {
    const fills: Fill[] = []
    const strokes: { alpha: number; style: unknown; lineWidth: number }[] = []
    const ctx = {
      fillStyle: '' as string,
      strokeStyle: '' as string,
      lineWidth: 0,
      globalAlpha: 1,
      fillRect(x: number, y: number, w: number, h: number) {
        fills.push({ x, y, w, h, alpha: this.globalAlpha, style: this.fillStyle })
      },
      beginPath() {},
      moveTo() {},
      arcTo() {},
      lineTo() {},
      stroke() {
        strokes.push({
          alpha: this.globalAlpha,
          style: this.strokeStyle,
          lineWidth: this.lineWidth,
        })
      },
    }
    return { ctx: ctx as unknown as CanvasRenderingContext2D, fills, strokes }
  }

  function spec(ch: string) {
    const s = syntheticSpecForCluster(ch)
    if (!s) throw new Error(`no spec: ${ch}`)
    return s
  }

  it('█ fills the whole cell box with the ink color at the origin offset', () => {
    const { ctx, fills } = stubCtx()
    drawSyntheticGlyph(ctx, spec('█'), 5, 7, 9, 20, 'rgb(255,0,0)')
    expect(fills).toHaveLength(1)
    expect(fills[0]).toMatchObject({ x: 5, y: 7, w: 9, h: 20, alpha: 1 })
    expect(fills[0].style).toBe('rgb(255,0,0)')
  })

  it('░ shade composes spec alpha with the cell alpha (dim) and resets', () => {
    const { ctx, fills } = stubCtx()
    drawSyntheticGlyph(ctx, spec('░'), 0, 0, 9, 20, '#fff', 0.6)
    expect(fills).toHaveLength(1)
    expect(fills[0].alpha).toBeCloseTo(0.25 * 0.6)
    expect(ctx.globalAlpha).toBe(1) // reset after draw
  })

  it('╭ strokes with the ink color, light thickness, and cell alpha', () => {
    const { ctx, strokes } = stubCtx()
    drawSyntheticGlyph(ctx, spec('╭'), 0, 0, 9, 20, 'rgb(0,255,0)', 0.6)
    expect(strokes).toHaveLength(1)
    expect(strokes[0].style).toBe('rgb(0,255,0)')
    expect(strokes[0].lineWidth).toBe(glyphMetrics(9, 20, 1).lt)
    expect(strokes[0].alpha).toBe(0.6)
    expect(ctx.globalAlpha).toBe(1)
  })

  it('defaults stay white/opaque — the WebGL atlas contract', () => {
    const { ctx, fills } = stubCtx()
    drawSyntheticGlyph(ctx, spec('─'), 0, 0, 9, 20)
    expect(fills.length).toBeGreaterThan(0)
    for (const f of fills) {
      expect(f.style).toBe('#ffffff')
      expect(f.alpha).toBe(1)
    }
  })
})
