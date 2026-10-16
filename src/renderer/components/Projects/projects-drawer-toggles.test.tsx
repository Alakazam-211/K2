// @vitest-environment jsdom
//
// Drawer toggles on the Projects page (0.46.0): no nav / chat toggles in
// the top bar. The chat panel's » / « sits in its own slot at the far right
// of the dashboard tab row while closed (with the unread dot) and in the
// chat header, at the end nearest the dashboard, while open. The nav (which
// collapses to a rail, never fully hides) gets the main nav's pattern: the
// toggle at the bottom of the nav and of the rail.

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
const viewerRole = vi.hoisted(() => ({ current: 'member' as 'member' | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => viewerRole.current) }))
vi.mock('./ProjectDashboard', () => ({ default: () => <div data-testid="dashboard-stub" /> }))
vi.mock('./DashboardPresetsMenu', () => ({ default: () => <div data-testid="presets-stub" /> }))
vi.mock('@/components/Feedback/ProjectFeedbackTab', () => ({ ProjectFeedbackTab: () => null }))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async () => SHOW),
    fetchProjectGroupMessages: vi.fn(async () => ({ messages: [], truncated: false })),
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { ProjectNavCollapseButton } from './ProjectNav'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
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
  dashboards: [{ id: 'd1', name: 'Main', revision: 1, sortOrder: 0 } as unknown as ProjectGroupShow['dashboards'][number]],
}

function chatToggle(): HTMLElement | null {
  return document.querySelector('[data-drawer-tabbar-toggle="right"]')
}

function must(el: HTMLElement | null, what: string): HTMLElement {
  if (!el) throw new Error(`missing ${what}`)
  return el
}

async function renderPage(): Promise<void> {
  render(<ProjectsPage />)
  await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
}

beforeEach(() => {
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({
    groups: [GROUP],
    selectedGroupId: 'g1',
    chatCollapsed: true,
    navCollapsed: false,
    unreadGroupIds: new Set<string>(),
    fetchGroups: vi.fn(async () => {}),
  })
})

afterEach(() => {
  cleanup()
})

describe('Projects top bar', () => {
  it('has no nav or chat toggles', async () => {
    await renderPage()
    const utilities = screen.getByTestId('top-bar-utilities')
    expect(utilities.childElementCount).toBe(0)
    expect(screen.queryByTitle('Toggle projects nav')).toBeNull()
    expect(screen.queryByTitle('Toggle project chat')).toBeNull()
  })
})

describe('Projects chat drawer toggle', () => {
  it('closed: « in its own slot at the far right of the dashboard tab row', async () => {
    await renderPage()
    const btn = must(chatToggle(), 'chat toggle')
    expect(btn.getAttribute('aria-label')).toBe('Show project chat')
    expect(btn.getAttribute('data-placement')).toBe('room')
    expect(btn.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('left')
    const slot = must(btn.closest('[data-project-chat-toggle-slot]'), 'slot')
    expect(slot.parentElement!.lastElementChild).toBe(slot)
    // After the layout-presets menu: the row's far right.
    expect(slot.previousElementSibling).toBe(screen.getByTestId('presets-stub'))
    expect(btn.querySelector('[data-project-chat-unread]')).toBeNull()
  })

  it('closed with unread messages: the dot rides the toggle', async () => {
    useProjectGroupsStore.setState({ unreadGroupIds: new Set(['g1']) })
    await renderPage()
    expect(must(chatToggle(), 'chat toggle').querySelector('[data-project-chat-unread]')).not.toBeNull()
  })

  it('open: » at the bottom left, across from Send; clicking it closes the chat', async () => {
    await renderPage()
    fireEvent.click(must(chatToggle(), 'chat toggle'))
    expect(useProjectGroupsStore.getState().chatCollapsed).toBe(false)
    expect(document.querySelector('[data-project-chat-toggle-slot]')).toBeNull()
    // The composer shows once the member role resolves.
    await waitFor(() => expect(screen.getByText('Send')).toBeTruthy())
    const hide = must(screen.queryByRole('button', { name: 'Hide chat' }), 'hide toggle')
    expect(hide.getAttribute('title')).toBe('Hide chat')
    expect(hide.getAttribute('data-placement')).toBe('drawer')
    expect(hide.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('right')
    // Same row as Send: Hide first (left), Send last (right).
    const row = must(hide.closest('[data-project-chat-actions]') as HTMLElement | null, 'composer action row')
    expect(row.firstElementChild).toBe(hide)
    expect(row.lastElementChild!.textContent).toContain('Send')
    expect(row.className).toContain('justify-between')
    // Not in the chat header any more.
    const header = must(screen.getByText('Project chat').parentElement, 'chat header')
    expect(header.querySelector('[data-drawer-tabbar-toggle]')).toBeNull()
    // The chat sits in the sliding drawer box.
    expect(hide.closest('[data-drawer-side="right"]')).not.toBeNull()

    fireEvent.click(hide)
    expect(useProjectGroupsStore.getState().chatCollapsed).toBe(true)
    expect(screen.queryByRole('button', { name: 'Hide chat' })).toBeNull()
    expect(chatToggle()).not.toBeNull()
  })

  it('viewers (no composer) still get the Hide chat button at the bottom', async () => {
    viewerRole.current = null
    try {
      useProjectGroupsStore.setState({ chatCollapsed: false })
      await renderPage()
      const hide = await waitFor(() => must(screen.queryByRole('button', { name: 'Hide chat' }), 'hide toggle'))
      await waitFor(() => expect(screen.getByText(/Viewers can read project chat/)).toBeTruthy())
      expect(hide.parentElement!.firstElementChild).toBe(hide)
      expect(screen.queryByText('Send')).toBeNull()
    } finally {
      viewerRole.current = 'member'
    }
  })
})

describe('Projects nav toggle (main nav pattern)', () => {
  it('collapses and opens the nav from its own » / « button', () => {
    render(<ProjectNavCollapseButton />)
    const btn = screen.getByRole('button', { name: 'Collapse projects nav' })
    expect(btn.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('left')
    fireEvent.click(btn)
    expect(useProjectGroupsStore.getState().navCollapsed).toBe(true)
    const open = screen.getByRole('button', { name: 'Open projects nav' })
    expect(open.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('right')
    expect(open.getAttribute('aria-expanded')).toBe('false')
  })

  it('sits at the bottom of the expanded nav and of the rail', async () => {
    await renderPage()
    const inNav = document.querySelectorAll('[data-projects-nav-toggle]')
    expect(inNav.length).toBe(1)
    expect(inNav[0].getAttribute('aria-label')).toBe('Collapse projects nav')
    act(() => {
      fireEvent.click(inNav[0])
    })
    const inRail = document.querySelectorAll('[data-projects-nav-toggle]')
    expect(inRail.length).toBe(1)
    expect(inRail[0].getAttribute('aria-label')).toBe('Open projects nav')
    expect(inRail[0].parentElement!.className).toContain('mt-auto')
  })
})
