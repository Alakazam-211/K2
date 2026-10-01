// Sidecar refresh keeps the same strip tab. The guard is per renderer
// window. These tests do not boot a daemon. Fail loud — no skip.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import {
  beginSidecarRefresh,
  paneGroupIdFromTabAgent,
  resetSidecarRefreshGuards,
  settleSidecarRefresh,
  sidecarRefreshMark,
  takeSessionRemoved,
} from './sidecar-refresh-tab'

const PRE_UNREGISTER = [
  'sidecar refresh has no resumable session',
  'sidecar refresh has no provider session id',
  'sidecar refresh would re-exec --session-id',
  'agent_name required',
  'cwd required',
]

describe('sidecar refresh tab guard', () => {
  beforeEach(() => {
    resetSidecarRefreshGuards()
  })

  it('skips SessionRemoved while a refresh of that pane group is in flight', () => {
    beginSidecarRefresh('pg-1')
    expect(sidecarRefreshMark('pg-1')).toBe('inflight')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(sidecarRefreshMark('pg-1')).toBe('inflight')
    // A different pane group is not covered by this refresh.
    expect(takeSessionRemoved('pg-2')).toBe('passthrough')
    expect(sidecarRefreshMark('pg-1')).toBe('inflight')
  })

  it('passes SessionRemoved through when no refresh is in flight', () => {
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
  })

  it('reads a tab- agent name as the pane group id and ignores other names', () => {
    expect(paneGroupIdFromTabAgent('tab-pg-1')).toBe('pg-1')
    expect(paneGroupIdFromTabAgent('tab-pg-1-extra')).toBe('pg-1-extra')
    expect(paneGroupIdFromTabAgent('sales')).toBeNull()
    expect(paneGroupIdFromTabAgent('proj-1')).toBeNull()
    expect(paneGroupIdFromTabAgent('tab-')).toBeNull()
    expect(paneGroupIdFromTabAgent('')).toBeNull()
    // The guard itself does not append a tab. A non-tab name is not a key.
    beginSidecarRefresh('pg-1')
    expect(takeSessionRemoved('sales')).toBe('passthrough')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
  })

  it('success before SessionRemoved still skips that one event, then passes through', () => {
    beginSidecarRefresh('pg-1')
    expect(settleSidecarRefresh('pg-1', { ok: true })).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('await-skip')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
  })

  it('a remove during the open refresh is skipped, and success then keeps and clears', () => {
    beginSidecarRefresh('pg-1')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(settleSidecarRefresh('pg-1', { ok: true })).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
  })

  it('pre-unregister failures clear the mark and do not drop', () => {
    for (const message of PRE_UNREGISTER) {
      beginSidecarRefresh('pg-1')
      expect(sidecarRefreshMark('pg-1')).toBe('inflight')
      expect(settleSidecarRefresh('pg-1', { ok: false, message })).toBe('keep')
      expect(sidecarRefreshMark('pg-1')).toBe('none')
      expect(takeSessionRemoved('pg-1')).toBe('passthrough')
    }
  })

  it('a 409 after the remove was already skipped still keeps and clears', () => {
    beginSidecarRefresh('pg-1')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(
      settleSidecarRefresh('pg-1', {
        ok: false,
        message: 'sidecar refresh has no resumable session',
      }),
    ).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
  })

  it('v2 spawn failed after the remove was skipped drops and clears the mark', () => {
    beginSidecarRefresh('pg-1')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(
      settleSidecarRefresh('pg-1', { ok: false, message: 'v2 spawn failed: pty: os error' }),
    ).toBe('drop')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
  })

  it('v2 spawn failed before SessionRemoved makes the next remove pass through', () => {
    beginSidecarRefresh('pg-1')
    expect(
      settleSidecarRefresh('pg-1', { ok: false, message: 'v2 spawn failed' }),
    ).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('await-drop')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
  })

  it('a network failure clears a waiting mark and does not drop', () => {
    beginSidecarRefresh('pg-1')
    expect(settleSidecarRefresh('pg-1', { ok: false, message: 'socket hang up' })).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')

    beginSidecarRefresh('pg-1')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(settleSidecarRefresh('pg-1', { ok: false, message: 'Failed to fetch' })).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
    expect(takeSessionRemoved('pg-1')).toBe('passthrough')
  })

  it('does not treat a message that merely contains the spawn failure text as a drop', () => {
    beginSidecarRefresh('pg-1')
    expect(takeSessionRemoved('pg-1')).toBe('skip')
    expect(
      settleSidecarRefresh('pg-1', { ok: false, message: 'wrapped: v2 spawn failed: nope' }),
    ).toBe('keep')
    expect(sidecarRefreshMark('pg-1')).toBe('none')
  })
})

// ── strip: the real onRemoved / onAdded handlers, no daemon ──────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => false),
}))

const strip = vi.hoisted(() => {
  type Fn = (...args: unknown[]) => void
  const sessionSubs: Array<{ path: string; handlers: Record<string, Fn> }> = []
  const layouts = new Map<string, string>()
  const daemonCliPost = vi.fn(
    async (
      route: string,
      _body?: { projectId?: string; workspaceId?: string; layoutJson?: string },
    ) => {
      if (route === 'workspace-layouts/save') return { success: true, revision: 1 }
      return {}
    },
  )
  const daemonCliGet = vi.fn(
    async (route: string, params?: { project_id?: string; workspace_id?: string }) => {
      if (route === 'workspace-layouts/load') {
        const key = `${params?.project_id}:${params?.workspace_id}`
        return layouts.get(key) ?? null
      }
      if (route === 'workspace-layouts/load-all') return []
      if (route === 'sessions/list-for-workspace') return []
      if (route === 'workspace/tab-titles') return []
      if (route === 'chat/list') return []
      return []
    },
  )
  return { sessionSubs, layouts, daemonCliPost, daemonCliGet }
})

vi.mock('@/stores/session-events', async () => {
  // Home M1: every subscription takes the server scope first; these
  // mocks fail loudly unless it is the primary scope.
  const { expectPrimaryScope } = await import('@/test-utils/scope')
  return {
    subscribeToWorkspaceSessionEvents: vi.fn(
      (scope: unknown, path: string, handlers: Record<string, (...args: unknown[]) => void>) => {
      expectPrimaryScope(scope)
        const entry = { path, handlers }
        strip.sessionSubs.push(entry)
        return () => {
          strip.sessionSubs = strip.sessionSubs.filter((e) => e !== entry)
        }
      },
    ),
    subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
    onSessionAddedApp: vi.fn(() => () => undefined),
    onSessionRemovedApp: vi.fn(() => () => undefined),
    onAppHello: vi.fn(() => () => undefined),
    onOpenUrl: vi.fn(() => () => undefined),
    onceRecovered: vi.fn(() => () => undefined),
  }
})

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: Parameters<typeof strip.daemonCliGet>) => strip.daemonCliGet(...a)),
    daemonCliGetText: primaryOnly(vi.fn(async () => '')),
    daemonCliPost: primaryOnly((...a: Parameters<typeof strip.daemonCliPost>) => strip.daemonCliPost(...a)),
    localDaemonCliPost: vi.fn(async () => ({})),
    RecoveringError: class RecoveringError extends Error {},
  }
})

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 0, token: 't', secure: false, host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:0'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:0'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))

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

const CWD = '/ws/proj'
const PG = 'pg-main'

function seedLayout(key: string): void {
  const layout = {
    version: 2,
    tabs: [
      {
        id: 'tab-main',
        title: 'sales/reviewer',
        mosaicTree: PG,
        paneGroups: {
          [PG]: {
            id: PG,
            items: [{ id: 'item-main', type: 'terminal', paneGroupId: PG }],
            activeItemIndex: 0,
          },
        },
      },
    ],
  }
  strip.layouts.set(key, JSON.stringify(layout))
}

async function flush(n = 12): Promise<void> {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0))
}

function sessionHandlers(): Record<string, (...args: unknown[]) => void> {
  const sub = strip.sessionSubs[strip.sessionSubs.length - 1]
  if (!sub) throw new Error('no workspace session subscription')
  return sub.handlers
}

function pushRemoved(agentName: string, paneGroupId: string | null): void {
  const onRemoved = sessionHandlers().onRemoved
  if (!onRemoved) throw new Error('onRemoved was not registered')
  onRemoved({
    kind: 'session_removed',
    workspace_path: CWD,
    pane_group_id: paneGroupId,
    agent_name: agentName,
  })
}

function pushAdded(paneGroupId: string): void {
  const onAdded = sessionHandlers().onAdded
  if (!onAdded) throw new Error('onAdded was not registered')
  onAdded({
    kind: 'session_added',
    workspace_path: CWD,
    pane_group_id: paneGroupId,
    agent_name: `tab-${paneGroupId}`,
    command: 'claude',
    args: [],
    session_id: 'pty-new',
    isV2: true,
  })
}

describe('sidecar refresh strip', () => {
  let projectId: string
  let workspaceId: string
  let key: string
  let keyCounter = 0

  beforeEach(() => {
    resetSidecarRefreshGuards()
    keyCounter += 1
    projectId = `p${keyCounter}`
    workspaceId = `w${keyCounter}`
    key = `${projectId}:${workspaceId}`
    strip.layouts.clear()
    strip.sessionSubs = []
    strip.daemonCliPost.mockClear()
    strip.daemonCliGet.mockClear()
  })

  async function loadWorkspace(): Promise<typeof import('@/stores/tabs')> {
    seedLayout(key)
    const mod = await import('@/stores/tabs')
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
    if (strip.sessionSubs.length === 0) throw new Error('session subscription never opened')
    const state = mod.useTabsStore.getState()
    if (state.tabs.length !== 1) {
      throw new Error(`expected 1 restored tab, got ${state.tabs.length}`)
    }
    if (![...state.tabs[0].paneGroups.keys()].includes(PG)) {
      throw new Error(`restored pane groups ${[...state.tabs[0].paneGroups.keys()].join(',')}`)
    }
    strip.daemonCliPost.mockClear()
    return mod
  }

  it('SessionRemoved during refresh does not drop, and SessionAdded does not append', async () => {
    const { useTabsStore } = await loadWorkspace()
    const before = useTabsStore.getState()
    const tabId = before.tabs[0].id
    expect(tabId).toBe('tab-main')
    expect(before.activeTabId).toBe(tabId)

    beginSidecarRefresh(PG)
    pushRemoved(`tab-${PG}`, PG)

    const afterRemove = useTabsStore.getState()
    expect(afterRemove.tabs.map((t) => t.id)).toEqual([tabId])
    expect(afterRemove.activeTabId).toBe(tabId)
    expect([...afterRemove.tabs[0].paneGroups.keys()]).toEqual([PG])
    expect(strip.daemonCliPost).not.toHaveBeenCalled()
    expect(takeSessionRemoved(PG)).toBe('skip')

    pushAdded(PG)
    const afterAdd = useTabsStore.getState()
    expect(afterAdd.tabs.map((t) => t.id)).toEqual([tabId])
    expect(afterAdd.tabs[0].title).toBe('sales/reviewer')
    expect(afterAdd.activeTabId).toBe(tabId)
    const routes = strip.daemonCliPost.mock.calls.map((call) => call[0])
    expect(routes).not.toContain('sessions/v2/close')
    expect(routes).not.toContain('workspace-layouts/save')
  })

  it('SessionRemoved with no refresh drops a single-terminal non-system tab and saves the layout', async () => {
    const { useTabsStore } = await loadWorkspace()
    useTabsStore.setState((s) => ({
      tabs: [
        ...s.tabs,
        {
          id: 'tab-other',
          title: 'Other',
          mosaicTree: 'pg-other',
          paneGroups: new Map([
            [
              'pg-other',
              {
                id: 'pg-other',
                items: [
                  {
                    id: 'item-other',
                    type: 'terminal' as const,
                    data: { terminalId: 'pg-other', cwd: CWD },
                  },
                ],
                activeItemIndex: 0,
              },
            ],
          ]),
        },
      ],
      activeTabId: 'tab-main',
    }))

    expect(sidecarRefreshMark(PG)).toBe('none')
    pushRemoved(`tab-${PG}`, PG)

    const state = useTabsStore.getState()
    expect(state.tabs.map((t) => t.id)).toEqual(['tab-other'])
    expect(state.activeTabId).toBe('tab-other')
    const routes = strip.daemonCliPost.mock.calls.map((call) => call[0])
    expect(routes).toContain('workspace-layouts/save')
    expect(routes).not.toContain('sessions/v2/close')
    expect(routes).not.toContain('workspace-layouts/delete')
    const save = strip.daemonCliPost.mock.calls.find((call) => call[0] === 'workspace-layouts/save')
    if (!save) throw new Error('layout save was not posted')
    const body = save[1] as { projectId?: string; workspaceId?: string; layoutJson?: string }
    expect(body.projectId).toBe(projectId)
    expect(body.workspaceId).toBe(workspaceId)
    const layout = JSON.parse(body.layoutJson ?? '') as { tabs: Array<{ id: string }> }
    expect(layout.tabs.map((t) => t.id)).toEqual(['tab-other'])
  })

  it('a system tab and a multi-pane tab are not drop candidates', async () => {
    const { useTabsStore } = await loadWorkspace()
    useTabsStore.setState((s) => ({
      tabs: s.tabs.map((t) => ({ ...t, isSystemAgent: true })),
    }))
    pushRemoved(`tab-${PG}`, PG)
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    expect(strip.daemonCliPost).not.toHaveBeenCalled()

    useTabsStore.setState((s) => ({
      tabs: s.tabs.map((t) => {
        const paneGroups = new Map(t.paneGroups)
        const existing = paneGroups.get(PG)
        if (!existing) throw new Error('missing pane group')
        paneGroups.set('pg-extra', { ...existing, id: 'pg-extra' })
        return { ...t, isSystemAgent: false, paneGroups }
      }),
    }))
    pushRemoved(`tab-${PG}`, PG)
    const state = useTabsStore.getState()
    expect(state.tabs).toHaveLength(1)
    expect(state.tabs[0].id).toBe('tab-main')
    expect(state.tabs[0].paneGroups.size).toBe(2)
    expect(strip.daemonCliPost).not.toHaveBeenCalled()
  })

  it('a name that does not start with tab- does not consult the guard or drop the tab', async () => {
    const { useTabsStore } = await loadWorkspace()
    beginSidecarRefresh(PG)
    expect(settleSidecarRefresh(PG, { ok: true })).toBe('keep')
    expect(sidecarRefreshMark(PG)).toBe('await-skip')

    pushRemoved('proj-1', null)
    pushRemoved('proj-1', PG)
    pushRemoved('sales/reviewer', PG)

    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    expect(sidecarRefreshMark(PG)).toBe('await-skip')
    expect(takeSessionRemoved(PG)).toBe('skip')
    expect(sidecarRefreshMark(PG)).toBe('none')
    expect(takeSessionRemoved(PG)).toBe('passthrough')
    expect(strip.daemonCliPost).not.toHaveBeenCalled()
  })

  it('spawn failure after a skipped remove drops through the same helper and does not close', async () => {
    const { useTabsStore, dropTabAfterFailedSidecarRefresh } = await loadWorkspace()
    beginSidecarRefresh(PG)
    pushRemoved(`tab-${PG}`, PG)
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    expect(settleSidecarRefresh(PG, { ok: false, message: 'v2 spawn failed: exit 1' })).toBe('drop')
    expect(sidecarRefreshMark(PG)).toBe('none')

    dropTabAfterFailedSidecarRefresh(PG)

    expect(useTabsStore.getState().tabs).toEqual([])
    expect(useTabsStore.getState().activeTabId).toBeNull()
    const routes = strip.daemonCliPost.mock.calls.map((call) => call[0])
    // V16 — the now-empty strip is saved as an empty layout (the revision
    // keeps climbing and other windows hear it), never deleted.
    expect(routes).toContain('workspace-layouts/save')
    expect(routes).not.toContain('workspace-layouts/delete')
    const save = strip.daemonCliPost.mock.calls.find((call) => call[0] === 'workspace-layouts/save')
    if (!save) throw new Error('expected a workspace-layouts/save')
    expect(JSON.parse((save[1] as { layoutJson: string }).layoutJson)).toEqual({ version: 2, tabs: [] })
    expect(routes).not.toContain('sessions/v2/close')
    expect(takeSessionRemoved(PG)).toBe('passthrough')
  })
})
