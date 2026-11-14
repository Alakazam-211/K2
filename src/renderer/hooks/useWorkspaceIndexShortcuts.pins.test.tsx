// @vitest-environment jsdom
// Home groups: ⌘1–9 on Home opens the group's pinned rows in pin order; a
// group with no pins keeps ⌘N on the Nth row as shown (pins first, then
// most recent first when that's on). ⌥⌘1–9 still switch groups.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

const h = vi.hoisted(() => ({ openHomeRow: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/zen/zen-view', async (orig) => ({
  ...(await orig<typeof import('@/lib/zen/zen-view')>()),
  zenShownNow: () => false,
  zenOnNow: () => false,
}))
vi.mock('@/lib/home-open', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-open')>()),
  openHomeRow: h.openHomeRow,
}))
vi.mock('@/components/Home/home-room', async (orig) => ({
  ...(await orig<typeof import('@/components/Home/home-room')>()),
  homeRowOpenableNow: () => true,
}))

import { useWorkspaceIndexShortcuts } from './useWorkspaceIndexShortcuts'
import { usePageViewStore } from '@/stores/page-view'
import { selectedHome, useHomesStore } from '@/stores/homes'

function chord(digit: number, alt = false): void {
  act(() => {
    window.dispatchEvent(new KeyboardEvent('keydown', { key: String(digit), code: `Digit${digit}`, metaKey: true, altKey: alt }))
  })
}

function opened(): string[] {
  return h.openHomeRow.mock.calls.map((c) => (c[0] as { address: string }).address)
}

/** Exactly these groups (the store keeps at least one), each with rows. */
function setGroups(groups: Array<{ name: string; rows: string[] }>): string[] {
  const st = useHomesStore.getState()
  const before = st.homes.map((x) => x.id)
  const ids: string[] = []
  for (const g of groups) {
    const id = st.createHome(g.name)
    if (!id) throw new Error(`createHome refused ${g.name}`)
    for (const a of g.rows) if (!useHomesStore.getState().addRow(id, { address: a, workspaceId: null, label: a })) throw new Error(`addRow ${a}`)
    ids.push(id)
  }
  for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error('deleteHome refused')
  return ids
}

describe('⌘1–9 on Home with pins', () => {
  let ids: string[] = []
  beforeEach(() => {
    localStorage.clear()
    sessionStorage.clear()
    h.openHomeRow.mockReset()
    usePageViewStore.setState({ page: 'home' })
    ids = setGroups([
      { name: 'Work', rows: ['a::local', 'b::local', 'c::local', 'd::local'] },
      { name: 'Play', rows: ['x::local', 'y::local'] },
    ])
    act(() => useHomesStore.getState().selectHome(ids[0]))
    renderHook(() => useWorkspaceIndexShortcuts())
  })
  afterEach(() => cleanup())

  it('opens the pinned rows in pin order, and nothing past the last pin', () => {
    expect(useHomesStore.getState().pinRow(ids[0], 'c::local')).toBe('pinned')
    expect(useHomesStore.getState().pinRow(ids[0], 'a::local')).toBe('pinned')
    chord(1)
    chord(2)
    expect(opened()).toEqual(['c::local', 'a::local'])
    chord(3)
    expect(opened()).toEqual(['c::local', 'a::local'])
    // Reordering the pins reorders the chords.
    expect(useHomesStore.getState().reorderPins(ids[0], ['a::local', 'c::local'])).toBe(true)
    chord(1)
    expect(opened().at(-1)).toBe('a::local')
  })

  it('a group with no pins: ⌘N is the Nth row as shown (most recent first when on)', () => {
    chord(2)
    expect(opened()).toEqual(['b::local'])
    act(() => {
      useHomesStore.getState().setRecentFirst(ids[0], true)
      useHomesStore.getState().stampRowsMessaged('d::local', 5000)
      useHomesStore.getState().stampRowsMessaged('c::local', 6000)
    })
    chord(1)
    chord(2)
    chord(3)
    expect(opened().slice(1)).toEqual(['c::local', 'd::local', 'a::local'])
  })

  it('⌥⌘N still switches groups', () => {
    chord(2, true)
    expect(selectedHome(useHomesStore.getState()).name).toBe('Play')
    expect(opened()).toEqual([])
  })
})
