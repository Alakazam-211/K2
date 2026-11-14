// Thread re-sync by delta (thread-delta-v1, speed pass P3).
//
// `GET /cli/thread?after_seq=N` answers what changed in a Thread since a
// reader's cursor, in one read on the daemon:
//   - `items`: rows with seq > N, oldest first, at most `limit`
//     (`has_more` when newer ones remain past it);
//   - `changed`: rows at or below N rewritten in place since (a card
//     answered, set, voided or expired, a handoff closed, a message deleted
//     into its tombstone, a collected line updated), in their current state;
//   - `removed`: `{id, seq}` of rows that left the Thread (a fork-tab split).
// Every in-place doc write in the overlay store records its change in the
// same write (`put_card_doc`, the import transaction's `put_doc`), so a
// delta carries whatever a full re-read of the loaded window would show,
// plus removals a full re-read never told.
//
// A view re-syncs with the delta when the server reports thread-delta-v1
// and it holds a cursor: the newest seq of its last completed server read
// (vs-live SP-8: never the newest row a socket frame brought). It falls back
// to the full re-read when the delta has more rows than one page, when the
// answer is not a delta (an older daemon ignores `after_seq` and sends its
// newest page), or when the route refuses the query (400).

import {
  mergeThreadItems,
  threadItemsFromSnapshot,
  threadWindowFloor,
  type OverlayThreadItem,
} from './overlayThread'

/** Most new rows one delta re-sync takes. Past it (`has_more`), the view
 *  re-reads its window the full way instead. */
export const THREAD_DELTA_PAGE = 200

export interface ThreadDeltaAnswer {
  conversation_id: string
  items: OverlayThreadItem[]
  hasMore: boolean
  changed: OverlayThreadItem[]
  removed: Array<{ id: string; seq: number }>
}

/** A thread-delta-v1 answer, or null when the body is not one (no
 *  `after_seq` echo or no `changed` list: an older daemon's newest page). */
export function parseThreadDelta(raw: unknown): ThreadDeltaAnswer | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  if (typeof r.after_seq !== 'number' || !Array.isArray(r.changed)) return null
  const snap = threadItemsFromSnapshot(r)
  const changed = threadItemsFromSnapshot({ items: r.changed }).items
  const removed: Array<{ id: string; seq: number }> = []
  for (const x of Array.isArray(r.removed) ? r.removed : []) {
    if (!x || typeof x !== 'object') continue
    const row = x as Record<string, unknown>
    if (typeof row.id === 'string' && row.id && typeof row.seq === 'number') removed.push({ id: row.id, seq: row.seq })
  }
  return { conversation_id: snap.conversation_id, items: snap.items, hasMore: snap.has_more, changed, removed }
}

/**
 * Apply a delta to a Thread list: drop the removed rows, merge the new rows
 * and the changed ones BY ID (`mergeThreadItems`: no duplicates, a repeat of
 * a change already held is a no-op). A changed row the list doesn't hold,
 * below the loaded window while older pages remain (`hasMore`), is left
 * out: its page brings it in its current state (the socket frame rule).
 * Returns `current` itself when nothing changed.
 */
export function applyThreadDelta(
  current: OverlayThreadItem[],
  delta: Pick<ThreadDeltaAnswer, 'items' | 'changed' | 'removed'>,
  hasMore: boolean,
): OverlayThreadItem[] {
  let list = current
  if (delta.removed.length > 0) {
    const gone = new Set(delta.removed.map((r) => r.id))
    const kept = list.filter((it) => !gone.has(it.id))
    if (kept.length !== list.length) list = kept
  }
  const floor = threadWindowFloor(list, hasMore)
  const held = new Set(list.map((it) => it.id))
  const changed = delta.changed.filter((it) => held.has(it.id) || it.seq >= floor)
  return mergeThreadItems(list, [...changed, ...delta.items])
}
