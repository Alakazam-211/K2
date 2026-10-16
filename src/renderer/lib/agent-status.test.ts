// plan-agent-status-unify-v1 §5: the pure fold. Every combination of
// rollup counts × busy × unseen × heartbeat × live gives exactly one status
// plus independent working / live / heartbeat, by the §3.1 rank.

import { describe, expect, it } from 'vitest'
import { agentStatus, shortcutLabel, STATUS_RANK, type AgentRollupInput, type AgentStatus } from './agent-status'
import type { ActivityDisplay } from '@/stores/session-events'

function rollup(
  counts: Partial<Record<ActivityDisplay, number>>,
  extra: { live?: number; busy?: number } = {},
): AgentRollupInput {
  const order: ActivityDisplay[] = ['waiting', 'working', 'monitoring', 'unverifiable', 'idle']
  const display = order.find((d) => (counts[d] ?? 0) > 0) ?? 'idle'
  return {
    display,
    counts: { working: 0, waiting: 0, monitoring: 0, unverifiable: 0, idle: 0, ...counts },
    ...extra,
  }
}

describe('agentStatus: the named cases', () => {
  it('busy + waiting → working AND needs-you (a subagent runs while the lead asks)', () => {
    const v = agentStatus({ rollup: rollup({ waiting: 1 }, { busy: 1, live: 1 }), unseen: null, heartbeatEnabled: false })
    expect(v.working).toBe(true)
    expect(v.status).toBe('needs-you')
  })

  it('busy + monitoring → working and monitoring', () => {
    const v = agentStatus({ rollup: rollup({ working: 1, monitoring: 1 }, { busy: 1, live: 2 }), unseen: null, heartbeatEnabled: false })
    expect(v.working).toBe(true)
    expect(v.status).toBe('monitoring')
  })

  it('monitoring + unseen done → done (done outranks background work)', () => {
    const v = agentStatus({ rollup: rollup({ monitoring: 1 }, { busy: 0, live: 1 }), unseen: 'done', heartbeatEnabled: false })
    expect(v.status).toBe('done')
    expect(v.working).toBe(false)
  })

  it('idle + live → live; idle + not live → reaped', () => {
    expect(agentStatus({ rollup: rollup({ idle: 1 }, { live: 1, busy: 0 }), unseen: null, heartbeatEnabled: false }).status).toBe('live')
    expect(agentStatus({ rollup: rollup({ idle: 1 }, { live: 0, busy: 0 }), unseen: null, heartbeatEnabled: false }).status).toBe('reaped')
  })

  it('needs-you + live → needs-you (a status wins the slot over live)', () => {
    const v = agentStatus({ rollup: rollup({ waiting: 1 }, { live: 1, busy: 0 }), unseen: null, heartbeatEnabled: false })
    expect(v.status).toBe('needs-you')
    expect(v.live).toBe(true)
  })

  it('live null → empty slot (idle), never guessed', () => {
    const v = agentStatus({ rollup: null, unseen: null, heartbeatEnabled: true, liveFallback: null })
    expect(v.status).toBe('idle')
    expect(v.live).toBeNull()
    expect(v.title).toBe('')
    expect(v.heartbeat).toBe(true)
  })

  it('failed outranks done; needs-you outranks failed', () => {
    expect(agentStatus({ rollup: null, unseen: 'failed', heartbeatEnabled: false }).status).toBe('failed')
    expect(agentStatus({ rollup: rollup({ waiting: 1 }), unseen: 'failed', heartbeatEnabled: false }).status).toBe('needs-you')
  })
})

describe('agentStatus: fallback on a daemon without D1 (no live / busy)', () => {
  it('working falls back to counts.working > 0', () => {
    expect(agentStatus({ rollup: rollup({ working: 1 }), unseen: null, heartbeatEnabled: false }).working).toBe(true)
    // Before D1 a waiting lead with a running subagent can't be seen as working.
    expect(agentStatus({ rollup: rollup({ waiting: 1 }), unseen: null, heartbeatEnabled: false }).working).toBe(false)
  })

  it('live falls back to the renderer signal, and is null without one', () => {
    expect(agentStatus({ rollup: rollup({ idle: 1 }), unseen: null, heartbeatEnabled: false, liveFallback: true }).status).toBe('live')
    expect(agentStatus({ rollup: rollup({ idle: 1 }), unseen: null, heartbeatEnabled: false, liveFallback: false }).status).toBe('reaped')
    expect(agentStatus({ rollup: rollup({ idle: 1 }), unseen: null, heartbeatEnabled: false }).live).toBeNull()
  })

  it('a daemon that reports live: no rollup = reaped, and its field beats the fallback', () => {
    expect(agentStatus({ rollup: null, unseen: null, heartbeatEnabled: false, daemonLive: true, liveFallback: true }).status).toBe('reaped')
    expect(agentStatus({ rollup: rollup({ idle: 1 }, { live: 0 }), unseen: null, heartbeatEnabled: false, liveFallback: true }).status).toBe('reaped')
  })

  it('busy wins over counts.working when the daemon sends it', () => {
    expect(agentStatus({ rollup: rollup({ working: 1 }, { busy: 0 }), unseen: null, heartbeatEnabled: false }).working).toBe(false)
  })
})

describe('agentStatus: the full table', () => {
  const countsCases: Array<[string, Partial<Record<ActivityDisplay, number>>]> = [
    ['none', {}],
    ['waiting', { waiting: 1 }],
    ['working', { working: 1 }],
    ['monitoring', { monitoring: 1 }],
    ['unverifiable', { unverifiable: 1 }],
    ['waiting+monitoring', { waiting: 1, monitoring: 1 }],
    ['monitoring+unverifiable', { monitoring: 1, unverifiable: 2 }],
    ['idle', { idle: 2 }],
  ]
  const busyCases: Array<number | undefined> = [undefined, 0, 1]
  const unseenCases: Array<'done' | 'failed' | null> = [null, 'done', 'failed']
  const liveCases: Array<number | undefined> = [undefined, 0, 1]

  for (const [label, counts] of countsCases)
    for (const busy of busyCases)
      for (const unseen of unseenCases)
        for (const heartbeat of [false, true])
          for (const live of liveCases) {
            it(`${label} busy=${busy} unseen=${unseen} hb=${heartbeat} live=${live}`, () => {
              const r = rollup(counts, { ...(busy !== undefined ? { busy } : {}), ...(live !== undefined ? { live } : {}) })
              const v = agentStatus({ rollup: r, unseen, heartbeatEnabled: heartbeat })
              // One status, by rank.
              let expected: AgentStatus
              if ((counts.waiting ?? 0) > 0) expected = 'needs-you'
              else if (unseen === 'failed') expected = 'failed'
              else if (unseen === 'done') expected = 'done'
              else if ((counts.monitoring ?? 0) > 0) expected = 'monitoring'
              else if ((counts.unverifiable ?? 0) > 0) expected = 'no-update'
              else if (live === undefined) expected = 'idle'
              else expected = live > 0 ? 'live' : 'reaped'
              expect(v.status).toBe(expected)
              // Independent attributes.
              expect(v.heartbeat).toBe(heartbeat)
              expect(v.working).toBe(busy !== undefined ? busy > 0 : (counts.working ?? 0) > 0)
              expect(v.live).toBe(live === undefined ? null : live > 0)
              expect(v.workingTitle === '').toBe(!v.working)
              expect(v.title === '').toBe(v.status === 'idle')
            })
          }
})

describe('agentStatus: tooltips', () => {
  it('names the reason from the row the status comes from', () => {
    const v = agentStatus({
      rollup: rollup({ waiting: 1 }),
      unseen: null,
      heartbeatEnabled: false,
      titleRow: { display: 'waiting', evidenceAt: 0, staleSince: null, reason: 'waiting_permission' },
    })
    expect(v.title).toBe('Needs you · Waiting for your permission')
  })

  it('no update gives its minutes', () => {
    const v = agentStatus({
      rollup: rollup({ unverifiable: 1 }),
      unseen: null,
      heartbeatEnabled: false,
      titleRow: { display: 'unverifiable', evidenceAt: 0, staleSince: null, reason: 'stale' },
      now: 34 * 60_000,
    })
    expect(v.title).toBe('No update in 34m · Nothing heard from the agent for a while')
  })

  it('live and reaped say what they mean', () => {
    expect(agentStatus({ rollup: rollup({}, { live: 1 }), unseen: null, heartbeatEnabled: false }).title).toBe('Live session (using RAM)')
    expect(agentStatus({ rollup: rollup({}, { live: 0 }), unseen: null, heartbeatEnabled: false }).title).toBe(
      'No live session (reaped to save RAM)',
    )
  })
})

describe('shortcutLabel (the number column)', () => {
  it('1…9, 0 for the tenth, then -', () => {
    expect(Array.from({ length: 13 }, (_, i) => shortcutLabel(i))).toEqual([
      '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '-', '-',
    ])
  })

  it('a negative or broken index is a dash', () => {
    expect(shortcutLabel(-1)).toBe('-')
    expect(shortcutLabel(1.5)).toBe('-')
  })
})

describe('STATUS_RANK', () => {
  it('is the approved order', () => {
    expect(STATUS_RANK).toEqual(['needs-you', 'failed', 'done', 'monitoring', 'no-update'])
  })
})
