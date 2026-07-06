// §6.7.7 — module-level cache for project-group ICONS (the
// Sidebar/ProjectAvatar iconCache idiom, group-shaped). Icons ride their
// own GET route (`/cli/project-group/icon`) because the dataUrl blob is
// deliberately NOT in list/show payloads; caching here keeps the nav's
// expanded rows + collapsed rail from re-fetching per render.
//
// Keys are `${hostKey}:${groupId}` (the project-groups last-seen-cursor
// idiom) so remoting into a server never shows the local host's icons.
// Invalidation: stores/project-groups.ts drops a group's entries on
// `project-group:groups-changed` (set-icon/set-color emit it), then its
// revision bump makes mounted avatars re-run their fetch effect — a
// fresh upload propagates everywhere without a reload.

export interface GroupIconEntry {
  found: boolean
  dataUrl: string | null
}

const cache = new Map<string, GroupIconEntry>()

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

/** Drop a group's cached icon across ALL hosts (group ids are UUIDs, so
 *  a suffix match can't collide across hosts) — the groups-changed
 *  invalidation hook. No `groupId` (malformed/legacy payload) drops
 *  everything: correctness over thrift. */
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
