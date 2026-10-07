// @vitest-environment jsdom
//
// Home M3 — rooms and window-level routing (prd-home-multi-server-client
// MS2, MS17, MS18, MS68; MS52 h/q; answers Q4, Q5).
//
//   - A room component with no RoomProvider throws (MS52 q).
//   - Two rooms mounted at once: Cmd+T, Cmd+W, menu:new-tab, Cmd+[ and the
//     assistant's `workspace:*` ops act on the FOCUSED room only — one tab,
//     never two (MS52 h). Pointer-down inside a room focuses it.
//   - The assistant refuses a room on another server (Q4).
//   - On Home, Cmd+1–9 selects Home row N; on Agents it switches workspace
//     as before (Q5).
//   (A pinned room's activity — its server's rows, and a chime that reads
//   its own server's project record, MS68 — is `room-agent-status.test.ts`
//   and `activity.test.ts` since prd-daemon-activity-and-thread-working-v1
//   S5.)

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import type { ReactElement } from 'react'

const opened = vi.hoisted(() => ({ rows: [] as string[] }))
vi.mock('@/lib/home-open', () => ({
  openHomeRow: vi.fn((row: { address: string }) => {
    opened.rows.push(row.address)
    return 'selected'
  }),
}))
vi.mock('@/components/Home/home-room', () => ({
  homeRowOpenableNow: (row: { address: string }) => !row.address.startsWith('noaccess'),
}))

import { MissingRoomError, RoomProvider, useRoom, useRoomTabs } from '@/components/Room/RoomContext'
import { TerminalArea } from '@/components/Terminal/TerminalArea'
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'
import { useWorkspaceIndexShortcuts } from '@/hooks/useWorkspaceIndexShortcuts'
import { menuNewTab } from '@/lib/menu-new-tab'
import { routeWorkspaceOp } from '@/lib/workspace-ops-router'
import type { Room } from '@/stores/room'
import {
  __resetWindowRoomForTests,
  focusedRoom,
  installRoomFocusTracking,
  roomHidden,
  roomShown,
} from '@/stores/window-room'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore } from '@/stores/homes'
import { useProjectsStore } from '@/stores/projects'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { projectsStoreOf, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { primaryScope } from '@/kessel/server-scope'

/** A tabs store double that records every room operation it receives. */
function recordingTabs(name: string, log: string[]) {
  const state = {
    tabs: [{ id: `${name}-t1`, mosaicTree: `${name}-pg`, paneGroups: new Map() }],
    activeTabId: `${name}-t1`,
    addTab: (cwd: string) => {
      log.push(`${name}:addTab:${cwd}`)
      return 'pg'
    },
    removeTab: (id: string) => void log.push(`${name}:removeTab:${id}`),
    goBack: () => void log.push(`${name}:goBack`),
    openUntitledDocument: (cwd: string) => void log.push(`${name}:doc:${cwd}`),
  }
  return {
    getState: () => state,
    room: {
      applyWorkspaceOp: (op: { kind: string }) => void log.push(`${name}:op:${op.kind}`),
    },
  }
}

function Shortcuts({ room }: { room: Room }): null {
  useTerminalShortcuts(room, room.cwd())
  return null
}

function RoomArea({ room }: { room: Room }): ReactElement {
  return (
    <RoomProvider room={room} shown>
      <div data-room-key={room.key} data-testid={`area-${room.key}`}>
        <Shortcuts room={room} />
      </div>
    </RoomProvider>
  )
}

let uninstallFocus: (() => void) | null = null

beforeEach(() => {
  __resetWindowRoomForTests()
  uninstallFocus = installRoomFocusTracking(document)
  opened.rows = []
})

afterEach(() => {
  cleanup()
  uninstallFocus?.()
  uninstallFocus = null
})

describe('a room component with no RoomProvider throws (MS2, MS52 q)', () => {
  function Probe({ hook }: { hook: 'room' | 'tabs' }): null {
    if (hook === 'room') useRoom()
    else useRoomTabs((s) => s.tabs)
    return null
  }

  it('useRoom and useRoomTabs throw MissingRoomError', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      expect(() => render(<Probe hook="room" />)).toThrow(MissingRoomError)
      expect(() => render(<Probe hook="tabs" />)).toThrow(MissingRoomError)
    } finally {
      spy.mockRestore()
    }
  })

  it('a real room component (TerminalArea) throws without its room', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      expect(() => render(<TerminalArea cwd="/ws" />)).toThrow(/no RoomProvider/)
    } finally {
      spy.mockRestore()
    }
  })
})

describe('window-level input acts on the focused room only (MS17, MS18, MS52 h)', () => {
  function twoRooms(log: string[]): { a: Room; b: Room } {
    const a = testRoom({ key: 'local|p1:w1', tabs: recordingTabs('A', log), cwd: '/a' })
    const b = testRoom({ key: 'b.test|p1:w1', tabs: recordingTabs('B', log), cwd: '/b', isPrimary: false })
    return { a, b }
  }

  it('Cmd+T with two rooms mounted opens ONE tab, in the focused room', () => {
    const log: string[] = []
    const { a, b } = twoRooms(log)
    const view = render(
      <>
        <RoomArea room={a} />
        <RoomArea room={b} />
      </>,
    )
    // The last room shown takes focus.
    expect(focusedRoom()).toBe(b)
    fireEvent.keyDown(window, { key: 't', metaKey: true })
    expect(log).toEqual(['B:addTab:/b'])

    // Pointer-down inside room A focuses A; Cmd+T and Cmd+W now act on A.
    fireEvent.pointerDown(view.getByTestId(`area-${a.key}`))
    expect(focusedRoom()).toBe(a)
    fireEvent.keyDown(window, { key: 't', metaKey: true })
    fireEvent.keyDown(window, { key: 'w', metaKey: true })
    expect(log).toEqual(['B:addTab:/b', 'A:addTab:/a', 'A:removeTab:A-t1'])
  })

  it('menu:new-tab, Cmd+[ and the assistant ops go to the focused room only', () => {
    const log: string[] = []
    const { a, b } = twoRooms(log)
    const view = render(
      <>
        <RoomArea room={a} />
        <RoomArea room={b} />
      </>,
    )
    fireEvent.pointerDown(view.getByTestId(`area-${a.key}`))
    menuNewTab()
    // App's Cmd+[ handler is `focusedRoom()?.tabs.getState().goBack()`.
    const target = focusedRoom()
    if (!target) throw new Error('no focused room')
    target.tabs.getState().goBack()
    routeWorkspaceOp({ kind: 'new-tab', payload: { cwd: '/x' } })
    expect(log).toEqual(['A:addTab:/a', 'A:goBack', 'A:op:new-tab'])
  })

  it('with no focused room, window-level input does nothing (MS17)', () => {
    const log: string[] = []
    const { a } = twoRooms(log)
    roomShown(a)
    roomHidden(a)
    expect(focusedRoom()).toBe(null)
    menuNewTab()
    routeWorkspaceOp({ kind: 'close-tab', payload: { tabId: 'x' } })
    expect(log).toEqual([])
  })

  it('unmounting the focused room moves focus to the room still shown', () => {
    const log: string[] = []
    const { a, b } = twoRooms(log)
    const view = render(<RoomArea room={a} />)
    const other = render(<RoomArea room={b} />)
    expect(focusedRoom()).toBe(b)
    other.unmount()
    expect(focusedRoom()).toBe(a)
    view.unmount()
    expect(focusedRoom()).toBe(null)
  })

  it("the assistant refuses a focused room on another server (Q4)", () => {
    const log: string[] = []
    const remote = testRoom({
      key: 'b.test|p1:w1',
      tabs: recordingTabs('B', log),
      scope: fakeScope('b.test'),
      isPrimary: false,
    })
    roomShown(remote)
    expect(remote.scope.hostKey).not.toBe(primaryScope().hostKey)
    routeWorkspaceOp({ kind: 'new-tab', payload: { cwd: '/x' } })
    expect(log).toEqual([])
  })
})

describe('Cmd+1–9 is one window-level handler (MS18, Q5)', () => {
  function Index(): null {
    useWorkspaceIndexShortcuts()
    return null
  }

  it('on Home, Cmd+N selects Home row N; a row that cannot open does nothing', () => {
    const homes = useHomesStore.getState()
    const id = homes.createHome('Test Home')
    if (!id) throw new Error('createHome failed')
    for (const address of ['cortana::local', 'noaccess::b.test', 'anna::dtl.k2.dev']) {
      useHomesStore.getState().addRow(id, { address, workspaceId: null, label: address })
    }
    usePageViewStore.getState().setPage('home')
    render(<Index />)
    act(() => {
      fireEvent.keyDown(window, { key: '3', metaKey: true })
      fireEvent.keyDown(window, { key: '1', metaKey: true })
      fireEvent.keyDown(window, { key: '2', metaKey: true })
      fireEvent.keyDown(window, { key: '9', metaKey: true })
    })
    expect(opened.rows).toEqual(['anna::dtl.k2.dev', 'cortana::local'])
  })

  it('on Agents, Cmd+N switches workspace as before and opens no Home row', () => {
    const setActiveWorkspace = vi.fn()
    useProjectsStore.setState({
      projects: [
        { id: 'p1', path: '/p1', agentMode: 'agent', pinned: 0, worktreeMode: 0, workspaces: [{ id: 'w1', tabOrder: 0 }] },
      ] as never,
      setActiveWorkspace,
    } as never)
    usePageViewStore.getState().setPage('agents')
    // Cmd+N = the Nth pinned/agent workspace in this layout.
    useTerminalSettingsStore.setState({ shortcutLayout: 'cmd-pinned-cmdshift-active' })
    render(<Index />)
    fireEvent.keyDown(window, { key: '1', metaKey: true })
    expect(setActiveWorkspace).toHaveBeenCalledWith('p1', 'w1')
    expect(opened.rows).toEqual([])
  })
})

describe('Cmd+Option+1–9: Home switcher on Home, Agents switch elsewhere (0.43.2)', () => {
  function Index(): null {
    useWorkspaceIndexShortcuts()
    return null
  }

  /** Three named Homes (one row each) and a pinned agent workspace, with
   *  the default layout: Cmd+Option+N = Nth pinned on Agents. */
  function setup(): { ids: string[]; setActiveWorkspace: ReturnType<typeof vi.fn> } {
    const st = useHomesStore.getState()
    for (const h of [...st.homes].slice(1)) useHomesStore.getState().deleteHome(h.id)
    const first = useHomesStore.getState().homes[0]
    for (const r of [...first.rows]) useHomesStore.getState().removeRow(first.id, r.address)
    useHomesStore.getState().addRow(first.id, { address: 'cortana::local', workspaceId: null, label: 'cortana' })
    const second = useHomesStore.getState().createHome('Second')
    const third = useHomesStore.getState().createHome('Third')
    if (!second || !third) throw new Error('createHome failed')
    useHomesStore.getState().addRow(second, { address: 'anna::dtl.k2.dev', workspaceId: null, label: 'anna' })
    useHomesStore.getState().selectHome(first.id)
    const setActiveWorkspace = vi.fn()
    useProjectsStore.setState({
      projects: [
        { id: 'p1', path: '/p1', agentMode: 'agent', pinned: 0, worktreeMode: 0, workspaces: [{ id: 'w1', tabOrder: 0 }] },
        { id: 'p2', path: '/p2', agentMode: 'agent', pinned: 0, worktreeMode: 0, workspaces: [{ id: 'w2', tabOrder: 0 }] },
      ] as never,
      setActiveWorkspace,
    } as never)
    useTerminalSettingsStore.setState({ shortcutLayout: 'cmd-active-cmdshift-pinned' })
    const ids = useHomesStore.getState().homes.map((h) => h.id)
    expect(ids).toEqual([first.id, second, third])
    return { ids, setActiveWorkspace }
  }

  it('on Home, ⌥⌘N selects the Nth Home and never switches an Agents workspace', () => {
    const { ids, setActiveWorkspace } = setup()
    usePageViewStore.getState().setPage('home')
    render(<Index />)
    const ev = new KeyboardEvent('keydown', { code: 'Digit2', key: '™', metaKey: true, altKey: true, cancelable: true })
    act(() => void window.dispatchEvent(ev))
    expect(ev.defaultPrevented).toBe(true)
    expect(useHomesStore.getState().selectedId).toBe(ids[1])
    act(() => void fireEvent.keyDown(window, { code: 'Digit3', key: '£', metaKey: true, altKey: true }))
    expect(useHomesStore.getState().selectedId).toBe(ids[2])
    // No Home 9: nothing changes.
    act(() => void fireEvent.keyDown(window, { code: 'Digit9', key: 'ª', metaKey: true, altKey: true }))
    expect(useHomesStore.getState().selectedId).toBe(ids[2])
    expect(setActiveWorkspace).not.toHaveBeenCalled()
    expect(opened.rows).toEqual([])
  })

  it('on Agents, ⌥⌘N switches the pinned workspace as before and never changes the Home', () => {
    const { ids, setActiveWorkspace } = setup()
    usePageViewStore.getState().setPage('agents')
    render(<Index />)
    fireEvent.keyDown(window, { code: 'Digit2', key: '™', metaKey: true, altKey: true })
    expect(setActiveWorkspace).toHaveBeenCalledTimes(1)
    expect(setActiveWorkspace).toHaveBeenCalledWith('p2', 'w2')
    expect(useHomesStore.getState().selectedId).toBe(ids[0])
  })

  it('on Home, Cmd+N still opens row N and Ctrl+N (presets) never switches a Home', () => {
    const { ids, setActiveWorkspace } = setup()
    usePageViewStore.getState().setPage('home')
    render(<Index />)
    fireEvent.keyDown(window, { code: 'Digit1', key: '1', metaKey: true })
    expect(opened.rows).toEqual(['cortana::local'])
    fireEvent.keyDown(window, { code: 'Digit2', key: '2', ctrlKey: true })
    fireEvent.keyDown(window, { code: 'Digit2', key: '@', metaKey: true, shiftKey: true })
    fireEvent.keyDown(window, { code: 'Digit2', key: '2', metaKey: true, altKey: true, shiftKey: true })
    expect(useHomesStore.getState().selectedId).toBe(ids[0])
    expect(setActiveWorkspace).not.toHaveBeenCalled()
  })
})
