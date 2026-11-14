// People and channels on Home in Homes sync (prd-home-people-channels-v1
// §5.2, migration 0306): the new row kinds round-trip between devices, a
// device that doesn't know a kind leaves it alone, and an older home server
// that refuses them keeps them on this device only, then takes them once it
// updates.

import { describe, expect, it } from 'vitest'
import type { Home } from '@/stores/homes'
import {
  ackPush,
  applyPull,
  buildLocal,
  diffLocal,
  emptySyncState,
  itemKey,
  parsePullAnswer,
  parseWireItem,
  type AddressMap,
  type SyncState,
  type WireItem,
} from './engine'
import { FakeHomeServer } from './fake-server'
import { HomesSyncRunner, type RunnerDeps } from './runner'
import { createHomesStore } from '@/stores/homes'

const SID = 'aaaaaaaaaaaaaaaa'
const map: AddressMap = { toWire: (a) => a, fromWire: (a) => a }
const ctx = (deviceId: string, now: number) => ({ ...map, deviceId, now })

function merged(deviceId: string): SyncState {
  return { ...emptySyncState(deviceId), merged: true, serverId: SID, cursor: 0 }
}

const PERSON = 'k2:connect:3f2a-9b::s1'
const CHANNEL = '184467::s1'

function workHome(): Home {
  return {
    id: 'h1',
    name: 'Work',
    rows: [
      { address: 'atlas::s1', workspaceId: null, label: 'Atlas' },
      { kind: 'person', address: PERSON, workspaceId: null, label: 'Alex', pinOrder: 'V', lastMessagedAt: 5 },
      { kind: 'channel', address: CHANNEL, workspaceId: null, label: 'design' },
    ],
  }
}

describe('homes-sync engine: person and channel rows', () => {
  it('queues each row with its kind and builds it back with its kind', () => {
    const { state, changes } = diffLocal([workHome()], merged('laptop'), ctx('laptop', 100))
    const rows = changes.filter((c) => c.kind !== 'home')
    expect(rows.map((r) => [r.kind, (r as { address: string }).address])).toEqual([
      ['row', 'atlas::s1'],
      ['person', PERSON],
      ['channel', CHANNEL],
    ])
    expect(Object.keys(state.outbox).sort()).toEqual(['c|h1|184467::s1', 'h|h1', 'p|h1|k2:connect:3f2a-9b::s1', 'r|h1|atlas::s1'])
    // Another device receives them.
    const pull = parsePullAnswer({ cursor: 1, full: true, changes, redirects: {}, serverId: SID, kinds: ['home', 'row', 'person', 'channel'] })
    const phone = applyPull(merged('phone'), pull, 200)
    const built = buildLocal(phone, [], map)
    expect(built?.[0].rows).toEqual([
      { address: 'atlas::s1', workspaceId: null, label: 'Atlas' },
      { kind: 'person', address: PERSON, workspaceId: null, label: 'Alex', pinOrder: 'V', lastMessagedAt: 5 },
      { kind: 'channel', address: CHANNEL, workspaceId: null, label: 'design' },
    ])
  })

  it('an agent row and a channel on the same address are two items', () => {
    const a: WireItem = { kind: 'row', homeId: 'h1', address: '42::s1', label: 'x', workspaceId: null, order: 'V', updatedAt: 1 }
    const c: WireItem = { ...a, kind: 'channel' }
    expect(itemKey(a)).not.toBe(itemKey(c))
  })

  it('a device that does not know a kind drops it: never drawn, never tombstoned', () => {
    // The same rule an older client applies to `person` and `channel`
    // (its parseWireItem returns null for them): here a kind from the future.
    const future = { kind: 'garden', homeId: 'h1', address: 'g1::s1', label: 'g', workspaceId: null, order: 'k', updatedAt: 3 }
    expect(parseWireItem(future)).toBeNull()
    const home: WireItem = { kind: 'home', id: 'h1', name: 'Work', order: 'V', updatedAt: 1 }
    const pull = parsePullAnswer({ cursor: 2, full: true, changes: [home, future], redirects: {}, serverId: SID })
    let st = applyPull(merged('old'), pull, 10)
    const built = buildLocal(st, [], map)
    expect(built?.[0].rows).toEqual([])
    // Its next diff has nothing to say about the row it never knew.
    const d = diffLocal(built ?? [], st, ctx('old', 20))
    st = d.state
    expect(d.changes).toEqual([])
  })

  it('an older home server refuses them: they stay here, not synced, and go once it updates', async () => {
    const server = new FakeHomeServer(SID)
    server.kinds = null // an older server: home and row only, no `kinds` in its pulls
    const kv = new Map<string, string>()
    const storage = { getItem: (k: string) => kv.get(k) ?? null, setItem: (k: string, v: string) => void kv.set(k, v) }
    const store = createHomesStore({ local: storage, session: null, newId: () => 'h1' })
    store.getState().renameHome('h1', 'Work')
    store.getState().addRow('h1', { address: 'atlas::s1', workspaceId: null, label: 'Atlas' })
    store.getState().addRow('h1', { kind: 'person', address: PERSON, workspaceId: null, label: 'Alex' })
    const statuses: Array<{ localOnly: number; phase: string }> = []
    let clock = 1_760_000_000_000
    const deps: RunnerDeps = {
      store,
      kv: storage,
      transport: server.transport(),
      device: { name: 'Laptop', platform: 'macos' },
      addresses: map,
      newDeviceId: () => 'laptop',
      now: () => ++clock,
      noteServerId: () => undefined,
      onStatus: (s) => statuses.push({ localOnly: s.localOnly, phase: s.phase }),
      debounceMs: 0,
      setTimer: () => null,
      clearTimer: () => undefined,
    }
    const runner = new HomesSyncRunner(deps)
    runner.start()
    await runner.syncNow()
    await runner.syncNow()
    expect(server.view()[0].rows).toEqual(['atlas::s1'])
    expect(store.getState().homes[0].rows.map((r) => r.address)).toEqual(['atlas::s1', PERSON])
    expect(statuses[statuses.length - 1]).toEqual({ localOnly: 1, phase: 'synced' })
    // The server updates: its pull lists the kinds, and the row goes.
    server.kinds = ['home', 'row', 'person', 'channel']
    await runner.syncNow()
    await runner.syncNow()
    const stored = [...server.items.values()].map((s) => s.item).filter((i) => i.kind === 'person')
    expect(stored.map((i) => (i as { address: string }).address)).toEqual([PERSON])
    expect(statuses[statuses.length - 1]).toEqual({ localOnly: 0, phase: 'synced' })
  })

  it('a refused tombstone is simply forgotten', () => {
    const live: WireItem = { kind: 'person', homeId: 'h1', address: PERSON, label: 'Alex', workspaceId: null, order: 'V', updatedAt: 2, deviceId: 'd' }
    const tomb: WireItem = { ...live, deleted: true, updatedAt: 3 }
    let st = merged('d')
    st = { ...st, known: { [itemKey(tomb)]: tomb }, outbox: { [itemKey(tomb)]: tomb } }
    const acked = ackPush(st, [tomb], [{ item: tomb, reason: 'invalid_kind' }])
    expect(acked.state.known[itemKey(tomb)]).toBeUndefined()
    expect(acked.state.localOnly).toEqual({})
    expect(acked.needsFullPull).toBe(false)
  })
})
