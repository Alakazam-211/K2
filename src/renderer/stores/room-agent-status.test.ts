// Home 0.43.2 (prd-home-seamless-0432 Z22, Z23, Z30; T4.4) — a pinned room
// applies its own server's `agent_status_changed`.
//
// The frame travels the real path: a workspace socket on the room's server
// carrying the app bus (no app socket open there), into the room's handler,
// matched to a pane through the tab's `sessionId` (a hook's `paneId` is the
// v2 session id, never the terminal id). The two-daemon half (a real
// `/hook/complete` on B) is `room-agent-status.mstest.ts`.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

const h = vi.hoisted(() => ({ chimes: [] as Array<string | null> }))

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
import { scopeForHost, __resetServerScopesForTests } from '@/kessel/server-scope'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import { openAppBus, subscribeToWorkspaceSessionEvents, type UnsubscribeFn } from '@/stores/session-events'
import {
  createPinnedRoom,
  createRoomActivity,
  roomPaneForHook,
  type PinnedRoom,
  type RoomProjectsStore,
} from '@/stores/room'
import { roomRowActivity } from '@/lib/home-status'
import { mergePaneStatus } from '@/stores/active-agents'
import type { ProjectWithWorkspaces, } from '@/stores/projects'
import type { TerminalItemData } from '@/stores/tabs'

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: 'z3thon.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}
const ROOT = '/srv/anna'

const cleanups: Array<() => unknown> = []

beforeEach(() => {
  mem.clear()
  h.chimes = []
  FakeWebSocket.instances = []
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  resetGridDialQueueForTests()
  useConnectHostStore.getState().addHost(B)
})

afterEach(async () => {
  while (cleanups.length > 0) await cleanups.pop()!()
})

function emptyProjects(): RoomProjectsStore {
  return createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects: [] })) as RoomProjectsStore
}

function pinned(): PinnedRoom {
  const room = createPinnedRoom({
    scope: scopeForHost(B),
    workspace: { projectId: 'p-anna', workspaceId: 'w', path: ROOT },
    projects: emptyProjects(),
    activateProject: () => {},
  })
  cleanups.push(() => room.dispose())
  return room
}

/** Add a terminal tab whose daemon session is `sessionId`; returns its
 *  terminal id. */
function tabWithSession(room: PinnedRoom, sessionId: string): string {
  room.tabs.getState().addTab(ROOT)
  const tab = room.tabs.getState().tabs[room.tabs.getState().tabs.length - 1]
  const data = [...tab.paneGroups.values()][0].items[0].data as TerminalItemData
  // TerminalPane stamps the v2 session id after spawn; stand in for it.
  data.sessionId = sessionId
  return data.terminalId
}

/** The room's server's workspace socket, carrying its app bus. */
async function carrier(room: PinnedRoom): Promise<FakeWebSocket> {
  const before = FakeWebSocket.instances.length
  const off: UnsubscribeFn = subscribeToWorkspaceSessionEvents(room.scope, ROOT, { carryAppBus: true })
  cleanups.push(off)
  await vi.waitFor(() => expect(FakeWebSocket.instances.length).toBe(before + 1))
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  expect(ws.url.startsWith('wss://z3thon.k2.dev/cli/sessions/events?path=%2Fsrv%2Fanna')).toBe(true)
  ws.open()
  return ws
}

function hook(ws: FakeWebSocket, paneId: string, status: 'start' | 'stop' | 'permission', workspacePath?: string): void {
  if (!ws.onmessage) throw new Error('carrier socket has no onmessage')
  ws.onmessage({
    data: JSON.stringify({ kind: 'agent_status_changed', paneId, tabId: paneId, status, ...(workspacePath ? { workspacePath } : {}) }),
  })
}

describe('a pinned room applies its server’s agent_status_changed (T4.4)', () => {
  it('working → idle with unseen-done and one chime; other workspaces change nothing', async () => {
    const room = pinned()
    const term = tabWithSession(room, 'sid-1')
    const ws = await carrier(room)
    expect(openAppBus(room.scope).openSockets).toBe(0)

    hook(ws, 'sid-1', 'start', ROOT)
    expect(room.activityView.getState().paneStatuses.get(term)).toBe('working')

    // Another workspace on the same server, and a nested-but-other path.
    const before = new Map(room.activityView.getState().paneStatuses)
    hook(ws, 'sid-other', 'start', '/srv/anna-other')
    hook(ws, 'sid-1', 'stop', '/srv/anna-other')
    hook(ws, 'sid-unknown', 'start')
    expect(room.activityView.getState().paneStatuses).toEqual(before)

    hook(ws, 'sid-1', 'stop', ROOT)
    expect(room.activityView.getState().paneStatuses.get(term)).toBe('idle')
    expect(room.activityView.getState().unseenDone.has(term)).toBe(true)
    expect(h.chimes).toEqual(['p-anna'])

    // A second stop, or the title's own idle, never chimes again.
    hook(ws, 'sid-1', 'stop', ROOT)
    room.activity.recordTitleActivity(term, false)
    expect(h.chimes).toEqual(['p-anna'])
  })

  it('a frame with no path counts only when the session is one of the room’s tabs', async () => {
    const room = pinned()
    const term = tabWithSession(room, 'sid-2')
    const ws = await carrier(room)
    hook(ws, 'sid-2', 'permission')
    expect(room.activityView.getState().paneStatuses.get(term)).toBe('permission')
    // The hook owns permission: a title idle can't clear it.
    room.activity.recordTitleActivity(term, false)
    expect(room.activityView.getState().paneStatuses.get(term)).toBe('permission')
  })

  it('a busy session under the root with no tab (the pinned Chat) still makes the room busy', async () => {
    const room = pinned()
    const ws = await carrier(room)
    hook(ws, 'sid-chat', 'start', ROOT)
    expect(roomRowActivity(room.activityView.getState(), mergePaneStatus)).toBe('working')
    hook(ws, 'sid-chat', 'permission', `${ROOT}/sub`)
    expect(roomRowActivity(room.activityView.getState(), mergePaneStatus)).toBe('permission')
  })

  it('dispose stops the room hearing its server', async () => {
    const room = pinned()
    const term = tabWithSession(room, 'sid-3')
    const ws = await carrier(room)
    await room.dispose()
    hook(ws, 'sid-3', 'start', ROOT)
    expect(room.activityView.getState().paneStatuses.has(term)).toBe(false)
  })
})

describe('roomPaneForHook (Z30)', () => {
  const tabsState = { tabs: [], extraGroups: [] }

  it('drops a path outside the root before any lookup', () => {
    const aliases = new Map([['sid-a', 't-a']])
    expect(roomPaneForHook({ paneId: 'sid-a', workspacePath: '/srv/other' }, ROOT, tabsState, aliases)).toBe(null)
  })

  it('maps through the aliases, then falls back to the session id under the root', () => {
    const aliases = new Map([['sid-a', 't-a']])
    expect(roomPaneForHook({ paneId: 'sid-a', workspacePath: ROOT }, ROOT, tabsState, aliases)).toBe('t-a')
    expect(roomPaneForHook({ paneId: 'sid-b', workspacePath: ROOT }, ROOT, tabsState, aliases)).toBe('sid-b')
    expect(roomPaneForHook({ paneId: 'sid-b' }, ROOT, tabsState, aliases)).toBe(null)
  })
})

describe('createRoomActivity.applyHookStatus', () => {
  it('a stop for a pane never seen busy records idle without a chime', () => {
    const activity = createRoomActivity(emptyProjects(), 'p')
    activity.applyHookStatus('t', 'stop')
    expect(activity.getState().paneStatuses.get('t')).toBe('idle')
    expect(activity.getState().unseenDone.size).toBe(0)
    expect(h.chimes).toEqual([])
  })
})
