// @vitest-environment jsdom
//
// 0302: each agent row in a project's settings (Settings → Projects and the
// Projects page's sheet) has "Opens in": the view its tile opens in for
// everyone.
//   - it shows the agent's own view, or the project's marked "(default)";
//   - the menu sets one through set-member-default-view, or clears it with
//     "Project default (Thread)" (view: null), shown at once;
//   - a Member sees it as text with the reason; an older daemon too;
//   - a 403 is one plain line; any other failure says so with Try again.

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
const api = vi.hoisted(() => ({ show: null as unknown }))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async () => api.show),
    fetchProjectGroupResources: vi.fn(async () => []),
    fetchProjectGroupIcon: vi.fn(async () => ({ found: false, dataUrl: null })),
    setProjectGroupMemberDefaultView: vi.fn(),
  }
})

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ProjectSettings from './ProjectSettings'
import * as projectsApi from './projects-api'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { ProjectGroup, ProjectGroupMemberInfo, ProjectGroupShow } from './projects-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}
class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
  unobserve(): void {}
}
;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver

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
  defaultView: 'thread',
}
const CORTANA: ProjectGroupMemberInfo = {
  workspaceId: 'w1',
  name: 'cortana',
  path: '/w/cortana',
  agentName: 'Cortana',
  createdAt: 1,
  defaultView: 'thread',
  ownDefaultView: null,
}
const ADA: ProjectGroupMemberInfo = {
  workspaceId: 'w2',
  name: 'ada',
  path: '/w/ada',
  agentName: 'Ada',
  createdAt: 2,
  defaultView: 'terminal',
  ownDefaultView: 'terminal',
}
const SHOW: ProjectGroupShow = {
  ...GROUP,
  members: [CORTANA, ADA],
  dashboards: [
    { id: 'd1', groupId: 'g1', name: 'Main', layoutJson: '{"version":1,"panes":[]}', revision: 1, position: 0, createdAt: 1, updatedAt: 1 },
  ],
}

const setMemberView = vi.mocked(projectsApi.setProjectGroupMemberDefaultView)

function agentRow(name: string): HTMLElement {
  const row = screen.getAllByTestId('project-agent-row').find((r) => r.textContent?.includes(name))
  if (!row) throw new Error(`no row for ${name}`)
  return row
}

function opensIn(name: string): HTMLElement {
  return within(agentRow(name)).getByTestId('agent-opens-in')
}

async function renderReady(): Promise<void> {
  render(<ProjectSettings />)
  await waitFor(() => expect(screen.getAllByTestId('project-agent-row')).toHaveLength(2))
  // Let whoami land (the role decides whether it can be changed).
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

function pick(name: string, item: RegExp | string): void {
  fireEvent.click(opensIn(name))
  fireEvent.click(screen.getByRole('menuitemradio', { name: item }))
}

beforeEach(() => {
  viewerRole.current = 'owner'
  api.show = SHOW
  setMemberView.mockReset()
  setMemberView.mockImplementation(async (_scope, group, workspace, view) => ({
    ok: true,
    groupId: group,
    workspaceId: workspace,
    defaultView: view ?? 'thread',
    ownDefaultView: view,
  }))
  useProjectGroupsStore.setState({
    groups: [GROUP],
    groupsError: null,
    selectedGroupId: 'g1',
    revision: 0,
    tileViews: {},
    fetchGroups: vi.fn(async () => {}),
  })
  useProjectsStore.setState({ projects: [] } as never)
  useSettingsStore.setState({ initialProjectGroupId: 'g1' })
})

afterEach(() => {
  cleanup()
})

describe('Opens in, per agent', () => {
  it("shows the agent's own view, or the project's marked as the default", async () => {
    await renderReady()
    expect(opensIn('Cortana').textContent).toBe('Opens inThread (default)')
    expect(opensIn('Ada').textContent).toBe('Opens inTerminal')
    expect(opensIn('Ada').getAttribute('data-own-view')).toBe('terminal')
    expect(opensIn('Cortana').getAttribute('data-own-view')).toBeNull()
  })

  it('the menu lists the project default, then the three views, the current one checked', async () => {
    await renderReady()
    fireEvent.click(opensIn('Cortana'))
    const radios = screen.getAllByRole('menuitemradio')
    expect(radios.map((r) => r.textContent?.replace('Follows the project’s view.', ''))).toEqual([
      'Project default (Thread)',
      'Terminal',
      'Thread',
      'Chatter',
    ])
    expect(radios.map((r) => r.getAttribute('aria-checked'))).toEqual(['true', 'false', 'false', 'false'])
  })

  it('a pick saves through set-member-default-view, showing it at once', async () => {
    let resolve: (() => void) | null = null
    setMemberView.mockImplementationOnce(
      (_s, group, workspace, view) =>
        new Promise((r) => {
          resolve = () => r({ ok: true, groupId: group, workspaceId: workspace, defaultView: view ?? 'thread', ownDefaultView: view })
        }),
    )
    await renderReady()
    pick('Cortana', 'Chatter')
    expect(setMemberView).toHaveBeenCalledWith(expect.anything(), 'g1', 'w1', 'chatter')
    // Optimistic: shown before the daemon answers.
    expect(opensIn('Cortana').textContent).toBe('Opens inChatter')
    await act(async () => {
      resolve!()
    })
    await waitFor(() => expect(within(agentRow('Cortana')).getByText('Opens in Chatter now')).toBeTruthy())
  })

  it('"Project default (Thread)" clears the agent\'s own view (view: null)', async () => {
    await renderReady()
    pick('Ada', /Project default \(Thread\)/)
    expect(setMemberView).toHaveBeenCalledWith(expect.anything(), 'g1', 'w2', null)
    await waitFor(() => expect(opensIn('Ada').textContent).toBe('Opens inThread (default)'))
  })

  it('picking the current choice saves nothing', async () => {
    await renderReady()
    pick('Ada', 'Terminal')
    expect(setMemberView).not.toHaveBeenCalled()
  })

  it('a Member sees it as text, with the reason, and no menu', async () => {
    viewerRole.current = 'member'
    await renderReady()
    const el = opensIn('Ada')
    expect(el.tagName).toBe('SPAN')
    expect(el.hasAttribute('data-locked')).toBe(true)
    expect(el.getAttribute('title')).toBe('Opens in Terminal. Only the owner or an admin can change where an agent opens.')
    expect(opensIn('Cortana').getAttribute('title')).toContain('Opens in Thread, the project default.')
    expect(within(agentRow('Ada')).queryByRole('button', { name: /change where Ada opens/ })).toBeNull()
  })

  it('an older daemon (no per-agent view) shows the project view, locked, and says why', async () => {
    api.show = {
      ...SHOW,
      members: SHOW.members.map(({ defaultView: _a, ownDefaultView: _b, ...m }) => m),
    }
    await renderReady()
    expect(opensIn('Ada').getAttribute('title')).toBe('Opens in Thread, the project default. Needs a newer K2 on this server.')
  })

  it('a 403 rolls back and says so in one line, no Try again', async () => {
    setMemberView.mockRejectedValueOnce(new DaemonHttpError('only the owner…', 403, 'forbidden'))
    await renderReady()
    pick('Cortana', 'Terminal')
    const alert = await within(agentRow('Cortana')).findByRole('alert')
    expect(alert.textContent).toBe('Couldn’t change it. Only the owner or an admin can.')
    expect(within(agentRow('Cortana')).queryByRole('button', { name: 'Try again' })).toBeNull()
    expect(opensIn('Cortana').textContent).toBe('Opens inThread (default)')
  })

  it('any other failure says what failed, the reason quietly, and Try again retries', async () => {
    setMemberView.mockRejectedValueOnce(new Error('database is locked'))
    await renderReady()
    pick('Cortana', 'Terminal')
    const alert = await within(agentRow('Cortana')).findByRole('alert')
    expect(alert.textContent).toContain('Couldn’t change where Cortana opens. Nothing changed.')
    expect(alert.textContent).toContain('The server said: database is locked.')
    fireEvent.click(within(alert).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(setMemberView).toHaveBeenCalledTimes(2))
    expect(setMemberView).toHaveBeenLastCalledWith(expect.anything(), 'g1', 'w1', 'terminal')
    await waitFor(() => expect(within(agentRow('Cortana')).queryByRole('alert')).toBeNull())
    expect(opensIn('Cortana').textContent).toBe('Opens inTerminal')
  })
})
