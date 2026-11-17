// @vitest-environment jsdom
//
// Home groups: how a group's agents sort (pins first, then most recent
// first or manual order), the nine-pin limit, the drag rules, and the
// store calls that pin, unpin, reorder and stamp.

import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  MAX_PINS,
  NO_SHORTCUT_INDEX,
  displayRows,
  pinnedRows,
  planRowDrop,
  rowDropLanding,
  shortcutIndexFor,
  shortcutRows,
} from './home-row-order'
import { dropTargetIndex } from '@/components/Sidebar/row-drag-reorder'
import { selectedHome, useHomesStore, type Home, type HomeRow } from '@/stores/homes'

function row(address: string, extra: Partial<HomeRow> = {}): HomeRow {
  return { address, workspaceId: null, label: address, ...extra }
}

const names = (rows: readonly HomeRow[]): string[] => rows.map((r) => r.address.split('::')[0])

describe('the order a group shows', () => {
  const home: Home = {
    id: 'h',
    name: 'Work',
    rows: [
      row('a::s', { lastMessagedAt: 100 }),
      row('b::s', { pinOrder: 'k' }),
      row('c::s'),
      row('d::s', { lastMessagedAt: 300 }),
      row('e::s', { pinOrder: 'V', lastMessagedAt: 999 }),
      row('f::s', { lastMessagedAt: 200 }),
    ],
  }

  it('pins first in pin order, then the manual order', () => {
    expect(names(displayRows(home))).toEqual(['e', 'b', 'a', 'c', 'd', 'f'])
  })

  it('with "Most recent at the top": pins first, then newest send first, never-sent last in manual order', () => {
    expect(names(displayRows({ ...home, recentFirst: true }))).toEqual(['e', 'b', 'd', 'f', 'a', 'c'])
    // Ties keep manual order.
    const tie: Home = { id: 't', name: 'T', recentFirst: true, rows: [row('x::s', { lastMessagedAt: 5 }), row('y::s', { lastMessagedAt: 5 }), row('z::s')] }
    expect(names(displayRows(tie))).toEqual(['x', 'y', 'z'])
  })

  it('⌘1–9 rows: the pins; with no pins, every row as shown', () => {
    expect(names(shortcutRows(home))).toEqual(['e', 'b'])
    const nopins: Home = { ...home, recentFirst: true, rows: home.rows.map((r) => ({ ...r, pinOrder: undefined })) }
    expect(names(shortcutRows(nopins))).toEqual(['e', 'd', 'f', 'a', 'b', 'c'])
    // The number column: only the pinned rows carry one when there are pins.
    expect([0, 1, 2, 5].map((i) => shortcutIndexFor(home, i))).toEqual([0, 1, NO_SHORTCUT_INDEX, NO_SHORTCUT_INDEX])
    expect([0, 1, 2, 5].map((i) => shortcutIndexFor(nopins, i))).toEqual([0, 1, 2, 5])
  })

  it('a drag moves pins among pins, unpinned among unpinned, and nothing in recent-first', () => {
    // Shown: e b | a c d f. Drag the second pin to the top.
    expect(planRowDrop(home, 1, 0)).toEqual({ pins: ['b::s', 'e::s'] })
    // A pin dragged below the pins stays the last pin.
    expect(planRowDrop(home, 0, 5)).toEqual({ pins: ['b::s', 'e::s'] })
    // `d` (shown 4) to the top of the unpinned (shown 2): the manual order
    // changes, the pinned rows keep their slots.
    expect(planRowDrop(home, 4, 2)).toEqual({ rows: ['d::s', 'b::s', 'a::s', 'c::s', 'e::s', 'f::s'] })
    // Dragged into the pins: lands first among the unpinned.
    expect(planRowDrop(home, 5, 0)).toEqual({ rows: ['f::s', 'b::s', 'a::s', 'c::s', 'e::s', 'd::s'] })
    expect(planRowDrop({ ...home, recentFirst: true }, 4, 2)).toBeNull()
    expect(planRowDrop(home, 3, 3)).toBeNull()
  })

  it('the drop line\'s gap: kept to the row\'s own section, none where nothing would move', () => {
    // Shown: e b | a c d f (gaps 0..6; the pins end at gap 2).
    // A pin pointed past the pins lands at their end.
    expect(rowDropLanding(home, 0, 5)).toBe(2)
    expect(rowDropLanding(home, 1, 0)).toBe(0)
    // The last pin pointed past the pins: its own place, no line.
    expect(rowDropLanding(home, 1, 5)).toBeNull()
    // An unpinned row pointed into the pins lands first among the unpinned.
    expect(rowDropLanding(home, 5, 0)).toBe(2)
    expect(rowDropLanding(home, 2, 0)).toBeNull()
    expect(rowDropLanding(home, 2, 6)).toBe(6)
    // Most recent at the top: an unpinned row has nowhere to land; pins still do.
    expect(rowDropLanding({ ...home, recentFirst: true }, 4, 2)).toBeNull()
    expect(rowDropLanding({ ...home, recentFirst: true }, 1, 0)).toBe(0)
    // Out of range.
    expect(rowDropLanding(home, -1, 0)).toBeNull()
    expect(rowDropLanding(home, 6, 0)).toBeNull()
  })

  it('a drop at the line\'s gap writes what a drop at the pointer\'s gap always wrote', () => {
    for (const recentFirst of [false, true]) {
      const h = { ...home, recentFirst }
      const n = displayRows(h).length
      for (let from = 0; from < n; from++) {
        for (let gap = 0; gap <= n; gap++) {
          const to = dropTargetIndex(from, gap)
          const before = to === null ? null : planRowDrop(h, from, to)
          const landing = rowDropLanding(h, from, gap)
          const after = landing === null ? null : planRowDrop(h, from, dropTargetIndex(from, landing)!)
          expect(after, `from ${from} gap ${gap} recentFirst ${recentFirst}`).toEqual(before)
        }
      }
    }
  })
})

/** Exactly one group named Work with these rows. */
function oneGroup(addresses: string[]): string {
  const st = useHomesStore.getState()
  const before = st.homes.map((h) => h.id)
  const id = st.createHome('Work')
  if (!id) throw new Error('createHome refused')
  for (const a of addresses) if (!useHomesStore.getState().addRow(id, row(a))) throw new Error(`addRow ${a}`)
  for (const b of before) if (!useHomesStore.getState().deleteHome(b)) throw new Error('deleteHome refused')
  return id
}

describe('the Homes store: pins, recent first, group order, stamps', () => {
  beforeEach(() => {
    localStorage.clear()
    sessionStorage.clear()
  })

  it('pins up to nine; the tenth is refused as full', () => {
    const addrs = Array.from({ length: 11 }, (_, i) => `a${i}::s`)
    const id = oneGroup(addrs)
    for (let i = 0; i < MAX_PINS; i++) expect(useHomesStore.getState().pinRow(id, addrs[i])).toBe('pinned')
    expect(useHomesStore.getState().pinRow(id, addrs[9])).toBe('full')
    expect(useHomesStore.getState().pinRow(id, addrs[0])).toBe('already')
    expect(useHomesStore.getState().pinRow(id, 'nobody::s')).toBe('missing')
    const home = selectedHome(useHomesStore.getState())
    expect(pinnedRows(home).map((r) => r.address)).toEqual(addrs.slice(0, 9))
    // Unpin one and the tenth fits.
    expect(useHomesStore.getState().unpinRow(id, addrs[4])).toBe(true)
    expect(useHomesStore.getState().unpinRow(id, addrs[4])).toBe(false)
    expect(useHomesStore.getState().pinRow(id, addrs[9])).toBe('pinned')
    expect(pinnedRows(selectedHome(useHomesStore.getState())).at(-1)?.address).toBe(addrs[9])
  })

  it('reorders pins (exactly the pinned set) and keeps keys that are still in order', () => {
    const id = oneGroup(['a::s', 'b::s', 'c::s'])
    for (const a of ['a::s', 'b::s', 'c::s']) useHomesStore.getState().pinRow(id, a)
    const keysBefore = selectedHome(useHomesStore.getState()).rows.map((r) => r.pinOrder)
    expect(useHomesStore.getState().reorderPins(id, ['a::s', 'b::s'])).toBe(false)
    expect(useHomesStore.getState().reorderPins(id, ['c::s', 'a::s', 'b::s'])).toBe(true)
    const after = selectedHome(useHomesStore.getState())
    expect(pinnedRows(after).map((r) => r.address)).toEqual(['c::s', 'a::s', 'b::s'])
    // a and b kept their keys; only c moved.
    expect(after.rows.filter((r) => r.address !== 'c::s').map((r) => r.pinOrder)).toEqual(keysBefore.filter((_, i) => i !== 2))
  })

  it('"Most recent at the top", group order and row order', () => {
    const id = oneGroup(['a::s', 'b::s'])
    const other = useHomesStore.getState().createHome('Play')
    if (!other) throw new Error('createHome refused')
    expect(useHomesStore.getState().setRecentFirst(id, true)).toBe(true)
    expect(useHomesStore.getState().homes[0].recentFirst).toBe(true)
    expect(useHomesStore.getState().setRecentFirst(id, false)).toBe(true)
    expect('recentFirst' in useHomesStore.getState().homes[0]).toBe(false)
    expect(useHomesStore.getState().reorderHomes([other])).toBe(false)
    expect(useHomesStore.getState().reorderHomes([other, id])).toBe(true)
    expect(useHomesStore.getState().homes.map((h) => h.name)).toEqual(['Play', 'Work'])
    expect(useHomesStore.getState().setRowOrder(id, ['b::s', 'a::s'])).toBe(true)
    expect(useHomesStore.getState().homes[1].rows.map((r) => r.address)).toEqual(['b::s', 'a::s'])
    expect(useHomesStore.getState().setRowOrder(id, ['b::s'])).toBe(false)
  })

  it('a send stamps every row on that agent, in every group, and never moves a stamp back', () => {
    const id = oneGroup(['a::s', 'b::s'])
    const other = useHomesStore.getState().createHome('Play')
    if (!other) throw new Error('createHome refused')
    useHomesStore.getState().addRow(other, row('a::s'))
    expect(useHomesStore.getState().stampRowsMessaged('A::S', 500)).toBe(2)
    expect(useHomesStore.getState().stampRowsMessaged('a::s', 400)).toBe(0)
    const stamps = useHomesStore.getState().homes.flatMap((h) => h.rows.map((r) => [h.id === id ? 'work' : 'play', r.address, r.lastMessagedAt ?? null]))
    expect(stamps).toEqual([
      ['work', 'a::s', 500],
      ['work', 'b::s', null],
      ['play', 'a::s', 500],
    ])
  })

  it('a rename repair keeps a row’s pin and last send', () => {
    const id = oneGroup(['old::s'])
    useHomesStore.getState().pinRow(id, 'old::s')
    useHomesStore.getState().stampRowsMessaged('old::s', 77)
    useHomesStore.getState().repairRows((r) => (r.address === 'old::s' ? { address: 'new::s', workspaceId: 'w', label: 'New' } : null))
    const r = selectedHome(useHomesStore.getState()).rows[0]
    expect([r.address, r.label, typeof r.pinOrder, r.lastMessagedAt]).toEqual(['new::s', 'New', 'string', 77])
  })
})
