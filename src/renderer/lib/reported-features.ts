// Feature keys a daemon REPORTS in `/boot-status` `features` (Home M5).
//
// The version gates in `server-capabilities.ts` cannot tell main from the
// last release: both carry the same version string until the next cut. A
// behaviour a client must not guess at (e.g. whether `sessions/v2/spawn`
// honours `attach_only`) is therefore read from the list the daemon sends.
// `ServerScope.serverSupports` accepts these keys too (`kessel/server-scope`).
// Kept out of `server-capabilities.ts` so suites that mock that module still
// get the real registry.

/**
 * Feature keys the daemon REPORTS in `/boot-status` `features`, for what the
 * version string cannot tell apart (main and the last release share it).
 * A server that does not list the key — or reports no `features` at all,
 * like every daemon before the key was added — does not have it.
 */
export const REPORTED_FEATURES = {
  /** `sessions/v2/spawn` honours `attach_only`: a live session is returned
   *  as-is, anything else is `404 session_not_live`. Released daemons up to
   *  0.41.6 ignore the field and would SPAWN, so a view-only room on such a
   *  server never spawns a tab it has not seen live (`lib/room-spawn.ts`). */
  'spawn-attach-only': 'spawn-attach-only',
  /** `GET /cli/feedback/list-all` exists and `waiting-count` counts only
   *  tickets on a registered workspace. The Tickets page reads the host-wide
   *  list in one GET (with an Unlinked workspace group); a server without
   *  the key shows the badge as a dimmed `?` (prd-tickets-badge-orphans). */
  'tickets-list-all': 'tickets-list-all',
  /** `GET /cli/feedback/waiting-count` counts waiting + needs_discussion
   *  and takes `me=1`: only tickets assigned to the calling login, the
   *  board's "Waiting on me" count. The Tickets badge sends `me=1` only to
   *  a server with this key; an older one keeps its host-wide count. */
  'tickets-badge-me': 'tickets-badge-me',
  /** `GET /cli/thread/latest?addrs=` (prd-zen-mode-v1 Z41): the newest
   *  Thread item per addr in one request, for Zen's list previews. A server
   *  without the key gets one `GET /cli/thread?addr=&limit=1` per row. */
  'thread-latest': 'thread-latest',
  /** prd-daemon-activity-and-thread-working-v1 RL8/A34: the daemon owns agent
   *  activity (`activity_changed` + `GET /cli/activity/snapshot`). A client
   *  renders those rows; a server without the key gets RL13 (its
   *  `session_activity_changed` stream as-is). */
  'daemon-activity': 'daemon-activity',
  /** prd-zen-user-widgets-v2 UWB12a: `POST /cli/thread/post` takes
   *  `origin: {widget, garden}` on a person's own post (via=compose) from a
   *  Garden widget; the daemon stores it on the doc as `widget` and keeps the
   *  post out of the compose history. Send `origin` only to servers that
   *  list this key (an older server refuses nothing but drops the field and
   *  records the post in compose history). */
  'thread-widget-origin': 'thread-widget-origin',
  /** prd-thread-push-notifications-v1 §5.6: Thread push notifications are
   *  live on this server (`/cli/notify/*` routes, the agent-post trigger,
   *  `notify_prefs_changed`, and `viewing` frames on the overlay socket).
   *  It does NOT mean the server can reach the relay (`push` in
   *  `GET /cli/notify/prefs` says that). Settings → Notifications, the
   *  Agent tab's phone switch and the viewing sender all hide without it. */
  'thread-push-v1': 'thread-push-v1',
  /** Local Grok Bots (0.45.7): `/cli/grokbots/*` and `grokbots_changed`.
   *  A server without the key shows no Grok Bot rows and no Settings page. */
  'grokbots-v1': 'grokbots-v1',
  /** plan-agent-status-unify-v1 D1/D2 (0.46.0): activity rollups carry
   *  `live` (live PTYs, 0 = reaped) and `busy` (mid-turn or subagents
   *  running), and `presence/summary` `agentActivity` carries `heartbeat`,
   *  `live`, `busy`, `counts` plus idle entries. Without it the renderer
   *  keeps its own live-session set and reads working from
   *  `counts.working`; a missing `live` is unknown, never 0. */
  'activity-live-busy': 'activity-live-busy',
  /** agent-unread-tickets-v1 (0.46.4): `presence/summary` `agentActivity`
   *  entries carry the caller's `unread` / `waiting` / `muted` on that
   *  agent's Threads and its `ticketsWaiting` / `ticketsWaitingMe` (idle
   *  workspaces with any listed too). A remote Home row draws its unread
   *  and ticket badges only from a server with this key; an older server's
   *  rows show none (never guessed). */
  'agent-unread-tickets-v1': 'agent-unread-tickets-v1',
  /** thread-read-all-v1 (0.46.5): `POST /cli/thread/read-all {agent |
   *  projectId}` marks every Thread of one agent read for the caller (an
   *  agent row's badge sums them all, earlier chats' Threads included).
   *  "Mark all as read" on an agent row shows only for a server with it. */
  'thread-read-all-v1': 'thread-read-all-v1',
  /** prd-mobile-dev-v1 M-A13 (0.46.6): this server can run at least one
   *  device lane (`/cli/sim/*`, `/cli/sim/stream`, `open_mobile`). Without
   *  it the + menu has no Mobile, ⌘⇧M does nothing and `open_mobile` opens
   *  no tab, so no box shows a dead tab. `GET /cli/sim/status` lists the
   *  lanes. */
  'mobile-sim-v1': 'mobile-sim-v1',
  /** thread-read-v1 (0.46.1): per-person unread counts for Threads and
   *  project chats. `GET /cli/thread/unread`, `POST /cli/thread/read`,
   *  `POST /cli/project-group/read`, `POST /cli/thread/mute`, the
   *  `thread_read` / `project_read` frames, and `unread` / `waiting` /
   *  `muted` on `thread/latest` and `project-group/list` rows. Without it
   *  the client shows no unread dots and never posts a read
   *  (`stores/unread.ts`). */
  'thread-read-v1': 'thread-read-v1',
  /** homes-sync-v1 (0.46.1): `/cli/me/homes*` (a person's Homes synced
   *  between their devices), the person-scoped `homes_changed` /
   *  `homes_device_removed` frames and `serverId` on `/boot-status`. The
   *  desktop syncs its Homes with its own daemon only when it lists this. */
  'homes-sync-v1': 'homes-sync-v1',
  /** thread-attachments-v1 (0.46.1): file objects on Thread messages.
   *  `attachments` on `POST /cli/thread/post` (`[{path, line?, lineEnd?}]`,
   *  text optional when files ride) and `GET /cli/thread/attachment`. With
   *  it, files a person picks in the Thread's message box ride the post as
   *  attachments (S4); without it they go in as path text, as before. */
  'thread-attachments-v1': 'thread-attachments-v1',
  /** thread-images-v1 (0.46.3): `GET /cli/thread/attachment?mode=thumb&px=`
   *  answers a picture for an image attachment (SVG sanitized, HEIC
   *  converted where the box can). With it, an image attachment draws as
   *  the picture under the bubble (S3); without it, today's chip. */
  'thread-images-v1': 'thread-images-v1',
  /** ticket-attachments-v1 (0.46.6, prd-ticket-attachments-v1): files on
   *  ticket messages. `show` comments carry `id` and `attachments`;
   *  `attachments` on `POST /cli/feedback/comment`; the door
   *  `GET /cli/feedback/attachment?id=&commentId=&n=` (meta, preview, thumb,
   *  range, download). With it the ticket rail draws pictures and takes
   *  files in its message box; without it, chips only and no "+". */
  'ticket-attachments-v1': 'ticket-attachments-v1',
  /** permission-cards-v1 (0.46.3, Beta): listed only while the server's
   *  "Permission cards" switch is on. With it the Thread asks for the
   *  `cards=2` permission card view (`GET /cli/thread`, the overlay socket)
   *  and draws PermissionCard; without it nothing is asked and a permission
   *  card is the daemon's fallback (an ordinary card or line). */
  'permission-cards-v1': 'permission-cards-v1',
  /** thread-context-v1 (0.46.3 r10): `POST /cli/thread/post` takes the
   *  Board context chip (`context: {text, origin}`) on the person's own
   *  compose post, stores it as the doc's own `context` and gives the agent
   *  K2's quoted line before the `[from]` stamp. Without it the message goes
   *  without the context ("Not sent: <server> is older"), never folded into
   *  the body. */
  'thread-context-v1': 'thread-context-v1',
} as const

export type ReportedFeatureKey = keyof typeof REPORTED_FEATURES

export function isReportedFeature(feature: string): feature is ReportedFeatureKey {
  return Object.prototype.hasOwnProperty.call(REPORTED_FEATURES, feature)
}
