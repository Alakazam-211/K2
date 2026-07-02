// Regression tests for the split-into-columns lifecycle (the TabBar
// "Split into columns" button → `splitTerminalArea`).
//
// The reported bug: clicking split showed the second column for an
// instant, then it vanished and the new terminal "merged back" into the
// main column. Three cooperating causes, all in tabs.ts:
//
//   1. SELF-ECHO CLOBBER — the daemon emits `TabOrderChanged` BEFORE it
//      writes the `workspace-layouts/save` response, so our OWN in-flight
//      save's broadcast could reach `onTabOrderChanged` while
//      `layoutRevisions` still held the pre-save base. The handler
//      misread the echo as a REMOTE write and refetched+applied the
//      canonical layout — which was serialized BEFORE the split, so the
//      rebuild wiped `extraGroups` (column gone). The split terminal's
//      `session_added` then found its pane group unsurfaced and adopted
//      it into GROUP 0 (the "merge back"). Fixed by settling in-flight
//      saves before judging a broadcast's revision.
//
//   2. RESTORE RE-MINT — `restoreLayout` reused saved paneGroup ids for
//      group 0 ("Reuse the saved ID") but re-minted fresh UUIDs for
//      `extraGroups`, so any rebuild/restart orphaned every split-column
//      PTY (fresh id → duplicate spawn; orphan later adopted into the
//      main group). Fixed by reusing saved ids in extraGroups too.
//
//   3. RECONCILE BLIND SPOT — `reconcileWithDaemon`'s surfaced-set only
//      scanned `state.tabs`, so a live split-column terminal was treated
//      as an orphan on workspace re-entry and adopted into the main
//      group as a duplicate tab. Fixed by scanning extraGroups too
//      (parity with `isPaneGroupSurfaced`).
//
// The suite drives the REAL store through mocked daemon boundaries in
// the exact event order the daemon produces (broadcast before save
// response — the worst-case but realistic ordering).

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'k2so_sessions_list_for_workspace') {
      return JSON.stringify(daemon.sessions)
    }
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: vi.fn(() => true),
}))

// Captured workspace-scoped subscriptions so a test can play the daemon.
const ev = vi.hoisted(() => {
  type Fn = (...a: unknown[]) => void
  return {
    sessionSubs: [] as Array<{ path: string; handlers: Record<string, Fn> }>,
    tabSubs: [] as Array<{ path: string; handlers: Record<string, Fn> }>,
  }
})
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn((path: string, handlers: Record<string, (...a: unknown[]) => void>) => {
    const entry = { path, handlers }
    ev.sessionSubs.push(entry)
    return () => void (ev.sessionSubs = ev.sessionSubs.filter((e) => e !== entry))
  }),
  subscribeToWorkspaceTabEvents: vi.fn((path: string, handlers: Record<string, (...a: unknown[]) => void>) => {
    const entry = { path, handlers }
    ev.tabSubs.push(entry)
    return () => void (ev.tabSubs = ev.tabSubs.filter((e) => e !== entry))
  }),
  onSessionAddedApp: vi.fn(() => () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 0, token: 't', secure: false, host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:0'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:0'),
  invalidateDaemonWs: vi.fn(),
  prewarmDaemonWs: vi.fn(),
}))

// In-memory daemon: layout store with monotonic revision stamping
// (mirrors db_routes.rs::handle_layout_save) + a live-session list for
// the reconcile pass. `saveBroadcast` fires synchronously INSIDE the
// save POST — i.e. before the response promise resolves — reproducing
// the daemon's emit-before-respond ordering.
const daemon = vi.hoisted(() => ({
  layouts: new Map<string, { json: string; revision: number }>(),
  revisionCounter: 0,
  sessions: [] as Array<{
    sessionId: string
    agentName: string
    command: string | null
    args: string[]
    cwd: string
    isV2: boolean
  }>,
  saveBroadcast: null as null | ((revision: number) => void),
  deferSaveResponse: false,
  pendingSaveResolvers: [] as Array<() => void>,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string, params?: { project_id?: string; workspace_id?: string }) => {
    if (route === 'workspace-layouts/load') {
      const key = `${params?.project_id}:${params?.workspace_id}`
      return daemon.layouts.get(key)?.json ?? null
    }
    if (route === 'workspace/tab-titles') return []
    return []
  }),
  daemonCliPost: vi.fn(async (route: string, body?: { projectId?: string; workspaceId?: string; layoutJson?: string }) => {
    if (route === 'workspace-layouts/save') {
      const key = `${body?.projectId}:${body?.workspaceId}`
      daemon.revisionCounter += 1
      const revision = daemon.revisionCounter
      daemon.layouts.set(key, { json: body?.layoutJson ?? '', revision })
      // Broadcast fires daemon-side BEFORE the HTTP response is written —
      // but never before the renderer's synchronous post-POST code (a WS
      // frame can't outrun the statement after fetch()). queueMicrotask
      // models that: after the caller's synchronous frame, before (or
      // racing) the response promise chain.
      queueMicrotask(() => daemon.saveBroadcast?.(revision))
      if (daemon.deferSaveResponse) {
        await new Promise<void>((r) => daemon.pendingSaveResolvers.push(r))
      }
      return { success: true, revision }
    }
    return {}
  }),
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

/** Seed the daemon layout store with a plain 1-tab v2 layout (the shape
 *  serializeCurrentLayout writes), optionally with a split column. */
function seedLayout(key: string, opts?: { extraPgId?: string }): void {
  const pg = 'pg-main'
  const layout: Record<string, unknown> = {
    version: 2,
    tabs: [
      {
        id: 'tab-main',
        title: 'Terminal 1',
        mosaicTree: pg,
        paneGroups: {
          [pg]: {
            id: pg,
            items: [{ id: 'item-main', type: 'terminal', paneGroupId: pg }],
            activeItemIndex: 0,
          },
        },
      },
    ],
  }
  if (opts?.extraPgId) {
    layout.extraGroups = [
      {
        tabs: [
          {
            id: 'tab-split',
            title: 'Terminal 2',
            mosaicTree: opts.extraPgId,
            paneGroups: {
              [opts.extraPgId]: {
                id: opts.extraPgId,
                items: [{ id: 'item-split', type: 'terminal', paneGroupId: opts.extraPgId }],
                activeItemIndex: 0,
              },
            },
          },
        ],
      },
    ]
    layout.splitCount = 2
  }
  daemon.layouts.set(key, { json: JSON.stringify(layout), revision: daemon.revisionCounter })
}

async function flush(n = 10): Promise<void> {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0))
}

/** Deliver the daemon's session_added push for a `tab-<pgId>` session. */
function pushSessionAdded(pgId: string, sessionId: string): void {
  for (const sub of ev.sessionSubs) {
    sub.handlers.onAdded?.({
      kind: 'session_added',
      workspace_path: CWD,
      pane_group_id: pgId,
      agent_name: `tab-${pgId}`,
      command: null,
      args: [],
      session_id: sessionId,
      is_v2: true,
    })
  }
}

/** All pane-group ids surfaced in group 0 (main column). */
function group0PgIds(state: { tabs: Array<{ paneGroups: Map<string, unknown> }> }): string[] {
  return state.tabs.flatMap((t) => [...t.paneGroups.keys()])
}

let keyCounter = 0

describe('split-into-columns lifecycle', () => {
  // Fresh workspace key per test: `layoutRevisions` and the in-flight
  // save tracker are module-level and keyed by workspace.
  let projectId: string
  let workspaceId: string
  let key: string

  beforeEach(() => {
    keyCounter += 1
    projectId = `p${keyCounter}`
    workspaceId = `w${keyCounter}`
    key = `${projectId}:${workspaceId}`
    daemon.layouts.clear()
    daemon.revisionCounter = 0
    daemon.sessions = []
    daemon.saveBroadcast = null
    daemon.deferSaveResponse = false
    daemon.pendingSaveResolvers = []
    ev.sessionSubs = []
    ev.tabSubs = []
  })

  async function loadWorkspace(): Promise<typeof import('./tabs')> {
    const mod = await import('./tabs')
    mod.useTabsStore.setState({
      tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0,
      backgroundWorkspaces: {}, workspaceLayouts: {}, activeWorkspaceKey: null,
      activeProjectId: null, activeWorkspaceId: null,
    })
    await mod.useTabsStore.getState().loadLayoutForWorkspace(projectId, workspaceId, CWD)
    await flush()
    return mod
  }

  it('split survives its own save broadcast racing the save response (the reported collapse)', async () => {
    seedLayout(key)
    const { useTabsStore } = await loadWorkspace()

    // A PTY-title flap fired the debounced autosave just before the
    // click: the PRE-SPLIT layout's save POST is IN FLIGHT (response
    // deferred) when the user clicks the split button.
    daemon.saveBroadcast = (revision) => {
      for (const sub of ev.tabSubs) {
        sub.handlers.onTabOrderChanged?.({ project: projectId, workspace: workspaceId, revision })
      }
    }
    daemon.deferSaveResponse = true
    useTabsStore.getState().saveLayoutForWorkspace(projectId, workspaceId)
    await flush(2)

    // Click "Split into columns".
    useTabsStore.getState().splitTerminalArea(CWD)
    const afterClick = useTabsStore.getState()
    expect(afterClick.splitCount).toBe(2)
    const splitPgId = [...afterClick.extraGroups[0].tabs[0].paneGroups.keys()][0]
    const splitTabId = afterClick.extraGroups[0].tabs[0].id
    const mainTabId = afterClick.tabs[0].id

    // The echo broadcast was already delivered synchronously inside the
    // save POST (daemon emits before responding). Now the deferred save
    // response resolves.
    await flush()
    daemon.pendingSaveResolvers.forEach((r) => r())
    daemon.pendingSaveResolvers = []
    daemon.deferSaveResponse = false
    await flush()

    // The split terminal's spawn registered on the daemon → push.
    pushSessionAdded(splitPgId, 'sess-split')
    await flush()

    // Let the debounced post-split autosave (and its own echo) run too.
    await new Promise((r) => setTimeout(r, 1100))
    await flush(16)

    const final = useTabsStore.getState()
    // The column survives, with the SAME pane group (no re-mint/remount).
    expect(final.splitCount).toBe(2)
    expect(final.extraGroups).toHaveLength(1)
    expect(final.extraGroups[0].tabs).toHaveLength(1)
    expect([...final.extraGroups[0].tabs[0].paneGroups.keys()]).toEqual([splitPgId])
    // The split terminal was NOT adopted into the main column.
    expect(group0PgIds(final)).not.toContain(splitPgId)
    expect(final.tabs).toHaveLength(1)
    // The self-echo must not have triggered ANY adoption rebuild: a
    // rebuild re-mints tab ids (remounting every live terminal), so the
    // live Tab objects must be the very ones the click created.
    expect(final.tabs[0].id).toBe(mainTabId)
    expect(final.extraGroups[0].tabs[0].id).toBe(splitTabId)
  }, 20000)

  it('session_added for the split terminal is deduped against extraGroups', async () => {
    seedLayout(key)
    const { useTabsStore } = await loadWorkspace()

    useTabsStore.getState().splitTerminalArea(CWD)
    const splitPgId = [...useTabsStore.getState().extraGroups[0].tabs[0].paneGroups.keys()][0]

    pushSessionAdded(splitPgId, 'sess-split')
    await flush()

    const final = useTabsStore.getState()
    expect(final.tabs).toHaveLength(1) // no group-0 duplicate
    expect(final.extraGroups[0].tabs).toHaveLength(1)
  })

  it('restoreLayout reuses saved extraGroups pane-group ids (split PTYs re-attach)', async () => {
    seedLayout(key, { extraPgId: 'pg-split' })
    const { useTabsStore } = await loadWorkspace()

    const state = useTabsStore.getState()
    expect(state.splitCount).toBe(2)
    // Group 0 already reused 'pg-main'; the split column must reuse
    // 'pg-split' the same way — a re-minted UUID would spawn a duplicate
    // PTY and orphan the daemon's `tab-pg-split` session.
    expect(group0PgIds(state)).toEqual(['pg-main'])
    expect([...state.extraGroups[0].tabs[0].paneGroups.keys()]).toEqual(['pg-split'])
    expect(state.extraGroups[0].tabs[0].mosaicTree).toBe('pg-split')
  })

  // 2026-07-02 PTY-leak regression — two clients on one daemon exchanged
  // TabOrderChanged broadcasts all morning; each refetch+rebuild re-minted
  // the split column's pane-group id (pre-fix `crypto.randomUUID()` in
  // restoreLayout's extraGroups branch), and every fresh id became a
  // `tab-<uuid>` bare-shell spawn nothing ever attached to — one leaked
  // login/zsh per cycle until the box exhausted kern.tty.ptmx_max (511).
  // A split layout always takes the FULL rebuild in
  // refetchLayoutForRemoteReorder (tryReorderTabsInPlace defers when
  // extraGroups exist), so this drives the exact loop: N remote revisions,
  // N rebuilds — and pins that the terminal identity set NEVER changes.
  // Pre-b339c70 the split id re-mints on the first cycle and this fails.
  it('repeated remote TabOrderChanged rebuilds never mint fresh terminal ids (PTY-leak regression)', async () => {
    seedLayout(key, { extraPgId: 'pg-split' })
    const { useTabsStore } = await loadWorkspace()

    const allPgIds = (s: ReturnType<typeof useTabsStore.getState>): string[] =>
      [
        ...group0PgIds(s),
        ...s.extraGroups.flatMap((g) => g.tabs.flatMap((t) => [...t.paneGroups.keys()])),
      ].sort()
    expect(allPgIds(useTabsStore.getState())).toEqual(['pg-main', 'pg-split'])

    // The other client saves ahead of our base revision, N times. Same
    // structure each time (its ids are stable too once fixed) — but the
    // handler can't know that until it refetches and rebuilds.
    for (let cycle = 0; cycle < 5; cycle++) {
      daemon.revisionCounter += 1
      const revision = daemon.revisionCounter
      const current = daemon.layouts.get(key)!
      daemon.layouts.set(key, { json: current.json, revision })
      for (const sub of ev.tabSubs) {
        sub.handlers.onTabOrderChanged?.({ project: projectId, workspace: workspaceId, revision })
      }
      await flush(16)
    }
    // Let any debounced autosave (and its echo) settle too — a mint loop
    // sustains itself through exactly that save/broadcast round-trip.
    await new Promise((r) => setTimeout(r, 1100))
    await flush(16)

    const final = useTabsStore.getState()
    // The terminal identity set is EXACTLY the seeded pair: nothing
    // re-minted (no fresh `tab-<uuid>` spawn possible), nothing adopted
    // as a duplicate, nothing dropped.
    expect(allPgIds(final)).toEqual(['pg-main', 'pg-split'])
    expect(final.tabs).toHaveLength(1)
    expect(final.extraGroups).toHaveLength(1)
    expect(final.extraGroups[0].tabs).toHaveLength(1)
    expect(final.splitCount).toBe(2)
  }, 20000)

  it('reconcile does not re-adopt a live split-column terminal into the main column', async () => {
    seedLayout(key, { extraPgId: 'pg-split' })
    // The daemon still holds live PTYs for BOTH pane groups (the normal
    // switch-away-and-back case).
    daemon.sessions = [
      { sessionId: 's-main', agentName: 'tab-pg-main', command: null, args: [], cwd: CWD, isV2: true },
      { sessionId: 's-split', agentName: 'tab-pg-split', command: null, args: [], cwd: CWD, isV2: true },
    ]
    const { useTabsStore } = await loadWorkspace()

    const state = useTabsStore.getState()
    // The split terminal is surfaced in its column — reconcile must not
    // duplicate it as an "orphan" tab in group 0.
    expect(state.tabs).toHaveLength(1)
    expect(group0PgIds(state)).toEqual(['pg-main'])
    expect(state.extraGroups[0].tabs).toHaveLength(1)
    expect(state.splitCount).toBe(2)
  })
})
