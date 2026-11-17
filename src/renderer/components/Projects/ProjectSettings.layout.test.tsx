// @vitest-environment jsdom
//
// Settings → Projects, 0.46.1 (Rosson 2026-10-18; eval-project-settings-v1):
//   - edit rights follow the login's ROLE (whoami), not the window's
//     eye/pencil switch; a member sees one banner saying why;
//   - two halves (left General + Dashboards, right Agents) that scroll on
//     their own, stacking at narrow widths; no tab strips;
//   - the dashboard selector is a sticky bar inside the left half and
//     stays mounted while that half scrolls;
//   - a failed list shows the error with Retry (never endless Loading…),
//     a failed project too;
//   - "Open project" opens it on the Projects page;
//   - "Add to <dashboard>" sends ONE pane to the daemon (add-pane), never
//     a whole layout.

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
const viewerRole = vi.hoisted(() => ({ current: 'owner' as 'owner' | 'admin' | 'member' | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: vi.fn(async () => viewerRole.current),
}))
const api = vi.hoisted(() => ({
  show: null as unknown,
  showError: null as Error | null,
}))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async () => {
      if (api.showError) throw api.showError
      return api.show
    }),
    fetchProjectGroupResources: vi.fn(async () => DOCS),
    fetchProjectGroupIcon: vi.fn(async () => ({ found: false, dataUrl: null })),
    addDashboardPane: vi.fn(async () => ({ ...DASH, revision: 2, changed: true })),
    setProjectGroupPoc: vi.fn(async () => ({ ...GROUP, pocWorkspaceId: 'w2' })),
    removeDashboardPane: vi.fn(async () => ({ ...DASH, revision: 2, changed: true })),
    saveDashboardLayout: vi.fn(async () => {
      throw new Error('the settings page must not save whole layouts')
    }),
  }
})

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ProjectSettings, { HALVES_MIN_WIDTH, PROJECTS_SPLIT_MIN_PX, shortFolder } from './ProjectSettings'
import * as projectsApi from './projects-api'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'
import * as daemonCli from '@/lib/daemon-cli'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore } from '@/stores/page-view'
import type { ProjectGroup, ProjectGroupDashboard, ProjectGroupHtmlDoc, ProjectGroupShow } from './projects-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

// The detail body's measured width drives split vs stacked.
const measured = vi.hoisted(() => ({ width: 1000 }))
class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
  unobserve(): void {}
}
;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver
const realRect = Element.prototype.getBoundingClientRect
Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
  if ((this as HTMLElement).dataset?.testid === 'project-settings-halves') {
    return { width: measured.width, height: 600, top: 0, left: 0, right: measured.width, bottom: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
  }
  return realRect.call(this)
}

const GROUP: ProjectGroup = {
  id: 'g1',
  name: 'Launch',
  pocWorkspaceId: 'w1',
  pinned: false,
  color: null,
  sortOrder: 0,
  createdAt: 1,
  updatedAt: 1,
  memberCount: 2,
}
const DASH: ProjectGroupDashboard = {
  id: 'd1',
  groupId: 'g1',
  name: 'Main',
  layoutJson: '{"version":1,"panes":[]}',
  revision: 1,
  position: 0,
  createdAt: 1,
  updatedAt: 1,
}
const SHOW: ProjectGroupShow = {
  ...GROUP,
  members: [
    { workspaceId: 'w1', name: 'cortana', path: '/w/cortana', agentName: 'Cortana', createdAt: 1 },
    { workspaceId: 'w2', name: 'ada', path: '/w/ada', agentName: 'Ada', createdAt: 2 },
  ],
  dashboards: [DASH, { ...DASH, id: 'd2', name: 'Ops', position: 1 }],
}
const DOCS: ProjectGroupHtmlDoc[] = [
  { workspaceId: 'w2', workspaceName: 'ada', agentName: 'Ada', filePath: '/w/ada/status.html', fileName: 'status.html' },
  { workspaceId: 'w2', workspaceName: 'ada', agentName: 'Ada', filePath: '/w/ada/notes.md', fileName: 'notes.md' },
]

const fetchGroups = vi.fn(async () => {})

async function renderReady(): Promise<void> {
  render(<ProjectSettings />)
  await waitFor(() => expect(screen.getByTestId('project-settings-detail')).toBeTruthy())
  // Let whoami and the resources list land.
  await waitFor(() => expect(screen.getByText('status.html')).toBeTruthy())
}

beforeEach(() => {
  viewerRole.current = 'owner'
  measured.width = 1000
  api.show = SHOW
  api.showError = null
  fetchGroups.mockClear()
  vi.mocked(projectsApi.addDashboardPane).mockClear()
  useProjectGroupsStore.setState({
    groups: [GROUP],
    groupsError: null,
    selectedGroupId: 'g1',
    revision: 0,
    fetchGroups,
  })
  useProjectsStore.setState({ projects: [] } as never)
  useSettingsStore.setState({ initialProjectGroupId: 'g1' })
})

afterEach(() => {
  cleanup()
})

describe('edit rights follow the role, not the window mode', () => {
  it('owner: no banner, delete and dashboard controls enabled', async () => {
    await renderReady()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Delete project' })).toBeTruthy())
    expect(screen.queryByTestId('project-settings-readonly-banner')).toBeNull()
    expect((screen.getByRole('button', { name: 'Delete project' }) as HTMLButtonElement).disabled).toBe(false)
    expect(screen.getByRole('button', { name: 'New dashboard' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Add to Main' })).toBeTruthy()
    expect(screen.getByLabelText('Automatic color')).toBeTruthy()
  })

  it('admin: the same as the owner', async () => {
    viewerRole.current = 'admin'
    await renderReady()
    await waitFor(() => expect(screen.getByRole('button', { name: 'New dashboard' })).toBeTruthy())
    expect(screen.queryByTestId('project-settings-readonly-banner')).toBeNull()
    expect((screen.getByRole('button', { name: 'Delete project' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('member: one banner says why; name, pin and agents stay editable; the rest is read-only', async () => {
    viewerRole.current = 'member'
    await renderReady()
    const banner = await screen.findByTestId('project-settings-readonly-banner')
    expect(banner.textContent).toMatch(/Only the owner or an admin/)
    const del = screen.getByRole('button', { name: 'Delete project' }) as HTMLButtonElement
    expect(del.disabled).toBe(true)
    expect(screen.queryByRole('button', { name: 'New dashboard' })).toBeNull()
    expect(screen.queryByRole('button', { name: /Add to Main/ })).toBeNull()
    expect(screen.queryByLabelText('Automatic color')).toBeNull()
    expect(screen.getByText('Automatic')).toBeTruthy()
    // Still editable for a member.
    expect((screen.getByLabelText('Project name') as HTMLInputElement).readOnly).toBe(false)
    expect((screen.getByRole('switch', { name: 'Show in Pinned' }) as HTMLButtonElement).disabled).toBe(false)
    // Make point of contact is in the agent's … menu.
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Ada' }))
    expect(screen.getByRole('menuitem', { name: /Make point of contact/ })).toBeTruthy()
    expect(screen.getByLabelText('Search agents to add')).toBeTruthy()
  })
})

describe('two halves', () => {
  it('wide: General and Dashboards on the left, Agents on the right, each scrolling on its own', async () => {
    await renderReady()
    const halves = screen.getByTestId('project-settings-halves')
    expect(halves.getAttribute('data-layout')).toBe('split')
    const left = screen.getByTestId('project-settings-left')
    const right = screen.getByTestId('project-settings-right')
    expect(within(left).getByRole('region', { name: 'General' })).toBeTruthy()
    expect(within(left).getByRole('region', { name: 'Dashboards' })).toBeTruthy()
    expect(within(right).getByRole('region', { name: 'Agents' })).toBeTruthy()
    expect(within(left).queryByRole('region', { name: 'Agents' })).toBeNull()
    expect(left.className).toContain('overflow-y-auto')
    expect(right.className).toContain('overflow-y-auto')
    // Each half's bottom fade ends on the half's bottom edge: the half has
    // no bottom padding (a sticky child stops short of it) and its last
    // child is the fade, holding the space under the last row.
    for (const half of [left, right]) {
      expect(half.className).not.toMatch(/(^|\s)(pb|py|p)-/)
      const edge = half.lastElementChild as HTMLElement
      expect(edge.getAttribute('data-scroll-edge')).toBe('bottom')
      expect(edge.className).toMatch(/(^|\s)h-8(\s|$)/)
    }
    // No tab strips anywhere: no nested dashboard tabs, no page tabs.
    expect(screen.queryByRole('tablist')).toBeNull()
    expect(screen.queryByRole('tab')).toBeNull()
  })

  it(`narrow (< ${HALVES_MIN_WIDTH}px): the halves stack, left above right, in one scroll`, async () => {
    measured.width = 480
    await renderReady()
    const halves = screen.getByTestId('project-settings-halves')
    expect(halves.getAttribute('data-layout')).toBe('stacked')
    expect(halves.className).toContain('overflow-y-auto')
    const left = screen.getByTestId('project-settings-left')
    const right = screen.getByTestId('project-settings-right')
    expect(left.className).not.toContain('overflow-y-auto')
    expect(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it(`a page under ${PROJECTS_SPLIT_MIN_PX} CSS px (a small window, zoomed in) puts the project list on top of the project`, async () => {
    applyAppZoomForTest(1.75)
    const rect = Element.prototype.getBoundingClientRect
    Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
      // 800 on screen at 175 %: 457 CSS px.
      if ((this as HTMLElement).dataset?.projectSettingsLayout !== undefined) {
        return { width: 800, height: 600, top: 0, left: 0, right: 800, bottom: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
      }
      return rect.call(this)
    }
    try {
      const { container } = render(<ProjectSettings />)
      await waitFor(() => expect(screen.getByTestId('project-settings-detail')).toBeTruthy())
      const page = container.querySelector<HTMLElement>('[data-project-settings-layout]')!
      expect(page.dataset.projectSettingsLayout).toBe('stacked')
      expect(screen.getByRole('complementary', { name: 'Projects' }).className).not.toContain('w-72')
    } finally {
      Element.prototype.getBoundingClientRect = rect
      resetAppZoomForTest()
    }
  })

  it('at 150% app zoom, compares the body’s CSS width: 900 px on screen is 600 CSS px, so the halves stack', async () => {
    applyAppZoomForTest(1.5)
    measured.width = 900
    try {
      await renderReady()
      expect(screen.getByTestId('project-settings-halves').getAttribute('data-layout')).toBe('stacked')
    } finally {
      resetAppZoomForTest()
    }
  })
})

describe('the dashboard selector', () => {
  it('sits in the left half as a sticky bar and stays mounted and visible while the half scrolls', async () => {
    await renderReady()
    const left = screen.getByTestId('project-settings-left')
    const selector = screen.getByTestId('dashboard-selector')
    expect(left.contains(selector)).toBe(true)
    // Sticky to the top AND bottom of the half's own scroll area.
    expect(selector.className).toContain('sticky')
    expect(selector.className).toContain('top-0')
    expect(selector.className).toContain('bottom-0')
    // A direct child of the left half (so its sticky range is the whole
    // half, not just the dashboards part), between General and the
    // dashboard itself.
    expect(selector.parentElement).toBe(left)
    const general = within(left).getByRole('region', { name: 'General' })
    const dashboards = within(left).getByRole('region', { name: 'Dashboards' })
    expect(general.compareDocumentPosition(selector) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(selector.compareDocumentPosition(dashboards) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Its nearest scrolling ancestor is the left half itself.
    let scroller: HTMLElement | null = selector.parentElement
    while (scroller && !scroller.className.includes('overflow-y-auto')) scroller = scroller.parentElement
    expect(scroller).toBe(left)

    left.scrollTop = 400
    fireEvent.scroll(left)
    const after = screen.getByTestId('dashboard-selector')
    expect(after).toBe(selector)
    expect(after.isConnected).toBe(true)
    expect(within(after).getByRole('button', { name: 'Dashboard' })).toBeTruthy()
  })

  it('stays sticky inside the single scroll when the halves stack', async () => {
    measured.width = 480
    await renderReady()
    const halves = screen.getByTestId('project-settings-halves')
    const selector = screen.getByTestId('dashboard-selector')
    expect(selector.className).toContain('sticky')
    let scroller: HTMLElement | null = selector.parentElement
    while (scroller && !scroller.className.includes('overflow-y-auto')) scroller = scroller.parentElement
    expect(scroller).toBe(halves)
    expect(selector.parentElement).toBe(screen.getByTestId('project-settings-left'))
  })

  it('"Add to Main" sends one pane to the daemon, not a whole layout', async () => {
    await renderReady()
    fireEvent.click(screen.getByRole('button', { name: 'Add to Main' }))
    await waitFor(() => expect(projectsApi.addDashboardPane).toHaveBeenCalledTimes(1))
    expect(vi.mocked(projectsApi.addDashboardPane).mock.calls[0].slice(1)).toEqual([
      'g1',
      'd1',
      { kind: 'htmlDoc', workspaceId: 'w2', filePath: '/w/ada/status.html' },
    ])
    expect(projectsApi.saveDashboardLayout).not.toHaveBeenCalled()
    // A non-HTML file says why it can't be added, in plain words.
    expect(screen.getByText('Not an HTML file, so it can’t go on a dashboard.')).toBeTruthy()
  })
})

describe('dead ends', () => {
  it('a failed project list shows the error with Retry, not endless Loading…', async () => {
    useProjectGroupsStore.setState({ groups: null, groupsError: 'daemon unreachable' })
    render(<ProjectSettings />)
    const box = await screen.findByTestId('project-settings-list-error')
    expect(box.textContent).toContain('daemon unreachable')
    expect(screen.queryByText('Loading…')).toBeNull()
    fetchGroups.mockClear()
    fireEvent.click(within(box).getByRole('button', { name: 'Try again' }))
    expect(fetchGroups).toHaveBeenCalledTimes(1)
  })

  it('a failed project shows the error with Retry, and Retry loads it', async () => {
    api.showError = new Error('boom')
    render(<ProjectSettings />)
    const box = await screen.findByTestId('project-settings-detail-error')
    expect(box.textContent).toContain('boom')
    api.showError = null
    fireEvent.click(within(box).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getByTestId('project-settings-detail')).toBeTruthy())
  })

  it('"Open project" opens it on the Projects page and closes Settings', async () => {
    usePageViewStore.setState({ page: 'agents' })
    useProjectGroupsStore.setState({ selectedGroupId: null })
    const closeSettings = vi.fn()
    useSettingsStore.setState({ closeSettings })
    await renderReady()
    fireEvent.click(screen.getByRole('button', { name: 'Open project' }))
    expect(useProjectGroupsStore.getState().selectedGroupId).toBe('g1')
    expect(usePageViewStore.getState().page).toBe('projects')
    expect(closeSettings).toHaveBeenCalledTimes(1)
  })

  it('plain words: no "PoC", "injected" or "rev N"', async () => {
    await renderReady()
    const text = screen.getByTestId('project-settings-detail').textContent ?? ''
    expect(text).not.toMatch(/\bPoC\b/)
    expect(text).not.toMatch(/inject/i)
    expect(text).not.toMatch(/\brev \d/)
    expect(text).toContain('Point of contact')
  })
})

describe('quiet destructive actions, steady paths, search follows', () => {
  it('Delete project is a neutral button; deleting a dashboard and removing an agent live in … menus', async () => {
    await renderReady()
    const del = screen.getByRole('button', { name: 'Delete project' })
    expect(del.className).not.toMatch(/status-error|danger/)
    expect(screen.queryByRole('button', { name: 'Delete dashboard' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'More actions for the dashboard Main' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete dashboard/ }))
    expect(screen.getByTestId('delete-dashboard-confirm')).toBeTruthy()
  })

  it('the point of contact can’t be removed from its menu, and says why', async () => {
    await renderReady()
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Cortana' }))
    const remove = screen.getByRole('menuitem', { name: /Remove from project/ })
    expect(remove.getAttribute('aria-disabled')).toBe('true')
    expect(remove.textContent).toContain('Make another agent the point of contact first.')
    expect(screen.queryByRole('menuitem', { name: /Make point of contact/ })).toBeNull()
  })

  it('each agent but the point of contact has a quiet "Make point of contact" beside its … menu', async () => {
    await renderReady()
    const rows = screen.getAllByTestId('project-agent-row')
    const [poc, ada] = rows
    expect(within(poc).queryByRole('button', { name: 'Make point of contact' })).toBeNull()
    const make = within(ada).getByRole('button', { name: 'Make point of contact' })
    expect(make.className).not.toMatch(/status-error|danger|accent/)
    fireEvent.click(make)
    await waitFor(() => expect(projectsApi.setProjectGroupPoc).toHaveBeenCalledWith(expect.anything(), 'g1', 'w2'))
  })

  it('"Add an agent" by the Agents heading takes you to the picker, ready to type', async () => {
    await renderReady()
    const agents = screen.getByRole('region', { name: 'Agents' })
    fireEvent.click(within(agents).getByRole('button', { name: 'Add an agent' }))
    expect(document.activeElement).toBe(screen.getByLabelText('Search agents to add'))
  })

  it('folders show from the home folder, the full path as the title', () => {
    expect(shortFolder('/Users/alex/Projects/web-app')).toBe('~/Projects/web-app')
    expect(shortFolder('/home/alex/code')).toBe('~/code')
    expect(shortFolder('/Users/alex')).toBe('~')
    expect(shortFolder('/srv/agents/ops')).toBe('/srv/agents/ops')
  })

  it('a search that hides the open project opens the first match; no match shows Pick a project', async () => {
    useProjectGroupsStore.setState({ groups: [GROUP, { ...GROUP, id: 'g2', name: 'Billing' }] })
    await renderReady()
    const search = screen.getByLabelText('Search projects')
    fireEvent.change(search, { target: { value: 'bill' } })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Show settings for Billing' }).getAttribute('aria-current')).toBe('true'))
    fireEvent.change(search, { target: { value: 'zebra' } })
    expect(await screen.findByTestId('project-settings-pick')).toBeTruthy()
    expect(screen.queryByTestId('project-settings-detail')).toBeNull()
  })
})

describe('sheet round 2: rows read first, failures say what next, the bar never covers General', () => {
  it('"Make point of contact" on a row shows on the row’s hover and keyboard focus only (the … menu keeps it)', async () => {
    await renderReady()
    const ada = screen.getAllByTestId('project-agent-row')[1]
    expect(ada.className).toContain('group/agent')
    const make = within(ada).getAllByRole('button').find((b) => b.getAttribute('data-reveal') === 'row')
    expect(make).toBeTruthy()
    expect(make?.className).toContain('opacity-0')
    expect(make?.className).toContain('group-hover/agent:opacity-100')
    expect(make?.className).toContain('group-focus-within/agent:opacity-100')
    fireEvent.click(within(ada).getByRole('button', { name: 'More actions for Ada' }))
    expect(screen.getByRole('menuitem', { name: /Make point of contact/ })).toBeTruthy()
  })

  it('the Add an agent list shows display names (from projects/list), the folder under them, and searches them', async () => {
    useProjectsStore.setState({
      projects: [
        { id: 'w9', name: 'dn-ledger', path: '/w/dn-ledger', agentDisplayName: 'Ledger Desk' },
        // An older daemon's row: no agentDisplayName.
        { id: 'w8', name: 'dn-sales', path: '/w/dn-sales' },
      ],
    } as never)
    await renderReady()
    const list = screen.getByRole('list', { name: 'Agents you can add' })
    await waitFor(() => expect(within(list).getByRole('button', { name: 'Add Ledger Desk' })).toBeTruthy())
    // The names come with the list: no call per folder.
    expect(
      vi.mocked(daemonCli.daemonCliGet).mock.calls.filter(([, route]) => route === 'workspace/agent-display-name'),
    ).toEqual([])
    // No display name: the sidebar name.
    expect(within(list).getByRole('button', { name: 'Add dn-sales' })).toBeTruthy()
    expect(within(list).getByText('/w/dn-ledger')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Search agents to add'), { target: { value: 'desk' } })
    expect(within(list).getByRole('button', { name: 'Add Ledger Desk' })).toBeTruthy()
    expect(within(list).queryByRole('button', { name: 'Add dn-sales' })).toBeNull()
    fireEvent.change(screen.getByLabelText('Search agents to add'), { target: { value: 'zebra' } })
    expect(within(list).getByText('No agents match “zebra”')).toBeTruthy()
  })

  it('a failed change says what failed, then the server’s reason quietly, then Try again (which retries)', async () => {
    vi.mocked(projectsApi.setProjectGroupPoc).mockRejectedValueOnce(new Error('database is locked'))
    await renderReady()
    const ada = screen.getAllByTestId('project-agent-row')[1]
    fireEvent.click(within(ada).getByRole('button', { name: 'More actions for Ada' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /Make point of contact/ }))
    const alert = await within(ada).findByRole('alert')
    const lines = Array.from(alert.querySelectorAll('p')).map((p) => p.textContent)
    expect(lines).toEqual(['Couldn’t change the point of contact. Nothing changed.', 'The server said: database is locked.'])
    vi.mocked(projectsApi.setProjectGroupPoc).mockClear()
    fireEvent.click(within(alert).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(projectsApi.setProjectGroupPoc).toHaveBeenCalledWith(expect.anything(), 'g1', 'w2'))
  })

  it('the custom color field sits right after the swatches, in the row’s own type', async () => {
    await renderReady()
    const field = screen.getByLabelText('Custom color')
    expect(field.className).not.toContain('font-mono')
    expect(field.className).not.toContain('ml-auto')
  })

  it('at 150% app zoom, the bar measures General in CSS px: one that fits the half still lets it stick to the bottom', async () => {
    // General is 560 CSS px (840 on screen); the half is 600 CSS px tall.
    applyAppZoomForTest(1.5)
    const rect = Element.prototype.getBoundingClientRect
    Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
      if (this.firstElementChild?.getAttribute('aria-label') === 'General') {
        return { width: 600, height: 840, top: 0, left: 0, right: 600, bottom: 840, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
      }
      return rect.call(this)
    }
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
      configurable: true,
      get(this: HTMLElement) {
        return this.dataset?.testid === 'project-settings-left' ? 600 : 0
      },
    })
    try {
      await renderReady()
      expect(screen.getByTestId('dashboard-selector').className).toContain('bottom-0')
    } finally {
      delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight
      Element.prototype.getBoundingClientRect = rect
      resetAppZoomForTest()
    }
  })

  it('in a half too short for General and the bar together, the bar sticks to the top only', async () => {
    const fit = { h: 600, leadBottom: 700 }
    const rect = Element.prototype.getBoundingClientRect
    Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
      if (this.firstElementChild?.getAttribute('aria-label') === 'General') {
        return { width: 400, height: fit.leadBottom, top: 0, left: 0, right: 400, bottom: fit.leadBottom, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
      }
      return rect.call(this)
    }
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
      configurable: true,
      get(this: HTMLElement) {
        return this.dataset?.testid === 'project-settings-left' ? fit.h : 0
      },
    })
    try {
      await renderReady()
      const selector = screen.getByTestId('dashboard-selector')
      expect(selector.className).toContain('sticky')
      expect(selector.className).toContain('top-0')
      expect(selector.className).not.toContain('bottom-0')
    } finally {
      delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight
      Element.prototype.getBoundingClientRect = rect
    }
  })
})
