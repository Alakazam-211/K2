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
//   - uncaught errors and rejections are reported (console, and `onReport`
//     to the daemon's runtime log for `k2 zen widget errors`); 20 in a minute →
//     "keeps failing";
//   - a `{pointer: "down" | "up"}` (the runtime's report of a trusted
//     primary-button press, for `k2.window.startDrag`) goes to `onPointer`,
//     before anything that follows it on the port;
//   - a forwarded chord goes to `onChord` with whether this frame has focus;
//     the page's one gate (`createZenChordGate`, zen-shortcut.ts) acts on it
//     only while that frame has focus, at most once every 500 ms (UW33).
// Stopping removes only that frame; the rest of the Garden keeps running.

import type { ZenFrameHello, ZenFrameReply } from './zen-custom-types'
import type { CatalogWireError } from '../contract/catalog-types'
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
  makeLayer(push: (sub: number, value: unknown) => void, pushError: (sub: number, error: CatalogWireError) => void): ZenCustomLayer
  stop(reason: ZenWidgetStopReason): void
  /** Does this frame have keyboard focus now? */
  focused(): boolean
  /** A forwarded chord (still to be gated: known chord, focus, 500 ms). */
  onChord(chord: unknown, frameFocused: boolean): void
  /** A trusted primary-button press went down (true) or up in the frame. */
  onPointer?(down: boolean): void
  /** The frame's first `ready` and every uncaught error it reports, for
   *  the daemon's runtime log (decision-k2-runtime-v1 stage 0 (d)): agents
   *  read them with `k2 zen widget errors`. Never throws into the host. */
  onReport?(report: ZenFrameReport): void
  /** The widget's name, for logs. */
  label: string
  now?: () => number
  channel?: () => MessageChannel
}

/** What the host passes to `onReport`. A `refusal` is a bridge refusal
 *  the frame got (Garden v3, GV49): its verb and fixed code, never text. */
export type ZenFrameReport =
  | { event: 'ready' }
  | { event: 'error'; message: string; stack?: string }
  | { event: 'refusal'; verb: string; code: string; address?: string }

/** The address a refusal of a `sources.*` verb names (its `room`), for the
 *  daemon's log; the daemon keeps it only when a placement config names it
 *  (0.46.1 S3). Agent verbs never send theirs. */
export function zenRefusalAddress(verb: string, err: CatalogWireError): string | undefined {
  if (!verb.startsWith('sources.') || typeof err.room !== 'string' || !err.room) return undefined
  return err.room.slice(0, 200)
}

/** Refusals worth an agent's attention (contract-sources-v1 §11.4): the
 *  widget asked for something it can't have, or the source's server is
 *  away. Budget and size refusals are reported only as `rate_limited`. */
export const ZEN_REPORTED_REFUSALS: ReadonlySet<string> = new Set([
  'not_bound',
  'cap_not_granted',
  'rate_limited',
  'unreachable',
  'server_not_saved',
  'removed',
])

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

  /** A reply that carries bytes moves its buffer (prd-garden-board-needs-v1
   *  GB12, V2); everything else is cloned. No size cap on replies (GH12). */
  const post = (msg: ZenFrameReply): void => {
    if (disposed) return
    const bytes = 'bytes' in msg ? msg.bytes : undefined
    if (bytes instanceof ArrayBuffer) port.postMessage(msg, [bytes])
    else port.postMessage(msg)
  }
  const report = (r: ZenFrameReport): void => {
    try {
      opts.onReport?.(r)
    } catch (err) {
      console.warn(`[zen] widget ${opts.label}: the runtime report failed:`, err)
    }
  }
  /** The verb each live subscription asked for, so a refusal that ends one
   *  later (`pushError`) is reported like an answered one. */
  const subVerbs = new Map<number, string>()
  const reportRefusal = (verb: string, err: CatalogWireError): void => {
    if (!ZEN_REPORTED_REFUSALS.has(err.code)) return
    const address = zenRefusalAddress(verb, err)
    report({ event: 'refusal', verb, code: err.code, ...(address ? { address } : {}) })
  }
  const layer = opts.makeLayer(
    (sub, value) => post({ sub, value }),
    (sub, error) => {
      post({ sub, error })
      const verb = subVerbs.get(sub)
      subVerbs.delete(sub)
      if (verb) reportRefusal(verb, error)
    },
  )

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
      report({ event: 'ready' })
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
      const stack = (m.error as { stack?: unknown }).stack
      console.warn(`[zen] widget ${opts.label}: ${typeof msg === 'string' ? msg.slice(0, 300) : 'error'}`)
      report({
        event: 'error',
        message: typeof msg === 'string' ? msg.slice(0, 500) : 'error',
        ...(typeof stack === 'string' && stack ? { stack: stack.slice(0, 2000) } : {}),
      })
      if (errors.length >= ZEN_FRAME_ERRORS_PER_MINUTE) stop('failing')
      return
    }
    if (m.pointer === 'down' || m.pointer === 'up') {
      opts.onPointer?.(m.pointer === 'down')
      return
    }
    if ('chord' in m) {
      opts.onChord(m.chord, opts.focused())
      return
    }
    const verb = typeof m.verb === 'string' ? m.verb.slice(0, 60) : 'unknown'
    if (typeof m.sub === 'number' && typeof m.verb === 'string') subVerbs.set(m.sub, verb)
    if (typeof m.unsub === 'number') subVerbs.delete(m.unsub)
    void layer.handle(m).then((reply) => {
      if (!reply) return
      post(reply)
      const err = 'error' in reply ? reply.error : null
      if (err) {
        if ('sub' in reply && typeof reply.sub === 'number') subVerbs.delete(reply.sub)
        reportRefusal(verb, err)
      }
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
