// Home P1 (vs-live H15) — "open this workspace once that server is up".
//
// Opening a Home row on another saved server switches this window's
// server (`pickHost`). The switch clears the projects store and re-runs
// `fetchProjects`, whose restore branch would land on that host's
// `lastActiveProjectId`. This one-shot request wins over that restore: when
// the NEW host's list lands, the restore takes the requested workspace.
//
// It is keyed by the switcher host id (`'local'` or `ConnectHost.id`), the
// id `pickHost` activates, and expires so a cancelled sign-in cannot fire
// on some later, unrelated switch to the same server.

import type { ActiveHost } from '@/stores/connect-host'
import { findWorkspaceForRow, type HomeRowRef } from '@/lib/home-address'

export const PENDING_SELECT_TTL_MS = 5 * 60_000

export interface PendingHostSelect {
  hostId: string
  row: HomeRowRef
  expiresAt: number
  /** Runs after the workspace is selected (lands the window on Home). */
  onSelected?: () => void
}

let pending: PendingHostSelect | null = null

export function requestHostSelect(
  hostId: string,
  row: HomeRowRef,
  onSelected?: () => void,
  now: number = Date.now(),
): void {
  pending = { hostId, row, expiresAt: now + PENDING_SELECT_TTL_MS, onSelected }
}

export function peekHostSelect(): PendingHostSelect | null {
  return pending
}

export function clearHostSelect(): void {
  pending = null
}

/**
 * Called from the restore branch with the new host's list. Returns the
 * requested workspace when the request is for THIS host, still fresh, and
 * the workspace exists — and consumes the request either way once it is
 * for this host (found or not), or when it expired.
 */
export function takeHostSelect<P extends { id: string; handle?: string | null; name: string }>(
  active: ActiveHost,
  workspaces: P[],
  now: number = Date.now(),
): { workspace: P; onSelected?: () => void } | null {
  const p = pending
  if (!p) return null
  if (now > p.expiresAt) {
    pending = null
    return null
  }
  const activeId = active === 'local' ? 'local' : active.id
  if (p.hostId !== activeId) return null
  pending = null
  const workspace = findWorkspaceForRow(workspaces, p.row)
  if (!workspace) return null
  return { workspace, onSelected: p.onSelected }
}
