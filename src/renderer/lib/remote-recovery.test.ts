// Unit tests for the pure three-state recovery reducer (owner contract):
//
//   1. 'reconnecting'     — host down OR reachable-but-not-ready (never a
//                           login prompt; carries the boot phase)
//   2. 'reauthenticating' — host ready, session rejected, silent re-login
//   3. 'signin-required'  — the re-login itself failed; the only user state
//
// Boot state DOMINATES auth state; the schedule backs off while
// reconnecting and holds the health cadence otherwise.

import { describe, it, expect } from 'vitest'
import {
  deriveRecovery,
  authSignalFromRevive,
  recoveryPollMs,
  recoveryStatusText,
  RECOVERY_HEALTH_POLL_MS,
  RECOVERY_RECONNECT_MAX_MS,
} from './remote-recovery'

describe('deriveRecovery — boot state dominates', () => {
  it('unreachable host → reconnecting (no boot phase), REGARDLESS of auth', () => {
    for (const auth of ['ok', 'unknown', 'rejected', 'reauth-failed'] as const) {
      expect(deriveRecovery({ bootStatus: { reachable: false }, auth })).toEqual({
        kind: 'reconnecting',
        bootPhase: null,
      })
    }
  })

  it("reachable but pre-'ready' → reconnecting WITH the boot phase, never a login prompt", () => {
    for (const auth of ['ok', 'rejected', 'reauth-failed'] as const) {
      expect(
        deriveRecovery({ bootStatus: { reachable: true, phase: 'migrating' }, auth }),
      ).toEqual({ kind: 'reconnecting', bootPhase: 'migrating' })
    }
    expect(
      deriveRecovery({ bootStatus: { reachable: true, phase: 'starting' }, auth: 'unknown' }),
    ).toEqual({ kind: 'reconnecting', bootPhase: 'starting' })
  })
})

describe('deriveRecovery — ready host resolves by auth signal', () => {
  const ready = { reachable: true, phase: 'ready' } as const

  it("session ok → connected; a blip ('unknown') is NOT an alarm", () => {
    expect(deriveRecovery({ bootStatus: ready, auth: 'ok' })).toEqual({ kind: 'connected' })
    expect(deriveRecovery({ bootStatus: ready, auth: 'unknown' })).toEqual({ kind: 'connected' })
  })

  it('session rejected → reauthenticating (silent re-login, no user action)', () => {
    expect(deriveRecovery({ bootStatus: ready, auth: 'rejected' })).toEqual({
      kind: 'reauthenticating',
    })
  })

  it('re-login itself failed → signin-required (the ONLY user state)', () => {
    expect(deriveRecovery({ bootStatus: ready, auth: 'reauth-failed' })).toEqual({
      kind: 'signin-required',
    })
  })
})

describe('authSignalFromRevive — folding revival outcomes', () => {
  it('a fresh token or a confirmed-alive session is ok', () => {
    expect(authSignalFromRevive('revived')).toBe('ok')
    expect(authSignalFromRevive('still-valid')).toBe('ok')
    expect(authSignalFromRevive('not-applicable')).toBe('ok')
  })

  it('signin-required maps to reauth-failed', () => {
    expect(authSignalFromRevive('signin-required')).toBe('reauth-failed')
  })

  it('unreachable/cooldown keep re-auth pending (rejected)', () => {
    expect(authSignalFromRevive('unreachable')).toBe('rejected')
    expect(authSignalFromRevive('cooldown')).toBe('rejected')
  })
})

describe('recoveryPollMs — the retry schedule', () => {
  it('reconnecting backs off exponentially from 500ms to the 8s cap', () => {
    const r = { kind: 'reconnecting', bootPhase: null } as const
    expect(recoveryPollMs(r, 0)).toBe(500)
    expect(recoveryPollMs(r, 1)).toBe(1000)
    expect(recoveryPollMs(r, 2)).toBe(2000)
    expect(recoveryPollMs(r, 4)).toBe(8000)
    expect(recoveryPollMs(r, 5)).toBe(RECOVERY_RECONNECT_MAX_MS)
    expect(recoveryPollMs(r, 50)).toBe(RECOVERY_RECONNECT_MAX_MS) // capped, never wraps
  })

  it('connected / reauthenticating hold the normal health cadence', () => {
    expect(recoveryPollMs({ kind: 'connected' }, 3)).toBe(RECOVERY_HEALTH_POLL_MS)
    expect(recoveryPollMs({ kind: 'reauthenticating' }, 3)).toBe(RECOVERY_HEALTH_POLL_MS)
    expect(recoveryPollMs({ kind: 'signin-required' }, 3)).toBe(RECOVERY_HEALTH_POLL_MS)
  })
})

describe('recoveryStatusText — the shared user-facing copy', () => {
  it('reconnecting carries the boot phase when known (the "still booting" affordance)', () => {
    expect(recoveryStatusText('rpm', { kind: 'reconnecting', bootPhase: 'migrating' })).toBe(
      'rpm is restarting (migrating)… reconnecting automatically',
    )
    expect(recoveryStatusText('rpm', { kind: 'reconnecting', bootPhase: null })).toBe(
      'Reconnecting to rpm… retrying automatically',
    )
  })

  it('reauthenticating and signin-required each name their state', () => {
    expect(recoveryStatusText('rpm', { kind: 'reauthenticating' })).toBe(
      'Re-authenticating with rpm…',
    )
    expect(recoveryStatusText('rpm', { kind: 'signin-required' })).toBe(
      'rpm needs you to sign in again.',
    )
    expect(recoveryStatusText('rpm', { kind: 'connected' })).toBe('')
  })
})
