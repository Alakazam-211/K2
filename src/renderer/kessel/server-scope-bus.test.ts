// Home M1 — per-server event buses and grid dial queues. Two servers with
// the same workspace path and project id never share an event delivery or a
// grid dial slot. A bus for a non-primary server is lazy: registering a
// handler opens no socket.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'available', port: 50123, token: 'local-tok' }
    return null
  }),
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
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  close(): void {
    this.readyState = 3
  }
}
vi.stubGlobal('WebSocket', FakeWebSocket)

import { primaryScope, scopeForHost, __resetServerScopesForTests } from './server-scope'
import {
  openAppBus,
  onFsChanged,
  onAppHello,
  onSessionAddedApp,
  subscribeToActiveState,
  subscribeToWorkspaceSessionEvents,
  subscribeToWorkspaceTabEvents,
} from '@/stores/session-events'
import {
  BACKOFF_MS,
  gridDialBackoffRemainingMs,
  gridDialInflightForTests,
  noteGridDialFailure,
  openQueuedGridWebSocket,
  resetGridDialQueueForTests,
} from '@/lib/grid-dial-queue'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

const SAME_PATH = '/Users/z3thon/DevProjects/K2'
const SAME_PROJECT = 'p-k2'

function host(over: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'a',
    label: 'A',
    hostname: 'rosson.k2.dev',
    port: 443,
    secure: true,
    token: 'tok-a',
    remember: false,
    lastConnectedAt: null,
    ...over,
  }
}
const A = host({})
const B = host({ id: 'b', label: 'B', hostname: 'z3thon.k2.dev', token: 'tok-b' })

beforeEach(() => {
  mem.clear()
  FakeWebSocket.instances = []
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  resetGridDialQueueForTests()
  useConnectHostStore.getState().addHost(A)
  useConnectHostStore.getState().addHost(B)
})

function socketFor(prefix: string): FakeWebSocket {
  const ws = FakeWebSocket.instances.find((w) => w.url.startsWith(prefix))
  if (!ws) throw new Error(`no socket for ${prefix}; have ${FakeWebSocket.instances.map((w) => w.url).join(', ')}`)
  return ws
}

function push(ws: FakeWebSocket, frame: unknown): void {
  if (!ws.onmessage) throw new Error(`socket ${ws.url} has no onmessage`)
  ws.onmessage({ data: JSON.stringify(frame) })
}

describe('app event bus per server', () => {
  it('a bus for another server opens no socket when a handler registers', async () => {
    const b = scopeForHost(B)
    const seen: string[] = []
    const off = onFsChanged(b, (e) => seen.push(e.workspacePath))
    await new Promise((r) => setTimeout(r, 0))
    expect(FakeWebSocket.instances).toHaveLength(0)
    expect(openAppBus(b).openSockets).toBe(0)
    expect(openAppBus(b).handlerCount()).toBe(1)
    off()
    expect(openAppBus(b).handlerCount()).toBe(0)
  })

  it('the primary bus and a pinned bus are different objects with different handler sets', () => {
    const p = openAppBus(primaryScope())
    const b = openAppBus(scopeForHost(B))
    expect(p).not.toBe(b)
    expect(p.scopeId).toBe('primary')
    expect(b.scopeId).toBe('host:z3thon.k2.dev')
    expect(openAppBus(primaryScope())).toBe(p)
    expect(openAppBus(scopeForHost('z3thon.k2.dev'))).toBe(b)
  })

  it('same path + project on two servers: an event on A never reaches B and vice versa', async () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const onA: string[] = []
    const onB: string[] = []
    const helloA: number[] = []
    const helloB: number[] = []
    const offs = [
      onFsChanged(a, (e) => onA.push(`fs:${e.workspacePath}`)),
      onFsChanged(b, (e) => onB.push(`fs:${e.workspacePath}`)),
      onSessionAddedApp(a, (e) => onA.push(`add:${e.workspace_path}`)),
      onSessionAddedApp(b, (e) => onB.push(`add:${e.workspace_path}`)),
      onAppHello(a, () => helloA.push(1)),
      onAppHello(b, () => helloB.push(1)),
    ]
    const unsubA = subscribeToActiveState(a)
    const unsubB = subscribeToActiveState(b)
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2))
    expect(openAppBus(a).openSockets).toBe(1)
    expect(openAppBus(b).openSockets).toBe(1)
    const wsA = socketFor('wss://rosson.k2.dev/cli/sessions/events?path=&token=tok-a')
    const wsB = socketFor('wss://z3thon.k2.dev/cli/sessions/events?path=&token=tok-b')

    push(wsA, { kind: 'fs_changed', workspacePath: SAME_PATH, paths: [`${SAME_PATH}/a.txt`] })
    push(wsA, { kind: 'session_added', workspace_path: SAME_PATH, agent_name: SAME_PROJECT, args: [], isV2: true })
    push(wsA, { kind: 'hello', workspace_path: '', subscriber_id: 1 })
    expect(onA).toEqual([`fs:${SAME_PATH}`, `add:${SAME_PATH}`])
    expect(onB).toEqual([])
    expect(helloA).toEqual([1])
    expect(helloB).toEqual([])

    push(wsB, { kind: 'fs_changed', workspacePath: SAME_PATH, paths: [`${SAME_PATH}/b.txt`] })
    expect(onA).toEqual([`fs:${SAME_PATH}`, `add:${SAME_PATH}`])
    expect(onB).toEqual([`fs:${SAME_PATH}`])

    unsubA()
    unsubB()
    expect(openAppBus(a).openSockets).toBe(0)
    expect(openAppBus(b).openSockets).toBe(0)
    for (const off of offs) off()
  })

  it('a primary-bus subscriber does not hear a pinned server, even the one the window is on', async () => {
    useConnectHostStore.getState().selectHost(A)
    const primarySeen: string[] = []
    const off = onFsChanged(primaryScope(), (e) => primarySeen.push(e.workspacePath))
    const unsub = subscribeToActiveState(scopeForHost(B))
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1))
    push(socketFor('wss://z3thon.k2.dev/'), { kind: 'fs_changed', workspacePath: SAME_PATH, paths: [] })
    expect(primarySeen).toEqual([])
    unsub()
    off()
  })

  it('workspace sockets for the same path dial each server with its own token', async () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const offs = [
      subscribeToWorkspaceSessionEvents(a, SAME_PATH, {}),
      subscribeToWorkspaceSessionEvents(b, SAME_PATH, {}),
      subscribeToWorkspaceTabEvents(a, SAME_PATH, {}),
      subscribeToWorkspaceTabEvents(b, SAME_PATH, {}),
    ]
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(4))
    const q = `/cli/sessions/events?path=${encodeURIComponent(SAME_PATH)}`
    const urls = FakeWebSocket.instances.map((w) => w.url).sort()
    expect(urls).toEqual(
      [
        `wss://rosson.k2.dev${q}&token=tok-a`,
        `wss://rosson.k2.dev${q}&token=tok-a`,
        `wss://z3thon.k2.dev${q}&token=tok-b`,
        `wss://z3thon.k2.dev${q}&token=tok-b`,
      ].sort(),
    )
    for (const off of offs) off()
  })
})

describe('grid dial queue per server', () => {
  it('a failure burst on A backs off A only', () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const t0 = 1_000_000
    noteGridDialFailure(a, t0)
    noteGridDialFailure(a, t0 + 10)
    noteGridDialFailure(a, t0 + 20)
    expect(gridDialBackoffRemainingMs(a, t0 + 30)).toBe(BACKOFF_MS - 10)
    expect(gridDialBackoffRemainingMs(b, t0 + 30)).toBe(0)
    expect(gridDialBackoffRemainingMs(primaryScope(), t0 + 30)).toBe(0)
  })

  it('two servers never share a dial slot: A full does not count against B', async () => {
    // Window on local → global cap 4; each remote scope's own cap is 2.
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const url = `/cli/sessions/grid?path=${encodeURIComponent(SAME_PATH)}&project=${SAME_PROJECT}`
    const a1 = openQueuedGridWebSocket(a, `wss://rosson.k2.dev${url}`)
    const a2 = openQueuedGridWebSocket(a, `wss://rosson.k2.dev${url}`)
    const a3 = openQueuedGridWebSocket(a, `wss://rosson.k2.dev${url}`)
    const b1 = openQueuedGridWebSocket(b, `wss://z3thon.k2.dev${url}`)
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(3))
    expect(gridDialInflightForTests(a)).toBe(2)
    expect(gridDialInflightForTests(b)).toBe(1)
    expect(gridDialInflightForTests(null)).toBe(3)
    const dialed = FakeWebSocket.instances.map((w) => w.url)
    expect(dialed.filter((u) => u.startsWith('wss://rosson.k2.dev/'))).toHaveLength(2)
    expect(dialed.filter((u) => u.startsWith('wss://z3thon.k2.dev/'))).toHaveLength(1)
    // Opening B's socket frees B's slot only; A's third dial still waits for A.
    const wsB = socketFor('wss://z3thon.k2.dev/')
    wsB.readyState = 1
    wsB.onopen!()
    await expect(b1).resolves.toBe(wsB)
    expect(gridDialInflightForTests(b)).toBe(0)
    expect(gridDialInflightForTests(a)).toBe(2)
    expect(FakeWebSocket.instances).toHaveLength(3)
    // Opening one of A's lets A's waiter dial.
    const firstA = FakeWebSocket.instances[0]!
    firstA.readyState = 1
    firstA.onopen!()
    await expect(a1).resolves.toBe(firstA)
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(4))
    expect(FakeWebSocket.instances[3]!.url.startsWith('wss://rosson.k2.dev/')).toBe(true)
    for (const ws of FakeWebSocket.instances.slice(1)) {
      if (ws === wsB) continue
      ws.readyState = 1
      ws.onopen!()
    }
    await Promise.all([a2, a3])
    expect(gridDialInflightForTests(null)).toBe(0)
  })

  it('the global cap still bounds every server together', async () => {
    // Window on a remote → global cap 2 (unchanged from before M1).
    useConnectHostStore.getState().selectHost(A)
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const p1 = openQueuedGridWebSocket(a, 'wss://rosson.k2.dev/grid')
    const p2 = openQueuedGridWebSocket(b, 'wss://z3thon.k2.dev/grid')
    const p3 = openQueuedGridWebSocket(b, 'wss://z3thon.k2.dev/grid')
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2))
    await new Promise((r) => setTimeout(r, 0))
    expect(FakeWebSocket.instances).toHaveLength(2)
    expect(gridDialInflightForTests(null)).toBe(2)
    for (const ws of FakeWebSocket.instances) {
      ws.readyState = 1
      ws.onopen!()
    }
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(3))
    const third = FakeWebSocket.instances[2]!
    third.readyState = 1
    third.onopen!()
    await Promise.all([p1, p2, p3])
    expect(gridDialInflightForTests(null)).toBe(0)
  })
})
