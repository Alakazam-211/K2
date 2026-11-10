// @vitest-environment jsdom

// Deleting a focus group asks first, from both of its menus, and says what
// happens to the workspaces in it. Cancel leaves the group alone.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const showContextMenu = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})), settingsReset: vi.fn() }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))
vi.mock('@/lib/context-menu', () => ({ showContextMenu }))

import { ProjectsSection } from './ProjectsSection'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'

const confirm = vi.fn()
const deleteFocusGroup = vi.fn(async () => {})
const fetchProjects = vi.fn(async () => {})
const realConfirm = useConfirmDialogStore.getState().confirm
const realDelete = useFocusGroupsStore.getState().deleteFocusGroup
const realFetch = useProjectsStore.getState().fetchProjects

function workspace(id: string, focusGroupId: string | null): Record<string, unknown> {
  return { id, name: id, path: `/w/${id}`, color: '#3b82f6', workspaces: [], pinned: 0, focusGroupId }
}

function seed(projects: Record<string, unknown>[]): void {
  useProjectsStore.setState({ projects, fetchProjects } as never)
}

beforeEach(() => {
  vi.clearAllMocks()
  useSettingsStore.setState({ initialProjectId: null, projectsPage: null, initialWorkspaceTab: null })
  useConfirmDialogStore.setState({ confirm } as never)
  useFocusGroupsStore.setState({
    focusGroups: [{ id: 'g1', name: 'Labs', color: null, tabOrder: 0 }],
    focusGroupsEnabled: true,
    deleteFocusGroup,
  } as never)
  seed([workspace('a', 'g1'), workspace('b', 'g1')])
})

afterEach(() => {
  cleanup()
  useConfirmDialogStore.setState({ confirm: realConfirm } as never)
  useFocusGroupsStore.setState({ deleteFocusGroup: realDelete, focusGroups: [], focusGroupsEnabled: false } as never)
  useProjectsStore.setState({ projects: [], fetchProjects: realFetch } as never)
})

async function deleteFromGroupMenu(): Promise<void> {
  render(<ProjectsSection />)
  fireEvent.click(screen.getByRole('button', { name: 'More actions for Labs' }))
  await act(async () => {
    fireEvent.click(await screen.findByRole('menuitem', { name: /Delete group/ }))
  })
}

async function deleteFromRightClick(): Promise<void> {
  showContextMenu.mockResolvedValue('delete')
  render(<ProjectsSection />)
  const header = document.querySelector('[data-focus-group-id="g1"]')
  if (!header) throw new Error('group header not rendered')
  await act(async () => {
    fireEvent.contextMenu(header)
  })
}

describe('Delete focus group', () => {
  it('names the group and says its workspaces move to Ungrouped', async () => {
    confirm.mockResolvedValue(false)
    await deleteFromGroupMenu()
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    const opts = confirm.mock.calls[0][0] as { title: string; message: string; confirmLabel: string; destructive: boolean }
    expect(opts.title).toContain('Labs')
    expect(opts.message).toBe('Its 2 workspaces aren’t removed. They move to Ungrouped.')
    expect(opts.confirmLabel).toBe('Delete group')
    expect(opts.destructive).toBe(true)
  })

  it('counts one workspace, and an empty group, in plain words', async () => {
    confirm.mockResolvedValue(false)
    seed([workspace('a', 'g1'), workspace('c', null)])
    await deleteFromGroupMenu()
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect((confirm.mock.calls[0][0] as { message: string }).message).toBe('Its 1 workspace isn’t removed. It moves to Ungrouped.')
    cleanup()
    confirm.mockClear()
    seed([workspace('c', null)])
    await deleteFromGroupMenu()
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect((confirm.mock.calls[0][0] as { message: string }).message).toBe('It has no workspaces in it.')
  })

  it('from the group menu: confirm deletes and refreshes the list', async () => {
    confirm.mockResolvedValue(true)
    await deleteFromGroupMenu()
    await waitFor(() => expect(deleteFocusGroup).toHaveBeenCalledWith('g1'))
    await waitFor(() => expect(fetchProjects).toHaveBeenCalled())
  })

  it('from the group menu: cancel changes nothing', async () => {
    confirm.mockResolvedValue(false)
    await deleteFromGroupMenu()
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(deleteFocusGroup).not.toHaveBeenCalled()
    expect(fetchProjects).not.toHaveBeenCalled()
  })

  it('from right-click: confirm deletes', async () => {
    confirm.mockResolvedValue(true)
    await deleteFromRightClick()
    await waitFor(() => expect(deleteFocusGroup).toHaveBeenCalledWith('g1'))
    expect(confirm).toHaveBeenCalledTimes(1)
  })

  it('from right-click: cancel changes nothing', async () => {
    confirm.mockResolvedValue(false)
    await deleteFromRightClick()
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(deleteFocusGroup).not.toHaveBeenCalled()
  })
})
