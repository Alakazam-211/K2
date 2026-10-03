// Home M4/M5 — the Home rooms manager (stores/home-rooms.ts): opening a
// remote row builds ONE pinned room on its server — usable when that server
// answers its layout with a revision (M5), view only on an older server —
// the tiers open and close its sockets (hot = mounted, warm = its workspace
// socket only, cold = disposed), re-showing a cold room rebuilds it, the
// keep-alive follows, and the room's project list follows its server.

import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

// The switch tests flip the real window server: the primary stores reload
// against it. Nothing here may reach a network.
globalThis.fetch = (async (input: RequestInfo | URL) => {
  throw new Error(`no network in this test: ${String(input)}`)
}) as typeof fetch

import { createHomeRooms, installWindowHostSwitch, promoteToPrimary, type HomeRooms } from '@/stores/home-rooms'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { clearHostSelect, peekHostSelect, takeHostSelect } from '@/lib/home-pending-select'
import { createRoomTierManager, type RoomTierManager } from '@/lib/room-tiers'
import { KEEP_ALIVE_HOT_INTERVAL_MS } from '@/lib/room-keep-alive'
import type { PinnedRoom, PinnedRoomInput } from '@/stores/room'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { HomeRow } from '@/stores/homes'

const MIN = 60_000

/** One fake room: records open/dispose, holds a "socket" count that open
 *  raises and dispose drops (the tabs store's one workspace socket). */
interface FakeRoom {
  room: PinnedRoom
  input: PinnedRoomInput
  opened: number
  disposed: number
  sockets: number
  /** ensurePinnedAgentTabForMode calls (agent mode, project path). */
  pinned: Array<[string, string]>
}

function harness() {
  let now = 1_000_000
  const timers: Array<{ at: number; fn: () => void; every: number | null; id: number }> = []
  let nextId = 1
  const tiers: RoomTierManager = createRoomTierManager({
    now: () => now,
    setTimer: (fn, ms) => {
      const id = nextId++
      timers.push({ at: now + ms, fn, every: null, id })
      return id
    },
    clearTimer: (h) => {
      const i = timers.findIndex((t) => t.id === h)
      if (i >= 0) timers.splice(i, 1)
    },
  })
  const rooms: FakeRoom[] = []
  const keepAlives: Array<[string, string]> = []
  const lists = new Map<string, ProjectWithWorkspaces[] | Error>()
  /** host key → does it answer `with_revision` (absent = yes). */
  const revisions = new Map<string, boolean | Error>()
  const revisionProbes: Array<[string, string, string]> = []
  const knownServers: string[] = []
  /** host key → its projects_changed handlers. */
  const projectsChanged = new Map<string, Set<() => void>>()
  const scopes = new Map<string, ServerScope>()
  /** Rows promoted to the primary room (Z7). */
  const promoted: HomeRow[] = []
  /** Z42: host key → what the pool knows once `knowServer` resolved
   *  (absent = not read: the room goes on). */
  const floors = new Map<string, { belowFloor: boolean; version: string | null }>()
  /** Z5/Z42: below-floor fallbacks (row address, host, version, switched). */
  const belowFloor: Array<[string, string, string | null, boolean]> = []
  const scopeFor = (hostKey: string): ServerScope => {
    let s = scopes.get(hostKey)
    if (!s) {
      s = { id: `host:${hostKey}`, hostKey, isPrimary: false, isRemote: true, label: hostKey } as unknown as ServerScope
      scopes.set(hostKey, s)
    }
    return s
  }
  const homeRooms: HomeRooms = createHomeRooms({
    tiers,
    createRoom: (input) => {
      const fake = { input, opened: 0, disposed: 0, sockets: 0, pinned: [] } as unknown as FakeRoom
      fake.room = {
        key: `${input.scope.hostKey}|${input.workspace.projectId}:${input.workspace.workspaceId}`,
        isPrimary: false,
        readOnly: input.readOnly === true,
        scope: input.scope,
        tabs: {
          room: {
            open: async () => {
              fake.opened += 1
              fake.sockets += 1
            },
            ensurePinnedAgentTabForMode: (mode: string, path: string) => {
              fake.pinned.push([mode, path])
            },
          },
        },
        dispose: async () => {
          fake.disposed += 1
          fake.sockets = 0
        },
      } as unknown as PinnedRoom
      rooms.push(fake)
      return fake.room
    },
    listProjects: async (scope) => {
      const l = lists.get(scope.hostKey)
      if (!l) throw new Error(`no list for ${scope.hostKey}`)
      if (l instanceof Error) throw l
      return l
    },
    keepAlive: async (hostKey, projectId) => {
      keepAlives.push([hostKey, projectId])
      return 'sent'
    },
    scopeFor,
    knowServer: async (hostKey) => {
      knownServers.push(hostKey)
    },
    layoutRevisionSupported: async (scope, projectId, workspaceId) => {
      revisionProbes.push([scope.hostKey, projectId, workspaceId])
      const r = revisions.get(scope.hostKey)
      if (r instanceof Error) throw r
      return r === undefined ? true : r
    },
    onProjectsChanged: (scope, fn) => {
      const set = projectsChanged.get(scope.hostKey) ?? new Set()
      set.add(fn)
      projectsChanged.set(scope.hostKey, set)
      return () => set.delete(fn)
    },
    promote: (r) => {
      promoted.push(r)
    },
    floorCheck: (hostKey) => floors.get(hostKey) ?? null,
    belowFloor: (r, hostKey, version, switchWindow) => {
      belowFloor.push([r.address, hostKey, version, switchWindow])
    },
    now: () => now,
    setInterval: (fn, ms) => {
      const id = nextId++
      timers.push({ at: now + ms, fn, every: ms, id })
      return id
    },
    clearInterval: (h) => {
      const i = timers.findIndex((t) => t.id === h)
      if (i >= 0) timers.splice(i, 1)
    },
  })
  const advance = async (ms: number): Promise<void> => {
    const until = now + ms
    for (;;) {
      timers.sort((a, b) => a.at - b.at)
      const next = timers[0]
      if (!next || next.at > until) break
      now = next.at
      if (next.every === null) timers.shift()
      else next.at = now + next.every
      next.fn()
      await Promise.resolve()
    }
    now = until
    await new Promise((r) => setTimeout(r, 0))
  }
  return {
    tiers,
    homeRooms,
    rooms,
    keepAlives,
    lists,
    revisions,
    revisionProbes,
    knownServers,
    projectsChanged,
    promoted,
    floors,
    belowFloor,
    advance,
    intervals: () => timers.filter((t) => t.every !== null).length,
  }
}

function project(id: string, handle: string, path: string): ProjectWithWorkspaces {
  return {
    id,
    name: handle,
    path,
    handle,
    workspaces: [{ id: `${id}-ws`, tabOrder: 0, worktreePath: null }],
  } as unknown as ProjectWithWorkspaces
}

function row(handle: string, host: string): HomeRow {
  return { address: `${handle}::${host}`, workspaceId: null, label: handle }
}

describe('Home rooms (M4, M5)', () => {
  let h: ReturnType<typeof harness>
  beforeEach(() => {
    h = harness()
    h.lists.set('b.test', [project('pb', 'anna', '/srv/anna')])
    h.lists.set('c.test', [project('pc', 'appa', '/srv/appa')])
  })

  it('opens ONE usable room on the row’s server (it answers with_revision), shows it hot, opens its socket and keeps it alive', async () => {
    const entry = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(entry.phase).toBe('open')
    expect(entry.access).toBe('use')
    expect(h.knownServers).toEqual(['b.test'])
    expect(h.revisionProbes).toEqual([['b.test', 'pb', 'pb-ws']])
    expect(h.rooms.length).toBe(1)
    const fake = h.rooms[0]
    expect(fake.input.readOnly).toBe(false)
    expect(fake.input.scope.hostKey).toBe('b.test')
    expect(fake.input.workspace).toEqual({ projectId: 'pb', workspaceId: 'pb-ws', path: '/srv/anna' })
    expect(fake.opened).toBe(1)
    expect(fake.sockets).toBe(1)
    // Its pinned Chat / Inbox tabs, as the window's room gets them.
    expect(fake.pinned).toEqual([['off', '/srv/anna']])
    expect(h.tiers.tier(fake.room.key)).toBe('hot')
    expect(h.homeRooms.store.getState().shown).toBe('anna::b.test')
    // projects/activate on B through the pool, on open; and the room's own
    // activate gesture goes there too.
    await Promise.resolve()
    expect(h.keepAlives).toEqual([['b.test', 'pb']])
    fake.input.activateProject('pb')
    expect(h.keepAlives[1]).toEqual(['b.test', 'pb'])
    // Hot rooms re-send hourly.
    expect(h.intervals()).toBe(1)
    await h.advance(KEEP_ALIVE_HOT_INTERVAL_MS)
    expect(h.keepAlives.filter(([k]) => k === 'b.test').length).toBe(3)
  })

  it('Z42: a server found below the floor once the pool knows it: no room, the entry is gone, and the shown row switches', async () => {
    h.floors.set('b.test', { belowFloor: true, version: '0.40.150' })
    const entry = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(entry.phase).toBe('switched')
    expect(h.knownServers).toEqual(['b.test'])
    expect(h.rooms.length).toBe(0)
    expect(h.revisionProbes).toEqual([])
    expect(h.keepAlives).toEqual([])
    expect(h.homeRooms.store.getState().entries['anna::b.test']).toBe(undefined)
    expect(h.homeRooms.store.getState().shown).toBe(null)
    expect(h.belowFloor).toEqual([['anna::b.test', 'b.test', '0.40.150', true]])
  })

  it('Z42: no version counts as below the floor too; a row the user already left only tears down (no switch)', async () => {
    h.floors.set('b.test', { belowFloor: true, version: null })
    const pending = h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    // The user moves on to C before B's check lands.
    const c = h.homeRooms.open(row('appa', 'c.test'), 'c.test')
    expect((await pending).phase).toBe('switched')
    expect((await c).phase).toBe('open')
    expect(h.belowFloor).toEqual([['anna::b.test', 'b.test', null, false]])
    expect(h.homeRooms.store.getState().shown).toBe('appa::c.test')
    expect(Object.keys(h.homeRooms.store.getState().entries)).toEqual(['appa::c.test'])
  })

  it('Z42: a server at or above the floor opens as before; an unread one (no boot) is not refused', async () => {
    h.floors.set('b.test', { belowFloor: false, version: '0.41.6' })
    expect((await h.homeRooms.open(row('anna', 'b.test'), 'b.test')).phase).toBe('open')
    expect((await h.homeRooms.open(row('appa', 'c.test'), 'c.test')).phase).toBe('open')
    expect(h.belowFloor).toEqual([])
    expect(h.rooms.length).toBe(2)
  })

  it('opening it again shows the same room (no second instance, no second open)', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    h.homeRooms.showPrimary()
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(h.rooms.length).toBe(1)
    expect(h.rooms[0].opened).toBe(1)
    expect(h.homeRooms.store.getState().shown).toBe('anna::b.test')
  })

  it('tiers: hidden stays hot 5 min, then warm (socket kept, keep-alive stops), then cold (disposed: socket closed)', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    const fake = h.rooms[0]
    h.homeRooms.showPrimary()
    expect(h.homeRooms.store.getState().shown).toBe(null)
    expect(h.tiers.tier(fake.room.key)).toBe('hot')

    await h.advance(5 * MIN)
    expect(h.tiers.tier(fake.room.key)).toBe('warm')
    expect(fake.disposed).toBe(0)
    expect(fake.sockets).toBe(1)
    expect(h.intervals()).toBe(0)

    await h.advance(25 * MIN)
    expect(h.tiers.tier(fake.room.key)).toBe('cold')
    expect(fake.disposed).toBe(1)
    expect(fake.sockets).toBe(0)
    expect(h.homeRooms.store.getState().entries['anna::b.test']).toBe(undefined)
  })

  it('showing a cold room again rebuilds it and reopens its socket', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    h.homeRooms.showPrimary()
    await h.advance(30 * MIN)
    expect(h.rooms[0].sockets).toBe(0)

    const again = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(again.phase).toBe('open')
    expect(h.rooms.length).toBe(2)
    expect(h.rooms[1].opened).toBe(1)
    expect(h.rooms[1].sockets).toBe(1)
    expect(h.tiers.tier(h.rooms[1].room.key)).toBe('hot')
    expect(again.generation).toBeGreaterThan(0)
  })

  it('a fourth hot room demotes the least recent to warm; switching rooms hides the previous one', async () => {
    h.lists.set('d.test', [project('pd', 'dora', '/srv/dora')])
    h.lists.set('e.test', [project('pe', 'eve', '/srv/eve')])
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    await h.homeRooms.open(row('appa', 'c.test'), 'c.test')
    await h.homeRooms.open(row('dora', 'd.test'), 'd.test')
    await h.homeRooms.open(row('eve', 'e.test'), 'e.test')
    const [b, c, d, e] = h.rooms
    expect(h.tiers.tier(e.room.key)).toBe('hot')
    expect(h.tiers.tier(d.room.key)).toBe('hot')
    expect(h.tiers.tier(c.room.key)).toBe('hot')
    expect(h.tiers.tier(b.room.key)).toBe('warm')
    expect(b.sockets).toBe(1)
    expect(h.tiers.store.getState().rooms[d.room.key].visible).toBe(false)
  })

  it('Home leaving the screen starts the shown room’s clock; coming back stops it', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    const key = h.rooms[0].room.key
    h.homeRooms.setPageVisible(false)
    expect(h.tiers.store.getState().rooms[key].visible).toBe(false)
    h.homeRooms.setPageVisible(true)
    expect(h.tiers.store.getState().rooms[key].visible).toBe(true)
    await h.advance(60 * MIN)
    expect(h.tiers.tier(key)).toBe('hot')
  })

  it('a server that cannot list its projects leaves the row in error, with no room; retry builds it', async () => {
    h.lists.set('b.test', new Error('B is offline'))
    const failed = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(failed.phase).toBe('error')
    expect(failed.error).toBe('B is offline')
    expect(h.rooms.length).toBe(0)
    h.lists.set('b.test', [project('pb', 'anna', '/srv/anna')])
    const retried = await h.homeRooms.retry('anna::b.test')
    if (!retried) throw new Error('retry returned no entry')
    expect(retried.phase).toBe('open')
    expect(h.rooms.length).toBe(1)
  })

  it('an older server (no layout revision) opens the room view only (MS43)', async () => {
    h.revisions.set('b.test', false)
    const entry = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(entry.phase).toBe('open')
    expect(entry.access).toBe('view-older-server')
    expect(h.rooms.length).toBe(1)
    expect(h.rooms[0].input.readOnly).toBe(true)
    expect(h.rooms[0].opened).toBe(1)
  })

  it('a revision probe that fails leaves the row in error with no room', async () => {
    h.revisions.set('b.test', new Error('B went away'))
    const entry = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(entry.phase).toBe('error')
    expect(entry.error).toBe('B went away')
    expect(h.rooms.length).toBe(0)
  })

  it('the room’s project list follows its server’s projects_changed; closing the room stops listening', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    const fake = h.rooms[0]
    const handlers = h.projectsChanged.get('b.test')
    if (!handlers) throw new Error('no projects_changed handler on b.test')
    expect(handlers.size).toBe(1)
    const withWorktree = project('pb', 'anna', '/srv/anna')
    withWorktree.workspaces.push({ id: 'pb-wt', tabOrder: 1, worktreePath: '/srv/anna-wt' } as never)
    h.lists.set('b.test', [withWorktree])
    for (const fn of handlers) fn()
    await new Promise((r) => setTimeout(r, 0))
    expect(fake.input.projects.getState().projects[0].workspaces.map((w) => w.id)).toEqual(['pb-ws', 'pb-wt'])
    await h.homeRooms.close('anna::b.test')
    expect(handlers.size).toBe(0)
  })

  it('a row whose agent is gone on that server is not-found', async () => {
    const entry = await h.homeRooms.open(row('ghost', 'b.test'), 'b.test')
    expect(entry.phase).toBe('not-found')
    expect(h.rooms.length).toBe(0)
  })

  it('closeAll disposes every room and shows the window’s own room', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    await h.homeRooms.open(row('appa', 'c.test'), 'c.test')
    await h.homeRooms.closeAll()
    expect(h.rooms.map((r) => r.disposed)).toEqual([1, 1])
    expect(h.rooms.map((r) => r.sockets)).toEqual([0, 0])
    expect(h.homeRooms.store.getState()).toEqual({ entries: {}, shown: null })
    expect(Object.keys(h.tiers.store.getState().rooms)).toEqual([])
  })
})

// prd-home-seamless-0432 item 2 (Z6, Z7, Z29; T2.1): a top-switcher change
// keeps the rooms. Only the destination's rooms close; a shown one is
// promoted to the primary room.
describe('a window server switch keeps Home rooms (Z6/Z7)', () => {
  let h: ReturnType<typeof harness>
  beforeEach(async () => {
    h = harness()
    h.lists.set('b.test', [project('pb', 'anna', '/srv/anna')])
    h.lists.set('c.test', [project('pc', 'appa', '/srv/appa')])
    await h.homeRooms.open(row('appa', 'c.test'), 'c.test')
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
  })

  it('a same Home key (a session mint, an id-only re-key) disposes nothing and keeps `shown`', async () => {
    await h.homeRooms.onWindowHostChanged('local', 'local')
    await h.homeRooms.onWindowHostChanged('b.test', 'b.test')
    expect(h.rooms.map((r) => r.disposed)).toEqual([0, 0])
    expect(h.rooms.map((r) => r.sockets)).toEqual([1, 1])
    expect(h.homeRooms.store.getState().shown).toBe('anna::b.test')
    expect(Object.keys(h.homeRooms.store.getState().entries).sort()).toEqual(['anna::b.test', 'appa::c.test'])
    expect(h.promoted).toEqual([])
  })

  it('a switch to a third server keeps every room: same objects, same generation, same tier, same keep-alive', async () => {
    const before = h.homeRooms.store.getState()
    const keepAlivesBefore = h.intervals()
    await h.homeRooms.onWindowHostChanged('local', 'd.test')
    const after = h.homeRooms.store.getState()
    expect(after.shown).toBe('anna::b.test')
    expect(after.entries['anna::b.test']).toBe(before.entries['anna::b.test'])
    expect(after.entries['appa::c.test']).toBe(before.entries['appa::c.test'])
    expect(h.rooms.map((r) => [r.opened, r.disposed, r.sockets])).toEqual([
      [1, 0, 1],
      [1, 0, 1],
    ])
    expect(h.tiers.tier(h.rooms[1].room.key)).toBe('hot')
    expect(h.intervals()).toBe(keepAlivesBefore)
    expect(h.promoted).toEqual([])
  })

  it('a switch to C disposes only C’s room; the shown B room stays shown', async () => {
    await h.homeRooms.onWindowHostChanged('local', 'c.test')
    expect(h.rooms.map((r) => r.disposed)).toEqual([1, 0])
    expect(h.rooms.map((r) => r.sockets)).toEqual([0, 1])
    const s = h.homeRooms.store.getState()
    expect(Object.keys(s.entries)).toEqual(['anna::b.test'])
    expect(s.shown).toBe('anna::b.test')
    expect(Object.keys(h.tiers.store.getState().rooms)).toEqual([h.rooms[1].room.key])
    // C's room was hidden: nothing to promote.
    expect(h.promoted).toEqual([])
  })

  it('a switch to the shown room’s server promotes it (a pending primary select), then disposes it', async () => {
    await h.homeRooms.onWindowHostChanged('local', 'b.test')
    expect(h.promoted).toEqual([row('anna', 'b.test')])
    expect(h.rooms.map((r) => r.disposed)).toEqual([0, 1])
    const s = h.homeRooms.store.getState()
    expect(s.shown).toBe(null)
    expect(Object.keys(s.entries)).toEqual(['appa::c.test'])
  })

  it('a room still resolving on the destination is dropped too (the build stops)', async () => {
    h.lists.set('e.test', [project('pe', 'eve', '/srv/eve')])
    const pending = h.homeRooms.open(row('eve', 'e.test'), 'e.test')
    expect(h.homeRooms.store.getState().entries['eve::e.test']?.phase).toBe('resolving')
    await h.homeRooms.onWindowHostChanged('local', 'e.test')
    await pending
    expect(h.homeRooms.store.getState().entries['eve::e.test']).toBe(undefined)
    expect(h.rooms.length).toBe(2)
    expect(h.promoted).toEqual([row('eve', 'e.test')])
  })
})

describe('installWindowHostSwitch compares Home host keys (Z29)', () => {
  const d: ConnectHost = {
    id: 'id-d', label: 'D', hostname: 'd.test', port: 443, secure: true,
    username: 'anna', token: '', remember: false, lastConnectedAt: null,
  } as ConnectHost

  beforeEach(() => {
    useConnectHostStore.setState({ hosts: [d], activeHost: 'local' } as never)
    clearHostSelect()
  })

  it('fires the rooms with (prev, next) Home keys; a session mint and an id-only re-key pass the same key', () => {
    const calls: Array<[string, string]> = []
    const off = installWindowHostSwitch({
      onWindowHostChanged: async (prev, next) => {
        calls.push([prev, next])
      },
    })
    try {
      useConnectHostStore.getState().selectHost(d)
      expect(calls).toEqual([['local', 'd.test']])
      // Same server, session minted (tokenless → token).
      useConnectHostStore.setState({ activeHost: { ...d, token: 'tok' } } as never)
      expect(calls).toEqual([
        ['local', 'd.test'],
        ['d.test', 'd.test'],
      ])
      // The same saved server re-added under a new client id.
      useConnectHostStore.setState({ activeHost: { ...d, id: 'id-d2', token: 'tok' } } as never)
      expect(calls).toEqual([
        ['local', 'd.test'],
        ['d.test', 'd.test'],
        ['d.test', 'd.test'],
      ])
    } finally {
      off()
    }
  })

  it('with the real manager: a mint on the window server keeps a room on that server’s alias; promotion carries the switcher id', async () => {
    const h = harness()
    h.lists.set('d.test', [project('pd', 'dora', '/srv/dora')])
    h.lists.set('b.test', [project('pb', 'anna', '/srv/anna')])
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    const off = installWindowHostSwitch(h.homeRooms)
    try {
      useConnectHostStore.getState().selectHost(d)
      useConnectHostStore.setState({ activeHost: { ...d, token: 'tok' } } as never)
      await new Promise((r) => setTimeout(r, 0))
      expect(h.rooms.map((r) => r.disposed)).toEqual([0])
      expect(h.homeRooms.store.getState().shown).toBe('anna::b.test')
    } finally {
      off()
    }
  })
})

describe('the production promote dep (Z29): requestHostSelect keyed by the switcher id', () => {
  it('names the server the window is on now; the projects restore takes it for that host only', () => {
    clearHostSelect()
    const b: ConnectHost = {
      id: 'id-b', label: 'B', hostname: 'b.test', port: 443, secure: true,
      username: 'anna', token: 'tok', remember: false, lastConnectedAt: null,
    } as ConnectHost
    useConnectHostStore.setState({ hosts: [b], activeHost: b } as never)
    promoteToPrimary(row('anna', 'b.test'))
    const pending = peekHostSelect()
    if (!pending) throw new Error('no pending select')
    expect(pending.hostId).toBe('id-b')
    expect(pending.row).toEqual(row('anna', 'b.test'))
    const list = [{ id: 'pb', handle: 'anna', name: 'anna' }]
    expect(takeHostSelect('local', list)).toBe(null)
    expect(takeHostSelect(b, list)?.workspace).toEqual(list[0])
    expect(peekHostSelect()).toBe(null)
  })
})
