//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! every `/cli/*` route outside the areas that have a file of their own, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::{Admin, Member, NoLogin, Owner, Public, VoiceChat};
use super::{both, get, post, split, Route};

pub(super) const ROWS: &[Route] = &[
    get("/cli/activity/events", Member),
    // prd-daemon-activity-and-thread-working-v1 RL3: rows + rollups + seq.
    get("/cli/activity/snapshot", Member),
    // k2 sidecar v1: the "Allow hiring and managing agents" switch.
    post("/cli/agent-access/set", Admin),
    // Agent hire (0.45.6): instances list + recipe hire + teardown are
    // Admin (T7); links GET is Member (a passport sees its own workspace).
    get("/cli/agent-instances", Admin),
    post("/cli/agent-instances/hire", Admin),
    post("/cli/agent-instances/retire", Admin),
    post("/cli/agent-teams/link", Admin),
    get("/cli/agent-teams/links", Member),
    post("/cli/agent-teams/unlink", Admin),
    // Agent templates (prd-agent-templates-v1 T1): list + one card. Member
    // (anyone who may hire); agent passports read them too.
    get("/cli/agent-templates", Member),
    get("/cli/agent-templates/show", Member),
    post("/cli/agent/complete", Member),
    get("/cli/agent/conf", Member),
    // 0.45.5 app identity surface: an App pass (agent:handle /
    // agent:identity in the room) or the owner / a Connect login, the same
    // floor as set-handle and set-agent-display-name.
    post("/cli/agent/handle", Member),
    // `k2 agent harness` (0.46.6): moves a pinned chat. A person at the
    // Admin floor; an agent passport gets `harness_needs_person` (a card).
    post("/cli/agent/harness", Admin),
    both("/cli/agent/identity", Member),
    get("/cli/agent/list", Member),
    // `k2 agent refresh` (0.46.8): a new pinned chat on the same harness. A
    // person at the Admin floor; an agent passport by its own switch, or a card.
    post("/cli/agent/refresh", Admin),
    post("/cli/agent/reply", Member),
    post("/cli/agent/retire", Member),
    post("/cli/agent/update", Member),
    both("/cli/agentic", Member),
    post("/cli/agents-create-connections", Admin),
    post("/cli/agents-manage-skin", Owner),
    post("/cli/agents/archive-orphans", Member),
    // 0.46.4 desktop mirrors (`desktop_mirror_routes`: the desktop app
    // forwards its former in-process k2-core commands): this computer
    // only, so NoLogin here and owner-token-only in the handler, like the
    // Home avatar cache. Rows: agents build-launch / editor-context /
    // teardown-workspace, heartbeat preview-schedule / show-sessions,
    // skills list, workspace session-get. (`/cli/inbox/count` mirrors
    // `/cli/inbox/list` and keeps its Member floor.)
    post("/cli/agents/build-launch", NoLogin),
    // Which agent CLIs are installed (read only): ensure-cli's floor.
    get("/cli/agents/cli-status", Member),
    post("/cli/agents/create", Member),
    post("/cli/agents/delegate", Member),
    post("/cli/agents/delete", Member),
    post("/cli/agents/disable-workspace-claude-md", Member),
    post("/cli/agents/editor-context", NoLogin),
    post("/cli/agents/ensure-cli", Member),
    post("/cli/agents/generate-claude-md", Member),
    get("/cli/agents/heartbeat", Member),
    get("/cli/agents/heartbeat/action", Member),
    get("/cli/agents/heartbeat/noop", Member),
    // `k2 agent hire <dir>` on the shared hire core (same floor as the
    // Member routes it replaced); the hire root: read Member, set Owner.
    post("/cli/agents/hire", Member),
    split("/cli/agents/hire-root", Member, Owner),
    post("/cli/agents/launch", Member),
    get("/cli/agents/list", Member),
    post("/cli/agents/lock", Member),
    get("/cli/agents/profile", Member),
    post("/cli/agents/reap", Member),
    post("/cli/agents/regenerate-workspace-skill", Member),
    post("/cli/agents/run-workspace-ingest", Member),
    get("/cli/agents/running", Member),
    post("/cli/agents/save-agent-md", Member),
    post("/cli/agents/save-session-id", Member),
    post("/cli/agents/teardown-workspace", NoLogin),
    get("/cli/agents/triage", Member),
    post("/cli/agents/unlock", Member),
    get("/cli/agents/work", Member),
    get("/cli/agents/work/create", Member),
    get("/cli/agents/work/move", Member),
    post("/cli/api-keys/create", Owner),
    post("/cli/api-keys/disable", Owner),
    post("/cli/api-keys/enable", Owner),
    post("/cli/api-keys/hire-ceiling", Owner),
    get("/cli/api-keys/hire-ceiling-options", Owner),
    get("/cli/api-keys/list", Owner),
    post("/cli/api-keys/revoke", Owner),
    post("/cli/auth/change-password", VoiceChat),
    // phone-web-off (2026-10-24): Companion pairing. The pair redeem is
    // Public (the one-time code is the credential); minting a code, the
    // paired-phone list and revoke take a login (the owner sees and ends
    // everyone's, a Member only its own; the handlers enforce that).
    post("/cli/auth/device/pair", Public),
    post("/cli/auth/device/pair-code", Member),
    // 0.46.6 pair success: one code's state, for the actor that minted it
    // (the handler scopes it; anyone else gets 404 no_such_code).
    get("/cli/auth/device/pair-code/status", Member),
    post("/cli/auth/device/pair/check", Public),
    post("/cli/auth/device/revoke", Member),
    get("/cli/auth/devices", Member),
    post("/cli/auth/login", Public),
    post("/cli/auth/logout", VoiceChat),
    get("/cli/auth/whoami", VoiceChat),
    post("/cli/awareness/publish", Member),
    get("/cli/awareness/subscribe", Member),
    // Board files (`board_file/routes.rs`): any login; the handler checks
    // rights per file, the same way as docs. Gated by `board_file_preview`.
    get("/cli/board/file/blob", Member),
    post("/cli/board/file/blob/put", Member),
    post("/cli/board/file/copy", Member),
    post("/cli/board/file/copy/rescue", Member),
    post("/cli/board/file/create", Member),
    get("/cli/board/file/fields", Member),
    post("/cli/board/file/held/mine", Member),
    post("/cli/board/file/held/resolve", Member),
    get("/cli/board/file/info", Member),
    get("/cli/board/file/list", Member),
    post("/cli/board/file/merge", Member),
    get("/cli/board/file/read", Member),
    post("/cli/board/file/save", Member),
    post("/cli/board/file/start", Member),
    post("/cli/board/file/verb", Member),
    post("/cli/browser/notes", Member),
    post("/cli/browser/notes/redeliver", Member),
    post("/cli/browser/open-url", Member),
    post("/cli/canonical/detect-state", Member),
    // 0.46.7 (TC-17): what this server can do for Apps (any login; a
    // passport sees its own workspace's Apps and nodes).
    get("/cli/capabilities", Member),
    get("/cli/certs", Member),
    both("/cli/certs/config", Admin),
    post("/cli/certs/issue", Member),
    post("/cli/certs/renew", Member),
    post("/cli/certs/upload", Admin),
    post("/cli/chat/archive", Member),
    post("/cli/chat/continue-seed", Member),
    get("/cli/chat/custom-names", Member),
    get("/cli/chat/detect-active", Member),
    get("/cli/chat/discover-ide", Member),
    get("/cli/chat/list", Member),
    post("/cli/chat/migrate-ide", Member),
    get("/cli/chat/pinned", Member),
    post("/cli/chat/release-names", Member),
    post("/cli/chat/rename", Member),
    post("/cli/chat/restore", Member),
    get("/cli/chat/session-exists", Member),
    get("/cli/chat/session-path", Member),
    get("/cli/chat/storage-paths", Member),
    post("/cli/chat/toggle-pin", Member),
    get("/cli/chatter", Member),
    get("/cli/chatterlog", Member),
    post("/cli/checkin", Member),
    // `k2 ci` (0.46.10, C6a; `ci_routes`, on the compute router): status
    // and run are agent verbs (the workspace switch + its own workspace);
    // watch, freeze and init need Admin people or a passport for its own
    // workspace (the handler refuses a Member login `owner_only`).
    // release and evidence (0.46.11, C5/C6b): agent verbs like run.
    post("/cli/ci/evidence", Member),
    post("/cli/ci/freeze", Member),
    post("/cli/ci/init", Member),
    post("/cli/ci/release", Member),
    post("/cli/ci/run", Member),
    // Signer pairing (0.46.10 C3a, vs-live V17; `compute_trust`): a person
    // only (owner or Admin with a fresh confirm; the handler refuses every
    // passport). The SAS is answered only to that person.
    post("/cli/ci/signer/add", Admin),
    post("/cli/ci/signer/finish", Admin),
    post("/cli/ci/signer/remove", Admin),
    post("/cli/ci/signer/reveal", Admin),
    get("/cli/ci/signers", Admin),
    get("/cli/ci/status", Member),
    post("/cli/ci/watch", Member),
    post("/cli/claude-auth/install-scheduler", Member),
    post("/cli/claude-auth/refresh-now", Member),
    get("/cli/claude-auth/status", Member),
    post("/cli/claude-auth/uninstall-scheduler", Member),
    // Agent clone proposals (prd-clone-carries-threads-and-vault-v1 CL21).
    // A passport proposes/lists/withdraws its own; approve/deny/finish are
    // Owner/Admin people (the handler refuses passports).
    post("/cli/clone-proposal/approve", Admin),
    post("/cli/clone-proposal/deny", Admin),
    post("/cli/clone-proposal/finish", Admin),
    get("/cli/clone-proposal/list", Member),
    post("/cli/clone-proposal/new", Member),
    post("/cli/clone-proposal/withdraw", Member),
    post("/cli/clone/bundle", Member),
    post("/cli/clone/pack", Member),
    post("/cli/clone/pack-cleanup", Member),
    get("/cli/clone/pack-status", Member),
    // Names-only clone plan (CL25); the clone gate narrows in the arm.
    post("/cli/clone/preview", Member),
    post("/cli/clone/unpack", Member),
    // Clone carries vault secrets (0.46.0 slice A): Member floor like the
    // other clone doors; the handler narrows copies and confirms to
    // Owner/Admin (D1) and the clone gate (slice C) narrows further.
    post("/cli/clone/vault/confirm", Member),
    post("/cli/clone/vault/copy", Member),
    post("/cli/clone/vault/export", Member),
    post("/cli/clone/vault/import", Member),
    post("/cli/clone/vault/plan", Member),
    post("/cli/commit", Member),
    post("/cli/commit-merge", Member),
    get("/cli/companion/presets", Member),
    get("/cli/companion/projects", Member),
    get("/cli/companion/projects-summary", Member),
    get("/cli/companion/sessions", Member),
    // 0.45.7: the CLI's K2 Connect control-plane calls, made with the
    // daemon-held tunnel token. Logins: Owner/Admin (the handler's
    // `token_is_owner_or_admin`). Agent passports are not logins; they
    // pass as an agent verb and `connect_proxy` limits them to their own
    // workspace's nested labels (like `/cli/publish/run`).
    post("/cli/connect/proxy", Admin),
    both("/cli/connections", Member),
    post("/cli/context/add", Member),
    get("/cli/context/catalog", Member),
    post("/cli/context/catalog/create", Admin),
    post("/cli/context/catalog/delete", Admin),
    get("/cli/context/layers", Member),
    post("/cli/context/move", Member),
    post("/cli/context/regen", Member),
    post("/cli/context/remove", Member),
    post("/cli/context/set-enabled", Member),
    get("/cli/context/show", Member),
    post("/cli/daemon/app-update/progress", Admin),
    post("/cli/daemon/handoff", Admin),
    post("/cli/daemon/handoff/setup", Admin),
    get("/cli/daemon/handoff/status", Admin),
    post("/cli/daemon/hook-revoke-all", NoLogin),
    post("/cli/daemon/restart", Admin),
    // k2-ptyd S3 (P52): quit the daemon and end its agents.
    post("/cli/daemon/stop", Admin),
    post("/cli/daemon/update/apply", Admin),
    post("/cli/daemon/update/check", Admin),
    // 0.46.3: cached latest-known version, readable by every login.
    get("/cli/daemon/update/latest", Member),
    post("/cli/daemon/update/start", Admin),
    get("/cli/daemon/update/status", Admin),
    // prd-k2-db-rename-v1: the home database switch (DB-A4/A6).
    get("/cli/db-home", Admin),
    post("/cli/db-home/keep", Owner),
    post("/cli/db/bind", Admin),
    // Named Postgres clusters (0.46.11): reading is any login; the handler's
    // versions_access gate decides every POST (owner or the agent switch).
    both("/cli/db/clusters", Member),
    post("/cli/db/create", Member),
    both("/cli/db/doctor", Admin),
    post("/cli/db/drop", Member),
    get("/cli/db/dsn", Member),
    post("/cli/db/dump", Member),
    post("/cli/db/grant", Member),
    get("/cli/db/list", Member),
    post("/cli/db/migrate", Member),
    both("/cli/db/query", Member),
    post("/cli/db/restore", Member),
    post("/cli/db/revoke", Member),
    both("/cli/db/rows", Member),
    post("/cli/db/rows/delete", Member),
    post("/cli/db/rows/update", Member),
    // Owner pins the major new databases go on (0.46.3).
    post("/cli/db/server/default-major", Owner),
    post("/cli/db/server/disable", Admin),
    post("/cli/db/server/enable", Admin),
    post("/cli/db/server/uninstall", Admin),
    get("/cli/db/status", Member),
    get("/cli/db/tables", Member),
    // `k2 db upgrade` (0.46.3 S5): logins need Admin (check only); the
    // handler's versions_access gate decides the rest (§12).
    both("/cli/db/upgrade", Admin),
    // Remote desktop (prd-remote-desktop-tab-v1 §5, 0.46.11). The switch,
    // policy, grants, viewers, kick and the access log are the Owner's
    // (S1: never an Admin). Status, displays, open and the stream ticket
    // are Member-floor: the handler then admits Owners and Admins, and a
    // Member only with a grant (`k2_core::desktop::policy`). Voice & Chat
    // only is refused here (Member floor).
    get("/cli/desktop/audit", Owner),
    post("/cli/desktop/disable", Owner),
    get("/cli/desktop/displays", Member),
    post("/cli/desktop/enable", Owner),
    // An agent's own desktop (L8): passports only; the handler refuses
    // every login (desktop_env_agents_only), so the floor keeps them out.
    post("/cli/desktop/env", Owner),
    post("/cli/desktop/grant", Owner),
    get("/cli/desktop/grants", Owner),
    post("/cli/desktop/kick", Owner),
    // Virtual desktops (prd-linux-virtual-desktop-v1 L4): the picker list is
    // Member-floor (the handler lists only what the caller may open); start
    // and stop are Owners' and Admins'; the packages are the Owner's.
    get("/cli/desktop/list", Member),
    post("/cli/desktop/open", Member),
    post("/cli/desktop/policy", Owner),
    post("/cli/desktop/revoke", Owner),
    post("/cli/desktop/setup", Owner),
    post("/cli/desktop/start", Admin),
    get("/cli/desktop/status", Member),
    post("/cli/desktop/stop", Admin),
    get("/cli/desktop/stream", Member),
    post("/cli/desktop/ticket", Member),
    get("/cli/desktop/viewers", Owner),
    post("/cli/dns-manage", Admin),
    both("/cli/dns/access", Member),
    both("/cli/dns/records", Member),
    post("/cli/dns/records/add", Member),
    post("/cli/dns/records/remove", Member),
    post("/cli/dns/verify", Member),
    both("/cli/dns/zones", Member),
    post("/cli/dns/zones/create", Member),
    post("/cli/dns/zones/delete", Member),
    // Live docs (Markdown multiplayer v1, `docs/routes.rs`): any login; the
    // handler checks rights per doc (read / edit / history) on every call.
    // Shares to another workspace (MI25) need an Admin login, like
    // connections; the handler refuses agents (they ask with a card).
    get("/cli/docs/activity", Member),
    get("/cli/docs/approvals", Member),
    post("/cli/docs/approve", Member),
    get("/cli/docs/blame", Member),
    post("/cli/docs/claim", Member),
    // Comments on any-file docs: the owner's purge and participant erasure
    // (the handler and the kit both ask for the doc owner: the box owner
    // or an Admin login, never an agent).
    post("/cli/docs/comments/purge", Admin),
    get("/cli/docs/cores", Member),
    post("/cli/docs/cursor", Member),
    get("/cli/docs/diff", Member),
    post("/cli/docs/edit", Member),
    post("/cli/docs/erase", Admin),
    post("/cli/docs/export", Member),
    post("/cli/docs/export/cancel", Member),
    get("/cli/docs/export/fetch", Member),
    get("/cli/docs/export/status", Member),
    get("/cli/docs/export/targets", Member),
    post("/cli/docs/flush", Member),
    post("/cli/docs/lease", Member),
    post("/cli/docs/leave", Member),
    // The Live view's socket (`docs/live.rs`): people only; agents and App
    // passes are refused before the upgrade.
    get("/cli/docs/live", Member),
    get("/cli/docs/live/check", Member),
    get("/cli/docs/log", Member),
    post("/cli/docs/mv", Member),
    post("/cli/docs/name", Member),
    post("/cli/docs/open", Member),
    get("/cli/docs/read", Member),
    post("/cli/docs/redo", Member),
    post("/cli/docs/restore", Member),
    post("/cli/docs/revert", Member),
    post("/cli/docs/share", Admin),
    get("/cli/docs/shares", Admin),
    get("/cli/docs/show", Member),
    get("/cli/docs/status", Member),
    post("/cli/docs/tool", Member),
    post("/cli/docs/undo", Member),
    post("/cli/docs/unshare", Admin),
    get("/cli/docs/versions", Member),
    post("/cli/docs/view", Member),
    get("/cli/docs/who", Member),
    post("/cli/docs/write", Member),
    split("/cli/domains", Member, Admin),
    both("/cli/domains/names", Member),
    post("/cli/domains/names/remove", Member),
    post("/cli/domains/refresh", Admin),
    post("/cli/domains/remove", Admin),
    post("/cli/done", Member),
    // Email Notifications part 2, slice V: the confirm and stop links
    // (the token is the credential; a GET only renders, a POST acts).
    // The local mail server's delivery webhook (part 2, N6): loopback
    // only; the HMAC signature is the credential.
    post("/cli/email/events", Public),
    both("/cli/email/stop", Public),
    both("/cli/email/verify", Public),
    post("/cli/events", Member),
    post("/cli/federation/address", Public),
    // Compute seats between a node owner and a client server (0.46.10
    // C3a, vs-live V19): the signed federation envelope from a pinned,
    // trusted peer is the credential (`compute_trust::handle_federation`).
    post("/cli/federation/compute-seat", Public),
    post("/cli/federation/inbound", Public),
    get("/cli/federation/outbox", Admin),
    post("/cli/federation/pair/confirm", Admin),
    post("/cli/federation/pair/request", Public),
    post("/cli/federation/pair/v2/init", Admin),
    post("/cli/federation/pair/v2/respond", Admin),
    post("/cli/federation/pair/v2/reveal", Admin),
    post("/cli/federation/pair/v2/verify", Admin),
    get("/cli/federation/peer-roster", Admin),
    get("/cli/federation/peer-rosters", Admin),
    get("/cli/federation/peers", Admin),
    get("/cli/federation/pubkey", Admin),
    get("/cli/federation/roster", Public),
    post("/cli/federation/send", Admin),
    // Secret handoff (prd-agent-secret-handoff-v1 §13): signed envelopes
    // from a pinned peer, no token (like /inbound).
    post("/cli/federation/vault-handoff", Public),
    post("/cli/federation/vault-recipient", Public),
    get("/cli/feed", Member),
    both("/cli/feedback/answer", Member),
    post("/cli/feedback/assign", Member),
    // 0.46.6: the ticket attachment door (ticket-attachments-v1); GET only.
    get("/cli/feedback/attachment", Member),
    both("/cli/feedback/comment", Member),
    post("/cli/feedback/create", Member),
    get("/cli/feedback/list", Member),
    get("/cli/feedback/list-all", Member),
    both("/cli/feedback/resolve", Member),
    get("/cli/feedback/show", Member),
    get("/cli/feedback/waiting-count", Member),
    // 0.46.6 (prd-custom-domains-path-a-0466-v1 §4.3): the public-ports
    // view and the owner's firewall options. Logins: Admin (reads and
    // changes); the applied-ack is the owner's. Passports reach the handler
    // (reads; a change is `owner_only`).
    get("/cli/firewall", Admin),
    post("/cli/firewall/close", Admin),
    post("/cli/firewall/guard", Admin),
    post("/cli/firewall/open", Admin),
    get("/cli/firewall/ports", Admin),
    get("/cli/firewall/rules", Admin),
    post("/cli/firewall/ssh", Admin),
    post("/cli/firewall/upstream", Owner),
    // The setup checklist (prd-setup-checklist-v1 C1): a person's own list,
    // bound to the credential (owner token or a login). Member floor, so a
    // Voice & Chat only login is refused; agent passports are on the
    // `session_token` denylist (`/cli/first-run/`).
    // Kessel at first run (prd-setup-checklist-v1 §10): the seat-only route
    // takes no login (Kessel's own pinned-chat passport only, checked in the
    // handler; the owner token is refused there too), the one exception to
    // the passport denylist. Making Kessel and starting the Chief of Staff
    // task are Admin's; dismissing the offer is the person's own.
    post("/cli/first-run/chief-of-staff", NoLogin),
    post("/cli/first-run/chief-of-staff/start", Admin),
    post("/cli/first-run/hide", Member),
    post("/cli/first-run/kessel", Admin),
    post("/cli/first-run/offer/dismiss", Member),
    post("/cli/first-run/show", Member),
    post("/cli/first-run/skip", Member),
    get("/cli/first-run/status", Member),
    post("/cli/focus-groups/assign", Member),
    post("/cli/focus-groups/create", Member),
    post("/cli/focus-groups/delete", Member),
    get("/cli/focus-groups/list", Member),
    post("/cli/focus-groups/reconcile", Member),
    post("/cli/focus-groups/update", Member),
    // Folder trust (prd-security-sweep-0464-v1 SS10): status and the
    // agent's ask are Member; deciding and Reset are the owner's (owner
    // token or an Owner login; the handler's `require_action_card_person`
    // also refuses passports, terminals, widgets and guests).
    post("/cli/folder-trust/decide", Owner),
    post("/cli/folder-trust/forget", Owner),
    post("/cli/folder-trust/request", Member),
    post("/cli/folder-trust/reset", Owner),
    both("/cli/folder-trust/status", Member),
    // front443 (0.46.1): the box :443 SNI router. Enable/disable change who
    // owns the box's :443 (a root-helper socket) and restart the daemon.
    post("/cli/front/disable", Admin),
    post("/cli/front/enable", Admin),
    // 0.46.8: the :80 front's switch (the https redirect, ACME HTTP-01).
    post("/cli/front/http", Admin),
    // 0.46.6: the owner's passthrough names on :443.
    get("/cli/front/passthrough", Admin),
    post("/cli/front/passthrough/add", Admin),
    post("/cli/front/passthrough/rm", Admin),
    get("/cli/front/status", Admin),
    get("/cli/fs/clipboard-paths", Member),
    post("/cli/fs/compress", Member),
    post("/cli/fs/compress-cancel", Member),
    get("/cli/fs/compress-status", Member),
    post("/cli/fs/copy", Member),
    post("/cli/fs/create", Member),
    // A new file's first save (Cmd+N, the Board tab): never overwrites.
    post("/cli/fs/create-untitled", Member),
    post("/cli/fs/delete", Member),
    // prd-app-caps-0464-v1 AC34 (0.46.4): App guests save exposed files;
    // the handlers check the pass (a login gets a usage 400).
    get("/cli/fs/download", Member),
    // 0.46.9 resumable transfers: a file's download token for the token
    // door; an App guest's pass only (a login gets a usage 400).
    post("/cli/fs/download/token", Member),
    get("/cli/fs/downloads", Member),
    post("/cli/fs/duplicate", Member),
    get("/cli/fs/events", Member),
    post("/cli/fs/extract", Member),
    post("/cli/fs/extract-cancel", Member),
    get("/cli/fs/extract-status", Member),
    get("/cli/fs/info", Member),
    post("/cli/fs/move", Member),
    post("/cli/fs/open-external", Member),
    post("/cli/fs/open-finder", Member),
    get("/cli/fs/read-binary", Member),
    get("/cli/fs/read-dir", Member),
    get("/cli/fs/read-file", Member),
    get("/cli/fs/read-range", Member),
    post("/cli/fs/rename", Member),
    post("/cli/fs/resolve-links", Member),
    post("/cli/fs/search-tree", Member),
    get("/cli/fs/stat", Member),
    post("/cli/fs/untitled-name", Member),
    post("/cli/fs/upload-binary", Member),
    post("/cli/fs/upload-chunk", Member),
    // 0.46.9 resumable transfers: an App guest's upload session; the bytes
    // go to the token door (`/_k2/transfers/`). A login gets a usage 400.
    post("/cli/fs/upload/abort", Member),
    post("/cli/fs/upload/complete", Member),
    post("/cli/fs/upload/create", Member),
    get("/cli/fs/upload/status", Member),
    post("/cli/fs/upload/token", Member),
    post("/cli/fs/write-file", Member),
    get("/cli/fs/zip-list", Member),
    // 0.46.4 first run without git: state (read), Apple's installer (the
    // handler's `require_owner` = owner token only, like tunnel/start: it
    // opens a dialog on this machine), recheck + waiting git steps.
    get("/cli/git-tools", Member),
    post("/cli/git-tools/install", NoLogin),
    post("/cli/git-tools/recheck", Member),
    post("/cli/git/abort-merge", Member),
    get("/cli/git/branches", Member),
    get("/cli/git/changes", Member),
    post("/cli/git/commit", Member),
    post("/cli/git/create-worktree", Member),
    post("/cli/git/delete-branch", Member),
    get("/cli/git/diff-between", Member),
    get("/cli/git/diff-file", Member),
    get("/cli/git/diff-summary", Member),
    // Changes tab v1: untracked files and lost copies go to the Trash.
    post("/cli/git/discard", Member),
    get("/cli/git/file-at-ref", Member),
    get("/cli/git/info", Member),
    // C5 scope add: `git init` at a workspace root (Changes empty state).
    post("/cli/git/init", Member),
    post("/cli/git/merge-branch", Member),
    get("/cli/git/merge-status", Member),
    // Changes tab v1.1 (C5): review notes. List + write for any login;
    // update/delete check author-or-Admin in the handler.
    get("/cli/git/notes", Member),
    post("/cli/git/notes/add", Member),
    post("/cli/git/notes/delete", Member),
    post("/cli/git/notes/resolve", Member),
    post("/cli/git/notes/send", Member),
    post("/cli/git/notes/update", Member),
    // C5: `k2 changes open` asks clients to show the Changes tab.
    post("/cli/git/open-changes", Member),
    post("/cli/git/prune-worktrees", Member),
    post("/cli/git/remove-worktree", Member),
    post("/cli/git/reopen-worktree", Member),
    post("/cli/git/resolve", Member),
    // Changes tab v1: status + branch + counts + rev in one read.
    get("/cli/git/snapshot", Member),
    post("/cli/git/stage", Member),
    post("/cli/git/stage-all", Member),
    post("/cli/git/unstage", Member),
    get("/cli/git/worktrees", Member),
    get("/cli/glossary", Member),
    get("/cli/glossary/get", Member),
    get("/cli/glossary/list", Member),
    // Local Grok Bots (0.45.7). Reads are any login; detect (the Settings
    // preview), send (a billed turn that speaks as the account holder) and
    // the setting are Admin. The handler refuses passports and App passes.
    get("/cli/grokbots/detect", Admin),
    get("/cli/grokbots/list", Member),
    post("/cli/grokbots/send", Admin),
    split("/cli/grokbots/settings", Member, Admin),
    get("/cli/grokbots/status", Member),
    get("/cli/grokbots/thread", Member),
    get("/cli/heartbeat-log", Member),
    post("/cli/heartbeat/active-projects", Member),
    get("/cli/heartbeat/active-session", Member),
    post("/cli/heartbeat/add", Member),
    post("/cli/heartbeat/apply-wake-scheduler", Member),
    post("/cli/heartbeat/archive", Member),
    post("/cli/heartbeat/edit", Member),
    post("/cli/heartbeat/enable", Member),
    post("/cli/heartbeat/fire", Member),
    get("/cli/heartbeat/fires-list", Member),
    get("/cli/heartbeat/fires-list-all", Member),
    post("/cli/heartbeat/install-launchd", Member),
    post("/cli/heartbeat/launch", Member),
    get("/cli/heartbeat/list", Member),
    get("/cli/heartbeat/list-all", Member),
    get("/cli/heartbeat/list-archived", Member),
    post("/cli/heartbeat/preview-schedule", NoLogin),
    // 0.46.11: one-time reminders; an agent's reach is its own workspace.
    post("/cli/heartbeat/reminder/add", Member),
    post("/cli/heartbeat/reminder/cancel", Member),
    get("/cli/heartbeat/reminder/list", Member),
    get("/cli/heartbeat/reminder/show", Member),
    post("/cli/heartbeat/reminder/snooze", Member),
    post("/cli/heartbeat/remove", Owner),
    post("/cli/heartbeat/rename", Member),
    // 0.46.9: an agent restarts its own heartbeat's session (no hire switch).
    post("/cli/heartbeat/restart", Member),
    both("/cli/heartbeat/schedule", Member),
    get("/cli/heartbeat/scheduler-status", Member),
    post("/cli/heartbeat/set-session", Member),
    post("/cli/heartbeat/set-show-sessions", Member),
    post("/cli/heartbeat/set-use-workspace-session", Member),
    get("/cli/heartbeat/show", Member),
    post("/cli/heartbeat/show-sessions", NoLogin),
    get("/cli/heartbeat/status", Member),
    post("/cli/heartbeat/unarchive", Member),
    post("/cli/heartbeat/uninstall-launchd", Member),
    post("/cli/heartbeat/wake", Member),
    // Home avatar cache (prd-home-picker-and-remote-avatars-v1 P18): this
    // computer only. NoLogin refuses every Connect login at role_gate; the
    // handler refuses anything but the owner token (`home_avatars_local_only`).
    get("/cli/home/avatars", NoLogin),
    post("/cli/home/avatars/prune", NoLogin),
    post("/cli/home/avatars/put", NoLogin),
    // prd-daemon-activity-and-thread-working-v1 DA17: re-run (or, with
    // `remove`, undo) the hook installer. Writes the daemon user's CLI
    // configs, so Admin; the handler also checks owner-or-admin.
    post("/cli/hooks/install", Admin),
    get("/cli/hooks/status", Member),
    get("/cli/host-sessions/list", Member),
    get("/cli/inbox", Member),
    post("/cli/inbox/archive", Member),
    post("/cli/inbox/compose", Member),
    get("/cli/inbox/count", Member),
    post("/cli/inbox/delete", Member),
    post("/cli/inbox/deliver", Member),
    post("/cli/inbox/deliver-bundle", Member),
    get("/cli/inbox/folders", Member),
    get("/cli/inbox/list", Member),
    post("/cli/inbox/migrate", Member),
    post("/cli/inbox/move", Member),
    get("/cli/inbox/read", Member),
    post("/cli/inbox/respond", Member),
    get("/cli/inbox/search", Member),
    // `k2 job` (prd-vault-gaps-v1 item 5): a job runs code unattended as
    // the daemon user, so logins need Admin; agent passports reach these
    // through is_agent_verb and are pinned to their own workspace.
    post("/cli/jobs/add", Admin),
    post("/cli/jobs/disable", Admin),
    post("/cli/jobs/edit", Admin),
    post("/cli/jobs/enable", Admin),
    post("/cli/jobs/kill", Admin),
    get("/cli/jobs/list", Admin),
    get("/cli/jobs/logs", Admin),
    post("/cli/jobs/pause", Admin),
    post("/cli/jobs/remove", Admin),
    post("/cli/jobs/resume", Admin),
    post("/cli/jobs/run", Admin),
    get("/cli/jobs/show", Admin),
    // LLM login wallet (Settings → LLMs, `k2 llm accounts`): the
    // `/cli/agents/*` floor. Passports are read-only in the handler.
    // Changing the server default (switch / next) is Admin and above
    // (Rosson 2026-10-20); the list stays readable for Members.
    post("/cli/llm/accounts/add", Member),
    post("/cli/llm/accounts/add-key", Member),
    post("/cli/llm/accounts/clear-default", Admin),
    get("/cli/llm/accounts/for", Member),
    post("/cli/llm/accounts/for/refresh", Member),
    get("/cli/llm/accounts/list", Member),
    post("/cli/llm/accounts/login", Member),
    post("/cli/llm/accounts/login/cancel", Member),
    post("/cli/llm/accounts/login/input", Member),
    get("/cli/llm/accounts/login/status", Member),
    post("/cli/llm/accounts/next", Admin),
    post("/cli/llm/accounts/pin", Member),
    get("/cli/llm/accounts/pins", Member),
    post("/cli/llm/accounts/refresh", Member),
    post("/cli/llm/accounts/remove", Member),
    post("/cli/llm/accounts/rename", Member),
    get("/cli/llm/accounts/status", Member),
    // Where saved logins live (prd-llm-logins-in-vault-v1): read by
    // Members, flipped by Admins and the Owner (audited).
    get("/cli/llm/accounts/storage", Member),
    post("/cli/llm/accounts/storage/set", Admin),
    post("/cli/llm/accounts/switch", Admin),
    post("/cli/llm/accounts/unpin", Member),
    get("/cli/llm/accounts/usage", Member),
    post("/cli/mail-manage", Admin),
    // homes-sync-v1 (0.46.1): a person's own Homes and devices. Any login
    // (and the owner token); the handler binds the person from the
    // credential. Agents, app passes and API keys are refused there (403).
    get("/cli/me/homes", Member),
    get("/cli/me/homes/devices", Member),
    post("/cli/me/homes/devices/register", Member),
    post("/cli/me/homes/devices/remove", Member),
    post("/cli/me/homes/push", Member),
    // stash-sync-v1 (0.46.7): a person's own prompt stash, same rule.
    get("/cli/me/stash", Member),
    post("/cli/me/stash/push", Member),
    // Box moves carry the vault (prd-box-move-carry-v1): the steps are the
    // box owner's (the handler also takes the person + vault confirm, or
    // holds the step for the owner); begin and status are Admin.
    post("/cli/migrate/secrets/approve", Owner),
    post("/cli/migrate/secrets/begin", Admin),
    post("/cli/migrate/secrets/export", Owner),
    post("/cli/migrate/secrets/import", Owner),
    get("/cli/migrate/secrets/status", Admin),
    post("/cli/migrate/secrets/undo", Owner),
    post("/cli/migrate/secrets/unfreeze", Owner),
    both("/cli/mode", Member),
    // `k2 msg who <id>` (0.45.6 S7): any login; a passport sees only
    // deliveries to or from its own workspace.
    get("/cli/msg/who", Member),
    // Email Notifications (0.46.5, `k2 notifications email`): the owner's
    // and Admins' (passports are refused: deny list + handler). Adding a
    // sender or relay, the system sender, tests and settings are the
    // owner's, with a fresh confirm where they widen.
    get("/cli/notifications/email", Admin),
    // N10: a workspace's agent email notices (on: the owner + a confirm).
    post("/cli/notifications/email/agent/off", Admin),
    post("/cli/notifications/email/agent/on", Owner),
    get("/cli/notifications/email/agents", Admin),
    get("/cli/notifications/email/log", Admin),
    post("/cli/notifications/email/relay/add", Owner),
    post("/cli/notifications/email/relay/rm", Admin),
    post("/cli/notifications/email/relay/test", Owner),
    get("/cli/notifications/email/relays", Admin),
    post("/cli/notifications/email/sender/add", Owner),
    post("/cli/notifications/email/sender/check", Admin),
    // N6: resuming an auto-paused sender is the owner's, with a confirm.
    post("/cli/notifications/email/sender/resume", Owner),
    post("/cli/notifications/email/sender/rm", Admin),
    post("/cli/notifications/email/sender/set", Admin),
    get("/cli/notifications/email/senders", Admin),
    post("/cli/notifications/email/settings", Owner),
    // N6: suppressed addresses (hashed). Adding narrows (Admins); removing
    // widens (the owner).
    get("/cli/notifications/email/suppressions", Admin),
    post("/cli/notifications/email/suppressions/add", Admin),
    post("/cli/notifications/email/suppressions/rm", Owner),
    get("/cli/notifications/email/system-sender", Admin),
    post("/cli/notifications/email/system-sender/clear", Admin),
    post("/cli/notifications/email/system-sender/set", Owner),
    post("/cli/notifications/email/system-sender/test", Owner),
    // prd-thread-push-notifications-v1 §5 (TP19): own row = any login;
    // everyone + defaults = Admin. Other people's rows are Owner-only in
    // the handler (Admins are read-only on people).
    get("/cli/notify/admin", Admin),
    post("/cli/notify/admin/set", Admin),
    // N10: an agent emails a person here (the handler takes only this
    // workspace's agent: its passport or a shell tab in it).
    post("/cli/notify/email", Member),
    // Email Notifications part 2, slice V: a person's own notice address
    // (the handler resolves the person; agents never reach it).
    get("/cli/notify/email/address", Member),
    post("/cli/notify/email/address/remove", Member),
    post("/cli/notify/email/address/set", Member),
    // N10: a person mutes one workspace's notices; the agent turns its own off.
    post("/cli/notify/email/mute", Member),
    post("/cli/notify/email/off", Member),
    both("/cli/notify/prefs", Member),
    post("/cli/onboarding/adopt", Member),
    post("/cli/onboarding/agents-md-generate-enabled", Member),
    post("/cli/onboarding/harness-fanout-enabled", Member),
    get("/cli/onboarding/scan", Member),
    post("/cli/onboarding/set-agents-md-generate-enabled", Member),
    post("/cli/onboarding/set-harness-fanout-enabled", Member),
    post("/cli/onboarding/skip", Member),
    post("/cli/onboarding/start-fresh", Member),
    get("/cli/ops/activity", Member),
    get("/cli/ops/overview", Member),
    get("/cli/ops/stream", Member),
    // The Org Chart (prd-org-chart-v1 §6.2). People look at Member; they
    // change it at Admin. NoLogin rows are an agent's own verbs (its read
    // marker, its org, update/ask/reach): a passport only, never a login.
    // Agents pass these floors (passports aren't logins); the handler
    // checks head / member / "Manage the org chart". No Voice & Chat rows.
    post("/cli/organization/ask", NoLogin),
    get("/cli/organization/bulletin", Member),
    post("/cli/organization/bulletin/read", NoLogin),
    post("/cli/organization/bulletin/set", Admin),
    get("/cli/organization/bulletin/status", NoLogin),
    get("/cli/organization/chart", Member),
    post("/cli/organization/create", Admin),
    post("/cli/organization/delete", Admin),
    post("/cli/organization/head", Admin),
    post("/cli/organization/layout/move", Admin),
    post("/cli/organization/layout/tidy", Admin),
    post("/cli/organization/line/add", Admin),
    post("/cli/organization/line/remove", Admin),
    get("/cli/organization/list", Member),
    post("/cli/organization/member/add", Admin),
    post("/cli/organization/member/move", Admin),
    post("/cli/organization/member/remove", Admin),
    get("/cli/organization/mine", NoLogin),
    // Tonight's night-run queue and the wake governor (0.46.11): anyone
    // on the server, like list.
    get("/cli/organization/night", Member),
    post("/cli/organization/purpose", Admin),
    post("/cli/organization/reach", NoLogin),
    post("/cli/organization/rename", Admin),
    get("/cli/organization/show", Member),
    post("/cli/organization/update", NoLogin),
    get("/cli/organization/updates", Admin),
    get("/cli/overlay/events", Member),
    // "Your account" setup prompt (0.46.0): about who the owner is, so the
    // owner daemon token only. Dismiss is POST-only (a GET is 405).
    get("/cli/owner/account-prompt", NoLogin),
    post("/cli/owner/account-prompt/dismiss", NoLogin),
    // Owner-linked accounts: Admins see the links; only the owner daemon
    // token may change them (identity, never privilege).
    split("/cli/owner/accounts", Admin, NoLogin),
    // Person colours (0.46.0): any login reads them and sets its own; the
    // handler lets an Admin+ change someone else's (Owner for the owner).
    both("/cli/people/color", Member),
    // Person names (0.46.0): same floors as colours (any login reads them
    // and sets its own; Admin+ others; Owner for the owner person).
    both("/cli/people/name", Member),
    // Person pictures (0.46.8): any login reads them and sets its own; the
    // handler lets an Admin+ change someone else's (Owner for the owner),
    // the names' rule. Agent passports read only (the handler refuses a
    // write with `person_only`).
    both("/cli/people/picture", Member),
    post("/cli/people/picture/clear", Member),
    // Permission cards (Beta, prd-agent-settings-cards-v1 §3.2, §6.7,
    // §11): absent from every wire decision while the global switch is off
    // ([`live_row`], vs-live C1). The agent verbs take a passport only (no
    // login asks for a card); "Waiting on you" is for the people who can
    // answer one (Admin is the lowest catalog floor). People answer and
    // deny through `/cli/thread/answer`, so there is no approve route.
    // Permission cards (A1, D2's ask): the agents' switches and recent
    // requests for the Settings page; people only (Owner/Admin).
    get("/cli/permission/agents", Admin),
    post("/cli/permission/cancel", NoLogin),
    get("/cli/permission/enabled", NoLogin),
    get("/cli/permission/list", NoLogin),
    get("/cli/permission/pending", Admin),
    post("/cli/permission/request", NoLogin),
    get("/cli/permission/status", NoLogin),
    post("/cli/permission/update", NoLogin),
    // Power-helper S1: Set up shows the admin dialog on the host, so
    // Admin (and the handler refuses anything but loopback).
    post("/cli/power/helper", Admin),
    // Keep awake changes the host machine's power (0.43.2 Q3): Admin and
    // up, for a direct login and a Home room alike. Reading it stays Member.
    post("/cli/power/keep-awake", Admin),
    get("/cli/power/status", Member),
    post("/cli/presence/kick", Admin),
    get("/cli/presence/roster", Member),
    get("/cli/presence/summary", VoiceChat),
    post("/cli/presets/create", Admin),
    post("/cli/presets/delete", Admin),
    get("/cli/presets/get", Member),
    get("/cli/presets/list", Member),
    post("/cli/presets/reorder", Admin),
    post("/cli/presets/reset", Admin),
    post("/cli/presets/update", Admin),
    // Send feedback to K2 (prd-k2-product-feedback-v1 PF17, 0.46.11). Every
    // route is any login at Member and up; list/show narrow to the caller's
    // own rows unless Owner/Admin, and the server switches in `settings`
    // are Admin in the handler (a person, never a passport). The handler
    // refuses App passes and API keys.
    post("/cli/product-feedback/cancel", Member),
    // Collects + scrubs the consented K2 details for the preview (an agent
    // only with the owner's agentDiagnostics, in the handler).
    post("/cli/product-feedback/diagnostics", Member),
    get("/cli/product-feedback/list", Member),
    post("/cli/product-feedback/retry", Member),
    // A person's screenshot draft (prd-feedback-screenshot-v1 §5.2): upload
    // and read back their own; every passport is refused in the handler
    // (people only).
    both("/cli/product-feedback/screenshot", Member),
    post("/cli/product-feedback/screenshot/forget", Member),
    post("/cli/product-feedback/send", Member),
    both("/cli/product-feedback/settings", Member),
    get("/cli/product-feedback/show", Member),
    get("/cli/project-config/get", Member),
    get("/cli/project-config/has-run-command", Member),
    get("/cli/project-config/run-command", Member),
    post("/cli/project-group/add-member", Member),
    post("/cli/project-group/create", Member),
    post("/cli/project-group/dashboard/add-pane", Admin),
    post("/cli/project-group/dashboard/create", Admin),
    post("/cli/project-group/dashboard/delete", Admin),
    post("/cli/project-group/dashboard/remove-pane", Admin),
    post("/cli/project-group/dashboard/rename", Admin),
    post("/cli/project-group/dashboard/reorder", Admin),
    post("/cli/project-group/dashboard/save-layout", Admin),
    // Deleting a project removes its chat: owner or admin among logins; an
    // agent passes to the handler, which follows its 'Manage K2 projects'
    // switch (Rosson 2026-10-18).
    post("/cli/project-group/delete", Admin),
    get("/cli/project-group/html-docs", Member),
    get("/cli/project-group/icon", Member),
    get("/cli/project-group/list", Member),
    get("/cli/project-group/messages", Member),
    post("/cli/project-group/msg", Member),
    post("/cli/project-group/pin", Member),
    // thread-read-v1 (0.46.1): a person marks a project chat read (their
    // own cursor; agent passports and app passes are refused by the arm).
    post("/cli/project-group/read", Member),
    post("/cli/project-group/remove-member", Member),
    post("/cli/project-group/rename", Member),
    get("/cli/project-group/resources", Member),
    post("/cli/project-group/set-color", Admin),
    // prd-projects-threads-v1 §4.2: the view everyone's agent tiles open in.
    // Owner or admin among logins; an agent passes to the handler, which
    // follows its 'Manage K2 projects' switch.
    post("/cli/project-group/set-default-view", Admin),
    post("/cli/project-group/set-icon", Admin),
    post("/cli/project-group/set-poc", Member),
    get("/cli/project-group/show", Member),
    post("/cli/project-group/sort", Member),
    post("/cli/projects/activate", Member),
    get("/cli/projects/active", Member),
    post("/cli/projects/add-from-path", Member),
    post("/cli/projects/add-without-git", Member),
    post("/cli/projects/clear-icon", Member),
    post("/cli/projects/create", Member),
    post("/cli/projects/delete", Member),
    post("/cli/projects/detect-icon", Member),
    post("/cli/projects/dismiss", Member),
    post("/cli/projects/enable-worktrees", Member),
    get("/cli/projects/get-all-editors", Member),
    get("/cli/projects/get-editors", Member),
    get("/cli/projects/get-icon", Member),
    post("/cli/projects/init-git-and-open", Member),
    get("/cli/projects/list", Member),
    post("/cli/projects/open-in-editor", Member),
    post("/cli/projects/open-in-finder", Member),
    post("/cli/projects/open-in-terminal", Member),
    post("/cli/projects/pin", Member),
    post("/cli/projects/refresh-editors", Member),
    post("/cli/projects/reorder", Member),
    post("/cli/projects/set-icon", Member),
    post("/cli/projects/touch-interaction", Member),
    post("/cli/projects/touch-interaction-clear", Member),
    post("/cli/projects/update", Member),
    // 0.46.3: which nested services get K2's client-IP rewrite.
    post("/cli/publish/client-ip", Member),
    get("/cli/publish/client-ip/list", Member),
    // FA3: Owner on purpose (the other publish mutations are Member): this
    // widens which other sites may frame an App.
    post("/cli/publish/frame", Owner),
    get("/cli/publish/leftovers", Member),
    get("/cli/publish/list", Member),
    get("/cli/publish/logs", Member),
    // 0.46.7: a person's one-time link to a private site. Agents are not logins;
    // the handler refuses them.
    post("/cli/publish/open", Admin),
    post("/cli/publish/rm", Member),
    post("/cli/publish/run", Member),
    // 0.46.7: who is signed in to a site. The handlers scope agents to their own.
    get("/cli/publish/sessions", Admin),
    post("/cli/publish/sessions/revoke", Admin),
    post("/cli/publish/start", Member),
    post("/cli/publish/stop", Member),
    post("/cli/publish/subdomain/claim", Member),
    post("/cli/publish/subdomain/unclaim", Member),
    // decision-k2-runtime-v1 stage 0: `k2 app validate` (parse + leak lints).
    get("/cli/publish/validate", Member),
    // 0.46.7: who may open a site. Member floor: anyone who can run a site may
    // make it private; the handler refuses a Member PIN and public.
    both("/cli/publish/visibility", Member),
    get("/cli/push/device", Member),
    // prd-e2e-push-v1 §4a: the E2E switch, the per-device override and
    // the phone list are the owner's (testing and rollback controls).
    post("/cli/push/device/e2e", Owner),
    // 0.46.7 (K4): the owner or an Admin removes one stale phone.
    post("/cli/push/device/remove", Admin),
    get("/cli/push/devices", Owner),
    both("/cli/push/e2e", Owner),
    // 0.46.3: the push attempt ring (read-only; no content or tokens).
    get("/cli/push/log", Owner),
    post("/cli/push/register-device", Member),
    post("/cli/push/unregister-device", Member),
    post("/cli/relations/create", Member),
    post("/cli/relations/delete", Member),
    get("/cli/relations/list", Member),
    get("/cli/relations/list-incoming", Member),
    post("/cli/release", Member),
    post("/cli/remote-instruct", Admin),
    post("/cli/remote-session/disable", Admin),
    post("/cli/remote-session/enable", Admin),
    post("/cli/remote-session/grant", Admin),
    get("/cli/remote-session/grants", Admin),
    post("/cli/remote-session/revoke", Admin),
    post("/cli/remote-session/shell/spawn", Member),
    get("/cli/remote-session/status", Admin),
    post("/cli/reserve", Member),
    both("/cli/respond", NoLogin),
    post("/cli/review-checklist/init", Member),
    get("/cli/review-checklist/read", Member),
    post("/cli/review-checklist/toggle", Member),
    post("/cli/review-checklist/write", Member),
    post("/cli/review/approve", Member),
    post("/cli/review/feedback", Member),
    post("/cli/review/reject", Member),
    get("/cli/reviews", Member),
    // Root helper (prd-k2-root-helper-v1 §8.3). Status is a plain read
    // (agents too); update/rollback refuse passports in the handler.
    post("/cli/root-helper/rollback", Admin),
    get("/cli/root-helper/status", Member),
    post("/cli/root-helper/update", Admin),
    get("/cli/sandbox/list", Member),
    post("/cli/sandbox/open", NoLogin),
    post("/cli/sandbox/reopen", Member),
    post("/cli/scheduler-tick", Member),
    post("/cli/sections/assign", Member),
    post("/cli/sections/create", Member),
    post("/cli/sections/delete", Member),
    get("/cli/sections/list", Member),
    post("/cli/sections/reorder", Member),
    post("/cli/sections/update", Member),
    post("/cli/session/complete", NoLogin),
    post("/cli/session/set-surfaced", Member),
    get("/cli/sessions/bytes", Member),
    get("/cli/sessions/events", Member),
    get("/cli/sessions/grid", Member),
    post("/cli/sessions/label", Member),
    get("/cli/sessions/list-for-workspace", Member),
    get("/cli/sessions/lookup-by-agent", Member),
    // k2 sessions (prd-workspace-sessions-cli-v1 §5): open is Admin for
    // logins, like sidecar/new (agents: the sidecar gate in the handler);
    // reads are Member (the handler keeps agents to their own workspace).
    get("/cli/sessions/main", Member),
    post("/cli/sessions/main/set", Admin),
    post("/cli/sessions/open", Admin),
    get("/cli/sessions/past", Member),
    post("/cli/sessions/resize", Member),
    get("/cli/sessions/roster", Member),
    get("/cli/sessions/show", Member),
    get("/cli/sessions/subscribe", Member),
    post("/cli/sessions/v2/close", Member),
    post("/cli/sessions/v2/refresh", Member),
    post("/cli/sessions/v2/spawn", Member),
    get("/cli/settings", Member),
    get("/cli/settings/get", Member),
    // LM5: a reset rewrites every gated key (web client, tunnel login
    // ingress, LAN, air-gap) at once — Owner only.
    post("/cli/settings/reset", Owner),
    post("/cli/settings/update", Member),
    // Voice & Chat only shares (`shared_routes`): what the owner shared
    // with this login, one shared agent's Thread, a post to it.
    get("/cli/shared", VoiceChat),
    get("/cli/shared/thread", VoiceChat),
    post("/cli/shared/thread/post", VoiceChat),
    // k2 sidecar v1 (prd-k2-sidecar-cli-v1 §5.1), and Workbench W1
    // (0.46.7): interrupt is a keystroke (Member, like terminal write);
    // land and retire change the workspace's main branch / delete a
    // worktree (Admin, like new/stop; agents ask with a card); review is a
    // read.
    post("/cli/sidecar/archive", Admin),
    post("/cli/sidecar/interrupt", Member),
    post("/cli/sidecar/land", Admin),
    get("/cli/sidecar/list", Member),
    post("/cli/sidecar/new", Admin),
    post("/cli/sidecar/retire", Admin),
    get("/cli/sidecar/review", Member),
    post("/cli/sidecar/stop", Admin),
    // Mobile dev (prd-mobile-dev-v1 §8, 0.46.6): Members drive devices for
    // workspaces they can open (remote Members watch only applies to the
    // stream's input, Rosson 2026-10-27); the owner's licence setup is
    // Owner; the device limits are Admin to write.
    post("/cli/sim/act", Member),
    post("/cli/sim/boot", Member),
    get("/cli/sim/devices", Member),
    post("/cli/sim/erase", Member),
    post("/cli/sim/hold", Member),
    post("/cli/sim/install", Member),
    post("/cli/sim/launch", Member),
    get("/cli/sim/licence", Member),
    get("/cli/sim/logs", Member),
    post("/cli/sim/screenshot", Member),
    split("/cli/sim/settings", Member, Admin),
    post("/cli/sim/setup", Owner),
    get("/cli/sim/status", Member),
    post("/cli/sim/stop", Member),
    get("/cli/sim/stream", Member),
    post("/cli/sim/terminate", Member),
    get("/cli/sim/tree", Member),
    post("/cli/sim/wait", Member),
    post("/cli/skill-layers/create", Member),
    post("/cli/skill-layers/delete", Member),
    get("/cli/skill-layers/get-content", Member),
    get("/cli/skill-layers/list", Member),
    post("/cli/skills/create", Member),
    post("/cli/skills/list", NoLogin),
    post("/cli/skills/regenerate", Member),
    post("/cli/skills/remove", Member),
    post("/cli/skills/write-opt-in", Member),
    both("/cli/skin-tokens", Owner),
    post("/cli/skin-tokens/revoke", Owner),
    post("/cli/skin-tokens/rooms", Owner),
    // Garden v3 data sources (prd-garden-v3-external-data-cards-v1, contract-sources-v1 §4).
    post("/cli/sources/add", Admin),
    post("/cli/sources/approve", Admin),
    post("/cli/sources/batch-approve", Admin),
    get("/cli/sources/changes", Member),
    post("/cli/sources/edit", Admin),
    get("/cli/sources/export", Member),
    get("/cli/sources/history", Member),
    get("/cli/sources/list", Member),
    post("/cli/sources/mute", Admin),
    post("/cli/sources/pause", Admin),
    get("/cli/sources/pending", Admin),
    post("/cli/sources/pin", Admin),
    post("/cli/sources/propose", Admin),
    get("/cli/sources/read", Member),
    post("/cli/sources/refresh", Member),
    post("/cli/sources/remove", Admin),
    post("/cli/sources/resume", Admin),
    post("/cli/sources/secret", Admin),
    get("/cli/sources/show", Member),
    post("/cli/sources/simulate", Admin),
    get("/cli/sources/status", Member),
    post("/cli/sources/triggers/add", Admin),
    post("/cli/sources/triggers/approve", Admin),
    post("/cli/sources/triggers/edit", Admin),
    get("/cli/sources/triggers/list", Member),
    get("/cli/sources/triggers/log", Member),
    post("/cli/sources/triggers/pause", Admin),
    post("/cli/sources/triggers/propose", Admin),
    post("/cli/sources/triggers/remove", Admin),
    post("/cli/sources/triggers/resume", Admin),
    get("/cli/sources/triggers/show", Member),
    post("/cli/sources/triggers/test", Admin),
    post("/cli/sources/try", Admin),
    post("/cli/sources/unmute", Admin),
    post("/cli/sources/unpin", Admin),
    post("/cli/status", Member),
    post("/cli/store/create", Member),
    post("/cli/store/drop", Member),
    get("/cli/store/get", Member),
    get("/cli/store/list", Member),
    post("/cli/store/put", Member),
    get("/cli/store/query", Member),
    post("/cli/store/rm", Member),
    get("/cli/terminal/active-count", Member),
    get("/cli/terminal/classify", Member),
    get("/cli/terminal/compose-history", Member),
    post("/cli/terminal/create", Member),
    get("/cli/terminal/exists", Member),
    get("/cli/terminal/foreground-cmd", Member),
    get("/cli/terminal/get-grid", Member),
    post("/cli/terminal/kill", Member),
    post("/cli/terminal/kill-foreground", Member),
    post("/cli/terminal/lifecycle-write", Member),
    get("/cli/terminal/list-running", Member),
    post("/cli/terminal/log", Member),
    post("/cli/terminal/pin-size", Member),
    get("/cli/terminal/read", Member),
    post("/cli/terminal/resize", Member),
    post("/cli/terminal/scroll", Member),
    post("/cli/terminal/send-message", Member),
    post("/cli/terminal/set-focus", Member),
    post("/cli/terminal/spawn", Member),
    post("/cli/terminal/spawn-background", Member),
    post("/cli/terminal/write", Member),
    post("/cli/themes/create-template", Member),
    post("/cli/themes/delete", Member),
    post("/cli/themes/ensure-dir", Member),
    get("/cli/themes/get-dir", Member),
    get("/cli/themes/list", Member),
    get("/cli/thread", Member),
    get("/cli/thread/activity", Member),
    // 0.46.5 activity SDK (review D2): the batched catch-up.
    get("/cli/thread/activity/rooms", Member),
    // KV10: the handler 405s a GET; the row says so.
    post("/cli/thread/answer", Member),
    post("/cli/thread/ask", Member),
    // thread-attachments-v1: previews and ranged downloads of a Thread
    // message's file objects (Thread rights + the caller's file rights).
    get("/cli/thread/attachment", Member),
    // 0.46.9: a download token for one attachment (the save's rights).
    post("/cli/thread/attachment/token", Member),
    // inbox-catchup-v1 (0.46.3): a read with a JSON body (200 cursors
    // don't fit in a URL), so POST only; a GET is 405 before auth.
    post("/cli/thread/catchup", Member),
    // Thread export / import outside a clone (prd-clone-carries-threads-
    // and-vault-v1 T4): Owner/Admin people + the clone gate; the list of a
    // workspace's imported Threads is a read for any login.
    post("/cli/thread/export", Admin),
    post("/cli/thread/import", Admin),
    get("/cli/thread/imports", Member),
    // Zen Z41: latest Thread item per addr, for previews. GET only.
    get("/cli/thread/latest", Member),
    // thread-message-delete-v1 (0.46.11): a person deletes their own post
    // (the owner anyone's); the handler checks the author and refuses
    // every passport.
    post("/cli/thread/message/delete", Member),
    // thread-read-v1 (0.46.1): a person's own read state. Every login
    // reads and changes only its own person's cursors and mutes.
    post("/cli/thread/mute", Member),
    // Permission cards (Beta, prd-agent-settings-cards-v1 §4.3, §4.4;
    // lane A2): absent while the global switch is off ([`live_row`]). A
    // note is for a person below the card's floor (any login); "Turn off
    // again" is at the action's floor (Admin is the lowest; the handler
    // checks the card's own). People only: the handler refuses passports.
    post("/cli/thread/permission-note", Member),
    post("/cli/thread/permission-undo", Admin),
    both("/cli/thread/post", Member),
    post("/cli/thread/read", Member),
    // thread-read-all-v1 (0.46.5): Mark all as read for one agent, the
    // caller's own cursors only (a person; an App pass its own room's).
    post("/cli/thread/read-all", Member),
    post("/cli/thread/secret", Member),
    // app-room-stream (0.46.3): one Thread socket across the caller's
    // rooms (a login reads every Thread, as on GET /cli/thread).
    get("/cli/thread/stream", Member),
    get("/cli/thread/unread", Member),
    both("/cli/thread/void", Member),
    // Settings → Tickets (0.45.4): any login reads the rules; changing
    // them is Owner/Admin (the handler also refuses agent passports).
    split("/cli/tickets/settings", Member, Admin),
    post("/cli/timer/create", Member),
    post("/cli/timer/delete", Member),
    get("/cli/timer/entries-export", Member),
    get("/cli/timer/entries-list", Member),
    post("/cli/trust-folders", Member),
    split("/cli/tunnel/config", Member, Owner),
    post("/cli/tunnel/disable", NoLogin),
    post("/cli/tunnel/enable", NoLogin),
    post("/cli/tunnel/release", NoLogin),
    post("/cli/tunnel/start", NoLogin),
    get("/cli/tunnel/status", Member),
    post("/cli/tunnel/stop", NoLogin),
    get("/cli/tunnel/subdomains", Member),
    post("/cli/tunnel/subdomains/claim", Member),
    post("/cli/tunnel/subdomains/refresh", Member),
    post("/cli/tunnel/subdomains/unclaim", Member),
    post("/cli/tunnel/switch", NoLogin),
    // Usage v2 (Rosson Q3, 2026-10-16): Owner and Admin, not Members. A
    // passport reaches the reads through is_agent_verb (own workspace).
    get("/cli/usage/limits", Admin),
    post("/cli/usage/limits/refresh", Admin),
    get("/cli/usage/subscriptions", Member),
    post("/cli/usage/subscriptions/refresh", Member),
    get("/cli/usage/tokens", Admin),
    get("/cli/usage/turns", Admin),
    get("/cli/users", Admin),
    post("/cli/users/add", Admin),
    get("/cli/users/audit", Owner),
    split("/cli/users/policy", VoiceChat, NoLogin),
    post("/cli/users/remove", Owner),
    post("/cli/users/set-can-clone", Owner),
    post("/cli/users/set-disabled", Admin),
    post("/cli/users/set-password", Owner),
    // prd-voice-chat-only-members-v1 Q1/V8: an Admin moves people between
    // Member and Voice & Chat only; the handler keeps every other change
    // Owner-only (`connect_users::can_set_role`).
    post("/cli/users/set-role", Admin),
    // The shares of a Voice & Chat only login (agents' Threads, Gardens).
    post("/cli/users/set-shares", Admin),
    post("/cli/users/unlock", Owner),
    // Voice rooms (prd-k2-voice-rooms-v1 §9.1, §9.4; preview, dark while
    // off): join, the call's own routes and a recording consent are any
    // login's (the handler refuses the bare owner token and passports);
    // moderation, the count, renames and closing are Owner/Admin; status is
    // Owner/Admin (agents read it with a passport, which this table never gates).
    get("/cli/voice/events", VoiceChat),
    post("/cli/voice/join", VoiceChat),
    post("/cli/voice/moderate", Admin),
    post("/cli/voice/recording/consent", VoiceChat),
    // Sew 0.9.0 (K-g): a person's own Record and Stop, and what their Sew
    // permissions allow; a Voice & Chat only login never records.
    post("/cli/voice/recording/controls", Member),
    // Sew 0.9.0: the K5 rooms' recording (the owner; start level an Admin
    // too; `required` the owner only: handler).
    post("/cli/voice/recording/settings", Admin),
    post("/cli/voice/recording/start", Member),
    post("/cli/voice/recording/stop", Member),
    // 0.46.11: the K5 rooms' recordings and transcripts. Logins: the
    // owner's and Admins'; agents with "Read call transcripts" (passports
    // are the handler's).
    get("/cli/voice/recordings", Admin),
    post("/cli/voice/repair", Admin),
    get("/cli/voice/rooms", VoiceChat),
    post("/cli/voice/rooms/close", Admin),
    post("/cli/voice/rooms/rename", Admin),
    post("/cli/voice/session/keys/join", VoiceChat),
    post("/cli/voice/session/keys/key_packages", VoiceChat),
    post("/cli/voice/session/leave", VoiceChat),
    post("/cli/voice/session/state", VoiceChat),
    post("/cli/voice/session/stats", VoiceChat),
    post("/cli/voice/settings", Admin),
    get("/cli/voice/status", Admin),
    get("/cli/voice/transcript", Admin),
    get("/cli/voice/transcripts", Admin),
    get("/cli/voice/transcripts/reads", Admin),
    post("/cli/voice/transcripts/subscribe", Admin),
    get("/cli/whats_new", Member),
    post("/cli/whats_new/mark_seen", Member),
    post("/cli/whats_new/reset", Member),
    get("/cli/whoami", Member),
    post("/cli/wiki/chat", Member),
    post("/cli/wiki/chat/off", Member),
    post("/cli/wiki/chat/on", Member),
    get("/cli/wiki/index", Member),
    get("/cli/wiki/note", Member),
    post("/cli/wiki/seed", Member),
    post("/cli/wiki/serve", Member),
    post("/cli/wiki/serve/off", Member),
    post("/cli/wiki/serve/on", Member),
    get("/cli/wiki/serve/status", Member),
    get("/cli/wiki/status", Member),
    get("/cli/window-state/get", Member),
    post("/cli/window-state/set", Member),
    get("/cli/work/inbox", Member),
    get("/cli/work/inbox/create", Member),
    // 0.46.7 Workbench (W2): this computer's sidecars as Garden rows.
    get("/cli/workbench/agents", Member),
    post("/cli/workspace-layouts/delete", Member),
    get("/cli/workspace-layouts/load", Member),
    get("/cli/workspace-layouts/load-all", Member),
    post("/cli/workspace-layouts/save", Member),
    get("/cli/workspace/agent-display-name", Member),
    post("/cli/workspace/api-key", NoLogin),
    post("/cli/workspace/cleanup", Member),
    post("/cli/workspace/create", Member),
    post("/cli/workspace/ensure-canonical-session", Member),
    post("/cli/workspace/ensure-pinned-chat", Member),
    get("/cli/workspace/handle", Member),
    post("/cli/workspace/msg", Member),
    post("/cli/workspace/open", Member),
    post("/cli/workspace/remove", Member),
    get("/cli/workspace/resolve", Member),
    get("/cli/workspace/resources", Member),
    post("/cli/workspace/resources/add", Member),
    post("/cli/workspace/resources/remove", Member),
    get("/cli/workspace/resume-chat-args", Member),
    post("/cli/workspace/session-get", NoLogin),
    post("/cli/workspace/set", Member),
    post("/cli/workspace/set-agent-display-name", Member),
    post("/cli/workspace/set-chat-session", Member),
    post("/cli/workspace/set-handle", Member),
    post("/cli/workspace/set-tab-title", Member),
    post("/cli/workspace/swap-canonical", Member),
    get("/cli/workspace/tab-titles", Member),
    post("/cli/workspaces/create", Member),
    post("/cli/workspaces/delete", Member),
    get("/cli/workspaces/list", Member),
    post("/cli/workspaces/set-nav-visible", Member),
    post("/cli/worktree", Member),
];
