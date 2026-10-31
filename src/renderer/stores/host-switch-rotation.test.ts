// Sign-in with a TEMPORARY password, then the rotation step — the agent
// list must land (0.46.8: every login someone else makes is temporary).
//
// THE BUG THIS PINS: loginToHost with a temporary password commits the
// RESTRICTED token on the active host. empty → token used to count as a
// session "mint", so every onActiveHostChange subscriber (projects, tabs,
// unread, presence, project groups…) ran its fetch burst against a session
// that answers 403 password_change_required, and projects went to [].
// After the rotation step, loginToHost commits the FULL token — but token
// → token on the same host key was not a mint, and RemoteSignIn's
// same-key selectHost fires nothing, so the list stayed empty until a
// relaunch. Fix: a restricted session is not "authed" (no burst), the full
// token after rotation IS the mint, and a person's own re-sign-in on the
// active host (full → new full) replays the burst too. The silent
// revival (remote-session) stays quiet.

import { describe, it, expect, beforeEach, vi } from 'vitest'

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

const daemonCliGet = vi.fn<(route: string, params?: Record<string, unknown>) => Promise<unknown>>()
const daemonCliPost = vi.fn(async () => ({}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...args: unknown[]) => daemonCliGet(...(args as [string, Record<string, unknown>?]))),
    daemonCliGetText: primaryOnly(vi.fn(async () => '')),
    daemonCliPost: primaryOnly((...args: unknown[]) => (daemonCliPost as unknown as (...a: unknown[]) => Promise<unknown>)(...args)),
  }
})

const settingsGetMock = vi.fn<() => Promise<Record<string, unknown>>>()
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: () => settingsGetMock(),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
  subscribeToWorkspaceTabEvents: vi.fn(() => () => undefined),
  onSessionAddedApp: vi.fn(() => () => undefined),
  onSessionRemovedApp: vi.fn(() => () => undefined),
  onProjectsChanged: vi.fn(() => () => undefined),
  onAgentIdentityChanged: vi.fn(() => () => undefined),
}))

import {
  useConnectHostStore,
  onActiveHostChange,
  loginToHost,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from './connect-host'
import { useProjectsStore } from './projects'
import { useSettingsStore } from './settings'

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
    dnsManageEnabled: 0,
    agentsCanCreateConnections: 0,
  }
}

/** What the remote lists; a test can add a project to see a reload land. */
let remoteProjects: Record<string, unknown>[] = []

function makeRemoteHost(token: string): ConnectHost {
  return {
    id: 'host-1',
    label: 'Office box',
    hostname: '192.168.1.50',
    username: 'anna',
    port: 60710,
    secure: false,
    token,
    remember: true,
    lastConnectedAt: null,
  }
}

/** The host-aware daemon: a restricted (or missing) session gets the 403
 *  the real daemon answers; any other token sees the remote's projects. */
function installDaemon(): void {
  const refused = (): boolean => {
    const active = useConnectHostStore.getState().activeHost
    return active !== 'local' && (active.token === 'restricted' || active.token.length === 0)
  }
  daemonCliGet.mockImplementation(async (route) => {
    if (refused()) throw new Error('403: password_change_required')
    if (route === 'projects/list') return remoteProjects
    if (route === 'workspaces/list') return []
    if (route === 'themes/list') return []
    return null
  })
  settingsGetMock.mockImplementation(async () => {
    if (refused()) throw new Error('settings_get 403: password_change_required')
    return { lastActiveProjectId: null, lastActiveWorkspaceId: null }
  })
}

function loginRes(token: string, mustChangePassword: boolean): Response {
  const body = { token, username: 'anna', expiresAt: 'x', mustChangePassword }
  return {
    ok: true,
    status: 200,
    text: async () => JSON.stringify(body),
    json: async () => body,
  } as unknown as Response
}

function projectIds(): string[] {
  return useProjectsStore.getState().projects.map((p) => p.id)
}

async function settle(): Promise<void> {
  await new Promise((r) => setTimeout(r, 20))
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.unstubAllGlobals()
  __resetConnectHostStoreForTests()
  remoteProjects = [mkProject('remote-p1')]
  useProjectsStore.setState({ projects: [], activeProjectId: null, activeWorkspaceId: null })
  useSettingsStore.setState({ settingsOpen: false, loaded: true, lastActiveProjectId: null, lastActiveWorkspaceId: null })
  installDaemon()
})

describe('a restricted session is not a mint; the full token after rotation is', () => {
  it('setHostToken: restricted → no burst, projects stay []; full → the burst lands the list, no reload', async () => {
    // Active remote, signed out (empty token): the switch burst can only fail.
    useConnectHostStore.getState().selectHost(makeRemoteHost(''))
    await settle()
    expect(projectIds()).toEqual([])

    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))

    useConnectHostStore.getState().setHostToken('host-1', 'restricted', { restricted: true })
    await settle()
    expect(fired).toHaveLength(0)
    expect(projectIds()).toEqual([])
    expect(useConnectHostStore.getState().restrictedHosts).toEqual(['host-1'])
    // The restricted token is still on the host (the rotation step uses it).
    const active = useConnectHostStore.getState().activeHost
    if (active === 'local') throw new Error('active host flipped to local')
    expect(active.token).toBe('restricted')

    useConnectHostStore.getState().setHostToken('host-1', 'full')
    expect(fired).toHaveLength(1)
    await vi.waitFor(() => expect(projectIds()).toEqual(['remote-p1']), { timeout: 2000 })
    expect(useConnectHostStore.getState().restrictedHosts).toEqual([])
    off()
  })

  it('the real loginToHost: temporary password, then the rotated one — the agent list lands', async () => {
    const host = makeRemoteHost('')
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    await settle()

    const fetchMock = vi
      .fn<(url: string, init?: RequestInit) => Promise<Response>>()
      .mockResolvedValueOnce(loginRes('restricted', true))
      .mockResolvedValueOnce(loginRes('full', false))
    vi.stubGlobal('fetch', fetchMock)

    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))
    const callsBefore = daemonCliGet.mock.calls.length

    const first = await loginToHost(host, 'temp-pass')
    expect(first).toEqual({ ok: true, token: 'restricted', mustChangePassword: true })
    await settle()
    expect(fired).toHaveLength(0)
    expect(projectIds()).toEqual([])
    // No doomed burst ran against the restricted session.
    expect(daemonCliGet.mock.calls.slice(callsBefore).map((c) => c[0])).not.toContain('projects/list')

    // PasswordRotationStep: change-password, then the same loginToHost with
    // the NEW password → a FULL token.
    const second = await loginToHost(host, 'my-own-pass')
    expect(second).toEqual({ ok: true, token: 'full', mustChangePassword: false })
    expect(fired).toHaveLength(1)
    await vi.waitFor(() => expect(projectIds()).toEqual(['remote-p1']), { timeout: 2000 })
    expect(fetchMock).toHaveBeenCalledTimes(2)
    off()
  })
})

describe('a re-sign-in on the active host (full → new full token)', () => {
  it("the person's own sign-in replays the burst; the silent revival does not", async () => {
    const host = makeRemoteHost('full-a')
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    await vi.waitFor(() => expect(projectIds()).toEqual(['remote-p1']), { timeout: 2000 })

    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))
    // The remote gained an agent the stale list never fetched.
    remoteProjects = [mkProject('remote-p1'), mkProject('remote-p2')]

    vi.stubGlobal(
      'fetch',
      vi
        .fn<(url: string, init?: RequestInit) => Promise<Response>>()
        .mockResolvedValueOnce(loginRes('full-b', false))
        .mockResolvedValueOnce(loginRes('full-c', false)),
    )

    // Silent revival (remote-session): token refresh, no store reset.
    await loginToHost(host, 'remembered', undefined, { revival: true })
    await settle()
    expect(fired).toHaveLength(0)
    expect(projectIds()).toEqual(['remote-p1'])

    // The person signs in again (RemoteSignIn): the list reloads.
    await loginToHost(host, 'typed')
    expect(fired).toHaveLength(1)
    await vi.waitFor(() => expect(projectIds()).toEqual(['remote-p1', 'remote-p2']), { timeout: 2000 })

    // RemoteSignIn's follow-up same-key selectHost does not double-fire.
    const now = useConnectHostStore.getState().hosts.find((h) => h.id === 'host-1')
    if (!now) throw new Error('host vanished')
    useConnectHostStore.getState().selectHost(now)
    expect(fired).toHaveLength(1)
    off()
  })

  it('a restricted re-login over a full session does not fire (its burst could only 403)', async () => {
    const host = makeRemoteHost('full-a')
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    await vi.waitFor(() => expect(projectIds()).toEqual(['remote-p1']), { timeout: 2000 })

    const fired: string[] = []
    const off = onActiveHostChange((next) => fired.push(next))
    vi.stubGlobal('fetch', vi.fn(async () => loginRes('restricted', true)))
    await loginToHost(host, 'reset-by-owner')
    await settle()
    expect(fired).toHaveLength(0)
    expect(useConnectHostStore.getState().restrictedHosts).toEqual(['host-1'])
    off()
  })
})
