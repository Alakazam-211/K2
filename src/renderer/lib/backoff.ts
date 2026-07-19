// Shared backoff jitter (0.40.48 connection resilience).
//
// THE BUG THIS FIXES
// Every retry loop in the renderer (three session-events WS reconnectors,
// ConnectionGate's health poll, withRemoteRetry, the revive backoff) used
// FIXED delays. After a remote reboot they all fail at the same instant and
// then retry in lockstep forever — the live incident measured ~11 req/s of
// synchronized retries against the tunnel edge. Jitter decorrelates the
// loops so the aggregate load spreads out instead of spiking.
//
// Two flavors:
//   - fullJitter(ms)      — AWS-style "full jitter": uniform in [0, ms].
//     Maximum decorrelation; use where an immediate retry is acceptable.
//   - jittered(ms, f)     — bounded jitter: uniform in [ms·(1−f), ms].
//     Keeps a floor so a backoff schedule still eases off (a reconnect
//     delay never collapses to ~0), and never EXCEEDS the base, so caps
//     (e.g. recoveryPollMs's 8s ceiling) still hold.
//
// Both are pure functions of Math.random(); schedules that feed them
// (recoveryPollMs, DEFAULT_REMOTE_RETRY_DELAYS_MS, REVIVE_BACKOFF_MS)
// stay deterministic and unit-tested — jitter is applied at the call
// site, on the already-computed base delay.

/** Uniform random delay in [0, ms]. `fullJitter(0)` is 0. */
export function fullJitter(ms: number): number {
  if (ms <= 0) return 0
  return Math.floor(Math.random() * (ms + 1))
}

/**
 * Bounded jitter: uniform random delay in [ms·(1−factor), ms].
 * The default factor 0.5 gives [ms/2, ms] — enough spread to break
 * retry-loop lockstep while preserving the schedule's easing/caps.
 * `jittered(0)` is 0, so "immediate first retry" semantics survive.
 */
export function jittered(ms: number, factor = 0.5): number {
  if (ms <= 0) return 0
  const f = Math.min(Math.max(factor, 0), 1)
  const floor = ms * (1 - f)
  return Math.floor(floor + Math.random() * (ms - floor + 1))
}
