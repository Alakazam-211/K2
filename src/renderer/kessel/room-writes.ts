// Home M5 — what a usable remote room may write on its server
// (prd-home-multi-server-client MS38 writes, MS42, MS72; plan M5).
//
// A remote room on Home runs on `remoteRoomScope(B)`: every request goes to
// B with B's login, never to the window's server. On top of that the request
// layer (`assertScopeMayWrite`) sends a write ONLY when its route is listed
// here. The list is the room's own work on its server — typing and terminal
// control, its tab layout, closes, its chat history, its files, its pinned
// chat, its heartbeats, its worktrees. It is deliberately not a blanket open:
// projects/delete, presets, settings, people, mail, publish, API keys,
// sandbox/open (owner token only), Finder or a browser on the daemon's
// machine (`fs/open-finder`, `fs/open-external` — a link clicked in a room
// opens on this computer, `lib/terminal-link-open.ts`) and every other route
// stay refused from a room. Those are server management: "Open B's server"
// (switch) does them (plan decision 6).
//
// A GET-shaped verb (an older route that writes on GET) is listed too; its
// call site checks `assertScopeMayWrite` / `scopeMayWrite` itself, since the
// request layer only checks POSTs.
//
// This computer's Tauri commands are a separate gate (`lib/local-only-
// actions.ts`, MS57/MS67): a remote room runs none of them.
//
// Adding a route: say why a room needs it, and make sure the code that sends
// it uses the ROOM's scope (`room.scope`), never `primaryScope()`.

/** route → why a remote room sends it to its own server. */
export const ROOM_WRITES: Readonly<Record<string, string>> = Object.freeze({
  // Keep-alive (MS39).
  'projects/activate': 'tells B the room is open, so B does not reap the workspace',

  // Terminals: real spawns for new tabs and splits, closes, refresh.
  'sessions/v2/spawn': 'a pane starts or attaches its session on B (new tabs and splits spawn)',
  'sessions/v2/close': 'a tab close ends its session on B (reason tab_close, MS42)',
  'sessions/v2/refresh': 'the session chrome refresh restarts a session on B',
  'terminal/pin-size': 'the tab menu pins a session size on B',
  'terminal/send-message': 'compose, Chat about and chat-history sends type into a session on B',
  'workspace/ensure-pinned-chat': 'the pinned Chat starts B’s canonical chat session',
  'session/set-surfaced': 'closing a heartbeat tab minimizes it on B instead of killing it',
  'agents/ensure-cli': 'an agent launch (+ menu, Ctrl+1–9) makes sure that CLI is ready on B',
  'workspace/set-chat-session': 'picking a chat for the pinned Chat on B (GET-shaped verb)',
  'agents/lock': 'the pinned Chat / worktree chat claims its agent lock on B (GET-shaped verb)',
  // Legacy (Alacritty) terminal panes, daemon-owned on B.
  'terminal/create': 'a legacy terminal pane starts on B',
  'terminal/kill': 'a legacy terminal pane closes on B',
  'terminal/kill-foreground': 'Ctrl+C on a legacy pane’s foreground job on B',
  'terminal/resize': 'a legacy pane resizes its PTY on B',
  'terminal/set-focus': 'a legacy pane reports focus to B',
  'terminal/scroll': 'a legacy pane scrolls on B',
  'terminal/lifecycle-write': 'a legacy pane writes its lifecycle marker on B',

  // The tab layout (MS42: baseRevision, 409 then merge).
  'workspace-layouts/save': 'the room saves B’s tab layout with the revision check',
  'workspace/set-tab-title': 'a tab rename on B',

  // Chat history.
  'chat/rename': 'rename a chat on B',
  'chat/archive': 'archive a chat on B',
  'chat/restore': 'restore an archived chat on B',
  'chat/toggle-pin': 'pin a chat on B',
  'chat/continue-seed': 'continue a chat in a new session on B',
  'sandbox/reopen': 'reopen a sandboxed chat from B’s chat history',

  // Files in B's workspace (drag and drop within B; never across servers).
  'fs/create': 'new file or folder in B’s workspace',
  'fs/rename': 'rename in B’s workspace',
  'fs/delete': 'delete (to B’s trash) in B’s workspace',
  'fs/move': 'drag and drop within B’s workspace',
  'fs/copy': 'copy within B’s workspace',
  'fs/duplicate': 'duplicate within B’s workspace',
  'fs/write-file': 'save a file tab on B',
  'fs/upload-binary': 'a file dropped on B’s terminal is uploaded to B',
  'fs/upload-chunk': 'a large file dropped on B’s terminal is uploaded to B',
  'fs/compress': 'compress in B’s workspace',
  'fs/compress-cancel': 'cancel a compress on B',
  'fs/extract': 'extract in B’s workspace',
  'fs/extract-cancel': 'cancel an extract on B',
  'fs/search-tree': 'file search in B’s workspace (a read sent as POST)',
  'workspace/resources/add': 'add a Workspace Resource in B’s Files drawer',
  'workspace/resources/remove': 'remove a Workspace Resource in B’s Files drawer',

  // Changes panel on B's checkout.
  'git/stage': 'stage a file in B’s checkout',
  'git/unstage': 'unstage a file in B’s checkout',
  'git/stage-all': 'stage everything in B’s checkout',
  'git/commit': 'commit in B’s checkout',

  // Heartbeats (row launch and toggle; the scheduler is B's).
  'heartbeat/launch': 'Launch on a heartbeat row fires it on B (GET-shaped verb)',
  'heartbeat/enable': 'the heartbeat row toggle on B (GET-shaped verb)',

  // The Workspace panel.
  'workspace/set': 'the drawer’s completion bell and Hide API sessions toggle on B',
  'git/create-worktree': 'New worktree in B’s workspace',
  'workspaces/delete': 'Close Worktree on B',
  'git/remove-worktree': 'Recycle Worktree on B',

  // Thread overlay.
  'thread/post': 'post a Thread message to the agent on B',
  'thread/answer': 'answer a Thread ask on B',
  'thread/void': 'void a Thread message on B',
})

/** The routes, as the request layer checks them. */
export const ROOM_WRITE_ROUTES: ReadonlySet<string> = new Set(Object.keys(ROOM_WRITES))
