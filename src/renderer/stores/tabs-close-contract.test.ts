// A6 close-contract pins (2026-07-02 PTY-leak incident).
//
// The contract (documented in kessel-term/TerminalPane.tsx and on
// `closeTerminalForRenderer`): a DELIBERATE tab close issues
// `POST /cli/sessions/v2/close {agent_name: "tab-<pgId>", force: true}`
// for daemon-hosted (Kessel) terminals, while view-lifecycle paths
// (workspace switch, pinned system tabs, heartbeat minimize) never do.
// A daemon session that misses its A6 close outlives every client
// forever — no reaper covers bare tab sessions — so every row of the
// close-semantics table is pinned here at the store level.
//
// vitest env is `node` (no Tauri). Boundary mocks mirror
// api-sandbox-adoption.test.ts so importing the tabs module graph is
// inert; assertions read the recorded fetch / terminalKill /
// daemonCliPost traffic and FAIL LOUD on any drift.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── localStorage (real, in-memory) so the tabs module graph's persist paths run ─
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
const cliPosts = vi.hoisted(() => ({ calls: [] as Array<{ route: string; body: unknown }> }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => (route === 'workspace-layouts/load' ? null : [])),
  daemonCliPost: vi.fn(async (route: string, body: unknown) => {
    cliPosts.calls.push({ route, body })
    return {}
  }),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'resolved-agent'),
  setChatSession: vi.fn(async () => undefined),
}))
const killed = vi.hoisted(() => ({ ids: [] as string[] }))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalKill: vi.fn(async (id: string) => void killed.ids.push(id)),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
  invalidateDaemonWs: vi.fn(),
}))
vi.mock('./session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn(() => () => undefined),
}))

// Record every fetch — the A6 close is a raw fetch (not daemonCliPost),
// so this is where /cli/sessions/v2/close traffic lands.
const fetches = vi.hoisted(() => ({ calls: [] as Array<{ url: string; body: string }> }))
vi.stubGlobal(
  'fetch',
  vi.fn(async (url: string, init?: RequestInit) => {
    fetches.calls.push({ url: String(url), body: String(init?.body ?? '') })
    return { ok: true, text: async () => '', json: async () => ({}) } as Response
  }),
)

import { useTabsStore, type Tab, type TerminalItemData } from './tabs'

/** Every recorded /cli/sessions/v2/close call, parsed. */
function v2Closes(): Array<{ agent_name: string; force: boolean }> {
  return fetches.calls
    .filter((c) => c.url.includes('/cli/sessions/v2/close'))
    .map((c) => JSON.parse(c.body) as { agent_name: string; force: boolean })
}

/** Let the fire-and-forget closeV2Session promise chain settle. */
async function flushAsync(): Promise<void> {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0))
}

function terminalTab(
  tabId: string,
  pgId: string,
  data: Partial<TerminalItemData> = {},
  flags: Partial<Tab> = {},
): Tab {
  return {
    id: tabId,
    title: 'Terminal',
    mosaicTree: pgId,
    paneGroups: new Map([
      [
        pgId,
        {
          id: pgId,
          items: [
            {
              id: `item-${pgId}`,
              type: 'terminal' as const,
              data: { terminalId: pgId, cwd: '/tmp/proj', renderer: 'kessel', ...data } as TerminalItemData,
            },
          ],
          activeItemIndex: 0,
        },
      ],
    ]),
    ...flags,
  } as Tab
}

function reset(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
  })
  fetches.calls = []
  killed.ids = []
  cliPosts.calls = []
}

describe('A6 close contract — deliberate closes issue the daemon v2 close', () => {
  beforeEach(reset)

  it('removeTab on a kessel terminal tab closes its daemon session (force: true)', async () => {
    useTabsStore.setState({ tabs: [terminalTab('tab-A', 'pg-A')], activeTabId: 'tab-A' })

    useTabsStore.getState().removeTab('tab-A')
    await flushAsync()

    expect(useTabsStore.getState().tabs).toHaveLength(0)
    expect(v2Closes()).toEqual([{ agent_name: 'tab-pg-A', force: true }])
    // The legacy Tauri kill must NOT fire for a daemon-hosted tab.
    expect(killed.ids).toEqual([])
  })

  it('removeTabFromGroup closes daemon sessions in split columns too', async () => {
    useTabsStore.setState({
      extraGroups: [{ tabs: [terminalTab('tab-B', 'pg-B')], activeTabId: 'tab-B' }],
      splitCount: 2,
    })

    useTabsStore.getState().removeTabFromGroup(1, 'tab-B')
    await flushAsync()

    expect(useTabsStore.getState().extraGroups[0].tabs).toHaveLength(0)
    expect(v2Closes()).toEqual([{ agent_name: 'tab-pg-B', force: true }])
  })

  it('closeItemInPaneGroup closes the removed terminal item\'s daemon session', async () => {
    useTabsStore.setState({ tabs: [terminalTab('tab-C', 'pg-C')], activeTabId: 'tab-C' })

    useTabsStore.getState().closeItemInPaneGroup('tab-C', 'pg-C', 'item-pg-C')
    await flushAsync()

    expect(v2Closes()).toEqual([{ agent_name: 'tab-pg-C', force: true }])
  })

  it('an UNKNOWN renderer stamp still issues the v2 close (drift guard)', async () => {
    // A future renderer value must never silently skip the A6 close —
    // that exact fall-through is a forever-leak (no reaper coverage).
    useTabsStore.setState({
      tabs: [terminalTab('tab-D', 'pg-D', { renderer: 'kessel-webgl-9000' as TerminalItemData['renderer'] })],
      activeTabId: 'tab-D',
    })

    useTabsStore.getState().removeTab('tab-D')
    await flushAsync()

    expect(v2Closes()).toEqual([{ agent_name: 'tab-pg-D', force: true }])
    expect(killed.ids).toEqual([])
  })

  it("legacy 'alacritty' tabs (and unstamped items) route to terminal/kill, not v2 close", async () => {
    useTabsStore.setState({
      tabs: [
        terminalTab('tab-E', 'pg-E', { renderer: 'alacritty' }),
        terminalTab('tab-F', 'pg-F', { renderer: undefined }),
      ],
      activeTabId: 'tab-E',
    })

    useTabsStore.getState().removeTab('tab-E')
    useTabsStore.getState().removeTab('tab-F')
    await flushAsync()

    expect(killed.ids).toEqual(['pg-E', 'pg-F'])
    expect(v2Closes()).toEqual([])
  })
})

describe('A6 close contract — view-lifecycle paths never close sessions', () => {
  beforeEach(reset)

  it('clearAllTabs (workspace switch view-clear) closes NOTHING', async () => {
    useTabsStore.setState({
      tabs: [terminalTab('tab-G', 'pg-G'), terminalTab('tab-H', 'pg-H')],
      activeTabId: 'tab-G',
    })

    useTabsStore.getState().clearAllTabs()
    await flushAsync()

    expect(useTabsStore.getState().tabs).toHaveLength(0)
    expect(v2Closes()).toEqual([])
    expect(killed.ids).toEqual([])
  })

  it('removeTab on a pinned system agent tab is a no-op (tab retained, no close)', async () => {
    const pinned = terminalTab('tab-I', 'pg-I', {}, { isSystemAgent: true })
    useTabsStore.setState({ tabs: [pinned], activeTabId: 'tab-I' })

    useTabsStore.getState().removeTab('tab-I')
    await flushAsync()

    // The pinned canonical tab survives AND its session is untouched.
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    expect(v2Closes()).toEqual([])
    expect(killed.ids).toEqual([])
  })

  it('heartbeat-surfaced tabs close-as-minimize: surfaced=false, PTY survives', async () => {
    useTabsStore.setState({
      tabs: [
        terminalTab('tab-J', 'pg-J', {
          heartbeatName: 'nightly-sync',
          projectPath: '/tmp/proj',
          surfacedAgentName: 'manager',
        }),
      ],
      activeTabId: 'tab-J',
    })

    useTabsStore.getState().removeTab('tab-J')
    await flushAsync()

    expect(useTabsStore.getState().tabs).toHaveLength(0)
    // Minimize, don't kill: the surfaced flag flips off…
    const surfacedPosts = cliPosts.calls.filter((c) => c.route === 'session/set-surfaced')
    expect(surfacedPosts).toHaveLength(1)
    expect(surfacedPosts[0].body).toMatchObject({
      project_path: '/tmp/proj',
      agent_name: 'manager',
      surfaced: false,
    })
    // …and NO close reaches the daemon session.
    expect(v2Closes()).toEqual([])
    expect(killed.ids).toEqual([])
  })
})
