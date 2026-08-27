// Loud tests for Agents extra-group column drag
// (prd-agents-column-resize-drag-v1 R1–R6). Fail if someone brings back
// delta/offsetWidth (zoom gain) or a 15% flex clamp (cursor fight).

import { describe, expect, it } from 'vitest'
import {
  COLUMN_MIN_WIDTH_PX,
  applyColumnResize,
  columnMinPct,
  dragSplitPct,
} from './columnResize'

const ROW = { left: 0, width: 1000 }

describe('dragSplitPct (R1: clientX vs row rect, same client space)', () => {
  it('is (clientX - rowLeft) / rowWidth * 100', () => {
    expect(dragSplitPct(0, ROW)).toBe(0)
    expect(dragSplitPct(400, ROW)).toBe(40)
    expect(dragSplitPct(500, ROW)).toBe(50)
    expect(dragSplitPct(1000, ROW)).toBe(100)
  })

  it('accounts for a non-zero row left', () => {
    expect(dragSplitPct(300, { left: 100, width: 1000 })).toBe(20)
  })

  it('does not mix clientX with offsetWidth (the outrun-the-mouse bug)', () => {
    // html { zoom: 1.2 } scales clientX + getBoundingClientRect, not offsetWidth.
    const zoom = 1.2
    const offsetWidth = 1000
    const rowRect = { left: 0, width: offsetWidth * zoom }
    const clientX = 400 * zoom
    expect(dragSplitPct(clientX, rowRect)).toBeCloseTo(40)
    const wrongDeltaOverOffsetWidth = (clientX / offsetWidth) * 100
    expect(wrongDeltaOverOffsetWidth).toBeCloseTo(48)
    expect(wrongDeltaOverOffsetWidth).not.toBeCloseTo(40)
  })
})

describe('chrome zoom must not change gain (R5)', () => {
  it('zoom 1.0 and 1.2 yield the same split fraction', () => {
    const layout = { left: 100, width: 1000 }
    const zoomed = { left: 100 * 1.2, width: 1000 * 1.2 }
    const x = 100 + 350
    expect(dragSplitPct(x, layout)).toBeCloseTo(35)
    expect(dragSplitPct(x * 1.2, zoomed)).toBeCloseTo(35)
    expect(dragSplitPct(x, layout)).toBeCloseTo(dragSplitPct(x * 1.2, zoomed))
  })

  it('applyColumnResize mid-row is zoom-invariant when not at the min', () => {
    const layout = { left: 0, width: 1000 }
    const zoomed = { left: 0, width: 1200 }
    const a = applyColumnResize({
      clientX: 400,
      rowRect: layout,
      handleIndex: 0,
      flexes: [50, 50],
    })
    const b = applyColumnResize({
      clientX: 480,
      rowRect: zoomed,
      handleIndex: 0,
      flexes: [50, 50],
    })
    expect(a[0]).toBeCloseTo(40)
    expect(b[0]).toBeCloseTo(40)
    expect(a).toEqual(b)
  })
})

describe('min width is px of the same rect, not 15% (R4)', () => {
  it('160px of a 1000px row is 16%, not 15%', () => {
    expect(columnMinPct(ROW)).toBeCloseTo(16)
    expect(COLUMN_MIN_WIDTH_PX).toBe(160)
  })

  it('wide row: 160px is 8% of 2000 — a 15% clamp would stop 140px early', () => {
    const row = { left: 0, width: 2000 }
    expect(columnMinPct(row)).toBeCloseTo(8)
    const out = applyColumnResize({
      clientX: 100,
      rowRect: row,
      handleIndex: 0,
      flexes: [50, 50],
    })
    expect(out[0]).toBeCloseTo(8)
    expect(out[0]).not.toBe(15)
    expect(out[1]).toBeCloseTo(92)
  })

  it('narrow row: 160px is 20% of 800 — a 15% clamp would undershoot', () => {
    const row = { left: 0, width: 800 }
    expect(columnMinPct(row)).toBeCloseTo(20)
    const out = applyColumnResize({
      clientX: 0,
      rowRect: row,
      handleIndex: 0,
      flexes: [50, 50],
    })
    expect(out[0]).toBeCloseTo(20)
    expect(out[0]).not.toBe(15)
    expect(out[1]).toBeCloseTo(80)
  })

  it('pair too narrow for two mins splits evenly (no pause/leap)', () => {
    const row = { left: 0, width: 300 }
    const out = applyColumnResize({
      clientX: 0,
      rowRect: row,
      handleIndex: 0,
      flexes: [50, 50],
    })
    expect(out[0]).toBeCloseTo(50)
    expect(out[1]).toBeCloseTo(50)
  })
})

describe('2-col leftover right (R2)', () => {
  it('left = (clientX-rowLeft)/rowWidth; leftover is right', () => {
    const out = applyColumnResize({
      clientX: 400,
      rowRect: ROW,
      handleIndex: 0,
      flexes: [50, 50, 0],
    })
    expect(out[0]).toBeCloseTo(40)
    expect(out[1]).toBeCloseTo(60)
    expect(out[2]).toBe(0)
  })

  it('clamps the dragged pair and still leaves leftover on the right', () => {
    const out = applyColumnResize({
      clientX: 0,
      rowRect: ROW,
      handleIndex: 0,
      flexes: [50, 50],
    })
    expect(out[0]).toBeCloseTo(16)
    expect(out[1]).toBeCloseTo(84)
  })
})

describe('3-col only the pair sharing the handle (R2)', () => {
  it('handle 0 redistributes cols 0+1; col 2 is unchanged', () => {
    const out = applyColumnResize({
      clientX: 400,
      rowRect: ROW,
      handleIndex: 0,
      flexes: [34, 33, 33],
    })
    expect(out[0]).toBeCloseTo(40)
    expect(out[1]).toBeCloseTo(27)
    expect(out[2]).toBe(33)
  })

  it('handle 1 redistributes cols 1+2; col 0 is unchanged', () => {
    const out = applyColumnResize({
      clientX: 600,
      rowRect: ROW,
      handleIndex: 1,
      flexes: [34, 33, 33],
    })
    expect(out[0]).toBe(34)
    expect(out[1]).toBeCloseTo(26)
    expect(out[2]).toBeCloseTo(40)
  })

  it('handle 0 min-clamp does not steal from col 2', () => {
    const out = applyColumnResize({
      clientX: 0,
      rowRect: ROW,
      handleIndex: 0,
      flexes: [34, 33, 33],
    })
    expect(out[0]).toBeCloseTo(16)
    expect(out[1]).toBeCloseTo(51)
    expect(out[2]).toBe(33)
  })

  it('handle 1 min-clamp does not steal from col 0', () => {
    const out = applyColumnResize({
      clientX: 1000,
      rowRect: ROW,
      handleIndex: 1,
      flexes: [34, 33, 33],
    })
    expect(out[0]).toBe(34)
    expect(out[1]).toBeCloseTo(50)
    expect(out[2]).toBeCloseTo(16)
  })
})
