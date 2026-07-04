// Synthetic glyph geometry contract (syntheticGlyphs.ts).
//
// The two user-verified defects this layer kills, pinned as
// properties: (1) stacked block elements must tile with ZERO gaps —
// a rect's end and its neighbor's start round to the same device
// pixel (Claude Code's logo showed horizontal seams from font ink
// underfilling the line box); (2) box-drawing strokes must span the
// FULL cell dimension at a thickness computed ONCE from cell
// metrics, so runs of ─ are a single continuous crisp line (borders
// had hairline breaks/misjoins at some zooms).

import { describe, expect, it } from 'vitest'
import {
  colorWithAlpha,
  glyphMetrics,
  paintSyntheticGlyph,
  resolveArcChild,
  resolveGlyphRects,
  syntheticGlyphSpec,
  type DeviceRect,
  type SyntheticGlyph,
} from './syntheticGlyphs'

// A deliberately awkward cell: fractional css width, dpr 2 — the
// combination that produced antialiased fuzz and seams with fonts.
const CELL_W = 8.6
const CELL_H = 19
const DPR = 2
const M = glyphMetrics(CELL_W, CELL_H, DPR)

function rects(ch: string): DeviceRect[] {
  const spec = syntheticGlyphSpec(ch.codePointAt(0) as number)
  expect(spec).not.toBeNull()
  return resolveGlyphRects(spec as SyntheticGlyph, M)
}

/** Is every device pixel of [lo, hi) on `axis` covered by some rect
 *  that also spans the given cross-axis band? */
function covers(
  rs: DeviceRect[],
  axis: 'x' | 'y',
  lo: number,
  hi: number,
): boolean {
  const spans = rs
    .map((r) => (axis === 'x' ? [r.x0, r.x1] : [r.y0, r.y1]))
    .sort((a, b) => a[0] - b[0])
  let reach = lo
  for (const [a, b] of spans) {
    if (a > reach) return false
    reach = Math.max(reach, b)
    if (reach >= hi) return true
  }
  return reach >= hi
}

describe('syntheticGlyphSpec coverage', () => {
  it('covers all of U+2500–257F except the three diagonals', () => {
    for (let cp = 0x2500; cp <= 0x257f; cp++) {
      const spec = syntheticGlyphSpec(cp)
      if (cp >= 0x2571 && cp <= 0x2573) {
        expect(spec, `U+${cp.toString(16)} (diagonal) stays font-rendered`).toBeNull()
      } else {
        expect(spec, `U+${cp.toString(16)} must be synthetic`).not.toBeNull()
      }
    }
  })

  it('covers all of U+2580–259F (blocks + shades)', () => {
    for (let cp = 0x2580; cp <= 0x259f; cp++) {
      expect(syntheticGlyphSpec(cp), `U+${cp.toString(16)}`).not.toBeNull()
    }
  })

  it('covers the legacy-computing sextants U+1FB00–1FB3B', () => {
    for (let cp = 0x1fb00; cp <= 0x1fb3b; cp++) {
      const spec = syntheticGlyphSpec(cp)
      expect(spec, `U+${cp.toString(16)}`).not.toBeNull()
      // Every sextant paints 1..5 sub-rects (empty/full/halves are
      // excluded from the block by Unicode).
      const n = (spec as Extract<SyntheticGlyph, { kind: 'rects' }>).rects.length
      expect(n).toBeGreaterThanOrEqual(1)
      expect(n).toBeLessThanOrEqual(5)
    }
  })

  it('returns null for everything else (ordinary text, braille, geometric shapes, powerline)', () => {
    for (const ch of ['A', 'z', '0', ' ', '⠋', '■', '◆', '', '日', '😀']) {
      expect(syntheticGlyphSpec(ch.codePointAt(0) as number), ch).toBeNull()
    }
  })
})

describe('block element geometry', () => {
  it('█ is the full cell', () => {
    expect(rects('█')).toEqual([{ x0: 0, y0: 0, x1: M.Wd, y1: M.Hd }])
  })

  it('▀ is the top half, split at the device-rounded midline', () => {
    expect(rects('▀')).toEqual([
      { x0: 0, y0: 0, x1: M.Wd, y1: Math.round(M.Hd / 2) },
    ])
  })

  it('▖ is the bottom-left quadrant', () => {
    expect(rects('▖')).toEqual([
      {
        x0: 0,
        y0: Math.round(M.Hd / 2),
        x1: Math.round(M.Wd / 2),
        y1: M.Hd,
      },
    ])
  })

  it('SEAM KILLER: ▀ bottom edge and ▄ top edge land on the SAME device pixel', () => {
    const upper = rects('▀')[0]
    const lower = rects('▄')[0]
    expect(upper.y1).toBe(lower.y0)
    // And the pair covers the full cell height with zero gap/overlap.
    expect(upper.y0).toBe(0)
    expect(lower.y1).toBe(M.Hd)
  })

  it('the 1/8-step series tiles: each step\'s edge equals the complementary step\'s edge', () => {
    // ▂ (lower 2/8) + a hypothetical upper 6/8 share the 6/8 line;
    // check every k that k/8 rounds identically from both tables.
    const lowerSeries = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'] // lower k/8, k=1..7
    lowerSeries.forEach((ch, idx) => {
      const k = idx + 1
      expect(rects(ch)).toEqual([
        { x0: 0, y0: Math.round(((8 - k) / 8) * M.Hd), x1: M.Wd, y1: M.Hd },
      ])
    })
    const leftSeries = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'] // left k/8
    leftSeries.forEach((ch, idx) => {
      const k = idx + 1
      expect(rects(ch)).toEqual([
        { x0: 0, y0: 0, x1: Math.round((k / 8) * M.Wd), y1: M.Hd },
      ])
    })
  })

  it('quadrant composites tile the full cell without gap (▙ = left half + lower-right)', () => {
    const rs = rects('▙')
    const midX = Math.round(M.Wd / 2)
    const midY = Math.round(M.Hd / 2)
    expect(rs).toEqual([
      { x0: 0, y0: 0, x1: midX, y1: M.Hd },
      { x0: midX, y0: midY, x1: M.Wd, y1: M.Hd },
    ])
  })

  it('shades ░▒▓ are full-cell fills at 25/50/75% alpha', () => {
    for (const [ch, alpha] of [
      ['░', 0.25],
      ['▒', 0.5],
      ['▓', 0.75],
    ] as const) {
      const spec = syntheticGlyphSpec(ch.codePointAt(0) as number)
      expect(spec).toEqual({ kind: 'rects', rects: [[0, 0, 1, 1]], alpha })
      expect(rects(ch)).toEqual([{ x0: 0, y0: 0, x1: M.Wd, y1: M.Hd }])
    }
  })

  it('sextant ▚-like pattern: U+1FB00 is the upper-left sixth only', () => {
    const rs = resolveGlyphRects(
      syntheticGlyphSpec(0x1fb00) as SyntheticGlyph,
      M,
    )
    expect(rs).toEqual([
      { x0: 0, y0: 0, x1: Math.round(M.Wd / 2), y1: Math.round(M.Hd / 3) },
    ])
  })

  it('vertically adjacent sextant rows share their third-line device pixel', () => {
    // U+1FB03 = upper-left + middle-left (pattern bits 0 and 2).
    // 1FB00+i, i<0x14 ⇒ pattern i+1; want pattern 5 ⇒ i=4.
    const rs = resolveGlyphRects(
      syntheticGlyphSpec(0x1fb04) as SyntheticGlyph,
      M,
    )
    expect(rs).toHaveLength(2)
    expect(rs[0].y1).toBe(rs[1].y0) // rows meet exactly at round(Hd/3)
  })
})

describe('box drawing geometry', () => {
  it('─ is a full-width horizontal stroke at the vertical center', () => {
    const rs = rects('─')
    // Full width, zero gap (two arms overlap through the center).
    expect(covers(rs, 'x', 0, M.Wd)).toBe(true)
    for (const r of rs) {
      expect(r.y0).toBe(M.ha)
      expect(r.y1).toBe(M.hb)
    }
    // Stroke straddles the cell midline at light thickness.
    expect(M.hb - M.ha).toBe(M.lt)
    expect(M.ha).toBeLessThanOrEqual(Math.round(M.Hd / 2))
    expect(M.hb).toBeGreaterThanOrEqual(Math.round(M.Hd / 2))
  })

  it('│ is a full-height vertical stroke; ┃/━ use the single heavy thickness', () => {
    expect(covers(rects('│'), 'y', 0, M.Hd)).toBe(true)
    for (const r of rects('┃')) {
      expect(r.x1 - r.x0).toBe(M.ht)
    }
    for (const r of rects('━')) {
      expect(r.y1 - r.y0).toBe(M.ht)
    }
  })

  it('ADJACENCY: neighboring ─ cells tile without a device-pixel gap', () => {
    // Each cell's stroke reaches both cell edges (x0 = 0, x1 = Wd);
    // cell k ends at k·Wd exactly where cell k+1 begins — the seam
    // condition reduces to full in-cell coverage at identical bands.
    const a = rects('─')
    const b = rects('─')
    expect(Math.min(...a.map((r) => r.x0))).toBe(0)
    expect(Math.max(...a.map((r) => r.x1))).toBe(M.Wd)
    // Same for a ┼ meeting a ─ (junction cells keep the same band).
    const cross = rects('┼')
    const crossH = cross.filter((r) => r.y0 === M.ha && r.y1 === M.hb)
    expect(covers(crossH, 'x', 0, M.Wd)).toBe(true)
    expect(b[0].y0).toBe(crossH[0].y0)
  })

  it('THICKNESS UNIFORMITY: every light-only glyph strokes at exactly the shared light thickness', () => {
    for (const ch of ['─', '│', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '╴', '╵', '╶', '╷']) {
      for (const r of rects(ch)) {
        const t = Math.min(r.x1 - r.x0, r.y1 - r.y0)
        expect(t, `${ch} stroke thickness`).toBe(M.lt)
      }
    }
  })

  it('┌ joins: both arms reach their cell edges and overlap at the center', () => {
    const rs = rects('┌')
    expect(rs).toHaveLength(2)
    const [right, down] = rs
    expect(right.x1).toBe(M.Wd) // reaches the right edge → joins neighbor ─
    expect(down.y1).toBe(M.Hd) // reaches the bottom edge → joins │ below
    expect(right.x0).toBeLessThanOrEqual(down.x0) // corner block covered
    expect(down.y0).toBeLessThanOrEqual(right.y0)
  })

  it('═ is two parallel full-width strokes with a light-thickness gap', () => {
    const rs = rects('═')
    expect(rs).toEqual([
      { x0: 0, y0: M.h1a, x1: M.Wd, y1: M.h1b },
      { x0: 0, y0: M.h2a, x1: M.Wd, y1: M.h2b },
    ])
    expect(M.h1b - M.h1a).toBe(M.lt)
    expect(M.h2b - M.h2a).toBe(M.lt)
    expect(M.h2a - M.h1b).toBe(M.lt) // the gap
  })

  it('║ is two parallel full-height strokes', () => {
    expect(rects('║')).toEqual([
      { x0: M.v1a, y0: 0, x1: M.v1b, y1: M.Hd },
      { x0: M.v2a, y0: 0, x1: M.v2b, y1: M.Hd },
    ])
  })

  it('╬ leaves the junction center open (no rect covers the cell center)', () => {
    const cx = Math.round(M.Wd / 2)
    const cy = Math.round(M.Hd / 2)
    for (const r of rects('╬')) {
      const coversCenter =
        r.x0 < cx && r.x1 > cx && r.y0 < cy && r.y1 > cy
      expect(coversCenter).toBe(false)
    }
    // ...but all four double arms reach their cell edges.
    const rs = rects('╬')
    expect(Math.min(...rs.map((r) => r.x0))).toBe(0)
    expect(Math.max(...rs.map((r) => r.x1))).toBe(M.Wd)
    expect(Math.min(...rs.map((r) => r.y0))).toBe(0)
    expect(Math.max(...rs.map((r) => r.y1))).toBe(M.Hd)
  })

  it('╭ resolves to an arc corner child: top-left radius, strokes reaching bottom and right edges', () => {
    const spec = syntheticGlyphSpec(0x256d) as SyntheticGlyph
    expect(spec).toEqual({ kind: 'arc', corner: 'tl' })
    expect(resolveGlyphRects(spec, M)).toEqual([]) // no background rects
    const arc = resolveArcChild('tl', M)
    expect(arc.left).toBe(M.va / DPR) // flush with the │ band below
    expect(arc.top).toBe(M.ha / DPR) // flush with the ─ band to the right
    expect(arc.borderWidth).toBe(M.lt / DPR)
    expect(arc.radius).toBe(Math.min(arc.width, arc.height))
    // One device px of far-edge slack (clipped by the cell span).
    expect(arc.width).toBe((M.Wd - M.va + 1) / DPR)
    expect(arc.height).toBe((M.Hd - M.ha + 1) / DPR)
  })

  it('all four arcs map to their corners', () => {
    expect(syntheticGlyphSpec(0x256e)).toEqual({ kind: 'arc', corner: 'tr' })
    expect(syntheticGlyphSpec(0x256f)).toEqual({ kind: 'arc', corner: 'br' })
    expect(syntheticGlyphSpec(0x2570)).toEqual({ kind: 'arc', corner: 'bl' })
  })

  it('every painted line glyph stays inside the cell box', () => {
    for (let cp = 0x2500; cp <= 0x257f; cp++) {
      const spec = syntheticGlyphSpec(cp)
      if (!spec) continue
      for (const r of resolveGlyphRects(spec, M)) {
        expect(r.x0, `U+${cp.toString(16)}`).toBeGreaterThanOrEqual(0)
        expect(r.y0, `U+${cp.toString(16)}`).toBeGreaterThanOrEqual(0)
        expect(r.x1, `U+${cp.toString(16)}`).toBeLessThanOrEqual(M.Wd)
        expect(r.y1, `U+${cp.toString(16)}`).toBeLessThanOrEqual(M.Hd)
        expect(r.x1).toBeGreaterThan(r.x0)
        expect(r.y1).toBeGreaterThan(r.y0)
      }
    }
  })
})

describe('glyphMetrics device rounding', () => {
  it('computes stroke thickness once from cell metrics: ≥1 device px, heavy > light', () => {
    const m = glyphMetrics(8.6, 19, 2)
    expect(m.lt).toBe(Math.max(1, Math.round(m.Hd / 12)))
    expect(m.ht).toBeGreaterThan(m.lt)
    // Tiny cell: thickness clamps to 1 device px, never 0.
    const tiny = glyphMetrics(3, 6, 1)
    expect(tiny.lt).toBe(1)
    expect(tiny.ht).toBe(2)
  })

  it('rounds the cell to whole device pixels (dpr multiply → round)', () => {
    const m = glyphMetrics(8.6, 19, 2)
    expect(m.Wd).toBe(17) // round(8.6 × 2)
    expect(m.Hd).toBe(38)
    expect(Number.isInteger(m.va)).toBe(true)
    expect(Number.isInteger(m.h2b)).toBe(true)
  })
})

describe('paintSyntheticGlyph CSS assembly', () => {
  const paint = (ch: string, color = 'rgb(224,224,224)') =>
    paintSyntheticGlyph(
      syntheticGlyphSpec(ch.codePointAt(0) as number) as SyntheticGlyph,
      CELL_W,
      CELL_H,
      DPR,
      color,
    )

  it('█ paints one full-coverage solid gradient layer', () => {
    const p = paint('█')
    if (!('background' in p)) throw new Error('expected background paint')
    expect(p.background.backgroundImage).toBe(
      'linear-gradient(rgb(224,224,224),rgb(224,224,224))',
    )
    expect(p.background.backgroundPosition).toBe('0px 0px')
    expect(p.background.backgroundSize).toBe('100% 100%')
    expect(p.background.backgroundRepeat).toBe('no-repeat')
  })

  it('▗ (far-edge quadrant) gets device-px position and clip-safe slack size', () => {
    const p = paint('▗')
    if (!('background' in p)) throw new Error('expected background paint')
    const midX = Math.round(M.Wd / 2) / DPR
    const midY = Math.round(M.Hd / 2) / DPR
    expect(p.background.backgroundPosition).toBe(`${midX}px ${midY}px`)
    // Far edges: (Wd - x0 + 1)/dpr — one device px of slack, clipped
    // at the box edge, so the rect stays flush however fractional
    // the cell origin is.
    expect(p.background.backgroundSize).toBe(
      `${(M.Wd - midX * DPR + 1) / DPR}px ${(M.Hd - midY * DPR + 1) / DPR}px`,
    )
  })

  it('░ paints with precomputed 25%-alpha rgba ink', () => {
    const p = paint('░')
    if (!('background' in p)) throw new Error('expected background paint')
    expect(p.background.backgroundImage).toBe(
      'linear-gradient(rgba(224,224,224,0.25),rgba(224,224,224,0.25))',
    )
  })

  it('═ stacks two layers (comma-joined longhands)', () => {
    const p = paint('═')
    if (!('background' in p)) throw new Error('expected background paint')
    expect(p.background.backgroundImage.match(/linear-gradient/g)).toHaveLength(2)
    expect(p.background.backgroundPosition.split(',')).toHaveLength(2)
    expect(p.background.backgroundSize.split(',')).toHaveLength(2)
  })

  it('┄ paints a repeating hard-stop gradient across the full stroke band', () => {
    const p = paint('┄')
    if (!('background' in p)) throw new Error('expected background paint')
    expect(p.background.backgroundImage).toContain('repeating-linear-gradient(to right')
    expect(p.background.backgroundSize).toBe(`100% ${M.lt / DPR}px`)
    expect(p.background.backgroundPosition).toBe(`0px ${M.ha / DPR}px`)
  })

  it('┇ (heavy vertical dash) uses the heavy band, to-bottom', () => {
    const p = paint('┇')
    if (!('background' in p)) throw new Error('expected background paint')
    expect(p.background.backgroundImage).toContain('repeating-linear-gradient(to bottom')
    expect(p.background.backgroundSize).toBe(`${M.ht / DPR}px 100%`)
  })

  it('╭ paints as an arc child carrying the resolved ink', () => {
    const p = paint('╭', 'rgb(10,20,30)')
    if (!('arc' in p)) throw new Error('expected arc paint')
    expect(p.color).toBe('rgb(10,20,30)')
    expect(p.arc.corner).toBe('tl')
  })
})

describe('colorWithAlpha', () => {
  it('precomputes rgba from rgb() and hex forms', () => {
    expect(colorWithAlpha('rgb(224,224,224)', 0.25)).toBe('rgba(224,224,224,0.25)')
    expect(colorWithAlpha('rgb(0, 128, 255)', 0.5)).toBe('rgba(0,128,255,0.5)')
    expect(colorWithAlpha('#10a0ff', 0.75)).toBe('rgba(16,160,255,0.75)')
    expect(colorWithAlpha('#fff', 0.5)).toBe('rgba(255,255,255,0.5)')
  })

  it('falls back to color-mix for anything it cannot parse', () => {
    expect(colorWithAlpha('rebeccapurple', 0.25)).toBe(
      'color-mix(in srgb, rebeccapurple 25%, transparent)',
    )
  })
})
