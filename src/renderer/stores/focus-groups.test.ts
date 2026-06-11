// Plan B (Bulk-1) — vitest coverage for the focus-groups store after
// migrating its DB-backed actions OFF the Tauri `focus_groups_*` invoke
// proxy ONTO the host-aware `daemonCli*` HTTP layer.
//
// What this asserts:
//   - fetchFocusGroups   → GET  `focus-groups/list`   (no params)
//   - createFocusGroup   → POST `focus-groups/create` + emits sync:focus-groups
//   - deleteFocusGroup   → POST `focus-groups/delete` + emits sync:focus-groups
//   - renameFocusGroup   → POST `focus-groups/update` (camelCase body) + emit
//   - updateFocusGroupColor → POST `focus-groups/update` + emit
//   - reorderFocusGroups → POST `focus-groups/update` per id + ONE emit
//   - assignProjectToGroup → POST `focus-groups/assign` + emits BOTH
//                            sync:focus-groups AND sync:projects
//   - a failed mutation does NOT emit sync
//
// The store has an import-time side effect (`initFromSettings()` →
// `settingsGet()` → `fetchFocusGroups()`), so every dependency is mocked
// via hoisted `vi.mock` BEFORE the store import.

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

// ── Mock daemon-settings so the import-time init resolves cleanly ────────
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => Promise.resolve({})),
  settingsUpdate: vi.fn(() => Promise.resolve({})),
}))

// ── Mock the daemon-reconnect bus (no-op listener registration) ──────────
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// ── Toast spy ────────────────────────────────────────────────────────────
const addToast = vi.fn()
vi.mock('./toast', () => ({
  useToastStore: { getState: () => ({ addToast }) },
}))

// ── Projects store (setActiveFocusGroup's auto-activation reads it) ──────
const setActiveWorkspaceSpy = vi.fn()
let mockProjects: Array<{
  id: string
  focusGroupId: string | null
  workspaces: Array<{ id: string }>
}> = []
vi.mock('./projects', () => ({
  useProjectsStore: {
    getState: () => ({ projects: mockProjects, setActiveWorkspace: setActiveWorkspaceSpy }),
  },
}))

import { useFocusGroupsStore, type FocusGroup } from './focus-groups'

function resetStore(): void {
  useFocusGroupsStore.setState({
    focusGroups: [],
    activeFocusGroupId: null,
    focusGroupsEnabled: false,
  })
}

describe('focus-groups store — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
    emitMock.mockClear()
    addToast.mockClear()
    resetStore()
  })

  it('fetchFocusGroups GETs focus-groups/list (no params)', async () => {
    const rows: FocusGroup[] = [
      { id: 'fg1', name: 'A', color: null, tabOrder: 0, createdAt: 1 },
    ]
    daemonCliGet.mockResolvedValueOnce(rows)

    await useFocusGroupsStore.getState().fetchFocusGroups()

    expect(daemonCliGet).toHaveBeenCalledTimes(1)
    expect(daemonCliGet).toHaveBeenCalledWith('focus-groups/list')
    expect(useFocusGroupsStore.getState().focusGroups).toEqual(rows)
  })

  it('createFocusGroup POSTs focus-groups/create and emits sync:focus-groups', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValueOnce([]) // the follow-up fetch

    await useFocusGroupsStore.getState().createFocusGroup('Work', '#fff')

    expect(daemonCliPost).toHaveBeenCalledWith('focus-groups/create', { name: 'Work', color: '#fff' })
    expect(emitMock).toHaveBeenCalledWith('sync:focus-groups')
    expect(addToast).toHaveBeenCalledWith('Focus group created', 'success')
  })

  it('deleteFocusGroup POSTs focus-groups/delete and emits sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValueOnce([])

    await useFocusGroupsStore.getState().deleteFocusGroup('fg1')

    expect(daemonCliPost).toHaveBeenCalledWith('focus-groups/delete', { id: 'fg1' })
    expect(emitMock).toHaveBeenCalledWith('sync:focus-groups')
  })

  it('renameFocusGroup POSTs focus-groups/update (camelCase) and emits sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValueOnce([])

    await useFocusGroupsStore.getState().renameFocusGroup('fg1', 'Renamed')

    expect(daemonCliPost).toHaveBeenCalledWith('focus-groups/update', { id: 'fg1', name: 'Renamed' })
    expect(emitMock).toHaveBeenCalledWith('sync:focus-groups')
  })

  it('updateFocusGroupColor POSTs focus-groups/update and emits sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    daemonCliGet.mockResolvedValueOnce([])

    await useFocusGroupsStore.getState().updateFocusGroupColor('fg1', '#abc')

    expect(daemonCliPost).toHaveBeenCalledWith('focus-groups/update', { id: 'fg1', color: '#abc' })
    expect(emitMock).toHaveBeenCalledWith('sync:focus-groups')
  })

  it('reorderFocusGroups POSTs focus-groups/update per id with tabOrder and emits sync once', async () => {
    daemonCliPost.mockResolvedValue({ success: true })
    daemonCliGet.mockResolvedValueOnce([])

    await useFocusGroupsStore.getState().reorderFocusGroups(['a', 'b', 'c'])

    expect(daemonCliPost).toHaveBeenNthCalledWith(1, 'focus-groups/update', { id: 'a', tabOrder: 0 })
    expect(daemonCliPost).toHaveBeenNthCalledWith(2, 'focus-groups/update', { id: 'b', tabOrder: 1 })
    expect(daemonCliPost).toHaveBeenNthCalledWith(3, 'focus-groups/update', { id: 'c', tabOrder: 2 })
    expect(emitMock.mock.calls.filter((c) => c[0] === 'sync:focus-groups')).toHaveLength(1)
  })

  it('assignProjectToGroup POSTs focus-groups/assign and emits BOTH sync channels', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })

    await useFocusGroupsStore.getState().assignProjectToGroup('proj-1', 'fg1')

    expect(daemonCliPost).toHaveBeenCalledWith('focus-groups/assign', {
      projectId: 'proj-1',
      focusGroupId: 'fg1',
    })
    expect(emitMock).toHaveBeenCalledWith('sync:focus-groups')
    expect(emitMock).toHaveBeenCalledWith('sync:projects')
  })

  it('a failed mutation does NOT emit sync', async () => {
    daemonCliPost.mockRejectedValueOnce(new Error('daemon down'))

    await useFocusGroupsStore.getState().createFocusGroup('Work')

    expect(emitMock).not.toHaveBeenCalledWith('sync:focus-groups')
  })
})

// ── setActiveFocusGroup auto-activation (0.39.47 wrong-session fix) ──────
//
// Switching the group auto-activates its first workspace — the right UX
// for clicking a GROUP, and exactly wrong for callers (ActiveBar, command
// palette, shortcuts, review-jump) that activate a SPECIFIC project
// themselves: the nested setActiveWorkspace made those entry points a
// DOUBLE workspace switch whose racing deferred pinned-tab resolutions
// rendered another workspace's session in the pinned tab.
describe('setActiveFocusGroup auto-activation', () => {
  beforeEach(() => {
    setActiveWorkspaceSpy.mockClear()
    mockProjects = [
      { id: 'proj-first', focusGroupId: 'fg-1', workspaces: [{ id: 'ws-first' }] },
      { id: 'proj-clicked', focusGroupId: 'fg-1', workspaces: [{ id: 'ws-clicked' }] },
    ]
  })

  it('auto-activates the first workspace in the group by default (group-tab UX)', () => {
    useFocusGroupsStore.getState().setActiveFocusGroup('fg-1')
    expect(setActiveWorkspaceSpy).toHaveBeenCalledTimes(1)
    expect(setActiveWorkspaceSpy).toHaveBeenCalledWith('proj-first', 'ws-first')
  })

  it('does NOT auto-activate with { autoActivate: false } — the caller activates explicitly', () => {
    useFocusGroupsStore.getState().setActiveFocusGroup('fg-1', { autoActivate: false })
    expect(setActiveWorkspaceSpy).not.toHaveBeenCalled()
    // The group itself still switched.
    expect(useFocusGroupsStore.getState().activeFocusGroupId).toBe('fg-1')
  })

  it('clearing the group (null) never auto-activates', () => {
    useFocusGroupsStore.getState().setActiveFocusGroup(null)
    expect(setActiveWorkspaceSpy).not.toHaveBeenCalled()
  })
})
