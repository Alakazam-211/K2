// Presence S2 — renderer mirror of the daemon's presence roster
// (presence/multiplayer arc, PRD §5.2).
//
// The daemon (S1, `crates/k2-daemon/src/presence.rs`) owns "who is
// connected to this daemon right now" and speaks two wire surfaces:
//   - APP-LEVEL `presence_changed` events (whole-set, last-write-wins —
//     the ActiveChanged convention) on the `/cli/sessions/events` bus, and
//   - `GET /cli/presence/roster` — the identical JSON as a snapshot.
//
// This store mirrors that truth 1:1: it registers on the app-level event
// registry (`onPresenceChanged`) for deltas and re-fetches the snapshot on
// every app-level `hello` (`onAppHello` — the reconnect-reconcile pattern
// `refreshActiveSnapshot` uses), so a drop window can never leave the
// roster stale. No polling loops; events + hello snapshots only.
//
// Graceful degradation: an OLDER daemon has no `/cli/presence/roster`
// route — its dispatcher 404s with the canonical `{"error":"route not
// found"}` body. On that specific failure we flip `supported` to false and
// every presence surface renders nothing. The hello re-probe keeps firing
// (hellos only happen on (re)connects — cheap), so a host that gains the
// capability mid-session starts mirroring on its next reconnect; a
// `presence_changed` frame arriving also proves support.
//
// Host switch: the roster is per-daemon state, so `onActiveHostChange`
// (#625 seam) clears it and resets `supported` optimistically — the new
// host's app-level WS reopens, its `hello` fires, and the snapshot
// re-establishes truth against the NEW host.

import { create } from 'zustand'
import { daemonCliGet } from '@/lib/daemon-cli'
import {
  onAppHello,
  onPresenceChanged,
  type PresenceRosterUser,
} from '@/stores/session-events'
import { onActiveHostChange } from '@/stores/connect-host'

// Re-export the wire row type under the name the components use — the
// shape is FROZEN against `presence.rs::RosterUser` (S1).
export type RosterUser = PresenceRosterUser

interface PresenceState {
  /** The live roster, owner-first then usernames ascending (daemon
   *  ordering preserved verbatim — whole-set replace on every update). */
  roster: RosterUser[]
  /** False once the active daemon proved it lacks the presence routes
   *  (roster GET 404'd). Every presence surface hides when false. */
  supported: boolean
  /** Whole-set replace (an event or snapshot arrived) — also proof the
   *  daemon speaks presence. */
  applyRoster: (roster: RosterUser[]) => void
  /** Fetch the `GET /cli/presence/roster` snapshot (hello reconcile /
   *  boot). A `route not found` rejection flips `supported` off; any
   *  other failure (transient network) leaves state untouched. */
  refreshRoster: () => Promise<void>
}

export const usePresenceStore = create<PresenceState>((set) => ({
  roster: [],
  supported: true,
  applyRoster: (roster) => set({ roster, supported: true }),
  refreshRoster: async () => {
    try {
      const snap = await daemonCliGet<{ roster: RosterUser[] }>('presence/roster')
      set({ roster: Array.isArray(snap?.roster) ? snap.roster : [], supported: true })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg.includes('route not found')) {
        // Older daemon — no presence surface. Hide everything.
        set({ roster: [], supported: false })
      } else {
        // Transient failure — keep the last-known roster; the next
        // hello (or presence_changed frame) reconciles.
        console.debug('[presence] roster snapshot skipped:', err)
      }
    }
  },
}))

// ── Roster display math (top-bar stack) ───────────────────────────────────

/** Max avatars in the top-bar stack before the `+N` overflow chip. */
export const MAX_ROSTER_AVATARS = 10

/** Split the roster into the visible avatar rows + the `+N` overflow
 *  count (0 = no chip). Pure — unit-tested directly. */
export function rosterDisplay(
  roster: RosterUser[],
  max: number = MAX_ROSTER_AVATARS,
): { visible: RosterUser[]; overflow: number } {
  if (roster.length <= max) return { visible: roster, overflow: 0 }
  return { visible: roster.slice(0, max), overflow: roster.length - max }
}

/** The top-bar roster hides entirely when the daemon doesn't speak
 *  presence OR when you're alone (just the owner — nothing to show). */
export function shouldShowRoster(roster: RosterUser[], supported: boolean): boolean {
  return supported && roster.length > 1
}

// ── Workspace-nav presence (S6) — path matcher + selector ─────────────────
//
// Roster rows carry `workspaces: string[]` — the `?path=` values of the
// user's live per-workspace event subscriptions. The nav joins those to
// its rows by the SAME prefix rule the daemon uses to route events to
// those subscriptions (`session_events_ws.rs::event_matches_workspace`),
// applied in BOTH directions: a user subscribed to a nested worktree
// shows on the parent project's row, and a user subscribed to the
// project shows on rows at or under it.

/** Max mini avatars on a nav row before the `+N` overflow chip (S6). */
export const MAX_WORKSPACE_AVATARS = 3

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

/** True when at most one human is subscribed to this workspace path
 *  (or presence is unsupported — treat as single-player). */
export function isSoleWorkspaceViewer(
  roster: RosterUser[],
  path: string,
  supported: boolean,
): boolean {
  if (!supported) return true
  if (!path) return true
  return usersForWorkspace(roster, path).length <= 1
}

/** The roster rows to show on a nav row for `path` (daemon ordering
 *  preserved). A user with multiple windows carries every subscribed
 *  workspace in one row, so they appear on EVERY matching nav row. An
 *  empty row path selects nobody (only subscriptions normalize to the
 *  match-all `/`, never nav rows). */
export function usersForWorkspace(roster: RosterUser[], path: string): RosterUser[] {
  if (!path) return []
  return roster.filter(
    (u) => Array.isArray(u.workspaces) && u.workspaces.some((w) => workspacePathsOverlap(w, path)),
  )
}

// ── Event wiring (module-scope, mirrors active-agents conventions) ────────
//
// Registered once at import. The registries are pure-callback singletons
// that survive the app-level WS teardown/reopen on a host switch, so this
// never needs re-wiring.

onPresenceChanged((e) => {
  usePresenceStore.getState().applyRoster(Array.isArray(e.roster) ? e.roster : [])
})

// Snapshot on every app-level (re)connect — the same reconcile point
// `refreshActiveSnapshot` uses (deltas may have been missed in the gap).
// This is ALSO the support re-probe against the (possibly new) host.
onAppHello(() => {
  void usePresenceStore.getState().refreshRoster()
})

// #625 — per-daemon state resets on a host switch; the new host's hello
// snapshot re-establishes truth. `supported` resets optimistically so a
// capable new host isn't hidden by the old host's 404.
onActiveHostChange(() => {
  usePresenceStore.setState({ roster: [], supported: true })
})
