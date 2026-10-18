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
//   - whether a dashboard already shows an HTML file (the Add/Remove
//     button state; the daemon's add-pane/remove-pane routes do the
//     actual layout edit — 0.46.1 daemon-first, no client-side
//     read-modify-write of the whole layout),
//   - who may change what (0.46.1, Rosson 2026-10-18): from the login's
//     ROLE as the daemon reports it, never the window's eye/pencil.

import { adoptLayout, isHtmlDocPane, readingOrder, type LayoutNode } from './dashboard-layout'
import type { SettingEntry } from '@/components/Settings/searchManifest'

export type ProjectRole = 'owner' | 'admin' | 'member'

/** What the signed-in person may change on the project settings page.
 *  Mirrors the daemon floors (`project_group_routes.rs` is_admin_only +
 *  route_policy): name, pin and agents are any login; picture, color,
 *  dashboards and deleting the project are owner or admin. An unknown
 *  role (whoami not answered yet, an older daemon) leaves it to the
 *  daemon, like `roleMeetsFloor`. */
export interface ProjectSettingsCaps {
  canEdit: boolean
  canEditLooks: boolean
  canEditDashboards: boolean
  canDelete: boolean
}

export function projectSettingsCaps(role: ProjectRole | null): ProjectSettingsCaps {
  const admin = role === null || role === 'owner' || role === 'admin'
  return { canEdit: true, canEditLooks: admin, canEditDashboards: admin, canDelete: admin }
}

/** The one banner shown when something on the page is read-only for
 *  this person, or null when they can change everything. */
export function readOnlyBannerText(role: ProjectRole | null): string | null {
  if (projectSettingsCaps(role).canDelete) return null
  return (
    'You can rename and pin this project and change its agents. ' +
    'Only the owner or an admin can change its picture, color and dashboards, or delete it.'
  )
}

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
    return 'Make another agent the point of contact first.'
  }
  return null
}

/** Whether the tree already holds the htmlDoc pane for (workspaceId,
 *  filePath) — the #587 idempotence key: one pane per pinned doc. */
export function hasHtmlDocPane(
  root: LayoutNode | null,
  workspaceId: string,
  filePath: string,
): boolean {
  return readingOrder(root).some(
    (p) => isHtmlDocPane(p) && p.workspaceId === workspaceId && p.filePath === filePath,
  )
}

/** Whether the dashboard (as it draws — an untouched blob draws the
 *  point of contact's pane) already shows this HTML file. */
export function dashboardShowsDoc(
  layoutJson: string,
  pocWorkspaceId: string | null,
  doc: { workspaceId: string; filePath: string },
): boolean {
  return hasHtmlDocPane(adoptLayout(layoutJson, pocWorkspaceId), doc.workspaceId, doc.filePath)
}

/** Settings search entries for Settings → Projects (each id is the
 *  row's `data-settings-id`). */
export const PROJECT_GROUPS_MANIFEST: SettingEntry[] = [
  { id: 'project-groups.name', section: 'project-groups', label: 'Project name', description: 'Rename a project', keywords: ['rename', 'project', 'name'] },
  { id: 'project-groups.picture', section: 'project-groups', label: 'Project picture', description: 'Upload or remove a project picture', keywords: ['icon', 'image', 'avatar', 'picture'] },
  { id: 'project-groups.color', section: 'project-groups', label: 'Project color', description: 'The color behind the project picture', keywords: ['color', 'colour', 'swatch'] },
  { id: 'project-groups.pinned', section: 'project-groups', label: 'Show in Pinned', description: 'Pin a project for everyone on this server', keywords: ['pin', 'pinned', 'favorite'] },
  { id: 'project-groups.delete', section: 'project-groups', label: 'Delete project', description: 'Remove a project with its dashboards and chat', keywords: ['delete', 'remove', 'project'] },
  { id: 'project-groups.dashboards', section: 'project-groups', label: 'Dashboards', description: 'Add, rename, move and delete project dashboards', keywords: ['dashboard', 'tabs', 'layout'] },
  { id: 'project-groups.agents', section: 'project-groups', label: 'Project agents', description: 'Agents in a project and its point of contact', keywords: ['agents', 'members', 'point of contact', 'poc'] },
  { id: 'project-groups.add-agent', section: 'project-groups', label: 'Add agent to project', description: 'Add an agent to a project', keywords: ['add', 'agent', 'member'] },
]
