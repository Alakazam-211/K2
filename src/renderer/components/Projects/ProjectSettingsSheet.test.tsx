// @vitest-environment jsdom
//
// Project settings as a sheet over the Projects page (projects reorg §3.6):
//   - the store action opens it on one project: that project's settings,
//     no project list, no "Open project", a × that closes it;
//   - Esc closes it (a field's own Esc goes first), focus goes back to
//     what opened it, and Tab stays inside it;
//   - deleting the project from it closes it and moves the page to the
//     next project (or none), with a note saying what happened;
//   - off the Projects page, the same entry opens Settings → Projects.

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
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))
const api = vi.hoisted(() => ({ showError: null as Error | null }))
vi.mock('./projects-api', async () => {
  const actual = await vi.importActual<typeof import('./projects-api')>('./projects-api')
  return {
    ...actual,
    fetchProjectGroupShow: vi.fn(async (_s: unknown, id: string) => {
      if (api.showError) throw api.showError
      return { ...SHOWS[id] }
    }),
    fetchProjectGroupResources: vi.fn(async () => []),
    fetchProjectGroupIcon: vi.fn(async () => ({ found: false, dataUrl: null })),
    deleteProjectGroup: vi.fn(async () => undefined),
  }
})

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import ProjectSettingsSheet, { openProjectSettings, projectAfterDelete } from './ProjectSettingsSheet'
import * as projectsApi from './projects-api'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import type { ProjectGroup, ProjectGroupShow } from './projects-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}
// jsdom lays nothing out: count every element as on screen for the Tab trap.
Element.prototype.getClientRects = function (): DOMRectList {
  return [{}] as unknown as DOMRectList
}

const group = (id: string, name: string, pinned = false, sortOrder = 0): ProjectGroup => ({
  id,
  name,
  pocWorkspaceId: null,
  pinned,
  color: null,
  sortOrder,
  createdAt: 1,
  updatedAt: 1,
  memberCount: 0,
  memberWorkspaceIds: [],
})
const GROUPS = [group('g1', 'Spring launch', true), group('g2', 'Docs refresh', false, 1), group('g3', 'Research', false, 2)]
const SHOWS: Record<string, ProjectGroupShow> = Object.fromEntries(
  GROUPS.map((g) => [
    g.id,
    {
      ...g,
      members: [],
      dashboards: [
        { id: `${g.id}-d1`, groupId: g.id, name: 'Main', layoutJson: '{"version":1,"panes":[]}', revision: 1, position: 0, createdAt: 1, updatedAt: 1 },
      ],
    },
  ]),
)

const openSettings = vi.fn()
const fetchGroups = vi.fn(async () => {})

beforeEach(() => {
  api.showError = null
  openSettings.mockClear()
  fetchGroups.mockClear()
  vi.mocked(projectsApi.deleteProjectGroup).mockClear()
  usePageViewStore.setState({ page: 'projects' })
  useSettingsStore.setState({ openSettings, settingsOpen: false })
  useToastStore.setState({ toasts: [] })
  useProjectGroupsStore.setState({
    groups: GROUPS,
    groupsError: null,
    selectedGroupId: 'g1',
    settingsSheet: null,
    fetchGroups,
  })
})

afterEach(() => {
  cleanup()
  document.body.innerHTML = ''
})

/** A button on the page that opens the sheet (the opener). */
function renderWithOpener(): HTMLButtonElement {
  render(
    <>
      <button type="button" data-testid="opener">
        Project settings
      </button>
      <ProjectSettingsSheet />
    </>,
  )
  return screen.getByTestId('opener') as HTMLButtonElement
}

async function openSheet(id = 'g1', opener?: HTMLElement): Promise<HTMLElement> {
  act(() => {
    openProjectSettings(id, opener)
  })
  const sheet = await screen.findByTestId('project-settings-sheet')
  await waitFor(() => expect(within(sheet).getByTestId('project-settings-detail')).toBeTruthy())
  return sheet
}

describe('opening', () => {
  it('shows one project’s settings: no project list, no Open project, a close ×', async () => {
    renderWithOpener()
    const sheet = await openSheet('g2')
    expect(sheet.getAttribute('role')).toBe('dialog')
    expect(sheet.getAttribute('aria-modal')).toBe('true')
    const title = within(sheet).getByRole('heading', { name: 'Docs refresh' })
    expect(sheet.getAttribute('aria-labelledby')).toBe(title.id)
    expect(within(sheet).getByText(/^0 agents · No point of contact yet$/)).toBeTruthy()
    expect(within(sheet).getByRole('button', { name: 'Open in Settings' })).toBeTruthy()
    expect(within(sheet).queryByRole('button', { name: 'Open project' })).toBeNull()
    expect(within(sheet).queryByLabelText('Search projects')).toBeNull()
    expect(within(sheet).getByRole('region', { name: 'General' })).toBeTruthy()
    expect(within(sheet).getByRole('region', { name: 'Agents' })).toBeTruthy()
    // The page's selection is not moved by opening another project's settings.
    expect(useProjectGroupsStore.getState().selectedGroupId).toBe('g1')
    expect(openSettings).not.toHaveBeenCalled()
  })

  it('a project that fails to load says so, with Try again and the ×', async () => {
    api.showError = new Error('database is locked')
    renderWithOpener()
    act(() => openProjectSettings('g3'))
    const err = await screen.findByTestId('project-settings-sheet-error')
    expect(err.textContent).toMatch(/Couldn’t load this project’s settings/)
    expect(err.textContent).toMatch(/database is locked/)
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Research' })).toBeTruthy()
    expect(screen.getByTestId('project-settings-sheet-close')).toBeTruthy()
  })

  it('opened at Agents (an empty dashboard’s Add an agent): the add-an-agent search has the cursor', async () => {
    const opener = renderWithOpener()
    act(() => useProjectGroupsStore.getState().openProjectSettingsSheet('g2', opener, 'agents'))
    const sheet = await screen.findByTestId('project-settings-sheet')
    await waitFor(() => {
      const field = sheet.querySelector('input[aria-label="Search agents to add"]')
      expect(field).not.toBeNull()
      expect(document.activeElement).toBe(field)
    })
    expect(useProjectGroupsStore.getState().settingsSheet?.focus).toBe('agents')
  })

  it('off the Projects page the same entry opens Settings → Projects instead', () => {
    usePageViewStore.setState({ page: 'agents' })
    openProjectSettings('g2')
    expect(openSettings).toHaveBeenCalledWith('project-groups', 'g2')
    expect(useProjectGroupsStore.getState().settingsSheet).toBeNull()
  })
})

describe('closing and focus', () => {
  it('the × closes it and focus goes back to what opened it', async () => {
    const opener = renderWithOpener()
    const sheet = await openSheet('g1', opener)
    expect(sheet.contains(document.activeElement)).toBe(true)
    fireEvent.click(within(sheet).getByTestId('project-settings-sheet-close'))
    await waitFor(() => expect(screen.queryByTestId('project-settings-sheet')).toBeNull())
    expect(useProjectGroupsStore.getState().settingsSheet).toBeNull()
    await waitFor(() => expect(document.activeElement).toBe(opener))
  })

  it('Esc closes it; Esc in the name field reverts the name and leaves it open', async () => {
    const opener = renderWithOpener()
    const sheet = await openSheet('g1', opener)
    const name = within(sheet).getByLabelText('Project name') as HTMLInputElement
    name.focus()
    fireEvent.change(name, { target: { value: 'Spring launch v2' } })
    fireEvent.keyDown(name, { key: 'Escape' })
    expect(name.value).toBe('Spring launch')
    expect(screen.getByTestId('project-settings-sheet')).toBeTruthy()
    fireEvent.keyDown(within(sheet).getByTestId('project-settings-sheet-close'), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('project-settings-sheet')).toBeNull())
    await waitFor(() => expect(document.activeElement).toBe(opener))
  })

  it('"Open in Settings" closes the sheet and opens Settings → Projects on this project', async () => {
    renderWithOpener()
    const sheet = await openSheet('g2')
    fireEvent.click(within(sheet).getByRole('button', { name: 'Open in Settings' }))
    expect(openSettings).toHaveBeenCalledWith('project-groups', 'g2')
    await waitFor(() => expect(screen.queryByTestId('project-settings-sheet')).toBeNull())
  })

  it('a press on the scrim closes it', async () => {
    renderWithOpener()
    await openSheet('g1')
    fireEvent.pointerDown(screen.getByTestId('project-settings-sheet-scrim'), { button: 0 })
    await waitFor(() => expect(screen.queryByTestId('project-settings-sheet')).toBeNull())
  })

  it('Tab from the last control wraps to the first, Shift+Tab from the first to the last', async () => {
    renderWithOpener()
    const sheet = await openSheet('g1')
    const close = within(sheet).getByTestId('project-settings-sheet-close')
    const all = Array.from(
      sheet.querySelectorAll<HTMLElement>(
        'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((el) => !el.closest('[aria-hidden="true"], [inert]'))
    // The title row comes first: Open in Settings, then the ×.
    expect(all[0]).toBe(within(sheet).getByRole('button', { name: 'Open in Settings' }))
    expect(all[1]).toBe(close)
    const first = all[0]
    const last = all[all.length - 1]
    first.focus()
    fireEvent.keyDown(first, { key: 'Tab', shiftKey: true })
    expect(document.activeElement).toBe(last)
    fireEvent.keyDown(last, { key: 'Tab' })
    expect(document.activeElement).toBe(first)
  })

  it('focus that lands behind the sheet comes back into it', async () => {
    const opener = renderWithOpener()
    const sheet = await openSheet('g1', opener)
    opener.focus()
    expect(sheet.contains(document.activeElement)).toBe(true)
  })

  it('leaving the Projects page closes it', async () => {
    renderWithOpener()
    await openSheet('g1')
    act(() => usePageViewStore.setState({ page: 'agents' }))
    await waitFor(() => expect(useProjectGroupsStore.getState().settingsSheet).toBeNull())
  })
})

describe('deleting the project from the sheet', () => {
  it('asks first; then closes the sheet, moves the page to the next project and says what happened', async () => {
    renderWithOpener()
    const sheet = await openSheet('g1')
    fireEvent.click(within(sheet).getByRole('button', { name: 'Delete project' }))
    const confirm = await screen.findByTestId('delete-project-confirm')
    // Esc here cancels the confirm only; the sheet stays.
    fireEvent.keyDown(within(confirm).getByRole('button', { name: 'Cancel' }), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('delete-project-confirm')).toBeNull())
    expect(screen.getByTestId('project-settings-sheet')).toBeTruthy()

    fireEvent.click(within(screen.getByTestId('project-settings-sheet')).getByRole('button', { name: 'Delete project' }))
    const again = await screen.findByTestId('delete-project-confirm')
    fireEvent.click(within(again).getByRole('button', { name: 'Delete project' }))
    await waitFor(() => expect(screen.queryByTestId('project-settings-sheet')).toBeNull())
    expect(projectsApi.deleteProjectGroup).toHaveBeenCalledWith(expect.anything(), 'g1')
    const st = useProjectGroupsStore.getState()
    expect(st.settingsSheet).toBeNull()
    expect(st.groups?.map((g) => g.id)).toEqual(['g2', 'g3'])
    expect(st.selectedGroupId).toBe('g2')
    expect(fetchGroups).toHaveBeenCalled()
    const toasts = useToastStore.getState().toasts
    expect(toasts.some((t) => t.type === 'success' && /Deleted “Spring launch”/.test(t.message))).toBe(true)
  })

  it('deleting another project than the page’s keeps the page where it is', async () => {
    renderWithOpener()
    const sheet = await openSheet('g3')
    fireEvent.click(within(sheet).getByRole('button', { name: 'Delete project' }))
    const confirm = await screen.findByTestId('delete-project-confirm')
    fireEvent.click(within(confirm).getByRole('button', { name: 'Delete project' }))
    await waitFor(() => expect(screen.queryByTestId('project-settings-sheet')).toBeNull())
    expect(useProjectGroupsStore.getState().selectedGroupId).toBe('g1')
  })

  it('picks the next project in the nav’s order, else the one before, else none', () => {
    expect(projectAfterDelete(GROUPS, 'g1')?.id).toBe('g2')
    expect(projectAfterDelete(GROUPS, 'g2')?.id).toBe('g3')
    expect(projectAfterDelete(GROUPS, 'g3')?.id).toBe('g2')
    expect(projectAfterDelete([GROUPS[0]], 'g1')).toBeNull()
  })
})

