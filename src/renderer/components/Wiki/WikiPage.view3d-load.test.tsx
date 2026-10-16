// @vitest-environment jsdom
//
// prd-wiki-3d-view-v1 §3.8 F5: the lazy 3D chunk fails to load → 2D plus
// one toast, and the page stays usable. (Its own file: the module mock
// below always rejects.)

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
vi.mock('./WikiGraph3D', () => {
  // The chunk can't be fetched (a dropped Connect link): the import rejects.
  throw new Error('Failed to fetch dynamically imported module')
})
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

describe('F5 chunk load failure', () => {
  it('falls back to 2D with one toast and keeps working', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    await open()
    fireEvent.click(viewButton('3d'))
    await waitFor(() => expect(usePageViewStore.getState().wikiView).toBe('2d'))
    expect(toasts()).toEqual(["info: Couldn't load the 3D view. Showing 2D."])
    expect(document.querySelector('[data-stub="graph2d"]')?.getAttribute('data-selected')).toBe('Home.md')
    expect(screen.getByText(/Home body/)).toBeTruthy()
    err.mockRestore()
  })
})
