// @vitest-environment jsdom
// Design tab in the tab strip (prd-browser-design-mode-v1 DD2/DD3, D-A1,
// D-A5, Q-F2): the + menu offers "Design" right after Browser only while
// the workspace's Design mode is on, picking it opens a NEW Design tab
// every time, the pinned Design tab is a compact palette rail next to Chat
// and Inbox, and flipping the setting adds or removes it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
      if (route === 'git/info') return { isRepo: false, currentBranch: 'main', ahead: 0, behind: 0, changedFiles: 0, untrackedFiles: 0 }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async () => ({})),
  }
})

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { TabBar } from '@/components/TabBar/TabBar'
import { useContextMenuStore } from '@/stores/context-menu'
import { __resetGitChangesForTests } from '@/stores/git-changes'
import { usePresetsStore } from '@/stores/presets'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useTabsStore, withPinnedDesignTab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const realFetch = usePresetsStore.getState().fetchPresets

function setProject(designModeEnabled?: number): void {
  const p = {
    id: 'p-docs',
    path: '/w/docs',
    workspaces: [],
    ...(designModeEnabled === undefined ? {} : { designModeEnabled }),
  } as unknown as ProjectWithWorkspaces
  useProjectsStore.setState({ projects: [p] })
}

function resetTabs(): void {
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

async function openPlus(): Promise<void> {
  const add = document.querySelector('[data-tab-add]') as HTMLButtonElement | null
  if (!add) throw new Error('no + button')
  await act(async () => {
    fireEvent.click(add)
  })
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  })
}

function menuIds(): string[] {
  return useContextMenuStore.getState().items.map((i) => i.id)
}

function renderBar(): void {
  renderInPrimaryRoom(
    <>
      <TabBar cwd="/w/docs" groupIndex={0} />
      <ContextMenu />
    </>,
  )
}

describe('+ menu Design row', () => {
  beforeEach(() => {
    __resetGitChangesForTests()
    resetTabs()
    usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
    useContextMenuStore.getState().close()
  })

  afterEach(() => {
    cleanup()
    useContextMenuStore.getState().close()
    resetTabs()
    useProjectsStore.setState({ projects: [] })
    usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  })

  it('Design mode on (or missing): Design right after Browser, with the palette', async () => {
    setProject()
    renderBar()
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser', 'design', 'mobile', 'changes'])
    const row = useContextMenuStore.getState().items.find((i) => i.id === 'design')
    if (!row) throw new Error('no Design row')
    expect(row.label).toBe('Design')
  })

  it('Design mode off: no Design row', async () => {
    setProject(0)
    renderBar()
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser', 'mobile', 'changes'])
  })

  it('a workspace the room does not know gets no Design row', async () => {
    useProjectsStore.setState({ projects: [] })
    renderBar()
    await openPlus()
    expect(menuIds()).not.toContain('design')
  })

  it('picking Design opens a NEW Design tab every time', async () => {
    setProject(1)
    renderBar()
    for (let i = 0; i < 2; i++) {
      await openPlus()
      await act(async () => {
        useContextMenuStore.getState().selectItem('design')
        for (let k = 0; k < 5; k++) await Promise.resolve()
      })
    }
    const tabs = useTabsStore.getState().tabs
    expect(tabs).toHaveLength(2)
    for (const t of tabs) {
      expect(t.title).toBe('Design')
      expect(Array.from(t.paneGroups.values())[0].items[0].type).toBe('design')
      expect(t.isPinnedDesign).toBeUndefined()
    }
    expect(tabs[0].id).not.toBe(tabs[1].id)
  })
})

describe('pinned Design rail and the Design mode switch', () => {
  beforeEach(() => {
    resetTabs()
    usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
  })

  afterEach(() => {
    cleanup()
    resetTabs()
    useProjectsStore.setState({ projects: [] })
    usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  })

  it('draws the pinned Design tab as a compact rail with the palette, no close or pin', () => {
    setProject(1)
    useTabsStore.setState({ tabs: withPinnedDesignTab([], true, true) })
    renderBar()
    const rail = document.querySelector('[data-tab-pinned-design]')
    if (!rail) throw new Error('no pinned Design rail')
    expect(rail.hasAttribute('data-tab-rail')).toBe(true)
    expect(rail.querySelector('[data-k2-icon="palette"]')).toBeTruthy()
    expect(rail.querySelector('[data-tab-close-slot]')).toBeNull()
    act(() => {
      fireEvent.click(rail)
    })
    expect(useTabsStore.getState().activeTabId).toBe(useTabsStore.getState().tabs[0].id)
  })

  it('a + menu Design tab shows the palette in the strip', () => {
    setProject(1)
    useTabsStore.getState().openDesignTab()
    renderBar()
    const tab = document.querySelector(`[data-tab-id="${useTabsStore.getState().tabs[0].id}"]`)
    if (!tab) throw new Error('no Design tab')
    expect(tab.querySelector('[data-k2-icon="palette"]')).toBeTruthy()
    expect(tab.hasAttribute('data-tab-pinned-design')).toBe(false)
  })

  it('turning Design mode off removes the pinned Design tab; on brings it back', () => {
    setProject(1)
    useTabsStore.setState({ tabs: withPinnedDesignTab([], true, true) })
    renderBar()
    expect(document.querySelector('[data-tab-pinned-design]')).toBeTruthy()

    act(() => setProject(0))
    expect(useTabsStore.getState().tabs.some((t) => t.isPinnedDesign)).toBe(false)
    expect(document.querySelector('[data-tab-pinned-design]')).toBeNull()

    act(() => setProject(1))
    expect(useTabsStore.getState().tabs.filter((t) => t.isPinnedDesign)).toHaveLength(1)
    expect(document.querySelector('[data-tab-pinned-design]')).toBeTruthy()
  })
})
