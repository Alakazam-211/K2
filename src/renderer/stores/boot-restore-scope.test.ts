// OPEN-1 — boot-restore scope.
//
// Locks the renderer's boot invariant: `loadWorkspaceSessionsFromDb`
// caches saved layouts but restores NONE into live tabs /
// `backgroundWorkspaces`, and `fetchProjects` mounts exactly ONE
// workspace. So the renderer never SPAWNS non-active PTYs at boot.
//
// #672 — the boot-time daemon sweep half of this suite was REMOVED: the
// renderer no longer reaps. The daemon now owns reaping (incl. its own
// boot reconciliation that reaps aged-out survivors), keyed on the
// canonical Active set. See .k2so/prds/daemon-canonical-active.md §4.3.

import { describe, it, expect, beforeEach, vi } from 'vitest'

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

// Hoisted so the vi.mock factory (hoisted above top-level consts) can
// close over the saved-layout list + the mutable running-PTY list without
// a TDZ on the import-time `loadWorkspaceSessionsFromDb()` call.
const h = vi.hoisted(() => {
  // The boot loader fetches saved layouts from the daemon. Return two
  // workspaces' worth so we can prove the loader caches BOTH without
  // restoring EITHER into live tabs / background snapshots.
  const SAVED = [
    { projectId: 'projA', workspaceId: 'wsA', layoutJson: JSON.stringify({ version: 2, tabs: [], activeTabId: null }) },
    { projectId: 'projB', workspaceId: 'wsB', layoutJson: JSON.stringify({ version: 2, tabs: [], activeTabId: null }) },
  ]
  // The daemon's live PTY list — controllable per-test so we can model
  // boot survivors (aged-out chat PTYs alive from a prior app session).
  const state: { runningPtys: Array<{ terminalId: string; cwd: string; command: string | null }> } = {
    runningPtys: [],
  }
  return { SAVED, state }
})
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    if (route === 'workspace-layouts/load-all') return h.SAVED
    if (route === 'terminal/list-running') return h.state.runningPtys
    return []
  }),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => ''),
  setChatSession: vi.fn(async () => undefined),
}))

import { useTabsStore } from './tabs'

describe('OPEN-1 — boot-restore does not spawn non-active sessions', () => {
  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      backgroundWorkspaces: {},
      workspaceLayouts: {},
      extraGroups: [],
    })
  })

  it('loadWorkspaceSessionsFromDb caches every saved layout but restores none', async () => {
    await useTabsStore.getState().loadWorkspaceSessionsFromDb()
    const s = useTabsStore.getState()

    // Both saved workspaces are cached in the in-memory layout map…
    expect(Object.keys(s.workspaceLayouts).sort()).toEqual(['projA:wsA', 'projB:wsB'])

    // …but NONE are restored: no live tabs, no background snapshots. The
    // renderer never SPAWNS a non-active workspace's PTY at boot — it only
    // mounts the ONE workspace `fetchProjects` restores. (A regression that
    // iterated backgroundWorkspaces / built tabs at boot would fail here.)
    expect(s.tabs).toEqual([])
    expect(s.backgroundWorkspaces).toEqual({})
    expect(s.activeTabId).toBeNull()
  })
})
