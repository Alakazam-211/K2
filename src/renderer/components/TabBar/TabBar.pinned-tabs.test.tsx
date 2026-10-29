// @vitest-environment jsdom
// Right-click on the pinned Design or Mobile tab (Rosson 2026-10-29), fail
// loud: K2's context menu offers "Remove from this workspace's tabs"; picking
// it removes the tab here at once, writes that workspace's switch off on the
// room's server (`/cli/workspace/set`, so every client follows), mirrors it
// into the projects list, and shows "Bring it back in Workspace settings →
// Tabs" with an Open link to those rows. A failed write puts the tab back.
// The rails draw the palette / phone, with no close button.

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
    daemonCliPost: vi.fn(async () => ({ ok: true })),
  }
})

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { TabBar } from '@/components/TabBar/TabBar'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useContextMenuStore } from '@/stores/context-menu'
import { __resetGitChangesForTests } from '@/stores/git-changes'
import { usePresetsStore } from '@/stores/presets'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore, withPinnedDesignTab, withPinnedMobileTab } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const realFetch = usePresetsStore.getState().fetchPresets
const realOpenSettings = useSettingsStore.getState().openSettings

function setProject(fields: { designModeEnabled?: number; mobileTabEnabled?: number | null } = {}): void {
  const p = { id: 'p-app', path: '/w/app', workspaces: [], ...fields } as unknown as ProjectWithWorkspaces
  useProjectsStore.setState({ projects: [p] })
}

function resetTabs(): void {
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

function renderBar(): void {
  renderInPrimaryRoom(
    <>
      <TabBar cwd="/w/app" groupIndex={0} />
      <ContextMenu />
    </>,
  )
}

function rail(kind: 'design' | 'mobile'): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-tab-pinned-${kind}]`)
  if (!el) throw new Error(`no pinned ${kind} rail`)
  return el
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 8; i++) await Promise.resolve()
  })
}

async function rightClickAndRemove(kind: 'design' | 'mobile'): Promise<void> {
  await act(async () => {
    fireEvent.contextMenu(rail(kind))
  })
  await flush()
  const items = useContextMenuStore.getState().items
  expect(items.map((i) => [i.id, i.label])).toEqual([['remove-pinned-tab', "Remove from this workspace's tabs"]])
  await act(async () => {
    useContextMenuStore.getState().selectItem('remove-pinned-tab')
  })
  await flush()
}

beforeEach(() => {
  __resetGitChangesForTests()
  resetTabs()
  usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
  useContextMenuStore.getState().close()
  useToastStore.setState({ toasts: [] })
  vi.mocked(daemonCliPost).mockReset()
  vi.mocked(daemonCliPost).mockImplementation(async () => ({ ok: true }))
})

afterEach(() => {
  cleanup()
  useContextMenuStore.getState().close()
  resetTabs()
  useProjectsStore.setState({ projects: [] })
  usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  useSettingsStore.setState({ openSettings: realOpenSettings })
  useToastStore.setState({ toasts: [] })
})

describe('the pinned Mobile rail', () => {
  it('draws the phone, no close slot, after the Design rail', () => {
    setProject()
    useTabsStore.setState({ tabs: withPinnedMobileTab(withPinnedDesignTab([], true, true), true, true) })
    renderBar()
    const mobile = rail('mobile')
    expect(mobile.hasAttribute('data-tab-rail')).toBe(true)
    expect(mobile.querySelector('[data-k2-icon="smartphone"]')).toBeTruthy()
    expect(mobile.querySelector('[data-tab-close-slot]')).toBeNull()
    expect(mobile.hasAttribute('data-tab-pinned-design')).toBe(false)
    const ids = Array.from(document.querySelectorAll('[data-tab-id]')).map((el) => el.getAttribute('data-tab-id'))
    const tabs = useTabsStore.getState().tabs
    expect(ids).toEqual([tabs[0].id, tabs[1].id])
    expect(tabs[1].isPinnedMobile).toBe(true)
  })
})

describe("right-click → Remove from this workspace's tabs", () => {
  it('Mobile: removes the tab, writes mobile_tab_enabled 0, mirrors it, and points to Settings → Tabs', async () => {
    setProject()
    useTabsStore.setState({ tabs: withPinnedMobileTab([], true, true) })
    const openSettings = vi.fn()
    useSettingsStore.setState({ openSettings })
    renderBar()

    await rightClickAndRemove('mobile')

    expect(useTabsStore.getState().tabs.some((t) => t.isPinnedMobile)).toBe(false)
    expect(document.querySelector('[data-tab-pinned-mobile]')).toBeNull()
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledTimes(1)
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(expect.objectContaining({ isPrimary: true }), 'workspace/set', {
      project: '/w/app',
      fields: { mobile_tab_enabled: '0' },
    })
    expect(useProjectsStore.getState().projects[0].mobileTabEnabled).toBe(0)

    const toasts = useToastStore.getState().toasts
    expect(toasts).toHaveLength(1)
    expect(toasts[0].message).toBe('Bring it back in Workspace settings → Tabs')
    const action = toasts[0].action
    if (!action) throw new Error('the toast has no link to Settings')
    expect(action.label).toBe('Open')
    action.onClick()
    expect(openSettings).toHaveBeenCalledWith('projects', 'p-app', 'agent')
  })

  it('Design: removes the tab and writes design_mode_enabled 0', async () => {
    setProject({ designModeEnabled: 1 })
    useTabsStore.setState({ tabs: withPinnedDesignTab([], true, true) })
    renderBar()

    await rightClickAndRemove('design')

    expect(useTabsStore.getState().tabs.some((t) => t.isPinnedDesign)).toBe(false)
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(expect.anything(), 'workspace/set', {
      project: '/w/app',
      fields: { design_mode_enabled: '0' },
    })
    expect(useProjectsStore.getState().projects[0].designModeEnabled).toBe(0)
    expect(useToastStore.getState().toasts.map((t) => t.message)).toEqual(['Bring it back in Workspace settings → Tabs'])
  })

  it('a failed write puts the tab back and says why', async () => {
    setProject()
    useTabsStore.setState({ tabs: withPinnedMobileTab([], true, true) })
    vi.mocked(daemonCliPost).mockImplementation(async () => {
      throw new Error('forbidden: not your workspace')
    })
    renderBar()

    await rightClickAndRemove('mobile')

    expect(useTabsStore.getState().tabs.filter((t) => t.isPinnedMobile)).toHaveLength(1)
    expect(useProjectsStore.getState().projects[0].mobileTabEnabled).toBeUndefined()
    const toasts = useToastStore.getState().toasts
    expect(toasts).toHaveLength(1)
    expect(toasts[0].type).toBe('error')
    expect(toasts[0].message).toContain('forbidden')
  })

  it("another client's flip (projects list) removes and restores the Mobile tab", () => {
    setProject({ mobileTabEnabled: null })
    useTabsStore.setState({ tabs: withPinnedMobileTab([], true, true) })
    renderBar()
    act(() => setProject({ mobileTabEnabled: 0 }))
    expect(useTabsStore.getState().tabs.some((t) => t.isPinnedMobile)).toBe(false)
    act(() => setProject({ mobileTabEnabled: 1 }))
    expect(useTabsStore.getState().tabs.filter((t) => t.isPinnedMobile)).toHaveLength(1)
  })
})
