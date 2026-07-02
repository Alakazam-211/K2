import { describe, it, expect } from 'vitest'
import { FrameBuffers, packFrame, RowCache, RectList } from './packFrame'
import type { PainterFrame } from './painterTypes'
import type { WireCellRun } from '../gridWire'

const THEME = { fg: 0xe0e0e0, bg: 0x0a0a0a, selection: 0x444444 }

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

// Device grid: 10×20 px cells at dpr 2 (css cell 5×10).
function pack(f: PainterFrame, cache = new RowCache(), buffers = new FrameBuffers()) {
  return packFrame({
    frame: f,
    cssCellH: 10,
    deviceCellW: 10,
    deviceCellH: 20,
    dpr: 2,
    cache,
    buffers,
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
