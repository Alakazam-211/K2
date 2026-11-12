// @vitest-environment jsdom
//
// A project list that fails to load (the daemon's project-group/list
// errors) says so in the canvas, with the server's reason and Try again,
// and the nav says "Couldn't load projects". Before, both showed
// "Loading projects…" forever: the store kept `groupsError` but nothing
// read it.

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
  default: ({ children }: { children?: React.ReactNode }) => <div data-testid="top-bar-utilities">{children}</div>,
}))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => null }))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'member') }))
vi.mock('./ProjectDashboard', () => ({ default: () => <div data-testid="dashboard-stub" /> }))
vi.mock('./DashboardPresetsMenu', () => ({ default: () => <div data-testid="presets-stub" /> }))
vi.mock('@/components/Feedback/ProjectFeedbackTab', () => ({ ProjectFeedbackTab: () => null }))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async () => { throw new Error('unused') }),
    fetchProjectGroupMessages: vi.fn(async () => ({ messages: [], truncated: false })),
  }
})

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useUnreadStore } from '@/stores/unread'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const fetchGroups = vi.fn(async () => {})

beforeEach(() => {
  fetchGroups.mockClear()
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({
    groups: null,
    groupsError: 'daemon unreachable: connection refused',
    selectedGroupId: null,
    chatCollapsed: true,
    navCollapsed: false,
    fetchGroups,
  })
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: true })
})

afterEach(() => {
  cleanup()
})

describe('Projects page: a project list that fails to load', () => {
  it('shows the failure in the canvas and the nav, not Loading forever', () => {
    render(<ProjectsPage />)
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toContain("Couldn't load your projects")
    expect(alert.textContent).toContain('Your agents are fine.')
    expect(alert.textContent).toContain('daemon unreachable: connection refused')
    expect(screen.getByText("Couldn't load projects")).toBeTruthy()
    expect(screen.queryByText('Loading projects…')).toBeNull()
  })

  it('Try again fetches the list again', async () => {
    render(<ProjectsPage />)
    await waitFor(() => expect(fetchGroups).toHaveBeenCalled())
    const before = fetchGroups.mock.calls.length
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(fetchGroups.mock.calls.length).toBe(before + 1)
  })

  it('still says Loading while the first fetch is in flight (no error yet)', () => {
    useProjectGroupsStore.setState({ groupsError: null })
    render(<ProjectsPage />)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getAllByText('Loading projects…').length).toBe(2)
  })
})
