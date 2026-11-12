// @vitest-environment jsdom
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { __resetServerScopesForTests, noteServerVersion } from '@/kessel/server-scope'
import * as daemonCli from '@/lib/daemon-cli'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'

// The chord labels below are the macOS glyphs (shortcut-label.ts paints
// Super+… on Linux and none on Windows). Pin a Mac navigator before any
// module computes a label.
const setPlatform = vi.hoisted(() => {
  const set = (platform: string): void => {
    Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  }
  set('MacIntel')
  return set
})
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { PresetsBar } from '@/components/PresetsBar/PresetsBar'
import { AGENTS_MANIFEST, AgentsSection } from '@/components/Settings/sections/AgentsSection'
import { scoreEntry } from '@/components/Settings/searchManifest'
import { TabBar } from '@/components/TabBar/TabBar'
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'
import { daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { menuNewTab } from '@/lib/menu-new-tab'
import { useContextMenuStore } from '@/stores/context-menu'
import { showLaunchBarStorageKey, usePresetsStore, type AgentPreset } from '@/stores/presets'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { FEATURE_ICONS } from '@/components/icons/feature-icons'
import {
  adoptApiSandboxSession,
  registerProjectsPathIndex,
  useTabsStore,
  type BrowserItemData,
  type TerminalItemData,
} from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'
import { primaryRoom } from '@/stores/room'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
      if (route === 'agents/ensure-cli') return { ok: true, installed: false }
      if (route === 'fs/untitled-name') return { name: 'Untitled' }
      return {}
    }),
  }
})

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

const realAddTab = useTabsStore.getState().addTab
const realAddTabToGroup = useTabsStore.getState().addTabToGroup
const realLaunch = usePresetsStore.getState().launchPreset
const realFetch = usePresetsStore.getState().fetchPresets

function preset(over: Partial<AgentPreset> & Pick<AgentPreset, 'id' | 'label'>): AgentPreset {
  return {
    command: over.label.toLowerCase(),
    icon: null,
    enabled: 1,
    sortOrder: 0,
    isBuiltIn: 1,
    createdAt: 1,
    ...over,
  }
}

function resetTabs(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    activeWorkspaceKey: null,
    addTab: realAddTab,
    addTabToGroup: realAddTabToGroup,
  })
}

function seedTabs(groupIndex: number, count: number): void {
  if (groupIndex > 0) {
    const extra = useTabsStore.getState().extraGroups.slice()
    while (extra.length < groupIndex) extra.push({ tabs: [], activeTabId: null })
    useTabsStore.setState({
      splitCount: Math.max(useTabsStore.getState().splitCount, groupIndex + 1),
      extraGroups: extra,
    })
  }
  for (let i = 0; i < count; i++) {
    useTabsStore.getState().addTabToGroup(groupIndex, '/ws', { title: `T${groupIndex}-${i}` })
  }
}

function watchAdds(): ReturnType<typeof vi.fn> {
  const orig = useTabsStore.getState().addTabToGroup
  const spy = vi.fn(orig)
  useTabsStore.setState({ addTabToGroup: spy })
  return spy
}

function Shortcuts({ cwd }: { cwd: string }): null {
  useTerminalShortcuts(primaryRoom(), cwd)
  return null
}

function renderBar(groupIndex = 0) {
  return renderInPrimaryRoom(
    <>
      <TabBar cwd="/ws" groupIndex={groupIndex} />
      <ContextMenu />
    </>,
  )
}

async function openPlus(): Promise<HTMLButtonElement> {
  const add = document.querySelector('[data-tab-add]') as HTMLButtonElement | null
  expect(add).toBeTruthy()
  await act(async () => {
    fireEvent.click(add!)
  })
  return add!
}

describe('tab bar plus menu', () => {
  beforeEach(() => {
    // A server with a device lane, and with `board_file_preview` off (the Board row is its own test).
    noteServerVersion(LOCAL_HOME_HOST, null, ['mobile-sim-v1'])
    localStorage.clear()
    resetTabs()
    usePresetsStore.setState({
      presets: [],
      showPresetsBar: true,
      launchPreset: realLaunch,
      fetchPresets: vi.fn(async () => {}),
    })
    useContextMenuStore.getState().close()
    vi.mocked(invoke).mockClear()
  })

  afterEach(() => {
    __resetServerScopesForTests()
    cleanup()
    useContextMenuStore.getState().close()
    resetTabs()
    registerProjectsPathIndex(() => [])
    useProjectsStore.setState({
      projects: [],
      activeProjectId: null,
      activeWorkspaceId: null,
    })
    useToastStore.setState({ toasts: [] })
    usePresetsStore.setState({
      presets: [],
      showPresetsBar: true,
      launchPreset: realLaunch,
      fetchPresets: realFetch,
    })
    localStorage.clear()
  })

  it('clicking + does not add a tab until Terminal is chosen, including column 1', async () => {
    seedTabs(1, 1)
    useTabsStore.setState({ activeGroupIndex: 0 })
    const beforeRoot = useTabsStore.getState().tabs.length
    const beforeExtra = useTabsStore.getState().extraGroups[0].tabs.length
    const addSpy = watchAdds()
    renderBar(1)

    await openPlus()
    expect(addSpy).not.toHaveBeenCalled()
    expect(useTabsStore.getState().tabs).toHaveLength(beforeRoot)
    expect(useTabsStore.getState().extraGroups[0].tabs).toHaveLength(beforeExtra)
    expect(useContextMenuStore.getState().isOpen).toBe(true)

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Terminal/ }))
    })
    expect(addSpy).toHaveBeenCalledTimes(1)
    expect(addSpy).toHaveBeenCalledWith(1, '/ws')
    expect(useTabsStore.getState().tabs).toHaveLength(beforeRoot)
    expect(useTabsStore.getState().extraGroups[0].tabs).toHaveLength(beforeExtra + 1)
    expect(useTabsStore.getState().activeGroupIndex).toBe(0)
  })

  it('Esc and a click outside create nothing', async () => {
    seedTabs(0, 1)
    const addSpy = watchAdds()
    renderBar(0)

    await openPlus()
    await act(async () => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(addSpy).not.toHaveBeenCalled()
    expect(useContextMenuStore.getState().isOpen).toBe(false)

    await openPlus()
    const backdrop = document.querySelector('[data-context-backdrop]')
    expect(backdrop).toBeTruthy()
    await act(async () => {
      fireEvent.mouseDown(backdrop!)
    })
    expect(addSpy).not.toHaveBeenCalled()
    expect(useContextMenuStore.getState().isOpen).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('New File and Browser from column 1 append there and an empty url does not call browser_create', async () => {
    seedTabs(1, 1)
    useTabsStore.setState({ activeGroupIndex: 0 })
    renderBar(1)

    await openPlus()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^New File/ }))
    })
    await waitFor(() =>
      expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.title)).toContain('Untitled'),
    )
    expect(useTabsStore.getState().tabs.map((t) => t.title)).not.toContain('Untitled')

    vi.mocked(invoke).mockClear()
    await openPlus()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Browser' }))
    })
    expect(vi.mocked(invoke).mock.calls.map((c) => c[0])).not.toContain('browser_create')
    expect(useTabsStore.getState().tabs.map((t) => t.title)).not.toContain('Browser')
    const browser = useTabsStore.getState().extraGroups[0].tabs.find((t) => t.title === 'Browser')
    expect(browser).toBeTruthy()
    const item = Array.from(browser!.paneGroups.values())[0].items[0]
    expect(item.type).toBe('browser')
    expect((item.data as BrowserItemData).url).toBe('')
  })

  it('with the launch bar hidden and the agent list never loaded, + loads it and lists the agents', async () => {
    const fetchPresets = vi.fn(async () => {
      usePresetsStore.setState({
        presets: [preset({ id: 'claude', label: 'Claude', icon: null, command: 'claude', enabled: 1 })],
        presetsLoaded: true,
      })
    })
    usePresetsStore.setState({ presets: [], presetsLoaded: false, showPresetsBar: false, fetchPresets })
    seedTabs(1, 1)
    useTabsStore.setState({ activeGroupIndex: 0 })
    renderBar(1)
    await openPlus()

    expect(fetchPresets).toHaveBeenCalledTimes(1)
    const items = useContextMenuStore.getState().items
    expect(items.filter((item) => item.id.startsWith('preset:')).map((item) => item.label)).toEqual(['Claude'])
  })

  it('a preset row launches into this bar group in strip order, and disabled rows are absent', async () => {
    usePresetsStore.setState({
      presets: [
        preset({ id: 'zed', label: 'Zed', icon: '⚡', command: 'zed', enabled: 1 }),
        preset({ id: 'off', label: 'Off', command: 'off', enabled: 0 }),
        preset({ id: 'claude', label: 'Claude', icon: null, command: 'claude', enabled: 1 }),
      ],
    })
    seedTabs(1, 1)
    useTabsStore.setState({ activeGroupIndex: 0 })
    renderBar(1)
    await openPlus()

    const items = useContextMenuStore.getState().items
    expect(items.map((item) => item.label)).toEqual([
      'Terminal',
      'New File',
      'Browser',
      'Design',
      'Mobile',
      'Changes',
      '',
      'Zed',
      'Claude',
    ])
    expect(items.find((item) => item.label === 'Zed')?.type).toBeUndefined()
    expect(items.some((item) => item.type === 'separator')).toBe(true)
    expect(items.map((item) => item.label)).not.toContain('Off')
    expect(screen.getByText('⚡')).toBeTruthy()
    const claude = screen.getByRole('button', { name: /^Claude/ })
    expect(claude.querySelector('svg')).toBeTruthy()

    const enabled = usePresetsStore.getState().presets.filter((p) => p.enabled)
    expect(items.filter((item) => item.id.startsWith('preset:')).map((item) => item.label)).toEqual(
      enabled.map((p) => p.label),
    )

    await act(async () => {
      fireEvent.click(claude)
    })
    expect(useTabsStore.getState().tabs.map((t) => t.title)).not.toContain('Claude')
    expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.title)).toContain('Claude')
    expect(useTabsStore.getState().activeGroupIndex).toBe(0)
  })

  it('the Board row: only on a server with the feature; it creates in the bar’s folder and opens what comes back', async () => {
    const post = vi.spyOn(daemonCli, 'daemonCliPost').mockResolvedValue({ path: '/tmp/k2-plus-menu/Untitled-2.k5board', name: 'Untitled-2.k5board', hash: 'sha256-x' } as never)
    usePresetsStore.setState({ presets: [] })
    seedTabs(0, 1)
    renderBar(0)
    await openPlus()
    expect(useContextMenuStore.getState().items.map((i) => i.label)).not.toContain('Board')
    await act(async () => useContextMenuStore.getState().close())

    noteServerVersion(LOCAL_HOME_HOST, null, ['mobile-sim-v1', 'board-file-v1'])
    await openPlus()
    const items = useContextMenuStore.getState().items
    expect(items.map((i) => i.label)).toEqual(['Terminal', 'New File', 'Browser', 'Design', 'Board', 'Mobile', 'Changes'])
    expect(items.find((i) => i.id === 'board')?.badge).toBe('Beta')
    // Its own icon, never Design's palette (Rosson 2026-11-09).
    const iconOf = (id: string): unknown => (items.find((i) => i.id === id)?.icon as { type?: unknown } | undefined)?.type
    expect(iconOf('board')).toBe(FEATURE_ICONS.board)
    expect(iconOf('design')).toBe(FEATURE_ICONS.design)
    expect(iconOf('board')).not.toBe(iconOf('design'))
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Board/ }))
    })
    await waitFor(() => expect(post).toHaveBeenCalled())
    const [, route, body] = post.mock.calls[0]
    expect(route).toBe('board/file/create')
    expect(Object.keys(body as object)).toEqual(['dir'])
    await waitFor(() => {
      const paths = useTabsStore.getState().tabs.flatMap((t) => [...t.paneGroups.values()].flatMap((pg) => pg.items.map((it) => (it.data as { filePath?: string } | undefined)?.filePath)))
      expect(paths).toContain('/tmp/k2-plus-menu/Untitled-2.k5board')
    })
    post.mockRestore()
  })

  it('the Desktop row (Beta): only on a server with remote-desktop-v1; each click opens another Desktop tab (LD-35)', async () => {
    usePresetsStore.setState({ presets: [] })
    seedTabs(0, 1)
    renderBar(0)
    await openPlus()
    expect(useContextMenuStore.getState().items.map((i) => i.id)).not.toContain('desktop')
    await act(async () => useContextMenuStore.getState().close())

    noteServerVersion(LOCAL_HOME_HOST, null, ['mobile-sim-v1', 'remote-desktop-v1'])
    await openPlus()
    const items = useContextMenuStore.getState().items
    expect(items.map((i) => i.label)).toEqual(['Terminal', 'New File', 'Browser', 'Design', 'Mobile', 'Desktop', 'Changes'])
    expect(items.find((i) => i.id === 'desktop')?.badge).toBe('Beta')
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Desktop/ }))
    })
    const desktops = (): number =>
      useTabsStore.getState().tabs.filter((t) => [...t.paneGroups.values()].some((pg) => pg.items.some((it) => it.type === 'desktop'))).length
    expect(desktops()).toBe(1)
    await openPlus()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Desktop/ }))
    })
    expect(desktops()).toBe(2)
  })

  it('the Board row is in the first list, not only after a repaint: a second open with the changes state cached keeps it', async () => {
    // Rosson 2026-11-04: no Board in the + menu. The first `show` left the row
    // out and only a repaint (Option, or the git probe moving the changes
    // state) added it, so once the state was cached it never came back.
    // git/info answers the same each time (a clean repo), so the second open
    // starts from the cached 'clean' and the probe moves nothing: no repaint.
    const get = vi.spyOn(daemonCli, 'daemonCliGet').mockImplementation((async (_scope: unknown, route: string) => {
      if (route === 'git/info') return { isRepo: true, gitAvailable: true, changedFiles: 0, untrackedFiles: 0 }
      return {}
    }) as never)
    usePresetsStore.setState({ presets: [] })
    seedTabs(0, 1)
    renderBar(0)
    noteServerVersion(LOCAL_HOME_HOST, null, ['board-file-v1'])
    await openPlus()
    await waitFor(() => expect(get.mock.calls.some((c) => c[1] === 'git/info')).toBe(true))
    expect(useContextMenuStore.getState().items.map((i) => i.label)).toContain('Board')
    await act(async () => useContextMenuStore.getState().close())
    get.mockClear()
    await openPlus()
    await waitFor(() => expect(get.mock.calls.some((c) => c[1] === 'git/info')).toBe(true))
    await act(async () => {})
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(useContextMenuStore.getState().items.map((i) => i.label)).toContain('Board')
    get.mockRestore()
  })

  it('a refused create opens no tab and names the folder and the reason', async () => {
    noteServerVersion(LOCAL_HOME_HOST, null, ['board-file-v1'])
    const post = vi.spyOn(daemonCli, 'daemonCliPost').mockRejectedValue(new Error('this folder is read only'))
    seedTabs(0, 1)
    const tabsBefore = useTabsStore.getState().tabs.length
    renderBar(0)
    await openPlus()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Board/ }))
    })
    await waitFor(() => expect(post).toHaveBeenCalled())
    await waitFor(() => expect(useToastStore.getState().toasts.some((t) => /read only/.test(t.message) && /Couldn’t make a board/.test(t.message))).toBe(true))
    expect(useTabsStore.getState().tabs.length).toBe(tabsBefore)
    post.mockRestore()
  })

  it('omits the separator when no preset is enabled', async () => {
    usePresetsStore.setState({
      presets: [preset({ id: 'off', label: 'Off', enabled: 0 })],
    })
    seedTabs(0, 1)
    renderBar(0)
    await openPlus()
    const items = useContextMenuStore.getState().items
    expect(items.map((item) => item.label)).toEqual(['Terminal', 'New File', 'Browser', 'Design', 'Mobile', 'Changes'])
    expect(items.some((item) => item.type === 'separator')).toBe(false)
    expect(items.find((item) => item.id === 'browser')?.icon).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Off' })).toBeNull()
  })

  it('the + tooltip does not claim the click is Cmd+T, and the Terminal row may show it', async () => {
    seedTabs(0, 1)
    renderBar(0)
    const add = document.querySelector('[data-tab-add]') as HTMLButtonElement
    expect(add.title).toBe('New tab')
    expect(add.title).not.toMatch(/cmd\+t/i)
    expect(add.title).not.toContain('⌘T')
    await openPlus()
    const symbols = document.querySelectorAll('[data-context-menu] .key-symbol')
    expect(symbols[0]?.textContent).toBe('⌘')
    expect(symbols[0]?.parentElement?.textContent).toBe('⌘T')
    expect(symbols[1]?.textContent).toBe('⌘')
    expect(symbols[1]?.parentElement?.textContent).toBe('⌘N')
  })

  it('off macOS the Terminal and New File rows show no Mac glyphs', async () => {
    setPlatform('Linux x86_64')
    try {
      seedTabs(0, 1)
      renderBar(0)
      await openPlus()
      const items = useContextMenuStore.getState().items
      expect(items.find((item) => item.id === 'terminal')?.shortcut).toBe('Super+T')
      expect(items.find((item) => item.id === 'new-file')?.shortcut).toBe('Super+N')
      expect(document.querySelector('[data-context-menu]')?.textContent).not.toMatch(/[⌘⇧]/)
    } finally {
      setPlatform('MacIntel')
    }
  })

  it('puts ⇧⌘T on the workspace default agent, not the first preset', async () => {
    usePresetsStore.setState({
      presets: [
        preset({ id: 'claude', label: 'Claude', sortOrder: 0 }),
        preset({ id: 'codex', label: 'Codex', sortOrder: 1 }),
      ],
    })
    useSettingsStore.setState({ defaultAgent: 'claude' })
    useProjectsStore.setState({
      activeProjectId: 'p',
      projects: [
        {
          id: 'p',
          path: '/ws',
          defaultAgent: 'codex',
          workspaces: [],
        } as never,
      ],
    })
    seedTabs(0, 1)
    renderBar(0)
    await openPlus()
    const items = useContextMenuStore.getState().items
    expect(items.find((item) => item.label === 'Claude')?.shortcut).toBeUndefined()
    expect(items.find((item) => item.label === 'Codex')?.shortcut).toBe('⇧⌘T')
    const shift = document.querySelector('[data-context-menu] .key-symbol')
    expect(
      Array.from(document.querySelectorAll('[data-context-menu] .key-symbol')).some(
        (node) => node.textContent === '⇧' && node.parentElement?.textContent === '⇧⌘T',
      ),
    ).toBe(true)
    expect(shift).toBeTruthy()
  })

  it('with two short tabs the + follows the scroller and split follows the +', () => {
    seedTabs(0, 2)
    renderBar(0)
    const scroller = document.querySelector('[data-tab-scroller]') as HTMLElement
    const add = document.querySelector('[data-tab-add]') as HTMLElement
    const spacer = document.querySelector('[data-tab-bar-spacer]') as HTMLElement
    const split = screen.getByTitle('Split into columns')
    expect(scroller.className.split(/\s+/)).not.toContain('flex-1')
    expect(scroller.className).toContain('min-w-0')
    expect(scroller.className).toContain('overflow-x-auto')
    expect(scroller.style.flex).toBe('0 1 auto')
    expect(scroller.querySelectorAll('[data-tab-id]')).toHaveLength(2)
    expect(scroller.contains(add)).toBe(false)
    expect(scroller.nextElementSibling).toBe(add)
    expect(add.nextElementSibling).toBe(spacer)
    expect(spacer.className.split(/\s+/)).toContain('flex-1')
    expect(add.compareDocumentPosition(split) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(spacer.compareDocumentPosition(split) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(screen.queryByTitle('Remove column')).toBeNull()
  })

  it('a non-rightmost bar has no split button and still keeps + outside the scroller', () => {
    seedTabs(0, 2)
    useTabsStore.setState({
      splitCount: 2,
      extraGroups: [{ tabs: [], activeTabId: null }],
    })
    renderBar(0)
    const scroller = document.querySelector('[data-tab-scroller]') as HTMLElement
    const add = document.querySelector('[data-tab-add]') as HTMLElement
    expect(scroller.nextElementSibling).toBe(add)
    expect(scroller.contains(add)).toBe(false)
    expect(screen.queryByTitle('Split into columns')).toBeNull()
    expect(screen.queryByTitle('Remove column')).toBeNull()
  })

  it('the rightmost split bar still puts split after the +', () => {
    seedTabs(1, 2)
    renderBar(1)
    const add = document.querySelector('[data-tab-add]') as HTMLElement
    const split = screen.getByTitle('Split into columns')
    expect(screen.getByTitle('Remove column')).toBeTruthy()
    expect(add.compareDocumentPosition(split) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
  })

  it('the split button keeps the focused column and moves its selected tab (the 3rd column too)', () => {
    // Two columns, focus on column 0 where the user picked a regular tab.
    seedTabs(0, 2)
    seedTabs(1, 1)
    const col0 = useTabsStore.getState().tabs
    const picked = col0[1]
    const col1Only = useTabsStore.getState().extraGroups[0].tabs[0]
    useTabsStore.setState({ activeTabId: picked.id, activeGroupIndex: 0 })
    const columnMouseDown = vi.fn()
    renderInPrimaryRoom(
      // The column's own mousedown focuses it (TerminalArea TabGroupColumn).
      <div onMouseDown={columnMouseDown}>
        <TabBar cwd="/ws" groupIndex={1} />
      </div>,
    )
    const split = screen.getByTitle('Split into columns')
    fireEvent.mouseDown(split)
    expect(columnMouseDown).not.toHaveBeenCalled()
    fireEvent.click(split)
    const s = useTabsStore.getState()
    expect(s.splitCount).toBe(3)
    expect(s.tabs.map((t) => t.id)).toEqual([col0[0].id])
    expect(s.extraGroups[0].tabs.map((t) => t.id)).toEqual([col1Only.id])
    expect(s.extraGroups[1].tabs.map((t) => t.id)).toEqual([picked.id])
    expect(s.activeGroupIndex).toBe(2)
  })

  it('Cmd+T and Cmd+Shift+T stay direct and do not open the menu', () => {
    usePresetsStore.setState({
      presets: [preset({ id: 'claude', label: 'Claude', command: 'claude' })],
    })
    const addTab = vi.fn()
    const launch = vi.fn()
    useTabsStore.setState({ addTab })
    usePresetsStore.setState({ launchPreset: launch })
    renderInPrimaryRoom(<Shortcuts cwd="/ws" />)

    fireEvent.keyDown(window, { key: 't', metaKey: true })
    expect(addTab).toHaveBeenCalledWith('/ws')
    expect(useContextMenuStore.getState().isOpen).toBe(false)

    fireEvent.keyDown(window, { key: 'T', metaKey: true, shiftKey: true })
    expect(launch).toHaveBeenCalledWith(useTabsStore, 'claude', '/ws', 'tab')
    expect(launch.mock.calls[0]).toHaveLength(4)
    expect(useContextMenuStore.getState().isOpen).toBe(false)
  })

  it('menu:new-tab calls addTab and does not open the menu', () => {
    const addTab = vi.fn()
    useTabsStore.setState({ addTab })
    // Window-level menu items act on the focused room (MS17/MS18).
    renderInPrimaryRoom(<></>)
    menuNewTab()
    expect(addTab).toHaveBeenCalledWith('~')
    expect(useContextMenuStore.getState().isOpen).toBe(false)
    const app = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), '../../App.tsx'),
      'utf8',
    )
    expect(app).toMatch(/listen\('menu:new-tab',\s*\(\)\s*=>\s*\{\s*menuNewTab\(\)/)
  })

  it('Show launch bar off hides PresetsBar, keeps menu presets, and the bit survives a remount', async () => {
    usePresetsStore.setState({
      presets: [
        preset({ id: 'zed', label: 'Zed', icon: '⚡', enabled: 1 }),
        preset({ id: 'off', label: 'Off', enabled: 0 }),
      ],
    })
    usePresetsStore.getState().setShowLaunchBar(false)
    expect(localStorage.getItem(showLaunchBarStorageKey('local'))).toBe('0')

    const hidden = renderInPrimaryRoom(<PresetsBar cwd="/ws" />)
    expect(hidden.container.firstChild).toBeNull()
    expect(usePresetsStore.getState().fetchPresets).toHaveBeenCalled()
    hidden.unmount()

    const again = renderInPrimaryRoom(<PresetsBar cwd="/ws" />)
    expect(again.container.firstChild).toBeNull()
    expect(localStorage.getItem(showLaunchBarStorageKey('local'))).toBe('0')
    again.unmount()

    seedTabs(0, 1)
    renderBar(0)
    await openPlus()
    expect(screen.getByRole('button', { name: /^Zed/ })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Off' })).toBeNull()
  })

  function llmPresets(): void {
    useSettingsStore.setState({ defaultAgent: 'claude' })
    usePresetsStore.setState({
      presets: [
        preset({ id: 'claude', label: 'Claude', command: 'claude', icon: 'C' }),
        preset({ id: 'codex', label: 'Codex', command: 'codex', icon: 'X' }),
        preset({ id: 'off', label: 'Off', command: 'grok', enabled: 0 }),
      ],
    })
  }

  async function openHeld(altKey: boolean): Promise<void> {
    const add = document.querySelector('[data-tab-add]') as HTMLButtonElement
    await act(async () => {
      fireEvent.click(add, { altKey })
    })
  }

  it('replaceItems swaps rows without closing the menu', async () => {
    seedTabs(0, 1)
    renderBar(0)
    await openPlus()
    const onSelect = useContextMenuStore.getState().onSelect
    const focused = useContextMenuStore.getState().focusedIndex
    useContextMenuStore.getState().replaceItems([{ id: 'only', label: 'Only' }])
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(useContextMenuStore.getState().onSelect).toBe(onSelect)
    expect(useContextMenuStore.getState().focusedIndex).toBe(focused)
    expect(useContextMenuStore.getState().items.map((item) => item.id)).toEqual(['only'])
  })

  it('Option down adds open in sandbox and Beta on enabled presets only, and keyup restores the shortcut', async () => {
    seedTabs(0, 1)
    llmPresets()
    renderBar(0)
    await openHeld(false)
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(screen.queryByText('open in sandbox')).toBeNull()

    const before = useContextMenuStore.getState().items.length
    await act(async () => {
      fireEvent.keyDown(window, { key: 'Alt' })
      fireEvent.keyDown(window, { key: 'Alt' })
    })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(useContextMenuStore.getState().items).toHaveLength(before)
    expect(screen.getAllByText('open in sandbox')).toHaveLength(2)
    expect(screen.getAllByText('Beta')).toHaveLength(2)
    expect(screen.queryByRole('button', { name: 'Off' })).toBeNull()
    const held = useContextMenuStore.getState().items
    for (const id of ['terminal', 'new-file', 'browser']) {
      const row = held.find((item) => item.id === id)
      expect(row?.hint).toBeUndefined()
      expect(row?.badge).toBeUndefined()
    }
    expect(held.find((item) => item.id === 'terminal')?.shortcut).toBe('⌘T')
    expect(held.find((item) => item.id === 'new-file')?.shortcut).toBe('⌘N')
    const claude = held.find((item) => item.id === 'preset:claude')
    const codex = held.find((item) => item.id === 'preset:codex')
    expect(claude?.hint).toBe('open in sandbox')
    expect(claude?.badge).toBe('Beta')
    expect(claude?.shortcut).toBeUndefined()
    expect(codex?.hint).toBe('open in sandbox')
    expect(codex?.badge).toBe('Beta')
    expect(codex?.shortcut).toBeUndefined()

    await act(async () => {
      fireEvent.keyUp(window, { key: 'Alt' })
    })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(screen.queryByText('open in sandbox')).toBeNull()
    expect(screen.queryByText('Beta')).toBeNull()
    const restored = useContextMenuStore.getState().items
    expect(restored.find((item) => item.id === 'preset:claude')?.shortcut).toBe('⇧⌘T')
    expect(restored.find((item) => item.id === 'preset:codex')?.shortcut).toBeUndefined()
    expect(restored.find((item) => item.id === 'terminal')?.shortcut).toBe('⌘T')
  })

  it('opening with Option already held starts in that picture, and blur clears only the hint', async () => {
    seedTabs(0, 1)
    llmPresets()
    renderBar(0)
    await openHeld(true)
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(screen.getAllByText('open in sandbox')).toHaveLength(2)
    expect(screen.getAllByText('Beta')).toHaveLength(2)

    await act(async () => {
      window.dispatchEvent(new Event('blur'))
    })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(screen.queryByText('open in sandbox')).toBeNull()
    expect(useContextMenuStore.getState().items.find((item) => item.id === 'preset:claude')?.shortcut).toBe('⇧⌘T')
  })

  it('a preset click without Option calls launchPreset and does not post sandbox/open', async () => {
    seedTabs(0, 1)
    llmPresets()
    const launch = vi.fn(async () => {})
    usePresetsStore.setState({ launchPreset: launch })
    vi.mocked(daemonCliPost).mockClear()
    renderBar(0)
    await openHeld(false)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Codex/ }))
    })
    expect(launch).toHaveBeenCalledTimes(1)
    expect(launch).toHaveBeenCalledWith(useTabsStore, 'codex', '/ws', 'tab', 0)
    expect(vi.mocked(daemonCliPost).mock.calls.some((call) => call[1] === 'sandbox/open')).toBe(false)
  })

  it('Option-click posts sandbox/open and does not call launchPreset; 409 adds no tab', async () => {
    seedTabs(1, 1)
    llmPresets()
    const launch = vi.fn(async () => {})
    usePresetsStore.setState({ launchPreset: launch })
    vi.mocked(daemonCliPost).mockClear()
    vi.mocked(daemonCliPost).mockRejectedValueOnce(
      new Error('this daemon cannot sandbox (microVM backend unavailable)'),
    )
    useToastStore.setState({ toasts: [] })
    const beforeRoot = useTabsStore.getState().tabs.length
    const beforeExtra = useTabsStore.getState().extraGroups[0].tabs.length
    renderBar(1)
    await openHeld(true)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Codex/ }), { altKey: true })
      await Promise.resolve()
    })
    expect(launch).not.toHaveBeenCalled()
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(primaryScope(), 'sandbox/open', {
      project_path: '/ws',
      preset_id: 'codex',
    })
    expect(useTabsStore.getState().tabs).toHaveLength(beforeRoot)
    expect(useTabsStore.getState().extraGroups[0].tabs).toHaveLength(beforeExtra)
    expect(
      useToastStore.getState().toasts.some((toast) =>
        toast.message.includes('this daemon cannot sandbox (microVM backend unavailable)'),
      ),
    ).toBe(true)
  })

  it('a 200 places the microvm tab in the clicked group even when hideApiSessions is on', async () => {
    seedTabs(1, 1)
    llmPresets()
    registerProjectsPathIndex(() => [
      { id: 'p', path: '/ws', primaryWorkspaceId: 'w', hideApiSessions: true },
    ])
    const hidden = adoptApiSandboxSession({
      kind: 'session_added',
      workspace_path: '/ws',
      pane_group_id: null,
      agent_name: 'api-owner-broadcast',
      command: 'codex',
      args: [],
      session_id: 'sess-broadcast',
      isV2: true,
      sandbox_backend: 'microvm',
    })
    expect(hidden).toBe(false)
    const beforeRoot = useTabsStore.getState().tabs.length
    const launch = vi.fn(async () => {})
    usePresetsStore.setState({ launchPreset: launch })
    vi.mocked(daemonCliPost).mockResolvedValueOnce({
      sessionId: 'sess-clicked',
      agentName: 'api-owner-clicked',
      sandbox: 'microvm',
    })
    renderBar(1)
    await openHeld(false)
    await act(async () => {
      fireEvent.keyDown(window, { key: 'Alt' })
    })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Claude/ }), { altKey: true })
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(launch).not.toHaveBeenCalled()
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(primaryScope(), 'sandbox/open', {
      project_path: '/ws',
      preset_id: 'claude',
    })
    expect(useTabsStore.getState().tabs).toHaveLength(beforeRoot)
    const column = useTabsStore.getState().extraGroups[0]
    const added = column.tabs.find((tab) =>
      [...tab.paneGroups.values()].some((pg) =>
        pg.items.some((item) => {
          if (item.type !== 'terminal') return false
          const data = item.data as TerminalItemData
          return data.attachAgentName === 'api-owner-clicked' && data.sandboxBackend === 'microvm'
        }),
      ),
    )
    expect(added).toBeTruthy()
    expect(column.activeTabId).toBe(added!.id)
    expect(useTabsStore.getState().tabs.some((tab) => tab.id === added!.id)).toBe(false)
  })
})

describe('Settings → LLMs show launch bar', () => {
  beforeEach(() => {
    localStorage.clear()
    usePresetsStore.setState({
      presets: [],
      showPresetsBar: true,
      fetchPresets: vi.fn(async () => {}),
    })
  })

  afterEach(() => {
    cleanup()
    localStorage.clear()
    usePresetsStore.setState({
      presets: [],
      showPresetsBar: true,
      fetchPresets: realFetch,
      launchPreset: realLaunch,
    })
  })

  it('sits under Default agent, above Launch presets, and search can find it', () => {
    render(<AgentsSection />)
    const box = screen.getByRole('switch', { name: 'Show launch bar' })
    expect(box.getAttribute('aria-checked')).toBe('true')
    // Default agent is a row now (Settings hill-climb): its picker carries the name.
    const defaults = document.querySelector('[aria-label="Default agent"]') as Element
    const presets = screen.getByRole('heading', { name: 'Launch presets' })
    expect(defaults.compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(box.compareDocumentPosition(presets) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)

    const entry = AGENTS_MANIFEST.find((item) => item.id === 'agents.show-launch-bar')
    expect(entry?.label).toBe('Show launch bar')
    expect(entry?.section).toBe('agents')
    expect(scoreEntry(entry!, 'launch bar')).toBeGreaterThan(0)
    expect(scoreEntry(entry!, 'show launch')).toBeGreaterThan(0)
  })

  it('unchecking writes the host bit', () => {
    render(<AgentsSection />)
    fireEvent.click(screen.getByRole('switch', { name: 'Show launch bar' }))
    expect(usePresetsStore.getState().showPresetsBar).toBe(false)
    expect(localStorage.getItem(showLaunchBarStorageKey('local'))).toBe('0')
  })
})
