// Unit tests for the Update-connected-host control (P4).
//
// The repo's vitest env is `node` (no jsdom / testing-library), so we lock
// the control's DECISION LOGIC + PHASE STATE MACHINE + COPY via the pure
// helpers the component renders through — the same pattern as
// restart-host.test.ts / server-capabilities.test.ts. This is exactly the
// unmistakable-remote-targeting requirement made testable:
//   - the control is REMOTE-ONLY (never shown for the local Mac),
//   - hidden on an older remote (serverSupports=false) and for Members,
//   - the confirm + phase copy NAME the active host explicitly,
//   - the phase state machine drives staged/terminal/failure transitions.
//
// The host-aware wire contract (check/start/apply = POST, status = GET,
// against the ACTIVE host) is locked separately in
// lib/host-aware-cli-contract.test.ts.

import { describe, it, expect } from 'vitest'
import {
  updateHostVisibility,
  updateHostConfirmCopy,
  updateAvailableCopy,
  updatePhaseCopy,
  updateForbiddenCopy,
  isForbiddenError,
  isStaged,
  isTerminalPhase,
  isFailurePhase,
  type UpdatePhase,
} from './update-host'

describe('updateHostVisibility — REMOTE-only gating (P4)', () => {
  it('is NEVER shown for the local Mac (no ambiguity with "update my Mac")', () => {
    expect(
      updateHostVisibility({ isRemote: false, supportsUpdate: true, role: 'owner' }),
    ).toEqual({ show: false, canUpdate: false })
  })

  it('is hidden on an older remote that lacks the routes (serverSupports=false)', () => {
    expect(
      updateHostVisibility({ isRemote: true, supportsUpdate: false, role: 'owner' }),
    ).toEqual({ show: false, canUpdate: false })
  })

  it('is hidden for a Member viewer (the owner-gated routes would 403)', () => {
    expect(
      updateHostVisibility({ isRemote: true, supportsUpdate: true, role: 'member' }),
    ).toEqual({ show: false, canUpdate: false })
  })

  it('is shown + ENABLED for an Owner on a supported remote', () => {
    expect(
      updateHostVisibility({ isRemote: true, supportsUpdate: true, role: 'owner' }),
    ).toEqual({ show: true, canUpdate: true })
  })

  it('is shown + ENABLED for an Admin on a supported remote', () => {
    expect(
      updateHostVisibility({ isRemote: true, supportsUpdate: true, role: 'admin' }),
    ).toEqual({ show: true, canUpdate: true })
  })

  it('is shown but DISABLED while the role is still unknown (whoami pending)', () => {
    expect(
      updateHostVisibility({ isRemote: true, supportsUpdate: true, role: null }),
    ).toEqual({ show: true, canUpdate: false })
  })
})

describe('phase state machine', () => {
  it('isStaged is true ONLY for the staged phase', () => {
    expect(isStaged('staged')).toBe(true)
    const others: UpdatePhase[] = [
      'downloading', 'verifying', 'applying', 'restarting', 'done', 'failed', 'rolled-back',
    ]
    for (const p of others) expect(isStaged(p)).toBe(false)
    expect(isStaged(null)).toBe(false)
  })

  it('isTerminalPhase stops polling on done/restarting/failed/rolled-back', () => {
    expect(isTerminalPhase('done')).toBe(true)
    expect(isTerminalPhase('restarting')).toBe(true)
    expect(isTerminalPhase('failed')).toBe(true)
    expect(isTerminalPhase('rolled-back')).toBe(true)
    // Mid-flight phases keep the poll loop running.
    expect(isTerminalPhase('downloading')).toBe(false)
    expect(isTerminalPhase('verifying')).toBe(false)
    expect(isTerminalPhase('staged')).toBe(false)
    expect(isTerminalPhase('applying')).toBe(false)
    expect(isTerminalPhase(null)).toBe(false)
  })

  it('isFailurePhase is true for failed + rolled-back only', () => {
    expect(isFailurePhase('failed')).toBe(true)
    expect(isFailurePhase('rolled-back')).toBe(true)
    expect(isFailurePhase('done')).toBe(false)
    expect(isFailurePhase('staged')).toBe(false)
    expect(isFailurePhase(null)).toBe(false)
  })
})

describe('updatePhaseCopy — host-named phase lines', () => {
  it('NAMES the host in every phase line', () => {
    const phases: UpdatePhase[] = [
      'downloading', 'verifying', 'staged', 'applying', 'restarting', 'done', 'failed', 'rolled-back',
    ]
    for (const p of phases) {
      expect(updatePhaseCopy(p, 'Hetzner box')).toContain('Hetzner box')
    }
  })

  it('threads download progress as a percentage', () => {
    expect(updatePhaseCopy('downloading', 'box', { progress: 42 })).toContain('42%')
    // No progress → no stray "(NaN%)"
    expect(updatePhaseCopy('downloading', 'box')).not.toContain('%')
  })

  it('frames failed/rolled-back as the host staying on its current version', () => {
    const failed = updatePhaseCopy('failed', 'box', { current: '0.39.30' })
    expect(failed).toContain('still on 0.39.30')
    const rolled = updatePhaseCopy('rolled-back', 'box', { current: '0.39.30' })
    expect(rolled).toContain('Update rolled back')
    expect(rolled).toContain('still on 0.39.30')
  })

  it('restarting copy promises auto-reconnect', () => {
    expect(updatePhaseCopy('restarting', 'box')).toContain('reconnect automatically')
  })
})

describe('updateAvailableCopy — current → latest banner', () => {
  it('NAMES the host and shows the version delta', () => {
    const copy = updateAvailableCopy('Hetzner box', '0.39.30', '0.39.33')
    expect(copy).toContain('Hetzner box')
    expect(copy).toContain('0.39.30')
    expect(copy).toContain('0.39.33')
  })
})

describe('updateHostConfirmCopy — unmistakable remote targeting (P4)', () => {
  it('NAMES the host + version in the title, message, and confirm button', () => {
    const copy = updateHostConfirmCopy('Hetzner box', 'rosson.k2.dev', '0.39.33')
    expect(copy.title).toContain('Hetzner box')
    expect(copy.title).toContain('0.39.33')
    expect(copy.confirmLabel).toContain('Hetzner box')
    expect(copy.message).toContain('Hetzner box')
    // The hostname is shown too, framed as the REMOTE machine — NOT this Mac.
    expect(copy.message).toContain('rosson.k2.dev')
    expect(copy.message).toContain('REMOTE')
    expect(copy.message).toContain('not this Mac')
    expect(copy.message).toContain('0.39.33')
  })

  it('falls back to the hostname when used as the label (never blank copy)', () => {
    const copy = updateHostConfirmCopy('rosson.k2.dev', 'rosson.k2.dev', '0.39.33')
    expect(copy.title).toBe('Update rosson.k2.dev to 0.39.33?')
    expect(copy.confirmLabel).toBe('Install & restart rosson.k2.dev')
  })
})

describe('403 handling', () => {
  it('updateForbiddenCopy NAMES the host + says owner/admin only', () => {
    const copy = updateForbiddenCopy('Hetzner box')
    expect(copy).toContain('Hetzner box')
    expect(copy).toMatch(/owner|admin/i)
  })

  it('isForbiddenError matches 403 / forbidden / token messages', () => {
    expect(isForbiddenError('daemon returned 403')).toBe(true)
    expect(isForbiddenError('Forbidden')).toBe(true)
    expect(isForbiddenError('invalid or missing token')).toBe(true)
    // A plain network error is NOT a permission failure.
    expect(isForbiddenError('Load failed')).toBe(false)
  })
})
