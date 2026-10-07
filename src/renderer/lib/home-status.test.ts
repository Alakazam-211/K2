// Home P1 / M2 — row status resolver (ready / starting / offline / sign-in /
// no access / presence) over the connection pool's entry for the row's
// server. The probe itself is the pool's (lib/host-pool.test.ts). Plan
// decisions 1 and 5; vs-live H14 / H19 f; MS27, MS81, MS83.

import { describe, it, expect } from 'vitest'
import {
  activityForRow,
  bootIsReady,
  personInitials,
  presenceForRow,
  resolveRowStatus,
  roomRowActivity,
  type PresenceWorkspace,
} from './home-status'
import type { HostEntry } from './host-pool'
import type { ActivityDisplay, ActivityRow } from '@/stores/activity'
import { parsePresenceActivity } from '@/lib/host-pool'
import type { RosterUser } from '@/stores/presence'

const row = { address: 'bee::b.k2.dev', workspaceId: 'pb' }

function probe(p: Partial<HostEntry>): HostEntry {
  return {
    hostKey: 'b.k2.dev',
    saved: true,
    hostId: 'b',
    reach: 'live',
    boot: null,
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: [],
    offlineStreak: 0,
    checkedAt: 1,
    ...p,
  }
}

const summary: PresenceWorkspace[] = [
  {
    workspaceId: 'pb',
    handle: 'bee',
    name: 'Bee',
    path: '/srv/bee',
    count: 3,
    people: [
      { user: 'owner', name: 'Rosson', role: 'owner' },
      { user: 'anna', name: 'anna', role: 'member' },
      { user: 'me', name: 'me', role: 'member' },
    ],
  },
  { workspaceId: 'pz', handle: 'zed', name: 'Zed', path: '/srv/zed', count: 1, people: [{ user: 'x', name: 'x', role: 'member' }] },
]

describe('bootIsReady', () => {
  it('accepts phase:"ready" or ready:true, nothing else', () => {
    expect(bootIsReady({ phase: 'ready' })).toBe(true)
    expect(bootIsReady({ ready: true, version: '0.41.6' } as { ready: boolean })).toBe(true)
    expect(bootIsReady({ phase: 'migrating' })).toBe(false)
    expect(bootIsReady({ ready: false })).toBe(false)
    expect(bootIsReady({ ready: 'true' })).toBe(false)
    expect(bootIsReady(null)).toBe(false)
  })
})

describe('resolveRowStatus — another saved server', () => {
  const base = { where: 'other' as const, row, saved: true, hasLogin: true, self: 'me' }

  it('ready → live, with the others on that agent (never you)', () => {
    const s = resolveRowStatus({ ...base, entry: probe({ presence: summary }) })
    expect(s.kind).toBe('live')
    expect(s.label).toBe('Live')
    expect(s.people.map((p) => p.user)).toEqual(['owner', 'anna'])
  })

  it('unreachable / not ready → offline', () => {
    expect(resolveRowStatus({ ...base, entry: probe({ reach: 'offline', presence: null }) }).kind).toBe('offline')
  })

  it('no login held → "Sign in" instead of offline, even with no probe', () => {
    const s = resolveRowStatus({ ...base, hasLogin: false, entry: undefined })
    expect(s.kind).toBe('sign-in')
    expect(s.label).toBe('Sign in')
    expect(resolveRowStatus({ ...base, hasLogin: false, entry: probe({ reach: 'offline' }) }).kind).toBe('sign-in')
  })

  it('the server refused the saved login → "Sign in"', () => {
    expect(resolveRowStatus({ ...base, entry: probe({ auth: 'signin-required', presence: null }) }).kind).toBe('sign-in')
  })

  it('kicked / new password → "Sign in", with the reason as the detail', () => {
    const kicked = resolveRowStatus({ ...base, serverLabel: 'Box B', entry: probe({ auth: 'kicked' }) })
    expect(kicked.kind).toBe('sign-in')
    expect(kicked.detail).toBe('Removed from Box B. Sign in again.')
    const rotate = resolveRowStatus({ ...base, serverLabel: 'Box B', entry: probe({ auth: 'rotate-required' }) })
    expect(rotate.kind).toBe('sign-in')
    expect(rotate.detail).toBe('Box B needs a new password.')
    const noted = resolveRowStatus({
      ...base,
      hasLogin: false,
      entry: probe({ auth: 'signin-required', authNote: 'Too many sign-ins from this network. Try again in 4 min.' }),
    })
    expect(noted.kind).toBe('sign-in')
    expect(noted.detail).toBe('Too many sign-ins from this network. Try again in 4 min.')
  })

  it('another window is signing in → "Signing in…", never a login of our own', () => {
    expect(resolveRowStatus({ ...base, hasLogin: false, entry: probe({ auth: 'signing-in' }) }).kind).toBe('signing-in')
    expect(resolveRowStatus({ ...base, entry: probe({ auth: 'signing-in' }) }).label).toBe('Signing in…')
  })

  it('restarting (not ready yet) → "Starting"', () => {
    const s = resolveRowStatus({ ...base, entry: probe({ reach: 'starting' }) })
    expect(s.kind).toBe('starting')
    expect(s.label).toBe('Starting')
  })

  it('MS83: a role below Member, or one this app does not know, → "No access"', () => {
    for (const role of ['viewer', 'auditor', '']) {
      const s = resolveRowStatus({ ...base, serverLabel: 'Box B', entry: probe({ role }) })
      expect([role, s.kind]).toEqual([role, 'no-access'])
      expect(s.label).toBe('No access')
    }
    for (const role of ['owner', 'admin', 'member']) {
      expect([role, resolveRowStatus({ ...base, entry: probe({ role }) }).kind]).toEqual([role, 'live'])
    }
    // Not read (an older server without whoami) is not "No access".
    expect(resolveRowStatus({ ...base, entry: probe({ role: null }) }).kind).toBe('live')
  })

  it('MS81: the same daemon at a second address carries a "same server as" note', () => {
    const s = resolveRowStatus({ ...base, entry: probe({}), sameServerAs: 'This computer' })
    expect(s.kind).toBe('live')
    expect(s.note).toBe('Same server as This computer')
    expect(resolveRowStatus({ ...base, entry: probe({}) }).note).toBeUndefined()
  })

  it('not probed yet → checking; not a saved server → offline', () => {
    expect(resolveRowStatus({ ...base, entry: undefined }).kind).toBe('checking')
    expect(resolveRowStatus({ ...base, saved: false, entry: undefined }).kind).toBe('offline')
  })

  it('live on an older server with no summary → live, nobody shown', () => {
    const s = resolveRowStatus({ ...base, entry: probe({ presence: null }) })
    expect(s.kind).toBe('live')
    expect(s.people).toEqual([])
  })
})

describe('resolveRowStatus — the connected server', () => {
  const roster: RosterUser[] = [
    { user: 'owner', role: 'owner', windowCount: 1, workspaces: ['/srv/bee'], grantedEdit: false, connectedAt: 1 },
    { user: 'anna', role: 'member', windowCount: 1, workspaces: ['/srv/bee/sub'], grantedEdit: false, connectedAt: 1 },
    { user: 'zoe', role: 'member', windowCount: 1, workspaces: ['/srv/other'], grantedEdit: false, connectedAt: 1 },
  ]
  const base = {
    where: 'connected' as const,
    row,
    workspace: { id: 'pb', path: '/srv/bee' },
    roster,
    rosterSupported: true,
    self: 'owner',
  }

  it('shows working / idle from the activity store, presence from the live roster', () => {
    const w = resolveRowStatus({ ...base, activity: 'working' })
    expect(w.kind).toBe('working')
    expect(w.people.map((p) => p.user)).toEqual(['anna'])
    expect(resolveRowStatus({ ...base, activity: 'idle' }).kind).toBe('idle')
    expect(resolveRowStatus({ ...base, activity: 'permission' }).label).toBe('Needs you')
  })

  it('never offline; a missing workspace is not-found', () => {
    expect(resolveRowStatus({ ...base, workspace: null, activity: 'idle' }).kind).toBe('not-found')
  })

  it('presence unsupported → nobody shown', () => {
    expect(resolveRowStatus({ ...base, rosterSupported: false, activity: 'idle' }).people).toEqual([])
  })
})

describe('Home 0.43.2 (Z23): what an agent on another server is doing', () => {
  const other = {
    where: 'other' as const,
    row,
    saved: true,
    hasLogin: true,
    self: 'me',
    serverLabel: 'B',
  }

  it('a closed row reads the server’s folded summary: Working, Needs you, else Live', () => {
    const working = resolveRowStatus({ ...other, entry: probe({ presence: summary, activity: [{ workspaceId: 'pb', status: 'working' }] }) })
    expect([working.kind, working.label]).toEqual(['working', 'Working'])
    expect(working.people.map((p) => p.user)).toEqual(['owner', 'anna'])
    const waiting = resolveRowStatus({ ...other, entry: probe({ activity: [{ workspaceId: 'pb', status: 'permission' }] }) })
    expect([waiting.kind, waiting.label]).toEqual(['permission', 'Needs you'])
    // Another workspace busy on the same server: this row stays Live.
    expect(resolveRowStatus({ ...other, entry: probe({ activity: [{ workspaceId: 'pz', status: 'working' }] }) }).kind).toBe('live')
    // An older server sends no activity.
    expect(resolveRowStatus({ ...other, entry: probe({}) }).kind).toBe('live')
    expect(resolveRowStatus({ ...other, entry: probe({ activity: null }) }).kind).toBe('live')
  })

  it('an open room’s live slice wins over a stale summary, both ways', () => {
    const stale = probe({ activity: [{ workspaceId: 'pb', status: 'working' }] })
    expect(resolveRowStatus({ ...other, entry: stale, roomActivity: 'idle' }).kind).toBe('live')
    expect(resolveRowStatus({ ...other, entry: probe({}), roomActivity: 'permission' }).kind).toBe('permission')
    expect(resolveRowStatus({ ...other, entry: probe({}), roomActivity: 'working' }).kind).toBe('working')
    expect(resolveRowStatus({ ...other, entry: stale, roomActivity: null }).kind).toBe('working')
  })

  it('activity never paints over offline, sign-in or no access', () => {
    const busy = [{ workspaceId: 'pb', status: 'working' as const }]
    expect(resolveRowStatus({ ...other, entry: probe({ reach: 'offline', activity: busy }) }).kind).toBe('offline')
    expect(resolveRowStatus({ ...other, entry: probe({ auth: 'kicked', activity: busy }) }).kind).toBe('sign-in')
    expect(resolveRowStatus({ ...other, entry: probe({ role: 'viewer', activity: busy }), roomActivity: 'working' }).kind).toBe(
      'no-access',
    )
  })

  it('activityForRow matches by workspace id only', () => {
    expect(activityForRow([{ workspaceId: 'pb', status: 'permission' }], row)).toBe('permission')
    expect(activityForRow([{ workspaceId: 'pb', status: 'permission' }], { address: 'bee::b.k2.dev', workspaceId: null })).toBe(null)
    expect(activityForRow(null, row)).toBe(null)
  })

  it('a server with daemon activity sends the real display; the row shows it (S5, A31)', () => {
    const parsed = parsePresenceActivity([
      { workspaceId: 'pb', status: 'working', display: 'monitoring' },
      { workspaceId: 'pc', status: 'idle', display: 'unverifiable' },
      { workspaceId: 'pd', status: 'permission', display: 'waiting' },
      { workspaceId: 'pe', status: 'working' },
      { workspaceId: 'pf', status: 'idle' },
    ])
    if (!parsed) throw new Error('parse returned null')
    expect(parsed.map((a) => [a.workspaceId, a.display ?? null])).toEqual([
      ['pb', 'monitoring'],
      ['pc', 'unverifiable'],
      ['pd', 'waiting'],
      ['pe', null],
    ])
    expect(activityForRow(parsed, row)).toBe('monitoring')
    expect(activityForRow(parsed, { ...row, workspaceId: 'pd' })).toBe('permission')
    const mon = resolveRowStatus({ ...other, entry: probe({ activity: parsed }) })
    expect([mon.kind, mon.label]).toEqual(['monitoring', 'Monitoring'])
    const stale = resolveRowStatus({ ...other, row: { ...row, workspaceId: 'pc' }, entry: probe({ activity: parsed }) })
    expect([stale.kind, stale.label]).toEqual(['unverifiable', 'No update'])
  })

  it('roomRowActivity: the highest display among the server’s rows under the room’s root', () => {
    const r = (sessionId: string, display: ActivityDisplay, workspacePath: string): ActivityRow => ({
      sessionId,
      agentName: `tab-${sessionId}`,
      projectId: null,
      workspacePath,
      harness: 'claude',
      display,
      lead: { state: 'idle', outcome: 'none', since: 0, promptId: null },
      children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
      turnStartedAt: null,
      evidenceAt: null,
      evidenceSource: null,
      reason: 'turn_done',
      staleSince: null,
      confirmed: true,
      rev: 1,
    })
    const view = (rows: ActivityRow[]) => ({ rows: new Map(rows.map((x) => [x.sessionId, x])) })
    expect(roomRowActivity(view([]), '/srv/b')).toBe('idle')
    expect(roomRowActivity(view([r('a', 'working', '/srv/b')]), '/srv/b')).toBe('working')
    expect(roomRowActivity(view([r('a', 'working', '/srv/b'), r('b', 'waiting', '/srv/b/sub')]), '/srv/b')).toBe('permission')
    expect(roomRowActivity(view([r('a', 'monitoring', '/srv/b'), r('b', 'unverifiable', '/srv/b')]), '/srv/b')).toBe('monitoring')
    // Another workspace on the same server is not this room's.
    expect(roomRowActivity(view([r('a', 'working', '/srv/b-other')]), '/srv/b')).toBe('idle')
  })
})

describe('presenceForRow', () => {
  it('matches by id, then by handle, else nobody', () => {
    expect(presenceForRow(summary, { address: 'renamed::b.k2.dev', workspaceId: 'pb' }, 'me')).toHaveLength(2)
    expect(presenceForRow(summary, { address: 'zed::b.k2.dev', workspaceId: null }, 'me').map((p) => p.user)).toEqual(['x'])
    expect(presenceForRow(summary, { address: 'nope::b.k2.dev', workspaceId: 'p404' }, 'me')).toEqual([])
    expect(presenceForRow(null, row, 'me')).toEqual([])
  })

  it('initials', () => {
    expect(personInitials('anna')).toBe('A')
    expect(personInitials('Rosson Long')).toBe('RL')
    expect(personInitials('sum_member')).toBe('SM')
    expect(personInitials('  ')).toBe('?')
  })
})
