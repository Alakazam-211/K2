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
// The rows' status cluster reads the activity store through this hook.
vi.mock('@/components/AgentStatus/useAgentStatus', () => ({
  useAgentStatus: () => ({ heartbeat: false, live: null, working: false, status: 'idle', title: '', workingTitle: '' }),
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

/** Each nav row is ROW_H tall, stacked in its own list from y=0; a list is
 *  as tall as its rows; the panels a drag must stay in are 240 × 1000. */
function stubRowRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const id = this.getAttribute('data-project-id')
    const siblings = id && this.parentElement
      ? Array.from(this.parentElement.querySelectorAll(':scope > [data-project-id]'))
      : []
    const i = siblings.indexOf(this)
    const top = i < 0 ? 0 : i * ROW_H
    const height = this.hasAttribute('data-nav-rows')
      ? this.querySelectorAll(':scope > [data-project-id]').length * ROW_H
      : this.matches('[data-nav-section], [data-agents-nav-list]') ? 1000 : ROW_H
    return { top, height, bottom: top + height, left: 0, right: 240, width: 240, x: 0, y: top, toJSON: () => ({}) } as DOMRect
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

  it('while dragging no row moves: an accent line floats where it will land, at the top, mid-list and the bottom', () => {
    useProjectsStore.setState({ projects: [proj('a'), proj('b'), proj('c'), proj('d')] })
    const { container } = render(<Sidebar />)
    const list = container.querySelector('[data-agents-nav-list] [data-nav-rows]') as HTMLElement
    const rowsBefore = Array.from(list.querySelectorAll('[data-project-id]'))
    const dividersBefore = list.querySelectorAll('[data-row-divider]').length
    const line = (): HTMLElement | null => list.querySelector<HTMLElement>(':scope > [data-row-drop-line]')
    const b = within(rowsBefore[1] as HTMLElement).getByText('Agent b')
    fireEvent.mouseDown(b, { button: 0, clientX: 20, clientY: 72 })
    const topAt = (y: number): string | null => {
      fireEvent.mouseMove(document, { clientX: 20, clientY: y })
      expect(Array.from(list.querySelectorAll('[data-project-id]'))).toEqual(rowsBefore)
      expect(list.querySelectorAll('[data-row-divider]').length).toBe(dividersBefore)
      for (const r of rowsBefore) expect((r as HTMLElement).style.transform).toBe('')
      return line()?.style.top ?? null
    }
    // Top (gap 0): half the 6px cap inside the list, the 2px centred there.
    expect(topAt(5)).toBe(`${3 - 1}px`)
    // Mid-list (gap 3): the seam between c and d.
    expect(topAt(130)).toBe(`${3 * ROW_H - 1}px`)
    // Bottom (gap 4): half the cap inside the list's end.
    expect(topAt(180)).toBe(`${4 * ROW_H - 3 - 1}px`)
    // Its own place: no line.
    expect(topAt(60)).toBeNull()
    expect(line()).toBeNull()
    // The line spans the rows' text, not the panel's bleed.
    topAt(130)
    expect(line()!.style.left).toBe('14px')
    expect(line()!.style.right).toBe('12px')
    expect(line()!.className).toContain('bg-[var(--color-accent)]')
    fireEvent.mouseUp(document)
    expect(h.reorder.calls).toEqual([['a', 'c', 'b', 'd']])
    expect(line()).toBeNull()
  })

  it('Pinned draws its line in Pinned; off the list\'s panel there is no line and a release does nothing', () => {
    useProjectsStore.setState({ projects: [proj('p1', { pinned: true }), proj('p2', { pinned: true }), proj('u1')] })
    const { container } = render(<Sidebar />)
    const pinned = container.querySelector('[data-nav-section="pinned"] [data-nav-rows]') as HTMLElement
    const p2 = within(container.querySelector('[data-project-id="p2"]') as HTMLElement).getByText('Agent p2')
    fireEvent.mouseDown(p2, { button: 0, clientX: 20, clientY: 72 })
    fireEvent.mouseMove(document, { clientX: 20, clientY: 5 })
    expect(pinned.querySelector(':scope > [data-row-drop-line]')).not.toBeNull()
    expect(container.querySelectorAll('[data-row-drop-line]')).toHaveLength(1)
    // Over the main pane: nothing.
    fireEvent.mouseMove(document, { clientX: 400, clientY: 5 })
    expect(container.querySelector('[data-row-drop-line]')).toBeNull()
    fireEvent.mouseUp(document)
    expect(h.reorder.calls).toEqual([])
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
