// Home M5 — a remote room you can USE, against two REAL daemons
// (prd-home-multi-server-client MS38 writes, MS42, MS43, MS52 i/k; plan M5;
// split view D1–D4). Run with `bun run test:multiserver`.
//
// The window is on A (`local` = daemon A). B is a saved server, signed in as
// the Member `anna`. "Remote rooms (preview)" is on. A and B hold the SAME
// checkout path (MS3). Opening B's Home row gives a usable room on B:
//   - typing into a terminal in B's room reaches B's PTY (claimer mode, as a
//     Member) — the session was really spawned on B, not attached;
//   - opening, closing and splitting a tab saves B's layout with the
//     revision check, B's own layout shows it, and the close ends the
//     session on B (`v2/close`, reason tab_close);
//   - A's layout stays byte-identical and no request or socket goes to A;
//   - a save from a stale revision gets B's 409 and is merged, not lost;
//   - a route that is not on the room allowlist is refused before any
//     request, and this computer's commands stay off in B's room;
//   - an older B (no `with_revision`) keeps the room view only: no save.

import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
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
    requests: [] as Array<{ url: string; method: string; body: string | null; status: number | null }>,
    sockets: [] as Array<{ url: string; ws: WebSocket }>,
    /** Simulate an older B: strip `with_revision` from B's layout reads. */
    oldB: { port: 0, on: false },
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
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  let url = String(input)
  if (h.oldB.on && url.includes(`:${h.oldB.port}/cli/workspace-layouts/load`)) {
    const u = new URL(url)
    u.searchParams.delete('with_revision')
    url = u.toString()
  }
  const rec = {
    url,
    method: (init?.method ?? 'GET').toUpperCase(),
    body: typeof init?.body === 'string' ? init.body : null,
    status: null as number | null,
  }
  h.requests.push(rec)
  const res = await realFetch(url, init)
  rec.status = res.status
  return res
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
import { workspaceHandle } from '@/lib/home-address'
import { getDaemonWs, daemonHttpBase, daemonWsBase } from '@/kessel/daemon-ws'
import { openQueuedGridWebSocket } from '@/lib/grid-dial-queue'
import { daemonCliPost } from '@/lib/daemon-cli'
import { RoomWriteRefusedError, scopeMayWrite } from '@/kessel/server-scope'
import { LOCAL_ONLY_ACTIONS, roomMayRun } from '@/lib/local-only-actions'
import { paneRoomMode, type PinnedRoom } from '@/stores/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { HomeRow } from '@/stores/homes'
import type { TerminalItemData } from '@/stores/tabs'

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
h.oldB.port = B_PORT

const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-home-use-'))

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

interface StoredTab { id: string; isSystemAgent?: boolean }
interface StoredLayout { tabs: StoredTab[]; splitCount?: number; extraGroups?: Array<{ tabs: StoredTab[] }> }

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

/** User tab ids of a stored layout, column 0 then the extra columns. */
function storedUserTabs(json: string): string[][] {
  const l = JSON.parse(json) as StoredLayout
  const cols = [l.tabs, ...(l.extraGroups ?? []).map((g) => g.tabs)]
  return cols.map((c) => c.filter((t) => !t.isSystemAgent).map((t) => t.id))
}

interface SessionRow { agentName: string; cwd: string }

async function sessionsOn(port: number, owner: string): Promise<SessionRow[]> {
  return daemon<SessionRow[]>(port, owner, 'sessions/list-for-workspace', { query: { path: CHECKOUT } })
}

function userTabs(r: PinnedRoom): string[] {
  return r.tabs.getState().tabs.filter((t) => !t.isSystemAgent).map((t) => t.id)
}

function portOf(url: string): number {
  return Number(new URL(url).port)
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 15_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

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
let aAtOpen: { layoutJson: string; revision: number }
let requestsFrom = 0
let socketsFrom = 0
let key = ''

async function openRow(): Promise<PinnedRoom> {
  expect(openHomeRow(row)).toBe('room')
  await until(() => homeRooms.store.getState().entries[row.address]?.phase === 'open', 'the room to open')
  const opened = homeRooms.store.getState().entries[row.address]?.room
  if (!opened) throw new Error('the room opened without a room')
  await until(() => opened.tabs.getState().activeWorkspaceKey !== null, "B's layout to load")
  return opened
}

/** Wait until B's stored layout satisfies `pred`, and return it. */
async function untilStoredOnB(pred: (cols: string[][], rev: number) => boolean, what: string): Promise<{ cols: string[][]; revision: number }> {
  let last: { cols: string[][]; revision: number } = { cols: [], revision: -1 }
  await until(async () => {
    const s = await stored(B_PORT, B_OWNER, projB)
    last = { cols: storedUserTabs(s.layoutJson), revision: s.revision }
    return pred(last.cols, last.revision)
  }, what)
  return last
}

function sinceOpen(): Array<{ url: string; method: string; body: string | null; status: number | null }> {
  return h.requests.slice(requestsFrom)
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
  key = `${projB.id}:${projB.workspaces[0].id}`

  await untilQuiet(1_500)
  aAtOpen = await stored(A_PORT, A_OWNER, projA)
  requestsFrom = h.requests.length
  socketsFrom = h.sockets.length
  room = await openRow()
}, 60_000)

afterAll(async () => {
  h.oldB.on = false
  await homeRooms.closeAll()
  rmSync(CHECKOUT, { recursive: true, force: true })
})

describe("B's room is usable on Home while the window is on A (Home M5)", () => {
  it('opens usable on B: the room allowlist scope, claimer terminals, no switch', () => {
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    const entry = homeRooms.store.getState().entries[row.address]
    if (!entry) throw new Error('no entry for the row')
    expect(entry.access).toBe('use')
    expect(room.readOnly).toBe(false)
    expect(room.scope.remoteRoom).toBe(true)
    expect(room.scope.viewOnly).toBe(undefined)
    expect(room.scope.hostKey).toBe(B_KEY)
    expect(paneRoomMode(room)).toBe('claimer')
    expect(userTabs(room)).toEqual(['tab-b1', 'tab-b2'])
  })

  let typedTabId = ''
  let typedAgent = ''

  it('opening a tab saves B’s layout with the revision check, and B’s own layout shows it', async () => {
    const before = await stored(B_PORT, B_OWNER, projB)
    room.tabs.getState().addTab(CHECKOUT)
    room.tabs.getState().flushLayoutPersist()
    const tabs = userTabs(room)
    expect(tabs.length).toBe(3)
    typedTabId = tabs[2]
    const after = await untilStoredOnB((cols) => cols[0].includes(typedTabId), 'the new tab in B’s layout')
    expect(after.cols[0]).toEqual(['tab-b1', 'tab-b2', typedTabId])
    expect(after.revision).toBeGreaterThan(before.revision)
    const saves = sinceOpen().filter((r) => r.url.includes('/cli/workspace-layouts/save'))
    expect(saves.length).toBeGreaterThan(0)
    for (const s of saves) {
      expect(portOf(s.url)).toBe(B_PORT)
      if (s.body === null) throw new Error('a layout save without a body')
      expect(typeof (JSON.parse(s.body) as { baseRevision?: unknown }).baseRevision).toBe('number')
    }
  })

  it('typing into B’s terminal reaches B: a real spawn on B (not attach-only), claimer input as the Member', async () => {
    const tab = room.tabs.getState().tabs.find((t) => t.id === typedTabId)
    if (!tab) throw new Error('the new tab is gone')
    const item = [...tab.paneGroups.values()][0].items[0]
    const terminalId = (item.data as TerminalItemData).terminalId
    typedAgent = `tab-${terminalId}`
    // The pane's spawn, as TerminalPane sends it: on the room's scope,
    // through the room allowlist, no attach_only in a usable room.
    expect(scopeMayWrite(room.scope, 'sessions/v2/spawn')).toBe(true)
    const creds = await getDaemonWs(room.scope)
    expect(creds.port).toBe(B_PORT)
    const res = await fetch(`${daemonHttpBase(creds)}/cli/sessions/v2/spawn?token=${encodeURIComponent(creds.token)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ agent_name: typedAgent, cwd: CHECKOUT, command: 'sh', args: [], cols: 80, rows: 24 }),
    })
    const text = await res.text()
    if (!res.ok) throw new Error(`spawn on B: ${res.status} ${text}`)
    const spawn = JSON.parse(text) as { sessionId: string; reused: boolean; attachOnly?: boolean }
    expect(spawn.reused).toBe(false)
    expect(spawn.attachOnly).toBe(undefined)
    await until(async () => (await sessionsOn(B_PORT, B_OWNER)).some((s) => s.agentName === typedAgent), 'the session on B')
    expect((await sessionsOn(A_PORT, A_OWNER)).some((s) => s.agentName === typedAgent)).toBe(false)

    // The pane's grid socket on B, its mode from the room, then keystrokes.
    const ws = await openQueuedGridWebSocket(
      room.scope,
      `${daemonWsBase(creds)}/cli/sessions/grid?session=${spawn.sessionId}&token=${encodeURIComponent(creds.token)}&proto=k1`,
    )
    expect(portOf(ws.url)).toBe(B_PORT)
    const mode = paneRoomMode(room)
    if (mode === null) throw new Error('a pinned room has a mode')
    ws.send(JSON.stringify({ action: 'set_mode', mode }))
    const marker = join(CHECKOUT, 'm5-typed.txt')
    ws.send(JSON.stringify({ action: 'input', text: `echo typed-in-b > ${marker}\r` }))
    await until(() => existsSync(marker) && readFileSync(marker, 'utf8').trim() === 'typed-in-b', 'the typed command to run on B')
    ws.close()
  })

  it('closing the tab saves B’s layout at once and ends its session on B (v2/close, tab_close)', async () => {
    const before = await stored(B_PORT, B_OWNER, projB)
    room.tabs.getState().removeTab(typedTabId)
    const after = await untilStoredOnB((cols) => !cols[0].includes(typedTabId), 'the closed tab gone from B’s layout')
    expect(after.cols[0]).toEqual(['tab-b1', 'tab-b2'])
    expect(after.revision).toBeGreaterThan(before.revision)
    await until(async () => !(await sessionsOn(B_PORT, B_OWNER)).some((s) => s.agentName === typedAgent), 'the session on B to end')
    const closes = sinceOpen().filter((r) => r.url.includes('/cli/sessions/v2/close'))
    expect(closes.length).toBe(1)
    expect(portOf(closes[0].url)).toBe(B_PORT)
    if (closes[0].body === null) throw new Error('a close without a body')
    expect(JSON.parse(closes[0].body)).toEqual({ agent_name: typedAgent, force: true, reason: 'tab_close' })
  })

  it('splitting saves the shared columns on B (D1), with a revision', async () => {
    const before = await stored(B_PORT, B_OWNER, projB)
    room.tabs.getState().splitTerminalArea(CHECKOUT)
    room.tabs.getState().flushLayoutPersist()
    const after = await untilStoredOnB((cols) => cols.length === 2, 'the split in B’s layout')
    expect(after.revision).toBeGreaterThan(before.revision)
    const json = JSON.parse((await stored(B_PORT, B_OWNER, projB)).layoutJson) as StoredLayout
    expect(json.splitCount).toBe(2)
    expect(after.cols[0]).toEqual(['tab-b1', 'tab-b2'])
    expect(after.cols[1].length).toBe(1)
    // B's own room state says the same.
    expect(room.tabs.getState().splitCount).toBe(2)
    room.tabs.getState().unsplitTerminalArea()
    room.tabs.getState().flushLayoutPersist()
    await untilStoredOnB((cols) => cols.length === 1, 'the unsplit in B’s layout')
  })

  it('a save from a stale revision gets B’s 409 and is merged, not dropped', async () => {
    // Another writer on B (B's own window) adds a tab.
    const cur = await stored(B_PORT, B_OWNER, projB)
    const theirs = JSON.parse(cur.layoutJson) as StoredLayout & { tabs: unknown[] }
    theirs.tabs.push(JSON.parse(layoutJson(['tab-theirs'])).tabs[0])
    await daemon(B_PORT, B_OWNER, 'workspace-layouts/save', {
      body: { projectId: projB.id, workspaceId: projB.workspaces[0].id, layoutJson: JSON.stringify(theirs), baseRevision: cur.revision },
    })
    await until(() => userTabs(room).includes('tab-theirs'), 'the room to hear the other writer')
    // The room still believes the older revision when it saves.
    room.tabs.room.__test.layoutRevisions.set(key, cur.revision)
    const mark = h.requests.length
    room.tabs.getState().addTab(CHECKOUT)
    const mine = userTabs(room)[userTabs(room).length - 1]
    room.tabs.getState().flushLayoutPersist()
    const merged = await untilStoredOnB((cols) => cols[0].includes(mine), 'the merged save on B')
    expect(merged.cols[0]).toContain('tab-theirs')
    expect(merged.cols[0]).toContain(mine)
    const saves = h.requests.slice(mark).filter((r) => r.url.includes('/cli/workspace-layouts/save'))
    expect(saves.map((s) => s.status)).toContain(409)
    expect(saves[saves.length - 1].status).toBe(200)
    expect(saves.every((s) => portOf(s.url) === B_PORT)).toBe(true)
    room.tabs.getState().removeTab(mine)
    await untilStoredOnB((cols) => !cols[0].includes(mine), 'the merged tab closed on B')
  })

  it('a route that is not on the room allowlist is refused before any request; this computer’s commands stay off', async () => {
    const mark = h.requests.length
    await expect(daemonCliPost(room.scope, 'projects/delete', { id: projB.id })).rejects.toBeInstanceOf(RoomWriteRefusedError)
    await expect(daemonCliPost(room.scope, 'presets/reset', {})).rejects.toBeInstanceOf(RoomWriteRefusedError)
    await expect(daemonCliPost(room.scope, 'fs/open-finder', { target: CHECKOUT })).rejects.toBeInstanceOf(RoomWriteRefusedError)
    expect(h.requests.slice(mark)).toEqual([])
    expect((await daemon<ProjectWithWorkspaces[]>(B_PORT, B_OWNER, 'projects/list')).some((p) => p.id === projB.id)).toBe(true)
    for (const name of Object.keys(LOCAL_ONLY_ACTIONS)) expect([name, roomMayRun(room, name)]).toEqual([name, false])
  })

  it('A was never touched: its layout is byte-identical and no request or socket went to A', async () => {
    const aNow = await stored(A_PORT, A_OWNER, projA)
    expect(aNow.layoutJson).toBe(aAtOpen.layoutJson)
    expect(aNow.revision).toBe(aAtOpen.revision)
    expect(sinceOpen().filter((r) => portOf(r.url) === A_PORT)).toEqual([])
    expect(h.sockets.slice(socketsFrom).filter((s) => portOf(s.url) === A_PORT).map((s) => s.url)).toEqual([])
    const tokens = new Set(sinceOpen().filter((r) => portOf(r.url) === B_PORT).map((r) => new URL(r.url).searchParams.get('token')))
    expect(tokens.has(A_OWNER)).toBe(false)
    expect(tokens.has(B_OWNER)).toBe(false)
  })
})

describe('an older B (no layout revision) stays view only (MS43, MS52 k)', () => {
  it('the room opens view only, and a tab change saves nothing on B', async () => {
    await homeRooms.closeAll()
    h.oldB.on = true
    const before = await stored(B_PORT, B_OWNER, projB)
    const old = await openRow()
    const entry = homeRooms.store.getState().entries[row.address]
    if (!entry) throw new Error('no entry for the row')
    expect(entry.access).toBe('view-older-server')
    expect(old.readOnly).toBe(true)
    expect(old.scope.viewOnly).toBe(true)
    expect(paneRoomMode(old)).toBe('viewer')
    const mark = h.requests.length
    old.tabs.getState().addTab(CHECKOUT)
    old.tabs.getState().flushLayoutPersist()
    await new Promise((r) => setTimeout(r, 1_500))
    expect(h.requests.slice(mark).filter((r) => r.method === 'POST' && !r.url.includes('/cli/projects/activate'))).toEqual([])
    const after = await stored(B_PORT, B_OWNER, projB)
    expect(after.revision).toBe(before.revision)
    expect(after.layoutJson).toBe(before.layoutJson)
    h.oldB.on = false
  })
})
