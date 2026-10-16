// @vitest-environment jsdom
//
// prd-wiki-3d-view-v1 §3.1, §3.8, §6.4, §8.1: the 3D view on the Wiki page
// with `./WikiGraph3D` and the WebGL probe mocked — the switch, carry-over,
// every fallback (one toast each, the window's choice back to 2D), the
// reduced-motion flag, and wikilink focus requests.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type G3Props = {
  selectedId: string | null
  search: string
  mode: string
  k2Lens?: string
  reducedMotion: boolean
  focusRequest: { id: string; seq: number } | null
  onFallback: (r: 'slow' | 'crashed') => void
  onSelect: (id: string | null) => void
}

const h = vi.hoisted(() => ({
  nodeCount: 4,
  probe: { blockReason: null as null | 'noWebGL2' | 'software', renderer: null as string | null },
  probeCalls: 0,
  g3: null as null | G3Props,
  g3Throw: false,
  rm: false,
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
    fetchWikiIndex: vi.fn(async (project: string) => {
      const nodes = [
        { id: 'Home.md', title: 'Home', aliases: [], tags: [], path: 'Home.md', exists: true },
        { id: 'A.md', title: 'Alpha', aliases: [], tags: [], path: 'A.md', exists: true },
      ]
      for (let i = nodes.length; i < h.nodeCount; i++) {
        nodes.push({ id: `n${i}.md`, title: `n${i}`, aliases: [], tags: [], path: `n${i}.md`, exists: true })
      }
      return {
        workspacePath: project,
        generatedAt: '2026-10-16T00:00:00Z',
        nodes,
        links: [{ source: 'Home.md', target: 'A.md' }],
        noteCount: nodes.length,
        scope: 'workspace',
      }
    }),
    fetchWikiServeStatus: vi.fn(async () => ({ enabled: false })),
    fetchWikiNote: vi.fn(async (_p: string | null, id: string) => ({
      id,
      title: id,
      aliases: [],
      tags: [],
      body: id === 'Home.md' ? 'Home body, see [[Alpha]].' : 'Alpha body.',
      path: id,
      workspacePath: '/w/alpha',
    })),
  }
})
vi.mock('./wiki-3d-guard', async (importOriginal) => {
  const real = await importOriginal<typeof import('./wiki-3d-guard')>()
  return {
    ...real,
    probeWiki3d: () => {
      h.probeCalls++
      return { ...h.probe }
    },
  }
})
vi.mock('./WikiGraph3D', () => ({
  default: (p: G3Props) => {
    if (h.g3Throw) throw new Error('renderer exploded')
    h.g3 = p
    return <div data-stub="graph3d" data-selected={p.selectedId ?? ''} />
  },
}))
vi.mock('./WikiGraph', () => ({
  default: (p: { selectedId: string | null }) => <div data-stub="graph2d" data-selected={p.selectedId ?? ''} />,
}))
vi.mock('@/hooks/useReducedMotion', () => ({ useReducedMotion: () => h.rm }))
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

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import WikiPage from './WikiPage'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'

beforeEach(() => {
  h.nodeCount = 4
  h.probe = { blockReason: null, renderer: 'Apple M1' }
  h.probeCalls = 0
  h.g3 = null
  h.g3Throw = false
  h.rm = false
  localStorage.clear()
  useToastStore.setState({ toasts: [] })
  useProjectsStore.setState({
    projects: [
      { id: 'alpha', name: 'alpha', path: '/w/alpha', color: '#888', iconUrl: null, focusGroupId: null, pinned: 0, workspaces: [] },
    ] as never,
    activeProjectId: 'alpha',
  })
  usePageViewStore.setState({ wikiView: '2d' })
  usePageViewStore.getState().setPage('home')
})

afterEach(() => {
  cleanup()
  usePageViewStore.getState().setPage('agents')
})

function viewButton(v: 'wiki' | '2d' | '3d'): HTMLButtonElement {
  const el = document.querySelector(`[data-wiki-view="${v}"]`)
  if (!(el instanceof HTMLButtonElement)) throw new Error(`no ${v} view button`)
  return el
}

async function open(): Promise<void> {
  act(() => usePageViewStore.getState().openWiki('/w/alpha'))
  render(<WikiPage />)
  await waitFor(() => expect(screen.getByText(/Home body/)).toBeTruthy())
}

function toasts(): string[] {
  return useToastStore.getState().toasts.map((t) => `${t.type}: ${t.message}`)
}

async function graph3d(): Promise<HTMLElement> {
  return waitFor(() => {
    const el = document.querySelector('[data-stub="graph3d"]')
    if (!(el instanceof HTMLElement)) throw new Error('3D view not shown')
    return el
  })
}

describe('2D | 3D switch', () => {
  it('2D is the default; 3D renders the lazy view with the same selection, search and mode; back keeps them', async () => {
    usePageViewStore.setState({ wikiView: '2d' })
    await open()
    expect(viewButton('2d').getAttribute('aria-pressed')).toBe('true')
    expect(document.querySelector('[data-stub="graph2d"]')?.getAttribute('data-selected')).toBe('Home.md')
    fireEvent.change(screen.getByPlaceholderText('Search notes, tags, aliases…'), { target: { value: 'al' } })
    fireEvent.click(viewButton('3d'))
    expect(usePageViewStore.getState().wikiView).toBe('3d')
    expect(localStorage.getItem('k2.wikiView.main')).toBe('3d')
    const g = await graph3d()
    expect(g.getAttribute('data-selected')).toBe('Home.md')
    expect([h.g3?.search, h.g3?.mode, h.g3?.reducedMotion]).toEqual(['al', 'global', false])
    expect(document.querySelector('[data-stub="graph2d"]')).toBeNull()
    // Pick on the 3D graph, then back to 2D: same note, panel unchanged.
    act(() => h.g3?.onSelect('A.md'))
    await waitFor(() => expect(screen.getByText('Alpha body.')).toBeTruthy())
    fireEvent.click(viewButton('2d'))
    expect(document.querySelector('[data-stub="graph2d"]')?.getAttribute('data-selected')).toBe('A.md')
    expect(screen.getByText('Alpha body.')).toBeTruthy()
    expect(toasts()).toEqual([])
  })

  it('the 3D choice survives close/reopen and a Settings unmount', async () => {
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await graph3d()
    act(() => usePageViewStore.getState().closeWiki())
    cleanup() // Settings opening unmounts WikiPage
    await open()
    await graph3d()
    expect(viewButton('3d').getAttribute('aria-pressed')).toBe('true')
  })

  it('the reduced-motion flag reaches the 3D view', async () => {
    h.rm = true
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await graph3d()
    expect(h.g3?.reducedMotion).toBe(true)
  })

  it('a wikilink click in the note panel bumps the focus request', async () => {
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await graph3d()
    expect(h.g3?.focusRequest).toBeNull()
    fireEvent.click(screen.getByRole('link', { name: 'Alpha' }))
    await waitFor(() => expect(h.g3?.focusRequest).toEqual({ id: 'A.md', seq: 1 }))
    expect(h.g3?.selectedId).toBe('A.md')
  })
})

describe('Fallback to 2D (§3.8)', () => {
  it('F1 no WebGL 2: the 3D button is disabled and says why', async () => {
    h.probe = { blockReason: 'noWebGL2', renderer: null }
    await open()
    const b = viewButton('3d')
    expect(b.disabled).toBe(true)
    expect(b.title).toBe("3D needs WebGL 2, which this window doesn't have.")
  })

  it('F1 with a saved 3D choice: 2D plus one toast', async () => {
    h.probe = { blockReason: 'noWebGL2', renderer: null }
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await waitFor(() => expect(usePageViewStore.getState().wikiView).toBe('2d'))
    expect(toasts()).toEqual(["info: 3D needs WebGL 2, which this window doesn't have. Showing 2D."])
  })

  it('F2 software rendering: 2D plus one toast; the store holds 2d; a later try probes again', async () => {
    h.probe = { blockReason: 'software', renderer: 'llvmpipe' }
    await open()
    const before = h.probeCalls
    fireEvent.click(viewButton('3d'))
    await waitFor(() => expect(usePageViewStore.getState().wikiView).toBe('2d'))
    expect(document.querySelector('[data-stub="graph3d"]')).toBeNull()
    expect(document.querySelector('[data-stub="graph2d"]')).not.toBeNull()
    expect(toasts()).toEqual(["info: 3D needs graphics acceleration, which this machine isn't using. Showing 2D."])
    expect(h.probeCalls).toBeGreaterThan(before)
    // The user can try again (the GPU came back).
    h.probe = { blockReason: null, renderer: 'Apple M1' }
    fireEvent.click(viewButton('3d'))
    await graph3d()
    expect(toasts()).toHaveLength(1)
  })

  it('F3 more than 3000 nodes: 2D with the count in the toast', async () => {
    h.nodeCount = 3001
    await open()
    fireEvent.click(viewButton('3d'))
    await waitFor(() => expect(usePageViewStore.getState().wikiView).toBe('2d'))
    expect(toasts()).toEqual(['info: Too many notes for 3D (3,001). Showing 2D.'])
  })

  it('3000 nodes is fine', async () => {
    h.nodeCount = 3000
    await open()
    fireEvent.click(viewButton('3d'))
    await graph3d()
    expect(toasts()).toEqual([])
  })

  it('F4 slow frames (the view reports slow)', async () => {
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await graph3d()
    act(() => h.g3?.onFallback('slow'))
    await waitFor(() => expect(document.querySelector('[data-stub="graph2d"]')).not.toBeNull())
    expect(usePageViewStore.getState().wikiView).toBe('2d')
    expect(toasts()).toEqual(['info: 3D was running slowly here. Showing 2D.'])
  })

  it('F6 the renderer throws: the error boundary falls back', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    h.g3Throw = true
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await waitFor(() => expect(usePageViewStore.getState().wikiView).toBe('2d'))
    expect(document.querySelector('[data-stub="graph2d"]')).not.toBeNull()
    expect(toasts()).toEqual(['info: 3D view stopped. Showing 2D.'])
    err.mockRestore()
  })

  it('F6 a lost context (the view reports crashed)', async () => {
    usePageViewStore.setState({ wikiView: '3d' })
    await open()
    await graph3d()
    act(() => h.g3?.onFallback('crashed'))
    await waitFor(() => expect(usePageViewStore.getState().wikiView).toBe('2d'))
    expect(toasts()).toEqual(['info: 3D view stopped. Showing 2D.'])
  })
})
