// @vitest-environment jsdom
//
// Opening a workspace's Chat tab in the Thread view (K5): the Thread is asked
// for explicitly, Settings closes, the Agents page selects the workspace
// (after a fresh read when it isn't listed yet), and the pinned Chat tab
// becomes the active tab once the workspace's tabs show.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { StoreApi } from 'zustand'

type T = { id: string; isSystemAgent?: boolean; paneGroups: Map<string, { items: Array<{ type: string; data: unknown }> }> }
type TabsState = { activeWorkspaceKey: string | null; tabs: T[]; activeTabId: string | null; setActiveTab: (id: string) => void }

const h = vi.hoisted(() => ({
  closeSettings: vi.fn(),
  openOnAgentsPage: vi.fn(),
  fetchProjects: vi.fn(async () => {}),
  projects: [] as Array<{ id: string; workspaces: Array<{ id: string }> }>,
  activeProjectId: null as string | null,
  page: 'home',
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: { getState: () => ({ closeSettings: h.closeSettings }) },
}))
vi.mock('@/stores/page-view', () => ({ usePageViewStore: { getState: () => ({ page: h.page }) } }))
vi.mock('@/lib/page-selection', () => ({ openOnAgentsPage: (...a: unknown[]) => h.openOnAgentsPage(...a) }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: {
    getState: () => ({ projects: h.projects, activeProjectId: h.activeProjectId, fetchProjects: h.fetchProjects }),
  },
}))

const tabsStore = vi.hoisted(() => ({ ref: null as unknown }))
vi.mock('@/stores/tabs', async () => {
  const { create: mk } = await import('zustand')
  const store = mk<TabsState>((set) => ({
    activeWorkspaceKey: null,
    tabs: [],
    activeTabId: null,
    setActiveTab: (id) => set({ activeTabId: id }),
  }))
  tabsStore.ref = store
  return {
    useTabsStore: store,
    pinnedTabOf: (tabs: T[]) => {
      const system = tabs.filter((t) => t.isSystemAgent)
      const chat = system.find((t) => (Array.from(t.paneGroups.values())[0]?.items[0]?.data as { section?: string })?.section === 'chat')
      return chat ?? system[0] ?? null
    },
  }
})

import { CHAT_TAB_WAIT_MS, openWorkspaceThread } from './open-workspace-thread'
import { useSessionViewRequestStore } from '@/stores/session-view-request'

const tabs = (): StoreApi<TabsState> => tabsStore.ref as StoreApi<TabsState>

function agentTab(id: string, section: string): T {
  return { id, isSystemAgent: true, paneGroups: new Map([['pg', { items: [{ type: 'agent', data: { section } }] }]]) }
}

beforeEach(() => {
  vi.useFakeTimers()
  h.closeSettings.mockReset()
  h.openOnAgentsPage.mockReset()
  h.fetchProjects.mockReset()
  h.projects = [{ id: 'ws-k5', workspaces: [{ id: 'w1' }] }]
  h.activeProjectId = null
  h.page = 'home'
  useSessionViewRequestStore.setState({ pending: null })
  tabs().setState({ activeWorkspaceKey: null, tabs: [], activeTabId: null })
})

afterEach(() => {
  vi.useRealTimers()
})

describe('openWorkspaceThread', () => {
  it('asks for the Thread, closes Settings, selects the workspace and makes its Chat tab active', async () => {
    const p = openWorkspaceThread('ws-k5')
    expect(useSessionViewRequestStore.getState().pending).toMatchObject({ projectId: 'ws-k5', view: 'thread' })
    expect(h.closeSettings).toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(0)
    expect(h.openOnAgentsPage).toHaveBeenCalledWith('ws-k5', 'w1')
    // The workspace's tabs land: Inbox active, Chat there.
    tabs().setState({ activeWorkspaceKey: 'ws-k5:w1', tabs: [agentTab('chat-1', 'chat'), agentTab('inbox-1', 'inbox')], activeTabId: 'inbox-1' })
    await expect(p).resolves.toBe(true)
    expect(tabs().getState().activeTabId).toBe('chat-1')
  })

  it('reads the list again for a workspace it does not know yet', async () => {
    h.projects = []
    h.fetchProjects.mockImplementation(async () => {
      h.projects = [{ id: 'ws-new', workspaces: [{ id: 'w9' }] }]
    })
    tabs().setState({ activeWorkspaceKey: 'ws-new:w9', tabs: [agentTab('chat-9', 'chat')], activeTabId: null })
    await expect(openWorkspaceThread('ws-new')).resolves.toBe(true)
    expect(h.fetchProjects).toHaveBeenCalledTimes(1)
    expect(h.openOnAgentsPage).toHaveBeenCalledWith('ws-new', 'w9')
    expect(tabs().getState().activeTabId).toBe('chat-9')
  })

  it('a workspace the server still does not list opens nothing', async () => {
    h.projects = []
    await expect(openWorkspaceThread('ws-gone')).resolves.toBe(false)
    expect(h.openOnAgentsPage).not.toHaveBeenCalled()
  })

  it('stops waiting for the tabs after a while; the view ask stands', async () => {
    const p = openWorkspaceThread('ws-k5')
    await vi.advanceTimersByTimeAsync(CHAT_TAB_WAIT_MS + 1)
    await expect(p).resolves.toBe(true)
    expect(useSessionViewRequestStore.getState().pending?.projectId).toBe('ws-k5')
  })

  it('does not select again when Agents already shows it', async () => {
    h.page = 'agents'
    h.activeProjectId = 'ws-k5'
    tabs().setState({ activeWorkspaceKey: 'ws-k5:w1', tabs: [agentTab('chat-1', 'chat')], activeTabId: 'other' })
    await openWorkspaceThread('ws-k5')
    expect(h.openOnAgentsPage).not.toHaveBeenCalled()
    expect(tabs().getState().activeTabId).toBe('chat-1')
  })
})
