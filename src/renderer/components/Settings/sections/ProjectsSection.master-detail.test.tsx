// @vitest-environment jsdom

// Settings → Workspaces / Agents, side by side: the list on the left, the
// selected workspace's settings on the right, in every theme. A row shows its
// settings at once (no page, no back button); a workspace is selected on
// entry; ↑/↓ move the selection; deep links and search land on the right
// workspace and tab. Below WORKSPACES_TWO_COLUMN_MIN_PX it falls back to one
// page at a time.
//
// The order of the first two tests matters: "last viewed" is remembered for
// the session (module state), so the first test sees nothing remembered.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})), settingsReset: vi.fn() }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))

import { ProjectsSection, WORKSPACES_TWO_COLUMN_MIN_PX } from './ProjectsSection'
import { lastOpenedProject } from './workspace-tabs'
import { searchPickPage } from '../searchManifest'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'

// ── Pane width: jsdom has no layout, so the test sets it ──────────────────
let paneWidth = 1200
const observers = new Set<() => void>()
class FakeResizeObserver {
  private cb: () => void
  constructor(cb: () => void) { this.cb = cb }
  observe(): void { observers.add(this.cb) }
  unobserve(): void {}
  disconnect(): void { observers.delete(this.cb) }
}
function resizePane(width: number): void {
  paneWidth = width
  act(() => { for (const cb of observers) cb() })
}
const realClientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')

function workspace(id: string, name: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, name, path: `/Users/sam/DevProjects/${name}`, color: '#3b82f6', workspaces: [], pinned: 0, focusGroupId: null, ...extra }
}

beforeEach(() => {
  paneWidth = 1200
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get(this: HTMLElement) { return this.dataset.workspacesLayout ? paneWidth : 0 },
  })
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  useSettingsStore.setState({ initialProjectId: null, projectsPage: null, initialWorkspaceTab: null })
  useFocusGroupsStore.setState({
    focusGroups: [{ id: 'g1', name: 'Labs', color: null, tabOrder: 0 }],
    focusGroupsEnabled: true,
  } as never)
  // Listed order: Pinned (cortana), Labs (atlas, beacon), Ungrouped (delta).
  useProjectsStore.setState({
    projects: [
      workspace('a', 'atlas', { focusGroupId: 'g1' }),
      workspace('b', 'beacon', { focusGroupId: 'g1' }),
      workspace('c', 'cortana', { pinned: 1 }),
      workspace('d', 'delta'),
    ],
  } as never)
})

afterEach(() => {
  cleanup()
  observers.clear()
  vi.unstubAllGlobals()
  if (realClientWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', realClientWidth)
  useSettingsStore.setState({ initialProjectId: null, initialWorkspaceTab: null, projectsPage: null })
  useFocusGroupsStore.setState({ focusGroups: [], focusGroupsEnabled: false } as never)
  useProjectsStore.setState({ projects: [] } as never)
})

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-settings-project-id="${id}"]`)
  if (!el) throw new Error(`row ${id} not rendered`)
  return el
}

function selectedRowIds(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-settings-project-id][aria-current="true"]'))
    .map((el) => el.dataset.settingsProjectId ?? '')
}

/** The workspace whose settings are showing, by its header. */
function shownName(): string | null {
  return screen.queryByRole('heading', { level: 2 })?.textContent ?? null
}

describe('side by side', () => {
  it('selects the first listed workspace on entry (nothing viewed yet), with the list beside it', () => {
    render(<ProjectsSection />)
    expect(document.querySelector('[data-workspaces-layout="two-column"]')).toBeTruthy()
    // Pinned comes first in the list.
    expect(selectedRowIds()).toEqual(['c'])
    expect(shownName()).toBe('cortana')
    expect(screen.getByRole('tablist', { name: 'Workspace settings' })).toBeTruthy()
    // No page to go back from.
    expect(screen.queryByTestId('workspaces-back')).toBeNull()
    // Every workspace is still listed.
    for (const id of ['a', 'b', 'c', 'd']) expect(row(id)).toBeTruthy()
    expect(lastOpenedProject()).toBe('c')
  })

  it('a row shows its settings at once, keeps the list, and is the one remembered next time', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('b'))
    expect(selectedRowIds()).toEqual(['b'])
    expect(shownName()).toBe('beacon')
    expect(row('a')).toBeTruthy()
    cleanup()
    render(<ProjectsSection />)
    expect(selectedRowIds()).toEqual(['b'])
    expect(shownName()).toBe('beacon')
  })

  it('rows, their pin buttons and the tabs draw keyboard focus in the theme\'s shape', () => {
    render(<ProjectsSection />)
    for (const id of ['a', 'b', 'c', 'd']) {
      expect(row(id).hasAttribute('data-k2-focus')).toBe(true)
      expect(row(id).className).toContain('relative')
      expect(row(id).className).not.toContain('focus-visible:outline')
      const pin = within(row(id)).getByRole('button', { pressed: id === 'c' })
      expect(pin.hasAttribute('data-k2-focus')).toBe(true)
    }
    for (const tab of screen.getAllByRole('tab')) {
      expect(tab.hasAttribute('data-k2-focus')).toBe(true)
      expect(tab.className).toContain('relative')
      expect(tab.className).not.toContain('focus-visible:outline')
    }
  })

  it('empty lists on Connections and Advanced say what would show there and how to add one', async () => {
    render(<ProjectsSection />)
    fireEvent.click(screen.getByRole('tab', { name: 'Connections' }))
    expect((await screen.findByTestId('connections-local-empty')).textContent).toContain('Use Add connection')
    expect((await screen.findByTestId('connections-remote-empty')).textContent).toContain('Use Add connection')
    fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }))
    expect(screen.getByTestId('worktrees-empty').textContent).toContain('New Worktree')
    const keys = await screen.findByTestId('workspace-api-keys-empty')
    expect(keys.textContent).toContain('Create one in Settings')
    expect(within(keys).getByRole('button', { name: 'Create a key' })).toBeTruthy()
  })

  it('Advanced worktree rows start on the card\'s text edge (no leading icon)', () => {
    useProjectsStore.setState({
      projects: [workspace('c', 'cortana', { pinned: 1, workspaces: [{ id: 'w1', name: 'main', branch: 'main', type: 'branch' }] })],
    } as never)
    render(<ProjectsSection />)
    fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }))
    const rowEl = document.querySelector<HTMLElement>('[data-worktree-row]')
    expect(rowEl?.className).toContain('px-4')
    expect(rowEl?.querySelector('svg')).toBeNull()
  })

  it('keeps the tab while moving between workspaces', () => {
    render(<ProjectsSection />)
    fireEvent.click(screen.getByRole('tab', { name: 'Permissions' }))
    fireEvent.click(row('d'))
    expect(shownName()).toBe('delta')
    expect(screen.getByRole('tab', { name: 'Permissions' }).getAttribute('aria-selected')).toBe('true')
    expect(document.querySelector('[data-settings-id="projects.agents-can-manage-agents"]')).toBeTruthy()
  })

  it('↑/↓ in search move the selection through the list as painted', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('c'))
    const search = screen.getByRole('searchbox', { name: 'Search workspaces' })
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    expect(shownName()).toBe('atlas')
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    expect(shownName()).toBe('beacon')
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    // Clamps at the end.
    expect(shownName()).toBe('delta')
    fireEvent.keyDown(search, { key: 'ArrowUp' })
    expect(selectedRowIds()).toEqual(['b'])
  })

  it('↑/↓ on a focused row move focus and the selection; Enter on the pin button pins, not selects', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('a'))
    row('a').focus()
    fireEvent.keyDown(row('a'), { key: 'ArrowDown' })
    expect(shownName()).toBe('beacon')
    expect(document.activeElement).toBe(row('b'))
    fireEvent.keyDown(row('b'), { key: 'ArrowUp' })
    fireEvent.keyDown(row('a'), { key: 'ArrowUp' })
    expect(shownName()).toBe('cortana')
    expect(document.activeElement).toBe(row('c'))
    // A key on the pin button stays the button's.
    const pin = within(row('d')).getByRole('button', { name: 'Pin delta to top' })
    fireEvent.keyDown(pin, { key: 'Enter' })
    expect(shownName()).toBe('cortana')
  })

  it('a search that hides the shown workspace selects the first match; one that still matches stays', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('b'))
    const search = screen.getByRole('searchbox', { name: 'Search workspaces' })
    // "a" still matches beacon: it stays.
    fireEvent.change(search, { target: { value: 'a' } })
    expect(shownName()).toBe('beacon')
    expect(selectedRowIds()).toEqual(['b'])
    // "del" hides it: the first match is selected, list and pane agree.
    fireEvent.change(search, { target: { value: 'del' } })
    expect(document.querySelector('[data-settings-project-id="b"]')).toBeNull()
    expect(shownName()).toBe('delta')
    expect(selectedRowIds()).toEqual(['d'])
    // Nothing matches: the pane keeps the last one, the list says so.
    fireEvent.change(search, { target: { value: 'zzz' } })
    expect(shownName()).toBe('delta')
    expect(screen.getByTestId('workspaces-no-match')).toBeTruthy()
  })

  it('the New workspace footer is solid, so the list scrolls under it', () => {
    render(<ProjectsSection />)
    const footer = document.querySelector<HTMLElement>('[data-workspaces-footer]')
    expect(footer?.className).toContain('bg-[var(--color-bg)]')
    expect(footer?.className).toContain('border-t')
    expect(within(footer!).getByRole('button', { name: 'New workspace' })).toBeTruthy()
  })

  it('Workspace settings is selectable in the list and shows on the right', () => {
    render(<ProjectsSection />)
    const entry = screen.getByTestId('workspace-settings-entry')
    fireEvent.click(entry)
    expect(screen.getByTestId('workspace-settings-page')).toBeTruthy()
    expect(entry.getAttribute('aria-current')).toBe('true')
    expect(selectedRowIds()).toEqual([])
    expect(row('a')).toBeTruthy()
    fireEvent.click(row('a'))
    expect(screen.queryByTestId('workspace-settings-page')).toBeNull()
    expect(shownName()).toBe('atlas')
    expect(entry.getAttribute('aria-current')).toBeNull()
  })

  it('the list keeps its controls: Focus Groups, sort, New workspace, the group menu and its Delete confirmation', async () => {
    const confirm = vi.fn(async () => false)
    const realConfirm = useConfirmDialogStore.getState().confirm
    useConfirmDialogStore.setState({ confirm } as never)
    try {
      render(<ProjectsSection />)
      const list = screen.getByRole('complementary', { name: 'Workspaces' })
      expect(within(list).getByRole('switch', { name: 'Focus Groups' })).toBeTruthy()
      expect(within(list).getByRole('button', { name: 'Sort workspaces' })).toBeTruthy()
      expect(list.querySelector('[data-settings-id="projects.add"]')?.textContent).toBe('New workspace')
      expect(within(list).getByRole('textbox', { name: 'New focus group name' })).toBeTruthy()
      fireEvent.click(within(list).getByRole('button', { name: 'More actions for Labs' }))
      await act(async () => {
        fireEvent.click(await screen.findByRole('menuitem', { name: /Delete group/ }))
      })
      await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    } finally {
      useConfirmDialogStore.setState({ confirm: realConfirm } as never)
    }
  })
})

describe('deep links and search land on the right workspace and tab', () => {
  it('a deep link selects its workspace on its tab', () => {
    useSettingsStore.setState({ initialProjectId: 'd', initialWorkspaceTab: 'schedule' })
    render(<ProjectsSection />)
    expect(selectedRowIds()).toEqual(['d'])
    expect(shownName()).toBe('delta')
    expect(screen.getByRole('tab', { name: 'Heartbeats' }).getAttribute('aria-selected')).toBe('true')
    expect(document.querySelector('[data-settings-id="projects.heartbeat"]')).toBeTruthy()
  })

  it('a deep link while open moves the selection, and the same link works again after picking another row', () => {
    render(<ProjectsSection />)
    act(() => useSettingsStore.setState({ initialProjectId: 'b', initialWorkspaceTab: 'connections' }))
    expect(shownName()).toBe('beacon')
    expect(screen.getByRole('tab', { name: 'Connections' }).getAttribute('aria-selected')).toBe('true')
    fireEvent.click(row('a'))
    expect(useSettingsStore.getState().initialProjectId).toBeNull()
    act(() => useSettingsStore.setState({ initialProjectId: 'b', initialWorkspaceTab: 'permissions' }))
    expect(shownName()).toBe('beacon')
    expect(screen.getByRole('tab', { name: 'Permissions' }).getAttribute('aria-selected')).toBe('true')
  })

  it('a search pick for a per-workspace setting lands on the workspace showing, on its tab, at its row', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('b'))
    const page = searchPickPage({ section: 'projects', id: 'projects.agents-can-use-compute' })
    expect(page).toEqual({ workspaceTab: 'permissions' })
    // What Settings.handlePick does with it.
    act(() => useSettingsStore.setState({ initialProjectId: lastOpenedProject(), initialWorkspaceTab: page.workspaceTab ?? null }))
    expect(shownName()).toBe('beacon')
    expect(document.querySelector('[data-settings-id="projects.agents-can-use-compute"]')).toBeTruthy()
  })

  it('a search pick for a list setting keeps the workspace showing and finds the anchor in the list', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('b'))
    fireEvent.click(screen.getByTestId('workspace-settings-entry'))
    expect(searchPickPage({ section: 'projects', id: 'projects.focus-groups' })).toEqual({ projectsPage: 'list' })
    act(() => useSettingsStore.setState({ projectsPage: 'list' }))
    expect(screen.queryByTestId('workspace-settings-page')).toBeNull()
    expect(shownName()).toBe('beacon')
    const list = screen.getByRole('complementary', { name: 'Workspaces' })
    expect(list.querySelector('[data-settings-id="projects.focus-groups"]')).toBeTruthy()
    expect(list.getAttribute('data-settings-id')).toBe('projects.list')
  })

  it('the Workspace settings deep link shows that page beside the list', () => {
    useSettingsStore.setState({ projectsPage: 'workspace-settings' })
    render(<ProjectsSection />)
    expect(screen.getByTestId('workspace-settings-page')).toBeTruthy()
    expect(row('a')).toBeTruthy()
  })
})

describe('narrow panes fall back to one page at a time', () => {
  it(`below ${WORKSPACES_TWO_COLUMN_MIN_PX}px: the list, then a page with All workspaces`, () => {
    paneWidth = WORKSPACES_TWO_COLUMN_MIN_PX - 1
    render(<ProjectsSection />)
    expect(document.querySelector('[data-workspaces-layout="single"]')).toBeTruthy()
    expect(screen.queryByRole('tablist')).toBeNull()
    expect(selectedRowIds()).toEqual([])
    fireEvent.click(row('a'))
    expect(document.querySelector('[data-settings-project-id]')).toBeNull()
    expect(shownName()).toBe('atlas')
    fireEvent.click(screen.getByTestId('workspaces-back'))
    expect(row('a')).toBeTruthy()
    expect(screen.queryByRole('tablist')).toBeNull()
  })

  it('↑/↓ in search highlight and Enter opens, as before', () => {
    paneWidth = 600
    render(<ProjectsSection />)
    const search = screen.getByRole('searchbox', { name: 'Search workspaces' })
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    expect(screen.queryByRole('tablist')).toBeNull()
    fireEvent.keyDown(search, { key: 'Enter' })
    expect(shownName()).toBe('atlas')
  })

  it('switches layout as the pane resizes, keeping the workspace', () => {
    render(<ProjectsSection />)
    fireEvent.click(row('b'))
    resizePane(WORKSPACES_TWO_COLUMN_MIN_PX - 40)
    expect(document.querySelector('[data-settings-project-id]')).toBeNull()
    expect(shownName()).toBe('beacon')
    expect(screen.getByTestId('workspaces-back')).toBeTruthy()
    resizePane(WORKSPACES_TWO_COLUMN_MIN_PX)
    expect(screen.queryByTestId('workspaces-back')).toBeNull()
    expect(selectedRowIds()).toEqual(['b'])
  })

  it('with no workspaces it is the one-page empty state, however wide', async () => {
    useProjectsStore.setState({ projects: [] } as never)
    render(<ProjectsSection />)
    expect(document.querySelector('[data-workspaces-layout="single"]')).toBeTruthy()
    expect(screen.queryByRole('complementary', { name: 'Workspaces' })).toBeNull()
    expect(screen.getByRole('heading', { name: 'Workspaces / Agents' })).toBeTruthy()
  })

  it('while the list loads, a spinner shows and New workspace is not the loud button', () => {
    useProjectsStore.setState({ projects: [] } as never)
    render(<ProjectsSection />)
    const loading = screen.getByTestId('workspaces-loading')
    expect(loading.querySelector('[data-k2-spinner]')).toBeTruthy()
    const add = screen.getByRole('button', { name: 'New workspace' })
    expect(add.className).not.toContain('accent-fill')
  })
})
