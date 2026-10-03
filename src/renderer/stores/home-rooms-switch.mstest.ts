// prd-home-seamless-0432 item 2 (Z6, Z7, Z11, Z12; T2.3, T2.6) — a
// top-switcher change keeps Home's remote rooms, against REAL daemons.
// Run with `bun run test:multiserver`.
//
// Two daemons, three host keys: A is saved twice (`127.0.0.1:<A>` and
// `localhost:<A>`, the same daemon under two Home keys), B once. The window
// starts on A. B's room is open and shown on Home.
//   - T2.3: switching the window to A's alias keeps B's room exactly: same
//     room object, phase open, same generation, no new socket to B, its
//     workspace socket still open, B still lists `anna` on the workspace,
//     and nothing for B's room goes to A's alias.
//   - Z7 promotion: switching the window to B itself closes B's room and the
//     window's own room selects the same workspace once B's list lands.
//   - T2.6 (MS5): a third daemon C is the room's server and dies; the window
//     then switches. Every later request and socket for C's room targets
//     C's port, never the window's server.

import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const h = vi.hoisted(() => {
  const mem = new Map<string, string>()
  ;(globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => (mem.has(k) ? (mem.get(k) as string) : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
    key: (i: number) => Array.from(mem.keys())[i] ?? null,
    get length() {
      return mem.size
    },
  }
  return {
    requests: [] as Array<{ url: string; method: string }>,
    sockets: [] as Array<{ url: string; ws: WebSocket }>,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'available', port: Number(process.env.K2_MS_A_PORT), token: process.env.K2_MS_A_OWNER }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const realFetch = globalThis.fetch
globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
  h.requests.push({ url: String(input), method: (init?.method ?? 'GET').toUpperCase() })
  return realFetch(input, init)
}) as typeof fetch
const RealWebSocket = globalThis.WebSocket
globalThis.WebSocket = class extends RealWebSocket {
  constructor(url: string | URL, protocols?: string | string[]) {
    super(url, protocols)
    h.sockets.push({ url: String(url), ws: this })
  }
} as typeof WebSocket

import { loginToHost, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { openHomeRow } from '@/lib/home-open'
import { homeRooms } from '@/stores/home-rooms'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { workspaceHandle } from '@/lib/home-address'
import { seedMember, spawnDaemon, type Spawned } from '@/test-utils/two-daemons.global-setup'
import { primaryRoom, type PinnedRoom } from '@/stores/room'
import { usePresenceStore } from '@/stores/presence'
import { appBusCarrierCountForTests, appBusHandlerCount } from '@/stores/session-events'
import { serverViewCountForTests } from '@/stores/server-view'
import { roomTiers } from '@/lib/room-tiers'
import { hostPool } from '@/lib/host-pool-instance'
import type { HomeRow } from '@/stores/homes'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const B_PORT = Number(env('K2_MS_B_PORT'))
const A_OWNER = env('K2_MS_A_OWNER')
const B_OWNER = env('K2_MS_B_OWNER')
const B_USER = env('K2_MS_B_USER')
const B_PASSWORD = env('K2_MS_B_PASSWORD')
const B_KEY = `127.0.0.1:${B_PORT}`

const CHECKOUT_A = mkdtempSync(join(tmpdir(), 'k2-ms-switch-a-'))
const CHECKOUT_B = mkdtempSync(join(tmpdir(), 'k2-ms-switch-b-'))
const CHECKOUT_C = mkdtempSync(join(tmpdir(), 'k2-ms-switch-c-'))

async function daemon<T>(port: number, owner: string, route: string, init?: { body?: unknown }): Promise<T> {
  const q = new URLSearchParams({ token: owner })
  const res = await realFetch(`http://127.0.0.1:${port}/cli/${route}?${q}`, init?.body === undefined
    ? undefined
    : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(init.body) })
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on ${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function register(port: number, owner: string, path: string): Promise<ProjectWithWorkspaces> {
  await daemon(port, owner, 'projects/add-without-git', {
    body: { path, seedWiki: false, seedAgentsMd: false, fanout: false },
  })
  const list = await daemon<ProjectWithWorkspaces[]>(port, owner, 'projects/list')
  const p = list.find((x) => x.path === path)
  if (!p || !p.workspaces || p.workspaces.length === 0) throw new Error(`no workspace for ${path} on ${port}`)
  return p
}

function layoutJson(tabIds: string[]): string {
  return JSON.stringify({
    version: 2,
    tabs: tabIds.map((id) => ({
      id,
      title: id,
      mosaicTree: `pg-${id}`,
      paneGroups: { [`pg-${id}`]: { id: `pg-${id}`, items: [{ id: `item-${id}`, type: 'terminal', paneGroupId: `pg-${id}` }], activeItemIndex: 0 } },
      locked: false,
    })),
  })
}

interface RosterRow { user: string; workspaces?: string[]; windowCount?: number }

async function annaRowOnB(): Promise<RosterRow | null> {
  const snap = await daemon<{ roster: RosterRow[] }>(B_PORT, B_OWNER, 'presence/roster')
  return snap.roster.find((r) => r.user === B_USER) ?? null
}

function portOf(url: string): number {
  return Number(new URL(url).port)
}

function hostOf(url: string): string {
  return new URL(url).hostname
}

function liveSocketsTo(port: number): Array<{ url: string; ws: WebSocket }> {
  return h.sockets.filter(
    (s) => portOf(s.url) === port && s.ws.readyState !== RealWebSocket.CLOSED && s.ws.readyState !== RealWebSocket.CLOSING,
  )
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 15_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

async function openRow(row: HomeRow): Promise<PinnedRoom> {
  expect(openHomeRow(row)).toBe('room')
  await until(() => homeRooms.store.getState().entries[row.address]?.phase === 'open', 'the room to open')
  const opened = homeRooms.store.getState().entries[row.address]?.room
  if (!opened) throw new Error('the room opened without a room')
  await until(() => opened.tabs.getState().activeWorkspaceKey !== null, "the room's layout to load")
  return opened
}

/** Does this request or socket URL name the given project or path? */
function namesRoom(url: string, projectId: string, path: string): boolean {
  return url.includes(projectId) || url.includes(encodeURIComponent(path)) || url.includes(path)
}

let projB: ProjectWithWorkspaces
let rowB: HomeRow
let roomB: PinnedRoom
let aHost: ConnectHost
let aAlias: ConnectHost
let bHost: ConnectHost

beforeAll(async () => {
  await register(A_PORT, A_OWNER, CHECKOUT_A)
  projB = await register(B_PORT, B_OWNER, CHECKOUT_B)
  await daemon(B_PORT, B_OWNER, 'workspace-layouts/save', {
    body: { projectId: projB.id, workspaceId: projB.workspaces[0].id, layoutJson: layoutJson(['sw-b1', 'sw-b2']) },
  })

  // A saved twice (two Home keys, one daemon), signed in with A's owner
  // token; B signed in as the Member `anna`.
  aHost = {
    id: 'id-a', label: 'A', hostname: '127.0.0.1', port: A_PORT, secure: false,
    username: 'owner', token: A_OWNER, remember: false, lastConnectedAt: null,
  }
  aAlias = { ...aHost, id: 'id-a-alias', label: 'A alias', hostname: 'localhost' }
  bHost = {
    id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
    username: B_USER, token: '', remember: false, lastConnectedAt: null,
  }
  const login = await loginToHost(bHost, B_PASSWORD)
  if (!login.ok) throw new Error(`login as ${B_USER} on B failed: ${login.reason}`)
  bHost = { ...bHost, token: login.token }
  useConnectHostStore.setState({ hosts: [aHost, aAlias, bHost], activeHost: aHost } as never)
  useRemoteRoomsPreviewStore.setState({ enabled: true })

  const handle = workspaceHandle(projB)
  if (!handle) throw new Error("B's workspace has no handle")
  rowB = { address: `${handle}::${B_KEY}`, workspaceId: projB.id, label: projB.name }
  roomB = await openRow(rowB)
  await until(async () => ((await annaRowOnB())?.workspaces ?? []).includes(CHECKOUT_B), "B's roster to list anna")
}, 60_000)

afterAll(async () => {
  await homeRooms.closeAll()
  rmSync(CHECKOUT_A, { recursive: true, force: true })
  rmSync(CHECKOUT_B, { recursive: true, force: true })
  rmSync(CHECKOUT_C, { recursive: true, force: true })
})

describe('a top-switcher change keeps Home rooms on other servers (T2.3)', () => {
  it("switching the window to A's alias keeps B's room: same room, same generation, same sockets; nothing for it reaches the alias", async () => {
    const before = homeRooms.store.getState().entries[rowB.address]
    if (!before) throw new Error("B's room is not open")
    const socketsBefore = liveSocketsTo(B_PORT)
    const workspaceSocket = socketsBefore.find((s) => s.url.includes('/cli/sessions/events') && s.url.includes(encodeURIComponent(CHECKOUT_B)))
    if (!workspaceSocket) throw new Error(`no workspace socket on B; open: ${socketsBefore.map((s) => s.url).join(', ')}`)
    const allSocketsBefore = h.sockets.length
    const requestsBefore = h.requests.length
    const annaBefore = await annaRowOnB()
    // T2.4, "must not reset": counted before the switch.
    const keep = {
      handlers: appBusHandlerCount(roomB.scope),
      carriers: appBusCarrierCountForTests(roomB.scope),
      views: serverViewCountForTests(),
      tier: roomTiers.tier(roomB.key),
      poolEntry: hostPool.entry(B_KEY) !== undefined,
      tabs: roomB.tabs.getState().tabs.map((t) => t.id),
      workspaceKey: roomB.tabs.getState().activeWorkspaceKey,
      roomRoster: roomB.presence.getState().roster,
    }
    expect(keep.handlers).toBeGreaterThan(0)
    expect(keep.carriers).toBeGreaterThan(0)

    useConnectHostStore.getState().selectHost(aAlias)
    // T2.4, "still resets": the window's own stores are at their reset
    // state synchronously after the switch.
    expect(useProjectsStore.getState().projects).toEqual([])
    expect(useProjectsStore.getState().activeProjectId).toBe(null)
    expect(primaryRoom().tabs.getState().tabs).toEqual([])
    expect(primaryRoom().tabs.getState().activeWorkspaceKey).toBe(null)
    expect(usePresenceStore.getState().roster).toEqual([])
    // ... and nothing of B's room moved.
    expect({
      handlers: appBusHandlerCount(roomB.scope),
      carriers: appBusCarrierCountForTests(roomB.scope),
      views: serverViewCountForTests(),
      tier: roomTiers.tier(roomB.key),
      poolEntry: hostPool.entry(B_KEY) !== undefined,
      tabs: roomB.tabs.getState().tabs.map((t) => t.id),
      workspaceKey: roomB.tabs.getState().activeWorkspaceKey,
      roomRoster: roomB.presence.getState().roster,
    }).toEqual(keep)
    // The window's own stores reload from the alias; give the room time to
    // do anything it would wrongly do.
    await until(() => useProjectsStore.getState().projects.length > 0, "the window's projects from A's alias")
    await new Promise((r) => setTimeout(r, 1_500))

    const after = homeRooms.store.getState()
    expect(after.shown).toBe(rowB.address)
    const entry = after.entries[rowB.address]
    if (!entry) throw new Error("B's room was closed by the switch")
    expect(entry).toBe(before)
    expect(entry.phase).toBe('open')
    expect(entry.generation).toBe(before.generation)
    expect(entry.room).toBe(roomB)
    expect(roomB.scope.hostKey).toBe(B_KEY)
    expect(roomB.scope.isWindowHost()).toBe(false)
    expect(appBusHandlerCount(roomB.scope)).toBe(keep.handlers)
    expect(appBusCarrierCountForTests(roomB.scope)).toBe(keep.carriers)
    expect(serverViewCountForTests()).toBe(keep.views)
    expect(roomTiers.tier(roomB.key)).toBe('hot')
    expect(roomB.tabs.getState().tabs.map((t) => t.id)).toEqual(keep.tabs)

    // B saw no new socket upgrade from this window; its workspace socket
    // is the same one, still open.
    const newSockets = h.sockets.slice(allSocketsBefore)
    expect(newSockets.filter((s) => portOf(s.url) === B_PORT).map((s) => s.url)).toEqual([])
    expect(workspaceSocket.ws.readyState).toBe(RealWebSocket.OPEN)
    expect(liveSocketsTo(B_PORT).map((s) => s.url).sort()).toEqual(socketsBefore.map((s) => s.url).sort())
    // (Other suites share B and `anna`, so only this workspace is compared.)
    expect((annaBefore?.workspaces ?? []).includes(CHECKOUT_B)).toBe(true)
    expect(((await annaRowOnB())?.workspaces ?? []).includes(CHECKOUT_B)).toBe(true)

    // Nothing for B's room went to A (either spelling).
    const since = h.requests.slice(requestsBefore)
    const toA = since.filter((r) => portOf(r.url) === A_PORT)
    expect(toA.length).toBeGreaterThan(0)
    expect(toA.filter((r) => namesRoom(r.url, projB.id, CHECKOUT_B)).map((r) => r.url)).toEqual([])
    expect(newSockets.filter((s) => portOf(s.url) === A_PORT && namesRoom(s.url, projB.id, CHECKOUT_B)).map((s) => s.url)).toEqual([])
    // The window's own reload went to the alias, not the first spelling.
    expect(toA.filter((r) => hostOf(r.url) === 'localhost').length).toBeGreaterThan(0)
  })

  it("switching the window to B itself promotes the room: B's room closes and the window selects the same workspace (Z7)", async () => {
    const workspaceSocket = liveSocketsTo(B_PORT).find(
      (s) => s.url.includes('/cli/sessions/events') && s.url.includes(encodeURIComponent(CHECKOUT_B)),
    )
    if (!workspaceSocket) throw new Error("no workspace socket on B before the promotion")

    useConnectHostStore.getState().selectHost(bHost)
    expect(homeRooms.store.getState().entries[rowB.address]).toBe(undefined)
    expect(homeRooms.store.getState().shown).toBe(null)
    await until(() => useProjectsStore.getState().activeProjectId === projB.id, "the window's own room to select B's workspace")
    expect(useProjectsStore.getState().activeWorkspaceId).toBe(projB.workspaces[0].id)
    await until(() => workspaceSocket.ws.readyState === RealWebSocket.CLOSED, "the room's own workspace socket to close")
  })
})

describe('MS5 across a switch: a kept room whose server died never retries on the window server (T2.6)', () => {
  let c: Spawned
  let projC: ProjectWithWorkspaces
  let rowC: HomeRow

  beforeAll(async () => {
    // Back to A for the window.
    useConnectHostStore.getState().selectHost(aHost)
    c = await spawnDaemon('switch-c')
    await seedMember(c)
    projC = await register(c.port, c.owner, CHECKOUT_C)
    await daemon(c.port, c.owner, 'workspace-layouts/save', {
      body: { projectId: projC.id, workspaceId: projC.workspaces[0].id, layoutJson: layoutJson(['sw-c1']) },
    })
    let cHost: ConnectHost = {
      id: 'id-c', label: 'C', hostname: '127.0.0.1', port: c.port, secure: false,
      username: B_USER, token: '', remember: false, lastConnectedAt: null,
    }
    const login = await loginToHost(cHost, B_PASSWORD)
    if (!login.ok) throw new Error(`login on C failed: ${login.reason}`)
    cHost = { ...cHost, token: login.token }
    useConnectHostStore.setState((s) => ({ hosts: [...s.hosts, cHost] }))
    const handle = workspaceHandle(projC)
    if (!handle) throw new Error("C's workspace has no handle")
    rowC = { address: `${handle}::127.0.0.1:${c.port}`, workspaceId: projC.id, label: projC.name }
    await openRow(rowC)
  }, 60_000)

  afterAll(() => {
    c?.child.kill('SIGKILL')
    if (c) rmSync(c.home, { recursive: true, force: true })
  })

  it('C dies, the window switches to the alias: every later request and socket for the room targets C’s port or nothing', async () => {
    const roomC = homeRooms.store.getState().entries[rowC.address]?.room
    if (!roomC) throw new Error("C's room is not open")
    c.child.kill('SIGKILL')
    await until(
      () => liveSocketsTo(c.port).length === 0,
      "the room's sockets on C to drop",
    )
    const requestsBefore = h.requests.length
    const socketsBefore = h.sockets.length

    useConnectHostStore.getState().selectHost(aAlias)
    await new Promise((r) => setTimeout(r, 3_000))

    // Still the same room, still pinned to C.
    expect(homeRooms.store.getState().entries[rowC.address]?.room).toBe(roomC)
    expect(roomC.scope.hostKey).toBe(`127.0.0.1:${c.port}`)
    const requests = h.requests.slice(requestsBefore).filter((r) => namesRoom(r.url, projC.id, CHECKOUT_C))
    const sockets = h.sockets.slice(socketsBefore).filter((s) => namesRoom(s.url, projC.id, CHECKOUT_C))
    expect(requests.filter((r) => portOf(r.url) !== c.port).map((r) => r.url)).toEqual([])
    expect(sockets.filter((s) => portOf(s.url) !== c.port).map((s) => s.url)).toEqual([])
  })
})
