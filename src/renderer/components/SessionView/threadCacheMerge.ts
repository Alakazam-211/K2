// Chat cache merges (stale-while-revalidate). Pure: no store, no I/O.
//
// Ported from K2 Companion's `src/zen/threadCacheMerge.ts` (MIT, Alakazam
// Labs), retyped from the phone's `ThreadMessage` to the desktop's
// `OverlayThreadItem` (`doc` + `seq`). The project chat merges stay on the
// phone (the desktop does not cache project chats), and a delta's copy
// replaces the held one whole: the desktop's delta carries each file's
// live `state` (`thread_items_view`), so there is no state to keep.
//
// Rules (a server read over what this window already shows):
//   - the server copy wins for every message it carries (an answered or
//     voided card, a file whose state changed, a deleted message);
//   - inside the seq window the server page covers, a cached message the
//     page does not carry is gone and is dropped; older than the page
//     (when it says there is more) the cached copy stays; newer than its
//     newest only a live frame that arrived while the read ran stays
//     (`arrivedLive`): anything else the server already had, so it is gone;
//   - this window's unsent posts (`pending`) are never touched;
//   - another conversation id (the Thread was reset) means the cache is
//     another conversation: the server page replaces it whole;
//   - a message that came by a push (`provisional`) never becomes a cursor
//     (`newestSeq`): a gap before it would otherwise be skipped for good;
//   - unchanged messages keep their objects, and an unchanged list keeps
//     its array, so nothing re-renders for nothing.

import { sameThreadItem, type OverlayThreadItem } from './overlayThread'

/** Most messages a cached chat keeps (the newest). */
export const CACHE_MAX_MESSAGES = 100

/** Same message as far as the screen (and the cursor) can tell. */
export function sameMessage(a: OverlayThreadItem, b: OverlayThreadItem): boolean {
  return sameThreadItem(a, b) && !!a.provisional === !!b.provisional
}

/** `next`, reusing `prev`'s objects (and `prev` itself) where nothing changed. */
export function shareUnchanged(prev: OverlayThreadItem[], next: OverlayThreadItem[]): OverlayThreadItem[] {
  const byId = new Map<string, OverlayThreadItem>()
  for (const m of prev) if (m.id) byId.set(m.id, m)
  let changed = prev.length !== next.length
  const out = next.map((m, i) => {
    const old = m.id ? byId.get(m.id) : undefined
    const keep = old && sameMessage(old, m) ? old : m
    if (keep !== prev[i]) changed = true
    return keep
  })
  return changed ? out : prev
}

function bySeq(a: OverlayThreadItem, b: OverlayThreadItem): number {
  return a.seq - b.seq
}

function local(m: OverlayThreadItem): boolean {
  return !!m.pending
}

/** The server's copy: never provisional. */
function confirmed(m: OverlayThreadItem): OverlayThreadItem {
  if (!m.provisional) return m
  const { provisional: _p, ...rest } = m
  return rest
}

/**
 * Merge the server's newest page (`page`, any order) over `current`.
 * `hasMore`: the server has older messages than the page. `sameConversation`
 * false: `current` belongs to another conversation (replace).
 */
export function mergeServerPage(
  current: OverlayThreadItem[],
  page: OverlayThreadItem[],
  opts: { hasMore: boolean; sameConversation?: boolean; arrivedLive?: ReadonlySet<string> },
): OverlayThreadItem[] {
  const pageSorted = page.map(confirmed).sort(bySeq)
  const keepLocal = current.filter(local)
  if (opts.sameConversation === false || !current.length) {
    return shareUnchanged(current, [...pageSorted, ...keepLocal])
  }
  const seqs = pageSorted.map((m) => m.seq).filter((s) => Number.isFinite(s))
  const hi = seqs.length ? Math.max(...seqs) : null
  const lo = seqs.length ? Math.min(...seqs) : null
  const pageIds = new Set(pageSorted.map((m) => m.id).filter(Boolean))
  const kept: OverlayThreadItem[] = []
  for (const m of current) {
    if (local(m)) continue
    if (m.id && pageIds.has(m.id)) continue // the server's copy wins
    if (!Number.isFinite(m.seq)) continue // cannot be placed: the page decides
    if (hi == null) {
      // An empty page: the Thread is empty now (bar frames that raced it).
      if (opts.arrivedLive?.has(m.id) || opts.hasMore) kept.push(m)
      continue
    }
    if (m.seq > hi) {
      // Newer than the read: a live frame that raced it stays.
      if (opts.arrivedLive?.has(m.id)) kept.push(m)
    } else if (m.seq < (lo as number) && opts.hasMore) kept.push(m) // older than the page
    // else inside the page's window (or proven gone below it): dropped
  }
  const merged = [...kept, ...pageSorted].sort(bySeq)
  return shareUnchanged(current, [...merged, ...keepLocal])
}

/**
 * Merge a delta (`after_seq` read: messages newer than the cache's
 * newest, plus older messages that CHANGED since, e.g. an answered or
 * voided card). Upsert by id; a new seq is appended in order. Nothing is
 * dropped (`removeIds` takes the delta's `removed`).
 */
export function mergeDelta(current: OverlayThreadItem[], delta: OverlayThreadItem[]): OverlayThreadItem[] {
  if (!delta.length) return current
  const byId = new Map<string, OverlayThreadItem>()
  for (const m of delta) if (m.id) byId.set(m.id, confirmed(m))
  const seen = new Set<string>()
  const next = current.map((m) => {
    const d = m.id ? byId.get(m.id) : undefined
    if (!d) return m
    seen.add(m.id)
    return d
  })
  // A changed doc may come without an id match only if its id moved;
  // match by seq then (replace in place).
  const bySeqAt = new Map<number, number>()
  next.forEach((m, i) => {
    if (Number.isFinite(m.seq)) bySeqAt.set(m.seq, i)
  })
  for (const raw of delta) {
    const d = confirmed(raw)
    if (d.id && seen.has(d.id)) continue
    const at = Number.isFinite(d.seq) ? bySeqAt.get(d.seq) : undefined
    if (at != null && !local(next[at])) {
      next[at] = d
      continue
    }
    next.push(d)
  }
  return shareUnchanged(current, next.sort(bySeq))
}

/** The newest `max` server messages (unsent posts never persist). */
export function cacheSlice(
  thread: readonly OverlayThreadItem[],
  max = CACHE_MAX_MESSAGES,
): { messages: OverlayThreadItem[]; truncated: boolean } {
  const settled = thread.filter((m) => !local(m))
  if (settled.length <= max) return { messages: settled, truncated: false }
  return { messages: settled.slice(settled.length - max), truncated: true }
}

/** Newest server seq in `thread` (the delta cursor), else null. A message
 *  that came by a push isn't confirmed yet: counting it would skip any
 *  message before it that the push didn't carry. */
export function newestSeq(thread: readonly OverlayThreadItem[]): number | null {
  let best: number | null = null
  for (const m of thread) {
    if (local(m) || m.provisional) continue
    if (!Number.isFinite(m.seq)) continue
    if (best == null || m.seq > best) best = m.seq
  }
  return best
}

/** Delete messages by id (a delta's `removed`). Same array when none. */
export function removeIds(current: OverlayThreadItem[], ids: readonly string[]): OverlayThreadItem[] {
  if (!ids.length) return current
  const gone = new Set(ids)
  const next = current.filter((m) => !m.id || !gone.has(m.id) || local(m))
  return next.length === current.length ? current : next
}
