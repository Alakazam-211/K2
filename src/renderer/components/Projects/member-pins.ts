// Per-project-group "pin member to top of Members list" preference.
// Thin-client localStorage (same spirit as Active-bar pin *order* UX on the
// agents page) — not multi-client SSOT. Keyed by project group id + workspace
// membership ids.

import type { ProjectGroupHtmlDoc, ProjectGroupMemberInfo } from './projects-api'

export function memberPinsStorageKey(groupId: string): string {
  return `k2:project-nav:pinned-members:${groupId}`
}

/** Load pinned workspace ids for a group (order = pin order, first = top). */
export function loadPinnedMemberIds(groupId: string): string[] {
  if (!groupId) return []
  try {
    const raw = localStorage.getItem(memberPinsStorageKey(groupId))
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter((x): x is string => typeof x === 'string' && x.length > 0)
  } catch {
    return []
  }
}

export function savePinnedMemberIds(groupId: string, ids: string[]): void {
  if (!groupId) return
  try {
    if (ids.length === 0) localStorage.removeItem(memberPinsStorageKey(groupId))
    else localStorage.setItem(memberPinsStorageKey(groupId), JSON.stringify(ids))
  } catch {
    /* storage full / disabled */
  }
}

/** Toggle pin; returns the new ordered id list. */
export function togglePinnedMember(groupId: string, workspaceId: string): string[] {
  const cur = loadPinnedMemberIds(groupId)
  const next = cur.includes(workspaceId)
    ? cur.filter((id) => id !== workspaceId)
    : [...cur, workspaceId]
  savePinnedMemberIds(groupId, next)
  return next
}

/**
 * Pinned members first (stable order of `pinnedIds`), then the rest in their
 * original order. Drop pin ids that are no longer members.
 */
export function sortMembersPinnedFirst(
  members: ProjectGroupMemberInfo[],
  pinnedIds: string[],
): { sorted: ProjectGroupMemberInfo[]; pinnedCount: number } {
  if (members.length === 0) return { sorted: [], pinnedCount: 0 }
  const byId = new Map(members.map((m) => [m.workspaceId, m]))
  const pinned: ProjectGroupMemberInfo[] = []
  const pinnedSet = new Set<string>()
  for (const id of pinnedIds) {
    const m = byId.get(id)
    if (m) {
      pinned.push(m)
      pinnedSet.add(id)
    }
  }
  const rest = members.filter((m) => !pinnedSet.has(m.workspaceId))
  return { sorted: [...pinned, ...rest], pinnedCount: pinned.length }
}

// ── Resources (pinned HTML docs) — same UX, key = workspaceId + filePath ──

/** Stable key for a resource row (workspace + absolute path). */
export function resourceDocKey(doc: { workspaceId: string; filePath: string }): string {
  return `${doc.workspaceId}\0${doc.filePath}`
}

export function resourcePinsStorageKey(groupId: string): string {
  return `k2:project-nav:pinned-resources:${groupId}`
}

export function loadPinnedResourceKeys(groupId: string): string[] {
  if (!groupId) return []
  try {
    const raw = localStorage.getItem(resourcePinsStorageKey(groupId))
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter((x): x is string => typeof x === 'string' && x.length > 0)
  } catch {
    return []
  }
}

export function savePinnedResourceKeys(groupId: string, keys: string[]): void {
  if (!groupId) return
  try {
    if (keys.length === 0) localStorage.removeItem(resourcePinsStorageKey(groupId))
    else localStorage.setItem(resourcePinsStorageKey(groupId), JSON.stringify(keys))
  } catch {
    /* storage full / disabled */
  }
}

export function togglePinnedResource(
  groupId: string,
  doc: { workspaceId: string; filePath: string },
): string[] {
  const key = resourceDocKey(doc)
  const cur = loadPinnedResourceKeys(groupId)
  const next = cur.includes(key) ? cur.filter((k) => k !== key) : [...cur, key]
  savePinnedResourceKeys(groupId, next)
  return next
}

/**
 * User-pinned resources first (pin order), then the rest alphabetical by
 * fileName (case-insensitive), then filePath for stability.
 */
export function sortResourcesPinnedThenAlpha(
  docs: ProjectGroupHtmlDoc[],
  pinnedKeys: string[],
): { sorted: ProjectGroupHtmlDoc[]; pinnedCount: number } {
  if (docs.length === 0) return { sorted: [], pinnedCount: 0 }
  const byKey = new Map(docs.map((d) => [resourceDocKey(d), d]))
  const pinned: ProjectGroupHtmlDoc[] = []
  const pinnedSet = new Set<string>()
  for (const k of pinnedKeys) {
    const d = byKey.get(k)
    if (d) {
      pinned.push(d)
      pinnedSet.add(k)
    }
  }
  const rest = docs
    .filter((d) => !pinnedSet.has(resourceDocKey(d)))
    .slice()
    .sort((a, b) => {
      const an = (a.fileName || a.filePath).toLowerCase()
      const bn = (b.fileName || b.filePath).toLowerCase()
      if (an !== bn) return an < bn ? -1 : 1
      const ap = a.filePath.toLowerCase()
      const bp = b.filePath.toLowerCase()
      if (ap !== bp) return ap < bp ? -1 : 1
      return a.workspaceId < b.workspaceId ? -1 : a.workspaceId > b.workspaceId ? 1 : 0
    })
  return { sorted: [...pinned, ...rest], pinnedCount: pinned.length }
}
