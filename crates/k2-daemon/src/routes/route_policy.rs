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
/// its role is at or above the floor. `Public` and `VoiceChat` admit every
/// login; `Member` admits Member and above, so it REFUSES a Voice & Chat
/// only login (prd-voice-chat-only-members-v1 V1: that one comparison is
/// the role's whole safety); `NoLogin` admits none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Floor {
    /// No credential needed (login, app-pass login/reset, federation
    /// envelopes). A stray login token on it is never refused, except a
    /// Voice & Chat only login's on a row outside [`VOICE_CHAT_PUBLIC`].
    Public,
    /// Any login, Voice & Chat only included: ONLY the rows on
    /// [`VOICE_CHAT_ALLOWLIST`] (a test pins the two together).
    VoiceChat,
    /// Member, Admin or Owner login.
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
            Floor::VoiceChat => "voice_chat",
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
            Floor::Public | Floor::VoiceChat => true,
            Floor::Member => role >= Role::Member,
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

/// Every `(method, path)` a Voice & Chat only login may use
/// (prd-voice-chat-only-members-v1 §5.1, plus the reviewer shares). These
/// are exactly the `VoiceChat` cells of [`ROUTES`]; the test
/// `voice_chat_floor_is_exactly_the_allowlist` walks every row and fails
/// when they differ, so adding a route a Voice & Chat only login can reach
/// is always a reviewed change here. Everything else answers 403
/// `role_required`.
pub const VOICE_CHAT_ALLOWLIST: &[(&str, &str)] = &[
    // Its own account: the client reads the role; first sign-in may use a
    // temporary password; sign out.
    ("GET", "/cli/auth/whoami"),
    ("POST", "/cli/auth/logout"),
    ("POST", "/cli/auth/change-password"),
    ("GET", "/cli/users/policy"),
    // V3 compat only: answers an empty summary and reads nothing, so an
    // older client's Home pool doesn't loop on sign-in.
    ("GET", "/cli/presence/summary"),
    // Voice rooms (the role's core).
    ("GET", "/cli/voice/events"),
    ("GET", "/cli/voice/rooms"),
    ("POST", "/cli/voice/join"),
    ("POST", "/cli/voice/recording/consent"),
    ("POST", "/cli/voice/session/keys/join"),
    ("POST", "/cli/voice/session/keys/key_packages"),
    ("POST", "/cli/voice/session/leave"),
    ("POST", "/cli/voice/session/state"),
    ("POST", "/cli/voice/session/stats"),
    // What the owner shares with this login by id (`shared_routes`): the
    // list, one shared agent's Thread (read, post as the person).
    ("GET", "/cli/shared"),
    ("GET", "/cli/shared/thread"),
    ("POST", "/cli/shared/thread/post"),
    // A shared Garden, read-only, through the phone's remote Garden door
    // (`zen_remote_routes::gate` admits only the shared Gardens and only
    // read verbs for this role).
    ("GET", "/cli/zen/remote/blob/get"),
    ("POST", "/cli/zen/remote/call"),
    ("GET", "/cli/zen/remote/frame"),
    ("GET", "/cli/zen/remote/gardens"),
    ("GET", "/cli/zen/remote/lib"),
    ("POST", "/cli/zen/remote/lib/fetch"),
    ("GET", "/cli/zen/remote/page"),
    ("GET", "/cli/zen/remote/sources-changes"),
];

/// `Public` rows a Voice & Chat only login's token may be presented on
/// (signing in again, an App's own sign-in pages in a browser that also
/// holds the login's cookie, an email link). Every other `Public` row
/// (federation envelopes, compute node doors, webhooks, and any `Public`
/// row added later) refuses it: deny by default.
pub const VOICE_CHAT_PUBLIC: &[&str] = &[
    "/cli/auth/device/pair",
    "/cli/auth/device/pair/check",
    "/cli/auth/login",
    "/cli/email/stop",
    "/cli/email/verify",
    "/cli/skin/app-info",
    "/cli/skin/auth/exchange",
    "/cli/skin/login",
    "/cli/skin/logout",
    "/cli/skin/pair",
    "/cli/skin/pair/check",
    "/cli/skin/password/change",
    "/cli/skin/password/forgot",
    "/cli/skin/password/reset",
    "/cli/skin/password/set",
    "/cli/skin/refresh",
];

// The rows of [`ROUTES`]: one file per busy first path segment
// (`/cli/<area>/...`) and `core.rs` for every other route, each sorted by
// path. Add a row in its area's file.
#[path = "policy/app.rs"]
mod rows_app;
#[path = "policy/compute.rs"]
mod rows_compute;
#[path = "policy/core.rs"]
mod rows_core;
#[path = "policy/mail.rs"]
mod rows_mail;
#[path = "policy/sew.rs"]
mod rows_sew;
#[path = "policy/skin.rs"]
mod rows_skin;
#[path = "policy/vault.rs"]
mod rows_vault;
#[path = "policy/zen.rs"]
mod rows_zen;

/// Every `/cli/*` route the daemon answers, sorted by path (the lookup is
/// a binary search; a unit test pins the order and uniqueness). The rows
/// live in `routes/policy/*.rs` and are merged here when the daemon is
/// compiled: a row out of order in its file, or a path listed twice, fails
/// the build.
pub const ROUTES: &[Route] = &MERGED;
const PARTS: &[&[Route]] = &[rows_app::ROWS, rows_compute::ROWS, rows_core::ROWS, rows_mail::ROWS, rows_sew::ROWS, rows_skin::ROWS, rows_vault::ROWS, rows_zen::ROWS];
const TOTAL: usize = rows_app::ROWS.len() + rows_compute::ROWS.len() + rows_core::ROWS.len() + rows_mail::ROWS.len() + rows_sew::ROWS.len() + rows_skin::ROWS.len() + rows_vault::ROWS.len() + rows_zen::ROWS.len();
const MERGED: [Route; TOTAL] = merge_rows(PARTS);

/// The row files merged into one table sorted by path.
const fn merge_rows<const N: usize>(parts: &[&[Route]]) -> [Route; N] {
    let mut out = [Route { path: "", get: None, post: None }; N];
    let mut at = [0usize; 16];
    assert!(parts.len() <= at.len(), "more row files than merge_rows has room for");
    let mut n = 0;
    while n < N {
        // The file whose next row has the smallest path.
        let mut best = usize::MAX;
        let mut p = 0;
        while p < parts.len() {
            if at[p] < parts[p].len() && (best == usize::MAX || path_lt(parts[p][at[p]].path, parts[best][at[best]].path)) {
                best = p;
            }
            p += 1;
        }
        assert!(best != usize::MAX, "the row files hold fewer rows than TOTAL");
        let row = parts[best][at[best]];
        assert!(n == 0 || path_lt(out[n - 1].path, row.path), "a route row is out of order in its file, or a path is listed twice");
        out[n] = row;
        at[best] += 1;
        n += 1;
    }
    out
}

/// `a < b` by bytes: the order `str::cmp` gives, which the lookup uses.
const fn path_lt(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}

/// Non-`/cli` POST paths the top-level method guard admits. The external
/// `/v1` surface authenticates with API keys and refuses a login on its
/// own; it is outside this table's role check.
pub const V1_POST_EXACT: &[&str] = &[
    "/v1/sandboxes",
    // Agent hire through the v1 API (0.45.6, HT7).
    "/v1/agents/hire",
    "/v1/agents/retire",
    "/v1/agent-teams/link",
    "/v1/agent-teams/unlink",
];

/// Non-`/cli` hook ingest (prd-daemon-activity-and-thread-working-v1 DA12,
/// A4). Authenticated by the owner token or a scoped token pinned to the
/// `X-K2-Pane` session; outside the login role table.
pub const HOOK_POST_EXACT: &[&str] = &["/hook/event"];

/// Look up the classified row for an exact `/cli/*` path: the table as
/// written, in every switch state. This is the classifier (the source
/// walk, the contract and the floor tests read it). Every decision on the
/// wire (the POST allowlist, the GET refusal, the login role gate) reads
/// [`live_row`] instead, which skips the permission-cards rows while that
/// switch is off.
pub fn lookup(path: &str) -> Option<&'static Route> {
    ROUTES
        .binary_search_by(|r| r.path.cmp(path))
        .ok()
        .map(|i| &ROUTES[i])
}

/// The row the wire answers from: [`lookup`], except that the permission
/// cards routes (`/cli/permission/*`, `/cli/thread/permission-note`,
/// `/cli/thread/permission-undo`) are absent while the global "Permission
/// cards" switch is off (prd-agent-settings-cards-v1 §2.4, vs-live C1).
/// Off, they answer exactly as a path with no row does today: the 405
/// guard for a non-GET, `route_unclassified` for a login, the codeless 403
/// for a passport.
pub fn live_row(path: &str) -> Option<&'static Route> {
    live_row_when(path, k2_core::permission::gate::server_on())
}

/// [`live_row`] with the global switch given (tests). Garden access (G0a):
/// its routes exist only with the `gardenAccess` switch on, for every
/// reader of the rows, the POST allowlist included, so off they answer
/// 0.46.3's 405 to a POST (A6, off-golden F1).
fn live_row_when(path: &str, server_on: bool) -> Option<&'static Route> {
    lookup(path)
        .filter(|_| k2_core::permission::gate::route_live_when(path, server_on))
        .filter(|_| crate::zen_access_routes::route_live(path))
}

/// The answer a `/cli/permission/*` handler gives an AGENT whose own "Can
/// ask for permission cards" switch is off while the global switch is on
/// (PRD §2.4): exactly today's unknown-route body for its transport and
/// method, so the agent can't tell the family exists. TCP (port or
/// tunnel): a GET gets the passport's 403 `Invalid or missing auth token`,
/// a POST the 405 guard's body (any other method never gets this far: the
/// guard answers it). The cell socket: its codeless 403 for every method.
/// Pinned against the off-golden fixtures.
pub fn permission_agent_off_response(on_cell: bool, method: &str) -> CliResponse {
    if on_cell || method.eq_ignore_ascii_case("GET") {
        CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"Invalid or missing auth token"}"#.to_string(),
        }
    } else {
        CliResponse {
            status: "405 Method Not Allowed",
            content_type: "application/json",
            body: r#"{"error":"method not allowed for this route"}"#.to_string(),
        }
    }
}

/// The POST allowlist (was the inline `matches!` in the dispatcher). A
/// non-GET request whose path is not allowlisted here gets the top-level
/// 405. Sandbox v2 `/v1/w` + `/v1/w/<ws>/…` carry dynamic segments, so
/// they are admitted by prefix and method-gated inside the `/v1/` arm.
pub fn post_allowed(path: &str) -> bool {
    post_allowed_when(path, k2_core::permission::gate::server_on())
}

/// [`post_allowed`] with the permission-cards switch given (pure; tests).
fn post_allowed_when(path: &str, server_on: bool) -> bool {
    live_row_when(path, server_on).is_some_and(|r| r.post.is_some())
        || V1_POST_EXACT.contains(&path)
        || HOOK_POST_EXACT.contains(&path)
        || crate::apps::app_key_routes::post_allowed(path)
        || path == "/v1/w"
        || path.starts_with("/v1/w/")
}

// ── No state change over GET (security patch 0.44.4) ───────────────────
//
// A `post(...)` row is POST-only: the dispatcher answers every GET to it
// with 405 BEFORE auth and before any handler ([`get_refused`]), for every
// caller (owner token, login, passport, cookie). A `both(...)` /
// `split(...)` row serves GET as a real read; the few whose GET form can
// still write — a mode param flips them from read to write — are listed in
// [`GET_WRITE_PARAMS`], and that GET form is refused the same way. Clients
// send those writes as POST (params may stay in the query string).

/// Routes that changed state on GET before 0.44.4 and are POST-only now.
/// Pinned by tests (each must be a `post(...)` row and must 405 on GET);
/// the per-cell socket admits these as POST for agent verbs.
pub const FORMERLY_GET_WRITES: &[&str] = &[
    "/cli/agent/complete",
    "/cli/agent/reply",
    "/cli/agent/update",
    "/cli/agents-create-connections",
    "/cli/agents/create",
    "/cli/agents/delegate",
    "/cli/agents/delete",
    "/cli/agents/generate-claude-md",
    "/cli/agents/launch",
    "/cli/agents/lock",
    "/cli/agents/reap",
    "/cli/agents/unlock",
    "/cli/awareness/publish",
    "/cli/checkin",
    "/cli/commit",
    "/cli/commit-merge",
    "/cli/dns-manage",
    "/cli/done",
    "/cli/events",
    "/cli/heartbeat/active-projects",
    "/cli/heartbeat/add",
    "/cli/heartbeat/archive",
    "/cli/heartbeat/edit",
    "/cli/heartbeat/enable",
    "/cli/heartbeat/fire",
    "/cli/heartbeat/launch",
    "/cli/heartbeat/reminder/add",
    "/cli/heartbeat/reminder/cancel",
    "/cli/heartbeat/reminder/snooze",
    "/cli/heartbeat/remove",
    "/cli/heartbeat/rename",
    // 0.46.9: new, never a GET; listed for the same query-param POST shape
    // so the per-cell socket admits it like its heartbeat siblings.
    "/cli/heartbeat/restart",
    "/cli/heartbeat/set-session",
    "/cli/heartbeat/set-use-workspace-session",
    "/cli/heartbeat/unarchive",
    "/cli/onboarding/adopt",
    "/cli/onboarding/skip",
    "/cli/onboarding/start-fresh",
    "/cli/release",
    "/cli/remote-instruct",
    "/cli/reserve",
    "/cli/review/approve",
    "/cli/review/feedback",
    "/cli/review/reject",
    "/cli/scheduler-tick",
    "/cli/sessions/label",
    "/cli/sessions/resize",
    "/cli/sessions/v2/close",
    "/cli/sessions/v2/refresh",
    "/cli/sessions/v2/spawn",
    "/cli/skills/regenerate",
    "/cli/status",
    "/cli/terminal/spawn",
    "/cli/terminal/spawn-background",
    "/cli/terminal/write",
    "/cli/themes/ensure-dir",
    "/cli/trust-folders",
    "/cli/whats_new/mark_seen",
    "/cli/whats_new/reset",
    "/cli/workspace/cleanup",
    "/cli/workspace/create",
    "/cli/workspace/ensure-canonical-session",
    "/cli/workspace/msg",
    "/cli/workspace/open",
    "/cli/workspace/remove",
    "/cli/workspace/set-agent-display-name",
    "/cli/workspace/set-chat-session",
    "/cli/worktree",
];

/// `(path, param, read values)`: on these readable routes a GET that
/// carries `param` with any value NOT in `read values` is a write and is
/// refused (405); send it as POST. A GET without `param` stays a read.
pub const GET_WRITE_PARAMS: &[(&str, &str, &[&str])] = &[
    // `?action=add|remove` connects / disconnects; list and resolve read.
    ("/cli/connections", "action", &["", "list", "resolve"]),
    // `?set=<mode>` writes agent_mode (and may spawn the canonical session).
    ("/cli/mode", "set", &[]),
    // any `?enable=` writes the agentic-systems setting.
    ("/cli/agentic", "enable", &[]),
    // `?mode=` writes the workspace heartbeat mode.
    ("/cli/heartbeat/schedule", "mode", &[]),
    // `?out=<path>` writes an attachment into the workspace.
    ("/cli/mail/attachments", "out", &[]),
];

/// GET routes reviewed in 0.44.4 whose read has an incidental, idempotent
/// side effect (a cache refresh, housekeeping, IMAP \Seen on read). They
/// stay GET; the cookie Origin gate is what keeps a foreign page off them.
/// The verb-name ratchet test reads this list too.
// Read by the ratchet tests (the list is the reviewed record).
#[cfg_attr(not(test), allow(dead_code))]
pub const GET_READS_WITH_HOUSEKEEPING: &[(&str, &str)] = &[
    ("/cli/agents/running", "drops dead PTYs from the in-memory session map"),
    ("/cli/auth/device/pair-code/status", "moves pairing codes whose ten minutes ran out to expired (and reports them on the events bus)"),
    ("/cli/auth/devices", "validating the caller's session may reap it or slide a paired phone's expiry"),
    ("/cli/auth/whoami", "validating a session may reap an expired one"),
    // 0.46.10 C1/C3a: lists held trust requests (approve, deny and withdraw
    // are POST routes); listing marks requests past their time expired.
    ("/cli/compute/trust/requests", "moves held trust requests whose time ran out to expired"),
    ("/cli/context/layers", "refreshes generated roster packs on disk when they changed"),
    ("/cli/db/dsn", "lazily mints the agent role's vaulted password / bind membership"),
    ("/cli/federation/peer-roster", "lazy-heals stale connection rows from the peer roster"),
    ("/cli/federation/peer-rosters", "lazy-heals stale connection rows from each peer roster"),
    ("/cli/federation/pubkey", "creates the host keypair once if missing"),
    // The setup checklist: the first read that finds every row done or
    // skipped records `finished_at` once (prd-setup-checklist-v1).
    ("/cli/first-run/status", "records the person's list as finished, once, when every row is done or skipped"),
    ("/cli/heartbeat/active-session", "clears a dead active terminal id / records a live one"),
    ("/cli/host-sessions/list", "drops dead host-session PTYs from the session map"),
    ("/cli/mail/approvals/list", "auto-denies sends pending longer than 7 days"),
    ("/cli/mail/approvals/show", "auto-denies sends pending longer than 7 days"),
    ("/cli/mail/outbox", "auto-denies sends pending longer than 7 days"),
    ("/cli/mail/read", "reading a message marks it \\Seen (IMAP semantics)"),
    // Drafted replies: a tracked draft its mailbox no longer has (sent or
    // deleted in the person's own mail app) is recorded as ended. Never
    // touches the mailbox; never marks anything read.
    ("/cli/mail/reader/draft-open", "records a tracked draft its mailbox no longer has as ended"),
    ("/cli/mail/reader/thread", "records a tracked draft its mailbox no longer has as ended"),
    ("/cli/mail/status", "health=1 refreshes the stored health snapshot"),
    ("/cli/mail/wait", "a matched message is marked \\Seen"),
    ("/cli/power/status", "re-applies the saved keep-awake setting"),
    ("/cli/projects/get-icon", "caches a detected icon url"),
    ("/cli/skill-layers/get-content", "creates the layers folder if missing"),
    ("/cli/skill-layers/list", "creates the layers folder if missing"),
    ("/cli/wiki/index", "regenerates the derived host wiki index"),
    ("/cli/wiki/note", "regenerates the derived host wiki index"),
    ("/cli/workspace/resume-chat-args", "assigns a resume session id when none is saved"),
    ("/cli/zen/get", "snapshots Zen history when the files changed"),
    ("/cli/zen/widget/bundle", "keeps a widget's last good bundle when its folder changed"),
    ("/cli/zen/widgets", "keeps each widget's last good bundle when its folder changed"),
];

/// Central "no state change over GET" check (0.44.4). True when a GET to
/// `path?query` must be refused with 405: the row is POST-only, or the
/// query carries a [`GET_WRITE_PARAMS`] write param. Runs for every
/// caller, before auth.
///
/// The permission-cards family is never refused here (PRD §2.4, vs-live
/// C1): before auth nobody knows the caller's workspace, so refusing a GET
/// to one of its POST rows would tell an agent whose own switch is off that
/// the family exists. Its handlers keep their own method guard after auth.
/// While the global switch is off its rows are absent, so this is false
/// for it today as well.
pub fn get_refused(path: &str, query: &str) -> bool {
    if k2_core::permission::gate::agent_verb_gated(path) {
        return false;
    }
    if live_row(path).is_some_and(|r| r.get.is_none() && r.post.is_some()) {
        return true;
    }
    if !GET_WRITE_PARAMS.iter().any(|(p, _, _)| *p == path) {
        return false;
    }
    get_refused_params(path, &super::http::parse_params(path, query))
}

/// [`get_refused`] over an already-parsed params map (the per-cell socket
/// parses once up front). Same permission-cards rule as [`get_refused`].
pub fn get_refused_params(
    path: &str,
    params: &std::collections::HashMap<String, String>,
) -> bool {
    get_refused_params_when(path, params, k2_core::permission::gate::server_on())
}

/// [`get_refused_params`] with the permission-cards switch given (pure).
fn get_refused_params_when(
    path: &str,
    params: &std::collections::HashMap<String, String>,
    server_on: bool,
) -> bool {
    if k2_core::permission::gate::agent_verb_gated(path) {
        return false;
    }
    if live_row_when(path, server_on).is_some_and(|r| r.get.is_none() && r.post.is_some()) {
        return true;
    }
    let Some((_, param, reads)) = GET_WRITE_PARAMS.iter().find(|(p, _, _)| *p == path) else {
        return false;
    };
    match params.get(*param) {
        None => false,
        Some(v) => !reads.contains(&v.as_str()),
    }
}

/// A route that used to be a GET-shaped verb and is now (also) a POST:
/// the [`FORMERLY_GET_WRITES`] list plus the [`GET_WRITE_PARAMS`] routes.
/// The per-cell socket serves these as POST through the same params
/// handler it used for the GET form.
pub fn is_post_shaped_get_verb(path: &str) -> bool {
    FORMERLY_GET_WRITES.contains(&path) || GET_WRITE_PARAMS.iter().any(|(p, _, _)| *p == path)
}

/// Refusal for a login below the route's floor.
pub(crate) fn role_required(floor: Floor, role: k2_core::connect_users::Role) -> CliResponse {
    let message = match floor {
        Floor::NoLogin => {
            "No login can use this route. It takes the owner token on the server.".to_string()
        }
        _ if role == k2_core::connect_users::Role::VoiceChat => VOICE_CHAT_REFUSAL.to_string(),
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

/// The message every refusal of a Voice & Chat only login carries.
pub const VOICE_CHAT_REFUSAL: &str = "This server gives you Voice & Chat only: you can join its voice rooms and use what the owner shared with you, not open it.";

/// For a door outside `/cli` that takes any login (`/events`, `/status`):
/// 403 `role_required` when the request's credential is a live Voice & Chat
/// only login, else `None` (the door's own check runs as before). Never the
/// "invalid or missing token" text, which starts the client's re-login loop
/// (V4).
pub fn refuse_voice_chat_login(query: &str, owner_token: &str) -> Option<CliResponse> {
    let tok = super::http::extract_token(query)?;
    if tok.is_empty() || super::http::ct_eq_token(tok, owner_token) {
        return None;
    }
    // L6: the session and its role in ONE read; a login store that can't be
    // read refuses (503) instead of passing the request on.
    let Some((_, role)) = k2_core::connect_users::validate_session_role(tok) else {
        return session_shaped_and_unreadable(tok).then(role_unreadable);
    };
    (role == k2_core::connect_users::Role::VoiceChat).then(|| role_required(Floor::Member, role))
}

/// 503 `role_unreadable` (security review L6): a login's role couldn't be
/// read, so nothing that depends on it runs.
pub(crate) fn role_unreadable() -> CliResponse {
    CliResponse {
        status: "503 Service Unavailable",
        content_type: "application/json",
        body: r#"{"error":"role_unreadable","message":"K2 couldn't read this login's role just now; try again."}"#.to_string(),
    }
}

/// A login-session-shaped token while the login store can't be read: the
/// request may be a login whose role nobody can tell (L6).
pub(crate) fn session_shaped_and_unreadable(tok: &str) -> bool {
    k2_core::connect_users::looks_like_session_token(tok) && k2_core::connect_users::store_unreadable()
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
    // L6: the session and its role from ONE read of the login store. Not a
    // live session: the per-route gates decide, unless it is shaped like a
    // login session and the store can't be read (then 503, never through).
    let Some((_username, role)) = k2_core::connect_users::validate_session_role(tok) else {
        return session_shaped_and_unreadable(tok).then(role_unreadable);
    };
    // The permission-cards rows are unclassified while that switch is off.
    let Some(route) = live_row(path) else {
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
    // Deny by default for Voice & Chat only on Public rows too: a token it
    // presents is refused on any Public row not reviewed for it.
    if role == k2_core::connect_users::Role::VoiceChat
        && floor == Floor::Public
        && !VOICE_CHAT_PUBLIC.contains(&path)
    {
        return Some(role_required(Floor::Member, role));
    }
    if floor.admits(role) {
        None
    } else {
        Some(role_required(floor, role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::Floor::Member;
    use k2_core::connect_users::{self, Role};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    // ── table shape ──────────────────────────────────────────────────

    /// Not a check: writes the table, one row a line, to the file named by
    /// `K2_ROUTES_DUMP`. A change to how [`ROUTES`] is put together is shown
    /// to leave every row as it was by comparing the file before and after.
    #[test]
    #[ignore = "writes ROUTES to the file named by K2_ROUTES_DUMP"]
    fn dump_routes() {
        let to = std::env::var("K2_ROUTES_DUMP").expect("K2_ROUTES_DUMP names the file to write");
        let text: String = ROUTES.iter().map(|r| format!("{} GET={:?} POST={:?}\n", r.path, r.get, r.post)).collect();
        std::fs::write(&to, text).expect("write the dump");
    }

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

    /// Garden access (G0a, A6 off-golden F1): off, its rows are absent for
    /// every reader, so a POST meets 0.46.3's 405 guard; on, the table.
    #[test]
    fn garden_access_rows_follow_their_switch_for_the_post_guard_too() {
        let _env = k2_core::test_env::lock();
        let was = k2_core::zen::garden_access::set_switch(false);
        let rows: Vec<&Route> = ROUTES.iter().filter(|r| crate::zen_access_routes::is_route(r.path)).collect();
        assert_eq!(rows.len(), crate::zen_access_routes::ROUTES.len(), "every Garden access route has a row");
        for r in &rows {
            assert!(live_row(r.path).is_none(), "{} is live while off", r.path);
            assert!(!post_allowed(r.path), "{}: off, a POST meets the 405 guard", r.path);
            assert_eq!(post_allowed(r.path), post_allowed("/cli/not-a-route"), "{}", r.path);
        }
        k2_core::zen::garden_access::set_switch(true);
        let on: Vec<(&str, bool, bool)> = rows.iter().map(|r| (r.path, live_row(r.path).is_some(), post_allowed(r.path))).collect();
        k2_core::zen::garden_access::set_switch(was);
        for (r, (path, live, post)) in rows.iter().zip(on) {
            assert!(live, "{path} is live while on");
            assert_eq!(post, r.post.is_some(), "{path}: on, the table's POST");
        }
        assert!(rows.iter().any(|r| r.path == "/cli/zen/garden/access" && r.post.is_some() && r.get.is_some()), "garden/access is GET and POST");
    }

    #[test]
    fn post_allowed_matches_the_table_and_the_v1_carve_out() {
        for r in ROUTES {
            // The permission-cards rows follow their switch (tested below,
            // without touching the process-wide switch).
            if k2_core::permission::gate::route_gated(r.path) {
                continue;
            }
            // Garden access rows follow `gardenAccess` (asserted below).
            if crate::zen_access_routes::is_route(r.path) {
                continue;
            }
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
        // T-S4f (S1 half): the hook installer and the hook ingest.
        assert!(post_allowed("/cli/hooks/install"));
        assert!(post_allowed("/hook/event"));
        assert!(!post_allowed("/hook/complete"), "the legacy hook stays GET-only");
        // S4: the activity snapshot is a plain read.
        assert!(!post_allowed("/cli/activity/snapshot"));
        assert_eq!(lookup("/cli/activity/snapshot").and_then(|r| r.get), Some(Member));
        // S6 (TW9, A26): the Thread strip's catch-up is a plain read.
        assert!(!post_allowed("/cli/thread/activity"));
        assert_eq!(lookup("/cli/thread/activity").and_then(|r| r.get), Some(Member));
    }

    // ── permission cards: the tables follow the global switch (C1) ────
    //
    // These never flip the process-wide switch (other tests in this binary
    // walk ROUTES in parallel); they drive the pure `_when` twins. The
    // daemon-level flip is in tests/permission_gate_integration.rs.

    fn permission_rows() -> Vec<&'static Route> {
        ROUTES.iter().filter(|r| k2_core::permission::gate::route_gated(r.path)).collect()
    }

    #[test]
    fn permission_rows_exist_and_are_every_gated_row() {
        let paths: Vec<&str> = permission_rows().iter().map(|r| r.path).collect();
        for p in ["/cli/permission/request", "/cli/permission/enabled", "/cli/permission/pending", "/cli/permission/agents"] {
            assert!(paths.contains(&p), "{p} has a row: {paths:?}");
        }
        for r in permission_rows() {
            assert!(
                r.path.starts_with(k2_core::permission::ROUTE_PREFIX)
                    || r.path == k2_core::permission::NOTE_ROUTE
                    || r.path == k2_core::permission::UNDO_ROUTE,
                "{} is gated but not a permission route",
                r.path
            );
        }
    }

    #[test]
    fn permission_rows_are_absent_on_the_wire_while_off_and_live_while_on() {
        // Garden access lists (another switch) also open the two Thread card
        // routes; hold the lock its tests flip it under.
        let _env = k2_core::test_env::lock();
        let rows = permission_rows();
        assert!(!rows.is_empty());
        for r in rows {
            // The classifier always sees the row (source walk, contracts).
            assert_eq!(lookup(r.path).map(|x| x.path), Some(r.path));
            // Off: exactly a path with no row.
            assert!(live_row_when(r.path, false).is_none(), "{} is live while off", r.path);
            assert!(!post_allowed_when(r.path, false), "{}: POST must hit today's 405 guard while off", r.path);
            assert_eq!(post_allowed_when(r.path, false), post_allowed_when("/cli/not-a-route", false));
            // On: the row as written.
            assert_eq!(live_row_when(r.path, true).map(|x| x.path), Some(r.path));
            assert_eq!(post_allowed_when(r.path, true), r.post.is_some(), "{}", r.path);
        }
    }

    #[test]
    fn permission_family_is_never_get_refused_before_auth() {
        let none = std::collections::HashMap::new();
        for r in permission_rows() {
            if !k2_core::permission::gate::agent_verb_gated(r.path) {
                continue;
            }
            // A POST-only row: off it has no row (false today); on, the
            // handler's own guard answers after auth (an agent whose own
            // switch is off must get today's body, not a 405 that shows
            // the route exists).
            assert!(!get_refused_params_when(r.path, &none, false), "{}", r.path);
            assert!(!get_refused_params_when(r.path, &none, true), "{}", r.path);
            assert!(!get_refused(r.path, ""), "{}", r.path);
        }
        // Every other POST-only row is still refused on GET, in both states.
        assert!(get_refused_params_when("/cli/heartbeat/remove", &none, false));
        assert!(get_refused_params_when("/cli/heartbeat/remove", &none, true));
    }

    #[test]
    fn the_thread_card_routes_follow_the_switch_too() {
        // Their rows come with lane A2; the gate already covers them, so a
        // row added later is skipped while off with no other change.
        let _env = k2_core::test_env::lock();
        for p in [k2_core::permission::NOTE_ROUTE, k2_core::permission::UNDO_ROUTE] {
            assert!(k2_core::permission::gate::route_gated(p));
            assert!(live_row_when(p, false).is_none());
            assert_eq!(live_row_when(p, true).map(|r| r.path), lookup(p).map(|r| r.path));
        }
    }

    #[test]
    fn agent_off_response_is_todays_unknown_route_body_for_each_transport() {
        // The exact bodies the off-goldens captured for a passport on an
        // unknown /cli route (tests/golden/permission_off/daemon/unknown/).
        let tcp_get = permission_agent_off_response(false, "GET");
        assert_eq!(tcp_get.status, "403 Forbidden");
        assert_eq!(tcp_get.content_type, "application/json");
        assert_eq!(tcp_get.body, r#"{"error":"Invalid or missing auth token"}"#);
        let tcp_post = permission_agent_off_response(false, "POST");
        assert_eq!(tcp_post.status, "405 Method Not Allowed");
        assert_eq!(tcp_post.body, r#"{"error":"method not allowed for this route"}"#);
        for m in ["GET", "POST", "PUT", "DELETE"] {
            let cell = permission_agent_off_response(true, m);
            assert_eq!(cell.status, "403 Forbidden", "{m}");
            assert_eq!(cell.body, r#"{"error":"Invalid or missing auth token"}"#, "{m}");
        }
        let golden = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/golden/permission_off/daemon/unknown/permission_request.txt"),
        )
        .expect("the off-golden for /cli/permission/request");
        for body in [&tcp_get.body, &tcp_post.body] {
            assert!(golden.contains(body.as_str()), "{body} is not a captured off body");
        }
        assert!(golden.contains("## tcp passport\n> GET /cli/permission/request\n> body: \nHTTP/1.1 403 Forbidden\ncontent-type: application/json\n{\"error\":\"Invalid or missing auth token\"}"));
        assert!(golden.contains("## tcp passport\n> POST /cli/permission/request\n> body: {\"action\":\"compute.use\",\"why\":\"x\"}\nHTTP/1.1 405 Method Not Allowed\ncontent-type: application/json\n{\"error\":\"method not allowed for this route\"}"));
        assert!(golden.contains("## cell passport\n> POST /cli/permission/request\n> body: {\"action\":\"compute.use\",\"why\":\"x\"}\nHTTP/1.1 403 Forbidden\ncontent-type: application/json\n{\"error\":\"Invalid or missing auth token\"}"));
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
        // 0.44.4: POST-only (was a GET-shaped verb).
        assert!(remove.get.is_none(), "heartbeat remove is POST-only");
        let floor = remove.post.expect("heartbeat remove is a POST route");
        assert_eq!(floor.as_wire(), "owner");
        assert!(!floor.admits(Role::Member));
        assert!(!floor.admits(Role::Admin));
        assert!(floor.admits(Role::Owner));
        let archive = lookup("/cli/heartbeat/archive").expect("heartbeat archive is classified");
        assert_eq!(archive.post.expect("archive accepts POST").as_wire(), "member");
    }

    // ── 0.44.4: no state change over GET ─────────────────────────────

    #[test]
    fn formerly_get_writes_are_post_only_and_refused_on_get() {
        assert!(FORMERLY_GET_WRITES.len() >= 60, "list shrank: {}", FORMERLY_GET_WRITES.len());
        for p in FORMERLY_GET_WRITES {
            let row = lookup(p).unwrap_or_else(|| panic!("{p} is not classified"));
            assert!(row.get.is_none(), "{p} still serves GET");
            assert!(row.post.is_some(), "{p} has no POST floor");
            assert!(get_refused(p, ""), "{p}: GET must be refused");
            assert!(get_refused(p, "token=x&id=1"), "{p}: GET with params must be refused");
            assert!(is_post_shaped_get_verb(p), "{p}");
        }
        // The terminal + spawn verbs named by the security review.
        for p in [
            "/cli/terminal/write",
            "/cli/terminal/spawn",
            "/cli/terminal/spawn-background",
            "/cli/sessions/resize",
            "/cli/scheduler-tick",
        ] {
            assert!(FORMERLY_GET_WRITES.contains(&p), "{p} missing from the list");
        }
    }

    #[test]
    fn agent_switch_writers_need_admin() {
        for p in ["/cli/dns-manage", "/cli/agents-create-connections", "/cli/remote-instruct"] {
            let row = lookup(p).expect("classified");
            assert!(row.get.is_none(), "{p} is POST-only");
            let floor = row.post.expect("POST floor");
            assert_eq!(floor.as_wire(), "admin", "{p}");
            assert!(!floor.admits(Role::Member), "{p}");
            assert!(floor.admits(Role::Admin), "{p}");
        }
    }

    #[test]
    fn get_write_params_refuse_only_the_write_form() {
        for (p, _, _) in GET_WRITE_PARAMS {
            let row = lookup(p).unwrap_or_else(|| panic!("{p} not classified"));
            assert!(row.get.is_some() && row.post.is_some(), "{p} must serve GET reads and POST writes");
            assert!(!get_refused(p, "project=%2Ftmp%2Fw"), "{p}: plain GET stays a read");
        }
        assert!(get_refused("/cli/connections", "action=add&target=x"));
        assert!(get_refused("/cli/connections", "action=remove&target=x"));
        assert!(get_refused("/cli/connections", "action=bogus"));
        assert!(!get_refused("/cli/connections", "action=list"));
        assert!(!get_refused("/cli/connections", "action=resolve&target=x"));
        assert!(!get_refused("/cli/connections", "action="));
        assert!(get_refused("/cli/mode", "set=manager"));
        assert!(get_refused("/cli/agentic", "enable=1"));
        assert!(get_refused("/cli/heartbeat/schedule", "mode=off"));
        assert!(get_refused("/cli/mail/attachments", "id=1&get=1&out=%2Ftmp%2Fa"));
        assert!(!get_refused("/cli/mail/attachments", "id=1"));
        // Plain reads are never refused.
        for p in ["/cli/projects/list", "/cli/terminal/read", "/cli/sessions/events", "/cli/thread"] {
            assert!(!get_refused(p, "token=x"), "{p}");
        }
    }

    /// Words that name a state change. A GET-serving row whose path holds
    /// one must be a reviewed read ([`GET_READS_WITH_HOUSEKEEPING`],
    /// [`GET_VERB_NAMED_READS`]) or a [`GET_WRITE_PARAMS`] route.
    const MUTATING_WORDS: &[&str] = &[
        "add", "adopt", "apply", "approve", "archive", "bind", "cancel", "claim", "cleanup",
        "close", "commit", "complete", "confirm", "copy", "create", "delegate", "delete",
        "deny", "disable", "done", "edit", "enable", "ensure", "fire", "fresh", "generate",
        "import", "install", "invite", "issue", "kill", "launch", "lock", "mark", "mint",
        "move", "open", "pair", "publish", "put", "reap", "refresh", "regenerate", "reject",
        "release", "remove", "rename", "renew", "reply", "reserve", "reset", "resize",
        "revoke", "rotate", "run", "save", "send", "set", "skip", "spawn", "start", "stop",
        "tick", "trust", "unarchive", "unclaim", "uninstall", "unlock", "update", "upload",
        "write",
    ];

    /// GET-serving rows whose path holds a [`MUTATING_WORDS`] word but
    /// whose GET is a read (verified handler by handler for 0.44.4).
    const GET_VERB_NAMED_READS: &[(&str, &str)] = &[
        ("/cli/vault/send-allow", "GET lists the standing send grants (and a person's picker); adding and removing are POST with a body"),
        ("/cli/vault/run-helper", "returns the path of this daemon's own binary; nothing changes (the run itself is POST /cli/vault/run)"),
        ("/cli/agents/work/create", "410 Gone, no state"),
        ("/cli/folder-trust/status", "reads the scan and the stored decision; grants, cards and decisions are POST routes"),
        ("/cli/agents/work/move", "410 Gone, no state"),
        ("/cli/app/move-plan", "lists a move's steps (doctor, dry-run copy-host); the request is POST /cli/app/move-plan/request"),
        ("/cli/work/inbox/create", "410 Gone, no state"),
        ("/cli/daemon/update/latest", "reads the cached latest-known version; never fetches"),
        ("/cli/daemon/update/status", "reads the updater state"),
        ("/cli/fs/upload/status", "reads an upload session's received ranges; create, complete, abort and token are POST routes"),
        ("/cli/project-config/has-run-command", "reads project config"),
        ("/cli/project-config/run-command", "returns the configured command string; never runs it"),
        ("/cli/publish/client-ip/list", "reads the client-IP opt-in list; writes nothing"),
        ("/cli/publish/leftovers", "lists published-service leftovers"),
        ("/cli/publish/list", "lists published services"),
        ("/cli/publish/logs", "reads service logs"),
        ("/cli/publish/sessions", "lists a site's sessions; ending them is POST /cli/publish/sessions/revoke"),
        ("/cli/publish/validate", "parses an App folder's code; writes nothing"),
        ("/cli/publish/visibility", "reads a site's mode; changing it is the POST"),
        ("/cli/skin/password/set", "serves K2's own fixed set-password page; the password is set by POST /cli/skin/password/reset"),
        ("/cli/skin/pair/status", "reads a pairing's state for its own person; start, cancel and redeem are POST routes"),
        ("/cli/email/stop", "renders the stop page (link scanners follow GETs); only POST /cli/email/stop stops"),
    ];

    fn path_words(path: &str) -> Vec<String> {
        path.trim_start_matches("/cli/")
            .split(['/', '-', '_'])
            .filter(|w| !w.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    }

    #[test]
    fn get_rows_named_like_writes_are_reviewed_reads() {
        let reviewed: BTreeSet<&str> = GET_READS_WITH_HOUSEKEEPING
            .iter()
            .map(|(p, _)| *p)
            .chain(GET_VERB_NAMED_READS.iter().map(|(p, _)| *p))
            .chain(GET_WRITE_PARAMS.iter().map(|(p, _, _)| *p))
            .collect();
        let mut offenders = Vec::new();
        for r in ROUTES {
            if r.get.is_none() {
                continue;
            }
            let words = path_words(r.path);
            if words.iter().any(|w| MUTATING_WORDS.contains(&w.as_str())) && !reviewed.contains(r.path) {
                offenders.push(r.path);
            }
        }
        assert!(
            offenders.is_empty(),
            "GET-serving route(s) named like a state change — make the row `post(...)` \
             (and update every client), or, if the GET is a verified read, add it to \
             GET_VERB_NAMED_READS / GET_READS_WITH_HOUSEKEEPING with the reason:\n  {}",
            offenders.join("\n  ")
        );
        // Every reviewed entry is a real GET row (no stale names).
        for p in &reviewed {
            assert!(lookup(p).is_some_and(|r| r.get.is_some()), "stale reviewed entry {p}");
        }
    }

    // ── clients never GET a POST-only route ──────────────────────────

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repo root")
            .to_path_buf()
    }

    /// `"/cli/…"` literals that follow one of `markers` on the same line.
    fn literals_after(src: &str, markers: &[&str]) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (n, line) in src.lines().enumerate() {
            for m in markers {
                let mut from = 0;
                while let Some(off) = line[from..].find(m) {
                    let start = from + off + m.len();
                    let rest = &line[start..];
                    let lit: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || "/_.-".contains(*c))
                        .collect();
                    if lit.starts_with("/cli/") {
                        out.push((n + 1, lit));
                    }
                    from = start;
                }
            }
        }
        out
    }

    fn get_to_post_only(origin: &str, hits: &[(usize, String)], bad: &mut Vec<String>) {
        for (line, lit) in hits {
            if let Some(r) = lookup(lit) {
                if r.get.is_none() && r.post.is_some() {
                    bad.push(format!("{origin}:{line} GETs POST-only {lit}"));
                }
            }
        }
    }

    fn assert_no_bad(bad: Vec<String>) {
        assert!(bad.is_empty(), "client GET to a POST-only route (would 405):\n  {}", bad.join("\n  "));
    }

    #[test]
    fn the_cli_never_gets_a_post_only_route() {
        let cli = repo_root().join("cli").join("k2");
        let src = std::fs::read_to_string(&cli).unwrap_or_else(|e| panic!("read {}: {e}", cli.display()));
        let hits = literals_after(
            &src,
            &[
                "cli_request \"",
                "cli_request_uds \"",
                "cli_send cli_request \"",
                "http_get(\"",
                "call(\"GET\", \"",
                "request(\"GET\", \"",
            ],
        );
        let n = hits.len();
        let mut bad = Vec::new();
        get_to_post_only("cli/k2", &hits, &mut bad);
        assert_no_bad(bad);
        assert!(n > 200, "CLI GET extractor found only {n} call sites — broken?");
    }

    #[test]
    fn the_renderer_and_tauri_shell_never_get_a_post_only_route() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "ts" || x == "tsx" || x == "rs")
                    && !p.to_string_lossy().contains(".test.")
                    && !p.to_string_lossy().contains(".mstest.")
                {
                    out.push(p);
                }
            }
        }
        let root = repo_root();
        let mut files = Vec::new();
        walk(&root.join("src").join("renderer"), &mut files);
        walk(&root.join("src-tauri").join("src"), &mut files);
        assert!(files.len() > 200, "walk found too few files");
        let mut total = 0;
        let mut bad = Vec::new();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
            // `daemonCliGet<T>(\n  scope,\n  'route'` → `/cli/route` (the
            // call may span lines and carry a type argument). Rust:
            // `cli_get(\n  "/cli/route"`.
            let mut hits = Vec::new();
            for name in ["daemonCliGet", "daemonCliGetText", "hostGet", "cliGet", "cli_get"] {
                let mut from = 0;
                while let Some(off) = src[from..].find(name) {
                    let at = from + off;
                    let after_name = at + name.len();
                    from = after_name;
                    // Whole identifier only (`daemonCliGet` ≠ `daemonCliGetText`).
                    let prev_ok = at == 0
                        || !src.as_bytes()[at - 1].is_ascii_alphanumeric() && src.as_bytes()[at - 1] != b'_';
                    let next = src.as_bytes().get(after_name).copied();
                    if !prev_ok || !matches!(next, Some(b'(') | Some(b'<')) {
                        continue;
                    }
                    let mut window_end = (after_name + 300).min(src.len());
                    while !src.is_char_boundary(window_end) {
                        window_end -= 1;
                    }
                    let window = &src[after_name..window_end];
                    let Some(paren) = window.find('(') else { continue };
                    let rest = &window[paren + 1..];
                    let Some(q) = rest.find(['\'', '"']) else { continue };
                    let quote = rest.as_bytes()[q] as char;
                    let lit_and_on = &rest[q + 1..];
                    let Some(end) = lit_and_on.find(quote) else { continue };
                    let route = lit_and_on[..end].trim_start_matches('/');
                    let route = route.strip_prefix("cli/").unwrap_or(route);
                    if !route.is_empty()
                        && route.chars().all(|c| c.is_ascii_alphanumeric() || "/_-.".contains(c))
                    {
                        let line = src[..at].matches('\n').count() + 1;
                        hits.push((line, format!("/cli/{route}")));
                    }
                }
            }
            let shown = f.strip_prefix(&root).unwrap_or(&f).display().to_string();
            total += hits.len();
            get_to_post_only(&shown, &hits, &mut bad);
        }
        assert_no_bad(bad);
        assert!(total > 100, "renderer GET extractor found only {total} call sites — broken?");
    }

    /// Q7 (prd-agent-app-permissions-v1 slice C): Members are read-only for
    /// Apps. No `/cli/app/*` POST admits a Member login, and the shell
    /// (a WebSocket GET into a running App) is Admin; the reads stay Member.
    #[test]
    fn no_app_route_lets_a_member_change_anything() {
        let mut member_posts = Vec::new();
        let mut posts = 0;
        for r in ROUTES.iter().filter(|r| r.path.starts_with("/cli/app/")) {
            if let Some(f) = r.post {
                posts += 1;
                if f.admits(Role::Member) {
                    member_posts.push(r.path);
                }
            }
        }
        assert!(member_posts.is_empty(), "Member-floor App POSTs: {member_posts:?}");
        assert!(posts >= 30, "the App routes are classified ({posts} POSTs)");
        assert!(!lookup("/cli/app/shell").and_then(|r| r.get).expect("shell GET").admits(Role::Member));
        for read in ["/cli/app/list", "/cli/app/inspect", "/cli/app/logs", "/cli/app/approvals", "/cli/app/quota", "/cli/app/egress"] {
            let f = lookup(read).and_then(|r| r.get).unwrap_or_else(|| panic!("{read} is a GET"));
            assert!(f.admits(Role::Member), "{read} stays a Member read");
        }
        for p in ["/cli/app/suspend", "/cli/app/install", "/cli/app/uninstall", "/cli/app/deny", "/cli/app/guest/rooms"] {
            assert_eq!(lookup(p).and_then(|r| r.post).expect("POST").as_wire(), "admin", "{p}");
        }
    }

    #[test]
    fn a_member_login_cannot_suspend_an_app_at_the_route_floor() {
        // Rosson (0.46.8): the owner, an Admin or the App's own agent suspend.
        crate::test_support::with_temp_home(|| {
            let member = session("susp_member", Role::Member);
            let admin = session("susp_admin", Role::Admin);
            let owner = session("susp_owner", Role::Owner);
            let r = role_gate("POST", "/cli/app/suspend", &format!("token={member}"), OWNER).expect("member refused");
            assert_eq!(r.status, "403 Forbidden", "{}", r.body);
            let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON");
            assert_eq!(v["error"], "role_required", "{v}");
            assert_eq!(v["message"], "This needs an admin login on this server. You are signed in as member.", "{v}");
            assert!(role_gate("POST", "/cli/app/suspend", &format!("token={admin}"), OWNER).is_none(), "admin passes");
            assert!(role_gate("POST", "/cli/app/suspend", &format!("token={owner}"), OWNER).is_none(), "owner login passes");
            assert!(role_gate("POST", "/cli/app/suspend", &format!("token={OWNER}"), OWNER).is_none(), "owner token passes");
            assert!(role_gate("POST", "/cli/app/suspend", "token=sess.secret", OWNER).is_none(), "a passport reaches the handler");
        });
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
    fn llm_server_default_change_needs_admin_and_the_list_stays_member() {
        // Rosson 2026-10-20 (#7): switch / next move every chat on Server
        // default, so they are Admin and above. Reading stays Member.
        for p in ["/cli/llm/accounts/switch", "/cli/llm/accounts/next"] {
            let row = lookup(p).expect("classified");
            assert!(row.get.is_none(), "{p} is POST-only");
            let floor = row.post.expect("POST floor");
            assert_eq!(floor.as_wire(), "admin", "{p}");
            assert!(!floor.admits(Role::Member), "{p}");
            assert!(floor.admits(Role::Admin), "{p}");
            assert!(floor.admits(Role::Owner), "{p}");
        }
        for p in ["/cli/llm/accounts/list", "/cli/llm/accounts/status", "/cli/llm/accounts/usage", "/cli/llm/accounts/for"] {
            assert_eq!(lookup(p).expect("classified").get.expect("GET").as_wire(), "member", "{p}");
        }
        // The phone's chat-header chip: read like the list; its one-token
        // refresh is a Member POST (no GET).
        assert!(lookup("/cli/llm/accounts/for").expect("classified").post.is_none());
        let refresh = lookup("/cli/llm/accounts/for/refresh").expect("classified");
        assert!(refresh.get.is_none(), "for/refresh is POST-only");
        assert_eq!(refresh.post.expect("POST floor").as_wire(), "member");
        crate::test_support::with_temp_home(|| {
            let member = session("llm_member", Role::Member);
            let admin = session("llm_admin", Role::Admin);
            let owner = session("llm_owner", Role::Owner);
            for p in ["/cli/llm/accounts/switch", "/cli/llm/accounts/next"] {
                let r = role_gate("POST", p, &format!("token={member}"), OWNER).expect("member refused");
                assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
                assert!(role_gate("POST", p, &format!("token={admin}"), OWNER).is_none(), "{p}: admin passes");
                assert!(role_gate("POST", p, &format!("token={owner}"), OWNER).is_none(), "{p}: owner login passes");
                assert!(role_gate("POST", p, &format!("token={OWNER}"), OWNER).is_none(), "{p}: owner token passes");
            }
            assert!(role_gate("GET", "/cli/llm/accounts/list", &format!("token={member}"), OWNER).is_none());
        });
    }

    #[test]
    fn desktop_mirrors_refuse_every_connect_login() {
        // 0.46.4 review: the desktop mirrors replace Tauri commands only
        // this computer's app could call; no Connect login of any role may
        // reach them over the tunnel (the handler then refuses passports).
        for p in crate::desktop_mirror_routes::ROUTES {
            let row = lookup(p).unwrap_or_else(|| panic!("{p} is not classified"));
            assert!(row.get.is_none(), "{p} is POST-only");
            assert_eq!(row.post.expect("POST floor").as_wire(), "owner-token", "{p}");
            assert!(post_allowed(p), "{p} must stay on the POST allowlist");
            assert!(get_refused(p, ""), "{p}: GET must be refused");
        }
        crate::test_support::with_temp_home(|| {
            let member = session("mirror_member", Role::Member);
            let admin = session("mirror_admin", Role::Admin);
            let owner = session("mirror_owner", Role::Owner);
            for p in crate::desktop_mirror_routes::ROUTES {
                for (who, tok) in [("member", &member), ("admin", &admin), ("owner login", &owner)] {
                    let r = role_gate("POST", p, &format!("token={tok}"), OWNER)
                        .unwrap_or_else(|| panic!("{p}: a {who} login must be refused"));
                    assert_eq!(r.status, "403 Forbidden", "{p} {who}: {}", r.body);
                }
                assert!(role_gate("POST", p, &format!("token={OWNER}"), OWNER).is_none(), "{p}: owner token passes");
            }
            // inbox/count keeps inbox/list's floor.
            assert_eq!(
                lookup("/cli/inbox/count").expect("classified").get.expect("GET").as_wire(),
                lookup("/cli/inbox/list").expect("classified").get.expect("GET").as_wire(),
            );
        });
    }

    #[test]
    fn wallets_are_for_app_users_never_a_connect_login() {
        // Rosson #9 (0.46.3): Connect members have no wallet. Every person
        // verb is NoLogin (an App user's own session pass); the owner's
        // side is Admin.
        crate::test_support::with_temp_home(|| {
            let member = session("wallet_member", Role::Member);
            let owner = session("wallet_owner", Role::Owner);
            for (m, p) in [
                ("GET", "/cli/skin/wallet/list"),
                ("POST", "/cli/skin/wallet/add"),
                ("POST", "/cli/skin/wallet/remove"),
                ("GET", "/cli/skin/wallet/login/status"),
            ] {
                for who in [&member, &owner] {
                    let r = role_gate(m, p, &format!("token={who}"), OWNER).expect("a Connect login is refused");
                    assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
                }
                assert!(role_gate(m, p, "token=k2skn_abc", OWNER).is_none(), "{p}: an App pass reaches the handler");
            }
            for (m, p) in [("GET", "/cli/app/wallet/people"), ("POST", "/cli/app/wallet/remove"), ("POST", "/cli/app/wallet/settings")] {
                assert!(role_gate(m, p, &format!("token={member}"), OWNER).is_some(), "{p}: Member refused");
                assert!(role_gate(m, p, &format!("token={owner}"), OWNER).is_none(), "{p}: Owner passes");
            }
        });
    }

    #[test]
    fn ticket_rules_change_needs_admin_and_reading_stays_member() {
        // Settings → Tickets (0.45.4): the rules agents are held to.
        let route = lookup("/cli/tickets/settings").expect("tickets settings is classified");
        assert_eq!(route.get.expect("GET reads the rules").as_wire(), "member");
        let floor = route.post.expect("POST changes the rules");
        assert_eq!(floor.as_wire(), "admin");
        assert!(!floor.admits(Role::Member));
        assert!(floor.admits(Role::Admin));
        assert!(floor.admits(Role::Owner));
        assert!(post_allowed("/cli/tickets/settings"));
    }

    // ── Voice & Chat only (prd-voice-chat-only-members-v1 §11) ─────────

    /// Every `(method, row floor)` cell the table declares.
    /// The setup checklist (prd-setup-checklist-v1 C1) is a person's own
    /// list: the Member floor on every route (so Voice & Chat is refused),
    /// never on the Voice & Chat allowlist, never an agent verb, and the
    /// GET chain (an agent's cell socket) refuses it too.
    #[test]
    fn the_setup_checklist_is_a_persons_list_at_the_member_floor() {
        let routes = [
            ("GET", "/cli/first-run/status"),
            ("POST", "/cli/first-run/hide"),
            ("POST", "/cli/first-run/show"),
            ("POST", "/cli/first-run/skip"),
        ];
        for (method, p) in routes {
            let row = lookup(p).unwrap_or_else(|| panic!("{p} is not classified"));
            if method == "GET" {
                assert_eq!((row.get, row.post), (Some(Floor::Member), None), "{p}");
                assert!(!get_refused(p, ""), "{p} is a read");
            } else {
                assert_eq!((row.get, row.post), (None, Some(Floor::Member)), "{p}");
                assert!(get_refused(p, ""), "{p}: a GET is refused");
            }
            assert!(
                !VOICE_CHAT_ALLOWLIST.iter().any(|(_, a)| *a == p),
                "{p} must not be on the Voice & Chat allowlist"
            );
            assert!(!crate::session_token::is_agent_verb(p), "{p}: an agent passport never reaches it");
            let door = crate::first_run_routes::refuse_outside_person_arm(p, method == "POST")
                .unwrap_or_else(|| panic!("{p}: the GET chain must refuse it"));
            assert_eq!(door.status, "403 Forbidden", "{p}");
        }
        // Prefix: a future /cli/first-run/ route is denied to agents too.
        assert!(!crate::session_token::is_agent_verb("/cli/first-run/anything-new"));
        assert!(!crate::session_token::is_agent_verb("/cli/first-run/chief-of-staff/anything"));
        crate::test_support::with_temp_home(|| {
            let vc = session("rp_fr_voice", Role::VoiceChat);
            let member = session("rp_fr_member", Role::Member);
            let passport = "sid-123.secret-abc";
            assert!(connect_users::validate_session(passport).is_none(), "a passport is not a login session");
            for (method, p) in routes {
                let got = role_gate(method, p, &format!("token={vc}"), OWNER);
                assert_eq!(status(&got), "403 Forbidden", "Voice & Chat on {method} {p}");
                let got = role_gate(method, p, &format!("token={member}"), OWNER);
                assert_eq!(status(&got), "pass", "a Member on {method} {p}");
                let got = role_gate(method, p, &format!("token={OWNER}"), OWNER);
                assert_eq!(status(&got), "pass", "the owner token on {method} {p}");
            }
        });
    }

    /// Kessel at first run (prd-setup-checklist-v1 §10): the floors, the one
    /// agent verb in the family, and who the role gate lets through.
    #[test]
    fn the_kessel_routes_have_their_floors_and_one_seat_only_agent_verb() {
        let rows = [
            ("/cli/first-run/offer/dismiss", Floor::Member),
            ("/cli/first-run/kessel", Floor::Admin),
            ("/cli/first-run/chief-of-staff/start", Floor::Admin),
            ("/cli/first-run/chief-of-staff", Floor::NoLogin),
        ];
        for (p, floor) in rows {
            let row = lookup(p).unwrap_or_else(|| panic!("{p} is not classified"));
            assert_eq!((row.get, row.post), (None, Some(floor)), "{p}");
            assert!(get_refused(p, ""), "{p}: a GET is refused");
            assert!(
                !VOICE_CHAT_ALLOWLIST.iter().any(|(_, a)| *a == p),
                "{p} must not be on the Voice & Chat allowlist"
            );
            assert!(crate::first_run_routes::is_route(p) && crate::first_run_routes::is_post_only(p), "{p}");
            let seat_only = p == crate::first_run_routes::COS_PATH;
            assert_eq!(crate::session_token::is_agent_verb(p), seat_only, "{p}: only the seat-only route is an agent verb");
        }
        crate::test_support::with_temp_home(|| {
            let member = session("rp_kessel_member", Role::Member);
            let admin = session("rp_kessel_admin", Role::Admin);
            let owner = session("rp_kessel_owner", Role::Owner);
            let gate = |tok: &str, p: &str| status(&role_gate("POST", p, &format!("token={tok}"), OWNER));
            assert_eq!(gate(&member, "/cli/first-run/offer/dismiss"), "pass");
            for p in ["/cli/first-run/kessel", "/cli/first-run/chief-of-staff/start"] {
                assert_eq!(gate(&member, p), "403 Forbidden", "a Member on {p}");
                assert_eq!(gate(&admin, p), "pass", "an Admin on {p}");
            }
            for tok in [&member, &admin, &owner] {
                assert_eq!(gate(tok, "/cli/first-run/chief-of-staff"), "403 Forbidden", "no login on the seat-only route");
            }
            assert_eq!(gate(OWNER, "/cli/first-run/chief-of-staff"), "pass", "the owner token: the handler refuses it");
        });
    }

    fn cells() -> Vec<(&'static str, &'static Route, Floor)> {
        let mut out = Vec::new();
        for r in ROUTES {
            if let Some(f) = r.get {
                out.push(("GET", r, f));
            }
            if let Some(f) = r.post {
                out.push(("POST", r, f));
            }
        }
        out
    }

    #[test]
    fn the_member_floor_refuses_voice_chat() {
        // V1: this comparison is the role's whole safety.
        assert!(!Floor::Member.admits(Role::VoiceChat));
        assert!(Floor::Member.admits(Role::Member));
        assert!(Floor::VoiceChat.admits(Role::VoiceChat));
        assert!(!Floor::Admin.admits(Role::VoiceChat));
        assert!(!Floor::Owner.admits(Role::VoiceChat));
        assert!(!Floor::NoLogin.admits(Role::VoiceChat));
        assert_eq!(Floor::VoiceChat.as_wire(), "voice_chat");
    }

    /// Deny by default: the `VoiceChat` cells are EXACTLY the reviewed
    /// allowlist, both ways. Adding a route with a `VoiceChat` floor, or a
    /// floor change that lets this role in, fails here until
    /// `VOICE_CHAT_ALLOWLIST` changes with it (a reviewed edit).
    #[test]
    fn voice_chat_floor_is_exactly_the_allowlist() {
        let mut table: BTreeSet<(String, String)> = BTreeSet::new();
        for (m, r, f) in cells() {
            if f == Floor::VoiceChat {
                table.insert((m.to_string(), r.path.to_string()));
            }
        }
        let list: BTreeSet<(String, String)> =
            VOICE_CHAT_ALLOWLIST.iter().map(|(m, p)| (m.to_string(), p.to_string())).collect();
        assert_eq!(list.len(), VOICE_CHAT_ALLOWLIST.len(), "the allowlist has a duplicate");
        assert_eq!(table, list, "VoiceChat rows in ROUTES vs VOICE_CHAT_ALLOWLIST");
        // The PRD's 14 rows plus the reviewer shares (3 + 8 Garden reads).
        assert_eq!(list.len(), 14 + 3 + 8);
        for p in VOICE_CHAT_PUBLIC {
            let r = lookup(p).unwrap_or_else(|| panic!("{p} is not a row"));
            assert!(
                r.get == Some(Floor::Public) || r.post == Some(Floor::Public),
                "{p} on VOICE_CHAT_PUBLIC must be a Public row"
            );
        }
    }

    /// The full-route walk through the real gate with a real Voice & Chat
    /// only session: every row, every declared method (and a GET on every
    /// row) is refused (403 `role_required`, or 404 for a row its switch
    /// hides) unless the cell is on the allowlist or a reviewed Public row.
    #[test]
    fn every_route_refuses_a_voice_chat_login_except_the_allowlist() {
        crate::test_support::with_temp_home(|| {
            let vc = session("rp_voicechat", Role::VoiceChat);
            let q = format!("token={vc}");
            let mut passed = Vec::new();
            let mut checked = 0;
            for r in ROUTES {
                for m in ["GET", "POST"] {
                    let declared = if m == "POST" { r.post.or(r.get) } else { r.get.or(r.post) };
                    let Some(floor) = declared else { continue };
                    checked += 1;
                    let got = role_gate(m, r.path, &q, OWNER);
                    // The cell the gate reads for this method.
                    let cell_m = if m == "POST" { if r.post.is_some() { "POST" } else { "GET" } } else if r.get.is_some() { "GET" } else { "POST" };
                    let allowed = VOICE_CHAT_ALLOWLIST.contains(&(cell_m, r.path))
                        || (floor == Floor::Public && VOICE_CHAT_PUBLIC.contains(&r.path));
                    match (&got, allowed) {
                        (None, true) => passed.push(format!("{m} {}", r.path)),
                        (None, false) => panic!("{m} {} lets a voice_chat login through (floor {floor:?})", r.path),
                        (Some(resp), true) => panic!("{m} {} is allowlisted but refused: {}", r.path, resp.body),
                        (Some(resp), false) => {
                            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
                            if live_row(r.path).is_some() {
                                assert_eq!(resp.status, "403 Forbidden", "{m} {}: {}", r.path, resp.body);
                                assert_eq!(v["error"], "role_required", "{m} {}", r.path);
                                assert_eq!(v["role"], "voice_chat");
                                if floor != Floor::NoLogin {
                                    assert_eq!(v["message"], VOICE_CHAT_REFUSAL, "{m} {}", r.path);
                                }
                            } else {
                                assert_eq!(resp.status, "404 Not Found", "{m} {} (switch off)", r.path);
                            }
                        }
                    }
                }
            }
            assert!(checked > 1700, "walked only {checked} cells");
            // Spot checks the PRD names (§13).
            for (m, p) in [
                ("POST", "/cli/users/add"),
                ("GET", "/cli/projects/list"),
                ("POST", "/cli/voice/moderate"),
                ("POST", "/cli/terminal/spawn"),
                ("POST", "/cli/sessions/v2/spawn"),
                ("GET", "/cli/sessions/grid"),
                ("GET", "/cli/sessions/events"),
                ("GET", "/cli/vault/list"),
                ("GET", "/cli/thread"),
                ("POST", "/cli/thread/post"),
                ("GET", "/cli/agent/list"),
                ("POST", "/cli/users/set-role"),
                ("POST", "/cli/users/set-shares"),
                ("GET", "/cli/zen/remote/agents-order"),
                ("POST", "/cli/zen/remote/blob/put"),
                ("POST", "/cli/sew/pass"),
                ("GET", "/cli/federation/roster"),
            ] {
                assert!(lookup(p).is_some(), "{p} is a row (a renamed route must be renamed here too)");
                assert!(role_gate(m, p, &q, OWNER).is_some(), "{m} {p} must refuse voice_chat");
            }
            assert!(role_gate("POST", "/cli/voice/join", &q, OWNER).is_none());
            assert!(role_gate("GET", "/cli/shared", &q, OWNER).is_none());
            assert!(role_gate("POST", "/cli/auth/login", &q, OWNER).is_none());
            // An unclassified path still 404s.
            assert_eq!(status(&role_gate("GET", "/cli/not-a-route", &q, OWNER)), "404 Not Found");
            // Outside /cli: the /events and /status doors refuse it, never
            // with the stale-session text.
            let r = refuse_voice_chat_login(&q, OWNER).expect("refused");
            assert_eq!(r.status, "403 Forbidden");
            assert!(r.body.contains("role_required") && !r.body.contains("invalid or missing token"));
            assert!(refuse_voice_chat_login(&format!("token={OWNER}"), OWNER).is_none());
            let member = session("rp_vcmember", Role::Member);
            assert!(refuse_voice_chat_login(&format!("token={member}"), OWNER).is_none());
            // A downgrade acts on the next request with no sign-out, and an
            // upgrade gives the Member rows back.
            assert!(role_gate("GET", "/cli/projects/list", &format!("token={member}"), OWNER).is_none());
            connect_users::set_role("rp_vcmember", Role::VoiceChat).expect("down");
            assert_eq!(status(&role_gate("GET", "/cli/projects/list", &format!("token={member}"), OWNER)), "403 Forbidden");
            connect_users::set_role("rp_vcmember", Role::Member).expect("up");
            assert!(role_gate("GET", "/cli/projects/list", &format!("token={member}"), OWNER).is_none());
            assert!(passed.len() >= VOICE_CHAT_ALLOWLIST.len(), "{passed:?}");
        });
    }

    /// Security review L6: when the login store can't be read, a login's
    /// request is refused (503 `role_unreadable`) at the gate and at the
    /// doors outside `/cli`, never passed on.
    #[test]
    fn an_unreadable_role_refuses_instead_of_passing() {
        crate::test_support::with_temp_home(|| {
            let vc = session("rp_unread", Role::VoiceChat);
            let q = format!("token={vc}");
            assert_eq!(status(&role_gate("GET", "/cli/projects/list", &q, OWNER)), "403 Forbidden");
            std::fs::write(connect_users::store_path(), "{ not json").expect("corrupt the login store");
            for (m, p) in [("GET", "/cli/projects/list"), ("GET", "/cli/auth/whoami"), ("POST", "/cli/voice/join"), ("POST", "/cli/auth/login")] {
                let r = role_gate(m, p, &q, OWNER).unwrap_or_else(|| panic!("{m} {p} must not pass with an unreadable role"));
                assert_eq!(r.status, "503 Service Unavailable", "{m} {p}: {}", r.body);
                assert!(r.body.contains("role_unreadable"), "{}", r.body);
            }
            let r = refuse_voice_chat_login(&q, OWNER).expect("the /events and /status doors refuse it too");
            assert_eq!(r.status, "503 Service Unavailable");
            // Non-login credentials are not affected.
            assert!(role_gate("GET", "/cli/projects/list", &format!("token={OWNER}"), OWNER).is_none());
            assert!(role_gate("GET", "/cli/projects/list", "token=sid-1.secret", OWNER).is_none());
        });
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
            let owner_route = ("POST", "/cli/users/set-can-clone");
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
