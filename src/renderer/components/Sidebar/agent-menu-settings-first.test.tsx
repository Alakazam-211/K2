// @vitest-environment jsdom
//
// Right-clicking an agent puts "Workspace Settings" first, with a separator
// under it (Rosson 2026-11-18), in the Agents nav and in the Home nav, even
// while the agent has unread (Mark all as read used to sit above it). Both
// menus build through lib/agent-menu-order.ts; only the server and the
// native menu are faked.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  editors: [
    { id: 'cursor', label: 'Cursor' },
    { id: 'vscode', label: 'VS Code' },
    { id: 'zed', label: 'Zed' },
  ],
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => (cmd === 'projects_get_editors' ? h.editors : null)),
}))
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
// The server keeps every settings write and echoes it back.
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async (u: Record<string, unknown>) => ({ terminal: {}, ...u })),
  settingsReset: vi.fn(async () => ({})),
}))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => null) }))
vi.mock('@/lib/clone-permissions', () => ({
  fetchClonePerms: vi.fn(async () => ({})),
  pullAllowed: () => false,
  pushAllowed: () => false,
}))
vi.mock('@/stores/activity', () => ({
  useActivity: () => 'idle',
  workspaceDisplay: () => 'idle',
  projectHasUnseen: () => false,
}))
vi.mock('@/components/AgentStatus/useAgentStatus', () => ({
  useAgentStatus: () => ({ heartbeat: false, live: null, working: false, status: 'idle', title: '', workingTitle: '' }),
}))
vi.mock('@/hooks/useGit', () => ({ useGitInfo: () => ({ data: undefined }) }))
vi.mock('./ProjectAvatar', () => ({ default: () => <span /> }))
vi.mock('@/components/Presence/PresenceWorkspaceAvatars', () => ({ default: () => null }))
vi.mock('./NavProjectTags', () => ({ NavProjectTags: () => null }))
vi.mock('./ActiveBar', () => ({ default: () => null, useActiveBarItems: () => [] }))
vi.mock('./ResizeHandle', () => ({ default: () => null }))
vi.mock('./WorktreeDialog', () => ({ default: () => null }))
vi.mock('@/components/GrokBots/GrokBotRows', () => ({ GrokBotSidebarSection: () => null }))
vi.mock('@/stores/grokbots', () => ({ closeGrokChat: () => undefined }))

vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  const useProjectsStore = create<Record<string, unknown>>(() => ({
    projects: [
      {
        id: 'a',
        name: 'Agent a',
        path: '/w/a',
        color: '#61afef',
        pinned: 0,
        focusGroupId: null,
        iconUrl: null,
        handle: 'a',
        workspaces: [{ id: 'ws-a', type: 'main', navVisible: 0 }],
      },
    ],
    activeProjectId: null,
    activeWorkspaceId: null,
    setActiveProject: () => undefined,
    setActiveWorkspace: () => undefined,
    reorderProjects: async () => undefined,
    removeProject: async () => undefined,
    addProject: async () => undefined,
    fetchProjects: async () => undefined,
  }))
  return { useProjectsStore }
})

vi.mock('@/stores/focus-groups', async () => {
  const { create } = await import('zustand')
  const useFocusGroupsStore = create(() => ({
    focusGroups: [],
    activeFocusGroupId: null,
    focusGroupsEnabled: false,
    setActiveFocusGroup: () => undefined,
  }))
  return { useFocusGroupsStore }
})

// The agent has unread on a server that can mark it read: the menu offers
// Mark all as read (the case that used to sit above Workspace Settings).
vi.mock('@/lib/mark-all-read-menu', () => ({
  MARK_ALL_READ_MENU_ID: 'mark-all-read',
  markAllReadMenuItems: (_scope: unknown, _unread: number, opts: { separator?: boolean } = {}) =>
    opts.separator === false
      ? [{ id: 'mark-all-read', label: 'Mark all as read' }]
      : [
          { id: 'mark-all-read', label: 'Mark all as read' },
          { id: 'separator-mark-all-read', label: '', type: 'separator' },
        ],
  runMarkAllRead: vi.fn(async () => null),
}))

import Sidebar from './Sidebar'
import { homeRowContextMenu } from '@/components/Home/HomeSidebar'
import { agentMenuWithSettingsFirst, WORKSPACE_SETTINGS_MENU_LABEL } from '@/lib/agent-menu-order'
import { showContextMenu } from '@/lib/context-menu'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import type { Home } from '@/stores/homes'

/** The items of the one menu `open` shows. */
async function menuOf(open: () => void | Promise<void>): Promise<ContextMenuItemDef[]> {
  const menu = vi.mocked(showContextMenu)
  menu.mockClear()
  await act(async () => {
    await open()
  })
  await waitFor(() => expect(menu).toHaveBeenCalledTimes(1))
  return menu.mock.calls[0][0] as ContextMenuItemDef[]
}

function expectSettingsFirst(items: ContextMenuItemDef[], settingsId: string): void {
  expect(items[0]).toEqual({ id: settingsId, label: 'Workspace Settings' })
  expect(items[1].type).toBe('separator')
  expect(items.filter((i) => i.label === 'Workspace Settings')).toHaveLength(1)
  expect(items.map((i) => i.id)).toContain('mark-all-read')
}

afterEach(() => cleanup())

describe('Workspace Settings is the first item when right-clicking an agent', () => {
  it('the Agents nav', async () => {
    render(<Sidebar />)
    const items = await menuOf(() => void fireEvent.contextMenu(screen.getByText('Agent a')))
    expectSettingsFirst(items, 'settings')
    expect(items.slice(0, 5).map((i) => i.id)).toEqual(['settings', 'settings-separator', 'mark-all-read', 'view-wiki', 'separator-settings'])
  })

  it('the Home nav (and its rail, which opens the same menu)', async () => {
    const row = { address: 'a::local', workspaceId: 'a' }
    const home = { id: 'h1', name: 'Group 1', rows: [row] } as unknown as Home
    const e = { preventDefault: () => undefined } as unknown as React.MouseEvent
    const items = await menuOf(() => homeRowContextMenu(e, home, row as Home['rows'][number]))
    expectSettingsFirst(items, 'home-settings')
    expect(items.slice(0, 5).map((i) => i.id)).toEqual(['home-settings', 'home-settings-separator', 'mark-all-read', 'home-wiki', 'home-sep'])
  })
})

describe('agentMenuWithSettingsFirst', () => {
  const sep = (id: string): ContextMenuItemDef => ({ id, label: '', type: 'separator' })

  it('puts the item first with a line under it, dropping a stray copy and a leading line', () => {
    const items = agentMenuWithSettingsFirst('s', [sep('x'), { id: 'a', label: 'A' }, { id: 's', label: 'Old' }, { id: 'b', label: 'B' }])
    expect(items).toEqual([{ id: 's', label: WORKSPACE_SETTINGS_MENU_LABEL }, sep('s-separator'), { id: 'a', label: 'A' }, { id: 'b', label: 'B' }])
  })

  it('no line when it is the only item; null leaves the menu as it is', () => {
    expect(agentMenuWithSettingsFirst('s', [])).toEqual([{ id: 's', label: WORKSPACE_SETTINGS_MENU_LABEL }])
    const rest = [{ id: 'a', label: 'A' }]
    expect(agentMenuWithSettingsFirst(null, rest)).toBe(rest)
  })
})
