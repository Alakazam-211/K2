// Projects V1 P8 (prd-projects-v1 §6.5) — pure logic for the
// per-project Settings surface. No React/Tauri imports so every rule
// is unit-testable in isolation (project-settings.test.ts), the
// dashboard-layout.ts discipline:
//
//   - the left master-list filter (the ProjectsSection search idiom,
//     restated over project GROUPS),
//   - the add-member picker's candidate set (registered workspaces
//     minus current members),
//   - the PoC-successor surface rule (§6.5: removing the PoC is
//     disabled with an explanation until a successor is chosen — the
//     daemon's 409 `poc_successor_required` is the backstop),
//   - "Add to dashboard" for the pinned-HTML browser: append an
//     `{kind:"htmlDoc",workspaceId,filePath}` column to a dashboard's
//     canonical layout via the P5 machinery (adopt → insert →
//     serialize), idempotent per (workspaceId, filePath).

import {
  adoptLayout,
  insertColumnAt,
  isHtmlDocPane,
  serializeDashboardLayout,
  type DashboardColumn,
} from './dashboard-layout'

/** Case-insensitive name filter for the left master list (the
 *  ProjectsSection `matchesSearch` idiom — groups have no path, so the
 *  name is the whole haystack). A blank query keeps everything. */
export function filterGroupsByQuery<T extends { name: string }>(groups: T[], query: string): T[] {
  const q = query.trim().toLowerCase()
  if (!q) return groups
  return groups.filter((g) => g.name.toLowerCase().includes(q))
}

/** The add-member picker's candidates: registered workspaces that are
 *  NOT already members, filtered by a case-insensitive name/path query
 *  (the workspace-picker idiom). */
export function addableWorkspaces<T extends { id: string; name: string; path: string }>(
  registered: T[],
  memberWorkspaceIds: Iterable<string>,
  query = '',
): T[] {
  const members = new Set(memberWorkspaceIds)
  const q = query.trim().toLowerCase()
  return registered.filter((w) => {
    if (members.has(w.id)) return false
    if (!q) return true
    return w.name.toLowerCase().includes(q) || w.path.toLowerCase().includes(q)
  })
}

/** §6.5 — why a member's Remove button is disabled, or null when
 *  removal may proceed. The ONLY block in V1 is the PoC-successor rule
 *  (no auto-reassignment; the daemon 409s as the backstop). */
export function removeMemberBlockedReason(
  workspaceId: string,
  pocWorkspaceId: string | null,
): string | null {
  if (pocWorkspaceId !== null && workspaceId === pocWorkspaceId) {
    return 'This agent is the Point of Contact — reassign the PoC to a successor first.'
  }
  return null
}

/** Index of the htmlDoc column matching (workspaceId, filePath), or -1
 *  (the #587 idempotence key — one pane per pinned doc). */
export function findHtmlDocColumn(
  columns: DashboardColumn[],
  workspaceId: string,
  filePath: string,
): number {
  return columns.findIndex(
    (c) => isHtmlDocPane(c.pane) && c.pane.workspaceId === workspaceId && c.pane.filePath === filePath,
  )
}

/** "Add to dashboard" (§6.5): append an htmlDoc column to the
 *  dashboard's canonical layout. Adopts the stored blob first (an
 *  untouched 'Main' materializes its PoC seed — same as the
 *  dashboard's first real edit), then appends at the end. Idempotent:
 *  a doc already on the dashboard returns `added: false` and the
 *  ORIGINAL blob untouched — callers skip the save. */
export function appendHtmlDocColumn(
  layoutJson: string,
  pocWorkspaceId: string | null,
  doc: { workspaceId: string; filePath: string },
): { layoutJson: string; added: boolean } {
  const columns = adoptLayout(layoutJson, pocWorkspaceId)
  if (findHtmlDocColumn(columns, doc.workspaceId, doc.filePath) >= 0) {
    return { layoutJson, added: false }
  }
  const next = insertColumnAt(columns, columns.length, {
    kind: 'htmlDoc',
    workspaceId: doc.workspaceId,
    filePath: doc.filePath,
  })
  return { layoutJson: serializeDashboardLayout(next), added: true }
}
