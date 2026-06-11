// 0.39.39 (#675/#677) — renderer cutover from polling to daemon broadcasts.
//
// Each renderer consumer (assistant / active-agents / review-queue /
// ReviewPanel / CompanionSection) drops its `setInterval`/`setTimeout` poll
// and subscribes to the Wave B broadcast when the daemon supports it
// (`serverSupports('daemon-broadcasts')`), and KEEPS its polling fallback
// when it does not. This suite proves, per consumer:
//   - SUPPORTED  → it subscribes to its event AND does not install its poll.
//   - SUPPORTED  → its event handler triggers a refetch.
//   - UNSUPPORTED → it installs its poll (fallback) and does NOT subscribe.
//
// vitest env is `node` (no Tauri / WebSocket). We mock the session-events
// subscription helpers (the boundary this wave owns) + serverSupports, so
// importing the consumers is inert and we assert the wiring directly.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the consumers import) ───────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

// The capability gate — flipped per test.
const supports = vi.hoisted(() => ({ value: true }))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => supports.value),
}))

// The session-events helpers this wave owns. We record registered handlers
// so a test can fire the event the consumer subscribed to.
const ev = vi.hoisted(() => {
  type Fn = (...a: unknown[]) => void
  const reg = {
    llm: [] as Fn[],
    agent: [] as Fn[],
    tunnel: [] as Fn[],
    appHello: [] as Fn[],
    sessionAdded: [] as Fn[],
    sessionRemoved: [] as Fn[],
    reviewSubs: [] as Array<{ path: string; handlers: Record<string, Fn> }>,
    tabSubs: [] as Array<{ path: string; handlers: Record<string, Fn> }>,
  }
  return { reg }
})
vi.mock('@/stores/session-events', () => ({
  onLlmStatusChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.llm.push(fn)
    return () => void (ev.reg.llm = ev.reg.llm.filter((f) => f !== fn))
  }),
  onAgentStatusChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.agent.push(fn)
    return () => void (ev.reg.agent = ev.reg.agent.filter((f) => f !== fn))
  }),
  onTunnelStatusChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.tunnel.push(fn)
    return () => void (ev.reg.tunnel = ev.reg.tunnel.filter((f) => f !== fn))
  }),
  onAppHello: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.appHello.push(fn)
    return () => void (ev.reg.appHello = ev.reg.appHello.filter((f) => f !== fn))
  }),
  onSessionAddedApp: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.sessionAdded.push(fn)
    return () => void (ev.reg.sessionAdded = ev.reg.sessionAdded.filter((f) => f !== fn))
  }),
  onSessionRemovedApp: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.reg.sessionRemoved.push(fn)
    return () => void (ev.reg.sessionRemoved = ev.reg.sessionRemoved.filter((f) => f !== fn))
  }),
  subscribeToWorkspaceReviewEvents: vi.fn(
    (path: string, handlers: Record<string, (...a: unknown[]) => void>) => {
      const entry = { path, handlers }
      ev.reg.reviewSubs.push(entry)
      return () => void (ev.reg.reviewSubs = ev.reg.reviewSubs.filter((e) => e !== entry))
    },
  ),
  subscribeToWorkspaceTabEvents: vi.fn(
    (path: string, handlers: Record<string, (...a: unknown[]) => void>) => {
      const entry = { path, handlers }
      ev.reg.tabSubs.push(entry)
      return () => void (ev.reg.tabSubs = ev.reg.tabSubs.filter((e) => e !== entry))
    },
  ),
  // tabs.ts's active-workspace subscription also opens the session-events
  // WS; inert here (orthogonal to the tab-title/order wiring under test).
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
}))

// Daemon transport boundary — keep every fire-and-forget fetch inert so a
// store's module-load / startup side effects (settings fetch, agents/running
// snapshot) don't reach the network in node and reject.
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 0, token: 't', secure: false, host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:0'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:0'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))
// Daemon CLI boundary — controllable per test. `cli.getImpl` lets a test
// stub a GET response (e.g. the workspace-layouts/load re-fetch); posts are
// recorded so a test can assert `workspace/set-tab-title` fired.
const cli = vi.hoisted(() => ({
  posts: [] as Array<{ route: string; body: unknown }>,
  getImpl: (async () => []) as (route: string, params?: unknown) => Promise<unknown>,
  postImpl: (async () => ({})) as (route: string, body?: unknown) => Promise<unknown>,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string, params?: unknown) => cli.getImpl(route, params)),
  daemonCliPost: vi.fn(async (route: string, body?: unknown) => {
    cli.posts.push({ route, body })
    return cli.postImpl(route, body)
  }),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
  terminalCreate: vi.fn(async () => undefined),
  terminalExists: vi.fn(async () => false),
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: Object.assign(vi.fn(() => undefined), {
    getState: () => ({ defaultAgent: null, agenticSystemsEnabled: true, fetchSettings: vi.fn() }),
    setState: vi.fn(),
    subscribe: vi.fn(() => () => undefined),
  }),
}))

// `startAgentPolling` fire-and-forgets `import('@tauri-apps/api/event')` to
// wire legacy hook/CLI listeners. The dynamic import resolves the REAL Tauri
// transport, which reaches for `window` and throws in node. That wiring is
// orthogonal to the broadcast cutover under test; stub a minimal `window` so
// `listen(...)` is inert instead of producing unhandled rejections.
vi.stubGlobal('window', {
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  __TAURI_INTERNALS__: { transformCallback: () => 0, invoke: async () => undefined },
})

// localStorage stub (some stores load it transitively in node).
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

beforeEach(() => {
  supports.value = true
  ev.reg.llm = []
  ev.reg.agent = []
  ev.reg.tunnel = []
  ev.reg.appHello = []
  ev.reg.sessionAdded = []
  ev.reg.sessionRemoved = []
  ev.reg.reviewSubs = []
  ev.reg.tabSubs = []
  cli.posts = []
  cli.getImpl = async () => []
  cli.postImpl = async () => ({})
  vi.clearAllTimers()
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
})

// ── active-agents (#675.2) ──────────────────────────────────────────────

describe('active-agents — agent_status_changed cutover', () => {
  it('subscribes (no interval) when supported and refetches via lifecycle handler', async () => {
    vi.useFakeTimers()
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore } = await import(
      './active-agents'
    )
    const handle = vi
      .spyOn(useActiveAgentsStore.getState(), 'handleLifecycleEvent')
      .mockImplementation(() => undefined)
    // Also stub pollOnce so the initial snapshot doesn't hit the daemon.
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()
    expect(ev.reg.agent.length).toBe(1)
    expect(setInterval).not.toHaveBeenCalled()

    // Firing the subscribed event maps through the canonical lifecycle path.
    ev.reg.agent[0]({ paneId: 'p1', tabId: 't1', status: 'start' })
    expect(handle).toHaveBeenCalledWith('p1', 't1', 'start')

    stopAgentPolling()
  })

  it('falls back to the poll interval when NOT supported (no subscription)', async () => {
    vi.useFakeTimers()
    supports.value = false
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore } = await import(
      './active-agents'
    )
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()
    expect(ev.reg.agent.length).toBe(0)
    expect(setInterval).toHaveBeenCalledTimes(1)

    stopAgentPolling()
  })
})

// ── #688 — live-session dot from SessionAdded/SessionRemoved (NO poll) ─────
//
// Bug: the Active-bar "live session" dot stayed grey for a daemon-owned
// pinned chat opened AFTER startup. `liveSessionCwds` was rebuilt only in
// `pollOnce`, which (post Wave C-1) runs only at startup + reconnect. The
// fix subscribes APP-LEVEL to the daemon's SessionAdded/SessionRemoved and
// keeps `liveSessionCwds` fresh push-style. These tests prove the cwd is
// added on SessionAdded (dot turns green) and removed on SessionRemoved —
// with NO poll firing.

describe('active-agents — live-session dot push (#688)', () => {
  it('SessionAdded adds the cwd to liveSessionCwds (dot turns green, no poll)', async () => {
    vi.useFakeTimers()
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore, projectHasLiveSession } =
      await import('./active-agents')
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()
    expect(ev.reg.sessionAdded.length).toBe(1)
    expect(ev.reg.sessionRemoved.length).toBe(1)

    // Baseline: workspace cwd is NOT live.
    expect(
      projectHasLiveSession(
        useActiveAgentsStore.getState().liveSessionCwds,
        '/ws/pinned',
      ),
    ).toBe(false)

    // A daemon-owned pinned chat opened after startup → SessionAdded.
    ev.reg.sessionAdded[0]({
      kind: 'session_added',
      workspace_path: '/ws/pinned',
      pane_group_id: null,
      agent_name: 'projP',
      command: 'claude',
      args: [],
      session_id: 'sess-1',
      isV2: true,
    })

    // Dot turns green WITHOUT a poll.
    expect(useActiveAgentsStore.getState().liveSessionCwds.has('/ws/pinned')).toBe(true)
    expect(
      projectHasLiveSession(
        useActiveAgentsStore.getState().liveSessionCwds,
        '/ws/pinned',
      ),
    ).toBe(true)

    stopAgentPolling()
  })

  it('SessionRemoved removes the cwd from liveSessionCwds (dot greys)', async () => {
    vi.useFakeTimers()
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore } = await import(
      './active-agents'
    )
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()

    ev.reg.sessionAdded[0]({
      kind: 'session_added',
      workspace_path: '/ws/pinned',
      pane_group_id: null,
      agent_name: 'projP',
      command: 'claude',
      args: [],
      session_id: 'sess-1',
      isV2: true,
    })
    expect(useActiveAgentsStore.getState().liveSessionCwds.has('/ws/pinned')).toBe(true)

    // The pinned chat exits → SessionRemoved (carries the same cwd + key).
    ev.reg.sessionRemoved[0]({
      kind: 'session_removed',
      workspace_path: '/ws/pinned',
      pane_group_id: null,
      agent_name: 'projP',
    })
    expect(useActiveAgentsStore.getState().liveSessionCwds.has('/ws/pinned')).toBe(false)

    stopAgentPolling()
  })

  it('SessionRemoved keeps the cwd green while another session shares it', async () => {
    vi.useFakeTimers()
    const { startAgentPolling, stopAgentPolling, useActiveAgentsStore } = await import(
      './active-agents'
    )
    vi.spyOn(useActiveAgentsStore.getState(), 'pollOnce').mockResolvedValue(undefined)

    startAgentPolling()

    // Two live sessions in the SAME cwd.
    ev.reg.sessionAdded[0]({
      kind: 'session_added', workspace_path: '/ws/shared', pane_group_id: null,
      agent_name: 'keyA', command: 'claude', args: [], session_id: 's-a', isV2: true,
    })
    ev.reg.sessionAdded[0]({
      kind: 'session_added', workspace_path: '/ws/shared', pane_group_id: 'tab-x',
      agent_name: 'tab-x', command: 'claude', args: [], session_id: 's-b', isV2: true,
    })
    expect(useActiveAgentsStore.getState().liveSessionCwds.has('/ws/shared')).toBe(true)

    // Closing ONE must not grey the dot — the other still lives there.
    ev.reg.sessionRemoved[0]({
      kind: 'session_removed', workspace_path: '/ws/shared', pane_group_id: null,
      agent_name: 'keyA',
    })
    expect(useActiveAgentsStore.getState().liveSessionCwds.has('/ws/shared')).toBe(true)

    // Closing the last one greys it.
    ev.reg.sessionRemoved[0]({
      kind: 'session_removed', workspace_path: '/ws/shared', pane_group_id: 'tab-x',
      agent_name: 'tab-x',
    })
    expect(useActiveAgentsStore.getState().liveSessionCwds.has('/ws/shared')).toBe(false)

    stopAgentPolling()
  })
})

// ── review-queue (#675.3) ───────────────────────────────────────────────

describe('review-queue — review_changed cutover', () => {
  it('subscribes (no interval) when supported and refetches on event', async () => {
    vi.useFakeTimers()
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startReviewQueuePolling, stopReviewQueuePolling, useReviewQueueStore } =
      await import('./review-queue')
    const { useProjectsStore } = await import('./projects')
    // Give the store an active project so it subscribes on a real path.
    useProjectsStore.setState({
      activeProjectId: 'pr1',
      projects: [{ id: 'pr1', path: '/ws/foo', workspaces: [] }] as never,
    })
    const fetchAll = vi
      .spyOn(useReviewQueueStore.getState(), 'fetchAll')
      .mockResolvedValue(undefined)

    startReviewQueuePolling()
    expect(setInterval).not.toHaveBeenCalled()
    expect(ev.reg.reviewSubs.length).toBe(1)
    expect(ev.reg.reviewSubs[0].path).toBe('/ws/foo')

    const callsBefore = fetchAll.mock.calls.length
    ev.reg.reviewSubs[0].handlers.onReviewChanged({
      kind: 'review_changed',
      workspacePath: '/ws/foo',
      agent: null,
    })
    expect(fetchAll.mock.calls.length).toBe(callsBefore + 1)

    stopReviewQueuePolling()
  })

  it('falls back to the 30s poll when NOT supported (no subscription)', async () => {
    vi.useFakeTimers()
    supports.value = false
    const setInterval = vi.spyOn(globalThis, 'setInterval')
    const { startReviewQueuePolling, stopReviewQueuePolling, useReviewQueueStore } =
      await import('./review-queue')
    vi.spyOn(useReviewQueueStore.getState(), 'fetchAll').mockResolvedValue(undefined)

    startReviewQueuePolling()
    expect(ev.reg.reviewSubs.length).toBe(0)
    expect(setInterval).toHaveBeenCalledTimes(1)
    expect(setInterval).toHaveBeenCalledWith(expect.any(Function), 30_000)

    stopReviewQueuePolling()
  })
})

// ── tabs: tab titles (#676) ─────────────────────────────────────────────

// Seed a live workspace into backgroundWorkspaces and restore it so the
// fast path registers the tab-events subscription synchronously (no daemon
// GET dance). Returns the registered handler bundle + the tab id.
async function openWorkspaceWithLiveTab(
  tabsMod: typeof import('./tabs'),
  key: string,
  cwd: string,
): Promise<{ tabId: string; handlers: Record<string, (...a: unknown[]) => void> }> {
  const { useTabsStore } = tabsMod
  const tabId = 'tab-1'
  useTabsStore.setState({
    tabs: [],
    extraGroups: [],
    activeTabId: null,
    activeWorkspaceKey: null,
    backgroundWorkspaces: {
      [key]: {
        tabs: [
          {
            id: tabId,
            title: 'Original',
            mosaicTree: 'pg-1',
            paneGroups: new Map([['pg-1', { id: 'pg-1', items: [], activeItemIndex: 0 }]]),
          },
        ],
        activeTabId: tabId,
        extraGroups: [],
        splitCount: 1,
        activeGroupIndex: 0,
      },
    } as never,
  })
  await useTabsStore.getState().restoreWorkspace(key, cwd)
  expect(ev.reg.tabSubs.length).toBe(1)
  return { tabId, handlers: ev.reg.tabSubs[0].handlers }
}

describe('tabs — tab-title daemon-canonical (#676)', () => {
  it('setTabTitle POSTs workspace/set-tab-title when supported', async () => {
    const tabsMod = await import('./tabs')
    const { useTabsStore } = tabsMod
    const { tabId } = await openWorkspaceWithLiveTab(tabsMod, 'projA:wsA', '/ws/a')

    cli.posts = [] // ignore the restore's layout saves
    useTabsStore.getState().setTabTitle(tabId, 'Renamed')

    const post = cli.posts.find((p) => p.route === 'workspace/set-tab-title')
    expect(post).toBeDefined()
    expect(post!.body).toMatchObject({ projectId: 'projA', tabId, title: 'Renamed' })
    // Local apply happened too.
    const t = useTabsStore.getState().tabs.find((x) => x.id === tabId)
    expect(t?.title).toBe('Renamed')
  })

  it('does NOT POST set-tab-title when unsupported (local-only fallback)', async () => {
    supports.value = false
    const tabsMod = await import('./tabs')
    const { useTabsStore } = tabsMod
    // Unsupported: no tab-events sub opens; drive setTabTitle on a plain tab.
    useTabsStore.setState({
      tabs: [
        {
          id: 'tab-x',
          title: 'Original',
          mosaicTree: 'pg-x',
          paneGroups: new Map([['pg-x', { id: 'pg-x', items: [], activeItemIndex: 0 }]]),
        },
      ] as never,
      extraGroups: [],
      activeTabId: 'tab-x',
      activeWorkspaceKey: 'projA:wsA',
    })
    cli.posts = []
    useTabsStore.getState().setTabTitle('tab-x', 'Renamed')

    expect(cli.posts.find((p) => p.route === 'workspace/set-tab-title')).toBeUndefined()
    expect(useTabsStore.getState().tabs.find((x) => x.id === 'tab-x')?.title).toBe('Renamed')
  })

  it('tab_title_changed event applies the title locally without re-POST', async () => {
    const tabsMod = await import('./tabs')
    const { useTabsStore } = tabsMod
    const { tabId, handlers } = await openWorkspaceWithLiveTab(tabsMod, 'projA:wsA', '/ws/a')

    cli.posts = []
    handlers.onTabTitleChanged({
      kind: 'tab_title_changed',
      workspacePath: '/ws/a',
      project: '/ws/a',
      tabId,
      title: 'FromOtherClient',
    })

    expect(useTabsStore.getState().tabs.find((x) => x.id === tabId)?.title).toBe('FromOtherClient')
    // Applying a remote rename must NOT echo a POST back (no feedback loop).
    expect(cli.posts.find((p) => p.route === 'workspace/set-tab-title')).toBeUndefined()
  })
})

// ── tabs: tab-order revision LWW (#677.3) ───────────────────────────────

describe('tabs — tab-order revision conflict (#677.3)', () => {
  it('skips a stale tab_order_changed (revision <= base) — no layout re-fetch', async () => {
    const tabsMod = await import('./tabs')
    const { __setLayoutRevisionForTests, __getLayoutRevisionForTests } = tabsMod
    const { handlers } = await openWorkspaceWithLiveTab(tabsMod, 'projB:wsB', '/ws/b')

    __setLayoutRevisionForTests('projB:wsB', 5)
    const getSpy = vi.fn(async () => null)
    cli.getImpl = getSpy as never

    // A broadcast at-or-below our base = our own (or older) write — skip.
    handlers.onTabOrderChanged({
      kind: 'tab_order_changed',
      workspacePath: '/ws/b',
      project: 'projB',
      workspace: 'wsB',
      revision: 5,
    })

    expect(getSpy).not.toHaveBeenCalled()
    expect(__getLayoutRevisionForTests('projB:wsB')).toBe(5)
  })

  it('re-fetches the layout on a newer tab_order_changed (remote reorder)', async () => {
    const tabsMod = await import('./tabs')
    const { useTabsStore, __setLayoutRevisionForTests, __getLayoutRevisionForTests } = tabsMod
    const { handlers } = await openWorkspaceWithLiveTab(tabsMod, 'projB:wsB', '/ws/b')

    __setLayoutRevisionForTests('projB:wsB', 2)
    const remoteLayout = {
      version: 2,
      tabs: [
        { id: 'tab-remote', title: 'Remote', mosaicTree: 'pg-r', paneGroups: { 'pg-r': { id: 'pg-r', items: [], activeItemIndex: 0 } } },
      ],
      activeTabId: 'tab-remote',
      splitCount: 1,
      activeGroupIndex: 0,
    }
    cli.getImpl = async (route: string) =>
      route === 'workspace-layouts/load' ? JSON.stringify(remoteLayout) : null

    handlers.onTabOrderChanged({
      kind: 'tab_order_changed',
      workspacePath: '/ws/b',
      project: 'projB',
      workspace: 'wsB',
      revision: 7,
    })
    // Let the async re-fetch (GET → JSON.parse → restoreLayout) settle.
    await new Promise((r) => setTimeout(r, 0))
    await new Promise((r) => setTimeout(r, 0))

    // Base advanced to the remote revision, and the remote layout was
    // adopted (restoreLayout assigns fresh tab ids, so match on title).
    expect(__getLayoutRevisionForTests('projB:wsB')).toBe(7)
    const tabs = useTabsStore.getState().tabs
    expect(tabs.length).toBe(1)
    expect(tabs[0].title).toBe('Remote')
  })
})

// ── tabs: silent remote-reorder adoption — no echo loop (#676/#677) ──────
//
// The 0.39.39 regression: with ≥2 clients on one daemon, adopting a peer's
// reorder triggered a fresh `workspace-layouts/save` (via the store's
// debounced autosave subscription firing on the `restoreLayout` set), which
// bumped the revision and re-broadcast `tab_order_changed` → the other client
// adopted → re-saved → infinite ping-pong. The fix makes adoption SILENT.
describe('tabs — silent remote-reorder adoption (no echo loop) (#676/#677)', () => {
  it('adopts a newer tab_order_changed WITHOUT echoing workspace-layouts/save', async () => {
    vi.useFakeTimers()
    const tabsMod = await import('./tabs')
    const { useTabsStore, __setLayoutRevisionForTests } = tabsMod
    const { handlers } = await openWorkspaceWithLiveTab(tabsMod, 'projB:wsB', '/ws/b')

    __setLayoutRevisionForTests('projB:wsB', 2)
    const remoteLayout = {
      version: 2,
      tabs: [
        { id: 'tab-remote', title: 'Remote', mosaicTree: 'pg-r', paneGroups: { 'pg-r': { id: 'pg-r', items: [], activeItemIndex: 0 } } },
      ],
      activeTabId: 'tab-remote',
      splitCount: 1,
      activeGroupIndex: 0,
    }
    cli.getImpl = async (route: string) =>
      route === 'workspace-layouts/load' ? JSON.stringify(remoteLayout) : null

    // Clear the restore's own (legitimate) layout saves before the adoption.
    cli.posts = []

    handlers.onTabOrderChanged({
      kind: 'tab_order_changed',
      workspacePath: '/ws/b',
      project: 'projB',
      workspace: 'wsB',
      revision: 7,
    })

    // Flush the async re-fetch (GET → JSON.parse → restoreLayout) AND advance
    // past the 1000ms autosave debounce window. If the adoption echoed, the
    // debounced `workspace-layouts/save` would have fired by now.
    await vi.runAllTimersAsync()

    // The peer's layout was adopted locally...
    const tabs = useTabsStore.getState().tabs
    expect(tabs.length).toBe(1)
    expect(tabs[0].title).toBe('Remote')

    // ...but the adopting client did NOT echo a save back (the loop-breaker).
    expect(cli.posts.find((p) => p.route === 'workspace-layouts/save')).toBeUndefined()
  })

  it('two-client convergence: a second adoption at the same revision is a no-op (no re-fetch, no save)', async () => {
    vi.useFakeTimers()
    const tabsMod = await import('./tabs')
    const { __setLayoutRevisionForTests, __getLayoutRevisionForTests } = tabsMod
    const { handlers } = await openWorkspaceWithLiveTab(tabsMod, 'projB:wsB', '/ws/b')

    __setLayoutRevisionForTests('projB:wsB', 2)
    const remoteLayout = {
      version: 2,
      tabs: [
        { id: 'tab-remote', title: 'Remote', mosaicTree: 'pg-r', paneGroups: { 'pg-r': { id: 'pg-r', items: [], activeItemIndex: 0 } } },
      ],
      activeTabId: 'tab-remote',
      splitCount: 1,
      activeGroupIndex: 0,
    }
    const getSpy = vi.fn(async (route: string) =>
      route === 'workspace-layouts/load' ? JSON.stringify(remoteLayout) : null,
    )
    cli.getImpl = getSpy as never
    cli.posts = []

    // First adoption: base 2 < revision 7 → fetch + adopt, base advances to 7.
    handlers.onTabOrderChanged({
      kind: 'tab_order_changed', workspacePath: '/ws/b', project: 'projB', workspace: 'wsB', revision: 7,
    })
    await vi.runAllTimersAsync()
    expect(__getLayoutRevisionForTests('projB:wsB')).toBe(7)
    const fetchesAfterFirst = getSpy.mock.calls.length

    // Second client re-broadcasts at the SAME revision (the daemon echoes the
    // canonical state). Because the adoption did NOT bump the revision, this is
    // <= our base → a pure no-op: no re-fetch, no save. This is what makes two
    // clients converge instead of ping-ponging.
    handlers.onTabOrderChanged({
      kind: 'tab_order_changed', workspacePath: '/ws/b', project: 'projB', workspace: 'wsB', revision: 7,
    })
    await vi.runAllTimersAsync()

    expect(getSpy.mock.calls.length).toBe(fetchesAfterFirst) // no new layout fetch
    expect(cli.posts.find((p) => p.route === 'workspace-layouts/save')).toBeUndefined()
    expect(__getLayoutRevisionForTests('projB:wsB')).toBe(7)
  })

  it('does NOT suppress a legitimate user-initiated save (drag-reorder) outside the adoption window', async () => {
    vi.useFakeTimers()
    const tabsMod = await import('./tabs')
    const { useTabsStore } = tabsMod
    await openWorkspaceWithLiveTab(tabsMod, 'projB:wsB', '/ws/b')
    cli.posts = []

    // A normal local layout save (e.g. drag-reorder commit) must still POST.
    useTabsStore.getState().saveLayoutForWorkspace('projB', 'wsB')
    await vi.runAllTimersAsync()

    expect(cli.posts.find((p) => p.route === 'workspace-layouts/save')).toBeDefined()
  })
})

// ── tabs: in-place reorder on remote adoption — no remount/flicker (#676/#677)
//
// Companion to the silent-adoption fix: a PURE reorder of the current tab set
// (same tabs, different order) must permute the EXISTING `Tab` objects in
// place — preserving each tab's `id` (the React key) and its live
// `paneGroups`/terminals — instead of the full `restoreLayout` rebuild (which
// re-mints every tab/pane id → React unmounts+remounts every terminal AND the
// pinned-chat pane → a full terminal reload/flicker for a single reorder).
// `restoreLayout` is the fallback ONLY when the tab SET actually differs.
describe('tabs — in-place reorder on remote adoption (no remount) (#676/#677)', () => {
  // Seed a live workspace with TWO tabs (each owning one paneGroup) so a
  // reorder is meaningful. After restore, tab ids are re-minted but the main-
  // group paneGroupIds are preserved ("Reuse the saved ID"), which is the
  // stable identity the in-place reorder matches on. Returns the live tab
  // ids/pgIds (post-restore) + the registered handler bundle.
  async function openWorkspaceWithTwoLiveTabs(
    tabsMod: typeof import('./tabs'),
    key: string,
    cwd: string,
  ): Promise<{
    tabIds: string[]
    pgIds: string[]
    handlers: Record<string, (...a: unknown[]) => void>
  }> {
    const { useTabsStore } = tabsMod
    useTabsStore.setState({
      tabs: [],
      extraGroups: [],
      activeTabId: null,
      activeWorkspaceKey: null,
      backgroundWorkspaces: {
        [key]: {
          tabs: [
            {
              id: 'seed-1',
              title: 'One',
              mosaicTree: 'pg-1',
              paneGroups: new Map([['pg-1', { id: 'pg-1', items: [], activeItemIndex: 0 }]]),
            },
            {
              id: 'seed-2',
              title: 'Two',
              mosaicTree: 'pg-2',
              paneGroups: new Map([['pg-2', { id: 'pg-2', items: [], activeItemIndex: 0 }]]),
            },
          ],
          activeTabId: 'seed-1',
          extraGroups: [],
          splitCount: 1,
          activeGroupIndex: 0,
        },
      } as never,
    })
    await useTabsStore.getState().restoreWorkspace(key, cwd)
    expect(ev.reg.tabSubs.length).toBe(1)
    const liveTabs = useTabsStore.getState().tabs
    expect(liveTabs.length).toBe(2)
    return {
      tabIds: liveTabs.map((t) => t.id),
      pgIds: liveTabs.map((t) => Array.from(t.paneGroups.keys())[0]),
      handlers: ev.reg.tabSubs[0].handlers,
    }
  }

  // A serialized tab carrying a single paneGroup keyed by `pgId` — the shape
  // the daemon persists and `workspace-layouts/load` returns.
  function serializedTabFor(pgId: string, title: string) {
    return {
      id: `serialized-${pgId}`,
      title,
      mosaicTree: pgId,
      paneGroups: { [pgId]: { id: pgId, items: [], activeItemIndex: 0 } },
    }
  }

  it('PURE reorder permutes the SAME tab ids in place — no new ids, no save POST', async () => {
    vi.useFakeTimers()
    const tabsMod = await import('./tabs')
    const { useTabsStore, __setLayoutRevisionForTests } = tabsMod
    const { tabIds, pgIds, handlers } = await openWorkspaceWithTwoLiveTabs(
      tabsMod,
      'projC:wsC',
      '/ws/c',
    )

    __setLayoutRevisionForTests('projC:wsC', 2)
    // Peer reordered: the SAME two paneGroups in SWAPPED order.
    const remoteLayout = {
      version: 2,
      tabs: [serializedTabFor(pgIds[1], 'Two'), serializedTabFor(pgIds[0], 'One')],
      activeTabId: `serialized-${pgIds[0]}`,
      splitCount: 1,
      activeGroupIndex: 0,
    }
    cli.getImpl = async (route: string) =>
      route === 'workspace-layouts/load' ? JSON.stringify(remoteLayout) : null
    cli.posts = []

    handlers.onTabOrderChanged({
      kind: 'tab_order_changed',
      workspacePath: '/ws/c',
      project: 'projC',
      workspace: 'wsC',
      revision: 7,
    })
    await vi.runAllTimersAsync()

    const after = useTabsStore.getState().tabs
    // Same SET of ids, reversed order — a permutation of the EXISTING tabs.
    expect(after.map((t) => t.id)).toEqual([tabIds[1], tabIds[0]])
    // The exact same Set of ids (no id was minted / dropped → no remount).
    expect(new Set(after.map((t) => t.id))).toEqual(new Set(tabIds))
    // paneGroupIds (and thus the live terminals) were reused, not re-minted.
    expect(after.map((t) => Array.from(t.paneGroups.keys())[0])).toEqual([pgIds[1], pgIds[0]])
    // The active tab object stayed active (its id is unchanged in place).
    expect(useTabsStore.getState().activeTabId).toBe(tabIds[0])
    // Adoption was visual-only — NO save echoed back to the daemon.
    expect(cli.posts.find((p) => p.route === 'workspace-layouts/save')).toBeUndefined()
  })

  it('falls back to restoreLayout (rebuild) when the tab SET differs (tab added)', async () => {
    vi.useFakeTimers()
    const tabsMod = await import('./tabs')
    const { useTabsStore, __setLayoutRevisionForTests } = tabsMod
    const { tabIds, pgIds, handlers } = await openWorkspaceWithTwoLiveTabs(
      tabsMod,
      'projC:wsC',
      '/ws/c',
    )

    __setLayoutRevisionForTests('projC:wsC', 2)
    // Peer ADDED a third tab (new paneGroup) — the SET differs, so the
    // in-place path must NOT apply; the full rebuild adopts the new tab.
    const remoteLayout = {
      version: 2,
      tabs: [
        serializedTabFor(pgIds[0], 'One'),
        serializedTabFor(pgIds[1], 'Two'),
        serializedTabFor('pg-3-new', 'Three'),
      ],
      activeTabId: 'serialized-pg-3-new',
      splitCount: 1,
      activeGroupIndex: 0,
    }
    cli.getImpl = async (route: string) =>
      route === 'workspace-layouts/load' ? JSON.stringify(remoteLayout) : null
    cli.posts = []

    handlers.onTabOrderChanged({
      kind: 'tab_order_changed',
      workspacePath: '/ws/c',
      project: 'projC',
      workspace: 'wsC',
      revision: 7,
    })
    await vi.runAllTimersAsync()

    const after = useTabsStore.getState().tabs
    // The added tab was adopted → 3 tabs now.
    expect(after.length).toBe(3)
    expect(after.map((t) => t.title)).toEqual(['One', 'Two', 'Three'])
    // restoreLayout re-mints tab ids on the rebuild path — the old ids are gone
    // (this is the remount path we accept only when the set changed).
    for (const oldId of tabIds) {
      expect(after.some((t) => t.id === oldId)).toBe(false)
    }
    // Still silent — adoption never echoes a save (the #1 invariant holds on
    // the fallback path too).
    expect(cli.posts.find((p) => p.route === 'workspace-layouts/save')).toBeUndefined()
  })
})

// ── heartbeat-sessions: heartbeat_state_changed (#677.1) ────────────────

describe('heartbeat-sessions — heartbeat-live cutover (#677.1)', () => {
  function seedRow(store: typeof import('./heartbeat-sessions')['useHeartbeatSessionsStore']) {
    store.setState({
      active: [
        {
          row: {
            id: 'hb-1',
            projectId: 'projH',
            name: 'nightly',
            frequency: 'daily',
            specJson: '{}',
            wakeupPath: '/w',
            enabled: true,
            lastFired: null,
            lastSessionId: null,
            archivedAt: null,
            createdAt: 0,
          },
          state: 'scheduled',
          liveTerminalId: null,
        },
      ],
      archived: [],
      loadedFor: '/ws/h',
    } as never)
  }

  it('subscribes when supported; live event flips the row to live', async () => {
    const mod = await import('./heartbeat-sessions')
    const { useHeartbeatSessionsStore, subscribeHeartbeatLive, unsubscribeHeartbeatLive } = mod
    seedRow(useHeartbeatSessionsStore)

    subscribeHeartbeatLive('/ws/h')
    expect(ev.reg.tabSubs.length).toBe(1)

    ev.reg.tabSubs[0].handlers.onHeartbeatStateChanged({
      kind: 'heartbeat_state_changed',
      workspacePath: '',
      project: 'projH',
      agent: 'nightly',
      live: true,
    })
    expect(useHeartbeatSessionsStore.getState().active[0].state).toBe('live')

    ev.reg.tabSubs[0].handlers.onHeartbeatStateChanged({
      kind: 'heartbeat_state_changed',
      workspacePath: '',
      project: 'projH',
      agent: 'nightly',
      live: false,
    })
    // live=false re-derives the idle state (no lastSessionId → scheduled).
    expect(useHeartbeatSessionsStore.getState().active[0].state).toBe('scheduled')

    unsubscribeHeartbeatLive()
  })

  it('does NOT subscribe when unsupported (poll-derivation fallback)', async () => {
    supports.value = false
    const mod = await import('./heartbeat-sessions')
    const { subscribeHeartbeatLive } = mod

    subscribeHeartbeatLive('/ws/h')
    expect(ev.reg.tabSubs.length).toBe(0)
  })

  it('applyHeartbeatLive ignores a non-matching row', async () => {
    const mod = await import('./heartbeat-sessions')
    const { useHeartbeatSessionsStore } = mod
    seedRow(useHeartbeatSessionsStore)

    useHeartbeatSessionsStore.getState().applyHeartbeatLive('projH', 'other-hb', true)
    expect(useHeartbeatSessionsStore.getState().active[0].state).toBe('scheduled')
  })
})
