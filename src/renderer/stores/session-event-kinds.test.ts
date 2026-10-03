// Home 0.43.2 (prd-home-seamless-0432 Z24, Z25, Z39; T4.2, T4.3) — the
// shared event-kind registry and the three socket factories that read it.
//
// The ratchet: `src/shared/session-event-kinds.json` is checked against the
// daemon's `SessionEvent` enum by a Rust test
// (`session_event_kinds_match_shared_registry`), and against this client's
// route table here. A kind the daemon adds fails one of the two until the
// client handles it or marks it ignored on purpose.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

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
  /** Open the way a browser does: listeners (the dial queue), then onopen. */
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

import registry from '@shared/session-event-kinds.json'
import { scopeForHost, __resetServerScopesForTests, type ServerScope } from '@/kessel/server-scope'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import {
  onAgentStatusChanged,
  onMailChanged,
  subscribeToActiveState,
  subscribeToWorkspaceSessionEvents,
  subscribeToWorkspaceTabEvents,
  type AgentStatusChangedEvent,
  type UnsubscribeFn,
} from '@/stores/session-events'
import {
  APP_SOCKET_KINDS,
  CARRIED_KINDS,
  SESSION_EVENT_ROUTES,
  __resetUnknownEventKindsForTests,
  isKnownSessionEventKind,
  unknownEventKindCounts,
  type SessionEventRoute,
} from '@/stores/session-event-kinds'

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
const WS_PATH = '/srv/anna'

const routes = SESSION_EVENT_ROUTES as Record<string, SessionEventRoute>
const registryKinds = Object.keys(registry as Record<string, string>).sort()

const cleanups: UnsubscribeFn[] = []
let warn: ReturnType<typeof vi.spyOn>

beforeEach(() => {
  mem.clear()
  FakeWebSocket.instances = []
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  resetGridDialQueueForTests()
  __resetUnknownEventKindsForTests()
  useConnectHostStore.getState().addHost(A)
  useConnectHostStore.getState().addHost(B)
  warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
})

afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()!()
  warn.mockRestore()
})

function unknownWarnings(): unknown[][] {
  return warn.mock.calls.filter((args) => String(args[0]).includes('unknown event kind'))
}

async function newSocket(open: () => UnsubscribeFn, urlPart: string): Promise<FakeWebSocket> {
  const before = FakeWebSocket.instances.length
  cleanups.push(open())
  await vi.waitFor(() => expect(FakeWebSocket.instances.length).toBe(before + 1))
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  expect(ws.url).toContain(urlPart)
  ws.open()
  return ws
}

function push(ws: FakeWebSocket, frame: unknown): void {
  if (!ws.onmessage) throw new Error(`socket ${ws.url} has no onmessage`)
  ws.onmessage({ data: JSON.stringify(frame) })
}

/** The three socket factories on one server, each with no handlers. */
async function threeSockets(scope: ServerScope): Promise<{ workspace: FakeWebSocket; tabs: FakeWebSocket; app: FakeWebSocket }> {
  const workspace = await newSocket(() => subscribeToWorkspaceSessionEvents(scope, WS_PATH, {}), `path=${encodeURIComponent(WS_PATH)}`)
  const tabs = await newSocket(() => subscribeToWorkspaceTabEvents(scope, WS_PATH, {}), `path=${encodeURIComponent(WS_PATH)}`)
  const app = await newSocket(() => subscribeToActiveState(scope), 'path=&')
  return { workspace, tabs, app }
}

describe('the shared registry (Z24)', () => {
  it('the route table names exactly the registry kinds, with the same class', () => {
    expect(Object.keys(routes).sort()).toEqual(registryKinds)
    for (const kind of registryKinds) {
      expect([kind, routes[kind].class]).toEqual([kind, (registry as Record<string, string>)[kind]])
    }
  })

  it('every kind is handled somewhere or ignored on purpose, never both', () => {
    for (const [kind, r] of Object.entries(routes)) {
      const owned = r.workspace === true || r.tabs === true || r.app === true
      const ignored = typeof r.ignored === 'string' && r.ignored.trim().length > 0
      expect([kind, owned || ignored]).toEqual([kind, true])
      expect([kind, owned && ignored]).toEqual([kind, false])
    }
  })

  it('only hello is a handshake; carried kinds are app-level kinds the app socket dispatches', () => {
    const handshakes = Object.entries(routes).filter(([, r]) => r.class === 'handshake').map(([k]) => k)
    expect(handshakes).toEqual(['hello'])
    for (const kind of CARRIED_KINDS) {
      expect([kind, routes[kind].class, APP_SOCKET_KINDS.has(kind)]).toEqual([kind, 'app', true])
    }
    // Z22: a room hears its server's hook status; Q7: mail has a consumer.
    expect(CARRIED_KINDS.has('agent_status_changed')).toBe(true)
    expect(APP_SOCKET_KINDS.has('mail_changed')).toBe(true)
    // A room never opens tabs on another server's say-so.
    expect(CARRIED_KINDS.has('open_url')).toBe(false)
  })

  it('a prototype key is not a kind', () => {
    expect(isKnownSessionEventKind('constructor')).toBe(false)
    expect(isKnownSessionEventKind('toString')).toBe(false)
    expect(isKnownSessionEventKind('agent_status_changed')).toBe(true)
  })
})

describe('the three socket factories (T4.2, T4.3)', () => {
  it('a frame of every registry kind through every socket warns nothing', async () => {
    const { workspace, tabs, app } = await threeSockets(scopeForHost(A))
    for (const kind of registryKinds) {
      for (const ws of [workspace, tabs, app]) push(ws, { kind })
    }
    expect(unknownWarnings()).toEqual([])
    expect(unknownEventKindCounts()).toEqual({})
  })

  it('an unknown kind warns once per page, and every frame is counted', async () => {
    const { workspace, tabs, app } = await threeSockets(scopeForHost(A))
    push(workspace, { kind: 'bogus_kind' })
    push(app, { kind: 'bogus_kind' })
    expect(unknownWarnings()).toHaveLength(1)
    expect(unknownEventKindCounts()).toEqual({ bogus_kind: 2 })
    push(tabs, { kind: 'bogus_kind' })
    push(app, { kind: 'constructor' })
    expect(unknownWarnings()).toHaveLength(2)
    expect(unknownEventKindCounts()).toEqual({ bogus_kind: 3, constructor: 1 })
  })

  it('agent_status_changed on a per-workspace socket is silent (the 0.39.39 drift)', async () => {
    const ws = await newSocket(() => subscribeToWorkspaceSessionEvents(scopeForHost(A), WS_PATH, {}), 'path=%2Fsrv')
    push(ws, { kind: 'agent_status_changed', paneId: 's1', tabId: 's1', status: 'start', workspacePath: WS_PATH })
    expect(unknownWarnings()).toEqual([])
  })
})

describe('agent_status_changed reaches a room through its own server (Z22)', () => {
  it('a carrier on B with no app socket delivers to B’s bus only', async () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const onA: string[] = []
    const onB: AgentStatusChangedEvent[] = []
    cleanups.push(onAgentStatusChanged(a, (e) => onA.push(e.paneId)))
    cleanups.push(onAgentStatusChanged(b, (e) => onB.push(e)))
    const carrier = await newSocket(
      () => subscribeToWorkspaceSessionEvents(b, WS_PATH, { carryAppBus: true }),
      'wss://z3thon.k2.dev/cli/sessions/events?path=%2Fsrv%2Fanna',
    )
    const frame = { kind: 'agent_status_changed', paneId: 'sid-1', tabId: 'sid-1', status: 'permission', workspacePath: WS_PATH }
    push(carrier, frame)
    expect(onB).toEqual([frame])
    expect(onA).toEqual([])
  })
})

describe('mail_changed (Q7)', () => {
  it('the app socket fires onMailChanged with the reason, on that server only', async () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const onA: string[] = []
    const onB: string[] = []
    cleanups.push(onMailChanged(a, (r) => onA.push(r)))
    cleanups.push(onMailChanged(b, (r) => onB.push(r)))
    const app = await newSocket(() => subscribeToActiveState(a), 'wss://rosson.k2.dev/cli/sessions/events?path=&')
    push(app, { kind: 'mail_changed', reason: 'send-approval-requested' })
    push(app, { kind: 'mail_changed', reason: 'send-decided' })
    expect(onA).toEqual(['send-approval-requested', 'send-decided'])
    expect(onB).toEqual([])
  })
})
