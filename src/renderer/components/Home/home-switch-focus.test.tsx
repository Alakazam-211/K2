// @vitest-environment jsdom
// Home applies Settings → General → Workspaces → "When moving between
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
import { __resetWorkspaceSwitchFocusForTests } from '@/lib/workspace-switch-focus'
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
function mountRoom(opts: { thread?: boolean } = {}): {
  terminal: HTMLElement
  pty: HTMLTextAreaElement
  thread: HTMLTextAreaElement | null
} {
  document.body.innerHTML = ''
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
  document.body.appendChild(room)
  return { terminal, pty, thread }
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
