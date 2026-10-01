// Home M3 — two pinned rooms against two REAL daemons (MS3, MS42, MS52 a/g).
// Run with `bun run test:multiserver` (vitest.multiserver.config.ts spawns
// both daemons from this checkout's target/debug/k2-daemon).
//
// The same checkout path is registered on A and on B (each daemon mints
// its own project id; MS52 a with the SAME id is the unit test in
// tabs-room-instances.test.ts). Both start from the same saved layout. One
// room per server, each built by `createPinnedRoom` on a pinned scope:
//
//   - a tab added in B's room is saved to B: B's revision rises, A's stored
//     layout is byte-identical and A's revision does not move;
//   - every request and socket of B's room goes to B's port, every one of
//     A's room to A's port, nothing crosses;
//   - a tab close in B's room reaches B only;
//   - `dispose()` closes each room's sockets.

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
  return { requests: [] as string[], sockets: [] as string[] }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    // `local` (the window's primary scope) is daemon A, as MS74 sets up.
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

// Record every request and socket URL by port (no mock of the transport:
// these go to the real daemons).
const realFetch = globalThis.fetch
globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
  h.requests.push(String(input))
  return realFetch(input, init)
}) as typeof fetch
const RealWebSocket = globalThis.WebSocket
globalThis.WebSocket = class extends RealWebSocket {
  constructor(url: string | URL, protocols?: string | string[]) {
    h.sockets.push(String(url))
    super(url, protocols)
  }
} as typeof WebSocket

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { scopeForHost } from '@/kessel/server-scope'
import { createPinnedRoom, type Room } from '@/stores/room'
import { projectsStoreOf } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'

function env(name: string): string {
  const v = process.env[name]
  if (!v) throw new Error(`${name} is not set: run through vitest.multiserver.config.ts`)
  return v
}

const A_PORT = Number(env('K2_MS_A_PORT'))
const B_PORT = Number(env('K2_MS_B_PORT'))
const A_OWNER = env('K2_MS_A_OWNER')
const B_OWNER = env('K2_MS_B_OWNER')

// Both saved with their owner tokens (raw-token servers): this suite is
// about where requests GO, not about logins (that is host-pool.mstest).
const A: ConnectHost = {
  id: 'id-a', label: 'A', hostname: '127.0.0.1', port: A_PORT, secure: false,
  token: A_OWNER, remember: false, lastConnectedAt: null,
}
const B: ConnectHost = {
  id: 'id-b', label: 'B', hostname: '127.0.0.1', port: B_PORT, secure: false,
  token: B_OWNER, remember: false, lastConnectedAt: null,
}

const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-room-'))

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

function seedLayoutJson(pg: string): string {
  return JSON.stringify({
    version: 2,
    tabs: [{
      id: 'tab-main',
      title: 'Terminal 1',
      mosaicTree: pg,
      paneGroups: { [pg]: { id: pg, items: [{ id: 'item-main', type: 'terminal', paneGroupId: pg }], activeItemIndex: 0 } },
      locked: false,
    }],
  })
}

async function stored(port: number, owner: string, p: ProjectWithWorkspaces): Promise<{ layoutJson: string; revision: number }> {
  return daemon(port, owner, 'workspace-layouts/load', {
    query: { project_id: p.id, workspace_id: p.workspaces[0].id, with_revision: '1' },
  })
}

function portOf(url: string): number {
  return Number(new URL(url).port)
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

let projA: ProjectWithWorkspaces
let projB: ProjectWithWorkspaces
let roomA: Room
let roomB: Room
/** Every HTTP request made while the two rooms opened. */
let openRequests: string[] = []

beforeAll(async () => {
  useConnectHostStore.setState({ hosts: [A, B] })
  projA = await register(A_PORT, A_OWNER)
  projB = await register(B_PORT, B_OWNER)
  for (const [port, owner, p] of [[A_PORT, A_OWNER, projA], [B_PORT, B_OWNER, projB]] as const) {
    await daemon(port, owner, 'workspace-layouts/save', {
      body: { projectId: p.id, workspaceId: p.workspaces[0].id, layoutJson: seedLayoutJson('pg-main') },
    })
  }
  const make = (host: ConnectHost, p: ProjectWithWorkspaces): Room =>
    createPinnedRoom({
      scope: scopeForHost(host),
      workspace: { projectId: p.id, workspaceId: p.workspaces[0].id, path: CHECKOUT },
      projects: projectsStoreOf([p]),
      activateProject: () => {},
    })
  roomA = make(A, projA)
  roomB = make(B, projB)
  h.requests = []
  h.sockets = []
  await roomA.tabs.room.open()
  await roomB.tabs.room.open()
  // Let the background reconcile / tab-title reads land.
  await new Promise((r) => setTimeout(r, 300))
  openRequests = [...h.requests]
}, 60_000)

afterAll(async () => {
  await roomA?.tabs.room.dispose()
  await roomB?.tabs.room.dispose()
  rmSync(CHECKOUT, { recursive: true, force: true })
})

describe('two pinned rooms on two real daemons, same checkout path (Home M3)', () => {
  it('each room loads its own server\'s layout', () => {
    expect(projA.path).toBe(projB.path)
    expect(roomA.tabs.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    expect(roomB.tabs.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    expect(roomA.tabs.getState().activeWorkspaceKey).toBe(`${projA.id}:${projA.workspaces[0].id}`)
    expect(roomB.tabs.getState().activeWorkspaceKey).toBe(`${projB.id}:${projB.workspaces[0].id}`)
  })

  it("a tab added in B's room saves to B; A's layout is byte-identical", async () => {
    const aBefore = await stored(A_PORT, A_OWNER, projA)
    const bBefore = await stored(B_PORT, B_OWNER, projB)

    roomB.tabs.getState().addTab(CHECKOUT)
    roomB.tabs.getState().flushLayoutPersist()
    await until(async () => (await stored(B_PORT, B_OWNER, projB)).revision > bBefore.revision, "B's save")

    const bAfter = await stored(B_PORT, B_OWNER, projB)
    expect((JSON.parse(bAfter.layoutJson) as { tabs: unknown[] }).tabs.length).toBe(2)
    const aAfter = await stored(A_PORT, A_OWNER, projA)
    expect(aAfter.layoutJson).toBe(aBefore.layoutJson)
    expect(aAfter.revision).toBe(aBefore.revision)
    expect(roomA.tabs.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    // The two instances keep separate revisions.
    const keyB = roomB.tabs.getState().activeWorkspaceKey as string
    const keyA = roomA.tabs.getState().activeWorkspaceKey as string
    expect(roomB.tabs.room.__test.layoutRevisions.get(keyB)).toBe(bAfter.revision)
    expect(roomA.tabs.room.__test.layoutRevisions.get(keyA)).toBe(aBefore.revision)
  })

  it("a tab close in B's room reaches B only", async () => {
    const added = roomB.tabs.getState().tabs.find((t) => t.id !== 'tab-main')
    if (!added) throw new Error('the added tab is gone')
    h.requests = []
    roomB.tabs.getState().removeTab(added.id)
    await until(() => h.requests.some((u) => /\/cli\/(sessions\/v2\/close|terminal\/kill)/.test(u)), 'the close request')
    const closes = h.requests.filter((u) => /\/cli\/(sessions\/v2\/close|terminal\/kill)/.test(u))
    expect(closes.map(portOf)).toEqual(closes.map(() => B_PORT))
  })

  it("every request and socket of each room went to that room's own server", () => {
    // A request that names B's project went to B, and one that names A's
    // project went to A. (The window's primary room also talks to A, so
    // port alone cannot attribute A's traffic.)
    const namesB = openRequests.filter((u) => u.includes(projB.id))
    const namesA = openRequests.filter((u) => u.includes(projA.id))
    expect(namesB.length).toBeGreaterThan(0)
    expect(namesA.length).toBeGreaterThan(0)
    expect(namesB.map(portOf)).toEqual(namesB.map(() => B_PORT))
    expect(namesA.map(portOf)).toEqual(namesA.map(() => A_PORT))
    // Sockets: the rooms' workspace sockets.
    const daemonSockets = h.sockets.filter((u) => u.includes('/cli/'))
    const toA = daemonSockets.filter((u) => portOf(u) === A_PORT)
    const toB = daemonSockets.filter((u) => portOf(u) === B_PORT)
    expect(toA.length).toBeGreaterThan(0)
    expect(toB.length).toBeGreaterThan(0)
    // A's sockets carry A's token, B's carry B's (MS32).
    expect(toA.every((u) => new URL(u).searchParams.get('token') === A_OWNER)).toBe(true)
    expect(toB.every((u) => new URL(u).searchParams.get('token') === B_OWNER)).toBe(true)
  })
})
