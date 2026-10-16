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
  zenPinGridText,
  zenPinLimit,
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

describe('pin-limit (0.46.0: Garden 1 sets 9)', () => {
  // Twelve pinned rows with an unpinned row after every fourth.
  const many: Row[] = []
  for (let i = 1; i <= 12; i++) {
    many.push({ address: `p${i}::local`, pinned: true })
    if (i % 4 === 0) many.push({ address: `u${i / 4}::local` })
  }
  const pinned = (n: number): string[] => Array.from({ length: n }, (_, i) => `p${i + 1}::local`)

  it('the first nine pinned rows go to the grid, the 10th on head the list, order kept', () => {
    const d = zenAgentsDisplay(many, { grid: true, recentFirst: false, sent: {}, limit: 9 })
    expect(order(d.pins)).toEqual(pinned(9))
    expect(order(d.list)).toEqual(['p10::local', 'p11::local', 'p12::local', 'u1::local', 'u2::local', 'u3::local'])
  })

  it('nine or fewer pins: all in the grid, the list unchanged', () => {
    const rows: Row[] = Array.from({ length: 9 }, (_, i) => ({ address: `p${i}::local`, pinned: true }))
    rows.splice(4, 0, { address: 'x::local' })
    const d = zenAgentsDisplay(rows, { grid: true, recentFirst: false, sent: {}, limit: 9 })
    expect(d.pins).toHaveLength(9)
    expect(order(d.list)).toEqual(['x::local'])
  })

  it('with recent first on, the overflow pins still head the list, before the newest sends', () => {
    const d = zenAgentsDisplay(many, { grid: true, recentFirst: true, sent: { 'u3::local': 9, 'p11::local': 99 }, limit: 9 })
    expect(order(d.pins)).toEqual(pinned(9))
    expect(order(d.list)).toEqual(['p10::local', 'p11::local', 'p12::local', 'u3::local', 'u1::local', 'u2::local'])
  })

  it('no limit (unset, older daemon): every pinned row in the grid, as before', () => {
    const d = zenAgentsDisplay(many, { grid: true, recentFirst: false, sent: {} })
    expect(order(d.pins)).toEqual(pinned(12))
    expect(order(d.list)).toEqual(['u1::local', 'u2::local', 'u3::local'])
    expect(zenAgentsDisplay(many, { grid: true, recentFirst: false, sent: {}, limit: null })).toEqual(d)
  })

  it('grid off ignores the limit: the view’s own order', () => {
    const d = zenAgentsDisplay(many, { grid: false, recentFirst: false, sent: {}, limit: 9 })
    expect(d.pins).toEqual([])
    expect(order(d.list)).toEqual(order(many))
  })

  it('splitZenPins: limit 1 keeps one pin and moves the rest to the list head', () => {
    const rows: Row[] = [{ address: 'a' }, { address: 'p1', pinned: true }, { address: 'b' }, { address: 'p2', pinned: true }]
    const s = splitZenPins(rows, (r) => r.pinned === true, true, 1)
    expect(order(s.pins)).toEqual(['p1'])
    expect(order(s.rest)).toEqual(['p2', 'a', 'b'])
  })

  it('zenPinLimit reads only an integer of 1 or more', () => {
    expect(zenPinLimit({ 'pin-limit': 9 })).toBe(9)
    expect(zenPinLimit({ 'pin-limit': 1 })).toBe(1)
    expect(zenPinLimit({})).toBeNull()
    expect(zenPinLimit({ 'pin-limit': 0 })).toBeNull()
    expect(zenPinLimit({ 'pin-limit': -3 })).toBeNull()
    expect(zenPinLimit({ 'pin-limit': 2.5 })).toBeNull()
    expect(zenPinLimit({ 'pin-limit': '9' })).toBeNull()
    expect(zenPinLimit({ 'pin-limit': null })).toBeNull()
    expect(zenPinLimit({ 'pin-limit': Number.NaN })).toBeNull()
  })

  it('zenPinGridText: 3 × 3 (9) for nine, “up to” for a partial row, “no limit” for none', () => {
    expect(zenPinGridText(9)).toBe('3 × 3 (9)')
    expect(zenPinGridText(6)).toBe('3 × 2 (6)')
    expect(zenPinGridText(10)).toBe('3 across, up to 10')
    expect(zenPinGridText(null)).toBe('3 across, no limit')
  })
})
