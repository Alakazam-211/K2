// Where a live workbook's frames come from. The view folds whatever a source
// sends through `liveWorkbookReducer`; it never knows which source it has.
//
// Sources:
//   - `SampleWorkbookSource` (sample-source.ts): plays K2 Cores' xlsx goldens
//     on a timer, with play, pause and step. Sample data, never the file.
//   - `KeyedWatchWorkbookSource` (below): a doc socket. When its welcome
//     lists `keyed_watch` (kit v0.16), the kit's `KeyedWatch` runs on the
//     socket: it sends `watch` (and again after a gap or a frame that
//     doesn't fit), its `report`s, and keeps the doc's keyed state and view;
//     the frames pass on to the view as they come. Without the feature (an
//     older host) nothing is sent and the frames pass on as before.
//
// Not wired to a socket yet: the entry (components/LiveView/
// LiveWorkbookEntry.tsx) still always picks the sample. The daemon serves
// the keyed watch for live decks; a workbook isn't a live doc on it yet.

import { KEYED_WATCH_FEATURE, KeyedWatch, type ServerFrame, type Transport } from '@k2/mpc-client'
import type { XlsxWelcome } from './not-in-kit'
import type { LiveWorkbookFrame } from './reducer'

/** What a source sends: the doc's frames, or "start over" (a sample's loop; a new doc on the same view). */
export type LiveWorkbookSourceEvent = LiveWorkbookFrame | { type: 'reset' }

export interface LiveWorkbookSource {
  /** `sample` draws the "Sample data" banner; `live` is a real doc. */
  readonly kind: 'sample' | 'live'
  /** Each event as it comes. Returns the unsubscribe. */
  subscribe(onEvent: (e: LiveWorkbookSourceEvent) => void): () => void
  /** Everything sent so far that a new subscriber folds to catch up (in order, from the last reset). */
  snapshot(): readonly LiveWorkbookSourceEvent[]
  /** Stop: no more events; timers and sockets go. */
  close(): void
}

/** The socket frames a workbook view folds (the rest, such as acks or the hunk channel's, aren't for it).
 *  `outside_save` and `verb_refused` (kit v0.19.0, unnumbered keyed-watch notices) pass on as the kit's
 *  watch frames do; the view doesn't draw them yet. */
const FOLDED: ReadonlySet<string> = new Set(['keyed_snapshot', 'keyed_page', 'ops_change', 'watch_refused', 'outside_save', 'verb_refused', 'roster_event', 'turn_end', 'tier'])

/** Frames of the keyed watch: a fresh snapshot's head makes every earlier one moot. */
const KEYED: ReadonlySet<string> = new Set(['keyed_snapshot', 'keyed_page', 'ops_change', 'watch_refused', 'outside_save', 'verb_refused'])

/** Changes kept since the last snapshot before the source asks for a fresh one (catch-up stays short). */
export const CHANGES_KEPT = 2048

/** Whether a welcome lists the keyed watch. */
export function listsKeyedWatch(welcome: { features?: string[] }): boolean {
  return Array.isArray(welcome.features) && welcome.features.includes(KEYED_WATCH_FEATURE)
}

/** A presence value that only its latest event says (a pointer or a viewport, per person). */
function latestOnly(e: LiveWorkbookSourceEvent): string | null {
  if (e.type !== 'roster_event') return null
  const ev = e.event
  return ev.kind === 'pointer' || ev.kind === 'viewport' ? `${ev.kind}:${ev.who}` : null
}

export class KeyedWatchWorkbookSource implements LiveWorkbookSource {
  readonly kind = 'live' as const
  /** The kit's watcher: the doc's keyed state and view as the frames built them. `null` when the welcome
   *  doesn't list `keyed_watch` (nothing is sent; the frames pass on as before). */
  watch: KeyedWatch | null
  private readonly t: Transport
  private readonly unsub: () => void
  private readonly listeners = new Set<(e: LiveWorkbookSourceEvent) => void>()
  private log: LiveWorkbookSourceEvent[] = []
  private changes = 0
  private closed = false

  constructor(transport: Transport, welcome: XlsxWelcome) {
    this.t = transport
    // Frames first: the watch's own subscription comes after, so a frame reaches the view before the watch reacts to it.
    this.unsub = transport.onFrame((f) => this.onFrame(f))
    this.watch = null
    this.start(welcome)
  }

  /** A new connection (a reconnect): its welcome replaces the last one's, and everything starts over. */
  reconnected(welcome: XlsxWelcome): void {
    if (this.closed) return
    this.log = []
    this.changes = 0
    for (const l of this.listeners) l({ type: 'reset' })
    if (this.watch && listsKeyedWatch(welcome)) {
      this.emit({ type: 'welcome', ...welcome })
      this.watch.reconnected(this.kitWelcome(welcome))
    } else {
      this.watch?.close()
      this.watch = null
      this.start(welcome)
    }
  }

  subscribe(onEvent: (e: LiveWorkbookSourceEvent) => void): () => void {
    this.listeners.add(onEvent)
    return () => this.listeners.delete(onEvent)
  }

  snapshot(): readonly LiveWorkbookSourceEvent[] {
    return this.log.slice()
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    this.watch?.close()
    this.unsub()
    this.listeners.clear()
  }

  private kitWelcome(w: XlsxWelcome) {
    return { ...w, root: w.root ?? '', version: w.version ?? 0 }
  }

  private start(welcome: XlsxWelcome): void {
    this.emit({ type: 'welcome', ...welcome })
    // The kit's watch sends `watch` at once when the feature is listed; otherwise there is none.
    this.watch = listsKeyedWatch(welcome) ? new KeyedWatch(this.t, this.kitWelcome(welcome)) : null
  }

  private onFrame(f: ServerFrame | { type: string }): void {
    if (this.closed || !FOLDED.has(f.type)) return
    const e = f as LiveWorkbookFrame
    if (e.type === 'keyed_snapshot') {
      // A fresh snapshot replaces every earlier keyed frame for a subscriber catching up.
      this.log = this.log.filter((x) => !KEYED.has(x.type))
      this.changes = 0
    } else if (e.type === 'ops_change' && ++this.changes > CHANGES_KEPT && this.watch) {
      this.watch.resync()
      this.changes = 0
    }
    this.emit(e)
  }

  private emit(e: LiveWorkbookSourceEvent): void {
    const only = latestOnly(e)
    if (only) this.log = this.log.filter((x) => latestOnly(x) !== only)
    this.log.push(e)
    for (const l of this.listeners) l(e)
  }
}
