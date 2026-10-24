// plan-agent-status-unify-v1 S4: a Home row on another server speaks the
// same status language — reach on the chip's LEDs, actions as words, the
// cluster from the summary (D2) or an open room.

import { describe, expect, it } from 'vitest'
import { grokAgentStatus, remoteAgentStatus, remoteRowBadges, rowActionWord, rowReach, summaryRollup } from './home-agent-status'
import type { PresenceActivity, RowStatus } from './home-status'
import { parsePresenceActivity } from './host-pool'

const row = { address: 'press::akzm.k2.dev', workspaceId: 'w1', label: 'press' }
const live: RowStatus = { kind: 'live', label: 'Live', people: [] }

function st(kind: RowStatus['kind']): RowStatus {
  return { kind, label: kind, people: [] }
}

describe('reach and action words', () => {
  it('maps every row kind to a reach', () => {
    expect(rowReach('live')).toBe('ok')
    expect(rowReach('working')).toBe('ok')
    expect(rowReach('no-access')).toBe('ok')
    expect(rowReach('checking')).toBe('wait')
    expect(rowReach('starting')).toBe('wait')
    expect(rowReach('signing-in')).toBe('wait')
    expect(rowReach('sign-in')).toBe('wait')
    expect(rowReach('offline')).toBe('off')
  })

  it('keeps words for actions only (Q2)', () => {
    expect(rowActionWord(st('sign-in'))).toBe('Sign in')
    expect(rowActionWord(st('no-access'))).toBe('No access')
    expect(rowActionWord({ kind: 'not-found', label: 'Not found', people: [] })).toBe('Not found')
    for (const k of ['working', 'permission', 'monitoring', 'unverifiable', 'review', 'idle', 'live', 'offline', 'checking', 'starting'] as const) {
      expect(rowActionWord(st(k))).toBeNull()
    }
  })
})

describe('remoteAgentStatus from the summary', () => {
  it('a server without D2: an unlisted row is idle with live unknown (never 0)', () => {
    const v = remoteAgentStatus({ status: live, row, activity: [], room: null })
    expect(v.status).toBe('idle')
    expect(v.live).toBeNull()
  })

  it('a server with the feature: an unlisted row is reaped', () => {
    const v = remoteAgentStatus({ status: live, row, activity: [], room: null, serverLiveBusy: true })
    expect(v.status).toBe('reaped')
  })

  it('a D2 idle entry with a live session and a heartbeat', () => {
    const activity: PresenceActivity[] = [
      { workspaceId: 'w1', status: 'working', display: 'idle', heartbeat: true, live: true, busy: false, counts: { idle: 1 } },
    ]
    const v = remoteAgentStatus({ status: live, row, activity, room: null })
    expect(v).toMatchObject({ status: 'live', heartbeat: true, working: false, live: true })
  })

  it('working + needs you on D2 (busy with a waiting lead)', () => {
    const activity: PresenceActivity[] = [
      { workspaceId: 'w1', status: 'permission', display: 'waiting', live: true, busy: true, counts: { waiting: 1 } },
    ]
    const v = remoteAgentStatus({ status: st('permission'), row, activity, room: null })
    expect(v.working).toBe(true)
    expect(v.status).toBe('needs-you')
  })

  it('an older summary entry (display only) still names the status', () => {
    const activity: PresenceActivity[] = [{ workspaceId: 'w1', status: 'working', display: 'monitoring' }]
    expect(remoteAgentStatus({ status: st('monitoring'), row, activity, room: null }).status).toBe('monitoring')
    const old: PresenceActivity[] = [{ workspaceId: 'w1', status: 'working' }]
    expect(remoteAgentStatus({ status: st('working'), row, activity: old, room: null }).working).toBe(true)
  })

  it('rows that cannot answer show nothing in the cluster', () => {
    for (const k of ['offline', 'checking', 'sign-in', 'no-access', 'not-found'] as const) {
      const v = remoteAgentStatus({ status: st(k), row, activity: [], room: null, serverLiveBusy: true })
      expect(v.status).toBe('idle')
      expect(v.working).toBe(false)
    }
  })
})

describe('remoteAgentStatus from an open room', () => {
  it('reads "not viewed" (only an open room can) and the D1 live count', () => {
    const v = remoteAgentStatus({
      status: live,
      row,
      activity: null,
      room: { counts: { working: 0, waiting: 0, monitoring: 1, unverifiable: 0, idle: 0 }, busy: 0, live: 1, unseen: 'failed' },
    })
    expect(v.status).toBe('failed')
    expect(v.live).toBe(true)
  })

  it('a room on a daemon without D1: working from counts, live unknown', () => {
    const v = remoteAgentStatus({
      status: live,
      row,
      activity: null,
      room: { counts: { working: 1, waiting: 0, monitoring: 0, unverifiable: 0, idle: 0 }, busy: null, live: null, unseen: null },
    })
    expect(v.working).toBe(true)
    expect(v.live).toBeNull()
    expect(v.status).toBe('idle')
  })
})

describe('summary parsing (D2)', () => {
  it('keeps heartbeat / live / busy / counts and idle entries', () => {
    const out = parsePresenceActivity([
      { workspaceId: 'w1', status: 'idle', display: 'idle', heartbeat: true, live: true, busy: false, counts: { idle: 1, working: 0, bogus: 4 } },
      { workspaceId: 'w2', status: 'working', display: 'working', live: 'yes' },
    ])
    expect(out).toEqual([
      { workspaceId: 'w1', status: 'working', display: 'idle', heartbeat: true, live: true, busy: false, counts: { idle: 1, working: 0 } },
      { workspaceId: 'w2', status: 'working', display: 'working' },
    ])
  })

  it('a summary rollup keeps D2 booleans as counts', () => {
    expect(summaryRollup({ workspaceId: 'w', status: 'working', display: 'working', busy: true, live: false })).toEqual({
      display: 'working',
      counts: { working: 1 },
      busy: 1,
      live: 0,
    })
  })
})

describe('remote row badges (agent-unread-tickets-v1)', () => {
  const entry: PresenceActivity = {
    workspaceId: 'w1',
    status: 'working',
    display: 'idle',
    unread: 4,
    waiting: 1,
    muted: true,
    ticketsWaiting: 2,
    ticketsWaitingMe: 1,
  }

  it('parses the inbox fields and drops malformed ones', () => {
    const out = parsePresenceActivity([
      { workspaceId: 'w1', status: 'idle', display: 'idle', unread: 4, waiting: 1, muted: true, ticketsWaiting: 2, ticketsWaitingMe: 1 },
      { workspaceId: 'w2', status: 'idle', display: 'idle', unread: -1, waiting: 'x', muted: 'no', ticketsWaiting: Number.NaN },
    ])
    expect(out).toEqual([
      entry,
      { workspaceId: 'w2', status: 'working', display: 'idle' },
    ])
  })

  it('shows only with the feature, on a reachable signed-in row, from the row\'s own entry', () => {
    const want = { unread: 4, waiting: 1, muted: true, tickets: 2, ticketsMine: 1 }
    expect(remoteRowBadges({ status: live, row, activity: [entry], serverReports: true })).toEqual(want)
    // An older server: never guessed, even if the fields are there.
    expect(remoteRowBadges({ status: live, row, activity: [entry], serverReports: false })).toBeNull()
    // Not reading that server (sign in, offline, no access): none.
    for (const k of ['sign-in', 'offline', 'no-access', 'checking', 'not-found'] as const) {
      expect(remoteRowBadges({ status: st(k), row, activity: [entry], serverReports: true })).toBeNull()
    }
    // Another workspace's entry, or none listed: nothing.
    expect(remoteRowBadges({ status: live, row, activity: [{ ...entry, workspaceId: 'w9' }], serverReports: true })).toBeNull()
    expect(remoteRowBadges({ status: live, row, activity: null, serverReports: true })).toBeNull()
    // All zero: no badge.
    expect(
      remoteRowBadges({
        status: live,
        row,
        activity: [{ ...entry, unread: 0, waiting: 0, ticketsWaiting: 0, ticketsWaitingMe: 0 }],
        serverReports: true,
      }),
    ).toBeNull()
    // Tickets alone are enough.
    expect(
      remoteRowBadges({ status: st('working'), row, activity: [{ ...entry, unread: 0, waiting: 0 }], serverReports: true }),
    ).toEqual({ unread: 0, waiting: 0, muted: true, tickets: 2, ticketsMine: 1 })
  })
})

describe('Grok Bot rows', () => {
  it('map onto the same vocabulary, live unknown', () => {
    expect(grokAgentStatus('working')).toMatchObject({ working: true, status: 'idle', live: null })
    expect(grokAgentStatus('needs-you')).toMatchObject({ working: false, status: 'needs-you' })
    expect(grokAgentStatus('idle').status).toBe('idle')
    expect(grokAgentStatus('not-running').status).toBe('idle')
  })
})
