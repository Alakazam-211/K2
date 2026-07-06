// §6.7.1 + §6.7.7 — the project-group avatar: the group's ICON when one
// is set (fetched from `/cli/project-group/icon`, module-cached — the
// Sidebar/ProjectAvatar anatomy), else initials on the group's canonical
// `color`, else initials on a STABLE hashed palette pick. Used by the
// nav's collapsed icon rail AND the expanded rows (and the Settings
// preview), so every surface stays in lockstep.
//
// Fetch/cache (the ProjectAvatar idiom, host-aware): a synchronous cache
// check seeds state so a cached icon never flashes initials; a miss
// fetches once and caches per host+group (group-icon-cache.ts). The
// store's `revision` is an effect dep: `project-group:groups-changed`
// drops the group's cache entry then bumps revision, so a fresh upload
// (this client's or another's) refetches live.

import { useEffect, useState } from 'react'
import { fetchProjectGroupIcon } from './projects-api'
import { getCachedGroupIcon, setCachedGroupIcon } from './group-icon-cache'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { useProjectGroupsStore } from '@/stores/project-groups'

// The workspace default-color palette's accent family — hand-picked to
// read on the dark surface; the hash keeps a group's color stable
// across sessions and clients (it derives from the canonical group id).
// Exported for the Settings color-swatch row (§6.7.7 — picking from the
// same palette the fallback hashes into keeps the nav coherent).
export const GROUP_AVATAR_COLORS = [
  '#e06c75',
  '#d19a66',
  '#e5c07b',
  '#98c379',
  '#56b6c2',
  '#61afef',
  '#c678dd',
  '#be5046',
  '#4ec9b0',
  '#c586c0',
]

/** Stable per-group color: FNV-ish hash of the group id → palette. */
export function groupAvatarColor(groupId: string): string {
  let h = 0
  for (let i = 0; i < groupId.length; i++) {
    h = (h * 31 + groupId.charCodeAt(i)) >>> 0
  }
  return GROUP_AVATAR_COLORS[h % GROUP_AVATAR_COLORS.length]
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
  const hostKey = activeHostKey(useConnectHostStore((s) => s.activeHost))
  // groups-changed drops the cache entry THEN bumps revision — re-run
  // the effect so mounted avatars pick a fresh upload up live.
  const revision = useProjectGroupsStore((s) => s.revision)

  const [fetchedUrl, setFetchedUrl] = useState<string | null>(() => {
    if (iconUrl) return null
    // Check cache synchronously to avoid flash (ProjectAvatar idiom).
    const cached = getCachedGroupIcon(hostKey, groupId)
    return cached?.found && cached.dataUrl ? cached.dataUrl : null
  })

  useEffect(() => {
    // A prop icon wins — skip the query entirely.
    if (iconUrl) return

    const cached = getCachedGroupIcon(hostKey, groupId)
    if (cached) {
      setFetchedUrl(cached.found && cached.dataUrl ? cached.dataUrl : null)
      return
    }

    let cancelled = false
    fetchProjectGroupIcon(groupId)
      .then((result) => {
        setCachedGroupIcon(hostKey, groupId, result)
        if (!cancelled) setFetchedUrl(result.found && result.dataUrl ? result.dataUrl : null)
      })
      .catch(() => {
        // Advisory decoration — cache the miss so a broken route doesn't
        // hammer the daemon; the initials fallback is always right.
        setCachedGroupIcon(hostKey, groupId, { found: false, dataUrl: null })
        if (!cancelled) setFetchedUrl(null)
      })

    return () => {
      cancelled = true
    }
  }, [hostKey, groupId, iconUrl, revision])

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
        color: '#ffffff',
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
