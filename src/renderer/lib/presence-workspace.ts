// The roster-to-workspace join (presence S6), side-effect free: the store
// (`stores/presence.ts`) re-exports it, and controls that only need the
// join (the eye/pencil's "is anyone else here?") import it from here
// without loading the store's event wiring.
//
// Roster rows carry `workspaces: string[]` — the `?path=` values of the
// user's live per-workspace event subscriptions. The join uses the SAME
// prefix rule the daemon uses to route events to those subscriptions
// (`session_events_ws.rs::event_matches_workspace`), applied in BOTH
// directions.

import type { PresenceRosterUser } from '@/stores/session-events'

/** One direction of the join: does `subscribed` cover `path`? EXACT
 *  mirror of the daemon's `event_matches_workspace` normalization
 *  (`crates/k2-daemon/src/session_events_ws.rs`): trim ALL trailing
 *  slashes off the subscribed path, then match when the trimmed paths
 *  are equal OR `path` starts with `<trimmed>/` — the appended slash
 *  enforces the segment boundary (`/a/b` must NOT cover `/a/bc`). An
 *  empty subscribed path normalizes to the `/` prefix (covers every
 *  absolute path), same as the daemon. */
function pathCovers(subscribed: string, path: string): boolean {
  const trimmed = subscribed.replace(/\/+$/, '')
  const prefixWithSlash = trimmed === '' ? '/' : `${trimmed}/`
  return path.replace(/\/+$/, '') === trimmed || path.startsWith(prefixWithSlash)
}

/** Symmetric prefix overlap: true when either path sits at or under the
 *  other (after the daemon's trailing-slash normalization). Pure —
 *  unit-tested directly. */
export function workspacePathsOverlap(a: string, b: string): boolean {
  return pathCovers(a, b) || pathCovers(b, a)
}

/** The roster rows to show on a nav row for `path` (daemon ordering
 *  preserved). A user with multiple windows carries every subscribed
 *  workspace in one row, so they appear on EVERY matching nav row. An
 *  empty row path selects nobody (only subscriptions normalize to the
 *  match-all `/`, never nav rows). */
export function usersForWorkspace(roster: PresenceRosterUser[], path: string): PresenceRosterUser[] {
  if (!path) return []
  return roster.filter(
    (u) => Array.isArray(u.workspaces) && u.workspaces.some((w) => workspacePathsOverlap(w, path)),
  )
}
