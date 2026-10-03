// Home 0.43.2 — "working dots everywhere" against two REAL daemons
// (prd-home-seamless-0432 item 4: Z22, Z23, Z30; T4.4, T4.6). Run with
// `bun run test:multiserver`.
//
// The window is on A (`local` = daemon A). B is a saved server, signed in as
// the Member `anna`. Every activity signal is B's own: a real
// `sessions/v2/spawn` on B, then a real `/hook/complete` on B whose `paneId`
// is that session's id (never a made-up id).
//   - A CLOSED row for B turns Working at the next pool check, from B's
//     `/cli/presence/summary` (`agentActivity`), and Needs you on a newer
//     permission hook.
//   - An OPEN room for B hears B's hooks through its own workspace socket
//     (no app socket on B, nothing on A): the tab goes working → permission
//     → idle with unseen-done, and the row follows with no poll.
//   - A hook for another workspace on B changes nothing in the room.

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
  return { sockets: [] as string[], chimes: [] as Array<string | null> }
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
vi.mock('@/lib/completion-sound', () => ({
  playCompletionSound: (projectId: string | null) => {
    h.chimes.push(projectId)
  },
}))

const RealWebSocket = globalThis.WebSocket
globalThis.WebSocket = class extends RealWebSocket {
  constructor(url: string | URL, protocols?: string | string[]) {
    super(url, protocols)
    h.sockets.push(String(url))
  }
} as typeof WebSocket

import { loginToHost, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { openHomeRow } from '@/lib/home-open'
import { homeRooms } from '@/stores/home-rooms'
import { hostPool } from '@/lib/host-pool-instance'
import { workspaceHandle } from '@/lib/home-address'
import { resolveRowStatus, roomRowActivity } from '@/lib/home-status'
import { mergePaneStatus } from '@/stores/active-agents'
import { openAppBus } from '@/stores/session-events'
import type { PinnedRoom } from '@/stores/room'
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
const B_OWNER = env('K2_MS_B_OWNER')
const B_USER = env('K2_MS_B_USER')
const B_KEY = `127.0.0.1:${B_PORT}`

const CHECKOUT = mkdtempSync(join(tmpdir(), 'k2-ms-row-activity-'))
const OTHER = mkdtempSync(join(tmpdir(), 'k2-ms-row-activity-other-'))

async function onB<T>(route: string, init?: { body?: unknown; query?: Record<string, string> }): Promise<T> {
  const q = new URLSearchParams({ ...(init?.query ?? {}), token: B_OWNER })
  const res = await fetch(
    `http://127.0.0.1:${B_PORT}/cli/${route}?${q}`,
    init?.body === undefined
      ? undefined
      : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(init.body) },
  )
  const text = await res.text()
  if (!res.ok) throw new Error(`${route} on B: ${res.status} ${text}`)
  return JSON.parse(text) as T
}

async function registerOnB(path: string): Promise<ProjectWithWorkspaces> {
  await onB('projects/add-without-git', { body: { path, seedWiki: false, seedAgentsMd: false, fanout: false } })
  const list = await onB<ProjectWithWorkspaces[]>('projects/list')
  const p = list.find((x) => x.path === path)
  if (!p) throw new Error(`project for ${path} missing on B`)
  return p
}

/** A real shell session on B; returns its v2 session id. */
async function spawnOnB(agentName: string, cwd: string): Promise<string> {
  const r = await onB<{ sessionId: string }>('sessions/v2/spawn', {
    body: { agent_name: agentName, cwd, command: 'sh', args: [], cols: 80, rows: 24 },
  })
  if (typeof r.sessionId !== 'string' || r.sessionId.length === 0) throw new Error(`spawn on B: no sessionId`)
  return r.sessionId
}

/** A real lifecycle hook on B, as a harness's hook script sends it. */
async function hookOnB(sessionId: string, eventType: string, cwd: string): Promise<void> {
  const q = new URLSearchParams({ token: B_OWNER, paneId: sessionId, tabId: sessionId, eventType, cwd })
  const res = await fetch(`http://127.0.0.1:${B_PORT}/hook/complete?${q}`)
  const text = await res.text()
  if (res.status !== 200) throw new Error(`hook on B: ${res.status} ${text}`)
}

async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 15_000): Promise<void> {
  const deadline = Date.now() + ms
  for (;;) {
    if (await cond()) return
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}

let projB: ProjectWithWorkspaces
let otherB: ProjectWithWorkspaces
let row: HomeRow

function rowStatus(roomActivity: ReturnType<typeof roomRowActivity> | null = null): string {
  return resolveRowStatus({
    where: 'other',
    row,
    saved: true,
    hasLogin: true,
    entry: hostPool.store.getState().entries[B_KEY],
    self: B_USER,
    serverLabel: 'B',
    roomActivity,
  }).kind
}

beforeAll(async () => {
  projB = await registerOnB(CHECKOUT)
  otherB = await registerOnB(OTHER)
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
}, 60_000)

afterAll(async () => {
  await homeRooms.closeAll()
  // The sessions spawned on B end with B (the harness kills both daemons).
  rmSync(CHECKOUT, { recursive: true, force: true })
  rmSync(OTHER, { recursive: true, force: true })
})

describe('a closed Home row for B (T4.6, summary)', () => {
  it('Live → Working → Needs you → Live, each at the next pool check', async () => {
    let e = await hostPool.check(B_KEY)
    expect(e.auth).toBe('ok')
    expect(e.activity).toEqual([])
    expect(rowStatus()).toBe('live')

    const agent = 'tab-row-activity-closed'
    const sid = await spawnOnB(agent, CHECKOUT)
    await hookOnB(sid, 'UserPromptSubmit', CHECKOUT)
    e = await hostPool.check(B_KEY)
    expect(e.activity).toEqual([{ workspaceId: projB.id, status: 'working' }])
    expect(rowStatus()).toBe('working')

    await hookOnB(sid, 'PermissionRequest', CHECKOUT)
    await hostPool.check(B_KEY)
    expect(rowStatus()).toBe('permission')

    await hookOnB(sid, 'Stop', CHECKOUT)
    e = await hostPool.check(B_KEY)
    expect(e.activity).toEqual([])
    expect(rowStatus()).toBe('live')
  })
})

describe('an open room for B (T4.4, T4.6 open half)', () => {
  let room: PinnedRoom
  let terminalId = ''
  let sid = ''

  it('opens on B with no app socket there and nothing dialed on A for activity', async () => {
    expect(openHomeRow(row)).toBe('room')
    await until(() => homeRooms.store.getState().entries[row.address]?.phase === 'open', 'the room to open')
    const opened = homeRooms.store.getState().entries[row.address]?.room
    if (!opened) throw new Error('the room opened without a room')
    room = opened
    await until(() => room.tabs.getState().activeWorkspaceKey !== null, "B's layout to load")
    expect(openAppBus(room.scope).openSockets).toBe(0)
    await until(
      () => h.sockets.some((u) => u.includes(`:${B_PORT}/cli/sessions/events?path=${encodeURIComponent(CHECKOUT)}`)),
      "the room's workspace socket on B",
    )
    expect(h.sockets.filter((u) => u.includes(`:${A_PORT}/cli/sessions/events?path=${encodeURIComponent(CHECKOUT)}`))).toEqual([])
  })

  it('B’s hooks reach the room’s tab through its sessionId: working → permission → idle, one chime', async () => {
    room.tabs.getState().addTab(CHECKOUT)
    const tab = room.tabs.getState().tabs[room.tabs.getState().tabs.length - 1]
    const data = [...tab.paneGroups.values()][0].items[0].data as TerminalItemData
    terminalId = data.terminalId
    const agent = `tab-${terminalId}`
    sid = await spawnOnB(agent, CHECKOUT)
    // TerminalPane stamps the session id it got back from v2/spawn.
    data.sessionId = sid

    await hookOnB(sid, 'UserPromptSubmit', CHECKOUT)
    await until(() => room.activityView.getState().paneStatuses.get(terminalId) === 'working', 'working in the room')
    expect(roomRowActivity(room.activityView.getState(), mergePaneStatus)).toBe('working')
    expect(rowStatus(roomRowActivity(room.activityView.getState(), mergePaneStatus))).toBe('working')

    await hookOnB(sid, 'PermissionRequest', CHECKOUT)
    await until(() => room.activityView.getState().paneStatuses.get(terminalId) === 'permission', 'permission in the room')
    expect(rowStatus(roomRowActivity(room.activityView.getState(), mergePaneStatus))).toBe('permission')

    const chimesBefore = h.chimes.length
    await hookOnB(sid, 'Stop', CHECKOUT)
    await until(() => room.activityView.getState().paneStatuses.get(terminalId) === 'idle', 'idle in the room')
    expect(room.activityView.getState().unseenDone.has(terminalId)).toBe(true)
    expect(h.chimes.slice(chimesBefore)).toEqual([projB.id])
  })

  it('a hook for another workspace on B changes nothing in the room', async () => {
    const agent = 'tab-row-activity-other'
    const otherSid = await spawnOnB(agent, OTHER)
    const before = new Map(room.activityView.getState().paneStatuses)
    await hookOnB(otherSid, 'UserPromptSubmit', OTHER)
    // The same hook reaches B's summary, so it has been processed by now.
    await until(async () => {
      const e = await hostPool.check(B_KEY)
      return (e.activity ?? []).some((a) => a.workspaceId === otherB.id)
    }, "the other workspace's hook on B")
    expect(room.activityView.getState().paneStatuses).toEqual(before)
    expect(roomRowActivity(room.activityView.getState(), mergePaneStatus)).toBe('idle')
  })
})
