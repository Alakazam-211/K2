// Presence S2 — store coverage: snapshot ingest, presence_changed
// whole-set replace, 404 → unsupported (graceful degradation on older
// daemons), host-switch reset (#625 seam), and the top-bar roster
// overflow math (10 visible + `+N`).
//
// The store wires its event subscriptions at module import (the
// active-agents convention), so the session-events registries are mocked
// BEFORE the import and the recorded handlers are fired directly —
// mirroring daemon-broadcasts.test.ts. The connect-host store is REAL
// (like host-switch-reset.test.ts) so the host-switch reset is proven
// through the actual onActiveHostChange seam, not a mock of it.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Boundary mocks (installed BEFORE the store import) ───────────────────

// Tauri core — connect-host imports `invoke` at module scope.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

// daemon-cli — the snapshot GET, controllable per test.
const cli = vi.hoisted(() => ({
  getImpl: (async () => ({ roster: [] })) as (route: string) => Promise<unknown>,
  getCalls: [] as string[],
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    cli.getCalls.push(route)
    return cli.getImpl(route)
  }),
}))

// session-events — record the registered app-level handlers so tests can
// fire presence_changed / hello exactly as the app-level WS would.
const ev = vi.hoisted(() => {
  type Fn = (...a: unknown[]) => void
  return {
    presence: [] as Fn[],
    appHello: [] as Fn[],
  }
})
vi.mock('@/stores/session-events', () => ({
  onPresenceChanged: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.presence.push(fn)
    return () => void (ev.presence = ev.presence.filter((f) => f !== fn))
  }),
  onAppHello: vi.fn((fn: (...a: unknown[]) => void) => {
    ev.appHello.push(fn)
    return () => void (ev.appHello = ev.appHello.filter((f) => f !== fn))
  }),
}))

// Import the module under test (registers the module-scope wiring) + the
// REAL connect-host store for the host-switch seam.
import {
  usePresenceStore,
  rosterDisplay,
  shouldShowRoster,
  workspacePathsOverlap,
  usersForWorkspace,
  isSoleWorkspaceViewer,
  MAX_ROSTER_AVATARS,
  MAX_WORKSPACE_AVATARS,
  type RosterUser,
} from './presence'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from './connect-host'

function row(user: string, overrides: Partial<RosterUser> = {}): RosterUser {
  return {
    user,
    role: user === 'owner' ? 'owner' : 'member',
    windowCount: 1,
    workspaces: [],
    grantedEdit: false,
    connectedAt: 1_700_000_000,
    ...overrides,
  }
}

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

beforeEach(() => {
  __resetConnectHostStoreForTests()
  usePresenceStore.setState({ roster: [], supported: true })
  cli.getImpl = async () => ({ roster: [] })
  cli.getCalls = []
})

describe('presence store — module wiring', () => {
  it('registered exactly one presence_changed handler and one app-hello handler at import', () => {
    expect(ev.presence.length).toBe(1)
    expect(ev.appHello.length).toBe(1)
  })
})

describe('presence store — snapshot ingest (hello reconcile)', () => {
  it('app-level hello fetches GET presence/roster and ingests the snapshot', async () => {
    const snapshot = [row('owner', { windowCount: 2 }), row('alice', { workspaces: ['/x/foo'] })]
    cli.getImpl = async () => ({ roster: snapshot })

    ev.appHello[0]()
    await vi.waitFor(() => {
      expect(usePresenceStore.getState().roster).toEqual(snapshot)
    })

    expect(cli.getCalls).toEqual(['presence/roster'])
    expect(usePresenceStore.getState().supported).toBe(true)
  })

  it('a malformed snapshot body (no roster array) ingests as empty, not a crash', async () => {
    usePresenceStore.setState({ roster: [row('owner'), row('bob')] })
    cli.getImpl = async () => ({ nope: true })

    await usePresenceStore.getState().refreshRoster()

    expect(usePresenceStore.getState().roster).toEqual([])
    expect(usePresenceStore.getState().supported).toBe(true)
  })

  it('a transient (non-404) failure keeps the last-known roster and stays supported', async () => {
    const known = [row('owner'), row('carol')]
    usePresenceStore.setState({ roster: known })
    cli.getImpl = async () => {
      throw new Error('fetch failed: ECONNREFUSED')
    }

    await usePresenceStore.getState().refreshRoster()

    expect(usePresenceStore.getState().roster).toEqual(known)
    expect(usePresenceStore.getState().supported).toBe(true)
  })
})

describe('presence store — presence_changed replace-whole-set', () => {
  it('each event REPLACES the roster wholesale (last-write-wins, no merge)', () => {
    ev.presence[0]({
      kind: 'presence_changed',
      roster: [row('owner'), row('alice'), row('bob')],
    })
    expect(usePresenceStore.getState().roster.map((r) => r.user)).toEqual([
      'owner',
      'alice',
      'bob',
    ])

    // Second event drops bob and adds nobody — alice's row must be the
    // carried one, bob's must be GONE (whole-set semantics).
    ev.presence[0]({
      kind: 'presence_changed',
      roster: [row('owner'), row('alice', { windowCount: 3 })],
    })
    const roster = usePresenceStore.getState().roster
    expect(roster.map((r) => r.user)).toEqual(['owner', 'alice'])
    expect(roster[1].windowCount).toBe(3)
  })

  it('a grant flip rides the whole-set replace (S4: viewer grantedEdit live-update)', () => {
    // Grant: the daemon re-broadcasts the roster with grantedEdit true.
    ev.presence[0]({
      kind: 'presence_changed',
      roster: [row('owner'), row('vera', { role: 'viewer', grantedEdit: false })],
    })
    expect(usePresenceStore.getState().roster[1]).toMatchObject({
      user: 'vera',
      role: 'viewer',
      grantedEdit: false,
    })

    ev.presence[0]({
      kind: 'presence_changed',
      roster: [row('owner'), row('vera', { role: 'viewer', grantedEdit: true })],
    })
    expect(usePresenceStore.getState().roster[1].grantedEdit).toBe(true)

    // Auto-revoke on last disconnect: the departure broadcast drops the
    // row; a reconnect broadcast carries grantedEdit false again.
    ev.presence[0]({ kind: 'presence_changed', roster: [row('owner')] })
    ev.presence[0]({
      kind: 'presence_changed',
      roster: [row('owner'), row('vera', { role: 'viewer', grantedEdit: false })],
    })
    expect(usePresenceStore.getState().roster[1].grantedEdit).toBe(false)
  })

  it('an empty-roster event clears everything', () => {
    usePresenceStore.setState({ roster: [row('owner'), row('alice')] })
    ev.presence[0]({ kind: 'presence_changed', roster: [] })
    expect(usePresenceStore.getState().roster).toEqual([])
  })

  it('a presence_changed frame proves support (re-enables after a 404)', () => {
    usePresenceStore.setState({ supported: false, roster: [] })
    ev.presence[0]({ kind: 'presence_changed', roster: [row('owner')] })
    expect(usePresenceStore.getState().supported).toBe(true)
    expect(usePresenceStore.getState().roster.length).toBe(1)
  })
})

describe('presence store — 404 → unsupported (older daemon)', () => {
  it('a "route not found" rejection flips supported off and clears the roster', async () => {
    usePresenceStore.setState({ roster: [row('owner'), row('alice')] })
    // daemon-cli surfaces the daemon's canonical 404 body
    // (`{"error":"route not found"}`) as the Error message.
    cli.getImpl = async () => {
      throw new Error('route not found')
    }

    await usePresenceStore.getState().refreshRoster()

    expect(usePresenceStore.getState().supported).toBe(false)
    expect(usePresenceStore.getState().roster).toEqual([])
  })

  it('the next hello re-probes even while unsupported (mid-session daemon upgrade recovers)', async () => {
    usePresenceStore.setState({ supported: false, roster: [] })
    cli.getImpl = async () => ({ roster: [row('owner'), row('dave')] })

    ev.appHello[0]()
    await vi.waitFor(() => {
      expect(usePresenceStore.getState().supported).toBe(true)
    })
    expect(usePresenceStore.getState().roster.map((r) => r.user)).toEqual(['owner', 'dave'])
  })
})

describe('presence store — host-switch reset (#625)', () => {
  it('switching hosts clears the roster and resets supported optimistically', () => {
    usePresenceStore.setState({
      roster: [row('owner'), row('local-user')],
      supported: false,
    })

    useConnectHostStore.getState().selectHost(makeRemoteHost())

    expect(usePresenceStore.getState().roster).toEqual([])
    expect(usePresenceStore.getState().supported).toBe(true)
  })

  it('reselecting the SAME host is not a change — no reset', () => {
    const seeded = [row('owner'), row('erin')]
    usePresenceStore.setState({ roster: seeded })

    useConnectHostStore.getState().selectHost('local')

    expect(usePresenceStore.getState().roster).toEqual(seeded)
  })

  it('switching BACK to local resets again (every real change)', () => {
    useConnectHostStore.getState().selectHost(makeRemoteHost())
    usePresenceStore.setState({ roster: [row('owner'), row('remote-user')] })

    useConnectHostStore.getState().selectHost('local')

    expect(usePresenceStore.getState().roster).toEqual([])
  })
})

describe('presence roster — overflow math (10 visible + N)', () => {
  const users = (n: number): RosterUser[] =>
    Array.from({ length: n }, (_, i) => row(i === 0 ? 'owner' : `user-${String(i).padStart(2, '0')}`))

  it('MAX_ROSTER_AVATARS is the PRD-specced 10', () => {
    expect(MAX_ROSTER_AVATARS).toBe(10)
  })

  it('≤10 users → all visible, no overflow chip', () => {
    expect(rosterDisplay(users(1))).toEqual({ visible: users(1), overflow: 0 })
    expect(rosterDisplay(users(10)).visible.length).toBe(10)
    expect(rosterDisplay(users(10)).overflow).toBe(0)
  })

  it('11 users → 10 visible + "+1"', () => {
    const { visible, overflow } = rosterDisplay(users(11))
    expect(visible.length).toBe(10)
    expect(overflow).toBe(1)
    // The FIRST 10 (daemon order: owner-first) stay visible.
    expect(visible[0].user).toBe('owner')
    expect(visible.map((u) => u.user)).toEqual(users(11).slice(0, 10).map((u) => u.user))
  })

  it('25 users → 10 visible + "+15"', () => {
    const { visible, overflow } = rosterDisplay(users(25))
    expect(visible.length).toBe(10)
    expect(overflow).toBe(15)
  })

  it('shouldShowRoster hides when alone, when empty, and when unsupported', () => {
    expect(shouldShowRoster([], true)).toBe(false)
    expect(shouldShowRoster(users(1), true)).toBe(false) // just the owner
    expect(shouldShowRoster(users(2), true)).toBe(true)
    expect(shouldShowRoster(users(2), false)).toBe(false) // older daemon
  })
})

// ── S6 — workspace-nav presence: path matcher + selector ─────────────────
//
// `workspacePathsOverlap` must mirror the daemon's
// `event_matches_workspace` normalization (session_events_ws.rs) exactly:
// trim trailing slashes, equal-after-trim OR prefix-plus-`/`. The daemon's
// own tests (`workspace_path_filter_rules_match_cli_endpoint`) pin the
// same cases from the Rust side.

describe('presence S6 — workspacePathsOverlap (event_matches_workspace mirror)', () => {
  it('exact match', () => {
    expect(workspacePathsOverlap('/x/foo', '/x/foo')).toBe(true)
  })

  it('nested worktree under the project path matches BOTH directions', () => {
    // Subscribed to the project → covers the worktree row…
    expect(workspacePathsOverlap('/x/foo', '/x/foo/worktrees/wt-1')).toBe(true)
    // …and subscribed to the worktree → shows on the project row.
    expect(workspacePathsOverlap('/x/foo/worktrees/wt-1', '/x/foo')).toBe(true)
  })

  it('trailing-slash normalization (either side, including doubled slashes)', () => {
    expect(workspacePathsOverlap('/x/foo/', '/x/foo')).toBe(true)
    expect(workspacePathsOverlap('/x/foo', '/x/foo/')).toBe(true)
    expect(workspacePathsOverlap('/x/foo//', '/x/foo')).toBe(true)
    expect(workspacePathsOverlap('/x/foo/', '/x/foo/bar')).toBe(true)
  })

  it('unrelated sibling prefix is NOT a match (/a/bc vs /a/b — segment boundary)', () => {
    expect(workspacePathsOverlap('/a/b', '/a/bc')).toBe(false)
    expect(workspacePathsOverlap('/a/bc', '/a/b')).toBe(false)
    // The daemon's own pinned case: foo-website is not under foo.
    expect(workspacePathsOverlap('/x/foo', '/x/foo-website/x')).toBe(false)
  })

  it('disjoint paths never match', () => {
    expect(workspacePathsOverlap('/x/foo', '/x/other')).toBe(false)
  })
})

describe('presence S6 — usersForWorkspace selector', () => {
  const roster: RosterUser[] = [
    row('owner', { workspaces: ['/x/foo'] }),
    // Multi-window user viewing TWO workspaces — must appear on both rows.
    row('alice', { windowCount: 2, workspaces: ['/x/foo/worktrees/wt-1', '/y/bar'] }),
    row('bob', { workspaces: ['/y/bar/'] }), // trailing slash on the wire
    row('carol', { workspaces: [] }), // app-socket only, no workspace views
  ]

  it('joins by the symmetric prefix rule (project row picks up nested-worktree viewers)', () => {
    expect(usersForWorkspace(roster, '/x/foo').map((u) => u.user)).toEqual(['owner', 'alice'])
  })

  it('a multi-window user appears in EVERY matching workspace row', () => {
    expect(usersForWorkspace(roster, '/x/foo/worktrees/wt-1').map((u) => u.user)).toEqual([
      'owner', // subscribed to the project ABOVE the worktree — covers it
      'alice',
    ])
    expect(usersForWorkspace(roster, '/y/bar').map((u) => u.user)).toEqual(['alice', 'bob'])
  })

  it('daemon roster ordering is preserved; non-viewers and non-matches drop out', () => {
    const hit = usersForWorkspace(roster, '/y/bar')
    expect(hit.map((u) => u.user)).toEqual(['alice', 'bob']) // carol (no views) absent
    expect(usersForWorkspace(roster, '/z/unrelated')).toEqual([])
  })

  it('an empty row path selects nobody (never match-all)', () => {
    expect(usersForWorkspace(roster, '')).toEqual([])
  })

  it('isSoleWorkspaceViewer is true when unsupported or at most one subscriber', () => {
    expect(isSoleWorkspaceViewer(roster, '/x/foo', false)).toBe(true)
    expect(isSoleWorkspaceViewer(roster, '/z/unrelated', true)).toBe(true)
    expect(isSoleWorkspaceViewer([row('owner', { workspaces: ['/x/foo'] })], '/x/foo', true)).toBe(
      true,
    )
    expect(isSoleWorkspaceViewer(roster, '/x/foo', true)).toBe(false)
  })

  it('nav clusters cap at 3 avatars before the +N chip (rosterDisplay reuse)', () => {
    expect(MAX_WORKSPACE_AVATARS).toBe(3)
    const five = Array.from({ length: 5 }, (_, i) =>
      row(`u${i}`, { workspaces: ['/x/foo'] }),
    )
    const { visible, overflow } = rosterDisplay(usersForWorkspace(five, '/x/foo'), MAX_WORKSPACE_AVATARS)
    expect(visible.length).toBe(3)
    expect(overflow).toBe(2)
  })
})
