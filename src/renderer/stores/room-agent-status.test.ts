// prd-daemon-activity-and-thread-working-v1 S5 (T-S5e; was Home 0.43.2
// Z22/Z23/Z30 T4.4) — a pinned room renders its OWN server's activity rows.
//
// Frames travel the real path: a workspace socket on the room's server
// carrying the app bus (no app socket open there), into that server's
// activity store, never the window's. A tab maps to its row through its
// `sessionId`. A server without `daemon-activity` (0.44.x) is fed by the
// legacy adapter: its observer and hook events over the same socket (RL13).
// The snapshot and `agents/running` routes are faked per server.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

const h = vi.hoisted(() => ({
  chimes: [] as Array<string | null>,
  snapshots: [] as Array<{ scopeId: string; body: unknown }>,
  pulls: [] as string[],
  running: new Map<string, Array<{ terminalId: string; agentName: string; cwd: string }>>(),
}))

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async (scope: { id: string }, route: string) => {
    if (route === 'agents/running') return h.running.get(scope.id) ?? []
    if (route !== 'activity/snapshot') throw new Error(`unexpected GET ${route}`)
    h.pulls.push(scope.id)
    const i = h.snapshots.findIndex((s) => s.scopeId === scope.id)
    if (i < 0) throw new Error(`no snapshot queued for ${scope.id}`)
    return h.snapshots.splice(i, 1)[0].body
  },
  daemonCliPost: async () => {
    throw new Error('unexpected POST')
  },
}))
vi.mock('@/lib/completion-sound', () => ({
  playCompletionSound: (projectId: string | null) => {
    h.chimes.push(projectId)
  },
}))

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  url: string
  readyState = 0
  binaryType = 'blob'
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: ((ev: { code: number; reason?: string; wasClean?: boolean }) => void) | null = null
  private listeners = new Map<string, Set<() => void>>()
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  addEventListener(type: string, fn: () => void): void {
    let set = this.listeners.get(type)
    if (!set) {
      set = new Set()
      this.listeners.set(type, set)
    }
    set.add(fn)
  }
  removeEventListener(type: string, fn: () => void): void {
    this.listeners.get(type)?.delete(fn)
  }
  open(): void {
    this.readyState = 1
    for (const fn of [...(this.listeners.get('open') ?? [])]) fn()
    this.onopen?.()
  }
  close(): void {
    this.readyState = 3
  }
}
vi.stubGlobal('WebSocket', FakeWebSocket)

import { createStore } from 'zustand/vanilla'
import { noteServerVersion, scopeForHost, __resetServerScopesForTests } from '@/kessel/server-scope'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import { openAppBus, subscribeToWorkspaceSessionEvents, type ActivityRow, type UnsubscribeFn } from '@/stores/session-events'
import { createPinnedRoom, type PinnedRoom, type RoomProjectsStore } from '@/stores/room'
import {
  __resetActivityForTests,
  activityStore,
  agentHasUnseen,
  projectHasUnseen,
  terminalDisplay,
  workspaceDisplay,
} from '@/stores/activity'
import { roomRowActivity } from '@/lib/home-status'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { TerminalItemData } from '@/stores/tabs'

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: 'b.example.com',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}
const ROOT = '/srv/anna'
const INSTANCE = 'inst-b'

const cleanups: Array<() => unknown> = []

beforeEach(() => {
  mem.clear()
  h.chimes = []
  h.snapshots = []
  h.pulls = []
  h.running.clear()
  FakeWebSocket.instances = []
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  __resetActivityForTests()
  resetGridDialQueueForTests()
  useConnectHostStore.getState().addHost(B)
})

afterEach(async () => {
  vi.useRealTimers()
  while (cleanups.length > 0) await cleanups.pop()!()
  __resetActivityForTests()
})

function emptyProjects(): RoomProjectsStore {
  return createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects: [] })) as RoomProjectsStore
}

/** B's project list as the room loaded it (attribution for legacy rows). */
function annaProjects(): RoomProjectsStore {
  const projects = [{ id: 'p-anna', path: ROOT }] as unknown as ProjectWithWorkspaces[]
  return createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects })) as RoomProjectsStore
}

function pinned(projects: RoomProjectsStore = emptyProjects()): PinnedRoom {
  const room = createPinnedRoom({
    scope: scopeForHost(B),
    workspace: { projectId: 'p-anna', workspaceId: 'w', path: ROOT },
    projects,
    activateProject: () => {},
  })
  cleanups.push(() => room.dispose())
  return room
}

/** Add a terminal tab whose daemon session is `sessionId`; returns its
 *  item data. */
function tabWithSession(room: PinnedRoom, sessionId: string): TerminalItemData {
  room.tabs.getState().addTab(ROOT)
  const tab = room.tabs.getState().tabs[room.tabs.getState().tabs.length - 1]
  const data = [...tab.paneGroups.values()][0].items[0].data as TerminalItemData
  // TerminalPane stamps the v2 session id after spawn; stand in for it.
  data.sessionId = sessionId
  return data
}

/** The room's server's workspace socket, carrying its app bus. */
async function carrier(room: PinnedRoom): Promise<FakeWebSocket> {
  const before = FakeWebSocket.instances.length
  const off: UnsubscribeFn = subscribeToWorkspaceSessionEvents(room.scope, ROOT, { carryAppBus: true })
  cleanups.push(off)
  await vi.waitFor(() => expect(FakeWebSocket.instances.length).toBe(before + 1))
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  expect(ws.url.startsWith('wss://b.example.com/cli/sessions/events?path=%2Fsrv%2Fanna')).toBe(true)
  ws.open()
  return ws
}

function row(sessionId: string, display: ActivityRow['display'], workspacePath = ROOT): ActivityRow {
  return {
    sessionId,
    agentName: `tab-${sessionId}`,
    projectId: workspacePath === ROOT ? 'p-anna' : 'p-other',
    workspacePath,
    harness: 'claude',
    display,
    lead: { state: display === 'idle' ? 'idle' : 'working', outcome: 'none', since: 0, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: null,
    evidenceAt: 1,
    evidenceSource: 'hook',
    reason: 'turn_running',
    staleSince: null,
    confirmed: true,
    rev: 1,
  }
}

function send(ws: FakeWebSocket, msg: unknown): void {
  if (!ws.onmessage) throw new Error('carrier socket has no onmessage')
  ws.onmessage({ data: JSON.stringify(msg) })
}

function changed(ws: FakeWebSocket, seq: number, r: ActivityRow, turnEnded: unknown = null): void {
  send(ws, { kind: 'activity_changed', seq, instanceId: INSTANCE, row: r, removed: null, turnEnded, workspace: null })
}

/** The first frame from B pulls its snapshot (we hold none yet). */
async function primed(ws: FakeWebSocket, room: PinnedRoom, rows: ActivityRow[]): Promise<void> {
  h.snapshots.push({
    scopeId: room.scope.id,
    body: { instanceId: INSTANCE, seq: 1, serverNow: Date.now(), staleAfterSecs: 1800, rows, workspaces: [] },
  })
  changed(ws, 1, rows[0])
  await vi.waitFor(() => expect(activityStore(room.scope).getState().seq).toBe(1))
}

describe('a pinned room renders its server’s activity rows (T-S5e)', () => {
  it('reads B’s rows through its tab’s sessionId; a turn end under its root marks + chimes once', async () => {
    const room = pinned()
    const data = tabWithSession(room, 'sid-1')
    const ws = await carrier(room)
    expect(openAppBus(room.scope).openSockets).toBe(0)
    await primed(ws, room, [row('sid-1', 'idle')])
    expect(h.pulls).toEqual([room.scope.id])

    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })
    changed(ws, 2, row('sid-1', 'working'))
    expect(terminalDisplay(room.activityView.getState(), data)).toBe('working')
    // The window's own server never sees B's rows.
    expect(activityStore({ id: 'primary' }).getState().rows.size).toBe(0)

    // Another workspace on the same server ends a turn: not this room's.
    changed(ws, 3, row('sid-other', 'working', '/srv/anna-other'))
    await vi.advanceTimersByTimeAsync(6_000)
    changed(ws, 4, row('sid-other', 'idle', '/srv/anna-other'), { outcome: 'success', reason: 'turn_done', at: Date.now() })
    changed(ws, 5, row('sid-1', 'idle'), { outcome: 'success', reason: 'turn_done', at: Date.now() })
    await vi.advanceTimersByTimeAsync(5_000)
    expect(terminalDisplay(room.activityView.getState(), data)).toBe('idle')
    expect(agentHasUnseen(room.activityView.getState(), 'tab-sid-1', 'sid-1')).toBe(true)
    expect(h.chimes).toEqual(['p-anna'])
  })

  it('a busy session under the root with no tab (the pinned Chat) still makes the room busy', async () => {
    const room = pinned()
    const ws = await carrier(room)
    await primed(ws, room, [row('sid-chat', 'working')])
    expect(roomRowActivity(room.activityView.getState(), ROOT)).toBe('working')
    changed(ws, 2, row('sid-chat', 'waiting'))
    expect(roomRowActivity(room.activityView.getState(), ROOT)).toBe('permission')
    changed(ws, 3, row('sid-chat', 'monitoring'))
    expect(roomRowActivity(room.activityView.getState(), ROOT)).toBe('monitoring')
  })

  it('dispose stops the room hearing its server', async () => {
    const room = pinned()
    const data = tabWithSession(room, 'sid-3')
    const ws = await carrier(room)
    await primed(ws, room, [row('sid-3', 'idle')])
    await room.dispose()
    changed(ws, 2, row('sid-3', 'working'))
    expect(terminalDisplay(room.activityView.getState(), data)).toBe('idle')
  })

  it('a 0.44.x server (no daemon-activity): its observer and hook events light the tab, the room row and its project (RL13)', async () => {
    // What B's /boot-status said: a 0.44.x feature list.
    noteServerVersion('b.example.com', '0.44.3', ['spawn-attach-only', 'tickets-list-all', 'thread-latest'])
    const room = pinned(annaProjects())
    const data = tabWithSession(room, 'sid-4')
    h.running.set(room.scope.id, [{ terminalId: 'sid-4', agentName: `tab-${data.terminalId}`, cwd: ROOT }])
    const ws = await carrier(room)
    // The socket opened: B's hello re-seeds which agent is which session.
    send(ws, { kind: 'hello', workspace_path: ROOT, subscriber_id: 1, instance_id: INSTANCE })
    await vi.waitFor(() => expect(room.activityView.getState().rows.size).toBe(0))

    // B's title/bell observer: the frame a 0.44.4 daemon sends.
    send(ws, {
      kind: 'session_activity_changed',
      workspacePath: ROOT,
      agentName: `tab-${data.terminalId}`,
      paneGroupId: data.terminalId,
      status: 'working',
    })
    let view = room.activityView.getState()
    expect(view.supported).toBe(false)
    expect(terminalDisplay(view, data)).toBe('working')
    expect(roomRowActivity(view, ROOT)).toBe('working')
    expect(workspaceDisplay(view, { projectId: 'p-anna' })).toBe('working')

    // B's hook bucket, keyed by the session id: a permission wins.
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })
    send(ws, { kind: 'agent_status_changed', paneId: 'sid-4', tabId: 'sid-4', status: 'permission', workspacePath: ROOT })
    view = room.activityView.getState()
    expect(terminalDisplay(view, data)).toBe('waiting')
    expect(roomRowActivity(view, ROOT)).toBe('permission')
    expect([...view.rows.keys()]).toEqual(['sid-4'])

    send(ws, { kind: 'agent_status_changed', paneId: 'sid-4', tabId: 'sid-4', status: 'start', workspacePath: ROOT })
    await vi.advanceTimersByTimeAsync(6_000)
    send(ws, {
      kind: 'session_activity_changed',
      workspacePath: ROOT,
      agentName: `tab-${data.terminalId}`,
      paneGroupId: data.terminalId,
      status: 'idle',
    })
    await vi.advanceTimersByTimeAsync(5_000)
    view = room.activityView.getState()
    expect(terminalDisplay(view, data)).toBe('idle')
    expect(agentHasUnseen(view, `tab-${data.terminalId}`, 'sid-4')).toBe(true)
    expect(projectHasUnseen(view, 'p-anna')).toBe(true)
    expect(h.chimes).toEqual(['p-anna'])
    // Never asked B for a snapshot it hasn't got.
    expect(h.pulls).toEqual([])
    // The window's own server never sees B's rows.
    expect(activityStore({ id: 'primary' }).getState().rows.size).toBe(0)
  })

  it('a server that reports daemon-activity: the same legacy frames change nothing (RL13)', async () => {
    noteServerVersion('b.example.com', '0.45.0', ['spawn-attach-only', 'daemon-activity'])
    // Known up front: the room pulls B's snapshot as it attaches.
    h.snapshots.push({
      scopeId: scopeForHost(B).id,
      body: { instanceId: INSTANCE, seq: 1, serverNow: Date.now(), staleAfterSecs: 1800, rows: [row('sid-5', 'working')], workspaces: [] },
    })
    const room = pinned(annaProjects())
    const data = tabWithSession(room, 'sid-5')
    await vi.waitFor(() => expect(activityStore(room.scope).getState().seq).toBe(1))
    const ws = await carrier(room)
    send(ws, {
      kind: 'session_activity_changed',
      workspacePath: ROOT,
      agentName: `tab-${data.terminalId}`,
      paneGroupId: data.terminalId,
      status: 'idle',
    })
    send(ws, { kind: 'agent_status_changed', paneId: 'sid-5', tabId: 'sid-5', status: 'permission', workspacePath: ROOT })
    const view = room.activityView.getState()
    expect(view.supported).toBe(true)
    expect(terminalDisplay(view, data)).toBe('working')
    expect([...view.rows.keys()]).toEqual(['sid-5'])
    expect(h.pulls).toEqual([room.scope.id])
  })
})
