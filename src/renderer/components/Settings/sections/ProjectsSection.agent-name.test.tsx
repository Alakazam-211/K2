// @vitest-environment jsdom

// Settings → Workspaces / Agents names each workspace by its AGENT: the
// list row and the page header show the agent's display name (projects/list
// `agentDisplayName`, AGENT.md `display_name`), with the folder path as the
// second line, the same name the nav and Home show. A rename (the daemon's
// `agent_identity_changed`) repaints both at once, search finds the agent by
// that name, and a row with no display name (an older daemon) keeps the
// registry name.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})), settingsReset: vi.fn() }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))

import { ProjectsSection } from './ProjectsSection'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { applyAgentIdentityChanged, useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import type { AgentIdentityChangedEvent } from '@/stores/session-events'

// jsdom has no layout: give the Workspaces pane a wide width (side by side).
const observers = new Set<() => void>()
class FakeResizeObserver {
  private cb: () => void
  constructor(cb: () => void) { this.cb = cb }
  observe(): void { observers.add(this.cb) }
  unobserve(): void {}
  disconnect(): void { observers.delete(this.cb) }
}
const realClientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')

const FOLDER = '/srv/agents/North Desk'

function workspace(id: string, name: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, name, path: `/srv/agents/${name}`, color: '#3b82f6', workspaces: [], pinned: 0, focusGroupId: null, ...extra }
}

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get(this: HTMLElement) { return this.dataset.workspacesLayout ? 1200 : 0 },
  })
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  useSettingsStore.setState({ initialProjectId: 'n', projectsPage: null, initialWorkspaceTab: null })
  useFocusGroupsStore.setState({ focusGroups: [], focusGroupsEnabled: false } as never)
  useProjectsStore.setState({
    projects: [
      // Renamed once already: the folder is "North Desk", the agent is "Wren".
      workspace('n', 'North Desk', { agentDisplayName: 'Wren' }),
      // An older daemon sends no display name.
      workspace('o', 'Oak Room'),
      // A blank display name falls back too.
      workspace('p', 'Pine Hall', { agentDisplayName: '  ' }),
    ],
  } as never)
})

afterEach(() => {
  cleanup()
  observers.clear()
  vi.unstubAllGlobals()
  if (realClientWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', realClientWidth)
  useSettingsStore.setState({ initialProjectId: null, initialWorkspaceTab: null, projectsPage: null })
  useProjectsStore.setState({ projects: [] } as never)
})

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-settings-project-id="${id}"]`)
  if (!el) throw new Error(`row ${id} not rendered`)
  return el
}

function rowIds(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-settings-project-id]'))
    .map((el) => el.dataset.settingsProjectId ?? '')
}

/** The header of the workspace whose settings are showing. */
function header(): HTMLElement {
  const h = screen.getByRole('heading', { level: 2 })
  if (!h.parentElement) throw new Error('header has no container')
  return h.parentElement
}

function renamed(displayName: string): AgentIdentityChangedEvent {
  return {
    kind: 'agent_identity_changed',
    projectId: 'n',
    workspacePath: FOLDER,
    fields: ['displayName', 'name'],
    // The registry name the daemon sends with the event: unchanged here,
    // so only the display name moves.
    name: 'North Desk',
    displayName,
    handle: 'north-desk',
    color: '#3b82f6',
  }
}

describe('Settings → Workspaces / Agents names the agent', () => {
  it('the list row and the page header show the display name, with the folder path under it', () => {
    render(<ProjectsSection />)
    const r = row('n')
    expect(within(r).getByText('Wren')).toBeTruthy()
    expect(within(r).getByText(FOLDER)).toBeTruthy()
    expect(within(r).queryByText('North Desk')).toBeNull()
    expect(r.getAttribute('aria-label')).toBe('Show settings for Wren')

    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Wren')
    expect(within(header()).getByText(FOLDER)).toBeTruthy()
  })

  it('a rename repaints the row and the header at once, without reopening Settings', () => {
    render(<ProjectsSection />)
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Wren')

    act(() => { applyAgentIdentityChanged(renamed('Juniper')) })

    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Juniper')
    expect(within(row('n')).getByText('Juniper')).toBeTruthy()
    expect(within(row('n')).queryByText('Wren')).toBeNull()
    expect(within(row('n')).getByText(FOLDER)).toBeTruthy()
  })

  it('search finds the agent by its display name, and still by its folder', () => {
    render(<ProjectsSection />)
    const search = screen.getByRole('searchbox', { name: 'Search workspaces' })

    fireEvent.change(search, { target: { value: 'wren' } })
    expect(rowIds()).toEqual(['n'])

    fireEvent.change(search, { target: { value: 'north' } })
    expect(rowIds()).toEqual(['n'])

    // After a rename, the new name finds it.
    act(() => { applyAgentIdentityChanged(renamed('Juniper')) })
    fireEvent.change(search, { target: { value: 'junip' } })
    expect(rowIds()).toEqual(['n'])
  })

  it('with no display name, a row and its header keep the registry name', () => {
    render(<ProjectsSection />)
    expect(within(row('o')).getByText('Oak Room')).toBeTruthy()
    expect(within(row('p')).getByText('Pine Hall')).toBeTruthy()

    fireEvent.click(row('o'))
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Oak Room')
    fireEvent.click(row('p'))
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Pine Hall')

    const search = screen.getByRole('searchbox', { name: 'Search workspaces' })
    fireEvent.change(search, { target: { value: 'oak' } })
    expect(rowIds()).toEqual(['o'])
  })
})
