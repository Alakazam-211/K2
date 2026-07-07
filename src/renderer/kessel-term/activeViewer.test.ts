import { beforeEach, describe, expect, it } from 'vitest'

import {
  __resetActiveViewerDedupForTests,
  CLAIM_INTERACTION_WINDOW_MS,
  computeDesiredActive,
  getLastSentActive,
  recordSentActive,
  shouldEmitResize,
  shouldHoldGridWs,
  shouldSendClaim,
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
  // Full truth table including the pinned-chat retention exemption
  // (`retainWhileHidden`). Invariants pinned here:
  //   - `retainWhileHidden` OMITTED ⇒ byte-identical to the pre-retention
  //     predicate (visible AND !exited) — every non-chat consumer passes
  //     nothing and must see zero behavior change.
  //   - `retainWhileHidden: true` lifts ONLY the visibility gate; `exited`
  //     always wins (a dead child is never streamed, retained or not).
  const cases: Array<{
    visible: boolean
    exited: boolean
    retainWhileHidden?: boolean
    expected: boolean
  }> = [
    // Legacy 2-input table (retainWhileHidden omitted).
    { visible: false, exited: false, expected: false },
    { visible: false, exited: true, expected: false },
    { visible: true, exited: false, expected: true },
    { visible: true, exited: true, expected: false },
    // retainWhileHidden: false — explicit false ≡ omitted.
    { visible: false, exited: false, retainWhileHidden: false, expected: false },
    { visible: false, exited: true, retainWhileHidden: false, expected: false },
    { visible: true, exited: false, retainWhileHidden: false, expected: true },
    { visible: true, exited: true, retainWhileHidden: false, expected: false },
    // retainWhileHidden: true — the exemption. Hidden+alive now streams;
    // exited still never does.
    { visible: false, exited: false, retainWhileHidden: true, expected: true },
    { visible: false, exited: true, retainWhileHidden: true, expected: false },
    { visible: true, exited: false, retainWhileHidden: true, expected: true },
    { visible: true, exited: true, retainWhileHidden: true, expected: false },
  ]

  for (const c of cases) {
    it(`visible=${c.visible} exited=${c.exited} retain=${String(c.retainWhileHidden)} -> ${c.expected}`, () => {
      expect(
        shouldHoldGridWs({
          visible: c.visible,
          exited: c.exited,
          ...(c.retainWhileHidden === undefined
            ? {}
            : { retainWhileHidden: c.retainWhileHidden }),
        }),
      ).toBe(c.expected)
    })
  }

  it('without the exemption, holds only for a visible, not-yet-exited pane', () => {
    const trueCount = cases.filter(
      (c) => c.expected && c.retainWhileHidden !== true,
    ).length
    expect(trueCount).toBe(2) // the omitted + explicit-false visible/alive rows
  })

  it('a hidden non-retained pane never holds a grid-WS regardless of exit state', () => {
    expect(shouldHoldGridWs({ visible: false, exited: false })).toBe(false)
    expect(shouldHoldGridWs({ visible: false, exited: true })).toBe(false)
  })

  it('retention never resurrects an exited session', () => {
    expect(shouldHoldGridWs({ visible: false, exited: true, retainWhileHidden: true })).toBe(false)
    expect(shouldHoldGridWs({ visible: true, exited: true, retainWhileHidden: true })).toBe(false)
  })

  it('the active-viewer claim predicate is unaffected by retention (hidden panes never claim)', () => {
    // `retainWhileHidden` is a GridWsInputs-only input; computeDesiredActive
    // has no such field, so a retained hidden pane still computes
    // desired=false and releases/never claims the active slot.
    expect(
      computeDesiredActive({ visible: false, paneFocused: true, windowFocused: true }),
    ).toBe(false)
  })
})

// Pinned-chat retention follow-up — resize emission predicate. The
// invariant pinned here (owner decision, workspace-switch zoom fix): a
// BACKGROUND pane never emits resize — switching AWAY from a workspace
// sends NOTHING size-related, the session just keeps its last-visited
// dims on the daemon until the next foreground claim re-sizes it. The
// old window-focus-only gate broke this: a retained pane parked in the
// hidden host sent its off-screen geometry — and because that pane had
// just RELEASED its active claim (active_subscriber == 0), the daemon's
// first-resize-wins rule ACCEPTED the resize: the PTY reflowed to
// hidden dims on every workspace switch-away and back on switch-in
// (the "zoom on workspace switch" bug).
describe('shouldEmitResize', () => {
  const cases: Array<{
    visible: boolean
    windowFocused: boolean
    expected: boolean
  }> = [
    { visible: false, windowFocused: false, expected: false },
    { visible: false, windowFocused: true, expected: false },
    { visible: true, windowFocused: false, expected: false },
    { visible: true, windowFocused: true, expected: true },
  ]

  for (const c of cases) {
    it(`visible=${c.visible} windowFocused=${c.windowFocused} -> ${c.expected}`, () => {
      expect(
        shouldEmitResize({
          visible: c.visible,
          windowFocused: c.windowFocused,
        }),
      ).toBe(c.expected)
    })
  }

  it('a hidden pane never emits — even in a focused window (the retained-background case)', () => {
    // This is the exact workspace-switch state: window focused, pane
    // just re-parented into the hidden host. One accepted resize here
    // reflows the PTY to off-screen geometry.
    expect(shouldEmitResize({ visible: false, windowFocused: true })).toBe(false)
  })

  it('a visible pane in a focused window emits WITHOUT holding the active claim', () => {
    // Deliberately weaker than computeDesiredActive (no paneFocused
    // input): a visible-but-unclaimed pane's first resize is how a
    // fresh session gets sized (daemon first-resize-wins).
    expect(shouldEmitResize({ visible: true, windowFocused: true })).toBe(true)
    expect(
      computeDesiredActive({
        visible: true,
        paneFocused: false,
        windowFocused: true,
      }),
    ).toBe(false)
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

// Multiplayer resurface-steal fix — deliberate-interaction claim gate.
// `computeDesiredActive` conflated SELECTION with CONTROL: every
// ambient recompute (OS window focus regain, tab show, programmatic
// refocus, plain mount) re-sent `set_active:true` and the daemon is
// most-recent-claim-wins — so resurfacing the K2 app RECLAIMED the
// session from whoever took over while you were away. `shouldSendClaim`
// pins the new rule: releases are always sendable; claims only when
// attributable to a deliberate interaction or a reconnect restore of a
// claim this client already held.
describe('shouldSendClaim', () => {
  const NOW = 100_000 // a long-running app: performance.now() is large

  describe('releases (desired=false) are always sendable', () => {
    it('regardless of reason, interaction recency, or restorability', () => {
      const reasons = ['ambient', 'interaction', 'restore'] as const
      for (const reason of reasons) {
        for (const lastInteractionAt of [Number.NEGATIVE_INFINITY, 0, NOW]) {
          for (const restorable of [undefined, false, true]) {
            expect(
              shouldSendClaim({
                desired: false,
                reason,
                lastInteractionAt,
                now: NOW,
                restorable,
              }),
            ).toBe(true)
          }
        }
      }
    })
  })

  describe('ambient claims', () => {
    it('BLOCKED with no interaction ever (the resurface-steal case)', () => {
      // The exact bug: user re-focuses the K2 window hours later; the
      // isFocused-effect recompute is ambient with a stale (or never
      // set) interaction stamp. The claim must NOT go out — whoever
      // controls the session keeps it.
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'ambient',
          lastInteractionAt: Number.NEGATIVE_INFINITY,
          now: NOW,
        }),
      ).toBe(false)
    })

    it('BLOCKED with an ancient interaction stamp (lastInteractionAt=0)', () => {
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'ambient',
          lastInteractionAt: 0,
          now: NOW,
        }),
      ).toBe(false)
    })

    it('BLOCKED just outside the interaction window', () => {
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'ambient',
          lastInteractionAt: NOW - CLAIM_INTERACTION_WINDOW_MS - 1,
          now: NOW,
        }),
      ).toBe(false)
    })

    it('sendable within the interaction window (pointerdown → async focus → effect chain)', () => {
      // pointerdown stamps, then focus event → React state → effect
      // recompute fires a few ticks later as 'ambient' — it must be
      // attributed to that click and allowed to claim.
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'ambient',
          lastInteractionAt: NOW - 5,
          now: NOW,
        }),
      ).toBe(true)
      // Boundary: exactly the window edge is still attributable.
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'ambient',
          lastInteractionAt: NOW - CLAIM_INTERACTION_WINDOW_MS,
          now: NOW,
        }),
      ).toBe(true)
    })
  })

  describe('interaction claims', () => {
    it('always sendable (typing / mode-flip-to-claimer recompute)', () => {
      // The stamp may even be stale relative to `now` — the reason
      // itself asserts deliberateness; the window only exists for the
      // ambient recomputes that TRAIL an interaction.
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'interaction',
          lastInteractionAt: Number.NEGATIVE_INFINITY,
          now: NOW,
        }),
      ).toBe(true)
    })
  })

  describe('restore claims (WS reconnect)', () => {
    it('sendable ONLY when this client already held the claim (restorable)', () => {
      // Network blip while we were the active controller: the fresh
      // daemon subscriber must be re-primed or we'd be silently demoted.
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'restore',
          lastInteractionAt: Number.NEGATIVE_INFINITY,
          now: NOW,
          restorable: true,
        }),
      ).toBe(true)
    })

    it('BLOCKED when not restorable — a passive pane must not promote itself on reconnect', () => {
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'restore',
          lastInteractionAt: Number.NEGATIVE_INFINITY,
          now: NOW,
          restorable: false,
        }),
      ).toBe(false)
      // Omitted restorable (no recorded last-sent, e.g. fresh session
      // whose first connect races a recompute) is NOT restorable.
      expect(
        shouldSendClaim({
          desired: true,
          reason: 'restore',
          lastInteractionAt: Number.NEGATIVE_INFINITY,
          now: NOW,
        }),
      ).toBe(false)
    })

    it('composes with the cross-remount map: recorded true ⇒ restorable, recorded false/absent ⇒ not', () => {
      // Mirrors the TerminalPane wiring: `restorable =
      // getLastSentActive(sessionId) === true` at the reconnect
      // re-prime. Pin the exact map-derived truth here.
      __resetActiveViewerDedupForTests()
      const SID = 'session-reconnect'
      const attempt = () =>
        shouldSendClaim({
          desired: true,
          reason: 'restore',
          lastInteractionAt: Number.NEGATIVE_INFINITY,
          now: NOW,
          restorable: getLastSentActive(SID) === true,
        })
      // Never sent anything → not restorable.
      expect(attempt()).toBe(false)
      // We held the claim before the blip → restorable.
      recordSentActive(SID, true)
      expect(attempt()).toBe(true)
      // We had released before the blip → not restorable.
      recordSentActive(SID, false)
      expect(attempt()).toBe(false)
    })
  })
})
