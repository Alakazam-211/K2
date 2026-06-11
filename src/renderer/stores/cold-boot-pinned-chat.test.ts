// #658 — cold-boot pinned-Chat-tab spawn race.
//
// Bug: on install / relaunch, the workspace the user LANDS ON has its
// pinned Chat tab created WITHOUT being the active tab and WITHOUT its
// saved sessionId, so it never becomes visible → its grid-WS never opens
// → the Claude session never spawns until a manual refresh. Other
// workspaces (reached via a later switch) work because by then the saved
// layout has restored and set the active tab.
//
// Root cause: a cold-boot ordering race. The pinned-tab ensure step
// (`ensureSystemAgentTabs`, deferred via setTimeout inside
// `ensurePinnedAgentTabForMode`) ran BEFORE the async layout restore
// (`loadLayoutForWorkspace`'s DB-load branch) had populated tabs +
// activeTabId + sessionId. The ensure found no restored pinned tab, so
// it created a fresh sessionless one and never set activeTabId.
//
// This suite drives the REAL tabs store against the cold-boot shape:
//   1. ensureSystemAgentTabs ADOPTS the Chat tab as the active tab when
//      nothing else is active (the cold-boot create case).
//   2. ensureSystemAgentTabs SEEDS the freshly-created Chat tab with the
//      saved chat sessionId from this workspace's in-memory layout (fast
//      `--resume` path, not the slow resumeChatArgs round-trip).
//   3. loadLayoutForWorkspace's DB-load branch (cold boot, empty
//      in-memory cache) RESOLVES only after restoreLayout has set the
//      active tab + sessionId, so a caller that awaits it before running
//      the ensure gets the restored (active, sessionId-carrying) Chat tab
//      and the ensure is a pure idempotent reconcile — no duplicate, no
//      lost active tab.
//
// vitest env is `node` (no Tauri). Mock the daemon/Tauri boundaries the
// tabs module touches so importing it is inert; the layout-load route is
// a controllable deferred so we can model "restore resolves AFTER ensure
// was invoked".

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────

vi.mock('@tauri-apps/api/core', () => ({
  // k2so_sessions_list_for_workspace (reconcile) returns null → reconcile
  // is inert (just subscribes). All other invokes resolve null.
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

// Hoisted so the vi.mock factory (hoisted above top-level consts) can
// close over the spy + the per-test deferred state without a TDZ.
const h = vi.hoisted(() => {
  const state: { loadLayoutJson: string | null; resolveLayoutLoad: (() => void) | null } = {
    loadLayoutJson: null,
    resolveLayoutLoad: null,
  }
  // Controllable layout-load route. `workspace-layouts/load` returns a
  // promise we resolve by hand so we can interleave the ensure call BEFORE
  // the restore lands (the cold-boot race). Everything else resolves inert.
  const daemonCliGet = vi.fn(async (route: string) => {
    if (route === 'workspace-layouts/load') {
      await new Promise<void>((r) => { state.resolveLayoutLoad = r })
      return state.loadLayoutJson
    }
    if (route === 'workspace-layouts/load-all') return []
    return []
  })
  return { state, daemonCliGet }
})
const daemonCliGet = h.daemonCliGet
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
// settings store fetches at module import → real fetch in node env. Stub
// the daemon-settings boundary so importing the store graph stays inert.
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'resolved-agent'),
  setChatSession: vi.fn(async () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
}))
// The reconcile pass (fire-and-forget) opens a workspace session-events
// WS subscription. Stub it to an inert unsubscribe so the background path
// doesn't reach into the real WS plumbing during the test.
vi.mock('./session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  // 0.39.39 (#676/#677) — the active-workspace subscription also opens a
  // tab-title/tab-order WS; stub it inert too.
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
}))

// Now import the REAL tabs store under test.
import {
  useTabsStore,
  type AgentItemData,
  type SerializedLayout,
} from './tabs'

const KEY = 'projA:wsA'
const CWD = '/tmp/workspaceA'
const SAVED_SESSION_ID = 'claude-session-uuid-658'

/** A saved layout whose pinned Chat tab carries a persisted Claude
 *  session id — exactly what cold-boot restore should reinstate. */
function savedLayoutWithChatSession(): SerializedLayout {
  return {
    version: 2,
    tabs: [
      {
        id: 'saved-chat',
        title: 'Chat',
        mosaicTree: 'pg-chat',
        isSystemAgent: true,
        paneGroups: {
          'pg-chat': {
            id: 'pg-chat',
            activeItemIndex: 0,
            items: [
              {
                id: 'item-chat',
                type: 'agent',
                agentName: 'resolved-agent',
                projectPath: CWD,
                section: 'chat',
                sessionId: SAVED_SESSION_ID,
              },
            ],
          },
        },
      },
      {
        id: 'saved-inbox',
        title: 'Inbox',
        mosaicTree: 'pg-inbox',
        isSystemAgent: true,
        paneGroups: {
          'pg-inbox': {
            id: 'pg-inbox',
            activeItemIndex: 0,
            items: [
              {
                id: 'item-inbox',
                type: 'agent',
                agentName: 'resolved-agent',
                projectPath: CWD,
                section: 'inbox',
              },
            ],
          },
        },
      },
    ],
    activeTabId: 'saved-chat',
  }
}

function reset(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    activeWorkspaceKey: null,
    workspaceLayouts: {},
    backgroundWorkspaces: {},
  })
}

function chatTab(): { tab: import('./tabs').Tab; data: AgentItemData } | null {
  const tab = useTabsStore.getState().tabs.find((t) => t.isSystemAgent && t.title === 'Chat')
  if (!tab) return null
  const item = Array.from(tab.paneGroups.values())[0]?.items[0]
  if (item?.type !== 'agent') return null
  return { tab, data: item.data as AgentItemData }
}

describe('#658 ensureSystemAgentTabs — cold-boot active-tab adoption + sessionId hint', () => {
  beforeEach(() => {
    reset()
    h.state.resolveLayoutLoad = null
    h.state.loadLayoutJson = null
    daemonCliGet.mockClear()
  })

  it('adopts the freshly-created Chat tab as the active tab when nothing is active (cold-boot create)', () => {
    // Cold boot: no tabs, no activeTabId, no restored layout yet — the
    // racing ensure runs against an empty view.
    useTabsStore.setState({ activeWorkspaceKey: KEY })

    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')

    const ct = chatTab()
    expect(ct).not.toBeNull()
    // The pinned Chat tab MUST become the active tab so isTabVisible is
    // true and its grid-WS opens (the session spawns without a refresh).
    expect(useTabsStore.getState().activeTabId).toBe(ct!.tab.id)
  })

  it('seeds the freshly-created Chat tab with the saved chat sessionId (fast --resume path)', () => {
    // The in-memory layout cache holds this workspace's saved layout
    // (loaded by loadWorkspaceSessionsFromDb) even though restoreLayout
    // hasn't replayed it into `tabs` yet — the cold-boot race window.
    useTabsStore.setState({
      activeWorkspaceKey: KEY,
      workspaceLayouts: { [KEY]: savedLayoutWithChatSession() },
    })

    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')

    const ct = chatTab()
    expect(ct).not.toBeNull()
    expect(ct!.data.sessionId).toBe(SAVED_SESSION_ID)
    // And it's active — so it actually spawns.
    expect(useTabsStore.getState().activeTabId).toBe(ct!.tab.id)
  })

  it('drops the saved sessionId when it belonged to a different workspace path', () => {
    const layout = savedLayoutWithChatSession()
    // Saved chat agent item points at a DIFFERENT workspace path — its
    // Claude session is not ours to resume.
    ;(layout.tabs[0].paneGroups['pg-chat'].items[0] as { projectPath?: string }).projectPath = '/tmp/otherWorkspace'
    useTabsStore.setState({
      activeWorkspaceKey: KEY,
      workspaceLayouts: { [KEY]: layout },
    })

    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')

    const ct = chatTab()
    expect(ct).not.toBeNull()
    expect(ct!.data.sessionId).toBeUndefined()
  })

  it('does NOT steal the active tab when a live active tab already exists (normal switch path)', () => {
    // Seed a non-system user tab that is already active — the restore /
    // user already chose this. ensure must leave it alone.
    useTabsStore.setState({
      activeWorkspaceKey: KEY,
      tabs: [
        {
          id: 'user-tab',
          title: 'README.md',
          mosaicTree: 'pg-x',
          paneGroups: new Map([['pg-x', { id: 'pg-x', items: [], activeItemIndex: 0 }]]),
        },
      ],
      activeTabId: 'user-tab',
    })

    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')

    // Active tab unchanged — we only adopt when nothing is active.
    expect(useTabsStore.getState().activeTabId).toBe('user-tab')
  })

  it('does not create a duplicate pinned Chat tab when one already exists', () => {
    useTabsStore.setState({ activeWorkspaceKey: KEY })
    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')
    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')

    const chatTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent && t.title === 'Chat')
    expect(chatTabs).toHaveLength(1)
  })
})

describe('#658 loadLayoutForWorkspace — awaited cold-boot ordering beats the ensure race', () => {
  beforeEach(() => {
    reset()
    h.state.resolveLayoutLoad = null
    h.state.loadLayoutJson = null
    daemonCliGet.mockClear()
  })

  it('resolves only AFTER restoreLayout has set the active tab + sessionId (DB-load branch)', async () => {
    // Cold boot: in-memory cache empty → loadLayoutForWorkspace takes the
    // async DB-load branch. The DB returns the saved layout (deferred).
    h.state.loadLayoutJson = JSON.stringify(savedLayoutWithChatSession())

    const loadPromise = useTabsStore.getState().loadLayoutForWorkspace('projA', 'wsA', CWD)
    // Let the load reach its `await daemonCliGet('workspace-layouts/load')`
    // so the deferred resolver is installed (the load is now PARKED — the
    // restore has NOT run yet, the cold-boot race window).
    await Promise.resolve()
    await Promise.resolve()
    expect(h.state.resolveLayoutLoad).not.toBeNull()

    // Model the race: BEFORE the layout load resolves, the pinned-tab
    // ensure fires (this is exactly what raced ahead in the bug). With
    // the empty in-memory cache primed below, it would create a fresh
    // sessionless Chat tab if it won.
    useTabsStore.setState({ workspaceLayouts: { [KEY]: savedLayoutWithChatSession() } })

    // Now let the DB layout load resolve, then await the documented
    // contract: the promise settles only after restoreLayout ran.
    h.state.resolveLayoutLoad!()
    await loadPromise

    // restoreLayout won: the restored Chat tab is the active tab and
    // carries the saved sessionId — so it spawns immediately, no refresh.
    const ct = chatTab()
    expect(ct).not.toBeNull()
    expect(useTabsStore.getState().activeTabId).toBe(ct!.tab.id)
    expect(ct!.data.sessionId).toBe(SAVED_SESSION_ID)

    // A post-restore ensure (the awaited-ordering fix runs it after the
    // load) is a pure idempotent reconcile: no duplicate Chat tab, the
    // active tab and sessionId survive.
    const activeBefore = useTabsStore.getState().activeTabId
    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')
    const chatTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent && t.title === 'Chat')
    expect(chatTabs).toHaveLength(1)
    expect(useTabsStore.getState().activeTabId).toBe(activeBefore)
    expect(chatTab()!.data.sessionId).toBe(SAVED_SESSION_ID)
  })
})

describe('#681 (Bug A) brand-new workspace — restoreWorkspace awaits load, ensure shows Chat+Inbox', () => {
  beforeEach(() => {
    reset()
    h.state.resolveLayoutLoad = null
    h.state.loadLayoutJson = null
    daemonCliGet.mockClear()
  })

  it('restoreWorkspace (no live tabs, no saved layout) RESOLVES, then ensure surfaces pinned Chat+Inbox with Chat active', async () => {
    // Brand-new / never-chatted workspace: no background tabs, no saved
    // layout JSON (loadLayoutJson stays null → loadLayoutForWorkspace
    // takes the launchDefaultAgent branch). Before the fix restoreWorkspace
    // was fire-and-forget and the projects store ran ensure synchronously,
    // racing the post-await default-agent + layout work — so the pinned
    // tabs didn't appear until a switch-away.
    const restorePromise = useTabsStore.getState().restoreWorkspace(KEY, CWD)

    // The slow path is parked at `await workspace-layouts/load`. The key is
    // already set synchronously so the active-workspace guards pass.
    await Promise.resolve()
    await Promise.resolve()
    expect(useTabsStore.getState().activeWorkspaceKey).toBe(KEY)
    expect(h.state.resolveLayoutLoad).not.toBeNull()

    // Resolve the (null) layout load → launchDefaultAgent is invoked and
    // restoreWorkspace's promise settles. The mirror of the projects
    // store's `await restoreWorkspace(...).then(ensure)` ordering.
    h.state.resolveLayoutLoad!()
    await restorePromise

    // Now ensure runs (as the projects store does after the await). The
    // active view is still empty (default-agent spawn is a deferred
    // setTimeout that bails once pinned tabs exist), so ensure adopts the
    // Chat tab as active and creates both pinned surfaces.
    useTabsStore.getState().ensureSystemAgentTabs('resolved-agent', CWD, 'Agent')

    const tabs = useTabsStore.getState().tabs
    const chat = tabs.find((t) => t.isSystemAgent && t.title === 'Chat')
    const inbox = tabs.find((t) => t.isSystemAgent && t.title === 'Inbox')
    expect(chat).toBeDefined()
    expect(inbox).toBeDefined()
    // Chat must be the active tab so it renders + spawns on first open.
    expect(useTabsStore.getState().activeTabId).toBe(chat!.id)
  })
})
