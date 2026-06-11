// K2 #682 — pinned-chat spawn circuit breaker unit tests.
//
// The breaker converts an unbounded spawn→exit→respawn loop (the ~334
// `claude` procs bug) into a bounded N attempts + a manual-retry error
// state. These tests pin the pure decision logic so the safety net can't
// regress without a red test.

import { describe, it, expect } from 'vitest'
import {
  initialBreakerState,
  recordSpawn,
  recordExit,
  resetBreaker,
  isSelfRetrigger,
  EARLY_EXIT_WINDOW_MS,
  MAX_RAPID_EXITS,
  type ResolveMemo,
} from './chat-spawn-breaker'

describe('chat-spawn-breaker', () => {
  it('does NOT trip on a single early exit', () => {
    let s = initialBreakerState()
    s = recordSpawn(s, 1_000)
    const d = recordExit(s, { now: 1_100, exitCode: 1 })
    expect(d.next.tripped).toBe(false)
    expect(d.shouldRespawn).toBe(true)
    expect(d.justTripped).toBe(false)
    expect(d.next.consecutiveEarlyExits).toBe(1)
  })

  it('TRIPS after MAX_RAPID_EXITS consecutive rapid early exits and stops respawning', () => {
    let s = initialBreakerState()
    let lastDecision = recordExit(recordSpawn(s, 0), { now: 50, exitCode: 1 })
    s = lastDecision.next

    // Drive exactly up to the limit.
    for (let i = 1; i < MAX_RAPID_EXITS; i++) {
      const spawnAt = i * 1_000
      s = recordSpawn(s, spawnAt)
      lastDecision = recordExit(s, { now: spawnAt + 100, exitCode: 1 })
      s = lastDecision.next
    }

    // The MAX_RAPID_EXITS-th early exit trips the breaker.
    expect(s.tripped).toBe(true)
    expect(lastDecision.justTripped).toBe(true)
    expect(lastDecision.shouldRespawn).toBe(false)
    expect(s.consecutiveEarlyExits).toBe(MAX_RAPID_EXITS)
  })

  it('justTripped fires exactly once (the transition), not on later exits', () => {
    let s = initialBreakerState()
    let d = recordExit(recordSpawn(s, 0), { now: 10, exitCode: 1 })
    s = d.next
    for (let i = 1; i < MAX_RAPID_EXITS; i++) {
      const at = i * 1_000
      d = recordExit(recordSpawn(s, at), { now: at + 10, exitCode: 1 })
      s = d.next
    }
    expect(d.justTripped).toBe(true)

    // A further (late/duplicate) exit while tripped: never respawn, never
    // re-announce.
    const after = recordExit(recordSpawn(s, 10_000), { now: 10_010, exitCode: 1 })
    expect(after.justTripped).toBe(false)
    expect(after.shouldRespawn).toBe(false)
    expect(after.next.tripped).toBe(true)
  })

  it('a NON-early exit (long-lived session, e.g. user typed exit) resets the count and never trips', () => {
    let s = initialBreakerState()
    // Two rapid early failures…
    s = recordExit(recordSpawn(s, 0), { now: 100, exitCode: 1 }).next
    s = recordExit(recordSpawn(s, 1_000), { now: 1_100, exitCode: 1 }).next
    expect(s.consecutiveEarlyExits).toBe(2)

    // …then a session the user actually used for a while and quit.
    s = recordSpawn(s, 2_000)
    const d = recordExit(s, { now: 2_000 + EARLY_EXIT_WINDOW_MS + 1, exitCode: 0 })
    expect(d.shouldRespawn).toBe(false) // normal end-of-session, no respawn
    expect(d.next.tripped).toBe(false)
    expect(d.next.consecutiveEarlyExits).toBe(0) // count reset
  })

  it('a non-early exit between early exits prevents tripping (not CONSECUTIVE)', () => {
    let s = initialBreakerState()
    for (let i = 0; i < MAX_RAPID_EXITS - 1; i++) {
      const at = i * 10_000
      s = recordExit(recordSpawn(s, at), { now: at + 50, exitCode: 1 }).next
    }
    // A healthy long-lived session resets the streak.
    const longAt = 100_000
    s = recordExit(recordSpawn(s, longAt), {
      now: longAt + EARLY_EXIT_WINDOW_MS + 1,
      exitCode: 0,
    }).next
    expect(s.consecutiveEarlyExits).toBe(0)
    // One more early exit must NOT trip — the streak was broken.
    const d = recordExit(recordSpawn(s, 200_000), { now: 200_050, exitCode: 1 })
    expect(d.next.tripped).toBe(false)
  })

  it('resetBreaker clears the trip and the count (manual refresh)', () => {
    let s = initialBreakerState()
    let d = recordExit(recordSpawn(s, 0), { now: 10, exitCode: 1 })
    s = d.next
    for (let i = 1; i < MAX_RAPID_EXITS; i++) {
      const at = i * 1_000
      d = recordExit(recordSpawn(s, at), { now: at + 10, exitCode: 1 })
      s = d.next
    }
    expect(s.tripped).toBe(true)

    const reset = resetBreaker()
    expect(reset.tripped).toBe(false)
    expect(reset.consecutiveEarlyExits).toBe(0)
    expect(reset.spawnedAt).toBe(null)

    // After reset, an early exit gets a fresh budget (back to count 1).
    const post = recordExit(recordSpawn(reset, 0), { now: 50, exitCode: 1 })
    expect(post.next.consecutiveEarlyExits).toBe(1)
    expect(post.next.tripped).toBe(false)
    expect(post.shouldRespawn).toBe(true)
  })

  it('an exit with no tracked spawn (spawnedAt null) is treated as non-early', () => {
    const s = initialBreakerState()
    const d = recordExit(s, { now: 5_000, exitCode: 1 })
    expect(d.shouldRespawn).toBe(false)
    expect(d.next.tripped).toBe(false)
    expect(d.next.consecutiveEarlyExits).toBe(0)
  })

  it('honors custom thresholds', () => {
    let s = initialBreakerState()
    const opts = { exitCode: 1, earlyWindowMs: 1_000, maxRapidExits: 2 }
    s = recordExit(recordSpawn(s, 0), { now: 100, ...opts }).next
    expect(s.tripped).toBe(false)
    const d = recordExit(recordSpawn(s, 2_000), { now: 2_100, ...opts })
    expect(d.next.tripped).toBe(true)
    expect(d.justTripped).toBe(true)
  })
})

describe('isSelfRetrigger — the resolve self-loop guard (K2 #682)', () => {
  const memo = (over: Partial<ResolveMemo> = {}): ResolveMemo => ({
    refreshNonce: 0,
    projectPath: '/work/proj',
    sessionId: 'sid-A',
    ...over,
  })

  it('SKIPS when the only change is restoredSessionId becoming the session we just resolved', () => {
    // resolve launched sid-A and stamped it; it comes back as the prop.
    expect(
      isSelfRetrigger(memo({ sessionId: 'sid-A' }), {
        refreshNonce: 0,
        projectPath: '/work/proj',
        restoredSessionId: 'sid-A',
      }),
    ).toBe(true)
  })

  it('PROCEEDS on a user refresh (refreshNonce changed) even if the session matches', () => {
    expect(
      isSelfRetrigger(memo({ refreshNonce: 0, sessionId: 'sid-A' }), {
        refreshNonce: 1,
        projectPath: '/work/proj',
        restoredSessionId: 'sid-A',
      }),
    ).toBe(false)
  })

  it('PROCEEDS on a workspace switch (projectPath changed)', () => {
    expect(
      isSelfRetrigger(memo({ projectPath: '/work/proj', sessionId: 'sid-A' }), {
        refreshNonce: 0,
        projectPath: '/work/other',
        restoredSessionId: 'sid-A',
      }),
    ).toBe(false)
  })

  it('PROCEEDS when restoredSessionId is a genuinely new session (dropdown switch)', () => {
    expect(
      isSelfRetrigger(memo({ sessionId: 'sid-A' }), {
        refreshNonce: 0,
        projectPath: '/work/proj',
        restoredSessionId: 'sid-B',
      }),
    ).toBe(false)
  })

  it('PROCEEDS when there is no prior resolve (first run)', () => {
    expect(
      isSelfRetrigger(null, {
        refreshNonce: 0,
        projectPath: '/work/proj',
        restoredSessionId: 'sid-A',
      }),
    ).toBe(false)
  })

  it('PROCEEDS when the prior resolve produced no session id (null)', () => {
    // e.g. a reattach/fallback path that didn't stamp a session — a later
    // restoredSessionId arriving is a real change, not a self-echo.
    expect(
      isSelfRetrigger(memo({ sessionId: null }), {
        refreshNonce: 0,
        projectPath: '/work/proj',
        restoredSessionId: 'sid-A',
      }),
    ).toBe(false)
  })
})
