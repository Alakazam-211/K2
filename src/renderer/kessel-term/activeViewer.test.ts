import { beforeEach, describe, expect, it } from 'vitest'

import {
  __resetActiveViewerDedupForTests,
  computeDesiredActive,
  getLastSentActive,
  recordSentActive,
  shouldHoldGridWs,
  shouldSkipRemountReclaim,
} from './activeViewer'

// Exhaustive truth table for the Issue #8 active-viewer predicate.
// The active claim must be true ONLY when all three signals hold;
// any one being false means this pane is not the live viewer and
// must release (send `set_active:false`).
describe('computeDesiredActive', () => {
  const cases: Array<{
    visible: boolean
    paneFocused: boolean
    windowFocused: boolean
    expected: boolean
  }> = [
    { visible: false, paneFocused: false, windowFocused: false, expected: false },
    { visible: false, paneFocused: false, windowFocused: true, expected: false },
    { visible: false, paneFocused: true, windowFocused: false, expected: false },
    { visible: false, paneFocused: true, windowFocused: true, expected: false },
    { visible: true, paneFocused: false, windowFocused: false, expected: false },
    { visible: true, paneFocused: false, windowFocused: true, expected: false },
    { visible: true, paneFocused: true, windowFocused: false, expected: false },
    { visible: true, paneFocused: true, windowFocused: true, expected: true },
  ]

  for (const c of cases) {
    it(`visible=${c.visible} paneFocused=${c.paneFocused} windowFocused=${c.windowFocused} -> ${c.expected}`, () => {
      expect(
        computeDesiredActive({
          visible: c.visible,
          paneFocused: c.paneFocused,
          windowFocused: c.windowFocused,
        }),
      ).toBe(c.expected)
    })
  }

  it('only the all-true combination claims active', () => {
    const trueCount = cases.filter((c) => c.expected).length
    expect(trueCount).toBe(1)
  })
})

// Issue #8 (0.39.13) — grid-WS hold predicate. A pane streams the
// session's live grid ONLY while visible AND its child hasn't exited.
// Hidden background tabs / off-screen heartbeat spawns hold no grid-WS
// (their daemon PTY survives untouched); an exited session has nothing
// left to stream.
describe('shouldHoldGridWs', () => {
  const cases: Array<{
    visible: boolean
    exited: boolean
    expected: boolean
  }> = [
    { visible: false, exited: false, expected: false },
    { visible: false, exited: true, expected: false },
    { visible: true, exited: false, expected: true },
    { visible: true, exited: true, expected: false },
  ]

  for (const c of cases) {
    it(`visible=${c.visible} exited=${c.exited} -> ${c.expected}`, () => {
      expect(shouldHoldGridWs({ visible: c.visible, exited: c.exited })).toBe(
        c.expected,
      )
    })
  }

  it('holds the grid-WS only for a visible, not-yet-exited pane', () => {
    const trueCount = cases.filter((c) => c.expected).length
    expect(trueCount).toBe(1)
  })

  it('a hidden pane never holds a grid-WS regardless of exit state', () => {
    expect(shouldHoldGridWs({ visible: false, exited: false })).toBe(false)
    expect(shouldHoldGridWs({ visible: false, exited: true })).toBe(false)
  })
})

// 0.39.43 (PRD `daemon-multi-client-arbitration.md` Issue A) —
// cross-remount active-claim dedup. The per-component `lastSentActiveRef`
// resets on every mount, so a BARE re-mount (AgentChatPane bumping
// `attachNonce`) used to re-fire `set_active:true` even with unchanged
// focus, letting the local window re-steal the daemon's active slot from
// a remote viewer. The per-session map persists the last-sent value
// across re-mounts so a re-mount with unchanged inputs does NOT re-claim,
// while a genuine focus transition still does.
describe('cross-remount active-claim dedup', () => {
  const SID = 'session-abc'

  beforeEach(() => {
    __resetActiveViewerDedupForTests()
  })

  it('a fresh session has no recorded value (must claim on first focus)', () => {
    // No prior send → undefined → never skips → the first instance
    // always emits its computed claim/release.
    expect(getLastSentActive(SID)).toBeUndefined()
    expect(shouldSkipRemountReclaim(SID, true)).toBe(false)
    expect(shouldSkipRemountReclaim(SID, false)).toBe(false)
  })

  it('a bare re-mount with UNCHANGED focus does NOT re-claim', () => {
    // Instance 1: visible+focused → claims active:true, records it.
    const desired1 = computeDesiredActive({
      visible: true,
      paneFocused: true,
      windowFocused: true,
    })
    expect(desired1).toBe(true)
    recordSentActive(SID, desired1)

    // Instance 2 (a bare re-mount — attachNonce bump): focus inputs are
    // identical, so it computes the SAME desired value. The cross-remount
    // guard must suppress the redundant re-claim (no second set_active:true
    // on the wire → the local window does not re-steal the active slot).
    const desired2 = computeDesiredActive({
      visible: true,
      paneFocused: true,
      windowFocused: true,
    })
    expect(shouldSkipRemountReclaim(SID, desired2)).toBe(true)
  })

  it('a GENUINE focus change after a re-mount DOES re-emit', () => {
    // Instance 1 claimed active.
    recordSentActive(SID, true)
    // Instance 2 re-mounts but is now blurred/hidden → desired flips to
    // false. The guard must NOT skip — the release must reach the wire.
    const desiredAfterBlur = computeDesiredActive({
      visible: true,
      paneFocused: true,
      windowFocused: false,
    })
    expect(desiredAfterBlur).toBe(false)
    expect(shouldSkipRemountReclaim(SID, desiredAfterBlur)).toBe(false)

    // Symmetric: if instance 1 had released (false) and instance 2 gains
    // focus (true), the claim must go out.
    recordSentActive(SID, false)
    expect(shouldSkipRemountReclaim(SID, true)).toBe(false)
  })

  it('dedup is keyed per-session — a different session is independent', () => {
    recordSentActive(SID, true)
    // A different PTY (real session switch) has its own (empty) window.
    expect(shouldSkipRemountReclaim('session-xyz', true)).toBe(false)
    // The original session still dedups.
    expect(shouldSkipRemountReclaim(SID, true)).toBe(true)
  })

  it('recordSentActive overwrites with the latest genuine decision', () => {
    recordSentActive(SID, true)
    expect(getLastSentActive(SID)).toBe(true)
    recordSentActive(SID, false)
    expect(getLastSentActive(SID)).toBe(false)
    // A re-mount that computes `false` now skips; one computing `true`
    // re-claims (genuine transition).
    expect(shouldSkipRemountReclaim(SID, false)).toBe(true)
    expect(shouldSkipRemountReclaim(SID, true)).toBe(false)
  })
})
