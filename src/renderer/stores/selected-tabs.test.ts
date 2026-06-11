// per-client-view-state.md — the user's SELECTED tab is PER-CLIENT view state.
//
// Pre-0.39.43, `activeTabId` was serialized INTO the shared workspace layout,
// POSTed to the daemon, and adopted by peers — so a teammate's reorder/save
// hijacked your selected tab. These tests lock the corrected behavior:
//
//   Phase 1 — `serializeCurrentLayout` no longer emits `activeTabId`.
//   Phase 2 — selection is sourced from / written to a per-client local store
//             (`selected-tabs.ts`), keyed by `projectId:workspaceId` and
//             persisted to localStorage; cold-load restores THIS client's
//             selection, not the serialized layout's.
//   Phase 3 — adoption (in-place reorder AND restoreLayout rebuild) NEVER moves
//             this client's selection.
//
// vitest env is `node` (no Tauri). We mock the daemon/Tauri boundaries the
// tabs module graph touches so importing it is inert, and provide a real
// in-memory localStorage so the per-client store's persistence path is
// exercised end-to-end.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── localStorage (real, in-memory) so the persist path runs ──────────────
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────
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

// Controllable daemon-cli: `getImpl` stubs GET (e.g. workspace-layouts/load);
// posts recorded so we can assert serialized payloads carry no selection.
const cli = vi.hoisted(() => ({
  posts: [] as Array<{ route: string; body: unknown }>,
  // Default: list-shaped routes (load-all / themes/list) get [] so module-load
  // side effects stay inert; scalar routes (workspace-layouts/load) get null.
  getImpl: (async (route: string) =>
    route === 'workspace-layouts/load' ? null : []) as (route: string, params?: unknown) => Promise<unknown>,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string, params?: unknown) => cli.getImpl(route, params)),
  daemonCliPost: vi.fn(async (route: string, body?: unknown) => {
    cli.posts.push({ route, body })
    return {}
  }),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
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
vi.mock('./session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
}))

import { useTabsStore, __tryReorderTabsInPlaceForTests, type SerializedLayout, type Tab } from './tabs'
import { useSelectedTabsStore, getSelectedTab } from './selected-tabs'

const PROJECT = 'projA'
const WORKSPACE = 'wsA'
const KEY = `${PROJECT}:${WORKSPACE}`
const CWD = '/tmp/workspaceA'

/** A two-tab serialized layout. paneGroupIds (pg-a/pg-b) are the STABLE
 *  identity preserved across `restoreLayout`'s tab-id re-mint, so they are the
 *  selection signature the per-client store keys on. NOTE: deliberately carries
 *  a LEAKED legacy `activeTabId` to prove restore IGNORES it. */
function twoTabLayout(): SerializedLayout {
  return {
    version: 2,
    tabs: [
      { id: 'ser-a', title: 'A', mosaicTree: 'pg-a', paneGroups: { 'pg-a': { id: 'pg-a', items: [], activeItemIndex: 0 } } },
      { id: 'ser-b', title: 'B', mosaicTree: 'pg-b', paneGroups: { 'pg-b': { id: 'pg-b', items: [], activeItemIndex: 0 } } },
    ],
    // Legacy leak: a peer's serialized selection. Must be ignored on restore.
    activeTabId: 'ser-b',
  }
}

/** Find a restored live tab by its single paneGroupId. */
function tabByPg(pgId: string): Tab | undefined {
  return useTabsStore.getState().tabs.find((t) => Array.from(t.paneGroups.keys())[0] === pgId)
}

function resetStores(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    activeWorkspaceKey: null,
    activeProjectId: null,
    activeWorkspaceId: null,
    workspaceLayouts: {},
    backgroundWorkspaces: {},
  })
  useSelectedTabsStore.setState({ selected: {} })
  mem.clear()
  cli.posts = []
  cli.getImpl = async (route: string) => (route === 'workspace-layouts/load' ? null : [])
}

beforeEach(resetStores)

// ── selected-tabs store (unit) ───────────────────────────────────────────

describe('selected-tabs store', () => {
  it('setSelected → getSelected round-trips and persists to localStorage', () => {
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'sig-x')
    expect(useSelectedTabsStore.getState().getSelected(PROJECT, WORKSPACE)).toBe('sig-x')
    // localStorage holds the persisted map (a fresh load would re-hydrate it).
    const raw = mem.get('k2so:selected-tabs')
    expect(raw).toBeTruthy()
    expect(JSON.parse(raw!)).toEqual({ [KEY]: 'sig-x' })
  })

  it('getSelected returns null for an unknown workspace', () => {
    expect(useSelectedTabsStore.getState().getSelected('nope', 'nope')).toBeNull()
  })

  it('reset clears all selections and the persisted map', () => {
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'sig-x')
    useSelectedTabsStore.getState().reset()
    expect(useSelectedTabsStore.getState().getSelected(PROJECT, WORKSPACE)).toBeNull()
    expect(JSON.parse(mem.get('k2so:selected-tabs')!)).toEqual({})
  })
})

// ── Phase 1: serialize strips selection ──────────────────────────────────

describe('Phase 1 — serializeCurrentLayout omits activeTabId', () => {
  it('top-level layout carries no activeTabId', () => {
    useTabsStore.setState({
      tabs: [
        { id: 'live-a', title: 'A', mosaicTree: 'pg-a', paneGroups: new Map([['pg-a', { id: 'pg-a', items: [], activeItemIndex: 0 }]]) },
      ],
      activeTabId: 'live-a',
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
    })
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect('activeTabId' in layout).toBe(false)
  })

  it('split-group layout carries no per-group activeTabId either', () => {
    useTabsStore.setState({
      tabs: [
        { id: 'live-a', title: 'A', mosaicTree: 'pg-a', paneGroups: new Map([['pg-a', { id: 'pg-a', items: [], activeItemIndex: 0 }]]) },
      ],
      activeTabId: 'live-a',
      extraGroups: [
        { tabs: [{ id: 'live-c', title: 'C', mosaicTree: 'pg-c', paneGroups: new Map([['pg-c', { id: 'pg-c', items: [], activeItemIndex: 0 }]]) }], activeTabId: 'live-c' },
      ],
      splitCount: 2,
      activeGroupIndex: 0,
    })
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect('activeTabId' in layout).toBe(false)
    expect(layout.extraGroups).toBeDefined()
    expect('activeTabId' in layout.extraGroups![0]).toBe(false)
  })

  it('a save POST body carries no activeTabId (would-be leak to peers)', () => {
    useTabsStore.setState({
      tabs: [
        { id: 'live-a', title: 'A', mosaicTree: 'pg-a', paneGroups: new Map([['pg-a', { id: 'pg-a', items: [], activeItemIndex: 0 }]]) },
      ],
      activeTabId: 'live-a',
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeWorkspaceKey: KEY,
    })
    cli.posts = []
    useTabsStore.getState().saveLayoutForWorkspace(PROJECT, WORKSPACE)
    const save = cli.posts.find((p) => p.route === 'workspace-layouts/save')
    expect(save).toBeDefined()
    const body = save!.body as { layoutJson: string }
    expect(body.layoutJson.includes('activeTabId')).toBe(false)
  })
})

// ── Phase 2: restoreLayout sources selection from the per-client store ────

describe('Phase 2 — restoreLayout uses the per-client selection, not the layout', () => {
  it('IGNORES the leaked serialized activeTabId; restores THIS client selection (B)', () => {
    // This client previously selected tab B (signature = its paneGroupId).
    useTabsStore.setState({ activeProjectId: PROJECT, activeWorkspaceId: WORKSPACE })
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'pg-b')

    useTabsStore.getState().restoreLayout(twoTabLayout(), CWD)

    // The leaked serialized `activeTabId` was 'ser-b' (tab B too here) — but
    // the point is selection comes from the LOCAL store: prove it by pointing
    // the local store at A while the layout leaks B (next test).
    const liveB = tabByPg('pg-b')
    expect(liveB).toBeDefined()
    expect(useTabsStore.getState().activeTabId).toBe(liveB!.id)
  })

  it('local selection (A) WINS even when the serialized layout leaks B', () => {
    useTabsStore.setState({ activeProjectId: PROJECT, activeWorkspaceId: WORKSPACE })
    // Local store selected A; serialized layout's leaked activeTabId is B.
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'pg-a')

    useTabsStore.getState().restoreLayout(twoTabLayout(), CWD)

    const liveA = tabByPg('pg-a')
    expect(useTabsStore.getState().activeTabId).toBe(liveA!.id)
  })

  it('no saved selection → first tab (preserves #658 pinned-chat-as-default)', () => {
    useTabsStore.setState({ activeProjectId: PROJECT, activeWorkspaceId: WORKSPACE })
    // No setSelected call — brand-new client.
    useTabsStore.getState().restoreLayout(twoTabLayout(), CWD)
    // First tab is A (pg-a) — the natural default (the system Chat tab on a
    // real cold boot).
    const first = useTabsStore.getState().tabs[0]
    expect(useTabsStore.getState().activeTabId).toBe(first.id)
    expect(Array.from(first.paneGroups.keys())[0]).toBe('pg-a')
  })

  it('saved selection no longer exists → falls back to first tab', () => {
    useTabsStore.setState({ activeProjectId: PROJECT, activeWorkspaceId: WORKSPACE })
    // Selected a paneGroup that isn't in the restored layout.
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'pg-gone')
    useTabsStore.getState().restoreLayout(twoTabLayout(), CWD)
    expect(useTabsStore.getState().activeTabId).toBe(useTabsStore.getState().tabs[0].id)
  })
})

// ── Phase 2: cold-load + single-client persistence ───────────────────────

describe('Phase 2 — cold-load restores the local selection (not the layout)', () => {
  it('loadLayoutForWorkspace (DB branch) restores the per-client selection (B), not the leaked layout', async () => {
    // The per-client store says B. The on-disk layout's leaked activeTabId is
    // also 'ser-b' here, so to make the assertion meaningful we point the
    // store at A and prove A wins on cold load.
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'pg-a')
    cli.getImpl = async (route: string) =>
      route === 'workspace-layouts/load' ? JSON.stringify(twoTabLayout()) : null

    await useTabsStore.getState().loadLayoutForWorkspace(PROJECT, WORKSPACE, CWD)

    const liveA = tabByPg('pg-a')
    expect(liveA).toBeDefined()
    expect(useTabsStore.getState().activeTabId).toBe(liveA!.id)
  })

  it('single-client: select a tab → it persists → a fresh restore restores it', () => {
    useTabsStore.setState({ activeProjectId: PROJECT, activeWorkspaceId: WORKSPACE })
    // First restore with no saved selection → first tab (A) active.
    useTabsStore.getState().restoreLayout(twoTabLayout(), CWD)
    expect(Array.from(tabByPg('pg-b')!.paneGroups.keys())[0]).toBe('pg-b')

    // User selects tab B (group 0). Persists by signature.
    const liveB = tabByPg('pg-b')!
    useTabsStore.getState().setActiveTabInGroup(0, liveB.id)
    expect(getSelectedTab(PROJECT, WORKSPACE)).toBe('pg-b')
    // ...and it hit localStorage so a brand-new process would see it.
    expect(JSON.parse(mem.get('k2so:selected-tabs')!)).toEqual({ [KEY]: 'pg-b' })

    // Simulate a fresh load (tabs cleared, ids re-minted) → restores B.
    useTabsStore.setState({ tabs: [], activeTabId: null })
    useTabsStore.getState().restoreLayout(twoTabLayout(), CWD)
    expect(useTabsStore.getState().activeTabId).toBe(tabByPg('pg-b')!.id)
  })
})

// ── Phase 3: adoption never moves selection (multi-client lock) ───────────

describe('Phase 3 — adoption never moves this client selection', () => {
  // Client B holds selection A locally. A peer (client A) reorders/saves and
  // B adopts the structure — B's selection must STAY on A, never jump to the
  // peer's selection.
  it('restoreLayout rebuild path: peer layout (leaks B) does NOT move local selection (A)', () => {
    useTabsStore.setState({ activeProjectId: PROJECT, activeWorkspaceId: WORKSPACE })
    // B's local selection is A.
    useSelectedTabsStore.getState().setSelected(PROJECT, WORKSPACE, 'pg-a')

    // Peer's adopted layout has its own (leaked) selection = B + reversed order.
    const peerLayout: SerializedLayout = {
      version: 2,
      tabs: [
        { id: 'ser-b', title: 'B', mosaicTree: 'pg-b', paneGroups: { 'pg-b': { id: 'pg-b', items: [], activeItemIndex: 0 } } },
        { id: 'ser-a', title: 'A', mosaicTree: 'pg-a', paneGroups: { 'pg-a': { id: 'pg-a', items: [], activeItemIndex: 0 } } },
      ],
      activeTabId: 'ser-b',
    }
    useTabsStore.getState().restoreLayout(peerLayout, CWD)

    // B stays on A (its own selection), NOT the peer's B.
    expect(useTabsStore.getState().activeTabId).toBe(tabByPg('pg-a')!.id)
  })

  it('in-place reorder path: peer reorder (leaks B) keeps local selection (A)', () => {
    // Seed two LIVE tabs whose paneGroupIds are pg-a/pg-b, with A selected.
    const tabA: Tab = { id: 'live-a', title: 'A', mosaicTree: 'pg-a', paneGroups: new Map([['pg-a', { id: 'pg-a', items: [], activeItemIndex: 0 }]]) }
    const tabB: Tab = { id: 'live-b', title: 'B', mosaicTree: 'pg-b', paneGroups: new Map([['pg-b', { id: 'pg-b', items: [], activeItemIndex: 0 }]]) }
    useTabsStore.setState({
      tabs: [tabA, tabB],
      activeTabId: 'live-a', // B (this client) is looking at A
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeWorkspaceKey: KEY,
      activeProjectId: PROJECT,
      activeWorkspaceId: WORKSPACE,
      workspaceLayouts: {},
    })

    // Peer reordered (swap) AND their serialized selection is B.
    const remoteLayout: SerializedLayout = {
      version: 2,
      tabs: [
        { id: 'ser-b', title: 'B', mosaicTree: 'pg-b', paneGroups: { 'pg-b': { id: 'pg-b', items: [], activeItemIndex: 0 } } },
        { id: 'ser-a', title: 'A', mosaicTree: 'pg-a', paneGroups: { 'pg-a': { id: 'pg-a', items: [], activeItemIndex: 0 } } },
      ],
      activeTabId: 'ser-b',
    }
    const ok = __tryReorderTabsInPlaceForTests(KEY, remoteLayout)
    expect(ok).toBe(true)

    // Order swapped (visual)...
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(['live-b', 'live-a'])
    // ...but selection STAYS on A (live-a) — the peer's B did NOT hijack it.
    expect(useTabsStore.getState().activeTabId).toBe('live-a')
  })

  it('two clients hold DIFFERENT selections simultaneously (no shared selection)', () => {
    // Client 1: workspace selection A.
    useSelectedTabsStore.getState().setSelected('p1', 'w1', 'pg-a')
    // Client 2 is a different store instance in reality; model its independence
    // by a different workspace key — selections never collide via the shared
    // layout because the layout no longer carries selection at all.
    useSelectedTabsStore.getState().setSelected('p2', 'w2', 'pg-b')
    expect(getSelectedTab('p1', 'w1')).toBe('pg-a')
    expect(getSelectedTab('p2', 'w2')).toBe('pg-b')
  })
})
