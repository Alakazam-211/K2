// @vitest-environment jsdom
//
// prd-projects-threads-v1 P5: Settings → Projects → General → "Agents open in".
//   - shows the project's default view (Thread when the daemon sends none);
//   - a pick saves through set-default-view, at once (optimistic);
//   - a refusal rolls the pick back and says why, inline;
//   - a Member sees it disabled with a one-line reason; an older daemon too;
//   - a saved change drops this person's tile switches made against the old
//     default (the store test covers other clients' changes, via the
//     groups-changed refetch);
//   - Settings search finds it.

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
    setProjectGroupDefaultView: vi.fn(),
  }
})

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ProjectSettings from './ProjectSettings'
import * as projectsApi from './projects-api'
import { PROJECT_GROUPS_MANIFEST } from './project-settings'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { tileViewKey } from './tile-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { ProjectGroup, ProjectGroupShow } from './projects-api'

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
  memberCount: 1,
  defaultView: 'thread',
}
const SHOW: ProjectGroupShow = {
  ...GROUP,
  members: [{ workspaceId: 'w1', name: 'cortana', path: '/w/cortana', agentName: 'Cortana', createdAt: 1 }],
  dashboards: [
    { id: 'd1', groupId: 'g1', name: 'Main', layoutJson: '{"version":1,"panes":[]}', revision: 1, position: 0, createdAt: 1, updatedAt: 1 },
  ],
}

const setDefaultView = vi.mocked(projectsApi.setProjectGroupDefaultView)

function row(): HTMLElement {
  return screen.getByTestId('project-default-view-row')
}

function segment(name: string): HTMLButtonElement {
  return within(row()).getByRole('radio', { name }) as HTMLButtonElement
}

function checked(): string | undefined {
  return within(row())
    .getAllByRole('radio')
    .find((r) => r.getAttribute('aria-checked') === 'true')?.textContent ?? undefined
}

async function renderReady(): Promise<void> {
  render(<ProjectSettings />)
  await waitFor(() => expect(screen.getByTestId('project-default-view-row')).toBeTruthy())
  // Let whoami land (the role decides whether the row is locked).
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

beforeEach(() => {
  viewerRole.current = 'owner'
  api.show = SHOW
  setDefaultView.mockReset()
  setDefaultView.mockImplementation(async (_scope, _group, view) => ({ ...GROUP, defaultView: view }))
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

describe('Agents open in', () => {
  it("shows the project's current default, right after Color", async () => {
    api.show = { ...SHOW, defaultView: 'chatter' }
    await renderReady()
    expect(within(row()).getByText('Agents open in')).toBeTruthy()
    expect(within(row()).getByRole('radiogroup', { name: 'Agents open in' })).toBeTruthy()
    expect(checked()).toBe('Chatter')
    const ids = Array.from(document.querySelectorAll('[data-settings-id]')).map((el) =>
      el.getAttribute('data-settings-id'),
    )
    expect(ids.indexOf('project-groups.default-view')).toBe(ids.indexOf('project-groups.color') + 1)
    expect(segment('Terminal').disabled).toBe(false)
  })

  it('reads an unknown word as Thread', async () => {
    api.show = { ...SHOW, defaultView: 'split' as never }
    await renderReady()
    expect(checked()).toBe('Thread')
  })

  it('saves a pick through set-default-view, showing it at once', async () => {
    let answer: (g: ProjectGroup) => void = () => {}
    setDefaultView.mockImplementation(() => new Promise((r) => (answer = r)))
    await renderReady()
    fireEvent.click(segment('Terminal'))
    expect(setDefaultView).toHaveBeenCalledTimes(1)
    expect(setDefaultView.mock.calls[0].slice(1)).toEqual(['g1', 'terminal'])
    // Optimistic: the row and the store's list move before the daemon answers.
    expect(checked()).toBe('Terminal')
    expect(within(row()).getByText('Saving…')).toBeTruthy()
    expect(useProjectGroupsStore.getState().groups?.[0].defaultView).toBe('terminal')
    await act(async () => answer({ ...GROUP, defaultView: 'terminal' }))
    expect(checked()).toBe('Terminal')
    expect(within(row()).queryByText('Saving…')).toBeNull()
    expect(within(row()).queryByRole('alert')).toBeNull()
  })

  it('pressing the chosen view saves nothing', async () => {
    await renderReady()
    fireEvent.click(segment('Thread'))
    expect(setDefaultView).not.toHaveBeenCalled()
  })

  it('rolls back with an inline error when the daemon refuses', async () => {
    setDefaultView.mockRejectedValue(new DaemonHttpError('owner or admin only', 403, 'forbidden'))
    await renderReady()
    fireEvent.click(segment('Chatter'))
    const alert = await within(row()).findByRole('alert')
    expect(alert.textContent).toBe("Couldn't change it: only the owner or an admin can.")
    expect(checked()).toBe('Thread')
    expect(useProjectGroupsStore.getState().groups?.[0].defaultView).toBe('thread')
  })

  it('a network failure says what went wrong, and the next try clears it', async () => {
    setDefaultView.mockRejectedValueOnce(new Error('connection refused'))
    await renderReady()
    fireEvent.click(segment('Terminal'))
    const alert = await within(row()).findByRole('alert')
    expect(alert.textContent).toBe("Couldn't change it: connection refused")
    fireEvent.click(segment('Terminal'))
    await waitFor(() => expect(within(row()).queryByRole('alert')).toBeNull())
    expect(checked()).toBe('Terminal')
  })

  it('a Member sees the value, disabled, with one line saying why', async () => {
    viewerRole.current = 'member'
    api.show = { ...SHOW, defaultView: 'terminal' }
    await renderReady()
    await waitFor(() => expect(segment('Thread').disabled).toBe(true))
    expect(segment('Terminal').disabled).toBe(true)
    expect(segment('Chatter').disabled).toBe(true)
    expect(checked()).toBe('Terminal')
    expect(screen.getByTestId('project-default-view-locked').textContent).toBe(
      'Only the owner or an admin can change this.',
    )
    fireEvent.click(segment('Chatter'))
    expect(setDefaultView).not.toHaveBeenCalled()
    // The page banner names the row too.
    expect(screen.getByTestId('project-settings-readonly-banner').textContent).toMatch(
      /the view its agents open in/,
    )
  })

  it('an older daemon (no defaultView) shows Thread, disabled, and says why', async () => {
    const { defaultView: _omit, ...old } = SHOW
    api.show = old
    await renderReady()
    expect(checked()).toBe('Thread')
    expect(segment('Terminal').disabled).toBe(true)
    expect(screen.getByTestId('project-default-view-locked').textContent).toBe(
      'Needs a newer K2 on this server.',
    )
  })

  it("a saved change drops this person's tile switches made against the old default", async () => {
    const mine = tileViewKey('local', 'g1', 'w1')
    const other = tileViewKey('local', 'g2', 'w1')
    useProjectGroupsStore.setState({
      tileViews: {
        [mine]: { view: 'chatter', against: 'thread' },
        [other]: { view: 'chatter', against: 'thread' },
      },
    })
    await renderReady()
    fireEvent.click(segment('Terminal'))
    await waitFor(() => expect(useProjectGroupsStore.getState().tileViews[mine]).toBeUndefined())
    // Another project's switch is not this project's business.
    expect(useProjectGroupsStore.getState().tileViews[other]).toEqual({ view: 'chatter', against: 'thread' })
  })

  it('a refused change keeps the tile switches', async () => {
    const mine = tileViewKey('local', 'g1', 'w1')
    useProjectGroupsStore.setState({ tileViews: { [mine]: { view: 'chatter', against: 'thread' } } })
    setDefaultView.mockRejectedValue(new Error('nope'))
    await renderReady()
    fireEvent.click(segment('Terminal'))
    await within(row()).findByRole('alert')
    expect(useProjectGroupsStore.getState().tileViews[mine]).toEqual({ view: 'chatter', against: 'thread' })
  })

  it('Settings search finds it', () => {
    const entry = PROJECT_GROUPS_MANIFEST.find((e) => e.id === 'project-groups.default-view')
    expect(entry?.label).toBe('Agents open in')
    expect(entry?.keywords).toEqual(expect.arrayContaining(['view', 'thread', 'terminal', 'chatter', 'default', 'open']))
  })
})
