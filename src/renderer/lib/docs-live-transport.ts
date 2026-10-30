// The Live view's transport (plan L2): K2's side of the socket between a
// tab and the kit's live session, `WS /cli/docs/live` (daemon
// `docs/live.rs`). The editor component (K2 Cores' `k2-mpc-client`) owns the
// retry loop and consumes this object; K2 implements one attempt.
//
// The contract (L2 transport spec v0 + its addendum, 2026-10-30):
//   - ONE stable object across reconnects. `LiveText` subscribes once, so
//     `onFrame` listeners live on the transport, never on a socket; each
//     `connect` swaps the socket inside it.
//   - `connect({resume, signal})` is one attempt: open, the daemon's auth
//     (`?token=` before the upgrade) and join, then the first frame,
//     `welcome {root, version, text?, client, self, rights}`. A new client id
//     on every connect. `text` is absent on a resume the window covers.
//     A refusal (`closed` as the first frame, or the socket failing before
//     it) rejects with a `LiveConnectError` carrying `{reason, retry}`.
//   - `send` DROPS while not open: the wire has no idempotency key, so a
//     replayed `edit` could apply twice. Never queued.
//   - `onFrame` gets every server frame but `welcome` and `closed`, in order,
//     from the current socket only (a late frame from a replaced socket is
//     ignored).
//   - `onClose({reason, retry, error?, ...})` fires once per opened socket
//     the daemon (or the network) ended: from the daemon's `closed` frame,
//     else from the close code (4010: the core was switched off), else
//     `network` (retry). Not for `close()`.
//   - `connect()` while a socket is open (addendum 2: a refused snapshot
//     makes the editor connect again) replaces it: the old socket closes
//     and the new one dials only after its close (at most 1 s), so the
//     daemon has left the old connection; never two for one editor.
//   - A reconnect sends `prev=<the last welcome's client id>` (or the
//     `prev` given to `connect`): the daemon retires that connection (same
//     person, doc and root) before the new one joins, so an edit still in
//     flight on the dropped socket can't apply after the new `since` (the
//     reconnect-race rule agreed with K2 Cores for WIRE.md).
//   - `close()`: the tab is closing; the daemon leaves the doc.
//   - Keepalive is the daemon's WS Ping (every 20 s); there is no app-level
//     ping. Visibility rides `{"t":"viewing","visible"}` through `send`.
//
// Provisional until K2 Cores' WIRE.md lands (G1, G2, G8, G9): `welcome`,
// `closed`, `snapshot`, `undo_result`, `error`, the viewing and view frames.

// The kit's hunk-channel frames, mirrored from the vendored client's
// `vendor/k2-mpc-base/client/src/wire.ts` (its `ClientFrame`, `ServerFrame`,
// `ErrorReport`), which isn't in this project's file list yet. The shapes are
// structural, so the component's `Transport` accepts this object as is. When
// the component's package is wired into the renderer, import its types here.

/** A hunk: replace UTF-8 bytes `start..end` with `insert`. */
export interface LiveHunk {
  start: number
  end: number
  insert: string
}

/** The kit's `ErrorReport` JSON. */
export interface ErrorReport {
  id: string
  message: string
  exit?: number
  next?: string
  rule?: string
  details?: Record<string, unknown>
}

/** The kit's `ChangeEvent`. */
export interface LiveChangeEvent {
  version: number
  pid: string
  client: number
  via: string
  root: string
  hunks: LiveHunk[]
  at_ms: number
  on_behalf_of?: string | null
}

/** The kit's client frames (`wire.ts` `ClientFrame`). */
export type ClientFrame =
  | { type: 'edit'; op: number; root: string; base: number; hunks: LiveHunk[]; group?: string }
  | { type: 'since'; op: number; version: number }
  | { type: 'cursor'; root: string; version: number; anchor: number; head: number }
  | { type: 'undo'; op: number }
  | { type: 'redo'; op: number }

/** The kit's server frames (`wire.ts` `ServerFrame`; roster shapes left open). */
export type ServerFrame =
  | { type: 'ack'; op: number; version: number; hunks: LiveHunk[] }
  | { type: 'refused'; op: number; error: ErrorReport }
  | { type: 'change'; event: LiveChangeEvent }
  | { type: 'events'; op: number; events: LiveChangeEvent[] }
  | { type: 'roster'; entries: Array<Record<string, unknown>> }
  | { type: 'roster_event'; event: { kind: string } & Record<string, unknown> }

export type LiveRights = 'read' | 'edit'

/** The daemon's first frame on a socket. */
export interface LiveWelcome {
  root: string
  /** The doc's version (the snapshot's `Seq`). */
  version: number
  /** The whole text: on a first connect, or when `resume` couldn't be served. */
  text?: string
  /** This socket's client id in the doc (new on every connect). */
  client: number
  /** This person's pid as the roster shows it (`person:owner`). */
  self: string
  rights: LiveRights
  /** K2's: the file's core (`markdown`, `text`), editor family, workspace path. */
  core?: string
  family?: string
  path?: string
}

export type LiveCloseReason =
  | 'left'
  | 'idle'
  | 'kicked'
  | 'rights_lost'
  | 'deleted'
  | 'moved'
  | 'released'
  | 'cut_off'
  | 'declined'
  | 'not_found'
  | 'auth'
  | 'network'
  | 'closed'

/** Why a socket ended (the daemon's `closed` frame, G2). */
export interface LiveClose {
  reason: LiveCloseReason
  /** Whether to reconnect with `resume` (the daemon says so). */
  retry: boolean
  /** The kit's refusal, when the kit refused. */
  error?: ErrorReport
  /** K2's own code (`core_off`, `secret`, `not_a_text_doc`...). */
  code?: string
  message?: string
  /** On `core_off`: the version the file was written at, and whether it was. */
  version?: number
  written?: boolean
  core?: string
}

/** Frames K2 takes beside the kit's `ClientFrame` (provisional, G1/G8). */
export type LiveExtraClientFrame =
  | { type: 'snapshot'; op: number }
  | { t: 'viewing'; visible: boolean }
  | { type: 'view'; first: number; last: number; root?: string }

export type LiveClientFrame = ClientFrame | LiveExtraClientFrame

/** Frames K2 sends beside the kit's `ServerFrame` (provisional, G1/G2/G9). */
export type LiveExtraServerFrame =
  | { type: 'snapshot'; op: number; text: string; version: number }
  | { type: 'undo_result'; op: number; outcome: string; steps: number; version: number }
  | { type: 'error'; error: ErrorReport }

export type LiveServerFrame = ServerFrame | LiveExtraServerFrame

export interface LiveTransport {
  /** One attempt: open, auth, join; resolves with `welcome`. */
  connect(opts: { resume?: number; prev?: number; signal: AbortSignal }): Promise<LiveWelcome>
  /** Dropped (never queued) while not open. */
  send(frame: LiveClientFrame): void
  onFrame(cb: (f: LiveServerFrame) => void): () => void
  onClose(cb: (e: LiveClose) => void): () => void
  /** The tab is closing: the daemon leaves the doc. */
  close(): void
  /** Whether a socket is open now. */
  readonly open: boolean
}

/** A refused or failed `connect`. */
export class LiveConnectError extends Error {
  constructor(readonly close: LiveClose) {
    super(close.message ?? `live socket: ${close.reason}`)
    this.name = 'LiveConnectError'
  }
}

/** The socket surface this module uses (a browser `WebSocket` fits). */
export interface LiveSocket {
  readonly readyState: number
  send(data: string): void
  close(code?: number, reason?: string): void
  onopen: ((ev: unknown) => void) | null
  onmessage: ((ev: { data: unknown }) => void) | null
  onclose: ((ev: { code?: number; reason?: string }) => void) | null
  onerror: ((ev: unknown) => void) | null
}

export interface LiveTransportOptions {
  /** The socket URL for one attempt (`resume` and `prev` when reconnecting). */
  url: (resume: number | undefined, prev: number | undefined) => Promise<string>
  /** Opens a socket (default: `new WebSocket(url)`). */
  open?: (url: string, signal: AbortSignal) => Promise<LiveSocket>
}

const OPEN = 1
const CLOSED = 3
/** How long a replaced socket gets to finish closing before the new dial. */
const RETIRE_WAIT_MS = 1000
/** The daemon's close code for a core switched off (PRD §6.1). */
export const CLOSE_CORE_OFF = 4010

const REASONS: ReadonlySet<string> = new Set<LiveCloseReason>([
  'left',
  'idle',
  'kicked',
  'rights_lost',
  'deleted',
  'moved',
  'released',
  'cut_off',
  'declined',
  'not_found',
  'auth',
  'network',
  'closed',
])

/** WIRE.md v2 "Closing", the retry column: only `cut_off` and `network`. */
export function canRetry(reason: LiveCloseReason): boolean {
  return reason === 'cut_off' || reason === 'network'
}

/** A `closed` frame's fields, checked. Unknown reasons are `network`; `retry`
 *  comes from the table, never from the frame. */
export function parseClose(f: Record<string, unknown>): LiveClose {
  const reason = typeof f.reason === 'string' && REASONS.has(f.reason) ? (f.reason as LiveCloseReason) : 'network'
  const out: LiveClose = { reason, retry: canRetry(reason) }
  if (f.error && typeof f.error === 'object') out.error = f.error as ErrorReport
  if (typeof f.code === 'string') out.code = f.code
  if (typeof f.message === 'string') out.message = f.message
  if (typeof f.version === 'number') out.version = f.version
  if (typeof f.written === 'boolean') out.written = f.written
  if (typeof f.core === 'string') out.core = f.core
  return out
}

/** What a close without a `closed` frame means. */
export function closeFromCode(code: number | undefined): LiveClose {
  if (code === CLOSE_CORE_OFF) return { reason: 'closed', retry: false, code: 'core_off' }
  return { reason: 'network', retry: true }
}

/** `WS /cli/docs/live` for `doc` (absolute, or relative with `workspace`). */
export function liveUrl(
  wsBase: string,
  token: string,
  doc: string,
  opts: { workspace?: string; resume?: number; prev?: number } = {},
): string {
  const q = new URLSearchParams()
  q.set('doc', doc)
  if (opts.workspace) q.set('workspace', opts.workspace)
  q.set('root', 'source')
  if (opts.resume !== undefined) q.set('resume', String(opts.resume))
  if (opts.prev !== undefined) q.set('prev', String(opts.prev))
  q.set('token', token)
  return `${wsBase}/cli/docs/live?${q.toString()}`
}

function abortError(): Error {
  const e = new Error('aborted')
  e.name = 'AbortError'
  return e
}

function defaultOpen(url: string, signal: AbortSignal): Promise<LiveSocket> {
  if (signal.aborted) return Promise.reject(abortError())
  return Promise.resolve(new WebSocket(url) as unknown as LiveSocket)
}

export function createLiveTransport(opts: LiveTransportOptions): LiveTransport {
  const open = opts.open ?? defaultOpen
  const frameCbs = new Set<(f: LiveServerFrame) => void>()
  const closeCbs = new Set<(e: LiveClose) => void>()
  let sock: LiveSocket | null = null
  /** The socket that delivered `welcome` and hasn't ended. */
  let live: LiveSocket | null = null
  /** The last welcome's client id: the next connect's `prev`. */
  let lastClient: number | undefined

  function detach(s: LiveSocket): void {
    s.onopen = null
    s.onmessage = null
    s.onclose = null
    s.onerror = null
  }

  function shut(s: LiveSocket): void {
    detach(s)
    if (s.readyState === 0) {
      // Never abort a handshake (WebKit logs it): close once it opens.
      s.onopen = () => {
        try {
          s.close(1000, 'left')
        } catch {
          /* already gone */
        }
      }
      return
    }
    if (s.readyState === OPEN) {
      try {
        s.close(1000, 'left')
      } catch {
        /* already gone */
      }
    }
  }

  /** Close `s` and wait (at most `RETIRE_WAIT_MS`) until it has closed, so
   *  the daemon has left its connection before a new one joins (addendum 2:
   *  never two live connections for one editor). */
  function retire(s: LiveSocket): Promise<void> {
    if (s.readyState === CLOSED) {
      detach(s)
      return Promise.resolve()
    }
    return new Promise<void>((resolve) => {
      const timer = setTimeout(done, RETIRE_WAIT_MS)
      function done(): void {
        clearTimeout(timer)
        s.onclose = null
        s.onerror = null
        resolve()
      }
      shut(s)
      s.onclose = done
      s.onerror = done
    })
  }

  function ended(s: LiveSocket, why: LiveClose): void {
    if (live !== s) return
    live = null
    sock = null
    detach(s)
    for (const cb of [...closeCbs]) cb(why)
  }

  return {
    get open() {
      return live !== null && live.readyState === OPEN
    },

    async connect({ resume, prev, signal }) {
      if (sock) {
        // A connect while a socket is open (or opening) replaces it: the
        // old one closes, and the daemon leaves it, before the new one dials.
        const old = sock
        sock = null
        live = null
        await retire(old)
      }
      if (signal.aborted) throw abortError()
      const url = await opts.url(resume, prev ?? lastClient)
      if (signal.aborted) throw abortError()
      const s = await open(url, signal)
      if (signal.aborted) {
        shut(s)
        throw abortError()
      }
      sock = s
      return new Promise<LiveWelcome>((resolve, reject) => {
        let settled = false
        const fail = (e: Error): void => {
          if (settled) return
          settled = true
          signal.removeEventListener('abort', onAbort)
          if (sock === s) sock = null
          shut(s)
          reject(e)
        }
        const onAbort = (): void => fail(abortError())
        signal.addEventListener('abort', onAbort, { once: true })
        s.onmessage = (ev) => {
          if (sock !== s) return
          let f: Record<string, unknown>
          try {
            f = JSON.parse(String(ev.data)) as Record<string, unknown>
          } catch {
            return
          }
          if (!settled) {
            if (f.type === 'welcome') {
              settled = true
              signal.removeEventListener('abort', onAbort)
              live = s
              if (typeof f.client === 'number') lastClient = f.client
              resolve(f as unknown as LiveWelcome)
            } else if (f.type === 'closed') {
              fail(new LiveConnectError(parseClose(f)))
            }
            // Nothing else comes before `welcome`.
            return
          }
          if (live !== s) return
          if (f.type === 'closed') {
            const why = parseClose(f)
            ended(s, why)
            shut(s)
            return
          }
          if (f.type === 'welcome') return
          for (const cb of [...frameCbs]) cb(f as unknown as LiveServerFrame)
        }
        s.onclose = (ev) => {
          if (!settled) {
            fail(new LiveConnectError(closeFromCode(ev?.code)))
            return
          }
          ended(s, closeFromCode(ev?.code))
        }
        s.onerror = () => {
          if (!settled) {
            fail(new LiveConnectError({ reason: 'network', retry: true }))
            return
          }
          ended(s, { reason: 'network', retry: true })
        }
      })
    },

    send(frame) {
      const s = live
      if (!s || s.readyState !== OPEN) return
      s.send(JSON.stringify(frame))
    },

    onFrame(cb) {
      frameCbs.add(cb)
      return () => {
        frameCbs.delete(cb)
      }
    },

    onClose(cb) {
      closeCbs.add(cb)
      return () => {
        closeCbs.delete(cb)
      }
    },

    close() {
      const s = sock
      sock = null
      live = null
      if (s) shut(s)
    },
  }
}
