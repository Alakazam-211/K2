// The viewing lease (prd-home-chat-pushes-v1 HC-15): while a conversation
// is open, on screen and the window has focus, the client tells the daemon
// so on `WS /cli/home-chat/events`, and the daemon holds that person's
// phone pushes for it. Text frames:
//
//   {"t":"viewing","conv":"<sew id>","visible":true}   every 30 s while so
//   {"t":"viewing","conv":"<sew id>","visible":false}  at once when it stops
//
// The daemon keeps each lease 90 s and drops it when the socket closes, so a
// reconnected socket gets the current lease again at once. This only quiets
// pushes: `POST /cli/home-chat/view` keeps joining channels and typing.
//
// One lease per server per window (`stores/home-chat.ts` keeps it beside
// the events socket). Pure: the timers and the send are given.

/** How often a held lease is sent again (the daemon keeps one 90 s). */
export const VIEWING_LEASE_REPEAT_MS = 30_000

/** One `viewing` text frame. */
export function viewingFrame(conv: string, visible: boolean): string {
  return JSON.stringify({ t: 'viewing', conv, visible })
}

export interface ViewingLeaseDeps {
  /** Send a text frame on the events socket; false when it isn't open. */
  send: (text: string) => boolean
  setInterval: (fn: () => void, ms: number) => unknown
  clearInterval: (t: unknown) => void
}

export interface ViewingLease {
  /** The conversation now viewed (null: none). A change ends the last one
   *  at once (`visible:false`) and starts the new one. */
  set: (conv: string | null) => void
  /** The conversation the lease is for now, if any. */
  current: () => string | null
  /** The socket (re)opened: send the current lease again now. */
  linked: () => void
  /** Ends the lease (`visible:false`) and its timer. */
  stop: () => void
}

export function createViewingLease(deps: ViewingLeaseDeps): ViewingLease {
  let conv: string | null = null
  let timer: unknown = null

  const stopTimer = (): void => {
    if (timer !== null) deps.clearInterval(timer)
    timer = null
  }
  const hold = (): void => {
    stopTimer()
    if (conv === null) return
    const c = conv
    deps.send(viewingFrame(c, true))
    timer = deps.setInterval(() => {
      if (conv === c) deps.send(viewingFrame(c, true))
    }, VIEWING_LEASE_REPEAT_MS)
  }

  return {
    set: (next) => {
      if (next === conv) return
      if (conv !== null) {
        stopTimer()
        deps.send(viewingFrame(conv, false))
      }
      conv = next
      hold()
    },
    current: () => conv,
    linked: () => hold(),
    stop: () => {
      stopTimer()
      if (conv !== null) deps.send(viewingFrame(conv, false))
      conv = null
    },
  }
}
