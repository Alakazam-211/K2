// The All groups modal's "Recent" icon row: the last icons this person picked
// on this computer. A per-viewer convenience in localStorage, never synced;
// storage can throw or come back empty, and then the row is just empty.

import { GROUP_ICON_NAME_RE } from '@/lib/group-icon-name'

export const GROUP_ICON_RECENTS_KEY = 'k2.homes.recentIcons.v1'
export const GROUP_ICON_RECENTS_MAX = 8

export function readRecentGroupIcons(): string[] {
  try {
    const raw = globalThis.localStorage?.getItem(GROUP_ICON_RECENTS_KEY)
    if (!raw) return []
    const v: unknown = JSON.parse(raw)
    if (!Array.isArray(v)) return []
    return v.filter((x): x is string => typeof x === 'string' && GROUP_ICON_NAME_RE.test(x)).slice(0, GROUP_ICON_RECENTS_MAX)
  } catch {
    return []
  }
}

/** Put `name` first (once). Returns the new list. */
export function rememberGroupIcon(name: string): string[] {
  const next = [name, ...readRecentGroupIcons().filter((n) => n !== name)].slice(0, GROUP_ICON_RECENTS_MAX)
  try {
    globalThis.localStorage?.setItem(GROUP_ICON_RECENTS_KEY, JSON.stringify(next))
  } catch {
    // A per-viewer convenience: the pick itself is already saved.
  }
  return next
}
