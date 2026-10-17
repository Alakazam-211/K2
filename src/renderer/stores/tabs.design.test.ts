// Design tab (prd-browser-design-mode-v1 DD1/DD3, D-A2–D-A6, Q-F2/Q-F3) in
// the tabs store: the `'design'` item type, the pinned Design tab next to
// Chat and Inbox gated on the workspace's Design mode, the + menu's new
// Design tab, and the mixed-version serialization (a Browser item plus a
// `design` marker).

import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'k2so_agents_list') return [{ name: 'docs-agent', agentType: 'custom' }]
    if (cmd === 'daemon_ws_url') return { state: 'unavailable', reason: 'test env', port: null, token: null }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  pinnedTabOf,
  registerProjectsPathIndex,
  useTabsStore,
  withPinnedDesignTab,
  type DesignItemData,
  type ProjectPathEntry,
  type SerializedLayout,
  type Tab,
} from './tabs'

let projectIndex: ProjectPathEntry[] = []
registerProjectsPathIndex(() => projectIndex)

function reset(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    activeWorkspaceKey: null,
  })
}

function firstItem(tab: Tab): { id: string; type: string; data: unknown } {
  const item = Array.from(tab.paneGroups.values())[0]?.items[0]
  if (!item) throw new Error(`tab ${tab.id} has no item`)
  return item
}

/** `chat` / `inbox` / `design` / `pinned-design` / other item type, per tab. */
function strip(tabs: Tab[] = useTabsStore.getState().tabs): string[] {
  return tabs.map((t) => {
    const item = firstItem(t)
    if (t.isSystemAgent) return (item.data as { section?: string }).section ?? 'system'
    if (t.isPinnedDesign) return `pinned-${item.type}`
    return item.type
  })
}

function project(path: string, designMode?: boolean): ProjectPathEntry {
  return { id: `id-${path}`, path, primaryWorkspaceId: null, ...(designMode === undefined ? {} : { designMode }) }
}

async function ensureSystemTabs(path: string): Promise<void> {
  await useTabsStore.room.ensurePinnedAgentTabForMode('custom', path)
}

describe('Design tab item and the + menu entry', () => {
  beforeEach(() => {
    reset()
    projectIndex = []
  })

  it('openDesignTab always adds a NEW Design tab (many allowed) and activates it', () => {
    const a = useTabsStore.getState().openDesignTab()
    const b = useTabsStore.getState().openDesignTab(0, 'http://localhost:5173/')
    const s = useTabsStore.getState()
    expect(a).not.toBe(b)
    expect(strip()).toEqual(['design', 'design'])
    expect(s.tabs.map((t) => t.title)).toEqual(['Design', 'Design'])
    expect(s.activeTabId).toBe(b)
    expect(firstItem(s.tabs[1]).data).toEqual({ url: 'http://localhost:5173/' })
    expect(s.tabs.some((t) => t.isPinnedDesign)).toBe(false)
  })

  it('opens in the given column', () => {
    useTabsStore.setState({ splitCount: 2, extraGroups: [{ tabs: [], activeTabId: null }] })
    const id = useTabsStore.getState().openDesignTab(1)
    const s = useTabsStore.getState()
    expect(s.tabs).toHaveLength(0)
    expect(s.extraGroups[0].tabs.map((t) => t.id)).toEqual([id])
    expect(s.extraGroups[0].activeTabId).toBe(id)
  })

  it('page URL / title / favicon stamps reach a Design item; the strip title stays "Design"', () => {
    const tabId = useTabsStore.getState().openDesignTab()
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    const itemId = pg.items[0].id
    useTabsStore.getState().setBrowserItemState(tabId, pg.id, itemId, { url: 'https://example.com/a' })
    useTabsStore.getState().applyBrowserPageMeta(tabId, pg.id, itemId, { title: 'Pricing', icon: 'data:image/png;base64,AA==' })
    const after = useTabsStore.getState().tabs[0]
    expect(firstItem(after).data).toEqual({ url: 'https://example.com/a', title: 'Pricing', icon: 'data:image/png;base64,AA==' })
    expect(after.title).toBe('Design')
  })
})

describe('serialization (D-A4 mixed-version guard, Q-F3)', () => {
  beforeEach(reset)

  it('saves as a Browser item plus a design marker and round-trips as a Design item', () => {
    useTabsStore.getState().openDesignTab(0, 'http://localhost:5173/pricing')
    const tab = useTabsStore.getState().tabs[0]
    const pg = Array.from(tab.paneGroups.values())[0]
    const itemId = pg.items[0].id
    useTabsStore.getState().applyBrowserPageMeta(tab.id, pg.id, itemId, { title: 'Pricing' })

    const layout = useTabsStore.getState().serializeCurrentLayout()
    const fromDisk = JSON.parse(JSON.stringify(layout)) as SerializedLayout
    const saved = Object.values(fromDisk.tabs[0].paneGroups)[0].items[0]
    expect(saved).toEqual({
      id: itemId,
      type: 'browser',
      url: 'http://localhost:5173/pricing',
      title: 'Pricing',
      design: { v: 1 },
    })
    expect(fromDisk.version).toBe(layout.version)

    reset()
    useTabsStore.getState().restoreLayout(fromDisk, '/w/docs')
    const restored = firstItem(useTabsStore.getState().tabs[0])
    expect(restored.id).toBe(itemId)
    expect(restored.type).toBe('design')
    expect(restored.data as DesignItemData).toEqual({ url: 'http://localhost:5173/pricing', title: 'Pricing' })
  })

  it('an older client re-save (no design field, no isPinnedDesign) restores as a plain Browser tab', () => {
    useTabsStore.setState({ tabs: withPinnedDesignTab([], true, true) })
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    expect(layout.tabs[0].isPinnedDesign).toBe(true)
    // What an older client writes back: it knows neither field.
    const old = layout.tabs[0]
    delete old.isPinnedDesign
    for (const pg of Object.values(old.paneGroups)) {
      for (const item of pg.items) delete (item as { design?: unknown }).design
    }

    reset()
    useTabsStore.getState().restoreLayout(layout, '/w/docs')
    expect(strip()).toEqual(['browser'])
    expect(useTabsStore.getState().tabs[0].isPinnedDesign).toBeUndefined()
  })

  it('the pinned flag round-trips', () => {
    useTabsStore.setState({ tabs: withPinnedDesignTab([], true, true) })
    const layout = JSON.parse(JSON.stringify(useTabsStore.getState().serializeCurrentLayout())) as SerializedLayout
    reset()
    useTabsStore.getState().restoreLayout(layout, '/w/docs')
    expect(strip()).toEqual(['pinned-design'])
  })
})

describe('the pinned Design tab', () => {
  beforeEach(() => {
    reset()
    projectIndex = []
  })

  it('a workspace restore puts it after Chat and Inbox when Design mode is on (missing = on)', async () => {
    projectIndex = [project('/w/docs')]
    useTabsStore.getState().addTab('/w/docs', { title: 'T' })
    await ensureSystemTabs('/w/docs')
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design', 'terminal'])
    // Idempotent: a second ensure adds nothing and changes nothing.
    const before = useTabsStore.getState().tabs
    useTabsStore.getState().ensurePinnedDesignTab('/w/docs')
    expect(useTabsStore.getState().tabs).toBe(before)
    await ensureSystemTabs('/w/docs')
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design', 'terminal'])
  })

  it('sits before the user-pinned tabs', async () => {
    projectIndex = [project('/w/docs')]
    useTabsStore.getState().addTab('/w/docs', { title: 'T' })
    useTabsStore.getState().pinTab(useTabsStore.getState().tabs[0].id)
    await ensureSystemTabs('/w/docs')
    const s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design', 'terminal'])
    expect(s.tabs[3].isPinnedFile).toBe(true)
  })

  it('Design mode off: no pinned Design tab; turning it off removes it and lands on Chat', async () => {
    projectIndex = [project('/w/docs', false)]
    await ensureSystemTabs('/w/docs')
    expect(strip()).toEqual(['chat', 'inbox'])

    useTabsStore.getState().ensurePinnedDesignTab('/w/docs', true)
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design'])
    const designId = useTabsStore.getState().tabs[2].id
    useTabsStore.getState().openDesignTab()
    useTabsStore.getState().setActiveTabInGroup(0, designId)

    useTabsStore.getState().ensurePinnedDesignTab('/w/docs', false)
    const s = useTabsStore.getState()
    // The + menu Design tab is not the pinned one: it stays.
    expect(strip()).toEqual(['chat', 'inbox', 'design'])
    expect(s.activeTabId).toBe(s.tabs[0].id)
  })

  it('a workspace the room does not know is left alone (no guess)', () => {
    useTabsStore.getState().ensurePinnedDesignTab('/w/unknown')
    expect(useTabsStore.getState().tabs).toHaveLength(0)
  })

  it('never in a Work Board room', () => {
    projectIndex = [project('/w/board')]
    useTabsStore.getState().ensureSystemAgentTabs('__workspace__', '/w/board', 'Workspace')
    // ensurePinnedAgentTabForMode resolves a real agent name; a board room
    // only ever gets its Inbox through ensureSystemAgentTabs directly.
    expect(strip()).toEqual(['inbox'])
  })

  it('cannot be closed, pinned or unpinned by the user, and pinnedTabOf never returns it', async () => {
    projectIndex = [project('/w/docs')]
    await ensureSystemTabs('/w/docs')
    const design = useTabsStore.getState().tabs[2]
    expect(design.isPinnedDesign).toBe(true)
    useTabsStore.getState().removeTab(design.id)
    useTabsStore.getState().togglePinTab(design.id)
    const s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design'])
    expect(s.tabs[2].isPinnedFile).toBeUndefined()
    const chat = pinnedTabOf(s.tabs)
    expect(chat && firstItem(chat).type).toBe('agent')
    expect(pinnedTabOf([design])).toBeNull()
  })

  it('a second pinned Design tab (two windows met) loses the flag and stays as a Design tab', () => {
    const one = withPinnedDesignTab([], true, true)
    const two = withPinnedDesignTab([], true, true)
    const merged = withPinnedDesignTab([...one, ...two], true, true)
    expect(strip(merged)).toEqual(['pinned-design', 'design'])
    expect(merged[0].id).toBe(one[0].id)
  })

  it('selecting a Design tab changes nothing shared (DD5): no split, same saved layout', async () => {
    projectIndex = [project('/w/docs')]
    await ensureSystemTabs('/w/docs')
    const before = JSON.stringify(useTabsStore.getState().serializeCurrentLayout())
    const { splitCount, extraGroups } = useTabsStore.getState()
    useTabsStore.getState().setActiveTabInGroup(0, useTabsStore.getState().tabs[2].id)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(splitCount)
    expect(s.extraGroups).toBe(extraGroups)
    expect(JSON.stringify(s.serializeCurrentLayout())).toBe(before)
  })

  it('splitting from the pinned Design tab leaves it in its column (the new column starts empty)', async () => {
    projectIndex = [project('/w/docs')]
    await ensureSystemTabs('/w/docs')
    useTabsStore.getState().setActiveTabInGroup(0, useTabsStore.getState().tabs[2].id)
    useTabsStore.getState().splitTerminalArea('/w/docs', 0)
    const s = useTabsStore.getState()
    expect(strip()).toEqual(['chat', 'inbox', 'pinned-design'])
    expect(s.extraGroups[0].tabs).toEqual([])
  })
})
