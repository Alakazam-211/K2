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

use Floor::{Admin, Member, NoLogin, Owner, Public, VoiceChat};

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

/// Every `/cli/*` route the daemon answers, sorted by path (the lookup is
/// a binary search; a unit test pins the order and uniqueness).
pub const ROUTES: &[Route] = &[
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
    // 0.46.9: `k2 app account-deletion` (the handler holds writes to the owner,
    // an Admin or the App's own agent with the App-user switch).
    get("/cli/app/account-deletion", Member),
    post("/cli/app/account-deletion/cancel", Admin),
    post("/cli/app/account-deletion/complete", Admin),
    // App runtime stage 1 (prd-k2-app-runtime-v1 §8.5). Passport rules
    // (own workspace + the two App switches, prd-agent-app-permissions-v1)
    // in the handler. Q7 (Rosson 2026-10-20): Members are read-only, so
    // every mutating `/cli/app/*` floor is Admin (suspend, deny, validate
    // and the shell included; a test below keeps it so). The handlers also
    // refuse a Member login with `member_read_only`.
    // Box prep (S10): the doctor is Admin (agents read it through their
    // passport); prepare runs root-helper verbs and is the owner's.
    // Held App requests (0.46.0 grace + S15 egress, one `app_widenings`
    // table): a passport lists its own workspace's; approve is the owner's
    // in person (handler: owner token or Owner login + a vault confirm
    // bound to the request, never a passport).
    get("/cli/app/approvals", Member),
    // 0.46.6 (client papercut 13): the owner withdraws an approved run's
    // standing for identical re-runs.
    post("/cli/app/approvals/revoke", Owner),
    post("/cli/app/approve", Owner),
    // S19 (0.46.1 door): the per-call audit of an App (a passport: its own
    // workspace's Apps).
    get("/cli/app/audit", Member),
    // 0.46.9 sign-in door: the box switch is the owner's (the handler
    // checks); one App's door and origins the owner's, Admins' and the
    // App's agent's.
    get("/cli/app/auth", Admin),
    post("/cli/app/auth/set", Admin),
    get("/cli/app/box", Admin),
    // Box services (0.46.0): read, propose and revoke are Member (a
    // passport sees its own workspace's Apps); a new one is always a held
    // request (approve is the owner's above); revoking is immediate.
    get("/cli/app/box-service", Member),
    post("/cli/app/box-service/request", Admin),
    post("/cli/app/box-service/revoke", Admin),
    // 0.46.3 F6: the owner's Retry of a blocked (or undone) automatic prep.
    post("/cli/app/box/auto-prepare/retry", Owner),
    // Lane P: remove K2's Podman bundle (prepare --install-podman put it in).
    post("/cli/app/box/podman/uninstall", Owner),
    post("/cli/app/box/prepare", Owner),
    // 0.46.3 F8: undo box prep.
    post("/cli/app/box/unprep", Owner),
    post("/cli/app/build", Admin),
    // App-governed Spaces S1 (0.46.7): an App's chat key. Read: Admin and
    // the App's agent; on / widen: the owner in person (an Admin within
    // the defaults) with a vault confirm; narrow and revoke: Admin or the
    // App's own agent (handler).
    split("/cli/app/chat-key", Admin, Admin),
    post("/cli/app/chat-key/revoke", Admin),
    // 0.46.0 grace: clean mode on narrows (anyone, own Apps for passports);
    // off is held unless the owner turns it off.
    post("/cli/app/clean-mode", Admin),
    // 0.46.9: `k2 app logs <app> --client`, read like the App's own logs.
    get("/cli/app/client-logs", Member),
    // S13: narrowing applies now, widening fields are held.
    post("/cli/app/config", Admin),
    // 0.46.4: read back, list, remove and rename inside an App's data
    // folder (handler: as data put; no Member reads App data).
    get("/cli/app/data/get", Admin),
    get("/cli/app/data/ls", Admin),
    post("/cli/app/data/mv", Admin),
    // 0.46.3: copy workspace files into an App's data folder (handler:
    // only the App's own workspace folder; switch 1 for passports).
    post("/cli/app/data/put", Admin),
    post("/cli/app/data/rm", Admin),
    // deny is an Owner/Admin human's, or the requester's withdraw (handler).
    post("/cli/app/deny", Admin),
    post("/cli/app/deploy", Admin),
    // Litmus L3: who, commit, live/failed/stalled of one deployment.
    get("/cli/app/deployment", Member),
    // 0.46.9: k2 app doctor --store reads a repo folder, as init does.
    post("/cli/app/doctor/store", Admin),
    // 0.46.4: any subdomain of the App's workspace onto the App (handler:
    // as label take; passports their own workspace's Apps and labels).
    post("/cli/app/domain/add", Admin),
    // 0.46.6: custom domains (re-check now; the App's primary origin).
    post("/cli/app/domain/check", Admin),
    // 0.46.6: the owner's own web server serves a custom name (on|off).
    post("/cli/app/domain/external", Admin),
    post("/cli/app/domain/primary", Admin),
    post("/cli/app/domain/rm", Admin),
    get("/cli/app/domains", Member),
    // Egress (S15): read and propose are Member (a passport sees its own
    // workspace's Apps); every new host is a held request; narrowing is
    // immediate.
    get("/cli/app/egress", Member),
    post("/cli/app/egress/add", Admin),
    get("/cli/app/egress/log", Member),
    post("/cli/app/egress/mode", Admin),
    post("/cli/app/egress/rm", Admin),
    // Email Notifications (0.46.5, `k2 app email`): reads, off, test and
    // send are Admin-floor (the App's agent by its passport; send rides
    // its agentSend gate); on is the owner's with a fresh confirm.
    get("/cli/app/email", Admin),
    get("/cli/app/email/log", Admin),
    post("/cli/app/email/off", Admin),
    post("/cli/app/email/on", Owner),
    post("/cli/app/email/send", Admin),
    get("/cli/app/email/status", Admin),
    post("/cli/app/email/test", Admin),
    // The guarded engine's install/update through the root helper (S11).
    post("/cli/app/engine/install", Owner),
    // 0.46.4 (update continuity R9): the engine's status, read-only.
    get("/cli/app/engine/status", Member),
    get("/cli/app/errors", Member),
    // The process exception (§4.3): on = the owner in person with an owner
    // confirm (handler); off (back to the guarded container) = manage.
    post("/cli/app/exception", Admin),
    // S28: own workspace + "Allow this agent to manage Apps" for passports
    // (handler); never App credentials or delegation; audited.
    post("/cli/app/exec", Admin),
    // 0.46.7: a public URL on (or off) for an installed App, no re-create
    // (handler: as install --tunnel; passports their own workspace's App).
    post("/cli/app/expose", Admin),
    // RS-A15 (0.46.1): the owner grants an App guests:rooms; revoking
    // narrows (a passport: its own App). Guest rooms over the CLI: the
    // owner, or a passport within its own App's grant (handler refuses
    // Admin/Member logins).
    post("/cli/app/grant", Owner),
    // 0.46.4: copy box logins onto an App (the owner at once; an agent's
    // passport files one held request: the handler).
    post("/cli/app/grant/copy-host", Owner),
    post("/cli/app/grant/revoke", Admin),
    get("/cli/app/grants", Member),
    // prd-app-caps-0464-v1 AC4 (0.46.4): reads (the owner, or a passport
    // on its own App; the handler refuses logins). GET only.
    get("/cli/app/guest/ceiling", Member),
    // Sew for Apps A2 (0.46.6): `k2 app user chat` (the owner: any person;
    // a passport: its own App's guests within the App's guests-chat
    // grant; the handler refuses Admin/Member logins).
    split("/cli/app/guest/chat", Member, Admin),
    get("/cli/app/guest/list", Member),
    split("/cli/app/guest/rooms", Member, Admin),
    // Q8 (prd-agent-app-permissions-v1 §6): the owner hands an App to
    // another workspace (handler: owner token or Owner login only).
    post("/cli/app/handover", Owner),
    // 0.46.5 (prd-app-handoff-v1, `k2 app handover`): B accepts or
    // declines an offer; A, the owner or an Admin cancels; retry a re-key
    // (B or the owner); end the undo window (the owner, or B after a
    // deploy); the plan, status and brief (handlers: passports their own
    // handovers; the plan never to a Member, it names guests). Undo files a
    // card for the owner. Every write is above Member (Q7).
    post("/cli/app/handover/accept", Admin),
    get("/cli/app/handover/brief", Member),
    post("/cli/app/handover/cancel", Admin),
    post("/cli/app/handover/decline", Admin),
    post("/cli/app/handover/finish", Admin),
    get("/cli/app/handover/plan", Admin),
    post("/cli/app/handover/rekey", Admin),
    get("/cli/app/handover/status", Member),
    post("/cli/app/handover/undo", Owner),
    get("/cli/app/history", Member),
    // 0.46.7 (TC-13): k2 app init --template writes into a folder, as
    // sdk add (handler: switch 1 for passports, their own workspace).
    post("/cli/app/init", Admin),
    get("/cli/app/inspect", Member),
    post("/cli/app/install", Admin),
    // 0.46.0 grace: App-scoped keys. Minting needs the owner's confirm
    // (anyone else's mint is held); revoking narrows (Admin; a passport for
    // its own Apps).
    post("/cli/app/key/mint", Admin),
    post("/cli/app/key/revoke", Admin),
    get("/cli/app/keys", Member),
    // 0.46.3: an App takes its own workspace's legacy label (and gives it
    // back); the handler scopes passports to their workspace.
    post("/cli/app/label/return", Admin),
    post("/cli/app/label/take", Admin),
    // S14: the App's gateway link (status; rotate the App pass now).
    get("/cli/app/link", Member),
    post("/cli/app/link/rotate", Admin),
    // 0.46.7 (prd-tauri-core-0467-v1 TC-20): link files as an App setting;
    // the handler lets the App's own agent ("Build and run Apps") in too.
    // The check fetches over the internet: the owner's.
    get("/cli/app/links", Admin),
    post("/cli/app/links/check", Owner),
    post("/cli/app/links/off", Admin),
    post("/cli/app/links/set", Admin),
    get("/cli/app/list", Member),
    get("/cli/app/logs", Member),
    // Room-scope R9 (0.46.1): who holds a grant on a room (owner/admin any
    // room; a passport its own room; a member login is refused).
    get("/cli/app/members", Member),
    // 0.46.4: the steps of a --cmd service's move; one held request for
    // the owner's steps (handler: passports their own workspace's).
    get("/cli/app/move-plan", Member),
    post("/cli/app/move-plan/request", Admin),
    // prd-app-phone-pairing-v1 (0.46.5): `k2 app pairing …`. Status, off
    // and the log are Admin-floor (the App's agent by its passport; the
    // log also needs its App-user switch); the phone app ids and the
    // network check are the Owner's (set: in person + a fresh confirm).
    get("/cli/app/pairing", Admin),
    post("/cli/app/pairing/check", Owner),
    get("/cli/app/pairing/log", Admin),
    post("/cli/app/pairing/off", Admin),
    post("/cli/app/pairing/set", Owner),
    // Q8: park stops an App and holds it for the owner (lowering: Admin).
    post("/cli/app/park", Admin),
    // app-user-profile (0.46.5): an App's profile settings. The owner or an
    // Admin; the App's own agent (passport) lowers only, in the handler.
    both("/cli/app/profile-settings", Admin),
    // prd-custom-app-push-v1 CP36: status, devices, send are Admin-floor
    // (the App's agent by its passport); keys and check are the Owner's.
    get("/cli/app/push", Admin),
    post("/cli/app/push/check", Owner),
    post("/cli/app/push/credentials", Owner),
    post("/cli/app/push/credentials/remove", Owner),
    get("/cli/app/push/devices", Admin),
    post("/cli/app/push/devices/remove", Admin),
    get("/cli/app/push/log", Admin),
    post("/cli/app/push/send", Admin),
    post("/cli/app/push/set", Admin),
    post("/cli/app/push/test", Admin),
    // S19: per-App quotas. Lowering applies now (manage); a raise is the
    // owner's, anyone else's is a held `quota` request (handler).
    get("/cli/app/quota", Member),
    post("/cli/app/quota/set", Admin),
    // 0.46.4 (update continuity R8/R16): the re-host wave's status, a Member
    // read. `/boot-status` and the update job point here from S1; the
    // handler is S2's (`apps::rehost`).
    get("/cli/app/rehost", Member),
    // Run one App's re-host now (restart-class: switch 1 for its own App,
    // A20) and its update settings (policy, window, "can't run two copies":
    // switch 2; Rosson Q4 + Q16).
    post("/cli/app/rehost/policy", Admin),
    post("/cli/app/rehost/run", Admin),
    // Q8: one way out for every App of a workspace before its removal
    // (uninstall / park: Admin; handover: the handler holds it to the owner).
    post("/cli/app/release", Admin),
    // AR10: remote App passes. Listing and revoking (narrows) are Member;
    // minting is a credential mint, the owner's (handler refuses passports).
    get("/cli/app/remote-pass", Member),
    post("/cli/app/remote-pass/revoke", Admin),
    post("/cli/app/remote-pass/rotate", Owner),
    // 0.46.3 box usage + disk guard (prd-app-resources-disk-guard-v1): the view
    // is owner/Admin/agents (agents never see folder paths, handler); rescan is
    // rate-limited; settings are the owner's (handler: owner token or Owner login).
    get("/cli/app/resources", Admin),
    // 0.46.4 Slice B: clean (an agent only its own Apps', handler) and the
    // CPU/memory trends, the view's readers.
    post("/cli/app/resources/clean", Admin),
    post("/cli/app/resources/rescan", Admin),
    post("/cli/app/resources/settings", Owner),
    get("/cli/app/resources/trends", Admin),
    post("/cli/app/restart", Admin),
    // S19: only the owner resumes a suspended App.
    post("/cli/app/resume", Owner),
    post("/cli/app/rollback", Admin),
    // Room-scope R9: the owner hides guests' names in one room.
    post("/cli/app/room-names", Owner),
    // 0.46.6: @k2/server (and Sew's App kit) into an App folder, its
    // status, and keeping Sew's kit: as build (handler: switch 1 for
    // passports, inside their own workspace).
    post("/cli/app/sdk/add", Admin),
    post("/cli/app/sdk/kit", Admin),
    get("/cli/app/sdk/status", Admin),
    // K-M3 (0.46.3): whether an App has its seal key (never the value);
    // rotating signs everyone out: the owner, or a passport with switch 2
    // for its own App (handler).
    get("/cli/app/seal", Member),
    post("/cli/app/seal/rotate", Owner),
    // Room-scope R16: an App's self (id, live Seed, health, deploy time).
    get("/cli/app/self", Member),
    // 0.46.9: `k2 app set` (account deletion, client log retention): any login
    // reads; the owner, an Admin or the App's agent (users switch) writes.
    split("/cli/app/settings", Member, Admin),
    // S28 `k2 app shell` (WebSocket; same rules as exec).
    get("/cli/app/shell", Admin),
    post("/cli/app/start", Admin),
    // 0.46.7 (TC-15): k2 app status (a passport: its own workspace's Apps;
    // each part keeps its own route's rule).
    get("/cli/app/status", Member),
    post("/cli/app/stop", Admin),
    // S19: suspend narrows, for the owner and Admins (Rosson 0.46.8: never a
    // Member). A passport: its own workspace's Apps, with no switch (handler).
    post("/cli/app/suspend", Admin),
    // 0.46.4: test guests scoped to one App. Guests are the owner's, as
    // `/cli/skin/users` (an agent passport is refused `owner_only` in the
    // handler while agent test guests are parked).
    post("/cli/app/test-guest/add", Owner),
    post("/cli/app/test-guest/password", Owner),
    post("/cli/app/test-guest/rm", Owner),
    get("/cli/app/test-guests", Member),
    // purge (deletes the App's data) is owner-only in the handler.
    post("/cli/app/uninstall", Admin),
    // Q8: the owner gives a parked App to a workspace.
    post("/cli/app/unpark", Owner),
    // 0.46.7: k2 app updates (the update feed). Reads Member; changes
    // Admin (the handler: switch 1 for agents, a key change after releases
    // held for the owner); the fleet view Admin.
    get("/cli/app/updates", Member),
    get("/cli/app/updates/fleet", Admin),
    get("/cli/app/updates/key", Member),
    post("/cli/app/updates/key/set", Admin),
    post("/cli/app/updates/promote", Admin),
    post("/cli/app/updates/pull", Admin),
    post("/cli/app/updates/push", Admin),
    post("/cli/app/updates/rollback", Admin),
    get("/cli/app/updates/stats", Member),
    get("/cli/app/updates/token", Admin),
    post("/cli/app/updates/token/set", Admin),
    // Room-scope R16: an App's quotas and per-call counts from its audit.
    get("/cli/app/usage", Member),
    post("/cli/app/validate", Admin),
    // App users' personal wallets (0.46.3), the owner's side: existence,
    // label and status only; force-remove; personal agents; per-App
    // settings. Admin floor for logins (the handler also takes the owner
    // token, and a switch-2 agent for its own App's fallback).
    get("/cli/app/wallet/people", Admin),
    post("/cli/app/wallet/personal-agent", Admin),
    get("/cli/app/wallet/personal-agents", Admin),
    post("/cli/app/wallet/remove", Admin),
    split("/cli/app/wallet/settings", Admin, Admin),
    // 0.46.0 grace: what breaks when a service moves to an App.
    get("/cli/app/warnings", Member),
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
    post("/cli/ci/freeze", Member),
    post("/cli/ci/init", Member),
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
    // K2 compute nodes (prd-k2-compute-nodes-v1 §15.1, CN18). attach and
    // enroll are the node's sockets: no HTTP credential, handlers fail
    // closed (CN11). Agent verbs are also `is_agent_verb` rows; owner/admin
    // verbs refuse passports in the handler (`owner_only`).
    // Node admin (prd-k2-node-admin-v1 §13.1). approve is Owner (Q2) and
    // needs a fresh vault confirm on top; passports are checked in the
    // handler (operate switch), never approve/deny.
    get("/cli/compute/admin/approval-key", Admin),
    post("/cli/compute/admin/approve", Owner),
    post("/cli/compute/admin/deny", Admin),
    post("/cli/compute/admin/propose", Admin),
    get("/cli/compute/admin/requests", Admin),
    post("/cli/compute/admin/withdraw", Admin),
    get("/cli/compute/agents", Admin),
    post("/cli/compute/agents/set", Admin),
    // A workspace's node list (0.46.8): anyone who may see its compute
    // reads it; the handler lets a person (owner, Admin) change it and an
    // agent passport only narrow its own (`compute_allow`).
    get("/cli/compute/allow-nodes", Member),
    post("/cli/compute/allow-nodes/set", Member),
    get("/cli/compute/attach", Public),
    // Job caches on the nodes (`--cache NAME`, 0.46.6): list and clear,
    // owner/Admin (or an agent under "Allow managing compute nodes").
    get("/cli/compute/cache", Admin),
    post("/cli/compute/cache/clear", Admin),
    get("/cli/compute/enroll", Public),
    get("/cli/compute/events", Admin),
    // Git access for jobs (0.46.9, GH#78): anyone who may see the
    // workspace's compute reads it; the handler lets a person grant, an
    // agent revoke its own; setup is the owner's or an Admin's.
    get("/cli/compute/git-access", Member),
    post("/cli/compute/git-access/grant", Member),
    post("/cli/compute/git-access/revoke", Member),
    post("/cli/compute/git-access/setup", Admin),
    get("/cli/compute/grants", Member),
    post("/cli/compute/grants/revoke", Admin),
    post("/cli/compute/grants/set", Admin),
    get("/cli/compute/jobs", Member),
    get("/cli/compute/jobs/artifact", Member),
    post("/cli/compute/jobs/cancel", Member),
    get("/cli/compute/jobs/get", Member),
    get("/cli/compute/jobs/logs", Member),
    post("/cli/compute/jobs/move", Member),
    get("/cli/compute/jobs/receipt", Member),
    post("/cli/compute/jobs/retry", Member),
    post("/cli/compute/jobs/tier", Member),
    // Node rosters, lanes, seats (0.46.10 C1/C3a, `compute_trust`): an
    // operator files (owner, Admin, or an agent with the operate switch);
    // a person decides (trust/approve, roster/key, consent on: the handler
    // refuses passports and needs a fresh confirm).
    get("/cli/compute/lanes", Admin),
    post("/cli/compute/lanes/set", Admin),
    post("/cli/compute/local/disable", Owner),
    post("/cli/compute/local/drain", Owner),
    post("/cli/compute/local/enable", Owner),
    post("/cli/compute/local/pause", Owner),
    post("/cli/compute/local/policy", Owner),
    // The remote-admin toggles (prd-compute-update-continuity-v1 Q4).
    post("/cli/compute/local/remote-admin", Owner),
    post("/cli/compute/local/resume", Owner),
    get("/cli/compute/local/status", Member),
    get("/cli/compute/nodes", Member),
    post("/cli/compute/nodes/alerts", Admin),
    post("/cli/compute/nodes/confirm", Owner),
    post("/cli/compute/nodes/enroll-code", Owner),
    get("/cli/compute/nodes/health", Member),
    get("/cli/compute/nodes/info", Member),
    // Package requests (0.46.9, prd-compute-self-serve-environments §12.2):
    // members with a grant read and ask; the owner or an Admin, in person,
    // decide (the handler refuses passports); the policy is an operator's
    // (narrowing at once, widening held).
    get("/cli/compute/nodes/packages", Member),
    post("/cli/compute/nodes/packages/decide", Admin),
    post("/cli/compute/nodes/packages/policy", Member),
    post("/cli/compute/nodes/packages/request", Member),
    get("/cli/compute/nodes/packages/requests", Member),
    post("/cli/compute/nodes/pause", Admin),
    get("/cli/compute/nodes/policy", Admin),
    post("/cli/compute/nodes/policy/set", Admin),
    post("/cli/compute/nodes/remove", Owner),
    post("/cli/compute/nodes/rename", Admin),
    post("/cli/compute/nodes/retire", Owner),
    post("/cli/compute/nodes/rollback", Admin),
    // k2-node rollouts (prd-compute-update-continuity-v1 §5.10): the
    // handler's operator() gate, as for update.
    get("/cli/compute/nodes/rollout", Admin),
    post("/cli/compute/nodes/rollout/pause", Admin),
    post("/cli/compute/nodes/rollout/resume", Admin),
    post("/cli/compute/nodes/rollout/stop", Admin),
    post("/cli/compute/nodes/routes", Admin),
    // Node sharing (prd-compute-shared-queue-v1 §3, §9): the servers on a
    // node this server owns. Agents need "Allow managing compute nodes" or
    // the operate switch (checked in the handler); revoke is the owner's.
    get("/cli/compute/nodes/servers", Admin),
    post("/cli/compute/nodes/servers/add", Admin),
    post("/cli/compute/nodes/servers/limits", Admin),
    // Changing a node's owner server is the owner's alone (Q9), and held
    // for the owner's approval (prd-macos-tenant-compute-v1 §6).
    post("/cli/compute/nodes/servers/owner", Owner),
    post("/cli/compute/nodes/servers/pause", Admin),
    post("/cli/compute/nodes/servers/revoke", Owner),
    // A node that says this server owns it now is taken over only by the
    // owner, in person (prd-macos-tenant-compute-v1 MT36).
    post("/cli/compute/nodes/take-owner", Owner),
    post("/cli/compute/nodes/update", Admin),
    // Q5 (0.46.4): an operator turns the Linux job wall on (handler: operator()).
    post("/cli/compute/nodes/wall", Admin),
    get("/cli/compute/owners", Admin),
    post("/cli/compute/owners/consent", Admin),
    // 0.46.7 (CE-18): Batch jobs that install --needs on granted nodes.
    post("/cli/compute/prewarm", Member),
    // A node's queue, redacted per §7 (an agent verb: its own workspace).
    get("/cli/compute/queue", Member),
    get("/cli/compute/roster", Admin),
    post("/cli/compute/roster/key", Admin),
    post("/cli/compute/roster/propose", Admin),
    post("/cli/compute/roster/publish", Admin),
    post("/cli/compute/run", Member),
    get("/cli/compute/seats", Admin),
    // Seat end / suspend / resume (11-07 trust review M7): the owner, in
    // person (the handler: owner_person + a fresh confirm bound to the seat).
    post("/cli/compute/seats/end", Admin),
    post("/cli/compute/seats/offer", Admin),
    post("/cli/compute/seats/request", Admin),
    post("/cli/compute/seats/resume", Admin),
    post("/cli/compute/seats/suspend", Admin),
    post("/cli/compute/trust/approve", Admin),
    post("/cli/compute/trust/deny", Admin),
    get("/cli/compute/trust/requests", Admin),
    post("/cli/compute/trust/withdraw", Admin),
    get("/cli/compute/usage", Admin),
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
    post("/cli/desktop/grant", Owner),
    get("/cli/desktop/grants", Owner),
    post("/cli/desktop/kick", Owner),
    post("/cli/desktop/open", Member),
    post("/cli/desktop/policy", Owner),
    post("/cli/desktop/revoke", Owner),
    get("/cli/desktop/status", Member),
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
    get("/cli/docs/cores", Member),
    post("/cli/docs/cursor", Member),
    get("/cli/docs/diff", Member),
    post("/cli/docs/edit", Member),
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
    post("/cli/mail/address/person", Admin),
    post("/cli/mail/address/person-link", Admin),
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
    get("/cli/mail/approvals/show", Admin),
    post("/cli/mail/archive", Member),
    both("/cli/mail/attachments", Member),
    both("/cli/mail/autoconfig", Admin),
    // S8 B1 backup: status read (like /cli/mail/status), plan + set are
    // mail-manage (Admin floor for logins, like quota/cert).
    get("/cli/mail/backup", Member),
    get("/cli/mail/backup/plan", Admin),
    post("/cli/mail/backup/set", Admin),
    both("/cli/mail/bans", Admin),
    post("/cli/mail/bans/clear", Admin),
    both("/cli/mail/bans/migrate", Admin),
    post("/cli/mail/bans/migrate/restore", Admin),
    // Calendars S3: agent calendar verbs, gated by k2 mail access levels
    // in the handlers (read: GET; draft: POST) — Member floor like
    // messages/draft.
    post("/cli/mail/calendar/create", Member),
    post("/cli/mail/calendar/delete", Member),
    get("/cli/mail/calendar/events", Member),
    get("/cli/mail/calendar/freebusy", Member),
    get("/cli/mail/calendar/list", Member),
    // Calendars S4: groups + sharing (mail-manage surface).
    post("/cli/mail/calendar/manage", Admin),
    post("/cli/mail/calendar/share", Admin),
    get("/cli/mail/calendar/shares", Admin),
    both("/cli/mail/calendar/sharing", Admin),
    get("/cli/mail/calendar/show", Member),
    post("/cli/mail/calendar/unshare", Admin),
    post("/cli/mail/calendar/update", Member),
    get("/cli/mail/calendar/wait", Member),
    // MCS1: per-inbox capabilities (agent view = own grants; owner view =
    // all inboxes, owner-or-admin in the dispatcher, like inboxes).
    get("/cli/mail/capabilities", Member),
    both("/cli/mail/catchall", Admin),
    both("/cli/mail/cert/names", Admin),
    both("/cli/mail/cert/owner", Admin),
    post("/cli/mail/cert/renew", Admin),
    get("/cli/mail/config", Member),
    post("/cli/mail/config/forward-copies-per-day", Owner),
    post("/cli/mail/config/forward-copies-per-hour", Owner),
    post("/cli/mail/config/forward-new-per-day", Owner),
    post("/cli/mail/config/set", Admin),
    post("/cli/mail/credentials/keep", Admin),
    both("/cli/mail/dav", Admin),
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
    post("/cli/mail/forward-message", Member),
    post("/cli/mail/forward/add", Admin),
    post("/cli/mail/forward/pause", Admin),
    post("/cli/mail/forward/resume", Admin),
    post("/cli/mail/forward/rm", Admin),
    post("/cli/mail/forward/unset", Admin),
    get("/cli/mail/forwards", Admin),
    both("/cli/mail/group", Admin),
    post("/cli/mail/group/delete", Admin),
    both("/cli/mail/group/members", Admin),
    post("/cli/mail/import", Admin),
    get("/cli/mail/inboxes", Member),
    post("/cli/mail/link/add", Member),
    // MCS3: the owner's switch for calendars on linked inboxes.
    both("/cli/mail/link/calendars", Admin),
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
    both("/cli/mail/outbound", Admin),
    get("/cli/mail/outbox", Member),
    post("/cli/mail/outbox/cancel", Member),
    get("/cli/mail/preflight", Member),
    post("/cli/mail/profile", Admin),
    both("/cli/mail/ptr", Admin),
    post("/cli/mail/ptr/set", Admin),
    get("/cli/mail/queue", Admin),
    post("/cli/mail/queue/drop", Admin),
    post("/cli/mail/queue/retry", Admin),
    both("/cli/mail/quota", Admin),
    get("/cli/mail/read", Member),
    post("/cli/mail/reply", Member),
    both("/cli/mail/self/forward", Member),
    post("/cli/mail/self/forward/add", Member),
    post("/cli/mail/self/forward/pause", Member),
    post("/cli/mail/self/forward/resume", Member),
    post("/cli/mail/self/forward/rm", Member),
    post("/cli/mail/self/forward/unset", Member),
    post("/cli/mail/send", Member),
    post("/cli/mail/server/disable", Admin),
    post("/cli/mail/server/enable", Admin),
    post("/cli/mail/server/rotate-admin", Admin),
    post("/cli/mail/server/uninstall", Admin),
    post("/cli/mail/server/upgrade", Admin),
    // 0.46.6: read-only Sieve audit (owner/admin, never mail_manage).
    get("/cli/mail/sieve-audit", Admin),
    post("/cli/mail/spam/allow", Admin),
    post("/cli/mail/spam/block", Admin),
    get("/cli/mail/spam/quarantine", Admin),
    post("/cli/mail/spam/quarantine/discard", Admin),
    post("/cli/mail/spam/quarantine/release", Admin),
    post("/cli/mail/spam/train", Admin),
    get("/cli/mail/status", Member),
    get("/cli/mail/wait", Member),
    // 0.46.11: agent mail notices. An agent reaches only its own (the
    // handlers take the workspace from its passport).
    get("/cli/mail/watch", Member),
    get("/cli/mail/watch/inbox", Member),
    post("/cli/mail/watch/inboxes", Member),
    get("/cli/mail/watch/log", Member),
    post("/cli/mail/watch/pause", Member),
    // Everyone's notices at once: an Admin's call, never an agent's (W23).
    post("/cli/mail/watch/pause-all", Admin),
    post("/cli/mail/watch/redeliver", Member),
    post("/cli/mail/watch/set", Member),
    post("/cli/mail/watch/vip", Member),
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
    // Sew on K2. Floors are for Connect logins; the owner token and agent
    // passports pass to the handlers, which apply the real gates:
    // - S29 server (K12) and S28 keys/pass/handoff: reads for any login,
    //   the lifecycle Admin (or a passport with "Manage Sew"); purge, the
    //   recovery key and drop-prev are person-only (owner + vault confirm).
    // - S30 `k2 chat` agent verbs `/cli/sew/chat/*` incl. community
    //   management `/cli/sew/chat/manage/*` (K17, K20): passports only
    //   (NoLogin); manage also needs Manage Sew + level manage.
    // - S30 approvals and the admin plane (K18, K19): Admin for logins, or
    //   a Manage-Sew passport (no self-loosening; approve is person-only
    //   with a vault confirm unless approvals.by allows agents).
    // - S30 on-behalf opt-ins: the person themselves (the handler refuses
    //   everyone else and passports), so any login reaches the handler.
    get("/cli/sew/admin/agents", Admin),
    post("/cli/sew/admin/agents/grant", Admin),
    post("/cli/sew/admin/agents/revoke", Admin),
    post("/cli/sew/admin/agents/set", Admin),
    // S31 K24: the delivery hub per workspace (Manage Sew for passports).
    get("/cli/sew/admin/deliveries", Admin),
    post("/cli/sew/admin/deliveries/reset", Admin),
    post("/cli/sew/admin/deliveries/set", Admin),
    get("/cli/sew/admin/log", Admin),
    get("/cli/sew/admin/people", Admin),
    post("/cli/sew/admin/people/grant", Admin),
    post("/cli/sew/admin/people/revoke", Admin),
    post("/cli/sew/admin/people/update", Admin),
    // - S32 K31: App ↔ Space bindings (read: any login; bind/unbind: the
    //   Manage Sew gate in the handler, Admin floor for logins).
    // ("app-spaces" sorts before "approvals": '-' < 'r'.)
    // Sew 0.6.0: `k2 app chat <app>` (a read), and the governance switch
    // (the owner or an Admin login in person, with a confirm: the handler).
    get("/cli/sew/app-chat", Member),
    get("/cli/sew/app-spaces", Member),
    post("/cli/sew/app-spaces/bind", Admin),
    post("/cli/sew/app-spaces/governance", Admin),
    post("/cli/sew/app-spaces/unbind", Admin),
    // Sew for Apps A6 (0.46.6): chat.member / chat.whois, an App's own
    // pass with chat:read (the handler refuses the rest).
    // Sew for Apps A9 (0.46.6): chat.createChannel, an App's own pass
    // with chat:channels in its own Space (the handler refuses the rest).
    post("/cli/sew/app/channels", NoLogin),
    // Sew 0.6.0: chat.channels / chat.setChannels, an App's own pass in its
    // own App-governed Space, with a chat key (the handler refuses the rest).
    post("/cli/sew/app/channels/bulk", NoLogin),
    get("/cli/sew/app/member", NoLogin),
    post("/cli/sew/app/member-channels", NoLogin),
    get("/cli/sew/app/space", NoLogin),
    post("/cli/sew/app/whois", NoLogin),
    get("/cli/sew/approvals", Admin),
    post("/cli/sew/approvals/approve", Admin),
    post("/cli/sew/approvals/approve-batch", Admin),
    post("/cli/sew/approvals/deny", Admin),
    get("/cli/sew/approvals/policy", Admin),
    post("/cli/sew/approvals/policy/set", Admin),
    get("/cli/sew/backups", Member),
    get("/cli/sew/chat/access", NoLogin),
    get("/cli/sew/chat/channels", NoLogin),
    post("/cli/sew/chat/delete", NoLogin),
    post("/cli/sew/chat/dm", NoLogin),
    post("/cli/sew/chat/edit", NoLogin),
    post("/cli/sew/chat/follow", NoLogin),
    post("/cli/sew/chat/manage/ban", NoLogin),
    post("/cli/sew/chat/manage/channel/create", NoLogin),
    post("/cli/sew/chat/manage/channel/perms", NoLogin),
    post("/cli/sew/chat/manage/channel/set", NoLogin),
    post("/cli/sew/chat/manage/role/add", NoLogin),
    post("/cli/sew/chat/manage/role/create", NoLogin),
    post("/cli/sew/chat/manage/role/remove", NoLogin),
    post("/cli/sew/chat/manage/space/create", NoLogin),
    get("/cli/sew/chat/manage/space/members", NoLogin),
    post("/cli/sew/chat/manage/space/members/add", NoLogin),
    post("/cli/sew/chat/manage/space/members/remove", NoLogin),
    post("/cli/sew/chat/manage/timeout", NoLogin),
    post("/cli/sew/chat/manage/unban", NoLogin),
    post("/cli/sew/chat/mark-read", NoLogin),
    get("/cli/sew/chat/messages", NoLogin),
    get("/cli/sew/chat/outbox", NoLogin),
    post("/cli/sew/chat/outbox/cancel", NoLogin),
    // S31 K24: an agent sets its OWN delivery prefs (self-quieting).
    get("/cli/sew/chat/prefs", NoLogin),
    post("/cli/sew/chat/prefs/set", NoLogin),
    get("/cli/sew/chat/read", NoLogin),
    post("/cli/sew/chat/reply", NoLogin),
    post("/cli/sew/chat/send", NoLogin),
    get("/cli/sew/chat/spaces", NoLogin),
    post("/cli/sew/chat/unfollow", NoLogin),
    get("/cli/sew/chat/unread", NoLogin),
    get("/cli/sew/chat/wait", NoLogin),
    get("/cli/sew/chat/whoami", NoLogin),
    // Decision 13 (Sew for Apps, 0.46.6): the home client and an App's
    // "App-only" mark (read: any login; writes: Manage Sew, Admin floor).
    get("/cli/sew/home", Member),
    post("/cli/sew/home/app", Admin),
    post("/cli/sew/home/client", Admin),
    get("/cli/sew/job", Member),
    get("/cli/sew/keys", Admin),
    post("/cli/sew/keys/drop-previous", Admin),
    post("/cli/sew/keys/rotate", Admin),
    post("/cli/sew/media/install", Admin),
    get("/cli/sew/media/preflight", Member),
    post("/cli/sew/media/probe", Admin),
    post("/cli/sew/media/rebind", Admin),
    post("/cli/sew/media/renew", Admin),
    post("/cli/sew/media/tier", Admin),
    post("/cli/sew/media/uninstall", Admin),
    post("/cli/sew/media/url", Admin),
    post("/cli/sew/meet/bind", Admin),
    post("/cli/sew/meet/unbind", Admin),
    get("/cli/sew/on-behalf", Member),
    post("/cli/sew/on-behalf/review", Member),
    post("/cli/sew/owner-handoff", Owner),
    post("/cli/sew/pass", Member),
    get("/cli/sew/preflight", Member),
    // S32 (K27): the app URL behind Open Sew as owner (Manage Sew).
    post("/cli/sew/server/app-url", Admin),
    post("/cli/sew/server/backup", Admin),
    post("/cli/sew/server/config", Admin),
    post("/cli/sew/server/disable", Admin),
    post("/cli/sew/server/doctor", Admin),
    post("/cli/sew/server/enable", Admin),
    // S32 (K27): Sew's keyring mirror (Manage Sew; never the vault).
    post("/cli/sew/server/mirror", Admin),
    // 0.46.6 (Sew 2026-10-27): `sew install --reconfigure` by hand, and
    // `k2 hostchat restart` (Manage Sew in the handler).
    post("/cli/sew/server/reconfigure", Admin),
    // 0.46.8 (voice rooms PRD v1.9 §2.7): the launch key is the owner's only.
    post("/cli/sew/server/recovery-key", Owner),
    post("/cli/sew/server/restart", Admin),
    post("/cli/sew/server/restore", Admin),
    post("/cli/sew/server/rollback", Admin),
    post("/cli/sew/server/rotate-master", Admin),
    // S32 (K27): off-box backup copies (Manage Sew; `none` is audited).
    post("/cli/sew/server/ship", Admin),
    post("/cli/sew/server/uninstall", Admin),
    post("/cli/sew/server/upgrade", Admin),
    get("/cli/sew/status", Member),
    get("/cli/sew/users", Admin),
    post("/cli/sew/users/invite", Admin),
    post("/cli/sew/users/owner", Admin),
    post("/cli/sew/users/reactivate", Admin),
    post("/cli/sew/users/reset", Admin),
    post("/cli/sew/users/revoke", Admin),
    post("/cli/sew/users/role", Admin),
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
    get("/cli/skin/access", Owner),
    post("/cli/skin/access/devices/revoke", Owner),
    post("/cli/skin/access/invites", Owner),
    post("/cli/skin/access/listed", Owner),
    // app-user-profile P6 (0.46.5): a person's profile, picture and
    // sessions as the owner sees and ends them.
    get("/cli/skin/access/profile", Owner),
    post("/cli/skin/access/profile/image/clear", Owner),
    post("/cli/skin/access/sessions/revoke", Owner),
    post("/cli/skin/access/settings", Owner),
    // 0.46.9: account deletion and client logs, a person's own session pass only.
    post("/cli/skin/account/delete", NoLogin),
    get("/cli/skin/agents", NoLogin),
    get("/cli/skin/app-info", Public),
    // prd-app-caps-0464-v1 AC5 (0.46.4): app.rooms, an App's own pass only.
    get("/cli/skin/app-rooms", NoLogin),
    // 0.46.9 sign-in door: the App's server swaps a door code on its
    // gateway link (the link's App; the handler refuses off a link).
    post("/cli/skin/auth/exchange", Public),
    post("/cli/skin/client-logs", NoLogin),
    get("/cli/skin/devices", NoLogin),
    post("/cli/skin/devices/revoke", NoLogin),
    // Email Notifications (0.46.5): the App's own email.* on its link
    // (the handler checks the pass and email:send).
    get("/cli/skin/email/log", NoLogin),
    post("/cli/skin/email/send", NoLogin),
    get("/cli/skin/email/status", NoLogin),
    both("/cli/skin/front-door", Owner),
    // Sew S27 grant hook: owner-tier like the grants routes; not an agent
    // verb (agents grant chat through K19's /cli/sew/admin/* later).
    get("/cli/skin/grant-hooks", Owner),
    get("/cli/skin/grant-hooks/log", Owner),
    post("/cli/skin/grant-hooks/reconcile", Owner),
    post("/cli/skin/grant-hooks/retry", Owner),
    post("/cli/skin/grant-hooks/skip", Owner),
    post("/cli/skin/grant-hooks/target", Owner),
    both("/cli/skin/grants", Owner),
    post("/cli/skin/grants/copy-host", Owner),
    post("/cli/skin/grants/delete", Owner),
    post("/cli/skin/grants/enabled", Owner),
    post("/cli/skin/grants/host", Owner),
    // An agent's own Apps' logins (the handler admits a passport only).
    get("/cli/skin/grants/own", Member),
    post("/cli/skin/grants/role", Owner),
    post("/cli/skin/grants/update", Owner),
    // Sew for Apps A2 (0.46.6): guests.chat / guests.setChat, an App's own
    // pass under the owner's guests-chat grant (the handler refuses the
    // rest). ("chat" sorts before "list".)
    both("/cli/skin/guests/chat", NoLogin),
    // prd-app-caps-0464-v1 AC5 (0.46.4): guests.list, an App's own pass
    // only (the handler refuses the rest).
    get("/cli/skin/guests/list", NoLogin),
    // RS-A15 (0.46.1): an App's own pass reads/sets its guests' rooms
    // under the owner's grant; no login (or owner token) uses it.
    both("/cli/skin/guests/rooms", NoLogin),
    both("/cli/skin/hydra", Owner),
    post("/cli/skin/login", Public),
    post("/cli/skin/logout", Public),
    // Room-scope R6 (0.46.1): a pass reads its own view; no login uses it.
    get("/cli/skin/me", NoLogin),
    get("/cli/skin/me/events", NoLogin),
    // Room-scope R9/R16 (0.46.1): room.members, app.self, app.usage for a
    // pass (the handler checks rooms, caps and the link).
    get("/cli/skin/members", NoLogin),
    // prd-app-phone-pairing-v1 (0.46.5): the phone redeems and checks a
    // one-time code (Public: the code is the credential, throttled); a
    // person's own web session starts, polls and cancels (the handler
    // checks the pass).
    post("/cli/skin/pair", Public),
    post("/cli/skin/pair/cancel", NoLogin),
    post("/cli/skin/pair/check", Public),
    post("/cli/skin/pair/start", NoLogin),
    get("/cli/skin/pair/status", NoLogin),
    post("/cli/skin/password/change", Public),
    post("/cli/skin/password/forgot", Public),
    post("/cli/skin/password/reset", Public),
    get("/cli/skin/password/set", Public),
    // app-user-profile (0.46.5): a person's own profile and picture (their
    // own session pass; the handler checks).
    both("/cli/skin/profile", NoLogin),
    post("/cli/skin/profile/image", NoLogin),
    post("/cli/skin/profile/image/clear", NoLogin),
    // 0.46.3 App project chat: a pass reads and posts in the projects
    // whose every member room it holds (the handler's all-members rule).
    get("/cli/skin/projects", NoLogin),
    get("/cli/skin/projects/events", NoLogin),
    get("/cli/skin/projects/messages", NoLogin),
    post("/cli/skin/projects/post", NoLogin),
    post("/cli/skin/projects/read", NoLogin),
    // prd-custom-app-push-v1: a guest's own device and the App's link
    // verbs (the handler checks the pass).
    get("/cli/skin/push/device", NoLogin),
    get("/cli/skin/push/devices", NoLogin),
    post("/cli/skin/push/prefs", NoLogin),
    post("/cli/skin/push/register", NoLogin),
    post("/cli/skin/push/send", NoLogin),
    get("/cli/skin/push/status", NoLogin),
    post("/cli/skin/push/unregister", NoLogin),
    post("/cli/skin/push/viewing", NoLogin),
    post("/cli/skin/refresh", Public),
    both("/cli/skin/roles", Owner),
    post("/cli/skin/roles/assign", Owner),
    post("/cli/skin/roles/from-template", Owner),
    post("/cli/skin/roles/paths", Owner),
    post("/cli/skin/roles/remove", Owner),
    post("/cli/skin/roles/room", Owner),
    get("/cli/skin/roles/suggest", Owner),
    post("/cli/skin/roles/unassign", Owner),
    post("/cli/skin/roles/update", Owner),
    get("/cli/skin/self", NoLogin),
    // app-user-profile (0.46.5): a person's own sessions in this App.
    get("/cli/skin/sessions", NoLogin),
    post("/cli/skin/sessions/revoke", NoLogin),
    post("/cli/skin/sessions/revoke-others", NoLogin),
    both("/cli/skin/templates", Owner),
    post("/cli/skin/templates/apply", Owner),
    post("/cli/skin/templates/delete", Owner),
    post("/cli/skin/templates/lines", Owner),
    post("/cli/skin/templates/lines/delete", Owner),
    post("/cli/skin/templates/lines/update", Owner),
    post("/cli/skin/templates/update", Owner),
    get("/cli/skin/usage", NoLogin),
    // 0.46.6 (Rosson 2026-10-29): an Admin lists App users and resets,
    // sets or unlocks App-only users' passwords; box logins stay the
    // owner's (`skin_routes::owner_or_admin_for`). Adding stays Owner.
    split("/cli/skin/users", Admin, Owner),
    post("/cli/skin/users/agent", Owner),
    post("/cli/skin/users/email", Owner),
    post("/cli/skin/users/full-name", Owner),
    post("/cli/skin/users/invite", Owner),
    post("/cli/skin/users/password", Admin),
    post("/cli/skin/users/remove", Owner),
    post("/cli/skin/users/reset-link", Admin),
    post("/cli/skin/users/rooms", Owner),
    // 0.46.8: `k2 app user sessions`; an App's own agent too (its own gate).
    get("/cli/skin/users/sessions", Owner),
    post("/cli/skin/users/sessions/revoke", Owner),
    post("/cli/skin/users/unlock", Admin),
    // App users' personal wallets (0.46.3): an App user's OWN session
    // pass only (`person_pass_required` for anything else); no Connect
    // login (Rosson #9: Connect members have no wallet).
    post("/cli/skin/wallet/add", NoLogin),
    get("/cli/skin/wallet/list", NoLogin),
    post("/cli/skin/wallet/login", NoLogin),
    post("/cli/skin/wallet/login/cancel", NoLogin),
    post("/cli/skin/wallet/login/input", NoLogin),
    get("/cli/skin/wallet/login/status", NoLogin),
    post("/cli/skin/wallet/remove", NoLogin),
    get("/cli/skin/wallet/status", NoLogin),
    post("/cli/skin/wallet/use", NoLogin),
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
    // K2 vault (prd-k2-vault-v1 §22.3, KV14). Passports reach the Member
    // rows through is_agent_verb and are checked in the handler; confirm,
    // reveal, recovery and key operations are never agent verbs.
    both("/cli/vault/accept-from", Member),
    post("/cli/vault/agent-policy", Member),
    post("/cli/vault/ask", Member),
    get("/cli/vault/asks", Member),
    get("/cli/vault/audit", Admin),
    post("/cli/vault/backup", Owner),
    post("/cli/vault/backup/recipient", Owner),
    post("/cli/vault/carry/export", Admin),
    post("/cli/vault/carry/import", Admin),
    post("/cli/vault/confirm-password/set", Owner),
    post("/cli/vault/confirm/finish", Admin),
    post("/cli/vault/confirm/start", Admin),
    post("/cli/vault/destroy", Member),
    post("/cli/vault/disable", Member),
    get("/cli/vault/doctor", Member),
    post("/cli/vault/enable", Member),
    post("/cli/vault/env-file", Member),
    post("/cli/vault/fetch", Member),
    post("/cli/vault/file", Member),
    post("/cli/vault/get", Member),
    post("/cli/vault/grant", Member),
    post("/cli/vault/import", Member),
    get("/cli/vault/incoming", Member),
    post("/cli/vault/incoming/decide", Member),
    post("/cli/vault/kek/rotate", Owner),
    get("/cli/vault/list", Member),
    // prd-vault-gaps-v1 item 4: manual placement records.
    // Open a vault that failed to open at boot (may prompt once).
    post("/cli/vault/open", Admin),
    post("/cli/vault/placement/add", Member),
    post("/cli/vault/placement/done", Member),
    post("/cli/vault/placement/remove", Member),
    get("/cli/vault/placements", Member),
    post("/cli/vault/policy", Member),
    get("/cli/vault/presets", Member),
    post("/cli/vault/proxy-url", Member),
    get("/cli/vault/public-key", Member),
    get("/cli/vault/recipient", Admin),
    post("/cli/vault/recovery/confirm", Owner),
    post("/cli/vault/recovery/rotate", Owner),
    post("/cli/vault/recovery/show-once", Owner),
    post("/cli/vault/rekey", Owner),
    get("/cli/vault/requests", Member),
    post("/cli/vault/restore", Owner),
    post("/cli/vault/reveal", Admin),
    post("/cli/vault/revoke", Member),
    post("/cli/vault/rollback", Owner),
    post("/cli/vault/rotate", Member),
    // `k2 vault run`: the handler serves agent passports only.
    post("/cli/vault/run", Member),
    get("/cli/vault/run-helper", Member),
    post("/cli/vault/send", Member),
    get("/cli/vault/sent", Member),
    both("/cli/vault/service", Member),
    post("/cli/vault/set", Member),
    get("/cli/vault/show", Member),
    get("/cli/vault/status", Member),
    post("/cli/vault/sync", Member),
    // prd-vault-gaps-v1 item 4: a root unit's credential is Owner/Admin +
    // a vault confirm (passports refused in the handler, KV14 DENY);
    // (manual placement records, above, are bookkeeping: agents keep
    // their own workspace's).
    post("/cli/vault/systemd-cred", Admin),
    post("/cli/vault/systemd-cred/remove", Admin),
    post("/cli/vault/unsend", Member),
    // Voice rooms (prd-k2-voice-rooms-v1 §9.1, §9.4; preview, dark while
    // off): join, the call's own routes and a recording consent are any
    // login's (the handler refuses the bare owner token and passports);
    // moderation, the count, renames and closing are Owner/Admin; status is
    // Owner/Admin (agents read it with a passport, which this table never gates).
    get("/cli/voice/events", VoiceChat),
    post("/cli/voice/join", VoiceChat),
    post("/cli/voice/moderate", Admin),
    post("/cli/voice/recording/consent", VoiceChat),
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
    // Zen Mode (prd-zen-mode-v1 Z15, vs-live Z59/Z62; prd-zen-gardens-v1
    // G13/G45): Member floor so a Member's own desktop works; every handler
    // then requires the LOCAL owner token and answers 403 `zen_local_only`
    // to anything else. `page/ensure` and `homes/sync` are gone (G13).
    // Zen v2 (prd-zen-user-widgets-v2 UW35, UWB6, UWB15, UWB23): the widget
    // and grant rows are Member too; the grant-giving ones take the owner
    // token only (403 `owner_only`, UWB3). `lib/*` is B3's zen_lib_routes.
    // The person's order and pins in a Garden's Agents widget (0.46.4):
    // the read takes the owner token or a local passport (agents read it),
    // the set the owner token only (zen_agents_order_routes).
    get("/cli/zen/agents/order", Member),
    post("/cli/zen/agents/order/set", Member),
    // Garden data (prd-gardens-local-services-v1 1A, contract-garden-data-v1
    // §4.4): this computer only, so no Connect login ever (NoLogin). The
    // owner token and agent passports reach the handlers, which apply
    // grants on every call; `/cli/zen/call` takes the owner token only.
    get("/cli/zen/blob/get", NoLogin),
    post("/cli/zen/blob/ingest", NoLogin),
    post("/cli/zen/blob/put", NoLogin),
    post("/cli/zen/call", NoLogin),
    post("/cli/zen/data/admin", NoLogin),
    post("/cli/zen/data/admin/revoke", NoLogin),
    post("/cli/zen/data/batch", NoLogin),
    get("/cli/zen/data/blob/get", NoLogin),
    get("/cli/zen/data/blob/list", NoLogin),
    post("/cli/zen/data/blob/put", NoLogin),
    post("/cli/zen/data/board", NoLogin),
    get("/cli/zen/data/changes", NoLogin),
    post("/cli/zen/data/delete", NoLogin),
    get("/cli/zen/data/export", NoLogin),
    get("/cli/zen/data/get", NoLogin),
    post("/cli/zen/data/grant", NoLogin),
    get("/cli/zen/data/grants", NoLogin),
    // prd-garden-board-needs-v1 §6 (garden-history-v1).
    get("/cli/zen/data/history", NoLogin),
    post("/cli/zen/data/import", NoLogin),
    get("/cli/zen/data/list", NoLogin),
    post("/cli/zen/data/patch", NoLogin),
    post("/cli/zen/data/put", NoLogin),
    post("/cli/zen/data/restore", NoLogin),
    post("/cli/zen/data/revoke", NoLogin),
    get("/cli/zen/data/show", NoLogin),
    post("/cli/zen/data/snapshot", NoLogin),
    post("/cli/zen/data/space/delete", NoLogin),
    post("/cli/zen/data/space/move", NoLogin),
    get("/cli/zen/data/spaces", NoLogin),
    post("/cli/zen/data/tx/abort", NoLogin),
    post("/cli/zen/data/tx/add", NoLogin),
    post("/cli/zen/data/tx/begin", NoLogin),
    post("/cli/zen/data/tx/commit", NoLogin),
    get("/cli/zen/doctor", Member),
    // `k2.download` consent (0.46.5): the widgets a Garden lets save files
    // without asking. The owner token only (zen_downloads_routes).
    get("/cli/zen/downloads", Member),
    post("/cli/zen/downloads/allow", Member),
    post("/cli/zen/downloads/forget", Member),
    post("/cli/zen/downloads/spend", Member),
    // Garden access (prd-garden-access-v1 G14), live only with the
    // `gardenAccess` switch on (`live_row`): GET the list (the person; a
    // passport learns whether it is listed), POST a change (the person);
    // the ask (a live local passport); History (the person).
    both("/cli/zen/garden/access", Member),
    post("/cli/zen/garden/access/ask", Member),
    // A staged Garden page checked, never written (0.46.1 S3).
    post("/cli/zen/garden/check", Member),
    post("/cli/zen/garden/delete", Member),
    get("/cli/zen/garden/history", Member),
    // Garden Lock (prd-garden-lock-and-sync-v1 §5.2): the person only; the
    // handler refuses every passport, login and app pass (403 owner_only).
    post("/cli/zen/garden/lock", Member),
    post("/cli/zen/garden/new", Member),
    // Agents run Gardens end to end (0181): a person or an agent with
    // "Manage Gardens and data sources" writes; any other local agent
    // proposes (zen_agent_routes).
    post("/cli/zen/garden/put", Member),
    post("/cli/zen/garden/rename", Member),
    post("/cli/zen/garden/reorder", Member),
    // Garden access (live only with the switch on): Restore a deleted
    // Garden (the person).
    post("/cli/zen/garden/restore", Member),
    post("/cli/zen/garden/sync", Member),
    post("/cli/zen/garden/template", Member),
    get("/cli/zen/gardens", Member),
    // Garden access (live only with the switch on): deleted Gardens.
    get("/cli/zen/gardens/deleted", Member),
    get("/cli/zen/get", Member),
    get("/cli/zen/history", Member),
    // Static review of a received Garden bundle or folder (Rosson
    // 2026-10-11: importing a Garden is an agent workflow). Reads only.
    get("/cli/zen/inspect", Member),
    post("/cli/zen/lib/fetch", Member),
    get("/cli/zen/lib/file", Member),
    // `k2.open` (0.46.4): the sites a Garden opens without asking. Reads
    // for the owner token and local passports; writes the owner token only
    // (zen_links_routes).
    get("/cli/zen/links", Member),
    post("/cli/zen/links/allow", Member),
    post("/cli/zen/links/forget", Member),
    get("/cli/zen/news", Member),
    post("/cli/zen/news/seen", Member),
    get("/cli/zen/proposals", Member),
    post("/cli/zen/proposals/approve", Member),
    post("/cli/zen/proposals/reject", Member),
    get("/cli/zen/proposals/show", Member),
    post("/cli/zen/reload", Member),
    // Gardens on the phone (prd-gardens-on-phone-v1 §3.2): the remote
    // Garden door. Member so role_gate lets every credential through; the
    // handler's gate takes only the owner token or a paired phone of the
    // owner person (zen_remote_routes::gate).
    get("/cli/zen/remote/agents-order", Member),
    get("/cli/zen/remote/blob/get", VoiceChat),
    post("/cli/zen/remote/blob/put", Member),
    post("/cli/zen/remote/call", VoiceChat),
    get("/cli/zen/remote/frame", VoiceChat),
    get("/cli/zen/remote/gardens", VoiceChat),
    get("/cli/zen/remote/lib", VoiceChat),
    post("/cli/zen/remote/lib/fetch", VoiceChat),
    get("/cli/zen/remote/page", VoiceChat),
    post("/cli/zen/remote/pause", Member),
    post("/cli/zen/remote/report", Member),
    post("/cli/zen/remote/resume", Member),
    get("/cli/zen/remote/sources-changes", VoiceChat),
    post("/cli/zen/reset", Member),
    post("/cli/zen/setup", Member),
    // Garden access (live only with the switch on): View this version.
    get("/cli/zen/snapshot", Member),
    get("/cli/zen/status", Member),
    get("/cli/zen/sync", Member),
    get("/cli/zen/templates", Member),
    get("/cli/zen/theme/list", Member),
    post("/cli/zen/theme/new", Member),
    post("/cli/zen/theme/next", Member),
    post("/cli/zen/theme/prev", Member),
    post("/cli/zen/theme/set", Member),
    get("/cli/zen/validate", Member),
    get("/cli/zen/widget/bundle", Member),
    // 0.46.1: the app checks a widget's approved cap before a post to
    // another server (owner token only, checked in zen_routes).
    post("/cli/zen/widget/check", Member),
    // Authorship (contract-garden-data-v1 §4.3, BG3): claim takes the
    // owner token only; sign takes it or a local agent/shell passport.
    post("/cli/zen/widget/claim", Member),
    // 0.46.5: an unplaced widget folder into history (k2 zen reset undoes).
    post("/cli/zen/widget/delete", Member),
    get("/cli/zen/widget/errors", Member),
    post("/cli/zen/widget/errors/clear", Member),
    // The widget dev loop (0.46.1 S3): a staged folder checked, written
    // (or proposed) whole, or previewed on K2's own preview Garden.
    post("/cli/zen/widget/folder/check", Member),
    post("/cli/zen/widget/folder/put", Member),
    get("/cli/zen/widget/history", Member),
    post("/cli/zen/widget/new", Member),
    post("/cli/zen/widget/pause", Member),
    post("/cli/zen/widget/preview", Member),
    post("/cli/zen/widget/put", Member),
    post("/cli/zen/widget/resume", Member),
    post("/cli/zen/widget/run", Member),
    // 0.46.3: a staged folder's page for a headless shot (writes nothing).
    post("/cli/zen/widget/shot", Member),
    get("/cli/zen/widget/show", Member),
    post("/cli/zen/widget/sign", Member),
    get("/cli/zen/widgets", Member),
];

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
    ("/cli/heartbeat/active-session", "clears a dead active terminal id / records a live one"),
    ("/cli/host-sessions/list", "drops dead host-session PTYs from the session map"),
    ("/cli/mail/approvals/list", "auto-denies sends pending longer than 7 days"),
    ("/cli/mail/approvals/show", "auto-denies sends pending longer than 7 days"),
    ("/cli/mail/outbox", "auto-denies sends pending longer than 7 days"),
    ("/cli/mail/read", "reading a message marks it \\Seen (IMAP semantics)"),
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
