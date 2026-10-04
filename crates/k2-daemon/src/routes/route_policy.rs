//! Route policy table — the minimum login role for every `/cli/*` route
//! (`prd-remove-viewer-role-v1.md` §5, locks RV8–RV14).
//!
//! **Why this exists.** The daemon used to ask only "is this a real
//! login?" on most routes (`token_ok`), and the method allowlist was an
//! inline `matches!` in the dispatcher. With the Viewer role gone every
//! login is Member, Admin or Owner; this table writes down, per route and
//! per method, the lowest login role the route admits TODAY, and one
//! central check ([`role_gate`]) enforces it before the route match. A new
//! route that is not in [`ROUTES`] is refused for logins at runtime, and
//! the source-walk test below fails CI until someone classifies it.
//!
//! **What it never touches.** The gate acts only on a live Connect
//! **login session** (`connect_users::validate_session` resolves the
//! token). The owner daemon token, agent passports (`<sid>.<secret>`),
//! the per-cell UDS (never enters the TCP dispatcher), app passes
//! (`k2skn_`), API keys (`k2sk_`, `/v1` only), remote-session grants
//! (`k2rs_`), grid stream tokens and federation envelopes are all passed
//! through untouched to the per-route gates, exactly as before.
//!
//! **Floors are today's behaviour, codified.** A route's floor is the
//! lowest login role its own handler gate already admits (`token_ok` →
//! Member, `require_manage` / `require_owner_or_admin` /
//! `token_is_owner_or_admin` → Admin, `owner_role_identity` → Owner,
//! `require_owner` / `token_is_owner` / passport-only → [`Floor::NoLogin`]).
//! Handlers keep their own gates; some Member-floor routes still refuse a
//! Member for a particular input (for example `settings/update` with a
//! remote-access key, `relations/create` with the toggle off, mail with an
//! inbox the caller can't reach). This table never widens a route and
//! never narrows what a Member could do before.
//!
//! **Methods.** `post: Some(_)` is exactly the POST allowlist the
//! dispatcher's top-level 405 guard uses ([`post_allowed`]). `get: Some(_)`
//! marks a route whose GET is a real read or a GET-shaped verb. A GET to a
//! POST-only path is not refused here: it is checked at the POST floor and
//! falls through to the same 405/404 the route answered before.

use crate::cli_response::CliResponse;

/// The lowest login that may reach a route. Ordered: a login passes when
/// its role is at or above the floor. `Public` and `Member` admit every
/// login (every role is Member or above since the Viewer role was
/// removed); `NoLogin` admits none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Floor {
    /// No credential needed (login, app-pass login/reset, federation
    /// envelopes). A stray login token on it is never refused.
    Public,
    /// Any login.
    Member,
    /// Admin or Owner login.
    Admin,
    /// Owner login.
    Owner,
    /// No Connect login may use it: the route takes the owner daemon
    /// token or another non-login credential (agent passport, app pass).
    NoLogin,
}

impl Floor {
    /// Wire name used in the refusal body (`"required"`).
    pub fn as_wire(self) -> &'static str {
        match self {
            Floor::Public => "public",
            Floor::Member => "member",
            Floor::Admin => "admin",
            Floor::Owner => "owner",
            Floor::NoLogin => "owner-token",
        }
    }

    /// Whether a login holding `role` clears this floor.
    pub fn admits(self, role: k2_core::connect_users::Role) -> bool {
        use k2_core::connect_users::Role;
        match self {
            Floor::Public | Floor::Member => true,
            Floor::Admin => role >= Role::Admin,
            Floor::Owner => role >= Role::Owner,
            Floor::NoLogin => false,
        }
    }
}

/// One classified `/cli/*` route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub path: &'static str,
    /// Floor for a GET, when the route serves GET.
    pub get: Option<Floor>,
    /// Floor for a POST, when the route is on the POST allowlist.
    pub post: Option<Floor>,
}

const fn get(path: &'static str, f: Floor) -> Route {
    Route { path, get: Some(f), post: None }
}
const fn post(path: &'static str, f: Floor) -> Route {
    Route { path, get: None, post: Some(f) }
}
const fn both(path: &'static str, f: Floor) -> Route {
    Route { path, get: Some(f), post: Some(f) }
}
const fn split(path: &'static str, g: Floor, p: Floor) -> Route {
    Route { path, get: Some(g), post: Some(p) }
}

use Floor::{Admin, Member, NoLogin, Owner, Public};

/// Every `/cli/*` route the daemon answers, sorted by path (the lookup is
/// a binary search; a unit test pins the order and uniqueness).
pub const ROUTES: &[Route] = &[
    get("/cli/activity/events", Member),
    get("/cli/agent/complete", Member),
    get("/cli/agent/conf", Member),
    get("/cli/agent/list", Member),
    get("/cli/agent/reply", Member),
    post("/cli/agent/retire", Member),
    get("/cli/agent/update", Member),
    get("/cli/agentic", Member),
    get("/cli/agents-create-connections", Member),
    post("/cli/agents-manage-skin", Owner),
    post("/cli/agents/archive-orphans", Member),
    get("/cli/agents/create", Member),
    get("/cli/agents/delegate", Member),
    get("/cli/agents/delete", Member),
    both("/cli/agents/disable-workspace-claude-md", Member),
    post("/cli/agents/ensure-cli", Member),
    get("/cli/agents/generate-claude-md", Member),
    get("/cli/agents/heartbeat", Member),
    get("/cli/agents/heartbeat/action", Member),
    get("/cli/agents/heartbeat/noop", Member),
    get("/cli/agents/launch", Member),
    get("/cli/agents/list", Member),
    get("/cli/agents/lock", Member),
    get("/cli/agents/profile", Member),
    get("/cli/agents/reap", Member),
    both("/cli/agents/regenerate-workspace-skill", Member),
    both("/cli/agents/run-workspace-ingest", Member),
    get("/cli/agents/running", Member),
    both("/cli/agents/save-agent-md", Member),
    both("/cli/agents/save-session-id", Member),
    get("/cli/agents/triage", Member),
    get("/cli/agents/unlock", Member),
    get("/cli/agents/work", Member),
    get("/cli/agents/work/create", Member),
    get("/cli/agents/work/move", Member),
    post("/cli/api-keys/create", Owner),
    post("/cli/api-keys/disable", Owner),
    post("/cli/api-keys/enable", Owner),
    get("/cli/api-keys/list", Owner),
    post("/cli/api-keys/revoke", Owner),
    post("/cli/auth/change-password", Member),
    post("/cli/auth/login", Public),
    post("/cli/auth/logout", Member),
    get("/cli/auth/whoami", Member),
    both("/cli/awareness/publish", Member),
    get("/cli/awareness/subscribe", Member),
    post("/cli/browser/open-url", Member),
    both("/cli/canonical/detect-state", Member),
    get("/cli/certs", Member),
    both("/cli/certs/config", Admin),
    both("/cli/certs/issue", Member),
    both("/cli/certs/renew", Member),
    both("/cli/certs/upload", Admin),
    post("/cli/chat/archive", Member),
    post("/cli/chat/continue-seed", Member),
    get("/cli/chat/custom-names", Member),
    get("/cli/chat/detect-active", Member),
    get("/cli/chat/discover-ide", Member),
    get("/cli/chat/list", Member),
    post("/cli/chat/migrate-ide", Member),
    get("/cli/chat/pinned", Member),
    post("/cli/chat/rename", Member),
    post("/cli/chat/restore", Member),
    get("/cli/chat/session-exists", Member),
    get("/cli/chat/session-path", Member),
    get("/cli/chat/storage-paths", Member),
    post("/cli/chat/toggle-pin", Member),
    get("/cli/chat/transcript", Member),
    get("/cli/chatter", Member),
    get("/cli/chatterlog", Member),
    get("/cli/checkin", Member),
    post("/cli/claude-auth/install-scheduler", Member),
    post("/cli/claude-auth/refresh-now", Member),
    get("/cli/claude-auth/status", Member),
    post("/cli/claude-auth/uninstall-scheduler", Member),
    both("/cli/clone/bundle", Member),
    both("/cli/clone/pack", Member),
    post("/cli/clone/pack-cleanup", Member),
    get("/cli/clone/pack-status", Member),
    both("/cli/clone/unpack", Member),
    get("/cli/commit", Member),
    get("/cli/commit-merge", Member),
    post("/cli/companion/disconnect-session", Member),
    get("/cli/companion/presets", Member),
    get("/cli/companion/projects", Member),
    get("/cli/companion/projects-summary", Member),
    get("/cli/companion/sessions", Member),
    post("/cli/companion/set-password", Member),
    get("/cli/companion/start", Member),
    get("/cli/companion/status", Member),
    get("/cli/companion/stop", Member),
    get("/cli/connections", Member),
    post("/cli/context/add", Member),
    get("/cli/context/catalog", Member),
    both("/cli/context/catalog/create", Admin),
    both("/cli/context/catalog/delete", Admin),
    get("/cli/context/layers", Member),
    post("/cli/context/move", Member),
    post("/cli/context/regen", Member),
    post("/cli/context/remove", Member),
    post("/cli/context/set-enabled", Member),
    get("/cli/context/show", Member),
    post("/cli/daemon/app-update/progress", Admin),
    post("/cli/daemon/hook-revoke-all", NoLogin),
    post("/cli/daemon/restart", Admin),
    post("/cli/daemon/update/apply", Admin),
    post("/cli/daemon/update/check", Admin),
    post("/cli/daemon/update/start", Admin),
    get("/cli/daemon/update/status", Admin),
    post("/cli/db/bind", Admin),
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
    both("/cli/db/rows/delete", Member),
    both("/cli/db/rows/update", Member),
    post("/cli/db/server/disable", Admin),
    post("/cli/db/server/enable", Admin),
    post("/cli/db/server/uninstall", Admin),
    get("/cli/db/status", Member),
    get("/cli/db/tables", Member),
    get("/cli/dns-manage", Member),
    both("/cli/dns/access", Member),
    both("/cli/dns/records", Member),
    post("/cli/dns/records/add", Member),
    post("/cli/dns/records/remove", Member),
    post("/cli/dns/verify", Member),
    both("/cli/dns/zones", Member),
    post("/cli/dns/zones/create", Member),
    post("/cli/dns/zones/delete", Member),
    split("/cli/domains", Member, Admin),
    both("/cli/domains/names", Member),
    both("/cli/domains/names/remove", Member),
    post("/cli/domains/refresh", Admin),
    both("/cli/domains/remove", Admin),
    get("/cli/done", Member),
    get("/cli/events", Member),
    post("/cli/federation/inbound", Public),
    get("/cli/federation/outbox", Admin),
    post("/cli/federation/pair/confirm", Admin),
    post("/cli/federation/pair/request", Public),
    get("/cli/federation/peer-roster", Admin),
    get("/cli/federation/peers", Admin),
    get("/cli/federation/pubkey", Admin),
    get("/cli/federation/roster", Public),
    post("/cli/federation/send", Admin),
    get("/cli/feed", Member),
    both("/cli/feedback/answer", Member),
    post("/cli/feedback/assign", Member),
    both("/cli/feedback/comment", Member),
    both("/cli/feedback/create", Member),
    get("/cli/feedback/list", Member),
    get("/cli/feedback/list-all", Member),
    both("/cli/feedback/resolve", Member),
    get("/cli/feedback/show", Member),
    get("/cli/feedback/waiting-count", Member),
    post("/cli/focus-groups/assign", Member),
    post("/cli/focus-groups/create", Member),
    post("/cli/focus-groups/delete", Member),
    get("/cli/focus-groups/list", Member),
    post("/cli/focus-groups/reconcile", Member),
    post("/cli/focus-groups/update", Member),
    get("/cli/fs/clipboard-paths", Member),
    post("/cli/fs/compress", Member),
    post("/cli/fs/compress-cancel", Member),
    get("/cli/fs/compress-status", Member),
    both("/cli/fs/copy", Member),
    both("/cli/fs/create", Member),
    post("/cli/fs/delete", Member),
    post("/cli/fs/duplicate", Member),
    get("/cli/fs/events", Member),
    post("/cli/fs/extract", Member),
    post("/cli/fs/extract-cancel", Member),
    get("/cli/fs/extract-status", Member),
    get("/cli/fs/info", Member),
    both("/cli/fs/move", Member),
    post("/cli/fs/open-external", Member),
    post("/cli/fs/open-finder", Member),
    get("/cli/fs/read-binary", Member),
    get("/cli/fs/read-dir", Member),
    get("/cli/fs/read-file", Member),
    get("/cli/fs/read-range", Member),
    both("/cli/fs/rename", Member),
    post("/cli/fs/search-tree", Member),
    post("/cli/fs/upload-binary", Member),
    post("/cli/fs/upload-chunk", Member),
    post("/cli/fs/write-file", Member),
    get("/cli/fs/zip-list", Member),
    post("/cli/git/abort-merge", Member),
    get("/cli/git/branches", Member),
    get("/cli/git/changes", Member),
    post("/cli/git/commit", Member),
    post("/cli/git/create-worktree", Member),
    post("/cli/git/delete-branch", Member),
    get("/cli/git/diff-between", Member),
    get("/cli/git/diff-file", Member),
    get("/cli/git/diff-summary", Member),
    get("/cli/git/file-at-ref", Member),
    get("/cli/git/info", Member),
    post("/cli/git/merge-branch", Member),
    get("/cli/git/merge-status", Member),
    post("/cli/git/prune-worktrees", Member),
    post("/cli/git/remove-worktree", Member),
    post("/cli/git/reopen-worktree", Member),
    post("/cli/git/resolve", Member),
    post("/cli/git/stage", Member),
    post("/cli/git/stage-all", Member),
    post("/cli/git/unstage", Member),
    get("/cli/git/worktrees", Member),
    get("/cli/glossary", Member),
    get("/cli/glossary/get", Member),
    get("/cli/glossary/list", Member),
    get("/cli/heartbeat-log", Member),
    get("/cli/heartbeat/active-projects", Member),
    get("/cli/heartbeat/active-session", Member),
    both("/cli/heartbeat/add", Member),
    post("/cli/heartbeat/apply-wake-scheduler", Member),
    both("/cli/heartbeat/archive", Member),
    both("/cli/heartbeat/edit", Member),
    both("/cli/heartbeat/enable", Member),
    both("/cli/heartbeat/fire", Member),
    get("/cli/heartbeat/fires-list", Member),
    get("/cli/heartbeat/fires-list-all", Member),
    post("/cli/heartbeat/install-launchd", Member),
    get("/cli/heartbeat/launch", Member),
    get("/cli/heartbeat/list", Member),
    get("/cli/heartbeat/list-all", Member),
    get("/cli/heartbeat/list-archived", Member),
    get("/cli/heartbeat/remove", Owner),
    both("/cli/heartbeat/rename", Member),
    get("/cli/heartbeat/schedule", Member),
    get("/cli/heartbeat/scheduler-status", Member),
    get("/cli/heartbeat/set-session", Member),
    both("/cli/heartbeat/set-show-sessions", Member),
    get("/cli/heartbeat/set-use-workspace-session", Member),
    get("/cli/heartbeat/show", Member),
    get("/cli/heartbeat/status", Member),
    get("/cli/heartbeat/unarchive", Member),
    post("/cli/heartbeat/uninstall-launchd", Member),
    post("/cli/heartbeat/wake", Member),
    get("/cli/hooks/status", Member),
    get("/cli/host-sessions/list", Member),
    get("/cli/inbox", Member),
    post("/cli/inbox/archive", Member),
    post("/cli/inbox/compose", Member),
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
    post("/cli/llm/chat", Member),
    get("/cli/llm/check", Member),
    post("/cli/llm/download-default", Member),
    post("/cli/llm/load-model", Member),
    get("/cli/llm/status", Member),
    post("/cli/mail-manage", Admin),
    post("/cli/mail/access/grant", Admin),
    post("/cli/mail/access/revoke", Admin),
    post("/cli/mail/access/set-level", Admin),
    post("/cli/mail/access/set-manage", Admin),
    post("/cli/mail/access/set-primary", Admin),
    both("/cli/mail/acl", Admin),
    post("/cli/mail/acl/revoke", Admin),
    post("/cli/mail/address/create", Member),
    post("/cli/mail/address/delete", Member),
    get("/cli/mail/address/list", Member),
    post("/cli/mail/address/password", Admin),
    both("/cli/mail/alias", Admin),
    post("/cli/mail/alias/remove", Admin),
    both("/cli/mail/allowlist", Admin),
    post("/cli/mail/allowlist/add", Admin),
    post("/cli/mail/allowlist/remove", Admin),
    both("/cli/mail/app-password", Admin),
    post("/cli/mail/app-password/revoke", Admin),
    post("/cli/mail/approvals/approve", Admin),
    post("/cli/mail/approvals/deny", Admin),
    get("/cli/mail/approvals/list", Admin),
    post("/cli/mail/archive", Member),
    get("/cli/mail/attachments", Member),
    both("/cli/mail/autoconfig", Admin),
    both("/cli/mail/bans", Admin),
    post("/cli/mail/bans/clear", Admin),
    both("/cli/mail/bans/migrate", Admin),
    post("/cli/mail/bans/migrate/restore", Admin),
    both("/cli/mail/catchall", Admin),
    post("/cli/mail/cert/renew", Admin),
    get("/cli/mail/config", Member),
    post("/cli/mail/config/set", Admin),
    post("/cli/mail/delete", Member),
    both("/cli/mail/dkim", Admin),
    post("/cli/mail/dkim/retire", Admin),
    post("/cli/mail/dkim/rotate", Admin),
    both("/cli/mail/dmarc", Admin),
    post("/cli/mail/dmarc/report-to", Admin),
    both("/cli/mail/doctor", Admin),
    post("/cli/mail/domain/add", Admin),
    post("/cli/mail/domain/check", Admin),
    get("/cli/mail/domain/list", Admin),
    post("/cli/mail/domain/remove", Admin),
    get("/cli/mail/domain/show", Admin),
    post("/cli/mail/draft", Member),
    post("/cli/mail/external/add", Admin),
    post("/cli/mail/external/remove", Admin),
    post("/cli/mail/flag", Member),
    post("/cli/mail/folder/create", Member),
    get("/cli/mail/folder/list", Member),
    post("/cli/mail/folder/rename", Member),
    both("/cli/mail/footer", Admin),
    post("/cli/mail/footer/unset", Admin),
    both("/cli/mail/forward", Admin),
    post("/cli/mail/forward/unset", Admin),
    post("/cli/mail/import", Admin),
    get("/cli/mail/inboxes", Member),
    post("/cli/mail/link/add", Member),
    post("/cli/mail/link/oauth/complete", Admin),
    post("/cli/mail/link/oauth/start", Admin),
    get("/cli/mail/link/oauth/status", Admin),
    post("/cli/mail/link/remove", Member),
    both("/cli/mail/list", Admin),
    post("/cli/mail/list/delete", Admin),
    both("/cli/mail/list/members", Admin),
    get("/cli/mail/messages", Member),
    post("/cli/mail/move", Member),
    get("/cli/mail/oauth-config", Admin),
    post("/cli/mail/oauth-config/clear", Admin),
    post("/cli/mail/oauth-config/set", Admin),
    both("/cli/mail/ooo", Admin),
    post("/cli/mail/ooo/unset", Admin),
    get("/cli/mail/outbox", Member),
    post("/cli/mail/outbox/cancel", Member),
    get("/cli/mail/preflight", Member),
    both("/cli/mail/ptr", Admin),
    post("/cli/mail/ptr/set", Admin),
    get("/cli/mail/queue", Admin),
    post("/cli/mail/queue/drop", Admin),
    post("/cli/mail/queue/retry", Admin),
    both("/cli/mail/quota", Admin),
    get("/cli/mail/read", Member),
    post("/cli/mail/reply", Member),
    post("/cli/mail/send", Member),
    post("/cli/mail/server/disable", Admin),
    post("/cli/mail/server/enable", Admin),
    post("/cli/mail/server/rotate-admin", Admin),
    post("/cli/mail/server/uninstall", Admin),
    post("/cli/mail/spam/allow", Admin),
    post("/cli/mail/spam/block", Admin),
    get("/cli/mail/spam/quarantine", Admin),
    post("/cli/mail/spam/quarantine/discard", Admin),
    post("/cli/mail/spam/quarantine/release", Admin),
    post("/cli/mail/spam/train", Admin),
    get("/cli/mail/status", Member),
    get("/cli/mail/wait", Member),
    get("/cli/mode", Member),
    get("/cli/onboarding/adopt", Member),
    both("/cli/onboarding/agents-md-generate-enabled", Member),
    both("/cli/onboarding/harness-fanout-enabled", Member),
    get("/cli/onboarding/scan", Member),
    both("/cli/onboarding/set-agents-md-generate-enabled", Member),
    both("/cli/onboarding/set-harness-fanout-enabled", Member),
    get("/cli/onboarding/skip", Member),
    get("/cli/onboarding/start-fresh", Member),
    get("/cli/ops/activity", Member),
    get("/cli/ops/overview", Member),
    get("/cli/ops/stream", Member),
    get("/cli/overlay/events", Member),
    // Power-helper S1: Set up shows the admin dialog on the host, so
    // Admin (and the handler refuses anything but loopback).
    post("/cli/power/helper", Admin),
    // Keep awake changes the host machine's power (0.43.2 Q3): Admin and
    // up, for a direct login and a Home room alike. Reading it stays Member.
    post("/cli/power/keep-awake", Admin),
    get("/cli/power/status", Member),
    post("/cli/presence/kick", Admin),
    get("/cli/presence/roster", Member),
    get("/cli/presence/summary", Member),
    post("/cli/presets/create", Admin),
    post("/cli/presets/delete", Admin),
    get("/cli/presets/get", Member),
    get("/cli/presets/list", Member),
    post("/cli/presets/reorder", Admin),
    post("/cli/presets/reset", Admin),
    post("/cli/presets/update", Admin),
    get("/cli/project-config/get", Member),
    get("/cli/project-config/has-run-command", Member),
    get("/cli/project-config/run-command", Member),
    post("/cli/project-group/add-member", Member),
    post("/cli/project-group/create", Member),
    post("/cli/project-group/dashboard/create", Admin),
    post("/cli/project-group/dashboard/delete", Admin),
    post("/cli/project-group/dashboard/rename", Admin),
    post("/cli/project-group/dashboard/reorder", Admin),
    post("/cli/project-group/dashboard/save-layout", Admin),
    post("/cli/project-group/delete", Member),
    get("/cli/project-group/html-docs", Member),
    get("/cli/project-group/icon", Member),
    get("/cli/project-group/list", Member),
    get("/cli/project-group/messages", Member),
    post("/cli/project-group/msg", Member),
    post("/cli/project-group/pin", Member),
    post("/cli/project-group/remove-member", Member),
    post("/cli/project-group/rename", Member),
    get("/cli/project-group/resources", Member),
    post("/cli/project-group/set-color", Admin),
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
    get("/cli/publish/leftovers", Member),
    get("/cli/publish/list", Member),
    get("/cli/publish/logs", Member),
    post("/cli/publish/rm", Member),
    post("/cli/publish/run", Member),
    post("/cli/publish/start", Member),
    post("/cli/publish/stop", Member),
    post("/cli/publish/subdomain/claim", Member),
    post("/cli/publish/subdomain/unclaim", Member),
    post("/cli/push/register-device", Member),
    post("/cli/push/unregister-device", Member),
    both("/cli/relations/create", Member),
    both("/cli/relations/delete", Member),
    get("/cli/relations/list", Member),
    get("/cli/relations/list-incoming", Member),
    get("/cli/release", Member),
    get("/cli/remote-instruct", Member),
    post("/cli/remote-session/disable", Admin),
    post("/cli/remote-session/enable", Admin),
    post("/cli/remote-session/grant", Admin),
    get("/cli/remote-session/grants", Admin),
    post("/cli/remote-session/revoke", Admin),
    post("/cli/remote-session/shell/spawn", Member),
    get("/cli/remote-session/status", Admin),
    get("/cli/reserve", Member),
    both("/cli/respond", NoLogin),
    post("/cli/review-checklist/init", Member),
    get("/cli/review-checklist/read", Member),
    post("/cli/review-checklist/toggle", Member),
    post("/cli/review-checklist/write", Member),
    get("/cli/review/approve", Member),
    get("/cli/review/feedback", Member),
    get("/cli/review/reject", Member),
    get("/cli/reviews", Member),
    get("/cli/sandbox/list", Member),
    post("/cli/sandbox/open", NoLogin),
    post("/cli/sandbox/reopen", Member),
    get("/cli/scheduler-tick", Member),
    post("/cli/sections/assign", Member),
    post("/cli/sections/create", Member),
    post("/cli/sections/delete", Member),
    get("/cli/sections/list", Member),
    post("/cli/sections/reorder", Member),
    post("/cli/sections/update", Member),
    both("/cli/session/complete", NoLogin),
    both("/cli/session/set-surfaced", Member),
    get("/cli/sessions/bytes", Member),
    get("/cli/sessions/events", Member),
    get("/cli/sessions/grid", Member),
    get("/cli/sessions/label", Member),
    get("/cli/sessions/list-for-workspace", Member),
    get("/cli/sessions/lookup-by-agent", Member),
    get("/cli/sessions/resize", Member),
    get("/cli/sessions/subscribe", Member),
    both("/cli/sessions/v2/close", Member),
    both("/cli/sessions/v2/refresh", Member),
    both("/cli/sessions/v2/spawn", Member),
    get("/cli/settings", Member),
    get("/cli/settings/get", Member),
    post("/cli/settings/reset", Member),
    post("/cli/settings/update", Member),
    post("/cli/skill-layers/create", Member),
    post("/cli/skill-layers/delete", Member),
    get("/cli/skill-layers/get-content", Member),
    get("/cli/skill-layers/list", Member),
    post("/cli/skills/create", Member),
    get("/cli/skills/regenerate", Member),
    post("/cli/skills/remove", Member),
    post("/cli/skills/write-opt-in", Member),
    both("/cli/skin-tokens", Owner),
    post("/cli/skin-tokens/revoke", Owner),
    post("/cli/skin-tokens/rooms", Owner),
    get("/cli/skin/agents", NoLogin),
    both("/cli/skin/front-door", Owner),
    both("/cli/skin/grants", Owner),
    post("/cli/skin/grants/delete", Owner),
    post("/cli/skin/grants/enabled", Owner),
    post("/cli/skin/grants/host", Owner),
    both("/cli/skin/hydra", Owner),
    post("/cli/skin/login", Public),
    post("/cli/skin/logout", Public),
    post("/cli/skin/password/change", Public),
    post("/cli/skin/password/forgot", Public),
    post("/cli/skin/password/reset", Public),
    both("/cli/skin/roles", Owner),
    post("/cli/skin/roles/assign", Owner),
    post("/cli/skin/roles/remove", Owner),
    post("/cli/skin/roles/room", Owner),
    post("/cli/skin/roles/unassign", Owner),
    post("/cli/skin/roles/update", Owner),
    both("/cli/skin/templates", Owner),
    post("/cli/skin/templates/apply", Owner),
    post("/cli/skin/templates/delete", Owner),
    post("/cli/skin/templates/lines", Owner),
    post("/cli/skin/templates/lines/delete", Owner),
    post("/cli/skin/templates/lines/update", Owner),
    post("/cli/skin/templates/update", Owner),
    both("/cli/skin/users", Owner),
    post("/cli/skin/users/email", Owner),
    post("/cli/skin/users/full-name", Owner),
    post("/cli/skin/users/password", Owner),
    post("/cli/skin/users/remove", Owner),
    post("/cli/skin/users/rooms", Owner),
    post("/cli/skin/users/unlock", Owner),
    get("/cli/status", Member),
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
    get("/cli/terminal/spawn", Member),
    get("/cli/terminal/spawn-background", Member),
    get("/cli/terminal/write", Member),
    post("/cli/themes/create-template", Member),
    post("/cli/themes/delete", Member),
    get("/cli/themes/ensure-dir", Member),
    get("/cli/themes/get-dir", Member),
    get("/cli/themes/list", Member),
    get("/cli/thread", Member),
    both("/cli/thread/answer", Member),
    post("/cli/thread/ask", Member),
    both("/cli/thread/post", Member),
    post("/cli/thread/secret", Member),
    both("/cli/thread/void", Member),
    post("/cli/timer/create", Member),
    post("/cli/timer/delete", Member),
    get("/cli/timer/entries-export", Member),
    get("/cli/timer/entries-list", Member),
    get("/cli/trust-folders", Member),
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
    get("/cli/usage/subscriptions", Member),
    post("/cli/usage/subscriptions/refresh", Member),
    get("/cli/usage/tokens", Owner),
    get("/cli/usage/turns", Owner),
    get("/cli/users", Admin),
    post("/cli/users/add", Admin),
    get("/cli/users/audit", Owner),
    split("/cli/users/policy", Member, NoLogin),
    post("/cli/users/remove", Owner),
    post("/cli/users/set-disabled", Admin),
    post("/cli/users/set-password", Owner),
    post("/cli/users/set-role", Owner),
    post("/cli/users/unlock", Owner),
    get("/cli/whats_new", Member),
    get("/cli/whats_new/mark_seen", Member),
    get("/cli/whats_new/reset", Member),
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
    post("/cli/workspace-layouts/delete", Member),
    get("/cli/workspace-layouts/load", Member),
    get("/cli/workspace-layouts/load-all", Member),
    post("/cli/workspace-layouts/save", Member),
    get("/cli/workspace/agent-display-name", Member),
    post("/cli/workspace/api-key", NoLogin),
    get("/cli/workspace/cleanup", Member),
    get("/cli/workspace/create", Member),
    get("/cli/workspace/ensure-canonical-session", Member),
    post("/cli/workspace/ensure-pinned-chat", Member),
    get("/cli/workspace/handle", Member),
    both("/cli/workspace/msg", Member),
    get("/cli/workspace/open", Member),
    get("/cli/workspace/remove", Member),
    get("/cli/workspace/resolve", Member),
    get("/cli/workspace/resources", Member),
    both("/cli/workspace/resources/add", Member),
    both("/cli/workspace/resources/remove", Member),
    get("/cli/workspace/resume-chat-args", Member),
    post("/cli/workspace/set", Member),
    get("/cli/workspace/set-agent-display-name", Member),
    get("/cli/workspace/set-chat-session", Member),
    post("/cli/workspace/set-handle", Member),
    post("/cli/workspace/set-tab-title", Member),
    get("/cli/workspace/tab-titles", Member),
    post("/cli/workspaces/create", Member),
    post("/cli/workspaces/delete", Member),
    get("/cli/workspaces/list", Member),
    post("/cli/workspaces/set-nav-visible", Member),
    get("/cli/worktree", Member),
];

/// Non-`/cli` POST paths the top-level method guard admits. The external
/// `/v1` surface authenticates with API keys and refuses a login on its
/// own; it is outside this table's role check.
pub const V1_POST_EXACT: &[&str] = &["/v1/sandboxes"];

/// Look up the classified row for an exact `/cli/*` path.
pub fn lookup(path: &str) -> Option<&'static Route> {
    ROUTES
        .binary_search_by(|r| r.path.cmp(path))
        .ok()
        .map(|i| &ROUTES[i])
}

/// The POST allowlist (was the inline `matches!` in the dispatcher). A
/// non-GET request whose path is not allowlisted here gets the top-level
/// 405. Sandbox v2 `/v1/w` + `/v1/w/<ws>/…` carry dynamic segments, so
/// they are admitted by prefix and method-gated inside the `/v1/` arm.
pub fn post_allowed(path: &str) -> bool {
    lookup(path).is_some_and(|r| r.post.is_some())
        || V1_POST_EXACT.contains(&path)
        || path == "/v1/w"
        || path.starts_with("/v1/w/")
}

/// Refusal for a login below the route's floor.
fn role_required(floor: Floor, role: k2_core::connect_users::Role) -> CliResponse {
    let message = match floor {
        Floor::NoLogin => {
            "No login can use this route. It takes the owner token on the server.".to_string()
        }
        other => format!(
            "This needs an {} login on this server. You are signed in as {}.",
            other.as_wire(),
            role.as_wire()
        ),
    };
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "role_required",
            "required": floor.as_wire(),
            "role": role.as_wire(),
            "message": message,
        })
        .to_string(),
    }
}

/// Refusal for a login on a `/cli/*` path that is not in [`ROUTES`].
fn unclassified() -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: r#"{"error":"route_unclassified","message":"This route is not in the daemon's route policy table, so no login may use it."}"#
            .to_string(),
    }
}

/// The central login-role check (RV9). Called ONCE per request by the
/// dispatcher, right after the must-change-password gate and before the
/// skin Host belt and the route match.
///
/// Returns `Some(response)` when the request must be refused, else `None`
/// (proceed). It never AUTHORIZES anything — per-route gates still run.
///
/// - Not a `/cli/*` path → `None`.
/// - No token, an empty token, the owner token, or any token that is not
///   a live login session (passport, `k2skn_`, `k2sk_`, `k2rs_`, stream
///   token, garbage) → `None`.
/// - A login on an unclassified path → 404 `route_unclassified`.
/// - A login below the route's floor for this method → 403
///   `role_required` (never the "invalid or missing token" text, which
///   would start the client's re-login loop).
pub fn role_gate(
    method: &str,
    path: &str,
    query: &str,
    owner_token: &str,
) -> Option<CliResponse> {
    if !path.starts_with("/cli/") {
        return None;
    }
    let tok = super::http::extract_token(query)?;
    if tok.is_empty() || super::http::ct_eq_token(tok, owner_token) {
        return None;
    }
    let username = k2_core::connect_users::validate_session(tok)?;
    // A session whose account vanished between the two reads: let the
    // per-route gates refuse it as they always have.
    let role = k2_core::connect_users::role_for_user(&username)?;
    let Some(route) = lookup(path) else {
        return Some(unclassified());
    };
    let floor = if method == "POST" {
        route.post.or(route.get)
    } else {
        route.get.or(route.post)
    };
    // Every row has at least one method (unit-tested); treat the
    // impossible empty row as unclassified rather than open.
    let Some(floor) = floor else {
        return Some(unclassified());
    };
    if floor.admits(role) {
        None
    } else {
        Some(role_required(floor, role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::connect_users::{self, Role};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    // ── table shape ──────────────────────────────────────────────────

    #[test]
    fn routes_are_sorted_unique_and_every_row_has_a_method() {
        for w in ROUTES.windows(2) {
            assert!(
                w[0].path < w[1].path,
                "ROUTES must be strictly sorted (and unique): {:?} then {:?}",
                w[0].path,
                w[1].path
            );
        }
        for r in ROUTES {
            assert!(r.path.starts_with("/cli/"), "non-/cli row: {}", r.path);
            assert!(
                r.get.is_some() || r.post.is_some(),
                "row with no method: {}",
                r.path
            );
        }
    }

    #[test]
    fn post_allowed_matches_the_table_and_the_v1_carve_out() {
        for r in ROUTES {
            assert_eq!(post_allowed(r.path), r.post.is_some(), "{}", r.path);
        }
        assert!(post_allowed("/v1/sandboxes"));
        assert!(post_allowed("/v1/w"));
        assert!(post_allowed("/v1/w/ws-1/sessions"));
        assert!(!post_allowed("/v1/wx"));
        assert!(!post_allowed("/cli/presence/grant"), "the Viewer grant route is gone");
        assert!(!post_allowed("/cli/projects/list"), "a GET read is not POST");
        assert!(!post_allowed("/cli/not-a-route"));
        assert!(post_allowed("/cli/sessions/v2/spawn"));
        assert!(post_allowed("/cli/users/set-role"));
    }

    // ── source walk: every /cli route literal is classified ───────────

    /// Drop `#[cfg(test)]` + `mod x {` blocks (rustfmt shape: the block
    /// closes with a line equal to the attribute's indent + `}`), keeping
    /// line numbers stable.
    fn strip_test_modules(src: &str) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        let mut out = Vec::with_capacity(lines.len());
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i];
            let next_is_mod = lines.get(i + 1).is_some_and(|n| {
                let t = n.trim_start();
                let t = t.strip_prefix("pub(crate) ").or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
                t.starts_with("mod ") && t.trim_end().ends_with('{')
            });
            if l.trim() == "#[cfg(test)]" && next_is_mod {
                let indent = &l[..l.len() - l.trim_start().len()];
                let close = format!("{indent}}}");
                let mut j = i + 2;
                while j < lines.len() && lines[j] != close {
                    j += 1;
                }
                for _ in i..=j.min(lines.len() - 1) {
                    out.push(String::new());
                }
                i = j + 1;
                continue;
            }
            out.push(l.to_string());
            i += 1;
        }
        out
    }

    /// Every full `"/cli/…"` string literal in a code (non-comment)
    /// position, except prefixes (ending in `/`) and `starts_with(` args.
    fn route_literals(src: &str) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        for (n, line) in strip_test_modules(src).iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let bytes = line.as_bytes();
            let mut i = 0;
            while let Some(off) = line[i..].find("\"/cli/") {
                let start = i + off;
                let body_start = start + 1;
                let mut end = body_start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || b"/_.-".contains(&bytes[end]))
                {
                    end += 1;
                }
                i = end.max(start + 1);
                if end >= bytes.len() || bytes[end] != b'"' {
                    continue;
                }
                // Inside a trailing `//` comment (outside any string)?
                if let Some(c) = line.find("//") {
                    if c < start && line[..c].matches('"').count() % 2 == 0 {
                        continue;
                    }
                }
                let lit = &line[body_start..end];
                if lit.ends_with('/') {
                    continue;
                }
                if line[..start].trim_end().ends_with("starts_with(") {
                    continue;
                }
                found.push((n + 1, lit.to_string()));
            }
        }
        found
    }

    /// The files that define routes: top-level `*_routes.rs` / `*_ws.rs`,
    /// `cli.rs`, one-level-nested `*_routes.rs` / `routes*.rs` (mail, sql,
    /// dns, domains), and the dispatcher. Not this file, not `http.rs`.
    fn route_source_files() -> Vec<PathBuf> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src.join("cli.rs"), src.join("routes").join("dispatcher.rs")];
        let top = std::fs::read_dir(&src).expect("read src dir");
        for entry in top {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_file() && (name.ends_with("_routes.rs") || name.ends_with("_ws.rs")) {
                files.push(path);
            } else if path.is_dir() {
                for sub in std::fs::read_dir(&path).expect("read sub dir") {
                    let sub = sub.expect("sub entry");
                    let sname = sub.file_name().to_string_lossy().to_string();
                    if sub.path().is_file()
                        && (sname.ends_with("_routes.rs") || sname.starts_with("routes"))
                    {
                        files.push(sub.path());
                    }
                }
            }
        }
        files.sort();
        files.dedup();
        files
    }

    #[test]
    fn every_cli_route_literal_is_in_the_policy_table() {
        let files = route_source_files();
        assert!(files.len() > 40, "route source walk found too few files: {files:?}");
        let mut seen = 0usize;
        let mut missing: BTreeSet<String> = BTreeSet::new();
        for f in &files {
            let src = std::fs::read_to_string(f)
                .unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
            for (line, lit) in route_literals(&src) {
                seen += 1;
                if lookup(&lit).is_none() {
                    missing.insert(format!(
                        "{lit}  ({}:{line})",
                        f.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap_or(f).display()
                    ));
                }
            }
        }
        assert!(seen > 500, "route source walk saw only {seen} literals — extractor broken?");
        assert!(
            missing.is_empty(),
            "{} /cli route(s) are not in routes/route_policy.rs ROUTES — add a row \
             with the route's minimum login role (Member/Admin/Owner/NoLogin/Public):\n  {}",
            missing.len(),
            missing.into_iter().collect::<Vec<_>>().join("\n  ")
        );
    }

    #[test]
    fn source_walk_extractor_sees_patterns_and_skips_prefixes_comments_and_tests() {
        let src = r#"
fn dispatch(p: &str) {
    match p {
        "/cli/a/one" => {}
        "/cli/a/two" | "/cli/a/three" => {}
        x if x.starts_with("/cli/a/") => {}
        x if x.starts_with("/cli/pre") => {}
        // "/cli/commented/out" => {}
        "/cli/a/four" => {} // "/cli/trailing/comment"
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    fn t() { let _ = "/cli/only/in/tests"; }
}
"#;
        let lits: Vec<String> = route_literals(src).into_iter().map(|(_, l)| l).collect();
        assert_eq!(lits, vec!["/cli/a/one", "/cli/a/two", "/cli/a/three", "/cli/a/four"]);
    }

    // ── the gate ─────────────────────────────────────────────────────

    const OWNER: &str = "route-policy-owner-token";

    fn session(name: &str, role: Role) -> String {
        connect_users::add_user(name, "password123").expect("add_user");
        if role != Role::Member {
            connect_users::set_role(name, role).expect("set_role");
        }
        connect_users::create_session(name)
    }

    fn status(r: &Option<CliResponse>) -> &'static str {
        match r {
            None => "pass",
            Some(resp) => resp.status,
        }
    }

    #[test]
    fn heartbeat_hard_delete_is_owner_only() {
        // Rosson R1 (prd-app-heartbeats-surface-v1): archive stays open to any
        // login; hard delete is the Owner role only.
        let remove = lookup("/cli/heartbeat/remove").expect("heartbeat remove is classified");
        let floor = remove.get.expect("heartbeat remove is a GET route");
        assert_eq!(floor.as_wire(), "owner");
        assert!(!floor.admits(Role::Member));
        assert!(!floor.admits(Role::Admin));
        assert!(floor.admits(Role::Owner));
        let archive = lookup("/cli/heartbeat/archive").expect("heartbeat archive is classified");
        assert_eq!(archive.post.expect("archive accepts POST").as_wire(), "member");
    }

    #[test]
    fn keep_awake_changes_need_admin_and_reading_stays_member() {
        // 0.43.2 Q3: Keep awake changes the host machine's power.
        let set = lookup("/cli/power/keep-awake").expect("keep-awake is classified");
        assert!(set.get.is_none(), "keep-awake is POST-only");
        let floor = set.post.expect("keep-awake accepts POST");
        assert_eq!(floor.as_wire(), "admin");
        assert!(!floor.admits(Role::Member));
        assert!(floor.admits(Role::Admin));
        assert!(floor.admits(Role::Owner));
        let status = lookup("/cli/power/status").expect("power status is classified");
        assert_eq!(status.get.expect("power status is a GET route").as_wire(), "member");
    }

    #[test]
    fn gate_matrix_by_caller_and_floor() {
        crate::test_support::with_temp_home(|| {
            let member = session("rp_member", Role::Member);
            let admin = session("rp_admin", Role::Admin);
            let owner = session("rp_owner", Role::Owner);

            // (path, method) for one row of each floor.
            let member_route = ("POST", "/cli/sessions/v2/spawn");
            let admin_route = ("POST", "/cli/users/add");
            let owner_route = ("POST", "/cli/users/set-role");
            let nologin_route = ("POST", "/cli/tunnel/start");
            let public_route = ("POST", "/cli/auth/login");
            let get_split = ("GET", "/cli/users/policy");
            let post_split = ("POST", "/cli/users/policy");

            let cases: Vec<(String, [&str; 7])> = vec![
                // caller → [member, admin, owner, nologin, public, GET split, POST split]
                (format!("token={OWNER}"), ["pass"; 7]),
                (String::new(), ["pass"; 7]),
                ("token=".to_string(), ["pass"; 7]),
                ("token=not-a-session".to_string(), ["pass"; 7]),
                ("token=sid-123.secret-abc".to_string(), ["pass"; 7]),
                ("token=k2skn_abc".to_string(), ["pass"; 7]),
                ("token=k2sk_abc".to_string(), ["pass"; 7]),
                ("token=k2rs_abc".to_string(), ["pass"; 7]),
                (
                    format!("token={member}"),
                    ["pass", "403 Forbidden", "403 Forbidden", "403 Forbidden", "pass", "pass", "403 Forbidden"],
                ),
                (
                    format!("token={admin}"),
                    ["pass", "pass", "403 Forbidden", "403 Forbidden", "pass", "pass", "403 Forbidden"],
                ),
                (
                    format!("token={owner}"),
                    ["pass", "pass", "pass", "403 Forbidden", "pass", "pass", "403 Forbidden"],
                ),
            ];
            let routes = [
                member_route, admin_route, owner_route, nologin_route, public_route, get_split,
                post_split,
            ];
            for (query, want) in &cases {
                for (k, (method, path)) in routes.iter().enumerate() {
                    let got = role_gate(method, path, query, OWNER);
                    assert_eq!(
                        status(&got),
                        want[k],
                        "caller {query:?} on {method} {path}: body {:?}",
                        got.as_ref().map(|r| r.body.clone())
                    );
                    if let Some(resp) = &got {
                        let v: serde_json::Value =
                            serde_json::from_str(&resp.body).expect("refusal is JSON");
                        assert_eq!(v["error"], "role_required", "{}", resp.body);
                        assert!(
                            !resp.body.contains("invalid or missing token"),
                            "must not trigger the client re-login loop: {}",
                            resp.body
                        );
                    }
                }
            }

            // A login on an unclassified /cli path is refused (404).
            let r = role_gate("GET", "/cli/not-a-classified-route", &format!("token={member}"), OWNER)
                .expect("unclassified must be refused for a login");
            assert_eq!(r.status, "404 Not Found");
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
            assert_eq!(v["error"], "route_unclassified");
            // ... but never for the owner token or a non-login credential.
            assert!(role_gate("GET", "/cli/not-a-classified-route", &format!("token={OWNER}"), OWNER).is_none());
            assert!(role_gate("GET", "/cli/not-a-classified-route", "token=k2skn_x", OWNER).is_none());
            // Non-/cli paths are never gated.
            assert!(role_gate("GET", "/events", &format!("token={member}"), OWNER).is_none());
            assert!(role_gate("GET", "/v1/anything", &format!("token={member}"), OWNER).is_none());

            // A GET on a POST-only path is checked at the POST floor and
            // otherwise falls through to the route's own 405.
            assert!(role_gate("GET", "/cli/sessions/v2/spawn", &format!("token={member}"), OWNER).is_none());
            assert_eq!(
                status(&role_gate("GET", "/cli/users/add", &format!("token={member}"), OWNER)),
                "403 Forbidden"
            );

            // A disabled login's session is dead: the gate passes it on to
            // the per-route gates, which refuse it as before.
            connect_users::set_disabled("rp_admin", true).expect("disable");
            assert!(role_gate("POST", "/cli/users/add", &format!("token={admin}"), OWNER).is_none());
        });
    }
}
