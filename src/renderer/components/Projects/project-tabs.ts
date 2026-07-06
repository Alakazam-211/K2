// Projects V1 §6.7 (live-review ergonomics) — pure logic for
// dashboards-AS-tabs (§6.7.6) and the Esc-to-pane rule (§6.7.4). No
// React/Tauri imports so every rule is unit-testable in isolation
// (project-tabs.test.ts) — the dashboard-layout.ts discipline.
//
//   - the project tab row = one tab per dashboard (name, `position`
//     order) + the Feedback tab; the SAME shape drives the Settings →
//     Projects Dashboards block,
//   - active-tab resolution survives dashboard delete/reorder (an
//     unknown/stale selection falls back to the first dashboard),
//   - reorder = left/right moves over the id order (the
//     dashboard/reorder route takes the FULL order array),
//   - Esc focus target: the dashboard's last-used terminal pane,
//     falling back to its first terminal pane; none → null (no-op).

import { isTerminalPane, readingOrder, type LayoutNode } from './dashboard-layout'

/** The non-dashboard tab. Dashboard ids are UUIDs, so the sentinel can
 *  never collide with one. */
export const FEEDBACK_TAB = 'feedback'

/** Tab order for the row: `position` ascending (the daemon's write
 *  order), tie-broken by createdAt then id so the row is stable even
 *  against duplicate positions from older rows. */
export function orderedDashboards<
  T extends { id: string; position: number; createdAt: number },
>(dashboards: T[]): T[] {
  return [...dashboards].sort(
    (a, b) => a.position - b.position || a.createdAt - b.createdAt || a.id.localeCompare(b.id),
  )
}

/** Resolve the ACTIVE tab from a requested one: 'feedback' sticks; a
 *  live dashboard id sticks; anything stale/unknown (deleted dashboard,
 *  fresh mount) falls back to the first dashboard — or Feedback when
 *  the project somehow has none. */
export function resolveActiveTab(
  requested: string | null,
  dashboardIds: string[],
): string {
  if (requested === FEEDBACK_TAB) return FEEDBACK_TAB
  if (requested !== null && dashboardIds.includes(requested)) return requested
  return dashboardIds[0] ?? FEEDBACK_TAB
}

/** One left/right move in the id order (§6.7.6 reorder buttons) — the
 *  new FULL order for the reorder route, or null when it's a no-op
 *  (unknown id, or already at that edge). */
export function moveDashboardId(
  order: string[],
  id: string,
  direction: -1 | 1,
): string[] | null {
  const from = order.indexOf(id)
  if (from < 0) return null
  const to = from + direction
  if (to < 0 || to >= order.length) return null
  const next = [...order]
  next[from] = next[to]
  next[to] = id
  return next
}

/** §6.7.4 — which terminal pane Esc should focus in the open
 *  dashboard: the last-used one when it's still a terminal pane in the
 *  tree, else the dashboard's FIRST terminal pane (reading order),
 *  else null (no pane → no-op). Returns the pane's workspaceId. */
export function resolveEscFocusPane(
  root: LayoutNode | null,
  lastWorkspaceId: string | null,
): string | null {
  const terminals = readingOrder(root).filter(isTerminalPane)
  if (lastWorkspaceId !== null && terminals.some((p) => p.workspaceId === lastWorkspaceId)) {
    return lastWorkspaceId
  }
  return terminals[0]?.workspaceId ?? null
}
