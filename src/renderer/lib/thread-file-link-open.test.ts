// Thread file links: a click opens the file the way the Files drawer does,
// in the Thread's room, and hands the editor / tree a one-shot request.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const activateTab = vi.fn()
vi.mock('@/stores/panels', () => ({ usePanelsStore: { getState: () => ({ activateTab }) } }))

// The window's project list: K2 (`/Users/me/Alakazam Labs/K2`) and Sew; the
// test sets which one is showing. `setActiveProject` holds its
// "workspace is showing" callback until the test runs it.
const projectsState = vi.hoisted(() => ({
  projects: [
    { id: 'p-k2', path: '/Users/me/Alakazam Labs/K2', workspaces: [{ id: 'w-k2' }] },
    { id: 'p-sew', path: '/Users/me/sew', workspaces: [{ id: 'w-sew' }] },
  ],
  activeProjectId: 'p-k2' as string | null,
  setActiveProject: null as unknown as (id: string | null, afterShown?: () => void) => void,
}))
const switched: Array<{ id: string | null; afterShown?: () => void }> = []
projectsState.setActiveProject = (id, afterShown) => {
  switched.push({ id, afterShown })
}

import { openFileInBackground, openThreadFileLink } from './thread-file-link-open'
import { useToastStore } from '@/stores/toast'
import { lineColToPos, onFileLineRequest, onFileTreeReveal, peekFileLine, takeFileLine, takeFileTreeReveal } from './file-open-requests'
import { Text } from '@codemirror/state'
import { registerPrimaryRoomProjects, type PrimaryProjectsStore } from '@/stores/room'

// The window's projects store, as the primary room reads it.
registerPrimaryRoomProjects({
  getState: () => projectsState,
  getInitialState: () => projectsState,
  subscribe: () => () => {},
} as unknown as PrimaryProjectsStore)

function fakeRoom(isPrimary = true) {
  const openFileAsTab = vi.fn()
  const parkFileTab = vi.fn(async () => 'opened' as const)
  return {
    room: { scope: { hostKey: 'local' }, tabs: { getState: () => ({ openFileAsTab }), room: { parkFileTab } }, isPrimary } as never,
    openFileAsTab,
    parkFileTab,
  }
}

beforeEach(() => {
  useToastStore.setState({ toasts: [] })
  activateTab.mockClear()
  switched.length = 0
  projectsState.activeProjectId = 'p-k2'
  vi.useRealTimers()
})
afterEach(() => vi.useRealTimers())

describe('openThreadFileLink', () => {
  it('a file opens as a file tab in the room and asks for its line', () => {
    const { room, openFileAsTab } = fakeRoom()
    const heard = vi.fn()
    const off = onFileLineRequest('local', '/ws/src/a.ts', heard)
    openThreadFileLink(room, { input: 'src/a.ts', linkable: true, kind: 'file', rel: 'src/a.ts', path: '/ws/src/a.ts' }, 12, 3)
    off()
    expect(openFileAsTab).toHaveBeenCalledWith('/ws/src/a.ts')
    expect(heard).toHaveBeenCalledTimes(1)
    expect(takeFileLine('local', '/ws/src/a.ts')).toEqual({ line: 12, col: 3 })
    // One shot.
    expect(takeFileLine('local', '/ws/src/a.ts')).toBeNull()
    expect(activateTab).not.toHaveBeenCalled()
  })

  it('a file without a line leaves no line request', () => {
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, { input: 'b.ts', linkable: true, kind: 'file', rel: 'b.ts', path: '/ws/b.ts' })
    expect(openFileAsTab).toHaveBeenCalledWith('/ws/b.ts')
    expect(takeFileLine('local', '/ws/b.ts')).toBeNull()
  })

  it('a folder opens the Files drawer and reveals it there', () => {
    const { room, openFileAsTab } = fakeRoom()
    const heard = vi.fn()
    const off = onFileTreeReveal('local', heard)
    openThreadFileLink(room, { input: 'src/', linkable: true, kind: 'directory', rel: 'src', path: '/ws/src' })
    off()
    expect(openFileAsTab).not.toHaveBeenCalled()
    expect(activateTab).toHaveBeenCalledWith('files')
    expect(heard).toHaveBeenCalledTimes(1)
    // Another tree's root does not take it; the right one does, once.
    expect(takeFileTreeReveal('local', '/other')).toBeNull()
    expect(takeFileTreeReveal('local', '/ws')).toBe('/ws/src')
    expect(takeFileTreeReveal('local', '/ws')).toBeNull()
  })

  it('a folder in a Home room does not touch the window drawers', () => {
    const { room } = fakeRoom(false)
    openThreadFileLink(room, { input: 'src/', linkable: true, kind: 'directory', rel: 'src', path: '/ws/src' })
    expect(activateTab).not.toHaveBeenCalled()
    expect(takeFileTreeReveal('local', '/ws')).toBe('/ws/src')
  })
})

describe('openThreadFileLink opens in the Thread\'s own workspace', () => {
  const K2 = '/Users/me/Alakazam Labs/K2'
  const link = {
    input: 'crates/k2-daemon/src/push_routes.rs',
    linkable: true,
    kind: 'file',
    rel: 'crates/k2-daemon/src/push_routes.rs',
    path: `${K2}/crates/k2-daemon/src/push_routes.rs`,
    root: K2,
  } as const

  it('the Thread\'s workspace is showing: the tab opens there at once', () => {
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, link, 40)
    expect(switched).toEqual([])
    expect(openFileAsTab).toHaveBeenCalledWith(link.path)
    expect(takeFileLine('local', link.path)).toEqual({ line: 40 })
  })

  it('another workspace is showing: it switches to the Thread\'s, then opens the tab there', () => {
    projectsState.activeProjectId = 'p-sew'
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, { ...link, root: `${K2}/` }, 40)
    expect(switched.map((s) => s.id)).toEqual(['p-k2'])
    // Nothing opens in Sew's tab strip while the switch runs.
    expect(openFileAsTab).not.toHaveBeenCalled()
    switched[0].afterShown?.()
    expect(openFileAsTab).toHaveBeenCalledWith(link.path)
    expect(takeFileLine('local', link.path)).toEqual({ line: 40 })
  })

  it('a folder in another workspace switches first, then reveals it', () => {
    projectsState.activeProjectId = 'p-sew'
    const { room } = fakeRoom()
    openThreadFileLink(room, { input: 'crates/', linkable: true, kind: 'directory', rel: 'crates', path: `${K2}/crates`, root: K2 })
    expect(activateTab).not.toHaveBeenCalled()
    switched[0].afterShown?.()
    expect(activateTab).toHaveBeenCalledWith('files')
    expect(takeFileTreeReveal('local', K2)).toBe(`${K2}/crates`)
  })

  it('a Home room shows one workspace (the Thread\'s): never switches the window', () => {
    projectsState.activeProjectId = 'p-sew'
    const { room, openFileAsTab } = fakeRoom(false)
    openThreadFileLink(room, link)
    expect(switched).toEqual([])
    expect(openFileAsTab).toHaveBeenCalledWith(link.path)
  })

  it('a root the window does not list opens in place', () => {
    projectsState.activeProjectId = 'p-sew'
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, { ...link, root: '/elsewhere' })
    expect(switched).toEqual([])
    expect(openFileAsTab).toHaveBeenCalledWith(link.path)
  })
})

describe('Option/Alt-click opens in the background', () => {
  const K2 = '/Users/me/Alakazam Labs/K2'
  const link = {
    input: 'crates/k2-daemon/src/push_routes.rs',
    linkable: true,
    kind: 'file',
    rel: 'crates/k2-daemon/src/push_routes.rs',
    path: `${K2}/crates/k2-daemon/src/push_routes.rs`,
    root: K2,
  } as const
  const flush = async (): Promise<void> => {
    for (let i = 0; i < 3; i++) await Promise.resolve()
  }
  const toasts = (): string[] => useToastStore.getState().toasts.map((t) => t.message)

  it('the Thread\'s workspace is showing: the tab is added unselected, nothing switches', async () => {
    const { room, openFileAsTab, parkFileTab } = fakeRoom()
    openThreadFileLink(room, link, 40, undefined, { background: true })
    await flush()
    expect(switched).toEqual([])
    expect(openFileAsTab).toHaveBeenCalledWith(link.path, { activate: false })
    expect(parkFileTab).not.toHaveBeenCalled()
    expect(activateTab).not.toHaveBeenCalled()
    expect(toasts()).toEqual(['Opened push_routes.rs in K2'])
  })

  it('another workspace is showing: the tab is parked in the Thread\'s workspace, no switch', async () => {
    projectsState.activeProjectId = 'p-sew'
    const { room, openFileAsTab, parkFileTab } = fakeRoom()
    openThreadFileLink(room, link, 40, undefined, { background: true })
    await flush()
    expect(switched).toEqual([])
    expect(openFileAsTab).not.toHaveBeenCalled()
    expect(parkFileTab).toHaveBeenCalledWith('p-k2', 'w-k2', link.path)
    expect(toasts()).toEqual(['Opened push_routes.rs in K2'])
  })

  it('keeps the line for when the tab is shown, past the plain request\'s life', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-10-18T12:00:00Z'))
    const { room } = fakeRoom()
    openThreadFileLink(room, link, 40, 2, { background: true })
    vi.setSystemTime(new Date('2026-10-18T12:05:00Z'))
    expect(peekFileLine('local', link.path)).toEqual({ line: 40, col: 2, background: true })
    expect(takeFileLine('local', link.path)).toEqual({ line: 40, col: 2, background: true })
  })

  it('a file already open: a small "already open" note and nothing else', async () => {
    const { room, openFileAsTab } = fakeRoom()
    openFileAsTab.mockReturnValue('existing')
    await openFileInBackground(room, link)
    expect(openFileAsTab).toHaveBeenCalledWith(link.path, { activate: false })
    expect(toasts()).toEqual(['push_routes.rs is already open in K2'])
  })

  it('a Home room opens in its own strip, unselected', async () => {
    projectsState.activeProjectId = 'p-sew'
    const { room, openFileAsTab, parkFileTab } = fakeRoom(false)
    await openFileInBackground(room, link)
    expect(parkFileTab).not.toHaveBeenCalled()
    expect(openFileAsTab).toHaveBeenCalledWith(link.path, { activate: false })
    expect(toasts()).toEqual(['Opened push_routes.rs in background'])
  })

  it('a layout save that fails says so', async () => {
    projectsState.activeProjectId = 'p-sew'
    const { room, parkFileTab } = fakeRoom()
    parkFileTab.mockRejectedValueOnce(new Error('boom'))
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(await openFileInBackground(room, link)).toBeNull()
    warn.mockRestore()
    expect(useToastStore.getState().toasts.map((t) => [t.type, t.message])).toEqual([['error', "Couldn't open push_routes.rs in K2"]])
  })

  it('a folder has no tab to open behind the view: it shows in Files as on a plain click', () => {
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, { input: 'src/', linkable: true, kind: 'directory', rel: 'src', path: `${K2}/src`, root: K2 }, undefined, undefined, { background: true })
    expect(openFileAsTab).not.toHaveBeenCalled()
    expect(activateTab).toHaveBeenCalledWith('files')
  })

  it('a plain click still shows the tab (one argument, as before)', () => {
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, link)
    expect(openFileAsTab).toHaveBeenCalledWith(link.path)
    expect(toasts()).toEqual([])
  })
})

describe('file-open-requests', () => {
  it('requests expire, so a jump nobody took never fires later', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-10-18T12:00:00Z'))
    const { room } = fakeRoom()
    openThreadFileLink(room, { input: 'a.md', linkable: true, kind: 'file', rel: 'a.md', path: '/ws/a.md' }, 4)
    vi.setSystemTime(new Date('2026-10-18T12:00:06Z'))
    expect(takeFileLine('local', '/ws/a.md')).toBeNull()
  })

  it('requests are per server', () => {
    const { room } = fakeRoom()
    openThreadFileLink(room, { input: 'a.ts', linkable: true, kind: 'file', rel: 'a.ts', path: '/ws/a.ts' }, 2)
    expect(takeFileLine('host:other', '/ws/a.ts')).toBeNull()
    expect(takeFileLine('local', '/ws/a.ts')).toEqual({ line: 2 })
  })
})

describe('lineColToPos', () => {
  const doc = Text.of(['first', 'second line', 'x'])
  it('maps 1-based line and column to an offset', () => {
    expect(lineColToPos(doc, 1)).toBe(0)
    expect(lineColToPos(doc, 2)).toBe(6)
    expect(lineColToPos(doc, 2, 3)).toBe(8)
  })
  it('clamps past the end of the line and the doc', () => {
    expect(lineColToPos(doc, 2, 99)).toBe(6 + 'second line'.length)
    expect(lineColToPos(doc, 99)).toBe(doc.line(3).from)
    expect(lineColToPos(doc, 0)).toBe(0)
  })
})
