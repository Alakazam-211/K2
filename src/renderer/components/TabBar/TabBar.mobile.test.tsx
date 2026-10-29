// @vitest-environment jsdom
// Mobile in the tab strip (prd-mobile-dev-v1 §2.3; vs-live M-A7–M-A9,
// M-A13): the + menu offers "Mobile" (with ⌘⇧M) after Browser and Design
// only on a server that lists `mobile-sim-v1`; picking it opens the Mobile
// tab once and then focuses it (one device per workspace); ⌘⇧M does the
// same, and nothing on a server without the key. The tab paints the
// smartphone glyph.

const setPlatform = vi.hoisted(() => {
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
})
void setPlatform

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
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'
import { __resetServerScopesForTests, noteServerVersion } from '@/kessel/server-scope'
import { useContextMenuStore } from '@/stores/context-menu'
import { __resetGitChangesForTests } from '@/stores/git-changes'
import { usePresetsStore } from '@/stores/presets'
import { primaryRoom } from '@/stores/room'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const realFetch = usePresetsStore.getState().fetchPresets

function resetTabs(): void {
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

function Shortcuts(): null {
  useTerminalShortcuts(primaryRoom(), '/w/docs')
  return null
}

function renderBar(): void {
  renderInPrimaryRoom(
    <>
      <TabBar cwd="/w/docs" groupIndex={0} />
      <ContextMenu />
      <Shortcuts />
    </>,
  )
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

async function pick(id: string): Promise<void> {
  await act(async () => {
    useContextMenuStore.getState().selectItem(id)
    for (let k = 0; k < 5; k++) await Promise.resolve()
  })
}

function menuIds(): string[] {
  return useContextMenuStore.getState().items.map((i) => i.id)
}

function mobileTabs(): Tab[] {
  return useTabsStore.getState().tabs.filter((t) => Array.from(t.paneGroups.values()).some((pg) => pg.items.some((i) => i.type === 'mobile')))
}

function cmdShiftM(): boolean {
  return fireEvent.keyDown(window, { key: 'M', code: 'KeyM', metaKey: true, shiftKey: true })
}

beforeEach(() => {
  __resetServerScopesForTests()
  __resetGitChangesForTests()
  resetTabs()
  usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
  useSettingsStore.setState({ keybindings: {} })
  useContextMenuStore.getState().close()
})

afterEach(() => {
  cleanup()
  useContextMenuStore.getState().close()
  resetTabs()
  usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  useSettingsStore.setState({ keybindings: {} })
  __resetServerScopesForTests()
})

describe('+ menu Mobile row', () => {
  it('a server with mobile-sim-v1: Mobile before Changes, with the phone glyph and ⌘⇧M', async () => {
    noteServerVersion('local', '0.46.6', ['mobile-sim-v1'])
    renderBar()
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser', 'design', 'mobile', 'changes'])
    const row = useContextMenuStore.getState().items.find((i) => i.id === 'mobile')
    if (!row) throw new Error('no Mobile row')
    expect(row.label).toBe('Mobile')
    expect(row.shortcut).toBe('⌘⇧M')
  })

  it('a server without the key shows no Mobile row', async () => {
    noteServerVersion('local', '0.46.6', ['tickets-list-all'])
    renderBar()
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser', 'design', 'changes'])
  })

  it('a rebound chord shows its own label; an empty override falls back to ⌘⇧M', async () => {
    noteServerVersion('local', '0.46.6', ['mobile-sim-v1'])
    useSettingsStore.setState({ keybindings: { openMobile: 'Meta+Alt+M' } })
    renderBar()
    await openPlus()
    expect(useContextMenuStore.getState().items.find((i) => i.id === 'mobile')?.shortcut).toBe('⌘⌥M')
    useContextMenuStore.getState().close()
    useSettingsStore.setState({ keybindings: { openMobile: '' } })
    // A cleared binding falls back to the default (effectiveKeybinding).
    await openPlus()
    expect(useContextMenuStore.getState().items.find((i) => i.id === 'mobile')?.shortcut).toBe('⌘⇧M')
  })

  it('picking Mobile opens the tab once, then focuses it', async () => {
    noteServerVersion('local', '0.46.6', ['mobile-sim-v1'])
    renderBar()
    await openPlus()
    await pick('mobile')
    expect(mobileTabs()).toHaveLength(1)
    const id = mobileTabs()[0].id
    expect(mobileTabs()[0].title).toBe('Mobile')
    expect(document.querySelector(`[data-tab-id="${id}"] [data-k2-icon="smartphone"]`)).toBeTruthy()
    useTabsStore.getState().addTab('/w/docs', { title: 'T' })
    expect(useTabsStore.getState().activeTabId).not.toBe(id)
    await openPlus()
    await pick('mobile')
    expect(mobileTabs()).toHaveLength(1)
    expect(useTabsStore.getState().activeTabId).toBe(id)
  })
})

describe('⌘⇧M (openMobile)', () => {
  it('opens the Mobile tab, then focuses it', () => {
    noteServerVersion('local', '0.46.6', ['mobile-sim-v1'])
    renderBar()
    act(() => {
      expect(cmdShiftM()).toBe(false) // prevented
    })
    expect(mobileTabs()).toHaveLength(1)
    const id = mobileTabs()[0].id
    useTabsStore.getState().addTab('/w/docs', { title: 'T' })
    act(() => {
      cmdShiftM()
    })
    expect(mobileTabs()).toHaveLength(1)
    expect(useTabsStore.getState().activeTabId).toBe(id)
  })

  it('does nothing on a server without mobile-sim-v1', () => {
    noteServerVersion('local', '0.46.6', [])
    renderBar()
    act(() => {
      expect(cmdShiftM()).toBe(true) // not prevented
    })
    expect(mobileTabs()).toHaveLength(0)
  })

  it('follows a rebind', () => {
    noteServerVersion('local', '0.46.6', ['mobile-sim-v1'])
    useSettingsStore.setState({ keybindings: { openMobile: 'Meta+Alt+M' } })
    renderBar()
    act(() => {
      cmdShiftM()
    })
    expect(mobileTabs()).toHaveLength(0)
    act(() => {
      fireEvent.keyDown(window, { key: 'M', code: 'KeyM', metaKey: true, altKey: true })
    })
    expect(mobileTabs()).toHaveLength(1)
  })
})
