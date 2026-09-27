// @vitest-environment jsdom
// Heartbeat click opens the session that is already running.
// Fail loud — no skip. Does not boot a daemon.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const cli = vi.hoisted(() => ({
  posts: [] as Array<{ route: string; body: unknown }>,
  get: vi.fn(async (_route: string, _params?: unknown): Promise<unknown> => []),
  getText: vi.fn(async (_route: string, _params?: unknown): Promise<string> => '{}'),
  invoke: vi.fn(async (cmd: string, _args?: unknown): Promise<unknown> => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    throw new Error(`unexpected invoke ${cmd}`)
  }),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => cli.invoke(cmd, args),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (route: string, params?: unknown) => cli.get(route, params),
  daemonCliGetText: (route: string, params?: unknown) => cli.getText(route, params),
  daemonCliPost: (route: string, body?: unknown) => {
    cli.posts.push({ route, body })
    return Promise.resolve({})
  },
  RecoveringError: class RecoveringError extends Error {},
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/lib/heartbeat-launch', () => ({
  launchHeartbeat: vi.fn(async () => undefined),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
  terminalKill: vi.fn(async () => undefined),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => false),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
  invalidateDaemonWs: vi.fn(),
}))
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn(() => () => undefined),
  onSessionRemovedApp: vi.fn(() => () => undefined),
  onAppHello: vi.fn(() => () => undefined),
  onOpenUrl: vi.fn(() => () => undefined),
  onChatHistoryChanged: vi.fn(() => () => undefined),
  onceRecovered: vi.fn(),
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: {
    getState: () => ({ activeHost: 'local', recovery: { kind: 'connected' } }),
    subscribe: vi.fn(() => () => undefined),
  },
  onActiveHostChange: vi.fn(() => () => undefined),
  activeHostKey: vi.fn(() => 'local'),
}))
vi.mock('@/stores/presets', () => ({
  usePresetsStore: { getState: () => ({ presets: [] }) },
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'claude'),
  setChatSession: vi.fn(async () => undefined),
}))

import { HeartbeatEntryRow } from '@/components/HeartbeatsPanel/HeartbeatEntry'
import { openHeartbeatTarget } from '@/components/common/HeartbeatSessionPicker'
import type { HeartbeatEntry } from '@/stores/heartbeat-sessions'
import { useTabsStore, type Tab, type TerminalItemData } from '@/stores/tabs'

const PROJECT = '/ws/proj'
const SID = '11111111-1111-4111-8111-111111111111'
const root = join(dirname(fileURLToPath(import.meta.url)), '../../..')

function agentTab(id: string, section: 'chat' | 'inbox'): Tab {
  const pgId = `pg-${id}`
  return {
    id,
    title: section === 'chat' ? 'Chat' : 'Inbox',
    isSystemAgent: true,
    mosaicTree: pgId,
    paneGroups: new Map([[
      pgId,
      {
        id: pgId,
        items: [{
          id: `item-${id}`,
          type: 'agent',
          data: { agentName: 'claude', projectPath: PROJECT, section },
        }],
        activeItemIndex: 0,
      },
    ]]),
  }
}

function convoTab(id: string, sessionId: string): Tab {
  const pgId = `pg-${id}`
  return {
    id,
    title: 'Saved',
    mosaicTree: pgId,
    paneGroups: new Map([[
      pgId,
      {
        id: pgId,
        items: [{
          id: `item-${id}`,
          type: 'terminal',
          data: {
            terminalId: `term-${id}`,
            cwd: PROJECT,
            command: 'claude',
            args: ['--resume', sessionId],
            conversationId: sessionId,
          } satisfies TerminalItemData,
        }],
        activeItemIndex: 0,
      },
    ]]),
  }
}

function entry(overrides: Partial<HeartbeatEntry['row']> & { state?: HeartbeatEntry['state'] }): HeartbeatEntry {
  const { state, ...row } = overrides
  return {
    state: state ?? 'scheduled',
    liveTerminalId: null,
    row: {
      id: 'hb-1',
      projectId: 'proj-1',
      name: 'daily',
      frequency: 'daily',
      specJson: '{"frequency":"daily","time":"09:00"}',
      wakeupPath: 'WAKEUP.md',
      enabled: true,
      lastFired: null,
      lastSessionId: SID,
      useWorkspaceSession: false,
      sessionProvider: null,
      archivedAt: null,
      createdAt: 0,
      ...row,
    },
  }
}

let heartbeatRows: Array<{ name: string; lastSessionId: string | null }> = []
let activePayload: Record<string, unknown> = {
  name: 'daily',
  claudeSessionId: null,
  activeTerminalId: null,
  activeAgentName: null,
  sessionAlive: false,
  isV2: false,
}

function resetTabs(partial?: Parameters<typeof useTabsStore.setState>[0]): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    navHistory: [],
    navIndex: -1,
    activeWorkspaceKey: null,
    activeProjectId: null,
    activeWorkspaceId: null,
    ...partial,
  })
}

function terminalData(tab: Tab): TerminalItemData[] {
  const out: TerminalItemData[] = []
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type === 'terminal') out.push(item.data as TerminalItemData)
    }
  }
  return out
}

beforeEach(() => {
  cli.posts = []
  cli.get.mockReset()
  cli.getText.mockReset()
  cli.invoke.mockClear()
  heartbeatRows = []
  activePayload = {
    name: 'daily',
    claudeSessionId: null,
    activeTerminalId: null,
    activeAgentName: null,
    sessionAlive: false,
    isV2: false,
  }
  cli.invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    if (cmd === 'k2so_agents_list') {
      return [{ name: 'claude', agentType: 'custom' }]
    }
    throw new Error(`unexpected invoke ${cmd}`)
  })
  cli.get.mockImplementation(async (route: string) => {
    if (route === 'chat/list') return []
    if (route === 'chat/session-exists') return { exists: true }
    if (route === 'agents/list') return [{ name: 'claude', agentType: 'custom' }]
    if (route === 'heartbeat/list') return heartbeatRows
    throw new Error(`unexpected GET ${route}`)
  })
  cli.getText.mockImplementation(async (route: string) => {
    if (route !== 'heartbeat/active-session') throw new Error(`unexpected GET text ${route}`)
    return JSON.stringify(activePayload)
  })
  resetTabs()
})

afterEach(() => {
  cleanup()
})

describe('pinned chat is decided before any conversation search', () => {
  function pinnedStrips(): void {
    resetTabs({
      tabs: [
        agentTab('chat', 'chat'),
        agentTab('inbox', 'inbox'),
        convoTab('leftover', SID),
      ],
      activeTabId: 'leftover',
      extraGroups: [{ tabs: [convoTab('extra-leftover', SID)], activeTabId: null }],
      splitCount: 2,
    })
  }

  it('the open icon focuses the pinned Chat tab and does not append a companion', async () => {
    pinnedStrips()
    await openHeartbeatTarget(PROJECT, 'daily', 'pinned')
    const state = useTabsStore.getState()
    expect(state.activeTabId).toBe('chat')
    expect(state.tabs.map((t) => t.id)).toEqual(['chat', 'inbox', 'leftover'])
    expect(state.extraGroups[0].tabs.map((t) => t.id)).toEqual(['extra-leftover'])
    expect(cli.getText).not.toHaveBeenCalled()
    expect(cli.posts).toEqual([])
    expect(cli.invoke).not.toHaveBeenCalledWith('k2so_heartbeat_list', expect.anything())
  })

  it('the drawer row focuses the pinned Chat tab and does not append a companion', async () => {
    pinnedStrips()
    render(
      <HeartbeatEntryRow
        projectPath={PROJECT}
        entry={entry({ useWorkspaceSession: true, lastSessionId: SID, sessionProvider: 'claude' })}
      />,
    )
    fireEvent.click(screen.getByTitle('daily — scheduled'))
    await waitFor(() => {
      expect(useTabsStore.getState().activeTabId).toBe('chat')
    })
    const state = useTabsStore.getState()
    expect(state.tabs.map((t) => t.id)).toEqual(['chat', 'inbox', 'leftover'])
    expect(state.extraGroups[0].activeTabId).toBeNull()
    expect(cli.getText).not.toHaveBeenCalled()
    expect(cli.posts).toEqual([])
    expect(cli.invoke.mock.calls.map((c) => c[0])).not.toContain('k2so_heartbeat_list')
  })
})

describe('an already-open session is focused by conversation id', () => {
  it('an extra-strip tab with that conversation id is focused; a main-strip tab-* is not required', async () => {
    resetTabs({
      tabs: [convoTab('main-other', '22222222-2222-4222-8222-222222222222')],
      activeTabId: 'main-other',
      extraGroups: [{ tabs: [convoTab('extra', SID)], activeTabId: null }],
      splitCount: 2,
    })
    activePayload = {
      name: 'daily',
      claudeSessionId: SID,
      activeTerminalId: null,
      activeAgentName: null,
      sessionAlive: false,
      isV2: false,
    }
    await openHeartbeatTarget(PROJECT, 'daily', 'session')
    const state = useTabsStore.getState()
    expect(state.extraGroups[0].activeTabId).toBe('extra')
    expect(state.tabs.map((t) => t.id)).toEqual(['main-other'])
    expect(state.activeTabId).toBe('main-other')
    expect(cli.posts).toEqual([])
    expect(cli.get.mock.calls.map((c) => c[0])).not.toContain('heartbeat/list')
    expect(cli.invoke.mock.calls.map((c) => c[0])).not.toContain('k2so_heartbeat_list')

    // Own-session mode uses the same open path.
    await openHeartbeatTarget(PROJECT, 'daily', 'auto')
    expect(useTabsStore.getState().extraGroups[0].activeTabId).toBe('extra')
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('the drawer focuses that extra-strip tab for an own-session id with no provider', async () => {
    resetTabs({
      tabs: [],
      extraGroups: [{ tabs: [convoTab('extra', SID)], activeTabId: null }],
      splitCount: 2,
    })
    activePayload = { ...activePayload, claudeSessionId: SID, sessionAlive: false }
    render(
      <HeartbeatEntryRow
        projectPath={PROJECT}
        entry={entry({ useWorkspaceSession: false, lastSessionId: SID, sessionProvider: null })}
      />,
    )
    fireEvent.click(screen.getByTitle('daily — scheduled'))
    await waitFor(() => {
      expect(useTabsStore.getState().extraGroups[0].activeTabId).toBe('extra')
    })
    expect(useTabsStore.getState().tabs).toHaveLength(0)
    expect(cli.posts).toEqual([])
  })
})

describe('a live heartbeat process with no tab is attached', () => {
  it('attaches <project>:hb:<name> and does not spawn or post set-surfaced', async () => {
    activePayload = {
      name: 'daily',
      claudeSessionId: null,
      activeTerminalId: 'pty-1',
      activeAgentName: 'proj1:hb:daily',
      sessionAlive: true,
      isV2: true,
    }
    const id = await useTabsStore.getState().openHeartbeatTab(PROJECT, 'daily')
    const state = useTabsStore.getState()
    expect(state.tabs).toHaveLength(1)
    expect(state.activeTabId).toBe(id)
    const data = terminalData(state.tabs[0])
    expect(data).toHaveLength(1)
    expect(data[0].attachAgentName).toBe('proj1:hb:daily')
    expect(data[0].terminalId).toBe('pty-1')
    expect(data[0].renderer).toBe('kessel')
    expect(data[0].heartbeatName).toBe('daily')
    expect(cli.posts).toEqual([])
    expect(cli.invoke.mock.calls.map((c) => c[0])).not.toContain('k2so_heartbeat_list')

    await useTabsStore.getState().openHeartbeatTab(PROJECT, 'daily')
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    expect(cli.posts).toEqual([])
  })

  it('focuses an open conversation instead of attaching a second tab', async () => {
    resetTabs({
      tabs: [],
      extraGroups: [{ tabs: [convoTab('extra', SID)], activeTabId: null }],
      splitCount: 2,
    })
    activePayload = {
      name: 'daily',
      claudeSessionId: SID,
      activeTerminalId: 'pty-1',
      activeAgentName: 'proj1:hb:daily',
      sessionAlive: true,
      isV2: true,
    }
    await useTabsStore.getState().openHeartbeatTab(PROJECT, 'daily')
    const state = useTabsStore.getState()
    expect(state.tabs).toHaveLength(0)
    expect(state.extraGroups[0].activeTabId).toBe('extra')
    expect(cli.posts).toEqual([])
  })
})

describe('cold path reads the host list or the id active-session already returned', () => {
  it('uses claudeSessionId and does not call k2so_heartbeat_list or heartbeat/list', async () => {
    activePayload = {
      name: 'daily',
      claudeSessionId: SID,
      activeTerminalId: null,
      activeAgentName: null,
      sessionAlive: false,
      isV2: false,
    }
    heartbeatRows = [{ name: 'daily', lastSessionId: 'should-not-be-read' }]
    const id = await useTabsStore.getState().openHeartbeatTab(PROJECT, 'daily')
    expect(id).toBeTruthy()
    const tab = useTabsStore.getState().tabs.find((t) => t.id === id)
    expect(tab).toBeTruthy()
    const data = terminalData(tab!)
    expect(data[0].conversationId).toBe(SID)
    expect(data[0].args).toContain(SID)
    expect(data[0].renderer).toBe('kessel')
    expect(data[0].attachAgentName).toBeUndefined()
    expect(cli.get.mock.calls.map((c) => c[0])).not.toContain('heartbeat/list')
    expect(cli.invoke.mock.calls.map((c) => c[0])).not.toContain('k2so_heartbeat_list')
    expect(cli.posts).toEqual([])
  })

  it('reads the host heartbeat list when active-session has no id', async () => {
    heartbeatRows = [{ name: 'daily', lastSessionId: SID }]
    const id = await useTabsStore.getState().openHeartbeatTab(PROJECT, 'daily')
    expect(id).toBeTruthy()
    expect(terminalData(useTabsStore.getState().tabs.find((t) => t.id === id)! )[0].conversationId).toBe(SID)
    expect(cli.get).toHaveBeenCalledWith('heartbeat/list', { project: PROJECT })
    expect(cli.invoke.mock.calls.map((c) => c[0])).not.toContain('k2so_heartbeat_list')
  })

  it('with no saved id, logs click Launch first and opens nothing', async () => {
    heartbeatRows = [{ name: 'daily', lastSessionId: null }]
    const info = vi.spyOn(console, 'info').mockImplementation(() => {})
    const id = await useTabsStore.getState().openHeartbeatTab(PROJECT, 'daily')
    expect(id).toBeNull()
    expect(useTabsStore.getState().tabs).toHaveLength(0)
    expect(info).toHaveBeenCalledWith(
      '[openHeartbeatTab] heartbeat has no saved session yet; click Launch first',
    )
    expect(cli.invoke.mock.calls.map((c) => c[0])).not.toContain('k2so_heartbeat_list')
    info.mockRestore()
  })
})

describe('existingTerminalId is not passed and ignored', () => {
  it('the drawer and openHeartbeatTab no longer mention existingTerminalId', () => {
    const tabsSrc = readFileSync(join(root, 'src/renderer/stores/tabs.ts'), 'utf8')
    const drawerSrc = readFileSync(
      join(root, 'src/renderer/components/HeartbeatsPanel/HeartbeatEntry.tsx'),
      'utf8',
    )
    expect(tabsSrc).not.toContain('existingTerminalId')
    expect(drawerSrc).not.toContain('existingTerminalId')
  })
})
