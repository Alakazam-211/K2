// #625 — per-machine UI session state must RESET when the active host
// changes.
//
// After the desktop client switches from the LOCAL daemon to a REMOTE
// one, the `<App key={hostKey}>` remount does NOT reset module-level
// singletons (zustand `create()` stores + module `Map`/`let` variables).
// So state keyed by LOCAL project/workspace IDs would leak across the
// switch and the Active Bar / IconRail active section would keep showing
// the local machine's workspaces.
//
// The fix wires a `onActiveHostChange` subscription in each module that
// OWNS the leaking state (tabs.ts, ActiveBar.tsx, active-agents.ts). This
// test drives a real activeHost change via the store and asserts every
// reset fired — and that the INITIAL 'local' subscribe does NOT reset.
//
// vitest env is `node` (no Tauri). We mock the Tauri + daemon-cli
// boundaries the imported modules touch at load time so importing them is
// inert, then spy on `loadWorkspaceSessionsFromDb` to prove the reload
// re-fires on a host switch.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────

// Tauri core/event bridges — inert in node. `daemon_ws_url` must return a
// well-SHAPED "unavailable" response (not null) so getDaemonWs throws a
// CLEAN caught error rather than a null-deref TypeError during the
// import-time store inits (focus-groups et al.). Everything else resolves
// null.
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

// daemon-cli — the host-aware HTTP layer. loadWorkspaceSessionsFromDb
// calls daemonCliGet('workspace-layouts/load-all'); return an empty list
// so the loader resolves cleanly without a daemon.
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))

// daemon-reconnect — its onDaemonConnected registers a Tauri listener.
// No-op it so import-time wiring doesn't reach Tauri.
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// daemon-settings — the settings store eagerly fetches on import. Resolve
// it with defaults so that import-time init doesn't surface an unhandled
// rejection (the daemon is unreachable in node). Not under test here.
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))

// Now import the REAL modules under test. Importing them registers their
// module-scope onActiveHostChange subscriptions (the thing we're testing).
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from './connect-host'
import {
  useTabsStore,
  __hasLoadedWorkspaceSessionsForTests,
} from './tabs'
import { __activeBarMemoryForTests } from '@/components/Sidebar/ActiveBar'
import { useActiveAgentsStore } from './active-agents'
// #625 — newly-wired daemon-backed stores that must re-fetch on a host
// change so the client is a pure view of the active host's daemon.
import { useSettingsStore } from './settings'
import { useFocusGroupsStore } from './focus-groups'
import { usePanelsStore, __resetPanelsLoadGateForTests } from './panels'
import { useTimerStore } from './timer'
import { useCustomThemesStore } from './custom-themes'
import { useProjectsStore } from './projects'

function makeRemoteHost(): ConnectHost {
  return {
    id: 'host-1',
    label: 'Hetzner box',
    hostname: '178.156.232.105',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'session-token',
    remember: false,
    lastConnectedAt: null,
  }
}

describe('#625 host-switch resets per-machine UI session state', () => {
  beforeEach(() => {
    __resetConnectHostStoreForTests() // activeHost back to 'local'
    // Seed each store/map with LOCAL-keyed state as if the user had been
    // using the local machine.
    const mem = __activeBarMemoryForTests()
    mem.memory.set('local-project-A', 123)
    mem.dismissed.set('local-project-B', 456)
    useTabsStore.setState({
      backgroundWorkspaces: { 'local-project-A:ws1': {} as never },
      workspaceLayouts: { 'local-project-A:ws1': {} as never },
      activeWorkspaceKey: 'local-project-A:ws1',
    })
    useActiveAgentsStore.setState({
      paneStatuses: new Map([['pane-local', 'working']]),
      paneProjectMap: new Map([['pane-local', 'local-project-A']]),
    })
  })

  it('clears ActiveBar memory + dismissed maps on a host change', () => {
    const mem = __activeBarMemoryForTests()
    expect(mem.memory.size).toBe(1)
    expect(mem.dismissed.size).toBe(1)

    useConnectHostStore.getState().selectHost(makeRemoteHost())

    expect(mem.memory.size).toBe(0)
    expect(mem.dismissed.size).toBe(0)
  })

  it('resets hasLoadedWorkspaceSessions + re-invokes loadWorkspaceSessionsFromDb', () => {
    const loadSpy = vi.spyOn(
      useTabsStore.getState(),
      'loadWorkspaceSessionsFromDb',
    )
    // Pretend the baseline load already happened for the LOCAL host.
    useTabsStore.setState({
      backgroundWorkspaces: { 'local-project-A:ws1': {} as never },
    })
    expect(useTabsStore.getState().backgroundWorkspaces['local-project-A:ws1']).toBeDefined()

    useConnectHostStore.getState().selectHost(makeRemoteHost())

    // Module gate reset so the reconnect-retry path will fire again, and
    // the loader was re-invoked against the NEW host.
    expect(__hasLoadedWorkspaceSessionsForTests()).toBe(false)
    expect(loadSpy).toHaveBeenCalledTimes(1)
    // Local-keyed session state cleared.
    expect(useTabsStore.getState().backgroundWorkspaces).toEqual({})
    expect(useTabsStore.getState().workspaceLayouts).toEqual({})
    expect(useTabsStore.getState().activeWorkspaceKey).toBeNull()
  })

  it('clears active-agents pane state on a host change', () => {
    expect(useActiveAgentsStore.getState().paneStatuses.size).toBe(1)
    expect(useActiveAgentsStore.getState().paneProjectMap.size).toBe(1)

    useConnectHostStore.getState().selectHost(makeRemoteHost())

    expect(useActiveAgentsStore.getState().paneStatuses.size).toBe(0)
    expect(useActiveAgentsStore.getState().paneProjectMap.size).toBe(0)
    expect(useActiveAgentsStore.getState().getProjectStatus('local-project-A')).toBe('idle')
  })

  it('does NOT reset on the initial local subscribe / a no-op reselect of local', () => {
    const mem = __activeBarMemoryForTests()
    // activeHost is 'local'; reselecting 'local' is not a CHANGE.
    useConnectHostStore.getState().selectHost('local')
    expect(mem.memory.size).toBe(1)
    expect(mem.dismissed.size).toBe(1)
    expect(useTabsStore.getState().activeWorkspaceKey).toBe('local-project-A:ws1')
  })

  it('resets again on a switch BACK to local (every real change)', () => {
    // local → remote (resets, asserted above), then remote → local.
    useConnectHostStore.getState().selectHost(makeRemoteHost())
    // Re-seed as if the remote populated some state.
    const mem = __activeBarMemoryForTests()
    mem.memory.set('remote-project-X', 999)
    useTabsStore.setState({ activeWorkspaceKey: 'remote-project-X:ws1' })

    useConnectHostStore.getState().selectHost('local')

    expect(mem.memory.size).toBe(0)
    expect(useTabsStore.getState().activeWorkspaceKey).toBeNull()
  })
})

// #625 — every REMAINING daemon-backed store must re-fetch/re-init on a
// host change so the client shows the NEW host's data, not stale LOCAL
// data left over from boot. Each case spies on the store's loader and
// asserts: (a) NOT called on the initial 'local' subscribe (the boot load
// already happened at module import, before the spy was installed),
// (b) called exactly once on local → remote, (c) called again on the
// switch BACK remote → local. Spying AFTER import means the import-time
// boot init is never counted.
describe('#625 host-switch re-fetches all remaining daemon-backed stores', () => {
  beforeEach(() => {
    __resetConnectHostStoreForTests() // activeHost back to 'local'
    vi.restoreAllMocks()
  })

  it('settings.fetchSettings re-fires on a host change (and switch-back)', () => {
    const spy = vi
      .spyOn(useSettingsStore.getState(), 'fetchSettings')
      .mockResolvedValue(undefined)

    // Initial subscribe is already 'local'; no reset on the initial state.
    expect(spy).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost(makeRemoteHost())
    expect(spy).toHaveBeenCalledTimes(1)

    useConnectHostStore.getState().selectHost('local')
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('focus-groups.initFromSettings re-fires on a host change (and switch-back)', () => {
    const spy = vi
      .spyOn(useFocusGroupsStore.getState(), 'initFromSettings')
      .mockResolvedValue(undefined)

    expect(spy).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost(makeRemoteHost())
    expect(spy).toHaveBeenCalledTimes(1)

    useConnectHostStore.getState().selectHost('local')
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('panels.initFromSettings re-fires + load gate is reset on a host change', () => {
    // Pretend the LOCAL baseline already loaded (gate true) as it would be
    // after the boot init.
    __resetPanelsLoadGateForTests()
    void usePanelsStore // referenced so the import isn't tree-shaken
    const spy = vi
      .spyOn(usePanelsStore.getState(), 'initFromSettings')
      .mockResolvedValue(undefined)

    expect(spy).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost(makeRemoteHost())
    expect(spy).toHaveBeenCalledTimes(1)

    useConnectHostStore.getState().selectHost('local')
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('timer.initFromSettings re-fires on a host change (and switch-back)', () => {
    const spy = vi
      .spyOn(useTimerStore.getState(), 'initFromSettings')
      .mockResolvedValue(undefined)

    expect(spy).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost(makeRemoteHost())
    expect(spy).toHaveBeenCalledTimes(1)

    useConnectHostStore.getState().selectHost('local')
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('custom-themes.loadCustomThemes re-fires on a host change (and switch-back)', () => {
    const spy = vi
      .spyOn(useCustomThemesStore.getState(), 'loadCustomThemes')
      .mockResolvedValue(undefined)

    expect(spy).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost(makeRemoteHost())
    expect(spy).toHaveBeenCalledTimes(1)

    useConnectHostStore.getState().selectHost('local')
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('projects.fetchProjects re-fires + active selection clears so restore re-runs', () => {
    // Seed an OLD-host active selection as if it survived the App remount.
    useProjectsStore.setState({
      activeProjectId: 'local-project-A',
      activeWorkspaceId: 'local-ws-1',
    })
    const spy = vi
      .spyOn(useProjectsStore.getState(), 'fetchProjects')
      .mockResolvedValue(undefined)

    expect(spy).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost(makeRemoteHost())
    // Active selection cleared so the restore branch inside fetchProjects
    // re-runs against the NEW host, and the loader re-fired.
    expect(spy).toHaveBeenCalledTimes(1)
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
    expect(useProjectsStore.getState().activeWorkspaceId).toBeNull()

    useConnectHostStore.getState().selectHost('local')
    expect(spy).toHaveBeenCalledTimes(2)
  })
})
