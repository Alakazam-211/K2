// @vitest-environment jsdom
//
// Toggles on the Projects page. 0.46.0: no nav / chat toggles in the top
// bar; the nav (which collapses to a rail, never fully hides) has its
// toggle where Home's is: « at the right end of the nav's top row, » at the
// top of the rail.
// 0.46.12 (projects-reorg-draft §3.5, the owner's mockup): the chat opens
// from the project bar's Chat button (with the unread count), which steps
// aside while the chat is open; the × in the chat's own header closes it.
// The » at the bottom left of the chat is gone.

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
import { NAV_TOGGLE_BOX } from '@/components/Layout/drawer-toggles'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
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
  dashboards: [{ id: 'd1', name: 'Main', revision: 1, sortOrder: 0 } as unknown as ProjectGroupShow['dashboards'][number]],
}

function chatButton(): HTMLElement {
  return must(document.querySelector('[data-project-chat-button]') as HTMLElement | null, 'chat button')
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
    fetchGroups: vi.fn(async () => {}),
  })
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: true })
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

describe('Project chat toggle (the project bar)', () => {
  it('closed: the bar\'s Chat button is the only chat toggle, last in the bar', async () => {
    await renderPage()
    const btn = chatButton()
    expect(btn.textContent).toContain('Chat')
    expect(btn.getAttribute('title')).toBe('Open the project chat')
    const actions = must(btn.closest('[data-project-actions]') as HTMLElement | null, 'bar actions')
    expect(actions.lastElementChild).toBe(btn)
    // No drawer » / « toggles for the chat anywhere on the page.
    expect(document.querySelector('[data-drawer-tabbar-toggle]')).toBeNull()
    expect(document.querySelector('[data-project-chat-toggle-slot]')).toBeNull()
    expect(btn.querySelector('[data-project-chat-unread]')).toBeNull()
  })

  it('closed with unread messages: the count rides the Chat button', async () => {
    useUnreadStore.setState({
      byKey: {
        'project:g1': { kind: 'project', id: 'g1', projectId: null, lastReadSeq: 1, lastSeq: 3, unread: 2, waiting: 0, muted: false, mutedUntil: null },
      },
      total: 2,
      waitingTotal: 0,
      loaded: true,
    })
    await renderPage()
    const badge = must(chatButton().querySelector('[data-project-chat-unread]') as HTMLElement | null, 'unread count')
    expect(badge.textContent).toBe('2')
  })

  it('open: the × in the chat header closes it; no » at the bottom of the chat', async () => {
    await renderPage()
    fireEvent.click(chatButton())
    expect(useProjectGroupsStore.getState().chatCollapsed).toBe(false)
    // The bar's button steps aside while the chat is open.
    expect(document.querySelector('[data-project-chat-button]')).toBeNull()
    // The composer shows once the member role resolves.
    await waitFor(() => expect(screen.getByTestId('project-chat-send')).toBeTruthy())
    expect(screen.queryByRole('button', { name: 'Hide chat' })).toBeNull()
    expect(document.querySelector('[data-project-chat-hide-slot]')).toBeNull()
    const close = screen.getByRole('button', { name: 'Close the chat' })
    // In the chat's own header, at its right end (the bar's Chat button's corner).
    const header = must(document.querySelector('[data-project-chat-header]') as HTMLElement | null, 'chat header')
    expect(header.lastElementChild).toBe(close)
    // The chat sits in the sliding drawer box.
    expect(close.closest('[data-drawer-side="right"]')).not.toBeNull()
    fireEvent.click(close)
    expect(useProjectGroupsStore.getState().chatCollapsed).toBe(true)
    expect(screen.queryByRole('button', { name: 'Close the chat' })).toBeNull()
    expect(chatButton()).toBeTruthy()
  })

  it('an open chat has no Chat button in the bar (its × closes it)', async () => {
    useProjectGroupsStore.setState({ chatCollapsed: false })
    await renderPage()
    expect(document.querySelector('[data-project-chat-button]')).toBeNull()
    expect(await screen.findByRole('button', { name: 'Close the chat' })).toBeTruthy()
  })

  it('closing slides the chat to 0 and removes it without a full-width frame', async () => {
    const realMatchMedia = window.matchMedia
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })) as typeof window.matchMedia
    try {
      useProjectGroupsStore.setState({ chatCollapsed: false })
      await renderPage()
      const close = await waitFor(() => screen.getByRole('button', { name: 'Close the chat' }))
      const el = must(close.closest('[data-drawer-side="right"]') as HTMLElement | null, 'chat drawer')
      fireEvent.click(close)
      expect(el.dataset.drawerAnim).toBe('closing')
      expect(el.style.width).toBe('0px')
      // End the slide outside act(): the end of this task is the next
      // painted frame, and the chat must not be back at full width in it.
      el.dispatchEvent(Object.assign(new Event('transitionend'), { propertyName: 'width' }))
      if (el.isConnected) expect(el.style.width).toBe('0px')
      expect(el.isConnected).toBe(false)
    } finally {
      window.matchMedia = realMatchMedia
    }
  })

  it('viewers (no composer) close it from the same × in the header', async () => {
    viewerRole.current = null
    try {
      useProjectGroupsStore.setState({ chatCollapsed: false })
      await renderPage()
      await waitFor(() => expect(screen.getByText(/View only: you can read but not post/)).toBeTruthy())
      expect(screen.getByRole('button', { name: 'Close the chat' })).toBeTruthy()
      expect(screen.queryByRole('button', { name: 'Hide chat' })).toBeNull()
      expect(screen.queryByText('Send')).toBeNull()
    } finally {
      viewerRole.current = 'member'
    }
  })
})

describe('Project settings from the bar', () => {
  it('⚙ opens this project\'s settings over the page (the sheet), focus returning to ⚙', async () => {
    useProjectGroupsStore.setState({ settingsSheet: null })
    await renderPage()
    const gear = screen.getByRole('button', { name: 'Project settings' })
    fireEvent.click(gear)
    const sheet = useProjectGroupsStore.getState().settingsSheet
    expect(sheet?.groupId).toBe('g1')
    expect(sheet?.opener).toBe(gear)
    act(() => useProjectGroupsStore.getState().closeProjectSettingsSheet())
  })
})

describe('Projects nav toggle (Home nav pattern)', () => {
  it('collapses and opens the nav from its own » / « button', () => {
    render(<ProjectNavCollapseButton />)
    const btn = screen.getByRole('button', { name: 'Collapse projects nav' })
    expect(btn.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('left')
    for (const cls of NAV_TOGGLE_BOX.split(' ')) expect(btn.classList.contains(cls)).toBe(true)
    expect(NAV_TOGGLE_BOX).toBe('h-8 w-8')
    fireEvent.click(btn)
    expect(useProjectGroupsStore.getState().navCollapsed).toBe(true)
    const open = screen.getByRole('button', { name: 'Open projects nav' })
    expect(open.querySelector('svg')!.getAttribute('data-double-chevron')).toBe('right')
    expect(open.getAttribute('aria-expanded')).toBe('false')
  })

  it('sits at the end of the expanded nav\'s top row and at the top of the rail', async () => {
    await renderPage()
    const inNav = document.querySelectorAll('[data-projects-nav-toggle]')
    expect(inNav.length).toBe(1)
    expect(inNav[0].getAttribute('aria-label')).toBe('Collapse projects nav')
    const nav = must(document.querySelector('[data-projects-nav]') as HTMLElement | null, 'nav')
    expect(nav.firstElementChild!.lastElementChild).toBe(inNav[0])
    act(() => {
      fireEvent.click(inNav[0])
    })
    const inRail = document.querySelectorAll('[data-projects-nav-toggle]')
    expect(inRail.length).toBe(1)
    expect(inRail[0].getAttribute('aria-label')).toBe('Open projects nav')
    const rail = must(document.querySelector('nav[aria-label="Projects"]') as HTMLElement | null, 'rail')
    expect(rail.firstElementChild!.contains(inRail[0])).toBe(true)
  })
})
