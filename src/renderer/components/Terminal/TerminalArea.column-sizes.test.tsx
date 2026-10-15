// @vitest-environment jsdom
//
// Workspace columns keep their dragged widths when you leave and come back
// (Rosson 2026-10-14: a 2-column split came back half/half, a 3-column split
// came back in thirds). Leaving a workspace stashes it — `splitCount` drops
// to 1 and `activeWorkspaceKey` to null — and coming back restores the
// split, and the old TerminalArea reset its widths on every `splitCount`
// change. Widths are per-window view state (split-view D2) in
// `k2.windowChrome.columns.<label>`.

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'
import { createStore, type StoreApi } from 'zustand/vanilla'

vi.mock('@/components/TabBar/TabBar', () => ({ TabBar: () => null }))
vi.mock('@/components/PaneLayout/PaneLayout', () => ({ PaneLayout: () => null }))
vi.mock('@/components/PresetsBar/PresetsBar', () => ({ PresetsBar: () => null }))
vi.mock('@/hooks/useTerminalShortcuts', () => ({ useTerminalShortcuts: () => {} }))
vi.mock('@/hooks/useResolvedAgentCommand', () => ({ useRoomResolvedAgentCommand: () => null }))

import { TerminalArea } from './TerminalArea'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { columnSizesKey } from '@/lib/column-sizes'
import type { Room } from '@/stores/room'

interface FakeTabs {
  tabs: never[]
  activeTabId: null
  extraGroups: Array<{ tabs: never[]; activeTabId: null }>
  splitCount: number
  activeGroupIndex: number
  activeWorkspaceKey: string | null
  setActiveGroup: (i: number) => void
}

const ROW_WIDTH = 1000

function tabsStore(splitCount: number, key: string | null): StoreApi<FakeTabs> {
  return createStore<FakeTabs>((set) => ({
    tabs: [],
    activeTabId: null,
    extraGroups: Array.from({ length: splitCount - 1 }, () => ({ tabs: [], activeTabId: null })),
    splitCount,
    activeGroupIndex: 0,
    activeWorkspaceKey: key,
    setActiveGroup: (i) => set({ activeGroupIndex: i }),
  }))
}

function roomFor(store: StoreApi<FakeTabs>, hostKey = 'local'): Room {
  return testRoom({ tabs: store, scope: fakeScope(hostKey, { remote: false }), isPrimary: false, cwd: '/ws' })
}

/** Each column's flex-grow, read off the rendered style. */
function columnFlexes(container: HTMLElement): number[] {
  return Array.from(container.querySelectorAll<HTMLElement>('[data-tab-group-index]')).map((el) =>
    Number(el.style.flexGrow),
  )
}

function drag(container: HTMLElement, handleIndex: number, toClientX: number): void {
  const handle = container.querySelectorAll<HTMLElement>('[data-column-resize-handle]')[handleIndex]
  expect(handle, `resize handle ${handleIndex}`).toBeTruthy()
  fireEvent.pointerDown(handle!, { button: 0, pointerId: 1, clientX: 0 })
  fireEvent.pointerMove(handle!, { pointerId: 1, clientX: toClientX })
  fireEvent.pointerUp(handle!, { pointerId: 1, clientX: toClientX })
}

/** What opening a workspace does to the room's tabs store
 *  (`restoreWorkspace` / `loadLayoutForWorkspace`): the split comes back. */
function open(store: StoreApi<FakeTabs>, splitCount: number, key: string): void {
  act(() => {
    store.setState({
      splitCount,
      extraGroups: Array.from({ length: splitCount - 1 }, () => ({ tabs: [], activeTabId: null })),
      activeWorkspaceKey: key,
    })
  })
}

/** What a workspace switch does: `stashWorkspace` (one column, no
 *  workspace), then the split is restored. */
function leaveAndReturn(store: StoreApi<FakeTabs>, splitCount: number, key: string): void {
  act(() => {
    store.setState({ splitCount: 1, extraGroups: [], activeWorkspaceKey: null })
  })
  open(store, splitCount, key)
}

beforeAll(() => {
  // jsdom has no layout and no pointer capture.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
    () => ({ left: 0, top: 0, right: ROW_WIDTH, bottom: 600, width: ROW_WIDTH, height: 600, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect,
  )
  const proto = HTMLElement.prototype as unknown as Record<string, unknown>
  proto.setPointerCapture = () => {}
  proto.hasPointerCapture = () => false
  proto.releasePointerCapture = () => {}
})

beforeEach(() => {
  localStorage.clear()
})

afterEach(() => {
  cleanup()
})

describe('workspace column widths hold across leave and return', () => {
  it('2 columns: a dragged 70/30 split comes back 70/30, not half/half', () => {
    // The room mounts with nothing open, as the app does, then opens.
    const store = tabsStore(1, null)
    const { container } = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    open(store, 2, 'p1:w1')
    expect(columnFlexes(container)).toEqual([50, 50])

    drag(container, 0, 700)
    expect(columnFlexes(container)).toEqual([70, 30])

    leaveAndReturn(store, 2, 'p1:w1')
    expect(columnFlexes(container)).toEqual([70, 30])
  })

  it('3 columns: dragged widths come back, not thirds', () => {
    const store = tabsStore(1, null)
    const { container } = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    open(store, 3, 'p1:w1')
    expect(columnFlexes(container)).toEqual([34, 33, 33])

    drag(container, 0, 200)
    drag(container, 1, 500)
    expect(columnFlexes(container)).toEqual([20, 30, 50])

    leaveAndReturn(store, 3, 'p1:w1')
    expect(columnFlexes(container)).toEqual([20, 30, 50])
  })

  it('a view that mounts already split starts even, not 2:1', () => {
    const { container } = renderInRoom(roomFor(tabsStore(2, 'p1:w1')), <TerminalArea cwd="/ws" />)
    expect(columnFlexes(container)).toEqual([50, 50])
  })

  it('widths survive the view unmounting (another page, a reopened window)', () => {
    const store = tabsStore(2, 'p1:w1')
    const first = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    drag(first.container, 0, 650)
    first.unmount()

    const second = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    expect(columnFlexes(second.container)).toEqual([65, 35])
  })

  it('each workspace, and the same workspace on another server, keeps its own widths', () => {
    const store = tabsStore(2, 'p1:w1')
    const { container } = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    drag(container, 0, 700)

    leaveAndReturn(store, 2, 'p1:w2')
    expect(columnFlexes(container)).toEqual([50, 50])
    drag(container, 0, 400)

    leaveAndReturn(store, 2, 'p1:w1')
    expect(columnFlexes(container)).toEqual([70, 30])
    leaveAndReturn(store, 2, 'p1:w2')
    expect(columnFlexes(container)).toEqual([40, 60])
    cleanup()

    const other = renderInRoom(roomFor(tabsStore(2, 'p1:w1'), 'b.test'), <TerminalArea cwd="/ws" />)
    expect(columnFlexes(other.container)).toEqual([50, 50])
  })

  it('saves to this window’s chrome key on release, never mid-drag', () => {
    const store = tabsStore(2, 'p1:w1')
    const { container } = renderInRoom(roomFor(store), <TerminalArea cwd="/ws" />)
    const handle = container.querySelector<HTMLElement>('[data-column-resize-handle]')!
    fireEvent.pointerDown(handle, { button: 0, pointerId: 1, clientX: 0 })
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 600 })
    expect(localStorage.getItem(columnSizesKey('main'))).toBeNull()
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 600 })
    expect(JSON.parse(localStorage.getItem(columnSizesKey('main'))!)).toEqual({
      'local|p1:w1': { '2': [60, 40] },
    })
  })
})
