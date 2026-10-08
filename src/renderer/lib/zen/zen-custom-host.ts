// prd-zen-user-widgets-v2 UW15, UW30, UW33 — the host end of one sealed
// widget frame: its private port, ready / ping / unload, errors, chords.
//
// On the frame's `load`, K2 makes a `MessageChannel` and posts
// `{k2: "hello", v: 1, …}` with one port to that frame's window. All
// traffic then uses that port, which belongs to this one placement. The
// host never listens to `message` events on its own window, so no other
// frame (or the widget pretending to be another) can speak to it.
//   - no `ready` within 10 s → stopped "didn't start";
//   - after ready, a ping every 5 s; three missed in a row → "stopped
//     responding" (a frozen frame blocks the window's main thread, so the
//     last resort is the shell's per-window watchdog, UW32, B3's);
//   - uncaught errors and rejections are reported; 20 in a minute →
//     "keeps failing";
//   - a forwarded chord acts only while that frame has focus, at most once
//     every 500 ms (UW33). What a chord does is the chord handler's
//     (`setZenFrameChordHandler`; B3 wires K2's own keys).
// Stopping removes only that frame; the rest of the Garden keeps running.

import type { ZenFrameHello, ZenFrameReply } from './zen-custom-types'
import type { ZenCustomLayer } from './zen-custom-bridge'
import type { ZenWidgetStopReason } from './zen-custom-run'

export const ZEN_FRAME_READY_MS = 10_000
export const ZEN_FRAME_PING_MS = 5_000
export const ZEN_FRAME_MISSED_PINGS = 3
export const ZEN_FRAME_ERRORS_PER_MINUTE = 20
export const ZEN_FRAME_CHORD_GAP_MS = 500

/** The chords a frame may forward (the shim's fixed set). */
export const ZEN_FRAME_CHORDS: ReadonlySet<string> = new Set([
  'zen-exit',
  'theme-next',
  'theme-prev',
  ...Array.from({ length: 9 }, (_, i) => `garden-${i + 1}`),
])

type ChordHandler = (chord: string) => void
let chordHandler: ChordHandler | null = null

/** B3 (UW33): what a forwarded chord does. Returns the unregister. */
export function setZenFrameChordHandler(fn: ChordHandler): () => void {
  chordHandler = fn
  return () => {
    if (chordHandler === fn) chordHandler = null
  }
}

export function zenFrameChordHandler(): ChordHandler | null {
  return chordHandler
}

/** The window a hello goes to (an iframe's `contentWindow`). */
export interface ZenFrameWindow {
  postMessage(message: unknown, targetOrigin: string, transfer: Transferable[]): void
}

export interface ZenFrameHostOptions {
  hello: ZenFrameHello
  /** The custom layer, given the function that pushes to this frame. */
  makeLayer(push: (sub: number, value: unknown) => void): ZenCustomLayer
  stop(reason: ZenWidgetStopReason): void
  /** Does this frame have keyboard focus now? */
  focused(): boolean
  /** The widget's name, for logs. */
  label: string
  now?: () => number
  channel?: () => MessageChannel
}

export interface ZenFrameHost {
  /** Ready seen (tests). */
  readonly ready: boolean
  dispose(): void
}

/** Open the frame's port and run its ready / ping / error rules. */
export function startZenFrameHost(frame: ZenFrameWindow, opts: ZenFrameHostOptions): ZenFrameHost {
  const now = opts.now ?? (() => Date.now())
  const ch = opts.channel ? opts.channel() : new MessageChannel()
  const port = ch.port1
  let disposed = false
  let ready = false
  let pingSeq = 0
  let answered = 0
  let missed = 0
  let lastChord = -Infinity
  const errors: number[] = []
  let pingTimer: ReturnType<typeof setInterval> | null = null

  const post = (msg: ZenFrameReply): void => {
    if (!disposed) port.postMessage(msg)
  }
  const layer = opts.makeLayer((sub, value) => post({ sub, value }))

  const dispose = (): void => {
    if (disposed) return
    disposed = true
    clearTimeout(readyTimer)
    if (pingTimer !== null) clearInterval(pingTimer)
    port.onmessage = null
    port.close()
    layer.dispose()
  }
  const stop = (reason: ZenWidgetStopReason): void => {
    if (disposed) return
    dispose()
    opts.stop(reason)
  }

  const readyTimer = setTimeout(() => {
    if (!ready) stop('not-started')
  }, ZEN_FRAME_READY_MS)

  const ping = (): void => {
    if (pingSeq > answered) missed += 1
    else missed = 0
    if (missed >= ZEN_FRAME_MISSED_PINGS) {
      stop('not-responding')
      return
    }
    pingSeq += 1
    post({ ping: pingSeq })
  }

  port.onmessage = (e: MessageEvent) => {
    if (disposed) return
    const m = e.data as Record<string, unknown> | null
    if (!m || typeof m !== 'object') return
    if (m.ready === true) {
      if (ready) return
      ready = true
      clearTimeout(readyTimer)
      pingTimer = setInterval(ping, ZEN_FRAME_PING_MS)
      return
    }
    if (typeof m.pong === 'number') {
      if (m.pong > answered && m.pong <= pingSeq) answered = m.pong
      missed = 0
      return
    }
    if ('error' in m && m.error && typeof m.error === 'object') {
      const t = now()
      errors.push(t)
      while (errors.length > 0 && t - errors[0] > 60_000) errors.shift()
      const msg = (m.error as { message?: unknown }).message
      console.warn(`[zen] widget ${opts.label}: ${typeof msg === 'string' ? msg.slice(0, 300) : 'error'}`)
      if (errors.length >= ZEN_FRAME_ERRORS_PER_MINUTE) stop('failing')
      return
    }
    if (typeof m.chord === 'string') {
      const t = now()
      if (!ZEN_FRAME_CHORDS.has(m.chord) || !opts.focused() || t - lastChord < ZEN_FRAME_CHORD_GAP_MS) return
      lastChord = t
      chordHandler?.(m.chord)
      return
    }
    void layer.handle(m).then((reply) => {
      if (reply) post(reply)
    })
  }

  // The hello carries the frame's end of the channel; nothing else does.
  frame.postMessage(opts.hello, '*', [ch.port2])

  return {
    get ready() {
      return ready
    },
    dispose,
  }
}
