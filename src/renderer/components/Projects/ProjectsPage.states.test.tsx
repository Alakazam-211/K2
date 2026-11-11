// @vitest-environment jsdom
//
// The Projects page's load, empty and failure states (0.46.12):
//   - a failed project list shows the state card with the server's reason
//     and Try again (it used to say "Loading projects…" for ever), and Try
//     again reads the list again;
//   - no projects: one empty state on the canvas with one New project
//     button; the nav keeps only its header and its own New Project;
//   - a project that fails to load: the state card, Try again reads it again;
//   - a project with no dashboard: an owner or admin can create one from
//     the page (`project-group/dashboard/create`), anyone else is told who can.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <div>{children}</div>,
}))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => null }))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
const role = vi.hoisted(() => ({ current: 'owner' as 'owner' | 'admin' | 'member' }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => role.current) }))
vi.mock('./ProjectDashboard', () => ({ default: () => <div data-testid="dashboard-stub" /> }))
vi.mock('./DashboardPresetsMenu', () => ({ default: () => null }))
vi.mock('@/components/Feedback/ProjectFeedbackTab', () => ({ ProjectFeedbackTab: () => null }))
const api = vi.hoisted(() => ({
  list: vi.fn(),
  show: vi.fn(),
  createDashboard: vi.fn(),
}))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroups: api.list,
    fetchProjectGroupShow: api.show,
    createProjectGroupDashboard: api.createDashboard,
    fetchProjectGroupMessages: vi.fn(async () => ({ messages: [], truncated: false })),
  }
})

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { emptyDashboardHint } from './ProjectStates'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useUnreadStore } from '@/stores/unread'
import type { ProjectGroup, ProjectGroupShow } from './projects-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

const realFetchGroups = useProjectGroupsStore.getState().fetchGroups

const GROUP: ProjectGroup = {
  id: 'g1',
  name: 'Launch',
  pocWorkspaceId: null,
  pinned: false,
  color: null,
  sortOrder: 0,
  createdAt: 1,
  updatedAt: 1,
  memberCount: 0,
  memberWorkspaceIds: [],
}
const MAIN = { id: 'd1', groupId: 'g1', name: 'Main', layoutJson: '{"version":1,"panes":[]}', revision: 1, position: 0, createdAt: 1, updatedAt: 1 }
const SHOW: ProjectGroupShow = { ...GROUP, members: [], dashboards: [MAIN] }

beforeEach(() => {
  role.current = 'owner'
  api.list.mockReset()
  api.show.mockReset()
  api.createDashboard.mockReset()
  api.list.mockResolvedValue([GROUP])
  api.show.mockResolvedValue(SHOW)
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({
    groups: null,
    groupsError: null,
    selectedGroupId: null,
    chatCollapsed: true,
    navCollapsed: false,
    fetchGroups: realFetchGroups,
  })
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: true })
})

afterEach(() => {
  cleanup()
})

describe('the project list', () => {
  it('a failed read shows the error card with the reason (never endless Loading), and Try again loads the list', async () => {
    api.list.mockRejectedValueOnce(new Error('database is locked'))
    render(<ProjectsPage />)
    const card = await screen.findByTestId('projects-list-error')
    expect(card.getAttribute('role')).toBe('alert')
    expect(card.textContent).toContain('Couldn’t load your projects')
    expect(card.textContent).toContain('The server said: database is locked.')
    expect(screen.queryByText(/Loading/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(document.querySelector('[data-group-id="g1"]')).toBeTruthy())
    expect(screen.queryByTestId('projects-list-error')).toBeNull()
    expect(api.list.mock.calls.length).toBeGreaterThanOrEqual(2)
  })

  it('while the list loads: one quiet loading card on the canvas, nothing doubled in the nav', async () => {
    api.list.mockReturnValue(new Promise(() => {}))
    render(<ProjectsPage />)
    const card = await screen.findByTestId('projects-list-loading')
    expect(card.getAttribute('aria-busy')).toBe('true')
    expect(screen.getAllByText(/Loading/)).toHaveLength(1)
  })

  it('no projects: one empty state and one New project button on the canvas; the form opens in place', async () => {
    api.list.mockResolvedValue([])
    render(<ProjectsPage />)
    const empty = await screen.findByTestId('projects-empty')
    expect(screen.getAllByText('No projects yet')).toHaveLength(1)
    expect(empty.textContent).not.toContain('—')
    const create = screen.getByRole('button', { name: 'New project' })
    expect(create.className).toContain('bg-[var(--color-accent-fill)]')
    expect(create.className).not.toContain('bg-white/')
    fireEvent.click(create)
    expect(screen.getByRole('textbox', { name: 'Project name' })).toBe(document.activeElement)
  })
})

describe('one project', () => {
  it('a failed read shows the error card, and Try again reads the project again', async () => {
    api.show.mockRejectedValueOnce(new Error('database is locked'))
    render(<ProjectsPage />)
    const card = await screen.findByTestId('project-load-error')
    expect(card.textContent).toContain('Couldn’t open this project')
    expect(card.textContent).toContain('The server said: database is locked.')
    expect(card.textContent).not.toContain('Failed to load project')
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
    expect(api.show).toHaveBeenCalledTimes(2)
  })

  it('no dashboard: an owner creates one from the page, and the page shows it', async () => {
    api.show.mockResolvedValueOnce({ ...SHOW, dashboards: [] })
    api.createDashboard.mockResolvedValue(MAIN)
    render(<ProjectsPage />)
    const state = await screen.findByTestId('project-no-dashboard')
    expect(state.textContent).toContain('This project has no dashboard yet')
    const create = await screen.findByRole('button', { name: 'Create a dashboard' })
    fireEvent.click(create)
    await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
    expect(api.createDashboard).toHaveBeenCalledWith(expect.anything(), 'g1', 'Main')
  })

  it('no dashboard, as a member: no button, and the page says who can add one', async () => {
    role.current = 'member'
    api.show.mockResolvedValue({ ...SHOW, dashboards: [] })
    render(<ProjectsPage />)
    const state = await screen.findByTestId('project-no-dashboard')
    await waitFor(() => expect(state.textContent).toContain('Only the project’s owner or an admin can add one.'))
    expect(screen.queryByRole('button', { name: 'Create a dashboard' })).toBeNull()
    // The Feedback tab still opens from the tab row.
    fireEvent.click(screen.getByRole('button', { name: 'Feedback' }))
    await waitFor(() => expect(screen.queryByTestId('project-no-dashboard')).toBeNull())
  })

  it('a refused create says so with the reason, and changes nothing', async () => {
    api.show.mockResolvedValue({ ...SHOW, dashboards: [] })
    api.createDashboard.mockRejectedValue(new Error('only the owner or an admin can do that'))
    render(<ProjectsPage />)
    fireEvent.click(await screen.findByRole('button', { name: 'Create a dashboard' }))
    const card = await screen.findByTestId('project-dashboard-create-error')
    expect(card.textContent).toContain('Couldn’t create the dashboard')
    expect(card.textContent).toContain('The server said: only the owner or an admin can do that.')
  })
})

describe('an empty dashboard', () => {
  it('says what to do next, in plain words', () => {
    expect(emptyDashboardHint(3, false)).toBe('Drag an agent from Members onto this space, or click one to open it.')
    expect(emptyDashboardHint(0, false)).toContain('Add an agent to this project')
    expect(emptyDashboardHint(3, true)).not.toMatch(/drag|click/i)
    for (const s of [emptyDashboardHint(3, false), emptyDashboardHint(0, false), emptyDashboardHint(3, true)]) {
      expect(s).not.toMatch(/PoC|drawer|pane|—/)
    }
  })
})
