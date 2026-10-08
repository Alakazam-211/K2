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
//   - a forwarded chord goes to `onChord` with whether this frame has focus;
//     the page's one gate (`createZenChordGate`, zen-shortcut.ts) acts on it
//     only while that frame has focus, at most once every 500 ms (UW33).
// Stopping removes only that frame; the rest of the Garden keeps running.

import type { ZenFrameHello, ZenFrameReply } from './zen-custom-types'
import type { ZenCustomLayer } from './zen-custom-bridge'
import type { ZenWidgetStopReason } from './zen-custom-run'

export const ZEN_FRAME_READY_MS = 10_000
export const ZEN_FRAME_PING_MS = 5_000
export const ZEN_FRAME_MISSED_PINGS = 3
export const ZEN_FRAME_ERRORS_PER_MINUTE = 20
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
  /** A forwarded chord (still to be gated: known chord, focus, 500 ms). */
  onChord(chord: unknown, frameFocused: boolean): void
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
    if ('chord' in m) {
      opts.onChord(m.chord, opts.focused())
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
