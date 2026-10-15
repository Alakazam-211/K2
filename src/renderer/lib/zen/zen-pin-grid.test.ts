// Garden 1's pin grid geometry and what the Agents widget draws (0.45.6,
// ported from K2 Companion's `zenPinGridColumns`): three across, more rows
// past nine (no paging), one or two pins centred, each agent once.

import { describe, expect, it } from 'vitest'
import {
  ZEN_PIN_AVATAR_PX,
  ZEN_PIN_COLUMNS,
  ZEN_PIN_COL_GAP,
  splitZenPins,
  zenAgentsDisplay,
  zenPinAvatarPx,
  zenPinGridCentred,
  zenPinGridColumns,
  zenPinGridRows,
} from './zen-pin-grid'

describe('zenPinGridColumns', () => {
  it('three or more pins: three equal columns', () => {
    expect(ZEN_PIN_COLUMNS).toBe(3)
    expect(zenPinGridColumns(3)).toBe('repeat(3, minmax(0, 1fr))')
    expect(zenPinGridColumns(4)).toBe('repeat(3, minmax(0, 1fr))')
    expect(zenPinGridColumns(25)).toBe('repeat(3, minmax(0, 1fr))')
  })

  it('one or two pins: that many cells of the three-column width, centred', () => {
    expect(zenPinGridColumns(1)).toBe(`repeat(1, calc((100% - ${2 * ZEN_PIN_COL_GAP}px) / 3))`)
    expect(zenPinGridColumns(2)).toBe(`repeat(2, calc((100% - ${2 * ZEN_PIN_COL_GAP}px) / 3))`)
    expect(zenPinGridCentred(1)).toBe(true)
    expect(zenPinGridCentred(2)).toBe(true)
    expect(zenPinGridCentred(3)).toBe(false)
    expect(zenPinGridCentred(4)).toBe(false)
  })

  it('keeps adding rows past nine: no paging', () => {
    expect(zenPinGridRows(0)).toBe(0)
    expect(zenPinGridRows(1)).toBe(1)
    expect(zenPinGridRows(9)).toBe(3)
    expect(zenPinGridRows(10)).toBe(4)
    expect(zenPinGridRows(19)).toBe(7)
  })
})

describe('zenPinAvatarPx', () => {
  it('is the phone’s 74px when the cell is wide enough, and shrinks to a narrow cell', () => {
    expect(ZEN_PIN_AVATAR_PX).toBe(74)
    expect(zenPinAvatarPx(0)).toBe(74)
    expect(zenPinAvatarPx(600)).toBe(74)
    // 224px of content: (224 - 2 × 37) / 3 = 50.
    expect(zenPinAvatarPx(224)).toBe(50)
    expect(zenPinAvatarPx(90)).toBe(28)
  })
})

interface Row {
  address: string
  pinned?: boolean
}

const order = (l: Row[]): string[] => l.map((r) => r.address)

describe('zenAgentsDisplay', () => {
  const list: Row[] = [
    { address: 'a::local' },
    { address: 'p1::local', pinned: true },
    { address: 'b::local' },
    { address: 'p2::box.k2.dev', pinned: true },
    { address: 'c::local' },
  ]

  it('grid on: pinned rows go to the grid, each row once, order kept', () => {
    const d = zenAgentsDisplay(list, { grid: true, recentFirst: false, sent: {} })
    expect(order(d.pins)).toEqual(['p1::local', 'p2::box.k2.dev'])
    expect(order(d.list)).toEqual(['a::local', 'b::local', 'c::local'])
  })

  it('grid off, recent first off: the view’s own order, nothing moved', () => {
    const d = zenAgentsDisplay(list, { grid: false, recentFirst: false, sent: { 'c::local': 9 } })
    expect(d.pins).toEqual([])
    expect(order(d.list)).toEqual(order(list))
  })

  it('recent first under the grid: the newest sends lead the list', () => {
    const d = zenAgentsDisplay(list, { grid: true, recentFirst: true, sent: { 'c::local': 9, 'b::local': 3, 'p1::local': 50 } })
    expect(order(d.pins)).toEqual(['p1::local', 'p2::box.k2.dev'])
    expect(order(d.list)).toEqual(['c::local', 'b::local', 'a::local'])
  })

  it('recent first with no grid: pinned rows still come first', () => {
    const d = zenAgentsDisplay(list, { grid: false, recentFirst: true, sent: { 'c::local': 9 } })
    expect(order(d.list)).toEqual(['p1::local', 'p2::box.k2.dev', 'c::local', 'a::local', 'b::local'])
  })

  it('splitZenPins with the grid off keeps every row in the list', () => {
    expect(splitZenPins(list, (r) => r.pinned === true, false)).toEqual({ pins: [], rest: list })
  })
})
