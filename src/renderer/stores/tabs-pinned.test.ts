// Pinned tabs (Rosson 2026-10-16): any tab pins from the tab itself and sits
// right after the system Chat / Inbox tabs, in pin order. Store side: pin /
// unpin, ordering, drags inside the pinned group, columns, persistence.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { useTabsStore, type Item, type PaneGroup, type Tab } from './tabs'
import { __resetNamedChatTitleCachesForTests } from '@/lib/chat-session-tab'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'k2so_agents_list') return []
    if (cmd === 'k2so_workspace_agent_display_name') return 'resolved-agent'
    return null
  }),
}))

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

function fileTab(id: string, filePath = `/tmp/proj/${id}.md`): Tab {
  const item: Item = { id: `${id}-item`, type: 'file-viewer', data: { filePath }, pinned: false }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: id, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
}

function browserTab(id: string): Tab {
  const item: Item = { id: `${id}-item`, type: 'browser', data: { url: 'https://example.com' } }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: id, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
}

const titles = (tabs: Tab[]): string[] => tabs.map((t) => t.title)
const mainTitles = (): string[] => titles(useTabsStore.getState().tabs)

/** Chat + Inbox, then three regular tabs: a terminal, a Markdown file, a
 *  browser. */
function seed(): void {
  useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')
  useTabsStore.getState().addTab('/tmp/proj', { title: 'Terminal 1' })
  useTabsStore.setState((s) => ({ tabs: [...s.tabs, fileTab('notes'), browserTab('web')] }))
}

function idOf(title: string): string {
  const tab = useTabsStore.getState().tabs.find((t) => t.title === title)
  if (!tab) throw new Error(`no tab ${title}`)
  return tab.id
}

describe('pinTab / unpinTab', () => {
  beforeEach(reset)

  it('pins any tab kind to the end of the pinned group, after Chat and Inbox', () => {
    seed()
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'Terminal 1', 'notes', 'web'])

    useTabsStore.getState().pinTab(idOf('web'))
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'Terminal 1', 'notes'])

    useTabsStore.getState().pinTab(idOf('Terminal 1'))
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'Terminal 1', 'notes'])
    expect(useTabsStore.getState().tabs.map((t) => !!t.isPinnedFile)).toEqual([false, false, true, true, false])

    useTabsStore.getState().pinTab(idOf('notes'))
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'Terminal 1', 'notes'])
    expect(useTabsStore.getState().tabs.slice(2).every((t) => t.isPinnedFile === true)).toBe(true)
  })

  it('pinning a file tab keeps its file viewer so a later open does not recycle it', () => {
    seed()
    useTabsStore.getState().pinTab(idOf('notes'))
    const tab = useTabsStore.getState().tabs.find((t) => t.title === 'notes')!
    const item = Array.from(tab.paneGroups.values())[0].items[0]
    expect(item.type).toBe('file-viewer')
    expect(item.pinned).toBe(true)
  })

  it('unpins to the first slot after the pinned group', () => {
    seed()
    useTabsStore.getState().pinTab(idOf('notes'))
    useTabsStore.getState().pinTab(idOf('web'))
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'notes', 'web', 'Terminal 1'])

    useTabsStore.getState().unpinTab(idOf('notes'))
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'notes', 'Terminal 1'])
    const notes = useTabsStore.getState().tabs.find((t) => t.title === 'notes')!
    expect(notes.isPinnedFile).toBeUndefined()
  })

  it('togglePinTab flips the pin', () => {
    seed()
    const id = idOf('Terminal 1')
    useTabsStore.getState().togglePinTab(id)
    expect(useTabsStore.getState().tabs.find((t) => t.id === id)!.isPinnedFile).toBe(true)
    useTabsStore.getState().togglePinTab(id)
    expect(useTabsStore.getState().tabs.find((t) => t.id === id)!.isPinnedFile).toBeUndefined()
  })

  it('never pins or moves the system Chat / Inbox tabs', () => {
    seed()
    const before = useTabsStore.getState().tabs
    useTabsStore.getState().pinTab(idOf('Chat'))
    useTabsStore.getState().togglePinTab(idOf('Inbox'))
    expect(useTabsStore.getState().tabs).toBe(before)
    expect(useTabsStore.getState().tabs.filter((t) => t.isSystemAgent).every((t) => !t.isPinnedFile)).toBe(true)
  })

  it('pins inside a split column, at that column\'s left', () => {
    useTabsStore.setState({
      tabs: [fileTab('a')],
      activeTabId: 'a',
      splitCount: 2,
      extraGroups: [{ tabs: [fileTab('b'), fileTab('c'), fileTab('d')], activeTabId: 'b' }],
    })
    useTabsStore.getState().pinTab('d')
    const col = useTabsStore.getState().extraGroups[0].tabs
    expect(col.map((t) => t.id)).toEqual(['d', 'b', 'c'])
    expect(col[0].isPinnedFile).toBe(true)
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(['a'])
  })

  it('a re-run ensureSystemAgentTabs keeps pinned tabs right after the system tabs', () => {
    seed()
    useTabsStore.getState().pinTab(idOf('web'))
    useTabsStore.getState().ensureSystemAgentTabs('manager', '/tmp/proj', 'Manager')
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'Terminal 1', 'notes'])
  })
})

describe('reorderTabs with pinned tabs', () => {
  beforeEach(() => {
    reset()
    seed()
    useTabsStore.setState((s) => ({ tabs: [...s.tabs, fileTab('more')] }))
    useTabsStore.getState().pinTab(idOf('notes'))
    useTabsStore.getState().pinTab(idOf('web'))
    // [Chat][Inbox][notes][web][Terminal 1][more]
  })

  it('reorders inside the pinned group', () => {
    useTabsStore.getState().reorderTabs(3, 2)
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'notes', 'Terminal 1', 'more'])
  })

  it('a pinned tab dragged into the regular tabs stays at the end of the pinned group', () => {
    useTabsStore.getState().reorderTabs(2, 5)
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'notes', 'Terminal 1', 'more'])
    expect(useTabsStore.getState().tabs[3].isPinnedFile).toBe(true)
  })

  it('a regular tab dragged into the pinned group stays the first regular tab', () => {
    useTabsStore.getState().reorderTabs(5, 2)
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'notes', 'web', 'more', 'Terminal 1'])
    expect(useTabsStore.getState().tabs[4].isPinnedFile).toBeUndefined()
  })

  it('system tabs never move and nothing moves in front of them', () => {
    useTabsStore.getState().reorderTabs(0, 4)
    useTabsStore.getState().reorderTabs(4, 0)
    expect(mainTitles().slice(0, 2)).toEqual(['Chat', 'Inbox'])
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'notes', 'web', 'Terminal 1', 'more'])
  })

  it('applies the same rule in a split column', () => {
    const p = { ...fileTab('p'), isPinnedFile: true }
    useTabsStore.setState({
      splitCount: 2,
      extraGroups: [{ tabs: [p, fileTab('x'), fileTab('y')], activeTabId: 'x' }],
    })
    useTabsStore.getState().reorderTabs(2, 0, 1)
    expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.id)).toEqual(['p', 'y', 'x'])
  })
})

describe('moving a pinned tab to another column', () => {
  beforeEach(reset)

  it('keeps it pinned, at the end of the target column\'s pinned group', () => {
    const a = { ...fileTab('a'), isPinnedFile: true }
    const q = { ...fileTab('q'), isPinnedFile: true }
    useTabsStore.setState({
      tabs: [a, fileTab('b')],
      activeTabId: 'a',
      splitCount: 2,
      extraGroups: [{ tabs: [q, fileTab('r')], activeTabId: 'r' }],
    })
    useTabsStore.getState().moveTabToGroup(0, 1, 'a')
    const col = useTabsStore.getState().extraGroups[0].tabs
    expect(col.map((t) => t.id)).toEqual(['q', 'a', 'r'])
    expect(col[1].isPinnedFile).toBe(true)
  })
})

describe('pinned tabs persist in the shared layout', () => {
  beforeEach(reset)

  it('survive a serialize → restore round-trip in pin order', () => {
    seed()
    useTabsStore.getState().pinTab(idOf('web'))
    useTabsStore.getState().pinTab(idOf('Terminal 1'))
    const layout = useTabsStore.getState().serializeCurrentLayout()
    expect(layout.tabs.map((t) => [t.title, t.isPinnedFile === true])).toEqual([
      ['Chat', false],
      ['Inbox', false],
      ['web', true],
      ['Terminal 1', true],
      ['notes', false],
    ])

    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')
    const tabs = useTabsStore.getState().tabs
    expect(titles(tabs)).toEqual(['Chat', 'Inbox', 'web', 'Terminal 1', 'notes'])
    expect(tabs.map((t) => t.isPinnedFile === true)).toEqual([false, false, true, true, false])
  })

  it('restore puts pinned tabs right after the system tabs whatever the saved order', () => {
    seed()
    useTabsStore.getState().pinTab(idOf('web'))
    const layout = useTabsStore.getState().serializeCurrentLayout()
    // An older client saved the pinned tab at the end of the strip.
    const web = layout.tabs.find((t) => t.title === 'web')!
    layout.tabs = [...layout.tabs.filter((t) => t !== web), web]
    reset()
    useTabsStore.getState().restoreLayout(layout, '/tmp/proj')
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'web', 'Terminal 1', 'notes'])
  })
})

describe('closing a pinned tab', () => {
  beforeEach(reset)

  it('removeTab closes it like any other tab', () => {
    seed()
    useTabsStore.getState().pinTab(idOf('notes'))
    useTabsStore.getState().removeTab(idOf('notes'))
    expect(mainTitles()).toEqual(['Chat', 'Inbox', 'Terminal 1', 'web'])
  })
})
