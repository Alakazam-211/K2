// P3c (D2) — generic API-spawned-sandbox tab adoption.
//
// An API-spawned sandbox cell (POST /v1/sandboxes) registers daemon-side under
// a host-minted `api-<principal>-<uuid>` agent_name with an EPHEMERAL cwd, so it
// reaches the renderer only on the APP-LEVEL session-events socket (the
// `onSessionAddedApp` registry). `adoptApiSandboxSession` surfaces it as a NEW
// terminal tab carrying `attachAgentName` (so TerminalPane ATTACHES to the
// existing cell, never re-spawns) + `sandboxBackend` (so TabBar.tsx lights the
// D9 orange marker immediately). This suite pins:
//   - the sandboxBackend-from-event mapping (orange marker source),
//   - the attach mechanism (attachAgentName === the event's agent_name, v2),
//   - the de-dupe (a second event for the same cell adds NO second tab),
//   - default-OFF parity (a non-sandbox SessionAdded is IGNORED here).
//
// vitest env is `node` (no Tauri). We mock the daemon/Tauri boundaries the tabs
// module graph touches so importing it is inert, then call the pure adoption fn
// against the real in-memory tabs store.

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
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string, params?: unknown) => cli.getImpl(route, params)),
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
// The app-level session-added registry this consumer rides — record the
// handler so a test can fire a SessionAdded through `initApiSandboxTabAdoption`.
const ev = vi.hoisted(() => {
  type Fn = (...a: unknown[]) => void
  return { added: [] as Fn[] }
})
vi.mock('./session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.added.push(fn)
    return () => void (ev.added = ev.added.filter((f) => f !== fn))
  }),
}))

import {
  useTabsStore,
  adoptApiSandboxSession,
  initApiSandboxTabAdoption,
  type Tab,
  type TerminalItemData,
} from './tabs'
import type { SessionAddedEvent } from './session-events'

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
    expect(data.renderer).toBe('alacritty-v2')
    expect(data.sandbox).toBe(true)
    // Carries the daemon session id for close-as-minimize cross-referencing.
    expect(data.sessionId).toBe(e.session_id)
    // Exactly one tab surfaced.
    expect(useTabsStore.getState().tabs).toHaveLength(1)
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
    // Firing a real-sandbox event through the registered handler adopts a tab.
    ev.added[0](apiSandboxEvent())
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    unsub()
    expect(ev.added).toHaveLength(0)
  })
})
