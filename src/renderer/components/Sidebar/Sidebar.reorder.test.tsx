// @vitest-environment jsdom
//
// Drag-to-reorder in the Agents nav (Rosson 2026-10-16: "reorganize the
// agents in the main nav without having to go to the workspace settings
// page"). The real Sidebar is rendered; its row leaves (avatar, presence,
// tags, git, activity) are stubbed.
//
// Asserted:
//   - A drag calls the canonical `reorderProjects` (daemon
//     `projects/reorder`, the order Settings → Workspaces shows) with the
//     FULL order, and the rows repaint in the new order before the POST
//     settles (optimistic).
//   - A press that moves under the threshold is a click: it opens the
//     agent and never reorders. A finished drag never opens one.
//   - A row stays in its list (Pinned vs the list under the picker); with
//     a focus group shown, rows of other groups keep their slots.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => null) }))
vi.mock('@/stores/activity', () => ({
  useActivity: () => 'idle',
  workspaceDisplay: () => 'idle',
  projectHasUnseen: () => false,
}))
vi.mock('@/hooks/useGit', () => ({ useGitInfo: () => ({ data: undefined }) }))
vi.mock('./ProjectAvatar', () => ({ default: () => <span /> }))
vi.mock('@/components/Presence/PresenceWorkspaceAvatars', () => ({ default: () => null }))
vi.mock('./NavProjectTags', () => ({ NavProjectTags: () => null }))
vi.mock('./ActiveBar', () => ({ default: () => null }))
vi.mock('./ResizeHandle', () => ({ default: () => null }))
vi.mock('./WorktreeDialog', () => ({ default: () => null }))
vi.mock('@/components/GrokBots/GrokBotRows', () => ({ GrokBotSidebarSection: () => null }))
vi.mock('@/stores/grokbots', () => ({ closeGrokChat: () => undefined }))

const h = vi.hoisted(() => ({
  reorder: { calls: [] as string[][] },
  picks: [] as string[],
}))

vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  interface Proj {
    id: string
    name: string
    path: string
    color: string
    pinned: number
    focusGroupId: string | null
    iconUrl: null
    handle: string
    workspaces: { id: string; type: string; navVisible: number }[]
  }
  const useProjectsStore = create<{
    projects: Proj[]
    activeProjectId: string | null
    activeWorkspaceId: string | null
    reorderProjects: (ids: string[]) => Promise<void>
    setActiveWorkspace: (projectId: string, workspaceId: string) => void
    removeProject: () => Promise<void>
    addProject: () => Promise<void>
    fetchProjects: () => Promise<void>
  }>((set, get) => ({
    projects: [],
    activeProjectId: null,
    activeWorkspaceId: null,
    // The real action paints first and POSTs after (stores/projects.test.ts
    // covers that); here the POST never settles, so a repaint proves the
    // nav does not wait on it.
    reorderProjects: (ids) => {
      h.reorder.calls.push(ids)
      const byId = new Map(get().projects.map((p) => [p.id, p]))
      set({ projects: ids.map((id) => byId.get(id)!).filter(Boolean) })
      return new Promise<void>(() => undefined)
    },
    setActiveWorkspace: (projectId) => { h.picks.push(projectId) },
    removeProject: async () => undefined,
    addProject: async () => undefined,
    fetchProjects: async () => { throw new Error('fetchProjects must not run on a nav reorder') },
  }))
  return { useProjectsStore }
})

vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  const useFocusGroupsStore = create<{
    focusGroups: { id: string; name: string; color: string }[]
    activeFocusGroupId: string | null
    focusGroupsEnabled: boolean
    setActiveFocusGroup: (id: string | null) => void
  }>((set) => ({
    focusGroups: [],
    activeFocusGroupId: null,
    focusGroupsEnabled: false,
    setActiveFocusGroup: (id) => set({ activeFocusGroupId: id }),
  }))
  return { useFocusGroupsStore }
})

import Sidebar from './Sidebar'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'

const ROW_H = 48

type StoreProject = ReturnType<typeof useProjectsStore.getState>['projects'][number]
type StoreFocusGroup = ReturnType<typeof useFocusGroupsStore.getState>['focusGroups'][number]

/** The fields the nav reads; the store's type is the real one (mocked above). */
function proj(id: string, opts: { pinned?: boolean; group?: string | null } = {}): StoreProject {
  return {
    id,
    name: `Agent ${id}`,
    path: `/w/${id}`,
    color: '#61afef',
    pinned: opts.pinned ? 1 : 0,
    focusGroupId: opts.group ?? null,
    iconUrl: null,
    handle: id,
    workspaces: [{ id: `ws-${id}`, type: 'main', navVisible: 0 }],
  } as unknown as StoreProject
}

function group(id: string, name: string): StoreFocusGroup {
  return { id, name, color: '#888888' } as unknown as StoreFocusGroup
}

/** Each nav row is ROW_H tall, stacked in its own list from y=0. */
function stubRowRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const id = this.getAttribute('data-project-id')
    const siblings = id && this.parentElement
      ? Array.from(this.parentElement.querySelectorAll(':scope > [data-project-id]'))
      : []
    const i = siblings.indexOf(this)
    const top = i < 0 ? 0 : i * ROW_H
    return { top, height: ROW_H, bottom: top + ROW_H, left: 0, right: 240, width: 240, x: 0, y: top, toJSON: () => ({}) } as DOMRect
  })
}

function rowOrder(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('[data-project-id]')).map((el) => el.getAttribute('data-project-id')!)
}

/** Press on the middle of `id`'s row, move to `toY` in its list, release. */
function dragRow(id: string, toY: number): void {
  const wrap = document.querySelector(`[data-project-id="${id}"]`) as HTMLElement
  const row = within(wrap).getByText(`Agent ${id}`)
  const fromY = wrap.getBoundingClientRect().top + ROW_H / 2
  fireEvent.mouseDown(row, { button: 0, clientX: 20, clientY: fromY })
  fireEvent.mouseMove(document, { clientX: 20, clientY: toY })
  fireEvent.mouseUp(document)
  // The release lands on a row and fires its click; the drag must eat it.
  fireEvent.click(row)
}

beforeEach(() => {
  h.reorder.calls = []
  h.picks = []
  localStorage.clear()
  stubRowRects()
  useFocusGroupsStore.setState({ focusGroups: [], activeFocusGroupId: null, focusGroupsEnabled: false })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('Agents nav drag-to-reorder', () => {
  it('drags a row down: canonical reorder with the full order, repainted before the POST settles', () => {
    useProjectsStore.setState({ projects: [proj('a'), proj('b'), proj('c'), proj('d')] })
    const { container } = render(<Sidebar />)
    expect(rowOrder(container)).toEqual(['a', 'b', 'c', 'd'])

    // Below c's middle (2*48+24 = 120), above d's (168) → gap 3.
    dragRow('a', 130)

    expect(h.reorder.calls).toEqual([['b', 'c', 'a', 'd']])
    expect(rowOrder(container)).toEqual(['b', 'c', 'a', 'd'])
    expect(h.picks).toEqual([])
  })

  it('a press under the threshold opens the agent and never reorders', () => {
    useProjectsStore.setState({ projects: [proj('a'), proj('b')] })
    render(<Sidebar />)
    const row = screen.getByText('Agent b')
    fireEvent.mouseDown(row, { button: 0, clientX: 20, clientY: 60 })
    fireEvent.mouseMove(document, { clientX: 23, clientY: 65 })
    fireEvent.mouseUp(document)
    fireEvent.click(row)
    expect(h.reorder.calls).toEqual([])
    expect(h.picks).toEqual(['b'])
  })

  it('a drop in the row\'s own gap writes nothing', () => {
    useProjectsStore.setState({ projects: [proj('a'), proj('b'), proj('c')] })
    render(<Sidebar />)
    dragRow('b', 92) // still over b's own slot (48–96)
    expect(h.reorder.calls).toEqual([])
  })

  it('Pinned reorders within Pinned; unpinned rows keep their slots', () => {
    useProjectsStore.setState({
      projects: [proj('u1'), proj('p1', { pinned: true }), proj('u2'), proj('p2', { pinned: true })],
    })
    const { container } = render(<Sidebar />)
    expect(rowOrder(container)).toEqual(['p1', 'p2', 'u1', 'u2'])

    dragRow('p2', 5) // top of the Pinned list → gap 0

    expect(h.reorder.calls).toEqual([['u1', 'p2', 'u2', 'p1']])
    expect(rowOrder(container)).toEqual(['p2', 'p1', 'u1', 'u2'])
  })

  it('a focus group view reorders what it shows; other groups keep their slots', () => {
    useFocusGroupsStore.setState({
      focusGroups: [group('g1', 'One'), group('g2', 'Two')],
      focusGroupsEnabled: true,
      activeFocusGroupId: 'g1',
    })
    useProjectsStore.setState({
      projects: [
        proj('g1a', { group: 'g1' }),
        proj('g2a', { group: 'g2' }),
        proj('n1'),
        proj('g1b', { group: 'g1' }),
        proj('g2b', { group: 'g2' }),
      ],
    })
    const { container } = render(<Sidebar />)
    // g1 view: its rows plus ungrouped ones.
    expect(rowOrder(container)).toEqual(['g1a', 'n1', 'g1b'])

    dragRow('g1b', 5)

    expect(h.reorder.calls).toEqual([['g1b', 'g2a', 'g1a', 'n1', 'g2b']])
    expect(rowOrder(container)).toEqual(['g1b', 'g1a', 'n1'])
    // Group membership is untouched: the drag reorders only.
    const byId = new Map(useProjectsStore.getState().projects.map((p) => [p.id, p.focusGroupId]))
    expect(byId.get('g1b')).toBe('g1')
    expect(byId.get('g2a')).toBe('g2')
  })
})
