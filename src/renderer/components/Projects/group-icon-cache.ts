// §6.7.7 — module-level cache for project-group ICONS (the
// Sidebar/ProjectAvatar iconCache idiom, group-shaped). Icons ride their
// own GET route (`/cli/project-group/icon`) because the dataUrl blob is
// deliberately NOT in list/show payloads; caching here keeps the nav's
// expanded rows + collapsed rail + Settings preview from re-fetching.
//
// Keys are `${hostKey}:${groupId}` (the project-groups last-seen-cursor
// idiom) so remoting into a server never shows the local host's icons.
//
// Invalidation (0.46.1): list/show rows carry `iconKey`, a daemon-side
// key that changes exactly when the icon does. An entry is fresh while
// its `iconKey` matches the row's, so a rename, pin or chat line never
// refetches the 10-50 KB blob, and a new upload refetches it once.
// Older daemons send no key (`iconKey` undefined): there the store still
// drops a group's entry on `groups-changed` and avatars refetch.
//
// `loadGroupIcon` is single-flight per (host, group, key): the nav row,
// the rail and the Settings preview mounting together share one GET.

import { fetchProjectGroupIcon } from './projects-api'
import { primaryScope } from '@/kessel/server-scope'

export interface GroupIconEntry {
  found: boolean
  dataUrl: string | null
  /** The daemon's key for this icon (0.46.1+); undefined from older
   *  daemons. */
  iconKey?: string | null
}

const cache = new Map<string, GroupIconEntry>()
const inflight = new Map<string, Promise<GroupIconEntry>>()

const NO_ICON: GroupIconEntry = Object.freeze({ found: false, dataUrl: null, iconKey: null })

function key(hostKey: string, groupId: string): string {
  return `${hostKey}:${groupId}`
}

/** Synchronous read — undefined = never fetched (fetch it). */
export function getCachedGroupIcon(hostKey: string, groupId: string): GroupIconEntry | undefined {
  return cache.get(key(hostKey, groupId))
}

export function setCachedGroupIcon(
  hostKey: string,
  groupId: string,
  entry: GroupIconEntry,
): void {
  cache.set(key(hostKey, groupId), entry)
}

/** The cached icon when it is still right for `iconKey`, else undefined
 *  (fetch it):
 *  - `iconKey === null` — the daemon says there is no icon: no fetch.
 *  - a string — fresh only when the cached entry was stored under it.
 *  - `undefined` (older daemon / row not loaded) — any cached entry; the
 *    store's groups-changed drop keeps it honest. */
export function resolveCachedGroupIcon(
  hostKey: string,
  groupId: string,
  iconKey: string | null | undefined,
): GroupIconEntry | undefined {
  if (iconKey === null) return NO_ICON
  const cached = cache.get(key(hostKey, groupId))
  if (!cached) return undefined
  if (iconKey === undefined) return cached
  return cached.iconKey === iconKey ? cached : undefined
}

/** Cached icon or ONE shared GET for it. A failed GET caches a miss under
 *  the requested key (advisory decoration: initials are always right, and
 *  a broken route must not be hammered); the next key change retries. */
export function loadGroupIcon(
  hostKey: string,
  groupId: string,
  iconKey: string | null | undefined,
): Promise<GroupIconEntry> {
  const hit = resolveCachedGroupIcon(hostKey, groupId, iconKey)
  if (hit) return Promise.resolve(hit)
  const flightKey = `${key(hostKey, groupId)}#${iconKey ?? ''}`
  const pending = inflight.get(flightKey)
  if (pending) return pending
  const p = fetchProjectGroupIcon(primaryScope(), groupId)
    .then((res): GroupIconEntry => {
      const entry: GroupIconEntry = {
        found: res.found,
        dataUrl: res.dataUrl,
        iconKey: res.iconKey !== undefined ? res.iconKey : iconKey,
      }
      setCachedGroupIcon(hostKey, groupId, entry)
      return entry
    })
    .catch((): GroupIconEntry => {
      const miss: GroupIconEntry = { found: false, dataUrl: null, iconKey }
      setCachedGroupIcon(hostKey, groupId, miss)
      return miss
    })
    .finally(() => {
      inflight.delete(flightKey)
    })
  inflight.set(flightKey, p)
  return p
}

/** Drop a group's cached icon across ALL hosts (group ids are UUIDs, so
 *  a suffix match can't collide across hosts) — the older-daemon
 *  groups-changed invalidation. No `groupId` drops everything:
 *  correctness over thrift. */
export function dropCachedGroupIcon(groupId?: string): void {
  if (!groupId) {
    cache.clear()
    return
  }
  const suffix = `:${groupId}`
  for (const k of cache.keys()) {
    if (k.endsWith(suffix)) cache.delete(k)
  }
}
