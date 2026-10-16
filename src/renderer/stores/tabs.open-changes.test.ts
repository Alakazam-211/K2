// C5 `k2 changes open`: the daemon's app-level `open_changes` opens (or
// focuses) the Changes tab in a room that shows that workspace — the
// primary room for the window's selected project, a pinned room for its
// own workspace — and nowhere else.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const bus = vi.hoisted(() => ({ handlers: new Map<string, Set<(e: { kind: 'open_changes'; workspacePath: string }) => void>>() }))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onOpenChanges: (scope: { id: string }, fn: (e: { kind: 'open_changes'; workspacePath: string }) => void) => {
      const set = bus.handlers.get(scope.id) ?? new Set()
      set.add(fn)
      bus.handlers.set(scope.id, set)
      return () => set.delete(fn)
    },
  }
})

import { fakeScope } from '@/test-utils/fake-scope'
import { primaryScope } from '@/kessel/server-scope'
import {
  createTabsStore,
  initOpenChangesTabs,
  registerActiveProjectIdGetter,
  registerProjectsPathIndex,
  useTabsStore,
  type ChangesItemData,
} from './tabs'

function fire(scopeId: string, workspacePath: string): void {
  for (const fn of bus.handlers.get(scopeId) ?? []) fn({ kind: 'open_changes', workspacePath })
}

function changesPaths(tabs: ReturnType<typeof useTabsStore.getState>['tabs']): string[] {
  return tabs
    .flatMap((t) => Array.from(t.paneGroups.values()).flatMap((pg) => pg.items))
    .filter((i) => i.type === 'changes')
    .map((i) => (i.data as ChangesItemData).workspacePath)
}

let active: string | null = 'p1'

beforeEach(() => {
  bus.handlers.clear()
  active = 'p1'
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0, activeWorkspaceKey: null })
  registerProjectsPathIndex(() => [
    { id: 'p1', path: '/srv/alpha', primaryWorkspaceId: 'w1' },
    { id: 'p2', path: '/srv/beta', primaryWorkspaceId: 'w2' },
  ])
  registerActiveProjectIdGetter(() => active)
})

afterEach(() => {
  registerProjectsPathIndex(() => [])
  registerActiveProjectIdGetter(() => null)
})

describe('open_changes', () => {
  it('the primary room opens the tab for its selected workspace (and folders inside it), once', () => {
    const off = initOpenChangesTabs()
    const id = primaryScope().id
    fire(id, '/srv/beta')
    expect(changesPaths(useTabsStore.getState().tabs)).toEqual([])
    fire(id, '/srv/alpha/')
    fire(id, '/srv/alpha')
    expect(changesPaths(useTabsStore.getState().tabs)).toEqual(['/srv/alpha'])
    const s = useTabsStore.getState()
    expect(s.tabs.find((t) => t.id === s.activeTabId)?.title).toBe('Changes')
    // A worktree checkout inside the workspace gets its own Changes tab.
    fire(id, '/srv/alpha/.worktrees/feat')
    expect(changesPaths(useTabsStore.getState().tabs)).toEqual(['/srv/alpha', '/srv/alpha/.worktrees/feat'])
    // The window switched projects: now beta's events count.
    active = 'p2'
    fire(id, '/srv/beta')
    expect(changesPaths(useTabsStore.getState().tabs)).toContain('/srv/beta')
    off()
    fire(id, '/srv/alpha/other')
    expect(changesPaths(useTabsStore.getState().tabs)).not.toContain('/srv/alpha/other')
  })

  it('the workspace key wins over the projects store selection', () => {
    const off = initOpenChangesTabs()
    active = null
    useTabsStore.setState({ activeWorkspaceKey: 'p2:w2' })
    fire(primaryScope().id, '/srv/beta')
    expect(changesPaths(useTabsStore.getState().tabs)).toEqual(['/srv/beta'])
    off()
  })

  it('a pinned room answers only for its own workspace on its own server', () => {
    const B = fakeScope('b.test')
    const room = createTabsStore({
      scope: B,
      workspace: { projectId: 'p9', workspaceId: 'w9', path: '/home/k2/ai/gamma' },
      deps: {
        projectsPathIndex: () => [{ id: 'p9', path: '/home/k2/ai/gamma', primaryWorkspaceId: 'w9' }],
        activeProjectId: () => 'p9',
        activateProject: () => {},
        projectDefaultAgent: () => undefined,
        presets: () => null,
        heartbeatEntries: () => [],
      },
      localCommands: false,
    })
    const off = room.room.initOpenChangesTabs()
    fire(primaryScope().id, '/home/k2/ai/gamma')
    expect(changesPaths(room.getState().tabs)).toEqual([])
    fire(B.id, '/home/k2/ai/delta')
    expect(changesPaths(room.getState().tabs)).toEqual([])
    fire(B.id, '/home/k2/ai/gamma')
    expect(changesPaths(room.getState().tabs)).toEqual(['/home/k2/ai/gamma'])
    expect(changesPaths(useTabsStore.getState().tabs)).toEqual([])
    off()
  })
})
