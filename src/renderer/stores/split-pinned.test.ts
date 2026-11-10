// The "Split into columns" button and the pinned tab (Rosson 2026-10-16):
//   1. On the pinned Chat/Inbox tab → the new column starts EMPTY.
//   2. On any other tab → that tab MOVES (same id, PTY untouched) into the
//      new column, which gets focus, and the source column shows the
//      pinned tab.
//   Fallbacks: no pinned tab → still move the tab when the source keeps
//   another one; the source's only tab stays and the new column starts
//   empty. A split never makes a terminal (Rosson 2026-11-09).
//   Every new column (2nd, 3rd) follows the same rule, from the FOCUSED
//   column (the split button keeps the focus where it was).
//   An empty column must survive serialize → restore and the 409 merge.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore, type Tab, type SerializedLayout } from './tabs'
import { mergeSerializedLayouts, layoutColumns } from '@/lib/layout-merge'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'k2so_agents_list') return []
    if (cmd === 'k2so_workspace_agent_display_name') return 'resolved-agent'
    return null
  }),
}))

function termTab(id: string): Tab {
  return {
    id,
    title: id,
    mosaicTree: `pg-${id}`,
    paneGroups: new Map([[
      `pg-${id}`,
      {
        id: `pg-${id}`,
        items: [{ id: `item-${id}`, type: 'terminal', data: { terminalId: `term-${id}`, cwd: '/ws' } }],
        activeItemIndex: 0,
      },
    ]]),
  }
}

function systemTab(id: string, section: 'chat' | 'inbox'): Tab {
  return {
    id,
    title: section === 'chat' ? 'Chat' : 'Inbox',
    mosaicTree: `pg-${id}`,
    paneGroups: new Map([[
      `pg-${id}`,
      {
        id: `pg-${id}`,
        items: [{ id: `item-${id}`, type: 'agent', data: { agentName: 'k2', projectPath: '/ws', section } }],
        activeItemIndex: 0,
      },
    ]]),
    isSystemAgent: true,
  }
}

function seed(
  tabs: Tab[],
  activeTabId: string | null,
  extraGroups: Array<{ tabs: Tab[]; activeTabId: string | null }> = [],
): void {
  useTabsStore.setState({
    tabs,
    activeTabId,
    extraGroups,
    splitCount: 1 + extraGroups.length,
    activeGroupIndex: 0,
    // No open workspace: flushLayoutPersist has nothing to save to.
    activeWorkspaceKey: null,
    activeProjectId: null,
    activeWorkspaceId: null,
    workspaceLayouts: {},
    backgroundWorkspaces: {},
  })
}

function ids(tabs: Tab[]): string[] {
  return tabs.map((t) => t.id)
}

/** Every pane-group id and terminal item across all columns: a split that
 *  minted a terminal shows up as a new id here. */
function liveTerminals(): { paneGroups: string[]; terminals: string[] } {
  const s = useTabsStore.getState()
  const all = [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
  const paneGroups = all.flatMap((t) => [...t.paneGroups.keys()]).sort()
  const terminals = all
    .flatMap((t) => [...t.paneGroups.values()].flatMap((pg) => pg.items))
    .filter((i) => i.type === 'terminal')
    .map((i) => i.id)
    .sort()
  return { paneGroups, terminals }
}

/** Split, and fail if it made a terminal or called the backend (a spawn). */
function splitNoSpawn(fromGroup?: number): void {
  const before = liveTerminals()
  vi.mocked(invoke).mockClear()
  useTabsStore.getState().splitTerminalArea('/ws', fromGroup)
  expect(liveTerminals()).toEqual(before)
  expect(vi.mocked(invoke)).not.toHaveBeenCalled()
}

describe('split button on the pinned tab (case 1)', () => {
  beforeEach(() => seed([systemTab('chat', 'chat'), systemTab('inbox', 'inbox'), termTab('t1')], 'chat'))

  it('adds an empty, focused column and creates no tab', () => {
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(2)
    expect(s.extraGroups).toEqual([{ tabs: [], activeTabId: null }])
    expect(s.activeGroupIndex).toBe(1)
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1'])
    expect(s.activeTabId).toBe('chat')
  })

  it('also applies on the pinned Inbox tab', () => {
    useTabsStore.setState({ activeTabId: 'inbox' })
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const s = useTabsStore.getState()
    expect(s.extraGroups).toEqual([{ tabs: [], activeTabId: null }])
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1'])
    expect(s.activeTabId).toBe('inbox')
  })

  it('the empty column fills when a tab is moved into it, and unsplit still works', () => {
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    useTabsStore.getState().moveTabToGroup(0, 1, 't1')
    let s = useTabsStore.getState()
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t1'])
    expect(s.extraGroups[0].activeTabId).toBe('t1')

    useTabsStore.getState().unsplitTerminalArea()
    s = useTabsStore.getState()
    expect(s.splitCount).toBe(1)
    expect(s.extraGroups).toEqual([])
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1'])
  })

  it('unsplitting the still-empty column leaves the tabs alone', () => {
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    useTabsStore.getState().unsplitTerminalArea()
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(1)
    expect(s.extraGroups).toEqual([])
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1'])
  })

  it('+ in the empty column adds a tab there', () => {
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    useTabsStore.getState().addTabToGroup(1, '/ws')
    const s = useTabsStore.getState()
    expect(s.extraGroups[0].tabs).toHaveLength(1)
    expect(s.extraGroups[0].activeTabId).toBe(s.extraGroups[0].tabs[0].id)
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1'])
  })
})

describe('split button on another tab (case 2)', () => {
  beforeEach(() => seed([systemTab('chat', 'chat'), systemTab('inbox', 'inbox'), termTab('t1'), termTab('t2')], 't2'))

  it('moves the current tab (same object) into the focused new column and shows Chat on the left', () => {
    const before = useTabsStore.getState().tabs.find((t) => t.id === 't2')
    if (!before) throw new Error('seed lost t2')
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(2)
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1'])
    expect(s.activeTabId).toBe('chat')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t2'])
    expect(s.extraGroups[0].activeTabId).toBe('t2')
    // Moved, not copied: the very same Tab and pane group (PTY untouched).
    expect(s.extraGroups[0].tabs[0]).toBe(before)
    expect([...s.extraGroups[0].tabs[0].paneGroups.keys()]).toEqual(['pg-t2'])
    expect(s.activeGroupIndex).toBe(1)
  })

  it('moves the only regular tab too (the pinned tab keeps the column)', () => {
    seed([systemTab('chat', 'chat'), termTab('t1')], 't1')
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const s = useTabsStore.getState()
    expect(ids(s.tabs)).toEqual(['chat'])
    expect(s.activeTabId).toBe('chat')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t1'])
    expect(s.activeGroupIndex).toBe(1)
  })

  it('with only an Inbox pinned tab (workspace board), shows the Inbox on the left', () => {
    seed([systemTab('inbox', 'inbox'), termTab('t1')], 't1')
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const s = useTabsStore.getState()
    expect(s.activeTabId).toBe('inbox')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t1'])
  })
})

describe('split button fallbacks (no pinned tab)', () => {
  it('moves the current tab when the column keeps another tab; the neighbour shows', () => {
    seed([termTab('t1'), termTab('t2')], 't2')
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const s = useTabsStore.getState()
    expect(ids(s.tabs)).toEqual(['t1'])
    expect(s.activeTabId).toBe('t1')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t2'])
    expect(s.extraGroups[0].activeTabId).toBe('t2')
    expect(s.activeGroupIndex).toBe(1)
  })

  it('the column’s only tab stays; the new column starts empty (no fresh terminal)', () => {
    seed([termTab('t1')], 't1')
    splitNoSpawn(0)
    const s = useTabsStore.getState()
    expect(ids(s.tabs)).toEqual(['t1'])
    expect(s.activeTabId).toBe('t1')
    expect(s.extraGroups).toEqual([{ tabs: [], activeTabId: null }])
    expect(s.activeGroupIndex).toBe(1)
  })

  it('3 columns: splitting from the rightmost moves its current tab into column 3', () => {
    seed([systemTab('chat', 'chat'), termTab('t1')], 'chat', [{ tabs: [termTab('a'), termTab('b')], activeTabId: 'a' }])
    useTabsStore.getState().splitTerminalArea('/ws', 1)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(ids(s.tabs)).toEqual(['chat', 't1'])
    expect(s.activeTabId).toBe('chat')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['b'])
    expect(s.extraGroups[0].activeTabId).toBe('b')
    expect(ids(s.extraGroups[1].tabs)).toEqual(['a'])
    expect(s.extraGroups[1].activeTabId).toBe('a')
    expect(s.activeGroupIndex).toBe(2)
  })

  it('3 columns: splitting from an empty rightmost column adds an empty column (no terminal)', () => {
    seed([systemTab('chat', 'chat'), termTab('t1')], 'chat', [{ tabs: [], activeTabId: null }])
    splitNoSpawn(1)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(s.extraGroups).toEqual([{ tabs: [], activeTabId: null }, { tabs: [], activeTabId: null }])
    expect(ids(s.tabs)).toEqual(['chat', 't1'])
    expect(s.activeGroupIndex).toBe(2)
  })

  it('no split past 3 columns', () => {
    seed([systemTab('chat', 'chat')], 'chat', [
      { tabs: [termTab('a')], activeTabId: 'a' },
      { tabs: [termTab('b')], activeTabId: 'b' },
    ])
    useTabsStore.getState().splitTerminalArea('/ws', 2)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(s.extraGroups.map((g) => ids(g.tabs))).toEqual([['a'], ['b']])
  })

  it('without fromGroup it splits from the focused column', () => {
    seed([termTab('t1')], 't1', [{ tabs: [termTab('a'), termTab('b')], activeTabId: 'b' }])
    useTabsStore.setState({ activeGroupIndex: 1 })
    useTabsStore.getState().splitTerminalArea('/ws')
    const s = useTabsStore.getState()
    expect(ids(s.tabs)).toEqual(['t1'])
    expect(ids(s.extraGroups[0].tabs)).toEqual(['a'])
    expect(ids(s.extraGroups[1].tabs)).toEqual(['b'])
  })
})

describe('every new column follows the 2nd column’s rule (Rosson 2026-11-09)', () => {
  // The reported path: Chat pinned in column 0, the 2nd column opened from a
  // regular tab, then the 3rd column opened with a fresh terminal.
  beforeEach(() => {
    seed([systemTab('chat', 'chat'), systemTab('inbox', 'inbox'), termTab('t1'), termTab('t2'), termTab('t3')], 't3')
    splitNoSpawn()
    const s = useTabsStore.getState()
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1', 't2'])
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t3'])
    expect(s.activeGroupIndex).toBe(1)
  })

  it('3rd column: the regular tab selected in the focused column moves there; that column shows Chat', () => {
    const before = useTabsStore.getState().tabs.find((t) => t.id === 't1')
    if (!before) throw new Error('seed lost t1')
    useTabsStore.getState().setActiveTabInGroup(0, 't1')
    useTabsStore.getState().setActiveGroup(0)
    splitNoSpawn()
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't2'])
    expect(s.activeTabId).toBe('chat')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t3'])
    expect(s.extraGroups[0].activeTabId).toBe('t3')
    expect(ids(s.extraGroups[1].tabs)).toEqual(['t1'])
    expect(s.extraGroups[1].activeTabId).toBe('t1')
    expect(s.extraGroups[1].tabs[0]).toBe(before)
    expect(s.activeGroupIndex).toBe(2)
  })

  it('3rd column: the same from the focused 2nd column when it has a tab to spare', () => {
    useTabsStore.getState().moveTabToGroup(0, 1, 't2')  // column 1 = [t3, t2], t2 selected
    splitNoSpawn()
    const s = useTabsStore.getState()
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t3'])
    expect(s.extraGroups[0].activeTabId).toBe('t3')
    expect(ids(s.extraGroups[1].tabs)).toEqual(['t2'])
    expect(s.activeGroupIndex).toBe(2)
  })

  it('3rd column with Chat selected in the focused column: an empty column, like the 2nd', () => {
    useTabsStore.getState().setActiveGroup(0)
    splitNoSpawn()
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't1', 't2'])
    expect(s.activeTabId).toBe('chat')
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t3'])
    expect(s.extraGroups[1]).toEqual({ tabs: [], activeTabId: null })
    expect(s.activeGroupIndex).toBe(2)
  })

  it('3rd column from the 2nd column’s only tab: it stays, the new column is empty', () => {
    splitNoSpawn()
    const s = useTabsStore.getState()
    expect(ids(s.extraGroups[0].tabs)).toEqual(['t3'])
    expect(s.extraGroups[1]).toEqual({ tabs: [], activeTabId: null })
    expect(s.activeGroupIndex).toBe(2)
  })

  it('3rd column from an empty 2nd column (opened on Chat): an empty column', () => {
    seed([systemTab('chat', 'chat'), termTab('t1')], 'chat')
    splitNoSpawn()
    expect(useTabsStore.getState().extraGroups).toEqual([{ tabs: [], activeTabId: null }])
    splitNoSpawn()
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(s.extraGroups).toEqual([{ tabs: [], activeTabId: null }, { tabs: [], activeTabId: null }])
    expect(ids(s.tabs)).toEqual(['chat', 't1'])
  })

  it('the 3-column layout round-trips through the remembered layout', () => {
    useTabsStore.getState().setActiveTabInGroup(0, 't1')
    useTabsStore.getState().setActiveGroup(0)
    splitNoSpawn()
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect(layout.splitCount).toBe(3)
    expect(layoutColumns(layout).map((c) => c.map((t) => t.id))).toEqual([['chat', 'inbox', 't2'], ['t3'], ['t1']])

    seed([], null)
    useTabsStore.getState().restoreLayout(JSON.parse(JSON.stringify(layout)) as SerializedLayout, '/ws')
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(ids(s.tabs)).toEqual(['chat', 'inbox', 't2'])
    expect(s.extraGroups.map((g) => ids(g.tabs))).toEqual([['t3'], ['t1']])
    // The moved tab keeps its pane group (its PTY re-attaches, none minted).
    expect([...s.extraGroups[1].tabs[0].paneGroups.keys()]).toEqual(['pg-t1'])
    expect(JSON.stringify(s.serializeCurrentLayout())).toBe(JSON.stringify(layout))
  })
})

describe('an empty split column persists', () => {
  beforeEach(() => seed([systemTab('chat', 'chat'), termTab('t1')], 'chat'))

  it('survives serialize → restore (not dropped, not filled)', () => {
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect(layout.splitCount).toBe(2)
    expect(layout.extraGroups).toEqual([{ tabs: [] }])

    // A cold load: nothing live, the saved JSON comes back.
    seed([], null)
    useTabsStore.getState().restoreLayout(JSON.parse(JSON.stringify(layout)) as SerializedLayout, '/ws')
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(2)
    expect(s.extraGroups).toEqual([{ tabs: [], activeTabId: null }])
    expect(ids(s.tabs)).toEqual(['chat', 't1'])
  })

  it('survives the 409 merge against another window’s newer layout', () => {
    const base = useTabsStore.getState().serializeCurrentLayout()
    useTabsStore.getState().splitTerminalArea('/ws', 0)
    const local = useTabsStore.getState().serializeCurrentLayout()
    // Another window opened a tab in column 0 meanwhile.
    const remote: SerializedLayout = JSON.parse(JSON.stringify(base))
    remote.tabs.push({
      id: 'theirs',
      title: 'theirs',
      mosaicTree: 'pg-theirs',
      paneGroups: { 'pg-theirs': { id: 'pg-theirs', items: [], activeItemIndex: 0 } },
    })
    const merged = mergeSerializedLayouts(base, local, remote)
    const cols = layoutColumns(merged)
    expect(cols).toHaveLength(2)
    expect(cols[0].map((t) => t.id)).toEqual(['chat', 't1', 'theirs'])
    expect(cols[1]).toEqual([])
    expect(merged.splitCount).toBe(2)
  })
})
