// §6.7.1 + §6.7.7 — the project-group avatar: the group's ICON when one
// is set (fetched from `/cli/project-group/icon`, module-cached — the
// Sidebar/ProjectAvatar anatomy), else initials on the group's canonical
// `color`, else initials on a STABLE hashed palette pick. Used by the
// nav's collapsed icon rail AND the expanded rows (and the Settings
// preview), so every surface stays in lockstep.
//
// Fetch/cache (the ProjectAvatar idiom, host-aware) — `useGroupIcon`:
// a synchronous cache read during render, so a cached icon never flashes
// initials; a miss fetches once (single-flight) and caches per host+group
// (group-icon-cache.ts). 0.46.1: the cached entry is checked against the
// row's `iconKey`, so a new upload (this client's or another's) refetches
// exactly once and a rename, pin or chat line never does. Older daemons
// (no key) keep the old path: groups-changed drops the entry and the
// project's revision re-runs the fetch.

import { useEffect, useState } from 'react'
import {
  getCachedGroupIcon,
  loadGroupIcon,
  resolveCachedGroupIcon,
  type GroupIconEntry,
} from './group-icon-cache'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { selectGroupRevision, useProjectGroupsStore } from '@/stores/project-groups'

// THE SAME default palette workspaces offer (Settings → Workspaces color
// row) so projects and workspaces share one color language; the hash
// keeps a group's fallback color stable across sessions and clients
// (it derives from the canonical group id). Exported for the Settings
// color-swatch row (§6.7.7 — the custom-hex input covers everything else).
export const GROUP_AVATAR_COLORS = [
  '#3b82f6',
  '#ef4444',
  '#22c55e',
  '#f59e0b',
  '#a855f7',
  '#ec4899',
  '#06b6d4',
  '#64748b',
]

/** Stable per-group color: FNV-ish hash of the group id → palette. */
export function groupAvatarColor(groupId: string): string {
  let h = 0
  for (let i = 0; i < groupId.length; i++) {
    h = (h * 31 + groupId.charCodeAt(i)) >>> 0
  }
  return GROUP_AVATAR_COLORS[h % GROUP_AVATAR_COLORS.length]
}

/** The project's icon from the shared cache, fetching it (once) when the
 *  cache has nothing valid for `iconKey`. `iconKey` defaults to the
 *  loaded list row's; pass the show view's when you have it. `null` while
 *  unknown (render the initials tile). `skip` disables it (a caller with
 *  its own dataUrl). */
export function useGroupIcon(
  groupId: string,
  iconKeyOverride?: string | null,
  skip = false,
): GroupIconEntry | null {
  const hostKey = activeHostKey(useConnectHostStore((s) => s.activeHost))
  const listKey = useProjectGroupsStore((s) => s.groups?.find((g) => g.id === groupId)?.iconKey)
  const iconKey = iconKeyOverride !== undefined ? iconKeyOverride : listKey
  // Older daemons: groups-changed drops the entry, then this moves.
  const groupRevision = useProjectGroupsStore(selectGroupRevision(groupId))

  // The cache is the state; this only re-renders once a load lands.
  const [, setLoads] = useState(0)

  useEffect(() => {
    if (skip) return
    if (resolveCachedGroupIcon(hostKey, groupId, iconKey)) return
    let cancelled = false
    void loadGroupIcon(hostKey, groupId, iconKey).then(() => {
      if (!cancelled) setLoads((n) => n + 1)
    })
    return () => {
      cancelled = true
    }
  }, [hostKey, groupId, iconKey, groupRevision, skip])

  if (skip) return null
  // Render-time cache read, so a cached icon never flashes initials.
  // While a NEW icon (new key) loads, keep showing the previous one for
  // this project rather than dropping to initials for one round trip.
  return (
    resolveCachedGroupIcon(hostKey, groupId, iconKey) ?? getCachedGroupIcon(hostKey, groupId) ?? null
  )
}

export default function ProjectGroupAvatar({
  name,
  groupId,
  size = 28,
  color = null,
  iconUrl = null,
}: {
  name: string
  groupId: string
  size?: number
  /** The group's canonical `color` (list/show rows carry it); null →
   *  the hashed-palette fallback. */
  color?: string | null
  /** Bypass the fetch with a known icon (data URL) — the Settings
   *  preview passes its already-fetched copy; nav callers pass none and
   *  the avatar fetches/caches itself. */
  iconUrl?: string | null
}): React.JSX.Element {
  // A prop icon wins — skip the query entirely.
  const icon = useGroupIcon(groupId, undefined, !!iconUrl)
  const fetchedUrl = icon?.found && icon.dataUrl ? icon.dataUrl : null

  const fallbackColor = color ?? groupAvatarColor(groupId)
  const shownUrl = iconUrl ?? fetchedUrl

  if (shownUrl) {
    return (
      <span
        className="flex-shrink-0"
        style={{
          width: size,
          height: size,
          border: `2px solid ${fallbackColor}`,
          overflow: 'hidden',
          display: 'block',
        }}
      >
        <img
          src={shownUrl}
          alt={name}
          style={{
            width: '100%',
            height: '100%',
            objectFit: 'cover',
            objectPosition: 'center',
            display: 'block',
          }}
        />
      </span>
    )
  }

  return (
    <span
      className="flex items-center justify-center flex-shrink-0"
      style={{
        width: size,
        height: size,
        backgroundColor: fallbackColor,
        color: 'var(--color-on-accent)',
        fontSize: size * 0.5,
        fontWeight: 700,
        lineHeight: 1,
        fontFamily: 'inherit',
      }}
    >
      {(name.trim()[0] ?? '?').toUpperCase()}
    </span>
  )
}
