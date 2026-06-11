// P1.C — Active-Bar rule 2 ("recently interacted") now honors a
// user-configurable window (settings.activeWindowHours, default 24, min 1)
// instead of a hard-coded 24h. `isWithinActiveWindow` is the extracted,
// pure predicate; this test pins the cutoff behavior at a small window.
//
// ActiveBar.tsx pulls in several stores at import time; mock the load-time
// boundaries so importing the module is inert in the node vitest env.

import { describe, it, expect, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))

import { isWithinActiveWindow, hasEnabledHeartbeat } from './ActiveBar'

describe('P1.C — isWithinActiveWindow (Active-Bar rule 2)', () => {
  // All values in unix SECONDS (matching ActiveBar's clock).
  const now = 1_000_000

  it('returns false when there is no lastInteractionAt', () => {
    expect(isWithinActiveWindow(null, now, 24)).toBe(false)
    expect(isWithinActiveWindow(undefined, now, 24)).toBe(false)
  })

  it('respects a SMALL configured window (1 hour)', () => {
    const oneHour = 60 * 60
    // 30 minutes ago — inside a 1h window.
    expect(isWithinActiveWindow(now - oneHour / 2, now, 1)).toBe(true)
    // 90 minutes ago — outside a 1h window.
    expect(isWithinActiveWindow(now - oneHour * 1.5, now, 1)).toBe(false)
  })

  it('respects a LARGER configured window (48 hours)', () => {
    const hour = 60 * 60
    // 36h ago — outside the default 24h, but inside a 48h window.
    expect(isWithinActiveWindow(now - hour * 36, now, 48)).toBe(true)
    // 60h ago — outside even the 48h window.
    expect(isWithinActiveWindow(now - hour * 60, now, 48)).toBe(false)
  })

  it('clamps sub-1 / non-finite windows to a 1h floor', () => {
    const hour = 60 * 60
    // windowHours 0 → clamped to 1h. 30min ago is in, 90min ago is out.
    expect(isWithinActiveWindow(now - hour / 2, now, 0)).toBe(true)
    expect(isWithinActiveWindow(now - hour * 2, now, 0)).toBe(false)
    expect(isWithinActiveWindow(now - hour / 2, now, NaN)).toBe(true)
  })
})

describe('EKG badge — hasEnabledHeartbeat (config-flag semantics)', () => {
  // Config-flag (2026-06-06): the EKG icon means "has ≥1 enabled heartbeat",
  // i.e. CAN self-drive — NOT "self-driving right now". It does NOT depend on
  // the Active window, recency of a fire, or whether the user is driving.
  it('shows whenever a heartbeat is enabled (heartbeatEnabled !== 0)', () => {
    expect(hasEnabledHeartbeat(1)).toBe(true)
  })

  it('does NOT show when no heartbeat is enabled', () => {
    expect(hasEnabledHeartbeat(0)).toBe(false)
  })

  it('is independent of recency — shows even for a long-idle workspace', () => {
    // No window/lastInteractionAt input at all: a stale-but-enabled
    // workspace still reads as "can self-drive".
    expect(hasEnabledHeartbeat(1)).toBe(true)
  })
})
