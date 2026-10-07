// k2 sidecar v1 (prd-k2-sidecar-cli-v1 T3, SC33/SC34): a sidecar the
// daemon opened (`k2 sidecar new`, or a `k2 msg` wake) shows up on every
// client as a tab titled with its NAME, locked — from the `session_added`
// push, the workspace-open reconcile and the reconnect hello. The tab id is
// `adopted-<paneGroupId>`, the same id the daemon writes into the saved
// layout, so a second adoption never makes a second tab. `session_removed`
// (what `k2 sidecar stop` emits) drops it.

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
  subscribeToWorkspaceSessionEvents: vi.fn((_scope: unknown, path: string, handlers: Record<string, Fn>) => {
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
    label?: string
    labelLocked?: boolean
  }>,
}))
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
vi.stubGlobal('fetch', vi.fn(async () => ({
  ok: true,
  status: 200,
  text: async () => '{}',
  json: async () => ({}),
})))

import type { Tab } from './tabs'

const CWD = '/ws/k2'

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

function tabFor(tabs: Tab[], pgId: string): Tab {
  const found = tabs.filter((t) => t.paneGroups.has(pgId))
  if (found.length !== 1) throw new Error(`expected exactly one tab for ${pgId}, got ${found.length}`)
  return found[0]
}

function sessionAdded(pgId: string, extra: Record<string, unknown>) {
  return {
    kind: 'session_added',
    workspace_path: CWD,
    pane_group_id: pgId,
    agent_name: `tab-${pgId}`,
    command: 'claude',
    args: ['--dangerously-skip-permissions', '--session-id', 'cid-1'],
    session_id: `s-${pgId}`,
    isV2: true,
    ...extra,
  }
}

let keyCounter = 0

describe('k2 sidecar adoption (session_added label)', () => {
  let projectId: string
  let workspaceId: string

  beforeEach(() => {
    keyCounter += 1
    projectId = `sp${keyCounter}`
    workspaceId = `sw${keyCounter}`
    daemon.layouts.clear()
    daemon.sessions = []
    ev.sessionSubs = []
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

  function sessionSub(): { handlers: Record<string, Fn> } {
    const sub = ev.sessionSubs.find((s) => s.handlers.onAdded)
    if (!sub) throw new Error('workspace session subscription was not opened')
    return sub
  }

  it('push: a labelled session_added adopts adopted-<pg> titled with the name, locked', async () => {
    daemon.layouts.set(`${projectId}:${workspaceId}`, {
      json: JSON.stringify({ version: 2, tabs: [layoutTab('tab-main', 'pg-main')] }),
      revision: 1,
    })
    const { useTabsStore } = await loadWorkspace()
    sessionSub().handlers.onAdded(sessionAdded('pg-gardens', { label: 'Gardens', labelLocked: true }))
    await flush()
    const tab = tabFor(useTabsStore.getState().tabs, 'pg-gardens')
    expect(tab.id).toBe('adopted-pg-gardens')
    expect(tab.title).toBe('Gardens')
    expect(tab.locked).toBe(true)
    // The adoption is saved through the based layout save.
    const save = vi.mocked(daemonCliPost).mock.calls.find((c) => c[0] === 'workspace-layouts/save')
    if (!save) throw new Error('expected workspace-layouts/save after adoption')
    expect(String((save[1] as { layoutJson?: string }).layoutJson)).toContain('adopted-pg-gardens')

    // A second adoption of the same pane (another window, a replayed event)
    // never makes a second tab.
    sessionSub().handlers.onAdded(sessionAdded('pg-gardens', { label: 'Gardens', labelLocked: true }))
    await flush()
    expect(useTabsStore.getState().tabs.filter((t) => t.paneGroups.has('pg-gardens'))).toHaveLength(1)

    // `k2 sidecar stop` → session_removed drops the tab on every client.
    sessionSub().handlers.onRemoved({
      kind: 'session_removed',
      workspace_path: CWD,
      pane_group_id: 'pg-gardens',
      agent_name: 'tab-pg-gardens',
    })
    await flush()
    expect(useTabsStore.getState().tabs.some((t) => t.paneGroups.has('pg-gardens'))).toBe(false)
  })

  it('push without a label keeps the command title and stays unlocked', async () => {
    daemon.layouts.set(`${projectId}:${workspaceId}`, {
      json: JSON.stringify({ version: 2, tabs: [layoutTab('tab-main', 'pg-main')] }),
      revision: 1,
    })
    const { useTabsStore } = await loadWorkspace()
    sessionSub().handlers.onAdded(sessionAdded('pg-plain', {}))
    await flush()
    const tab = tabFor(useTabsStore.getState().tabs, 'pg-plain')
    expect(tab.title).toBe('claude')
    expect(tab.locked).toBeUndefined()
  })

  it('workspace-open reconcile titles an orphan daemon sidecar from list-for-workspace', async () => {
    daemon.layouts.set(`${projectId}:${workspaceId}`, {
      json: JSON.stringify({ version: 2, tabs: [layoutTab('tab-main', 'pg-main')] }),
      revision: 1,
    })
    daemon.sessions = [{
      sessionId: 's-cal',
      agentName: 'tab-pg-cal',
      command: 'codex',
      args: ['--yolo'],
      cwd: CWD,
      isV2: true,
      label: 'Calendars',
      labelLocked: true,
    }]
    const { useTabsStore } = await loadWorkspace()
    const tab = tabFor(useTabsStore.getState().tabs, 'pg-cal')
    expect(tab.id).toBe('adopted-pg-cal')
    expect(tab.title).toBe('Calendars')
    expect(tab.locked).toBe(true)
  })

  it('reconnect hello adopts a sidecar opened while the socket was down, by name', async () => {
    daemon.layouts.set(`${projectId}:${workspaceId}`, {
      json: JSON.stringify({ version: 2, tabs: [layoutTab('tab-main', 'pg-main')] }),
      revision: 1,
    })
    const { useTabsStore } = await loadWorkspace()
    daemon.sessions = [{
      sessionId: 's-polish',
      agentName: 'tab-pg-polish',
      command: 'grok',
      args: [],
      cwd: CWD,
      isV2: true,
      label: 'Thread Polish',
      labelLocked: true,
    }]
    sessionSub().handlers.onHello()
    await flush()
    const tab = tabFor(useTabsStore.getState().tabs, 'pg-polish')
    expect(tab.id).toBe('adopted-pg-polish')
    expect(tab.title).toBe('Thread Polish')
    expect(tab.locked).toBe(true)
  })

  it('first open with no layout titles the adopted sidecar by name', async () => {
    daemon.sessions = [{
      sessionId: 's-first',
      agentName: 'tab-pg-first',
      command: 'claude',
      args: [],
      cwd: CWD,
      isV2: true,
      label: 'Gardens',
      labelLocked: true,
    }]
    const { useTabsStore } = await loadWorkspace()
    const tab = tabFor(useTabsStore.getState().tabs, 'pg-first')
    expect(tab.title).toBe('Gardens')
    expect(tab.locked).toBe(true)
  })
})
