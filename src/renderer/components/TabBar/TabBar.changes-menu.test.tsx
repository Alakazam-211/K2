// @vitest-environment jsdom
// Changes tab + menu row (C2/C5, no pop-in fix for 0.46.0). "Changes" is
// always listed right after Browser, so the open menu never grows or shifts
// when the async `git/info` answer lands. It is greyed (with a tooltip)
// while checking, in a clean repo, in a non-repo folder, or when git is not
// available, and becomes active in place once there are changes. In a
// non-repo folder on a server that can do git it looks greyed but still
// opens the tab (whose empty state offers Initialize repository).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

interface FakeRepo {
  changedFiles: number
  untrackedFiles: number
}

const repos = vi.hoisted(() => ({
  byPath: new Map<string, FakeRepo>(),
  calls: [] as Array<{ scope: unknown; path: string }>,
  // C5: `git/info.gitAvailable` (undefined = an older daemon without the field).
  gitAvailable: undefined as boolean | undefined,
  // When set, git/info waits for `release()` (the "checking" state).
  hold: null as null | { promise: Promise<void>; release: () => void },
  fail: false,
}))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (scope: unknown, route: string, params?: Record<string, string>) => {
      if (route === 'git/info') {
        const path = params?.path ?? ''
        repos.calls.push({ scope, path })
        if (repos.hold) await repos.hold.promise
        if (repos.fail) throw new Error('git/info boom')
        const repo = repos.byPath.get(path)
        const info: Record<string, unknown> = {
          isRepo: repo !== undefined,
          currentBranch: 'main',
          ahead: 0,
          behind: 0,
          changedFiles: repo?.changedFiles ?? 0,
          untrackedFiles: repo?.untrackedFiles ?? 0,
        }
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
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import {
  __resetGitChangesForTests,
  cachedChangesMenuState,
  probeChangesMenuState,
} from '@/stores/git-changes'
import { usePresetsStore } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const realFetch = usePresetsStore.getState().fetchPresets
const EXPECTED_IDS = ['terminal', 'new-file', 'browser', 'changes']

function resetTabs(): void {
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  })
}

async function clickPlus(): Promise<void> {
  const add = document.querySelector('[data-tab-add]') as HTMLButtonElement | null
  if (!add) throw new Error('no + button')
  await act(async () => {
    fireEvent.click(add)
  })
}

async function openPlus(): Promise<void> {
  await clickPlus()
  // Let the git/info probe answer and the menu repaint.
  await flush()
}

function menuIds(): string[] {
  return useContextMenuStore.getState().items.map((i) => i.id)
}

function changesRow(): ContextMenuItemDef {
  const row = useContextMenuStore.getState().items.find((i) => i.id === 'changes')
  if (!row) throw new Error('no Changes row in the + menu')
  return row
}

function changesButton(): HTMLButtonElement {
  const el = document.querySelector('[data-context-menu-item="changes"]') as HTMLButtonElement | null
  if (!el) throw new Error('no rendered Changes row')
  return el
}

function hold(): () => void {
  let release!: () => void
  const promise = new Promise<void>((r) => {
    release = r
  })
  repos.hold = { promise, release }
  return () => {
    repos.hold = null
    release()
  }
}

function renderBar(cwd: string): void {
  renderInPrimaryRoom(
    <>
      <TabBar cwd={cwd} groupIndex={0} />
      <ContextMenu />
    </>,
  )
}

describe('+ menu Changes row', () => {
  beforeEach(() => {
    __resetGitChangesForTests()
    repos.byPath.clear()
    repos.calls = []
    repos.gitAvailable = true
    repos.hold = null
    repos.fail = false
    resetTabs()
    usePresetsStore.setState({ presets: [], fetchPresets: vi.fn(async () => {}) })
    useContextMenuStore.getState().close()
  })

  afterEach(() => {
    repos.hold?.release()
    cleanup()
    useContextMenuStore.getState().close()
    resetTabs()
    usePresetsStore.setState({ presets: [], fetchPresets: realFetch })
  })

  it('checking → changes: greyed "Checking…" first, then active in place; ids and order never change', async () => {
    repos.byPath.set('/ws', { changedFiles: 2, untrackedFiles: 0 })
    const release = hold()
    renderBar('/ws')
    await clickPlus()
    await flush()

    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).toBe(false)
    expect(changesRow().tooltip).toBe('Checking…')
    expect(changesButton().getAttribute('title')).toBe('Checking…')
    expect(changesButton().getAttribute('aria-disabled')).toBe('true')
    const rowsBefore = document.querySelectorAll('[data-context-menu-item]').length

    await act(async () => {
      release()
    })
    await flush()

    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(document.querySelectorAll('[data-context-menu-item]').length).toBe(rowsBefore)
    expect(changesRow().enabled).not.toBe(false)
    expect(changesRow().muted).toBeUndefined()
    expect(changesRow().tooltip).toBeUndefined()
    expect(changesButton().hasAttribute('title')).toBe(false)
    expect(changesButton().hasAttribute('aria-disabled')).toBe(false)
    expect(repos.calls).toEqual([{ scope: primaryScope(), path: '/ws' }])
  })

  it('untracked files alone count as changes', async () => {
    repos.byPath.set('/ws', { changedFiles: 0, untrackedFiles: 1 })
    renderBar('/ws')
    await openPlus()
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).not.toBe(false)
  })

  it('a clean repo: Changes listed but disabled, "No changes to review"; a click does nothing', async () => {
    repos.byPath.set('/ws', { changedFiles: 0, untrackedFiles: 0 })
    renderBar('/ws')
    await openPlus()
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).toBe(false)
    expect(changesButton().getAttribute('title')).toBe('No changes to review')
    await act(async () => {
      fireEvent.click(changesButton())
    })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(useTabsStore.getState().tabs).toHaveLength(0)
  })

  it('C5: a non-repo folder on a server that can do git: greyed, tooltip, and a click still opens the tab (init)', async () => {
    renderBar('/plain')
    await openPlus()
    expect(menuIds()).toEqual(EXPECTED_IDS)
    const row = changesRow()
    expect(row.muted).toBe(true)
    expect(row.enabled).not.toBe(false)
    expect(changesButton().getAttribute('title')).toBe('Not a git repository. Click to open Changes and initialize one.')
    expect(changesButton().style.opacity).toBe('0.5')
    await act(async () => {
      fireEvent.click(changesButton())
    })
    const tabs = useTabsStore.getState().tabs
    expect(tabs).toHaveLength(1)
    expect(tabs[0].title).toBe('Changes')
  })

  it('an older daemon (no gitAvailable) in a non-repo folder: disabled, "Not a git repository"', async () => {
    repos.gitAvailable = undefined
    renderBar('/plain')
    await openPlus()
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).toBe(false)
    expect(changesButton().getAttribute('title')).toBe('Not a git repository')
  })

  it('a server that reports git unavailable: disabled, even in a repo with changes', async () => {
    repos.gitAvailable = false
    repos.byPath.set('/ws', { changedFiles: 3, untrackedFiles: 0 })
    renderBar('/ws')
    await openPlus()
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).toBe(false)
    expect(changesButton().getAttribute('title')).toBe('Git is not available on this server')
  })

  it('a failed git/info stops saying "Checking…" and keeps the row in place', async () => {
    repos.fail = true
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    renderBar('/ws')
    await openPlus()
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).toBe(false)
    expect(changesButton().getAttribute('title')).toBe('Could not check for changes')
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
  })

  it('the second open paints from the cached answer at once; picking Changes again focuses the one tab', async () => {
    repos.byPath.set('/ws', { changedFiles: 1, untrackedFiles: 0 })
    renderBar('/ws')
    await openPlus()
    await act(async () => {
      fireEvent.click(changesButton())
    })
    let s = useTabsStore.getState()
    expect(s.tabs).toHaveLength(1)
    const changesId = s.tabs[0].id
    expect(document.querySelector(`[data-tab-id="${changesId}"] [data-changes-tab-icon]`)).toBeTruthy()

    useTabsStore.getState().addTab('/ws', { title: 'T' })
    expect(useTabsStore.getState().activeTabId).not.toBe(changesId)

    const release = hold()
    await clickPlus()
    // Before the new probe answers: the cached "changes" answer, not Checking.
    expect(menuIds()).toEqual(EXPECTED_IDS)
    expect(changesRow().enabled).not.toBe(false)
    await act(async () => {
      fireEvent.click(changesButton())
    })
    await act(async () => {
      release()
    })
    s = useTabsStore.getState()
    expect(s.tabs.filter((t) => t.title === 'Changes')).toHaveLength(1)
    expect(s.activeTabId).toBe(changesId)
  })
})

describe('probeChangesMenuState (room scope)', () => {
  beforeEach(() => {
    __resetGitChangesForTests()
    repos.byPath.clear()
    repos.calls = []
    repos.gitAvailable = true
    repos.hold = null
    repos.fail = false
  })

  it("asks the room's own server and caches per server", async () => {
    const remote = fakeScope('remote.k2.dev')
    repos.byPath.set('/ws', { changedFiles: 1, untrackedFiles: 0 })
    expect(cachedChangesMenuState(remote, '/ws')).toBeUndefined()
    expect(await probeChangesMenuState(remote, '/ws')).toBe('changes')
    expect(repos.calls).toEqual([{ scope: remote, path: '/ws' }])
    expect(cachedChangesMenuState(remote, '/ws')).toBe('changes')
    expect(cachedChangesMenuState(primaryScope(), '/ws')).toBeUndefined()
  })
})
