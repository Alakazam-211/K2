// @vitest-environment jsdom
//
// The way into project settings from the Projects page (0.46.1 dead
// ends): right-clicking a project — in the expanded nav or the collapsed
// rail — offers the SAME menu, and its "Project Settings" opens
// Settings → Projects with that project selected.

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
const menu = vi.hoisted(() => ({ pick: 'group-settings' as string | null }))
vi.mock('@/lib/context-menu', () => ({
  showContextMenu: vi.fn(async () => menu.pick),
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
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))
vi.mock('./ProjectDashboard', () => ({ default: () => <div data-testid="dashboard-stub" /> }))
vi.mock('./DashboardPresetsMenu', () => ({ default: () => null }))
vi.mock('@/components/Feedback/ProjectFeedbackTab', () => ({ ProjectFeedbackTab: () => null }))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async () => SHOW),
    fetchProjectGroupMessages: vi.fn(async () => ({ messages: [], truncated: false })),
  }
})

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { showContextMenu } from '@/lib/context-menu'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useSettingsStore } from '@/stores/settings'
import { useUnreadStore } from '@/stores/unread'
import type { ProjectGroup, ProjectGroupShow } from './projects-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

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
const SHOW: ProjectGroupShow = {
  ...GROUP,
  members: [],
  dashboards: [
    { id: 'd1', groupId: 'g1', name: 'Main', layoutJson: '{"version":1,"panes":[]}', revision: 1, position: 0, createdAt: 1, updatedAt: 1 },
  ],
}

const openSettings = vi.fn()

beforeEach(() => {
  menu.pick = 'group-settings'
  vi.mocked(showContextMenu).mockClear()
  openSettings.mockClear()
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({
    groups: [GROUP],
    groupsError: null,
    selectedGroupId: 'g1',
    chatCollapsed: true,
    navCollapsed: false,
    fetchGroups: vi.fn(async () => {}),
  })
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: true })
  useSettingsStore.setState({ openSettings })
})

afterEach(() => {
  cleanup()
})

function menuLabels(): string[] {
  const items = vi.mocked(showContextMenu).mock.calls[0][0] as Array<{ label: string }>
  return items.map((i) => i.label)
}

describe('Project Settings from the Projects page', () => {
  it('right-click on a nav row → Project Settings opens Settings → Projects on that project', async () => {
    render(<ProjectsPage />)
    await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
    const row = document.querySelector('[data-group-id="g1"] button') as HTMLElement
    expect(row).toBeTruthy()
    fireEvent.contextMenu(row)
    await waitFor(() => expect(openSettings).toHaveBeenCalledWith('project-groups', 'g1'))
    expect(menuLabels()[0]).toBe('Project Settings')
  })

  it('the collapsed rail offers the same menu, and it works the same way', async () => {
    useProjectGroupsStore.setState({ navCollapsed: true })
    render(<ProjectsPage />)
    await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
    const icon = screen.getByTitle('Launch • 0 members')
    fireEvent.contextMenu(icon)
    await waitFor(() => expect(openSettings).toHaveBeenCalledWith('project-groups', 'g1'))
    const railLabels = menuLabels()
    vi.mocked(showContextMenu).mockClear()
    useProjectGroupsStore.setState({ navCollapsed: false })
    cleanup()
    render(<ProjectsPage />)
    await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
    menu.pick = null
    fireEvent.contextMenu(document.querySelector('[data-group-id="g1"] button') as HTMLElement)
    await waitFor(() => expect(showContextMenu).toHaveBeenCalledTimes(1))
    expect(menuLabels()).toEqual(railLabels)
  })
})
