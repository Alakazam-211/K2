// Thread push: quiet while the person is looking
// (prd-thread-push-notifications-v1 §6.4, TP16, TP26; slice S4).
//
// While a Thread pane is mounted, shown, and its window is visible and
// focused, the desktop tells the daemon on the Thread's overlay socket:
//
//   {"t":"viewing","visible":true}    at once, then every 30 s
//   {"t":"viewing","visible":false}   on blur, hide or unmount
//
// Frames are fire-and-forget (no ack). The daemon keeps each as state with
// a short TTL and checks it when a push is about to go out, so the
// person's phone stays quiet for THAT Thread while they read it here.
// A new socket starts with no state on the daemon (a closing socket
// clears its entries), so a socket that opens while viewing says so again.
//
// Only servers that report `thread-push-v1` get these frames; an older
// overlay socket has no use for them.

/** How often a still-viewing pane repeats `visible:true` (TP26: the
 *  daemon's TTL is longer than this). */
export const VIEWING_REPEAT_MS = 30_000

/** The text frame the overlay socket takes. */
export function viewingFrame(visible: boolean): string {
  return JSON.stringify({ t: 'viewing', visible })
}

export interface ViewingSender {
  /** Re-read `isViewing()` and send a frame if the answer changed
   *  (focus, blur, visibility, the pane shown or hidden). */
  update(): void
  /** A (new) socket is open: if viewing, say so on it now. */
  socketOpened(): void
  /** Unmount / teardown: `visible:false` if we had said true; stop the
   *  repeat. Idempotent. */
  dispose(): void
}

/**
 * `send` writes one text frame to the current overlay socket and returns
 * whether it went out (false: no open socket; the next open re-sends).
 * `isViewing` is the whole rule: pane shown, window visible and focused,
 * server reports the feature.
 */
export function createViewingSender(opts: {
  send: (frame: string) => boolean
  isViewing: () => boolean
  repeatMs?: number
}): ViewingSender {
  const repeatMs = opts.repeatMs ?? VIEWING_REPEAT_MS
  /** What we last told the daemon (or would have, had a socket been open). */
  let claimed = false
  let timer: ReturnType<typeof setInterval> | null = null
  let disposed = false

  function stopRepeat(): void {
    if (timer !== null) {
      clearInterval(timer)
      timer = null
    }
  }

  function startRepeat(): void {
    if (timer !== null) return
    timer = setInterval(() => {
      if (disposed) return
      // A missed blur event (e.g. the OS took focus) is caught here.
      if (!opts.isViewing()) {
        apply(false)
        return
      }
      opts.send(viewingFrame(true))
    }, repeatMs)
  }

  function apply(next: boolean): void {
    if (next === claimed) return
    claimed = next
    if (next) {
      opts.send(viewingFrame(true))
      startRepeat()
    } else {
      stopRepeat()
      opts.send(viewingFrame(false))
    }
  }

  return {
    update(): void {
      if (disposed) return
      apply(opts.isViewing())
    },
    socketOpened(): void {
      if (disposed) return
      const now = opts.isViewing()
      if (now && claimed) {
        // The new socket has no state on the daemon: say it again.
        opts.send(viewingFrame(true))
        return
      }
      apply(now)
    },
    dispose(): void {
      if (disposed) return
      if (claimed) {
        claimed = false
        opts.send(viewingFrame(false))
      }
      stopRepeat()
      disposed = true
    },
  }
}

/** The window half of the rule: the document is visible and focused. */
export function windowIsLookedAt(): boolean {
  if (typeof document === 'undefined') return false
  if (document.visibilityState !== 'visible') return false
  return typeof document.hasFocus === 'function' ? document.hasFocus() : false
}
