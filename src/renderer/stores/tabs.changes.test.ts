// Changes tab C2 — the `'changes'` item type in the tabs store: open (one
// per workspace), view-state stamps, serialize → restore.

import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { findChangesTab, useTabsStore, type ChangesItemData, type SerializedLayout } from './tabs'

function reset(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
  })
}

function changesItem(tabIndex: number, group = 0): { id: string; type: string; data: ChangesItemData } {
  const s = useTabsStore.getState()
  const tabs = group === 0 ? s.tabs : s.extraGroups[group - 1].tabs
  const tab = tabs[tabIndex]
  if (!tab) throw new Error(`no tab ${tabIndex} in column ${group}`)
  const item = Array.from(tab.paneGroups.values())[0].items[0]
  if (item.type !== 'changes') throw new Error(`tab ${tabIndex} holds a ${item.type}, not changes`)
  return item as { id: string; type: string; data: ChangesItemData }
}

describe('Changes tab item', () => {
  beforeEach(reset)

  it('openChangesTab adds a Changes tab with one changes item and activates it', () => {
    const id = useTabsStore.getState().openChangesTab('/repo/ws/')
    const s = useTabsStore.getState()
    expect(s.tabs).toHaveLength(1)
    expect(s.tabs[0].id).toBe(id)
    expect(s.tabs[0].title).toBe('Changes')
    expect(s.activeTabId).toBe(id)
    // Trailing slash trimmed: the routes' `path` and the event's workspacePath.
    expect(changesItem(0).data).toEqual({ workspacePath: '/repo/ws' })
  })

  it('one per workspace: a second open focuses the existing tab, even from another column', () => {
    useTabsStore.setState({ splitCount: 2, extraGroups: [{ tabs: [], activeTabId: null }] })
    const first = useTabsStore.getState().openChangesTab('/repo/ws', 1)
    useTabsStore.getState().addTab('/repo/ws', { title: 'T' })
    expect(useTabsStore.getState().activeTabId).not.toBe(first)

    const again = useTabsStore.getState().openChangesTab('/repo/ws/', 0)
    expect(again).toBe(first)
    const s = useTabsStore.getState()
    expect(s.tabs.filter((t) => t.title === 'Changes')).toHaveLength(0)
    expect(s.extraGroups[0].tabs).toHaveLength(1)
    expect(s.extraGroups[0].activeTabId).toBe(first)
    expect(s.activeGroupIndex).toBe(1)
    expect(findChangesTab(s, '/repo/ws')).toEqual({ groupIndex: 1, tabId: first })

    // Another workspace gets its own.
    const other = useTabsStore.getState().openChangesTab('/repo/other')
    expect(other).not.toBe(first)
    expect(findChangesTab(useTabsStore.getState(), '/repo/other')).toEqual({ groupIndex: 0, tabId: other })
  })

  it('setChangesItemState stamps view state; a same-value stamp changes nothing', () => {
    useTabsStore.getState().openChangesTab('/repo/ws')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    const itemId = pg.items[0].id
    const set = useTabsStore.getState().setChangesItemState

    set(tab.id, pg.id, itemId, { selectedPath: 'src/a.rs', selectedGroup: 'unstaged', mode: 'split' })
    expect(changesItem(0).data).toMatchObject({ selectedPath: 'src/a.rs', selectedGroup: 'unstaged', mode: 'split' })

    const before = useTabsStore.getState().tabs
    set(tab.id, pg.id, itemId, { selectedPath: 'src/a.rs', mode: 'split' })
    expect(useTabsStore.getState().tabs).toBe(before)

    // workspacePath is not stampable.
    set(tab.id, pg.id, itemId, { workspacePath: '/elsewhere' } as never)
    expect(useTabsStore.getState().tabs).toBe(before)
    expect(changesItem(0).data.workspacePath).toBe('/repo/ws')

    set(tab.id, pg.id, itemId, { base: 'main', view: 'all', listLayout: 'tree' })
    expect(useTabsStore.getState().tabs).not.toBe(before)
    expect(changesItem(0).data).toMatchObject({ base: 'main', view: 'all', listLayout: 'tree' })
  })

  it('serialize → restore keeps the item id and its view state', () => {
    useTabsStore.getState().openChangesTab('/repo/ws')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    const itemId = pg.items[0].id
    useTabsStore.getState().setChangesItemState(tab.id, pg.id, itemId, {
      selectedPath: 'src/a.rs',
      selectedGroup: 'staged',
      mode: 'split',
      base: 'main',
      view: 'all',
      listLayout: 'tree',
    })

    const layout = useTabsStore.getState().serializeCurrentLayout()
    const serialized = Object.values(layout.tabs[0].paneGroups)[0].items[0]
    expect(serialized).toEqual({
      id: itemId,
      type: 'changes',
      workspacePath: '/repo/ws',
      selectedPath: 'src/a.rs',
      selectedGroup: 'staged',
      mode: 'split',
      base: 'main',
      view: 'all',
      listLayout: 'tree',
    })
    // JSON round trip, as the daemon stores it.
    const fromDisk = JSON.parse(JSON.stringify(layout)) as SerializedLayout

    reset()
    useTabsStore.getState().restoreLayout(fromDisk, '/repo/ws')
    const restored = changesItem(0)
    expect(restored.id).toBe(itemId)
    expect(restored.data).toEqual({
      workspacePath: '/repo/ws',
      selectedPath: 'src/a.rs',
      selectedGroup: 'staged',
      mode: 'split',
      base: 'main',
      view: 'all',
      listLayout: 'tree',
    })
    expect(useTabsStore.getState().tabs[0].title).toBe('Changes')
  })

  it('a default item serializes with no optional fields', () => {
    useTabsStore.getState().openChangesTab('/repo/ws')
    const layout = useTabsStore.getState().serializeCurrentLayout()
    const item = Object.values(layout.tabs[0].paneGroups)[0].items[0]
    expect(Object.keys(item).sort()).toEqual(['id', 'type', 'workspacePath'])
  })

  it('restore drops unknown enum values and falls back to the cwd for a missing path', () => {
    const layout = {
      version: 2,
      tabs: [
        {
          id: 't1',
          title: 'Changes',
          mosaicTree: 'pg1',
          paneGroups: {
            pg1: {
              id: 'pg1',
              activeItemIndex: 0,
              items: [
                {
                  id: 'i1',
                  type: 'changes',
                  selectedPath: 'a.txt',
                  selectedGroup: 'weird',
                  mode: 'sideways',
                  view: 'every',
                  listLayout: 'grid',
                  base: '',
                },
              ],
            },
          },
        },
      ],
    } as unknown as SerializedLayout
    useTabsStore.getState().restoreLayout(layout, '/repo/cwd')
    expect(changesItem(0).data).toEqual({ workspacePath: '/repo/cwd', selectedPath: 'a.txt' })
  })
})
