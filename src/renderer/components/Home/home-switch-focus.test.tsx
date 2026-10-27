// @vitest-environment jsdom
// Home applies Settings → Workspaces / Agents → Workspace settings → "When moving between
// workspaces, auto-select" (Rosson 2026-10-16): arriving on Home, picking
// a row and Cmd+1–9 there land the caret like a switch on Agents — the
// Thread's composer when the Thread is open, else the agent message box.
// Never over an open dialog; "Nothing" moves nothing.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook } from '@testing-library/react'

const h = vi.hoisted(() => ({
  open: vi.fn(async () => ({})),
  showPrimary: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/stores/home-rooms', async () => {
  const { create } = await import('zustand')
  return {
    homeRooms: { open: h.open, showPrimary: h.showPrimary },
    useHomeRoomsStore: create(() => ({ shown: null })),
  }
})

import { useSelectTopHomeRowOnEnter } from './home-auto-select'
import { useWorkspaceIndexShortcuts } from '@/hooks/useWorkspaceIndexShortcuts'
import { openHomeRow } from '@/lib/home-open'
import {
  COMPOSE_WATCH_MS,
  __resetWorkspaceSwitchFocusForTests,
  installRoomPageSwitchFocus,
} from '@/lib/workspace-switch-focus'
import { useSettingsStore, type WorkspaceSwitchFocus } from '@/stores/settings'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useGrokBotsStore } from '@/stores/grokbots'

const row: HomeRow = { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' }

let setActiveProject: ReturnType<typeof vi.fn>

/** One room as TerminalPane mounts it: the grid, the PTY "Message the
 *  agent" bar, and (Thread open) the Thread column's composer. */
function mountRoom(opts: { thread?: boolean; parent?: HTMLElement; keep?: boolean } = {}): {
  room: HTMLElement
  terminal: HTMLElement
  pty: HTMLTextAreaElement
  thread: HTMLTextAreaElement | null
} {
  if (!opts.keep) document.body.innerHTML = ''
  const room = document.createElement('div')
  room.setAttribute('data-terminal-pane-wrapper', '')
  const terminal = document.createElement('div')
  terminal.setAttribute('data-terminal-container', '')
  terminal.setAttribute('data-terminal-visible', 'true')
  terminal.tabIndex = -1
  room.appendChild(terminal)
  const ptyBar = document.createElement('div')
  ptyBar.setAttribute('data-compose-bar', '')
  const pty = document.createElement('textarea')
  ptyBar.appendChild(pty)
  room.appendChild(ptyBar)
  let thread: HTMLTextAreaElement | null = null
  if (opts.thread) {
    const slot = document.createElement('div')
    slot.setAttribute('data-testid', 'agent-session-thread-compose-slot')
    const bar = document.createElement('div')
    bar.setAttribute('data-compose-bar', '')
    thread = document.createElement('textarea')
    bar.appendChild(thread)
    slot.appendChild(bar)
    room.appendChild(slot)
  }
  ;(opts.parent ?? document.body).appendChild(room)
  return { room, terminal, pty, thread }
}

/** Two frames: the page change's frame, then the apply's first retry. */
async function frames(): Promise<void> {
  for (let i = 0; i < 2; i++) await new Promise<void>((r) => requestAnimationFrame(() => r()))
}

function setPref(pref: WorkspaceSwitchFocus): void {
  useSettingsStore.setState({ workspaceSwitchFocus: pref, settingsOpen: false })
}

/** Navigate to Home and mount its entry effect. */
function arriveOnHome(): void {
  usePageViewStore.getState().setPage('home')
  renderHook(() => useSelectTopHomeRowOnEnter())
}

beforeEach(() => {
  __resetWorkspaceSwitchFocusForTests()
  document.body.innerHTML = ''
  h.open.mockClear()
  h.showPrimary.mockClear()
  setActiveProject = vi.fn()
  useGrokBotsStore.setState({ open: null } as never)
  useConnectHostStore.setState({ hosts: [], activeHost: 'local', connectionStatus: 'connected' } as never)
  useProjectsStore.setState({
    projects: [{ id: 'pl', name: 'cortana', path: '/ws/cortana', handle: 'cortana', workspaces: [] }],
    activeProjectId: 'pl',
    setActiveProject,
  } as never)
  // The row is already the selected one: arriving opens nothing, so only
  // Home's own entry can place the caret (the reported bug).
  useHomesStore.setState({ homes: [{ id: 'h1', name: 'Home', rows: [row] }], selectedId: 'h1' } as never)
  usePageViewStore.getState().setPage('agents')
})

afterEach(() => {
  __resetWorkspaceSwitchFocusForTests()
  document.body.innerHTML = ''
})

describe('Home applies the workspace-switch focus setting', () => {
  it('arriving on Home focuses the agent message box', () => {
    const { pty, terminal } = mountRoom()
    setPref('composer')
    arriveOnHome()
    expect(document.activeElement).toBe(pty)
    expect(document.activeElement).not.toBe(terminal)
  })

  it('arriving on Home with the Thread open focuses the Thread composer', () => {
    const { pty, thread } = mountRoom({ thread: true })
    setPref('composer')
    arriveOnHome()
    expect(document.activeElement).toBe(thread)
    expect(document.activeElement).not.toBe(pty)
  })

  it('arriving on Home with Terminal focuses the terminal', () => {
    const { terminal } = mountRoom({ thread: true })
    setPref('terminal')
    arriveOnHome()
    expect(document.activeElement).toBe(terminal)
  })

  it('arriving on Home with the setting off focuses nothing', () => {
    mountRoom({ thread: true })
    setPref('off')
    arriveOnHome()
    expect(document.activeElement).toBe(document.body)
  })

  it('arriving on Home does not steal focus from an open dialog', () => {
    mountRoom({ thread: true })
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'dialog')
    dialog.setAttribute('aria-modal', 'true')
    const field = document.createElement('input')
    dialog.appendChild(field)
    document.body.appendChild(dialog)
    field.focus()
    setPref('composer')
    arriveOnHome()
    expect(document.activeElement).toBe(field)
  })

  it('arriving on Home does not steal focus from a field the user is typing in', () => {
    mountRoom({ thread: true })
    const search = document.createElement('input')
    document.body.appendChild(search)
    search.focus()
    setPref('composer')
    arriveOnHome()
    expect(document.activeElement).toBe(search)
  })

  it('picking a row on Home focuses the Thread composer', () => {
    const { thread } = mountRoom({ thread: true })
    setPref('composer')
    usePageViewStore.getState().setPage('home')
    expect(openHomeRow(row)).toBe('selected')
    expect(setActiveProject).toHaveBeenCalledWith('pl')
    expect(document.activeElement).toBe(thread)
  })

  it('Cmd+1 on Home focuses the agent message box', () => {
    const { pty } = mountRoom()
    setPref('composer')
    usePageViewStore.getState().setPage('home')
    renderHook(() => useWorkspaceIndexShortcuts())
    window.dispatchEvent(new KeyboardEvent('keydown', { key: '1', code: 'Digit1', metaKey: true }))
    expect(setActiveProject).toHaveBeenCalledWith('pl')
    expect(document.activeElement).toBe(pty)
  })
})

describe('Home → Agents applies the setting in the room Agents shows', () => {
  let uninstall: (() => void) | null = null
  beforeEach(() => {
    uninstall = installRoomPageSwitchFocus()
  })
  afterEach(() => {
    uninstall?.()
    uninstall = null
  })

  it('a local agent: the Thread composer on Agents gets the caret', async () => {
    const local = mountRoom({ thread: true })
    setPref('composer')
    usePageViewStore.getState().setPage('home')
    await frames()
    // The user clicks around on Home (focus leaves the composer).
    local.terminal.focus()
    usePageViewStore.getState().setPage('agents')
    await frames()
    expect(document.activeElement).toBe(local.thread)
  })

  it('a local agent with the Thread closed: the agent message box', async () => {
    const local = mountRoom()
    setPref('composer')
    usePageViewStore.getState().setPage('home')
    await frames()
    usePageViewStore.getState().setPage('agents')
    await frames()
    expect(document.activeElement).toBe(local.pty)
  })

  it('a remote agent on Home: the caret leaves the parked room for the local Thread', async () => {
    document.body.innerHTML = ''
    setPref('composer')
    // Home shows another server's room; the window's own room is hidden.
    const primary = document.createElement('div')
    primary.style.display = 'none'
    document.body.appendChild(primary)
    const local = mountRoom({ thread: true, parent: primary, keep: true })
    const slot = document.createElement('div')
    document.body.appendChild(slot)
    const remote = mountRoom({ thread: true, parent: slot, keep: true })
    usePageViewStore.getState().setPage('home')
    await frames()
    remote.thread!.focus()
    expect(document.activeElement).toBe(remote.thread)

    // Agents: the remote room is parked (visibility:hidden, focus stays
    // there in WebKit) and the window's own room shows.
    usePageViewStore.getState().setPage('agents')
    slot.style.visibility = 'hidden'
    primary.style.display = ''
    await frames()
    expect(document.activeElement).toBe(local.thread)
  })

  it('a room whose Thread composer mounts late still gets it, within the watch window', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    try {
      const local = mountRoom()
      setPref('composer')
      usePageViewStore.getState().setPage('home')
      await frames()
      usePageViewStore.getState().setPage('agents')
      await frames()
      expect(document.activeElement).toBe(local.pty)
      // The Thread column loads a moment later (remote room / async open).
      const slot = document.createElement('div')
      slot.setAttribute('data-testid', 'agent-session-thread-compose-slot')
      const bar = document.createElement('div')
      bar.setAttribute('data-compose-bar', '')
      const late = document.createElement('textarea')
      bar.appendChild(late)
      slot.appendChild(bar)
      local.room.appendChild(slot)
      vi.advanceTimersByTime(450)
      expect(document.activeElement).toBe(late)
      vi.advanceTimersByTime(COMPOSE_WATCH_MS)
    } finally {
      vi.useRealTimers()
    }
  })

  it('off: Home → Agents moves nothing', async () => {
    const local = mountRoom({ thread: true })
    setPref('off')
    usePageViewStore.getState().setPage('home')
    await frames()
    usePageViewStore.getState().setPage('agents')
    await frames()
    expect(document.activeElement).toBe(document.body)
    expect(document.activeElement).not.toBe(local.thread)
  })
})
