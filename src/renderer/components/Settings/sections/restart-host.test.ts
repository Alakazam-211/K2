// Unit tests for the Restart-connected-host control (#661).
//
// The repo's vitest env is `node` (no jsdom / testing-library), so we lock
// the control's DECISION LOGIC + COPY via the pure helpers the component
// renders through — the same pattern as server-capabilities.test.ts. This
// is exactly the unmistakable-remote-targeting requirement made testable:
//   - the control is REMOTE-ONLY (never shown for the local Mac),
//   - hidden on an older remote (serverSupports=false) and for Members,
//   - the confirm copy NAMES the active host explicitly.
//
// The host-aware wire contract (button → daemonCliPost('daemon/restart',
// {}) against the ACTIVE host) is locked separately in
// lib/host-aware-cli-contract.test.ts.

import { describe, it, expect } from 'vitest'
import {
  restartHostVisibility,
  restartHostConfirmCopy,
} from './restart-host'

describe('restartHostVisibility — REMOTE-only gating (#661)', () => {
  it('is NEVER shown for the local Mac (no ambiguity with "restart my Mac")', () => {
    // Even with everything else green, local active host hides the control.
    expect(
      restartHostVisibility({ isRemote: false, supportsRestart: true, role: 'owner' }),
    ).toEqual({ show: false, canRestart: false })
  })

  it('is hidden on an older remote that lacks the route (serverSupports=false)', () => {
    expect(
      restartHostVisibility({ isRemote: true, supportsRestart: false, role: 'owner' }),
    ).toEqual({ show: false, canRestart: false })
  })

  it('is hidden for a Member viewer (the owner-gated route would 403)', () => {
    expect(
      restartHostVisibility({ isRemote: true, supportsRestart: true, role: 'member' }),
    ).toEqual({ show: false, canRestart: false })
  })

  it('is shown + ENABLED for an Owner on a supported remote', () => {
    expect(
      restartHostVisibility({ isRemote: true, supportsRestart: true, role: 'owner' }),
    ).toEqual({ show: true, canRestart: true })
  })

  it('is shown + ENABLED for an Admin on a supported remote', () => {
    expect(
      restartHostVisibility({ isRemote: true, supportsRestart: true, role: 'admin' }),
    ).toEqual({ show: true, canRestart: true })
  })

  it('is shown but DISABLED while the role is still unknown (whoami pending)', () => {
    expect(
      restartHostVisibility({ isRemote: true, supportsRestart: true, role: null }),
    ).toEqual({ show: true, canRestart: false })
  })
})

describe('restartHostConfirmCopy — unmistakable remote targeting (#661)', () => {
  it('NAMES the active host in the title, message, and confirm button', () => {
    const copy = restartHostConfirmCopy('Hetzner box', 'rosson.k2.dev')
    expect(copy.title).toContain('Hetzner box')
    expect(copy.confirmLabel).toContain('Hetzner box')
    expect(copy.message).toContain('Hetzner box')
    // The hostname is shown too, and it's framed as the REMOTE machine —
    // explicitly NOT this Mac.
    expect(copy.message).toContain('rosson.k2.dev')
    expect(copy.message).toContain('REMOTE')
    expect(copy.message).toContain('not this Mac')
  })

  it('falls back to the hostname when used as the label (never blank copy)', () => {
    const copy = restartHostConfirmCopy('rosson.k2.dev', 'rosson.k2.dev')
    expect(copy.title).toBe('Restart rosson.k2.dev?')
    expect(copy.confirmLabel).toBe('Restart rosson.k2.dev')
  })
})
