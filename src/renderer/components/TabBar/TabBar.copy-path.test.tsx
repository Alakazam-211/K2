// @vitest-environment jsdom
// Right-click a file tab → "Copy path": puts THAT tab's file path on the
// clipboard (the file the tab shows, as its viewer header names it), even
// when another tab is selected, and says "Path copied". The write goes
// through the Tauri clipboard plugin: in the desktop app's webview a web
// clipboard write after the menu's await is refused, and that refusal used
// to be swallowed. In a room on another server the path is that server's
// path, and there is no "Show in Finder". A tab with no file has no
// "Copy path".

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'

const clip = vi.hoisted(() => ({
  writeText: vi.fn(async (_text: string) => {}),
}))

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  writeText: (text: string) => clip.writeText(text),
  readText: vi.fn(async () => ''),
}))
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
      if (route === 'sessions/list-for-workspace') return []
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
import { useContextMenuStore } from '@/stores/context-menu'
import { primaryRoom, type Room } from '@/stores/room'
import { __resetGitChangesForTests } from '@/stores/git-changes'
import { usePresetsStore } from '@/stores/presets'
import { useTabsStore, type Item, type Tab } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { renderInRoom } from '@/test-utils/room'

const CWD = '/Users/me/work/site'
const realFetch = usePresetsStore.getState().fetchPresets

function fileItem(path: string, pinned = false): Item {
  return { id: `item-${path}`, type: 'file-viewer', data: { filePath: path }, pinned }
}

function tabOf(id: string, items: Item[], activeItemIndex = 0): Tab {
  const pg = `pg-${id}`
  return {
    id,
    title: id,
    mosaicTree: pg,
    paneGroups: new Map([[pg, { id: pg, items, activeItemIndex }]]),
  }
}

function terminalTab(id: string): Tab {
  return tabOf(id, [
    { id: `item-${id}`, type: 'terminal', data: { terminalId: `term-${id}`, cwd: CWD } },
  ])
}

function setTabs(tabs: Tab[], activeTabId: string): void {
  useTabsStore.setState({ tabs, activeTabId, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

function tabEl(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-tab-id="${id}"]`)
  if (!el) throw new Error(`no tab ${id}`)
  return el
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 8; i++) await Promise.resolve()
  })
}

async function openMenu(id: string): Promise<string[]> {
  await act(async () => {
    fireEvent.contextMenu(tabEl(id))
  })
  await flush()
  if (!useContextMenuStore.getState().isOpen) throw new Error('the tab menu did not open')
  return useContextMenuStore.getState().items.map((i) => i.label)
}

async function pick(itemId: string): Promise<void> {
  await act(async () => {
    useContextMenuStore.getState().selectItem(itemId)
  })
  await flush()
}

function renderBar(): void {
  renderInPrimaryRoom(
    <>
      <TabBar cwd={CWD} groupIndex={0} />
      <ContextMenu />
    </>,
  )
}

const webWrite = vi.fn(async (_text: string) => {
  // What the desktop webview does with a web clipboard write it does not
  // count as a user gesture.
  throw new DOMException('The request is not allowed', 'NotAllowedError')
})

beforeEach(() => {
  __resetGitChangesForTests()
  usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
  useContextMenuStore.getState().close()
  useToastStore.setState({ toasts: [] })
  clip.writeText.mockReset()
  clip.writeText.mockImplementation(async () => {})
  webWrite.mockClear()
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: webWrite }, configurable: true })
})

afterEach(() => {
  cleanup()
  useContextMenuStore.getState().close()
  setTabs([], '')
  usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  useToastStore.setState({ toasts: [] })
})

describe('tab right-click → Copy path', () => {
  it("copies the right-clicked tab's file, not the selected tab's, and says so", async () => {
    setTabs(
      [tabOf('plan', [fileItem(`${CWD}/drawings/plan.dxf`)]), tabOf('sampler', [fileItem(`${CWD}/drawings/sampler.dxf`)])],
      'plan',
    )
    renderBar()

    const labels = await openMenu('sampler')
    expect(labels).toContain('Copy path')
    await pick('copy-file-path')

    expect(clip.writeText).toHaveBeenCalledTimes(1)
    expect(clip.writeText).toHaveBeenCalledWith(`${CWD}/drawings/sampler.dxf`)
    expect(useToastStore.getState().toasts.map((t) => [t.type, t.message])).toEqual([['success', 'Path copied']])
    // Selecting the tab is not part of copying its path.
    expect(useTabsStore.getState().activeTabId).toBe('plan')
  })

  it('copies the file the tab shows when its pane holds more than one file', async () => {
    // A pinned file plus the file opened after it in the same pane: the
    // pane shows the second one.
    setTabs([tabOf('notes', [fileItem(`${CWD}/a.md`, true), fileItem(`${CWD}/b.md`)], 1)], 'notes')
    renderBar()

    await openMenu('notes')
    await pick('copy-file-path')

    expect(clip.writeText).toHaveBeenCalledWith(`${CWD}/b.md`)
  })

  it('a failed write says it failed instead of going quiet', async () => {
    clip.writeText.mockImplementation(async () => {
      throw new Error('clipboard busy')
    })
    setTabs([tabOf('plan', [fileItem(`${CWD}/plan.dxf`)])], 'plan')
    renderBar()

    await openMenu('plan')
    await pick('copy-file-path')

    const toasts = useToastStore.getState().toasts
    expect(toasts).toHaveLength(1)
    expect(toasts[0].type).toBe('error')
    expect(toasts[0].message).toContain("Couldn't copy the path")
  })

  it('a tab with no file has no Copy path', async () => {
    setTabs([terminalTab('shell'), tabOf('plan', [fileItem(`${CWD}/plan.dxf`)])], 'plan')
    renderBar()

    const labels = await openMenu('shell')
    expect(labels).not.toContain('Copy path')
    expect(useContextMenuStore.getState().items.map((i) => i.id)).not.toContain('copy-file-path')
  })
})

describe('a tab in a room on another server', () => {
  it("copies that server's path, with no Show in Finder", async () => {
    const serverPath = '/home/k2/ai/site/drawings/plan.dxf'
    setTabs([tabOf('plan', [fileItem(serverPath)]), terminalTab('shell')], 'shell')
    // The primary room's stores, on another server's scope.
    const room: Room = { ...primaryRoom(), key: 'box', scope: fakeScope('box.k2.dev'), isPrimary: false, localCommands: false }
    renderInRoom(
      room,
      <>
        <TabBar cwd="/home/k2/ai/site" groupIndex={0} />
        <ContextMenu />
      </>,
    )

    const labels = await openMenu('plan')
    expect(labels).toContain('Copy path')
    expect(labels).not.toContain('Show in Finder')
    await pick('copy-file-path')

    expect(clip.writeText).toHaveBeenCalledWith(serverPath)
  })
})
