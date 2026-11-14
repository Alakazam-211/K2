// People and channels on Home in the local Homes store
// (prd-home-people-channels-v1 §5.3, D4, D11): the rows keep their kind,
// a person's address must be a box person and a channel's a Sew id, pins
// and "Most recent at the top" work across kinds, the person's own sends
// stamp a chat row, and "Also on" swaps a row in place.

import { describe, expect, it } from 'vitest'
import { createHomesStore, parseHomesDoc, repairHomeRowsFromList, type KeyValueStorage } from './homes'
import { displayRows, shortcutRows } from '@/lib/home-row-order'

class Mem implements KeyValueStorage {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.get(k) ?? null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
}

function store() {
  return createHomesStore({ local: new Mem(), session: new Mem(), newId: () => 'h1' })
}

const ALEX = 'k2:connect:3f2a-9b::box.example.test'
const DESIGN = '184467::box.example.test'

describe('Homes store: person and channel rows', () => {
  it('adds them with their kind, and refuses addresses that are not what the kind says', () => {
    const s = store()
    expect(s.getState().addRow('h1', { kind: 'person', address: ALEX, workspaceId: null, label: 'Alex' })).toBe(true)
    expect(s.getState().addRow('h1', { kind: 'channel', address: DESIGN, workspaceId: null, label: 'design' })).toBe(true)
    // Never an agent's or an App user's id as a person; never a name as a channel.
    for (const bad of [
      { kind: 'person' as const, address: 'k2:workspace:5d1c::box.example.test' },
      { kind: 'person' as const, address: 'k2:principal:app-1::box.example.test' },
      { kind: 'person' as const, address: 'atlas::box.example.test' },
      { kind: 'channel' as const, address: 'general::box.example.test' },
    ]) {
      expect(s.getState().addRow('h1', { ...bad, workspaceId: null, label: 'x' })).toBe(false)
    }
    const rows = s.getState().homes[0].rows
    expect(rows.map((r) => [r.kind ?? 'agent', r.address])).toEqual([
      ['person', ALEX],
      ['channel', DESIGN],
    ])
    // The stored doc reads back with the kinds.
    const doc = parseHomesDoc(JSON.stringify({ version: 1, homes: s.getState().homes }))
    expect(doc?.homes[0].rows.map((r) => r.kind)).toEqual(['person', 'channel'])
  })

  it('pins and "Most recent at the top" apply to people and channels as to agents', () => {
    const s = store()
    s.getState().addRow('h1', { address: 'atlas::box.example.test', workspaceId: null, label: 'Atlas' })
    s.getState().addRow('h1', { kind: 'person', address: ALEX, workspaceId: null, label: 'Alex' })
    s.getState().addRow('h1', { kind: 'channel', address: DESIGN, workspaceId: null, label: 'design' })
    expect(s.getState().pinRow('h1', DESIGN)).toBe('pinned')
    let home = s.getState().homes[0]
    expect(shortcutRows(home).map((r) => r.address)).toEqual([DESIGN])
    // Recent first: the person's own send stamps Alex's row; Atlas never sent to.
    s.getState().setRecentFirst('h1', true)
    expect(s.getState().stampRowsMessaged(ALEX, 5_000)).toBe(1)
    home = s.getState().homes[0]
    expect(displayRows(home).map((r) => r.address)).toEqual([DESIGN, ALEX, 'atlas::box.example.test'])
    // A stamp never moves back.
    expect(s.getState().stampRowsMessaged(ALEX, 4_000)).toBe(0)
  })

  it('a server list never repairs people or channels as workspaces', () => {
    const s = store()
    s.getState().addRow('h1', { kind: 'channel', address: DESIGN, workspaceId: null, label: 'design' })
    repairHomeRowsFromList(s, 'box.example.test', [{ id: 'w1', handle: '184467', name: 'Not a channel' }])
    expect(s.getState().homes[0].rows).toEqual([{ kind: 'channel', address: DESIGN, workspaceId: null, label: 'design' }])
  })

  it('"Also on" puts the other server\'s person in the same place with the same pin', () => {
    const s = store()
    s.getState().addRow('h1', { address: 'atlas::box.example.test', workspaceId: null, label: 'Atlas' })
    s.getState().addRow('h1', { kind: 'person', address: ALEX, workspaceId: null, label: 'Alex' })
    s.getState().addRow('h1', { kind: 'channel', address: DESIGN, workspaceId: null, label: 'design' })
    s.getState().pinRow('h1', ALEX)
    s.getState().stampRowsMessaged(ALEX, 9)
    const other = 'k2:connect:77aa::lab.example.test'
    expect(s.getState().replaceRow('h1', ALEX, { kind: 'person', address: other, workspaceId: null, label: 'Alex' })).toBe(true)
    const rows = s.getState().homes[0].rows
    expect(rows[1]).toMatchObject({ kind: 'person', address: other, label: 'Alex', lastMessagedAt: 9 })
    expect(rows[1].pinOrder).toBeTruthy()
    // Onto an address already on the Home: refused.
    expect(s.getState().replaceRow('h1', other, { kind: 'channel', address: DESIGN, workspaceId: null, label: 'x' })).toBe(false)
  })
})
