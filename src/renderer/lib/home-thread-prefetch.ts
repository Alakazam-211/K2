// The Threads the catch-up reads ahead (speed pass P3b): the shown Home's
// top rows and its rows with unread, as on the phone (K2 Companion
// `chatPrefetch.ts`, its targets only: the reads themselves ride the one
// catch-up call per server, `chatCatchupRun.ts`). Nothing is shown or
// marked read by it.

import { parseHomeAddress } from '@/lib/home-address'
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { selectedHome, useHomesStore, type HomeRow } from '@/stores/homes'
import { displayRows } from '@/lib/home-row-order'
import { threadSumForProject, useUnreadStore } from '@/stores/unread'
import { hostPool } from '@/lib/host-pool-instance'
import { summaryHit } from '@/lib/home-agent-status'

/** The first rows of the shown Home whose Threads the catch-up reads ahead
 *  (as on the phone: the top rows, plus every row with unread). */
export const PREFETCH_TOP_ROWS = 8

/** A row's unread count: the window's server from the unread store, another
 *  server from its summary (the pool's). 0 when unknown. */
function rowUnread(row: HomeRow, host: string): number {
  if (host === LOCAL_HOME_HOST) {
    return row.workspaceId ? (threadSumForProject(useUnreadStore.getState(), row.workspaceId)?.unread ?? 0) : 0
  }
  return summaryHit(hostPool.store.getState().entries[host]?.activity ?? null, row)?.unread ?? 0
}

/** The Threads the catch-up reads ahead: the shown Home's top
 *  {@link PREFETCH_TOP_ROWS} rows and its rows with unread, as
 *  `{hostKey, addr}` (the row's handle is its pinned Chat's address). */
export function homePrefetchTargets(): Array<{ hostKey: string; addr: string }> {
  const s = useHomesStore.getState()
  if (!s.homes.length) return []
  const out: Array<{ hostKey: string; addr: string }> = []
  displayRows(selectedHome(s)).forEach((row, i) => {
    const parsed = parseHomeAddress(row.address)
    if (!parsed) return
    if (i < PREFETCH_TOP_ROWS || rowUnread(row, parsed.host) > 0) out.push({ hostKey: parsed.host, addr: parsed.handle })
  })
  return out
}
