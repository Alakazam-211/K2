// @vitest-environment jsdom
//
// The project bar and the nav after the 0.46.12 reorganisation
// (projects-reorg-draft §3.1–3.2, Rosson 2026-11-11):
//   - the nav: "Projects" with "+", Pinned / All projects, "N agents" (a
//     capped list); a divider, then the selected project's section: its
//     agents (view, awake or asleep, the Point of contact tag, ⌘N on hover)
//     with each agent's files folded under it, and "Add an agent…" (the
//     Settings picker in a dialog);
//   - the bar: identity ("N agents · Point of contact: X"), the dashboards
//     as tabs + "+" (owners and admins) + the open tab's Rename / Delete
//     menu, a divider and Feedback; then Layout, ⚙, Chat;
//   - a Member login sees no "+", no tab menu and no Layout;
//   - with the nav as a rail, the project's name opens a switcher and the
//     agents stay as pictures.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
const daemon = vi.hoisted(() => ({
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  fail: null as string | null,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, q?: Record<string, string>) =>
    route === 'project-group/resources'
      ? { ok: true, docs: [{ workspaceId: 'w1', workspaceName: 'atlas', agentName: 'Atlas', filePath: '/x/status.html', fileName: 'status.html', missing: false }] }
      : route === 'sessions/lookup-by-agent'
        ? { sessionAlive: q?.agent === 'w1', sessionId: q?.agent === 'w1' ? 's1' : null }
        : {},
  ),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: Record<string, unknown>) => {
    daemon.posts.push({ route, body })
    if (daemon.fail === route) throw new Error('database is locked')
    if (route === 'project-group/dashboard/create') {
      const d = { id: 'd-new', groupId: 'g1', name: body.name, layoutJson: '{}', revision: 1, position: 2, createdAt: 1, updatedAt: 1 }
      state.show = { ...state.show, dashboards: [...state.show.dashboards, d as never] }
      return { ok: true, dashboard: d }
    }
    if (route === 'project-group/add-member') {
      state.show = {
        ...state.show,
        members: [...state.show.members, { workspaceId: String(body.workspace), name: 'ledger', path: '/w/ledger', agentName: 'Ledger', createdAt: 1 }],
      }
    }
    return { ok: true }
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
const ctxMenu = vi.hoisted(() => ({ pick: null as string | null }))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => ctxMenu.pick) }))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({ default: () => null }))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => null }))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
const viewer = vi.hoisted(() => ({ role: 'owner' as 'owner' | 'member' }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => viewer.role) }))
vi.mock('./ProjectDashboard', () => ({ default: () => <div data-testid="dashboard-stub" /> }))
vi.mock('@/components/Feedback/ProjectFeedbackTab', () => ({ ProjectFeedbackTab: () => <div data-testid="feedback-stub" /> }))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async () => state.show),
    fetchProjectGroupMessages: vi.fn(async () => ({ messages: [], truncated: false })),
  }
})

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { showContextMenu } from '@/lib/context-menu'
import { nextDashboardName, projectSummary } from './ProjectBar'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'
import { useUnreadStore } from '@/stores/unread'
import type { ProjectGroup, ProjectGroupShow } from './projects-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

const GROUP: ProjectGroup = {
  id: 'g1',
  name: 'Launch',
  pocWorkspaceId: 'w1',
  pinned: true,
  color: null,
  sortOrder: 0,
  createdAt: 1,
  updatedAt: 1,
  memberCount: 2,
  memberWorkspaceIds: ['w1', 'w2'],
}
const OTHER: ProjectGroup = { ...GROUP, id: 'g2', name: 'Docs', pinned: false, pocWorkspaceId: null, memberCount: 1, memberWorkspaceIds: ['w2'] }
const dash = (id: string, name: string, position: number) => ({
  id, groupId: 'g1', name, layoutJson: '{"version":1,"panes":[]}', revision: 1, position, createdAt: 1, updatedAt: 1,
})
const BASE_SHOW: ProjectGroupShow = {
  ...GROUP,
  members: [
    { workspaceId: 'w1', name: 'atlas', path: '/w/atlas', agentName: 'Atlas', createdAt: 1 },
    { workspaceId: 'w2', name: 'web-app', path: '/w/web-app', agentName: 'Web App', createdAt: 2 },
  ],
  dashboards: [dash('d1', 'Main', 0), dash('d2', 'Release', 1)],
}
const state = vi.hoisted(() => ({ show: null as unknown as ProjectGroupShow }))

function bar(): HTMLElement {
  const el = document.querySelector('[data-project-bar]') as HTMLElement | null
  if (!el) throw new Error('no project bar')
  return el
}

async function renderPage(): Promise<void> {
  render(<ProjectsPage />)
  await waitFor(() => expect(screen.getByTestId('dashboard-stub')).toBeTruthy())
  // The role (whoami) resolves after the first paint.
  await waitFor(() => expect(document.querySelector('[data-project-actions]')).not.toBeNull())
}

beforeEach(() => {
  state.show = BASE_SHOW
  daemon.posts = []
  daemon.fail = null
  ctxMenu.pick = null
  viewer.role = 'owner'
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({
    groups: [GROUP, OTHER],
    groupsError: null,
    selectedGroupId: 'g1',
    chatCollapsed: true,
    navCollapsed: false,
    paneRequest: null,
    dashPaneNumbers: {},
    fetchGroups: vi.fn(async () => {}),
  })
  useProjectsStore.setState({
    projects: [
      { id: 'w1', name: 'atlas', path: '/w/atlas' },
      { id: 'w2', name: 'web-app', path: '/w/web-app' },
      { id: 'w3', name: 'ledger', path: '/w/ledger' },
    ] as never,
  })
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0, loaded: true })
})

afterEach(() => {
  cleanup()
})

describe('helpers', () => {
  it('names a new dashboard with the first free "New dashboard"', () => {
    expect(nextDashboardName(['Main'])).toBe('New dashboard')
    expect(nextDashboardName(['Main', 'new dashboard', 'New dashboard 2'])).toBe('New dashboard 3')
  })
  it('sums a project up in one quiet line', () => {
    expect(projectSummary(BASE_SHOW)).toBe('2 agents · Point of contact Atlas')
    expect(projectSummary({ ...BASE_SHOW, members: [BASE_SHOW.members[1]], pocWorkspaceId: null })).toBe('1 agent')
  })
})

describe('the nav', () => {
  it('has Projects + New project, Pinned and All projects, "N agents", no hover pin', async () => {
    await renderPage()
    const nav = document.querySelector('[data-projects-nav]') as HTMLElement
    const list = nav.querySelector('[data-projects-list]') as HTMLElement
    expect(within(nav).getByRole('heading', { name: 'Projects' })).toBeTruthy()
    expect(within(nav).getByRole('button', { name: 'New project' })).toBeTruthy()
    expect(within(list).getByRole('heading', { name: 'Pinned' })).toBeTruthy()
    expect(within(list).getByRole('heading', { name: 'All projects' })).toBeTruthy()
    // One line a project: picture, name, unread dot (the count is its tooltip).
    expect(list.textContent).not.toMatch(/agents?/)
    expect((list.querySelector('[data-group-id="g1"] button') as HTMLElement).title).toBe('Launch · 2 agents')
    // The list scrolls on its own, capped, so the project's section keeps room.
    expect(list.className).toContain('overflow-y-auto')
    expect(list.className).toContain('max-h-[45%]')
    // No hover-only pin glyph: pinning is in the row's menu.
    expect(nav.querySelector('[data-nav-pin]')).toBeNull()
  })

  it('the header\'s fold shows only the selected project, and opens again', async () => {
    await renderPage()
    const toggle = screen.getByRole('button', { name: 'Fold the project list' })
    fireEvent.click(toggle)
    const list = document.querySelector('[data-projects-list]') as HTMLElement
    expect(Array.from(list.querySelectorAll('[data-group-id]')).map((r) => r.getAttribute('data-group-id'))).toEqual(['g1'])
    expect(within(list).queryByRole('heading')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Show the project list' }))
    expect(list.querySelectorAll('[data-group-id]').length).toBe(2)
  })

  it('"+" opens the create form with the shared field and buttons', async () => {
    await renderPage()
    fireEvent.click(screen.getByRole('button', { name: 'New project' }))
    expect(screen.getByLabelText('Project name')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Create' })).toBeTruthy()
  })

  it('under a divider, the selected project: its agents, each one\'s files under it', async () => {
    await renderPage()
    const section = await waitFor(() => {
      const el = document.querySelector('[data-project-nav-section]') as HTMLElement | null
      if (!el) throw new Error('no section')
      return el
    })
    expect(section.parentElement!.className).toContain('border-t')
    expect(within(section).getByRole('heading', { name: 'Launch' })).toBeTruthy()
    expect(section.querySelector('[data-project-section-count]')!.textContent).toBe('2 agents')
    const atlas = section.querySelector('[data-agent-row="w1"]') as HTMLElement
    expect(atlas.textContent).toContain('Atlas')
    expect(atlas.textContent).toContain('Point of contact')
    // The view its tile opens in, and awake (a dot) or asleep (from the session lookup).
    expect(atlas.querySelector('[data-agent-view="thread"]')!.getAttribute('title')).toBe('Opens in Thread')
    await waitFor(() => expect(atlas.querySelector('[data-liveness="awake"]')).not.toBeNull())
    await waitFor(() =>
      expect(section.querySelector('[data-agent-row="w2"] [data-liveness="asleep"]')!.textContent).toBe('asleep'),
    )
    // Atlas's file nests under Atlas, and folds away.
    await waitFor(() => expect(section.querySelector('[data-agent-files="w1"] [data-file-row="status.html"]')).not.toBeNull())
    fireEvent.click(atlas.querySelector('[data-agent-fold]') as HTMLElement)
    expect(section.querySelector('[data-agent-files="w1"]')).toBeNull()
    fireEvent.click(atlas.querySelector('[data-agent-fold]') as HTMLElement)
    expect(section.querySelector('[data-agent-files="w1"]')).not.toBeNull()
  })

  it('an agent row opens (or focuses) its tile; a file row shows the file here', async () => {
    await renderPage()
    const row = await waitFor(() => document.querySelector('[data-agent-row="w2"] button') as HTMLElement)
    fireEvent.click(row)
    expect(useProjectGroupsStore.getState().paneRequest?.workspaceId).toBe('w2')
    const file = await waitFor(() => {
      const el = document.querySelector('[data-file-row="status.html"]') as HTMLElement | null
      if (!el) throw new Error('no file row')
      return el
    })
    const showBtn = within(file).getByRole('button', { name: 'Show status.html on this dashboard' })
    fireEvent.click(showBtn)
    expect(useProjectGroupsStore.getState().paneRequest).toMatchObject({ workspaceId: 'w1', filePath: '/x/status.html' })
  })

  it('an HTML file opens from its row: double-click, or Open first in its right-click menu', async () => {
    await renderPage()
    const file = await waitFor(() => {
      const el = document.querySelector('[data-file-row="status.html"]') as HTMLElement | null
      if (!el) throw new Error('no file row')
      return el
    })
    useProjectGroupsStore.setState({ paneRequest: null })
    fireEvent.doubleClick(file)
    expect(useProjectGroupsStore.getState().paneRequest).toMatchObject({ workspaceId: 'w1', filePath: '/x/status.html' })

    useProjectGroupsStore.setState({ paneRequest: null })
    ctxMenu.pick = 'open'
    const shown = vi.mocked(showContextMenu)
    shown.mockClear()
    fireEvent.contextMenu(file)
    await waitFor(() => expect(useProjectGroupsStore.getState().paneRequest).toMatchObject({ workspaceId: 'w1', filePath: '/x/status.html' }))
    const items = shown.mock.calls[0][0] as Array<{ id: string; label: string }>
    expect(items.map((i) => i.label)).toEqual(['Open', '', 'Pin to top', 'Remove from Files'])
  })

  it('⌘N shows on the row only on hover or focus', async () => {
    useProjectGroupsStore.setState({ dashPaneNumbers: { w2: 2 } })
    await renderPage()
    const hint = await waitFor(() => document.querySelector('[data-agent-row="w2"] [data-agent-shortcut]') as HTMLElement)
    expect(hint.textContent).toBe('⌘2')
    expect(hint.className).toContain('hidden')
    expect(hint.className).toContain('group-hover:inline')
    expect(hint.className).toContain('group-focus-within:inline')
  })

  it('"Add an agent…" opens the Settings picker in a dialog; adding updates the nav and the bar', async () => {
    await renderPage()
    const add = await waitFor(() => document.querySelector('[data-add-agent]') as HTMLElement)
    fireEvent.click(add)
    const dialog = await screen.findByTestId('add-agent-dialog')
    expect(within(dialog).getByTestId('add-agent-picker')).toBeTruthy()
    // Only agents not in the project yet.
    expect(within(dialog).queryByRole('button', { name: 'Add atlas' })).toBeNull()
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add ledger' }))
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/add-member')!.body).toMatchObject({ group: 'g1', workspace: 'w3' }),
    )
    await waitFor(() =>
      expect(bar().querySelector('[data-project-summary]')!.textContent).toBe('3 agents · Point of contact Atlas'),
    )
    expect(document.querySelector('[data-project-nav-section] [data-agent-row="w3"]')).not.toBeNull()
    expect(document.querySelector('[data-project-section-count]')!.textContent).toBe('3 agents')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Done' }))
    expect(screen.queryByTestId('add-agent-dialog')).toBeNull()
    expect(document.activeElement).toBe(add)
  })

  it('the rail: projects, then the selected project\'s agents as pictures, then its toggle', async () => {
    useProjectGroupsStore.setState({ navCollapsed: true })
    await renderPage()
    const rail = screen.getAllByRole('navigation', { name: 'Projects' })[0]
    await waitFor(() =>
      expect(within(rail).getAllByRole('button').map((b) => b.getAttribute('aria-label'))).toEqual([
        'Launch',
        'Docs',
        'Atlas',
        'Web App',
        'Add an agent',
        'Open projects nav',
      ]),
    )
    fireEvent.click(rail.querySelector('[data-rail-agent="w2"]') as HTMLElement)
    expect(useProjectGroupsStore.getState().paneRequest?.workspaceId).toBe('w2')
  })
})

describe('the project bar', () => {
  it('reads identity, tabs, + , divider, Feedback, then Layout, Settings, Chat', async () => {
    await renderPage()
    const b = bar()
    expect(within(b).getByRole('heading', { name: 'Launch' })).toBeTruthy()
    expect(b.querySelector('[data-project-summary]')!.textContent).toBe('2 agents · Point of contact Atlas')
    const tabs = within(b).getAllByRole('tab').map((t) => t.textContent)
    expect(tabs).toEqual(['Main', 'Release', 'Feedback'])
    expect(within(b).getByRole('button', { name: 'New dashboard' })).toBeTruthy()
    const actions = b.querySelector('[data-project-actions]') as HTMLElement
    const order = Array.from(actions.children).map((el) =>
      el.hasAttribute('data-layout-menu')
        ? 'layout'
        : el.hasAttribute('data-project-settings-button')
          ? 'settings'
          : el.hasAttribute('data-project-chat-button')
            ? 'chat'
            : el.tagName,
    )
    expect(order).toEqual(['layout', 'settings', 'chat'])
    expect(b.querySelector('[data-agents-menu-button]')).toBeNull()
  })

  it('a Member login sees no +, no tab menu and no Layout (Settings and Chat stay)', async () => {
    viewer.role = 'member'
    await renderPage()
    await waitFor(() => expect(bar().querySelector('[data-layout-menu]')).toBeNull())
    const b = bar()
    expect(within(b).queryByRole('button', { name: 'New dashboard' })).toBeNull()
    expect(b.querySelector('[data-tab-menu]')).toBeNull()
    expect(b.querySelector('[data-project-settings-button]')).not.toBeNull()
    expect(b.querySelector('[data-project-chat-button]')).not.toBeNull()
  })

  it('Feedback is a tab and opens the tickets board', async () => {
    await renderPage()
    fireEvent.click(within(bar()).getByRole('tab', { name: 'Feedback' }))
    expect(screen.getByTestId('feedback-stub')).toBeTruthy()
  })

  it('"+" adds a dashboard and leaves its tab ready to rename', async () => {
    await renderPage()
    fireEvent.click(within(bar()).getByRole('button', { name: 'New dashboard' }))
    const field = (await waitFor(() => screen.getByLabelText('Dashboard name'))) as HTMLInputElement
    expect(daemon.posts.find((p) => p.route === 'project-group/dashboard/create')!.body).toMatchObject({ group: 'g1', name: 'New dashboard' })
    expect(field.value).toBe('New dashboard')
    expect(document.activeElement).toBe(field)
    fireEvent.change(field, { target: { value: 'Launch week' } })
    fireEvent.keyDown(field, { key: 'Enter' })
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/dashboard/rename')!.body).toMatchObject({
        group: 'g1',
        dashboardId: 'd-new',
        name: 'Launch week',
      }),
    )
  })

  it('the open tab\'s menu deletes after the shared confirm', async () => {
    await renderPage()
    fireEvent.click(bar().querySelector('[data-tab-menu="d1"]') as HTMLElement)
    fireEvent.click(await screen.findByRole('menuitem', { name: /Delete/ }))
    const confirm = await screen.findByTestId('delete-dashboard-confirm')
    expect(confirm.textContent).toContain('Delete the dashboard “Main”?')
    expect(daemon.posts.some((p) => p.route === 'project-group/dashboard/delete')).toBe(false)
    fireEvent.click(within(confirm).getByRole('button', { name: 'Delete dashboard' }))
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/dashboard/delete')!.body).toMatchObject({ group: 'g1', dashboardId: 'd1' }),
    )
  })

  it('the menu\'s Rename turns the tab into its name field', async () => {
    await renderPage()
    fireEvent.click(bar().querySelector('[data-tab-menu="d1"]') as HTMLElement)
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Rename' }))
    const field = screen.getByLabelText('Dashboard name') as HTMLInputElement
    expect(field.value).toBe('Main')
    // Escape keeps the old name and writes nothing.
    fireEvent.keyDown(field, { key: 'Escape' })
    expect(screen.queryByLabelText('Dashboard name')).toBeNull()
    expect(daemon.posts.some((p) => p.route === 'project-group/dashboard/rename')).toBe(false)
  })
})

describe('tabs that don\'t fit', () => {
  it('"All ▾" appears and lists every dashboard; picking one opens it', async () => {
    const sw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollWidth')
    const cw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')
    Object.defineProperty(HTMLElement.prototype, 'scrollWidth', { configurable: true, get: () => 500 })
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 100 })
    try {
      await renderPage()
      const all = await waitFor(() => within(bar()).getByRole('button', { name: 'All dashboards (2)' }))
      fireEvent.click(all)
      const menu = await screen.findByTestId('all-dashboards')
      expect(within(menu).getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Main', 'Release'])
      fireEvent.click(within(menu).getByRole('menuitem', { name: 'Release' }))
      await waitFor(() => expect(within(bar()).getByRole('tab', { name: 'Release' }).getAttribute('aria-selected')).toBe('true'))
    } finally {
      if (sw) Object.defineProperty(HTMLElement.prototype, 'scrollWidth', sw)
      if (cw) Object.defineProperty(HTMLElement.prototype, 'clientWidth', cw)
    }
  })
})

describe('an agent\'s right-click menu', () => {
  it('has Go to its tile, Make point of contact, Show its files, Pin, then Remove (after the confirm)', async () => {
    useProjectGroupsStore.setState({ dashPaneNumbers: { w2: 2 } })
    ctxMenu.pick = 'remove'
    await renderPage()
    const shown = vi.mocked(showContextMenu)
    shown.mockClear()
    const row = await waitFor(() => document.querySelector('[data-agent-row="w2"] > button:last-child') as HTMLElement)
    fireEvent.contextMenu(row)
    await waitFor(() => expect(shown).toHaveBeenCalled())
    const items = shown.mock.calls[0][0] as Array<{ id: string; label: string; shortcut?: string; type?: string }>
    expect(items.map((i) => i.label)).toEqual(['Go to its tile', 'Make point of contact', 'Pin to top', '', 'Remove from Launch…'])
    expect(items[0].shortcut).toBe('⌘2')
    const confirm = await screen.findByTestId('remove-agent-confirm')
    expect(daemon.posts.some((p) => p.route === 'project-group/remove-member')).toBe(false)
    fireEvent.click(within(confirm).getByRole('button', { name: 'Remove from project' }))
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/remove-member')!.body).toMatchObject({ group: 'g1', workspace: 'w2' }),
    )
  })
})

describe('the switcher (nav as a rail)', () => {
  it('the name opens a menu of projects; picking one selects it', async () => {
    useProjectGroupsStore.setState({ navCollapsed: true })
    await renderPage()
    fireEvent.click(bar().querySelector('[data-project-switcher]') as HTMLElement)
    const menu = await screen.findByTestId('project-switcher')
    const find = within(menu).getByLabelText('Find a project') as HTMLInputElement
    expect(document.activeElement).toBe(find)
    fireEvent.change(find, { target: { value: 'doc' } })
    expect(within(menu).queryByRole('menuitem', { name: /Launch/ })).toBeNull()
    expect(within(menu).getByRole('menuitem', { name: 'New project' })).toBeTruthy()
    fireEvent.click(within(menu).getByRole('menuitem', { name: /Docs/ }))
    expect(useProjectGroupsStore.getState().selectedGroupId).toBe('g2')
  })
})
