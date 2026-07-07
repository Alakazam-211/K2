// rAF frame-coalescing policy — extracted from TerminalPane so the
// frame-budget tests can pin it (pi-mono study learning B2: K2 had
// the batching mechanism but nothing asserting it in CI).
//
// WS snapshot/delta messages queue here and apply once per animation
// frame: one apply (→ one setSnapshot) per display refresh, however
// many messages arrived. A queued full snapshot supersedes everything
// before it (the batch's merge starts from the snapshot, so earlier
// frames are dead weight). The size cap flushes synchronously if rAF
// is starved (occluded window) so the queue can't grow unbounded.
// A timer backstop covers the low-volume starvation case the cap
// can't: macOS pauses rAF entirely for occluded windows, so a small
// burst (e.g. a takeover's rescale frames) would otherwise sit
// unapplied — and unacked, wedging the daemon's ack-gated pacing.
//
// This module owns POLICY only — queue contents and when to flush.
// The scheduler (requestAnimationFrame / cancelAnimationFrame) and
// the applier (merge into the live grid, setSnapshot, k1 ack) are
// injected by TerminalPane, which keeps this pure and testable in
// vitest's node environment. Behavior must stay byte-equivalent to
// the pre-extraction inline version (0.39.x): any change here is a
// change to the renderer's frame pacing.

/** Frames the coalescer understands: full snapshots supersede, all
 *  other kinds accumulate. Matches TerminalPane's `PendingFrame`
 *  shape without importing its (component-local) payload types. */
export interface CoalescableFrame {
  kind: 'snapshot' | 'delta'
}

/** Queue depth at which a flush runs synchronously instead of
 *  waiting for the (starved) scheduled callback. 60 ≈ one second of
 *  daemon frames at the 16ms emitter floor. */
export const STARVATION_FLUSH_CAP = 60

/** Backstop delay for the setTimeout armed alongside every scheduled
 *  flush. Backgrounded WKWebViews pause rAF but only throttle timers
 *  (to ~1s ticks), so queued frames still apply — and acks still flow
 *  — while the window is occluded. 350ms is invisible in the
 *  foreground (rAF wins by two orders of magnitude) and comfortably
 *  under the throttled-timer floor. */
export const BACKSTOP_FLUSH_MS = 350

export interface FrameCoalescer<F extends CoalescableFrame> {
  /** Queue one frame; schedules a flush (or runs one synchronously
   *  at the starvation cap). */
  enqueue(frame: F): void
  /** Drain the queue into one `apply` call. Invoked by the scheduled
   *  callback and the timer backstop; exposed for teardown-free
   *  forced flushes and tests. No-op when the queue is empty. */
  flush(): void
  /** Synchronously apply all pending frames and clear both scheduled
   *  handles (rAF + backstop). No-op when the queue is empty. For
   *  event-driven resyncs (window became visible / focused). */
  flushNow(): void
  /** Teardown: cancel any scheduled flush and drop queued frames
   *  without applying them (unmount — nobody left to render them). */
  clear(): void
  /** Current queue depth (diagnostics + tests). */
  pendingCount(): number
}

export function createFrameCoalescer<F extends CoalescableFrame>(opts: {
  /** Schedule `flush` for the next paint (production:
   *  requestAnimationFrame). Returns a cancellation handle. */
  schedule: (flush: () => void) => number
  /** Cancel a scheduled flush (production: cancelAnimationFrame). */
  cancel: (id: number) => void
  /** Apply one drained batch, in arrival order. Called exactly once
   *  per flush that had frames — this is the "one setSnapshot per
   *  display refresh" guarantee. */
  apply: (batch: F[]) => void
}): FrameCoalescer<F> {
  const { schedule, cancel, apply } = opts
  let pending: F[] = []
  let scheduled: number | null = null
  let backstop: ReturnType<typeof setTimeout> | null = null

  const flush = (): void => {
    // Whichever path ran (rAF, backstop, cap, or manual): neutralize
    // BOTH handles so nothing double-fires and the next enqueue
    // reschedules cleanly — a rAF that WebKit cancelled at occlusion
    // time (fires never, clears never) can't strand the queue.
    if (scheduled !== null) {
      cancel(scheduled)
      scheduled = null
    }
    if (backstop !== null) {
      clearTimeout(backstop)
      backstop = null
    }
    if (pending.length === 0) return
    const batch = pending
    pending = []
    apply(batch)
  }

  const enqueue = (frame: F): void => {
    // A full snapshot replaces the client mirror wholesale — every
    // frame queued before it could only be merged and then discarded,
    // so drop them now (this also resets the starvation counter: the
    // queue can only hit the cap on a genuine un-flushed backlog).
    if (frame.kind === 'snapshot') pending.length = 0
    pending.push(frame)
    if (pending.length >= STARVATION_FLUSH_CAP) {
      flush()
      return
    }
    if (scheduled === null) {
      scheduled = schedule(() => {
        scheduled = null
        flush()
      })
    }
    // Armed independently of the scheduler handle: whichever fires
    // first flushes and neutralizes the other (see flush).
    if (backstop === null) {
      backstop = setTimeout(() => {
        backstop = null
        flush()
      }, BACKSTOP_FLUSH_MS)
    }
  }

  const clear = (): void => {
    if (scheduled !== null) {
      cancel(scheduled)
      scheduled = null
    }
    if (backstop !== null) {
      clearTimeout(backstop)
      backstop = null
    }
    pending = []
  }

  return {
    enqueue,
    flush,
    flushNow: flush,
    clear,
    pendingCount: () => pending.length,
  }
}
