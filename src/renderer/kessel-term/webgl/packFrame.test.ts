import { describe, it, expect } from 'vitest'
import {
  FrameBuffers,
  GLYPH_FLOATS,
  packFrame,
  prewarmRows,
  RowCache,
  RectList,
  type PackInput,
} from './packFrame'
import type { PainterFrame } from './painterTypes'
import type { WireCellRun } from '../gridWire'
import type { GlyphSource } from './glyphAtlas'

const THEME = { fg: 0xe0e0e0, bg: 0x0a0a0a, selection: 0x444444, textGamma: 1.2 }

function run(text: string, over: Partial<WireCellRun> = {}): WireCellRun {
  return {
    text,
    fg: null,
    bg: null,
    bold: false,
    italic: false,
    underline: false,
    inverse: false,
    dim: false,
    strikeout: false,
    ...over,
  }
}

function frame(
  grid: WireCellRun[][],
  scrollback: WireCellRun[][] = [],
  scrollPx = 0,
  cols = 4,
): PainterFrame {
  return {
    snapshot: { cols, rows: grid.length, grid, scrollback },
    scrollPx,
    selection: null,
    theme: THEME,
  }
}

// Deterministic stub atlas: texX = first code point, texY encodes
// style, quad = widthCells×cell. Honest stand-in for GlyphAtlas —
// the painter never introspects slots beyond copying them.
function stubGlyphs(epoch = 0): GlyphSource {
  return {
    epoch,
    get(text, bold, italic, widthCells) {
      const cp = text.codePointAt(0) ?? 0
      return {
        texX: cp,
        texY: (bold ? 1000 : 0) + (italic ? 2000 : 0),
        w: widthCells * 10,
        h: 20,
        color: cp >= 0x1f300,
      }
    },
  }
}

// Device grid: 10×20 px cells at dpr 2 (css cell 5×10).
function pack(
  f: PainterFrame,
  cache = new RowCache(),
  buffers = new FrameBuffers(),
  glyphs: GlyphSource = stubGlyphs(),
) {
  return packFrame({
    frame: f,
    cssCellH: 10,
    deviceCellW: 10,
    deviceCellH: 20,
    dpr: 2,
    cache,
    buffers,
    glyphs,
    decoThickness: 2,
  })
}

function rects(list: RectList): number[][] {
  const out: number[][] = []
  for (let i = 0; i < list.count; i++) {
    out.push(Array.from(list.data.subarray(i * 8, i * 8 + 8)))
  }
  return out
}

describe('packFrame — windowing', () => {
  it('pins to the last rows at scrollPx=0 with zero fraction', () => {
    const sb = [[run('old', { bg: 0x111111 })]]
    const g = [[run('a')], [run('b')]]
    const p = pack(frame(g, sb))
    expect(p.windowStart).toBe(1) // total 3 rows, viewport 2 → rows 1..2
    expect(p.rowCount).toBe(2)
    expect(p.fractionDevice).toBe(0)
  })

  it('exposes a partial extra row while scrolled to a fraction', () => {
    const sb = [[run('s0')], [run('s1')]]
    const g = [[run('a')], [run('b')]]
    // 4 css px up = 0.4 of a cell → window slides, fraction 6 css px
    // (topPx = (4-2)*10 - 4 = 16 → firstVisible=1, fraction 6).
    const p = pack(frame(g, sb, 4))
    expect(p.windowStart).toBe(1)
    expect(p.rowCount).toBe(3) // viewport 2 + partial third
    expect(p.fractionDevice).toBe(12) // 6 css px × dpr 2
  })

  it('whole-row scroll positions are exact (fraction 0, no drift)', () => {
    const sb = [[run('s0')], [run('s1')]]
    const g = [[run('a')], [run('b')]]
    // Exactly one cell height up.
    const p = pack(frame(g, sb, 10))
    expect(p.windowStart).toBe(1)
    expect(p.rowCount).toBe(2)
    expect(p.fractionDevice).toBe(0)
  })

  it('scrolled to the top of history the window starts at row 0', () => {
    const sb = [[run('s0')], [run('s1')]]
    const g = [[run('a')], [run('b')]]
    // Max scroll = scrollback.length * cellH = 20.
    const p = pack(frame(g, sb, 20))
    expect(p.windowStart).toBe(0)
    expect(p.rowCount).toBe(2)
    expect(p.fractionDevice).toBe(0)
  })

  it('windows the whole grid when there is no scrollback', () => {
    const g = [[run('a', { bg: 0x123456 })], [], []]
    const p = pack(frame(g))
    expect(p.windowStart).toBe(0)
    expect(p.rowCount).toBe(3)
  })
})

describe('packFrame — background rects', () => {
  it('emits device-px rects with unpacked colors, fraction baked into y', () => {
    const g = [
      [run('ab', { bg: 0xff0000 })],
      [run('x'), run('yz', { bg: 0x00ff00 })],
    ]
    const p = pack(frame(g))
    expect(rects(p.bg)).toEqual([
      // row 0: cols 0-1 red
      [0, 0, 20, 20, 1, 0, 0, 1],
      // row 1: cols 1-2 green (x is default-bg → nothing)
      [10, 20, 20, 20, 0, 1, 0, 1],
    ])
  })

  it('shifts rect y up by the device fraction while scrolled', () => {
    const sb = [[run('s', { bg: 0x0000ff })], [run('t')]]
    const g = [[run('a')], [run('b')]]
    const p = pack(frame(g, sb, 4))
    // windowStart=1, fractionDevice=12: row 't' contributes nothing,
    // rows a/b nothing, but had 's' been in-window its y would shift.
    // Use a case with a visible bg row instead:
    const p2 = pack(frame([[run('a', { bg: 0x0000ff })], [run('b')]], [[run('s')], [run('t')]], 4))
    expect(p.bg.count).toBe(0)
    expect(rects(p2.bg)).toEqual([
      // 'a' is abs row 2 → strip index 1 → y = 1*20 - 12 = 8
      [0, 8, 10, 20, 0, 0, 1, 1],
    ])
  })

  it('reuses buffers across frames (no growth churn)', () => {
    const buffers = new FrameBuffers()
    const cache = new RowCache()
    const f = frame([[run('a', { bg: 0x111111 })]])
    const p1 = pack(f, cache, buffers)
    const data1 = p1.bg.data
    const p2 = pack(f, cache, buffers)
    expect(p2.bg.data).toBe(data1)
    expect(p2.bg.count).toBe(1)
  })
})

describe('packFrame — decoration rects', () => {
  it('underline hugs the cell bottom, strikeout centers, dim alpha carried', () => {
    const g = [
      [run('ab', { underline: true })],
      [run('c', { strikeout: true, dim: true, fg: 0xff0000 })],
    ]
    const p = pack(frame(g))
    expect(rects(p.deco)).toEqual([
      // underline: y = 0 + (20 - 2*2) = 16, thickness 2
      [0, 16, 20, 2, Math.fround(0xe0 / 255), Math.fround(0xe0 / 255), Math.fround(0xe0 / 255), 1],
      // strikeout: y = 20 + round((20-2)/2) = 29
      [0, 29, 10, 2, 1, 0, 0, Math.fround(0.6)],
    ])
  })

  it('deco y shifts with the scroll fraction like every other pass', () => {
    const sb = [[run('s')], [run('t')]]
    const g = [[run('a', { underline: true })], [run('b')]]
    const p = pack(frame(g, sb, 4)) // fractionDevice 12
    // 'a' is strip index 1 → y = 20 - 12 + 16 = 24.
    expect(rects(p.deco)).toEqual([
      [0, 24, 10, 2, Math.fround(0xe0 / 255), Math.fround(0xe0 / 255), Math.fround(0xe0 / 255), 1],
    ])
  })
})

describe('packFrame — selection rects', () => {
  const SEL_R = Math.fround(0x44 / 255)
  const SEL_A = Math.fround(0.45)

  it('head/body/tail rows: partial, full-width, partial', () => {
    const g = [[run('aaaa')], [run('bbbb')], [run('cccc')]]
    const f = frame(g, [], 0, 4)
    f.selection = { startAbs: 0, startCol: 2, endAbs: 2, endCol: 3 }
    const p = pack(f)
    expect(rects(p.selection)).toEqual([
      // head: cols 2..4
      [20, 0, 20, 20, SEL_R, SEL_R, SEL_R, SEL_A],
      // body: full width
      [0, 20, 40, 20, SEL_R, SEL_R, SEL_R, SEL_A],
      // tail: cols 0..3
      [0, 40, 30, 20, SEL_R, SEL_R, SEL_R, SEL_A],
    ])
  })

  it('empty rows inside the range still highlight', () => {
    const g = [[run('a')], [], [run('c')]]
    const f = frame(g, [], 0, 4)
    f.selection = { startAbs: 0, startCol: 0, endAbs: 2, endCol: 1 }
    const p = pack(f)
    expect(p.selection.count).toBe(3)
  })

  it('clips to the visible window while scrolled', () => {
    const sb = [[run('s0')], [run('s1')]]
    const g = [[run('a')], [run('b')]]
    const f = frame(g, sb, 20) // scrolled to top: rows 0..1 visible
    f.selection = { startAbs: 2, startCol: 0, endAbs: 3, endCol: 2 }
    const p = pack(f)
    expect(p.selection.count).toBe(0)
  })

  it('no selection ⇒ no rects', () => {
    const p = pack(frame([[run('a')]]))
    expect(p.selection.count).toBe(0)
  })
})

describe('packFrame — glyph instances', () => {
  it('packs exact per-cell floats for a tiny grid', () => {
    // 2 cols × 1 row: 'A' (cp 65) then a blank cell.
    const f = frame([[run('A', { fg: 0xff8000, dim: true })]], [], 0, 2)
    const p = pack(f)
    expect(p.glyphCount).toBe(2)
    const cell0 = Array.from(p.glyphData.subarray(0, GLYPH_FLOATS))
    const cell1 = Array.from(
      p.glyphData.subarray(GLYPH_FLOATS, 2 * GLYPH_FLOATS),
    )
    expect(cell0).toEqual([
      0, 0, // offset within cell
      10, 20, // quad size (1 cell)
      65, 0, // atlas px origin (stub: cp, style)
      1, Math.fround(128 / 255), 0, // fg rgb
      Math.fround(0.6), // dim alpha
      0, // monochrome
      0, // reserved
    ])
    expect(cell1).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
  })

  it('wide glyphs occupy one instance sized two cells; spacer stays blank', () => {
    const f = frame([[run('日', { cols: 2 })]], [], 0, 3)
    const p = pack(f)
    const cell0 = Array.from(p.glyphData.subarray(0, GLYPH_FLOATS))
    expect(cell0[2]).toBe(20) // 2 cells wide
    expect(cell0[4]).toBe(0x65e5) // '日'
    // Spacer cell (col 1) and col 2 are zeroed.
    expect(
      Array.from(p.glyphData.subarray(GLYPH_FLOATS, 3 * GLYPH_FLOATS)),
    ).toEqual(new Array(2 * GLYPH_FLOATS).fill(0))
  })

  it('flags emoji as color glyphs', () => {
    const f = frame([[run('😀', { cols: 2 })]], [], 0, 2)
    const p = pack(f)
    expect(p.glyphData[10]).toBe(1)
  })

  it('zero-fills stale buffer contents for blank rows', () => {
    const buffers = new FrameBuffers()
    const cache = new RowCache()
    // Frame A: row full of glyphs. Frame C reuses A's buffer (double
    // buffering alternates A→B→A) with an EMPTY row — stale floats
    // must not leak through.
    const fA = frame([[run('ZZ')]], [], 0, 2)
    const pA = pack(fA, cache, buffers)
    expect(pA.glyphData[4]).toBe(90)
    pack(frame([[run('Y')]], [], 0, 2), cache, buffers)
    const pC = pack(frame([[]], [], 0, 2), cache, buffers)
    expect(pC.glyphData).toBe(pA.glyphData)
    expect(
      Array.from(pC.glyphData.subarray(0, 2 * GLYPH_FLOATS)),
    ).toEqual(new Array(2 * GLYPH_FLOATS).fill(0))
  })

  it('alternates glyph upload buffers across frames (double buffering)', () => {
    const buffers = new FrameBuffers()
    const cache = new RowCache()
    const f = frame([[run('a')]], [], 0, 2)
    const p1 = pack(f, cache, buffers)
    const p2 = pack(f, cache, buffers)
    expect(p2.glyphData).not.toBe(p1.glyphData)
  })
})

describe('RowCache — slab reuse', () => {
  it('returns the same slab reference for an unchanged row', () => {
    const cache = new RowCache()
    const glyphs = stubGlyphs()
    const row = [run('ab')]
    const s1 = cache.slabFor(row, THEME, 4, glyphs)
    const s2 = cache.slabFor(row, THEME, 4, glyphs)
    expect(s2).toBe(s1)
  })

  it('re-packs when the grid width changes', () => {
    const cache = new RowCache()
    const glyphs = stubGlyphs()
    const row = [run('ab')]
    const s1 = cache.slabFor(row, THEME, 4, glyphs)
    const s2 = cache.slabFor(row, THEME, 6, glyphs)
    expect(s2).not.toBe(s1)
    expect(s2.length).toBe(6 * GLYPH_FLOATS)
  })

  it('re-packs when the atlas epoch changes (page cleared)', () => {
    const cache = new RowCache()
    const row = [run('ab')]
    const s1 = cache.slabFor(row, THEME, 4, stubGlyphs(0))
    const s2 = cache.slabFor(row, THEME, 4, stubGlyphs(1))
    expect(s2).not.toBe(s1)
  })
})

describe('RowCache — identity damage test', () => {
  it('returns the same expansion for the same row reference', () => {
    const cache = new RowCache()
    const row = [run('abc')]
    const a = cache.get(row, THEME)
    const b = cache.get(row, THEME)
    expect(b).toBe(a)
  })

  it('re-expands when the row reference changes (damaged row)', () => {
    const cache = new RowCache()
    const a = cache.get([run('abc')], THEME)
    const b = cache.get([run('abc')], THEME)
    expect(b).not.toBe(a)
  })

  it('clears wholesale on theme change (defaults are baked in)', () => {
    const cache = new RowCache()
    const row = [run('x', { inverse: true })]
    const a = cache.get(row, THEME)
    expect(a.bgSpans[0].color).toBe(THEME.fg)
    const b = cache.get(row, { ...THEME, fg: 0x123456 })
    expect(b).not.toBe(a)
    expect(b.bgSpans[0].color).toBe(0x123456)
  })

  it('evicts least-recently-used rows past capacity', () => {
    const cache = new RowCache(2)
    const r1 = [run('1')]
    const r2 = [run('2')]
    const r3 = [run('3')]
    const e1 = cache.get(r1, THEME)
    cache.get(r2, THEME)
    cache.get(r1, THEME) // refresh r1 → r2 is now oldest
    cache.get(r3, THEME) // evicts r2
    expect(cache.size).toBe(2)
    expect(cache.get(r1, THEME)).toBe(e1)
  })
})

describe('prewarmRows — scroll-hop prewarm', () => {
  // 6 scrollback + 2 grid rows = 8 total, viewport 2, cols 4.
  function bigFrame(scrollPx: number): PainterFrame {
    const sb = Array.from({ length: 6 }, (_, i) => [run(`s${i}`)])
    const g = [[run('a')], [run('b')]]
    return frame(g, sb, scrollPx)
  }

  function inputFor(f: PainterFrame, cache: RowCache): PackInput {
    return {
      frame: f,
      cssCellH: 10,
      deviceCellW: 10,
      deviceCellH: 20,
      dpr: 2,
      cache,
      buffers: new FrameBuffers(),
      glyphs: stubGlyphs(),
      decoThickness: 2,
    }
  }

  it('warms rows on both sides of the window, nearest-first', () => {
    const cache = new RowCache()
    const f = bigFrame(30) // window rows 3..4 of 0..7
    const input = inputFor(f, cache)
    const packed = packFrame(input)
    expect(packed.windowStart).toBe(3)
    expect(cache.size).toBe(2) // the visible rows
    const warmed = prewarmRows(input, packed, 2, Infinity)
    expect(warmed).toBe(4) // rows 2,5 (d=1) + 1,6 (d=2)
    expect(cache.size).toBe(6)
  })

  it('skips out-of-range rows at the buffer edges', () => {
    const cache = new RowCache()
    const f = bigFrame(0) // pinned to bottom: window rows 6..7
    const input = inputFor(f, cache)
    const packed = packFrame(input)
    expect(packed.windowStart).toBe(6)
    const warmed = prewarmRows(input, packed, 2, Infinity)
    expect(warmed).toBe(2) // rows 5, 4 — nothing exists below 7
    expect(cache.size).toBe(4)
  })

  it('stops at the time budget between distance steps', () => {
    const cache = new RowCache()
    const f = bigFrame(30)
    const input = inputFor(f, cache)
    const packed = packFrame(input)
    let t = 0
    const now = () => (t += 1.5)
    const warmed = prewarmRows(input, packed, 24, 1, now)
    expect(warmed).toBe(2) // one distance step, then budget hit
    expect(cache.size).toBe(4)
  })

  it('warmed rows are cache hits for the next packed frame', () => {
    const cache = new RowCache()
    const f = bigFrame(30)
    const input = inputFor(f, cache)
    const packed = packFrame(input)
    prewarmRows(input, packed, 2, Infinity)
    const sizeAfterWarm = cache.size
    // Scroll up one row: the newly revealed row 2 was prewarmed.
    const scrolled = inputFor(bigFrame(40), cache)
    scrolled.frame.snapshot.scrollback = f.snapshot.scrollback
    scrolled.frame.snapshot.grid = f.snapshot.grid
    packFrame(scrolled)
    expect(cache.size).toBe(sizeAfterWarm) // no new expansions needed
  })
})

describe('emoji slot overhang (a_geom.x plumbing)', () => {
  it('packs slot.offsetX into float 0 of the glyph instance', () => {
    const glyphs: GlyphSource = {
      epoch: 0,
      get(text) {
        return {
          texX: 1,
          texY: 2,
          w: 24, // wider than the 2-cell box (20) — emoji overhang
          h: 20,
          color: true,
          offsetX: text === '✅' ? -2 : 0,
        }
      },
    }
    const f = frame([[run('✅', { cols: 2 })]], [], 0, 4)
    const p = pack(f, new RowCache(), new FrameBuffers(), glyphs)
    expect(p.glyphData[0]).toBe(-2) // a_geom.x — symmetric overhang
    expect(p.glyphData[2]).toBe(24) // quad width = widened slot
  })

  it('slots without offsetX pack 0 (fixed-slot glyphs unchanged)', () => {
    const f = frame([[run('A')]])
    const p = pack(f)
    expect(p.glyphData[0]).toBe(0)
  })
})
