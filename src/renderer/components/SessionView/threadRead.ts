// The first page of a Thread, read once and shared
// (research-remote-thread-load-v1 Fix 1 and Fix 4).
//
// `GET /cli/thread?addr=&limit=25` is the one call that paints a Thread.
// On a remote server it is shared: a view that opens while the same read
// is already on its way (a Home row's hover prefetch, the room starting its
// Thread before the pinned Chat is ensured) waits for that one instead of
// sending another. Every remote read also refreshes the Thread's view cache
// (`threadViewCache.ts`). A local read goes out as before.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { OVERLAY_PAGE_SIZE, threadCardsParams, threadItemsFromSnapshot } from './overlayThread'
import { peekThreadView, saveThreadView } from './threadViewCache'

const inflight = new Map<string, Promise<unknown>>()

function readKey(scope: ServerScope, addr: string, cards: string): string {
  return `${scope.connectionKey}|${addr}|${cards}`
}

/** `GET thread?addr=&limit=25` on `scope`; a remote read already on its way
 *  for the same Thread is joined. Throws what the read threw. */
export function readThreadFirstPage(scope: ServerScope, addr: string): Promise<unknown> {
  const params = { addr, limit: OVERLAY_PAGE_SIZE, ...threadCardsParams(scope) }
  if (!scope.isRemote) return daemonCliGet<unknown>(scope, 'thread', params)
  const key = readKey(scope, addr, params.cards ?? '')
  const running = inflight.get(key)
  if (running) return running
  const hostKey = scope.hostKey
  const p = daemonCliGet<unknown>(scope, 'thread', params)
    .then((raw) => {
      const snap = threadItemsFromSnapshot(raw)
      saveThreadView(hostKey, addr, snap.conversation_id, snap.items)
      return raw
    })
    .finally(() => {
      inflight.delete(key)
    })
  inflight.set(key, p)
  return p
}

/** A prefetch younger than this is not repeated. */
export const PREFETCH_FRESH_MS = 30_000

let prefetching: Promise<void> | null = null

/** Warm a remote Thread's view cache (a Home row hover). At most one
 *  prefetch in flight in this window; a Thread cached in the last
 *  {@link PREFETCH_FRESH_MS} is skipped. Never throws. Resolves true when a
 *  read was sent. */
export function prefetchThread(scope: ServerScope, addr: string, now: () => number = Date.now): Promise<boolean> {
  if (!scope.isRemote || !addr.trim() || prefetching) return Promise.resolve(false)
  const hit = peekThreadView(scope.hostKey, addr)
  if (hit && now() - hit.savedAt < PREFETCH_FRESH_MS) return Promise.resolve(false)
  const p = readThreadFirstPage(scope, addr).then(
    () => undefined,
    (e: unknown) => console.debug('[thread] prefetch failed:', e),
  )
  prefetching = p.finally(() => {
    if (prefetching === wrapped) prefetching = null
  })
  const wrapped = prefetching
  return wrapped.then(() => true)
}

/** Tests: forget in-flight reads. */
export function resetThreadReadsForTests(): void {
  inflight.clear()
  prefetching = null
}
