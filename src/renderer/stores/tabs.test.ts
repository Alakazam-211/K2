import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import * as pageTitle from '@/web/page-title'
import { useTabsStore, ensurePinnedAgentTabForMode, registerActiveProjectIdGetter, LAYOUT_SCHEMA_VERSION, type AgentItemData, type TerminalItemData, type BrowserItemData, type SerializedLayout } from './tabs'
import {
  __resetNamedChatTitleCachesForTests,
  rememberChatCustomName,
  rememberTabTitleSnapshot,
} from '@/lib/chat-session-tab'

import { primaryScope } from '@/kessel/server-scope'
// ensurePinnedAgentTabForMode resolves the agent name via Tauri
// `invoke`. Stub it so the async resolution completes deterministically
// in the jsdom/node test env (no Tauri bridge).
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'k2so_agents_list') return []
    if (cmd === 'k2so_workspace_agent_display_name') return 'resolved-agent'
    return null
  }),
}))

/** Flush the setTimeout(…,0) + awaited invoke chain inside
 *  ensurePinnedAgentTabForMode.
 *
 *  16 ticks, not the historical 6: the resolution races other store
 *  machinery's microtasks (e.g. 0.40.48's synchronous layout flush issues
 *  a fetch attempt whose rejection chain shares these ticks), and under
 *  full-suite CPU load 6 was observed to flake ~1/15 runs. The guard
 *  tests' NEGATIVE assertions are unaffected — waiting longer only gives
 *  a dropped resolution more chances to (correctly) not stamp. */
async function flushPinnedTabResolution(): Promise<void> {
  for (let i = 0; i < 16; i++) await new Promise((r) => setTimeout(r, 0))
}

/**
 * Tests for the post-0.36.0 pinned-agent-tab split behaviour.
 *
 * Pre-split: a single isSystemAgent tab held the agent's UI as 4
 * sub-tabs (Work / Chat / CLAUDE.md / Profile).
 *
 * Post-split: up to TWO isSystemAgent tabs per workspace —
 *   - section 'inbox' (always)
 *   - section 'chat'  (skipped for the workspace board)
 */

function reset(): void {
  __resetNamedChatTitleCachesForTests()
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
  })
}

function getAgentItem(tabIndex: number): AgentItemData | null {
  const tab = useTabsStore.getState().tabs[tabIndex]
  if (!tab) return null
  const item = Array.from(tab.paneGroups.values())[0]?.items[0]
  if (item?.type !== 'agent') return null
  return item.data as AgentItemData
}

describe('ensureSystemAgentTabs', () => {
  beforeEach(reset)

  it('creates two pinned tabs (Inbox + Chat) for a regular agent', () => {
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')

    const tabs = useTabsStore.getState().tabs
    const systemTabs = tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)

    // Canonical order: Chat first, Inbox second.
    expect(systemTabs[0].title).toBe('Chat')
    expect(systemTabs[1].title).toBe('Inbox')

    const chatItem = getAgentItem(0)
    const inboxItem = getAgentItem(1)
    expect(chatItem?.section).toBe('chat')
    expect(inboxItem?.section).toBe('inbox')
    expect(chatItem?.agentName).toBe('manager')
    expect(inboxItem?.agentName).toBe('manager')
  })

  it('forces canonical order even when only one section pre-existed', () => {
    // Simulate a half-migrated layout: only the Inbox tab is in place.
    useTabsStore.setState({
      tabs: [{
        id: 'existing-inbox',
        title: 'Inbox',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'agent',
            data: { agentName: 'manager', projectPath: '/tmp/proj', section: 'inbox' },
          }],
          activeItemIndex: 0,
        }]]),
        isSystemAgent: true,
      }],
      activeTabId: 'existing-inbox',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })

    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')

    // After the call, Chat must come first even though Inbox was the
    // only pre-existing tab. Pre-existing tab keeps its id (state
    // preservation); only ordering changes.
    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)
    expect(systemTabs[0].title).toBe('Chat')
    expect(systemTabs[1].title).toBe('Inbox')
    expect(systemTabs[1].id).toBe('existing-inbox')
  })

  it('creates only the Inbox tab for the workspace board (no chat surface)', () => {
    useTabsStore.getState().ensureSystemAgentTabs('__workspace__', '/tmp/proj', 'Work Board')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(1)
    expect(systemTabs[0].title).toBe('Work Board')

    const item = getAgentItem(0)
    expect(item?.section).toBe('inbox')
    expect(item?.agentName).toBe('__workspace__')
  })

  it('is idempotent — calling twice does not create duplicates', () => {
    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')
    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)
  })

  it('back-fills a missing section if only one pinned tab exists', () => {
    // Simulate a half-migrated state: only the inbox tab is in place.
    useTabsStore.setState({
      tabs: [{
        id: 'existing-inbox',
        title: 'Inbox',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'agent',
            data: { agentName: 'manager', projectPath: '/tmp/proj', section: 'inbox' },
          }],
          activeItemIndex: 0,
        }]]),
        isSystemAgent: true,
      }],
      activeTabId: 'existing-inbox',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })

    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(systemTabs).toHaveLength(2)
    const sections = systemTabs
      .map((t) => {
        const it = Array.from(t.paneGroups.values())[0]?.items[0]
        return it?.type === 'agent' ? (it.data as AgentItemData).section : null
      })
      .filter((s) => s !== null)
    expect(sections).toContain('inbox')
    expect(sections).toContain('chat')
  })

  it('heals a restored pinned tab carrying a stale workspace path', () => {
    // Repro: HK47's saved layout has a pinned Chat tab whose agent item
    // still points at the K2 workspace (projectPath baked in under the
    // wrong workspace and replayed verbatim by restoreLayout). Switching
    // into HK47 calls ensureSystemAgentTabs with HK47's authoritative
    // path — the existing tab must be reconciled, not reused as-is.
    useTabsStore.setState({
      tabs: [{
        id: 'stale-chat',
        title: 'Chat',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'agent',
            data: {
              agentName: 'k2so-manager',
              projectPath: '/workspaces/K2',
              section: 'chat',
              sessionId: 'old-k2so-session',
            },
          }],
          activeItemIndex: 0,
        }]]),
        isSystemAgent: true,
      }],
      activeTabId: 'stale-chat',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })

    useTabsStore.getState().ensureSystemAgentTabs('hk47-manager', '/workspaces/HK47', 'Manager')

    const systemTabs = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    const chatTab = systemTabs.find((t) => t.title === 'Chat')!
    const chatData = (() => {
      const it = Array.from(chatTab.paneGroups.values())[0].items[0]
      return it.type === 'agent' ? (it.data as AgentItemData) : null
    })()

    // Reconciled to HK47, not left pointing at K2.
    expect(chatData?.projectPath).toBe('/workspaces/HK47')
    expect(chatData?.agentName).toBe('hk47-manager')
    // The Claude session belonged to the old workspace — dropped.
    expect(chatData?.sessionId).toBeUndefined()
    // Tab identity is preserved (state continuity), only data changed.
    expect(chatTab.id).toBe('stale-chat')
  })

  it('does not mutate a pinned tab that already matches the workspace', () => {
    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')
    const before = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)

    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')
    const after = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)

    // Same tab object references — no needless re-render when nothing changed.
    expect(after[0]).toBe(before[0])
    expect(after[1]).toBe(before[1])
  })

  it('inserts pinned tabs at the front of the strip', () => {
    // Seed a non-system tab first.
    useTabsStore.setState((s) => ({
      tabs: [
        ...s.tabs,
        {
          id: 'user-tab',
          title: 'README.md',
          mosaicTree: 'pg-x',
          paneGroups: new Map([['pg-x', {
            id: 'pg-x',
            items: [],
            activeItemIndex: 0,
          }]]),
        },
      ],
    }))

    useTabsStore.getState().ensureSystemAgentTabs('alice', '/tmp/proj', 'Agent')

    const tabs = useTabsStore.getState().tabs
    expect(tabs[0].isSystemAgent).toBe(true)
    expect(tabs[1].isSystemAgent).toBe(true)
    expect(tabs[2].id).toBe('user-tab')
  })
})

describe('ensurePinnedAgentTabForMode active-workspace guard', () => {
  beforeEach(() => {
    reset()
    useTabsStore.setState({ activeWorkspaceKey: null })
  })

  it('does NOT stamp pinned tabs when the workspace changed during async resolution', async () => {
    // Repro of the HK47 corruption: a workspace switch races the async
    // agent-name resolution. The call is FOR workspace A, but the user
    // switches to B before resolution finishes. The stale callback must
    // not write A's agent into B's (now-active) tab set.
    useTabsStore.setState({ activeWorkspaceKey: 'projA:wsA' })
    ensurePinnedAgentTabForMode('off', '/tmp/workspaceA')
    // User switches away before resolution completes.
    useTabsStore.setState({ activeWorkspaceKey: 'projB:wsB' })

    await flushPinnedTabResolution()

    expect(useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)).toHaveLength(0)
  })

  it('stamps pinned tabs when the workspace is unchanged', async () => {
    useTabsStore.setState({ activeWorkspaceKey: 'projA:wsA' })
    ensurePinnedAgentTabForMode('off', '/tmp/workspaceA')

    await flushPinnedTabResolution()

    const sys = useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)
    expect(sys.length).toBeGreaterThan(0)
    const item = Array.from(sys[0].paneGroups.values())[0]?.items[0]
    expect(item?.type).toBe('agent')
    expect((item!.data as AgentItemData).projectPath).toBe('/tmp/workspaceA')
  })
})

describe('stampAgentSessionId owner guard (GH#608)', () => {
  // GH#608 — two workspaces sharing the same agentName + projectPath
  // could cross-stamp: a session resolved by ONE workspace's chat pane
  // landed on the OTHER workspace's pinned chat item, restoring the wrong
  // chat history. The owner guard keys the stamp on the OWNING workspace
  // (ownerProjectId === the active project, since `state.tabs` always
  // holds the active workspace's tabs).

  function seedChatTab(agentName: string, projectPath: string, activeProjectId = 'proj-ACTIVE'): void {
    useTabsStore.setState({
      tabs: [
        {
          id: 'tab-chat',
          title: 'Chat',
          mosaicTree: 'pg-1',
          isSystemAgent: true,
          paneGroups: new Map([
            ['pg-1', {
              items: [
                {
                  id: 'item-chat',
                  type: 'agent',
                  data: { agentName, projectPath, section: 'chat' },
                  pinned: true,
                },
              ],
              activeItemIndex: 0,
            }],
          ]),
        } as never,
      ],
      activeTabId: 'tab-chat',
      // GH#679 — the guard now keys on activeWorkspaceKey (set in lockstep
      // with the loaded tabs), not the cross-store projects-active id.
      activeWorkspaceKey: `${activeProjectId}:ws-1`,
    })
  }

  function chatSessionId(): string | undefined {
    const item = Array.from(useTabsStore.getState().tabs[0].paneGroups.values())[0].items[0]
    return (item.data as AgentItemData).sessionId
  }

  beforeEach(() => {
    reset()
    registerActiveProjectIdGetter(() => 'proj-ACTIVE')
  })

  afterEach(() => {
    registerActiveProjectIdGetter(() => null)
  })

  it('stamps the chat item when the owning project IS the active project', () => {
    seedChatTab('shared-agent', '/shared/path')
    useTabsStore.getState().stampAgentSessionId('shared-agent', '/shared/path', 'session-OWNED', 'proj-ACTIVE')
    expect(chatSessionId()).toBe('session-OWNED')
  })

  it('DROPS the stamp when the owning project is NOT the active project', () => {
    // The tabs in the store belong to proj-ACTIVE. A stale/background
    // chat pane OWNED by a DIFFERENT workspace (same agentName +
    // projectPath) must not write its session onto the active tab.
    seedChatTab('shared-agent', '/shared/path')
    useTabsStore.getState().stampAgentSessionId('shared-agent', '/shared/path', 'session-FOREIGN', 'proj-OTHER')
    expect(chatSessionId()).toBeUndefined()
  })

  it('GH#679: stamps via activeWorkspaceKey even when the projects-store getter is STALE', () => {
    // Regression: the dropdown switch wrote the new session to SQLite but
    // the live PTY never swapped because the old guard compared against
    // the cross-store projects-active id, which lagged the tab set during
    // host-switch / re-fetch flows and silently dropped the legit stamp.
    // The tabs in the store belong to proj-ACTIVE (activeWorkspaceKey);
    // a STALE projects getter pointing elsewhere must NOT block the stamp.
    seedChatTab('shared-agent', '/shared/path', 'proj-ACTIVE')
    registerActiveProjectIdGetter(() => 'proj-STALE')
    useTabsStore.getState().stampAgentSessionId('shared-agent', '/shared/path', 'session-SWITCHED', 'proj-ACTIVE')
    expect(chatSessionId()).toBe('session-SWITCHED')
  })

  it('GH#679: falls back to the projects getter when activeWorkspaceKey is unset', () => {
    // Pure-unit path (no workspace restored). seedChatTab sets the key, so
    // clear it to exercise the fallback branch — the projects getter
    // (proj-ACTIVE from beforeEach) then authorizes the stamp.
    seedChatTab('shared-agent', '/shared/path')
    useTabsStore.setState({ activeWorkspaceKey: null })
    useTabsStore.getState().stampAgentSessionId('shared-agent', '/shared/path', 'session-FALLBACK', 'proj-ACTIVE')
    expect(chatSessionId()).toBe('session-FALLBACK')
  })
})

describe('removeSystemAgentTab', () => {
  beforeEach(reset)

  it('removes BOTH pinned tabs (inbox + chat) when called after the split', () => {
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')
    expect(useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)).toHaveLength(2)

    useTabsStore.getState().removeSystemAgentTab()
    expect(useTabsStore.getState().tabs.filter((t) => t.isSystemAgent)).toHaveLength(0)
  })
})

describe('moveTabToGroup (cross-column drag)', () => {
  function makeTab(id: string, title: string): import('./tabs').Tab {
    return {
      id,
      title,
      mosaicTree: `pg-${id}`,
      paneGroups: new Map([
        [`pg-${id}`, { id: `pg-${id}`, items: [], activeItemIndex: 0 }],
      ]),
    }
  }

  beforeEach(() => {
    useTabsStore.setState({
      tabs: [makeTab('a', 'A'), makeTab('b', 'B')],
      activeTabId: 'a',
      splitCount: 2,
      extraGroups: [{ tabs: [makeTab('c', 'C')], activeTabId: 'c' }],
      activeGroupIndex: 0,
    })
  })

  it('moves a tab from group 0 to group 1 and activates it there', () => {
    useTabsStore.getState().moveTabToGroup(0, 1, 'a')

    const s = useTabsStore.getState()
    expect(s.tabs.map((t) => t.id)).toEqual(['b'])
    expect(s.extraGroups[0].tabs.map((t) => t.id)).toEqual(['c', 'a'])
    expect(s.extraGroups[0].activeTabId).toBe('a')
  })

  it('moves a tab from group 1 back to group 0', () => {
    useTabsStore.getState().moveTabToGroup(1, 0, 'c')

    const s = useTabsStore.getState()
    expect(s.tabs.map((t) => t.id)).toEqual(['a', 'b', 'c'])
    expect(s.extraGroups[0].tabs).toHaveLength(0)
    expect(s.activeTabId).toBe('c')
  })

  it('updates source activeTabId when moving the active tab away', () => {
    useTabsStore.getState().moveTabToGroup(0, 1, 'a')
    expect(useTabsStore.getState().activeTabId).toBe('b')
  })

  it('is a no-op when source and target groups match', () => {
    const before = useTabsStore.getState().tabs.map((t) => t.id)
    useTabsStore.getState().moveTabToGroup(0, 0, 'a')
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(before)
  })

  it('is a no-op when the tab does not exist in the source group', () => {
    const before = JSON.parse(JSON.stringify({
      tabs: useTabsStore.getState().tabs.map((t) => t.id),
      extra: useTabsStore.getState().extraGroups[0].tabs.map((t) => t.id),
    }))
    useTabsStore.getState().moveTabToGroup(0, 1, 'does-not-exist')
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(before.tabs)
    expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.id)).toEqual(before.extra)
  })
})

// ── #587 — pinned HTML file tabs ─────────────────────────────────────────
//
// A pinned HTML file is a top-level tab (isPinnedFile) carrying a pinned
// file-viewer item. It sits right after the system (Chat/Inbox) tabs and
// before regular tabs, survives a serialize → restore round-trip, and is
// closed by *unpinning* (removal) rather than hiding.

/** Pull the filePath off a tab's first file-viewer item (helper for the
 *  pinned-file assertions below). */
function pinnedFilePath(tab: { paneGroups: Map<string, { items: Array<{ type: string; data: unknown }> }> }): string | null {
  const item = Array.from(tab.paneGroups.values())[0]?.items[0]
  if (item?.type !== 'file-viewer') return null
  return (item.data as { filePath?: string }).filePath ?? null
}

describe('pinned HTML file tabs (#587)', () => {
  beforeEach(reset)

  it('pins an HTML file into the workspace pinned-file list', () => {
    useTabsStore.getState().pinFileAsTab('/tmp/proj/report.html')

    const s = useTabsStore.getState()
    const pinned = s.tabs.filter((t) => t.isPinnedFile)
    expect(pinned).toHaveLength(1)
    expect(pinnedFilePath(pinned[0])).toBe('/tmp/proj/report.html')
    expect(useTabsStore.getState().isFilePinned('/tmp/proj/report.html')).toBe(true)
    // The pinned file-viewer item is itself marked pinned so the
    // unpinned-slot recycling in openFileInPane won't reuse it.
    const item = Array.from(pinned[0].paneGroups.values())[0].items[0]
    expect(item.pinned).toBe(true)
    // Pinning focuses the new tab.
    expect(s.activeTabId).toBe(pinned[0].id)
  })

  it('does not duplicate when pinning the same file twice (focuses instead)', () => {
    useTabsStore.getState().pinFileAsTab('/tmp/proj/a.html')
    const firstId = useTabsStore.getState().tabs.find((t) => t.isPinnedFile)?.id
    useTabsStore.getState().pinFileAsTab('/tmp/proj/a.html')

    const s = useTabsStore.getState()
    expect(s.tabs.filter((t) => t.isPinnedFile)).toHaveLength(1)
    expect(s.activeTabId).toBe(firstId)
  })

  it('orders pinned HTML tabs right after the system Chat/Inbox tabs', () => {
    // Seed the system Chat + Inbox tabs, then a regular terminal tab.
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')
    useTabsStore.getState().addTab('/tmp/proj', { title: 'Terminal 1' })
    useTabsStore.getState().pinFileAsTab('/tmp/proj/dash.html')

    const titles = useTabsStore.getState().tabs.map((t) => t.title)
    // [Chat] [Inbox] [pinned HTML] [regular terminal]
    expect(titles).toEqual(['Chat', 'Inbox', 'dash.html', 'Terminal 1'])
  })

  it('unpins a pinned HTML tab (removes it from the list)', () => {
    useTabsStore.getState().pinFileAsTab('/tmp/proj/x.html')
    useTabsStore.getState().unpinFileTab('/tmp/proj/x.html')

    const s = useTabsStore.getState()
    expect(s.tabs.filter((t) => t.isPinnedFile)).toHaveLength(0)
    expect(s.isFilePinned('/tmp/proj/x.html')).toBe(false)
  })

  it('closing a pinned HTML tab unpins it (removeTab → unpin)', () => {
    useTabsStore.getState().pinFileAsTab('/tmp/proj/y.html')
    const tabId = useTabsStore.getState().tabs.find((t) => t.isPinnedFile)!.id
    useTabsStore.getState().removeTab(tabId)

    expect(useTabsStore.getState().isFilePinned('/tmp/proj/y.html')).toBe(false)
  })

  it('survives a serialize → restore round-trip in the right order', () => {
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')
    useTabsStore.getState().addTab('/tmp/proj', { title: 'Terminal 1' })
    useTabsStore.getState().pinFileAsTab('/tmp/proj/dash.html')

    // Round-trip through the same path workspace layout persistence uses.
    const layout = useTabsStore.getState().serializeCurrentLayout()
    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')

    const tabs = useTabsStore.getState().tabs
    expect(tabs.map((t) => t.title)).toEqual(['Chat', 'Inbox', 'dash.html', 'Terminal 1'])

    const pinned = tabs.filter((t) => t.isPinnedFile)
    expect(pinned).toHaveLength(1)
    expect(pinned[0].isPinnedFile).toBe(true)
    expect(pinnedFilePath(pinned[0])).toBe('/tmp/proj/dash.html')
    // The restored pinned-file tab still sits immediately after the
    // two system tabs and before the regular terminal tab.
    const pinnedIdx = tabs.findIndex((t) => t.isPinnedFile)
    expect(tabs[pinnedIdx - 1].isSystemAgent).toBe(true)
    expect(tabs[pinnedIdx + 1].isSystemAgent).toBeFalsy()
    expect(tabs[pinnedIdx + 1].isPinnedFile).toBeFalsy()
  })
})

/**
 * D9 — sandbox (microVM) orange tab marker.
 *
 * The marker is DORMANT on Mac / feature-off builds (resolve_sandbox
 * never returns Microvm there), so these unit tests are the primary
 * proof the gating works. They cover (a) the store action that stamps
 * the resolved backend onto the terminal item, and (b) the exact
 * gating predicate TabBar uses to decide whether to render the bar.
 */
describe('D9 sandbox tab marker', () => {
  beforeEach(reset)

  // Mirror of TabBar.tsx's `isSandboxed` derivation. Kept in lockstep
  // with the renderer: a tab is marked iff SOME terminal item resolved
  // to the 'microvm' backend — passthrough / undefined render nothing.
  const isSandboxed = (tab: { paneGroups: Map<string, { items: Array<{ type: string; data: unknown }> }> }): boolean =>
    Array.from(tab.paneGroups.values()).some((pg) =>
      pg.items.some(
        (i) => i.type === 'terminal' && (i.data as TerminalItemData).sandboxBackend === 'microvm',
      ),
    )

  function makeTerminalTab(terminalId: string, sandboxBackend?: string): void {
    useTabsStore.setState({
      tabs: [{
        id: 'term-tab',
        title: 'Terminal 1',
        mosaicTree: 'pg-1',
        paneGroups: new Map([['pg-1', {
          id: 'pg-1',
          items: [{
            id: 'item-1',
            type: 'terminal',
            data: { terminalId, cwd: '/tmp/proj', renderer: 'kessel', sandboxBackend } as TerminalItemData,
          }],
          activeItemIndex: 0,
        }]]),
      }],
      activeTabId: 'term-tab',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })
  }

  function firstTerminalData(): TerminalItemData {
    const tab = useTabsStore.getState().tabs[0]
    const item = Array.from(tab.paneGroups.values())[0].items[0]
    return item.data as TerminalItemData
  }

  it("gates STRICTLY on 'microvm' — microvm ⇒ marked", () => {
    makeTerminalTab('t1', 'microvm')
    expect(isSandboxed(useTabsStore.getState().tabs[0])).toBe(true)
  })

  it("'passthrough' ⇒ NOT marked (degraded/no real isolation)", () => {
    makeTerminalTab('t1', 'passthrough')
    expect(isSandboxed(useTabsStore.getState().tabs[0])).toBe(false)
  })

  it('undefined backend (default-OFF / normal tab) ⇒ NOT marked', () => {
    makeTerminalTab('t1', undefined)
    expect(isSandboxed(useTabsStore.getState().tabs[0])).toBe(false)
  })

  it('setTerminalSandboxBackend stamps the resolved backend onto the matching terminal', () => {
    makeTerminalTab('t1', undefined)
    expect(firstTerminalData().sandboxBackend).toBeUndefined()
    expect(isSandboxed(useTabsStore.getState().tabs[0])).toBe(false)

    useTabsStore.getState().setTerminalSandboxBackend('t1', 'microvm')
    expect(firstTerminalData().sandboxBackend).toBe('microvm')
    expect(isSandboxed(useTabsStore.getState().tabs[0])).toBe(true)

    // Clearing back to passthrough drops the marker (truthful echo).
    useTabsStore.getState().setTerminalSandboxBackend('t1', 'passthrough')
    expect(firstTerminalData().sandboxBackend).toBe('passthrough')
    expect(isSandboxed(useTabsStore.getState().tabs[0])).toBe(false)
  })

  it('setTerminalSandboxBackend is an immutable update (new tab reference on change)', () => {
    makeTerminalTab('t1', undefined)
    const before = useTabsStore.getState().tabs[0]
    useTabsStore.getState().setTerminalSandboxBackend('t1', 'microvm')
    const after = useTabsStore.getState().tabs[0]
    // Reference changed (Zustand selectors must notice) — no in-place mutation.
    expect(after).not.toBe(before)
    expect(before.paneGroups).not.toBe(after.paneGroups)
  })

  it('no-ops (same references) when terminalId does not match', () => {
    makeTerminalTab('t1', 'microvm')
    const before = useTabsStore.getState().tabs
    useTabsStore.getState().setTerminalSandboxBackend('does-not-exist', 'passthrough')
    expect(useTabsStore.getState().tabs).toBe(before)
  })

  it('setTerminalConversationId stamps the provider uuid, not the PTY id', () => {
    makeTerminalTab('t1', undefined)
    expect(firstTerminalData().conversationId).toBeUndefined()
    useTabsStore.getState().setTerminalConversationId(
      't1',
      '01920000-aaaa-7000-8000-000000000001',
    )
    expect(firstTerminalData().conversationId).toBe('01920000-aaaa-7000-8000-000000000001')
    expect(firstTerminalData().sessionId).not.toBe(firstTerminalData().conversationId)
  })
})

describe('v2 layout conversationId persist (extra LLM tab restore)', () => {
  beforeEach(reset)

  const CONV = '01920000-aaaa-7000-8000-0000000000ab'
  const PG = 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'

  function extraLlmTab(): void {
    useTabsStore.setState({
      tabs: [{
        id: 'extra-llm',
        title: 'Reviewer',
        mosaicTree: PG,
        paneGroups: new Map([[PG, {
          id: PG,
          items: [{
            id: 'item-1',
            type: 'terminal',
            data: {
              terminalId: PG,
              cwd: '/tmp/proj',
              renderer: 'kessel',
              command: 'claude',
              args: ['--resume', CONV],
              commandHint: 'claude',
              conversationId: CONV,
              sessionId: 'pty-not-the-chat-key',
            } as TerminalItemData,
          }],
          activeItemIndex: 0,
        }]]),
      }],
      activeTabId: 'extra-llm',
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })
  }

  it('serializeTab persists conversationId and commandHint, not command/args', () => {
    extraLlmTab()
    const layout = useTabsStore.getState().serializeCurrentLayout()
    const item = Object.values(layout.tabs[0].paneGroups)[0].items[0] as {
      type: string
      paneGroupId?: string
      conversationId?: string
      commandHint?: string
      command?: string
      args?: string[]
    }
    expect(item.type).toBe('terminal')
    expect(item.paneGroupId).toBe(PG)
    expect(item.conversationId).toBe(CONV)
    expect(item.commandHint).toBe('claude')
    expect(item.command).toBeUndefined()
    expect(item.args).toBeUndefined()
  })

  it('restoreLayout stamps conversationId and never copies commandHint into command', () => {
    extraLlmTab()
    const layout = useTabsStore.getState().serializeCurrentLayout()
    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')

    const tab = useTabsStore.getState().tabs[0]
    expect(tab.paneGroups.has(PG)).toBe(true)
    const data = Array.from(tab.paneGroups.values())[0].items[0].data as TerminalItemData
    expect(data.conversationId).toBe(CONV)
    expect(data.commandHint).toBe('claude')
    expect(data.command).toBeUndefined()
    expect(data.args).toBeUndefined()
    expect(data.sessionId).toBeUndefined()
  })
})

/**
 * Browser-pane arc (0.40.x) — 'browser' pane items.
 *
 * The page itself lives in a NATIVE child webview (src-tauri
 * browser_* commands); the store only carries { url, title }. These
 * tests cover the store surface: item shape, serialize→restore
 * round-trip, the navigate-in-place reuse rule of openUrlInPane, and
 * the setBrowserItemState stamp the current-URL poll uses.
 */
describe('browser pane items', () => {
  beforeEach(reset)

  function browserItems(tabIndex: number): Array<{ id: string; type: string; data: BrowserItemData }> {
    const tab = useTabsStore.getState().tabs[tabIndex]
    const pg = Array.from(tab.paneGroups.values())[0]
    return pg.items.filter((i) => i.type === 'browser') as Array<{ id: string; type: string; data: BrowserItemData }>
  }

  it('openUrlInNewTab creates a tab with a single browser item and activates it', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')

    const state = useTabsStore.getState()
    expect(state.tabs).toHaveLength(1)
    const tab = state.tabs[0]
    expect(state.activeTabId).toBe(tab.id)
    // Tab title is the hostname (best-effort display label).
    expect(tab.title).toBe('example.com')

    const pgs = Array.from(tab.paneGroups.values())
    expect(pgs).toHaveLength(1)
    expect(pgs[0].items).toHaveLength(1)
    const item = pgs[0].items[0]
    expect(item.type).toBe('browser')
    expect((item.data as BrowserItemData).url).toBe('https://example.com/docs')
    expect(pgs[0].activeItemIndex).toBe(0)
    // Mosaic tree is the single paneGroup leaf, like openFileInNewTab.
    expect(tab.mosaicTree).toBe(pgs[0].id)
  })

  // V8/V19 — a same-value stamp (a remounted webview reporting the URL the
  // layout already has) must not make new tab objects: that fired the
  // autosave, which rebuilt the other window, whose new webview stamped…
  it('setBrowserItemState with the same url and title changes nothing', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    const itemId = pg.items[0].id
    useTabsStore.getState().setBrowserItemState(tab.id, pg.id, itemId, { url: 'https://example.com/x', title: 'X' })
    const before = useTabsStore.getState().tabs

    useTabsStore.getState().setBrowserItemState(tab.id, pg.id, itemId, { url: 'https://example.com/x', title: 'X' })
    useTabsStore.getState().setBrowserItemState(tab.id, pg.id, itemId, { url: 'https://example.com/x' })

    expect(useTabsStore.getState().tabs).toBe(before)
    useTabsStore.getState().setBrowserItemState(tab.id, pg.id, itemId, { url: 'https://example.com/y' })
    expect(useTabsStore.getState().tabs).not.toBe(before)
    expect(browserItems(0)[0].data.url).toBe('https://example.com/y')
  })

  it('setTabDirty with the flag it already has changes nothing', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const tabId = useTabsStore.getState().tabs[0].id
    const before = useTabsStore.getState().tabs
    useTabsStore.getState().setTabDirty(tabId, false)
    expect(useTabsStore.getState().tabs).toBe(before)
    useTabsStore.getState().setTabDirty(tabId, true)
    expect(useTabsStore.getState().tabs).not.toBe(before)
    expect(useTabsStore.getState().tabs[0].isDirty).toBe(true)
  })

  // V21 — only this window's own navigate request bumps navSeq (what the
  // pane navigates on); it is never written into the shared layout.
  it('openUrlInPane bumps navSeq on the reused item, and navSeq is not serialized', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const tab = useTabsStore.getState().tabs[0]
    expect(browserItems(0)[0].data.navSeq).toBeUndefined()

    useTabsStore.getState().openUrlInPane(tab.id, 'https://example.com/next')
    expect(browserItems(0)[0].data.navSeq).toBe(1)
    useTabsStore.getState().openUrlInPane(tab.id, 'https://example.com/again')
    expect(browserItems(0)[0].data.navSeq).toBe(2)
    expect(browserItems(0)[0].data.url).toBe('https://example.com/again')

    const layout = useTabsStore.getState().serializeCurrentLayout()
    const item = Object.values(layout.tabs[0].paneGroups)[0].items[0] as unknown as Record<string, unknown>
    expect(item.url).toBe('https://example.com/again')
    expect('navSeq' in item).toBe(false)
  })

  it('browser item survives a serialize → restore round-trip (url + title)', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/start')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    const itemId = pg.items[0].id
    // Simulate the current-URL poll stamping in-page navigation + title.
    useTabsStore.getState().setBrowserItemState(tab.id, pg.id, itemId, {
      url: 'https://example.com/after-nav',
      title: 'After Nav',
    })

    const layout = useTabsStore.getState().serializeCurrentLayout()
    // The serialized shape is the additive SerializedBrowserItem.
    const serializedItems = Object.values(layout.tabs[0].paneGroups)[0].items
    expect(serializedItems).toHaveLength(1)
    expect(serializedItems[0]).toMatchObject({
      type: 'browser',
      url: 'https://example.com/after-nav',
      title: 'After Nav',
    })

    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')

    const restored = browserItems(0)
    expect(restored).toHaveLength(1)
    expect(restored[0].data.url).toBe('https://example.com/after-nav')
    expect(restored[0].data.title).toBe('After Nav')
    // V3/V20 — restore keeps the saved item id, so a remote save never
    // remounts (and re-creates the native view of) a surviving pane.
    expect(restored[0].id).toBe(itemId)
  })

  it('old layouts without browser items still restore unchanged', () => {
    useTabsStore.getState().addTab('/tmp/proj', { title: 'Terminal 1' })
    const layout = useTabsStore.getState().serializeCurrentLayout()

    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')

    const tabs = useTabsStore.getState().tabs
    expect(tabs).toHaveLength(1)
    const items = Array.from(tabs[0].paneGroups.values())[0].items
    expect(items).toHaveLength(1)
    expect(items[0].type).toBe('terminal')
  })

  it('openUrlInPane reuses the existing unpinned browser item (navigate-in-place)', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/a')
    const tabId = useTabsStore.getState().tabs[0].id

    useTabsStore.getState().openUrlInPane(tabId, 'https://example.com/b')

    const items = browserItems(0)
    expect(items).toHaveLength(1)
    expect(items[0].data.url).toBe('https://example.com/b')
  })

  it('openUrlInPane appends a new browser item when none exists in the active pane group', () => {
    useTabsStore.getState().addTab('/tmp/proj', { title: 'Terminal 1' })
    const tabId = useTabsStore.getState().tabs[0].id

    useTabsStore.getState().openUrlInPane(tabId, 'https://example.com/new')

    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    expect(pg.items).toHaveLength(2)
    expect(pg.items[0].type).toBe('terminal')
    expect(pg.items[1].type).toBe('browser')
    expect((pg.items[1].data as BrowserItemData).url).toBe('https://example.com/new')
    // The new browser item becomes the active item.
    expect(pg.activeItemIndex).toBe(1)
  })

  it('setBrowserItemState merges partial updates immutably', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com')
    const before = useTabsStore.getState().tabs[0]
    const pg = Array.from(before.paneGroups.values())[0]
    const itemId = pg.items[0].id

    useTabsStore.getState().setBrowserItemState(before.id, pg.id, itemId, { title: 'Example' })

    const after = useTabsStore.getState().tabs[0]
    expect(after).not.toBe(before)
    const item = Array.from(after.paneGroups.values())[0].items[0]
    expect((item.data as BrowserItemData).url).toBe('https://example.com')
    expect((item.data as BrowserItemData).title).toBe('Example')
    // Item title only. The strip stays the host name.
    expect(after.title).toBe('example.com')
    expect(after.locked).not.toBe(true)
  })
})

describe('browser page title and favicon', () => {
  beforeEach(reset)

  const ICON = 'data:image/png;base64,AAAA'

  function located(): { tabId: string; pgId: string; itemId: string } {
    const tab = useTabsStore.getState().tabs[0]
    if (!tab) throw new Error('expected a browser tab')
    const pg = Array.from(tab.paneGroups.values())[0]
    if (!pg) throw new Error('expected a pane group')
    const item = pg.items.find((i) => i.type === 'browser')
    if (!item) throw new Error('expected a browser item')
    return { tabId: tab.id, pgId: pg.id, itemId: item.id }
  }

  function itemData(): BrowserItemData {
    const { tabId, itemId } = located()
    const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId)
      ?? useTabsStore.getState().extraGroups.flatMap((g) => g.tabs).find((t) => t.id === tabId)
    if (!tab) throw new Error('tab missing')
    for (const pg of tab.paneGroups.values()) {
      const item = pg.items.find((i) => i.id === itemId)
      if (item) return item.data as BrowserItemData
    }
    throw new Error('browser item missing')
  }

  it('replaces the host name on an unlocked browser-only tab', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    expect(useTabsStore.getState().tabs[0].title).toBe('example.com')
    const ids = located()
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: '  Example Domain  ',
      icon: ICON,
    })
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Example Domain')
    expect(tab.locked).not.toBe(true)
    expect(itemData().title).toBe('Example Domain')
    expect(itemData().icon).toBe(ICON)
  })

  it('leaves the host name when the page title is empty', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const ids = located()
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: '   ',
      icon: '',
    })
    expect(useTabsStore.getState().tabs[0].title).toBe('example.com')
    expect(itemData().title).toBe('')
    expect(itemData().icon).toBeUndefined()
  })

  it('does not overwrite a locked strip title and still stores the page title', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const ids = located()
    useTabsStore.getState().setTabTitle(ids.tabId, 'Kept', { locked: true })
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: 'Example Domain',
    })
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Kept')
    expect(tab.locked).toBe(true)
    expect(itemData().title).toBe('Example Domain')
  })

  it('applies a harness-looking title that setTabTitle would drop', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const ids = located()
    useTabsStore.getState().setTabTitle(ids.tabId, 'Claude')
    expect(useTabsStore.getState().tabs[0].title).toBe('example.com')
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: 'Claude',
    })
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Claude')
    expect(tab.locked).not.toBe(true)
  })

  it('does not retitle a mixed tab', () => {
    useTabsStore.getState().addTab('/tmp/proj', { title: 'Terminal 1' })
    const tabId = useTabsStore.getState().tabs[0].id
    useTabsStore.getState().openUrlInPane(tabId, 'https://example.com/new')
    const ids = located()
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: 'Example Domain',
    })
    expect(useTabsStore.getState().tabs[0].title).toBe('Terminal 1')
    expect(itemData().title).toBe('Example Domain')
  })

  it('rejects an https favicon and keeps a data url across restore', () => {
    expect(LAYOUT_SCHEMA_VERSION).toBe(2)
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const ids = located()
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      icon: 'https://example.com/favicon.ico',
    })
    expect(itemData().icon).toBeUndefined()
    expect(useTabsStore.getState().tabs[0].title).toBe('example.com')

    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: 'Example Domain',
      icon: ICON,
    })
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect(layout.version).toBe(LAYOUT_SCHEMA_VERSION)
    const serialized = Object.values(layout.tabs[0].paneGroups)[0].items[0]
    expect(serialized).toMatchObject({
      type: 'browser',
      title: 'Example Domain',
      icon: ICON,
    })

    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')
    expect(useTabsStore.getState().tabs[0].title).toBe('Example Domain')
    const restored = Array.from(useTabsStore.getState().tabs[0].paneGroups.values())[0].items[0]
    expect((restored.data as BrowserItemData).icon).toBe(ICON)
    expect((restored.data as BrowserItemData).title).toBe('Example Domain')
  })

  it('tolerates an old browser layout with no icon field', () => {
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const layout = useTabsStore.getState().serializeCurrentLayout()
    const serialized = Object.values(layout.tabs[0].paneGroups)[0].items[0]
    if (serialized.type !== 'browser') throw new Error('expected a browser item')
    delete serialized.icon
    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')
    const restored = Array.from(useTabsStore.getState().tabs[0].paneGroups.values())[0].items[0]
    expect((restored.data as BrowserItemData).icon).toBeUndefined()
    expect((restored.data as BrowserItemData).url).toBe('https://example.com/docs')
  })

  it('does not call the window-title helper with the page title', () => {
    const spy = vi.spyOn(pageTitle, 'k2PageTitle')
    useTabsStore.getState().openUrlInNewTab('https://example.com/docs')
    const ids = located()
    useTabsStore.getState().applyBrowserPageMeta(ids.tabId, ids.pgId, ids.itemId, {
      title: 'Example Domain',
      icon: ICON,
    })
    expect(spy).not.toHaveBeenCalled()
    const joined = spy.mock.calls.map((args) => args.map(String).join(' ')).join('\n')
    expect(joined).not.toContain('Example Domain')
    spy.mockRestore()

    const root = join(dirname(fileURLToPath(import.meta.url)), '../../..')
    const tabsSrc = readFileSync(join(root, 'src/renderer/stores/tabs.ts'), 'utf8')
    const helper = sourceBetween(tabsSrc, 'function withBrowserPageMeta(', 'function placedTab(')
    const action = sourceBetween(tabsSrc, 'applyBrowserPageMeta: (tabId', 'openUntitledDocument:')
    for (const body of [helper, action]) {
      expect(body).not.toContain('setTabTitle')
      expect(body).not.toContain('adoptTabTitle')
      expect(body).not.toContain('applyDaemonTabTitle')
      expect(body).not.toContain('k2PageTitle')
      expect(body).not.toContain('setTitle')
      expect(body).not.toContain('locked:')
    }
    // One shared item restore for column 0 and the split columns (V20).
    expect(tabsSrc.match(/icon: si\.icon/g)?.length).toBe(1)
    expect(tabsSrc).toContain('icon: d.icon')

    const pane = readFileSync(join(root, 'src/renderer/components/BrowserPane/BrowserPane.tsx'), 'utf8')
    expect(pane).toContain('browser:page-meta')
    expect(pane).toContain('applyBrowserPageMeta')
    expect(pane).not.toContain('k2PageTitle')
    expect(pane).not.toContain('.setTitle(')

    const rust = readFileSync(join(root, 'src-tauri/src/commands/browser_webviews.rs'), 'utf8')
    expect(rust).toContain('on_document_title_changed')
    expect(rust).toContain('eval_with_callback')
    expect(rust).toContain('PageLoadEvent::Finished')
    expect(rust).not.toContain('set_title')
    expect(rust).not.toContain('.setTitle(')

    const conf = readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8')
    expect(conf).toContain("img-src 'self' asset: data: blob:")
    expect(conf).not.toMatch(/img-src[^;]*https:/)
  })
})

function sourceBetween(src: string, start: string, end: string): string {
  const i = src.indexOf(start)
  if (i < 0) throw new Error(`missing ${start}`)
  const j = src.indexOf(end, i + start.length)
  if (j < 0) throw new Error(`missing ${end} after ${start}`)
  return src.slice(i, j)
}

describe('addTab / addTabToGroup locked option', () => {
  beforeEach(reset)

  it('addTabToGroup returns a pane-group id and sets Tab.locked at construction', () => {
    const pgId = useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'My Chat',
      command: 'claude',
      args: ['--resume', '01920000-aaaa-7000-8000-000000000001'],
      locked: true,
    })
    const tabs = useTabsStore.getState().tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0].paneGroups.has(pgId)).toBe(true)
    expect(tabs[0].id).not.toBe(pgId)
    expect(tabs[0].locked).toBe(true)
    expect(tabs[0].title).toBe('My Chat')
  })

  it('addTab sets Tab.locked when options.locked is true', () => {
    const pgId = useTabsStore.getState().addTab('/tmp/proj', { title: 'Locked', locked: true })
    const tab = useTabsStore.getState().tabs.find((t) => t.paneGroups.has(pgId))
    expect(tab).toBeDefined()
    expect(tab!.locked).toBe(true)
    expect(tab!.title).toBe('Locked')
  })

  it('unlocked construction leaves Tab.locked unset', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', { title: 'Plain' })
    expect(useTabsStore.getState().tabs[0].locked).toBeUndefined()
  })
})

describe('named chat tab title (N1–N6)', () => {
  beforeEach(reset)

  const CONV = '01920000-aaaa-7000-8000-000000000001'

  function firstTerminalData(): TerminalItemData {
    const tab = useTabsStore.getState().tabs[0]
    return Array.from(tab.paneGroups.values())[0].items[0].data as TerminalItemData
  }

  it('open named chat: first tab.title is custom name, conversationId stamped, locked', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'Code Review',
      command: 'claude',
      args: ['--resume', CONV],
      locked: true,
      conversationId: CONV,
    })
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Code Review')
    expect(tab.locked).toBe(true)
    expect(firstTerminalData().conversationId).toBe(CONV)
    expect(firstTerminalData().sessionId).toBeUndefined()
  })

  it('conversationId at create locks the display name even without locked: true', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'Code Review',
      command: 'claude',
      conversationId: CONV,
    })
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Code Review')
    expect(tab.locked).toBe(true)
  })

  it('fake label_initial "claude" does not unlock or overwrite the custom name', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'Code Review',
      command: 'claude',
      args: ['--resume', CONV],
      locked: true,
      conversationId: CONV,
    })
    const tab = useTabsStore.getState().tabs[0]
    // TerminalPane label_initial / OSC path: unlocked setTabTitle
    useTabsStore.getState().setTabTitle(tab.id, 'claude')
    expect(useTabsStore.getState().tabs[0].title).toBe('Code Review')
    expect(useTabsStore.getState().tabs[0].locked).toBe(true)
    // applyDaemonTabTitle snapshot / broadcast without locked: true
    useTabsStore.getState().applyDaemonTabTitle(tab.id, 'claude')
    expect(useTabsStore.getState().tabs[0].title).toBe('Code Review')
    expect(useTabsStore.getState().tabs[0].locked).toBe(true)
    useTabsStore.getState().applyDaemonTabTitle(tab.id, 'Claude Code', false)
    expect(useTabsStore.getState().tabs[0].title).toBe('Code Review')
    expect(useTabsStore.getState().tabs[0].locked).toBe(true)
    // chat/list restamp with title=grok and locked:true
    useTabsStore.getState().setTabTitle(tab.id, 'grok', { locked: true })
    expect(useTabsStore.getState().tabs[0].title).toBe('Code Review')
  })

  it('a locked remote user rename still applies', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'Code Review',
      conversationId: CONV,
    })
    const tab = useTabsStore.getState().tabs[0]
    useTabsStore.getState().applyDaemonTabTitle(tab.id, 'Renamed elsewhere', true)
    expect(useTabsStore.getState().tabs[0].title).toBe('Renamed elsewhere')
    expect(useTabsStore.getState().tabs[0].locked).toBe(true)
  })

  it('Hi Test + restamp grok locked true stays Hi Test (T16a)', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'Hi Test',
      command: 'grok',
      locked: true,
      conversationId: CONV,
    })
    const tab = useTabsStore.getState().tabs[0]
    useTabsStore.getState().setTabTitle(tab.id, 'grok', { locked: true })
    useTabsStore.getState().applyDaemonTabTitle(tab.id, 'grok', true)
    expect(useTabsStore.getState().tabs[0].title).toBe('Hi Test')
    expect(useTabsStore.getState().tabs[0].locked).toBe(true)
  })

  it('restoreLayout restamps grok + conversationId from the custom-name map (T16d)', () => {
    rememberChatCustomName(primaryScope(), CONV, 'Hi Test')
    const layout: SerializedLayout = {
      version: 2,
      tabs: [{
        id: 'extra-1',
        title: 'grok',
        mosaicTree: 'pg-1',
        paneGroups: {
          'pg-1': {
            id: 'pg-1',
            items: [{
              id: 'item-1',
              type: 'terminal',
              paneGroupId: 'pg-1',
              commandHint: 'grok',
              conversationId: CONV,
            }],
            activeItemIndex: 0,
          },
        },
      }],
    }
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Hi Test')
    expect(tab.title).not.toBe('grok')
    expect(tab.locked).toBe(true)
    expect(tab.locked).not.toBeUndefined()
  })

  it('restoreLayout restamps grok extras from a tab_titles snapshot when conversationId is null (T10)', () => {
    rememberTabTitleSnapshot(primaryScope(), 'extra-1', 'Hi Test', true)
    const layout: SerializedLayout = {
      version: 2,
      tabs: [{
        id: 'extra-1',
        title: 'grok',
        mosaicTree: 'pg-1',
        paneGroups: {
          'pg-1': {
            id: 'pg-1',
            items: [{
              id: 'item-1',
              type: 'terminal',
              paneGroupId: 'pg-1',
              commandHint: 'grok',
            }],
            activeItemIndex: 0,
          },
        },
      }],
    }
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Hi Test')
    expect(tab.locked).toBe(true)
  })

  it('serializeTab emits locked: false when unlocked and never wipes conversationId to null (T15)', () => {
    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', {
      title: 'Hi Test',
      command: 'grok',
      conversationId: CONV,
    })
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect(layout.tabs[0].locked).toBe(true)
    const unlocked = useTabsStore.getState().serializeCurrentLayout()
    // commandHint is display-only — not the tab title.
    expect(unlocked.tabs[0].title).toBe('Hi Test')
    const item = Object.values(layout.tabs[0].paneGroups)[0].items[0] as {
      conversationId?: string | null
      commandHint?: string
    }
    expect(item.conversationId).toBe(CONV)
    expect(JSON.stringify(layout).includes('"conversationId":null')).toBe(false)
    expect(item.commandHint).not.toBe('Hi Test')

    useTabsStore.getState().addTabToGroup(0, '/tmp/proj', { title: 'Plain' })
    const plain = useTabsStore.getState().serializeCurrentLayout()
    const plainTab = plain.tabs.find((t) => t.title === 'Plain')
    expect(plainTab?.locked).toBe(false)
    expect(plainTab?.locked).not.toBeUndefined()
  })
})

describe('plus-menu column targets', () => {
  beforeEach(reset)

  function column1(): void {
    useTabsStore.setState({
      splitCount: 2,
      extraGroups: [{ tabs: [], activeTabId: null }],
      activeGroupIndex: 0,
    })
  }

  it('empty openUrlInNewTab titles Browser and does not call browser_create', () => {
    vi.mocked(invoke).mockClear()
    useTabsStore.getState().openUrlInNewTab('')
    const tab = useTabsStore.getState().tabs[0]
    expect(tab.title).toBe('Browser')
    const item = Array.from(tab.paneGroups.values())[0].items[0]
    expect(item.type).toBe('browser')
    expect((item.data as BrowserItemData).url).toBe('')
    expect(vi.mocked(invoke).mock.calls.map((c) => c[0])).not.toContain('browser_create')
  })

  it('openUrlInNewTab and openUntitledDocument with a group index append to that column', () => {
    column1()
    useTabsStore.getState().addTab('/tmp/proj', { title: 'Keep' })
    useTabsStore.getState().openUrlInNewTab('', 1)
    useTabsStore.getState().openUntitledDocument('/tmp/proj', 1)
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(['Keep'])
    const extra = useTabsStore.getState().extraGroups[0].tabs
    expect(extra.map((t) => t.title)).toEqual(['Browser', 'Untitled'])
    const browser = extra[0]
    expect((Array.from(browser.paneGroups.values())[0].items[0].data as BrowserItemData).url).toBe('')
    expect(useTabsStore.getState().extraGroups[0].activeTabId).toBe(extra[1].id)
  })

  it('omitted group index stays on column 0 when another column is active', () => {
    column1()
    useTabsStore.setState({ activeGroupIndex: 1 })
    useTabsStore.getState().openUntitledDocument('/tmp/proj')
    useTabsStore.getState().openUrlInNewTab('https://example.com')
    expect(useTabsStore.getState().tabs.map((t) => t.title)).toEqual(['Untitled', 'example.com'])
    expect(useTabsStore.getState().extraGroups[0].tabs).toHaveLength(0)
    expect(useTabsStore.getState().activeGroupIndex).toBe(1)
  })
})
