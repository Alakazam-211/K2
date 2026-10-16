// @vitest-environment jsdom
//
// prd-wiki-3d-view-v1 §3.1, §12, W6 (Rosson 2026-10-16): the Wiki page's
// three views (Wiki | 2D | 3D) on a POPULATED wiki — the standard reading
// view (note list, note, wikilink navigation, "Linked from"), selection
// shared across views, the per-window choice, and Esc (Home, then close).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  noteCalls: [] as string[],
  graphProps: null as null | { selectedId: string | null; search: string; mode: string; onSelect: (id: string | null) => void },
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

const BODIES: Record<string, string> = {
  'Home.md': '# Home\n\nStart at [[Feature - Alpha]].',
  'Feature - Alpha.md': 'Alpha links to [[Feature - Beta]].',
  'Feature - Beta.md': 'Beta body.',
  'Notes.md': 'Loose note that links [[Feature - Beta]].',
}

vi.mock('./wiki-api', async (importOriginal) => {
  const real = await importOriginal<typeof import('./wiki-api')>()
  const n = (id: string, title: string) => ({ id, title, aliases: [], tags: [], path: id, exists: true })
  return {
    ...real,
    fetchWikiIndex: vi.fn(async (project: string) => ({
      workspacePath: project,
      generatedAt: '2026-10-16T00:00:00Z',
      nodes: [
        n('Home.md', 'Home'),
        n('Feature - Alpha.md', 'Feature - Alpha'),
        n('Feature - Beta.md', 'Feature - Beta'),
        n('Notes.md', 'Notes'),
      ],
      links: [
        { source: 'Home.md', target: 'Feature - Alpha.md' },
        { source: 'Feature - Alpha.md', target: 'Feature - Beta.md' },
        { source: 'Notes.md', target: 'Feature - Beta.md' },
      ],
      noteCount: 4,
      scope: 'workspace',
    })),
    fetchWikiServeStatus: vi.fn(async () => ({ enabled: false })),
    fetchWikiNote: vi.fn(async (_project: string | null, id: string) => {
      h.noteCalls.push(id)
      const body = BODIES[id]
      if (body === undefined) throw new Error(`no note ${id}`)
      return { id, title: id.replace(/\.md$/, ''), aliases: [], tags: [], body, path: id, workspacePath: '/w/alpha' }
    }),
  }
})
vi.mock('./WikiGraph', () => ({
  default: (p: { selectedId: string | null; search: string; mode: string; onSelect: (id: string | null) => void }) => {
    h.graphProps = p
    return <div data-stub="graph2d" data-selected={p.selectedId ?? ''} />
  },
}))
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

beforeEach(() => {
  h.noteCalls = []
  h.graphProps = null
  localStorage.clear()
  useProjectsStore.setState({
    projects: [
      { id: 'alpha', name: 'alpha', path: '/w/alpha', color: '#888', iconUrl: null, focusGroupId: null, pinned: 0, workspaces: [] },
    ] as never,
    activeProjectId: 'alpha',
  })
  usePageViewStore.setState({ wikiView: 'wiki' })
  usePageViewStore.getState().setPage('home')
})

afterEach(() => {
  cleanup()
  usePageViewStore.getState().setPage('agents')
})

function list(): HTMLElement {
  const el = document.querySelector('[data-wiki-note-list]')
  if (!(el instanceof HTMLElement)) throw new Error('no note list')
  return el
}

function row(id: string): HTMLElement {
  const el = Array.from(list().querySelectorAll<HTMLElement>('[data-wiki-note-id]')).find(
    (r) => r.getAttribute('data-wiki-note-id') === id,
  )
  if (!el) throw new Error(`no list row ${id}`)
  return el
}

function viewButton(v: 'wiki' | '2d' | '3d'): HTMLButtonElement {
  const el = document.querySelector(`[data-wiki-view="${v}"]`)
  if (!(el instanceof HTMLButtonElement)) throw new Error(`no ${v} view button`)
  return el
}

async function openPopulated(): Promise<void> {
  act(() => usePageViewStore.getState().openWiki('/w/alpha'))
  render(<WikiPage />)
  // Home auto-selects and its body renders.
  await waitFor(() => expect(screen.getByText(/Start at/)).toBeTruthy())
}

describe('Wiki header view switch', () => {
  it('sits right after the workspace picker, labelled, Wiki pressed by default', async () => {
    await openPopulated()
    const picker = document.querySelector('[data-wiki-picker-trigger]')
    const group = screen.getByRole('group', { name: 'Wiki view' })
    if (!picker) throw new Error('no picker')
    // The toggle follows the picker's wrapper in the header's left cluster.
    const pickerTop = picker.closest('[data-wiki-picker-trigger]')?.parentElement
    const cluster = group.parentElement
    if (!cluster || !pickerTop) throw new Error('no header cluster')
    const kids = Array.from(cluster.children)
    expect(kids.indexOf(group)).toBe(kids.findIndex((k) => k.contains(picker)) + 1)
    expect(group.className).toContain('no-drag')
    expect(viewButton('wiki').getAttribute('aria-pressed')).toBe('true')
    expect(viewButton('2d').getAttribute('aria-pressed')).toBe('false')
    expect(document.querySelector('[data-wiki-body]')?.getAttribute('data-wiki-body')).toBe('wiki')
  })
})

describe('Standard Wiki view', () => {
  it('lists the notes grouped (Home on top, then categories), Home open with Linked from', async () => {
    await openPopulated()
    const groups = Array.from(list().querySelectorAll('[data-wiki-note-group]')).map((g) => [
      g.getAttribute('data-wiki-note-group'),
      Array.from(g.querySelectorAll('[data-wiki-note-id]')).map((r) => r.getAttribute('data-wiki-note-id')),
    ])
    expect(groups).toEqual([
      ['home', ['Home.md']],
      ['cat:feature', ['Feature - Alpha.md', 'Feature - Beta.md']],
      ['other', ['Notes.md']],
    ])
    expect(row('Home.md').getAttribute('aria-current')).toBe('true')
    // Home has no backlinks here; the graph is not mounted in the Wiki view.
    expect(document.querySelector('[data-wiki-backlinks]')).toBeNull()
    expect(document.querySelector('[data-stub="graph2d"]')).toBeNull()
    // Hide article is a graph-view control.
    expect(screen.queryByRole('button', { name: /Hide article/ })).toBeNull()
  })

  it('a list pick renders the note; wikilinks and Linked from navigate', async () => {
    await openPopulated()
    fireEvent.click(row('Feature - Alpha.md'))
    await waitFor(() => expect(screen.getByText(/Alpha links to/)).toBeTruthy())
    expect(row('Feature - Alpha.md').getAttribute('aria-current')).toBe('true')
    const back = within(screen.getByRole('region', { name: 'Linked from' }))
    expect(back.getAllByRole('button').map((b) => b.textContent)).toEqual(['Home'])

    // [[Feature - Beta]] in the body.
    fireEvent.click(screen.getByRole('link', { name: 'Feature - Beta' }))
    await waitFor(() => expect(screen.getByText('Beta body.')).toBeTruthy())
    expect(row('Feature - Beta.md').getAttribute('aria-current')).toBe('true')
    const back2 = within(screen.getByRole('region', { name: 'Linked from' }))
    expect(back2.getAllByRole('button').map((b) => b.textContent)).toEqual(['Feature - Alpha', 'Notes'])

    fireEvent.click(back2.getByRole('button', { name: 'Notes' }))
    await waitFor(() => expect(screen.getByText(/Loose note that links/)).toBeTruthy())
    expect(row('Notes.md').getAttribute('aria-current')).toBe('true')
  })

  it('the toolbar search filters the list to matches', async () => {
    await openPopulated()
    fireEvent.change(screen.getByPlaceholderText('Search notes, tags, aliases…'), { target: { value: 'beta' } })
    const ids = Array.from(list().querySelectorAll('[data-wiki-note-id]')).map((r) => r.getAttribute('data-wiki-note-id'))
    expect(ids).toEqual(['Feature - Beta.md'])
  })
})

describe('Selection, search and the choice carry across views', () => {
  it('a note picked in Wiki stays open in 2D and back; the search carries too', async () => {
    await openPopulated()
    fireEvent.click(row('Feature - Beta.md'))
    await waitFor(() => expect(screen.getByText('Beta body.')).toBeTruthy())
    fireEvent.change(screen.getByPlaceholderText('Search notes, tags, aliases…'), { target: { value: 'feat' } })

    fireEvent.click(viewButton('2d'))
    expect(usePageViewStore.getState().wikiView).toBe('2d')
    const g = document.querySelector('[data-stub="graph2d"]')
    if (!g) throw new Error('2D graph not shown')
    expect(g.getAttribute('data-selected')).toBe('Feature - Beta.md')
    expect([h.graphProps?.search, h.graphProps?.mode]).toEqual(['feat', 'global'])
    expect(screen.getByText('Beta body.')).toBeTruthy()
    expect(document.querySelector('[data-wiki-note-list]')).toBeNull()

    // A pick on the graph shows in the Wiki list.
    act(() => h.graphProps?.onSelect('Feature - Alpha.md'))
    await waitFor(() => expect(screen.getByText(/Alpha links to/)).toBeTruthy())
    fireEvent.click(viewButton('wiki'))
    expect(row('Feature - Alpha.md').getAttribute('aria-current')).toBe('true')
    expect((screen.getByPlaceholderText('Search notes, tags, aliases…') as HTMLInputElement).value).toBe('feat')
  })

  it('the choice is saved for this window and survives close/reopen and a Settings unmount', async () => {
    await openPopulated()
    fireEvent.click(viewButton('2d'))
    expect(localStorage.getItem('k2.wikiView.main')).toBe('2d')
    act(() => usePageViewStore.getState().closeWiki())
    cleanup()
    act(() => usePageViewStore.getState().openWiki('/w/alpha'))
    render(<WikiPage />)
    await waitFor(() => expect(document.querySelector('[data-stub="graph2d"]')).not.toBeNull())
    expect(viewButton('2d').getAttribute('aria-pressed')).toBe('true')
  })
})

describe('Esc on a populated wiki (W6)', () => {
  it('Esc on a non-Home note goes to Home; Esc on Home closes', async () => {
    await openPopulated()
    fireEvent.click(row('Feature - Alpha.md'))
    await waitFor(() => expect(screen.getByText(/Alpha links to/)).toBeTruthy())
    act(() => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(usePageViewStore.getState().page).toBe('wiki')
    await waitFor(() => expect(row('Home.md').getAttribute('aria-current')).toBe('true'))
    act(() => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('works the same in the 2D view', async () => {
    usePageViewStore.setState({ wikiView: '2d' })
    await openPopulated()
    act(() => h.graphProps?.onSelect('Notes.md'))
    await waitFor(() => expect(document.querySelector('[data-stub="graph2d"]')?.getAttribute('data-selected')).toBe('Notes.md'))
    act(() => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(document.querySelector('[data-stub="graph2d"]')?.getAttribute('data-selected')).toBe('Home.md')
    act(() => {
      fireEvent.keyDown(window, { key: 'Escape' })
    })
    expect(usePageViewStore.getState().page).toBe('home')
  })
})
