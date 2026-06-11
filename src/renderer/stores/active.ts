// useActiveStore — the canonical Active mirror (#672,
// .k2so/prds/daemon-canonical-active.md §1).
//
// The DAEMON owns the Active set: a workspace is Active iff it's pinned
// (manually_active) OR was interacted-with inside `active_window_hours`.
// The renderer no longer DERIVES Active — it mirrors the daemon's set
// 1:1 via a snapshot (GET /cli/projects/active) on connect/host-switch
// plus full-set `active_changed` deltas pushed over the session-events
// WS. Both paths replace the whole set (last-write-wins on a monotonic
// snapshot — see PRD §4.4), so convergence is trivial and order-
// independent across multiple connected clients.
//
// This store is intentionally tiny: it holds the mirror and two setters.
// The app-level subscription that feeds it lives in session-events.ts
// (`subscribeToActiveState`); the gestures that mutate the canonical set
// live in projects.ts (activate/pin/dismiss → routes). The Active bar
// reads `activeProjectIds` from here when the host supports
// `canonical-active`, and falls back to the local window derivation
// otherwise.

import { create } from 'zustand'
import { clampActiveWindowHours } from '@/stores/settings'

export interface ActiveState {
  /** The canonical set of Active project IDs, mirrored from the daemon.
   *  A Set so membership checks in the Active bar are O(1). */
  activeProjectIds: Set<string>
  /** The daemon's configured Active window (hours). Carried alongside the
   *  set so the renderer reflects a mid-session window change the daemon
   *  pushes, and so any local fallback derivation can reuse it. */
  activeWindowHours: number

  /** Replace the mirror from a fresh GET /cli/projects/active snapshot
   *  (connect / host-switch). Full-set replace. */
  setFromSnapshot: (snapshot: { projectIds: string[]; activeWindowHours: number }) => void
  /** Apply an `active_changed` delta. The daemon emits the WHOLE set, so
   *  this is also a full-set replace (last-write-wins). */
  applyActiveChanged: (delta: { activeProjectIds: string[]; activeWindowHours: number }) => void
  /** Optimistic local echo for snappiness — add a project to the mirror
   *  before the daemon's delta returns. Reconciles to the next
   *  snapshot/delta (daemon is authoritative). */
  echoActive: (projectId: string) => void
  /** Optimistic local echo for an explicit remove (dismiss). */
  echoInactive: (projectId: string) => void
}

export const useActiveStore = create<ActiveState>((set) => ({
  activeProjectIds: new Set<string>(),
  activeWindowHours: 24,

  setFromSnapshot: ({ projectIds, activeWindowHours }) => {
    set({
      activeProjectIds: new Set(projectIds),
      activeWindowHours: clampActiveWindowHours(activeWindowHours),
    })
  },

  applyActiveChanged: ({ activeProjectIds, activeWindowHours }) => {
    set({
      activeProjectIds: new Set(activeProjectIds),
      activeWindowHours: clampActiveWindowHours(activeWindowHours),
    })
  },

  echoActive: (projectId) => {
    set((s) => {
      if (s.activeProjectIds.has(projectId)) return s
      const next = new Set(s.activeProjectIds)
      next.add(projectId)
      return { activeProjectIds: next }
    })
  },

  echoInactive: (projectId) => {
    set((s) => {
      if (!s.activeProjectIds.has(projectId)) return s
      const next = new Set(s.activeProjectIds)
      next.delete(projectId)
      return { activeProjectIds: next }
    })
  },
}))

/** Test-only reset to a clean mirror. */
export function __resetActiveStoreForTests(): void {
  useActiveStore.setState({ activeProjectIds: new Set<string>(), activeWindowHours: 24 })
}
