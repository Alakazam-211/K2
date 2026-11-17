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
//     as tabs + "+" (owners and admins), a divider and Feedback; then
//     Layout, ⚙, Chat;
//   - a dashboard tab's Rename / Delete are on its right-click (any tab,
//     not only the open one), Shift+F10 or the context-menu key; F2 and a
//     double-click rename. No "…" sits beside a tab, so switching tabs
//     never moves one;
//   - a Member login sees no "+", no tab menu and no Layout;
//   - with the nav as a rail, the project's name opens a switcher and the
//     agents stay as pictures;
//   - awake or asleep comes from `show` (members[].sessionAlive), read again
//     when one of the project's agents starts or ends a session: no call per
//     agent;
//   - an agent's right-click menu has "Opens in ›": set it, clear it to the
//     project's, disabled for a Member, and a refusal or failure is a toast.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
const daemon = vi.hoisted(() => ({
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  fail: null as string | null,
  /** A refusal for that route: [status, code]. */
  refuse: null as { route: string; status: number; code: string } | null,
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { DaemonHttpError } = await vi.importActual<typeof import('@/lib/daemon-http-error')>('@/lib/daemon-http-error')
  return {
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    daemon.gets.push(route)
    return route === 'project-group/resources'
      ? { ok: true, docs: [{ workspaceId: 'w1', workspaceName: 'atlas', agentName: 'Atlas', filePath: '/x/status.html', fileName: 'status.html', missing: false }] }
      : {}
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: Record<string, unknown>) => {
    daemon.posts.push({ route, body })
    if (daemon.refuse?.route === route) throw new DaemonHttpError('refused', daemon.refuse.status, daemon.refuse.code)
    if (daemon.fail === route) throw new Error('database is locked')
    if (route === 'project-group/set-member-default-view') {
      const view = body.view as 'terminal' | 'thread' | 'chatter' | null
      return { ok: true, groupId: 'g1', workspaceId: body.workspace, defaultView: view ?? 'thread', ownDefaultView: view }
    }
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
  }
})
const sessions = vi.hoisted(() => ({
  added: [] as Array<(e: { workspace_path: string }) => void>,
  removed: [] as Array<(e: { workspace_path: string }) => void>,
}))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onSessionAddedApp: vi.fn((_s: unknown, fn: (e: { workspace_path: string }) => void) => {
      sessions.added.push(fn)
      return () => {
        sessions.added = sessions.added.filter((f) => f !== fn)
      }
    }),
    onSessionRemovedApp: vi.fn((_s: unknown, fn: (e: { workspace_path: string }) => void) => {
      sessions.removed.push(fn)
      return () => {
        sessions.removed = sessions.removed.filter((f) => f !== fn)
      }
    }),
  }
})
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

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ProjectsPage from './ProjectsPage'
import { showContextMenu } from '@/lib/context-menu'
import { nextDashboardName, projectSummary } from './ProjectBar'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'
import { useUnreadStore } from '@/stores/unread'
import { useToastStore } from '@/stores/toast'
import { useContextMenuStore } from '@/stores/context-menu'
import { fetchProjectGroupShow } from './projects-api'
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
  defaultView: 'thread',
  members: [
    {
      workspaceId: 'w1', name: 'atlas', path: '/w/atlas', agentName: 'Atlas', createdAt: 1,
      sessionAlive: true, sessionId: 's1', defaultView: 'thread', ownDefaultView: null,
    },
    {
      workspaceId: 'w2', name: 'web-app', path: '/w/web-app', agentName: 'Web App', createdAt: 2,
      sessionAlive: false, sessionId: null, defaultView: 'thread', ownDefaultView: null,
    },
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
  daemon.gets = []
  daemon.posts = []
  daemon.fail = null
  daemon.refuse = null
  useToastStore.setState({ toasts: [] })
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
  act(() => useContextMenuStore.getState().close())
  cleanup()
})

describe('helpers', () => {
  it('names a new dashboard with the first free "New dashboard"', () => {
    expect(nextDashboardName(['Main'])).toBe('New dashboard')
    expect(nextDashboardName(['Main', 'new dashboard', 'New dashboard 2'])).toBe('New dashboard 3')
  })
  it('sums a project up in one quiet line', () => {
    expect(projectSummary(BASE_SHOW)).toBe('2 agents · Point of contact: Atlas')
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

  it('a list longer than its cap fades at the cut, so it reads as scrollable', async () => {
    const sh = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight')
    const ch = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight')
    Object.defineProperty(HTMLElement.prototype, 'scrollHeight', { configurable: true, get: () => 600 })
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 200 })
    try {
      await renderPage()
      await waitFor(() => expect(document.querySelector('[data-projects-list-fade]')).not.toBeNull())
    } finally {
      if (sh) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', sh)
      if (ch) Object.defineProperty(HTMLElement.prototype, 'clientHeight', ch)
    }
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
    expect(atlas.querySelector('[data-agent-poc]')!.getAttribute('title')).toContain('Point of contact')
    // The view its tile opens in, and awake (a dot) or asleep (from show's sessionAlive).
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
      expect(bar().querySelector('[data-project-summary]')!.textContent).toBe('3 agents · Point of contact: Atlas'),
    )
    expect(document.querySelector('[data-project-nav-section] [data-agent-row="w3"]')).not.toBeNull()
    expect(document.querySelector('[data-project-section-count]')!.textContent).toBe('3 agents')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Done' }))
    expect(screen.queryByTestId('add-agent-dialog')).toBeNull()
    expect(document.activeElement).toBe(add)
  })

  it('the rail: its toggle, then projects, then the selected project\'s agents as pictures', async () => {
    useProjectGroupsStore.setState({ navCollapsed: true })
    await renderPage()
    const rail = screen.getAllByRole('navigation', { name: 'Projects' })[0]
    await waitFor(() =>
      expect(within(rail).getAllByRole('button').map((b) => b.getAttribute('aria-label'))).toEqual([
        'Open projects nav',
        'Launch',
        'Docs',
        'Atlas',
        'Web App',
        'Add an agent',
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
    expect(b.querySelector('[data-project-summary]')!.textContent).toBe('2 agents · Point of contact: Atlas')
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

  it('a Member login sees no + and no Layout (Settings and Chat stay)', async () => {
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

})

describe('a dashboard tab\'s menu (right-click, Shift+F10, F2)', () => {
  type Item = { id: string; label: string; type?: string; enabled?: boolean; hint?: string; danger?: boolean }

  /** An owner's bar, once the role is known (the + shows). */
  async function renderOwner(): Promise<void> {
    await renderPage()
    await waitFor(() => expect(within(bar()).getByRole('button', { name: 'New dashboard' })).toBeTruthy())
  }
  const tab = (name: string): HTMLElement => within(bar()).getByRole('tab', { name })
  async function rightClick(name: string, pick: string | null): Promise<Item[]> {
    ctxMenu.pick = pick
    const shown = vi.mocked(showContextMenu)
    shown.mockClear()
    fireEvent.contextMenu(tab(name))
    await waitFor(() => expect(shown).toHaveBeenCalledTimes(1))
    return shown.mock.calls[0][0] as Item[]
  }

  it('no tab has a "…" beside it', async () => {
    await renderOwner()
    expect(bar().querySelectorAll('[data-tab-menu]')).toHaveLength(0)
    fireEvent.click(tab('Release'))
    expect(bar().querySelectorAll('[data-tab-menu]')).toHaveLength(0)
  })

  it('switching tabs moves no tab: the same elements, in the same order, nothing between them', async () => {
    await renderOwner()
    const list = within(bar()).getByRole('tablist')
    const before = Array.from(list.children)
    const tabsBefore = within(list).getAllByRole('tab')
    const slots = (): number[] => Array.from(list.querySelectorAll('[data-dashboard-tab]')).map((s) => s.children.length)
    expect(slots()).toEqual([1, 1])
    fireEvent.click(tab('Release'))
    expect(tab('Release').getAttribute('aria-selected')).toBe('true')
    expect(Array.from(list.children)).toEqual(before)
    expect(within(list).getAllByRole('tab')).toEqual(tabsBefore)
    expect(slots()).toEqual([1, 1])
  })

  it('right-clicking a tab that isn\'t open offers Rename and Delete… for that tab', async () => {
    await renderOwner()
    expect(tab('Main').getAttribute('aria-selected')).toBe('true')
    const items = await rightClick('Release', null)
    expect(items.map((i) => i.label)).toEqual(['Rename', '', 'Delete…'])
    const del = items.find((i) => i.id === 'delete')!
    expect(del.danger).toBe(true)
    expect(del.enabled).not.toBe(false)
    // The open tab stays open.
    expect(tab('Main').getAttribute('aria-selected')).toBe('true')
  })

  it('Rename puts that tab\'s name field in its place', async () => {
    await renderOwner()
    await rightClick('Release', 'rename')
    const field = (await waitFor(() => screen.getByLabelText('Dashboard name'))) as HTMLInputElement
    expect(field.value).toBe('Release')
    expect(field.closest('[data-dashboard-tab]')!.getAttribute('data-dashboard-tab')).toBe('d2')
    fireEvent.change(field, { target: { value: 'Ship it' } })
    fireEvent.keyDown(field, { key: 'Enter' })
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/dashboard/rename')!.body).toMatchObject({
        group: 'g1',
        dashboardId: 'd2',
        name: 'Ship it',
      }),
    )
  })

  it('Delete… asks with the shared confirm, then deletes that tab and keeps the open one', async () => {
    await renderOwner()
    await rightClick('Release', 'delete')
    const confirm = await screen.findByTestId('delete-dashboard-confirm')
    expect(confirm.textContent).toContain('Delete the dashboard “Release”?')
    expect(daemon.posts.some((p) => p.route === 'project-group/dashboard/delete')).toBe(false)
    fireEvent.click(within(confirm).getByRole('button', { name: 'Delete dashboard' }))
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/dashboard/delete')!.body).toMatchObject({ group: 'g1', dashboardId: 'd2' }),
    )
    expect(tab('Main').getAttribute('aria-selected')).toBe('true')
  })

  it('cancelling the confirm puts focus back on the tab', async () => {
    await renderOwner()
    await rightClick('Release', 'delete')
    const confirm = await screen.findByTestId('delete-dashboard-confirm')
    fireEvent.click(within(confirm).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(document.activeElement).toBe(tab('Release')))
    expect(daemon.posts.some((p) => p.route === 'project-group/dashboard/delete')).toBe(false)
  })

  it('the last dashboard\'s Delete is disabled, and says why', async () => {
    state.show = { ...BASE_SHOW, dashboards: [dash('d1', 'Main', 0)] }
    await renderOwner()
    const del = (await rightClick('Main', null)).find((i) => i.id === 'delete')!
    expect(del.enabled).toBe(false)
    expect(del.hint).toBe('A project keeps at least one dashboard.')
  })

  it('Shift+F10 and the context-menu key open it at the tab; F2 renames', async () => {
    await renderOwner()
    const release = tab('Release')
    release.focus()
    fireEvent.keyDown(release, { key: 'F10', shiftKey: true })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    expect(useContextMenuStore.getState().items.map((i) => i.label)).toEqual(['Rename', '', 'Delete…'])
    act(() => useContextMenuStore.getState().close())

    fireEvent.keyDown(release, { key: 'ContextMenu' })
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    act(() => useContextMenuStore.getState().selectItem('rename'))
    const field = (await waitFor(() => screen.getByLabelText('Dashboard name'))) as HTMLInputElement
    expect(field.value).toBe('Release')
    fireEvent.keyDown(field, { key: 'Escape' })
    expect(screen.queryByLabelText('Dashboard name')).toBeNull()
    expect(daemon.posts.some((p) => p.route === 'project-group/dashboard/rename')).toBe(false)

    const main = tab('Main')
    main.focus()
    fireEvent.keyDown(main, { key: 'F2' })
    expect(((await waitFor(() => screen.getByLabelText('Dashboard name'))) as HTMLInputElement).value).toBe('Main')
  })

  it('double-click still renames', async () => {
    await renderOwner()
    fireEvent.doubleClick(tab('Release'))
    expect((screen.getByLabelText('Dashboard name') as HTMLInputElement).value).toBe('Release')
  })

  it('the tooltip says so, and the tab says it has a menu', async () => {
    await renderOwner()
    expect(tab('Release').title).toBe('Release · Right-click to rename or delete')
    expect(tab('Release').getAttribute('aria-haspopup')).toBe('menu')
  })

  it('a Member gets no menu, no rename and no browser menu', async () => {
    viewer.role = 'member'
    await renderPage()
    await waitFor(() => expect(bar().querySelector('[data-layout-menu]')).toBeNull())
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })
    const shown = vi.mocked(showContextMenu)
    shown.mockClear()
    const release = tab('Release')
    expect(release.title).toBe('Release')
    expect(release.hasAttribute('aria-haspopup')).toBe(false)
    // fireEvent returns false when the default (the browser's menu) was prevented.
    expect(fireEvent.contextMenu(release)).toBe(false)
    release.focus()
    fireEvent.keyDown(release, { key: 'F10', shiftKey: true })
    fireEvent.keyDown(release, { key: 'F2' })
    fireEvent.doubleClick(release)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })
    expect(shown).not.toHaveBeenCalled()
    expect(useContextMenuStore.getState().isOpen).toBe(false)
    expect(screen.queryByLabelText('Dashboard name')).toBeNull()
  })
})

describe('tabs that don\'t fit', () => {
  it('turn into one picker naming the open tab; it lists every dashboard, then Feedback', async () => {
    const sw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollWidth')
    const cw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')
    Object.defineProperty(HTMLElement.prototype, 'scrollWidth', { configurable: true, get: () => 500 })
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 100 })
    try {
      await renderPage()
      const picker = await waitFor(() => bar().querySelector('[data-tab-picker]') as HTMLElement)
      // No tab is cut or hidden behind the buttons: the tabs give way whole.
      expect(within(bar()).queryAllByRole('tab')).toHaveLength(0)
      expect(picker.textContent).toContain('Main')
      fireEvent.click(picker)
      const menu = await screen.findByTestId('tab-picker-menu')
      expect(within(menu).getAllByRole('menuitem').map((m) => m.textContent)).toEqual(['Main', 'Release', 'Feedback'])
      fireEvent.click(within(menu).getByRole('menuitem', { name: 'Feedback' }))
      expect(screen.getByTestId('feedback-stub')).toBeTruthy()
      expect((bar().querySelector('[data-tab-picker]') as HTMLElement).textContent).toContain('Feedback')
    } finally {
      if (sw) Object.defineProperty(HTMLElement.prototype, 'scrollWidth', sw)
      if (cw) Object.defineProperty(HTMLElement.prototype, 'clientWidth', cw)
    }
  })

  it('no "…" beside the picker; right-clicking it gives the open dashboard\'s menu', async () => {
    const sw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollWidth')
    const cw = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')
    Object.defineProperty(HTMLElement.prototype, 'scrollWidth', { configurable: true, get: () => 500 })
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 100 })
    try {
      await renderPage()
      await waitFor(() => expect(within(bar()).getByRole('button', { name: 'New dashboard' })).toBeTruthy())
      const picker = await waitFor(() => bar().querySelector('[data-tab-picker]') as HTMLElement)
      expect(bar().querySelectorAll('[data-tab-menu]')).toHaveLength(0)
      ctxMenu.pick = 'rename'
      const shown = vi.mocked(showContextMenu)
      shown.mockClear()
      fireEvent.contextMenu(picker)
      await waitFor(() => expect(shown).toHaveBeenCalledTimes(1))
      expect((shown.mock.calls[0][0] as Array<{ label: string }>).map((i) => i.label)).toEqual(['Rename', '', 'Delete…'])
      expect(((await waitFor(() => screen.getByLabelText('Dashboard name'))) as HTMLInputElement).value).toBe('Main')
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
    expect(items.map((i) => i.label)).toEqual(['Go to its tile', 'Opens in', 'Make point of contact', 'Pin to top', '', 'Remove from Launch…'])
    expect(items[0].shortcut).toBe('⌘2')
    const confirm = await screen.findByTestId('remove-agent-confirm')
    expect(daemon.posts.some((p) => p.route === 'project-group/remove-member')).toBe(false)
    fireEvent.click(within(confirm).getByRole('button', { name: 'Remove from project' }))
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/remove-member')!.body).toMatchObject({ group: 'g1', workspace: 'w2' }),
    )
  })
})

describe('awake or asleep, from show', () => {
  it('reads members[].sessionAlive, with no call per agent', async () => {
    await renderPage()
    const section = await waitFor(() => document.querySelector('[data-project-nav-section]') as HTMLElement)
    await waitFor(() => expect(section.querySelector('[data-agent-row="w1"] [data-liveness="awake"]')).not.toBeNull())
    expect(section.querySelector('[data-agent-row="w2"] [data-liveness="asleep"]')).not.toBeNull()
    expect(daemon.gets.filter((r) => r === 'sessions/lookup-by-agent')).toEqual([])
  })

  it('an older daemon (no sessionAlive) shows neither, rather than a guess', async () => {
    state.show = {
      ...BASE_SHOW,
      members: BASE_SHOW.members.map(({ sessionAlive: _a, sessionId: _b, ...m }) => m),
    }
    await renderPage()
    const section = await waitFor(() => document.querySelector('[data-project-nav-section] [data-agent-row="w2"]') as HTMLElement)
    expect(section.closest('[data-project-nav-section]')!.querySelector('[data-liveness]')).toBeNull()
  })

  it("an agent's session starting reads show again (once), and the row wakes", async () => {
    await renderPage()
    await waitFor(() => expect(document.querySelector('[data-agent-row="w2"] [data-liveness="asleep"]')).not.toBeNull())
    const reads = vi.mocked(fetchProjectGroupShow)
    reads.mockClear()
    state.show = {
      ...BASE_SHOW,
      members: BASE_SHOW.members.map((m) => (m.workspaceId === 'w2' ? { ...m, sessionAlive: true, sessionId: 's2' } : m)),
    }
    // Another folder's session: nothing to read.
    act(() => sessions.added.forEach((fn) => fn({ workspace_path: '/w/elsewhere' })))
    // Two events for the project's agents: one read.
    act(() => sessions.added.forEach((fn) => fn({ workspace_path: '/w/web-app/' })))
    act(() => sessions.removed.forEach((fn) => fn({ workspace_path: '/w/atlas' })))
    await waitFor(() => expect(document.querySelector('[data-agent-row="w2"] [data-liveness="awake"]')).not.toBeNull())
    expect(reads).toHaveBeenCalledTimes(1)
    expect(daemon.gets.filter((r) => r === 'sessions/lookup-by-agent')).toEqual([])
  })
})

describe('"Opens in", in an agent\'s right-click menu', () => {
  type Item = { id: string; label: string; hint?: string; enabled?: boolean; tooltip?: string; checked?: boolean; type?: string; submenu?: Item[] }
  async function openMenu(ws: string, pick: string | null): Promise<Item> {
    ctxMenu.pick = pick
    const shown = vi.mocked(showContextMenu)
    shown.mockClear()
    const row = await waitFor(() => document.querySelector(`[data-agent-row="${ws}"] > button:last-child`) as HTMLElement)
    fireEvent.contextMenu(row)
    await waitFor(() => expect(shown).toHaveBeenCalled())
    return (shown.mock.calls[0][0] as Item[]).find((i) => i.id === 'opens-in')!
  }

  it("shows the project's view as inherited, and the agent's own when it has one", async () => {
    state.show = {
      ...BASE_SHOW,
      members: [BASE_SHOW.members[0], { ...BASE_SHOW.members[1], defaultView: 'terminal', ownDefaultView: 'terminal' }],
    }
    await renderPage()
    const inherited = await openMenu('w1', null)
    expect(inherited.hint).toBe('Thread (project default)')
    expect(inherited.submenu!.map((i) => i.label)).toEqual(['Project default (Thread)', '', 'Terminal', 'Thread', 'Chatter'])
    expect(inherited.submenu!.filter((i) => i.checked).map((i) => i.label)).toEqual(['Project default (Thread)'])
    // Heartbeats is never a shared view.
    expect(inherited.submenu!.some((i) => i.label === 'Heartbeats')).toBe(false)

    const own = await openMenu('w2', null)
    expect(own.hint).toBe('Terminal')
    expect(own.submenu!.filter((i) => i.checked).map((i) => i.label)).toEqual(['Terminal'])
    // The row's view icon is the agent's own view.
    expect(document.querySelector('[data-agent-row="w2"] [data-agent-view]')!.getAttribute('data-agent-view')).toBe('terminal')
  })

  it('sets it through set-member-default-view, and says so', async () => {
    await renderPage()
    await openMenu('w2', 'view:chatter')
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/set-member-default-view')!.body).toEqual({
        group: 'g1',
        workspace: 'w2',
        view: 'chatter',
      }),
    )
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.message)).toContain('Web App now opens in Chatter for everyone.'),
    )
    // Shown at once (before show is read again).
    await waitFor(() =>
      expect(document.querySelector('[data-agent-row="w2"] [data-agent-view]')!.getAttribute('data-agent-view')).toBe('chatter'),
    )
  })

  it('"Project default" clears it (view: null)', async () => {
    state.show = {
      ...BASE_SHOW,
      members: [BASE_SHOW.members[0], { ...BASE_SHOW.members[1], defaultView: 'terminal', ownDefaultView: 'terminal' }],
    }
    await renderPage()
    await openMenu('w2', 'view:default')
    await waitFor(() =>
      expect(daemon.posts.find((p) => p.route === 'project-group/set-member-default-view')!.body).toEqual({
        group: 'g1',
        workspace: 'w2',
        view: null,
      }),
    )
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.message)).toContain(
        'Web App opens in the project default (Thread) again.',
      ),
    )
  })

  it('picking the current choice saves nothing', async () => {
    await renderPage()
    await openMenu('w2', 'view:default')
    await new Promise((r) => setTimeout(r, 0))
    expect(daemon.posts.some((p) => p.route === 'project-group/set-member-default-view')).toBe(false)
  })

  it('a Member sees it disabled, with the reason', async () => {
    viewer.role = 'member'
    await renderPage()
    await waitFor(() => expect(document.querySelector('[data-project-nav-section]')).not.toBeNull())
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })
    const item = await openMenu('w2', null)
    expect(item.enabled).toBe(false)
    expect(item.submenu).toBeUndefined()
    expect(item.tooltip).toBe('Only the owner or an admin can change where an agent opens.')
    expect(item.hint).toBe('Thread (project default)')
  })

  it('an older daemon (no per-agent view) shows it disabled: needs a newer K2', async () => {
    state.show = {
      ...BASE_SHOW,
      members: BASE_SHOW.members.map(({ defaultView: _a, ownDefaultView: _b, ...m }) => m),
    }
    await renderPage()
    const item = await openMenu('w1', null)
    expect(item.enabled).toBe(false)
    expect(item.tooltip).toBe('Needs a newer K2 on this server.')
  })

  it('a 403 is one plain sentence, no Try again, and the view rolls back', async () => {
    daemon.refuse = { route: 'project-group/set-member-default-view', status: 403, code: 'forbidden' }
    await renderPage()
    await openMenu('w2', 'view:terminal')
    await waitFor(() => expect(useToastStore.getState().toasts.length).toBe(1))
    const t = useToastStore.getState().toasts[0]
    expect(t.type).toBe('error')
    expect(t.message).toBe('Couldn’t change it. Only the owner or an admin can.')
    expect(t.action).toBeUndefined()
    expect(document.querySelector('[data-agent-row="w2"] [data-agent-view]')!.getAttribute('data-agent-view')).toBe('thread')
  })

  it('an agent no longer in the project (403 not_a_member) says so plainly', async () => {
    daemon.refuse = { route: 'project-group/set-member-default-view', status: 403, code: 'not_a_member' }
    await renderPage()
    await openMenu('w2', 'view:terminal')
    await waitFor(() => expect(useToastStore.getState().toasts.length).toBe(1))
    expect(useToastStore.getState().toasts[0].message).toBe('Couldn’t change it. Web App isn’t in this project any more.')
  })

  it('any other failure says so with Try again, which tries again', async () => {
    daemon.fail = 'project-group/set-member-default-view'
    await renderPage()
    await openMenu('w2', 'view:terminal')
    await waitFor(() => expect(useToastStore.getState().toasts.length).toBe(1))
    const t = useToastStore.getState().toasts[0]
    expect(t.type).toBe('error')
    expect(t.message).toBe('Couldn’t change where Web App opens. Nothing changed. The server said: database is locked')
    expect(t.action!.label).toBe('Try again')
    daemon.fail = null
    act(() => t.action!.onClick())
    await waitFor(() =>
      expect(daemon.posts.filter((p) => p.route === 'project-group/set-member-default-view')).toHaveLength(2),
    )
    expect(daemon.posts.at(-1)!.body).toEqual({ group: 'g1', workspace: 'w2', view: 'terminal' })
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
