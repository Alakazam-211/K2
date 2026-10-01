// Home M4 — the Home rooms manager (stores/home-rooms.ts): opening a remote
// row builds ONE view-only pinned room on its server, the tiers open and
// close its sockets (hot = mounted, warm = its workspace socket only, cold =
// disposed), re-showing a cold room rebuilds it, and the keep-alive follows.

import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { createHomeRooms, type HomeRooms } from '@/stores/home-rooms'
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
  const scopes = new Map<string, ServerScope>()
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
  return { tiers, homeRooms, rooms, keepAlives, lists, advance, intervals: () => timers.filter((t) => t.every !== null).length }
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

describe('Home rooms (M4)', () => {
  let h: ReturnType<typeof harness>
  beforeEach(() => {
    h = harness()
    h.lists.set('b.test', [project('pb', 'anna', '/srv/anna')])
    h.lists.set('c.test', [project('pc', 'appa', '/srv/appa')])
  })

  it('opens ONE view-only room on the row’s server, shows it hot, opens its socket and keeps it alive', async () => {
    const entry = await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    expect(entry.phase).toBe('open')
    expect(h.rooms.length).toBe(1)
    const fake = h.rooms[0]
    expect(fake.input.readOnly).toBe(true)
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

  it('a row whose agent is gone on that server is not-found', async () => {
    const entry = await h.homeRooms.open(row('ghost', 'b.test'), 'b.test')
    expect(entry.phase).toBe('not-found')
    expect(h.rooms.length).toBe(0)
  })

  it('closeAll (a server switch) disposes every room and shows the window’s own room', async () => {
    await h.homeRooms.open(row('anna', 'b.test'), 'b.test')
    await h.homeRooms.open(row('appa', 'c.test'), 'c.test')
    await h.homeRooms.closeAll()
    expect(h.rooms.map((r) => r.disposed)).toEqual([1, 1])
    expect(h.rooms.map((r) => r.sockets)).toEqual([0, 0])
    expect(h.homeRooms.store.getState()).toEqual({ entries: {}, shown: null })
    expect(Object.keys(h.tiers.store.getState().rooms)).toEqual([])
  })
})
