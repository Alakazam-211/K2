// @vitest-environment jsdom
//
// Settings → Editors / Fonts → Defaults → Default editor feeds the "Open in"
// items of a workspace's right-click menu, in the nav (Sidebar) and on the
// icon rail: the picked editor is the first editor offered, and a new pick
// changes it. The real settings store and its setter (the one the Defaults
// page calls) are used; only the server and the native menu are faked.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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

import Sidebar from './Sidebar'
import IconRail from './IconRail'
import { showContextMenu } from '@/lib/context-menu'
import { useSettingsStore } from '@/stores/settings'

type Item = { id: string; label: string }

/** Right-click workspace `a` and return the editor items the menu offered. */
async function editorItems(open: () => void): Promise<Item[]> {
  const menu = vi.mocked(showContextMenu)
  menu.mockClear()
  await act(async () => {
    open()
  })
  await waitFor(() => expect(menu).toHaveBeenCalledTimes(1))
  const items = menu.mock.calls[0][0] as Item[]
  return items.filter((i) => /^(ws-)?editor:/.test(i.id))
}

/** Pick the default editor the way the Defaults page does. */
async function pickDefault(label: string): Promise<void> {
  await act(async () => {
    await useSettingsStore.getState().updateProjectSetting('__global__', 'defaultEditor', label)
  })
  expect((useSettingsStore.getState().projectSettings.__global__ as { defaultEditor?: string }).defaultEditor).toBe(label)
}

beforeEach(() => {
  localStorage.clear()
  useSettingsStore.setState({ projectSettings: {} })
})

afterEach(() => cleanup())

describe('Default editor → Open in menus', () => {
  it('the nav: the default editor is the first Open in item, and a new pick changes it', async () => {
    render(<Sidebar />)
    const rightClick = (): void => void fireEvent.contextMenu(screen.getByText('Agent a'))

    expect((await editorItems(rightClick)).map((i) => i.label)).toEqual(['Open in Cursor', 'Open in VS Code', 'Open in Zed'])

    await pickDefault('Zed')
    const first = (await editorItems(rightClick))[0]
    expect(first.label).toBe('Open in Zed')

    await pickDefault('VS Code')
    expect((await editorItems(rightClick)).map((i) => i.label)).toEqual(['Open in VS Code', 'Open in Cursor', 'Open in Zed'])
  })

  it('the icon rail: the default editor is the first Open in item, and a new pick changes it', async () => {
    const { container } = render(<IconRail />)
    const icon = (): Element => {
      const el = container.querySelector('[title^="Agent a"]')
      if (!el) throw new Error('no rail icon for Agent a')
      return el
    }
    const rightClick = (): void => void fireEvent.contextMenu(icon())

    await pickDefault('Zed')
    expect((await editorItems(rightClick)).map((i) => i.label)).toEqual(['Open in Zed', 'Open in Cursor', 'Open in VS Code'])

    await pickDefault('VS Code')
    expect((await editorItems(rightClick))[0].label).toBe('Open in VS Code')
  })
})
