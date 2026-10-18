// homes-sync-v1 — the desktop store sync end to end in memory: real Homes
// stores (one per device, each with its own storage) and the runner, against
// one fake home server with the daemon's merge rules.

import { describe, it, expect, beforeEach } from 'vitest'
import { createHomesStore, HOMES_SYNC_STORAGE_KEY, HOMES_WINDOW_SELECTED_KEY, type KeyValueStorage } from '@/stores/homes'
import { HomesSyncRunner, readControl, startSyncing, stopSyncing, statusLine, type HomesSyncStatus } from './runner'
import { FakeHomeServer } from './fake-server'
import { parseSyncState, type AddressMap } from './engine'

const SID = 'aaaaaaaaaaaaaaaa'

class Mem implements KeyValueStorage {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
}

const laptopMap: AddressMap = {
  toWire: (a) => a.replace(/::local$/, `::${SID}`),
  fromWire: (a) => a.replace(new RegExp(`::${SID}$`), '::local'),
}
const phoneMap: AddressMap = {
  toWire: (a) => a.replace(/::rosson\.k2\.dev$/, `::${SID}`),
  fromWire: (a) => a.replace(new RegExp(`::${SID}$`), '::rosson.k2.dev'),
}

let clock = 1_760_000_000_000

interface Device {
  local: Mem
  session: Mem
  store: ReturnType<typeof createHomesStore>
  runner: HomesSyncRunner
  statuses: HomesSyncStatus[]
  status(): HomesSyncStatus
}

function device(server: FakeHomeServer, name: string, map: AddressMap, seed?: (d: Omit<Device, 'runner' | 'statuses' | 'status'>) => void): Device {
  const local = new Mem()
  const session = new Mem()
  let n = 0
  const store = createHomesStore({ local, session, newId: () => `${name}-h${++n}` })
  const partial = { local, session, store }
  seed?.(partial)
  const statuses: HomesSyncStatus[] = []
  const runner = new HomesSyncRunner({
    store,
    transport: server.transport(),
    kv: local,
    addresses: map,
    noteServerId: () => {},
    now: () => ++clock,
    newDeviceId: () => `dev-${name}`,
    device: { name, platform: name === 'phone' ? 'ios' : 'macos' },
    onStatus: (s) => statuses.push(s),
    debounceMs: 0,
  })
  return { ...partial, runner, statuses, status: () => statuses[statuses.length - 1] }
}

function names(d: Device): string[] {
  return d.store.getState().homes.map((h) => h.name)
}

describe('Homes sync runner (desktop store ⇄ home server)', () => {
  let server: FakeHomeServer
  beforeEach(() => {
    server = new FakeHomeServer(SID)
  })

  it('first sync on an empty server pushes this computer’s Homes and registers the device', async () => {
    const lap = device(server, 'laptop', laptopMap)
    lap.store.getState().renameHome(lap.store.getState().homes[0].id, 'Home')
    lap.store.getState().addRow(lap.store.getState().homes[0].id, { address: 'atlas::local', workspaceId: 'w1', label: 'Atlas' })
    lap.runner.start()
    await lap.runner.syncNow()
    expect(server.devices.get('dev-laptop')).toMatchObject({ name: 'laptop', platform: 'macos', removed: false })
    expect(server.view()).toEqual([{ id: 'laptop-h1', name: 'Home', rows: [`atlas::${SID}`] }])
    expect(lap.status().phase).toBe('synced')
    expect(lap.status().pending).toBe(0)
    const st = parseSyncState(lap.local.getItem(HOMES_SYNC_STORAGE_KEY))
    expect(st?.merged).toBe(true)
    expect(st?.cursor).toBe(server.seq)
    lap.runner.stop()
  })

  it('a second device’s first sync combines both lists, keeps old ids as redirects, and keeps its own selection', async () => {
    const lap = device(server, 'laptop', laptopMap, (d) => {
      const s = d.store.getState()
      s.addRow(s.homes[0].id, { address: 'atlas::local', workspaceId: null, label: 'Atlas' })
      s.createHome('Work')
    })
    lap.runner.start()
    await lap.runner.syncNow()
    const phone = device(server, 'phone', phoneMap, (d) => {
      const s = d.store.getState()
      // The phone's own "Home" has the same agent (by another host key) and one more.
      s.addRow(s.homes[0].id, { address: 'atlas::rosson.k2.dev', workspaceId: null, label: 'Atlas' })
      s.addRow(s.homes[0].id, { address: 'nova::rosson.k2.dev', workspaceId: null, label: 'Nova' })
      s.createHome('Errands')
      s.selectHome(s.homes[0].id)
    })
    const phoneOldHome = phone.store.getState().homes[0].id
    phone.runner.start()
    await phone.runner.syncNow()
    await lap.runner.syncNow()
    // Union by name; nothing deleted; Atlas is one row.
    expect(server.view().map((h) => [h.name, h.rows])).toEqual([
      ['Home', [`atlas::${SID}`, `nova::${SID}`]],
      ['Work', []],
      ['Errands', []],
    ])
    expect(names(phone)).toEqual(['Home', 'Work', 'Errands'])
    expect(names(lap)).toEqual(['Home', 'Work', 'Errands'])
    expect(lap.store.getState().homes[0].rows.map((r) => r.address)).toEqual(['atlas::local', 'nova::local'])
    expect(phone.store.getState().homes[0].rows.map((r) => r.address)).toEqual(['atlas::rosson.k2.dev', 'nova::rosson.k2.dev'])
    // The phone's old Home id redirects to the surviving one: its selection followed.
    const survivor = lap.store.getState().homes[0].id
    expect(phone.store.getState().redirects[phoneOldHome]).toBe(survivor)
    expect(phone.store.getState().selectedId).toBe(survivor)
    expect(phone.session.getItem(HOMES_WINDOW_SELECTED_KEY)).toBe(survivor)
    // Selection is per device: the laptop picks Work, the phone stays on Home.
    lap.store.getState().selectHome(lap.store.getState().homes[1].id)
    await lap.runner.syncNow()
    await phone.runner.syncNow()
    expect(phone.store.getState().selectedId).toBe(survivor)
    // The laptop sees the redirect too (Garden seedHome / old selections resolve).
    expect(lap.store.getState().redirects[phoneOldHome]).toBe(survivor)
  })

  it('edits flow both ways; concurrent renames resolve to the later one on both devices', async () => {
    const lap = device(server, 'laptop', laptopMap)
    lap.runner.start()
    await lap.runner.syncNow()
    const phone = device(server, 'phone', phoneMap)
    phone.runner.start()
    await phone.runner.syncNow()
    await lap.runner.syncNow()
    expect(names(phone)).toEqual(['Home'])
    const id = lap.store.getState().homes[0].id
    // Both rename "offline" (before syncing); the phone's is later.
    lap.store.getState().renameHome(id, 'Laptop name')
    phone.store.getState().renameHome(id, 'Phone name')
    await lap.runner.syncNow()
    await phone.runner.syncNow()
    await lap.runner.syncNow()
    expect(names(lap)).toEqual(['Phone name'])
    expect(names(phone)).toEqual(['Phone name'])
    // A row added on the phone shows on the laptop under its own host key.
    phone.store.getState().addRow(id, { address: 'atlas::rosson.k2.dev', workspaceId: null, label: 'Atlas' })
    await phone.runner.syncNow()
    lap.runner.onRemoteCursor(server.seq)
    await lap.runner.syncNow()
    expect(lap.store.getState().homes[0].rows.map((r) => r.address)).toEqual(['atlas::local'])
    // A label repair on the laptop never syncs.
    const pushes = server.pushes
    lap.store.getState().repairRows((r) => ({ ...r, label: 'Atlas (renamed)' }))
    await lap.runner.syncNow()
    expect(server.pushes).toBe(pushes)
    expect(lap.store.getState().homes[0].rows[0].label).toBe('Atlas (renamed)')
  })

  it('offline: the Home screen keeps working, edits queue, and replay merges with the other device on reconnect', async () => {
    const lap = device(server, 'laptop', laptopMap)
    lap.runner.start()
    await lap.runner.syncNow()
    const phone = device(server, 'phone', phoneMap)
    phone.runner.start()
    await phone.runner.syncNow()
    await lap.runner.syncNow()
    const id = lap.store.getState().homes[0].id

    // The phone goes offline (its server can't be reached) and keeps editing.
    const online = server.transport()
    let reachable = false
    const flaky = {
      ...online,
      pull: (d: string, s: number | null) => (reachable ? online.pull(d, s) : Promise.reject(new TypeError('Failed to fetch'))),
      push: (d: string, c: Parameters<typeof online.push>[1]) => (reachable ? online.push(d, c) : Promise.reject(new TypeError('Failed to fetch'))),
    }
    phone.runner.stop()
    const offlinePhone = new HomesSyncRunner({
      store: phone.store,
      transport: flaky,
      kv: phone.local,
      addresses: phoneMap,
      noteServerId: () => {},
      now: () => ++clock,
      newDeviceId: () => 'unused',
      device: { name: 'phone', platform: 'ios' },
      onStatus: (s) => phone.statuses.push(s),
      debounceMs: 0,
      setTimer: () => null,
      clearTimer: () => {},
    })
    offlinePhone.start()
    phone.store.getState().createHome('Phone offline')
    phone.store.getState().addRow(id, { address: 'nova::rosson.k2.dev', workspaceId: null, label: 'Nova' })
    await offlinePhone.syncNow()
    expect(phone.status().phase).toBe('offline')
    expect(phone.status().pending).toBe(2)
    expect(statusLine(phone.status())).toMatch(/Sync paused · 2 changes waiting/)
    // The local copy renders regardless.
    expect(names(phone)).toEqual(['Home', 'Phone offline'])

    // Meanwhile the laptop (online) adds a Home and a row.
    lap.store.getState().createHome('Laptop online')
    lap.store.getState().addRow(id, { address: 'atlas::local', workspaceId: null, label: 'Atlas' })
    await lap.runner.syncNow()

    // Back online: the queue replays, then both converge.
    reachable = true
    await offlinePhone.syncNow()
    await lap.runner.syncNow()
    expect(phone.status().phase).toBe('synced')
    expect(phone.status().pending).toBe(0)
    const want = ['Home', 'Laptop online', 'Phone offline']
    expect([...names(lap)].sort()).toEqual(want)
    expect([...names(phone)].sort()).toEqual(want)
    expect(lap.store.getState().homes[0].rows.map((r) => r.address).sort()).toEqual(['atlas::local', 'nova::local'])
    expect(phone.store.getState().homes[0].rows.map((r) => r.address).sort()).toEqual(['atlas::rosson.k2.dev', 'nova::rosson.k2.dev'])
    // Same order on both devices.
    expect(names(lap)).toEqual(names(phone))
    offlinePhone.stop()
  })

  it('a deleted Home stays deleted on the other device; the last Home is never deleted by sync', async () => {
    const lap = device(server, 'laptop', laptopMap, (d) => d.store.getState().createHome('Work'))
    lap.runner.start()
    await lap.runner.syncNow()
    const phone = device(server, 'phone', phoneMap)
    phone.runner.start()
    await phone.runner.syncNow()
    expect(names(phone)).toEqual(['Home', 'Work'])
    const work = lap.store.getState().homes[1].id
    lap.store.getState().deleteHome(work)
    await lap.runner.syncNow()
    await phone.runner.syncNow()
    expect(names(phone)).toEqual(['Home'])
    // The phone re-pushing a stale view can't bring it back.
    await phone.runner.syncNow()
    expect(server.view().map((h) => h.name)).toEqual(['Home'])
  })

  it('device removal: 410 stops sync (status removed); "Sync again" re-registers and re-runs the union', async () => {
    const lap = device(server, 'laptop', laptopMap)
    lap.runner.start()
    await lap.runner.syncNow()
    const phone = device(server, 'phone', phoneMap)
    phone.runner.start()
    await phone.runner.syncNow()
    // The laptop's Server settings removes the phone.
    server.remove('dev-phone')
    phone.store.getState().createHome('After removal')
    await phone.runner.syncNow()
    expect(phone.status().phase).toBe('removed')
    expect(server.view().map((h) => h.name)).toEqual(['Home'])
    expect(parseSyncState(phone.local.getItem(HOMES_SYNC_STORAGE_KEY))?.removed).toBe(true)
    // The frame alone also marks it (another device id is ignored).
    phone.runner.onDeviceRemoved('someone-else')
    // "Sync again" in Settings: control epoch bump.
    startSyncing(phone.local)
    expect(readControl(phone.local)).toEqual({ enabled: true, epoch: 1 })
    await phone.runner.syncNow()
    expect(phone.status().phase).toBe('synced')
    expect(server.devices.get('dev-phone')?.removed).toBe(false)
    expect(server.view().map((h) => h.name)).toEqual(['Home', 'After removal'])
  })

  it('"Stop syncing" leaves the server’s device list and keeps the Homes; nothing is sent until it starts again', async () => {
    const lap = device(server, 'laptop', laptopMap)
    lap.runner.start()
    await lap.runner.syncNow()
    await stopSyncing(lap.local, server.transport())
    expect(server.devices.get('dev-laptop')?.removed).toBe(true)
    const pushes = server.pushes
    lap.store.getState().createHome('Local only')
    await lap.runner.syncNow()
    expect(lap.status().phase).toBe('off')
    expect(server.pushes).toBe(pushes)
    expect(names(lap)).toEqual(['Home', 'Local only'])
    startSyncing(lap.local)
    await lap.runner.syncNow()
    expect(server.view().map((h) => h.name)).toEqual(['Home', 'Local only'])
  })

  it('a server without homes-sync-v1: unsupported, nothing sent, the Homes untouched', async () => {
    server.supported = false
    const lap = device(server, 'laptop', laptopMap)
    lap.runner.start()
    await lap.runner.syncNow()
    expect(lap.status().phase).toBe('unsupported')
    expect(server.devices.size).toBe(0)
    expect(names(lap)).toEqual(['Home'])
  })

  it('homes_changed with our own cursor does not pull again', async () => {
    const lap = device(server, 'laptop', laptopMap)
    lap.runner.start()
    await lap.runner.syncNow()
    let pulls = 0
    const t = server.transport()
    const counting = { ...t, pull: (d: string, s: number | null) => (pulls++, t.pull(d, s)) }
    const r = new HomesSyncRunner({
      store: lap.store,
      transport: counting,
      kv: lap.local,
      addresses: laptopMap,
      noteServerId: () => {},
      now: () => ++clock,
      newDeviceId: () => 'x',
      device: { name: 'laptop', platform: 'macos' },
      onStatus: () => {},
      debounceMs: 0,
    })
    lap.runner.stop()
    r.start()
    await r.syncNow()
    const before = pulls
    r.onRemoteCursor(server.seq)
    await r.syncNow()
    expect(pulls).toBe(before + 1)
    r.onRemoteCursor(server.seq)
    expect(pulls).toBe(before + 1)
    r.stop()
  })
})
