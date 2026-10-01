// Home M4 — a remote room on Home against two REAL daemons
// (prd-home-multi-server-client MS39, MS40, MS46, MS52 b/c/j, MS55; R7).
// Run with `bun run test:multiserver`.
//
// The window is on A (`local` = daemon A). B is a saved server, signed in as
// the Member `anna`. "Remote rooms (preview)" is on. Opening B's Home row:
//   - opens B's room without switching the window, view-only;
//   - shows B's tabs from B's layout;
//   - B's who's-here lists `anna` on that workspace (R7), and the room's own
//     roster comes from B;
//   - B's Active set holds the workspace (projects/activate on B);
//   - sends nothing to A, and writes no layout to B (not even after a local
//     tab change);
//   - one workspace socket on B (MS46); off screen it goes warm (socket
//     kept), then cold (socket closed, B drops `anna`), and opening the row
//     again reopens it.

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
    // `local` (the window's server) is daemon A (MS74).
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
import { DEFAULT_ROOM_TIER_CONFIG, roomTiers } from '@/lib/room-tiers'
import { workspaceHandle } from '@/lib/home-address'
import type { PinnedRoom } from '@/stores/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
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
const B_KEY = `127.0.0.1:${B_PORT}`

// The same checkout path on both servers (MS3): only the server tells
// them apart.
const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-home-room-'))

async function daemon<T>(port: number, owner: string, route: string, init?: { body?: unknown; query?: Record<string, string> }): Promise<T> {
  const q = new URLSearchParams({ ...(init?.query ?? {}), token: owner })
  const res = await realFetch(`http://127.0.0.1:${port}/cli/${route}?${q}`, init?.body === undefined
    ? undefined
    : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(init.body) })
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on ${port}: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function register(port: number, owner: string): Promise<ProjectWithWorkspaces> {
  await daemon(port, owner, 'projects/add-without-git', {
    body: { path: CHECKOUT, seedWiki: false, seedAgentsMd: false, fanout: false },
  })
  const list = await daemon<ProjectWithWorkspaces[]>(port, owner, 'projects/list')
  const p = list.find((x) => x.path === CHECKOUT)
  if (!p) throw new Error(`project for ${CHECKOUT} missing on ${port}`)
  if (!p.workspaces || p.workspaces.length === 0) throw new Error(`no workspace for ${CHECKOUT} on ${port}`)
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

async function stored(port: number, owner: string, p: ProjectWithWorkspaces): Promise<{ layoutJson: string; revision: number }> {
  return daemon(port, owner, 'workspace-layouts/load', {
    query: { project_id: p.id, workspace_id: p.workspaces[0].id, with_revision: '1' },
  })
}

interface RosterRow { user: string; workspaces?: string[] }

async function rosterOnB(): Promise<RosterRow[]> {
  const snap = await daemon<{ roster: RosterRow[] }>(B_PORT, B_OWNER, 'presence/roster')
  return snap.roster
}

function annaHere(roster: RosterRow[]): boolean {
  return roster.some((r) => r.user === B_USER && (r.workspaces ?? []).includes(CHECKOUT))
}

async function activeIds(port: number, owner: string): Promise<string[]> {
  return (await daemon<{ projectIds: string[] }>(port, owner, 'projects/active')).projectIds
}

function portOf(url: string): number {
  return Number(new URL(url).port)
}

function openSocketsTo(port: number): string[] {
  return h.sockets
    .filter((s) => portOf(s.url) === port && s.ws.readyState !== RealWebSocket.CLOSED && s.ws.readyState !== RealWebSocket.CLOSING)
    .map((s) => s.url)
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

/** Resolve once no request or socket has started for `quietMs`. */
async function untilQuiet(quietMs: number, maxMs = 15_000): Promise<void> {
  const deadline = Date.now() + maxMs
  let seen = h.requests.length + h.sockets.length
  let quietSince = Date.now()
  for (;;) {
    await new Promise((r) => setTimeout(r, 100))
    const now = h.requests.length + h.sockets.length
    if (now !== seen) {
      seen = now
      quietSince = Date.now()
    } else if (Date.now() - quietSince >= quietMs) {
      return
    }
    if (Date.now() > deadline) throw new Error('the window never went quiet before the open')
  }
}

let projA: ProjectWithWorkspaces
let projB: ProjectWithWorkspaces
let row: HomeRow
let room: PinnedRoom
let bRevisionAtOpen: number
/** Requests and sockets from the moment the row was opened. */
let sinceOpen: { requests: Array<{ url: string; method: string }>; sockets: string[] }

async function openRow(): Promise<PinnedRoom> {
  expect(openHomeRow(row)).toBe('room')
  await until(() => homeRooms.store.getState().entries[row.address]?.phase === 'open', 'the room to open')
  const opened = homeRooms.store.getState().entries[row.address]?.room
  if (!opened) throw new Error('the room opened without a room')
  await until(() => opened.tabs.getState().activeWorkspaceKey !== null, "B's layout to load")
  return opened
}

beforeAll(async () => {
  projA = await register(A_PORT, A_OWNER)
  projB = await register(B_PORT, B_OWNER)
  await daemon(A_PORT, A_OWNER, 'workspace-layouts/save', {
    body: { projectId: projA.id, workspaceId: projA.workspaces[0].id, layoutJson: layoutJson(['tab-a']) },
  })
  await daemon(B_PORT, B_OWNER, 'workspace-layouts/save', {
    body: { projectId: projB.id, workspaceId: projB.workspaces[0].id, layoutJson: layoutJson(['tab-b1', 'tab-b2']) },
  })
  await daemon(A_PORT, A_OWNER, 'projects/dismiss', { body: { projectId: projA.id } })
  await daemon(B_PORT, B_OWNER, 'projects/dismiss', { body: { projectId: projB.id } })

  // B is a saved server, signed in as the Member `anna`.
  const bHost: ConnectHost = {
    id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
    username: B_USER, token: '', remember: false, lastConnectedAt: null,
  }
  const login = await loginToHost(bHost, env('K2_MS_B_PASSWORD'))
  if (!login.ok) throw new Error(`login as ${B_USER} on B failed: ${login.reason}`)
  useConnectHostStore.setState({ hosts: [{ ...bHost, token: login.token }], activeHost: 'local' } as never)
  useRemoteRoomsPreviewStore.setState({ enabled: true })

  const handle = workspaceHandle(projB)
  if (!handle) throw new Error("B's workspace has no handle")
  row = { address: `${handle}::${B_KEY}`, workspaceId: projB.id, label: projB.name }
  bRevisionAtOpen = (await stored(B_PORT, B_OWNER, projB)).revision

  // The window's own room on A boots on module load (projects, its layout,
  // its debounced save). Let that traffic go quiet first, so everything
  // after this point is the B room's.
  await untilQuiet(1_500)
  h.requests = []
  const socketsBefore = h.sockets.length
  room = await openRow()
  await new Promise((r) => setTimeout(r, 300))
  sinceOpen = { requests: [...h.requests], sockets: h.sockets.slice(socketsBefore).map((s) => s.url) }
}, 60_000)

afterAll(async () => {
  roomTiers.reconfigure({ ...DEFAULT_ROOM_TIER_CONFIG })
  await homeRooms.closeAll()
  rmSync(CHECKOUT, { recursive: true, force: true })
})

describe("B's room on Home while the window is on A (Home M4)", () => {
  it('opens view-only on B without switching the window', () => {
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(room.readOnly).toBe(true)
    expect(room.scope.viewOnly).toBe(true)
    expect(room.scope.hostKey).toBe(B_KEY)
    expect(homeRooms.store.getState().shown).toBe(row.address)
  })

  it("shows B's tabs from B's layout", () => {
    expect(room.tabs.getState().tabs.map((t) => t.id)).toEqual(['tab-b1', 'tab-b2'])
    expect(room.tabs.getState().activeWorkspaceKey).toBe(`${projB.id}:${projB.workspaces[0].id}`)
  })

  it("B's who's-here lists the room's user on that workspace, and the room shows B's people (R7)", async () => {
    await until(async () => annaHere(await rosterOnB()), "B's roster to list anna")
    await until(() => annaHere(room.presence.getState().roster as RosterRow[]), "the room's roster to show anna")
  })

  it("B's Active set includes the workspace; A's does not move", async () => {
    await until(async () => (await activeIds(B_PORT, B_OWNER)).includes(projB.id), "B's Active set")
    expect(await activeIds(A_PORT, A_OWNER)).not.toContain(projA.id)
    const activates = sinceOpen.requests.filter((r) => r.url.includes('/cli/projects/activate'))
    expect(activates.length).toBeGreaterThan(0)
    expect(activates.map((r) => portOf(r.url))).toEqual(activates.map(() => B_PORT))
  })

  it('sends nothing to A for B’s room (no request, no socket), and B’s calls carry B’s login', () => {
    expect(sinceOpen.requests.filter((r) => portOf(r.url) === A_PORT)).toEqual([])
    expect(sinceOpen.sockets.filter((u) => portOf(u) === A_PORT)).toEqual([])
    const toB = sinceOpen.requests.filter((r) => portOf(r.url) === B_PORT)
    expect(toB.length).toBeGreaterThan(0)
    const tokens = new Set(toB.map((r) => new URL(r.url).searchParams.get('token')))
    expect(tokens.has(A_OWNER)).toBe(false)
    expect(tokens.has(B_OWNER)).toBe(false)
  })

  it('writes no layout to B, even after a tab change in the room', async () => {
    expect(sinceOpen.requests.filter((r) => r.url.includes('/cli/workspace-layouts/save'))).toEqual([])
    h.requests = []
    room.tabs.getState().addTab(CHECKOUT)
    room.tabs.getState().flushLayoutPersist()
    await new Promise((r) => setTimeout(r, 1_500))
    expect(h.requests.filter((r) => r.method === 'POST' && portOf(r.url) === B_PORT && !r.url.includes('/cli/projects/activate'))).toEqual([])
    const after = await stored(B_PORT, B_OWNER, projB)
    expect(after.revision).toBe(bRevisionAtOpen)
    expect((JSON.parse(after.layoutJson) as { tabs: Array<{ id: string }> }).tabs.map((t) => t.id)).toEqual(['tab-b1', 'tab-b2'])
  })

  it('holds ONE workspace socket on B (MS46)', () => {
    expect(openSocketsTo(B_PORT).length).toBe(1)
    expect(openSocketsTo(B_PORT)[0]).toContain(`/cli/sessions/events?path=${encodeURIComponent(CHECKOUT)}`)
  })

  it('tiers: off screen → warm keeps the socket; cold closes it and B drops the user; opening again reopens it', async () => {
    homeRooms.showPrimary()
    // Warm right away, cold 1.5 s later (the real clock, scaled down).
    roomTiers.reconfigure({ hotGraceMs: 0, coldAfterMs: 1_500 })
    expect(roomTiers.tier(room.key)).toBe('warm')
    expect(openSocketsTo(B_PORT).length).toBe(1)
    expect(annaHere(await rosterOnB())).toBe(true)

    await until(() => homeRooms.store.getState().entries[row.address] === undefined, 'the room to go cold')
    await until(() => openSocketsTo(B_PORT).length === 0, "the room's socket on B to close")
    await until(async () => !annaHere(await rosterOnB()), 'B to drop anna from that workspace')

    roomTiers.reconfigure({ ...DEFAULT_ROOM_TIER_CONFIG })
    const again = await openRow()
    expect(again).not.toBe(room)
    expect(again.tabs.getState().tabs.map((t) => t.id)).toEqual(['tab-b1', 'tab-b2'])
    await until(() => openSocketsTo(B_PORT).length === 1, "the room's socket on B to reopen")
    await until(async () => annaHere(await rosterOnB()), 'B to list anna again')
    room = again
  })
})
