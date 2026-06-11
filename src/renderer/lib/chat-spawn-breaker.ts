// K2 #682 — pinned-chat spawn circuit breaker.
//
// The pinned Chat tab resolves a launch config and spawns `claude`. If the
// child exits almost immediately (e.g. `claude --session-id <uuid>` where the
// uuid is already in use → "Session ID … is already in use" → exit 1, or
// `claude --resume <uuid>` where the conversation doesn't exist → "No
// conversation found" → exit 1), the pane's remount machinery would otherwise
// spawn AGAIN, exit AGAIN, forever — piling up hundreds of `claude` processes
// (the bug observed: ~334 procs).
//
// This module is the SYSTEMIC DEFENSE: a pure, side-effect-free state machine
// that counts RAPID, REPEATED early exits and decides when to STOP
// auto-respawning and surface a manual-retry (refresh) state instead. A normal
// single exit (user typed `exit`, or a long-lived session that eventually
// ends) does NOT trip the breaker — only a tight loop of early failures does.
//
// Kept pure so the decision is unit-testable without rendering AgentChatPane
// or booting a daemon (see `chat-spawn-breaker.test.ts`).

/** A child exit exits "early" if it dies within this window of being spawned.
 *  Tuned so a genuinely-failing spawn (claude prints an error and exits in
 *  well under a second) trips, while a session the user actually used for a
 *  few seconds and then quit does not. */
export const EARLY_EXIT_WINDOW_MS = 5_000

/** Number of consecutive early exits that trips the breaker. The 1st and 2nd
 *  rapid failures still auto-respawn (covers transient daemon/PTY races); the
 *  Nth (default 3rd) stops the loop. */
export const MAX_RAPID_EXITS = 3

export interface BreakerState {
  /** Count of consecutive early exits since the last reset. */
  consecutiveEarlyExits: number
  /** Wall-clock ms when the current launch config was spawned, or null if no
   *  spawn is being tracked yet (initial/idle). */
  spawnedAt: number | null
  /** True once the breaker has tripped — the caller must STOP auto-respawning
   *  and show the error/idle state until a manual refresh resets it. */
  tripped: boolean
}

export function initialBreakerState(): BreakerState {
  return { consecutiveEarlyExits: 0, spawnedAt: null, tripped: false }
}

/** Record that a fresh launch config was spawned at `now`. Does not change the
 *  trip state or the running count — the count only resets on an explicit
 *  manual refresh (`resetBreaker`) so that back-to-back rapid spawn→exit
 *  cycles accumulate toward the limit. */
export function recordSpawn(state: BreakerState, now: number): BreakerState {
  return { ...state, spawnedAt: now }
}

export interface ExitDecision {
  next: BreakerState
  /** True when the caller should auto-respawn (kill+remount) in response to
   *  this exit. False when the breaker has tripped — show the error state and
   *  wait for a manual refresh. */
  shouldRespawn: boolean
  /** True only on the transition where this exit caused the breaker to trip
   *  (so the caller can log/announce once). */
  justTripped: boolean
}

/** Fold a child-exit event into the breaker.
 *
 *  - A NON-early exit (the child lived ≥ `EARLY_EXIT_WINDOW_MS`, or no spawn
 *    was tracked) is treated as a normal end-of-session: reset the rapid-exit
 *    count and DO NOT respawn (the pane shows its idle/dead state as today —
 *    the user typed `exit`).
 *  - An EARLY exit increments the consecutive count. While the count is below
 *    `maxRapidExits`, respawn. Once it reaches the limit, trip the breaker and
 *    stop respawning.
 *
 *  `exitCode` is accepted for completeness/logging but does not change the
 *  decision: ANY rapid repeated early exit is a loop regardless of code. */
export function recordExit(
  state: BreakerState,
  opts: {
    now: number
    exitCode: number | null
    earlyWindowMs?: number
    maxRapidExits?: number
  },
): ExitDecision {
  const earlyWindowMs = opts.earlyWindowMs ?? EARLY_EXIT_WINDOW_MS
  const maxRapidExits = opts.maxRapidExits ?? MAX_RAPID_EXITS

  // Already tripped — stay tripped, never respawn. (Defensive: the caller
  // should have stopped spawning, but a late/duplicate exit event must not
  // resurrect the loop.)
  if (state.tripped) {
    return {
      next: { ...state, spawnedAt: null },
      shouldRespawn: false,
      justTripped: false,
    }
  }

  const lived =
    state.spawnedAt === null ? Number.POSITIVE_INFINITY : opts.now - state.spawnedAt
  const isEarly = lived < earlyWindowMs

  if (!isEarly) {
    // Normal end-of-session — clear the rapid count, do not respawn.
    return {
      next: { consecutiveEarlyExits: 0, spawnedAt: null, tripped: false },
      shouldRespawn: false,
      justTripped: false,
    }
  }

  const consecutiveEarlyExits = state.consecutiveEarlyExits + 1
  const tripped = consecutiveEarlyExits >= maxRapidExits
  return {
    next: { consecutiveEarlyExits, spawnedAt: null, tripped },
    shouldRespawn: !tripped,
    justTripped: tripped,
  }
}

/** Reset the breaker for a manual retry (refresh button / user-initiated
 *  switch). Clears the trip state and the rapid-exit count. */
export function resetBreaker(): BreakerState {
  return initialBreakerState()
}

// ── Self-retrigger guard ───────────────────────────────────────────────
//
// The pinned-chat resolve effect stamps the session id it JUST launched
// onto the layout item; that flows back in as the `restoredSessionId`
// prop, changing a resolve dependency and RE-RUNNING the effect — which
// would spawn again. Combined with a child that exits immediately, that's
// the unbounded loop. This pure predicate decides whether a resolve re-run
// is that self-loop (and must be skipped) vs. a legitimate re-resolve
// (workspace switch / user refresh) that must proceed.

/** What a prior resolve recorded: the (refreshNonce, projectPath) it
 *  resolved FOR and the session id it resolved TO. */
export interface ResolveMemo {
  refreshNonce: number
  projectPath: string
  sessionId: string | null
}

/** True when a resolve re-run is purely the self-stamp echo — the
 *  refreshNonce + projectPath are unchanged and `restoredSessionId` now
 *  equals the session id we just resolved. Such a re-run must be a no-op.
 *
 *  Returns false (⇒ proceed) when there's no prior resolve, when the
 *  refresh nonce or workspace changed (a real re-resolve), or when the
 *  incoming session id differs from what we last resolved (a genuine
 *  external change, e.g. the dropdown switched sessions). */
export function isSelfRetrigger(
  prior: ResolveMemo | null,
  current: { refreshNonce: number; projectPath: string; restoredSessionId: string | undefined },
): boolean {
  if (!prior) return false
  return (
    prior.refreshNonce === current.refreshNonce &&
    prior.projectPath === current.projectPath &&
    prior.sessionId !== null &&
    prior.sessionId === current.restoredSessionId
  )
}
