// @vitest-environment jsdom
//
// Reordering the agents kept in the Active drawer. Their order is the
// shortcut order (⌘1, ⌘2…), so moving one moves its shortcut. The real
// ActiveBar is rendered on the canonical-Active path (the daemon's set);
// its row leaves (avatar, status) are stubbed.
//
// Asserted:
//   - A drag writes the canonical workspace order (`reorderProjects`, the
//     daemon's `projects/reorder`) with the full order; the rows, their
//     shortcut numbers and the shortcut targets follow at once.
//   - While dragging no row moves: the dragged row is the 40% ghost and an
//     accent line floats in the kept agents' box where it will land.
//   - Only kept agents move, and only among themselves; other workspaces
//     keep their slots.
//   - Option+↑/↓ on a focused kept row moves it one place and says where.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

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
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
  settingsReset: vi.fn(async () => undefined),
}))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => null) }))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: (f: string) => f === 'canonical-active',
  useServerSupports: (f: string) => f === 'canonical-active',
}))
vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  desktopOsFromNavigator: () => 'mac',
}))
vi.mock('@/stores/activity', () => ({
  activityStore: () => ({ getState: () => ({}) }),
  isLiveDisplay: () => false,
  workspaceDisplay: () => 'idle',
}))
vi.mock('@/components/AgentStatus/useAgentStatus', () => ({
  useAgentStatus: () => ({ heartbeat: false, live: null, working: false, status: 'idle', title: '', workingTitle: '' }),
}))
vi.mock('./ProjectAvatar', () => ({ default: () => <span /> }))

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
    manuallyActive: number
    heartbeatEnabled: number
    focusGroupId: string | null
    iconUrl: null
    workspaces: { id: string }[]
  }
  const useProjectsStore = create<{
    projects: Proj[]
    activeProjectId: string | null
    reorderProjects: (ids: string[]) => Promise<void>
    setActiveWorkspace: (projectId: string, workspaceId: string) => void
    setManuallyActive: () => Promise<void>
    fetchProjects: () => Promise<void>
  }>((set, get) => ({
    projects: [],
    activeProjectId: null,
    // The real action paints first and POSTs after (stores/projects.test.ts
    // covers that); here the POST never settles, so a repaint proves the
    // drawer does not wait on it.
    reorderProjects: (ids) => {
      h.reorder.calls.push(ids)
      const byId = new Map(get().projects.map((p) => [p.id, p]))
      set({ projects: ids.map((id) => byId.get(id)!).filter(Boolean) })
      return new Promise<void>(() => undefined)
    },
    setActiveWorkspace: (projectId) => { h.picks.push(projectId) },
    setManuallyActive: async () => undefined,
    fetchProjects: async () => { throw new Error('fetchProjects must not run on an Active reorder') },
  }))
  return { useProjectsStore }
})

import ActiveBar, { getActiveBarItems } from './ActiveBar'
import { useProjectsStore } from '@/stores/projects'
import { useActiveStore } from '@/stores/active'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'

const ROW_H = 26

type StoreProject = ReturnType<typeof useProjectsStore.getState>['projects'][number]

function proj(id: string, kept = false): StoreProject {
  return {
    id,
    name: `Agent ${id}`,
    path: `/w/${id}`,
    color: '#61afef',
    manuallyActive: kept ? 1 : 0,
    heartbeatEnabled: 0,
    focusGroupId: null,
    iconUrl: null,
    workspaces: [{ id: `ws-${id}` }],
  } as unknown as StoreProject
}

/** Each kept row is ROW_H tall, stacked in the kept box from y=0; the box
 *  is as tall as its rows; the drawer a drag must stay in is 240 × 1000. */
function stubRowRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const id = this.getAttribute('data-active-pin')
    const siblings = id && this.parentElement
      ? Array.from(this.parentElement.querySelectorAll(':scope > [data-active-pin]'))
      : []
    const i = siblings.indexOf(this)
    const top = i < 0 ? 0 : i * ROW_H
    const height = this.hasAttribute('data-active-pins')
      ? this.querySelectorAll(':scope > [data-active-pin]').length * ROW_H
      : this.hasAttribute('data-active-bar') ? 1000 : ROW_H
    return { top, height, bottom: top + height, left: 0, right: 240, width: 240, x: 0, y: top, toJSON: () => ({}) } as DOMRect
  })
}

function setUp(projects: StoreProject[], active: string[]): void {
  useProjectsStore.setState({ projects, activeProjectId: null })
  useActiveStore.setState({ activeProjectIds: new Set(active) })
}

/** The drawer's rows top to bottom, with each row's shortcut number. */
function rows(container: HTMLElement): Array<[string, string | null]> {
  return Array.from(container.querySelectorAll<HTMLElement>('[data-active-row]')).map((el) => [
    el.getAttribute('data-active-row')!,
    el.querySelector('[data-slot="number"]')?.textContent ?? null,
  ])
}

function keptBox(container: HTMLElement): HTMLElement {
  return container.querySelector('[data-active-pins]') as HTMLElement
}

function line(container: HTMLElement): HTMLElement | null {
  return keptBox(container).querySelector<HTMLElement>(':scope > [data-row-drop-line]')
}

/** Press on the middle of `id`'s kept row, move to `toY`, release. */
function dragPin(id: string, toY: number): void {
  const wrap = document.querySelector(`[data-active-pin="${id}"]`) as HTMLElement
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
  stubRowRects()
  useTerminalSettingsStore.setState({ shortcutLayout: 'cmd-active-cmdshift-pinned' })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('Active drawer: reorder the kept agents', () => {
  it('a drag reorders the kept agents; the shortcut numbers and targets follow at once', () => {
    // x is not Active; u1 is Active but not kept.
    setUp([proj('k1', true), proj('x'), proj('k2', true), proj('u1'), proj('k3', true)], ['k1', 'k2', 'u1', 'k3'])
    const { container } = render(<ActiveBar />)
    expect(rows(container)).toEqual([['k1', '1'], ['k2', '2'], ['k3', '3'], ['u1', '4']])

    dragPin('k3', 5) // the top of the kept box → gap 0

    // The full canonical order: the kept agents swap slots among
    // themselves; x and u1 keep theirs.
    expect(h.reorder.calls).toEqual([['k3', 'x', 'k1', 'u1', 'k2']])
    expect(rows(container)).toEqual([['k3', '1'], ['k1', '2'], ['k2', '3'], ['u1', '4']])
    // ⌘1 now opens k3: the shortcuts read the same list.
    expect(getActiveBarItems().map((p) => p.id)).toEqual(['k3', 'k1', 'k2', 'u1'])
    // The drag never opened an agent.
    expect(h.picks).toEqual([])
  })

  it('while dragging no row moves: the ghost goes 40% and an accent line floats where it will land', () => {
    setUp([proj('a', true), proj('b', true), proj('c', true), proj('d', true)], ['a', 'b', 'c', 'd'])
    const { container } = render(<ActiveBar />)
    const box = keptBox(container)
    const before = Array.from(box.querySelectorAll('[data-active-pin]'))
    const b = within(before[1] as HTMLElement).getByText('Agent b')
    fireEvent.mouseDown(b, { button: 0, clientX: 20, clientY: ROW_H * 1.5 })
    const topAt = (y: number): string | null => {
      fireEvent.mouseMove(document, { clientX: 20, clientY: y })
      expect(Array.from(box.querySelectorAll('[data-active-pin]'))).toEqual(before)
      for (const r of before) expect((r as HTMLElement).style.transform).toBe('')
      return line(container)?.style.top ?? null
    }
    // Top (gap 0): half the 6px cap inside the box, the 2px line centred there.
    expect(topAt(3)).toBe(`${3 - 1}px`)
    // Mid-list (gap 3): on the seam between c and d.
    expect(topAt(ROW_H * 2.5 + 2)).toBe(`${3 * ROW_H - 1}px`)
    // Bottom (gap 4): half the cap inside the box's end.
    expect(topAt(ROW_H * 3.5 + 2)).toBe(`${4 * ROW_H - 3 - 1}px`)
    // Over its own place: no line.
    expect(topAt(ROW_H * 1.5)).toBeNull()
    // The ghost.
    expect((before[1] as HTMLElement).style.opacity).toBe('0.4')
    expect((before[0] as HTMLElement).style.opacity).toBe('1')
    // The line: the accent, spanning the rows' content, one per drawer.
    topAt(ROW_H * 2.5 + 2)
    expect(line(container)!.className).toContain('bg-[var(--color-accent)]')
    expect(line(container)!.className).toContain('rounded-[var(--radius-pill)]')
    expect(line(container)!.style.left).toBe('8px')
    expect(line(container)!.style.right).toBe('8px')
    expect(container.querySelectorAll('[data-row-drop-line]')).toHaveLength(1)
    // Off the drawer: no line, and a release there does nothing.
    fireEvent.mouseMove(document, { clientX: 400, clientY: 20 })
    expect(line(container)).toBeNull()
    fireEvent.mouseUp(document)
    expect(h.reorder.calls).toEqual([])
    expect((before[1] as HTMLElement).style.opacity).toBe('1')
  })

  it('the new order is the canonical order, so it holds after the drawer remounts', () => {
    setUp([proj('a', true), proj('b', true), proj('c', true)], ['a', 'b', 'c'])
    const first = render(<ActiveBar />)
    dragPin('a', ROW_H * 2.5 + 2) // below c's middle → gap 3, the end
    expect(h.reorder.calls).toEqual([['b', 'c', 'a']])
    first.unmount()
    const { container } = render(<ActiveBar />)
    expect(rows(container)).toEqual([['b', '1'], ['c', '2'], ['a', '3']])
  })

  it('only kept agents drag; an unkept Active row stays put', () => {
    setUp([proj('k1', true), proj('k2', true), proj('u1'), proj('u2')], ['k1', 'k2', 'u1', 'u2'])
    const { container } = render(<ActiveBar />)
    const u2 = screen.getByText('Agent u2')
    fireEvent.mouseDown(u2, { button: 0, clientX: 20, clientY: 90 })
    fireEvent.mouseMove(document, { clientX: 20, clientY: 2 })
    expect(container.querySelector('[data-row-drop-line]')).toBeNull()
    fireEvent.mouseUp(document)
    expect(h.reorder.calls).toEqual([])
  })

  it('a press under the threshold opens the agent and never reorders', () => {
    setUp([proj('a', true), proj('b', true)], ['a', 'b'])
    render(<ActiveBar />)
    const b = screen.getByText('Agent b')
    fireEvent.mouseDown(b, { button: 0, clientX: 20, clientY: ROW_H * 1.5 })
    fireEvent.mouseMove(document, { clientX: 22, clientY: ROW_H * 1.5 + 3 })
    fireEvent.mouseUp(document)
    fireEvent.click(b)
    expect(h.reorder.calls).toEqual([])
    expect(h.picks).toEqual(['b'])
  })

  it('Option+↓ and Option+↑ move a focused kept row one place and say where it went', () => {
    setUp([proj('a', true), proj('b', true), proj('c', true), proj('u1')], ['a', 'b', 'c', 'u1'])
    const { container } = render(<ActiveBar />)
    const status = container.querySelector('[data-active-move-status]') as HTMLElement
    expect(status.getAttribute('aria-live')).toBe('polite')
    const rowButton = (id: string): HTMLElement =>
      container.querySelector(`[data-active-row="${id}"]`) as HTMLElement

    act(() => rowButton('a').focus())
    fireEvent.keyDown(rowButton('a'), { key: 'ArrowDown', altKey: true })
    expect(h.reorder.calls).toEqual([['b', 'a', 'c', 'u1']])
    expect(rows(container)).toEqual([['b', '1'], ['a', '2'], ['c', '3'], ['u1', '4']])
    expect(status.textContent).toBe('Moved Agent a to ⌘2')
    // The moved row keeps focus, so the next press moves it again.
    expect(document.activeElement).toBe(rowButton('a'))

    fireEvent.keyDown(rowButton('a'), { key: 'ArrowDown', altKey: true })
    expect(rows(container).map(([id]) => id)).toEqual(['b', 'c', 'a', 'u1'])
    expect(status.textContent).toBe('Moved Agent a to ⌘3')

    // The last kept row can't pass the kept group's end.
    fireEvent.keyDown(rowButton('a'), { key: 'ArrowDown', altKey: true })
    expect(h.reorder.calls).toHaveLength(2)

    fireEvent.keyDown(rowButton('b'), { key: 'ArrowUp', altKey: true }) // already first
    expect(h.reorder.calls).toHaveLength(2)

    // The ⌥⌘ layout names its own chord.
    useTerminalSettingsStore.setState({ shortcutLayout: 'cmd-pinned-cmdshift-active' })
    fireEvent.keyDown(rowButton('a'), { key: 'ArrowUp', altKey: true })
    expect(rows(container).map(([id]) => id)).toEqual(['b', 'a', 'c', 'u1'])
    expect(status.textContent).toBe('Moved Agent a to ⌥⌘2')

    // Plain arrows, other modifiers and unkept rows do nothing.
    fireEvent.keyDown(rowButton('a'), { key: 'ArrowUp' })
    fireEvent.keyDown(rowButton('a'), { key: 'ArrowUp', altKey: true, metaKey: true })
    fireEvent.keyDown(rowButton('u1'), { key: 'ArrowUp', altKey: true })
    expect(h.reorder.calls).toHaveLength(3)
  })
})
