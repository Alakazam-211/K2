// @vitest-environment jsdom
// Changes tab C2 — the + menu offers "Changes" (after Browser) only when the
// bar's cwd is a git repo on the room's server, and picking it opens or
// focuses the workspace's one Changes tab.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const repos = vi.hoisted(() => ({
  paths: new Set<string>(),
  calls: [] as Array<{ scope: unknown; path: string }>,
  // C5: `git/info.gitAvailable` (undefined = an older daemon without the field).
  gitAvailable: undefined as boolean | undefined,
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (scope: unknown, route: string, params?: Record<string, string>) => {
      if (route === 'git/info') {
        const path = params?.path ?? ''
        repos.calls.push({ scope, path })
        const info: Record<string, unknown> = { isRepo: repos.paths.has(path), currentBranch: 'main', ahead: 0, behind: 0, changedFiles: 0, untrackedFiles: 0 }
        if (repos.gitAvailable !== undefined) info.gitAvailable = repos.gitAvailable
        return info
      }
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
import { primaryScope } from '@/kessel/server-scope'
import { useContextMenuStore } from '@/stores/context-menu'
import { __resetGitChangesForTests } from '@/stores/git-changes'
import { usePresetsStore } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const realFetch = usePresetsStore.getState().fetchPresets

function resetTabs(): void {
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

async function openPlus(): Promise<void> {
  const add = document.querySelector('[data-tab-add]') as HTMLButtonElement | null
  if (!add) throw new Error('no + button')
  await act(async () => {
    fireEvent.click(add)
  })
  // Let the git/info probe answer and the menu repaint.
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  })
}

function menuIds(): string[] {
  return useContextMenuStore.getState().items.map((i) => i.id)
}

describe('+ menu Changes row', () => {
  beforeEach(() => {
    __resetGitChangesForTests()
    repos.paths.clear()
    repos.calls = []
    repos.gitAvailable = undefined
    resetTabs()
    usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
    useContextMenuStore.getState().close()
  })

  afterEach(() => {
    cleanup()
    useContextMenuStore.getState().close()
    resetTabs()
    usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  })

  it('a git repo gets Changes right after Browser, asked on the room server', async () => {
    repos.paths.add('/ws')
    renderInPrimaryRoom(
      <>
        <TabBar cwd="/ws" groupIndex={0} />
        <ContextMenu />
      </>,
    )
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser', 'changes'])
    expect(repos.calls).toEqual([{ scope: primaryScope(), path: '/ws' }])
  })

  it('C5: a server that can do git offers Changes in a folder that is not a repo yet', async () => {
    repos.gitAvailable = true
    renderInPrimaryRoom(
      <>
        <TabBar cwd="/plain" groupIndex={0} />
        <ContextMenu />
      </>,
    )
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser', 'changes'])
  })

  it('C5: a server that reports git unavailable gets no Changes row, even in a repo', async () => {
    repos.gitAvailable = false
    repos.paths.add('/ws')
    renderInPrimaryRoom(
      <>
        <TabBar cwd="/ws" groupIndex={0} />
        <ContextMenu />
      </>,
    )
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser'])
  })

  it('an older daemon (no gitAvailable): a folder that is not a repo gets no Changes row', async () => {
    renderInPrimaryRoom(
      <>
        <TabBar cwd="/plain" groupIndex={0} />
        <ContextMenu />
      </>,
    )
    await openPlus()
    expect(menuIds()).toEqual(['terminal', 'new-file', 'browser'])
    expect(repos.calls.map((c) => c.path)).toEqual(['/plain'])
  })

  it('picking Changes opens one Changes tab; picking it again focuses that tab', async () => {
    repos.paths.add('/ws')
    renderInPrimaryRoom(
      <>
        <TabBar cwd="/ws" groupIndex={0} />
        <ContextMenu />
      </>,
    )
    await openPlus()
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Changes/ }))
    })
    let s = useTabsStore.getState()
    expect(s.tabs).toHaveLength(1)
    expect(s.tabs[0].title).toBe('Changes')
    const changesId = s.tabs[0].id
    expect(document.querySelector(`[data-tab-id="${changesId}"] [data-changes-tab-icon]`)).toBeTruthy()

    useTabsStore.getState().addTab('/ws', { title: 'T' })
    expect(useTabsStore.getState().activeTabId).not.toBe(changesId)

    await openPlus()
    // Second open paints from the cached answer at once.
    expect(menuIds()).toContain('changes')
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Changes/ }))
    })
    s = useTabsStore.getState()
    expect(s.tabs.filter((t) => t.title === 'Changes')).toHaveLength(1)
    expect(s.activeTabId).toBe(changesId)
  })
})
