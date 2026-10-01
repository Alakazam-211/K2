// Home M4 — actions that only make sense on THIS computer
// (prd-home-multi-server-client MS57; vs-live MS67).
//
// A room on another server shows that server's paths. A Tauri command that
// acts on this computer — Finder, an editor, a terminal app, a file
// watcher, the formatter, a local `k2_core` read — would act on this
// computer with the other server's path, or read this computer's daemon
// instead of the room's. In a remote room each one is either OFF (hidden or
// skipped) or ROUTED to that server's `/cli` route.
//
// `LOCAL_ONLY_ACTIONS` is the one list. `local-only-actions.test.ts` scans
// every renderer source for `invoke('k2so_*' | 'k2_core*' | 'projects_*' |
// 'fs_*' | 'read_worktree_file' | 'format_file')` and fails when a command
// is missing here, and lists every room-code site with its gate status.
//
// Which rooms may run them:
//   - a pinned room on `local` (this computer, while the window is on
//     another server): yes (MS57);
//   - a pinned room on any other server: no;
//   - the primary room: `room.localCommands` is true today, even when the
//     window itself is on a remote server.
//
// TODO(M4-integrate): decide the primary room on a remote window. MS57 says
// these actions are hidden in a remote room, and a primary room on a remote
// window is one; today it keeps them (Sidebar, IconRail, the Focus-window
// menu, TabBar's Finder/terminal). The one-line change is `primaryRoom()`'s
// `localCommands` becoming a getter `!primaryScope().isRemote` (stores/room.ts);
// sites gated with `mayRunLocalActions(room)` already follow it.

import type { ServerScope } from '@/kessel/server-scope'

/** Is this scope another machine's daemon (not this computer's)? */
export function isRemoteScope(scope: Pick<ServerScope, 'isRemote'>): boolean {
  return scope.isRemote
}

/** May code in `room` run this computer's Tauri commands right now? Both
 *  the room's own flag (M3) and its scope must say this computer. */
export function mayRunLocalActions(room: { localCommands: boolean; scope: Pick<ServerScope, 'isRemote'> }): boolean {
  return room.localCommands && !isRemoteScope(room.scope)
}

export type LocalOnlyKind = 'command' | 'event' | 'action'

export interface LocalOnlyAction {
  kind: LocalOnlyKind
  /** In a room on another server: hidden/skipped, or sent to its daemon. */
  inRemoteRoom: 'off' | 'route'
  /** The `/cli` route on THAT server, for `route`. */
  route?: string
  /** Why it is local-only. */
  why: string
}

/**
 * Every Tauri command, local event and OS action that acts on this
 * computer. Keyed by the command / event / action name.
 */
export const LOCAL_ONLY_ACTIONS: Readonly<Record<string, LocalOnlyAction>> = Object.freeze({
  // ── Finder, editors, terminal apps, windows (MS57) ──────────────────────
  projects_open_in_finder: { kind: 'command', inRemoteRoom: 'off', why: 'opens Finder on this computer with the other server’s path' },
  projects_open_in_editor: { kind: 'command', inRemoteRoom: 'off', why: 'opens an editor on this computer with the other server’s path' },
  projects_open_in_terminal: { kind: 'command', inRemoteRoom: 'off', why: 'opens a terminal app on this computer with the other server’s path' },
  projects_get_editors: { kind: 'command', inRemoteRoom: 'off', why: 'lists editors installed on this computer (feeds Open in …)' },
  projects_get_all_editors: { kind: 'command', inRemoteRoom: 'off', why: 'lists editors installed on this computer' },
  projects_refresh_editors: { kind: 'command', inRemoteRoom: 'off', why: 'rescans editors installed on this computer' },
  projects_open_focus_window: { kind: 'command', inRemoteRoom: 'off', why: 'looks the project up on this computer’s daemon; the Focus window has no host in its label' },
  projects_pick_folder: { kind: 'command', inRemoteRoom: 'off', why: 'a folder picker on this computer' },
  'fs/open-finder': { kind: 'action', inRemoteRoom: 'off', why: 'the daemon route opens Finder on the daemon’s machine (the other server’s screen)' },
  'drag-out': { kind: 'action', inRemoteRoom: 'off', why: 'a file-tree drag to Finder hands the OS another server’s path; a drop into a room on another server crosses servers (MS4)' },

  // ── Files ───────────────────────────────────────────────────────────────
  fs_watch_dir: { kind: 'command', inRemoteRoom: 'off', why: 'watches this computer’s disk; use that server’s fs_changed bus and the 2 s poll' },
  fs_unwatch_dir: { kind: 'command', inRemoteRoom: 'off', why: 'pairs with fs_watch_dir' },
  'fs://change': { kind: 'event', inRemoteRoom: 'off', why: 'this computer’s watcher events' },
  fs_read_dir: { kind: 'command', inRemoteRoom: 'route', route: 'fs/read-dir', why: 'reads this computer’s disk' },
  format_file: { kind: 'command', inRemoteRoom: 'off', why: 'runs a formatter on this computer’s copy of the path' },
  read_worktree_file: { kind: 'command', inRemoteRoom: 'route', route: 'fs/read-file', why: 'reads this computer’s disk' },

  // ── Local k2_core reads and writes (MS67) ──────────────────────────────
  k2so_session_lookup_by_agent: { kind: 'command', inRemoteRoom: 'off', why: 'asks this computer’s daemon, not the room’s (must fix before M4)' },
  k2so_chat_refresh_broadcast: { kind: 'command', inRemoteRoom: 'off', why: 'broadcasts on this computer’s daemon (must fix before M4)' },
  k2so_inbox_count: { kind: 'command', inRemoteRoom: 'route', route: 'inbox/list', why: 'counts this computer’s inbox' },
  k2so_agents_list: { kind: 'command', inRemoteRoom: 'route', route: 'agents/list', why: 'lists this computer’s agents' },
  k2so_agents_review_queue: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s review queue' },
  k2so_agents_build_launch: { kind: 'command', inRemoteRoom: 'off', why: 'builds a launch on this computer' },
  k2so_agents_get_editor_context: { kind: 'command', inRemoteRoom: 'off', why: 'reads this computer’s agent files' },
  k2so_agents_preview_schedule: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s heartbeat installer' },
  k2so_agents_install_heartbeat: { kind: 'command', inRemoteRoom: 'off', why: 'installs a heartbeat on this computer' },
  k2so_agents_update_heartbeat_projects: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s heartbeat installer' },
  k2so_agents_teardown_workspace: { kind: 'command', inRemoteRoom: 'off', why: 'tears down a workspace on this computer' },
  k2so_heartbeat_set_session: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s heartbeat store' },
  k2so_heartbeat_list_all: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s heartbeat store' },
  k2so_heartbeat_fires_list_all: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s heartbeat store' },
  k2so_workspace_get_show_heartbeat_sessions: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s workspace setting' },
  k2so_skills_list: { kind: 'command', inRemoteRoom: 'off', why: 'this computer’s skills' },

  // ── This computer's daemon events re-emitted by Tauri (MS14) ───────────
  'sync:projects': { kind: 'event', inRemoteRoom: 'off', why: 'this computer’s daemon changed its projects' },
  'agent:lifecycle': { kind: 'event', inRemoteRoom: 'off', why: 'this computer’s daemon' },
  'cli:agent-launch': { kind: 'event', inRemoteRoom: 'off', why: 'this computer’s daemon' },
})

/** The catalog entry for `name`. Throws for an unlisted name, so a new
 *  local-only call cannot be gated without being listed. */
export function localOnlyAction(name: string): LocalOnlyAction {
  const a = LOCAL_ONLY_ACTIONS[name]
  if (!a) throw new Error(`${name} is not in LOCAL_ONLY_ACTIONS (lib/local-only-actions.ts)`)
  return a
}

/** May `room` run local-only `name`? Throws for an unlisted name. */
export function roomMayRun(room: { localCommands: boolean; scope: Pick<ServerScope, 'isRemote'> }, name: string): boolean {
  localOnlyAction(name)
  return mayRunLocalActions(room)
}
