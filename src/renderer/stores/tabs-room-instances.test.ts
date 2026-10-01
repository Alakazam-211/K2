// Home M3 — one tabs store per room (prd-home-multi-server-client MS3, MS14,
// MS15, MS42; MS52 a/g).
//
// Two pinned rooms, A and B, on two servers that hold the SAME workspace:
// same checkout path, same project id, same workspace id, same saved layout.
// That is the case where a path or a bare `projectId:workspaceId` key would
// cross servers. Each room's store captures its own scope at creation, so:
//
//   - a tab change in B saves to B only; A's stored layout is byte-identical
//     and A's revision does not move (MS52 a);
//   - the two instances never share revisions, acked layouts or lanes
//     (MS52 g);
//   - a tab close in B posts `v2/close` to B with `reason:"tab_close"`;
//   - B's workspace socket is opened on B's scope, and a session B adopts
//     lands in B and is saved to B;
//   - a pinned room refuses primary-only paths (stash, load-all, another
//     workspace) loudly, and `dispose()` flushes its save and closes its
//     sockets.
//
// The request layer is mocked per scope: every daemon call records which
// server it went to, and a call for a server the test did not set up throws.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { fakeScope } from '@/test-utils/fake-scope'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    throw new Error(`unexpected local Tauri command in a room test: ${cmd}`)
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

type Fn = (...a: unknown[]) => void
type ScopeLike = { id: string; hostKey: string }

const net = vi.hoisted(() => ({
  /** Fake daemons by scope id. */
  daemons: new Map<
    string,
    {
      layouts: Map<string, { json: string; revision: number }>
      revisionCounter: number
      savePosts: Array<{ key: string; layoutJson: string; baseRevision?: number }>
      gets: string[]
      posts: string[]
    }
  >(),
  sessionSubs: [] as Array<{ scopeId: string; path: string; handlers: Record<string, Fn> }>,
  tabSubs: [] as Array<{ scopeId: string; path: string; handlers: Record<string, Fn> }>,
  fetches: [] as Array<{ url: string; body: Record<string, unknown> }>,
}))

function daemonFor(scope: ScopeLike) {
  const d = net.daemons.get(scope.id)
  if (!d) throw new Error(`request for a server this test did not set up: ${scope.id}`)
  return d
}

vi.mock('@/lib/daemon-cli', () => ({
  RecoveringError: class RecoveringError extends Error {},
  daemonCliGet: vi.fn(async (scope: ScopeLike, route: string, params?: Record<string, string>) => {
    const d = daemonFor(scope)
    d.gets.push(route)
    if (route === 'workspace-layouts/load') {
      const row = d.layouts.get(`${params?.project_id}:${params?.workspace_id}`)
      if (params?.with_revision === '1') return { layoutJson: row?.json ?? null, revision: row?.revision ?? 0 }
      return row?.json ?? null
    }
    if (route === 'workspace-layouts/load-all') return []
    return []
  }),
  daemonCliGetText: vi.fn(async () => ''),
  daemonCliPost: vi.fn(async (scope: ScopeLike, route: string, body?: Record<string, unknown>) => {
    const d = daemonFor(scope)
    d.posts.push(route)
    if (route === 'workspace-layouts/save') {
      const key = `${body?.projectId}:${body?.workspaceId}`
      const baseRevision = body?.baseRevision as number | undefined
      d.savePosts.push({ key, layoutJson: String(body?.layoutJson), baseRevision })
      const stored = d.layouts.get(key)
      if (baseRevision !== undefined && stored && baseRevision !== stored.revision) {
        throw new Error('layout_revision_conflict')
      }
      d.revisionCounter += 1
      d.layouts.set(key, { json: String(body?.layoutJson), revision: d.revisionCounter })
      return { success: true, revision: d.revisionCounter }
    }
    return {}
  }),
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async (scope: ScopeLike) => {
    daemonFor(scope)
    return { port: scope.hostKey === 'b.test' ? 2222 : 1111, token: `tok-${scope.hostKey}`, secure: false, host: '127.0.0.1' }
  }),
  daemonHttpBase: vi.fn((c: { port: number }) => `http://127.0.0.1:${c.port}`),
  daemonWsBase: vi.fn((c: { port: number }) => `ws://127.0.0.1:${c.port}`),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
  resolveWindowHostCreds: vi.fn(async () => ({ port: 9, token: 'primary', secure: false, host: '127.0.0.1' })),
  getLocalDaemonWs: vi.fn(async () => ({ port: 9, token: 'primary', secure: false, host: '127.0.0.1' })),
}))

vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn((scope: ScopeLike, path: string, handlers: Record<string, Fn>) => {
    const entry = { scopeId: scope.id, path, handlers }
    net.sessionSubs.push(entry)
    return () => void (net.sessionSubs = net.sessionSubs.filter((e) => e !== entry))
  }),
  subscribeToWorkspaceTabEvents: vi.fn((scope: ScopeLike, path: string, handlers: Record<string, Fn>) => {
    const entry = { scopeId: scope.id, path, handlers }
    net.tabSubs.push(entry)
    return () => void (net.tabSubs = net.tabSubs.filter((e) => e !== entry))
  }),
  onSessionAddedApp: vi.fn(() => () => undefined),
  onSessionRemovedApp: vi.fn(() => () => undefined),
  onAppHello: vi.fn(() => () => undefined),
  onOpenUrl: vi.fn(() => () => undefined),
  onceRecovered: vi.fn(() => () => undefined),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
  terminalKill: vi.fn(async () => {
    throw new Error('legacy terminal/kill reached in a Kessel room test')
  }),
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
vi.stubGlobal(
  'fetch',
  vi.fn(async (url: string, init?: { body?: string }) => {
    net.fetches.push({ url, body: JSON.parse(init?.body ?? '{}') as Record<string, unknown> })
    return { ok: true, status: 200, text: async () => '' }
  }),
)

const PATH = '/work/k2'
const WS = { projectId: 'p1', workspaceId: 'w1', path: PATH }
const KEY = 'p1:w1'

function seedLayout(scopeId: string): string {
  const json = JSON.stringify({
    version: 2,
    tabs: [
      {
        id: 'tab-main',
        title: 'Terminal 1',
        mosaicTree: 'pg-main',
        paneGroups: {
          'pg-main': { id: 'pg-main', items: [{ id: 'item-main', type: 'terminal', paneGroupId: 'pg-main' }], activeItemIndex: 0 },
        },
        locked: false,
      },
    ],
  })
  const d = net.daemons.get(scopeId)
  if (!d) throw new Error(`no daemon ${scopeId}`)
  d.revisionCounter = 7
  d.layouts.set(KEY, { json, revision: 7 })
  return json
}

function newDaemon(id: string): void {
  net.daemons.set(id, { layouts: new Map(), revisionCounter: 0, savePosts: [], gets: [], posts: [] })
}

async function flush(n = 12): Promise<void> {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0))
}

const A = fakeScope('a.test')
const B = fakeScope('b.test')

describe('one tabs store per room (two rooms, same path and project id)', () => {
  let activated: string[]

  beforeEach(() => {
    net.daemons.clear()
    // The primary room is created at import and loads from the window's
    // server: give it a daemon so its load-all does not throw.
    newDaemon('primary')
    newDaemon(A.id)
    newDaemon(B.id)
    net.sessionSubs = []
    net.tabSubs = []
    net.fetches = []
    activated = []
  })

  async function openRooms() {
    const { createTabsStore } = await import('./tabs')
    const deps = (scopeId: string) => ({
      projectsPathIndex: () => [{ id: 'p1', path: PATH, primaryWorkspaceId: 'w1' }],
      activeProjectId: () => 'p1',
      activateProject: (projectId: string) => void activated.push(`${scopeId}:${projectId}`),
      projectDefaultAgent: () => undefined,
      presets: () => null,
      heartbeatEntries: () => [],
    })
    const roomA = createTabsStore({ scope: A, workspace: WS, deps: deps(A.id), localCommands: false })
    const roomB = createTabsStore({ scope: B, workspace: WS, deps: deps(B.id), localCommands: false })
    await roomA.room.open()
    await roomB.room.open()
    await flush()
    return { roomA, roomB }
  }

  it('a tab change in B saves to B only; A\'s layout is byte-identical (MS52 a)', async () => {
    const aJson = seedLayout(A.id)
    seedLayout(B.id)
    const { roomA, roomB } = await openRooms()
    expect(roomA.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    expect(roomB.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])

    roomB.getState().addTab(PATH)
    // The per-instance autosave fires after its 1s debounce.
    await new Promise((r) => setTimeout(r, 1100))
    await flush()

    const a = net.daemons.get(A.id)!
    const b = net.daemons.get(B.id)!
    expect(b.savePosts.length).toBe(1)
    expect(b.savePosts[0].baseRevision).toBe(7)
    expect(b.layouts.get(KEY)!.revision).toBe(8)
    expect((JSON.parse(b.layouts.get(KEY)!.json) as { tabs: unknown[] }).tabs.length).toBe(2)

    expect(a.savePosts).toEqual([])
    expect(a.layouts.get(KEY)!.json).toBe(aJson)
    expect(a.layouts.get(KEY)!.revision).toBe(7)
    expect(roomA.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
    // The primary room was not touched by either.
    expect(net.daemons.get('primary')!.savePosts).toEqual([])
  }, 10000)

  it('two instances never share revisions, acked layouts or lanes (MS52 g)', async () => {
    seedLayout(A.id)
    seedLayout(B.id)
    const { roomA, roomB } = await openRooms()
    expect(roomA.room.__test.layoutRevisions).not.toBe(roomB.room.__test.layoutRevisions)
    expect(roomA.room.__test.ackedLayouts).not.toBe(roomB.room.__test.ackedLayouts)
    expect(roomA.room.__test.layoutRevisions.get(KEY)).toBe(7)
    expect(roomB.room.__test.layoutRevisions.get(KEY)).toBe(7)

    roomB.getState().addTab(PATH)
    roomB.getState().flushLayoutPersist()
    await flush()
    expect(roomB.room.__test.layoutRevisions.get(KEY)).toBe(8)
    expect(roomA.room.__test.layoutRevisions.get(KEY)).toBe(7)
    const ackedA = roomA.room.__test.ackedLayouts.get(KEY)
    const ackedB = roomB.room.__test.ackedLayouts.get(KEY)
    if (!ackedA || !ackedB) throw new Error('both rooms must have acked layouts')
    expect(ackedA.layout.tabs.length).toBe(1)
    expect(ackedB.layout.tabs.length).toBe(2)
  })

  it('a tab close in B posts v2/close to B with reason tab_close, never to A', async () => {
    seedLayout(A.id)
    seedLayout(B.id)
    const { roomB } = await openRooms()
    const pg = roomB.getState().addTab(PATH)
    const tab = roomB.getState().tabs.find((t) => t.paneGroups.has(pg))
    if (!tab) throw new Error('added tab not found')
    // Kessel renderer so the close goes through v2/close.
    roomB.setState((s) => ({
      tabs: s.tabs.map((t) =>
        t.id !== tab.id
          ? t
          : {
              ...t,
              paneGroups: new Map(
                [...t.paneGroups].map(([id, g]) => [
                  id,
                  { ...g, items: g.items.map((i) => ({ ...i, data: { ...i.data, renderer: 'kessel' as const } })) },
                ]),
              ),
            },
      ),
    }))
    roomB.getState().removeTab(tab.id)
    await flush()

    expect(net.fetches.length).toBe(1)
    expect(net.fetches[0].url.startsWith('http://127.0.0.1:2222/cli/sessions/v2/close')).toBe(true)
    expect(net.fetches[0].body).toEqual({ agent_name: `tab-${pg}`, force: true, reason: 'tab_close' })
  })

  it('B\'s workspace sockets are on B\'s scope, and a session B adopts lands in B and saves to B', async () => {
    seedLayout(A.id)
    seedLayout(B.id)
    const { roomA, roomB } = await openRooms()
    expect(net.sessionSubs.map((s) => s.scopeId).sort()).toEqual([A.id, B.id])
    expect(net.tabSubs.map((s) => s.scopeId).sort()).toEqual([A.id, B.id])
    expect(activated.sort()).toEqual([`${A.id}:p1`, `${B.id}:p1`])

    const bSub = net.sessionSubs.find((s) => s.scopeId === B.id)
    if (!bSub) throw new Error('no session subscription on B')
    bSub.handlers.onAdded({
      kind: 'session_added',
      workspace_path: PATH,
      pane_group_id: 'pg-from-b',
      agent_name: 'tab-pg-from-b',
      command: null,
      args: [],
      session_id: 'sess-b',
      is_v2: true,
    })
    await flush()

    expect(roomB.getState().tabs.some((t) => t.paneGroups.has('pg-from-b'))).toBe(true)
    expect(roomA.getState().tabs.some((t) => t.paneGroups.has('pg-from-b'))).toBe(false)
    expect(net.daemons.get(B.id)!.savePosts.length).toBe(1)
    expect(net.daemons.get(A.id)!.savePosts).toEqual([])
  })

  it('a pinned room refuses primary-only paths and another workspace, loudly', async () => {
    seedLayout(B.id)
    const { createTabsStore } = await import('./tabs')
    const roomB = createTabsStore({
      scope: B,
      workspace: WS,
      deps: {
        projectsPathIndex: () => [],
        activeProjectId: () => 'p1',
        activateProject: () => {},
        projectDefaultAgent: () => undefined,
        presets: () => null,
        heartbeatEntries: () => [],
      },
      localCommands: false,
    })
    expect(() => roomB.getState().stashWorkspace(KEY)).toThrow(/primary-room only/)
    await expect(roomB.getState().loadWorkspaceSessionsFromDb()).rejects.toThrow(/primary-room only/)
    await expect(roomB.getState().loadLayoutForWorkspace('p2', 'w2', '/elsewhere')).rejects.toThrow(
      /cannot load p2:w2/,
    )
    expect(() => roomB.room.resetForHostSwitch()).toThrow(/primary-room only/)
    // Nothing reached B's daemon from the refused calls.
    expect(net.daemons.get(B.id)!.gets).toEqual([])
  })

  it('dispose flushes the based save to its own server and closes its sockets', async () => {
    seedLayout(A.id)
    seedLayout(B.id)
    const { roomA, roomB } = await openRooms()
    roomB.getState().addTab(PATH)
    await roomB.room.dispose()
    await flush()

    expect(net.daemons.get(B.id)!.savePosts.length).toBe(1)
    expect(net.sessionSubs.map((s) => s.scopeId)).toEqual([A.id])
    expect(net.tabSubs.map((s) => s.scopeId)).toEqual([A.id])

    // No autosave after dispose.
    roomB.getState().addTab(PATH)
    await new Promise((r) => setTimeout(r, 1100))
    await flush()
    expect(net.daemons.get(B.id)!.savePosts.length).toBe(1)
    expect(net.daemons.get(A.id)!.savePosts).toEqual([])
    expect(roomA.getState().tabs.map((t) => t.id)).toEqual(['tab-main'])
  }, 10000)

  it('the primary room is its own instance on the primary scope', async () => {
    const { useTabsStore, createTabsStore } = await import('./tabs')
    const { primaryScope } = await import('@/kessel/server-scope')
    expect(useTabsStore.room.kind).toBe('primary')
    expect(useTabsStore.room.scope).toBe(primaryScope())
    expect(useTabsStore.room.localCommands).toBe(true)
    const roomB = createTabsStore({
      scope: B,
      workspace: WS,
      deps: {
        projectsPathIndex: () => [],
        activeProjectId: () => null,
        activateProject: () => {},
        projectDefaultAgent: () => undefined,
        presets: () => null,
        heartbeatEntries: () => [],
      },
      localCommands: false,
    })
    expect(roomB).not.toBe(useTabsStore)
    expect(roomB.room.kind).toBe('pinned')
    expect(roomB.room.__test.layoutRevisions).not.toBe(useTabsStore.room.__test.layoutRevisions)
  })
})
