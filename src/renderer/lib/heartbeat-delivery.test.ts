// Heartbeat delivery drop-down — pure-helper coverage. The load-bearing
// invariant lives in `selectableSessions`: the session bound to the
// workspace's pinned chat must NEVER be offered as a normal row (it is
// reachable only through the "Pinned chat" entry).

import { describe, it, expect, beforeEach, vi } from 'vitest'

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}))

import {
  applyDeliveryTarget,
  deriveDeliveryTarget,
  selectableSessions,
  setHeartbeatSession,
  type HeartbeatSessionCandidate,
} from './heartbeat-delivery'

function candidate(overrides: Partial<HeartbeatSessionCandidate>): HeartbeatSessionCandidate {
  return {
    sessionId: 'sid',
    title: 'A chat',
    timestamp: 1,
    messageCount: 3,
    provider: 'claude',
    ...overrides,
  }
}

describe('selectableSessions — pinned-chat exclusion', () => {
  it('never offers the pinned chat session as a normal row', () => {
    const rows = [
      candidate({ sessionId: 'aaa', timestamp: 3 }),
      candidate({ sessionId: 'pinned-id', timestamp: 2 }),
      candidate({ sessionId: 'bbb', timestamp: 1 }),
    ]
    const out = selectableSessions(rows, 'pinned-id')
    expect(out.map((r) => r.sessionId)).toEqual(['aaa', 'bbb'])
  })

  it('keeps every row (newest first) when no pinned session exists', () => {
    const rows = [
      candidate({ sessionId: 'old', timestamp: 1 }),
      candidate({ sessionId: 'new', timestamp: 9 }),
    ]
    expect(selectableSessions(rows, null).map((r) => r.sessionId)).toEqual(['new', 'old'])
  })
})

describe('deriveDeliveryTarget', () => {
  it('pinned wins regardless of saved-session columns', () => {
    expect(
      deriveDeliveryTarget({ useWorkspaceSession: true, lastSessionId: 'x', sessionProvider: 'pi' }),
    ).toEqual({ mode: 'pinned' })
  })

  it('explicit session = id + provider both stamped', () => {
    expect(
      deriveDeliveryTarget({ useWorkspaceSession: false, lastSessionId: 'x', sessionProvider: 'codex' }),
    ).toEqual({ mode: 'session', sessionId: 'x', provider: 'codex' })
  })

  it('auto-stamped lastSessionId WITHOUT a provider stays "own session"', () => {
    expect(
      deriveDeliveryTarget({ useWorkspaceSession: false, lastSessionId: 'x', sessionProvider: null }),
    ).toEqual({ mode: 'auto' })
    // Rows from older daemons omit the key entirely.
    expect(
      deriveDeliveryTarget({ useWorkspaceSession: false, lastSessionId: 'x' }),
    ).toEqual({ mode: 'auto' })
  })
})

describe('applyDeliveryTarget — optimistic row mirror', () => {
  const row = { useWorkspaceSession: false, lastSessionId: 'x', sessionProvider: 'pi', name: 'daily' }

  it('pinned flips the flag and leaves the saved session untouched', () => {
    expect(applyDeliveryTarget(row, { mode: 'pinned' })).toEqual({ ...row, useWorkspaceSession: true })
  })

  it('auto clears the saved session', () => {
    expect(applyDeliveryTarget(row, { mode: 'auto' })).toEqual({
      ...row,
      lastSessionId: null,
      sessionProvider: null,
    })
  })

  it('session pins id + provider', () => {
    expect(applyDeliveryTarget(row, { mode: 'session', sessionId: 'y', provider: 'codex' })).toEqual({
      ...row,
      lastSessionId: 'y',
      sessionProvider: 'codex',
    })
  })
})

describe('setHeartbeatSession — thin bridge contract', () => {
  beforeEach(() => invoke.mockReset())

  it('sends mode-only for pinned/auto and rides session_id+provider for session', async () => {
    invoke.mockResolvedValue('{"success":true}')
    await setHeartbeatSession('/w', 'daily', { mode: 'pinned' })
    expect(invoke).toHaveBeenCalledWith('k2so_heartbeat_set_session', {
      projectPath: '/w',
      name: 'daily',
      mode: 'pinned',
      sessionId: null,
      provider: null,
    })
    await setHeartbeatSession('/w', 'daily', { mode: 'session', sessionId: 's1', provider: 'pi' })
    expect(invoke).toHaveBeenLastCalledWith('k2so_heartbeat_set_session', {
      projectPath: '/w',
      name: 'daily',
      mode: 'session',
      sessionId: 's1',
      provider: 'pi',
    })
  })

  it('raises the daemon {"error":…} body so callers can revert', async () => {
    invoke.mockResolvedValue('{"error":"heartbeat not found"}')
    await expect(setHeartbeatSession('/w', 'daily', { mode: 'auto' })).rejects.toThrow(
      'heartbeat not found',
    )
  })
})
