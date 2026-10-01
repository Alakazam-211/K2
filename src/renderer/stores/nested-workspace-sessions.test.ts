// A nested registered project stays off the parent workspace's terminal
// list. Reconcile only sees that filtered list, so it must not adopt or
// refresh the omitted `tab-` row. A layout pane whose spawn comes back
// `session_owned_elsewhere` is removed and saved, with no v2/close.

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => true),
}))

type Fn = (...a: unknown[]) => void

const ev = vi.hoisted(() => {
  return {
    sessionSubs: [] as Array<{ path: string; handlers: Record<string, Fn> }>,
  }
})
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn((path: string, handlers: Record<string, Fn>) => {
    const entry = { path, handlers }
    ev.sessionSubs.push(entry)
    return () => void (ev.sessionSubs = ev.sessionSubs.filter((e) => e !== entry))
  }),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn(() => () => undefined),
  onSessionRemovedApp: vi.fn(() => () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 0, token: 't', secure: false, host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:0'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:0'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))

const daemon = vi.hoisted(() => ({
  layouts: new Map<string, { json: string; revision: number }>(),
  sessions: [] as Array<{
    sessionId: string
    agentName: string
    command: string | null
    args: string[]
    cwd: string
    isV2: boolean
  }>,
}))
// The test inspects `daemonCliPost` calls; the module export is a
// `primaryOnly` wrapper that checks the scope and forwards the rest here.
const daemonCliPost = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => ({ success: true, revision: 1 })))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: { project_id?: string; workspace_id?: string }) => {
      if (route === 'workspace-layouts/load') {
        const key = `${params?.project_id}:${params?.workspace_id}`
        return daemon.layouts.get(key)?.json ?? null
      }
      if (route === 'sessions/list-for-workspace') return daemon.sessions
      if (route === 'workspace/tab-titles') return []
      return []
    }),
    daemonCliPost: primaryOnly((...args: unknown[]) => daemonCliPost(...args)),
  }
})
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
  terminalCreate: vi.fn(async () => undefined),
  terminalExists: vi.fn(async () => false),
  terminalKill: vi.fn(async () => undefined),
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: Object.assign(vi.fn(() => undefined), {
    getState: () => ({ defaultAgent: null, agenticSystemsEnabled: true, fetchSettings: vi.fn() }),
    setState: vi.fn(),
    subscribe: vi.fn(() => () => undefined),
  }),
}))

vi.stubGlobal('window', {
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  __TAURI_INTERNALS__: { transformCallback: () => 0, invoke: async () => undefined },
})
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

const fetchUrls: string[] = []
vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
  fetchUrls.push(String(input))
  return {
    ok: true,
    status: 200,
    text: async () => '{}',
    json: async () => ({}),
  }
}))

import type { Tab, TerminalItemData } from './tabs'

const CWD = '/ws/parent'

function layoutTab(id: string, pgId: string) {
  return {
    id,
    title: id,
    mosaicTree: pgId,
    paneGroups: {
      [pgId]: {
        id: pgId,
        items: [{ id: `item-${pgId}`, type: 'terminal', paneGroupId: pgId }],
        activeItemIndex: 0,
      },
    },
  }
}

async function flush(n = 20): Promise<void> {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0))
}

function paneIds(tabs: Tab[]): string[] {
  return tabs.flatMap((tab) => [...tab.paneGroups.keys()]).sort()
}

function commandOf(tabs: Tab[], pgId: string): string | undefined {
  for (const tab of tabs) {
    const pg = tab.paneGroups.get(pgId)
    if (!pg) continue
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      return (item.data as TerminalItemData).command
    }
  }
  throw new Error(`pane ${pgId} is not surfaced`)
}

function terminalPane(paneId: string) {
  return {
    id: paneId,
    items: [{
      id: `item-${paneId}`,
      type: 'terminal' as const,
      data: {
        terminalId: paneId,
        cwd: CWD,
        renderer: 'kessel' as const,
      },
    }],
    activeItemIndex: 0,
  }
}

let keyCounter = 0

describe('nested workspace sessions', () => {
  let projectId: string
  let workspaceId: string
  let key: string

  beforeEach(() => {
    keyCounter += 1
    projectId = `p${keyCounter}`
    workspaceId = `w${keyCounter}`
    key = `${projectId}:${workspaceId}`
    daemon.layouts.clear()
    daemon.sessions = []
    ev.sessionSubs = []
    fetchUrls.length = 0
    vi.mocked(daemonCliPost).mockClear()
  })

  async function loadWorkspace(): Promise<typeof import('./tabs')> {
    const mod = await import('./tabs')
    mod.useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
      backgroundWorkspaces: {},
      workspaceLayouts: {},
      activeWorkspaceKey: null,
      activeProjectId: null,
      activeWorkspaceId: null,
    })
    await mod.useTabsStore.getState().loadLayoutForWorkspace(projectId, workspaceId, CWD)
    await flush()
    return mod
  }

  it('reconcile does not adopt or refresh a tab- row the filtered list omits', async () => {
    daemon.layouts.set(key, {
      json: JSON.stringify({
        version: 2,
        tabs: [
          layoutTab('tab-parent', 'pg-parent'),
          layoutTab('tab-nested', 'pg-nested'),
        ],
      }),
      revision: 0,
    })
    // The daemon list is already filtered. The nested `tab-` row, which
    // would have carried command `claude`, is not in it.
    daemon.sessions = [{
      sessionId: 's-parent',
      agentName: 'tab-pg-parent',
      command: 'sleep',
      args: [],
      cwd: CWD,
      isV2: true,
    }]

    const { useTabsStore } = await loadWorkspace()
    const state = useTabsStore.getState()
    expect(paneIds(state.tabs)).toEqual(['pg-nested', 'pg-parent'])
    expect(commandOf(state.tabs, 'pg-parent')).toBe('sleep')
    expect(commandOf(state.tabs, 'pg-nested')).toBeUndefined()

    for (const sub of ev.sessionSubs) sub.handlers.onHello?.()
    await flush()
    const afterHello = useTabsStore.getState()
    expect(paneIds(afterHello.tabs)).toEqual(['pg-nested', 'pg-parent'])
    expect(commandOf(afterHello.tabs, 'pg-nested')).toBeUndefined()
    expect(commandOf(afterHello.tabs, 'pg-parent')).toBe('sleep')
  })

  it('launchDefaultAgent does not adopt a tab- row the filtered list omits', async () => {
    daemon.sessions = [{
      sessionId: 's-parent',
      agentName: 'tab-pg-parent',
      command: 'sleep',
      args: [],
      cwd: CWD,
      isV2: true,
    }]
    const { useTabsStore } = await loadWorkspace()
    const state = useTabsStore.getState()
    expect(paneIds(state.tabs)).toEqual(['pg-parent'])
    expect(commandOf(state.tabs, 'pg-parent')).toBe('sleep')
    expect(() => commandOf(state.tabs, 'pg-nested')).toThrow(/pane pg-nested is not surfaced/)
  })

  it('session_owned_elsewhere removes a one-terminal pane and saves without close', async () => {
    const mod = await import('./tabs')
    const owned = 'pane-owned-elsewhere'
    const keep = 'pane-keep-sibling'
    mod.useTabsStore.setState({
      tabs: [
        {
          id: 'tab-owned',
          title: 'Owned',
          mosaicTree: owned,
          paneGroups: new Map([[owned, terminalPane(owned)]]),
        },
        {
          id: 'tab-keep',
          title: 'Keep',
          mosaicTree: keep,
          paneGroups: new Map([[keep, terminalPane(keep)]]),
        },
      ],
      activeTabId: 'tab-owned',
      extraGroups: [],
      splitCount: 1,
      activeProjectId: projectId,
      activeWorkspaceId: workspaceId,
      activeWorkspaceKey: key,
    })
    vi.mocked(daemonCliPost).mockClear()
    fetchUrls.length = 0

    mod.releasePaneOwnedElsewhere(owned)

    const state = mod.useTabsStore.getState()
    expect(paneIds(state.tabs)).toEqual([keep])
    const save = vi.mocked(daemonCliPost).mock.calls.find((call) => call[0] === 'workspace-layouts/save')
    if (!save) throw new Error('expected workspace-layouts/save')
    const layoutJson = (save[1] as { layoutJson?: string }).layoutJson
    if (typeof layoutJson !== 'string') throw new Error('save body missing layoutJson')
    expect(layoutJson).not.toContain(owned)
    expect(layoutJson).toContain(keep)
    expect(vi.mocked(daemonCliPost).mock.calls.some((call) => String(call[0]).includes('close'))).toBe(false)
    expect(JSON.stringify(vi.mocked(daemonCliPost).mock.calls)).not.toContain('clear_index')
    expect(fetchUrls.some((url) => url.includes('/cli/sessions/v2/close'))).toBe(false)
    expect(fetchUrls.some((url) => url.includes('clear_index'))).toBe(false)
  })

  it('session_owned_elsewhere drops only that pane id from a split and saves', async () => {
    const mod = await import('./tabs')
    const owned = 'pane-owned-elsewhere'
    const keep = 'pane-keep-sibling'
    mod.useTabsStore.setState({
      tabs: [{
        id: 'tab-split',
        title: 'Split',
        mosaicTree: { direction: 'row', first: owned, second: keep, splitPercentage: 50 },
        paneGroups: new Map([
          [owned, terminalPane(owned)],
          [keep, terminalPane(keep)],
        ]),
      }],
      activeTabId: 'tab-split',
      extraGroups: [],
      splitCount: 1,
      activeProjectId: projectId,
      activeWorkspaceId: workspaceId,
      activeWorkspaceKey: key,
    })
    vi.mocked(daemonCliPost).mockClear()
    fetchUrls.length = 0

    mod.releasePaneOwnedElsewhere(owned)

    const state = mod.useTabsStore.getState()
    expect(state.tabs).toHaveLength(1)
    expect(paneIds(state.tabs)).toEqual([keep])
    expect(state.tabs[0].mosaicTree).toBe(keep)
    const save = vi.mocked(daemonCliPost).mock.calls.find((call) => call[0] === 'workspace-layouts/save')
    if (!save) throw new Error('expected workspace-layouts/save')
    const layoutJson = (save[1] as { layoutJson?: string }).layoutJson
    if (typeof layoutJson !== 'string') throw new Error('save body missing layoutJson')
    expect(layoutJson).not.toContain(owned)
    expect(layoutJson).toContain(keep)
    expect(vi.mocked(daemonCliPost).mock.calls.some((call) => String(call[0]).includes('close'))).toBe(false)
    expect(JSON.stringify(vi.mocked(daemonCliPost).mock.calls)).not.toContain('clear_index')
    expect(fetchUrls.some((url) => url.includes('/cli/sessions/v2/close'))).toBe(false)
  })
})
