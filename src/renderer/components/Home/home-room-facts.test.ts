// plan-agent-status-unify-v1 S4: an open room's facts for its Home row.

import { describe, expect, it } from 'vitest'
import { roomFactsOf } from '@/lib/home-agent-status'
import type { ActivityRow, ActivityWorkspace } from '@/stores/session-events'
import type { UnseenDone } from '@/stores/activity'

function row(sessionId: string, workspacePath: string, display: ActivityRow['display']): ActivityRow {
  return {
    sessionId,
    agentName: sessionId,
    projectId: 'p1',
    workspacePath,
    harness: 'claude',
    display,
    lead: { state: 'idle', outcome: 'none', since: 0, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: null,
    evidenceAt: null,
    evidenceSource: null,
    reason: 'idle_prompt',
    staleSince: null,
    confirmed: true,
    rev: 1,
  }
}

function ws(path: string, extra: Partial<ActivityWorkspace> = {}): ActivityWorkspace {
  return {
    projectId: 'p1',
    workspacePath: path,
    display: 'idle',
    counts: { working: 0, waiting: 0, monitoring: 0, unverifiable: 0, idle: 0 },
    since: null,
    ...extra,
  }
}

describe('roomFactsOf', () => {
  it('counts the rows under the room, sums D1 busy/live, reads unseen', () => {
    const rows = new Map([
      ['a', row('a', '/w/app', 'working')],
      ['b', row('b', '/w/app/sub', 'waiting')],
      ['c', row('c', '/w-other', 'working')],
    ])
    const workspaces = new Map([['p:p1', ws('/w/app', { live: 2, busy: 1 })]])
    const unseen = new Map<string, UnseenDone>([
      ['a', { at: 1, agentName: 'a', projectId: 'p1', workspacePath: '/w/app', outcome: 'failure' }],
    ])
    expect(roomFactsOf({ rows, workspaces, unseenDone: unseen }, '/w/app')).toEqual({
      counts: { working: 1, waiting: 1, monitoring: 0, unverifiable: 0, idle: 0 },
      busy: 1,
      live: 2,
      unseen: 'failed',
    })
  })

  it('a daemon without D1: busy and live unknown', () => {
    const facts = roomFactsOf({ rows: new Map(), workspaces: new Map([['p:p1', ws('/w/app')]]), unseenDone: new Map() }, '/w/app')
    expect(facts.busy).toBeNull()
    expect(facts.live).toBeNull()
  })

  it('a D1 daemon with no rollup under the room: nothing live', () => {
    const facts = roomFactsOf(
      { rows: new Map(), workspaces: new Map([['p:p2', ws('/elsewhere', { live: 1, busy: 0 })]]), unseenDone: new Map() },
      '/w/app',
    )
    expect(facts.live).toBe(0)
    expect(facts.busy).toBe(0)
  })
})
