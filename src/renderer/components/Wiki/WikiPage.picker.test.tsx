// @vitest-environment jsdom
//
// 0.46.0 (Rosson 2026-10-16): the Wiki header's workspace picker and the
// close path. The picker lists this window's server's workspaces grouped by
// focus group, "All workspaces" (the server-wide `k2` map) first; picking
// one switches the wiki shown without changing the room. Close (Esc or the
// X) returns to the page the wiki was opened from.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  indexCalls: [] as { project: string; scope: string | undefined }[],
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))
vi.mock('./wiki-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./wiki-api')>()
  return {
    ...real,
    fetchWikiIndex: vi.fn(async (project: string, opts?: { scope?: 'workspace' | 'k2' }) => {
      h.indexCalls.push({ project, scope: opts?.scope })
      return {
        workspacePath: project,
        generatedAt: '2026-10-16T00:00:00Z',
        nodes: [],
        links: [],
        noteCount: 0,
        scope: opts?.scope === 'k2' ? 'k2' : 'workspace',
      }
    }),
    fetchWikiServeStatus: vi.fn(async () => ({ enabled: false })),
    fetchWikiNote: vi.fn(async () => {
      throw new Error('no note expected')
    }),
  }
})
vi.mock('./WikiGraph', () => ({ default: () => <div data-stub="graph" /> }))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({ default: () => <span data-stub="server" /> }))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => <span data-stub="tabs" /> }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <div data-stub="chrome-right">{children}</div>,
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <div data-stub="utilities">{children}</div>,
}))
vi.mock('@/components/Sidebar/ProjectAvatar', () => ({
  default: ({ projectName }: { projectName: string }) => <span data-avatar={projectName} />,
}))

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import WikiPage from './WikiPage'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'

const project = (id: string, focusGroupId: string | null, extra: Record<string, unknown> = {}) => ({
  id,
  name: id,
  path: `/w/${id}`,
  color: '#888888',
  iconUrl: null,
  focusGroupId,
  pinned: 0,
  workspaces: [],
  ...extra,
})

beforeEach(() => {
  h.indexCalls = []
  useProjectsStore.setState({
    projects: [
      project('alpha', 'g1'),
      project('bravo', 'g2'),
      project('cortana', null, { workspaces: [{ worktreePath: '/w/cortana/.wt/fix' }] }),
    ] as never,
    activeProjectId: 'alpha',
  })
  useFocusGroupsStore.setState({
    focusGroupsEnabled: true,
    focusGroups: [
      { id: 'g1', name: 'Work', color: '#ff0000', tabOrder: 0, createdAt: 0 },
      { id: 'g2', name: 'Play', color: '#00ff00', tabOrder: 1, createdAt: 0 },
    ],
  })
  usePageViewStore.getState().setPage('home')
})

afterEach(() => {
  cleanup()
  usePageViewStore.getState().setPage('agents')
})

function trigger(): HTMLElement {
  const el = document.querySelector('[data-wiki-picker-trigger]')
  if (!(el instanceof HTMLElement)) throw new Error('no wiki picker trigger')
  return el
}

function openMenu(): HTMLElement {
  fireEvent.click(trigger())
  const menu = document.querySelector('[data-wiki-picker-menu]')
  if (!(menu instanceof HTMLElement)) throw new Error('wiki picker menu did not open')
  return menu
}

function rowValues(menu: HTMLElement): string[] {
  return Array.from(menu.querySelectorAll('[data-ws-filter-value]')).map((r) => r.getAttribute('data-ws-filter-value') ?? '')
}

describe('Wiki header workspace picker', () => {
  it('shows the open wiki\'s workspace; lists All workspaces then this server\'s workspaces by focus group', async () => {
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    await waitFor(() => expect(h.indexCalls).toContainEqual({ project: '/w/alpha', scope: undefined }))
    expect(trigger().getAttribute('aria-label')).toBe('Wiki workspace: alpha')
    expect(trigger().querySelector('[data-avatar="alpha"]')).not.toBeNull()

    const menu = openMenu()
    expect(rowValues(menu)).toEqual(['all', '/w/alpha', '/w/bravo', '/w/cortana'])
    expect(within(menu).getAllByRole('option')[0].textContent).toBe('All workspaces')
    const groups = Array.from(menu.querySelectorAll('[data-agent-list-section]')).map((g) => g.getAttribute('aria-label'))
    expect(groups).toEqual([null, 'Work', 'Play', 'Ungrouped'])
    expect(menu.querySelector('[data-ws-filter-value="/w/alpha"]')?.getAttribute('aria-selected')).toBe('true')
    // Search filters the rows (All hides while searching).
    fireEvent.change(within(menu).getByRole('combobox'), { target: { value: 'bra' } })
    expect(rowValues(menu)).toEqual(['/w/bravo'])
  })

  it('picking a workspace switches the wiki shown, not the room or the return page', async () => {
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    const menu = openMenu()
    fireEvent.click(menu.querySelector('[data-ws-filter-value="/w/bravo"]') as HTMLElement)
    await waitFor(() => expect(h.indexCalls).toContainEqual({ project: '/w/bravo', scope: undefined }))
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage, s.lastWikiPath]).toEqual(['wiki', '/w/bravo', 'home', '/w/bravo'])
    expect(useProjectsStore.getState().activeProjectId).toBe('alpha')
    expect(trigger().getAttribute('aria-label')).toBe('Wiki workspace: bravo')
    expect(document.querySelector('[data-wiki-picker-menu]')).toBeNull()
  })

  it('All workspaces is the server-wide map; Global/Local hide, the K2 lens tabs show; a workspace brings Global back', async () => {
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    expect(screen.getByRole('button', { name: 'Global' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'K2' })).toBeNull()
    fireEvent.click(openMenu().querySelector('[data-ws-filter-value="all"]') as HTMLElement)
    await waitFor(() => expect(h.indexCalls).toContainEqual({ project: '/w/alpha', scope: 'k2' }))
    expect(trigger().getAttribute('aria-label')).toBe('Wiki workspace: All workspaces')
    expect(screen.queryByRole('button', { name: 'Global' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Local' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Projects' })).toBeTruthy()
    // Workspace-only actions are off on the server-wide map.
    expect((screen.getByRole('button', { name: 'Serve: OFF' }) as HTMLButtonElement).disabled).toBe(true)
    await waitFor(() => expect(screen.getByText('No workspace wikis on this server yet.')).toBeTruthy())
    expect(screen.queryByText('No knowledge base yet')).toBeNull()
    expect(usePageViewStore.getState().page).toBe('wiki')

    h.indexCalls = []
    fireEvent.click(openMenu().querySelector('[data-ws-filter-value="/w/cortana"]') as HTMLElement)
    await waitFor(() => expect(h.indexCalls).toContainEqual({ project: '/w/cortana', scope: undefined }))
    expect(screen.getByRole('button', { name: 'Global' }).className).toContain('bg-[var(--color-accent)]/15')
    expect((screen.getByRole('button', { name: 'Serve: OFF' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('a worktree wiki (nav View Wiki) shows its workspace in the picker', () => {
    act(() => usePageViewStore.getState().openWiki('/w/cortana/.wt/fix'))
    render(<WikiPage />)
    expect(trigger().getAttribute('aria-label')).toBe('Wiki workspace: cortana')
  })

  it('Esc with the picker open closes only the picker', () => {
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    openMenu()
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(document.querySelector('[data-wiki-picker-menu]')).toBeNull()
    expect(usePageViewStore.getState().page).toBe('wiki')
  })
})

describe('Wiki close returns to where you came from', () => {
  it('Esc returns to Home', () => {
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    act(() => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('the close X returns to Tickets', () => {
    act(() => usePageViewStore.getState().setPage('feedback'))
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    fireEvent.click(screen.getByTitle('Close (Esc)'))
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.lastWikiPath]).toEqual(['feedback', null, '/w/alpha'])
  })
})
