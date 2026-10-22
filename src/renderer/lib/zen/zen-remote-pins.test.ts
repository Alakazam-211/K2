// Garden 1's pin grid with agents on other servers (0.46.3 r9, a customer
// report: "Anna and Oaks will sometimes fall off the pinned chat"). A
// remote agent's pin (`projects.pinned` on its own server) used to be read
// only from its open room, so after a reload, re-entering Zen, a room going
// cold or a refresh it read "not pinned" until clicked. It now falls back
// to that server's cached `projects/list` (`zen-custom-scope`), refreshed
// on the preview poll. Fixtures are made up (the repo is public): servers
// under example.test.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ hostKey: string; route: string }>,
  /** `projects/list` per server; a missing server answers with an error. */
  lists: {} as Record<string, unknown>,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string) => {
    // The window server's pictures (the Agents page's avatar lookup).
    if (route === 'projects/get-icon') return { found: false, dataUrl: null }
    h.gets.push({ hostKey: scope.hostKey, route })
    if (route !== 'projects/list') throw new Error(`unexpected GET ${route}`)
    const list = h.lists[scope.hostKey]
    if (list === undefined) throw new Error(`${scope.hostKey} is unreachable`)
    return list
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
  withHostCliSlot: vi.fn(async (_s: unknown, fn: () => Promise<unknown>) => fn()),
}))

import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import type { HomeRoomEntry } from '@/stores/home-rooms'
import {
  __resetZenScopeForTests,
  requestZenServerRoster,
  rosterWorkspacesFrom,
  useZenServerRosters,
} from './zen-custom-scope'
import { __resetZenDataForTests, rowPinned, zenAgentRows, type ZenView } from './zen-data'

const BOX1 = 'box1.example.test'
const BOX2 = 'box2.example.test'
const ANNA: HomeRow = { address: `anna::${BOX1}`, workspaceId: 'wa', label: 'Anna' }
const OAKS: HomeRow = { address: `oaks::${BOX2}`, workspaceId: 'wo', label: 'Oaks' }
const PAT: HomeRow = { address: 'pat::local', workspaceId: 'wp', label: 'Pat' }
const HOME: Home = { id: 'home-1', name: 'Home', rows: [PAT, ANNA, OAKS] }

function saved(n: number): ConnectHost {
  return {
    id: `h${n}`,
    label: `Box ${n}`,
    hostname: `box${n}.example.test`,
    port: 443,
    secure: true,
    username: 'rosson',
    token: 'tok-test',
    remember: true,
    lastConnectedAt: null,
  } as ConnectHost
}

/** A signed-in, live server in the pool (a Home row on it reads `ok`). */
function live(hostKey: string): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.2', protocol: 1, instanceId: 'i1', features: [], at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: null,
  } as HostEntry
}

/** A server's `projects/list` answer: `pinned` is 0/1 as the daemon sends it. */
function list(...ws: Array<[id: string, handle: string, name: string, pinned: number]>): unknown[] {
  return ws.map(([id, handle, name, pinned]) => ({ id, handle, name, path: `/srv/${handle}`, pinned, workspaces: [] }))
}

/** An open room on another server whose project list is `projects`. */
function room(address: string, projects: unknown[]): HomeRoomEntry {
  return {
    address,
    hostKey: BOX1,
    label: 'Anna',
    phase: 'open',
    error: null,
    room: { projects: { getState: () => ({ projects }) } } as never,
    access: null,
    generation: 1,
  }
}

const VIEW: ZenView = { key: 'g-test0001/agents', gardenId: 'g-test0001', widgetId: 'agents', homeId: HOME.id, agent: null }

async function rosterReady(hostKey: string): Promise<void> {
  await vi.waitFor(() => expect(useZenServerRosters.getState().rosters[hostKey]?.status).toBe('ready'))
}

let prev: { homes: Home[]; hosts: ConnectHost[]; projects: unknown[]; active: unknown }

beforeEach(() => {
  prev = {
    homes: useHomesStore.getState().homes,
    hosts: useConnectHostStore.getState().hosts,
    projects: useProjectsStore.getState().projects,
    active: useConnectHostStore.getState().activeHost,
  }
  useHomesStore.setState({ homes: [HOME], selectedId: HOME.id })
  useConnectHostStore.setState({ hosts: [saved(1), saved(2)], activeHost: 'local', connectionStatus: 'connected' } as never)
  useProjectsStore.setState({ projects: list(['wp', 'pat', 'Pat', 1]) as never })
  hostPool.store.setState({ entries: { [BOX1]: live(BOX1), [BOX2]: live(BOX2) } })
  h.gets = []
  h.lists = {
    [BOX1]: list(['wa', 'anna', 'Anna', 1], ['wx', 'xavi', 'Xavi', 0]),
    [BOX2]: list(['wo', 'oaks', 'Oaks', 1]),
  }
  __resetZenScopeForTests()
  __resetZenDataForTests()
})

afterEach(() => {
  __resetZenDataForTests()
  __resetZenScopeForTests()
  hostPool.store.setState({ entries: {} })
  useHomesStore.setState({ homes: prev.homes })
  useConnectHostStore.setState({ hosts: prev.hosts, activeHost: prev.active as 'local' })
  useProjectsStore.setState({ projects: prev.projects as never })
})

describe('rosterWorkspacesFrom: pins from a server’s projects/list', () => {
  it('reads 0/1 and booleans; skips rows without an id or name; throws on a non-list', () => {
    expect(
      rosterWorkspacesFrom([
        { id: 'a', name: 'A', handle: 'a', pinned: 1 },
        { id: 'b', name: 'B', pinned: 0 },
        { id: 'c', name: 'C', handle: 'c', pinned: true },
        { id: 'd', name: 'D' },
        { name: 'no id', pinned: 1 },
      ]),
    ).toEqual([
      { id: 'a', name: 'A', handle: 'a', pinned: true },
      { id: 'b', name: 'B', handle: null, pinned: false },
      { id: 'c', name: 'C', handle: 'c', pinned: true },
      { id: 'd', name: 'D', handle: null, pinned: false },
    ])
    expect(() => rosterWorkspacesFrom({ nope: true })).toThrow(/not a list/)
  })
})

describe('rowPinned: an agent on another server', () => {
  it('reads pinned with no room open, once its server’s list is cached', async () => {
    expect(rowPinned(ANNA, false, undefined)).toBe(false)
    requestZenServerRoster(BOX1)
    await rosterReady(BOX1)
    expect(rowPinned(ANNA, false, undefined)).toBe(true)
    // An agent unpinned there reads unpinned.
    expect(rowPinned({ address: `xavi::${BOX1}`, workspaceId: 'wx', label: 'Xavi' }, false, undefined)).toBe(false)
    // Another server's cache never answers for this one.
    expect(rowPinned(OAKS, false, undefined)).toBe(false)
  })

  it('survives a cache refresh: while it is in flight, after it lands, and after one that fails', async () => {
    requestZenServerRoster(BOX1)
    await rosterReady(BOX1)
    const asked = h.gets.length
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    await Promise.resolve()
    expect(useZenServerRosters.getState().rosters[BOX1]?.status).toBe('loading')
    expect(rowPinned(ANNA, false, undefined)).toBe(true)
    await rosterReady(BOX1)
    expect(h.gets.length).toBe(asked + 1)
    expect(rowPinned(ANNA, false, undefined)).toBe(true)
    // The server stops answering: the last list stands.
    delete h.lists[BOX1]
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    await vi.waitFor(() => expect(useZenServerRosters.getState().rosters[BOX1]?.status).toBe('failed'))
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
    expect(rowPinned(ANNA, false, undefined)).toBe(true)
  })

  it('a ready list is not re-read without `refresh`; a refresh is not doubled while one is in flight', async () => {
    requestZenServerRoster(BOX1)
    await rosterReady(BOX1)
    const asked = h.gets.length
    requestZenServerRoster(BOX1)
    await Promise.resolve()
    expect(h.gets.length).toBe(asked)
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    await rosterReady(BOX1)
    expect(h.gets.length).toBe(asked + 1)
  })

  it('an unpin on its server removes it on the next refresh', async () => {
    requestZenServerRoster(BOX1)
    await rosterReady(BOX1)
    expect(rowPinned(ANNA, false, undefined)).toBe(true)
    h.lists[BOX1] = list(['wa', 'anna', 'Anna', 0])
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    await vi.waitFor(() => expect(rowPinned(ANNA, false, undefined)).toBe(false))
    // And a pin there shows on the next one.
    h.lists[BOX1] = list(['wa', 'anna', 'Anna', 1])
    requestZenServerRoster(BOX1, Date.now(), { refresh: true })
    await vi.waitFor(() => expect(rowPinned(ANNA, false, undefined)).toBe(true))
  })

  it('an open room’s live list wins; a room whose list has not landed falls back to the cache', async () => {
    requestZenServerRoster(BOX1)
    await rosterReady(BOX1)
    // Unpinned in the room just now (the cache still says pinned).
    expect(rowPinned(ANNA, false, room(ANNA.address, list(['wa', 'anna', 'Anna', 0])))).toBe(false)
    // A room still reading its list: no blink.
    expect(rowPinned(ANNA, false, room(ANNA.address, []))).toBe(true)
    // No cache, room pinned: pinned (today's behaviour).
    __resetZenScopeForTests()
    expect(rowPinned(ANNA, false, room(ANNA.address, list(['wa', 'anna', 'Anna', 1])))).toBe(true)
  })
})

describe('rowPinned: an agent on the window’s server is unchanged', () => {
  it('reads the projects store only, never a cached list', async () => {
    expect(rowPinned(PAT, true, undefined)).toBe(true)
    // A stale cached list for this computer says otherwise: ignored.
    h.lists.local = list(['wp', 'pat', 'Pat', 0])
    requestZenServerRoster('local')
    await rosterReady('local')
    expect(rowPinned(PAT, true, undefined)).toBe(true)
    useProjectsStore.setState({ projects: list(['wp', 'pat', 'Pat', 0]) as never })
    h.lists.local = list(['wp', 'pat', 'Pat', 1])
    requestZenServerRoster('local', Date.now(), { refresh: true })
    await rosterReady('local')
    expect(rowPinned(PAT, true, undefined)).toBe(false)
  })
})

describe('zenAgentRows: Garden 1’s rows carry remote pins', () => {
  it('a live remote row asks its server’s list once, then reads pinned with no room', async () => {
    const first = zenAgentRows(VIEW)
    expect(first.map((r) => [r.address, r.state])).toEqual([
      [PAT.address, 'ok'],
      [ANNA.address, 'ok'],
      [OAKS.address, 'ok'],
    ])
    expect(first.find((r) => r.address === PAT.address)?.pinned).toBe(true)
    await rosterReady(BOX1)
    await rosterReady(BOX2)
    const rows = zenAgentRows(VIEW)
    expect(rows.map((r) => [r.address, r.pinned])).toEqual([
      [PAT.address, true],
      [ANNA.address, true],
      [OAKS.address, true],
    ])
    // One list per server, none for the window's own server.
    expect(h.gets.filter((g) => g.route === 'projects/list').map((g) => g.hostKey).sort()).toEqual([BOX1, BOX2])
    // Reading the rows again asks nothing more.
    zenAgentRows(VIEW)
    await Promise.resolve()
    expect(h.gets.length).toBe(2)
  })

  it('a remote server that is signed out asks nothing', async () => {
    hostPool.store.setState({ entries: { [BOX1]: { ...live(BOX1), auth: 'signin-required' }, [BOX2]: live(BOX2) } })
    zenAgentRows(VIEW)
    await rosterReady(BOX2)
    expect(h.gets.map((g) => g.hostKey)).toEqual([BOX2])
  })
})
