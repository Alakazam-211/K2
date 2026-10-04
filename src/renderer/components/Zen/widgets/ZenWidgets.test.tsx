// @vitest-environment jsdom
//
// prd-zen-mode-v1 S6 — the built-in Agents and Conversation widgets and the
// texting template's controls, through the real ZenHost, Zen root, page,
// bridge, data verbs, Home rooms logic (`createHomeRooms`), overlay Thread
// hook and compose send. Only the edges are faked: the daemons
// (`daemon-cli`, keyed by each request's server), sockets, Tauri, the
// connection pool's network, file uploads, and layout.
//
// Asserted (fail loudly):
//   - rows follow the Home's order and ⌘1–9 selects row N;
//   - each row's live status updates (window's server, an open pinned room,
//     the pool), and a server that can't say shows none;
//   - previews come from one `thread/latest` per server (an older server:
//     `thread?limit=1`, never 0);
//   - a remote row opens in place even with "Open agents from other servers
//     here" off: no window switch;
//   - the conversation reads and posts through the bridge on the agent's
//     own server, at the Thread address the Agents page resolves;
//   - attachments go through the existing attach path (local paths, or an
//     upload to the agent's server);
//   - "Open in Agents" shows for a permission prompt;
//   - the template's own controls pass the required-controls check.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

type Call = { method: 'GET' | 'POST'; hostKey: string; route: string; data: Record<string, unknown> }
type FakeWs = {
  url: string
  hostKey: string
  readyState: number
  onmessage: ((ev: { data: string }) => void) | null
  onerror: unknown
  onopen: unknown
  close(): void
}

const h = vi.hoisted(() => ({
  calls: [] as Call[],
  threads: {} as Record<string, unknown[]>,
  latest: {} as Record<string, { preview: string; seq: number; via?: string }>,
  pageCaps: null as null | Record<string, string[]>,
  sockets: [] as FakeWs[],
  picked: ['/Users/me/shot.png'] as string[],
  remoteDrops: [] as Array<{ hostKey: string; paths: string[]; workspacePath: string | undefined }>,
  switched: [] as unknown[],
  keepAlive: [] as Array<{ hostKey: string; projectId: string }>,
  rooms: [] as Array<{ key: string; activity: { applyHookStatus(id: string, s: string): void } }>,
  seq: 100,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => (cmd === 'pick_local_files' ? h.picked : null)),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: 'main',
    startDragging: async () => undefined,
    isMaximized: async () => false,
    maximize: async () => undefined,
    unmaximize: async () => undefined,
    minimize: async () => undefined,
    close: async () => undefined,
    listen: async () => () => undefined,
  }),
}))

function threadKey(hostKey: string, addr: string): string {
  return `${hostKey}|${addr}`
}

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: Record<string, unknown>) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params ?? {} })
    const p = params ?? {}
    if (route === 'zen/get') {
      const caps = h.pageCaps ?? {
        agents: ['agents:read', 'presence:read'],
        conversation: ['agents:read', 'presence:read', 'thread:read', 'thread:post'],
      }
      return {
        ok: true,
        schema: 1,
        version: 'v1',
        page: {
          template: 'k2.texting@1',
          layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
          widgets: [
            { id: 'agents', kind: 'agents', column: 0, props: {}, caps: caps.agents, source: 'builtin' },
            { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: caps.conversation, source: 'builtin' },
          ],
          controls: ['zen-toggle', 'home-switcher', 'drag-region'],
        },
        theme: {},
        chrome: {},
        motion: {},
        errors: [],
        warnings: [],
        lastGoodAt: null,
      }
    }
    if (route === 'thread/latest') {
      const addrs = String(p.addrs).split(',')
      return {
        ok: true,
        items: addrs.map((addr) => {
          const l = h.latest[threadKey(scope.hostKey, addr)]
          return l
            ? { addr, conversationId: `c-${addr}`, seq: l.seq, at: 1_000, from: 'agent', via: l.via ?? null, kind: 'text', preview: l.preview }
            : { addr, conversationId: `c-${addr}`, seq: null, at: null, from: null, via: null, kind: null, preview: null }
        }),
      }
    }
    if (route === 'sessions/list-for-workspace') {
      // The Agents page resolver: the pinned Chat's durable address, which
      // is NOT the row handle.
      return scope.hostKey === 'local'
        ? [{ kind: 'canonical', handle: 'cortana-main/chat', agentName: 'p1' }]
        : [{ kind: 'canonical', handle: 'sales-desk/chat', agentName: 'bp1' }]
    }
    if (route === 'thread') {
      const addr = String(p.addr)
      const items = h.threads[threadKey(scope.hostKey, addr)] ?? []
      const limit = Number(p.limit)
      return { conversation_id: `c-${addr}`, has_more: false, items: limit > 0 ? items.slice(-limit) : items }
    }
    throw new Error(`unexpected GET ${route} on ${scope.hostKey}`)
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: Record<string, unknown>) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body ?? {} })
    if (route === 'thread/post') {
      const addr = String(body?.addr)
      h.seq += 1
      const item = {
        collection: 'thread',
        seq: h.seq,
        id: `m${h.seq}`,
        doc: { id: `m${h.seq}`, kind: 'text', from: 'you', body: String(body?.text), via: 'compose', created_at: 2_000 },
      }
      const k = threadKey(scope.hostKey, addr)
      h.threads[k] = [...(h.threads[k] ?? []), item]
      return { ok: true, id: item.id, seq: item.seq, from: 'you', body: item.doc.body, kind: 'text', via: 'compose', conversation_id: `c-${addr}` }
    }
    if (route === 'thread/answer') return { ok: true, id: body?.id, status: 'answered', answer: body?.answer }
    if (route === 'zen/page/ensure' || route === 'zen/homes/sync') return { ok: true }
    throw new Error(`unexpected POST ${route} on ${scope.hostKey}`)
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return { ...mod, onZenChanged: () => () => {}, subscribeToActiveState: () => () => {} }
})
vi.mock('@/kessel/daemon-ws', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/kessel/daemon-ws')>()
  return {
    ...mod,
    getDaemonWs: async () => ({ state: 'available', host: '127.0.0.1', port: 1, token: 't', secure: false }),
    daemonWsBase: () => 'ws://test',
  }
})
vi.mock('@/lib/grid-dial-queue', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/grid-dial-queue')>()
  return {
    ...mod,
    openQueuedWebSocket: async (scope: { hostKey: string }, url: string) => {
      const ws: FakeWs = { url, hostKey: scope.hostKey, readyState: 1, onmessage: null, onerror: null, onopen: null, close() {} }
      h.sockets.push(ws)
      return ws
    },
  }
})
vi.mock('@/lib/host-pool-instance', async () => {
  const { createStore } = await import('zustand/vanilla')
  const store = createStore<{ entries: Record<string, unknown> }>(() => ({ entries: {} }))
  return {
    hostPool: {
      store,
      entry: (k: string) => store.getState().entries[k],
      check: vi.fn(async (k: string) => store.getState().entries[k]),
      keepRoomAlive: vi.fn(async (hostKey: string, projectId: string) => {
        h.keepAlive.push({ hostKey, projectId })
        return 'sent'
      }),
    },
    installHostSessionSync: () => () => {},
    signOutOfHost: async () => {},
  }
})
vi.mock('@/stores/home-rooms', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/home-rooms')>()
  const { roomTiers } = await import('@/lib/room-tiers')
  const { scopeForHost } = await import('@/kessel/server-scope')
  const { createRoomActivity } = await import('@/stores/room')
  const { createStore } = await import('zustand/vanilla')
  const remoteProjects: Record<string, unknown[]> = {
    'akzm.k2.dev': [
      { id: 'bp1', name: 'sales', handle: 'sales', path: '/b/sales', color: '#888', agentMode: 'off', workspaces: [{ id: 'bw1', type: 'main', name: 'main' }] },
    ],
  }
  const rooms = mod.createHomeRooms({
    tiers: roomTiers,
    createRoom: (input) => {
      const projects = createStore(() => ({ projects: (remoteProjects[input.scope.hostKey] ?? []) as never[] }))
      const activity = createRoomActivity(projects, input.workspace.projectId)
      const key = `${input.scope.hostKey}|${input.workspace.projectId}:${input.workspace.workspaceId}`
      h.rooms.push({ key, activity })
      return {
        key,
        isPrimary: false,
        scope: input.scope,
        readOnly: false,
        activityView: activity,
        activity,
        cwd: () => input.workspace.path,
        activeProjectId: () => input.workspace.projectId,
        roomId: () => key,
        tabs: { room: { open: async () => {}, ensurePinnedAgentTabForMode: () => {} } },
        dispose: async () => {},
      } as never
    },
    listProjects: async (scope) => (remoteProjects[scope.hostKey] ?? []) as never,
    knowServer: async () => {},
    keepAlive: async (hostKey, projectId) => {
      h.keepAlive.push({ hostKey, projectId })
      return 'sent'
    },
    scopeFor: (hostKey) => scopeForHost(hostKey),
    layoutRevisionSupported: async () => true,
    onProjectsChanged: () => () => {},
    floorCheck: () => null,
    belowFloor: (row) => void h.switched.push(row),
    promote: () => {},
    now: () => Date.now(),
    setInterval: (fn, ms) => setInterval(fn, ms),
    clearInterval: (hd) => clearInterval(hd as ReturnType<typeof setInterval>),
  })
  return { ...mod, homeRooms: rooms, useHomeRoomsStore: rooms.store, useShownHomeRoom: () => rooms.store((s) => s.shown) }
})
vi.mock('@/lib/home-switch', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/home-switch')>()
  return {
    ...mod,
    switchWindowToRow: vi.fn((row: unknown) => {
      h.switched.push(row)
      return 'switching'
    }),
    toastOldServerOnce: vi.fn(),
  }
})
vi.mock('@/lib/handle-remote-drop', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/handle-remote-drop')>()
  return {
    ...mod,
    executeRemoteDrop: vi.fn(
      async (
        scope: { hostKey: string },
        paths: string[],
        _target: unknown,
        ctx: { workspacePath?: string },
        build: (p: string[]) => string,
      ) => {
        h.remoteDrops.push({ hostKey: scope.hostKey, paths, workspacePath: ctx.workspacePath })
        return build(paths.map((p) => `${ctx.workspacePath}/.k2/downloads/${p.split('/').pop()}`))
      },
    ),
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { hostPool } from '@/lib/host-pool-instance'
import { homeRooms } from '@/stores/home-rooms'
import { noteServerVersion } from '@/kessel/server-scope'
import { useZenHomesStore } from '@/lib/zen/zen-homes'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __setZenGeometryForTests, runZenControlChecksNow } from '@/lib/zen/zen-monitor'
import { useZenViewStore } from '@/lib/zen/zen-view'
import { __resetZenDataForTests } from '@/lib/zen/zen-data'
import type { ZenGeometry } from '@/lib/zen/zen-controls'
import { useWorkspaceIndexShortcuts } from '@/hooks/useWorkspaceIndexShortcuts'
import { ZenHost } from '../ZenHost'
import { installZenBuiltins } from './builtins'
import { __resetZenDraftsForTests } from './ZenCompose'
import { zenEmptyThreadText, zenPermissionText } from './ZenConversationWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const B = 'akzm.k2.dev'
const C = 'scout.k2.dev'
const D = 'old.k2.dev'
const ROWS = {
  cortana: { address: 'cortana::local', workspaceId: 'p1', label: 'cortana' },
  sales: { address: `sales::${B}`, workspaceId: 'bp1', label: 'sales' },
  julie: { address: `julie::${C}`, workspaceId: 'cp1', label: 'julie' },
  ops: { address: `ops::${D}`, workspaceId: 'dp1', label: 'ops' },
}

function host(id: string, label: string, hostname: string): ConnectHost {
  return { id, label, hostname, port: 443, secure: true, username: 'anna', token: `tok-${id}` } as ConnectHost
}

function poolEntry(
  hostKey: string,
  over: { role?: string; version?: string; features?: string[]; activity?: unknown } = {},
): Record<string, unknown> {
  const e: Record<string, unknown> = {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: {
      phase: 'ready',
      ready: true,
      version: over.version ?? '0.43.4',
      protocol: 1,
      instanceId: `i-${hostKey}`,
      features: over.features ?? ['thread-latest'],
      at: 0,
    },
    auth: 'ok',
    authNote: null,
    role: over.role ?? 'member',
    presence: [],
    offlineStreak: 0,
    checkedAt: 0,
  }
  if ('activity' in over) e.activity = over.activity
  else e.activity = []
  return e
}

function setPool(entries: Record<string, Record<string, unknown>>): void {
  ;(hostPool.store as unknown as { setState(s: unknown): void }).setState({ entries })
}

let lastRected: Element | null = null
const fakeGeometry: ZenGeometry = {
  rect(el) {
    lastRected = el
    if (el.hasAttribute('data-zen-drag')) return { left: 300, top: 8, width: 500, height: 28 }
    return { left: 120, top: 8, width: 80, height: 28 }
  },
  style() {
    return { display: 'block', visibility: 'visible', opacity: 1 }
  },
  viewport() {
    return { width: 1200, height: 800 }
  },
  elementFromPoint() {
    return lastRected
  },
}

let uninstall: (() => void) | null = null

beforeEach(() => {
  h.calls.length = 0
  h.threads = {
    [threadKey('local', 'cortana-main')]: [
      { collection: 'thread', seq: 1, id: 'a1', doc: { id: 'a1', kind: 'text', from: 'cortana', body: 'Pushed the fix.', created_at: 900 } },
      {
        collection: 'thread',
        seq: 2,
        id: 'a2',
        doc: {
          id: 'a2',
          kind: 'choice',
          from: 'cortana',
          body: 'Want me to ship it?',
          created_at: 950,
          choice: { prompt: 'Want me to ship it?', options: [{ label: 'Ship' }, { label: 'Wait' }], allow_custom: true, status: 'pending' },
        },
      },
    ],
    [threadKey(B, 'sales-desk')]: [
      { collection: 'thread', seq: 7, id: 'b7', doc: { id: 'b7', kind: 'text', from: 'sales', body: 'Draft is in your inbox.', created_at: 990 } },
    ],
  }
  h.latest = {
    [threadKey('local', 'cortana')]: { preview: 'Asked: Want me to ship it?', seq: 2 },
    [threadKey(B, 'sales')]: { preview: 'Draft is in your inbox.', seq: 7 },
  }
  h.pageCaps = null
  h.sockets.length = 0
  h.remoteDrops.length = 0
  h.switched.length = 0
  h.keepAlive.length = 0
  h.rooms.length = 0
  h.seq = 100
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  __setZenGeometryForTests(fakeGeometry)
  __resetZenApiForTests()
  __resetZenDataForTests()
  __resetZenDraftsForTests()
  localStorage.clear()
  // "Open agents from other servers here" OFF: Zen still opens in place.
  useRemoteRoomsPreviewStore.getState().setEnabled(false)
  useConnectHostStore.setState({
    activeHost: 'local',
    hosts: [host('b', 'akzm', B), host('c', 'scout', C), host('d', 'old box', D)],
    connectionStatus: 'connected',
  })
  useProjectsStore.setState({
    projects: [
      { id: 'p1', name: 'cortana', handle: 'cortana', path: '/w/cortana', color: '#c2662d', workspaces: [{ id: 'w1', type: 'main', name: 'main' }] },
    ] as never,
    activeProjectId: null,
    // The window's own room selects the workspace (its project switch is
    // tested elsewhere).
    setActiveProject: (id: string | null) => useProjectsStore.setState({ activeProjectId: id }),
  })
  useActiveAgentsStore.setState({ paneStatuses: new Map(), daemonPaneStatuses: new Map(), paneProjectMap: new Map() })
  useHomesStore.setState({
    homes: [
      { id: 'h1', name: 'Work', rows: [ROWS.cortana, ROWS.sales, ROWS.julie, ROWS.ops] },
      { id: 'h2', name: 'Personal', rows: [] },
    ],
    selectedId: 'h1',
  })
  setPool({
    [B]: poolEntry(B),
    // No access: a role this server's rooms refuse.
    [C]: poolEntry(C, { role: 'viewer' }),
    // An older server: no `thread-latest`, no activity in its summary.
    [D]: poolEntry(D, { version: '0.43.0', features: [], activity: undefined }),
  })
  noteServerVersion(B, '0.43.4', ['thread-latest'])
  noteServerVersion(C, '0.43.4', ['thread-latest'])
  noteServerVersion(D, '0.43.0', [])
  useZenHomesStore.setState({ on: { h1: true } })
  useZenViewStore.setState({ safe: null, epoch: 0 })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.getState().setPage('home')
  uninstall = installZenBuiltins()
})

afterEach(async () => {
  cleanup()
  uninstall?.()
  uninstall = null
  __resetZenDataForTests()
  __setZenGeometryForTests(null)
  await homeRooms.closeAll()
})

function Shortcuts(): null {
  useWorkspaceIndexShortcuts()
  return null
}

async function mountZen(): Promise<void> {
  render(
    <>
      <Shortcuts />
      <ZenHost />
    </>,
  )
  await waitFor(() => {
    if (document.querySelectorAll('[data-zen-agent-row]').length !== 4) throw new Error('rows not drawn')
  })
}

function rowEl(address: string): HTMLElement {
  const el = document.querySelector(`[data-zen-agent-row="${address}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no row ${address}`)
  return el
}

function rowAddresses(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-agent-row]')).map((e) => e.getAttribute('data-zen-agent-row') ?? '')
}

async function select(address: string): Promise<void> {
  await act(async () => {
    fireEvent.click(rowEl(address))
  })
  await waitFor(() => {
    if (!document.querySelector(`[data-zen-conversation="${address}"]`)) throw new Error('conversation not shown')
  })
}

async function threadReady(address: string): Promise<void> {
  await waitFor(() => {
    const conv = document.querySelector(`[data-zen-conversation="${address}"]`)
    if (!conv || conv.querySelector('[data-zen-conversation-note]')) throw new Error('conversation not ready')
    if (conv.querySelectorAll('[data-zen-message]').length === 0 && !conv.querySelector('[data-zen-thread-empty]')) {
      throw new Error('thread not loaded')
    }
  })
}

function composeInput(): HTMLTextAreaElement {
  const el = document.querySelector('[data-zen-compose-input]')
  if (!(el instanceof HTMLTextAreaElement)) throw new Error('no compose box')
  return el
}

async function typeAndSend(text: string): Promise<void> {
  await act(async () => {
    fireEvent.change(composeInput(), { target: { value: text } })
  })
  await act(async () => {
    fireEvent.keyDown(composeInput(), { key: 'Enter' })
  })
}

describe('Agents widget', () => {
  it('rows follow the Home’s order, and ⌘1–9 selects row N in place', async () => {
    await mountZen()
    expect(rowAddresses()).toEqual([ROWS.cortana.address, ROWS.sales.address, ROWS.julie.address, ROWS.ops.address])

    // Reorder the Home: Zen follows.
    act(() => useHomesStore.getState().moveRow('h1', 1, 0))
    expect(rowAddresses()).toEqual([ROWS.sales.address, ROWS.cortana.address, ROWS.julie.address, ROWS.ops.address])

    // ⌘2 is the Home's row 2 (cortana now).
    await act(async () => {
      fireEvent.keyDown(window, { key: '2', code: 'Digit2', metaKey: true })
    })
    await waitFor(() => expect(rowEl(ROWS.cortana.address).hasAttribute('data-selected')).toBe(true))
    expect(document.querySelector('[data-zen-conversation-title]')?.textContent).toBe('cortana')

    // ⌘1 is the remote row: opened in place, never a window switch.
    await act(async () => {
      fireEvent.keyDown(window, { key: '1', code: 'Digit1', metaKey: true })
    })
    await waitFor(() => expect(rowEl(ROWS.sales.address).hasAttribute('data-selected')).toBe(true))
    expect(h.switched).toEqual([])
    expect(useConnectHostStore.getState().activeHost).toBe('local')

    // An empty Home says so.
    act(() => {
      useZenHomesStore.setState({ on: { h1: true, h2: true } })
      useHomesStore.getState().selectHome('h2')
    })
    await waitFor(() =>
      expect(document.querySelector('[data-zen-agents-empty]')?.textContent).toBe(
        'This Home has no agents yet. Exit Zen to add some.',
      ),
    )
  })

  it('live status: the window’s server, an open pinned room, and the pool; a server that can’t say shows none', async () => {
    await mountZen()
    // Window's server, from active-agents.
    expect(rowEl(ROWS.cortana.address).getAttribute('data-activity')).toBe('idle')
    act(() =>
      useActiveAgentsStore.setState({
        paneStatuses: new Map([['pane1', 'working']]),
        paneProjectMap: new Map([['pane1', 'p1']]),
      }),
    )
    expect(rowEl(ROWS.cortana.address).getAttribute('data-activity')).toBe('working')
    expect(rowEl(ROWS.cortana.address).querySelector('[data-zen-activity]')?.textContent).toBe('working')
    act(() => useActiveAgentsStore.setState({ paneStatuses: new Map([['pane1', 'permission']]) }))
    expect(rowEl(ROWS.cortana.address).getAttribute('data-activity')).toBe('needs-you')

    // Another server, closed row: the pool's summary activity.
    expect(rowEl(ROWS.sales.address).getAttribute('data-activity')).toBe('idle')
    act(() => setPool({ [B]: poolEntry(B, { activity: [{ workspaceId: 'bp1', status: 'working' }] }), [C]: poolEntry(C, { role: 'viewer' }), [D]: poolEntry(D, { version: '0.43.0', features: [], activity: undefined }) }))
    expect(rowEl(ROWS.sales.address).getAttribute('data-activity')).toBe('working')

    // Open the remote room: its own slice (agent_status_changed) now wins.
    await select(ROWS.sales.address)
    expect(h.rooms.length).toBe(1)
    expect(rowEl(ROWS.sales.address).getAttribute('data-activity')).toBe('idle')
    act(() => h.rooms[0].activity.applyHookStatus('sess-1', 'permission'))
    expect(rowEl(ROWS.sales.address).getAttribute('data-activity')).toBe('needs-you')
    act(() => h.rooms[0].activity.applyHookStatus('sess-1', 'stop'))
    expect(rowEl(ROWS.sales.address).getAttribute('data-activity')).toBe('idle')

    // A server whose summary has no activity (before 0.43.2): no status.
    expect(rowEl(ROWS.ops.address).getAttribute('data-activity')).toBe('unknown')
    expect(rowEl(ROWS.ops.address).querySelector('[data-zen-activity]')).toBeNull()
    expect(rowEl(ROWS.ops.address).querySelector('[data-zen-status-dot]')).toBeNull()

    // No access: says so, no status.
    expect(rowEl(ROWS.julie.address).getAttribute('data-state')).toBe('no-access')
    expect(rowEl(ROWS.julie.address).textContent).toContain('No access')
  })

  it('previews come from one thread/latest per server; an older server gets limit=1 per row', async () => {
    await mountZen()
    await waitFor(() =>
      expect(rowEl(ROWS.sales.address).querySelector('[data-zen-preview]')?.textContent).toBe('Draft is in your inbox.'),
    )
    expect(rowEl(ROWS.cortana.address).querySelector('[data-zen-preview]')?.textContent).toBe('Asked: Want me to ship it?')
    const latest = h.calls.filter((c) => c.route === 'thread/latest')
    expect(latest.map((c) => [c.hostKey, c.data.addrs]).sort()).toEqual([
      [B, 'sales'],
      ['local', 'cortana'],
    ])
    // The older server: one newest item, never limit=0. No-access: nothing.
    const fallback = h.calls.filter((c) => c.route === 'thread')
    expect(fallback.map((c) => [c.hostKey, c.data])).toEqual([[D, { addr: 'ops', limit: 1 }]])
    expect(h.calls.some((c) => c.hostKey === C)).toBe(false)
    // No unread tracking (answer 9).
    expect(document.querySelector('[data-zen-unread]')).toBeNull()
  })
})

describe('Conversation widget', () => {
  it('a remote row opens in place with "Open agents from other servers here" off', async () => {
    expect(useRemoteRoomsPreviewStore.getState().enabled).toBe(false)
    await mountZen()
    await select(ROWS.sales.address)
    await threadReady(ROWS.sales.address)
    expect(h.switched).toEqual([])
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(homeRooms.store.getState().shown).toBe(ROWS.sales.address)
    // One keep-alive to that server for the open (Z37).
    expect(h.keepAlive).toEqual([{ hostKey: B, projectId: 'bp1' }])
    // The Thread is read on B, at the address the Agents page resolves.
    const reads = h.calls.filter((c) => c.route === 'thread' && c.hostKey === B)
    expect(reads.map((c) => c.data.addr)).toEqual(['sales-desk'])
    expect(h.sockets.map((s) => s.hostKey)).toEqual([B])
    expect(document.querySelector('[data-zen-message="b7"]')?.textContent).toContain('Draft is in your inbox.')
    // Header names the server.
    expect(document.querySelector('[data-zen-conversation]')?.textContent).toContain('akzm')
  })

  it('reads and posts through the bridge on the agent’s own server; live frames and answers land', async () => {
    await mountZen()
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    expect(useProjectsStore.getState().activeProjectId).toBe('p1')
    // vs-live Z52: the resolved pinned Chat address, not the row handle.
    expect(h.calls.filter((c) => c.route === 'thread' && c.hostKey === 'local').map((c) => [c.hostKey, c.data.addr])).toEqual([
      ['local', 'cortana-main'],
    ])
    const agentMsg = document.querySelector('[data-zen-message="a1"]')
    expect(agentMsg?.hasAttribute('data-mine')).toBe(false)
    expect(document.querySelector('[data-zen-message="a2"] [data-testid="thread-choice-card"]')).not.toBeNull()

    await typeAndSend('Ship it.')
    await waitFor(() => expect(document.querySelectorAll('[data-zen-message][data-mine]').length).toBe(1))
    const posts = h.calls.filter((c) => c.route === 'thread/post')
    expect(posts.map((c) => [c.hostKey, c.data])).toEqual([['local', { addr: 'cortana-main', text: 'Ship it.', via: 'compose' }]])
    expect(composeInput().value).toBe('')
    // The row's preview follows the open conversation at once.
    expect(rowEl(ROWS.cortana.address).querySelector('[data-zen-preview]')?.textContent).toBe('You: Ship it.')

    // A live overlay frame from the agent lands in the conversation.
    const live = h.sockets.filter((s) => s.hostKey === 'local' && s.onmessage !== null)
    expect(live.length).toBe(1)
    const ws = live[0]
    if (!ws.onmessage) throw new Error('no overlay socket')
    act(() =>
      ws.onmessage?.({
        data: JSON.stringify({ collection: 'thread', seq: 300, id: 'a3', doc: { id: 'a3', kind: 'text', from: 'cortana', body: 'Shipped.' } }),
      }),
    )
    await waitFor(() => expect(document.querySelector('[data-zen-message="a3"]')?.textContent).toContain('Shipped.'))

    // A tap on a choice answers through the bridge.
    const chip = document.querySelector('[data-zen-message="a2"] [data-testid="thread-choice-chip"][data-label="Ship"]')
    if (!(chip instanceof HTMLElement)) throw new Error('no Ship chip')
    await act(async () => {
      fireEvent.click(chip)
    })
    expect(h.calls.filter((c) => c.route === 'thread/answer').map((c) => [c.hostKey, c.data])).toEqual([
      ['local', { addr: 'cortana-main', id: 'a2', answer: 'Ship' }],
    ])
  })

  it('a widget without thread:post is refused by the bridge (built-ins go through it too)', async () => {
    h.pageCaps = { agents: ['agents:read'], conversation: ['agents:read', 'thread:read'] }
    await mountZen()
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    await typeAndSend('hello')
    await waitFor(() => expect(document.querySelector('[data-zen-compose-error]')?.textContent).toContain('cap_not_granted'))
    expect(h.calls.some((c) => c.route === 'thread/post')).toBe(false)
    // The text came back.
    expect(composeInput().value).toBe('hello')
  })

  it('an empty Thread invites the first message', async () => {
    h.threads[threadKey('local', 'cortana-main')] = []
    await mountZen()
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    expect(document.querySelector('[data-zen-thread-empty]')?.textContent).toBe(zenEmptyThreadText('cortana'))
  })

  it('attachments use the existing path: an upload to the agent’s own server, or this computer’s paths', async () => {
    await mountZen()
    // Remote: uploaded into that workspace's .k2/downloads on B, then sent.
    await select(ROWS.sales.address)
    await threadReady(ROWS.sales.address)
    const attach = document.querySelector('[data-zen-attach]')
    if (!(attach instanceof HTMLElement)) throw new Error('no attach button')
    await act(async () => {
      fireEvent.click(attach)
    })
    await waitFor(() => expect(document.querySelector('[data-zen-attachment="shot.png"]')).not.toBeNull())
    await typeAndSend('see this')
    await waitFor(() => expect(h.calls.filter((c) => c.route === 'thread/post').length).toBe(1))
    expect(h.remoteDrops).toEqual([{ hostKey: B, paths: ['/Users/me/shot.png'], workspacePath: '/b/sales' }])
    expect(h.calls.filter((c) => c.route === 'thread/post').map((c) => [c.hostKey, c.data])).toEqual([
      [B, { addr: 'sales-desk', text: 'see this\n/b/sales/.k2/downloads/shot.png', via: 'compose' }],
    ])
    expect(document.querySelector('[data-zen-attachment]')).toBeNull()

    // Local: this computer's path as it is, no upload.
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    const attach2 = document.querySelector('[data-zen-attach]')
    if (!(attach2 instanceof HTMLElement)) throw new Error('no attach button')
    await act(async () => {
      fireEvent.click(attach2)
    })
    await waitFor(() => expect(document.querySelector('[data-zen-attachment="shot.png"]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-send]') as HTMLElement)
    })
    await waitFor(() => expect(h.calls.filter((c) => c.route === 'thread/post').length).toBe(2))
    expect(h.remoteDrops.length).toBe(1)
    expect(h.calls.filter((c) => c.route === 'thread/post')[1].data).toEqual({
      addr: 'cortana-main',
      text: '/Users/me/shot.png',
      via: 'compose',
    })
  })

  it('“Open in Agents” shows for a permission prompt, and opens the agent without switching the window', async () => {
    await mountZen()
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    expect(document.querySelector('[data-zen-permission]')).toBeNull()
    act(() =>
      useActiveAgentsStore.setState({
        paneStatuses: new Map([['pane1', 'permission']]),
        paneProjectMap: new Map([['pane1', 'p1']]),
      }),
    )
    const banner = document.querySelector('[data-zen-permission]')
    expect(banner?.textContent).toContain(zenPermissionText('cortana'))
    await act(async () => {
      fireEvent.click(banner?.querySelector('[data-zen-open-in-agents]') as HTMLElement)
    })
    // The window's own server: the Agents page, that workspace; Zen stays on.
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useProjectsStore.getState().activeProjectId).toBe('p1')
    expect(useZenHomesStore.getState().on.h1).toBe(true)
    expect(h.switched).toEqual([])
  })

  it('“Open in Agents” for a remote agent shows its room on Home, in place', async () => {
    await mountZen()
    await select(ROWS.sales.address)
    await threadReady(ROWS.sales.address)
    act(() => h.rooms[0].activity.applyHookStatus('sess-1', 'permission'))
    const button = document.querySelector('[data-zen-permission] [data-zen-open-in-agents]')
    if (!(button instanceof HTMLElement)) throw new Error('no Open in Agents')
    await act(async () => {
      fireEvent.click(button)
    })
    await waitFor(() => expect(useZenHomesStore.getState().on.h1).toBeUndefined())
    expect(usePageViewStore.getState().page).toBe('home')
    expect(homeRooms.store.getState().shown).toBe(ROWS.sales.address)
    expect(h.switched).toEqual([])
    expect(useConnectHostStore.getState().activeHost).toBe('local')
  })

  it('a row that can’t be messaged says why and offers no box', async () => {
    await mountZen()
    await select(ROWS.julie.address)
    const note = document.querySelector('[data-zen-conversation-note]')
    expect(note?.textContent).toContain('Your login on scout can’t message agents.')
    expect(composeInput().disabled).toBe(true)
    expect(h.calls.some((c) => c.hostKey === C)).toBe(false)
  })
})

describe('texting template controls', () => {
  it('draws its own Home switcher, Zen toggle and drag area, and they pass the required-controls check', async () => {
    await mountZen()
    const bar = document.querySelector('[data-zen-texting-controls]')
    if (!bar) throw new Error('the S6 template controls are not registered')
    expect(bar.querySelector('[data-zen-home-pill]')?.getAttribute('data-zen-bound')).toBe('home-switcher')
    expect(bar.querySelector('[data-zen-switch]')?.getAttribute('data-zen-bound')).toBe('zen-toggle')
    expect(bar.querySelector('[data-zen-drag]')?.getAttribute('data-zen-bound')).toBe('drag-region')

    // Wired: activating the switcher binds an option for every Home in 1 s.
    await act(async () => {
      fireEvent.click(bar.querySelector('[data-zen-home-pill]') as HTMLElement)
    })
    const options = Array.from(document.querySelectorAll('[data-zen-home-option]'))
    expect(options.map((o) => [o.getAttribute('data-zen-home-option'), o.getAttribute('data-zen-bound')])).toEqual([
      ['h1', 'home-option'],
      ['h2', 'home-option'],
    ])
    expect(options[1].textContent).toContain('⌥⌘2')
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1100))
    })
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()

    // A Home option switches Homes (steps away; Zen stays on for h1).
    await act(async () => {
      fireEvent.click(options[1] as HTMLElement)
    })
    expect(selectedHome(useHomesStore.getState()).id).toBe('h2')
    expect(document.querySelector('[data-zen-home-menu]')).toBeNull()
    expect(useZenHomesStore.getState().on.h1).toBe(true)
    act(() => useHomesStore.getState().selectHome('h1'))

    // The Zen toggle exits: this Home off.
    await waitFor(() => expect(document.querySelector('[data-zen-switch]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-switch]') as HTMLElement)
    })
    expect(useZenHomesStore.getState().on.h1).toBeUndefined()
  }, 10_000)
})

describe('S6 source ratchets', () => {
  const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')
  const read = (p: string): string => readFileSync(join(RENDERER, p), 'utf8')

  it('T6.3: the Agents page compose bar and Zen send through the same helpers', () => {
    const bar = read('components/Terminal/TerminalComposeBar.tsx')
    expect(bar).toContain("import { postThreadCompose } from '@/components/SessionView/overlayThread'")
    expect(bar).toContain("import { composeAttachPayload } from '@/lib/compose-attach'")
    expect(bar).not.toContain("'thread/post'")
    const data = read('lib/zen/zen-data.ts')
    expect(data).toContain('postThreadCompose(c.scope, c.threadAddr, body)')
    expect(data).toContain('composeAttachPayload(c.scope, {')
  })

  it('widgets never hold a scope or a daemon call: only the bridge', () => {
    for (const f of ['ZenAgentsWidget.tsx', 'ZenConversationWidget.tsx', 'ZenCompose.tsx', 'ZenTextingControls.tsx', 'zen-widget-kit.tsx']) {
      const src = read(`components/Zen/widgets/${f}`)
      expect([f, /daemonCli|scopeForHost|primaryScope|primaryRoom|useOverlayThread|ServerScope/.test(src)]).toEqual([f, false])
    }
  })
})
