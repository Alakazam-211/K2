// Host switch while a full-screen overlay (Settings, command palette,
// onboarding, …) is open — the "topbar says REMOTE, dashboard shows LOCAL"
// desync.
//
// THE INVARIANT: hostKey changes ⇒ every host-scoped surface rebuilds from
// the new host, regardless of what UI overlay was open during the switch.
// The switch path must be overlay-agnostic: it lives entirely in module-
// level `onActiveHostChange` subscriptions (+ the `<App key={hostKey}>`
// remount), never in any overlay's mount/unmount lifecycle. These tests
// drive the REAL connect-host + settings + projects + tabs modules through
// the owner's exact repro (open Settings → switch host → close Settings)
// and assert the dashboard's data sources (projects list, active
// project/workspace, tabs workspace key, background stash) all point at
// the NEW host — and that closing Settings restores nothing stale.
//
// THE BUG THIS PINS (root cause): the host-switch fetch burst fires at
// `selectHost` time, when the new host's session can be DEAD (a remote
// restart wiped the daemon's in-memory connect-sessions). Every burst
// fetch then fails terminally ('signin-required'), the user signs in via
// RemoteSignIn — which re-activates the SAME host, so `activeHostKey` never
// changes — and a key-only `onActiveHostChange` rule never re-fires the
// burst. Result: the gate/top-bar report the remote as connected while
// projects/tabs still hold the LOCAL host's data. Fix: `onActiveHostChange`
// also fires when the ACTIVE host's session is MINTED (token empty →
// non-empty, same key). A token REFRESH (non-empty → non-empty, the
// daemon-cli revive-and-replay path) must NOT fire.
//
// Also pinned: an in-flight fetch started against the PREVIOUS host must
// never land after the switch (fetchProjects / loadWorkspaceSessionsFromDb
// host-staleness guards).

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the modules import) ─────────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    if (cmd === 'k2so_sessions_list_for_workspace') return '[]'
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

// daemon-cli — the host-aware HTTP layer. The implementation is installed
// per-test (host-aware: reads `activeHost` at CALL time, like the real
// layer) so each test controls what the local vs remote daemon returns —
// including a dead-session remote that rejects everything.
const daemonCliGet = vi.fn<(route: string, params?: Record<string, unknown>) => Promise<unknown>>()
const daemonCliPost = vi.fn(async () => ({}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...(args as [string, Record<string, unknown>?])),
  daemonCliGetText: vi.fn(async () => ''),
  daemonCliPost: (...args: unknown[]) => (daemonCliPost as unknown as (...a: unknown[]) => Promise<unknown>)(...args),
}))

// daemon-settings — host-aware like the real settingsGet (reads activeHost
// at call time); each host remembers ITS OWN last-active selection.
const settingsGetMock = vi.fn<() => Promise<Record<string, unknown>>>()
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: () => settingsGetMock(),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))

vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// session-events — WS push layer; inert here (no daemon). Restore paths
// call the subscribe fns; they must return unsubscribe fns.
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn(() => () => undefined),
  onProjectsChanged: vi.fn(() => () => undefined),
}))

// Now import the REAL modules under test. Importing registers their
// module-scope onActiveHostChange subscriptions — the overlay-agnostic
// switch path itself.
import {
  useConnectHostStore,
  onActiveHostChange,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from './connect-host'
import { useSettingsStore } from './settings'
import { useProjectsStore } from './projects'
import { useTabsStore } from './tabs'

// ── Per-host daemon fixtures ─────────────────────────────────────────────

function mkProject(id: string): Record<string, unknown> {
  return {
    id,
    name: id,
    path: `/tmp/${id}`,
    color: '#fff',
    tabOrder: 0,
    lastOpenedAt: null,
    worktreeMode: 0,
    iconUrl: null,
    focusGroupId: null,
    pinned: 0,
    manuallyActive: 0,
    lastInteractionAt: null,
    createdAt: 1,
    agentEnabled: 0,
    heartbeatEnabled: 0,
    agentMode: 'off',
    stateId: null,
    heartbeatMode: 'off',
    heartbeatSchedule: null,
    heartbeatLastFire: null,
    allowRemoteInstruct: 0,
  }
}

function mkWorkspace(id: string, projectId: string): Record<string, unknown> {
  return {
    id,
    projectId,
    sectionId: null,
    type: 'main',
    branch: null,
    name: 'main',
    tabOrder: 0,
    worktreePath: null,
    navVisible: 1,
    createdAt: 1,
  }
}

const LOCAL_DATA = {
  projects: [mkProject('local-p1')],
  workspaces: { 'local-p1': [mkWorkspace('local-w1', 'local-p1')] } as Record<string, unknown[]>,
  lastActiveProjectId: 'local-p1',
  lastActiveWorkspaceId: 'local-w1',
}
const REMOTE_DATA = {
  projects: [mkProject('remote-p1')],
  workspaces: { 'remote-p1': [mkWorkspace('remote-w1', 'remote-p1')] } as Record<string, unknown[]>,
  lastActiveProjectId: 'remote-p1',
  lastActiveWorkspaceId: 'remote-w1',
}

function makeRemoteHost(token: string): ConnectHost {
  return {
    id: 'host-1',
    label: 'Hetzner box',
    hostname: '178.156.232.105',
    username: 'rosson',
    port: 443,
    secure: true,
    token,
    remember: true,
    lastConnectedAt: null,
  }
}

/**
 * Install the host-aware daemon mock. `remoteSessionDead` models a remote
 * whose in-memory connect-session was wiped: every authed route rejects
 * (the terminal 'signin-required' outcome of daemon-cli's revive path —
 * daemon-cli is mocked, so its internal replay is out of scope here) until
 * the ACTIVE host carries a token OTHER than the dead one.
 */
function installDaemon(opts: { deadRemoteToken?: string } = {}): void {
  daemonCliGet.mockImplementation(async (route, params) => {
    const active = useConnectHostStore.getState().activeHost
    const isRemote = active !== 'local'
    if (
      isRemote &&
      opts.deadRemoteToken !== undefined &&
      (active.token === opts.deadRemoteToken || active.token.length === 0)
    ) {
      throw new Error('403: Invalid or missing auth token')
    }
    const data = isRemote ? REMOTE_DATA : LOCAL_DATA
    if (route === 'projects/list') return data.projects
    if (route === 'workspaces/list') return data.workspaces[String(params?.project_id)] ?? []
    if (route === 'sections/list') return []
    if (route === 'workspace-layouts/load-all') return []
    if (route === 'workspace-layouts/load') return null
    if (route === 'focus-groups/list') return []
    if (route === 'themes/list') return []
    if (route === 'chat/detect-active') return { sessionId: null }
    return null
  })
  settingsGetMock.mockImplementation(async () => {
    const active = useConnectHostStore.getState().activeHost
    if (
      active !== 'local' &&
      opts.deadRemoteToken !== undefined &&
      (active.token === opts.deadRemoteToken || active.token.length === 0)
    ) {
      // Real settingsGet throws on non-2xx — the dead-session burst must
      // fail CLEANLY (no unhandled rejection flipping vitest's exit code).
      throw new Error('settings_get 403: Invalid or missing auth token')
    }
    const data = active === 'local' ? LOCAL_DATA : REMOTE_DATA
    return {
      lastActiveProjectId: data.lastActiveProjectId,
      lastActiveWorkspaceId: data.lastActiveWorkspaceId,
    }
  })
}

/** Wait until the projects store has settled on `projectId` as active. */
async function waitForActiveProject(projectId: string): Promise<void> {
  await vi.waitFor(
    () => {
      expect(useProjectsStore.getState().activeProjectId).toBe(projectId)
    },
    { timeout: 2000 },
  )
}

/** The dashboard's host-scoped data sources, as one comparable snapshot. */
function dashboardSnapshot(): Record<string, unknown> {
  const p = useProjectsStore.getState()
  const t = useTabsStore.getState()
  return {
    projectIds: p.projects.map((x) => x.id),
    activeProjectId: p.activeProjectId,
    activeWorkspaceId: p.activeWorkspaceId,
    activeWorkspaceKey: t.activeWorkspaceKey,
    backgroundKeys: Object.keys(t.backgroundWorkspaces),
    layoutKeys: Object.keys(t.workspaceLayouts),
  }
}

function expectDashboardOnRemote(): void {
  const snap = dashboardSnapshot()
  expect(snap.projectIds).toEqual(['remote-p1'])
  expect(snap.activeProjectId).toBe('remote-p1')
  expect(snap.activeWorkspaceId).toBe('remote-w1')
  expect(snap.activeWorkspaceKey).toBe('remote-p1:remote-w1')
  // No LOCAL-keyed session state may survive — the stale stash is gone,
  // not merely hidden behind the overlay.
  expect((snap.backgroundKeys as string[]).filter((k) => k.startsWith('local-'))).toEqual([])
  expect((snap.layoutKeys as string[]).filter((k) => k.startsWith('local-'))).toEqual([])
}

/** Boot the stores onto the LOCAL host as a real session would. */
async function seedLocalBaseline(): Promise<void> {
  await useProjectsStore.getState().fetchProjects()
  await waitForActiveProject('local-p1')
  // A second workspace stashed in the background — the "local dashboard
  // content" that must not survive (or be restored after) a host switch.
  useTabsStore.setState({
    backgroundWorkspaces: {
      'local-p1:local-w2': {
        tabs: [],
        extraGroups: [],
        splitCount: 1,
        activeGroupIndex: 0,
        activeTabId: null,
      } as never,
    },
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  __resetConnectHostStoreForTests()
  useProjectsStore.setState({ projects: [], activeProjectId: null, activeWorkspaceId: null })
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    extraGroups: [],
    splitCount: 1,
    activeGroupIndex: 0,
    backgroundWorkspaces: {},
    workspaceLayouts: {},
    activeWorkspaceKey: null,
    activeProjectId: null,
    activeWorkspaceId: null,
  })
  useSettingsStore.setState({
    settingsOpen: false,
    loaded: true,
    lastActiveProjectId: LOCAL_DATA.lastActiveProjectId,
    lastActiveWorkspaceId: LOCAL_DATA.lastActiveWorkspaceId,
  })
  installDaemon()
})

describe('host switch while Settings is open (owner repro, healthy remote)', () => {
  it('open Settings → switch → close Settings: every dashboard source points at the NEW host', async () => {
    await seedLocalBaseline()

    // 1. Open the Settings overlay.
    useSettingsStore.getState().openSettings()
    // 2. Switch via the switcher path (token present ⇒ silent switch).
    useConnectHostStore.getState().pickHost(makeRemoteHost('live-token'))
    await waitForActiveProject('remote-p1')
    // 3. Exit Settings.
    useSettingsStore.getState().closeSettings()

    expectDashboardOnRemote()
  })

  it('closing Settings is inert for host-scoped state (no stale stash restore on exit)', async () => {
    await seedLocalBaseline()
    useSettingsStore.getState().openSettings()
    useConnectHostStore.getState().pickHost(makeRemoteHost('live-token'))
    await waitForActiveProject('remote-p1')

    const before = dashboardSnapshot()
    useSettingsStore.getState().closeSettings()
    // Give any (wrongly) exit-coupled restore a chance to fire.
    await new Promise((r) => setTimeout(r, 10))
    expect(dashboardSnapshot()).toEqual(before)
  })

  it('the switch path is overlay-agnostic: same end state with Settings closed', async () => {
    await seedLocalBaseline()
    // No overlay this time — the reference flow.
    useConnectHostStore.getState().pickHost(makeRemoteHost('live-token'))
    await waitForActiveProject('remote-p1')

    expectDashboardOnRemote()
  })

  it('the switch synchronously drops the old host state (nothing left for an overlay exit to resurrect)', async () => {
    await seedLocalBaseline()
    useSettingsStore.getState().openSettings()

    useConnectHostStore.getState().pickHost(makeRemoteHost('live-token'))

    // Immediately after the synchronous flip — before ANY new-host fetch
    // resolves — the local-keyed state is already gone. This is what makes
    // overlay-agnosticism structural: the reset lives in module-level
    // subscriptions, not in any component lifecycle.
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
    expect(useTabsStore.getState().backgroundWorkspaces).toEqual({})
    expect(useTabsStore.getState().activeWorkspaceKey).toBeNull()
  })
})

describe('host switch onto a DEAD remote session (the reproducible desync)', () => {
  it('sign-in mint after a failed switch burst replays the burst — dashboard lands on the remote', async () => {
    // The remote's in-memory session was wiped (daemon restarted since the
    // token was remembered): every switch-time fetch rejects.
    installDaemon({ deadRemoteToken: 'dead-token' })
    await seedLocalBaseline()

    useSettingsStore.getState().openSettings()
    const dead = makeRemoteHost('dead-token')
    useConnectHostStore.getState().pickHost(dead)
    // Let the doomed switch burst reject and settle.
    await new Promise((r) => setTimeout(r, 20))

    // The desync precondition: top bar targets the remote, but the burst
    // died — the dashboard still holds LOCAL data.
    expect(useConnectHostStore.getState().activeHost).not.toBe('local')
    expect(useProjectsStore.getState().projects.map((p) => p.id)).toEqual(['local-p1'])

    // Gate's first-connect whoami probe finds the session dead → expire →
    // RemoteSignIn. Then the user signs in: loginToHost mints a fresh
    // session (setHostToken) and RemoteSignIn re-activates the SAME host
    // (selectHost with an unchanged hostKey).
    useConnectHostStore.getState().expireSession('host-1')
    useConnectHostStore.getState().setHostToken('host-1', 'fresh-token')
    const refreshed = { ...dead, token: 'fresh-token' }
    useConnectHostStore.getState().selectHost(refreshed)

    // The mint must replay the host-switch burst: without it nothing ever
    // re-fetches (hostKey never changed) and the dashboard stays LOCAL
    // while the top bar says the remote is connected.
    await waitForActiveProject('remote-p1')
    useSettingsStore.getState().closeSettings()
    expectDashboardOnRemote()
  })

  it('a token REFRESH (non-empty → non-empty) does NOT re-fire the burst', () => {
    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))

    useConnectHostStore.getState().selectHost(makeRemoteHost('token-a'))
    expect(fired).toHaveLength(1)

    // daemon-cli revival path: a new token for the SAME live host. The
    // revive already replays the rejected request; a full store reset here
    // would churn every mounted surface mid-session.
    useConnectHostStore.getState().setHostToken('host-1', 'token-b')
    expect(fired).toHaveLength(1)

    off()
  })

  it('a mint for a NON-active host does not fire, and re-selecting local stays a no-op', () => {
    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))

    // Sign-in-for-management of a host we are NOT switched to.
    useConnectHostStore.getState().addHost(makeRemoteHost(''))
    useConnectHostStore.getState().setHostToken('host-1', 'managed-token')
    expect(fired).toHaveLength(0)

    // Re-selecting the already-active local host is not a change.
    useConnectHostStore.getState().selectHost('local')
    expect(fired).toHaveLength(0)

    off()
  })

  it('expiry alone (token dropped) does not fire a doomed burst; the mint does, once', () => {
    useConnectHostStore.getState().selectHost(makeRemoteHost('dying-token'))

    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))

    useConnectHostStore.getState().expireSession('host-1')
    expect(fired).toHaveLength(0) // tokenless fetches can only fail

    useConnectHostStore.getState().setHostToken('host-1', 'fresh-token')
    expect(fired).toHaveLength(1) // the recovery point

    // RemoteSignIn's follow-up selectHost (same key, already authed) must
    // not double-fire the burst.
    useConnectHostStore.getState().selectHost(makeRemoteHost('fresh-token'))
    expect(fired).toHaveLength(1)

    off()
  })
})

describe('in-flight fetches from the PREVIOUS host cannot land after the switch', () => {
  it('a slow local projects/list resolving post-switch does not clobber the remote dashboard', async () => {
    await seedLocalBaseline()

    // Re-arm the daemon mock so the NEXT local projects/list hangs until
    // we release it — an in-flight fetch racing the switch.
    let releaseLocal!: (v: unknown) => void
    const gate = new Promise((r) => {
      releaseLocal = r
    })
    const base = daemonCliGet.getMockImplementation()!
    daemonCliGet.mockImplementation(async (route, params) => {
      const active = useConnectHostStore.getState().activeHost
      if (active === 'local' && route === 'projects/list') {
        await gate
        return LOCAL_DATA.projects
      }
      return base(route, params)
    })

    // A local re-fetch is mid-flight (e.g. a sync:projects refresh) when
    // the user switches away…
    const inflight = useProjectsStore.getState().fetchProjects()
    useConnectHostStore.getState().pickHost(makeRemoteHost('live-token'))
    await waitForActiveProject('remote-p1')

    // …and only NOW does the old host's response arrive.
    releaseLocal(undefined)
    await inflight
    await new Promise((r) => setTimeout(r, 10))

    expectDashboardOnRemote()
  })
})
