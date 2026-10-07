// Home M5 — the pieces of a usable remote room that do not need a second
// daemon: tab dots from the server's own activity, the close prompt from
// the room's activity, OS drops landing on the room under the pointer, file
// clipboard and undo staying on their server, and links clicked in a room
// on another server opening on this computer.

import { describe, it, expect, vi, beforeEach } from 'vitest'

const h = vi.hoisted(() => ({
  activity: new Map<string, Set<(e: unknown) => void>>(),
  opened: [] as string[],
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(async (url: string) => {
    h.opened.push(url)
  }),
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...real,
    onSessionActivityChanged: (scope: { id: string }, fn: (e: unknown) => void) => {
      const set = h.activity.get(scope.id) ?? new Set()
      set.add(fn)
      h.activity.set(scope.id, set)
      return () => set.delete(fn)
    },
  }
})

import { createStore } from 'zustand/vanilla'
import { scopeForHost, primaryScope, __resetServerScopesForTests } from '@/kessel/server-scope'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { createPinnedRoom, paneRoomMode, pathUnderRoot, type PinnedRoom, type RoomProjectsStore } from '@/stores/room'
import { roomShown, __resetWindowRoomForTests } from '@/stores/window-room'
import { dropDestination } from '@/lib/external-drop-router'
import { fileUndoFor, useFileUndoStore } from '@/stores/file-undo'
import { useFileClipboardStore } from '@/stores/file-clipboard'
import { workingAgentsInTab } from '@/lib/room-close-guard'
import { __resetActivityForTests, displayUnderRoot, terminalDisplay } from '@/stores/activity'
import { openTerminalUrl } from '@/lib/terminal-link-open'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { TerminalItemData } from '@/stores/tabs'

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: '127.0.0.1',
  port: 59_997,
  secure: false,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}
const B_KEY = '127.0.0.1:59997'

function pinned(readOnly = false): PinnedRoom {
  const projects = createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects: [] }))
  return createPinnedRoom({
    scope: scopeForHost(B),
    workspace: { projectId: 'p', workspaceId: 'w', path: '/srv/anna' },
    projects: projects as RoomProjectsStore,
    activateProject: () => {},
    readOnly,
  })
}

function emitActivity(e: { workspacePath: string; agentName: string; paneGroupId: string | null; status: string }): void {
  const set = h.activity.get(`host:${B_KEY}`)
  if (!set || set.size === 0) throw new Error('no session_activity_changed handler on B')
  for (const fn of set) fn({ kind: 'session_activity_changed', ...e })
}

beforeEach(() => {
  __resetServerScopesForTests()
  __resetWindowRoomForTests()
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local' } as never)
  useFileUndoStore.getState().clear()
  useFileClipboardStore.getState().clear()
  h.opened = []
  __resetActivityForTests()
})

describe('tab dots from the room’s server (session_activity_changed, an older server: RL13)', () => {
  it('a pinned room shows its server’s stream as-is; its workspace reads only its own rows; dispose stops it', async () => {
    const room = pinned()
    emitActivity({ workspacePath: '/srv/anna', agentName: 'tab-t1', paneGroupId: 't1', status: 'working' })
    emitActivity({ workspacePath: '/srv/anna/.worktrees/x', agentName: 'tab-t2', paneGroupId: 't2', status: 'permission' })
    emitActivity({ workspacePath: '/srv/anna-other', agentName: 'tab-t3', paneGroupId: 't3', status: 'working' })
    const view = room.activityView.getState()
    expect(terminalDisplay(view, { terminalId: 't1' })).toBe('working')
    expect(terminalDisplay(view, { terminalId: 't2' })).toBe('waiting')
    expect(displayUnderRoot(view, '/srv/anna')).toBe('waiting')
    emitActivity({ workspacePath: '/srv/anna', agentName: 'tab-t2', paneGroupId: 't2', status: 'idle' })
    expect(displayUnderRoot(room.activityView.getState(), '/srv/anna')).toBe('working')
    await room.dispose()
    expect(h.activity.get(`host:${B_KEY}`)?.size).toBe(0)
  })

  it('path boundary: /srv/anna holds /srv/anna/x, not /srv/anna-other', () => {
    expect(pathUnderRoot('/srv/anna/x', '/srv/anna/')).toBe(true)
    expect(pathUnderRoot('/srv/anna', '/srv/anna')).toBe(true)
    expect(pathUnderRoot('/srv/anna-other', '/srv/anna')).toBe(false)
  })
})

describe('terminal mode and the close prompt in a room', () => {
  it('a usable room’s panes claim (claimer); a view-only room’s watch (viewer)', async () => {
    const usable = pinned()
    const viewOnly = pinned(true)
    try {
      expect(paneRoomMode(usable)).toBe('claimer')
      expect(paneRoomMode(viewOnly)).toBe('viewer')
      expect(paneRoomMode({ isPrimary: true, readOnly: false })).toBe(null)
    } finally {
      await usable.dispose()
      await viewOnly.dispose()
    }
  })

  it('closing a tab whose agent is working on B asks first, from the room’s own activity', async () => {
    const room = pinned()
    try {
      room.tabs.getState().addTab('/srv/anna')
      const tab = room.tabs.getState().tabs[room.tabs.getState().tabs.length - 1]
      const data = [...tab.paneGroups.values()][0].items[0].data as TerminalItemData
      expect(workingAgentsInTab(room, tab.id, 0)).toEqual([])
      emitActivity({ workspacePath: '/srv/anna', agentName: `tab-${data.terminalId}`, paneGroupId: data.terminalId, status: 'working' })
      const agents = workingAgentsInTab(room, tab.id, 0)
      expect(agents.map((a) => [a.terminalId, a.tabId, a.display])).toEqual([[data.terminalId, tab.id, 'working']])
    } finally {
      await room.dispose()
    }
  })
})

describe('OS file drops land on the room under the pointer (MS19)', () => {
  /** An element under the pointer, inside the room `roomKey` (or none). */
  function docWith(roomKey: string | null): Document {
    const el = { closest: () => (roomKey === null ? null : { getAttribute: () => roomKey }) }
    return { elementFromPoint: () => el } as unknown as Document
  }

  it('a usable room on B: its scope; outside any room: the window’s server', async () => {
    const room = pinned()
    try {
      roomShown(room)
      const dest = dropDestination(primaryScope(), { x: 1, y: 1 }, docWith(room.key))
      if ('refused' in dest) throw new Error(`refused: ${dest.refused}`)
      expect(dest.scope).toBe(room.scope)
      expect(dest.scope.hostKey).toBe(B_KEY)

      const elsewhere = dropDestination(primaryScope(), { x: 1, y: 1 }, docWith(null))
      if ('refused' in elsewhere) throw new Error('refused outside any room')
      expect(elsewhere.scope).toBe(primaryScope())
    } finally {
      await room.dispose()
    }
  })

  it('a view-only room refuses the drop', async () => {
    const viewOnly = pinned(true)
    try {
      roomShown(viewOnly)
      const dest = dropDestination(primaryScope(), { x: 1, y: 1 }, docWith(viewOnly.key))
      expect(dest).toEqual({ refused: `View only: nothing is dropped into this room on ${viewOnly.scope.label}.` })
    } finally {
      await viewOnly.dispose()
    }
  })
})

describe('file clipboard and undo stay on their server (MS4)', () => {
  it('copy records the server; undo in a room pops only that server’s operations', () => {
    useFileClipboardStore.getState().copy(['/srv/anna/a.txt'], B_KEY)
    expect(useFileClipboardStore.getState().hostKey).toBe(B_KEY)
    useFileClipboardStore.getState().clear()
    expect(useFileClipboardStore.getState().hostKey).toBe(null)

    fileUndoFor(B_KEY).push({ type: 'create', path: '/srv/anna/b.txt' })
    fileUndoFor('local').push({ type: 'create', path: '/Users/me/c.txt' })
    useFileUndoStore.getState().push({ type: 'create', path: '/Users/me/untagged.txt' })
    expect(fileUndoFor(B_KEY).pop()).toEqual({ type: 'create', path: '/srv/anna/b.txt', hostKey: B_KEY })
    expect(fileUndoFor(B_KEY).pop()).toBe(undefined)
    // Untagged operations are this computer's.
    expect(fileUndoFor('local').pop()).toEqual({ type: 'create', path: '/Users/me/untagged.txt' })
    expect(fileUndoFor('local').pop()).toEqual({ type: 'create', path: '/Users/me/c.txt', hostKey: 'local' })
    expect(useFileUndoStore.getState().stack).toEqual([])
  })
})

describe('a link clicked in a terminal', () => {
  it('in a room on another server opens on this computer, never on that server’s screen', async () => {
    const room = pinned()
    const fetchSpy = vi.spyOn(globalThis, 'fetch')
    try {
      openTerminalUrl(room, 'https://example.com/docs')
      await vi.waitFor(() => expect(h.opened).toEqual(['https://example.com/docs']))
      expect(fetchSpy.mock.calls.filter(([u]) => String(u).includes('fs/open-external'))).toEqual([])
    } finally {
      fetchSpy.mockRestore()
      await room.dispose()
    }
  })
})
