// Plan B (Bulk-1) — vitest coverage for the projects store after migrating
// its DB-backed actions OFF the Tauri `projects_*`/`workspaces_*`/`sections_*`
// invoke proxy ONTO the host-aware `daemonCli*` HTTP layer.
//
// What this asserts:
//   - fetchProjects   → GET `projects/list` + per-project `workspaces/list`
//                       and `sections/list` with snake_case `project_id`
//   - renameProject   → POST `projects/update`  + emits sync:projects
//   - reorderProjects → POST `projects/reorder` + emits sync:projects
//   - removeProject   → POST `workspace-layouts/delete` + `projects/delete`
//                       + emits sync:projects
//   - createSection   → POST `sections/create`  (NO sync — old shim emitted none)
//   - assignWorkspaceToSection → POST `sections/assign` (camelCase, no sync)
//   - setManuallyActive → POST `projects/update` + emits sync:projects
//   - touchInteraction  → POST `projects/touch-interaction` (NO sync)
//
// The store has an import-time side effect (`fetchProjects()`), so every
// dependency is mocked via hoisted `vi.mock` BEFORE the store import.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Mock the host-aware daemon-cli layer (the thing we migrated TO) ──────
const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliGetText: vi.fn(),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

// ── Mock the cross-window emit bus ───────────────────────────────────────
const emitMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => emitMock(...args),
}))

// ── Mock daemon-settings (fetchProjects' last-session restore reads it) ──
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => Promise.resolve({})),
  settingsUpdate: vi.fn(() => Promise.resolve({})),
}))

// ── Mock the daemon-reconnect bus (no-op listener registration) ──────────
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// ── Cross-store deps (only their shapes matter for these paths) ──────────
vi.mock('./git-init-dialog', () => ({
  useGitInitDialogStore: { getState: () => ({ open: vi.fn() }) },
}))
const addToast = vi.fn()
vi.mock('./toast', () => ({
  useToastStore: { getState: () => ({ addToast }) },
}))
// #681 (Bug A) — restoreWorkspace is now async (Promise<void>); the open
// paths await/chain it before ensurePinnedAgentTabForMode. The mock must
// resolve so the `.then()` chain runs and so the ordering test can observe
// ensure firing AFTER restore settles. `callOrder` records the sequence.
const callOrder: string[] = []
const restoreWorkspaceMock = vi.fn((..._args: unknown[]) => {
  callOrder.push('restore')
  return Promise.resolve()
})
const ensurePinnedMock = vi.fn((..._args: unknown[]) => {
  callOrder.push('ensure')
})
vi.mock('./tabs', () => ({
  useTabsStore: {
    getState: () => ({
      stashWorkspace: vi.fn(),
      clearAllTabs: vi.fn(),
      restoreWorkspace: (...args: unknown[]) => restoreWorkspaceMock(...args),
      loadLayoutForWorkspace: vi.fn(),
      clearBackgroundWorkspace: vi.fn(),
      cancelWorkspaceChatReap: vi.fn(),
      tabs: [],
      backgroundWorkspaces: {},
    }),
  },
  ensurePinnedAgentTabForMode: (...args: unknown[]) => ensurePinnedMock(...args),
  // #657 — projects.ts registers a lazy activeProjectId getter on the
  // tabs module at load time; the mock must expose the export so the
  // module-eval call resolves.
  registerActiveProjectIdGetter: vi.fn(),
  // #672 — projects.ts also registers the canonical activate gesture on
  // the tabs module at load time (open/attach⇒activate, PRD §4.3.1).
  registerActivateProject: vi.fn(),
}))
vi.mock('./focus-groups', () => ({
  useFocusGroupsStore: {
    getState: () => ({ focusGroupsEnabled: false, activeFocusGroupId: null }),
  },
}))
vi.mock('./settings', () => ({
  useSettingsStore: {
    getState: () => ({ loaded: true, lastActiveProjectId: null, lastActiveWorkspaceId: null }),
  },
}))

import { useProjectsStore, type ProjectWithWorkspaces } from './projects'

function mkProject(id: string): Record<string, unknown> {
  return {
    id,
    name: id,
    path: `/tmp/${id}`,
    color: '#fff',
    tabOrder: 0,
    lastOpenedAt: null,
    worktreeMode: 0,
    iconUrl: null,
    focusGroupId: null,
    pinned: 0,
    manuallyActive: 0,
    lastInteractionAt: null,
    createdAt: 1,
    agentEnabled: 0,
    heartbeatEnabled: 0,
    agentMode: 'off',
    stateId: null,
    heartbeatMode: 'off',
    heartbeatSchedule: null,
    heartbeatLastFire: null,
  }
}

function resetStore(): void {
  useProjectsStore.setState({ projects: [], activeProjectId: null, activeWorkspaceId: null })
}

describe('projects store — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
    emitMock.mockClear()
    addToast.mockClear()
    restoreWorkspaceMock.mockClear()
    ensurePinnedMock.mockClear()
    callOrder.length = 0
    resetStore()
  })

  it('fetchProjects GETs projects/list then workspaces/list + sections/list per project (snake_case)', async () => {
    daemonCliGet.mockImplementation((route: string, params?: Record<string, unknown>) => {
      if (route === 'projects/list') return Promise.resolve([mkProject('p1')])
      if (route === 'workspaces/list') {
        expect(params).toEqual({ project_id: 'p1' })
        return Promise.resolve([{ id: 'w1', projectId: 'p1', sectionId: null, type: 'main', branch: null, name: 'main', tabOrder: 0, worktreePath: null, navVisible: 1, createdAt: 1 }])
      }
      if (route === 'sections/list') {
        expect(params).toEqual({ project_id: 'p1' })
        return Promise.resolve([])
      }
      throw new Error(`unexpected GET ${route}`)
    })

    await useProjectsStore.getState().fetchProjects()

    expect(daemonCliGet).toHaveBeenCalledWith('projects/list')
    expect(daemonCliGet).toHaveBeenCalledWith('workspaces/list', { project_id: 'p1' })
    expect(daemonCliGet).toHaveBeenCalledWith('sections/list', { project_id: 'p1' })

    const projects = useProjectsStore.getState().projects as ProjectWithWorkspaces[]
    expect(projects).toHaveLength(1)
    expect(projects[0].id).toBe('p1')
    expect(projects[0].workspaces).toHaveLength(1)
    expect(projects[0].sections).toEqual([])
  })

  it('renameProject POSTs projects/update (camelCase) and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([]) // the refetch

    await useProjectsStore.getState().renameProject('p1', 'New Name')

    expect(daemonCliPost).toHaveBeenCalledWith('projects/update', { id: 'p1', name: 'New Name' })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('reorderProjects POSTs projects/reorder and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().reorderProjects(['a', 'b'])

    expect(daemonCliPost).toHaveBeenCalledWith('projects/reorder', { ids: ['a', 'b'] })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('removeProject POSTs workspace-layouts/delete + projects/delete and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValue({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().removeProject('p1')

    expect(daemonCliPost).toHaveBeenCalledWith('workspace-layouts/delete', { projectId: 'p1', workspaceId: null })
    expect(daemonCliPost).toHaveBeenCalledWith('projects/delete', { id: 'p1' })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('createSection POSTs sections/create and does NOT emit sync (old shim emitted none)', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().createSection('p1', 'Group', '#abc')

    expect(daemonCliPost).toHaveBeenCalledWith('sections/create', { projectId: 'p1', name: 'Group', color: '#abc' })
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })

  it('assignWorkspaceToSection POSTs sections/assign (camelCase) without sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().assignWorkspaceToSection('w1', 'sec1')

    expect(daemonCliPost).toHaveBeenCalledWith('sections/assign', { workspaceId: 'w1', sectionId: 'sec1' })
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })

  it('setManuallyActive POSTs projects/pin (canonical-active) and emits sync:projects', async () => {
    // #672 — the active host is 'local' in tests, which serverSupports()
    // treats as supporting every capability, so the pin gesture routes
    // through the canonical projects/pin route (not the legacy
    // projects/update write).
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().setManuallyActive('p1', true)

    expect(daemonCliPost).toHaveBeenCalledWith('projects/pin', { projectId: 'p1', pinned: true })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('touchInteraction POSTs projects/touch-interaction and does NOT emit sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })

    await useProjectsStore.getState().touchInteraction('p1')

    expect(daemonCliPost).toHaveBeenCalledWith('projects/touch-interaction', { id: 'p1' })
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })

  // P1.B — clicking a project in the icon rail (setActiveProject) must
  // reset its 24h Active window by touching lastInteractionAt. Before the
  // fix only setActiveWorkspace did this, so a bare project click never
  // surfaced the workspace in the Active Bar.
  it('setActiveProject touches lastInteractionAt for the clicked project (POSTs touch-interaction)', () => {
    // Use a project id not touched elsewhere in this file — touchInteraction
    // is debounced 5min via a module-level map shared across tests.
    const p = mkProject('p-click') as unknown as ProjectWithWorkspaces
    p.workspaces = [{ id: 'w1' } as never]
    useProjectsStore.setState({
      projects: [p],
      activeProjectId: null,
      activeWorkspaceId: null,
    })
    daemonCliPost.mockResolvedValue({ success: true })

    useProjectsStore.getState().setActiveProject('p-click')

    // touchInteraction (debounced) writes the new lastInteractionAt
    // optimistically AND POSTs to the daemon.
    const updated = (useProjectsStore.getState().projects as ProjectWithWorkspaces[])[0]
    expect(updated.lastInteractionAt).not.toBeNull()
    expect(daemonCliPost).toHaveBeenCalledWith('projects/touch-interaction', { id: 'p-click' })
  })

  // #681 (Bug A) — opening a brand-new workspace must ensure the pinned
  // Chat + Inbox tabs only AFTER restoreWorkspace's (now async) slow-path
  // layout load resolves. Before the fix the two raced (restoreWorkspace
  // was fire-and-forget, then ensure ran synchronously), so on a never-
  // opened workspace the pinned tabs didn't appear until a switch-away.
  // We assert the ordering at the store level: restore THEN ensure, and
  // ensure receives the project's agentMode + path.
  it('setActiveWorkspace awaits restoreWorkspace BEFORE ensurePinnedAgentTabForMode (restore→ensure)', async () => {
    const p = mkProject('p-aw') as unknown as ProjectWithWorkspaces
    p.agentMode = 'manager'
    p.workspaces = [{ id: 'w-aw', worktreePath: null } as never]
    useProjectsStore.setState({
      projects: [p],
      activeProjectId: null,
      activeWorkspaceId: null,
    })
    daemonCliPost.mockResolvedValue({ success: true })

    useProjectsStore.getState().setActiveWorkspace('p-aw', 'w-aw')

    // restore is invoked synchronously; ensure is deferred until the
    // restore promise resolves (chained via .then). Flush microtasks.
    expect(restoreWorkspaceMock).toHaveBeenCalledWith('p-aw:w-aw', '/tmp/p-aw')
    expect(ensurePinnedMock).not.toHaveBeenCalled()
    await Promise.resolve()
    await Promise.resolve()
    expect(ensurePinnedMock).toHaveBeenCalledWith('manager', '/tmp/p-aw')
    expect(callOrder).toEqual(['restore', 'ensure'])
  })

  it('setActiveProject awaits restoreWorkspace BEFORE ensurePinnedAgentTabForMode (restore→ensure)', async () => {
    const p = mkProject('p-ap2') as unknown as ProjectWithWorkspaces
    p.agentMode = 'off'
    p.workspaces = [{ id: 'w-ap2', worktreePath: null } as never]
    useProjectsStore.setState({
      projects: [p],
      activeProjectId: null,
      activeWorkspaceId: null,
    })
    daemonCliPost.mockResolvedValue({ success: true })

    useProjectsStore.getState().setActiveProject('p-ap2')

    expect(restoreWorkspaceMock).toHaveBeenCalledWith('p-ap2:w-ap2', '/tmp/p-ap2')
    expect(ensurePinnedMock).not.toHaveBeenCalled()
    await Promise.resolve()
    await Promise.resolve()
    expect(ensurePinnedMock).toHaveBeenCalledWith('off', '/tmp/p-ap2')
    expect(callOrder).toEqual(['restore', 'ensure'])
  })

  it('a failed mutation does NOT emit sync', async () => {
    daemonCliPost.mockRejectedValueOnce(new Error('daemon down'))
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().renameProject('p1', 'X')

    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
  })
})
