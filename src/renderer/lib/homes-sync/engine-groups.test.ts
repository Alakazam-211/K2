// Home groups in the homes-sync engine: a group's icon pushes and pulls like
// its name, a left-out icon (an older server or device) keeps the known one,
// and on a first sync an untouched seed Home merges whether its device named
// it "Home" or "Group 1".

import { describe, it, expect } from 'vitest'
import type { Home } from '@/stores/homes'
import {
  applyPull,
  buildLocal,
  diffLocal,
  emptySyncState,
  firstSync,
  parsePullAnswer,
  type AddressMap,
  type PullAnswer,
  type SyncState,
  type WireHome,
  type WireItem,
} from './engine'
import { FakeHomeServer } from './fake-server'

const SID = 'aaaaaaaaaaaaaaaa'
const same: AddressMap = { toWire: (a) => a, fromWire: (a) => a }
const ctx = (deviceId: string, now: number) => ({ ...same, deviceId, now })

function home(id: string, name: string, icon?: string | null, rows: string[] = []): Home {
  const h: Home = { id, name, rows: rows.map((address) => ({ address, workspaceId: null, label: address })) }
  if (icon !== undefined) h.icon = icon
  return h
}

function merged(deviceId: string): SyncState {
  return { ...emptySyncState(deviceId), merged: true, serverId: SID, cursor: 0 }
}

function pull(changes: WireItem[], cursor: number, extra: Partial<PullAnswer> = {}): PullAnswer {
  return { cursor, full: false, changes, redirects: {}, serverId: SID, ...extra }
}

function homeChange(changes: WireItem[], id: string): WireHome {
  const c = changes.find((x): x is WireHome => x.kind === 'home' && x.id === id)
  if (!c) throw new Error(`no change for Home ${id}: ${JSON.stringify(changes)}`)
  return c
}

describe('homes-sync engine: group icons', () => {
  it('setting, keeping and clearing an icon: only real changes queue, and a clear says null', () => {
    let st = merged('laptop')
    const first = diffLocal([home('h1', 'Work')], st, ctx('laptop', 100))
    expect('icon' in homeChange(first.changes, 'h1')).toBe(false)
    st = first.state

    const set = diffLocal([home('h1', 'Work', 'briefcase')], st, ctx('laptop', 200))
    expect(homeChange(set.changes, 'h1').icon).toBe('briefcase')
    st = set.state

    const same = diffLocal([home('h1', 'Work', 'briefcase')], st, ctx('laptop', 300))
    expect(same.changes).toEqual([])

    const cleared = diffLocal([home('h1', 'Work')], st, ctx('laptop', 400))
    const c = homeChange(cleared.changes, 'h1')
    expect(c.icon).toBeNull()
    expect('icon' in c).toBe(true)
  })

  it('a newer server icon lands locally; an item that leaves icon out keeps the known one', () => {
    let st = merged('laptop')
    st = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'Work', order: 'V', updatedAt: 10, deviceId: 'phone', icon: 'rocket' }], 1), 1000)
    expect(buildLocal(st, [], same)).toEqual([{ id: 'h1', name: 'Work', rows: [], icon: 'rocket' }])
    // An older server (or a device that predates icons) renames it: no icon key.
    st = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'Work 2', order: 'V', updatedAt: 20, deviceId: 'old' }], 2), 1000)
    expect(buildLocal(st, [], same)).toEqual([{ id: 'h1', name: 'Work 2', rows: [], icon: 'rocket' }])
    // An explicit null clears it.
    st = applyPull(st, pull([{ kind: 'home', id: 'h1', name: 'Work 2', order: 'V', updatedAt: 30, deviceId: 'phone', icon: null }], 3), 1000)
    expect(buildLocal(st, [], same)).toEqual([{ id: 'h1', name: 'Work 2', rows: [] }])
  })

  it('parses icon from a server answer: a name, null, or nothing when malformed', () => {
    const a = parsePullAnswer({
      cursor: 1,
      changes: [
        { kind: 'home', id: 'a', name: 'A', order: 'V', updatedAt: 1, icon: 'gamepad-2' },
        { kind: 'home', id: 'b', name: 'B', order: 'k', updatedAt: 1, icon: null },
        { kind: 'home', id: 'c', name: 'C', order: 's', updatedAt: 1, icon: 'Not An Icon' },
        { kind: 'home', id: 'd', name: 'D', order: 't', updatedAt: 1 },
      ],
    })
    const icons = a.changes.map((c) => (c.kind === 'home' ? ('icon' in c ? c.icon : 'absent') : 'row'))
    expect(icons).toEqual(['gamepad-2', null, 'absent', 'absent'])
  })

  it('the icon round-trips through a server, and an older device’s rename keeps it', () => {
    const server = new FakeHomeServer(SID)
    server.register('new', 'New', 'macos')
    server.register('old', 'Old', 'macos')
    const pushed = diffLocal([home('h1', 'Work', 'briefcase')], merged('new'), ctx('new', 100))
    server.push('new', pushed.changes)
    // The older device knows no icons: its rename carries no icon key.
    server.push('old', [{ kind: 'home', id: 'h1', name: 'Work stuff', order: 'V', updatedAt: 500, deviceId: 'old' }])
    const answer = parsePullAnswer(server.pull('new', null))
    const st = applyPull(pushed.state, answer, 1000)
    expect(buildLocal(st, [], same)).toEqual([{ id: 'h1', name: 'Work stuff', rows: [], icon: 'briefcase' }])
  })
})

describe('homes-sync engine: first sync and the seed Home', () => {
  const serverSeed = (name: string, extra: Partial<WireHome> = {}): WireItem => ({
    kind: 'home',
    id: 'srv',
    name,
    order: 'V',
    updatedAt: 5,
    deviceId: 'older',
    ...extra,
  })

  it('an untouched "Group 1" merges into an untouched "Home" (and the other way round)', () => {
    for (const [serverName, localName] of [['Home', 'Group 1'], ['Group 1', 'Home']]) {
      const { state, changes } = firstSync([home('loc', localName)], pull([serverSeed(serverName)], 1, { full: true }), emptySyncState('new'), ctx('new', 100))
      expect(state.redirects.loc, `${localName} → ${serverName}`).toBe('srv')
      expect(changes).toEqual([
        { kind: 'home', id: 'loc', name: localName, order: '', updatedAt: 100, deviceId: 'new', deleted: true, mergedInto: 'srv' },
      ])
      expect(buildLocal(state, [], same)?.map((h) => h.name)).toEqual([serverName])
    }
  })

  it('a seed that is not untouched stays its own group', () => {
    // The local seed has an agent on it.
    let r = firstSync([home('loc', 'Group 1', undefined, ['a::s'])], pull([serverSeed('Home')], 1, { full: true }), emptySyncState('new'), ctx('new', 100))
    expect(r.state.redirects.loc).toBeUndefined()
    expect(buildLocal(r.state, [], same)?.map((h) => h.name)).toEqual(['Home', 'Group 1'])
    // The server seed has a row.
    const row: WireItem = { kind: 'row', homeId: 'srv', address: 'a::s', label: 'a', workspaceId: null, order: 'V', updatedAt: 5, deviceId: 'older' }
    r = firstSync([home('loc', 'Group 1')], pull([serverSeed('Home'), row], 1, { full: true }), emptySyncState('new'), ctx('new', 100))
    expect(r.state.redirects.loc).toBeUndefined()
    // The server seed has an icon, or a name of its own.
    r = firstSync([home('loc', 'Group 1')], pull([serverSeed('Home', { icon: 'rocket' })], 1, { full: true }), emptySyncState('new'), ctx('new', 100))
    expect(r.state.redirects.loc).toBeUndefined()
    r = firstSync([home('loc', 'Group 1')], pull([serverSeed('Work')], 1, { full: true }), emptySyncState('new'), ctx('new', 100))
    expect(r.state.redirects.loc).toBeUndefined()
  })

  it('a local group merged by name hands its icon to a server group that has none', () => {
    const { changes } = firstSync([home('loc', 'Work', 'briefcase')], pull([serverSeed('Work')], 1, { full: true }), emptySyncState('new'), ctx('new', 100))
    const up = homeChange(changes, 'srv')
    expect([up.icon, up.name, up.updatedAt > 5]).toEqual(['briefcase', 'Work', true])
  })
})
