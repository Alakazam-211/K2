// @vitest-environment jsdom
//
// The Projects page's left column sits in the same panel as the Home and
// Agents navs (NavTile: a Glass tile, a plain panel in Terminal), with a
// pane gap between it and the main area, and the collapsed rail is a tile
// too. Its top row ("Projects", the list fold, New project and «) matches
// the tabs beside it, here the project bar: both read one height, so their
// bottom lines continue across the gap, and the chat's header reads it too.
// « sits at the right end of that row and » at the top of the rail, Home's
// places. Pixels (radius, rim, where the lines land) are checked in
// real-app shots, not in jsdom.

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
vi.mock('@/components/TopBar/TopBarUtilities', () => ({ default: () => null }))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => null }))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'member') }))
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

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { NAV_TILE_CLASS } from '@/components/Layout/NavTile'
import { PROJECT_BAR_HEIGHT_PX, PROJECT_BAR_ROW_HEIGHT } from './project-bar-height'
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
  pinned: true,
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

function must<T extends Element>(el: T | null, what: string): T {
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
})

afterEach(() => {
  cleanup()
})

describe('Projects nav panel', () => {
  it('the nav sits in the shared nav tile, the one Home and Agents use', async () => {
    await renderPage()
    const nav = must(document.querySelector('[data-projects-nav]'), 'nav')
    const tile = must(nav.closest<HTMLElement>('[data-nav-tile]'), 'nav tile')
    expect(tile.getAttribute('data-material')).toBe('tile')
    expect(tile.getAttribute('data-surface')).toBe('surface')
    for (const cls of NAV_TILE_CLASS.split(' ')) expect(tile.classList.contains(cls)).toBe(true)
    // The tile paints the column: the nav itself draws no fill or edge.
    expect(nav.className).not.toMatch(/\bbg-\[/)
    expect(nav.className).not.toMatch(/\bborder-r\b/)
  })

  it('a pane gap sits between the panel and the main area', async () => {
    await renderPage()
    const tile = must(document.querySelector<HTMLElement>('[data-nav-tile]'), 'nav tile')
    const body = must(tile.parentElement, 'page body')
    expect(body.classList.contains('gap-[var(--gap-pane)]')).toBe(true)
  })

  it('the top row is level with the project bar, with its bottom line, « at its right end', async () => {
    await renderPage()
    const nav = must(document.querySelector('[data-projects-nav]'), 'nav')
    const row = must(nav.querySelector<HTMLElement>('[data-projects-nav-header]'), 'top row')
    expect(nav.firstElementChild).toBe(row)
    for (const cls of 'flex items-center border-b flex-shrink-0'.split(' ')) expect(row.classList.contains(cls)).toBe(true)
    // One height, read by the bar and the row (the row less a panel's rim).
    expect(PROJECT_BAR_ROW_HEIGHT).toBe(`calc(${PROJECT_BAR_HEIGHT_PX}px - var(--tile-edge-width, 0px))`)
    expect(row.getAttribute('style')).toContain(`height: ${PROJECT_BAR_ROW_HEIGHT}`)
    const bar = must(document.querySelector<HTMLElement>('[data-project-bar]'), 'project bar')
    expect(bar.style.height).toBe(`${PROJECT_BAR_HEIGHT_PX}px`)
    expect(bar.classList.contains('border-b')).toBe(true)
    // No height class of its own on either.
    for (const el of [row, bar]) expect(el.className).not.toMatch(/(^|\s)(min-)?h-/)
    expect(row.textContent).toContain('Projects')
    expect(row.querySelector('[data-projects-list-toggle]')).not.toBeNull()
    expect(row.querySelector('[data-new-project]')).not.toBeNull()
    const collapse = must(row.querySelector<HTMLElement>('[data-projects-nav-toggle]'), '« in the top row')
    expect(row.lastElementChild).toBe(collapse)
    expect(collapse.getAttribute('aria-label')).toBe('Collapse projects nav')
    // Home's top-row buttons: the drawers' header button, glyph shape.
    expect(collapse.hasAttribute('data-k2-glyph-button')).toBe(true)
    expect(must(row.querySelector('[data-new-project]'), '+').hasAttribute('data-k2-glyph-button')).toBe(true)
    // Only one « on the page, and nothing left at the bottom of the nav.
    expect(document.querySelectorAll('[data-projects-nav-toggle]').length).toBe(1)
  })

  it('the open chat\'s header is level with the bar too', async () => {
    useProjectGroupsStore.setState({ chatCollapsed: false })
    await renderPage()
    const header = await waitFor(() => must(document.querySelector<HTMLElement>('[data-project-chat-header]'), 'chat header'))
    expect(header.getAttribute('style')).toContain(`height: ${PROJECT_BAR_ROW_HEIGHT}`)
    expect(header.classList.contains('border-b')).toBe(true)
    expect(header.className).not.toMatch(/(^|\s)(min-)?h-/)
  })

  it('collapsed: the rail is a tile with » at its top', async () => {
    await renderPage()
    act(() => {
      fireEvent.click(must(document.querySelector<HTMLElement>('[data-projects-nav-toggle]'), '«'))
    })
    expect(useProjectGroupsStore.getState().navCollapsed).toBe(true)
    const rail = must(document.querySelector<HTMLElement>('nav[aria-label="Projects"]'), 'rail')
    expect(rail.getAttribute('data-material')).toBe('tile')
    const open = must(rail.querySelector<HTMLElement>('[data-projects-nav-toggle]'), '» in the rail')
    expect(open.getAttribute('aria-label')).toBe('Open projects nav')
    expect(rail.firstElementChild!.contains(open)).toBe(true)
  })
})
