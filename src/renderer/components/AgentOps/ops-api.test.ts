// Pure-logic tests for the Agent Ops read API. No daemon, no WS, no DOM —
// every function under test is side-effect-free. Fail-loud: exact equality
// assertions, no try/catch swallowing, no skip-if-missing fallbacks.

import { describe, it, expect } from 'vitest'
import {
  applyStatusToRows,
  formatRelativeTime,
  interpretStreamEvent,
  normalizeStatus,
  shortAddress,
  workspaceBasename,
  type OverviewSession,
  type OpsStreamEnvelope,
} from './ops-api'

function row(overrides: Partial<OverviewSession> = {}): OverviewSession {
  return {
    sessionId: 'sid-1',
    workspacePath: '/Users/x/proj',
    agentAddress: 'proj::agent',
    active: false,
    agentStatus: null,
    heartbeatState: null,
    lastActivityAt: null,
    ...overrides,
  }
}

describe('normalizeStatus', () => {
  it('maps the daemon buckets to the working/idle/permission vocabulary', () => {
    expect(normalizeStatus('start')).toBe('working')
    expect(normalizeStatus('stop')).toBe('idle')
    expect(normalizeStatus('permission')).toBe('permission')
  })
  it('coerces an unknown bucket to idle (never renders an impossible badge)', () => {
    expect(normalizeStatus('garbage')).toBe('idle')
    expect(normalizeStatus('')).toBe('idle')
  })
})

describe('interpretStreamEvent', () => {
  const now = 1_000_000

  it('turns a session agent_status_changed into a precise status delta', () => {
    const env: OpsStreamEnvelope = {
      source: 'session',
      event: { kind: 'agent_status_changed', paneId: 'sid-7', tabId: 't', status: 'start' },
    }
    expect(interpretStreamEvent(env, now)).toEqual({
      type: 'status',
      sessionId: 'sid-7',
      status: 'working',
      at: now,
    })
  })

  it('asks for a coalesced refetch on structural / active-set changes', () => {
    for (const kind of ['session_added', 'session_removed', 'active_changed']) {
      const env: OpsStreamEnvelope = { source: 'session', event: { kind } }
      expect(interpretStreamEvent(env, now)).toEqual({ type: 'refetch' })
    }
  })

  it('ignores awareness signals and unknown session kinds', () => {
    expect(
      interpretStreamEvent({ source: 'awareness', event: { kind: 'whatever' } }, now),
    ).toEqual({ type: 'ignore' })
    expect(
      interpretStreamEvent({ source: 'session', event: { kind: 'tab_title_changed' } }, now),
    ).toEqual({ type: 'ignore' })
  })

  it('ignores a malformed status frame (missing paneId or status)', () => {
    expect(
      interpretStreamEvent({ source: 'session', event: { kind: 'agent_status_changed' } }, now),
    ).toEqual({ type: 'ignore' })
  })
})

describe('applyStatusToRows', () => {
  it('updates the matching row immutably and stamps lastActivityAt', () => {
    const rows = [row({ sessionId: 'a' }), row({ sessionId: 'b' })]
    const next = applyStatusToRows(rows, 'b', 'working', 555)
    expect(next).not.toBe(rows)
    expect(next[0]).toBe(rows[0]) // untouched row keeps identity
    expect(next[1]).toEqual(expect.objectContaining({ agentStatus: 'working', lastActivityAt: 555 }))
    // original is not mutated
    expect(rows[1].agentStatus).toBeNull()
  })

  it('returns the SAME array reference when nothing matched', () => {
    const rows = [row({ sessionId: 'a' })]
    expect(applyStatusToRows(rows, 'missing', 'idle', 1)).toBe(rows)
  })
})

describe('workspaceBasename', () => {
  it('returns the last path segment, tolerating trailing slashes', () => {
    expect(workspaceBasename('/Users/x/my-proj')).toBe('my-proj')
    expect(workspaceBasename('/Users/x/my-proj/')).toBe('my-proj')
  })
  it('handles empty / root input without throwing', () => {
    expect(workspaceBasename('')).toBe('(unknown)')
    expect(workspaceBasename('/')).toBe('(root)')
  })
})

describe('shortAddress', () => {
  it('takes the most specific trailing segment of a composite address', () => {
    expect(shortAddress('proj::agent')).toBe('agent')
    expect(shortAddress('/Users/x/proj')).toBe('proj')
    expect(shortAddress('')).toBe('—')
  })
})

describe('formatRelativeTime', () => {
  const now = 1_000_000
  it('renders coarse human buckets', () => {
    expect(formatRelativeTime(now, now)).toBe('just now')
    expect(formatRelativeTime(now - 10, now)).toBe('just now')
    expect(formatRelativeTime(now - 60, now)).toBe('1m ago')
    expect(formatRelativeTime(now - 120, now)).toBe('2m ago')
    expect(formatRelativeTime(now - 3600, now)).toBe('1h ago')
    expect(formatRelativeTime(now - 7200, now)).toBe('2h ago')
    expect(formatRelativeTime(now - 86400, now)).toBe('1d ago')
  })
  it('renders "—" for a null/absent timestamp', () => {
    expect(formatRelativeTime(null, now)).toBe('—')
    expect(formatRelativeTime(undefined, now)).toBe('—')
  })
  it('clamps a future timestamp (clock skew) to "just now"', () => {
    expect(formatRelativeTime(now + 500, now)).toBe('just now')
  })
})
