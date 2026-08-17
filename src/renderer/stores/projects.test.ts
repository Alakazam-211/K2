// Plan B (Bulk-1) — vitest coverage for the projects store after migrating
// its DB-backed actions OFF the Tauri `projects_*`/`workspaces_*` invoke
// proxy ONTO the host-aware `daemonCli*` HTTP layer.
//
// What this asserts:
//   - fetchProjects   → GET `projects/list` + per-project `workspaces/list`
//                       with snake_case `project_id`
//   - renameProject   → POST `projects/update`  + emits sync:projects
//   - reorderProjects → optimistic local reorder + POST `projects/reorder`
//                       + emits sync:projects; success path does NOT refetch
//   - setProjectColor → optimistic color patch + POST `projects/update`;
//                       success path does NOT refetch; failure rolls back
//   - removeProject   → POST `workspace-layouts/delete` + `projects/delete`
//                       + emits sync:projects
//   - setManuallyActive → POST `projects/update` + emits sync:projects
//   - touchInteraction  → POST `projects/touch-interaction` (NO sync)
//
// The store has an import-time side effect (`fetchProjects()`), so every
// dependency is mocked via hoisted `vi.mock` BEFORE the store import.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ── Mock the host-aware daemon-cli layer (the thing we migrated TO) ──────
const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliGetText: vi.fn(),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

// ── Mock the cross-window emit bus ───────────────────────────────────────
const emitMock = vi.fn((..._args: unknown[]) => Promise.resolve())
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
  // Agent-degeneralization S1 — per-workspace default-agent lazy reader,
  // registered at module load like the two above.
  registerProjectDefaultAgentGetter: vi.fn(),
  // Host-session tab routing — projects path index for adopt-by-workspace_path.
  registerProjectsPathIndex: vi.fn(),
  runLeaveGuard: vi.fn(async () => true),
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
vi.mock('@/lib/workspace-switch-focus', () => ({
  applyWorkspaceSwitchFocus: vi.fn(),
}))

import {
  useProjectsStore,
  scheduleProjectsRefreshFromSync,
  _resetProjectsChangedSyncForTests,
  type ProjectWithWorkspaces,
} from './projects'

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
    _resetProjectsChangedSyncForTests()
  })

  afterEach(() => {
    vi.useRealTimers()
    _resetProjectsChangedSyncForTests()
  })

  it('fetchProjects GETs projects/list then workspaces/list per project (snake_case)', async () => {
    daemonCliGet.mockImplementation((route: string, params?: Record<string, unknown>) => {
      if (route === 'projects/list') return Promise.resolve([mkProject('p1')])
      if (route === 'workspaces/list') {
        expect(params).toEqual({ project_id: 'p1' })
        return Promise.resolve([{ id: 'w1', projectId: 'p1', sectionId: null, type: 'main', branch: null, name: 'main', tabOrder: 0, worktreePath: null, navVisible: 1, createdAt: 1 }])
      }
      throw new Error(`unexpected GET ${route}`)
    })

    await useProjectsStore.getState().fetchProjects()

    expect(daemonCliGet).toHaveBeenCalledWith('projects/list')
    expect(daemonCliGet).toHaveBeenCalledWith('workspaces/list', { project_id: 'p1' })
    expect(daemonCliGet).not.toHaveBeenCalledWith('sections/list', expect.anything())

    const projects = useProjectsStore.getState().projects as ProjectWithWorkspaces[]
    expect(projects).toHaveLength(1)
    expect(projects[0].id).toBe('p1')
    expect(projects[0].workspaces).toHaveLength(1)
  })

  it('renameProject POSTs projects/update (camelCase) and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValue([]) // the refetch

    await useProjectsStore.getState().renameProject('p1', 'New Name')

    expect(daemonCliPost).toHaveBeenCalledWith('projects/update', { id: 'p1', name: 'New Name' })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('reorderProjects POSTs projects/reorder and emits sync:projects', async () => {
    const a = mkProject('a') as unknown as ProjectWithWorkspaces
    const b = mkProject('b') as unknown as ProjectWithWorkspaces
    a.tabOrder = 0
    b.tabOrder = 1
    a.workspaces = []
    b.workspaces = []
    useProjectsStore.setState({ projects: [a, b] })

    daemonCliPost.mockResolvedValueOnce({ success: true })

    await useProjectsStore.getState().reorderProjects(['b', 'a'])

    expect(daemonCliPost).toHaveBeenCalledWith('projects/reorder', { ids: ['b', 'a'] })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('reorderProjects optimistically reorders local projects and does NOT fetchProjects on success', async () => {
    const a = mkProject('a') as unknown as ProjectWithWorkspaces
    const b = mkProject('b') as unknown as ProjectWithWorkspaces
    const c = mkProject('c') as unknown as ProjectWithWorkspaces
    a.tabOrder = 0
    b.tabOrder = 1
    c.tabOrder = 2
    for (const p of [a, b, c]) {
      p.workspaces = []
    }
    useProjectsStore.setState({ projects: [a, b, c] })

    let resolvePost!: (v: unknown) => void
    daemonCliPost.mockImplementationOnce(
      () => new Promise((resolve) => { resolvePost = resolve }),
    )
    const fetchSpy = vi.spyOn(useProjectsStore.getState(), 'fetchProjects')

    const pending = useProjectsStore.getState().reorderProjects(['c', 'a', 'b'])

    // Paint before network resolves
    const mid = useProjectsStore.getState().projects as ProjectWithWorkspaces[]
    expect(mid.map((p) => p.id)).toEqual(['c', 'a', 'b'])
    expect(mid.map((p) => p.tabOrder)).toEqual([0, 1, 2])

    resolvePost({ success: true })
    await pending

    expect(daemonCliPost).toHaveBeenCalledWith('projects/reorder', { ids: ['c', 'a', 'b'] })
    expect(fetchSpy).not.toHaveBeenCalled()
    expect(daemonCliGet).not.toHaveBeenCalledWith('projects/list')
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
    fetchSpy.mockRestore()
  })

  it('reorderProjects rolls back local order when POST fails', async () => {
    const a = mkProject('a') as unknown as ProjectWithWorkspaces
    const b = mkProject('b') as unknown as ProjectWithWorkspaces
    a.tabOrder = 0
    b.tabOrder = 1
    a.workspaces = []
    b.workspaces = []
    useProjectsStore.setState({ projects: [a, b] })

    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    daemonCliPost.mockRejectedValueOnce(new Error('reorder boom'))

    await useProjectsStore.getState().reorderProjects(['b', 'a'])

    const after = useProjectsStore.getState().projects as ProjectWithWorkspaces[]
    expect(after.map((p) => p.id)).toEqual(['a', 'b'])
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
    expect(errSpy).toHaveBeenCalled()
    errSpy.mockRestore()
  })

  it('setProjectColor optimistically patches color and does NOT fetchProjects on success', async () => {
    const p = mkProject('p-color') as unknown as ProjectWithWorkspaces
    p.color = '#ffffff'
    p.workspaces = []
    useProjectsStore.setState({ projects: [p] })

    let resolvePost!: (v: unknown) => void
    daemonCliPost.mockImplementationOnce(
      () => new Promise((resolve) => { resolvePost = resolve }),
    )
    const fetchSpy = vi.spyOn(useProjectsStore.getState(), 'fetchProjects')

    const pending = useProjectsStore.getState().setProjectColor('p-color', '#ef4444')

    // Immediate paint
    expect((useProjectsStore.getState().projects as ProjectWithWorkspaces[])[0].color).toBe('#ef4444')

    resolvePost({ success: true })
    await pending

    expect(daemonCliPost).toHaveBeenCalledWith('projects/update', {
      id: 'p-color',
      color: '#ef4444',
    })
    expect(fetchSpy).not.toHaveBeenCalled()
    expect(daemonCliGet).not.toHaveBeenCalledWith('projects/list')
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
    expect((useProjectsStore.getState().projects as ProjectWithWorkspaces[])[0].color).toBe('#ef4444')
    fetchSpy.mockRestore()
  })

  it('setProjectColor rolls back previous color when POST fails', async () => {
    const p = mkProject('p-color-fail') as unknown as ProjectWithWorkspaces
    p.color = '#3b82f6'
    p.workspaces = []
    useProjectsStore.setState({ projects: [p] })

    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    daemonCliPost.mockRejectedValueOnce(new Error('color boom'))

    await useProjectsStore.getState().setProjectColor('p-color-fail', '#22c55e')

    expect((useProjectsStore.getState().projects as ProjectWithWorkspaces[])[0].color).toBe('#3b82f6')
    expect(emitMock).not.toHaveBeenCalledWith('sync:projects')
    expect(errSpy).toHaveBeenCalled()
    errSpy.mockRestore()
  })

  it('scheduleProjectsRefreshFromSync debounces fetchProjects (~150ms trailing)', async () => {
    vi.useFakeTimers()
    daemonCliGet.mockResolvedValue([])
    const fetchSpy = vi.spyOn(useProjectsStore.getState(), 'fetchProjects')

    scheduleProjectsRefreshFromSync()
    scheduleProjectsRefreshFromSync()
    scheduleProjectsRefreshFromSync()
    expect(fetchSpy).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(149)
    expect(fetchSpy).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(2)
    expect(fetchSpy).toHaveBeenCalledTimes(1)
    fetchSpy.mockRestore()
  })

  it('scheduleProjectsRefreshFromSync suppresses self-echo after optimistic mutation success', async () => {
    vi.useFakeTimers()
    daemonCliGet.mockResolvedValue([])

    const p = mkProject('p-echo') as unknown as ProjectWithWorkspaces
    p.color = '#fff'
    p.workspaces = []
    useProjectsStore.setState({ projects: [p] })
    daemonCliPost.mockResolvedValueOnce({ success: true })

    await useProjectsStore.getState().setProjectColor('p-echo', '#a855f7')

    const fetchSpy = vi.spyOn(useProjectsStore.getState(), 'fetchProjects')
    // Daemon / Tauri self-echo immediately after optimistic success
    scheduleProjectsRefreshFromSync()

    await vi.advanceTimersByTimeAsync(150)
    // Still inside the 500ms suppress window — no refetch
    expect(fetchSpy).not.toHaveBeenCalled()

    // After suppress window (+ debounce residue), the pending event may fire once
    await vi.advanceTimersByTimeAsync(500)
    // Peer reconcile: at most one fetch after the suppress window ends
    expect(fetchSpy.mock.calls.length).toBeLessThanOrEqual(1)
    fetchSpy.mockRestore()
  })

  it('removeProject POSTs workspace-layouts/delete + projects/delete and emits sync:projects', async () => {
    daemonCliPost.mockResolvedValue({ success: true })
    daemonCliGet.mockResolvedValue([])

    await useProjectsStore.getState().removeProject('p1')

    expect(daemonCliPost).toHaveBeenCalledWith('workspace-layouts/delete', { projectId: 'p1', workspaceId: null })
    expect(daemonCliPost).toHaveBeenCalledWith('projects/delete', { id: 'p1' })
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
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
