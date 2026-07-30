// P3c (D2) — generic API-spawned tab adoption + reaper close.
//
// An API-spawned sandbox cell (POST /v1/sandboxes) or host-session
// (POST /v1/w/.../host-sessions) registers daemon-side under a host-minted
// `api-<principal>-<uuid>` agent_name, so it reaches the renderer only on
// the APP-LEVEL session-events socket (`onSessionAddedApp` /
// `onSessionRemovedApp`). `adoptApiSandboxSession` surfaces it as a NEW
// terminal tab carrying `attachAgentName` (so TerminalPane ATTACHES to the
// existing cell, never re-spawns) + `sandboxBackend` (so TabBar.tsx can
// light the D9 orange marker for microvm). `dropApiSpawnedSession` closes
// that audit tab when the idle reaper (or any path) unregisters the PTY —
// which is what lets a post-reap resume open a *new* tab (de-dupe keys on
// sessionId; a leftover zombie would swallow the revived SessionAdded).
//
// This suite pins:
//   - the sandboxBackend-from-event mapping (orange marker source),
//   - the attach mechanism (attachAgentName === the event's agent_name, v2),
//   - host-session adoption (`sandbox_backend: "host"`, no jail request),
//   - the de-dupe (a second event for the same cell adds NO second tab),
//   - default-OFF parity (a non-sandbox SessionAdded is IGNORED here),
//   - reaper close drops the audit tab by attachAgentName,
//   - post-reap resume (same sessionId, fresh agent) opens a new tab,
//   - init wires both add + remove consumers.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── localStorage (real, in-memory) so the tabs module graph's persist paths run ─
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
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
const cli = vi.hoisted(() => ({
  getImpl: (async (route: string) =>
    route === 'workspace-layouts/load' ? null : []) as (route: string, params?: unknown) => Promise<unknown>,
  listSessionsImpl: (async () => [] as unknown[]) as () => Promise<unknown>,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string, params?: unknown) => {
    if (route === 'sessions/list-for-workspace') return cli.listSessionsImpl()
    return cli.getImpl(route, params)
  }),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'resolved-agent'),
  setChatSession: vi.fn(async () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
}))
// The app-level session registries this consumer rides — record handlers so
// tests can fire SessionAdded / SessionRemoved through init.
const ev = vi.hoisted(() => {
  type Fn = (...a: unknown[]) => void
  return { added: [] as Fn[], removed: [] as Fn[], hello: [] as Fn[] }
})
vi.mock('./session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.added.push(fn)
    return () => void (ev.added = ev.added.filter((f) => f !== fn))
  }),
  onSessionRemovedApp: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.removed.push(fn)
    return () => void (ev.removed = ev.removed.filter((f) => f !== fn))
  }),
  onAppHello: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.hello.push(fn)
    return () => void (ev.hello = ev.hello.filter((f) => f !== fn))
  }),
}))

import {
  useTabsStore,
  adoptApiSandboxSession,
  dropApiSpawnedSession,
  hydrateApiSandboxSessions,
  initApiSandboxTabAdoption,
  type Tab,
  type TerminalItemData,
} from './tabs'
import type { SessionAddedEvent, SessionRemovedEvent } from './session-events'

function apiSandboxEvent(over: Partial<SessionAddedEvent> = {}): SessionAddedEvent {
  return {
    kind: 'session_added',
    workspace_path: '/home/u/.k2/sandbox-sessions/abc',
    pane_group_id: null,
    agent_name: 'api-owner-11111111-2222-3333-4444-555555555555',
    command: 'claude',
    args: ['--dangerously-skip-permissions'],
    session_id: 'sess-aaaa',
    isV2: true,
    sandbox_backend: 'microvm',
    ...over,
  }
}

function apiHostEvent(over: Partial<SessionAddedEvent> = {}): SessionAddedEvent {
  return apiSandboxEvent({
    workspace_path: '/Users/u/projects/wiki-site',
    sandbox_backend: 'host',
    agent_name: 'api-owner-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
    session_id: '11111111-2222-3333-4444-555555555555',
    command: 'grok',
    args: [],
    ...over,
  })
}

function apiRemovedEvent(over: Partial<SessionRemovedEvent> = {}): SessionRemovedEvent {
  return {
    kind: 'session_removed',
    workspace_path: '/Users/u/projects/wiki-site',
    pane_group_id: null,
    agent_name: 'api-owner-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
    ...over,
  }
}

/** The single terminal item across all tabs (helper for the happy path). */
function onlyTerminalItem(): { tab: Tab; data: TerminalItemData } | null {
  for (const tab of useTabsStore.getState().tabs) {
    for (const pg of tab.paneGroups.values()) {
      for (const item of pg.items) {
        if (item.type === 'terminal') return { tab, data: item.data as TerminalItemData }
      }
    }
  }
  return null
}

function resetStore(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    activeWorkspaceKey: null,
    activeProjectId: null,
    activeWorkspaceId: null,
    workspaceLayouts: {},
    backgroundWorkspaces: {},
  })
  ev.added = []
  ev.removed = []
  ev.hello = []
  cli.listSessionsImpl = async () => []
  mem.clear()
}

beforeEach(resetStore)

describe('adoptApiSandboxSession', () => {
  it('adopts a real-sandbox cell into a new v2 terminal tab carrying the orange backend + attach name', () => {
    const e = apiSandboxEvent()
    const adopted = adoptApiSandboxSession(e)
    expect(adopted).toBe(true)

    const found = onlyTerminalItem()
    expect(found).not.toBeNull()
    const { data } = found!
    // sandboxBackend-from-event mapping — THIS is what TabBar.tsx:427 reads to
    // light the D9 orange marker.
    expect(data.sandboxBackend).toBe('microvm')
    // Attach (not re-spawn): the tab targets the existing cell's agent_name.
    expect(data.attachAgentName).toBe(e.agent_name)
    // Daemon-owned renderer + sandbox-request intent (belt-and-suspenders echo).
    expect(data.renderer).toBe('kessel')
    expect(data.sandbox).toBe(true)
    // Carries the daemon session id for close-as-minimize cross-referencing.
    expect(data.sessionId).toBe(e.session_id)
    // Exactly one tab surfaced.
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('adopts a host-session (sandbox_backend=host) without requesting a jail', () => {
    const e = apiHostEvent()
    expect(adoptApiSandboxSession(e)).toBe(true)
    const found = onlyTerminalItem()
    expect(found).not.toBeNull()
    expect(found!.data.attachAgentName).toBe(e.agent_name)
    expect(found!.data.sandboxBackend).toBe('host')
    // Host sessions are passthrough — never ask the daemon for a microvm.
    expect(found!.data.sandbox).toBeFalsy()
    expect(found!.data.sessionId).toBe(e.session_id)
    expect(found!.data.renderer).toBe('kessel')
  })

  it('de-dupes: a second event for the same cell (by agent_name) adds no second tab', () => {
    expect(adoptApiSandboxSession(apiSandboxEvent())).toBe(true)
    // Re-delivery with the SAME agent_name but a different session id — still a
    // no-op (the window already surfaced this cell).
    const second = adoptApiSandboxSession(apiSandboxEvent({ session_id: 'sess-bbbb' }))
    expect(second).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('de-dupes by session_id too (same session, re-emitted)', () => {
    expect(adoptApiSandboxSession(apiSandboxEvent())).toBe(true)
    const dup = adoptApiSandboxSession(
      apiSandboxEvent({ agent_name: 'api-owner-different-name' }),
    )
    // Different agent_name, SAME session_id → already surfaced → no-op.
    expect(dup).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('default-OFF parity: a non-sandbox SessionAdded (no sandbox_backend) is IGNORED — no tab, no orange', () => {
    const plain = apiSandboxEvent({
      agent_name: 'tab-pg-1',
      pane_group_id: 'pg-1',
      workspace_path: '/x/foo',
      sandbox_backend: undefined,
    })
    expect(adoptApiSandboxSession(plain)).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(0)
  })

  it('initApiSandboxTabAdoption wires the consumer onto the app-level registry', () => {
    const unsub = initApiSandboxTabAdoption()
    expect(ev.added).toHaveLength(1)
    expect(ev.removed).toHaveLength(1)
    // Firing a real-sandbox event through the registered handler adopts a tab.
    ev.added[0](apiSandboxEvent())
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    unsub()
    expect(ev.added).toHaveLength(0)
    expect(ev.removed).toHaveLength(0)
  })
})

describe('dropApiSpawnedSession', () => {
  it('closes the audit tab when the api agent is reaped', () => {
    const spawn = apiHostEvent()
    expect(adoptApiSandboxSession(spawn)).toBe(true)
    expect(useTabsStore.getState().tabs).toHaveLength(1)

    const dropped = dropApiSpawnedSession(
      apiRemovedEvent({ agent_name: spawn.agent_name }),
    )
    expect(dropped).toBe(true)
    expect(useTabsStore.getState().tabs).toHaveLength(0)
  })

  it('ignores non-api agent removals (workspace tab- path owns those)', () => {
    expect(adoptApiSandboxSession(apiHostEvent())).toBe(true)
    expect(
      dropApiSpawnedSession(
        apiRemovedEvent({
          agent_name: 'tab-some-pane-group',
          pane_group_id: 'some-pane-group',
        }),
      ),
    ).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('ignores removals for a different api agent', () => {
    expect(adoptApiSandboxSession(apiHostEvent())).toBe(true)
    expect(
      dropApiSpawnedSession(
        apiRemovedEvent({ agent_name: 'api-owner-other-other-other-other' }),
      ),
    ).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('post-reap resume: same sessionId + fresh agent opens a new audit tab', () => {
    // 1) Original host session surfaces.
    const original = apiHostEvent({
      agent_name: 'api-owner-old-old-old-old-old',
      session_id: 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
    })
    expect(adoptApiSandboxSession(original)).toBe(true)
    expect(useTabsStore.getState().tabs).toHaveLength(1)

    // 2) Idle reaper kills the PTY → SessionRemoved closes the zombie tab.
    expect(
      dropApiSpawnedSession(apiRemovedEvent({ agent_name: original.agent_name })),
    ).toBe(true)
    expect(useTabsStore.getState().tabs).toHaveLength(0)

    // 3) Caller wakes the same session (stored session id) → fresh agent,
    // same sessionId. Without the reaper close, sessionId de-dupe would
    // swallow this and leave the user watching nothing / a dead pane.
    const revived = apiHostEvent({
      agent_name: 'api-owner-new-new-new-new-new',
      session_id: original.session_id,
    })
    expect(adoptApiSandboxSession(revived)).toBe(true)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    const found = onlyTerminalItem()
    expect(found!.data.attachAgentName).toBe(revived.agent_name)
    expect(found!.data.sessionId).toBe(original.session_id)
  })

  it('without reaper close, same sessionId de-dupes and blocks the new tab (documents the bug)', () => {
    const original = apiHostEvent({
      agent_name: 'api-owner-old-old-old-old-old',
      session_id: 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
    })
    expect(adoptApiSandboxSession(original)).toBe(true)
    // Zombie tab still present — resume with fresh agent + same sessionId.
    const revived = apiHostEvent({
      agent_name: 'api-owner-new-new-new-new-new',
      session_id: original.session_id,
    })
    expect(adoptApiSandboxSession(revived)).toBe(false)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    // Still attached to the DEAD agent — the bug users hit post-reap.
    expect(onlyTerminalItem()!.data.attachAgentName).toBe(original.agent_name)
  })

  it('init wires SessionRemoved through to drop', () => {
    const unsub = initApiSandboxTabAdoption()
    const spawn = apiHostEvent()
    ev.added[0](spawn)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    ev.removed[0](apiRemovedEvent({ agent_name: spawn.agent_name }))
    expect(useTabsStore.getState().tabs).toHaveLength(0)
    unsub()
  })

  it('hydrateApiSandboxSessions adopts live api- rows from list-for-workspace', async () => {
    cli.listSessionsImpl = async () => [
      {
        sessionId: 'live-sess-1',
        agentName: 'api-owner-live-live-live-live',
        command: 'claude',
        args: ['--session-id', 'live-sess-1'],
        cwd: '/home/k2/ai/sales',
        isV2: true,
      },
      {
        // non-api: ignore
        sessionId: 'tab-sess',
        agentName: 'tab-abc',
        command: 'bash',
        args: [],
        cwd: '/home/k2/ai/sales',
        isV2: true,
      },
    ]
    const n = await hydrateApiSandboxSessions()
    expect(n).toBe(1)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    const t = onlyTerminalItem()!
    expect(t.data.attachAgentName).toBe('api-owner-live-live-live-live')
    expect(t.data.sessionId).toBe('live-sess-1')
    expect(t.data.sandboxBackend).toBe('host')
    // de-dupe on second hydrate
    expect(await hydrateApiSandboxSessions()).toBe(0)
    expect(useTabsStore.getState().tabs).toHaveLength(1)
  })

  it('init hello triggers hydrate', async () => {
    cli.listSessionsImpl = async () => [
      {
        sessionId: 'hello-sess',
        agentName: 'api-owner-hello-hello-hello-hello',
        command: null,
        args: [],
        cwd: '/tmp/ws',
        isV2: true,
      },
    ]
    const unsub = initApiSandboxTabAdoption()
    // immediate hydrate on init
    await vi.waitFor(() => {
      expect(useTabsStore.getState().tabs.length).toBeGreaterThanOrEqual(1)
    })
    // hello re-runs hydrate (de-dupe → still 1)
    await Promise.resolve(ev.hello[0]?.())
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    unsub()
  })
})
