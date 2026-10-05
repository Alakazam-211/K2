// @vitest-environment jsdom
//
// prd-zen-mode-v1 S6 and prd-zen-gardens-v1 S4–S6 — the built-in Agents,
// Conversation and empty-Garden widgets and the templates' controls,
// through the real ZenHost, Zen root, page,
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
//   - the template's own controls pass the required-controls check, all in
//     the top band: Garden switcher top left, then the theme control
//     immediately left of the Zen toggle in the top-right corner;
//   - Add agent is the Agents widget's last row (bottom left, inside the
//     list), not a page control;
//   - Add agent opens the Home's shared picker (`agents.add`), adds to the
//     Home the widget shows, the row shows in the Agents widget at once, and
//     the picker closes on Esc, the widget's Home changing and leaving Zen;
//   - the Agents widget's own Home picker (G27): its rows follow the pick,
//     the Home page's selection never moves, the pick survives a remount and
//     reaches another window; the Garden's seed Home comes first;
//   - single-agent and whole-Home modes, and a Garden's own layout (G38);
//   - ⌘⌥1–9 switches Gardens in Zen and workspaces outside it (G26);
//   - the empty Garden's Ask my agent (G28): only this computer's agents,
//     the draft in the box, nothing posted until you send.

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
  gardens: [] as Array<{ id: string; name: string; template: string; seedHome?: string }>,
  pages: {} as Record<string, { layout: unknown; widgets: unknown[]; template: string }>,
  localProjects: null as null | unknown[],
  sockets: [] as FakeWs[],
  picked: ['/Users/me/shot.png'] as string[],
  remoteDrops: [] as Array<{ hostKey: string; paths: string[]; workspacePath: string | undefined }>,
  switched: [] as unknown[],
  keepAlive: [] as Array<{ hostKey: string; projectId: string }>,
  rooms: [] as Array<{ key: string; activity: { applyHookStatus(id: string, s: string): void } }>,
  seq: 100,
  tickets: [] as Array<Record<string, unknown>>,
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
    if (route === 'zen/gardens') {
      return { ok: true, setUp: true, gardens: h.gardens.map((g, i) => ({ ...g, index: i + 1, hasFile: true })) }
    }
    if (route === 'zen/get') {
      const id = String(p.garden)
      const g = h.gardens.find((x) => x.id === id)
      if (!g) throw new Error('unknown_garden')
      const caps = h.pageCaps ?? {
        agents: ['agents:read', 'agents:add', 'presence:read'],
        conversation: ['agents:read', 'presence:read', 'thread:read', 'thread:post'],
      }
      const custom = h.pages[id]
      const page = custom
        ? { ...custom, controls: ['garden-switcher', 'drag-region', 'zen-toggle'] }
        : g.template === 'k2.blank@1'
          ? {
              template: 'k2.blank@1',
              layout: { kind: 'columns', split: [100], minWidths: [0] },
              widgets: [
                { id: 'garden-empty', kind: 'garden-empty', column: 0, props: {}, caps: ['agents:read', 'thread:read', 'thread:post'], source: 'builtin' },
              ],
              controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
            }
          : {
              template: 'k2.texting@1',
              layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
              widgets: [
                { id: 'agents', kind: 'agents', column: 0, props: { 'home-picker': true }, caps: caps.agents, source: 'builtin' },
                { id: 'conversation', kind: 'conversation', column: 1, props: {}, caps: caps.conversation, source: 'builtin' },
                { id: 'nav', kind: 'nav-rail', column: 0, props: {}, caps: ['app:navigate'], source: 'builtin' },
              ],
              controls: ['garden-switcher', 'drag-region', 'zen-toggle', 'add-agent'],
            }
      return {
        ok: true,
        schema: 1,
        version: 'v1',
        garden: { id: g.id, name: g.name, index: h.gardens.indexOf(g) + 1 },
        page,
        theme: {},
        themes: [
          { name: 'default', builtin: true, user: false },
          { name: 'paper', builtin: true, user: false },
        ],
        chrome: {},
        motion: {},
        errors: [],
        warnings: [],
        lastGoodAt: null,
      }
    }
    if (route === 'projects/list' && scope.hostKey === 'local') {
      if (!h.localProjects) throw new Error('unexpected GET projects/list on local')
      return h.localProjects
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
    // The top band's usage tool (the app top bar's UsageButton).
    if (route === 'usage/subscriptions') {
      return {
        harnesses: [
          {
            harness: 'claude',
            plan: 'Max 20x',
            windows: [{ label: 'Weekly', used: 0.31, resetsAt: '2026-10-10T00:00:00Z' }],
            checkedAt: new Date().toISOString(),
            status: '',
          },
        ],
      }
    }
    // Zen's Tickets view (the Tickets page's board and stores).
    if (route === 'feedback/list-all' || route === 'feedback/list') return { ok: true, items: h.tickets }
    if (route === 'feedback/show') {
      const t = h.tickets.find((x) => x.id === p.id)
      if (!t) throw new Error(`no ticket ${String(p.id)}`)
      if (p.brief) {
        return {
          ok: true,
          brief: { html: '<h2>Which DNS?</h2><p>Pick one.</p>', text: 'Which DNS? Pick one.', bytes: 40, sha256: 'feedc0de', sanitizer: 'k2-brief-v1', createdAt: 900 },
        }
      }
      return { ...t, workspace: 'cortana', canonicalSessionId: null, comments: [{ author: 'cortana', body: 'Which DNS host should I use?', at: 900 }] }
    }
    if (route === 'auth/whoami') return { owner: true, username: null }
    if (route === 'users') return { users: [] }
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
import { useFeedbackStore } from '@/stores/feedback'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { hostPool } from '@/lib/host-pool-instance'
import { homeRooms } from '@/stores/home-rooms'
import { noteServerVersion } from '@/kessel/server-scope'
import { __reloadZenWindowForTests, useZenWindowStore, zenWindowKey } from '@/lib/zen/zen-window'
import { __resetZenGardensForTests, useZenGardensStore } from '@/lib/zen/zen-gardens'
import { useZenGardenHomesStore, ZEN_GARDEN_HOMES_KEY } from '@/lib/zen/zen-garden-homes'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import { __resetZenApiForTests } from '@/lib/zen/zen-api'
import { __setZenGeometryForTests, runZenControlChecksNow } from '@/lib/zen/zen-monitor'
import { useZenViewStore } from '@/lib/zen/zen-view'
import { openZenCheatSheet } from '@/lib/zen/zen-theme-switch'
import { __resetZenDataForTests } from '@/lib/zen/zen-data'
import { useZenAddAgentStore } from '@/lib/zen/zen-add-agent'
import { requestZenComposeFocus, useZenComposeFocusStore, ZEN_COMPOSE_FOCUS_TTL_MS } from '@/lib/zen/zen-compose-focus'
import { ZenBridgeError, createZenBridge } from '@/lib/zen/zen-bridge'
import { BUILTIN_TEXTING_PAGE } from '@/lib/zen/zen-page'
import type { ZenGeometry } from '@/lib/zen/zen-controls'
import { useWorkspaceIndexShortcuts } from '@/hooks/useWorkspaceIndexShortcuts'
import { ZenHost } from '../ZenHost'
import { installZenBuiltins } from './builtins'
import { __resetZenDraftsForTests } from './ZenCompose'
import { zenEmptyThreadText, zenPermissionText } from './ZenConversationWidget'
import { zenGardenAskDraft } from './ZenGardenEmptyWidget'
import { ZEN_PROJECTS_TEXT, zenProjectsAskDraft } from './ZenProjectsViewWidget'
import { ZEN_EMPTY_FOCUS_GROUP } from './ZenAgentsWidget'
import { ZEN_NAV_PILL_SPRING, ZEN_NAV_RAIL_CSS, zenNavPillMotion } from './ZenNavRailWidget'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'

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
    // The top-right cluster: theme control, then the Zen toggle.
    if (el.hasAttribute('data-zen-switch')) return { left: 1110, top: 11, width: 76, height: 30 }
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
  h.gardens = [
    { id: 'g-default', name: 'Garden 1', template: 'k2.texting@1' },
    { id: 'g-notes', name: 'Notes', template: 'k2.blank@1' },
  ]
  h.pages = {}
  h.localProjects = null
  h.tickets = []
  useFocusGroupsStore.setState({ focusGroupsEnabled: false, focusGroups: [], activeFocusGroupId: null })
  __resetZenGardensForTests()
  useZenGardenHomesStore.setState({ picks: {} })
  useZenWindowStore.setState({ on: true, garden: 'g-default', view: 'home' })
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

/** Pick `homeId` in the (first) Agents widget's own Home picker. */
async function pickWidgetHome(homeId: string, widget = 'agents'): Promise<void> {
  const box = document.querySelector(`[data-zen-widget-id="${widget}"]`)
  if (!box) throw new Error(`no Agents widget ${widget}`)
  const button = box.querySelector('[data-zen-home-picker-button]')
  if (!(button instanceof HTMLElement)) throw new Error('no Home picker in the widget')
  await act(async () => {
    fireEvent.click(button)
  })
  const choice = box.querySelector(`[data-zen-home-choice="${homeId}"]`)
  if (!(choice instanceof HTMLElement)) throw new Error(`no Home ${homeId} in the widget's picker`)
  await act(async () => {
    fireEvent.click(choice)
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

    // An empty Home says so (picked in the widget's own Home picker).
    await pickWidgetHome('h2')
    await waitFor(() =>
      expect(document.querySelector('[data-zen-agents-empty]')?.textContent).toBe(
        'This Home has no agents yet. Use Add agent to add some.',
      ),
    )
    expect(useHomesStore.getState().selectedId).toBe('h1')
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

// Rosson 2026-10-04: Garden 1's thin left rail. Agents, Projects and
// Tickets switch Garden 1's VIEW in this window, inside Zen.
describe('the nav rail (Garden 1)', () => {
  function navButton(page: string): HTMLElement {
    const el = document.querySelector(`[data-zen-nav="${page}"]`)
    if (!(el instanceof HTMLElement)) throw new Error(`no ${page} in the rail`)
    return el
  }

  function currentNav(): string[] {
    return Array.from(document.querySelectorAll('[data-zen-nav][aria-current="page"]')).map(
      (b) => b.getAttribute('data-zen-nav') ?? '',
    )
  }

  async function showView(view: string): Promise<void> {
    await act(async () => {
      fireEvent.click(navButton(view))
    })
    await waitFor(() => {
      if (document.querySelector('[data-zen-page]')?.getAttribute('data-zen-view') !== view) throw new Error(`${view} not shown`)
    })
  }

  it('four icon buttons with tooltips, My Home current, 44px wide, outside the Agents box', async () => {
    await mountZen()
    const rail = document.querySelector('[data-zen-widget="nav-rail"]') as HTMLElement
    expect(rail).not.toBeNull()
    expect(rail.style.width).toBe('44px')
    expect(rail.closest('[data-zen-column]')).toBeNull()
    expect(rail.parentElement?.getAttribute('data-zen-column-slot')).toBe('0')
    expect(Array.from(rail.querySelectorAll('[data-zen-nav]')).map((b) => [b.getAttribute('data-zen-nav'), b.getAttribute('aria-label')])).toEqual([
      ['home', 'My Home'],
      ['agents', 'Agents'],
      ['projects', 'Projects'],
      ['tickets', 'Tickets'],
    ])
    for (const b of Array.from(rail.querySelectorAll('[data-zen-nav]'))) {
      expect(b.getAttribute('title')).toBeTruthy()
      // No tooltip says the rail leaves Zen any more.
      expect(b.getAttribute('title')).not.toMatch(/leaves zen/i)
      expect(b.querySelector('svg')).not.toBeNull()
      expect(b.textContent?.trim()).toBe('')
    }
    expect(currentNav()).toEqual(['home'])
    expect(navButton('agents').classList.contains('cursor-pointer')).toBe(true)
    // My Home is where you are: a click changes nothing.
    await act(async () => {
      fireEvent.click(navButton('home'))
    })
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(useZenWindowStore.getState().view).toBe('home')
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it.each(['agents', 'projects', 'tickets'] as const)(
    '%s switches Garden 1’s view in this window and never leaves Zen or moves the page under it',
    async (nav) => {
      await mountZen()
      await showView(nav)
      expect(useZenWindowStore.getState().on).toBe(true)
      expect(useZenWindowStore.getState().view).toBe(nav)
      expect(useZenViewStore.getState().safe).toBeNull()
      expect(usePageViewStore.getState().page).toBe('home')
      expect(document.querySelector('[data-zen-root]')).not.toBeNull()
      // It is the current item now; My Home isn't.
      expect(currentNav()).toEqual([nav])
      expect(navButton('home').classList.contains('cursor-pointer')).toBe(true)
      // The template's band is untouched: the Garden switcher and the toggle.
      expect(document.querySelector('[data-zen-garden-pill]')?.getAttribute('data-zen-bound')).toBe('garden-switcher')
      expect(document.querySelector('[data-zen-switch]')?.getAttribute('data-zen-bound')).toBe('zen-toggle')
      // The rail stays at the left edge.
      expect(document.querySelector('[data-zen-column-slot="0"]')?.firstElementChild?.getAttribute('data-zen-widget')).toBe('nav-rail')
      // My Home brings the Garden's own page back.
      await showView('home')
      expect(currentNav()).toEqual(['home'])
      await waitFor(() => expect(document.querySelectorAll('[data-zen-agent-row]').length).toBe(4))
      expect(document.querySelector('[data-zen-widget="agents"]')?.getAttribute('data-zen-agents-source')).toBe('home')
    },
  )

  it('the view is per window and remembered across a reload', async () => {
    await mountZen()
    await showView('tickets')
    expect(JSON.parse(localStorage.getItem(zenWindowKey('main')) ?? 'null')).toEqual({
      version: 1,
      on: true,
      garden: 'g-default',
      view: 'tickets',
    })
    // Another window has its own view.
    localStorage.setItem(zenWindowKey('window-2'), JSON.stringify({ version: 1, on: true, garden: 'g-default' }))
    act(() => __reloadZenWindowForTests('window-2'))
    expect(useZenWindowStore.getState().view).toBe('home')
    // A relaunch of this window comes back to Tickets.
    cleanup()
    act(() => __reloadZenWindowForTests('main'))
    expect(useZenWindowStore.getState().view).toBe('tickets')
    render(<ZenHost />)
    await waitFor(() => expect(document.querySelector('[data-zen-page]')?.getAttribute('data-zen-view')).toBe('tickets'))
    expect(currentNav()).toEqual(['tickets'])
  })

  it('switching rail views never trips the required-controls check into safe mode', async () => {
    await mountZen()
    for (const view of ['agents', 'projects', 'tickets', 'home', 'tickets', 'agents'] as const) {
      await showView(view)
      // Two failed checks in a row are a failure: run two after each switch.
      act(() => runZenControlChecksNow())
      act(() => runZenControlChecksNow())
      expect(useZenViewStore.getState().safe, view).toBeNull()
      expect(document.querySelector('[data-zen-safe-banner]'), view).toBeNull()
    }
    expect(useZenWindowStore.getState().on).toBe(true)
  })

  it('Tickets carries the top bar’s waiting badge, live; none when nothing waits', async () => {
    act(() => useFeedbackStore.setState({ waitingCount: 0, waitingStale: false, waitingUnsupported: false }))
    await mountZen()
    expect(document.querySelector('[data-zen-nav-badge]')).toBeNull()
    act(() => useFeedbackStore.setState({ waitingCount: 3 }))
    await waitFor(() => expect(document.querySelector('[data-zen-nav="tickets"] [data-zen-nav-badge]')?.textContent).toBe('3'))
    act(() => useFeedbackStore.setState({ waitingStale: true }))
    await waitFor(() => expect(document.querySelector('[data-zen-nav-badge]')?.getAttribute('data-stale')).toBe('true'))
    act(() => useFeedbackStore.setState({ waitingCount: 0, waitingStale: false }))
    await waitFor(() => expect(document.querySelector('[data-zen-nav-badge]')).toBeNull())
  })

  it('a blank Garden has no rail, and ignores this window’s rail view', async () => {
    useZenWindowStore.setState({ garden: 'g-notes', view: 'tickets' })
    await mountGarden('k2.blank@1')
    expect(document.querySelector('[data-zen-widget="nav-rail"]')).toBeNull()
    expect(document.querySelector('[data-zen-widget="garden-empty"]')).not.toBeNull()
    expect(document.querySelector('[data-zen-widget="tickets-view"]')).toBeNull()
    expect(document.querySelector('[data-zen-page]')?.getAttribute('data-zen-view')).toBe('home')
  })

  it('app.open needs app:navigate and a known page; app.current reads the view', async () => {
    await mountZen()
    const hostStub = {
      gardens: () => [],
      currentGardenId: () => '',
      switchGarden: () => {},
      createGarden: async () => {
        throw new Error('unused')
      },
      renameGarden: async () => {},
      deleteGarden: async () => {},
      homes: () => [],
      exit: () => {},
      controls: { bind: () => () => {}, bindings: () => [], wiringFailure: () => null, dispose: () => {} },
      page: () => BUILTIN_TEXTING_PAGE,
    }
    const without = createZenBridge(hostStub as never, { id: 'x', caps: [] })
    expect(() => without.call('app.open', 'agents')).toThrow(/cap_not_granted/)
    expect(() => without.call('app.current')).toThrow(/cap_not_granted/)
    const withCap = createZenBridge(hostStub as never, { id: 'x', caps: ['app:navigate'] })
    expect(() => withCap.call('app.open', 'settings')).toThrow(/page must be one of/)
    expect(withCap.call('app.current')).toBe('home')
    act(() => void withCap.call('app.open', 'projects'))
    expect(withCap.call('app.current')).toBe('projects')
    expect(useZenWindowStore.getState().on).toBe(true)
  })

  // Rosson 2026-10-04, Option A: one liquid glass pill under the current
  // item, springing between icons (Motion shared layout).
  describe('the glass selection pill', () => {
    let restoreMatchMedia: PropertyDescriptor | undefined

    beforeEach(() => {
      restoreMatchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia')
    })

    afterEach(() => {
      if (restoreMatchMedia) Object.defineProperty(window, 'matchMedia', restoreMatchMedia)
      else delete (window as { matchMedia?: unknown }).matchMedia
    })

    function setReducedMotion(on: boolean): void {
      Object.defineProperty(window, 'matchMedia', {
        configurable: true,
        value: (query: string) => ({
          matches: query === '(prefers-reduced-motion: reduce)' ? on : false,
          media: query,
          addEventListener: () => undefined,
          removeEventListener: () => undefined,
        }),
      })
    }

    function pills(): HTMLElement[] {
      return Array.from(document.querySelectorAll('[data-zen-widget="nav-rail"] [data-zen-nav-pill]')) as HTMLElement[]
    }

    function onePill(): HTMLElement {
      const all = pills()
      expect(all.length).toBe(1)
      return all[0]
    }

    it('exactly one pill, inside the current item, and it moves with the view', async () => {
      setReducedMotion(false)
      await mountZen()
      const pill = onePill()
      expect(pill.closest('[data-zen-nav]')?.getAttribute('data-zen-nav')).toBe('home')
      expect(pill.closest('[data-zen-nav]')?.getAttribute('aria-current')).toBe('page')
      expect(pill.getAttribute('aria-hidden')).toBe('true')
      expect(pill.getAttribute('data-zen-nav-pill-motion')).toBe('spring')
      for (const view of ['agents', 'tickets', 'projects', 'home'] as const) {
        await showView(view)
        const moved = onePill()
        expect(moved.closest('[data-zen-nav]')?.getAttribute('data-zen-nav'), view).toBe(view)
        expect(currentNav()).toEqual([view])
      }
      // Still Zen, no safe mode, every non-current button a pointer.
      expect(useZenWindowStore.getState().on).toBe(true)
      expect(useZenViewStore.getState().safe).toBeNull()
      expectEveryButtonPointer()
    })

    it('its layout scope is this rail’s own', async () => {
      setReducedMotion(false)
      await mountZen()
      const scope = onePill().getAttribute('data-zen-nav-pill-scope') ?? ''
      const railId = document.querySelector('[data-zen-widget="nav-rail"]')?.getAttribute('data-zen-widget-id')
      expect(railId).toBeTruthy()
      expect(scope.startsWith(`zen-nav-rail-${railId}-`)).toBe(true)
      expect(scope.length).toBeGreaterThan(`zen-nav-rail-${railId}-`.length)
    })

    it('reduced motion: no slide, the pill is drawn on the new item at once', async () => {
      setReducedMotion(true)
      await mountZen()
      expect(onePill().getAttribute('data-zen-nav-pill-motion')).toBe('instant')
      await showView('tickets')
      const pill = onePill()
      expect(pill.getAttribute('data-zen-nav-pill-motion')).toBe('instant')
      expect(pill.closest('[data-zen-nav]')?.getAttribute('data-zen-nav')).toBe('tickets')
    })

    it('the motion config: a gentle spring with a shared layout id, or none at all', () => {
      expect(ZEN_NAV_PILL_SPRING).toEqual({ type: 'spring', stiffness: 500, damping: 35, mass: 1 })
      expect(zenNavPillMotion(false, 'pill')).toEqual({ mode: 'spring', layoutId: 'pill', transition: ZEN_NAV_PILL_SPRING })
      expect(zenNavPillMotion(true, 'pill')).toEqual({ mode: 'instant', layoutId: undefined, transition: { duration: 0 } })
    })

    it('glass in WebKit terms: blur + saturate, Zen tokens only, no SVG filter, solid under reduced transparency', async () => {
      setReducedMotion(false)
      await mountZen()
      const css = document.querySelector('[data-zen-widget="nav-rail"] style[data-zen-nav-rail-glass]')?.textContent ?? ''
      expect(css).toBe(ZEN_NAV_RAIL_CSS)
      expect(css).toContain('backdrop-filter: blur(14px) saturate(180%)')
      expect(css).toContain('-webkit-backdrop-filter: blur(14px) saturate(180%)')
      expect(css).toContain('var(--zen-accent)')
      expect(css).toContain('[data-zen-scheme="dark"]')
      expect(css).not.toMatch(/url\(/)
      expect(css).not.toMatch(/var\(--color-/)
      const reduced = css.slice(css.indexOf('@media (prefers-reduced-transparency: reduce)'))
      expect(reduced).toContain('[data-zen-nav-pill]')
      expect(reduced).toContain('backdrop-filter: none')
      expect(css).toContain('@media (prefers-reduced-motion: reduce)')
    })
  })
})

/** Every button in Zen shows a pointer on hover (the agent rows keep the
 *  list's arrow, like a chat list; the current rail item is where you are). */
function expectEveryButtonPointer(): void {
  const bare = Array.from(document.querySelectorAll('[data-zen-root] button')).filter(
    (b) => !b.classList.contains('cursor-pointer') && !b.hasAttribute('data-zen-agent-row') && b.getAttribute('aria-current') !== 'page',
  )
  expect(bare.map((b) => b.outerHTML.slice(0, 120))).toEqual([])
}

async function openRailView(view: 'agents' | 'projects' | 'tickets'): Promise<void> {
  const el = document.querySelector(`[data-zen-nav="${view}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`no ${view} in the rail`)
  await act(async () => {
    fireEvent.click(el)
  })
  await waitFor(() => {
    if (document.querySelector('[data-zen-page]')?.getAttribute('data-zen-view') !== view) throw new Error(`${view} not shown`)
  })
}

// Rosson 2026-10-04: the rail's Agents view mirrors My Home with this
// server's workspaces, and the app's focus groups in place of the Home picker.
describe('the Agents view (Garden 1)', () => {
  const PROJECTS = [
    { id: 'p1', name: 'cortana', handle: 'cortana', path: '/w/cortana', color: '#c2662d', focusGroupId: 'g-work', pinned: 0, workspaces: [{ id: 'w1', type: 'main', name: 'main' }] },
    { id: 'p2', name: 'atlas', handle: 'atlas', path: '/w/atlas', color: '#3366aa', focusGroupId: 'g-play', pinned: 1, workspaces: [{ id: 'w2', type: 'main', name: 'main' }] },
    { id: 'p3', name: 'bolt', handle: 'bolt', path: '/w/bolt', color: '#33aa66', focusGroupId: 'g-play', pinned: 0, workspaces: [{ id: 'w3', type: 'main', name: 'main' }] },
    { id: 'p4', name: 'drift', handle: 'drift', path: '/w/drift', color: '#aa3366', focusGroupId: null, pinned: 0, workspaces: [{ id: 'w4', type: 'main', name: 'main' }] },
  ]
  const GROUPS = [
    { id: 'g-work', name: 'Work', color: '#ff8800', tabOrder: 0, createdAt: 0 },
    { id: 'g-play', name: 'Play', color: null, tabOrder: 1, createdAt: 0 },
  ]

  beforeEach(() => {
    useProjectsStore.setState({ projects: PROJECTS as never })
  })

  async function agentsViewRows(expected: number): Promise<string[]> {
    await waitFor(() => {
      const box = document.querySelector('[data-zen-widget="agents"][data-zen-agents-source="workspaces"]')
      if (!box || box.querySelectorAll('[data-zen-agent-row]').length !== expected) throw new Error('rows not drawn')
    })
    return rowAddresses()
  }

  it('focus groups off: every workspace on this server, the Agents page’s order, no dropdown, no Home picker, no Add agent', async () => {
    await mountZen()
    await openRailView('agents')
    // Pinned first, then the rest, as the Agents page lists them.
    expect(await agentsViewRows(4)).toEqual(['atlas::local', 'cortana::local', 'bolt::local', 'drift::local'])
    expect(document.querySelector('[data-zen-focus-group-picker]')).toBeNull()
    expect(document.querySelector('[data-zen-home-picker]')).toBeNull()
    expect(document.querySelector('[data-zen-add-agent]')).toBeNull()
    // One server: no server tags.
    expect(document.querySelector('[data-zen-server-tag]')).toBeNull()
    // The same texting layout: the Agents list beside the Conversation.
    expect(document.querySelector('[data-zen-column-slot="1"] [data-zen-widget="conversation"]')).not.toBeNull()
    expectEveryButtonPointer()
  })

  it('focus groups on: the app’s focus-group dropdown picks the group whose agents are listed, without switching workspaces', async () => {
    act(() => useFocusGroupsStore.setState({ focusGroupsEnabled: true, focusGroups: GROUPS, activeFocusGroupId: 'g-work' }))
    await mountZen()
    await openRailView('agents')
    // Pinned, then the group's, then ungrouped ones (GH #26).
    expect(await agentsViewRows(3)).toEqual(['atlas::local', 'cortana::local', 'drift::local'])
    const picker = document.querySelector('[data-zen-focus-group-picker]') as HTMLElement
    expect(picker).not.toBeNull()
    expect(picker.textContent).toContain('Work')
    // Rosson 2026-10-04: the dropdown reads "GROUP:" in front (uppercase via CSS).
    const label = picker.querySelector('[data-zen-focus-group-label]') as HTMLElement
    expect(label.textContent).toBe('Group:')
    expect(label.className).toContain('uppercase')
    expect(picker.firstElementChild).toBe(label)
    expect(document.querySelector('[data-zen-home-picker]')).toBeNull()
    expectEveryButtonPointer()

    await act(async () => {
      fireEvent.click(picker.querySelector('button') as HTMLElement)
    })
    const play = Array.from(picker.querySelectorAll('button')).find((b) => b.textContent?.includes('Play'))
    if (!play) throw new Error('no Play in the focus-group dropdown')
    expectEveryButtonPointer()
    await act(async () => {
      fireEvent.click(play)
    })
    expect(await agentsViewRows(3)).toEqual(['atlas::local', 'bolt::local', 'drift::local'])
    expect(useFocusGroupsStore.getState().activeFocusGroupId).toBe('g-play')
    // The list changed; the window's workspace didn't.
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
    expect(picker.textContent).toContain('Play')
  })

  it('focus groups on, a group with nothing in it says so', async () => {
    act(() =>
      useFocusGroupsStore.setState({
        focusGroupsEnabled: true,
        focusGroups: [...GROUPS, { id: 'g-empty', name: 'Empty', color: null, tabOrder: 2, createdAt: 0 }],
        activeFocusGroupId: 'g-empty',
      }),
    )
    useProjectsStore.setState({ projects: PROJECTS.filter((p) => p.focusGroupId === 'g-work') as never })
    await mountZen()
    await openRailView('agents')
    await waitFor(() => expect(document.querySelector('[data-zen-agents-empty]')?.textContent).toBe(ZEN_EMPTY_FOCUS_GROUP))
  })

  it('picking an agent opens its conversation in place and focuses its message box, like My Home', async () => {
    await mountZen()
    await openRailView('agents')
    await agentsViewRows(4)
    await select('cortana::local')
    await threadReady('cortana::local')
    await waitFor(() => expect(document.activeElement?.getAttribute('aria-label')).toBe('Message cortana'))
    expect(rowEl('cortana::local').hasAttribute('data-selected')).toBe(true)
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    // Its selection is the Agents view's own: My Home's list is untouched.
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-nav="home"]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelectorAll('[data-zen-agent-row]').length).toBe(4))
    expect(document.querySelector('[data-zen-agent-row][data-selected]')).toBeNull()
  })
})

// Rosson 2026-10-04: the rail's Projects view is coming soon.
describe('the Projects view (Garden 1)', () => {
  it('a centred “Coming soon — or build a new one yourself!”, and the link asks one of this computer’s agents', async () => {
    await mountZen()
    await openRailView('projects')
    const soon = document.querySelector('[data-zen-projects-soon]') as HTMLElement
    expect(soon.textContent).toBe(ZEN_PROJECTS_TEXT)
    expect(ZEN_PROJECTS_TEXT).toBe('Coming soon — or build a new one yourself!')
    const view = document.querySelector('[data-zen-widget="projects-view"]') as HTMLElement
    expect(view.className).toContain('items-center')
    expect(view.className).toContain('justify-center')
    expect(document.querySelector('[data-zen-widget="agents"]')).toBeNull()
    expectEveryButtonPointer()

    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-projects-build]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelector('[data-zen-ask-agent="cortana::local"]')).not.toBeNull())
    expectEveryButtonPointer()
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-agent="cortana::local"]') as HTMLElement)
    })
    await threadReady('cortana::local')
    const draft = zenProjectsAskDraft({ id: 'g-default', name: 'Garden 1' })
    await waitFor(() => expect(composeInput().value).toBe(draft))
    expect(h.calls.some((c) => c.route === 'thread/post')).toBe(false)
  })
})

// Rosson 2026-10-04: the rail's Tickets view, the Tickets page in glass,
// chat only.
describe('the Tickets view (Garden 1)', () => {
  const TICKET = {
    id: 't-1',
    projectId: 'p1',
    sessionId: 'sess-9',
    sessionKind: 'canonical',
    agentName: 'cortana',
    kind: 'question',
    title: 'Which DNS host?',
    body: null,
    options: null,
    priority: 2,
    status: 'waiting',
    answer: null,
    createdAt: 900,
    updatedAt: 900,
    answeredAt: null,
    commentCount: 1,
    assignees: [],
    hasBrief: true,
    briefBytes: 40,
    projectPath: '/w/cortana',
    projectName: 'cortana',
    linked: true,
  }

  beforeEach(() => {
    h.tickets = [TICKET]
  })

  async function openTicket(): Promise<HTMLElement> {
    await openRailView('tickets')
    await waitFor(() => expect(document.querySelector('[data-testid="ticket-scope-all"]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(document.querySelector('[data-testid="ticket-scope-all"]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelector('[data-testid="ticket-card"]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(document.querySelector('[data-testid="ticket-card"]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelector('[data-testid="ticket-rail-thread"]')).not.toBeNull())
    return document.querySelector('[data-testid="ticket-agent-rail"]') as HTMLElement
  }

  it('the Tickets page’s list + item in liquid glass panels, with no box around them', async () => {
    await mountZen()
    await openRailView('tickets')
    const view = document.querySelector('[data-zen-widget="tickets-view"]') as HTMLElement
    expect(view).not.toBeNull()
    expect(view.hasAttribute('data-zen-tickets')).toBe(true)
    // Its column has no box: the glass panels are the box.
    expect(view.closest('[data-zen-column]')?.hasAttribute('data-zen-column-bare')).toBe(true)
    const glass = view.querySelector('style[data-zen-tickets-glass]')?.textContent ?? ''
    expect(glass).toContain('backdrop-filter')
    expect(glass).toContain('prefers-reduced-transparency')
    // Zen tokens only (the board's Styles variables are Zen tokens under the shield).
    expect(glass).not.toMatch(/var\(--color-/)
    await waitFor(() => expect(view.querySelector('[data-testid="ticket-list"]')).not.toBeNull())
    expect(view.querySelector('[data-testid="ticket-board"]')).not.toBeNull()
  })

  it('an open ticket shows the chat only: no terminal tab or toggle; the HTML brief frame stays', async () => {
    await mountZen()
    const rail = await openTicket()
    expect(rail.querySelector('[role="tablist"]')).toBeNull()
    expect(document.querySelector('[data-testid="ticket-rail-tab-agent"]')).toBeNull()
    expect(document.querySelector('[data-testid="ticket-agent-terminal"]')).toBeNull()
    expect(rail.querySelector('[data-testid="ticket-compose-input"]')).not.toBeNull()
    expect(rail.textContent).toContain('Which DNS host should I use?')
    await waitFor(() => expect(document.querySelector('[data-testid="brief-frame"]')).not.toBeNull())
    // Still Zen, still the same window page.
    expect(useZenWindowStore.getState().on).toBe(true)
    expect(usePageViewStore.getState().page).toBe('home')
    expectEveryButtonPointer()
  })
})

// Rosson 2026-10-04: the app top bar's usage tool in Zen's top band.
describe('the usage tool in the top band', () => {
  it('sits immediately left of the theme control, in glass, and its menu opens inside Zen over the page', async () => {
    await mountZen()
    const topRight = document.querySelector('[data-zen-top-right]') as HTMLElement
    const order = Array.from(topRight.children).map((c) =>
      c.hasAttribute('data-zen-usage') ? 'usage' : c.hasAttribute('data-zen-theme-picker') ? 'theme' : c.hasAttribute('data-zen-switch') ? 'toggle' : c.tagName,
    )
    expect(order).toEqual(['usage', 'theme', 'toggle'])
    const usage = topRight.querySelector('[data-zen-usage]') as HTMLElement
    expect(usage.querySelector('style')?.textContent).toContain('backdrop-filter')
    await waitFor(() => expect(usage.querySelector('[data-testid="subscription-usage"]')?.textContent).toContain('31%'))
    // A stacking layer above the page, like the theme picker.
    expect(usage.style.position).toBe('relative')
    expect(Number(usage.style.zIndex)).toBeGreaterThan(0)

    await act(async () => {
      fireEvent.click(usage.querySelector('[data-testid="subscription-usage"]') as HTMLElement)
    })
    const menu = document.querySelector('[data-testid="subscription-usage-menu"]') as HTMLElement
    expect(menu).not.toBeNull()
    // Inside Zen (no portal), so Zen colours apply.
    expect(usage.contains(menu)).toBe(true)
    expect(menu.textContent).toContain('Weekly 31%')
    expectEveryButtonPointer()
    // An open menu never counts as hiding the page's controls.
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()
  })

  it('stays put in every rail view', async () => {
    await mountZen()
    for (const view of ['agents', 'projects', 'tickets'] as const) {
      await openRailView(view)
      expect(document.querySelector('[data-zen-top-right] > [data-zen-usage]'), view).not.toBeNull()
    }
  })
})

// Rosson 2026-10-04: picking an agent puts the caret in its message box.
describe('picking an agent focuses its message box', () => {
  const focusedLabel = (): string | null => document.activeElement?.getAttribute('aria-label') ?? null

  it('a row click focuses “Message <agent>” once it opens; first render and ⌘1–9 never do', async () => {
    await mountZen()
    // First render: nothing selected, nothing focused in a box.
    expect(document.activeElement?.hasAttribute('data-zen-compose-input')).toBe(false)

    // ⌘2 opens sales, but is not a pick: focus stays put.
    await act(async () => {
      fireEvent.keyDown(window, { key: '2', code: 'Digit2', metaKey: true })
    })
    await threadReady(ROWS.sales.address)
    expect(document.activeElement?.hasAttribute('data-zen-compose-input')).toBe(false)

    // A click on cortana: its box takes the caret.
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    await waitFor(() => expect(focusedLabel()).toBe('Message cortana'))
    expect(useZenComposeFocusStore.getState().request).toBeNull()

    // A remote update re-renders the box: focus is not taken back from
    // elsewhere.
    ;(document.activeElement as HTMLElement).blur()
    act(() => useActiveAgentsStore.setState({ paneStatuses: new Map() }))
    act(() => useHomesStore.getState().moveRow('h1', 1, 0))
    expect(document.activeElement?.hasAttribute('data-zen-compose-input')).toBe(false)
  })

  it('a pick waits for a remote conversation to open, and never steals focus from a field you started typing in', async () => {
    await mountZen()
    // The remote row opens a room first; the caret follows when it is ready.
    await select(ROWS.sales.address)
    await threadReady(ROWS.sales.address)
    await waitFor(() => expect(focusedLabel()).toBe('Message sales'))

    // A pick, then typing in another field before the box is ready: the
    // request is dropped, the field keeps focus.
    const other = document.createElement('input')
    document.body.appendChild(other)
    act(() => requestZenComposeFocus(ROWS.cortana.address))
    other.focus()
    await act(async () => {
      fireEvent.keyDown(window, { key: '1', code: 'Digit1', metaKey: true })
    })
    await threadReady(ROWS.cortana.address)
    expect(document.activeElement).toBe(other)
    expect(useZenComposeFocusStore.getState().request).toBeNull()
    other.remove()
  })

  it('a stale request (older than the TTL) is dropped', async () => {
    await mountZen()
    useZenComposeFocusStore.setState({
      request: { address: ROWS.cortana.address, at: Date.now() - ZEN_COMPOSE_FOCUS_TTL_MS - 1 },
    })
    await act(async () => {
      fireEvent.keyDown(window, { key: '1', code: 'Digit1', metaKey: true })
    })
    await threadReady(ROWS.cortana.address)
    expect(document.activeElement?.hasAttribute('data-zen-compose-input')).toBe(false)
    expect(useZenComposeFocusStore.getState().request).toBeNull()
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
    // One snapshot, then (socket open) one catch-up from its newest seq.
    const reads = (): unknown[] =>
      h.calls.filter((c) => c.route === 'thread' && c.hostKey === B).map((c) => [c.data.addr, c.data.since_seq ?? null])
    await waitFor(() =>
      expect(reads()).toEqual([
        ['sales-desk', null],
        ['sales-desk', 7],
      ]),
    )
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
    expect(
      h.calls
        .filter((c) => c.route === 'thread' && c.hostKey === 'local' && c.data.since_seq === undefined)
        .map((c) => [c.hostKey, c.data.addr]),
    ).toEqual([['local', 'cortana-main']])
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

  it('Thread sync: a message from another place lands once, and the Garden’s own send plus its echo is one bubble', async () => {
    await mountZen()
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    await typeAndSend('Ship it.')
    await waitFor(() => expect(document.querySelectorAll('[data-zen-message][data-mine]').length).toBe(1))
    const own = h.threads[threadKey('local', 'cortana-main')].at(-1) as { seq: number; id: string }
    const ws = h.sockets.find((s) => s.hostKey === 'local' && s.onmessage !== null)
    if (!ws?.onmessage) throw new Error('no overlay socket')
    const push = (frame: unknown): void => {
      act(() => ws.onmessage?.({ data: JSON.stringify(frame) }))
    }
    // The daemon echoes the Garden's own send.
    push(own)
    // A message sent from the Agents page Thread (or another person) that
    // raced the Garden's send: a lower seq, reaching the socket after the
    // send's answer. It used to be dropped as a "replay".
    const other = {
      collection: 'thread',
      seq: own.seq - 1,
      id: 'elsewhere1',
      doc: { id: 'elsewhere1', kind: 'text', from: 'cortana', body: 'Sent from the Agents page.' },
    }
    push(other)
    push(other)
    await waitFor(() =>
      expect(document.querySelector('[data-zen-message="elsewhere1"]')?.textContent).toContain('Sent from the Agents page.'),
    )
    expect(document.querySelectorAll(`[data-zen-message="${own.id}"]`).length).toBe(1)
    expect(document.querySelectorAll('[data-zen-message="elsewhere1"]').length).toBe(1)
    const order = [...document.querySelectorAll('[data-zen-message]')].map((e) => e.getAttribute('data-zen-message'))
    expect(order.indexOf('elsewhere1')).toBeLessThan(order.indexOf(own.id))
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
    // The window's own server: Zen turns off FIRST (G32/G54), then the
    // Agents page shows that workspace, live.
    expect(useZenWindowStore.getState().on).toBe(false)
    expect(document.querySelector('[data-zen-root]')).toBeNull()
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(useProjectsStore.getState().activeProjectId).toBe('p1')
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
    await waitFor(() => expect(useZenWindowStore.getState().on).toBe(false))
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

describe('template controls (G24, G25, G58)', () => {
  it('draws its own Garden switcher, Zen toggle and drag area, and they pass the required-controls check', async () => {
    await mountZen()
    const bar = document.querySelector('[data-zen-texting-controls]')
    if (!bar) throw new Error('the template controls are not registered')
    expect(bar.querySelector('[data-zen-garden-pill]')?.getAttribute('data-zen-bound')).toBe('garden-switcher')
    expect(bar.querySelector('[data-zen-garden-pill]')?.textContent).toContain('Garden 1')
    expect(bar.querySelector('[data-zen-drag]')?.getAttribute('data-zen-bound')).toBe('drag-region')
    // Rosson 2026-10-04: the Zen toggle is back in the top-right corner (the
    // top bar's spot outside Zen), with the theme control immediately left
    // of it. Band order: switcher, drag area, [theme, toggle].
    expect(Array.from(bar.children).filter((c) => c.tagName !== 'STYLE').map((c) =>
      c.hasAttribute('data-zen-garden-pill') || c.querySelector('[data-zen-garden-pill]')
        ? 'switcher'
        : c.hasAttribute('data-zen-drag')
          ? 'drag'
          : c.hasAttribute('data-zen-top-right')
            ? 'top-right'
            : c.tagName,
    )).toEqual(['switcher', 'drag', 'top-right'])
    const topRight = bar.querySelector('[data-zen-top-right]') as HTMLElement
    // Rosson 2026-10-04: usage, theme, Zen toggle.
    expect(Array.from(topRight.children).map((c) =>
      c.hasAttribute('data-zen-usage')
        ? 'usage'
        : c.hasAttribute('data-zen-theme-picker')
          ? 'theme'
          : c.hasAttribute('data-zen-switch')
            ? 'toggle'
            : c.tagName,
    )).toEqual(['usage', 'theme', 'toggle'])
    expect(document.querySelectorAll('[data-zen-switch]').length).toBe(1)
    expect(topRight.querySelector('[data-zen-switch]')?.getAttribute('data-zen-bound')).toBe('zen-toggle')
    // No footer under any column any more: both columns run to the bottom.
    // Column 0 holds the thin nav rail (left edge, outside the box), then
    // the Agents box.
    expect(document.querySelector('[data-zen-template-footer]')).toBeNull()
    const slot0 = Array.from(document.querySelector('[data-zen-column-slot="0"]')?.children ?? [])
    expect(slot0.map((c) => c.getAttribute('data-zen-widget') ?? c.getAttribute('data-zen-column'))).toEqual(['nav-rail', '0'])
    expect(document.querySelector('[data-zen-column-slot="1"]')?.children.length).toBe(1)
    // Add agent is the Agents widget's last row (bottom left inside it).
    const agents = document.querySelector('[data-zen-widget="agents"]') as HTMLElement
    const add = agents.querySelector('[data-zen-add-agent]')
    expect(add).not.toBeNull()
    expect(agents.lastElementChild?.hasAttribute('data-zen-agents-footer')).toBe(true)
    expect(agents.lastElementChild?.contains(add)).toBe(true)
    expect(document.querySelectorAll('[data-zen-add-agent]').length).toBe(1)
    expect(bar.querySelector('[data-zen-add-agent]')).toBeNull()
    // No Home switcher anywhere in Zen any more.
    expect(document.querySelector('[data-zen-home-pill], [data-zen-home-option]')).toBeNull()
    // Rosson 2026-10-04: every Zen button shows a pointer on hover (the
    // agent rows keep the list's default arrow, like a chat list).
    for (const sel of ['[data-zen-switch]', '[data-zen-add-agent]', '[data-zen-garden-pill]', '[data-zen-theme-button]', '[data-zen-home-picker-button]']) {
      expect(document.querySelector(sel)?.classList.contains('cursor-pointer'), sel).toBe(true)
    }
    const bare = Array.from(document.querySelectorAll('[data-zen-root] button')).filter(
      // The rail's My Home is where you are (aria-current): no pointer.
      (b) => !b.classList.contains('cursor-pointer') && !b.hasAttribute('data-zen-agent-row') && b.getAttribute('aria-current') !== 'page',
    )
    expect(bare.map((b) => b.outerHTML.slice(0, 80))).toEqual([])

    // Wired: activating the switcher binds an option for every Garden in 1 s.
    await act(async () => {
      fireEvent.click(bar.querySelector('[data-zen-garden-pill]') as HTMLElement)
    })
    const options = Array.from(document.querySelectorAll('[data-zen-garden-option]'))
    expect(options.map((o) => [o.getAttribute('data-zen-garden-option'), o.getAttribute('data-zen-bound')])).toEqual([
      ['g-default', 'garden-option'],
      ['g-notes', 'garden-option'],
    ])
    expect(options[1].textContent).toContain('⌥⌘2')
    expect(document.querySelector('[data-zen-new-garden]')?.textContent).toContain('New Garden')
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1100))
    })
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()

    // A Garden option switches this window's Garden; the Home page never moves.
    await act(async () => {
      fireEvent.click(options[1] as HTMLElement)
    })
    expect(useZenWindowStore.getState().garden).toBe('g-notes')
    expect(document.querySelector('[data-zen-garden-menu]')).toBeNull()
    expect(useHomesStore.getState().selectedId).toBe('h1')
    await waitFor(() => expect(document.querySelector('[data-zen-page="k2.blank@1"]')).not.toBeNull())

    // The Zen toggle exits: this window off.
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-switch]') as HTMLElement)
    })
    expect(useZenWindowStore.getState().on).toBe(false)
  }, 10_000)
})

// Rosson 2026-10-04 bug: "Zen Safe Mode keeps popping up" though the Zen
// toggle is on the page. Opening the Garden menu and closing it again
// within a second (a second click on the pill) left a sticky wiring
// failure, so the next two scheduled checks put the window in safe mode.
describe('no false safe mode (Rosson 2026-10-04)', () => {
  it('opening and closing the Garden menu quickly, then two scheduled checks: still not safe mode', async () => {
    await mountZen()
    const pill = document.querySelector('[data-zen-garden-pill]') as HTMLElement
    await act(async () => {
      fireEvent.click(pill)
    })
    expect(document.querySelector('[data-zen-garden-menu]')).not.toBeNull()
    await act(async () => {
      fireEvent.click(pill)
    })
    expect(document.querySelector('[data-zen-garden-menu]')).toBeNull()
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1100))
    })
    act(() => runZenControlChecksNow())
    act(() => runZenControlChecksNow())
    expect(useZenViewStore.getState().safe).toBeNull()
    expect(document.querySelector('[data-zen-safe]')).toBeNull()
  }, 10_000)

  it('Enter on the Garden pill opens the menu (the check does not swallow it)', async () => {
    await mountZen()
    const pill = document.querySelector('[data-zen-garden-pill]') as HTMLElement
    const e = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
    await act(async () => {
      pill.dispatchEvent(e)
    })
    expect(e.defaultPrevented).toBe(false)
  })
})

// Zen v1 bug (Rosson): agent messages drew white text on Zen's light agent
// bubble. The Thread markdown (`.chat-markdown p { color:
// var(--color-text-primary) }`) took the app Style's text colour. The real
// stylesheets (`styles.generated.css` + `globals.css`) are loaded here, so
// jsdom cascades the real rules; it leaves `var()` unresolved, so the test
// resolves them itself, each at the element that declares it.
describe('Zen text follows Zen tokens, never the app Style', () => {
  const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')
  const STYLE_PREFIXES = ['--color-', '--font-', '--radius-', '--term-', '--divider-']

  const raw = (el: Element, prop: string): string => getComputedStyle(el).getPropertyValue(prop).trim()

  /** The nearest ancestor-or-self that declares `prop` (jsdom hands inherited raw text down). */
  function declarer(el: Element, prop: string): Element {
    const v = raw(el, prop)
    let cur = el
    while (cur.parentElement && raw(cur.parentElement, prop) === v) cur = cur.parentElement
    return cur
  }

  type Hop = { name: string; at: Element }

  /** Resolve every `var()` in `text` as the browser would at `el`. */
  function resolveVars(el: Element, text: string, hops: Hop[]): string {
    const i = text.indexOf('var(')
    if (i < 0) return text
    let depth = 0
    let end = -1
    let comma = -1
    for (let j = i + 4; j < text.length; j++) {
      const c = text[j]
      if (c === '(') depth++
      else if (c === ')') {
        if (depth === 0) {
          end = j
          break
        }
        depth--
      } else if (c === ',' && depth === 0 && comma < 0) comma = j
    }
    if (end < 0) throw new Error(`unbalanced var() in ${text}`)
    const name = text.slice(i + 4, comma > 0 ? comma : end).trim()
    const fallback = comma > 0 ? text.slice(comma + 1, end).trim() : null
    let value: string
    if (raw(el, name) !== '') {
      const at = declarer(el, name)
      hops.push({ name, at })
      value = resolveVars(at, raw(at, name), hops)
    } else if (fallback !== null) {
      value = resolveVars(el, fallback, hops)
    } else {
      throw new Error(`${name} is not set at <${el.tagName.toLowerCase()}>`)
    }
    return resolveVars(el, text.slice(0, i) + value + text.slice(end + 1), hops)
  }

  /** The used value of a colour property on `el`, and the variables it went through. */
  function colorOf(el: Element, prop = 'color'): { value: string; hops: Hop[] } {
    const at = declarer(el, prop)
    const hops: Hop[] = []
    return { value: resolveVars(at, raw(at, prop), hops), hops }
  }

  function zenRoot(): HTMLElement {
    const el = document.querySelector('[data-zen-root]')
    if (!(el instanceof HTMLElement)) throw new Error('Zen is not shown')
    return el
  }

  /** Fails on any Styles variable read from outside the Zen root. */
  function expectNoStyleLeak(el: Element, prop: string): string {
    const { value, hops } = colorOf(el, prop)
    const root = zenRoot()
    const leaks = hops
      .filter((hp) => STYLE_PREFIXES.some((p) => hp.name.startsWith(p)) && !root.contains(hp.at))
      .map((hp) => `${hp.name} from <${hp.at.tagName.toLowerCase()}>`)
    expect([el.tagName.toLowerCase(), prop, leaks]).toEqual([el.tagName.toLowerCase(), prop, []])
    return value
  }

  function zenVar(name: string): string {
    const v = raw(zenRoot(), `--zen-${name}`)
    if (!v) throw new Error(`--zen-${name} is not set on the Zen root`)
    return v
  }

  const MARKDOWN = [
    'Pushed **the fix**.',
    '',
    '- one',
    '- two',
    '',
    'Run `k2 ship` or see [the notes](https://example.com).',
    '',
    '> quoted',
    '',
    '| a | b |',
    '|---|---|',
    '| 1 | 2 |',
  ].join('\n')

  const sheets: HTMLStyleElement[] = []
  let restoreMatchMedia: PropertyDescriptor | undefined

  beforeEach(() => {
    for (const f of ['styles.generated.css', 'globals.css']) {
      const s = document.createElement('style')
      s.setAttribute('data-test-sheet', f)
      s.textContent = readFileSync(join(RENDERER, f), 'utf8')
      document.head.appendChild(s)
      sheets.push(s)
    }
    restoreMatchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia')
    const first = h.threads[threadKey('local', 'cortana-main')]
    h.threads[threadKey('local', 'cortana-main')] = [
      { collection: 'thread', seq: 1, id: 'a1', doc: { id: 'a1', kind: 'text', from: 'cortana', body: MARKDOWN, created_at: 900 } },
      ...first.slice(1),
    ]
  })

  afterEach(() => {
    for (const s of sheets.splice(0)) s.remove()
    document.documentElement.removeAttribute('data-style')
    document.documentElement.removeAttribute('data-palette')
    if (restoreMatchMedia) Object.defineProperty(window, 'matchMedia', restoreMatchMedia)
    else delete (window as { matchMedia?: unknown }).matchMedia
  })

  function setSchemes(zen: 'light' | 'dark', app: 'light' | 'dark'): void {
    // The app Style: Square on Paper (light) or Charcoal (dark), the real tokens.
    document.documentElement.setAttribute('data-style', 'square')
    document.documentElement.setAttribute('data-palette', app === 'light' ? 'paper' : 'charcoal')
    // Zen's default theme is `scheme = "auto"`: this computer's setting.
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: (query: string) => ({
        matches: query === '(prefers-color-scheme: dark)' ? zen === 'dark' : false,
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      }),
    })
  }

  const COMBOS = [
    ['light', 'light'],
    ['light', 'dark'],
    ['dark', 'light'],
    ['dark', 'dark'],
  ] as const

  for (const [zen, app] of COMBOS) {
    it(`Zen ${zen} under app Style ${app}: messages, markdown, compose, list and cheat sheet use Zen tokens`, async () => {
      setSchemes(zen, app)
      const appText = raw(document.documentElement, '--color-text-primary')
      expect(appText).toBe(app === 'light' ? '#221e15' : '#e4e4e7')
      await mountZen()
      await select(ROWS.cortana.address)
      await threadReady(ROWS.cortana.address)
      const root = zenRoot()
      expect(root.getAttribute('data-zen-scheme')).toBe(zen)
      expect(root.style.colorScheme).toBe(zen)

      // The root maps the Styles variables to Zen tokens.
      expect(raw(root, '--color-text-primary')).toBe('var(--zen-text)')
      expect(raw(root, '--color-accent')).toBe('var(--zen-accent)')
      expect(raw(root, '--font-ui')).toBe('var(--zen-font-family)')

      // Agent bubble: every markdown child is the bubble's text colour.
      const agentText = zenVar('bubble-agent-text')
      expect(agentText).not.toBe(appText)
      const bubble = document.querySelector('[data-zen-message="a1"] [data-zen-bubble="agent"]')
      if (!bubble) throw new Error('no agent bubble')
      expect(expectNoStyleLeak(bubble, 'background')).toBe(zenVar('bubble-agent'))
      for (const sel of ['p', 'strong', 'li', 'code', 'td', 'th']) {
        const el = bubble.querySelector(sel)
        if (!el) throw new Error(`no <${sel}> in the agent bubble`)
        expect([sel, expectNoStyleLeak(el, 'color')]).toEqual([sel, agentText])
      }
      const link = bubble.querySelector('a')
      if (!link) throw new Error('no link in the agent bubble')
      expect(expectNoStyleLeak(link, 'color')).toBe(zenVar('accent'))
      const quote = bubble.querySelector('blockquote')
      if (!quote) throw new Error('no quote in the agent bubble')
      expect(expectNoStyleLeak(quote, 'color')).toBe(`color-mix(in srgb, ${agentText} 72%, transparent)`)
      const code = bubble.querySelector('code')
      if (!code) throw new Error('no code in the agent bubble')
      expect(expectNoStyleLeak(code, 'background')).toBe(`color-mix(in srgb, ${agentText} 10%, transparent)`)
      const meta = document.querySelector('[data-zen-message="a1"] [data-zen-message-meta]')
      if (!meta) throw new Error('no timestamp')
      expect(expectNoStyleLeak(meta, 'color')).toBe(zenVar('text-muted'))

      // The choice card (Thread's own component) reads Zen through its card tokens.
      const card = document.querySelector('[data-zen-message="a2"] [data-testid="thread-choice-card"]')
      if (!card) throw new Error('no choice card')
      const hops: Hop[] = []
      expect(resolveVars(card, 'var(--thread-card-text, var(--color-text-primary))', hops)).toBe(zenVar('text'))
      expect(resolveVars(card, 'var(--color-text-muted)', [])).toBe(`color-mix(in srgb, ${agentText} 72%, transparent)`)

      // My bubble: my text colour, links included (the bubble is the accent).
      await typeAndSend('Ship **it** with [this](https://example.com).')
      await waitFor(() => expect(document.querySelectorAll('[data-zen-message][data-mine]').length).toBe(1))
      const mine = document.querySelector('[data-zen-message][data-mine] [data-zen-bubble="me"]')
      if (!mine) throw new Error('no bubble of mine')
      const meText = zenVar('bubble-me-text')
      for (const sel of ['p', 'strong', 'a']) {
        const el = mine.querySelector(sel)
        if (!el) throw new Error(`no <${sel}> in my bubble`)
        expect([sel, expectNoStyleLeak(el, 'color')]).toEqual([sel, meText])
      }

      // Compose: box, attachment chip, and the placeholder rule.
      expect(expectNoStyleLeak(composeInput(), 'color')).toBe(zenVar('text'))
      await act(async () => {
        fireEvent.click(document.querySelector('[data-zen-attach]') as HTMLElement)
      })
      await waitFor(() => expect(document.querySelector('[data-zen-attachment]')).not.toBeNull())
      expect(expectNoStyleLeak(document.querySelector('[data-zen-attachment]') as Element, 'color')).toBe(zenVar('text'))
      expect(root.querySelector('style[data-zen-shield]')?.textContent).toContain(
        '[data-zen-root] ::placeholder { color: var(--zen-text-muted); opacity: 1; }',
      )

      // Agents list: name, preview and status.
      const row = rowEl(ROWS.sales.address)
      expect(expectNoStyleLeak(row.querySelector('[data-zen-agent-name]') as Element, 'color')).toBe(zenVar('text'))
      expect(expectNoStyleLeak(row.querySelector('[data-zen-preview]') as Element, 'color')).toBe(zenVar('text-muted'))

      // Cheat sheet.
      act(() => openZenCheatSheet())
      const sheet = document.querySelector('[data-zen-shortcut-sheet] [role="dialog"]')
      if (!sheet) throw new Error('no cheat sheet')
      expect(expectNoStyleLeak(sheet, 'color')).toBe(zenVar('text'))
      expect(expectNoStyleLeak(sheet.querySelector('kbd') as Element, 'color')).toBe(zenVar('text'))

      // Sweep: nothing in Zen reads a Styles variable from outside Zen.
      for (const el of Array.from(root.querySelectorAll('*'))) {
        if (el.tagName === 'STYLE' || el.namespaceURI !== 'http://www.w3.org/1999/xhtml') continue
        expectNoStyleLeak(el, 'color')
        expectNoStyleLeak(el, 'background')
      }
    }, 15_000)
  }
})

describe('Zen Add agent', () => {
  function addButton(): HTMLElement {
    const el = document.querySelector('[data-zen-widget="agents"] [data-zen-agents-footer] [data-zen-add-agent]')
    if (!(el instanceof HTMLElement)) throw new Error('no Add agent row in the Agents widget')
    return el
  }

  function picker(): HTMLElement | null {
    const el = document.querySelector('[data-zen-add-agent-picker]')
    return el instanceof HTMLElement ? el : null
  }

  function menuButton(label: string): HTMLElement {
    const p = picker()
    if (!p) throw new Error('picker not open')
    const b = Array.from(p.querySelectorAll('button')).find((x) => x.textContent?.includes(label))
    if (!(b instanceof HTMLElement)) throw new Error(`no "${label}" in the picker`)
    return b
  }

  it('opens the Home’s shared picker, adds to the Home the widget shows, and the row shows in the Agents widget', async () => {
    act(() => {
      useProjectsStore.setState({
        projects: [
          ...(useProjectsStore.getState().projects as never[]),
          { id: 'p2', name: 'atlas', handle: 'atlas', path: '/w/atlas', color: '#3366aa', workspaces: [{ id: 'w2', type: 'main', name: 'main' }] },
        ] as never,
      })
    })
    await mountZen()
    expect(picker()).toBeNull()

    await act(async () => {
      fireEvent.click(addButton())
    })
    const p = picker()
    if (!p) throw new Error('Add agent did not open the picker')
    // The regular Home's picker, for the current Home.
    expect(p.getAttribute('data-zen-add-agent-picker')).toBe('h1')
    expect(p.querySelector('[role="dialog"][aria-label="Add Agent"]')).not.toBeNull()
    expect(p.textContent).toContain('Add an agent to Work')
    menuButton('From a server')
    await act(async () => {
      fireEvent.click(menuButton('This server'))
    })
    // The shared searchable list: search box, cortana already on the Home
    // (checked), atlas pickable.
    expect(p.querySelector('[role="combobox"]')).not.toBeNull()
    expect(p.querySelector('[data-ws-filter-value="p1"]')?.getAttribute('data-row-state')).toBe('checked')
    const atlas = p.querySelector('[data-ws-filter-value="p2"]')
    if (!(atlas instanceof HTMLElement)) throw new Error('atlas is not in the picker')
    expect(atlas.getAttribute('data-row-state')).toBe('pickable')
    await act(async () => {
      fireEvent.click(atlas)
    })
    expect(selectedHome(useHomesStore.getState()).rows.map((r) => r.address)).toEqual([
      ROWS.cortana.address,
      ROWS.sales.address,
      ROWS.julie.address,
      ROWS.ops.address,
      'atlas::local',
    ])
    await waitFor(() => expect(rowAddresses()).toContain('atlas::local'))
    expect(rowEl('atlas::local').querySelector('[data-zen-agent-name]')?.textContent).toBe('atlas')
    // Rosson 2026-10-04: adding closes the picker and opens atlas, with the
    // caret in its message box.
    expect(picker()).toBeNull()
    await waitFor(() => expect(rowEl('atlas::local').getAttribute('data-selected')).toBe(''))
    await waitFor(() => expect(document.activeElement?.getAttribute('aria-label')).toBe('Message atlas'))
    expect(useZenAddAgentStore.getState().open).toBe(false)

    // The button toggles it closed (its press doesn't count as outside).
    await act(async () => {
      fireEvent.click(addButton())
    })
    expect(picker()).not.toBeNull()
    await act(async () => {
      fireEvent.mouseDown(addButton())
      fireEvent.click(addButton())
    })
    expect(picker()).toBeNull()

    // Esc closes it.
    await act(async () => {
      fireEvent.click(addButton())
    })
    expect(picker()).not.toBeNull()
    await act(async () => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(picker()).toBeNull()
    expect(useZenAddAgentStore.getState().open).toBe(false)

    // A click outside closes it.
    await act(async () => {
      fireEvent.click(addButton())
    })
    expect(picker()).not.toBeNull()
    const outside = document.querySelector('[data-zen-widget="conversation"]')
    if (!outside) throw new Error('no conversation widget')
    await act(async () => {
      fireEvent.mouseDown(outside)
    })
    expect(picker()).toBeNull()
  }, 10_000)

  it('closes when the widget’s Home changes and when leaving Zen, and stays shut on the next Zen-on', async () => {
    await mountZen()
    await act(async () => {
      fireEvent.click(addButton())
    })
    expect(picker()?.getAttribute('data-zen-add-agent-picker')).toBe('h1')

    // The widget's Home picker moving to another Home closes it.
    await pickWidgetHome('h2')
    await waitFor(() => expect(picker()).toBeNull())
    expect(useZenAddAgentStore.getState().open).toBe(false)
    // Opened again, it is for the widget's new Home; the Home page's own
    // selection never moved.
    await act(async () => {
      fireEvent.click(addButton())
    })
    expect(picker()?.getAttribute('data-zen-add-agent-picker')).toBe('h2')
    expect(useHomesStore.getState().selectedId).toBe('h1')
    await act(async () => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    await pickWidgetHome('h1')

    // Open, then leave Zen with the top-right toggle: gone, store closed.
    await act(async () => {
      fireEvent.click(addButton())
    })
    expect(picker()).not.toBeNull()
    const toggle = document.querySelector('[data-zen-top-right] [data-zen-switch]')
    if (!(toggle instanceof HTMLElement)) throw new Error('no Zen toggle top right')
    await act(async () => {
      fireEvent.click(toggle)
    })
    expect(useZenWindowStore.getState().on).toBe(false)
    await waitFor(() => expect(document.querySelector('[data-zen-root]')).toBeNull())
    expect(picker()).toBeNull()
    expect(useZenAddAgentStore.getState()).toMatchObject({ open: false, anchor: null })

    // Zen on again: the picker stays shut.
    act(() => useZenWindowStore.getState().setOn(true))
    await waitFor(() => expect(document.querySelector('[data-zen-add-agent]')).not.toBeNull())
    expect(picker()).toBeNull()
  }, 10_000)

  it('agents.add is a bridge verb behind agents:add: a widget without the cap is refused, one with it opens the same picker', async () => {
    await mountZen()
    const hostStub = {
      gardens: () => [{ id: 'g-default', name: 'Garden 1', index: 1 }],
      currentGardenId: () => 'g-default',
      switchGarden: () => {},
      createGarden: async () => {
        throw new Error('unused')
      },
      renameGarden: async () => {},
      deleteGarden: async () => {},
      homes: () => [],
      exit: () => {},
      controls: { bind: () => () => {}, bindings: () => [], wiringFailure: () => null, dispose: () => {} },
      // A v2-style widget on Garden 1's page: it acts for the
      // page's first Agents widget (Home h1).
      page: () => BUILTIN_TEXTING_PAGE,
    }
    const reader = createZenBridge(hostStub as never, { id: 'agents', caps: ['agents:read'] })
    let refused: unknown = null
    try {
      reader.call('agents.add')
    } catch (err) {
      refused = err
    }
    if (!(refused instanceof ZenBridgeError)) throw new Error('agents.add without agents:add was not refused')
    expect(refused.code).toBe('cap_not_granted')
    expect(picker()).toBeNull()

    // A v2-style widget granted agents:add opens the same picker (no
    // anchor: the bottom-left corner) and can toggle it shut.
    const adder = createZenBridge(hostStub as never, { id: 'v2', caps: ['agents:add'] })
    let opened: unknown = null
    await act(async () => {
      opened = adder.call('agents.add')
    })
    expect(opened).toBe(true)
    expect(picker()?.querySelector('[role="dialog"][aria-label="Add Agent"]')).not.toBeNull()
    let after: unknown = null
    await act(async () => {
      after = adder.call('agents.add', { toggle: true })
    })
    expect(after).toBe(false)
    expect(picker()).toBeNull()
    expect(() => adder.call('agents.add', 'nope')).toThrow(/options must be an object/)
  })
})

/** Render Zen and wait for the window's Garden page. */
async function mountGarden(template: string): Promise<void> {
  render(
    <>
      <Shortcuts />
      <ZenHost />
    </>,
  )
  await waitFor(() => {
    if (!document.querySelector(`[data-zen-page="${template}"]`)) throw new Error(`${template} not drawn`)
  })
}

function widgetRows(widget: string): string[] {
  const box = document.querySelector(`[data-zen-widget-id="${widget}"]`)
  if (!box) throw new Error(`no widget ${widget}`)
  return Array.from(box.querySelectorAll('[data-zen-agent-row]')).map((e) => e.getAttribute('data-zen-agent-row') ?? '')
}

describe('the Agents widget’s Home picker (G27, TG4.4)', () => {
  it('a pick changes its rows, never the Home page’s selection, is saved, and survives a remount', async () => {
    act(() => useHomesStore.getState().addRow('h2', ROWS.sales))
    await mountZen()
    expect(document.querySelector('[data-zen-home-picker-button]')?.textContent).toContain('Work')
    expect(useHomesStore.getState().selectedId).toBe('h1')
    await pickWidgetHome('h2')
    await waitFor(() => expect(widgetRows('agents')).toEqual([ROWS.sales.address]))
    expect(document.querySelector('[data-zen-home-picker-button]')?.textContent).toContain('Personal')
    // The Home page never moved.
    expect(useHomesStore.getState().selectedId).toBe('h1')
    expect(JSON.parse(localStorage.getItem(ZEN_GARDEN_HOMES_KEY) ?? 'null')).toEqual({
      version: 1,
      picks: { 'g-default/agents': 'h2' },
    })
    // Remount (leave Zen and come back): still Personal.
    act(() => useZenWindowStore.getState().setOn(false))
    await waitFor(() => expect(document.querySelector('[data-zen-root]')).toBeNull())
    act(() => useZenWindowStore.getState().setOn(true))
    await waitFor(() => expect(widgetRows('agents')).toEqual([ROWS.sales.address]))
  })

  it('switching the Home page’s Home doesn’t change the widget; another window’s pick (storage event) does', async () => {
    await mountZen()
    // Settles the widget's first Home as its own pick (the window's Home then).
    await waitFor(() => expect(useZenGardenHomesStore.getState().picks['g-default/agents']).toBe('h1'))
    act(() => useHomesStore.getState().selectHome('h2'))
    expect(rowAddresses()).toEqual([ROWS.cortana.address, ROWS.sales.address, ROWS.julie.address, ROWS.ops.address])
    act(() => {
      window.dispatchEvent(
        new StorageEvent('storage', {
          key: ZEN_GARDEN_HOMES_KEY,
          newValue: JSON.stringify({ version: 1, picks: { 'g-default/agents': 'h2' } }),
        }),
      )
    })
    await waitFor(() => expect(document.querySelector('[data-zen-agents-empty]')).not.toBeNull())
  })

  it('the Garden’s seed Home comes first', async () => {
    h.gardens[0].seedHome = 'h2'
    render(
      <>
        <Shortcuts />
        <ZenHost />
      </>,
    )
    await waitFor(() =>
      expect(document.querySelector('[data-zen-agents-empty]')?.textContent).toBe(
        'This Home has no agents yet. Use Add agent to add some.',
      ),
    )
    expect(document.querySelector('[data-zen-home-picker-button]')?.textContent).toContain('Personal')
    expect(useHomesStore.getState().selectedId).toBe('h1')
  })

  it('Add agent adds to the Home picked in the widget', async () => {
    act(() => {
      useProjectsStore.setState({
        projects: [
          ...(useProjectsStore.getState().projects as never[]),
          { id: 'p2', name: 'atlas', handle: 'atlas', path: '/w/atlas', color: '#3366aa', workspaces: [{ id: 'w2', type: 'main', name: 'main' }] },
        ] as never,
      })
    })
    await mountZen()
    await pickWidgetHome('h2')
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-widget="agents"] [data-zen-add-agent]') as HTMLElement)
    })
    const p = document.querySelector('[data-zen-add-agent-picker]')
    if (!(p instanceof HTMLElement)) throw new Error('Add agent did not open')
    expect(p.getAttribute('data-zen-add-agent-picker')).toBe('h2')
    expect(p.textContent).toContain('Add an agent to Personal')
    const thisServer = Array.from(p.querySelectorAll('button')).find((b) => b.textContent?.includes('This server'))
    if (!thisServer) throw new Error('no This server')
    await act(async () => {
      fireEvent.click(thisServer)
    })
    const atlas = p.querySelector('[data-ws-filter-value="p2"]')
    if (!(atlas instanceof HTMLElement)) throw new Error('atlas is not in the picker')
    await act(async () => {
      fireEvent.click(atlas)
    })
    const homes = useHomesStore.getState().homes
    expect(homes.find((x) => x.id === 'h2')?.rows.map((r) => r.address)).toEqual(['atlas::local'])
    expect(homes.find((x) => x.id === 'h1')?.rows.map((r) => r.address)).not.toContain('atlas::local')
    expect(useHomesStore.getState().selectedId).toBe('h1')
    await waitFor(() => expect(widgetRows('agents')).toEqual(['atlas::local']))
  })
})

describe('built-in widgets: modes and layout (G38, answer 5)', () => {
  beforeEach(() => {
    h.gardens.push({ id: 'g-layout', name: 'Layout', template: 'k2.blank@1' })
    h.pages['g-layout'] = {
      template: 'k2.blank@1',
      layout: { kind: 'columns', columns: [{ size: 60, 'min-width': 300 }, { size: 40 }] },
      widgets: [
        { id: 'talk', kind: 'conversation', column: 0, props: { agents: 'solo' }, caps: ['agents:read', 'thread:read', 'thread:post'], source: 'builtin' },
        // As the daemon sends them: every prop present, `mode` derived.
        { id: 'solo', kind: 'agents', column: 1, props: { mode: 'agent', home: 'Work', agent: 'sales', 'home-picker': false }, caps: ['agents:read'], source: 'builtin' },
        { id: 'whole', kind: 'agents', column: 1, props: { mode: 'home', home: 'Personal', 'home-picker': false }, caps: ['agents:read'], source: 'builtin' },
      ],
    }
    useHomesStore.getState().addRow('h2', ROWS.cortana)
    useZenWindowStore.setState({ garden: 'g-layout' })
  })

  it('places each widget in its column, with the Garden’s sizes', async () => {
    await mountGarden('k2.blank@1')
    const slot0 = document.querySelector('[data-zen-column-slot="0"]') as HTMLElement
    const slot1 = document.querySelector('[data-zen-column-slot="1"]') as HTMLElement
    expect(slot0.style.flex).toBe('60 1 0%')
    expect(slot0.style.minWidth).toBe('300px')
    expect(slot1.style.flex).toBe('40 1 0%')
    expect(Array.from(document.querySelectorAll('[data-zen-column="0"] [data-zen-widget]')).map((w) => w.getAttribute('data-zen-widget'))).toEqual([
      'conversation',
    ])
    expect(Array.from(document.querySelectorAll('[data-zen-column="1"] [data-zen-widget]')).map((w) => w.getAttribute('data-zen-widget-id'))).toEqual([
      'solo',
      'whole',
    ])
    // The blank template's controls stay: switcher and Zen toggle, no Add agent.
    expect(document.querySelector('[data-zen-garden-pill]')).not.toBeNull()
    expect(document.querySelector('[data-zen-switch]')).not.toBeNull()
    expect(document.querySelector('[data-zen-add-agent]')).toBeNull()
  })

  it('single-agent mode shows one agent filtered from its Home; whole-Home mode shows the Home', async () => {
    await mountGarden('k2.blank@1')
    await waitFor(() => expect(widgetRows('solo')).toEqual([ROWS.sales.address]))
    expect(document.querySelector('[data-zen-widget-id="solo"]')?.getAttribute('data-zen-agents-mode')).toBe('agent')
    expect(document.querySelector('[data-zen-widget-id="solo"]')?.getAttribute('data-zen-agents-home')).toBe('h1')
    expect(widgetRows('whole')).toEqual([ROWS.cortana.address])
    expect(document.querySelector('[data-zen-widget-id="whole"]')?.getAttribute('data-zen-agents-mode')).toBe('home')
    expect(document.querySelector('[data-zen-widget-id="whole"]')?.getAttribute('data-zen-agents-home')).toBe('h2')
    // No `home-picker` prop: no picker.
    expect(document.querySelector('[data-zen-home-picker]')).toBeNull()

    // The conversation follows the widget its `agents` prop names.
    await act(async () => {
      fireEvent.click(document.querySelector(`[data-zen-widget-id="solo"] [data-zen-agent-row="${ROWS.sales.address}"]`) as HTMLElement)
    })
    await waitFor(() =>
      expect(document.querySelector('[data-zen-column="0"] [data-zen-conversation]')?.getAttribute('data-zen-conversation')).toBe(
        ROWS.sales.address,
      ),
    )
    // Selecting in the other widget doesn't drive this conversation.
    await act(async () => {
      fireEvent.click(document.querySelector(`[data-zen-widget-id="whole"] [data-zen-agent-row="${ROWS.cortana.address}"]`) as HTMLElement)
    })
    expect(document.querySelector('[data-zen-column="0"] [data-zen-conversation]')?.getAttribute('data-zen-conversation')).toBe(
      ROWS.sales.address,
    )
  })

  it('`mode: "home"` shows the whole Home even with an `agent` set', async () => {
    h.pages['g-layout'].widgets = [
      { id: 'w', kind: 'agents', column: 0, props: { mode: 'home', home: 'Work', agent: 'sales' }, caps: ['agents:read'], source: 'builtin' },
    ]
    h.pages['g-layout'].layout = { kind: 'columns', split: [100] }
    await mountGarden('k2.blank@1')
    await waitFor(() => expect(widgetRows('w').length).toBe(4))
  })

  it('a Conversation pinned with `agent` + `home` opens that agent on its own, with no list', async () => {
    h.pages['g-layout'].widgets = [
      { id: 'pin', kind: 'conversation', column: 0, props: { agent: 'cortana', home: 'Work', compose: true, attachments: false, 'load-older': true }, caps: ['agents:read', 'thread:read', 'thread:post'], source: 'builtin' },
    ]
    h.pages['g-layout'].layout = { kind: 'columns', split: [100] }
    await mountGarden('k2.blank@1')
    await threadReady(ROWS.cortana.address)
    expect(document.querySelector('[data-zen-agent-row]')).toBeNull()
    // `attachments: false`: no "+" in the box.
    expect(document.querySelector('[data-zen-attach]')).toBeNull()
    expect(composeInput()).toBeTruthy()
  })

  it('display props: no preview, no server tag, only the statuses asked for, no compose box', async () => {
    h.pages['g-layout'].widgets = [
      { id: 'w', kind: 'agents', column: 0, props: { mode: 'home', home: 'Work', preview: false, 'server-tag': false, status: ['needs-you'] }, caps: ['agents:read'], source: 'builtin' },
      { id: 'c', kind: 'conversation', column: 1, props: { agents: 'w', compose: false }, caps: ['agents:read', 'thread:read', 'thread:post'], source: 'builtin' },
    ]
    h.pages['g-layout'].layout = { kind: 'columns', split: [50, 50] }
    await mountGarden('k2.blank@1')
    await waitFor(() => expect(widgetRows('w').length).toBe(4))
    // Previews arrive but aren't shown; the akzm tag isn't drawn; idle isn't shown.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30))
    })
    expect(document.querySelector('[data-zen-preview]')).toBeNull()
    expect(document.querySelector('[data-zen-server-tag]')).toBeNull()
    expect(rowEl(ROWS.cortana.address).getAttribute('data-activity')).toBe('unknown')
    await select(ROWS.cortana.address)
    await threadReady(ROWS.cortana.address)
    expect(document.querySelector('[data-zen-compose]')).toBeNull()
  })

  it('an agent that isn’t on the widget’s Home says so', async () => {
    h.pages['g-layout'].widgets = [
      { id: 'solo', kind: 'agents', column: 0, props: { home: 'Personal', agent: 'sales' }, caps: ['agents:read'], source: 'builtin' },
    ]
    h.pages['g-layout'].layout = { kind: 'columns', split: [100] }
    await mountGarden('k2.blank@1')
    await waitFor(() => expect(document.querySelector('[data-zen-agents-empty]')?.textContent).toBe('sales isn’t on Personal.'))
  })
})

describe('Garden shortcuts (G26, G52, TG3.6)', () => {
  function spyWorkspaceSwitch(): ReturnType<typeof vi.fn> {
    const spy = vi.fn()
    useProjectsStore.setState({
      projects: [
        { id: 'p1', name: 'cortana', handle: 'cortana', path: '/w/cortana', color: '#c2662d', pinned: true, workspaces: [{ id: 'w1', type: 'main', name: 'main' }] },
      ] as never,
      setActiveWorkspace: spy as never,
    })
    useTerminalSettingsStore.setState({ shortcutLayout: 'cmd-active-cmdshift-pinned' })
    return spy
  }

  it('in Zen on the Agents page, ⌘⌥2 switches to Garden 2 and never switches a workspace or a Home', async () => {
    const spy = spyWorkspaceSwitch()
    usePageViewStore.getState().setPage('agents')
    await mountZen()
    await act(async () => {
      fireEvent.keyDown(window, { key: '™', code: 'Digit2', metaKey: true, altKey: true })
    })
    expect(useZenWindowStore.getState().garden).toBe('g-notes')
    await waitFor(() => expect(document.querySelector('[data-zen-page="k2.blank@1"]')).not.toBeNull())
    expect(spy).not.toHaveBeenCalled()
    expect(useHomesStore.getState().selectedId).toBe('h1')
    expect(usePageViewStore.getState().page).toBe('agents')
    // ⌘⌥9 past the end does nothing.
    await act(async () => {
      fireEvent.keyDown(window, { key: 'ª', code: 'Digit9', metaKey: true, altKey: true })
    })
    expect(useZenWindowStore.getState().garden).toBe('g-notes')
    // ⌘1 in a Garden with no Agents widget does nothing.
    const before = h.calls.length
    await act(async () => {
      fireEvent.keyDown(window, { key: '1', code: 'Digit1', metaKey: true })
    })
    expect(h.calls.slice(before).filter((c) => c.route === 'thread' || c.route === 'sessions/list-for-workspace')).toEqual([])
    expect(spy).not.toHaveBeenCalled()
    // ⌘⌥1 back: ⌘1 opens row 1 of the first Agents widget.
    await act(async () => {
      fireEvent.keyDown(window, { key: '¡', code: 'Digit1', metaKey: true, altKey: true })
    })
    await waitFor(() => expect(document.querySelectorAll('[data-zen-agent-row]').length).toBe(4))
    await act(async () => {
      fireEvent.keyDown(window, { key: '1', code: 'Digit1', metaKey: true })
    })
    await waitFor(() => expect(rowEl(ROWS.cortana.address).hasAttribute('data-selected')).toBe(true))
    expect(spy).not.toHaveBeenCalled()
  })

  it('outside Zen, ⌘⌥1 is the Agents workspace switch again', async () => {
    const spy = spyWorkspaceSwitch()
    usePageViewStore.getState().setPage('agents')
    useZenWindowStore.setState({ on: false })
    render(<Shortcuts />)
    await act(async () => {
      fireEvent.keyDown(window, { key: '¡', code: 'Digit1', metaKey: true, altKey: true })
    })
    expect(spy).toHaveBeenCalledWith('p1', 'w1')
    expect(useZenWindowStore.getState().garden).toBe('g-default')
  })
})

describe('the empty Garden: Ask my agent (G28, TG5.1)', () => {
  beforeEach(() => {
    useZenWindowStore.setState({ garden: 'g-notes' })
    useProjectsStore.setState({
      projects: [
        ...(useProjectsStore.getState().projects as never[]),
        { id: 'p2', name: 'atlas', handle: 'atlas', path: '/w/atlas', color: '#3366aa', workspaces: [{ id: 'w2', type: 'main', name: 'main' }] },
      ] as never,
    })
  })

  it('says so, and offers only this computer’s agents', async () => {
    await mountGarden('k2.blank@1')
    expect(document.querySelector('[data-zen-ask-my-agent]')?.classList.contains('cursor-pointer')).toBe(true)
    expect(document.querySelector('[data-zen-garden-empty-title]')?.textContent).toBe('This Garden is empty.')
    expect(document.querySelector('[data-zen-garden-empty-ask]')?.textContent).toBe(
      'Ask your agents to add things to this Garden.',
    )
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-my-agent]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelectorAll('[data-zen-ask-agent]').length).toBe(2))
    const offered = Array.from(document.querySelectorAll('[data-zen-ask-agent]')).map((e) => e.getAttribute('data-zen-ask-agent'))
    // Home rows on this computer, then this computer's other workspaces;
    // never an agent on another server.
    expect(offered).toEqual(['cortana::local', 'atlas::local'])
    expect(offered).not.toContain(ROWS.sales.address)
  })

  it('a pick opens its conversation with the request drafted, not sent; sending posts it on this computer', async () => {
    await mountGarden('k2.blank@1')
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-my-agent]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelector('[data-zen-ask-agent="cortana::local"]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-agent="cortana::local"]') as HTMLElement)
    })
    await threadReady('cortana::local')
    const draft = zenGardenAskDraft({ id: 'g-notes', name: 'Notes' })
    expect(draft).toBe(
      'In my Zen Garden "Notes" (id g-notes), please add: \n(Use the k2-zen skill and k2 zen garden; check with k2 zen validate.)',
    )
    await waitFor(() => expect(composeInput().value).toBe(draft))
    // The caret waits after "please add: ".
    expect(document.activeElement).toBe(composeInput())
    expect(composeInput().selectionStart).toBe(draft.indexOf('\n'))
    expect(h.calls.some((c) => c.route === 'thread/post')).toBe(false)

    const finished = draft.replace('please add: ', 'please add: a clock')
    await typeAndSend(finished)
    await waitFor(() => expect(h.calls.filter((c) => c.route === 'thread/post').length).toBe(1))
    expect(h.calls.filter((c) => c.route === 'thread/post').map((c) => [c.hostKey, c.data])).toEqual([
      ['local', { addr: 'cortana-main', text: finished, via: 'compose' }],
    ])
  })

  it('leaving the Garden and coming back shows the empty page again', async () => {
    await mountGarden('k2.blank@1')
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-my-agent]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelector('[data-zen-ask-agent="atlas::local"]')).not.toBeNull())
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-agent="atlas::local"]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelector('[data-zen-asking="atlas::local"]')).not.toBeNull())
    act(() => useZenWindowStore.getState().setGarden('g-default'))
    await waitFor(() => expect(document.querySelectorAll('[data-zen-agent-row]').length).toBe(4))
    act(() => useZenWindowStore.getState().setGarden('g-notes'))
    await waitFor(() => expect(document.querySelector('[data-zen-ask-my-agent]')).not.toBeNull())
    expect(document.querySelector('[data-zen-asking]')).toBeNull()
  })

  it('with no agents on this computer it says so', async () => {
    useProjectsStore.setState({ projects: [] as never })
    useHomesStore.setState({
      homes: [
        { id: 'h1', name: 'Work', rows: [ROWS.sales, ROWS.julie] },
        { id: 'h2', name: 'Personal', rows: [] },
      ],
      selectedId: 'h1',
    })
    await mountGarden('k2.blank@1')
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-my-agent]') as HTMLElement)
    })
    await waitFor(() =>
      expect(document.querySelector('[data-zen-no-local-agents]')?.textContent).toBe(
        'No agents on this computer yet. Add one from Home.',
      ),
    )
    expect(document.querySelector('[data-zen-ask-agent]')).toBeNull()
  })

  it('on another server, this computer’s agents come from the local daemon’s list', async () => {
    h.localProjects = [{ id: 'lp9', name: 'nova', handle: 'nova', path: '/w/nova' }]
    act(() => useConnectHostStore.setState({ activeHost: useConnectHostStore.getState().hosts[0] }))
    await mountGarden('k2.blank@1')
    await act(async () => {
      fireEvent.click(document.querySelector('[data-zen-ask-my-agent]') as HTMLElement)
    })
    await waitFor(() => expect(document.querySelectorAll('[data-zen-ask-agent]').length).toBe(2))
    expect(Array.from(document.querySelectorAll('[data-zen-ask-agent]')).map((e) => e.getAttribute('data-zen-ask-agent'))).toEqual([
      'cortana::local',
      'nova::local',
    ])
    // One read of this computer's list (the window's own server keeps its own).
    expect(h.calls.filter((c) => c.route === 'projects/list' && c.hostKey === 'local').length).toBe(1)
  })
})

describe('S6 source ratchets', () => {
  const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')
  const read = (p: string): string => readFileSync(join(RENDERER, p), 'utf8')

  it('Zen Add agent reuses the Home picker and its add path (no copy)', () => {
    const zen = read('components/Zen/ZenAddAgent.tsx')
    expect(zen).toContain("import { AddAgentPicker } from '@/components/Home/HomeAddPanels'")
    expect(zen).toContain('<AddAgentPicker home={home} onAdded={noteZenAgentAdded} />')
    expect(zen).not.toContain('addRow(')
    expect(zen).not.toContain("from '@/components/ui/SearchableAgentList'")
    const panels = read('components/Home/HomeAddPanels.tsx')
    expect(panels).toContain('putHomeAvatarOnAdd(hostKey, address, w)')
  })

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
    for (const f of [
      'ZenAgentsWidget.tsx',
      'ZenConversationWidget.tsx',
      'ZenCompose.tsx',
      'ZenTextingControls.tsx',
      'ZenGardenEmptyWidget.tsx',
      'ZenNavRailWidget.tsx',
      'ZenProjectsViewWidget.tsx',
      'zen-widget-kit.tsx',
    ]) {
      const src = read(`components/Zen/widgets/${f}`)
      expect([f, /daemonCli|scopeForHost|primaryScope|primaryRoom|useOverlayThread|ServerScope|useFocusGroupsStore/.test(src)]).toEqual([f, false])
    }
  })

  it('Zen’s Tickets view is the Tickets page’s own board, the terminal hidden by context (no fork)', () => {
    const view = read('components/Zen/widgets/ZenTicketsViewWidget.tsx')
    expect(view).toContain("import { TicketsPageBoard } from '@/components/Feedback/FeedbackPage'")
    expect(view).toContain('<TicketRailTerminalContext.Provider value={false}>')
    expect(view).not.toContain('<TicketBoard')
    // Rosson 2026-10-04: no background gradient behind the Tickets view.
    expect(view).not.toMatch(/gradient\(/)
    const page = read('components/Feedback/FeedbackPage.tsx')
    expect(page).toContain('<TicketsPageBoardView data={data} />')
    // The Agents view uses the app's own focus-group dropdown.
    expect(read('components/Zen/widgets/ZenAgentsWidget.tsx')).toContain(
      "import FocusGroupDropdown from '@/components/Sidebar/FocusGroupDropdown'",
    )
  })
})
