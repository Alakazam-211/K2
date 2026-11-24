// The agent each Home group last had on screen (Rosson 2026-11-24: "the
// agent you had focused in a group should be focused when you come back
// to that group"). Coming back to a group by click, swipe or ⌥⌘1–9
// opens its remembered agent instead of the top row (home-auto-select).
//
// Per-viewer view state, so it lives here and not in the daemon: one map
// in localStorage `k2.homes.groupFocus.v1`, group id → row address. A
// row address already names its server (`handle::host`), and groups span
// servers, so the key is the group and the value says which server's
// agent. Not synced: what one computer had on screen is that computer's.
//
// Storage can throw (private mode, a locked-down webview): every read and
// write is guarded, and a failed read is an empty map (the group's top row
// opens, as before).

export const HOME_GROUP_FOCUS_KEY = 'k2.homes.groupFocus.v1'

let cache: Record<string, string> | null = null

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

function load(): Record<string, string> {
  if (cache) return cache
  const out: Record<string, string> = {}
  try {
    const raw = storage()?.getItem(HOME_GROUP_FOCUS_KEY)
    const parsed: unknown = raw ? JSON.parse(raw) : null
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) if (typeof v === 'string') out[k] = v
    }
  } catch (err) {
    console.warn('[homes] group focus read failed:', err)
  }
  cache = out
  return out
}

/** The row address this group last had on screen, if any. */
export function rememberedGroupFocus(groupId: string): string | null {
  return load()[groupId] ?? null
}

/** Remember `address` as this group's agent on screen. `liveGroups` (the
 *  ids of the groups that exist) drops entries of deleted groups. */
export function rememberGroupFocus(groupId: string, address: string, liveGroups?: readonly string[]): void {
  const current = load()
  if (current[groupId] === address) return
  const next: Record<string, string> = {}
  const keep = liveGroups ? new Set(liveGroups) : null
  for (const [k, v] of Object.entries(current)) if (!keep || keep.has(k)) next[k] = v
  next[groupId] = address
  cache = next
  try {
    storage()?.setItem(HOME_GROUP_FOCUS_KEY, JSON.stringify(next))
  } catch (err) {
    console.warn('[homes] group focus write failed:', err)
  }
}

/** Tests: drop the in-memory copy, as a reload does (storage stays). */
export function __resetGroupFocusCacheForTests(): void {
  cache = null
}
