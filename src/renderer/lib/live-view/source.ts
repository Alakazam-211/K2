// Where a live workbook's frames come from. The view folds whatever a source
// sends through `liveWorkbookReducer`; it never knows which source it has.
//
// Sources:
//   - `SampleWorkbookSource` (sample-source.ts): plays K2 Cores' xlsx goldens
//     on a timer, with play, pause and step. Sample data, never the file.
//   - `KeyedWatchWorkbookSource`: NOT BUILT. The seam is below.

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

/*
 * TODO(live-workbook): KeyedWatchWorkbookSource, the real source. Not built.
 *
 * Needs, in order:
 *   1. K2 vendors kit v0.16 (`KeyedWatch`, `PresenceChannel` with `cells`):
 *      kit.ts then re-exports from `@k2/mpc-client` instead of mirroring.
 *   2. The daemon relays the kit's keyed watch and element presence for
 *      workbooks over the doc's socket (addendum r1 Q-X15, RC gap 10), with the
 *      xlsx pack's view schema, `derived` (a1, merges, styles, written, bands)
 *      and the new `cells` / `claim_started_cells` roster kinds.
 *   3. The host's `turn_end` event and the doc's tier (live / blob / none).
 *
 * Shape: one `Transport` (the doc socket) shared by a `KeyedWatch` and a
 * `PresenceChannel`, read-only (a watcher sends only `watch` and `report`).
 *
 *   class KeyedWatchWorkbookSource implements LiveWorkbookSource {
 *     readonly kind = 'live'
 *     constructor(transport: Transport, welcome: Welcome) {
 *       // welcome           → emit({ type: 'welcome', ...welcome })
 *       // transport frames  → emit the keyed_snapshot / keyed_page /
 *       //                     ops_change frames as they are (the reducer
 *       //                     folds them exactly as KeyedWatch does), or,
 *       //                     once the reducer reads KeyedWatch's state,
 *       //                     emit a 'book' event from watch.onSnapshot /
 *       //                     watch.onChange instead;
 *       // roster_event       → emit({ type: 'roster_event', event })
 *       // the host's turn_end and tier → emit as they come
 *       // watch.onResync     → emit({ type: 'reset' }) then the new snapshot
 *     }
 *     snapshot(): the welcome, a keyed_snapshot head and pages built from
 *       `watch.state` and `watch.view`, the roster as one 'snapshot' event.
 *   }
 *
 * The entry (components/LiveView/LiveWorkbookEntry.tsx) picks the source:
 * today always the sample, behind the `liveWorkbookPreview` key.
 */
