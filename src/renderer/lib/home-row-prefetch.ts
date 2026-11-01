// Hovering a Home row on another server reads its Thread ahead
// (research-remote-thread-load-v1 Fix 4, "prefetch on hover").
//
// After a short rest on the row, the pinned Chat's Thread (its address is
// the row's handle) is read into the view cache (`threadViewCache.ts`), so
// a click paints it at once. Debounced; at most one prefetch in flight in
// the window and none for a Thread cached in the last 30 s
// (`prefetchThread`). Rows on this computer are skipped (a local read is
// fast). Nothing is shown or marked read by a prefetch.

import { parseHomeAddress } from '@/lib/home-address'
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { scopeForHost } from '@/kessel/server-scope'
import { prefetchThread } from '@/components/SessionView/threadRead'

/** How long the pointer rests on a row before the read goes out. */
export const HOVER_PREFETCH_DELAY_MS = 150

/** Read a Home row's Thread into the view cache (remote rows only). */
export function prefetchHomeRow(address: string): void {
  const parsed = parseHomeAddress(address)
  if (!parsed || parsed.host === LOCAL_HOME_HOST) return
  let scope
  try {
    scope = scopeForHost(parsed.host)
  } catch {
    return
  }
  if (!scope.isRemote) return
  void prefetchThread(scope, parsed.handle)
}

export interface HoverPrefetchDeps {
  prefetch(address: string): void
  setTimeout(fn: () => void, ms: number): unknown
  clearTimeout(handle: unknown): void
}

export interface HoverPrefetch {
  enter(address: string): void
  leave(): void
}

/** The debounce: a row the pointer rests on for
 *  {@link HOVER_PREFETCH_DELAY_MS} is prefetched; moving on cancels. */
export function createHoverPrefetch(deps: HoverPrefetchDeps): HoverPrefetch {
  let timer: unknown = null
  const cancel = (): void => {
    if (timer !== null) deps.clearTimeout(timer)
    timer = null
  }
  return {
    enter(address) {
      cancel()
      timer = deps.setTimeout(() => {
        timer = null
        deps.prefetch(address)
      }, HOVER_PREFETCH_DELAY_MS)
    },
    leave: cancel,
  }
}

/** The window's one hover prefetcher. */
export const homeRowHoverPrefetch: HoverPrefetch = createHoverPrefetch({
  prefetch: prefetchHomeRow,
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
})
