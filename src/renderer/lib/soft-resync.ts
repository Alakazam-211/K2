// Soft-resync bus (remote terminal paint recovery after connect blips).
//
// Control-plane recovery and the terminal grid-WS are decoupled: soft health
// accept can clear the banner while a half-open grid socket never fires
// `onclose` and freezes on the last frame. This module is the pure fan-out
// bus TerminalPane consumers subscribe to so they can grid-only tear down +
// re-attach (fresh k1 snapshot) without spawn re-POST.
//
// Deliberately store-free so `connect-host.setRecovery` can emit without
// import cycles.

/** Why a soft-resync was requested. v1 musts: recovery edge into connected,
 *  and session-events reopen after a drop while recovery stayed connected. */
export type SoftResyncReason = 'recovery-connected' | 'events-reopen' | (string & {})

const subscribers = new Set<(reason: SoftResyncReason) => void>()

/** Coalesce window: multiple emits of the same reason within this window
 *  collapse to one fan-out (recovery-connected + events-reopen still both fire). */
const COALESCE_MS = 50
let coalesceTimer: ReturnType<typeof setTimeout> | null = null
let pendingReasons = new Set<SoftResyncReason>()

/**
 * Pure edge detector for recovery → soft-resync.
 * True iff remote + kind actually entered `connected` (not connected→connected).
 *
 * `acceptedOnce` is effect-local in ConnectionGate and is intentionally not
 * required here: first-connect is already connected baseline so setRecovery
 * dedupes; only a real heal edge (reconnecting/etc → connected) emits.
 */
export function shouldEmitSoftResync(
  prev: { kind: string },
  next: { kind: string },
  opts: { isRemote: boolean },
): boolean {
  return opts.isRemote && prev.kind !== 'connected' && next.kind === 'connected'
}

/** Subscribe to soft-resync emits. Returns unsubscribe (safe to call twice). */
export function subscribeSoftResync(
  cb: (reason: SoftResyncReason) => void,
): () => void {
  subscribers.add(cb)
  return () => {
    subscribers.delete(cb)
  }
}

function flushPending(): void {
  coalesceTimer = null
  const reasons = pendingReasons
  pendingReasons = new Set()
  for (const reason of reasons) {
    for (const cb of subscribers) {
      try {
        cb(reason)
      } catch (err) {
        // Fail loud in the console; keep fan-out so one pane can't block peers.
        console.error('[soft-resync] subscriber threw:', err)
      }
    }
  }
}

/**
 * Fan-out a soft-resync to all subscribers. Safe with zero subscribers.
 * Same-reason emits within ~50ms coalesce to one delivery; distinct reasons
 * in the same window all flush once the timer fires.
 */
export function emitSoftResync(reason: SoftResyncReason): void {
  pendingReasons.add(reason)
  if (coalesceTimer !== null) return
  coalesceTimer = setTimeout(flushPending, COALESCE_MS)
}

/** Test / host-switch helper: drop pending coalesced emits without firing. */
export function resetSoftResyncBus(): void {
  if (coalesceTimer !== null) {
    clearTimeout(coalesceTimer)
    coalesceTimer = null
  }
  pendingReasons = new Set()
  subscribers.clear()
}

/** Exported for tests. */
export const SOFT_RESYNC_COALESCE_MS = COALESCE_MS
