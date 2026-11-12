//! Skin capability passes (prd-skin-auth-v1 + skin-identity-reshape-v1).
//!
//! Two `k2skn_…` classes, one prefix: guest **login session** (`session=1`,
//! on a principal) and **platform** token (`session=0`, no principal, `name`
//! is a label). **Not** Connect users, **not** `/v1` `k2sk_` API keys, **not**
//! the owner token. Overlay Thread rooms only — grid/PTY is never a skin room.
//! Optional `password_hash` on principals is K2-login only
//! (`POST /cli/skin/login`); NULL = cannot K2-login.
//!
//! ## Store (`~/.k2/skin.db`, WAL, own Mutex)
//! Tables: `principals` (guest roster), `roles` (named caps+rooms bundles;
//! nullable `app_id`, NULL = host-wide), `grants` (subject × kind × target;
//! `enabled` 0 is no grant; `target_id` `host` is this box's skin login),
//! `access_templates` + `access_template_lines` (kind and optional role,
//! no target — apply writes real grants),
//! and `tokens` (hashed passes). The raw secret is returned
//! **once** at mint and never stored. Lookup is hex SHA-256 of the
//! presented key (same construction as API keys / connect-user session
//! tokens — high-entropy CSPRNG, not argon2).
//!
//! Caps: `thread:read`, `thread:post`, `files:read`, `files:write`,
//! `tickets:read`, `tickets:post`, `wiki:read`, `store:read`, `store:write`,
//! `activity:read`, plus the opt-in `files:agentconfig` modifier (lets
//! `files:write` reach the agent's `.k2/` folder; off by default, owner
//! only). Empty/missing caps stay Thread-only — never silent-add
//! files, tickets, wiki, store, or activity. Never `pty:*`. `store:write`
//! is dump insert/update/delete only — it does not grant `_k2_store` PUT.
//! `files:read` and `files:write` do not imply `activity:read`.
//! Assigned guests snapshot the role onto `session=1`; platform tokens
//! keep their own caps+rooms (not a role). Session policy is **per-room**
//! (`room_policy` map). Platform `--name` tokens stay flat.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Prefix every skin pass carries. Distinct from `k2sk_` (`/v1` API keys).
pub const SKIN_KEY_PREFIX: &str = "k2skn_";

/// The box login: the leftover direct door (`k2_skin_session`), a BFF that
/// names no App, and the "host" row in Settings → Apps. Every other login
/// target is a published service id (S24, 0.45.5): a login for App X needs
/// an enabled `kind=app` grant whose `target_id` is X, and its pass is
/// bound to X ([`create_app_session_token`]).
pub const HOST_APP_ID: &str = "host";

/// [`create_app_session_token`] error when the principal has no enabled
/// `kind=app` grant for the App asked for (or the App is unknown). The
/// login route returns this text as the same 401 body as a bad password.
/// No username, no App and no grant detail (no App enumeration).
pub const HOST_LOGIN_DENIED: &str = "invalid username or password";

/// skin.db schema version this binary knows (`PRAGMA user_version`, S24-1).
/// Steps are additive only: a client's BFF reads `principals` and
/// `login_lockouts` directly, so no step renames, drops or rebuilds.
///   1 = AM1 sync: the host grant is the login truth (role or rooms).
///   2 = `tokens.app_service_id` (+ index) and `grants.created_by`.
///   3 = mark the one-time `--skin` App backfill pending (run at boot).
///   4 = `tokens.link_id` (+ index): App link passes (runtime S14, 0.46.0,
///       [`link`]). A K2 older than this refuses the file, so a downgrade
///       can never accept a link pass on its main port.
///   5 = `roles.room_paths`: folder path scope per room line (room-scope
///       R3, RS-A1..A3, 0.46.1, [`paths`]). `{}` = every line unscoped.
///   6 = `principal_agent_ties` + `app_user_invites` (agent-app-users,
///       0.46.4, [`ties`]). A K2 older than this refuses the file, so a
///       downgrade can never ignore a tie and mint an uncapped session.
///   7 = `principals.password_must_change` + `password_change_tokens`
///       (must-change at next sign-in, 0.46.6, [`must_change`]). A K2
///       older than this refuses the file, so a downgrade can never sign a
///       person in on a temporary password someone else chose.
///   8 = `login_lockouts.last_failed_at` (0.46.6, App guest login
///       lockout fix 1c): a failure count older than
///       [`LOCKOUT_FAIL_WINDOW_SECS`] is forgotten.
pub const SKIN_SCHEMA_VERSION: i64 = 8;

/// Agent ties and the tied user's ceiling (agent-app-users, 0.46.4).
#[path = "skin_ties.rs"]
pub mod ties;

/// Live access: the update hook, the generation, the listener (0.46.8).
#[path = "skin_access_live.rs"]
pub mod access_live;
/// One-time set-password links: invites, App resets, owner resets
/// (agent-app-users, 0.46.4).
#[path = "skin_invites.rs"]
pub mod invites;
/// Must-change-password at next sign-in (0.46.6): the flag and the
/// one-time change token a flagged login gets instead of a pass.
#[path = "skin_must_change.rs"]
pub mod must_change;
/// The sign-in door's skin.db half (prd-app-hosted-sign-in-v1, 0.46.9):
/// per-App door settings, token targets, the dry run and the mint a
/// one-time code turns into.
#[path = "skin_auth_door.rs"]
pub mod auth_door;

#[path = "skin_link.rs"]
pub mod link;

/// RS-A15 `guests:rooms` (0.46.1): an App sets its own guests' rooms.
#[path = "skin_guest_rooms.rs"]
pub mod guest_rooms;
/// Sew for Apps A2 (0.46.6): `guests:chat`, an App gives its own guests
/// chat under the owner's grant.
#[path = "skin_app_chat.rs"]
pub mod app_chat;
/// App-governed Spaces S1 (0.46.7): an App's chat key, the owner's grant
/// that lets K2 sign chat passes for that App's users up to a ceiling.
#[path = "skin_app_chat_key.rs"]
pub mod app_chat_key;
/// Sew for Apps A9 (0.46.6): `chat.createChannel`, an App makes a text
/// channel in its own bound Sew Space (Sew 0.5.0, flag
/// `app_channel_create`).
#[path = "skin_app_channels.rs"]
pub mod app_channels;
#[path = "skin_members.rs"]
pub mod members;
/// Person wallets (0.46.3): an App user's live access, read-only.
#[path = "skin_wallet_access.rs"]
pub mod wallet_access;
#[path = "skin_paths.rs"]
pub mod paths;
/// Device sign-in (refresh tokens) and per-App sign-in settings
/// (app-mobile-access-v1, 0.46.3).
#[path = "skin_app_login.rs"]
pub mod app_login;
/// Test guests of one App, short-lived (0.46.4).
#[path = "skin_test_guests.rs"]
pub mod test_guests;
/// Which workspace's agent added a guest (0.46.4 security review).
#[path = "skin_creators.rs"]
pub mod creators;
/// The agent access list, the owner's person × agent grid and the
/// suggested role templates (app-mobile-access-v1, 0.46.3).
#[path = "skin_access.rs"]
pub mod access;
/// Push notifications for custom Apps: the device list and per-App push
/// settings (prd-custom-app-push-v1, 0.46.4).
#[path = "skin_app_push.rs"]
pub mod app_push;
/// Phone pairing for custom Apps: one-time QR codes from a person's web
/// session, the owner's phone-app config and the pairing rows
/// (prd-app-phone-pairing-v1, 0.46.5).
#[path = "skin_app_pairing.rs"]
pub mod app_pairing;
/// In-memory guards on the password doors: hash slots, wrong current
/// passwords, forgot throttles, reset mail budgets (app-user-profile P0).
#[path = "skin_guard.rs"]
pub mod guard;
/// App user profile: per-App profile settings, names and picture, the
/// session list (prd-app-user-profile-v1, 0.46.5).
#[path = "skin_profile.rs"]
pub mod profile;
/// The skin.db half of an App handover (prd-app-handoff-v1, 0.46.5).
#[path = "skin_handover.rs"]
pub mod handover;
// App account deletion (0.46.9, app-account-deletion-v1): per-App mode and
// grace period, pending requests.
#[path = "skin_account_deletion.rs"]
pub mod account_deletion;
pub use paths::{RoomPathScope, RoomPaths};

/// `grants.created_by` on rows the 0.45.5 upgrade wrote (S24-10 backfill).
pub const UPGRADE_CREATED_BY: &str = "upgrade:0.45.5";

/// Sew S27/S28: the `chat` grant kind. A principal with an enabled
/// `kind=chat` grant on [`CHAT_GRANT_TARGET`] may mint chat passes
/// (`POST /cli/sew/pass`) and may log in to the box with only that grant
/// (S28 §13, Thread dark). **S27 owns writing these grants** (the grant
/// hook, [`crate::tool_hook`]); S28 only reads them.
pub const CHAT_GRANT_KIND: &str = crate::tool_hook::CHAT_KIND;
/// The one Sew install per box (S27: `target_id` is always `sew`). Every
/// App on the box (Hearth, Meet, …) reaches that same install, so a chat
/// grant is per person, never per App: the App a pass is for comes from
/// the login session (K-M4).
pub const CHAT_GRANT_TARGET: &str = crate::tool_hook::CHAT_TARGET_ID;

/// Q8 default (S24-6): switching a guest's box login off ends EVERY login
/// session of that guest, App sessions included — it is the one "turn this
/// person off" switch in User Access. `false` = only host sessions end.
pub const HOST_OFF_ENDS_ALL_APP_SESSIONS: bool = true;

/// Overlay Thread read (GET `/cli/thread`, WS `/cli/overlay/events`).
pub const CAP_THREAD_READ: &str = "thread:read";
/// Overlay Thread post (`POST /cli/thread/post`).
pub const CAP_THREAD_POST: &str = "thread:post";
/// Workspace files read (GET `/cli/fs/read-dir`, `/cli/fs/read-file`, WS `/cli/fs/events`).
pub const CAP_FILES_READ: &str = "files:read";
/// Workspace files write (`POST /cli/fs/write-file`). Does not imply read.
pub const CAP_FILES_WRITE: &str = "files:write";
/// Tickets read (`GET /cli/feedback/list`, `GET /cli/feedback/show`).
pub const CAP_TICKETS_READ: &str = "tickets:read";
/// Tickets write (`POST /cli/feedback/create|comment|answer|resolve`).
pub const CAP_TICKETS_POST: &str = "tickets:post";
/// Wiki read (`GET /cli/wiki/index`, `GET /cli/wiki/note`). Does not imply write.
pub const CAP_WIKI_READ: &str = "wiki:read";
/// Workspace dump store read (`GET /cli/store/list|get|query` and dump SELECT).
/// Does not imply write.
pub const CAP_STORE_READ: &str = "store:read";
/// Dump-table insert/update/delete (`POST /cli/db/rows|rows/update|rows/delete`).
/// Does not imply `_k2_store` PUT/create/rm/drop.
pub const CAP_STORE_WRITE: &str = "store:write";
/// Agent working mark (`WS /cli/activity/events`). Not implied by files.
pub const CAP_ACTIVITY_READ: &str = "activity:read";
/// Room heartbeats read (`GET /cli/heartbeat/list|show|status|fires-list`,
/// `heartbeat_changed` frames on `WS /cli/activity/events`). The WAKEUP.md
/// body also needs `files:read` (prd-app-heartbeats-surface-v1 AH9).
pub const CAP_HEARTBEATS_READ: &str = "heartbeats:read";
/// Room heartbeats write (`POST /cli/heartbeat/add|edit|enable|rename|
/// archive|fire`) and exactly each heartbeat's WAKEUP.md through
/// `/cli/fs/write-file` (Rosson R2). Does not imply read.
pub const CAP_HEARTBEATS_WRITE: &str = "heartbeats:write";
/// The room agent's name, color and picture (`GET|POST /cli/agent/identity`).
/// Not the handle: an address change breaks links, so it is its own cap.
pub const CAP_AGENT_IDENTITY: &str = "agent:identity";
/// The room agent's handle, its address (`POST /cli/agent/handle`). Does not
/// imply `agent:identity`, and `agent:identity` does not imply it.
pub const CAP_AGENT_HANDLE: &str = "agent:handle";
/// Opt-in modifier of `files:write` (GH#72, Rosson 2026-10-14): also let
/// that room's `files:write` reach the agent's own config folder `.k2/`.
/// Off by default — without it every guest write route refuses a target
/// in `.k2/` with 403 `agent_config_protected`. Means nothing without
/// `files:write` in the same room ([`parse_caps`] refuses it alone), never
/// opens `.k2/heartbeats/` (heartbeats:write rules stay), and reads are
/// unaffected. The owner's on box-wide roles and platform passes; the agent
/// that owns an App may put it on that App's roles (Rosson 2026-11-03), held back
/// by the tie until the person chooses their own password.
pub const CAP_FILES_AGENT_CONFIG: &str = "files:agentconfig";
/// The room access list (room-scope R9, 0.46.1): `GET /cli/skin/members`
/// (`room.members`), the guests and agent logins that hold a grant on the
/// room, and `room_members_changed` frames on `WS /cli/activity/events`.
/// Never in the defaults; meant for staff roles. Names can be hidden from
/// guests per room by the owner.
pub const CAP_ROOM_MEMBERS: &str = "room:members";
/// Post in a project chat as the guest (0.46.3, App project chat). Held per
/// room like every cap: a pass may post in a project's chat only when it
/// holds this cap in EVERY member room of that project (the all-members
/// rule; reading needs `thread:read` in every member room). Never in the
/// defaults; owner/role-granted. In no App core (the person approves it)
/// until a Projects core exists.
pub const CAP_PROJECT_WRITE: &str = "project:write";
/// Save a Thread attachment as a download (0.46.3, Rosson 2026-10-22: "if
/// they have threads available that doesn't mean they can download files
/// unless they have file downloads turned on"). Held per room. It gates
/// only the explicit save, `GET /cli/thread/attachment?mode=download`, and
/// the `canDownload` flag each attachment carries for the viewer; opening a
/// file (meta, preview, thumb, range) stays `thread:read` plus the guest's
/// own `files:read`, as before. Never in the defaults, never implied by
/// `thread:read` or `files:read`, in no App core. Who may grant it is the same
/// rule as [`CAP_FILES_AGENT_CONFIG`].
pub const CAP_FILES_DOWNLOAD: &str = "files:download";
/// Upload-only files (prd-app-caps-0464-v1 AC7, 0.46.4): put files in the
/// room line's drop folders (`upload`, [`paths::RoomPathScope::upload`])
/// through the two upload doors, each landing in the uploader's own
/// subfolder. Held per room. It never implies `files:read` or
/// `files:write`, neither implies it, and it is never a default and in no
/// App core. Strictly narrower than `files:write`, so an agent with
/// "manage Apps" may grant it (it is not in [`PERSON_ONLY_CAPS`]).
pub const CAP_FILES_UPLOAD: &str = "files:upload";
/// Send notifications to this App's guests' phones (prd-custom-app-push-v1
/// CP37, 0.46.4): the App's own cap, never a guest's (no role holds it:
/// it isn't in the accepted role caps). In no App core
/// (`app_core_of_cap` answers `None`), so a deploy that adds it is held for
/// the owner's one-time approval, as App mail sends are (N4).
pub const CAP_PUSH_SEND: &str = "push:send";
/// Send email from this App's own sender (prd-email-notifications-core-v1
/// §3.5, E24, 0.46.5): the App's own cap, never a guest's. In no App core
/// (`app_core_of_cap` answers `None`; the Mail tick doesn't pre-approve it,
/// Q13), so a deploy that adds it is held for the owner once. Approving it
/// grants no sender: the owner does that with `k2 app email on`.
pub const CAP_EMAIL_SEND: &str = "email:send";
/// Sew for Apps A1 (K9, 0.46.6): the chat caps an App role carries into a
/// person's Sew chat pass, as its `caps` ceiling (Sew SW3). They are
/// App-wide, not per room: a role holds one when ANY of its room lines
/// lists it (the session snapshot's union), and a pass carries the role's
/// chat caps narrowed by the App's own manifest ceiling
/// (`chat_pass::identity`). A ceiling never grants: Sew's roles inside a
/// Space still decide. Never defaults, never implied by a Thread cap.
///
/// Read the App's Spaces (every present ceiling holds it: Sew SW3).
pub const CAP_CHAT_READ: &str = "chat:read";
/// Post, react and upload in the App's Spaces.
pub const CAP_CHAT_POST: &str = "chat:post";
/// Answer cards (reserved in Sew).
pub const CAP_CHAT_RESPOND: &str = "chat:respond";
/// Join voice channels and DM calls (Rosson 2026-10-28: App users may use
/// voice and calls). K2 never mints a media token; Sew does.
pub const CAP_VOICE_JOIN: &str = "voice:join";
/// Camera and screen-share leases in a call (A12, Rosson yes 2026-10-28).
/// Means nothing without [`CAP_VOICE_JOIN`] ([`parse_caps`] refuses it
/// alone). A Sew older than 0.4.1 ignores it (an unknown, well-formed cap
/// grants nothing there), so its roles alone decide video.
pub const CAP_VOICE_VIDEO: &str = "voice:video";
/// Start and stop recordings in a call (Sew 0.9.0, PassCap::VoiceRecord).
/// A ceiling only: it never grants Sew's RECORD_CALLS, and each person in
/// the call still consents for themselves. Means nothing without
/// [`CAP_VOICE_JOIN`] (you record a call you are in; [`parse_caps`] refuses
/// it alone). Accepted in roles and manifests only once
/// [`crate::recording::in_apps`] (the Sew 0.9.0 pin); a pass carries it
/// only to a Sew that reports `voice_record_cap` (the mint drops it
/// otherwise: an older Sew refuses a pass with an unknown cap).
pub const CAP_VOICE_RECORD: &str = "voice:record";
/// The chat caps, in the order a pass lists them.
pub const CHAT_CAPS: &[&str] = &[CAP_CHAT_READ, CAP_CHAT_POST, CAP_CHAT_RESPOND, CAP_VOICE_JOIN, CAP_VOICE_VIDEO, CAP_VOICE_RECORD];

/// Is `cap` one of [`CHAT_CAPS`]?
pub fn is_chat_cap(cap: &str) -> bool {
    CHAT_CAPS.contains(&cap.trim())
}

/// Sew for Apps A9 (0.46.6): make text channels in the App's own bound
/// Sew Space and add its users to them (`chat.createChannel`). The App's
/// own cap, never a guest's (no role holds it; it is not one of
/// [`CHAT_CAPS`], so no person's chat pass ever carries it). In no App
/// core (`app_core_of_cap` answers `None`; the Sew core pre-approves only
/// the chat caps), so a deploy that adds it is held for the owner once:
/// a channel can be private and pulls people into it, which `chat:post`
/// never could.
pub const CAP_CHAT_CHANNELS: &str = "chat:channels";

/// Caps an App's own pass may carry that no guest role ever holds (they
/// aren't in the accepted role caps): the link's ceiling adds them
/// ([`link::link_caps`]).
pub const APP_ONLY_CAPS: &[&str] = &[CAP_PUSH_SEND, CAP_EMAIL_SEND, CAP_CHAT_CHANNELS];
/// Caps a tied login never holds ([`ties::clamp`]) and an agent never puts on a
/// platform pass. A role of an agent's own App may name them (Rosson 2026-11-03).
pub const PERSON_ONLY_CAPS: &[&str] = &[CAP_FILES_AGENT_CONFIG, CAP_FILES_DOWNLOAD];

const KEY_BODY_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Empty/missing `--caps` stay Thread-only. Never silent-add files, tickets, wiki, store, or activity.
const DEFAULT_CAPS: &[&str] = &[CAP_THREAD_READ, CAP_THREAD_POST];
const ACCEPTED_CAPS: &[&str] = &[
    CAP_THREAD_READ,
    CAP_THREAD_POST,
    CAP_FILES_READ,
    CAP_FILES_WRITE,
    CAP_TICKETS_READ,
    CAP_TICKETS_POST,
    CAP_WIKI_READ,
    CAP_STORE_READ,
    CAP_STORE_WRITE,
    CAP_ACTIVITY_READ,
    CAP_HEARTBEATS_READ,
    CAP_HEARTBEATS_WRITE,
    CAP_AGENT_IDENTITY,
    CAP_AGENT_HANDLE,
    CAP_FILES_AGENT_CONFIG,
    CAP_ROOM_MEMBERS,
    CAP_PROJECT_WRITE,
    CAP_FILES_DOWNLOAD,
    CAP_FILES_UPLOAD,
    CAP_CHAT_READ,
    CAP_CHAT_POST,
    CAP_CHAT_RESPOND,
    CAP_VOICE_JOIN,
    CAP_VOICE_VIDEO,
    CAP_VOICE_RECORD,
];

/// Connect / Server Access names — never a skin role. `viewer` stays
/// reserved after the Connect Viewer role was removed
/// (`prd-remove-viewer-role-v1.md`), so the retired word can't come back
/// as an app role and be read as the old view-only login.
const CONNECT_ROLE_NAMES: &[&str] = &["owner", "admin", "member", "viewer"];
const CONNECT_ROLE_NAME_ERR: &str = "Connect role names cannot be skin roles (owner/admin/member/viewer). Skin roles are named bundles of scopes+agents, not Server Access.";

fn accepted_caps_csv() -> String {
    ACCEPTED_CAPS.join(", ")
}

/// The refusal of `voice:record` before the Sew 0.9.0 pin
/// ([`crate::recording::in_apps`] off): an unknown capability, as before.
pub fn record_cap_not_yet() -> String {
    let now: Vec<&str> = ACCEPTED_CAPS.iter().copied().filter(|c| *c != CAP_VOICE_RECORD).collect();
    format!(
        "unknown capability \"{CAP_VOICE_RECORD}\" on this K2 (recording in Apps comes with K2's Sew 0.9.0 release); accepted: {}",
        now.join(", ")
    )
}

fn is_connect_role_name(name: &str) -> bool {
    CONNECT_ROLE_NAMES
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
}

fn assigned_guest_rooms_err(username: &str, role_name: &str) -> String {
    format!("guest '{username}' has role '{role_name}'; edit the role or unassign")
}

fn assigned_role_remove_err(name: &str) -> String {
    format!("role '{name}' is assigned; unassign guests first")
}

fn grant_role_remove_err(name: &str) -> String {
    format!("role '{name}' is used by a grant; revoke the grant first")
}

/// Copy of Connect: 3 consecutive failures → 15-minute per-username lockout.
/// Dummy argon2 + the 500 ms 401 delay is **not** lockout.
const LOCKOUT_THRESHOLD: u32 = 3;
const LOCKOUT_DURATION_SECS: i64 = 15 * 60;
/// 0.46.6 (App guest login lockout, fix 1c): failures count toward the
/// lock only while they are recent. A failure more than this long after
/// the last one starts the count again at 1, so stale failures (an old
/// test run, a typo last week) can't add up to a lock days later. A row
/// written before schema step 8 has no time and starts again too.
pub const LOCKOUT_FAIL_WINDOW_SECS: i64 = 15 * 60;

/// Public consume/change min length (Connect default). Owner `users/password`
/// empty still clears via [`set_principal_password`].
pub const PUBLIC_PASSWORD_MIN_LEN: usize = 8;
/// Longest password any door takes, in bytes (an argon2 input bound;
/// app-user-profile §6.1). Login is not capped (a longer password the
/// owner set before this still signs in).
pub const MAX_PASSWORD_BYTES: usize = 256;
/// The refusal text for a password over [`MAX_PASSWORD_BYTES`].
pub const PASSWORD_TOO_LONG: &str = "password must be at most 256 bytes";

/// True when `password` is over [`MAX_PASSWORD_BYTES`].
pub fn password_too_long(password: &str) -> bool {
    password.len() > MAX_PASSWORD_BYTES
}
const RESET_TTL_SECS: i64 = 60 * 60;
const RESET_THROTTLE_SECS: i64 = 60;

/// True iff `raw` is a skin-class credential (prefix only — may be revoked).
pub fn is_skin_token(raw: &str) -> bool {
    raw.starts_with(SKIN_KEY_PREFIX)
}

/// Never reuse the integrator prefix.
pub fn is_api_key_prefix(raw: &str) -> bool {
    raw.starts_with("k2sk_")
}

fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let out = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The alphabet of a generated App-user password: no look-alikes (0/O/o,
/// 1/l/I/i), so it can be read out loud or typed from a screen.
pub const GENERATED_PASSWORD_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789";
/// Characters in a generated password (four groups of five, joined by
/// `-`): 20 × log2(55) ≈ 115 bits.
pub const GENERATED_PASSWORD_CHARS: usize = 20;

/// A new App-user password from the OS CSPRNG (0.46.6: the owner's Set
/// password → Generate, Add user → Generate, and the agent reset card).
/// Unbiased (rejection sampling), `xxxxx-xxxxx-xxxxx-xxxxx`. The daemon
/// makes it; no client ever does.
pub fn generate_password() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let n = GENERATED_PASSWORD_ALPHABET.len();
    // The largest multiple of n that fits in a byte: bytes at or above it
    // would skew the low letters, so they are thrown away.
    let reject_at = (256 / n * n) as u16;
    let mut chars: Vec<u8> = Vec::with_capacity(GENERATED_PASSWORD_CHARS);
    let mut buf = [0u8; 64];
    while chars.len() < GENERATED_PASSWORD_CHARS {
        OsRng.fill_bytes(&mut buf);
        for &b in &buf {
            if (b as u16) < reject_at && chars.len() < GENERATED_PASSWORD_CHARS {
                chars.push(GENERATED_PASSWORD_ALPHABET[b as usize % n]);
            }
        }
    }
    chars
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Opaque CSPRNG hex (64 chars). Never a `k2skn_` / `k2sk_` pass.
fn generate_reset_raw() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    let mut hex = String::with_capacity(64);
    for b in buf {
        hex.push_str(&format!("{b:02x}"));
    }
    debug_assert!(
        !hex.starts_with(SKIN_KEY_PREFIX) && !hex.starts_with("k2sk_"),
        "reset token must not use a skin/API prefix"
    );
    hex
}

fn generate_raw_key() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut body = String::with_capacity(SKIN_KEY_PREFIX.len() + KEY_BODY_LEN);
    body.push_str(SKIN_KEY_PREFIX);
    const REJECT_AT: u8 = 248;
    let mut produced = 0;
    let mut scratch = [0u8; 64];
    while produced < KEY_BODY_LEN {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b >= REJECT_AT {
                continue;
            }
            body.push(BASE62[(b % 62) as usize] as char);
            produced += 1;
            if produced == KEY_BODY_LEN {
                break;
            }
        }
    }
    body
}

fn display_prefix(raw: &str) -> String {
    let tail = raw
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{SKIN_KEY_PREFIX}…{tail}")
}

/// The longest App username (review 0.46.4: every card, note and audit
/// line shows it).
pub const USERNAME_MAX_LEN: usize = 64;

/// Normalize a skin username: lowercase, `^[a-z0-9_-]{2,64}$`.
/// Do **not** accept `@` here — platform `--name` shares this charset.
pub fn normalize_username(raw: &str) -> Result<String, String> {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.len() < 2 {
        return Err("username must be at least 2 characters".to_string());
    }
    if lowered.len() > USERNAME_MAX_LEN {
        return Err(format!("username must be at most {USERNAME_MAX_LEN} characters"));
    }
    if !lowered
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err("username may contain only lowercase letters, digits, '_' and '-'".to_string());
    }
    Ok(lowered)
}

/// Trim + lowercase. Empty → `Ok(None)` (store NULL). Else
/// `^[^@\s]+@[^@\s]+\.[^@\s]+$` or `Err` (owner routes 400).
pub fn normalize_email(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let lowered = trimmed.to_ascii_lowercase();
    if email_shape_ok(&lowered) {
        Ok(Some(lowered))
    } else {
        Err("invalid email".to_string())
    }
}

/// `^[^@\s]+@[^@\s]+\.[^@\s]+$` without a regex crate.
fn email_shape_ok(s: &str) -> bool {
    if s.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let mut parts = s.split('@');
    let Some(local) = parts.next() else {
        return false;
    };
    let Some(domain) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    let Some(dot) = domain.find('.') else {
        return false;
    };
    let host = &domain[..dot];
    let rest = &domain[dot + 1..];
    !host.is_empty() && !rest.is_empty()
}

fn presented_looks_like_email(raw: &str) -> bool {
    raw.contains('@')
}

/// Parse + validate cap names. Empty/missing → default Thread read+post
/// (never silent-add `files:*`, `tickets:*`, `wiki:read`, `store:read`,
/// `store:write`, or `activity:read`). Unknown names fail loud. Write-only
/// is accepted at mint; list/read still require `files:read` /
/// `tickets:read` / `wiki:read` / `store:read` listed. `store:write` is
/// dump DML only. `files:read` and `files:write` do not imply
/// `activity:read`.
pub fn parse_caps(raw: Option<&[String]>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    match raw {
        None | Some([]) => {
            for c in DEFAULT_CAPS {
                out.push((*c).to_string());
            }
        }
        Some(list) => {
            for item in list {
                let t = item.trim();
                if t.is_empty() {
                    continue;
                }
                if t == CAP_VOICE_RECORD && !crate::recording::in_apps() {
                    return Err(record_cap_not_yet());
                }
                if ACCEPTED_CAPS.contains(&t) {
                    if !out.iter().any(|c| c == t) {
                        out.push(t.to_string());
                    }
                } else {
                    return Err(format!(
                        "unknown capability {t:?}; accepted: {}",
                        accepted_caps_csv()
                    ));
                }
            }
            if out.is_empty() {
                return Err(format!(
                    "caps must include at least one of {}",
                    accepted_caps_csv()
                ));
            }
            if out.iter().any(|c| c == CAP_FILES_AGENT_CONFIG)
                && !out.iter().any(|c| c == CAP_FILES_WRITE)
            {
                return Err(format!(
                    "{CAP_FILES_AGENT_CONFIG} only widens {CAP_FILES_WRITE}; list {CAP_FILES_WRITE} too"
                ));
            }
            if out.iter().any(|c| c == CAP_VOICE_VIDEO) && !out.iter().any(|c| c == CAP_VOICE_JOIN) {
                return Err(format!(
                    "{CAP_VOICE_VIDEO} only widens {CAP_VOICE_JOIN} (camera and screen in a call); list {CAP_VOICE_JOIN} too"
                ));
            }
            if out.iter().any(|c| c == CAP_VOICE_RECORD) && !out.iter().any(|c| c == CAP_VOICE_JOIN) {
                return Err(format!(
                    "{CAP_VOICE_RECORD} only widens {CAP_VOICE_JOIN} (start and stop recordings in a call you are in); list {CAP_VOICE_JOIN} too"
                ));
            }
        }
    }
    Ok(out)
}

/// The first person-only cap ([`PERSON_ONLY_CAPS`], in that order) in
/// `caps`, if any: what an agent may never grant.
pub fn person_only_cap_in<'a>(caps: impl IntoIterator<Item = &'a String>) -> Option<&'static str> {
    let caps: Vec<&String> = caps.into_iter().collect();
    PERSON_ONLY_CAPS.iter().copied().find(|p| caps.iter().any(|c| c.as_str() == *p))
}

/// The first person-only cap on any room line of `policy`.
pub fn person_only_cap_in_policy(policy: &RoomPolicy) -> Option<&'static str> {
    person_only_cap_in(policy.values().flatten())
}

/// The first person-only cap `new` adds on a room line where `old` (the
/// stored policy, `None` = nothing stored) doesn't hold it. `None` = no
/// widening.
pub fn person_only_cap_added(old: Option<&RoomPolicy>, new: &RoomPolicy) -> Option<&'static str> {
    PERSON_ONLY_CAPS.iter().copied().find(|p| {
        new.iter().any(|(room, caps)| {
            caps.iter().any(|c| c == p) && !old.and_then(|o| o.get(room)).is_some_and(|caps| caps.iter().any(|c| c == p))
        })
    })
}

/// `true` when any room line of `policy` carries [`CAP_FILES_AGENT_CONFIG`].
pub fn policy_has_agent_config(policy: &RoomPolicy) -> bool {
    policy
        .values()
        .any(|caps| caps.iter().any(|c| c == CAP_FILES_AGENT_CONFIG))
}

/// Rooms where `new` carries [`CAP_FILES_AGENT_CONFIG`] and `old` (the
/// stored policy, `None` = nothing stored) does not. Empty = no widening.
pub fn agent_config_added(old: Option<&RoomPolicy>, new: &RoomPolicy) -> Vec<String> {
    new.iter()
        .filter(|(_, caps)| caps.iter().any(|c| c == CAP_FILES_AGENT_CONFIG))
        .filter(|(room, _)| {
            !old.and_then(|o| o.get(*room))
                .is_some_and(|caps| caps.iter().any(|c| c == CAP_FILES_AGENT_CONFIG))
        })
        .map(|(room, _)| room.clone())
        .collect()
}

fn caps_json(caps: &[String]) -> String {
    serde_json::to_string(caps).unwrap_or_else(|_| "[]".to_string())
}

fn caps_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Missing/empty/unparseable rooms JSON → `[]` (deny all Thread).
/// **Never** copy [`parse_caps`] (empty caps → both verbs).
pub fn parse_rooms_json(raw: Option<&str>) -> Vec<String> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<serde_json::Value>>(s) {
        Ok(v) => v
            .into_iter()
            .filter_map(|x| match x {
                serde_json::Value::String(s) => {
                    let t = s.trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }
                _ => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn rooms_json(rooms: &[String]) -> String {
    serde_json::to_string(rooms).unwrap_or_else(|_| "[]".to_string())
}

fn rooms_from_json(raw: &str) -> Vec<String> {
    parse_rooms_json(Some(raw))
}

/// `project_id` → caps in that room. Empty map = Thread dark.
pub type RoomPolicy = BTreeMap<String, Vec<String>>;

fn default_thread_caps() -> Vec<String> {
    DEFAULT_CAPS.iter().map(|s| (*s).to_string()).collect()
}

fn cartesian_policy(caps: &[String], rooms: &[String]) -> RoomPolicy {
    let mut out = RoomPolicy::new();
    for id in rooms {
        let t = id.trim();
        if t.is_empty() {
            continue;
        }
        out.insert(t.to_string(), caps.to_vec());
    }
    out
}

fn thread_only_policy(rooms: &[String]) -> RoomPolicy {
    cartesian_policy(&default_thread_caps(), rooms)
}

fn union_caps(policy: &RoomPolicy) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in ACCEPTED_CAPS {
        if policy.values().any(|caps| caps.iter().any(|c| c == cap))
            && !out.iter().any(|c| c == cap)
        {
            out.push((*cap).to_string());
        }
    }
    out
}

fn rooms_from_policy(policy: &RoomPolicy) -> Vec<String> {
    policy.keys().cloned().collect()
}

fn room_policy_json(policy: &RoomPolicy) -> String {
    serde_json::to_string(policy).unwrap_or_else(|_| "{}".to_string())
}

fn stored_caps_list(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_stored_room_caps(list: Vec<String>) -> Vec<String> {
    if list.is_empty() {
        return default_thread_caps();
    }
    let mut out: Vec<String> = Vec::new();
    for c in list {
        if !out.iter().any(|x| x == &c) {
            out.push(c);
        }
    }
    out
}

/// Parse a stored `room_policy` JSON object. `None` = NULL / missing.
/// Empty object is `Some({})` (Thread dark), not cartesian.
fn parse_room_policy_json(raw: Option<&str>) -> Result<Option<RoomPolicy>, String> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty() && *s != "null") else {
        return Ok(None);
    };
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| format!("room_policy json: {e}"))?;
    match v {
        serde_json::Value::Object(map) => {
            let mut out = RoomPolicy::new();
            for (k, caps_v) in map {
                let k = k.trim().to_string();
                if k.is_empty() {
                    continue;
                }
                out.insert(k, normalize_stored_room_caps(stored_caps_list(&caps_v)));
            }
            Ok(Some(out))
        }
        _ => Err("room_policy must be a JSON object".to_string()),
    }
}

fn policy_or_cartesian(
    caps: &[String],
    rooms: &[String],
    raw: Option<&str>,
) -> Result<RoomPolicy, String> {
    match parse_room_policy_json(raw)? {
        Some(p) if p.is_empty() && !rooms.is_empty() => Ok(cartesian_policy(caps, rooms)),
        Some(p) => Ok(p),
        None => Ok(cartesian_policy(caps, rooms)),
    }
}

/// Session: JSON object → use it (including `{}`); NULL → caps × rooms.
/// Platform: always empty (flat `has_cap` && `has_room`). Never JOIN `roles`.
fn interpret_token_policy(
    session: bool,
    caps: &[String],
    rooms: &[String],
    raw: Option<&str>,
) -> RoomPolicy {
    if !session {
        return RoomPolicy::new();
    }
    match parse_room_policy_json(raw) {
        Ok(Some(p)) => p,
        Ok(None) => cartesian_policy(caps, rooms),
        // Unreadable: nothing, never every cap in every room.
        Err(e) => {
            crate::log_debug!("[skin] session room policy unreadable: {e}");
            RoomPolicy::new()
        }
    }
}

fn room_access_from_policy(policy: &RoomPolicy, paths: Option<&RoomPaths>) -> Vec<SkinRoomAccess> {
    let ids = rooms_from_policy(policy);
    live_agents(&ids)
        .into_iter()
        .map(|a| SkinRoomAccess {
            folders: paths.and_then(|p| p.get(&a.project_id)).filter(|s| !s.is_empty()).cloned(),
            handle: a.handle,
            caps: policy.get(&a.project_id).cloned().unwrap_or_default(),
        })
        .collect()
}

fn password_is_set(hash: Option<&str>) -> bool {
    hash.map(str::trim).is_some_and(|s| !s.is_empty())
}

fn hash_password(password: &str) -> Result<String, String> {
    let _slot = guard::hash_slot();
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

fn verify_hash(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let _slot = guard::hash_slot();
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn build_dummy_hash() -> String {
    let _slot = guard::hash_slot();
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"k2-skin-dummy-verify-target", &salt)
        .map(|h| h.to_string())
        .unwrap_or_else(|_| {
            String::from(
                "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
        })
}

fn dummy_verify(password: &str) {
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY_HASH.get_or_init(build_dummy_hash);
    let _ = verify_hash(password, dummy);
}

/// Host of Direct-mode `front_door.url` (no port). None when mode is not
/// `direct` or the URL is empty. Used for cookie-class matching — not the
/// Caddy bind filter (`host_from_front_door_url` rejects `*.k2.dev`).
pub fn direct_front_door_host() -> Option<String> {
    let door = get_front_door().ok()?;
    if !door.mode.eq_ignore_ascii_case("direct") {
        return None;
    }
    host_from_url(door.url.as_deref()?)
}

fn host_from_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(raw);
    let hostport = rest.split('/').next().unwrap_or(rest).trim();
    if hostport.is_empty() {
        return None;
    }
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(hostport).to_string()
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        if p.chars().all(|c| c.is_ascii_digit()) {
            h.to_string()
        } else {
            hostport.to_string()
        }
    } else {
        hostport.to_string()
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn normalize_room_ids(rooms: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in rooms {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// Nested label of the App (Skin) front door (`https://skin.<sub>.k2.dev`).
/// The skin door registers it itself (`skin_door::register_nested`); it
/// is in [`RESERVED_NESTED_LABELS`] so no workspace can claim it.
pub const RESERVED_NESTED_LABEL: &str = "skin";

/// Nested label of the future K2 Meet portal (`https://meet.<sub>.k2.dev`).
pub const MEET_NESTED_LABEL: &str = "meet";

/// Nested label of the future Sew chat web client (`https://chat.<sub>.k2.dev`).
pub const CHAT_NESTED_LABEL: &str = "chat";

/// Nested label of the future browser webmail front-end
/// (`https://mail.<sub>.k2.dev`). Web only, through the tunnel: it is
/// never a mail-server name (MX/SMTP/IMAP live on the client's own
/// delegated domain).
pub const MAIL_NESTED_LABEL: &str = "mail";

/// One box-level nested label that no workspace, user or agent may claim
/// as a publish / subdomain target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservedNestedLabel {
    pub label: &'static str,
    /// Short purpose, used in the refusal (`reserved for K2 Meet ...`).
    pub purpose: &'static str,
    /// `true` = a row that already used this name before it was reserved
    /// keeps working (start / stop / rm / re-run of that same row). Only
    /// NEW claims and points are refused. `skin` was reserved before any
    /// row could hold it, so it stays strict.
    pub grandfathered: bool,
}

/// Every reserved nested label. Box-level front doors register these
/// themselves through the same path the skin door uses
/// (`skin_door::register_nested`: control-plane create/point with the
/// box's tunnel token), never through the user publish / subdomain
/// routes. A future Sew door registers `meet` and `chat` that way.
pub const RESERVED_NESTED_LABELS: &[ReservedNestedLabel] = &[
    ReservedNestedLabel {
        label: RESERVED_NESTED_LABEL,
        purpose: "the App front door",
        grandfathered: false,
    },
    ReservedNestedLabel {
        label: MEET_NESTED_LABEL,
        purpose: "K2 Meet (meet.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: CHAT_NESTED_LABEL,
        purpose: "K2 chat (chat.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: MAIL_NESTED_LABEL,
        purpose: "K2 webmail (mail.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: VOICE_NESTED_LABEL,
        purpose: "K2 chat voice (voice.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: TURN_NESTED_LABEL,
        purpose: "K2 chat voice relay (TURN)",
        grandfathered: true,
    },
];

/// Sew voice signalling (`voice.<box>.k2.dev` → sew-media's loopback
/// 7880), registered box-level by the Sew supervisor (K13).
pub const VOICE_NESTED_LABEL: &str = "voice";
/// Kept from tenants: Sew's TURN host is the single label
/// `turn-<box>.k2.dev` (k2-connect turn mode), never a nested `turn.<box>`,
/// but a nested `turn` would read as it.
pub const TURN_NESTED_LABEL: &str = "turn";

/// The reserved entry for `name` (trimmed, case-insensitive), if any.
/// Exact match only: `meeting`, `chats`, `skins` are ordinary labels.
pub fn reserved_nested_label(name: &str) -> Option<&'static ReservedNestedLabel> {
    let n = name.trim();
    RESERVED_NESTED_LABELS
        .iter()
        .find(|r| n.eq_ignore_ascii_case(r.label))
}

pub fn is_reserved_nested_label(name: &str) -> bool {
    reserved_nested_label(name).is_some()
}

/// A reserved label whose pre-existing rows keep working (see
/// [`ReservedNestedLabel::grandfathered`]).
pub fn is_grandfathered_reserved_label(name: &str) -> bool {
    reserved_nested_label(name).is_some_and(|r| r.grandfathered)
}

pub fn reserved_nested_label_error(label: &str) -> String {
    let label = label.trim().to_ascii_lowercase();
    match reserved_nested_label(&label) {
        Some(r) if r.label != RESERVED_NESTED_LABEL => format!(
            "reserved_label: '{label}' is reserved for {}. Pick another nested label.",
            r.purpose
        ),
        // `skin` (and any non-reserved caller misuse) keeps the original text.
        _ => format!(
            "reserved_label: '{label}' is reserved. Pick another nested label for your UI (k2 study apps; leftover: k2 study skins)."
        ),
    }
}

/// `https://rosson.k2.dev` → `https://skin.rosson.k2.dev`. Idempotent if
/// the host already starts with `skin.`.
pub fn skin_url_from_public(public_url: &str) -> Option<String> {
    let rest = public_url.trim().trim_end_matches('/');
    let host = rest
        .strip_prefix("https://")
        .or_else(|| rest.strip_prefix("http://"))?;
    let host = host.split('/').next().unwrap_or(host).trim();
    if host.is_empty() {
        return None;
    }
    if host.starts_with("skin.") {
        return Some(format!("https://{host}"));
    }
    Some(format!("https://skin.{host}"))
}

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinPrincipal {
    pub id: String,
    pub username: String,
    pub created_at: i64,
    #[serde(default)]
    pub default_rooms: Vec<String>,
    #[serde(default)]
    pub default_room_handles: Vec<String>,
    /// True when a K2-login password is set. Never the hash.
    #[serde(default)]
    pub has_password: bool,
    /// Assigned skin role id. `None` = unassigned (I6 Thread-only).
    #[serde(default)]
    pub role_id: Option<String>,
    /// Assigned skin role name. `None` when unassigned.
    #[serde(default)]
    pub role_name: Option<String>,
    /// Optional guest email. Serialized `null` when unset. Not a username.
    #[serde(default)]
    pub email: Option<String>,
    /// Optional guest full name. Serialized `null` when unset. Not a username.
    /// Owner-only writes sanitize via `sanitize_owner_display_name` (cap 64).
    #[serde(default)]
    pub full_name: Option<String>,
    /// Owner-only mark: this guest principal is the agent of one workspace
    /// (`projects.id`). Its brokered db calls stamp actor kind `agent`.
    /// `None` = a person (actor kind `guest`).
    #[serde(default)]
    pub agent_workspace: Option<String>,
    /// Display only: live handle of `agent_workspace` (skip when gone).
    #[serde(default)]
    pub agent_handle: Option<String>,
}

/// Per-room functions on the wire: `{handle, caps, folders?}`. `folders`
/// is the role line's path scope (room-scope R3); absent = the whole room.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinRoomAccess {
    pub handle: String,
    pub caps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<RoomPathScope>,
}

/// Named bundle of per-room functions for Skin Access guests (not Connect roles).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinRole {
    pub id: String,
    pub name: String,
    /// Union of caps across rooms (display / naive SPA).
    pub caps: Vec<String>,
    /// Stored ACL: `projects.id` UUIDs. Empty = Thread dark.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Display-only, resolved live from `projects.handle`. Skip missing.
    #[serde(default)]
    pub room_handles: Vec<String>,
    /// Per-room functions. Handle is live; skip missing projects.
    #[serde(default)]
    pub room_access: Vec<SkinRoomAccess>,
    pub created_at: i64,
    /// App this role belongs to. `None` = host-wide (every existing row).
    #[serde(default)]
    pub app_id: Option<String>,
    /// SSOT map `project_id → caps[]`. Not on the wire.
    #[serde(skip)]
    pub room_policy: RoomPolicy,
    /// `project_id → folders` (room-scope R3). Not on the wire as a map:
    /// each `room_access` line carries its own `folders`.
    #[serde(skip)]
    pub room_paths: RoomPaths,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinTokenMeta {
    pub id: String,
    /// Platform label (`session=0`) or session stamp username (`session=1`).
    pub name: String,
    /// Display prefix (`k2skn_…ab12`). Never the secret.
    pub prefix: String,
    pub caps: Vec<String>,
    /// Stored ACL: `projects.id` UUIDs.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Display-only, resolved live from `projects.handle`. Skip missing.
    #[serde(default)]
    pub room_handles: Vec<String>,
    /// Per-room functions (session snapshot). Empty on platform tokens.
    #[serde(default)]
    pub room_access: Vec<SkinRoomAccess>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// One workspace a skin pass may Thread. Wire `{handle, projectId,
/// displayName, color, image}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinAgent {
    pub handle: String,
    pub project_id: String,
    /// Guest-facing label. Handle stays the id.
    pub display_name: String,
    /// The agent's color, `#rrggbb` (0.45.5). `None` only for a row read
    /// before the color was known.
    #[serde(default)]
    pub color: Option<String>,
    /// The agent's picture as a PNG/JPEG/WebP/GIF data URL, or `None` (no
    /// picture, or one an App is never sent: SVG, oversize). 0.45.5.
    #[serde(default)]
    pub image: Option<String>,
}

/// Resolved live pass. Safe to log `id` / `username` / `caps` — never a secret.
/// `username` is the overlay stamp: session principal or platform `name`.
/// No `role_id` — snapshot only, never a live JOIN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinPass {
    pub id: String,
    /// `Some` for login sessions; `None` for platform tokens.
    pub principal_id: Option<String>,
    pub username: String,
    /// Union of caps (display / naive SPA). Not a session files/thread door.
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    /// Login-minted session pass (`session=1`). Platform mint is `false`.
    pub session: bool,
    /// Session: `project_id → caps[]`. Platform: empty (use flat `has_cap`).
    pub room_policy: RoomPolicy,
    /// The App this login is bound to (S24-5): `host` or a published service
    /// id. A pre-0.45.5 session row (NULL) reads as `host`. Platform passes
    /// (`session=0`) are not App-bound yet (S14) and carry `""`. For logs and
    /// later S14/S43 checks; no handler authorizes on it in 0.45.5.
    pub app: String,
    /// Folder path scope per room line (room-scope R3), read at resolve
    /// time from the session's own grant → role (an edit applies from the
    /// session's next request). Platform and link passes: `Whole`.
    pub room_paths: paths::PassPaths,
    /// When this pass stops working by itself (unix seconds), if it does. A hub
    /// holding it open wakes then.
    pub expires_at: Option<i64>,
    /// A login with an enabled chat grant, so `chat.pass` may work. Platform and
    /// link passes: `false`.
    pub chat: bool,
}

impl SkinPass {
    /// This pass's folders on one room line (R3). An unscoped line (or a
    /// platform/link pass) is `RoomPathScope::default()`, the whole room.
    /// `Err` = the scope couldn't be read: callers fail closed with the
    /// missing-cap 403.
    pub fn path_scope(&self, project_id: &str) -> Result<RoomPathScope, String> {
        match &self.room_paths {
            paths::PassPaths::Whole => Ok(RoomPathScope::default()),
            paths::PassPaths::Lines(lines) => Ok(lines.get(project_id.trim()).cloned().unwrap_or_default()),
            paths::PassPaths::Unreadable(e) => Err(e.clone()),
        }
    }

    pub fn has_cap(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }

    /// Session: key exists on the map. Platform: listed in `rooms`.
    pub fn has_room(&self, project_id: &str) -> bool {
        let id = project_id.trim();
        if id.is_empty() {
            return false;
        }
        if self.session {
            self.room_policy.contains_key(id)
        } else {
            self.rooms.iter().any(|r| r == id)
        }
    }

    /// Session: cap listed on that room. Platform: `has_cap` && `has_room`.
    pub fn has_cap_in_room(&self, project_id: &str, cap: &str) -> bool {
        if self.session {
            self.room_policy
                .get(project_id.trim())
                .map(|caps| caps.iter().any(|c| c == cap))
                .unwrap_or(false)
        } else {
            self.has_cap(cap) && self.has_room(project_id)
        }
    }

    /// Dispatcher door. Session defers the cap until the room is known.
    /// Platform still requires the pass-level cap.
    pub fn dispatcher_admits_cap(&self, cap: &str) -> bool {
        if self.session {
            true
        } else {
            self.has_cap(cap)
        }
    }

    /// AH29 any-of door (one socket, several caps). Session defers to the
    /// room check. Platform needs at least one listed cap on the pass.
    pub fn dispatcher_admits_any(&self, caps: &[&str]) -> bool {
        if self.session {
            true
        } else {
            caps.iter().any(|c| self.has_cap(c))
        }
    }

    /// AH29: at least one of `caps` in that room (same rules as
    /// [`Self::has_cap_in_room`]).
    pub fn has_any_cap_in_room(&self, project_id: &str, caps: &[&str]) -> bool {
        caps.iter().any(|c| self.has_cap_in_room(project_id, c))
    }

    pub fn rooms_empty(&self) -> bool {
        if self.session {
            self.room_policy.is_empty()
        } else {
            self.rooms.is_empty()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinFrontDoor {
    /// `connect` (nested `skin.<sub>.k2.dev`) or `direct` (Caddy on this box).
    pub mode: String,
    pub url: Option<String>,
    pub hint: Option<String>,
    /// Optional same-origin UI reverse-proxy target for Caddy `handle /`.
    pub ui_port: Option<u16>,
}

impl SkinFrontDoor {
    pub fn default_connect() -> Self {
        Self {
            mode: "connect".to_string(),
            url: None,
            hint: Some(
                "Nested hostname on the existing tunnel. Operator URL stays the kingdom door."
                    .to_string(),
            ),
            ui_port: None,
        }
    }
}

pub fn parse_front_door_mode(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "connect" => Ok("connect"),
        "direct" => Ok("direct"),
        other => Err(format!(
            "unknown front-door mode {other:?}; accepted: connect, direct"
        )),
    }
}

fn default_hint_for_mode(mode: &str) -> &'static str {
    match mode {
        "direct" => {
            "Caddy :443 (or a LAN port) → daemon loopback. Do not bind k2-daemon to the world."
        }
        _ => "Nested hostname on the existing tunnel. Operator URL stays the kingdom door.",
    }
}

// ── DB ───────────────────────────────────────────────────────────────

struct SkinDb {
    path: PathBuf,
    conn: Connection,
}

fn store_path() -> PathBuf {
    crate::paths::k2_home().join("skin.db")
}

const TOKENS_DDL: &str = "CREATE TABLE IF NOT EXISTS tokens (
            id TEXT PRIMARY KEY,
            principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
            name TEXT,
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            caps TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            revoked_at INTEGER,
            session INTEGER NOT NULL DEFAULT 0,
            expires_at INTEGER,
            CHECK (
                (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                OR
                (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
            )
         )";

/// One `host` app row. `ON CONFLICT DO NOTHING` keeps an owner-written
/// grant (any role or scope). Used by the one-time copy and by a new
/// principal. A deleted row is not inserted again on a later open.
fn insert_host_grant(
    conn: &Connection,
    principal_id: &str,
    role_id: Option<String>,
    scope: Option<String>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO grants
         (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled)
         VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, 1)
         ON CONFLICT(subject_kind, subject_id, kind, target_id) DO NOTHING",
        params![
            uuid::Uuid::new_v4().to_string(),
            principal_id,
            HOST_APP_ID,
            role_id,
            scope,
            now_secs(),
        ],
    )
    .map_err(|e| format!("skin host grant: {e}"))?;
    Ok(())
}

/// Copy principals who have no host grant. Assigned: that `role_id`, scope
/// NULL, and the role stays host-wide (`roles.app_id` is not set).
/// Unassigned: `role_id` NULL and `scope` = stored `default_rooms` JSON,
/// including `[]`. A guest with no rooms still gets a row.
///
/// Call only from [`ensure_host_grant_backfill`]. A later open must not
/// call this: a deleted host grant stays deleted.
fn backfill_host_grants(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT id, role_id, default_rooms FROM principals")
        .map_err(|e| format!("skin host grant: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| format!("skin host grant: {e}"))?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row.map_err(|e| format!("skin host grant: {e}"))?);
    }
    drop(stmt);
    for (id, role_id, rooms) in pending {
        if let Some(role_id) = blank_to_none(role_id) {
            insert_host_grant(conn, &id, Some(role_id), None)?;
        } else {
            let scope = match rooms {
                Some(s) if !s.trim().is_empty() => s,
                _ => "[]".to_string(),
            };
            insert_host_grant(conn, &id, None, Some(scope))?;
        }
    }
    Ok(())
}

/// Run [`backfill_host_grants`] once per database. The row in
/// `host_grant_backfill` is the record. Later opens see that row and do
/// not copy again, so revoking the host grant survives a restart.
/// Principals created after the copy get their row from [`add_principal_with_email`].
fn ensure_host_grant_backfill(conn: &mut Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_grant_backfill (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            done_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin host grant: {e}"))?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("skin host grant: {e}"))?;
    let done: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM host_grant_backfill WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin host grant: {e}"))?;
    if done == 0 {
        backfill_host_grants(&tx)?;
        tx.execute(
            "INSERT INTO host_grant_backfill (id, done_at) VALUES (1, ?1)",
            params![now_secs()],
        )
        .map_err(|e| format!("skin host grant: {e}"))?;
    }
    tx.commit().map_err(|e| format!("skin host grant: {e}"))?;
    Ok(())
}

fn open_db(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    if let Err(e) = crate::private_home::prepare_private_db_file(path) {
        crate::log_debug!("[skin] WARN create {} owner-only: {e}", path.display());
    }
    let mut conn = Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    let _ = conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA temp_store = MEMORY;",
    );
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principals (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    conn.execute_batch(TOKENS_DDL)
        .map_err(|e| format!("skin.db schema: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS front_door (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL,
            url TEXT,
            hint TEXT,
            updated_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    let _ = conn.execute("ALTER TABLE front_door ADD COLUMN ui_port INTEGER", []);
    let _ = conn.execute(
        "ALTER TABLE tokens ADD COLUMN rooms TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN name TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE principals ADD COLUMN default_rooms TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    // Skin login (prd-skin-login-v1): ignore duplicate ALTER like `ui_port`.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN password_hash TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE tokens ADD COLUMN session INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN expires_at INTEGER", []);
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS login_lockouts (
            username TEXT PRIMARY KEY,
            failed_count INTEGER NOT NULL DEFAULT 0,
            locked_until INTEGER
         );",
    );
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS password_reset_tokens (
            id TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            expires_at INTEGER NOT NULL,
            used_at INTEGER,
            created_at INTEGER NOT NULL
         );",
    );
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS password_reset_throttle (
            username TEXT PRIMARY KEY,
            last_attempt INTEGER NOT NULL
         );",
    );
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS roles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            caps TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS roles_name ON roles(name COLLATE NOCASE);",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    let _ = conn.execute(
        "ALTER TABLE principals ADD COLUMN role_id TEXT REFERENCES roles(id) ON DELETE RESTRICT",
        [],
    );
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN email TEXT", []);
    // G10: full name on the guest. Ignore duplicate ALTER like `email`.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN full_name TEXT", []);
    // Owner-only agent mark (`k2 app user agent`). Additive like `full_name`:
    // skin.db has no schema version; a duplicate ALTER is ignored.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN agent_workspace TEXT", []);
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS principals_email
         ON principals(email COLLATE NOCASE)
         WHERE email IS NOT NULL AND trim(email) != '';",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    let _ = conn.execute(
        "ALTER TABLE roles ADD COLUMN room_policy TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    // Nullable app scope. Existing rows stay NULL (host-wide). Ignore
    // duplicate ALTER. Not a daemon migration.
    let _ = conn.execute("ALTER TABLE roles ADD COLUMN app_id TEXT", []);
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS grants (
            id TEXT PRIMARY KEY,
            subject_kind TEXT NOT NULL,
            subject_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            target_id TEXT NOT NULL,
            role_id TEXT,
            scope TEXT,
            created_at INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            UNIQUE(subject_kind, subject_id, kind, target_id)
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    // Existing databases created before `enabled`. Ignore duplicate ALTER.
    let _ = conn.execute(
        "ALTER TABLE grants ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    // Access templates are not roles and not daemon SQLite. A line stores
    // kind and, for kind=app, role_id. It does not store target_id.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS access_templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL COLLATE NOCASE UNIQUE,
            created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS access_template_lines (
            id TEXT PRIMARY KEY,
            template_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            role_id TEXT,
            position INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    // RS-A15 (0.46.1): the owner's per-App grants (`guests-rooms`).
    // Additive, so no numbered step: an older K2 ignores the table.
    guest_rooms::ensure_schema(&conn)?;
    // app-mobile-access-v1 (0.46.3): device sign-ins, per-App sign-in
    // settings. Additive, no numbered step.
    app_login::ensure_schema(&conn)?;
    // agent-app-users (0.46.4): per-App invite settings. Additive.
    invites::ensure_schema(&conn)?;
    // 0.46.4: test guests of one App. Additive, no step.
    test_guests::ensure_schema(&conn)?;
    // 0.46.4: which workspace's agent added a guest. Additive, no step.
    creators::ensure_schema(&conn)?;
    handover::ensure_schema(&conn)?;
    // prd-custom-app-push-v1 (0.46.4): push devices and per-App push
    // settings. Additive, no numbered step.
    app_push::ensure_schema(&conn)?;
    // prd-app-phone-pairing-v1 (0.46.5): the owner's phone-app config and
    // the pairing rows. Additive, no numbered step.
    app_pairing::ensure_schema(&conn)?;
    // app-user-profile (0.46.5): profile settings, profiles, session
    // meta. Additive, no numbered step (a 0.46.4 ignores them).
    profile::ensure_schema(&conn)?;
    // Sew for Apps A2 (0.46.6): the owner's per-App guests-chat grants.
    // Additive, no numbered step.
    app_chat::ensure_schema(&conn)?;
    // App-governed Spaces S1 (0.46.7): per-App chat keys. Additive, no
    // numbered step.
    app_chat_key::ensure_schema(&conn)?;
    // App account deletion (0.46.9): settings and pending requests.
    // Additive, no numbered step.
    account_deletion::ensure_schema(&conn)?;
    // The sign-in door (0.46.9): per-App door settings. Additive, no
    // numbered step.
    auth_door::ensure_schema(&conn)?;
    // Copy existing guests once. A deleted host row stays deleted.
    ensure_host_grant_backfill(&mut conn)?;
    rebuild_tokens_table_if_needed(&mut conn)?;
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN room_policy TEXT", []);
    ensure_platform_name_index(&conn)?;
    migrate_room_policy(&conn)?;
    // Sew S27 grant hook: `tool_outbox` + `tool_targets`. Idempotent and
    // additive like `grants`; NOT a ladder step (user_version stays put).
    crate::tool_hook::outbox::ensure_schema(&conn)?;
    // K31: App ↔ Space bindings and the membership ledger (same rules).
    crate::tool_hook::app_space::ensure_schema(&conn)?;
    // Sew for Apps A3: the picture Sew shows per member (same rules).
    crate::tool_hook::avatar::ensure_schema(&conn)?;
    // Sew for Apps A6: the identity map, principal ↔ Sew member.
    crate::tool_hook::members::ensure_schema(&conn)?;
    // Decision 13: the owner's home-client and per-App settings for Sew.
    crate::tool_hook::home::ensure_schema(&conn)?;
    // Sew's feature flags as the sweep last saw them.
    crate::tool_hook::features::ensure_schema(&conn)?;
    // Sew 0.5.0: the managed-fields policy as the sweep last saw it.
    crate::tool_hook::policy::ensure_schema(&conn)?;
    // Sew 0.6.0: App-governed Spaces (channels, the sync ledger, the App's
    // own lists, box users leaving, service owners to retire).
    crate::tool_hook::governed::ensure_schema(&conn)?;
    // Voice rooms (Sew 0.7.0): the K2 Admin Space, its members and rooms.
    crate::tool_hook::admin_space::ensure_schema(&conn)?;
    // Sew 0.9.0: recording in Apps (the cap rollout, locks, the feed).
    crate::tool_hook::recording::ensure_schema(&conn)?;
    // 0.46.11: agents review the K5 rooms' transcripts.
    crate::voice_transcripts::ensure_schema(&conn)?;
    // S24-1: numbered, additive steps on top of the idempotent block above.
    run_schema_steps(&mut conn)?;
    // skin.db AND its -wal/-shm: the WAL holds recent pages (guest
    // password hashes, pass hashes) until a checkpoint.
    if let Err(e) = crate::private_home::restrict_db_files(path) {
        crate::log_debug!("[skin] WARN {e}");
    }
    // Last, so the migrations above wake nothing.
    access_live::install(&mut conn).map_err(|e| format!("skin.db update hook: {e}"))?;
    Ok(conn)
}

fn tokens_principal_id_notnull(conn: &Connection) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(tokens)")
        .map_err(|e| format!("skin.db table_info: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            let name: String = r.get(1)?;
            let notnull: i64 = r.get(3)?;
            Ok((name, notnull))
        })
        .map_err(|e| format!("skin.db table_info: {e}"))?;
    for row in rows {
        let (name, notnull) = row.map_err(|e| format!("skin.db table_info row: {e}"))?;
        if name == "principal_id" {
            return Ok(notnull != 0);
        }
    }
    Ok(false)
}

fn key_prefix_last4_alnum(key_prefix: &str) -> Result<String, String> {
    let alnum: String = key_prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if alnum.len() < 4 {
        return Err(format!(
            "skin.db migrate: key_prefix {key_prefix:?} has no ASCII alnum last4"
        ));
    }
    Ok(alnum[alnum.len() - 4..].to_ascii_lowercase())
}

fn migrate_platform_name(
    username: &str,
    key_prefix: &str,
    live_taken: &HashSet<String>,
    live: bool,
) -> Result<String, String> {
    let username = username.trim().to_ascii_lowercase();
    if username.is_empty() {
        return Err("skin.db migrate: session=0 token has empty principal username".to_string());
    }
    let last4 = key_prefix_last4_alnum(key_prefix)?;
    let taken = |n: &str| live && live_taken.contains(n);
    let mut candidate = username.clone();
    if candidate == "owner" || taken(&candidate) {
        candidate = format!("{username}-{last4}");
    }
    if candidate == "owner" {
        return Err("skin.db migrate: platform token name 'owner' is reserved".into());
    }
    if taken(&candidate) {
        return Err(format!(
            "skin.db migrate: platform token name '{candidate}' collides after suffix"
        ));
    }
    Ok(candidate)
}

fn ensure_platform_name_index(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS tokens_platform_name_live
         ON tokens(name COLLATE NOCASE)
         WHERE session = 0 AND revoked_at IS NULL;",
    )
    .map_err(|e| format!("skin.db platform name index: {e}"))
}

/// Copy 132 cartesian caps onto each listed room. Then R8-rewrite live
/// `session=1`. Platform `session=0` stay NULL forever. Fail only on SQL/JSON.
fn migrate_room_policy(conn: &Connection) -> Result<(), String> {
    struct RoleRow {
        id: String,
        caps: String,
        rooms: String,
        room_policy: String,
    }
    let mut stmt = conn
        .prepare("SELECT id, caps, rooms, COALESCE(room_policy, '{}') FROM roles")
        .map_err(|e| format!("skin.db room_policy roles: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RoleRow {
                id: r.get(0)?,
                caps: r.get(1)?,
                rooms: r.get(2)?,
                room_policy: r.get(3)?,
            })
        })
        .map_err(|e| format!("skin.db room_policy roles: {e}"))?;
    let mut roles = Vec::new();
    for row in rows {
        roles.push(row.map_err(|e| format!("skin.db room_policy role row: {e}"))?);
    }
    drop(stmt);
    for role in roles {
        let caps = caps_from_json(&role.caps);
        let rooms = rooms_from_json(&role.rooms);
        let parsed = parse_room_policy_json(Some(&role.room_policy))?;
        let leftover_132 = match &parsed {
            Some(p) if p.is_empty() && !rooms.is_empty() => true,
            None if !rooms.is_empty() => true,
            _ => false,
        };
        if leftover_132 {
            let policy = cartesian_policy(&caps, &rooms);
            conn.execute(
                "UPDATE roles SET room_policy = ?1, caps = ?2, rooms = ?3 WHERE id = ?4",
                params![
                    room_policy_json(&policy),
                    caps_json(&union_caps(&policy)),
                    rooms_json(&rooms_from_policy(&policy)),
                    role.id
                ],
            )
            .map_err(|e| format!("skin.db room_policy role update: {e}"))?;
            if column_exists(conn, "tokens", "app_service_id")? {
                rewrite_sessions_for_role(conn, &role.id, &policy)?;
            } else {
                // Before schema step 2 the role column is still the login
                // truth (and the App column does not exist yet).
                conn.execute(
                    "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
                     WHERE session = 1 AND revoked_at IS NULL
                       AND principal_id IN (SELECT id FROM principals WHERE role_id = ?4)",
                    params![
                        caps_json(&union_caps(&policy)),
                        rooms_json(&rooms_from_policy(&policy)),
                        room_policy_json(&policy),
                        role.id
                    ],
                )
                .map_err(|e| format!("skin session rewrite: {e}"))?;
            }
        }
    }

    struct SessRow {
        id: String,
        caps: String,
        rooms: String,
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, caps, rooms FROM tokens
             WHERE session = 1 AND room_policy IS NULL",
        )
        .map_err(|e| format!("skin.db room_policy sessions: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SessRow {
                id: r.get(0)?,
                caps: r.get(1)?,
                rooms: r.get(2)?,
            })
        })
        .map_err(|e| format!("skin.db room_policy sessions: {e}"))?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| format!("skin.db room_policy session row: {e}"))?);
    }
    drop(stmt);
    for row in sessions {
        let caps = caps_from_json(&row.caps);
        let rooms = rooms_from_json(&row.rooms);
        let policy = cartesian_policy(&caps, &rooms);
        conn.execute(
            "UPDATE tokens SET room_policy = ?1 WHERE id = ?2",
            params![room_policy_json(&policy), row.id],
        )
        .map_err(|e| format!("skin.db room_policy session update: {e}"))?;
    }
    Ok(())
}

// ── Schema version (S24-1) ───────────────────────────────────────────

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("skin.db table_info {table}: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("skin.db table_info {table}: {e}"))?;
    for row in rows {
        if row.map_err(|e| format!("skin.db table_info {table} row: {e}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists_conn(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", params![name], |r| r.get::<_, bool>(0))
        .map_err(|e| format!("skin.db table lookup {name}: {e}"))
}

fn schema_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| format!("skin.db user_version: {e}"))
}

/// Open refusal when the file was written by a newer K2 (S24-1). Older
/// daemons never read the version, so this only guards later downgrades.
pub fn schema_newer_error(found: i64) -> String {
    format!(
        "skin.db schema version {found} is newer than this K2 understands ({SKIN_SCHEMA_VERSION}). \
         Refusing to open it: update K2, or restore the skin.db backup taken before the downgrade."
    )
}

/// One audit line (`~/.k2/auth-audit.jsonl`) for an access change the
/// daemon made on its own (upgrade sync, App backfill) or an owner copy.
fn record_access_audit(event: &str, actor: &str, line: &str) {
    crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
        event, actor, line, "-", "-", "daemon",
    ));
}

/// Run every schema step above the file's `user_version`, each in its own
/// IMMEDIATE transaction that also sets the version. Audit lines are
/// written after the step commits.
fn run_schema_steps(conn: &mut Connection) -> Result<(), String> {
    let mut v = schema_version(conn)?;
    if v > SKIN_SCHEMA_VERSION {
        return Err(schema_newer_error(v));
    }
    // agent-app-users (security review item 13): an existing file is copied
    // before step 6, so a downgrade can restore it (a 0.46.3 K2 refuses a
    // version-6 file). `skin.db.pre-6.bak` beside it, 0600.
    if (1..6).contains(&v) {
        backup_before_step(conn, 6)?;
    }
    // Must-change (0.46.6): a 0.46.5 K2 refuses a version-7 file, so a
    // version-6 file is copied first too (`skin.db.pre-7.bak`).
    if v == 6 {
        backup_before_step(conn, 7)?;
    }
    // Failure decay (0.46.6, step 8): a version-7 file (a 0.46.6 build
    // before it) is copied first too (`skin.db.pre-8.bak`).
    if v == 7 {
        backup_before_step(conn, 8)?;
    }
    while v < SKIN_SCHEMA_VERSION {
        let next = v + 1;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| format!("skin.db step {next} begin: {e}"))?;
        // Another opener may have run it between our read and the lock.
        let cur = schema_version(&tx)?;
        if cur >= next {
            tx.commit()
                .map_err(|e| format!("skin.db step {next} commit: {e}"))?;
            v = cur;
            continue;
        }
        let audit = match next {
            1 => step1_sync_host_grants(&tx)?,
            2 => {
                step2_app_columns(&tx)?;
                Vec::new()
            }
            3 => {
                step3_mark_app_backfill(&tx)?;
                Vec::new()
            }
            4 => {
                step4_link_column(&tx)?;
                Vec::new()
            }
            5 => {
                paths::step5_room_paths_column(&tx)?;
                Vec::new()
            }
            6 => {
                ties::step6_ties_and_invites(&tx)?;
                Vec::new()
            }
            7 => {
                must_change::step7_must_change(&tx)?;
                Vec::new()
            }
            8 => {
                step8_lockout_fail_time(&tx)?;
                Vec::new()
            }
            other => return Err(format!("skin.db: no schema step {other}")),
        };
        tx.execute_batch(&format!("PRAGMA user_version = {next};"))
            .map_err(|e| format!("skin.db step {next} version: {e}"))?;
        tx.commit()
            .map_err(|e| format!("skin.db step {next} commit: {e}"))?;
        for (user, line) in audit {
            record_access_audit("app_grant_upgrade", &user, &line);
        }
        v = next;
    }
    Ok(())
}

/// Step 8 (0.46.6, App guest login lockout fix 1c): when the last failure
/// on a `login_lockouts` row happened. Existing rows keep NULL: their
/// count starts again at the next failure ([`LOCKOUT_FAIL_WINDOW_SECS`]).
fn step8_lockout_fail_time(conn: &Connection) -> Result<(), String> {
    if !column_exists(conn, "login_lockouts", "last_failed_at")? {
        conn.execute_batch("ALTER TABLE login_lockouts ADD COLUMN last_failed_at INTEGER;")
            .map_err(|e| format!("skin.db step 8: {e}"))?;
    }
    Ok(())
}

/// Copy the open skin.db to `skin.db.pre-<step>.bak` (SQLite `VACUUM
/// INTO`, a consistent copy) unless that backup already exists.
fn backup_before_step(conn: &Connection, step: i64) -> Result<(), String> {
    let path = store_path();
    let bak = path.with_file_name(format!("skin.db.pre-{step}.bak"));
    if bak.exists() {
        return Ok(());
    }
    conn.execute("VACUUM INTO ?1", params![bak.to_string_lossy()])
        .map_err(|e| format!("skin.db backup before step {step}: {e}"))?;
    if let Err(e) = crate::private_home::restrict_db_files(&bak) {
        crate::log_debug!("[skin] WARN {e}");
    }
    record_access_audit("skin_db_backup", "daemon", &format!("before step {step}: {}", bak.display()));
    Ok(())
}

/// Step 1 (AM1, S24-2): the host grant becomes the login truth. Every host
/// row (enabled or disabled) is synced from what login used before 0.45.5:
/// an assigned guest's `principals.role_id` (it wins over a different grant
/// role, D1), else `role_id NULL` + `scope = default_rooms` (`[]` kept).
/// Principals with no host row (box login off) are left alone. Returns one
/// `(username, line)` per row whose value changed.
fn step1_sync_host_grants(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    struct Row {
        grant_id: String,
        grant_role: Option<String>,
        grant_scope: Option<String>,
        username: String,
        principal_role: Option<String>,
        default_rooms: Option<String>,
    }
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT g.id, g.role_id, g.scope, p.username, p.role_id, p.default_rooms
                 FROM grants g JOIN principals p ON p.id = g.subject_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1",
            )
            .map_err(|e| format!("skin.db step 1: {e}"))?;
        let mapped = stmt
            .query_map(params![HOST_APP_ID], |r| {
                Ok(Row {
                    grant_id: r.get(0)?,
                    grant_role: r.get(1)?,
                    grant_scope: r.get(2)?,
                    username: r.get(3)?,
                    principal_role: r.get(4)?,
                    default_rooms: r.get(5)?,
                })
            })
            .map_err(|e| format!("skin.db step 1: {e}"))?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin.db step 1 row: {e}"))?
    };
    let mut audit = Vec::new();
    for row in rows {
        let (want_role, want_scope) = match blank_to_none(row.principal_role) {
            Some(role) => (Some(role), None),
            None => (
                None,
                Some(rooms_json(&rooms_from_json(
                    row.default_rooms.as_deref().unwrap_or("[]"),
                ))),
            ),
        };
        let have_role = blank_to_none(row.grant_role);
        let have_scope = blank_to_none(row.grant_scope);
        let scope_same = match (&have_scope, &want_scope) {
            (Some(a), Some(b)) => rooms_from_json(a) == rooms_from_json(b),
            (None, None) => true,
            _ => false,
        };
        if have_role == want_role && scope_same {
            continue;
        }
        conn.execute(
            "UPDATE grants SET role_id = ?1, scope = ?2 WHERE id = ?3",
            params![want_role, want_scope, row.grant_id],
        )
        .map_err(|e| format!("skin.db step 1 update: {e}"))?;
        audit.push((
            row.username,
            format!(
                "host login synced from what login used (0.45.5 AM1): role {} -> {}, rooms {} -> {}",
                have_role.as_deref().unwrap_or("none"),
                want_role.as_deref().unwrap_or("none"),
                have_scope.as_deref().unwrap_or("none"),
                want_scope.as_deref().unwrap_or("none"),
            ),
        ));
    }
    Ok(audit)
}

/// Step 2 (S24-3): the App a login session is bound to, and who wrote a
/// grant. Both nullable; old session rows (NULL) read as `host`.
fn step2_app_columns(conn: &Connection) -> Result<(), String> {
    if !column_exists(conn, "tokens", "app_service_id")? {
        conn.execute("ALTER TABLE tokens ADD COLUMN app_service_id TEXT", [])
            .map_err(|e| format!("skin.db step 2 tokens: {e}"))?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS tokens_principal_app ON tokens(principal_id, app_service_id);",
    )
    .map_err(|e| format!("skin.db step 2 index: {e}"))?;
    if !column_exists(conn, "grants", "created_by")? {
        conn.execute("ALTER TABLE grants ADD COLUMN created_by TEXT", [])
            .map_err(|e| format!("skin.db step 2 grants: {e}"))?;
    }
    Ok(())
}

/// Step 3 (S24-10): mark the `--skin` App backfill pending. It needs
/// `published_services` (k2so.db), so the daemon runs it at boot
/// ([`run_skin_app_backfill`]) before any helper starts.
fn step3_mark_app_backfill(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skin_app_backfill (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            pending INTEGER NOT NULL,
            done_at INTEGER,
            summary TEXT
         );
         INSERT OR IGNORE INTO skin_app_backfill (id, pending) VALUES (1, 1);",
    )
    .map_err(|e| format!("skin.db step 3: {e}"))
}

/// Step 4 (runtime S14): `tokens.link_id`, the App link a pass is bound
/// to. NULL on every existing row (no existing pass is a link pass).
fn step4_link_column(conn: &Connection) -> Result<(), String> {
    if !column_exists(conn, "tokens", "link_id")? {
        conn.execute("ALTER TABLE tokens ADD COLUMN link_id TEXT", [])
            .map_err(|e| format!("skin.db step 4 tokens: {e}"))?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS tokens_app_link ON tokens(app_service_id, link_id);")
        .map_err(|e| format!("skin.db step 4 index: {e}"))
}

struct TokenRebuildRow {
    id: String,
    principal_id: Option<String>,
    key_hash: String,
    key_prefix: String,
    caps: String,
    rooms: String,
    created_at: i64,
    revoked_at: Option<i64>,
    session: i64,
    expires_at: Option<i64>,
    username: Option<String>,
}

fn rebuild_tokens_table_if_needed(conn: &mut Connection) -> Result<(), String> {
    if !tokens_principal_id_notnull(conn)? {
        return Ok(());
    }
    let tx = conn
        .transaction()
        .map_err(|e| format!("skin.db migrate begin: {e}"))?;
    let mut stmt = tx
        .prepare(
            "SELECT t.id, t.principal_id, t.key_hash, t.key_prefix, t.caps,
                    COALESCE(t.rooms, '[]'), t.created_at, t.revoked_at,
                    COALESCE(t.session, 0), t.expires_at, p.username
             FROM tokens t
             LEFT JOIN principals p ON p.id = t.principal_id",
        )
        .map_err(|e| format!("skin.db migrate select: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(TokenRebuildRow {
                id: r.get(0)?,
                principal_id: r.get(1)?,
                key_hash: r.get(2)?,
                key_prefix: r.get(3)?,
                caps: r.get(4)?,
                rooms: r.get(5)?,
                created_at: r.get(6)?,
                revoked_at: r.get(7)?,
                session: r.get(8)?,
                expires_at: r.get(9)?,
                username: r.get(10)?,
            })
        })
        .map_err(|e| format!("skin.db migrate select: {e}"))?;
    let mut old = Vec::new();
    for row in rows {
        old.push(row.map_err(|e| format!("skin.db migrate row: {e}"))?);
    }
    drop(stmt);

    tx.execute_batch(
        "CREATE TABLE tokens_new (
            id TEXT PRIMARY KEY,
            principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
            name TEXT,
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            caps TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            revoked_at INTEGER,
            session INTEGER NOT NULL DEFAULT 0,
            expires_at INTEGER,
            CHECK (
                (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                OR
                (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
            )
         );",
    )
    .map_err(|e| format!("skin.db migrate create: {e}"))?;

    let mut live_taken: HashSet<String> = HashSet::new();
    for row in &old {
        let (principal_id, name): (Option<String>, Option<String>) = if row.session != 0 {
            let Some(pid) = row
                .principal_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return Err(format!(
                    "skin.db migrate: session=1 token {} missing principal_id",
                    row.id
                ));
            };
            (Some(pid.to_string()), None)
        } else {
            let username = row.username.as_deref().unwrap_or("");
            let live = row.revoked_at.is_none();
            let candidate = migrate_platform_name(username, &row.key_prefix, &live_taken, live)?;
            if live {
                live_taken.insert(candidate.clone());
            }
            (None, Some(candidate))
        };
        tx.execute(
            "INSERT INTO tokens_new
             (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                row.id,
                principal_id,
                name,
                row.key_hash,
                row.key_prefix,
                row.caps,
                row.rooms,
                row.created_at,
                row.revoked_at,
                row.session,
                row.expires_at
            ],
        )
        .map_err(|e| format!("skin.db migrate insert {}: {e}", row.id))?;
    }
    tx.execute_batch("DROP TABLE tokens; ALTER TABLE tokens_new RENAME TO tokens;")
        .map_err(|e| format!("skin.db migrate swap: {e}"))?;
    tx.commit()
        .map_err(|e| format!("skin.db migrate commit: {e}"))?;
    Ok(())
}

fn db() -> &'static Mutex<Option<SkinDb>> {
    static DB: OnceLock<Mutex<Option<SkinDb>>> = OnceLock::new();
    DB.get_or_init(|| Mutex::new(None))
}

fn with_conn<T, F: FnOnce(&Connection) -> Result<T, String>>(f: F) -> Result<T, String> {
    with_conn_inner(|c| f(c))
}

/// [`with_conn`] with `&mut`: tests that set a hook (rusqlite takes `&mut` for
/// those).
#[cfg(test)]
fn with_conn_mut<T, F: FnOnce(&mut Connection) -> Result<T, String>>(f: F) -> Result<T, String> {
    with_conn_inner(f)
}

fn with_conn_inner<T, F: FnOnce(&mut Connection) -> Result<T, String>>(f: F) -> Result<T, String> {
    let path = store_path();
    let mut guard = db().lock();
    let reopen = match guard.as_ref() {
        Some(existing) => existing.path != path,
        None => true,
    };
    if reopen {
        let conn = open_db(&path)?;
        *guard = Some(SkinDb {
            path: path.clone(),
            conn,
        });
    }
    let conn = &mut guard.as_mut().expect("skin db opened").conn;
    let held = ties::HeldGuard::enter();
    let out = f(conn);
    // Under the mutex: whoever sees the generation move and then reads, reads
    // the write. On an `Err` too: a closure can autocommit, then fail.
    let moved = access_live::settle();
    drop(held);
    drop(guard);
    // The mutex isn't re-entrant, and the listener may hear from anywhere.
    if moved {
        access_live::tell();
    }
    out
}

/// Moves after any write to an access table ([`access_live::ACCESS_TABLES`])
/// through this process's connection, and on [`access_live::hint`]. An open socket
/// compares it before each frame and resolves its pass only when it moved, so a
/// quiet box costs one atomic load per frame and a revoke stops the very next
/// frame. A failed login or a new session doesn't move it, so an outsider can't.
/// A rollback or someone else's change does: that costs each socket one extra
/// resolve, never a wrong answer. Not a proof of access: it can't see expiry or the
/// other daemon during a handoff, so sockets also resolve on a tick.
pub fn write_generation() -> u64 {
    access_live::generation()
}

/// Close the cached skin.db connection (the next call opens it again). For
/// a test that removes the home it pointed at: Windows can't delete a
/// database file that is still open.
pub fn close_cached_connection() {
    *db().lock() = None;
}

/// The one skin.db connection, for [`crate::tool_hook`] (same mutex as
/// every `skin::*` function; not reentrant).
pub(crate) fn with_skin_conn<T, F: FnOnce(&Connection) -> Result<T, String>>(
    f: F,
) -> Result<T, String> {
    with_conn(f)
}

// ── Principals ───────────────────────────────────────────────────────

pub fn add_principal(username: &str) -> Result<SkinPrincipal, String> {
    add_principal_with_email(username, None)
}

/// Create-only. `email` None/empty → NULL. Invalid or duplicate → Err (400).
pub fn add_principal_with_email(
    username: &str,
    email: Option<&str>,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let email = match email {
        Some(raw) => normalize_email(raw)?,
        None => None,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    let r = with_conn(|conn| {
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM principals WHERE username = ?1",
                params![username],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin principal lookup: {e}"))?;
        if exists.is_some() {
            return Err(format!("skin user '{username}' already exists"));
        }
        if let Some(ref em) = email {
            ensure_email_free(conn, em, None)?;
        }
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin principal begin: {e}"))?;
        tx.execute(
            "INSERT INTO principals (id, username, created_at, email) VALUES (?1, ?2, ?3, ?4)",
            params![id, username, created_at, email],
        )
        .map_err(|e| unique_email_err(email.as_deref(), e, "skin principal insert"))?;
        // The one-time copy already ran in open_db and did not see this row.
        // Unassigned host grant: scope is `default_rooms` (`[]`).
        insert_host_grant(&tx, &id, None, Some("[]".to_string()))?;
        tx.commit()
            .map_err(|e| format!("skin principal commit: {e}"))?;
        Ok(SkinPrincipal {
            id,
            username,
            created_at,
            default_rooms: Vec::new(),
            default_room_handles: Vec::new(),
            has_password: false,
            role_id: None,
            role_name: None,
            email,
            full_name: None,
            agent_workspace: None,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    // App user change (Sew rev 147 / K31 seam).
    crate::tool_hook::kick_reconcile();
    members::changed(None);
    Ok(r)
}

const PRINCIPAL_SELECT: &str = "SELECT p.id, p.username, p.created_at, p.default_rooms,
                p.password_hash, p.role_id, r.name, p.email, p.full_name, p.agent_workspace
         FROM principals p
         LEFT JOIN roles r ON r.id = p.role_id";

fn email_from_stored(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn map_principal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinPrincipal> {
    let rooms_raw: String = r.get(3)?;
    let hash: Option<String> = r.get(4)?;
    let role_id: Option<String> = r.get(5)?;
    let role_name: Option<String> = r.get(6)?;
    let email: Option<String> = r.get(7)?;
    let full_name: Option<String> = r.get(8)?;
    let agent_workspace: Option<String> = r.get(9)?;
    Ok(SkinPrincipal {
        id: r.get(0)?,
        username: r.get(1)?,
        created_at: r.get(2)?,
        default_rooms: rooms_from_json(&rooms_raw),
        default_room_handles: Vec::new(),
        has_password: password_is_set(hash.as_deref()),
        role_id: role_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        role_name: role_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        email: email_from_stored(email),
        full_name: full_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        agent_workspace: agent_workspace
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        agent_handle: None,
    })
}

fn unique_email_err(email: Option<&str>, e: rusqlite::Error, ctx: &str) -> String {
    let s = e.to_string();
    if s.contains("principals_email") || s.contains("principals.email") {
        match email {
            Some(em) => format!("email '{em}' already in use"),
            None => "email already in use".to_string(),
        }
    } else {
        format!("{ctx}: {e}")
    }
}

fn ensure_email_free(
    conn: &Connection,
    email: &str,
    except_id: Option<&str>,
) -> Result<(), String> {
    let taken: i64 = match except_id {
        Some(id) => conn
            .query_row(
                "SELECT COUNT(*) FROM principals
                 WHERE email = ?1 COLLATE NOCASE
                   AND email IS NOT NULL AND trim(email) != ''
                   AND id != ?2",
                params![email, id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin email lookup: {e}"))?,
        None => conn
            .query_row(
                "SELECT COUNT(*) FROM principals
                 WHERE email = ?1 COLLATE NOCASE
                   AND email IS NOT NULL AND trim(email) != ''",
                params![email],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin email lookup: {e}"))?,
    };
    if taken > 0 {
        return Err(format!("email '{email}' already in use"));
    }
    Ok(())
}

pub fn list_principals() -> Result<Vec<SkinPrincipal>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "{PRINCIPAL_SELECT} ORDER BY p.username COLLATE NOCASE"
            ))
            .map_err(|e| format!("skin principal list: {e}"))?;
        let rows = stmt
            .query_map([], map_principal_row)
            .map_err(|e| format!("skin principal list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin principal row: {e}"))?);
        }
        Ok(out)
    })
    .map(|users| users.into_iter().map(attach_principal_handles).collect())
}

pub fn remove_principal(username: &str) -> Result<bool, String> {
    remove_principal_as(username, crate::tool_hook::ACTOR_SYSTEM)
}

/// Remove a principal in ONE transaction (S27 §14.2): its session tokens,
/// every grant row, and for each hooked grant (chat) the cascade
/// (assistant grants pinned to them) plus a `deprovision` outbox row.
pub fn remove_principal_as(username: &str, actor: &str) -> Result<bool, String> {
    use crate::tool_hook::{outbox, registry, Cause, HookCtx, HookEvent};
    let username = normalize_username(username)?;
    let actor = hook_actor(Some(actor));
    let ctx = HookCtx {
        actor: actor.clone(),
        ..Default::default()
    };
    let now = now_secs();
    let mut removed_email: Option<String> = None;
    let (r, enqueued) = with_conn(|conn| {
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT id, email FROM principals WHERE username = ?1",
                params![username],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("skin principal lookup: {e}"))?;
        let Some((id, email)) = row else {
            return Ok((false, 0));
        };
        removed_email = email;
        // Email Notifications E16: their proven addresses go with them.
        conn.execute("DELETE FROM principal_email_proven WHERE principal_id = ?1", params![id])
            .map_err(|e| format!("skin principal proven delete: {e}"))?;
        // S28 §12: the person's Sew sessions end with them. Check the chat
        // grant before the grants go, in the same Immediate transaction as
        // the deletes and the S27 outbox rows.
        let tx = immediate_tx(conn)?;
        let had_chat = principal_has_chat_grant_conn(&tx, &id)?;
        tx.execute(
            "DELETE FROM tokens WHERE principal_id = ?1 AND session = 1",
            params![id],
        )
        .map_err(|e| format!("skin session delete: {e}"))?;
        let mut ops = Vec::new();
        for p in registry() {
            let mut stmt = tx
                .prepare(&format!(
                    "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = ?2"
                ))
                .map_err(|e| format!("skin principal grants: {e}"))?;
            let hooked: Vec<SkinGrant> = stmt
                .query_map(params![id, p.kind()], map_grant_row)
                .map_err(|e| format!("skin principal grants: {e}"))?
                .collect::<Result<_, _>>()
                .map_err(|e| format!("skin principal grant row: {e}"))?;
            drop(stmt);
            for g in &hooked {
                let ev = HookEvent::GrantDeleted { grant: g };
                for c in p.cascade(&tx, &ev)? {
                    tx.execute("DELETE FROM grants WHERE id = ?1", params![c.id])
                        .map_err(|e| format!("skin grant cascade delete: {e}"))?;
                    ops.extend(p.plan(&tx, &HookEvent::GrantDeleted { grant: &c }, &ctx)?);
                }
                for mut op in p.plan(&tx, &ev, &ctx)? {
                    op.cause = Cause::PrincipalDelete;
                    ops.push(op);
                }
            }
        }
        // No FK from grants to principals. Leave the row and app detail
        // would keep showing a raw uuid.
        tx.execute(
            "DELETE FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1",
            params![id],
        )
        .map_err(|e| format!("skin principal grants delete: {e}"))?;
        let n = tx
            .execute("DELETE FROM principals WHERE id = ?1", params![id])
            .map_err(|e| format!("skin principal delete: {e}"))?;
        if n > 0 && had_chat {
            crate::sew_revoke::enqueue(
                &tx,
                &crate::sew_revoke::SewSessionRevoke::ExternalId(crate::sew_revoke::principal_external_id(&id)),
            )?;
        }
        let enqueued = outbox::enqueue(&tx, &ops, &actor, now)?.len();
        hook_failpoint()?;
        tx.commit()
            .map_err(|e| format!("skin principal remove commit: {e}"))?;
        Ok((n > 0, enqueued))
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
        // App user change (rev 147 / K31 seam).
        crate::tool_hook::kick_reconcile();
        // Email Notifications E22: the removed person's address is blanked
        // in the send log (k2.db). The rows and counts stay, so removing a
        // user never gives an App budget back (security review H1).
        if let Some(email) = removed_email.as_deref().filter(|e| !e.trim().is_empty()) {
            let db = crate::db::shared();
            let conn = db.lock();
            if let Err(e) = crate::email_notify::purge_recipient(&conn, email) {
                crate::log_debug!("[skin] WARN email log purge for a removed person: {e}");
            }
            // EN-A21: their delivery outcomes go too (a suppression, a hash
            // only, stays).
            if let Err(e) = crate::email_outcomes::forget_recipient(&conn, email) {
                crate::log_debug!("[skin] WARN email outcomes purge for a removed person: {e}");
            }
        }
    }
    if enqueued > 0 {
        crate::tool_hook::kick();
    }
    members::changed(None);
    Ok(r)
}

/// After a principal's email / full name change: enqueue the chat
/// profile `update` in the same transaction (no-op without a chat grant).
fn plan_principal_changed(
    tx: &Connection,
    principal: &SkinPrincipal,
    actor: &str,
) -> Result<usize, String> {
    use crate::tool_hook::{outbox, registry, HookCtx, HookEvent};
    let ctx = HookCtx {
        actor: actor.to_string(),
        ..Default::default()
    };
    let mut ops = Vec::new();
    for p in registry() {
        ops.extend(p.plan(tx, &HookEvent::PrincipalChanged { principal }, &ctx)?);
    }
    Ok(outbox::enqueue(tx, &ops, actor, now_secs())?.len())
}

/// Sew for Apps A3: a profile write (display name) changed what Sew shows
/// for `principal_id`: enqueue the chat `update` on the caller's open
/// transaction (no-op without a chat grant). Returns the rows queued.
pub(crate) fn plan_principal_changed_by_id(tx: &Connection, principal_id: &str, actor: &str) -> Result<usize, String> {
    let p = tx
        .query_row(&format!("{PRINCIPAL_SELECT} WHERE p.id = ?1"), params![principal_id], map_principal_row)
        .optional()
        .map_err(|e| format!("skin principal lookup: {e}"))?;
    match p {
        Some(p) => plan_principal_changed(tx, &p, actor),
        None => Ok(0),
    }
}

fn principal_by_username(
    conn: &Connection,
    username: &str,
) -> Result<Option<SkinPrincipal>, String> {
    conn.query_row(
        &format!("{PRINCIPAL_SELECT} WHERE p.username = ?1"),
        params![username],
        map_principal_row,
    )
    .optional()
    .map_err(|e| format!("skin principal lookup: {e}"))
}

fn attach_principal_handles(mut p: SkinPrincipal) -> SkinPrincipal {
    p.default_room_handles = handles_for_project_ids(&p.default_rooms);
    p.agent_handle = p
        .agent_workspace
        .as_ref()
        .and_then(|id| handles_for_project_ids(std::slice::from_ref(id)).into_iter().next());
    p
}

fn attach_token_handles(mut meta: SkinTokenMeta) -> SkinTokenMeta {
    meta.room_handles = handles_for_project_ids(&meta.rooms);
    meta
}

// ── Tokens ───────────────────────────────────────────────────────────

/// Mint a **platform** pass. Returns `(meta, raw_secret)` — the secret is shown
/// once. No principal lookup. `name` is a box-unique label (not a guest).
/// `rooms` are already-canonical `project_id` UUIDs. Empty → error (R5).
pub fn create_token(
    name: &str,
    caps: Option<&[String]>,
    rooms: &[String],
) -> Result<(SkinTokenMeta, String), String> {
    let name = normalize_username(name)?;
    if name == "owner" {
        return Err("platform token name 'owner' is reserved".to_string());
    }
    let caps = parse_caps(caps)?;
    let rooms = normalize_room_ids(rooms);
    if rooms.is_empty() {
        return Err("rooms must include at least one workspace".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    debug_assert!(
        raw.starts_with(SKIN_KEY_PREFIX) && !raw.starts_with("k2sk_"),
        "skin prefix must be k2skn_, never k2sk_"
    );
    let key_hash = sha256_hex(&raw);
    let prefix = display_prefix(&raw);
    let created_at = now_secs();
    let caps_stored = caps_json(&caps);
    let rooms_stored = rooms_json(&rooms);
    let r = with_conn(|conn| {
        let taken: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tokens
                 WHERE session = 0 AND revoked_at IS NULL AND name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin token name lookup: {e}"))?;
        if taken > 0 {
            return Err(format!("platform token name '{name}' already exists"));
        }
        conn.execute(
            "INSERT INTO tokens
             (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, NULL)",
            params![id, name, key_hash, prefix, caps_stored, rooms_stored, created_at],
        )
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("tokens_platform_name_live") || s.contains("UNIQUE") {
                format!("platform token name '{name}' already exists")
            } else {
                format!("skin token insert: {e}")
            }
        })?;
        Ok((
            SkinTokenMeta {
                id,
                name,
                prefix,
                caps,
                rooms,
                room_handles: Vec::new(),
                room_access: Vec::new(),
                created_at,
                revoked_at: None,
            },
            raw,
        ))
    })?;
    Ok((attach_token_handles(r.0), r.1))
}

/// Box login ([`HOST_APP_ID`]). Same as [`create_app_session_token`] with
/// `host`; kept for the leftover direct door and existing callers.
pub fn create_session_token(username: &str) -> Result<(SkinTokenMeta, String), String> {
    create_app_session_token(username, HOST_APP_ID).map(|s| (s.meta, s.raw))
}

/// A minted login session (S24-4). `raw` is the `k2skn_` secret, shown once.
#[derive(Debug, Clone)]
pub struct SkinAppSession {
    pub meta: SkinTokenMeta,
    pub raw: String,
    /// The App the pass is bound to: `host` or a published service id.
    pub app: String,
    /// The role on that App's grant, or `None` (rooms, Thread-only).
    pub role_name: Option<String>,
    /// What [`attach_session`] fills `meta.room_access` from.
    room_policy: RoomPolicy,
}

/// A minted session's handles and room lines, which read the daemon DB, so
/// after the skin mutex is released (A21), never inside the mint.
fn attach_session(s: SkinAppSession) -> SkinAppSession {
    let mut meta = attach_token_handles(s.meta);
    meta.room_access = room_access_from_policy(&s.room_policy, None);
    SkinAppSession { meta, ..s }
}

/// Empty / missing App → [`HOST_APP_ID`] (a BFF that names no App).
pub fn normalize_app_target(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(HOST_APP_ID)
        .to_string()
}

/// Login session mint for one App (S24-4). Needs an **enabled** `kind=app`
/// grant for `(principal, app)`; otherwise [`HOST_LOGIN_DENIED`] (same text
/// as a bad password: unknown App, no grant and a disabled grant all look
/// alike). The policy comes from that grant: its role's `room_policy`
/// (including `{}`), else its `scope` rooms **including []** with
/// Thread-only caps. Writes `app_service_id`. Do **not** call
/// [`create_token`] (empty rooms 400). `session=1`, `expires_at` = now +
/// [`crate::connect_users::session_ttl_days`].
pub fn create_app_session_token(username: &str, app: &str) -> Result<SkinAppSession, String> {
    let username = normalize_username(username)?;
    let app = normalize_app_target(Some(app));
    // A21: the App's workspace (daemon DB) before the skin mutex.
    let app_ws = ties::app_workspace(&app);
    let r = with_conn(|conn| mint_app_session_conn(conn, &username, &app, app_ws.as_deref()))?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_session(r))
}

/// The mint behind [`create_app_session_token`] on an open connection (so
/// device sign-in, [`app_login`], mints and rotates in one transaction).
/// `username` and `app` are already normalized. The caller refreshes the
/// roster and attaches handles.
///
/// `app_ws` is the App's workspace ([`ties::app_workspace`]), looked up by
/// the caller **before** it took the skin mutex. A tied guest
/// ([`ties`]) is held to its ceiling here: an App outside it, or the
/// chat-only box login, is [`HOST_LOGIN_DENIED`]; inside it, only the
/// tie's room without person-only caps.
pub(crate) fn mint_app_session_conn(
    conn: &Connection,
    username: &str,
    app: &str,
    app_ws: Option<&str>,
) -> Result<SkinAppSession, String> {
    let username = username.to_string();
    let app = app.to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    debug_assert!(
        raw.starts_with(SKIN_KEY_PREFIX) && !raw.starts_with("k2sk_"),
        "skin prefix must be k2skn_, never k2sk_"
    );
    let key_hash = sha256_hex(&raw);
    let prefix = display_prefix(&raw);
    let created_at = now_secs();
    let ttl_secs = crate::connect_users::session_ttl_days().saturating_mul(86_400);
    let mut expires_at = created_at.saturating_add(ttl_secs);
    {
        let Some(principal) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        // 0.46.4: a test guest gets a session only for its App, only
        // through that App's link, never past its own end (every door:
        // login, device sign-in, refresh, the browser helper).
        if let Some(t) = test_guests::get_conn(conn, &principal.id)? {
            match test_guests::session_allowed(&t, &app, created_at) {
                Ok(until) => expires_at = expires_at.min(until),
                Err(why) => {
                    crate::log_debug!("[skin] test guest {username}: {why}");
                    return Err(HOST_LOGIN_DENIED.to_string());
                }
            }
        }
        let tie = ties::get_conn(conn, &principal.id)?;
        let (policy, role_name) = match enabled_app_grant(conn, &principal.id, &app)? {
            Some(grant) => grant_login_policy(conn, &grant)?,
            // Sew S28 §13 (G7): chat-only login. A box login (`host`) with
            // no host grant but a `chat` grant gets a session with an
            // EMPTY room policy (Thread, files, tickets, wiki and store
            // stay dark): it exists to mint chat passes. App logins keep
            // needing that App's own grant (rev 147). A tied guest never
            // (a chat grant is box-wide: agent-app-users A3).
            None if app == HOST_APP_ID && tie.is_none() && principal_has_chat_grant_conn(conn, &principal.id)? => {
                (thread_only_policy(&[]), None)
            }
            None => return Err(HOST_LOGIN_DENIED.to_string()),
        };
        let app_rooms = ties::app_rooms_conn(conn, &app)?;
        let policy = match ties::clamp(tie.as_ref(), &app, app_ws, &app_rooms, policy) {
            Ok(p) => p,
            Err(ties::Denied) => {
                crate::log_debug!("[skin] tied guest {username}: {app} is outside the ceiling");
                return Err(HOST_LOGIN_DENIED.to_string());
            }
        };
        let caps = union_caps(&policy);
        let rooms = rooms_from_policy(&policy);
        let caps_stored = caps_json(&caps);
        let rooms_stored = rooms_json(&rooms);
        let policy_stored = room_policy_json(&policy);
        conn.execute(
            "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at, room_policy, app_service_id)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8, ?9, ?10)",
            params![
                id,
                principal.id,
                key_hash,
                prefix,
                caps_stored,
                rooms_stored,
                created_at,
                expires_at,
                policy_stored,
                app
            ],
        )
        .map_err(|e| format!("skin session insert: {e}"))?;
        Ok(SkinAppSession {
            meta: SkinTokenMeta {
                id,
                name: principal.username,
                prefix,
                caps,
                rooms,
                room_handles: Vec::new(),
                room_access: Vec::new(),
                created_at,
                revoked_at: None,
            },
            raw,
            app: app.clone(),
            role_name,
            room_policy: policy,
        })
    }
}

/// The enabled `kind=app` grant of one principal for one App, or `None`.
fn enabled_app_grant(
    conn: &Connection,
    principal_id: &str,
    app: &str,
) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!(
            "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = 'app' AND target_id = ?2 AND enabled != 0"
        ),
        params![principal_id, app],
        map_grant_row,
    )
    .optional()
    .map_err(|e| format!("skin app grant lookup: {e}"))
}

/// One principal's `kind=app` grant for one App (enabled or not).
fn app_grant(
    conn: &Connection,
    principal_id: &str,
    app: &str,
) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!(
            "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = 'app' AND target_id = ?2"
        ),
        params![principal_id, app],
        map_grant_row,
    )
    .optional()
    .map_err(|e| format!("skin app grant lookup: {e}"))
}

/// What a login through `grant` snapshots: its role's per-room policy, or
/// its `scope` rooms with Thread-only caps (a role-less grant, S24-7).
fn grant_login_policy(
    conn: &Connection,
    grant: &SkinGrant,
) -> Result<(RoomPolicy, Option<String>), String> {
    if let Some(rid) = grant.role_id.as_deref() {
        let Some(role) = role_by_id_or_name(conn, rid)? else {
            return Err(format!("unknown skin role '{rid}'"));
        };
        return Ok((role.room_policy, Some(role.name)));
    }
    let rooms = parse_rooms_json(grant.scope.as_deref());
    Ok((thread_only_policy(&rooms), None))
}

/// Login role for the host row. `role_id` is `principals.role_id`.
/// `grant_role_id` is the host grant's `role_id` and is not the login role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLoginDetail {
    pub principal_id: String,
    pub username: String,
    pub role_id: Option<String>,
    pub role_name: Option<String>,
    pub default_rooms: Vec<String>,
    pub grant_role_id: Option<String>,
}

pub fn host_login_detail(username: &str) -> Result<HostLoginDetail, String> {
    let username = normalize_username(username)?;
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let grant_role_id: Option<String> = conn
            .query_row(
                "SELECT role_id FROM grants
                 WHERE subject_kind = 'principal' AND subject_id = ?1
                   AND kind = 'app' AND target_id = ?2",
                params![p.id, HOST_APP_ID],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin host grant lookup: {e}"))?
            .flatten();
        Ok(HostLoginDetail {
            principal_id: p.id,
            username: p.username,
            role_id: p.role_id,
            role_name: p.role_name,
            default_rooms: p.default_rooms,
            grant_role_id: blank_to_none(grant_role_id),
        })
    })
}

/// Set or clear the K2-login password. Empty/`None` → NULL hash (cannot
/// K2-login; platform tokens still work). Revokes **session** passes only.
/// Hashes **before** taking `skin.db` so argon2 never holds the Mutex.
pub fn set_principal_password(
    username: &str,
    password: Option<&str>,
) -> Result<SkinPrincipal, String> {
    set_principal_password_tied(username, password, None, false)
}

/// [`set_principal_password`] that also ties the guest (`tie`: an agent or
/// an App key set it, agent-app-users R2) in the same transaction.
///
/// `must_change` (0.46.6, [`must_change`]): `true` turns the person's
/// must-change-at-next-sign-in flag on in the same transaction (someone
/// else chose this password: K2 generated it, the Owner or an Admin typed
/// it, an agent set it for anyone but its own test guest, an App key set
/// it); `false` clears it. A cleared password (`None`) never sets it.
pub fn set_principal_password_tied(
    username: &str,
    password: Option<&str>,
    tie: Option<ties::TieWrite<'_>>,
    must_change: bool,
) -> Result<SkinPrincipal, String> {
    set_principal_password_inner(username, password, tie, None, must_change)
}

/// A person changes their own password and stays signed in where they
/// are (`password.change {stay: true}`, app-user-profile §3.6.1): every
/// other login session and device sign-in of theirs ends, on every App;
/// the pass `keep_token_id` and the device sign-in it belongs to stay.
pub fn set_principal_password_staying(username: &str, password: &str, keep_token_id: &str) -> Result<SkinPrincipal, String> {
    set_principal_password_inner(username, Some(password), None, Some(keep_token_id), false)
}

fn set_principal_password_inner(
    username: &str,
    password: Option<&str>,
    tie: Option<ties::TieWrite<'_>>,
    keep_token_id: Option<&str>,
    must_change: bool,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    if password.is_some_and(|pw| password_too_long(pw.trim())) {
        return Err(PASSWORD_TOO_LONG.to_string());
    }
    let hash = match password.map(str::trim).filter(|s| !s.is_empty()) {
        Some(pw) => Some(hash_password(pw)?),
        None => None,
    };
    let has_password = hash.is_some();
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let tx = immediate_tx(conn)?;
        if let Some(t) = tie {
            ties::set_conn(&tx, &p.id, t)?;
        }
        tx.execute(
            "UPDATE principals SET password_hash = ?1 WHERE id = ?2",
            params![hash, p.id],
        )
        .map_err(|e| format!("skin password update: {e}"))?;
        // Must-change (0.46.6): on only for a password someone else chose,
        // never for a cleared one. Unused change tokens go either way.
        must_change::set_flag_conn(&tx, &p.id, must_change && has_password)?;
        match keep_token_id {
            Some(keep) => {
                revoke_sessions_for_principal_except(&tx, &p.id, keep, None)?;
            }
            None => {
                revoke_sessions_for_principal(&tx, &p.id)?;
            }
        }
        // U4: a new password voids the person's unused reset and invite
        // links (they went out for the old one); the owner's live reset
        // stays (only its own use, or the owner, ends it).
        invites::void_unused_conn(&tx, &p.id, invites::Void::KeepOwnerReset, None)?;
        tx.commit().map_err(|e| format!("skin password commit: {e}"))?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    })
    .map(attach_principal_handles)
}

/// Set or clear guest email. Empty/`None` → NULL. Invalid or duplicate → Err (400).
pub fn set_principal_email(username: &str, email: Option<&str>) -> Result<SkinPrincipal, String> {
    set_principal_email_as(username, email, crate::tool_hook::ACTOR_SYSTEM)
}

/// [`set_principal_email`] that records `actor` on the chat `update` row.
pub fn set_principal_email_as(
    username: &str,
    email: Option<&str>,
    actor: &str,
) -> Result<SkinPrincipal, String> {
    set_principal_email_tied(username, email, actor, None)
}

/// [`set_principal_email_as`] that also ties the guest (an agent set it,
/// agent-app-users R2) in the same transaction.
pub fn set_principal_email_tied(
    username: &str,
    email: Option<&str>,
    actor: &str,
    tie: Option<ties::TieWrite<'_>>,
) -> Result<SkinPrincipal, String> {
    // The owner (the owner token or an Owner login) setting it confirms the
    // email (an owner reset may mail it again); read before the skin lock.
    let owner_confirms = tie.is_none() && creators::person_may_claim(actor);
    let actor = hook_actor(Some(actor));
    let username = normalize_username(username)?;
    let email = match email {
        Some(raw) => normalize_email(raw)?,
        None => None,
    };
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        if let Some(ref em) = email {
            ensure_email_free(conn, em, Some(&p.id))?;
        }
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin email begin: {e}"))?;
        if let Some(t) = tie {
            ties::set_conn(&tx, &p.id, t)?;
        }
        if owner_confirms {
            ties::confirm_email_conn(&tx, &p.id)?;
        }
        tx.execute(
            "UPDATE principals SET email = ?1 WHERE id = ?2",
            params![email, p.id],
        )
        .map_err(|e| unique_email_err(email.as_deref(), e, "skin email update"))?;
        let out = SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        };
        let n = plan_principal_changed(&tx, &out, &actor)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin email commit: {e}"))?;
        Ok((out, n))
    })
    .map(|(p, n)| {
        if n > 0 {
            crate::tool_hook::kick();
        }
        attach_principal_handles(p)
    })
}

/// Set or clear the guest full name. Empty/`None` → NULL.
/// Does not rewrite live sessions. The HTTP writer sanitizes first
/// (`sanitize_owner_display_name`: cap 64, strip ESC, drop controls, trim).
pub fn set_principal_full_name(
    username: &str,
    full_name: Option<&str>,
) -> Result<SkinPrincipal, String> {
    set_principal_full_name_as(username, full_name, crate::tool_hook::ACTOR_SYSTEM)
}

/// [`set_principal_full_name`] that records `actor` on the chat `update` row.
pub fn set_principal_full_name_as(
    username: &str,
    full_name: Option<&str>,
    actor: &str,
) -> Result<SkinPrincipal, String> {
    let by_self = actor.trim().starts_with(profile::SELF_ACTOR_PREFIX);
    let actor = hook_actor(Some(actor));
    let username = normalize_username(username)?;
    let full_name = full_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    members::notify_ok(with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin full name begin: {e}"))?;
        tx.execute(
            "UPDATE principals SET full_name = ?1 WHERE id = ?2",
            params![full_name, p.id],
        )
        .map_err(|e| format!("skin full name update: {e}"))?;
        // U24: who set it (the person themselves, or the owner / an agent).
        profile::mark_name_conn(&tx, &p.id, by_self)?;
        let out = SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email: p.email,
            full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        };
        let n = plan_principal_changed(&tx, &out, &actor)?;
        hook_failpoint()?;
        tx.commit()
            .map_err(|e| format!("skin full name commit: {e}"))?;
        Ok((out, n))
    }))
    .map(|(p, n)| {
        if n > 0 {
            crate::tool_hook::kick();
        }
        attach_principal_handles(p)
    })
}

/// Owner-only (the HTTP writer gates it): mark a guest principal as the
/// agent of one workspace, or clear the mark with `None`. `workspace` is a
/// handle or a `projects.id`; it is stored as the id. Live sessions keep
/// working; the mark is read on every brokered db call
/// ([`principal_agent_workspace`]), so it applies from the next query.
pub fn set_principal_agent(
    username: &str,
    workspace: Option<&str>,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let workspace_id = match workspace.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(ws) => {
            let ids = resolve_room_tokens(&[ws.to_string()])?;
            match ids.as_slice() {
                [id] => Some(id.clone()),
                _ => return Err(format!("unknown workspace handle {ws:?}")),
            }
        }
    };
    members::notify_ok(with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let tx = immediate_tx(conn)?;
        tx.execute(
            "UPDATE principals SET agent_workspace = ?1 WHERE id = ?2",
            params![workspace_id, p.id],
        )
        .map_err(|e| format!("skin agent mark update: {e}"))?;
        // agent-app-users: an agent's own login is that workspace's to
        // control, so the mark agrees with the tie store (clearing the mark
        // leaves the tie: only an owner reset or claim lifts this one).
        if let Some(ws) = workspace_id.as_deref() {
            ties::set_conn(&tx, &p.id, ties::TieWrite { workspace_id: ws, set_by: "owner-mark", reason: ties::REASON_AGENT_LOGIN })?;
        }
        tx.commit().map_err(|e| format!("skin agent mark commit: {e}"))?;
        Ok(SkinPrincipal {
            agent_workspace: workspace_id,
            agent_handle: None,
            ..p
        })
    }))
    .map(attach_principal_handles)
}

/// The owner's agent mark on one principal (`projects.id`), or `None` for
/// a person. An unknown principal is `None`: the pass resolver already
/// requires a live principal for a login session.
pub fn principal_agent_workspace(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| {
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT agent_workspace FROM principals WHERE id = ?1",
                params![principal_id.trim()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin agent mark lookup: {e}"))?;
        Ok(raw
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    })
}

// ── Sew chat grant + session-revoke ops (S28 §12, K8) ────────────────

fn principal_has_chat_grant_conn(conn: &Connection, principal_id: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM grants
             WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = ?2 AND target_id = ?3 AND enabled != 0",
            params![principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin chat grant lookup: {e}"))?;
    Ok(n > 0)
}

/// Whether a principal holds an enabled `chat` grant on `sew` (S28 §5).
/// Read live on every pass mint, never from a session snapshot.
pub fn principal_has_chat_grant(principal_id: &str) -> Result<bool, String> {
    with_conn(|conn| principal_has_chat_grant_conn(conn, principal_id))
}

/// Test seam: write (or re-enable) a principal's `chat` grant directly,
/// without the grant hook (no chat target, no outbox rows). Tests of the
/// hook itself use [`create_grant`] with kind `chat`.
#[cfg(any(test, feature = "test-util"))]
pub fn insert_chat_grant_for_tests(principal_id: &str) -> Result<(), String> {
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE grants SET enabled = 1
                 WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = ?2 AND target_id = ?3",
                params![principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
            )
            .map_err(|e| format!("test chat grant enable: {e}"))?;
        if n == 0 {
            conn.execute(
                "INSERT INTO grants (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                 VALUES (?1, 'principal', ?2, ?3, ?4, NULL, NULL, ?5, 1, 'test')",
                params![uuid::Uuid::new_v4().to_string(), principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET, now_secs()],
            )
            .map_err(|e| format!("test chat grant insert: {e}"))?;
        }
        Ok(())
    })
}

/// Test seam: delete a principal's `chat` grant directly (S27 owns the
/// real route).
#[cfg(any(test, feature = "test-util"))]
pub fn delete_chat_grant_for_tests(principal_id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "DELETE FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = ?2 AND target_id = ?3",
            params![principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
        )
        .map_err(|e| format!("test chat grant delete: {e}"))?;
        Ok(())
    })
}

/// An Immediate transaction on the shared connection (S28-A3: `with_conn`
/// alone is autocommit). The revoke write and its Sew outbox op commit or
/// roll back together. Never nested: only the public entry points open one.
fn immediate_tx(conn: &Connection) -> Result<rusqlite::Transaction<'_>, String> {
    rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("skin begin immediate: {e}"))
}

/// [`with_conn`] inside one Immediate transaction: commits when `f`
/// succeeds, rolls back (the transaction drops) when it fails. For the
/// grant writes that end sessions, so their Sew ops ride the same commit.
fn with_immediate<T, F: FnOnce(&Connection) -> Result<T, String>>(f: F) -> Result<T, String> {
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let r = f(&tx)?;
        tx.commit().map_err(|e| format!("skin commit: {e}"))?;
        Ok(r)
    })
}

/// Queue `sew_session_revoke` ops that did not come from a `skin.db` write
/// (Connect sessions live in a JSON file, S28 §12). No-op when there is
/// no `skin.db` (so no chat target), so a box without Sew never creates
/// `skin.db` for this.
pub fn enqueue_sew_revokes(ops: &[crate::sew_revoke::SewSessionRevoke]) -> Result<(), String> {
    if ops.is_empty() || !crate::sew_revoke::writer_available() {
        for op in ops {
            crate::log_debug!("[skin] sew_session_revoke {} not queued: Sew is not set up on this box", op.label());
        }
        return Ok(());
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        for op in ops {
            crate::sew_revoke::enqueue(&tx, op)?;
        }
        tx.commit().map_err(|e| format!("skin sew revoke commit: {e}"))
    })
}

/// Live (not yet revoked) login-session rows matching `filter`, as
/// `(token id, principal id)`. `filter` is a fixed SQL fragment over
/// `tokens` with `?1`.. bound from `args`.
fn live_session_rows(
    conn: &Connection,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<(String, String)>, String> {
    let sql = format!(
        "SELECT id, principal_id FROM tokens
         WHERE session = 1 AND revoked_at IS NULL AND principal_id IS NOT NULL AND ({filter})"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("skin session list: {e}"))?;
    let rows = stmt
        .query_map(args, |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("skin session list: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("skin session list: {e}"))
}

/// One `{k2_sid: skn_<id>}` op per ended session whose principal holds a
/// `chat` grant at this moment (S28 §12). `conn` is inside the revoke's
/// transaction.
fn enqueue_session_revokes(conn: &Connection, ended: &[(String, String)]) -> Result<(), String> {
    let mut chat: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    // prd-custom-app-push-v1 CP6: a push device registered by an ended
    // pass (and not tied to a live device sign-in) goes in the same
    // transaction.
    let mut pruned: HashSet<&str> = HashSet::new();
    for (_, principal_id) in ended {
        if pruned.insert(principal_id.as_str()) {
            app_push::prune_principal_conn(conn, principal_id)?;
        }
    }
    for (token_id, principal_id) in ended {
        let has = match chat.get(principal_id.as_str()) {
            Some(v) => *v,
            None => {
                let v = principal_has_chat_grant_conn(conn, principal_id)?;
                chat.insert(principal_id.as_str(), v);
                v
            }
        };
        if has {
            crate::sew_revoke::enqueue(conn, &crate::sew_revoke::SewSessionRevoke::K2Sid(crate::sew_revoke::skin_sid(token_id)))?;
        }
    }
    Ok(())
}

/// Every live login session of one principal ends (password set, change
/// or reset; box login off; a non-App grant removed). Sew ends every
/// session of `k2:principal:<id>` (one op), when the principal has a
/// `chat` grant and at least one session ended.
fn revoke_sessions_for_principal(conn: &Connection, principal_id: &str) -> Result<usize, String> {
    let n = conn
        .execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE principal_id = ?2 AND session = 1 AND revoked_at IS NULL",
            params![now_secs(), principal_id],
        )
        .map_err(|e| format!("skin session revoke: {e}"))?;
    // A device sign-in ends with the person's sessions (app-mobile-access-v1).
    app_login::end_chains_where(conn, "principal_id = ?3", &[&principal_id], app_login::REASON_SESSIONS_ENDED)?;
    // Their push devices go with the sessions (prd-custom-app-push-v1 CP6).
    app_push::prune_principal_conn(conn, principal_id)?;
    if n > 0 && principal_has_chat_grant_conn(conn, principal_id)? {
        crate::sew_revoke::enqueue(
            conn,
            &crate::sew_revoke::SewSessionRevoke::ExternalId(crate::sew_revoke::principal_external_id(principal_id)),
        )?;
    }
    Ok(n)
}

/// End every live login session and device sign-in of one principal but
/// the pass `keep_token_id` and the device sign-in it belongs to:
/// `app = None` on every App (password change with `stay`), `Some(app)`
/// on that App only (`profile.signOutOthers`). Sew ends exactly the
/// sessions that ended (`skn_<id>` each), never the kept one; push devices
/// of the ended passes and sign-ins go too. Returns how many ended
/// (sessions + device sign-ins).
pub(crate) fn revoke_sessions_for_principal_except(
    conn: &Connection,
    principal_id: &str,
    keep_token_id: &str,
    app: Option<&str>,
) -> Result<usize, String> {
    let keep_chain: String = conn
        .query_row(
            "SELECT id FROM device_logins WHERE session_token_id = ?1 AND principal_id = ?2 AND revoked_at IS NULL",
            params![keep_token_id, principal_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("skin session keep: {e}"))?
        .unwrap_or_default();
    let app = app.unwrap_or("");
    let ended = live_session_rows(
        conn,
        "principal_id = ?1 AND id != ?2 AND (?3 = '' OR COALESCE(app_service_id, ?4) = ?3)",
        &[&principal_id, &keep_token_id, &app, &HOST_APP_ID],
    )?;
    let n = conn
        .execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE principal_id = ?2 AND session = 1 AND revoked_at IS NULL AND id != ?3
               AND (?4 = '' OR COALESCE(app_service_id, ?5) = ?4)",
            params![now_secs(), principal_id, keep_token_id, app, HOST_APP_ID],
        )
        .map_err(|e| format!("skin session revoke: {e}"))?;
    let chains = app_login::end_chains_where(
        conn,
        "principal_id = ?3 AND id != ?4 AND (?5 = '' OR app_id = ?5)",
        &[&principal_id, &keep_chain, &app],
        app_login::REASON_SESSIONS_ENDED,
    )?;
    enqueue_session_revokes(conn, &ended)?;
    app_push::prune_principal_conn(conn, principal_id)?;
    Ok(n + chains)
}

/// End one principal's live login sessions bound to one App (S24-6). An old
/// session row with no App is a `host` session. Sew ends the chat
/// sessions minted from exactly those (`skn_<id>` each).
fn revoke_sessions_for_principal_app(
    conn: &Connection,
    principal_id: &str,
    app: &str,
) -> Result<usize, String> {
    let ended = live_session_rows(
        conn,
        "principal_id = ?1 AND COALESCE(app_service_id, ?2) = ?3",
        &[&principal_id, &HOST_APP_ID, &app],
    )?;
    let n = conn
        .execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE principal_id = ?2 AND session = 1 AND revoked_at IS NULL
               AND COALESCE(app_service_id, ?3) = ?4",
            params![now_secs(), principal_id, HOST_APP_ID, app],
        )
        .map_err(|e| format!("skin session revoke: {e}"))?;
    app_login::end_chains_where(conn, "principal_id = ?3 AND app_id = ?4", &[&principal_id, &app], app_login::REASON_SESSIONS_ENDED)?;
    enqueue_session_revokes(conn, &ended)?;
    Ok(n)
}

/// S24-20: a published service went away (`publish rm`). End every live
/// login session bound to it. Its grants stay (Settings shows "App
/// removed") until S26 decides cleanup. Returns how many ended.
pub fn revoke_sessions_for_app(service_id: &str) -> Result<usize, String> {
    let service_id = service_id.trim();
    if service_id.is_empty() || service_id == HOST_APP_ID {
        return Err("revoke sessions: need a published service id".to_string());
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let ended = live_session_rows(&tx, "app_service_id = ?1", &[&service_id])?;
        let n = tx
            .execute(
                "UPDATE tokens SET revoked_at = ?1
                 WHERE session = 1 AND revoked_at IS NULL AND app_service_id = ?2",
                params![now_secs(), service_id],
            )
            .map_err(|e| format!("skin session revoke: {e}"))?;
        app_login::end_chains_where(&tx, "app_id = ?3", &[&service_id], app_login::REASON_SESSIONS_ENDED)?;
        enqueue_session_revokes(&tx, &ended)?;
        tx.commit().map_err(|e| format!("skin session revoke commit: {e}"))?;
        Ok(n)
    })
}

/// 60s per lowercase/trim username string (even when no user). True = 429.
/// Does not look up principals. Call after auth 403, before mint lookup.
///
/// Memory only (vs-live U9): an unauthenticated request never writes a
/// skin.db row for the name it typed (`password_reset_throttle` stays for
/// an older K2 and is no longer written). Restart-resets, bounded.
pub fn password_reset_throttled(username: &str) -> Result<bool, String> {
    let key = username.trim().to_ascii_lowercase();
    Ok(guard::forgot_throttled(&key, RESET_THROTTLE_SECS))
}

/// Privileged forgot hit. Miss is `Ok(None)` (HTTP `{ok:true}` no extra keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinResetMint {
    pub token: String,
    pub expires_at: i64,
    pub email: String,
}

/// Mint a one-hour consume token. `username` field matches username **or**
/// email (`@` → email lookup). Token only if password **and** populated
/// email. Unknown / invalid / no hash / no email → dummy argon2 and
/// `Ok(None)`. Invalid email string is a miss, not 400. Does **not** rate-limit.
pub fn mint_password_reset(username: &str) -> Result<Option<SkinResetMint>, String> {
    let presented = username.trim().to_ascii_lowercase();
    let stored = with_conn(|conn| lookup_reset_principal(conn, &presented))?;
    match stored {
        Some((pid, hash, email))
            if password_is_set(hash.as_deref()) && email_from_stored(email.clone()).is_some() =>
        {
            let email = email_from_stored(email).expect("populated email");
            let raw = generate_reset_raw();
            let token_hash = sha256_hex(&raw);
            let now = now_secs();
            let expires = now.saturating_add(RESET_TTL_SECS);
            let id = uuid::Uuid::new_v4().to_string();
            with_conn(|conn| {
                conn.execute(
                    "DELETE FROM password_reset_tokens
                     WHERE principal_id = ?1 AND used_at IS NULL",
                    params![pid],
                )
                .map_err(|e| format!("skin reset replace unused: {e}"))?;
                conn.execute(
                    "INSERT INTO password_reset_tokens
                     (id, principal_id, token_hash, expires_at, used_at, created_at)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                    params![id, pid, token_hash, expires, now],
                )
                .map_err(|e| format!("skin reset token insert: {e}"))?;
                Ok(())
            })?;
            Ok(Some(SkinResetMint {
                token: raw,
                expires_at: expires,
                email,
            }))
        }
        _ => {
            dummy_verify("k2-skin-dummy-verify-target");
            Ok(None)
        }
    }
}

fn lookup_reset_principal(
    conn: &Connection,
    presented: &str,
) -> Result<Option<(String, Option<String>, Option<String>)>, String> {
    if presented_looks_like_email(presented) {
        let Ok(Some(email)) = normalize_email(presented) else {
            return Ok(None);
        };
        conn.query_row(
            "SELECT id, password_hash, email FROM principals
             WHERE email = ?1 COLLATE NOCASE
               AND email IS NOT NULL AND trim(email) != ''",
            params![email],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin reset mint lookup: {e}"))
    } else {
        conn.query_row(
            "SELECT id, password_hash, email FROM principals WHERE username = ?1",
            params![presented],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin reset mint lookup: {e}"))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SkinResetConsume {
    Ok,
    BadToken,
}

/// Consume a reset token. Hashed lookup, TTL, single-use. Success sets
/// the password and revokes every login session (as
/// [`set_principal_password`] does). Empty password is refused so this
/// door never clears a hash. Does not mint a session.
///
/// 0.46.4 (agent-app-users S0): the use is one compare-and-set inside one
/// IMMEDIATE transaction: `used_at` is stamped only where it is still NULL
/// and the token unexpired, and the password write and session revoke
/// ride the same commit. Two requests presenting the same token at the
/// same moment get exactly one success (the old code checked, set the
/// password, then marked the token used, each under its own lock).
pub fn consume_password_reset(token: &str, password: &str) -> Result<SkinResetConsume, String> {
    let token = token.trim();
    if token.is_empty() {
        dummy_verify(password);
        return Ok(SkinResetConsume::BadToken);
    }
    if password.trim().is_empty() {
        return Err("password must not be empty".to_string());
    }
    if password_too_long(password.trim()) {
        return Err(PASSWORD_TOO_LONG.to_string());
    }
    let token_hash = sha256_hex(token);
    let now = now_secs();
    // U5 (b): look the token up BEFORE hashing. A 256-bit token needs no
    // timing cover, and a junk request must never cost an argon2 run (the
    // route adds its 500 ms on every miss). The compare-and-set below
    // decides; this only spares the hash.
    let live: bool = with_conn(|conn| {
        conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM password_reset_tokens WHERE token_hash = ?1 AND used_at IS NULL AND expires_at > ?2)",
            params![token_hash, now],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin reset consume lookup: {e}"))
    })?;
    if !live {
        return Ok(SkinResetConsume::BadToken);
    }
    // argon2 before the skin mutex, like `set_principal_password`.
    let pw_hash = hash_password(password.trim())?;
    let used = with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT t.id, t.principal_id FROM password_reset_tokens t
                 JOIN principals p ON p.id = t.principal_id
                 WHERE t.token_hash = ?1",
                params![token_hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("skin reset consume lookup: {e}"))?;
        let Some((id, principal_id)) = row else { return Ok(None) };
        let claimed = tx
            .execute(
                "UPDATE password_reset_tokens SET used_at = ?1
                 WHERE id = ?2 AND used_at IS NULL AND expires_at > ?1",
                params![now, id],
            )
            .map_err(|e| format!("skin reset mark used: {e}"))?;
        if claimed != 1 {
            return Ok(None);
        }
        tx.execute("UPDATE principals SET password_hash = ?1 WHERE id = ?2", params![pw_hash, principal_id])
            .map_err(|e| format!("skin password update: {e}"))?;
        // The person chose this password: must-change ends (0.46.6).
        must_change::set_flag_conn(&tx, &principal_id, false)?;
        revoke_sessions_for_principal(&tx, &principal_id)?;
        // U4 / U7: the other unused links go, and the person proved their
        // mailbox, so their login lockout ends, in this same transaction
        // (`clear_login_lockout` would take the skin mutex again).
        invites::void_unused_conn(&tx, &principal_id, invites::Void::KeepOwnerReset, None)?;
        clear_lockouts_for_principal_conn(&tx, &principal_id)?;
        // The person chose this password (a token only the owner is handed).
        let released = ties::release_by_person_conn(&tx, &principal_id, ties::VIA_OWNER_TOKEN)?;
        tx.commit().map_err(|e| format!("skin reset commit: {e}"))?;
        Ok(Some(released))
    })?;
    Ok(match used {
        None => SkinResetConsume::BadToken,
        Some(released) => {
            if let Some(r) = released {
                r.audit();
            }
            SkinResetConsume::Ok
        }
    })
}

/// The dummy argon2 verify a miss spends, for callers that answer a miss
/// without reaching a store lookup that already does (forgot through a
/// pass for a guest it may not touch).
pub fn reset_miss_dummy() {
    dummy_verify("k2-skin-dummy-verify-target");
}

/// What a current-password check found (vs-live U5 a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentPassword {
    Ok,
    Wrong,
    /// Too many wrong answers for this person lately: refused before any
    /// argon2 run.
    Throttled { retry_after_secs: i64 },
}

/// Check `password` is the current password of `principal_id`
/// (`username`), counting wrong answers per person ([`guard`]: 5 per 15
/// minutes, then [`CurrentPassword::Throttled`]). Its own counter, never
/// the login lockout.
pub fn check_current_password(principal_id: &str, username: &str, password: &str) -> CurrentPassword {
    if let Some(retry_after_secs) = guard::current_password_blocked(principal_id) {
        return CurrentPassword::Throttled { retry_after_secs };
    }
    if verify_principal_password(username, password) {
        CurrentPassword::Ok
    } else {
        guard::record_wrong_current_password(principal_id);
        CurrentPassword::Wrong
    }
}

/// Verify a guest's current password. Dummy argon2 on miss / NULL hash.
/// Does **not** bump `login_lockouts`.
pub fn verify_principal_password(username: &str, password: &str) -> bool {
    let key = match normalize_username(username) {
        Ok(k) => k,
        Err(_) => {
            dummy_verify(password);
            return false;
        }
    };
    let stored = with_conn(|conn| {
        let hash: Option<Option<String>> = conn
            .query_row(
                "SELECT password_hash FROM principals WHERE username = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin password verify lookup: {e}"))?;
        Ok(hash.flatten().filter(|s| !s.trim().is_empty()))
    })
    .unwrap_or(None);
    match stored.as_deref() {
        Some(h) => verify_hash(password, h),
        None => {
            dummy_verify(password);
            false
        }
    }
}

/// Login gate: lockout then dummy-argon2 verify. Never holds `skin.db`
/// across hashing.
#[derive(Debug)]
pub enum SkinLoginOutcome {
    Ok(SkinPrincipal),
    /// The password is right but someone else chose it (0.46.6,
    /// [`must_change`]): no pass and no session. The caller answers with a
    /// change token ([`must_change::start`]), never a login.
    MustChange(SkinPrincipal),
    BadCreds,
    /// The login is locked (3 bad tries → 15 min). A name that resolves to
    /// a principal (by username OR email) is locked per principal, so the
    /// username and the email share one counter and one lock. A name that
    /// resolves to nothing is counted per presented string and locks the
    /// same way, so "locked" never says which usernames exist.
    LockedOut { retry_after_secs: i64 },
}

/// Presented-string key: trim + ASCII lowercase; an `@` keeps that string;
/// otherwise [`normalize_username`], and on failure the trim+lowercase
/// string. Used for names that resolve to no principal, and the key
/// [`clear_login_lockout`] reports back.
fn login_lockout_key(username: &str) -> String {
    let presented = username.trim().to_ascii_lowercase();
    if presented_looks_like_email(&presented) {
        presented
    } else {
        normalize_username(username).unwrap_or(presented)
    }
}

/// The principal a login name resolves to (by email when it has an `@`,
/// else by username), password or not.
struct LoginPrincipalRow {
    username: String,
    email: Option<String>,
    password_hash: Option<String>,
    /// `principals.password_must_change` (0.46.6).
    must_change: bool,
}

/// Which `login_lockouts` rows one login attempt reads and writes.
///
/// `primary` is the one counter failures bump. For a known principal it is
/// the principal's username: usernames are unique, never renamed, and can
/// never contain `@`, so no presented email can land on it, and typing the
/// username or the email hits the same row. For an unknown name it is the
/// presented-string key, exactly as before.
///
/// `extra` is the principal's email key: rows written there by 0.45.5
/// (which counted the email separately) still lock until they expire, and
/// unlock/success clear them. For an unknown name `extra == primary`, so
/// both paths run the very same queries.
struct LoginLockSlot {
    primary: String,
    extra: String,
}

fn login_lock_slot(presented_key: &str, principal: Option<&LoginPrincipalRow>) -> LoginLockSlot {
    match principal {
        Some(p) => LoginLockSlot {
            primary: p.username.clone(),
            extra: p
                .email
                .as_deref()
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| p.username.clone()),
        },
        None => LoginLockSlot {
            primary: presented_key.to_string(),
            extra: presented_key.to_string(),
        },
    }
}

/// Delete every `login_lockouts` row for this login: for a name that
/// resolves to a principal (username or email) that is the principal's
/// username row, its email row and the presented row; otherwise only the
/// presented row. Does not create a principal, touch `password_hash`, or
/// revoke sessions. Returns the presented key and whether any row was
/// deleted (`false` when none was stored).
/// The username a presented login name resolves to, as login and unlock
/// resolve it: an `@` means email (any case), else username. `None`: no
/// such guest.
pub fn login_principal_username(presented: &str) -> Result<Option<String>, String> {
    let presented = presented.trim().to_ascii_lowercase();
    with_conn(|conn| Ok(lookup_login_principal(conn, &presented)?.map(|p| p.username)))
}

/// Delete every `login_lockouts` row of one principal (its username row
/// and its email row, which is every key a login for it counts on), on
/// the caller's connection: a used reset or invite link (vs-live U7, Q4:
/// the person proved their mailbox). The owner's password set never
/// calls this (Rosson 2026-09-29: set and unlock stay apart).
pub(crate) fn clear_lockouts_for_principal_conn(conn: &Connection, principal_id: &str) -> Result<usize, String> {
    let row: Option<(String, Option<String>)> = conn
        .query_row("SELECT username, email FROM principals WHERE id = ?1", params![principal_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| format!("skin lockout clear: {e}"))?;
    let Some((username, email)) = row else { return Ok(0) };
    let email = email.map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| username.clone());
    conn.execute("DELETE FROM login_lockouts WHERE username IN (?1, ?2)", params![username, email])
        .map_err(|e| format!("skin lockout clear: {e}"))
}


pub fn clear_login_lockout(username: &str) -> Result<(String, bool), String> {
    let key = login_lockout_key(username);
    let presented = username.trim().to_ascii_lowercase();
    let deleted = with_conn(|conn| {
        let principal = lookup_login_principal(conn, &presented)?;
        let slot = login_lock_slot(&key, principal.as_ref());
        let n = conn
            .execute(
                "DELETE FROM login_lockouts WHERE username IN (?1, ?2, ?3)",
                params![key, slot.primary, slot.extra],
            )
            .map_err(|e| format!("skin lockout clear: {e}"))?;
        Ok(n)
    })?;
    Ok((key, deleted > 0))
}

/// Every principal whose login is locked right now, by principal id, with
/// when the lock ends (epoch seconds). The same rows and test the login
/// check reads ([`LoginLockSlot`]): the principal's username row and its
/// email row, `locked_until` in the future. A failed count with no lock
/// is not a lock. For the owner's App users list (its Unlock action).
pub fn active_login_locks() -> Result<std::collections::HashMap<String, i64>, String> {
    let now = now_secs();
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, MAX(l.locked_until) FROM principals p
                 JOIN login_lockouts l
                   ON l.username = p.username
                   OR (p.email IS NOT NULL AND TRIM(p.email) <> '' AND l.username = LOWER(TRIM(p.email)))
                 WHERE l.locked_until > ?1
                 GROUP BY p.id",
            )
            .map_err(|e| format!("skin lockout list: {e}"))?;
        let rows = stmt
            .query_map(params![now], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| format!("skin lockout list: {e}"))?;
        rows.collect::<Result<_, _>>().map_err(|e| format!("skin lockout list: {e}"))
    })
}

/// Who a login name counts against, for a throttle in front of
/// [`check_and_record_login`]: the principal it signs in as (its
/// username, whether the name was the username or the email, in any case),
/// else the presented name as the lockout keys it. 0.46.5 (papercuts
/// review): the App link's per-guest failure count keyed on the raw name,
/// so `ada` and `ada@example.com` (and the JSON `name` key) were two
/// budgets for one guest while the lockout counted them as one.
pub fn login_throttle_subject(presented: &str) -> String {
    let key = login_lockout_key(presented);
    let lowered = presented.trim().to_ascii_lowercase();
    match with_conn(|conn| lookup_login_principal(conn, &lowered)) {
        Ok(Some(p)) => p.username,
        _ => key,
    }
}

pub fn check_and_record_login(username: &str, password: &str) -> SkinLoginOutcome {
    check_and_record_login_hashed(username, password).0
}

/// [`check_and_record_login`] plus the stored hash the password was
/// checked against (on `Ok` and `MustChange` only). The sign-in door
/// (`auth_door`, 0.46.9) keeps it beside its one-time code, so a code
/// dies if the password changes before the code is swapped.
pub fn check_and_record_login_hashed(username: &str, password: &str) -> (SkinLoginOutcome, Option<String>) {
    // Lockout 3/15. `@` → email lookup; else username. Invalid email is a
    // miss, not 400. A name that resolves to a principal is counted on
    // that principal (see [`LoginLockSlot`]); anything else on the
    // presented string. The lock is checked before the password, with the
    // same queries for known and unknown names.
    let presented = username.trim().to_ascii_lowercase();
    let key = login_lockout_key(username);

    let looked_up = with_conn(|conn| {
        let principal = lookup_login_principal(conn, &presented)?;
        let slot = login_lock_slot(&key, principal.as_ref());
        let now = now_secs();
        let until: Option<i64> = conn
            .query_row(
                "SELECT MAX(locked_until) FROM login_lockouts
                 WHERE username IN (?1, ?2) AND locked_until > ?3",
                params![slot.primary, slot.extra, now],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin lockout lookup: {e}"))?;
        if let Some(until) = until {
            return Ok((principal, slot, Some(until - now)));
        }
        conn.execute(
            "UPDATE login_lockouts SET locked_until = NULL, failed_count = 0, last_failed_at = NULL
             WHERE username IN (?1, ?2) AND locked_until IS NOT NULL",
            params![slot.primary, slot.extra],
        )
        .map_err(|e| format!("skin lockout expire: {e}"))?;
        Ok((principal, slot, None))
    });
    let (principal, slot) = match looked_up {
        Ok((_, _, Some(left))) => {
            return (SkinLoginOutcome::LockedOut { retry_after_secs: left.max(1) }, None);
        }
        Ok((principal, slot, None)) => (principal, slot),
        Err(_) => (None, login_lock_slot(&key, None)),
    };

    let ok = match principal.as_ref().and_then(|p| p.password_hash.as_deref()) {
        Some(h) => verify_hash(password, h),
        None => {
            dummy_verify(password);
            false
        }
    };

    let _ = with_conn(|conn| {
        if ok {
            conn.execute(
                "DELETE FROM login_lockouts WHERE username IN (?1, ?2, ?3)",
                params![key, slot.primary, slot.extra],
            )
            .map_err(|e| format!("skin lockout clear: {e}"))?;
        } else {
            let now = now_secs();
            // 0.46.6 (fix 1c): only recent failures add up. A count whose
            // last failure is older than the window (or has no time: a row
            // from before schema step 8) is forgotten.
            let earlier: i64 = conn
                .query_row(
                    "SELECT failed_count, last_failed_at FROM login_lockouts WHERE username = ?1",
                    params![slot.primary],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
                )
                .optional()
                .map_err(|e| format!("skin lockout count: {e}"))?
                .map(|(count, last)| match last {
                    Some(at) if now.saturating_sub(at) <= LOCKOUT_FAIL_WINDOW_SECS => count,
                    _ => 0,
                })
                .unwrap_or(0);
            let failed = earlier + 1;
            if failed >= i64::from(LOCKOUT_THRESHOLD) {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until, last_failed_at)
                     VALUES (?1, 0, ?2, ?3)
                     ON CONFLICT(username) DO UPDATE SET failed_count = 0, locked_until = excluded.locked_until,
                        last_failed_at = excluded.last_failed_at",
                    params![slot.primary, now.saturating_add(LOCKOUT_DURATION_SECS), now],
                )
                .map_err(|e| format!("skin lockout set: {e}"))?;
            } else {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until, last_failed_at)
                     VALUES (?1, ?2, NULL, ?3)
                     ON CONFLICT(username) DO UPDATE SET failed_count = excluded.failed_count, locked_until = NULL,
                        last_failed_at = excluded.last_failed_at",
                    params![slot.primary, failed, now],
                )
                .map_err(|e| format!("skin lockout bump: {e}"))?;
            }
        }
        Ok(())
    });

    if !ok {
        return (SkinLoginOutcome::BadCreds, None);
    }
    let Some(principal) = principal else {
        return (SkinLoginOutcome::BadCreds, None);
    };
    let hash = principal.password_hash.clone();
    match with_conn(|conn| principal_by_username(conn, &principal.username)) {
        Ok(Some(p)) if principal.must_change => (SkinLoginOutcome::MustChange(attach_principal_handles(p)), hash),
        Ok(Some(p)) => (SkinLoginOutcome::Ok(attach_principal_handles(p)), hash),
        _ => (SkinLoginOutcome::BadCreds, None),
    }
}

/// Resolve a presented login name (already trim + lowercase) to its
/// principal: an `@` means email (any case), else username. A malformed
/// email or username is a miss. The password hash is `None` when unset or
/// blank (that principal can never password-login).
fn lookup_login_principal(
    conn: &Connection,
    presented: &str,
) -> Result<Option<LoginPrincipalRow>, String> {
    let row: Option<(String, Option<String>, Option<String>, i64)> =
        if presented_looks_like_email(presented) {
            let Ok(Some(email)) = normalize_email(presented) else {
                return Ok(None);
            };
            conn.query_row(
                "SELECT username, email, password_hash, password_must_change FROM principals
                 WHERE email = ?1 COLLATE NOCASE
                   AND email IS NOT NULL AND trim(email) != ''",
                params![email],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| format!("skin password lookup: {e}"))?
        } else {
            let key = normalize_username(presented).unwrap_or_else(|_| presented.to_string());
            conn.query_row(
                "SELECT username, email, password_hash, password_must_change FROM principals WHERE username = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| format!("skin password lookup: {e}"))?
        };
    Ok(row.map(|(username, email, hash, must_change)| LoginPrincipalRow {
        username,
        email,
        password_hash: hash.filter(|s| !s.trim().is_empty()),
        must_change: must_change != 0,
    }))
}

pub fn list_tokens() -> Result<Vec<SkinTokenMeta>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.name, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
                 FROM tokens t
                 WHERE t.session = 0 AND t.link_id IS NULL
                 ORDER BY t.created_at DESC",
            )
            .map_err(|e| format!("skin token list: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                let caps_raw: String = r.get(3)?;
                let rooms_raw: String = r.get(4)?;
                Ok(SkinTokenMeta {
                    id: r.get(0)?,
                    name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    prefix: r.get(2)?,
                    caps: caps_from_json(&caps_raw),
                    rooms: rooms_from_json(&rooms_raw),
                    room_handles: Vec::new(),
                    room_access: Vec::new(),
                    created_at: r.get(5)?,
                    revoked_at: r.get(6)?,
                })
            })
            .map_err(|e| format!("skin token list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin token row: {e}"))?);
        }
        Ok(out)
    })
    .map(|mut tokens| {
        for t in &mut tokens {
            t.room_handles = handles_for_project_ids(&t.rooms);
        }
        tokens
    })
}

/// Soft-revoke. `Ok(true)` if a live token was just revoked.
pub fn revoke_token(id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing token id".to_string());
    }
    let r = with_conn(|conn| {
        // S28-A3: read the row, revoke it and queue its Sew op in one
        // Immediate transaction; only a login session (`session=1`) whose
        // principal holds a `chat` grant gets an op.
        let tx = immediate_tx(conn)?;
        let ended = live_session_rows(&tx, "id = ?1", &[&id])?;
        let n = tx
            .execute(
                "UPDATE tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                params![now_secs(), id],
            )
            .map_err(|e| format!("skin token revoke: {e}"))?;
        if n > 0 {
            enqueue_session_revokes(&tx, &ended)?;
            // Logout or an owner revoke of a device's pass ends that device
            // sign-in too (its refresh token stops working).
            app_login::end_chains_where(&tx, "session_token_id = ?3", &[&id], app_login::REASON_SELF)?;
        }
        tx.commit().map_err(|e| format!("skin token revoke commit: {e}"))?;
        Ok(n > 0)
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    }
    Ok(r)
}

fn token_meta_from_row(conn: &Connection, id: &str) -> Result<Option<SkinTokenMeta>, String> {
    conn.query_row(
        "SELECT t.id, t.session, t.name, p.username, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
         FROM tokens t
         LEFT JOIN principals p ON p.id = t.principal_id
         WHERE t.id = ?1",
        params![id],
        |r| {
            let session: i64 = r.get(1)?;
            let token_name: Option<String> = r.get(2)?;
            let principal_username: Option<String> = r.get(3)?;
            let stamp = if session != 0 {
                principal_username.unwrap_or_default()
            } else {
                token_name.unwrap_or_default()
            };
            let caps_raw: String = r.get(5)?;
            let rooms_raw: String = r.get(6)?;
            Ok(SkinTokenMeta {
                id: r.get(0)?,
                name: stamp,
                prefix: r.get(4)?,
                caps: caps_from_json(&caps_raw),
                rooms: rooms_from_json(&rooms_raw),
                room_handles: Vec::new(),
                room_access: Vec::new(),
                created_at: r.get(7)?,
                revoked_at: r.get(8)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("skin token lookup: {e}"))
}

/// PATCH rooms on a live (or revoked) key. Does **not** touch `key_hash`.
/// Empty `rooms` is allowed (Thread dark, keep secret).
pub fn set_token_rooms(id: &str, rooms: &[String]) -> Result<SkinTokenMeta, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing token id".to_string());
    }
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE tokens SET rooms = ?1 WHERE id = ?2 AND link_id IS NULL",
                params![stored, id],
            )
            .map_err(|e| format!("skin token rooms: {e}"))?;
        if n == 0 {
            return Err("unknown token id".to_string());
        }
        let Some(meta) = token_meta_from_row(conn, id)? else {
            return Err("unknown token id".to_string());
        };
        Ok(meta)
    })
    .map(attach_token_handles)
}

/// What a `tokens` row is, for a caller deciding who may change it
/// (`None` from [`token_scope`]: no such row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenScope {
    /// A guest's sign-in session (`session = 1`), not a platform pass.
    pub session: bool,
    /// The session's guest (sessions only).
    pub principal_id: Option<String>,
    /// The App it is bound to (an App session or an App's gateway pass).
    pub app_service_id: Option<String>,
    /// An App gateway link's pass (`link_id` set).
    pub linked: bool,
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    /// The stored rooms parsed as a list of rooms (else `rooms` is empty
    /// because it couldn't be read, not because there are none).
    pub rooms_readable: bool,
}

/// [`TokenScope`] of the token `id` (any kind, live or revoked).
pub fn token_scope(id: &str) -> Result<Option<TokenScope>, String> {
    let id = id.trim().to_string();
    with_conn(|conn| {
        conn.query_row(
            "SELECT session, principal_id, app_service_id, link_id, caps, rooms FROM tokens WHERE id = ?1",
            params![id],
            |r| {
                let caps_raw: String = r.get(4)?;
                let rooms_raw: String = r.get(5)?;
                Ok(TokenScope {
                    session: r.get::<_, i64>(0)? != 0,
                    principal_id: r.get(1)?,
                    app_service_id: blank_to_none(r.get(2)?),
                    linked: r.get::<_, Option<String>>(3)?.is_some_and(|l| !l.trim().is_empty()),
                    caps: caps_from_json(&caps_raw),
                    rooms: rooms_from_json(&rooms_raw),
                    rooms_readable: serde_json::from_str::<Vec<String>>(&rooms_raw).is_ok(),
                })
            },
        )
        .optional()
        .map_err(|e| format!("skin token lookup: {e}"))
    })
}

/// Set principal default rooms. Live sessions follow at their next request
/// ([`live_session`]); `apply_tokens` also copies onto the stored
/// **sessions** rows
/// only — never platform tokens. Without it, live sessions still lose any
/// room that is no longer in `rooms` (removal is never deferred).
pub fn set_principal_default_rooms(
    username: &str,
    rooms: &[String],
    apply_tokens: bool,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    members::notify_ok(with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        if p.role_id.is_some() {
            let name = p.role_name.as_deref().unwrap_or("unknown");
            return Err(assigned_guest_rooms_err(&username, name));
        }
        conn.execute(
            "UPDATE principals SET default_rooms = ?1 WHERE id = ?2",
            params![stored, p.id],
        )
        .map_err(|e| format!("skin user rooms: {e}"))?;
        // AM1: the host grant's scope is what box login reads; the column
        // stays a shadow for one release. No host row (box login off) → 0.
        conn.execute(
            "UPDATE grants SET role_id = NULL, scope = ?1
             WHERE subject_kind = 'principal' AND subject_id = ?2
               AND kind = 'app' AND target_id = ?3",
            params![stored, p.id, HOST_APP_ID],
        )
        .map_err(|e| format!("skin host grant rooms: {e}"))?;
        if apply_tokens {
            let policy = thread_only_policy(&rooms);
            rewrite_live_sessions(conn, &p.id, HOST_APP_ID, &policy)?;
        } else {
            // The stored copy only: a live session reads its grant at each
            // request either way (`live_session`, 0.46.8).
            narrow_live_sessions(conn, &p.id, HOST_APP_ID, &rooms)?;
        }
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    }))
    .map(attach_principal_handles)
}

// ── Roles ────────────────────────────────────────────────────────────

const ROLE_SELECT: &str =
    "SELECT id, name, caps, rooms, created_at, COALESCE(room_policy, '{}'), app_id, room_paths FROM roles";

fn blank_to_none(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn map_role_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinRole> {
    map_role_row_at(r, 0)
}

/// [`ROLE_SELECT`]'s columns, starting at column `at`.
fn map_role_row_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<SkinRole> {
    let caps_raw: String = r.get(at + 2)?;
    let rooms_raw: String = r.get(at + 3)?;
    let policy_raw: String = r.get(at + 5)?;
    let app_id: Option<String> = r.get(at + 6)?;
    let paths_raw: Option<String> = r.get(at + 7)?;
    let caps = caps_from_json(&caps_raw);
    let rooms = rooms_from_json(&rooms_raw);
    // A room policy that can't be read gives nothing, never every cap in
    // every room (security review 2026-11-02).
    let room_policy = policy_or_cartesian(&caps, &rooms, Some(&policy_raw)).unwrap_or_else(|e| {
        crate::log_debug!("[skin] role room policy unreadable: {e}");
        RoomPolicy::new()
    });
    Ok(SkinRole {
        id: r.get(at)?,
        name: r.get(at + 1)?,
        caps: union_caps(&room_policy),
        rooms: rooms_from_policy(&room_policy),
        room_handles: Vec::new(),
        room_access: Vec::new(),
        created_at: r.get(at + 4)?,
        app_id: blank_to_none(app_id),
        room_policy,
        // Display and editing only. Enforcement re-reads the column
        // strictly and fails closed ([`paths::session_paths`]).
        room_paths: paths::parse_room_paths(paths_raw.as_deref()).unwrap_or_default(),
    })
}

fn attach_role_handles(mut role: SkinRole) -> SkinRole {
    role.room_handles = handles_for_project_ids(&role.rooms);
    role.room_access = room_access_from_policy(&role.room_policy, Some(&role.room_paths));
    role.caps = union_caps(&role.room_policy);
    role.rooms = rooms_from_policy(&role.room_policy);
    role
}

fn persist_role_policy(
    conn: &Connection,
    role_id: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE roles SET caps = ?1, rooms = ?2, room_policy = ?3 WHERE id = ?4",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            role_id
        ],
    )
    .map_err(|e| format!("skin role update: {e}"))?;
    members::changed(None);
    Ok(())
}

fn role_from_policy(id: String, name: String, created_at: i64, policy: RoomPolicy) -> SkinRole {
    SkinRole {
        id,
        name,
        caps: union_caps(&policy),
        rooms: rooms_from_policy(&policy),
        room_handles: Vec::new(),
        room_access: Vec::new(),
        created_at,
        app_id: None,
        room_policy: policy,
        room_paths: RoomPaths::new(),
    }
}

fn role_by_id_or_name(conn: &Connection, token: &str) -> Result<Option<SkinRole>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let by_id: Option<SkinRole> = conn
        .query_row(
            &format!("{ROLE_SELECT} WHERE id = ?1"),
            params![token],
            map_role_row,
        )
        .optional()
        .map_err(|e| format!("skin role lookup: {e}"))?;
    if by_id.is_some() {
        return Ok(by_id);
    }
    conn.query_row(
        &format!("{ROLE_SELECT} WHERE name = ?1 COLLATE NOCASE"),
        params![token],
        map_role_row,
    )
    .optional()
    .map_err(|e| format!("skin role lookup: {e}"))
}

/// Rewrite one principal's live sessions bound to one App (S24-6). Host
/// writers (assign, unassign, default rooms) pass [`HOST_APP_ID`]; an old
/// session row with no App is a host session.
fn rewrite_live_sessions(
    conn: &Connection,
    principal_id: &str,
    app: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
         WHERE principal_id = ?4 AND session = 1 AND revoked_at IS NULL
           AND COALESCE(app_service_id, ?5) = ?6",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            principal_id,
            HOST_APP_ID,
            app
        ],
    )
    .map_err(|e| format!("skin session rewrite: {e}"))?;
    Ok(())
}

/// Drop every room not in `keep` from one principal's live sessions bound
/// to one App. Caps on the rooms that stay are unchanged; nothing is added.
fn narrow_live_sessions(
    conn: &Connection,
    principal_id: &str,
    app: &str,
    keep: &[String],
) -> Result<(), String> {
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT id, caps, rooms, room_policy FROM tokens
                 WHERE principal_id = ?1 AND session = 1 AND revoked_at IS NULL
                   AND COALESCE(app_service_id, ?2) = ?3",
            )
            .map_err(|e| format!("skin session narrow: {e}"))?;
        let mapped = stmt
            .query_map(params![principal_id, HOST_APP_ID, app], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| format!("skin session narrow: {e}"))?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin session narrow row: {e}"))?
    };
    for (id, caps_raw, rooms_raw, policy_raw) in rows {
        let caps = caps_from_json(&caps_raw);
        let rooms = rooms_from_json(&rooms_raw);
        let mut policy = interpret_token_policy(true, &caps, &rooms, policy_raw.as_deref());
        let before = policy.len();
        policy.retain(|room, _| keep.iter().any(|k| k == room));
        if policy.len() == before {
            continue;
        }
        conn.execute(
            "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3 WHERE id = ?4",
            params![
                caps_json(&union_caps(&policy)),
                rooms_json(&rooms_from_policy(&policy)),
                room_policy_json(&policy),
                id
            ],
        )
        .map_err(|e| format!("skin session narrow: {e}"))?;
    }
    Ok(())
}

/// R8 rewrite after a role change (S24-6): every live session whose own
/// `(principal, App)` grant carries this role. A session's App is its
/// `app_service_id` (NULL = `host`). Never keyed on `principals.role_id`.
fn rewrite_sessions_for_role(
    conn: &Connection,
    role_id: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
         WHERE session = 1 AND revoked_at IS NULL
           AND EXISTS (
             SELECT 1 FROM grants g
             WHERE g.subject_kind = 'principal' AND g.subject_id = tokens.principal_id
               AND g.kind = 'app'
               AND g.target_id = COALESCE(tokens.app_service_id, ?5)
               AND g.role_id = ?4)",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            role_id,
            HOST_APP_ID
        ],
    )
    .map_err(|e| format!("skin session rewrite: {e}"))?;
    Ok(())
}

fn unique_role_err(name: &str, e: rusqlite::Error) -> String {
    let s = e.to_string();
    if s.contains("roles_name") || s.contains("UNIQUE") {
        format!("role '{name}' already exists")
    } else {
        format!("skin role insert: {e}")
    }
}

/// Thread-only caps on each listed room (add-room default / unassigned I6).
pub fn thread_only_room_policy(rooms: &[String]) -> RoomPolicy {
    thread_only_policy(&normalize_room_ids(rooms))
}

/// Mint a named per-room bundle. Empty map = Thread dark.
/// `app_id` omitted (`None`) stays NULL — host-wide. Role names stay
/// box-unique (`roles_name`); this does not change that index.
/// A role name may not parse as a UUID (role ids are UUIDs).
pub const ROLE_NAME_LOOKS_LIKE_ID: &str = "a role name can't look like a role id (a UUID): pick a word";

pub fn create_role(name: &str, policy: &RoomPolicy) -> Result<SkinRole, String> {
    create_role_for_app(name, policy, None)
}

/// Same as [`create_role`], writing `roles.app_id` when `app_id` is set.
/// The stored app id is `published_services.id` (or the literal `host` is
/// not used here — host-wide roles stay NULL).
pub fn create_role_for_app(
    name: &str,
    policy: &RoomPolicy,
    app_id: Option<&str>,
) -> Result<SkinRole, String> {
    let name = normalize_username(name)?;
    if is_connect_role_name(&name) {
        return Err(CONNECT_ROLE_NAME_ERR.to_string());
    }
    // Roles are found by id first, then name: a name shaped like an id
    // could stand in for another role (security review, 0.46.4).
    if uuid::Uuid::parse_str(&name).is_ok() {
        return Err(ROLE_NAME_LOOKS_LIKE_ID.to_string());
    }
    let app_id = app_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let policy = normalize_policy(policy)?;
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin role begin: {e}"))?;
        let taken: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM roles WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin role name lookup: {e}"))?;
        if taken > 0 {
            return Err(format!("role '{name}' already exists"));
        }
        tx.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy, app_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                caps_json(&union_caps(&policy)),
                rooms_json(&rooms_from_policy(&policy)),
                created_at,
                room_policy_json(&policy),
                app_id,
            ],
        )
        .map_err(|e| unique_role_err(&name, e))?;
        guest_rooms::note_role_conn(&tx, &id, app_id.as_deref())?;
        tx.commit().map_err(|e| format!("skin role commit: {e}"))?;
        let mut role = role_from_policy(id, name, created_at, policy);
        role.app_id = app_id;
        Ok(role)
    })
    .map(attach_role_handles)
}

pub fn list_roles() -> Result<Vec<SkinRole>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!("{ROLE_SELECT} ORDER BY name COLLATE NOCASE"))
            .map_err(|e| format!("skin role list: {e}"))?;
        let rows = stmt
            .query_map([], map_role_row)
            .map_err(|e| format!("skin role list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin role row: {e}"))?);
        }
        Ok(out)
    })
    .map(|mut roles| {
        for r in &mut roles {
            *r = attach_role_handles(r.clone());
        }
        roles
    })
}

/// One role by id or name (case-insensitive), with its stored
/// `room_policy`. `None` when no such role.
pub fn find_role(id_or_name: &str) -> Result<Option<SkinRole>, String> {
    with_conn(|conn| role_by_id_or_name(conn, id_or_name))
}

/// Present policy replaces the whole map. `None` is a no-op persist of the
/// current snapshot (still R8-rewrites live sessions).
pub fn update_role(id_or_name: &str, policy: Option<&RoomPolicy>) -> Result<SkinRole, String> {
    let id_or_name = id_or_name.trim();
    if id_or_name.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let policy = match policy {
        Some(p) => Some(normalize_policy(p)?),
        None => None,
    };
    let role = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin role begin: {e}"))?;
        let Some(mut role) = role_by_id_or_name(&tx, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        if let Some(p) = policy {
            role.room_policy = p;
        }
        persist_role_policy(&tx, &role.id, &role.room_policy)?;
        paths::prune_role_paths(&tx, &role)?;
        role.room_paths = paths::role_room_paths(&tx, &role.id).unwrap_or_default();
        rewrite_sessions_for_role(&tx, &role.id, &role.room_policy)?;
        guest_rooms::note_role_conn(&tx, &role.id, role.app_id.as_deref())?;
        tx.commit().map_err(|e| format!("skin role commit: {e}"))?;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

/// Add or replace one room on a role. Omitted caps → Thread-only.
pub fn set_role_room(
    id_or_name: &str,
    project_id: &str,
    caps: Option<&[String]>,
) -> Result<SkinRole, String> {
    let rooms = normalize_room_ids(std::slice::from_ref(&project_id.to_string()));
    let Some(project_id) = rooms.into_iter().next() else {
        return Err("missing workspace".to_string());
    };
    let caps = parse_caps(caps)?;
    let role = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin role begin: {e}"))?;
        let Some(mut role) = role_by_id_or_name(&tx, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        role.room_policy.insert(project_id, caps);
        persist_role_policy(&tx, &role.id, &role.room_policy)?;
        rewrite_sessions_for_role(&tx, &role.id, &role.room_policy)?;
        guest_rooms::note_role_conn(&tx, &role.id, role.app_id.as_deref())?;
        tx.commit().map_err(|e| format!("skin role commit: {e}"))?;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

/// Drop one room from a role (`--clear`).
pub fn clear_role_room(id_or_name: &str, project_id: &str) -> Result<SkinRole, String> {
    let project_id = project_id.trim();
    if project_id.is_empty() {
        return Err("missing workspace".to_string());
    }
    let role = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin role begin: {e}"))?;
        let Some(mut role) = role_by_id_or_name(&tx, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        role.room_policy.remove(project_id);
        persist_role_policy(&tx, &role.id, &role.room_policy)?;
        // The room's folders go with its line: re-adding the room later
        // starts unscoped, never with a stale scope.
        paths::prune_role_paths(&tx, &role)?;
        role.room_paths.remove(project_id);
        rewrite_sessions_for_role(&tx, &role.id, &role.room_policy)?;
        guest_rooms::note_role_conn(&tx, &role.id, role.app_id.as_deref())?;
        tx.commit().map_err(|e| format!("skin role commit: {e}"))?;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

fn normalize_policy(policy: &RoomPolicy) -> Result<RoomPolicy, String> {
    let mut out = RoomPolicy::new();
    for (id, caps) in policy {
        let t = id.trim();
        if t.is_empty() {
            continue;
        }
        let parsed = parse_caps(if caps.is_empty() {
            None
        } else {
            Some(caps.as_slice())
        })?;
        out.insert(t.to_string(), parsed);
    }
    Ok(out)
}

pub fn remove_role(id_or_name: &str) -> Result<bool, String> {
    let id_or_name = id_or_name.trim();
    if id_or_name.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let r = with_conn(|conn| {
        let Some(role) = role_by_id_or_name(conn, id_or_name)? else {
            return Ok(false);
        };
        let assigned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM principals WHERE role_id = ?1",
                params![role.id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin role assigned lookup: {e}"))?;
        if assigned > 0 {
            return Err(assigned_role_remove_err(&role.name));
        }
        let granted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE role_id = ?1",
                params![role.id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin role grant lookup: {e}"))?;
        if granted > 0 {
            return Err(grant_role_remove_err(&role.name));
        }
        conn.execute("DELETE FROM agent_written_access WHERE kind = ?1 AND id = ?2", params![guest_rooms::WRITTEN_ROLE, role.id])
            .map_err(|e| format!("skin role mark delete: {e}"))?;
        let n = conn
            .execute("DELETE FROM roles WHERE id = ?1", params![role.id])
            .map_err(|e| {
                let s = e.to_string();
                if s.contains("FOREIGN KEY") || s.to_ascii_lowercase().contains("constraint") {
                    assigned_role_remove_err(&role.name)
                } else {
                    format!("skin role delete: {e}")
                }
            })?;
        Ok(n > 0)
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    }
    members::changed(None);
    Ok(r)
}

pub fn assign_role(username: &str, role: &str) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let role_token = role.trim();
    if role_token.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let p = with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let Some(role) = role_by_id_or_name(conn, role_token)? else {
            return Err(format!("unknown skin role '{role_token}'"));
        };
        conn.execute(
            "UPDATE principals SET role_id = ?1 WHERE id = ?2",
            params![role.id, p.id],
        )
        .map_err(|e| format!("skin role assign: {e}"))?;
        // AM1: the host grant is the box-login truth; the column is the
        // shadow. No host row (box login off) → nothing to write.
        conn.execute(
            "UPDATE grants SET role_id = ?1, scope = NULL
             WHERE subject_kind = 'principal' AND subject_id = ?2
               AND kind = 'app' AND target_id = ?3",
            params![role.id, p.id, HOST_APP_ID],
        )
        .map_err(|e| format!("skin host grant role: {e}"))?;
        rewrite_live_sessions(conn, &p.id, HOST_APP_ID, &role.room_policy)?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: Some(role.id),
            role_name: Some(role.name),
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    members::changed(None);
    Ok(attach_principal_handles(p))
}

pub fn unassign_role(username: &str) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let p = with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        conn.execute(
            "UPDATE principals SET role_id = NULL WHERE id = ?1",
            params![p.id],
        )
        .map_err(|e| format!("skin role unassign: {e}"))?;
        conn.execute(
            "UPDATE grants SET role_id = NULL, scope = ?1
             WHERE subject_kind = 'principal' AND subject_id = ?2
               AND kind = 'app' AND target_id = ?3",
            params![rooms_json(&p.default_rooms), p.id, HOST_APP_ID],
        )
        .map_err(|e| format!("skin host grant role: {e}"))?;
        rewrite_live_sessions(
            conn,
            &p.id,
            HOST_APP_ID,
            &thread_only_policy(&p.default_rooms),
        )?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: None,
            role_name: None,
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    members::changed(None);
    Ok(attach_principal_handles(p))
}

// ── Grants ───────────────────────────────────────────────────────────
// kind=app is a login: `target_id` is `host` (box login) or a published
// service id, and a login for that App needs this row enabled (S24-4).
// It carries EITHER a role (the role's `app_id` is NULL or this App) OR a
// role-less `scope` that is a rooms list (Thread-only caps, S24-7) — the
// shape box logins of unassigned guests have. Login reads the policy from
// the grant (AM1). Other kinds (mailbox, database, cli) hold a scope and
// no role; they are recorded, not enforced, in 0.45.5.

fn default_grant_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinGrant {
    pub id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub kind: String,
    pub target_id: String,
    pub role_id: Option<String>,
    pub scope: Option<String>,
    pub created_at: i64,
    /// `false` is no grant for any authorization read. The row stays.
    #[serde(default = "default_grant_enabled")]
    pub enabled: bool,
    /// Who wrote the row (`owner-token`, `user:<name>`, `agent:<handle>`,
    /// [`UPGRADE_CREATED_BY`]); `None` on rows from before 0.45.5.
    #[serde(default)]
    pub created_by: Option<String>,
}

/// Inputs for one grant. `role` is a role id or name (kind=app only).
#[derive(Debug, Clone, Default)]
pub struct SkinGrantWrite {
    pub subject_kind: String,
    pub subject_id: String,
    pub kind: String,
    pub target_id: String,
    pub role: Option<String>,
    pub scope: Option<String>,
    /// Audit only (`grants.created_by`).
    pub created_by: Option<String>,
}

fn grant_subject_kind(raw: &str) -> Result<String, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "principal" => Ok("principal".to_string()),
        "workspace" => Ok("workspace".to_string()),
        // Sew S27: a K2 Connect login (stored by its K7 id), kind=chat only.
        "connect" => Ok("connect".to_string()),
        _ => Err("subjectKind must be principal, workspace or connect".to_string()),
    }
}

/// A Connect login token (its K7 id, or its username) → its id.
fn resolve_connect_subject(token: &str) -> Result<String, String> {
    let t = token.trim();
    if crate::tool_hook::sew_wire::is_lower_uuid(t) {
        return Ok(t.to_string());
    }
    crate::connect_users::user_uuid(t)?.ok_or_else(|| format!("unknown Connect user '{t}'"))
}

fn grant_kind(raw: &str) -> Result<String, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "app" => Ok("app".to_string()),
        "mailbox" => Ok("mailbox".to_string()),
        "database" => Ok("database".to_string()),
        "cli" => Ok("cli".to_string()),
        // Sew S27: a chat grant creates the Sew account (tool_hook).
        "chat" => Ok("chat".to_string()),
        _ => Err("kind must be app, mailbox, database, cli, or chat".to_string()),
    }
}

pub(crate) fn map_grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinGrant> {
    let role_id: Option<String> = r.get(5)?;
    let scope: Option<String> = r.get(6)?;
    let enabled: i64 = r.get(8)?;
    let created_by: Option<String> = r.get(9)?;
    Ok(SkinGrant {
        id: r.get(0)?,
        subject_kind: r.get(1)?,
        subject_id: r.get(2)?,
        kind: r.get(3)?,
        target_id: r.get(4)?,
        role_id: blank_to_none(role_id),
        scope: blank_to_none(scope),
        created_at: r.get(7)?,
        enabled: enabled != 0,
        created_by: blank_to_none(created_by),
    })
}

pub(crate) const GRANT_SELECT: &str =
    "SELECT id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by FROM grants";

fn principal_id_exists(conn: &Connection, id: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM principals WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin grant principal lookup: {e}"))?;
    Ok(n > 0)
}

/// A kind=app `scope` must be a JSON array of room ids (`[]` allowed).
/// Stored normalized (trimmed, de-duplicated) like `default_rooms`.
pub fn normalize_app_scope(raw: &str) -> Result<String, String> {
    let list: Vec<String> = serde_json::from_str(raw.trim()).map_err(|_| {
        "kind=app scope must be a JSON array of rooms, e.g. [\"<room id>\"]".to_string()
    })?;
    Ok(rooms_json(&normalize_room_ids(&list)))
}

/// kind=app: exactly one of a role (whose `app_id` is NULL or `target`) or
/// a rooms scope (S24-7). Returns `(role_id, stored_scope)`.
fn app_grant_shape(
    conn: &Connection,
    target_id: &str,
    role_token: Option<&str>,
    scope: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    match (role_token, scope) {
        (Some(_), Some(_)) => Err("kind=app takes roleId or a rooms scope, not both".to_string()),
        (None, None) => Err("kind=app requires roleId or a rooms scope".to_string()),
        (None, Some(scope)) => Ok((None, Some(normalize_app_scope(scope)?))),
        (Some(token), None) => {
            let Some(role) = role_by_id_or_name(conn, token)? else {
                return Err(format!("unknown skin role '{token}'"));
            };
            if let Some(app_id) = role.app_id.as_deref() {
                if app_id != target_id {
                    return Err(format!(
                        "role '{}' is for app '{app_id}', not '{target_id}'",
                        role.name
                    ));
                }
            }
            Ok((Some(role.id), None))
        }
    }
}

/// Insert one grant. Does not mint a principal and does not rewrite sessions.
/// A `host` app grant also writes the shadow columns (`principals.role_id`
/// / `default_rooms`) for one release (AM1).
pub fn create_grant(write: &SkinGrantWrite) -> Result<SkinGrant, String> {
    if let Some(p) = crate::tool_hook::provisioner_for(&write.kind) {
        return create_hooked_grant(p, write);
    }
    let created = create_plain_grant(write)?;
    if created.kind == "app" {
        // App user change: K31 derives App Space membership from App
        // grants; S27 only asks for a sweep (rev 147).
        crate::tool_hook::kick_reconcile();
    }
    Ok(created)
}

fn create_plain_grant(write: &SkinGrantWrite) -> Result<SkinGrant, String> {
    let subject_kind = grant_subject_kind(&write.subject_kind)?;
    if subject_kind == "connect" {
        return Err("subjectKind connect is only for kind=chat".to_string());
    }
    let subject_id = write.subject_id.trim().to_string();
    if subject_id.is_empty() {
        return Err("missing subject id".to_string());
    }
    let kind = grant_kind(&write.kind)?;
    let target_id = write.target_id.trim().to_string();
    if target_id.is_empty() {
        return Err("missing target id".to_string());
    }
    let role_token = write
        .role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scope = write
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let created_by = write
        .created_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    members::notify_ok(with_conn(|conn| {
        if subject_kind == "principal" && !principal_id_exists(conn, &subject_id)? {
            return Err(format!("unknown principal id '{subject_id}'"));
        }
        let (role_id, stored_scope) = if kind == "app" {
            app_grant_shape(conn, &target_id, role_token, scope.as_deref())?
        } else {
            if role_token.is_some() {
                return Err("roleId is only set for kind=app".to_string());
            }
            let Some(scope) = scope.clone() else {
                return Err(format!("kind={kind} requires scope"));
            };
            (None, Some(scope))
        };

        let taken: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM grants
                 WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
                params![subject_kind, subject_id, kind, target_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin grant lookup: {e}"))?;
        if taken > 0 {
            return Err("grant already exists for this subject, kind, and target".to_string());
        }

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now_secs();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        tx.execute(
            "INSERT INTO grants
             (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
            params![
                id,
                subject_kind,
                subject_id,
                kind,
                target_id,
                role_id,
                stored_scope,
                created_at,
                created_by
            ],
        )
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("UNIQUE") {
                "grant already exists for this subject, kind, and target".to_string()
            } else {
                format!("skin grant insert: {e}")
            }
        })?;
        if kind == "app" && target_id == HOST_APP_ID && subject_kind == "principal" {
            // Shadow columns (AM1): keep `principals.role_id` /
            // `default_rooms` equal to the box-login grant for one release.
            match role_id.as_deref() {
                Some(rid) => tx.execute(
                    "UPDATE principals SET role_id = ?1 WHERE id = ?2",
                    params![rid, subject_id],
                ),
                None => tx.execute(
                    "UPDATE principals SET role_id = NULL, default_rooms = ?1 WHERE id = ?2",
                    params![stored_scope.as_deref().unwrap_or("[]"), subject_id],
                ),
            }
            .map_err(|e| format!("skin grant shadow: {e}"))?;
        }
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok(SkinGrant {
            id,
            subject_kind,
            subject_id,
            kind,
            target_id,
            role_id,
            scope: stored_scope,
            created_at,
            enabled: true,
            created_by,
        })
    }))
}


// ── Hooked grants (Sew S27 tool_hook) ────────────────────────────────
//
// A kind with a provisioner (`chat`) validates, writes the grant AND its
// outbox rows in ONE skin.db transaction, then kicks the worker. Facts
// from the main DB (workspace handle/name) are read BEFORE the skin lock.

fn hook_actor(created_by: Option<&str>) -> String {
    created_by
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::tool_hook::ACTOR_SYSTEM)
        .to_string()
}

/// Test-only failpoint: error after the outbox insert, before commit.
#[cfg(any(test, feature = "test-util"))]
pub static HOOK_FAIL_BEFORE_COMMIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn hook_failpoint() -> Result<(), String> {
    #[cfg(any(test, feature = "test-util"))]
    if HOOK_FAIL_BEFORE_COMMIT.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("injected failure before commit".to_string());
    }
    Ok(())
}

/// A workspace subject token (uuid, handle or alias) → `projects.id`.
fn resolve_workspace_subject(token: &str) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("missing subject id".to_string());
    }
    match resolve_room_tokens(&[token.to_string()]) {
        Ok(ids) if ids.len() == 1 => Ok(ids[0].clone()),
        Ok(_) => Err(format!("unknown workspace '{token}'")),
        Err(e) if e.starts_with("ambiguous") => Err(e),
        Err(_) => Err(format!("unknown workspace '{token}'")),
    }
}

fn create_hooked_grant(
    p: &'static dyn crate::tool_hook::ToolProvisioner,
    write: &SkinGrantWrite,
) -> Result<SkinGrant, String> {
    use crate::tool_hook::{outbox, HookCtx, HookEvent};
    let subject_kind = grant_subject_kind(&write.subject_kind)?;
    let mut write = write.clone();
    write.subject_kind = subject_kind.clone();
    let mut ws_ids = Vec::new();
    if subject_kind == "workspace" {
        write.subject_id = resolve_workspace_subject(&write.subject_id)?;
        ws_ids.push(write.subject_id.clone());
    } else if subject_kind == "connect" {
        write.subject_id = resolve_connect_subject(&write.subject_id)?;
    } else if write.subject_id.trim().is_empty() {
        return Err("missing subject id".to_string());
    }
    let actor = hook_actor(write.created_by.as_deref());
    let ctx = HookCtx::load(&actor, &ws_ids);
    let now = now_secs();
    let grant = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let w = p.validate(&tx, &write, &ctx)?;
        let taken: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM grants
                 WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
                params![w.subject_kind, w.subject_id, w.kind, w.target_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin grant lookup: {e}"))?;
        if taken > 0 {
            return Err("grant already exists for this subject, kind, and target".to_string());
        }
        let grant = SkinGrant {
            id: uuid::Uuid::new_v4().to_string(),
            subject_kind: w.subject_kind.clone(),
            subject_id: w.subject_id.clone(),
            kind: w.kind.clone(),
            target_id: w.target_id.clone(),
            role_id: None,
            scope: w.scope.clone(),
            created_at: now,
            enabled: true,
            created_by: w.created_by.clone(),
        };
        tx.execute(
            "INSERT INTO grants
             (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, 1, ?8)",
            params![
                grant.id,
                grant.subject_kind,
                grant.subject_id,
                grant.kind,
                grant.target_id,
                grant.scope,
                grant.created_at,
                grant.created_by
            ],
        )
        .map_err(|e| format!("skin grant insert: {e}"))?;
        let ops = p.plan(&tx, &HookEvent::GrantCreated { grant: &grant }, &ctx)?;
        outbox::enqueue(&tx, &ops, &actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok(grant)
    })?;
    crate::tool_hook::kick();
    // K31: a Sew member grant decides App Space membership too.
    crate::tool_hook::kick_app_spaces();
    members::changed(None);
    Ok(grant)
}

/// `POST /cli/skin/grants/update`: change a hooked grant's scope (§6.4).
/// Only hooked kinds; `coworker` ↔ `assistant:*` is refused (delete and
/// add again). A no-op scope enqueues nothing.
pub fn update_grant_scope(id: &str, scope: &str, actor: &str) -> Result<SkinGrant, String> {
    use crate::tool_hook::{outbox, provisioner_for, HookCtx, HookEvent};
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let old = with_conn(|conn| grant_by_id(conn, id))?.ok_or_else(|| "unknown grant".to_string())?;
    let Some(p) = provisioner_for(&old.kind) else {
        return Err("only chat grants can be updated".to_string());
    };
    let ws_ids: Vec<String> = if old.subject_kind == "workspace" {
        vec![old.subject_id.clone()]
    } else {
        Vec::new()
    };
    let actor = hook_actor(Some(actor));
    let ctx = HookCtx::load(&actor, &ws_ids);
    let now = now_secs();
    let (grant, changed) = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let old = grant_by_id(&tx, id)?.ok_or_else(|| "unknown grant".to_string())?;
        let stored = p.validate_update(&tx, &old, scope, &ctx)?;
        if old.scope.as_deref() == Some(stored.as_str()) {
            return Ok((old, false));
        }
        tx.execute(
            "UPDATE grants SET scope = ?1 WHERE id = ?2",
            params![stored, id],
        )
        .map_err(|e| format!("skin grant update: {e}"))?;
        let new = SkinGrant {
            scope: Some(stored),
            ..old.clone()
        };
        let ops = p.plan(&tx, &HookEvent::GrantUpdated { old: &old, new: &new }, &ctx)?;
        outbox::enqueue(&tx, &ops, &actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok((new, true))
    })?;
    if changed {
        crate::tool_hook::kick();
        // K31: a Sew member grant decides App Space membership too.
        crate::tool_hook::kick_app_spaces();
        members::changed(None);
    }
    Ok(grant)
}

/// Delete one hooked grant plus its cascade (assistant grants of a
/// person) and enqueue their deprovisions, in one transaction. `None`
/// when the grant does not exist.
fn delete_hooked_grant(
    p: &'static dyn crate::tool_hook::ToolProvisioner,
    id: &str,
    actor: &str,
) -> Result<Option<Vec<SkinGrant>>, String> {
    use crate::tool_hook::{outbox, HookCtx, HookEvent};
    let ctx = HookCtx {
        actor: actor.to_string(),
        ..Default::default()
    };
    let now = now_secs();
    let r = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let Some(grant) = grant_by_id(&tx, id)? else {
            return Ok(None);
        };
        let cascaded = p.cascade(&tx, &HookEvent::GrantDeleted { grant: &grant })?;
        let mut ops = Vec::new();
        for c in &cascaded {
            tx.execute("DELETE FROM grants WHERE id = ?1", params![c.id])
                .map_err(|e| format!("skin grant cascade delete: {e}"))?;
            ops.extend(p.plan(&tx, &HookEvent::GrantDeleted { grant: c }, &ctx)?);
        }
        tx.execute("DELETE FROM grants WHERE id = ?1", params![grant.id])
            .map_err(|e| format!("skin grant delete: {e}"))?;
        ops.extend(p.plan(&tx, &HookEvent::GrantDeleted { grant: &grant }, &ctx)?);
        outbox::enqueue(&tx, &ops, actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok(Some(cascaded))
    })?;
    if r.is_some() {
        crate::tool_hook::kick();
        // K31: a Sew member grant decides App Space membership too.
        crate::tool_hook::kick_app_spaces();
        members::changed(None);
    }
    Ok(r)
}

/// Delete by id and return the cascaded grants (assistant grants of a
/// person's chat grant). `Ok(None)` = unknown grant.
pub fn delete_grant_by_id_with_cascade(
    id: &str,
    actor: &str,
) -> Result<Option<Vec<SkinGrant>>, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let kind = with_conn(|conn| Ok(grant_by_id(conn, id)?.map(|g| g.kind)))?;
    let Some(kind) = kind else {
        return Ok(None);
    };
    match crate::tool_hook::provisioner_for(&kind) {
        Some(p) => delete_hooked_grant(p, id, &hook_actor(Some(actor))),
        None => {
            let deleted = delete_plain_grant_by_id(id)?;
            if deleted && kind == "app" {
                crate::tool_hook::kick_reconcile();
            }
            Ok(deleted.then(Vec::new))
        }
    }
}

/// Delete by the unique key, with the cascade. `Ok(None)` = unknown.
pub fn delete_grant_by_key_with_cascade(
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
    actor: &str,
) -> Result<Option<Vec<SkinGrant>>, String> {
    let subject_kind = grant_subject_kind(subject_kind)?;
    let kind = grant_kind(kind)?;
    let Some(p) = crate::tool_hook::provisioner_for(&kind) else {
        return Ok(delete_grant_by_key(&subject_kind, subject_id, &kind, target_id)?.then(Vec::new));
    };
    let mut subject_id = subject_id.trim().to_string();
    if subject_kind == "workspace" {
        subject_id = resolve_workspace_subject(&subject_id)?;
    } else if subject_kind == "connect" {
        subject_id = resolve_connect_subject(&subject_id)?;
    }
    let target = match target_id.trim() {
        "" => p.default_target().to_string(),
        t => t.to_ascii_lowercase(),
    };
    let id: Option<String> = with_conn(|conn| {
        let mut sid = subject_id.clone();
        if subject_kind == "principal" {
            if let Some(found) = crate::tool_hook::chat::principal_by_token(conn, &sid)? {
                sid = found.id;
            }
        }
        conn.query_row(
            "SELECT id FROM grants
             WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
            params![subject_kind, sid, kind, target],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("skin grant lookup: {e}"))
    })?;
    match id {
        Some(id) => delete_hooked_grant(p, &id, &hook_actor(Some(actor))),
        None => Ok(None),
    }
}

/// Enable/disable a hooked grant: re-validates on enable (the target must
/// be on, the assistant's person must still hold a member grant), refuses
/// a person's disable while assistants are pinned to them, and enqueues
/// provision / deprovision in the same transaction.
fn set_hooked_grant_enabled(
    p: &'static dyn crate::tool_hook::ToolProvisioner,
    old: SkinGrant,
    enabled: bool,
    actor: &str,
) -> Result<SkinGrant, String> {
    use crate::tool_hook::{outbox, HookCtx, HookEvent};
    let ws_ids: Vec<String> = if old.subject_kind == "workspace" {
        vec![old.subject_id.clone()]
    } else {
        Vec::new()
    };
    let actor = hook_actor(Some(actor));
    let ctx = HookCtx::load(&actor, &ws_ids);
    let now = now_secs();
    let (grant, changed) = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let g = grant_by_id(&tx, &old.id)?.ok_or_else(|| "unknown grant".to_string())?;
        if g.enabled == enabled {
            return Ok((g, false));
        }
        if enabled {
            p.validate(
                &tx,
                &SkinGrantWrite {
                    subject_kind: g.subject_kind.clone(),
                    subject_id: g.subject_id.clone(),
                    kind: g.kind.clone(),
                    target_id: g.target_id.clone(),
                    role: None,
                    scope: g.scope.clone(),
                    created_by: g.created_by.clone(),
                },
                &ctx,
            )?;
        } else {
            p.validate_disable(&tx, &g)?;
        }
        tx.execute(
            "UPDATE grants SET enabled = ?1 WHERE id = ?2",
            params![if enabled { 1 } else { 0 }, g.id],
        )
        .map_err(|e| format!("skin grant enabled: {e}"))?;
        let new = SkinGrant { enabled, ..g };
        let ev = if enabled {
            HookEvent::GrantEnabled { grant: &new }
        } else {
            HookEvent::GrantDisabled { grant: &new }
        };
        let ops = p.plan(&tx, &ev, &ctx)?;
        outbox::enqueue(&tx, &ops, &actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok((new, true))
    })?;
    if changed {
        crate::tool_hook::kick();
        // K31: a Sew member grant decides App Space membership too.
        crate::tool_hook::kick_app_spaces();
        members::changed(None);
    }
    Ok(grant)
}

/// [`set_grant_enabled`] with the caller recorded on hooked rows.
pub fn set_grant_enabled_as(id: &str, enabled: bool, actor: &str) -> Result<SkinGrant, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let old = with_conn(|conn| grant_by_id(conn, id))?;
    if let Some(old) = old {
        if let Some(p) = crate::tool_hook::provisioner_for(&old.kind) {
            return set_hooked_grant_enabled(p, old, enabled, actor);
        }
        let r = set_plain_grant_enabled(id, enabled)?;
        if r.kind == "app" {
            crate::tool_hook::kick_reconcile();
        }
        return Ok(r);
    }
    set_plain_grant_enabled(id, enabled)
}

pub fn list_grants() -> Result<Vec<SkinGrant>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!("{GRANT_SELECT} ORDER BY created_at, id"))
            .map_err(|e| format!("skin grant list: {e}"))?;
        let rows = stmt
            .query_map([], map_grant_row)
            .map_err(|e| format!("skin grant list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin grant row: {e}"))?);
        }
        Ok(out)
    })
}

/// Called when a subject loses a grant (disable or delete). Sessions are
/// snapshots, so a removed grant must not ride on until `expires_at`.
/// - a principal's `kind=app` grant: only the sessions bound to that App
///   end (S24-6). Switching off App X does not log the guest out of App Y.
/// - a principal's other kinds (recorded, not enforced): every session, as
///   in 0.45.4 (AM0).
/// - workspace subjects hold no `k2skn_` sessions; platform passes
///   (`session=0`) have no principal and are never touched.
fn revoke_sessions_for_grant(
    conn: &Connection,
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
) -> Result<usize, String> {
    if subject_kind != "principal" {
        return Ok(0);
    }
    // A chat grant is not a K2 login: its Sew sessions end in Sew on
    // deprovision. Never log the guest out of their Apps (S24 per-App).
    if crate::tool_hook::provisioner_for(kind).is_some() {
        return Ok(0);
    }
    if kind == "app" {
        revoke_sessions_for_principal_app(conn, subject_id, target_id)
    } else {
        revoke_sessions_for_principal(conn, subject_id)
    }
}

/// Delete the grant row. The role bundle stays. A principal subject's live
/// login sessions for that App are revoked (they log in again against what
/// is left).
pub fn delete_grant_by_id(id: &str) -> Result<bool, String> {
    Ok(delete_grant_by_id_with_cascade(id, crate::tool_hook::ACTOR_SYSTEM)?.is_some())
}

fn delete_plain_grant_by_id(id: &str) -> Result<bool, String> {
    members::notify_ok(with_immediate(|conn| {
        let Some(grant) = grant_by_id(conn, id)? else {
            return Ok(false);
        };
        let n = conn
            .execute("DELETE FROM grants WHERE id = ?1", params![id])
            .map_err(|e| format!("skin grant delete: {e}"))?;
        if n > 0 {
            revoke_sessions_for_grant(
                conn,
                &grant.subject_kind,
                &grant.subject_id,
                &grant.kind,
                &grant.target_id,
            )?;
        }
        Ok(n > 0)
    }))
}

/// Delete by the unique (subject, kind, target) key. The role stays.
/// A principal subject's live login sessions for that App are revoked.
pub fn delete_grant_by_key(
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
) -> Result<bool, String> {
    if crate::tool_hook::provisioner_for(kind).is_some() {
        return Ok(delete_grant_by_key_with_cascade(
            subject_kind,
            subject_id,
            kind,
            target_id,
            crate::tool_hook::ACTOR_SYSTEM,
        )?
        .is_some());
    }
    let deleted = delete_plain_grant_by_key(subject_kind, subject_id, kind, target_id)?;
    if deleted && kind.trim().eq_ignore_ascii_case("app") {
        crate::tool_hook::kick_reconcile();
    }
    Ok(deleted)
}

fn delete_plain_grant_by_key(
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
) -> Result<bool, String> {
    let subject_kind = grant_subject_kind(subject_kind)?;
    let subject_id = subject_id.trim();
    if subject_id.is_empty() {
        return Err("missing subject id".to_string());
    }
    let kind = grant_kind(kind)?;
    let target_id = target_id.trim();
    if target_id.is_empty() {
        return Err("missing target id".to_string());
    }
    members::notify_ok(with_immediate(|conn| {
        let n = conn
            .execute(
                "DELETE FROM grants
                 WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
                params![subject_kind, subject_id, kind, target_id],
            )
            .map_err(|e| format!("skin grant delete: {e}"))?;
        if n > 0 {
            revoke_sessions_for_grant(conn, &subject_kind, subject_id, &kind, target_id)?;
        }
        Ok(n > 0)
    }))
}

fn grant_by_id(conn: &Connection, id: &str) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!("{GRANT_SELECT} WHERE id = ?1"),
        params![id],
        map_grant_row,
    )
    .optional()
    .map_err(|e| format!("skin grant lookup: {e}"))
}

/// Set `enabled` to 0 or 1. Does not delete the row. Disabling a
/// principal's App grant revokes that principal's live login sessions for
/// that App; enabling never revives a revoked pass (the guest logs in again).
pub fn set_grant_enabled(id: &str, enabled: bool) -> Result<SkinGrant, String> {
    set_grant_enabled_as(id, enabled, crate::tool_hook::ACTOR_SYSTEM)
}

fn set_plain_grant_enabled(id: &str, enabled: bool) -> Result<SkinGrant, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let flag: i64 = if enabled { 1 } else { 0 };
    members::notify_ok(with_immediate(|conn| {
        let n = conn
            .execute(
                "UPDATE grants SET enabled = ?1 WHERE id = ?2",
                params![flag, id],
            )
            .map_err(|e| format!("skin grant enabled: {e}"))?;
        if n == 0 {
            return Err("unknown grant".to_string());
        }
        let grant = grant_by_id(conn, id)?.ok_or_else(|| "unknown grant".to_string())?;
        if !enabled {
            revoke_sessions_for_grant(
                conn,
                &grant.subject_kind,
                &grant.subject_id,
                &grant.kind,
                &grant.target_id,
            )?;
        }
        Ok(grant)
    }))
}

/// Present inserts one host grant (`enabled` 1) carrying the guest's current
/// role, or their default rooms when unassigned (AM1) — never a blank row.
/// Absent deletes that principal+app+host row, does not clear
/// `principals.role_id`, and revokes that principal's live login sessions:
/// all of them while [`HOST_OFF_ENDS_ALL_APP_SESSIONS`] (Q8), else only
/// host ones. An existing row is left as the owner wrote it.
pub fn set_host_login(username: &str, present: bool) -> Result<bool, String> {
    let username = normalize_username(username)?;
    let r = members::notify_ok(with_immediate(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        if present {
            match p.role_id.clone() {
                Some(role) => insert_host_grant(conn, &p.id, Some(role), None)?,
                None => insert_host_grant(conn, &p.id, None, Some(rooms_json(&p.default_rooms)))?,
            }
            Ok(true)
        } else {
            let n = conn
                .execute(
                    "DELETE FROM grants
                     WHERE subject_kind = 'principal' AND subject_id = ?1
                       AND kind = 'app' AND target_id = ?2",
                    params![p.id, HOST_APP_ID],
                )
                .map_err(|e| format!("skin host grant delete: {e}"))?;
            if HOST_OFF_ENDS_ALL_APP_SESSIONS {
                revoke_sessions_for_principal(conn, &p.id)?;
            } else {
                revoke_sessions_for_principal_app(conn, &p.id, HOST_APP_ID)?;
            }
            Ok(n > 0)
        }
    }))?;
    // App user change (Sew rev 147 / K31 seam): never provisions anyone
    // by itself; the sweep only reconciles chat grants.
    crate::tool_hook::kick_reconcile();
    Ok(r)
}

// ── Per-App logins (S24) ─────────────────────────────────────────────

/// What a copy of box logins onto one App did (S24-10 / S24-11).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CopyHostReport {
    pub app: String,
    pub dry_run: bool,
    /// Usernames that got a new App grant (or would, on a dry run).
    pub added: Vec<String>,
    /// Usernames that already had an App grant for this App; it wins,
    /// including a disabled one (Q2). `(username, enabled)`.
    pub kept_existing: Vec<(String, bool)>,
    /// Usernames whose box-login role is tagged for a different App: not
    /// copied (Q2).
    pub skipped_other_app_role: Vec<String>,
    /// Box-login guests another workspace's agent added (`creators`): not
    /// copied onto this App (security review E).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_other_workspace_agent: Vec<String>,
}

/// Copy every ENABLED box login (`host` grant) onto App `app` (S24-10,
/// S24-11): the host grant's role when that role's `app_id` is NULL or this
/// App, else its rooms scope role-less (S24-7). A grant the owner already
/// wrote for this App wins (`ON CONFLICT DO NOTHING`), enabled or not.
/// A role tagged for another App is skipped. Runs inside the caller's
/// transaction; `dry_run` writes nothing.
fn copy_host_to_app_tx(
    conn: &Connection,
    app: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    dry_run: bool,
) -> Result<CopyHostReport, String> {
    if app.is_empty() || app == HOST_APP_ID {
        return Err("copy host: name a published App, not host".to_string());
    }
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, p.username, g.role_id, g.scope, r.app_id
                 FROM grants g
                 JOIN principals p ON p.id = g.subject_id
                 LEFT JOIN roles r ON r.id = g.role_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app'
                   AND g.target_id = ?1 AND g.enabled != 0
                 ORDER BY p.username",
            )
            .map_err(|e| format!("skin copy host: {e}"))?;
        let mapped = stmt
            .query_map(params![HOST_APP_ID], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|e| format!("skin copy host: {e}"))?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin copy host row: {e}"))?
    };
    let mut report = CopyHostReport {
        app: app.to_string(),
        dry_run,
        ..Default::default()
    };
    for (pid, username, role_id, scope, role_app) in rows {
        // A box guest another workspace's agent added never lands on this
        // App (only its own workspace's Apps; unknown workspace: none).
        if let Some(by_ws) = creators::workspace_of_conn(conn, &pid)? {
            if app_workspace != Some(by_ws.as_str()) {
                report.skipped_other_workspace_agent.push(username);
                continue;
            }
        }
        if let Some(existing) = app_grant(conn, &pid, app)? {
            report.kept_existing.push((username, existing.enabled));
            continue;
        }
        let role_id = blank_to_none(role_id);
        if role_id.is_some() {
            if let Some(other) = blank_to_none(role_app) {
                if other != app {
                    report.skipped_other_app_role.push(username);
                    continue;
                }
            }
        }
        let scope = if role_id.is_some() {
            None
        } else {
            Some(rooms_json(&rooms_from_json(scope.as_deref().unwrap_or("[]"))))
        };
        if !dry_run {
            conn.execute(
                "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                 VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, 1, ?7)
                 ON CONFLICT(subject_kind, subject_id, kind, target_id) DO NOTHING",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    pid,
                    app,
                    role_id,
                    scope,
                    now_secs(),
                    created_by
                ],
            )
            .map_err(|e| format!("skin copy host insert: {e}"))?;
        }
        report.added.push(username);
    }
    Ok(report)
}

/// One summary audit line plus one line per skipped or pre-existing row.
pub fn audit_copy_host_report(event: &str, actor: &str, report: &CopyHostReport) {
    if report.dry_run {
        return;
    }
    record_access_audit(
        event,
        actor,
        &format!(
            "app {}: added {} login(s), kept {} existing App grant(s), skipped {} other-App role(s)",
            report.app,
            report.added.len(),
            report.kept_existing.len(),
            report.skipped_other_app_role.len()
        ),
    );
    for (user, enabled) in &report.kept_existing {
        record_access_audit(
            event,
            user,
            &format!(
                "app {}: kept the owner's existing {} App grant (it now applies at login)",
                report.app,
                if *enabled { "enabled" } else { "disabled" }
            ),
        );
    }
    for user in &report.skipped_other_app_role {
        record_access_audit(
            event,
            user,
            &format!(
                "app {}: not copied, the box-login role is tagged for another App",
                report.app
            ),
        );
    }
}

/// Owner (S24-11, `k2 app grant copy-host`): give everyone who has the box
/// login the same access on App `app`. `app` is a published service id the
/// caller already resolved. Writes an audit summary unless `dry_run`.
pub fn copy_host_grants_to_app(
    app: &str,
    created_by: &str,
    dry_run: bool,
) -> Result<CopyHostReport, String> {
    copy_host_grants_to_app_in(app, None, created_by, dry_run)
}

/// [`copy_host_grants_to_app`] for an App of `app_workspace`
/// (`projects.id`, from the daemon: this module never reads k2so.db). A
/// box guest an agent added is copied only onto its own workspace's Apps;
/// with no workspace given, none of them is.
pub fn copy_host_grants_to_app_in(
    app: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    dry_run: bool,
) -> Result<CopyHostReport, String> {
    let app = app.trim().to_string();
    let report = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin copy host begin: {e}"))?;
        let report = copy_host_to_app_tx(&tx, &app, app_workspace, created_by, dry_run)?;
        tx.commit()
            .map_err(|e| format!("skin copy host commit: {e}"))?;
        Ok(report)
    })?;
    audit_copy_host_report("app_grant_copy_host", created_by, &report);
    members::changed(None);
    Ok(report)
}

/// Why [`copy_host_grants_to_app_expect`] wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyHostExpectError {
    /// The box logins it would copy now aren't the ones expected (sorted).
    Changed { now: Vec<String> },
    /// A store error.
    Store(String),
}

/// [`copy_host_grants_to_app_in`], but only when the box logins it would
/// copy are exactly `expected` (any order): the comparison and the write
/// are one transaction, so a permission card approves exactly the logins
/// it showed. Anything else writes nothing ([`CopyHostExpectError::Changed`]).
pub fn copy_host_grants_to_app_expect(
    app: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    expected: &[String],
) -> Result<CopyHostReport, CopyHostExpectError> {
    let app = app.trim().to_string();
    let mut want: Vec<String> = expected.iter().map(|u| u.trim().to_ascii_lowercase()).filter(|u| !u.is_empty()).collect();
    want.sort();
    want.dedup();
    let report = with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin copy host begin: {e}"))?;
        let mut now = copy_host_to_app_tx(&tx, &app, app_workspace, created_by, true)?.added;
        now.sort();
        if now != want {
            return Ok(Err(now));
        }
        let report = copy_host_to_app_tx(&tx, &app, app_workspace, created_by, false)?;
        tx.commit().map_err(|e| format!("skin copy host commit: {e}"))?;
        Ok(Ok(report))
    })
    .map_err(CopyHostExpectError::Store)?
    .map_err(|now| CopyHostExpectError::Changed { now })?;
    audit_copy_host_report("app_grant_copy_host", created_by, &report);
    members::changed(None);
    Ok(report)
}

/// True until the one-time `--skin` App backfill (S24-10) has run.
pub fn skin_app_backfill_pending() -> Result<bool, String> {
    with_conn(|conn| {
        let pending: Option<i64> = conn
            .query_row(
                "SELECT pending FROM skin_app_backfill WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin app backfill: {e}"))?;
        Ok(pending.unwrap_or(0) != 0)
    })
}

/// S24-10, once per database: copy box logins onto every `--skin` App that
/// existed at the upgrade (`skin_app_ids`, from `published_services` in
/// k2so.db — the daemon passes them; this module never reads k2so.db).
/// Runs at boot before any helper starts. `Ok(None)` = already done. One
/// transaction for all Apps plus the done mark; audit after commit.
/// `--cmd` Apps get no backfill: their BFF logs in as `host` until it sends
/// `app`. Apps published later start with nobody (Q6, fail closed).
pub fn run_skin_app_backfill(
    skin_app_ids: &[String],
) -> Result<Option<Vec<CopyHostReport>>, String> {
    let reports = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin app backfill begin: {e}"))?;
        let pending: Option<i64> = tx
            .query_row(
                "SELECT pending FROM skin_app_backfill WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin app backfill: {e}"))?;
        if pending.unwrap_or(0) == 0 {
            return Ok(None);
        }
        let mut reports = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for raw in skin_app_ids {
            let app = raw.trim();
            if app.is_empty() || app == HOST_APP_ID || !seen.insert(app.to_string()) {
                continue;
            }
            reports.push(copy_host_to_app_tx(&tx, app, None, UPGRADE_CREATED_BY, false)?);
        }
        let summary = serde_json::to_string(&reports).unwrap_or_else(|_| "[]".to_string());
        tx.execute(
            "UPDATE skin_app_backfill SET pending = 0, done_at = ?1, summary = ?2 WHERE id = 1",
            params![now_secs(), summary],
        )
        .map_err(|e| format!("skin app backfill mark: {e}"))?;
        tx.commit()
            .map_err(|e| format!("skin app backfill commit: {e}"))?;
        Ok(Some(reports))
    })?;
    if let Some(list) = reports.as_ref() {
        record_access_audit(
            "app_grant_upgrade",
            UPGRADE_CREATED_BY,
            &format!(
                "--skin App backfill: {} App(s), {} login(s) copied from the box login",
                list.len(),
                list.iter().map(|r| r.added.len()).sum::<usize>()
            ),
        );
        for r in list {
            audit_copy_host_report("app_grant_upgrade", UPGRADE_CREATED_BY, r);
        }
    }
    Ok(reports)
}

/// S24-12: a new guest for one App. Creates the principal with that App's
/// grant (the role, or empty rooms `[]`) and **no** host grant, so this
/// person can't sign in at the box login. `app` must be a published service
/// id the caller resolved (never `host`; plain `add_principal_with_email`
/// is the box-login path). The role's `app_id` must be NULL or `app`.
pub fn add_principal_for_app(
    username: &str,
    email: Option<&str>,
    app: &str,
    role: Option<&str>,
    created_by: &str,
) -> Result<SkinPrincipal, String> {
    add_principal_for_app_by(username, email, app, role, created_by, AddedBy::default())
}

/// Who, beyond the actor label, made an App guest (agent-app-users S2).
#[derive(Debug, Clone, Copy, Default)]
pub struct AddedBy<'a> {
    /// The workspace whose agent added the guest: its creator stamp
    /// ([`creators`]) is written in the same transaction as the principal
    /// and its grant, so there is never a moment when the guest exists
    /// unstamped (read as the owner's). It is also tied to that workspace
    /// (`created`, [`ties`]) in the same transaction.
    pub agent_workspace: Option<&'a str>,
    /// The workspace of the App whose App key made the guest: tied to it
    /// (`app_key`, Q3), no creator stamp.
    pub app_key_workspace: Option<&'a str>,
}

/// Test-only failpoint: an error after the principal, grant and stamp
/// are written, before the commit (agent-app-users T2.4).
#[cfg(any(test, feature = "test-util"))]
pub static AGENT_ADD_FAIL_BEFORE_COMMIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn agent_add_failpoint() -> Result<(), String> {
    #[cfg(any(test, feature = "test-util"))]
    if AGENT_ADD_FAIL_BEFORE_COMMIT.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("injected failure before commit".to_string());
    }
    Ok(())
}

/// [`add_principal_for_app`] with [`AddedBy`]: everything it writes
/// commits together or not at all.
pub fn add_principal_for_app_by(
    username: &str,
    email: Option<&str>,
    app: &str,
    role: Option<&str>,
    created_by: &str,
    by: AddedBy<'_>,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let email = match email {
        Some(raw) => normalize_email(raw)?,
        None => None,
    };
    let app = app.trim().to_string();
    if app.is_empty() || app == HOST_APP_ID {
        return Err("app must be a published App (use a plain add for the box login)".to_string());
    }
    if (by.agent_workspace.is_some() || by.app_key_workspace.is_some()) && creators::name_reserved(&username)? {
        return Err(format!("{}: '{username}' is a name people on this box sign in or stamp under: pick another", creators::NAME_RESERVED));
    }
    let role = role.map(str::trim).filter(|s| !s.is_empty());
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    let r = with_conn(|conn| {
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM principals WHERE username = ?1",
                params![username],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin principal lookup: {e}"))?;
        if exists.is_some() {
            return Err(format!("skin user '{username}' already exists"));
        }
        if let Some(ref em) = email {
            ensure_email_free(conn, em, None)?;
        }
        let (role_id, scope) = match role {
            Some(token) => app_grant_shape(conn, &app, Some(token), None)?,
            None => (None, Some("[]".to_string())),
        };
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin principal begin: {e}"))?;
        tx.execute(
            "INSERT INTO principals (id, username, created_at, email) VALUES (?1, ?2, ?3, ?4)",
            params![id, username, created_at, email],
        )
        .map_err(|e| unique_email_err(email.as_deref(), e, "skin principal insert"))?;
        let grant_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO grants
             (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, 1, ?7)",
            params![
                grant_id,
                id,
                app,
                role_id,
                scope,
                created_at,
                created_by
            ],
        )
        .map_err(|e| format!("skin app grant insert: {e}"))?;
        if let Some(ws) = by.agent_workspace {
            creators::stamp_conn(&tx, &id, ws)?;
        }
        if by.agent_workspace.is_some() {
            agent_add_failpoint()?;
        }
        if let Some(ws) = by.agent_workspace {
            ties::set_conn(&tx, &id, ties::TieWrite { workspace_id: ws, set_by: created_by, reason: ties::REASON_CREATED })?;
        }
        if let Some(ws) = by.app_key_workspace {
            ties::set_conn(&tx, &id, ties::TieWrite { workspace_id: ws, set_by: created_by, reason: ties::REASON_APP_KEY })?;
        }
        tx.commit()
            .map_err(|e| format!("skin principal commit: {e}"))?;
        Ok(SkinPrincipal {
            id,
            username,
            created_at,
            default_rooms: Vec::new(),
            default_room_handles: Vec::new(),
            has_password: false,
            role_id: None,
            role_name: None,
            email,
            full_name: None,
            agent_workspace: None,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    // App user change (Sew rev 147 / K31 seam).
    crate::tool_hook::kick_reconcile();
    members::changed(None);
    Ok(r)
}

/// S24-15: set the rooms of one guest's role-less grant on App `app`
/// (`host` included). Creates the grant when there is none (owner route).
/// A grant that carries a role is refused: change the role's rooms instead.
/// Live sessions follow at their next request ([`live_session`]);
/// `apply_tokens` only decides how the stored rows are rewritten. A disabled
/// grant stays disabled.
pub fn set_app_grant_rooms(
    username: &str,
    app: &str,
    rooms: &[String],
    apply_tokens: bool,
    created_by: &str,
) -> Result<SkinGrant, String> {
    let username = normalize_username(username)?;
    let app = normalize_app_target(Some(app));
    if app == HOST_APP_ID {
        // Box login keeps the long-standing path (column + host grant).
        let p = set_principal_default_rooms(&username, rooms, apply_tokens)?;
        return with_conn(|conn| {
            app_grant(conn, &p.id, HOST_APP_ID)?
                .ok_or_else(|| format!("'{username}' has no box login (host grant)"))
        });
    }
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    members::notify_ok(with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("skin app grant begin: {e}"))?;
        let Some(p) = principal_by_username(&tx, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        match app_grant(&tx, &p.id, &app)? {
            Some(g) if g.role_id.is_some() => {
                return Err(format!(
                    "'{username}' has a role on that App; change the role's rooms (k2 app role room) or remove the grant first"
                ));
            }
            Some(g) => {
                tx.execute(
                    "UPDATE grants SET scope = ?1 WHERE id = ?2",
                    params![stored, g.id],
                )
                .map_err(|e| format!("skin app grant rooms: {e}"))?;
            }
            None => {
                tx.execute(
                    "INSERT INTO grants
                     (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                     VALUES (?1, 'principal', ?2, 'app', ?3, NULL, ?4, ?5, 1, ?6)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        p.id,
                        app,
                        stored,
                        now_secs(),
                        created_by
                    ],
                )
                .map_err(|e| format!("skin app grant insert: {e}"))?;
            }
        }
        if apply_tokens {
            rewrite_live_sessions(&tx, &p.id, &app, &thread_only_policy(&rooms))?;
        } else {
            narrow_live_sessions(&tx, &p.id, &app, &rooms)?;
        }
        let grant = app_grant(&tx, &p.id, &app)?.ok_or_else(|| "app grant vanished".to_string())?;
        tx.commit().map_err(|e| format!("skin app grant commit: {e}"))?;
        Ok(grant)
    }))
}

/// `POST /cli/skin/grants/role` (`k2 app grant role <id> --role <r>`,
/// 0.46.5): give one App login (a `kind=app` grant) another role IN PLACE.
/// Before, re-roling meant `grant rm` + `grant add`, which ended that App's
/// live logins. Here the row keeps its id, its on/off and its sessions:
/// every live session of that `(principal, App)` is rewritten (R8, like a
/// role edit) to the new role's rooms, held to the guest's tie ceiling
/// exactly as a fresh sign-in is ([`ties::clamp`]; outside it, the session
/// keeps no room). The role follows the add rule ([`app_grant_shape`]: a
/// box-wide role or one of this App's). A rooms-only grant becomes
/// role-backed (its scope is cleared). The box login (`host`) also writes
/// the shadow column, like [`assign_role`]. Same role again = no change.
/// Returns the grant and whether it changed.
pub fn set_app_grant_role(id: &str, role_token: &str) -> Result<(SkinGrant, bool), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let role_token = role_token.trim();
    if role_token.is_empty() {
        return Err("missing role".to_string());
    }
    // A21: the App's workspace (daemon DB) before the skin mutex.
    let target = with_conn(|conn| grant_by_id(conn, id))?
        .ok_or_else(|| "unknown grant".to_string())?
        .target_id;
    let app_ws = ties::app_workspace(&target);
    let (grant, changed) = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let Some(old) = grant_by_id(&tx, id)? else {
            return Err("unknown grant".to_string());
        };
        if old.kind != "app" {
            return Err(format!(
                "only an App login (kind=app) has a role; this grant is kind={} (a chat grant changes with k2 app grant set --scope)",
                old.kind
            ));
        }
        if old.target_id != target {
            return Err("the grant changed while it was read; try again".to_string());
        }
        let (Some(role_id), None) = app_grant_shape(&tx, &old.target_id, Some(role_token), None)? else {
            return Err("kind=app requires roleId".to_string());
        };
        if old.role_id.as_deref() == Some(role_id.as_str()) && old.scope.is_none() {
            return Ok((old, false));
        }
        let Some(role) = role_by_id_or_name(&tx, &role_id)? else {
            return Err(format!("unknown skin role '{role_token}'"));
        };
        // Compare-and-set on what was read: a concurrent writer is an
        // error, never a silent overwrite.
        let n = tx
            .execute(
                "UPDATE grants SET role_id = ?1, scope = NULL
                 WHERE id = ?2 AND role_id IS ?3 AND scope IS ?4",
                params![role.id, old.id, old.role_id, old.scope],
            )
            .map_err(|e| format!("skin grant role: {e}"))?;
        if n != 1 {
            return Err("the grant changed while it was read; try again".to_string());
        }
        if old.subject_kind == "principal" {
            // Shadow column (AM1), as `assign_role` writes it: the box
            // login's role; for an App-only guest (no box login) its App
            // login's role, so `k2 app user list` names the role it has.
            let box_login: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM grants
                     WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app' AND target_id = ?2",
                    params![old.subject_id, HOST_APP_ID],
                    |r| r.get(0),
                )
                .map_err(|e| format!("skin grant shadow: {e}"))?;
            if old.target_id == HOST_APP_ID || box_login == 0 {
                tx.execute(
                    "UPDATE principals SET role_id = ?1 WHERE id = ?2",
                    params![role.id, old.subject_id],
                )
                .map_err(|e| format!("skin grant shadow: {e}"))?;
            }
            let tie = ties::get_conn(&tx, &old.subject_id)?;
            let app_rooms = ties::app_rooms_conn(&tx, &old.target_id)?;
            let policy = ties::clamp(tie.as_ref(), &old.target_id, app_ws.as_deref(), &app_rooms, role.room_policy.clone())
                .unwrap_or_default();
            rewrite_live_sessions(&tx, &old.subject_id, &old.target_id, &policy)?;
        }
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok((
            SkinGrant {
                role_id: Some(role.id),
                scope: None,
                ..old
            },
            true,
        ))
    })?;
    if changed {
        members::changed(None);
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
        // K31 derives App Space membership from App grants.
        crate::tool_hook::kick_reconcile();
    }
    Ok((grant, changed))
}

/// `POST /cli/skin/roles/assign` (`k2 app user role <user> <role>`),
/// 0.46.5 (item 13, Agent Acme Vanilla): for an App-only guest (App
/// logins, no box login) the role lives on its App login's grant, so the
/// role goes THERE, through the one in-place path [`set_app_grant_role`]
/// (`k2 app grant role`): a rooms-only grant (a test guest added with
/// --room) becomes role-backed and its live sessions get the role's caps;
/// before, only `principals.role_id` moved, `user list` showed the role
/// and a login still got Thread-only rooms. The role must fit one of its
/// App logins (box-wide, or that App's); more than one that fits is
/// ambiguous and names `k2 app grant role`. A box login, or a guest with no
/// login grant at all, keeps [`assign_role`].
pub fn assign_login_role(username: &str, role: &str) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let role_token = role.trim();
    if role_token.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let app_grant = with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let Some(role) = role_by_id_or_name(conn, role_token)? else {
            return Err(format!("unknown skin role '{role_token}'"));
        };
        let grants: Vec<SkinGrant> = {
            let mut stmt = conn
                .prepare(&format!(
                    "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app' ORDER BY created_at, id"
                ))
                .map_err(|e| format!("skin grant list: {e}"))?;
            let rows = stmt
                .query_map(params![p.id], map_grant_row)
                .map_err(|e| format!("skin grant list: {e}"))?;
            rows.collect::<Result<_, _>>().map_err(|e| format!("skin grant row: {e}"))?
        };
        if grants.is_empty() || grants.iter().any(|g| g.target_id == HOST_APP_ID) {
            return Ok(None);
        }
        let fits: Vec<&SkinGrant> = grants
            .iter()
            .filter(|g| role.app_id.as_deref().is_none_or(|a| a == g.target_id))
            .collect();
        match fits.as_slice() {
            [one] => Ok(Some(one.id.clone())),
            [] => Err(format!(
                "role '{}' is for another App: '{username}' signs in only to {} (k2 app role list)",
                role.name,
                grants.iter().map(|g| g.target_id.as_str()).collect::<Vec<_>>().join(", ")
            )),
            many => Err(format!(
                "'{username}' signs in to {} Apps: give one login the role with k2 app grant role <grant-id> --role {} (ids: k2 app grant list)",
                many.len(),
                role.name
            )),
        }
    })?;
    let Some(grant_id) = app_grant else {
        return assign_role(&username, role_token);
    };
    set_app_grant_role(&grant_id, role_token)?;
    let p = with_conn(|conn| {
        principal_by_username(conn, &username)?.ok_or_else(|| format!("unknown skin user '{username}'"))
    })?;
    Ok(attach_principal_handles(p))
}

// S24-19 (agent hire S5, 0.45.6): an App login a hire makes calls
// `add_principal_for_app` (an App grant, never the box login) for the App
// the recipe names and the key's ceiling allows; see `k2_core::hire::apply`.

/// Is `username` taken by a guest (App or box login)? The hire plan checks
/// before any write.
pub fn principal_exists(username: &str) -> Result<bool, String> {
    let username = normalize_username(username)?;
    with_conn(|conn| Ok(principal_by_username(conn, &username)?.is_some()))
}

/// Hire-key App access limit (0.45.6): the role `token` (id or name) as
/// usable on App `app`: a role of that App or a box-wide role (`app_id`
/// NULL), the same rule an App grant applies. `Ok(None)` = no such role,
/// or a role of another App.
pub fn role_for_app(app: &str, token: &str) -> Result<Option<SkinRole>, String> {
    let app = app.trim();
    with_conn(|conn| {
        Ok(role_by_id_or_name(conn, token)?
            .filter(|r| r.app_id.as_deref().is_none_or(|a| a == app)))
    })
}

/// Every role usable on App `app` (its own roles and box-wide roles).
pub fn roles_for_app(app: &str) -> Result<Vec<SkinRole>, String> {
    let app = app.trim();
    Ok(list_roles()?
        .into_iter()
        .filter(|r| r.app_id.as_deref().is_none_or(|a| a == app))
        .collect())
}

/// Is `inner` no wider than `outer`: every room of `inner` is in `outer`
/// with a subset of its functions? (A role "up to" another.)
pub fn policy_within(inner: &RoomPolicy, outer: &RoomPolicy) -> bool {
    inner.iter().all(|(room, caps)| {
        outer
            .get(room)
            .is_some_and(|allowed| caps.iter().all(|c| allowed.iter().any(|a| a == c)))
    })
}

// ── Access templates ─────────────────────────────────────────────────
// A line is kind + optional role. Apply copies that into real grants
// with the target (and scope) supplied on the POST. Later edits of the
// template do not rewrite grants already written.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessTemplateLine {
    pub id: String,
    pub template_id: String,
    pub kind: String,
    pub role_id: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessTemplate {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub lines: Vec<AccessTemplateLine>,
}

/// One apply binding. `role` is rejected for mailbox, database, and cli.
/// App role comes from the template line, not this field.
#[derive(Debug, Clone)]
pub struct TemplateApplyLine {
    pub line_id: String,
    pub target_id: String,
    pub scope: Option<String>,
    pub role: Option<String>,
}

fn normalize_template_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("missing template name".to_string());
    }
    Ok(name.to_string())
}

fn map_template_line(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccessTemplateLine> {
    let role_id: Option<String> = r.get(3)?;
    Ok(AccessTemplateLine {
        id: r.get(0)?,
        template_id: r.get(1)?,
        kind: r.get(2)?,
        role_id: blank_to_none(role_id),
        position: r.get(4)?,
    })
}

fn template_lines(conn: &Connection, template_id: &str) -> Result<Vec<AccessTemplateLine>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, template_id, kind, role_id, position
             FROM access_template_lines
             WHERE template_id = ?1
             ORDER BY position, id",
        )
        .map_err(|e| format!("skin template lines: {e}"))?;
    let rows = stmt
        .query_map(params![template_id], map_template_line)
        .map_err(|e| format!("skin template lines: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("skin template line: {e}"))?);
    }
    Ok(out)
}

fn template_by_id(conn: &Connection, id: &str) -> Result<Option<AccessTemplate>, String> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT id, name, created_at FROM access_templates WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin template lookup: {e}"))?;
    let Some((id, name, created_at)) = row else {
        return Ok(None);
    };
    let lines = template_lines(conn, &id)?;
    Ok(Some(AccessTemplate {
        id,
        name,
        created_at,
        lines,
    }))
}

fn unique_template_err(name: &str, e: rusqlite::Error) -> String {
    let s = e.to_string();
    if s.contains("UNIQUE") {
        format!("template '{name}' already exists")
    } else {
        format!("skin template insert: {e}")
    }
}

pub fn list_access_templates() -> Result<Vec<AccessTemplate>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, created_at FROM access_templates ORDER BY name COLLATE NOCASE",
            )
            .map_err(|e| format!("skin template list: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("skin template list: {e}"))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("skin template row: {e}"))?);
        }
        drop(stmt);
        let mut out = Vec::new();
        for (id, name, created_at) in ids {
            out.push(AccessTemplate {
                lines: template_lines(conn, &id)?,
                id,
                name,
                created_at,
            });
        }
        Ok(out)
    })
}

pub fn create_access_template(name: &str) -> Result<AccessTemplate, String> {
    let name = normalize_template_name(name)?;
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs().to_string();
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO access_templates (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, created_at],
        )
        .map_err(|e| unique_template_err(&name, e))?;
        Ok(AccessTemplate {
            id,
            name,
            created_at,
            lines: Vec::new(),
        })
    })
}

pub fn update_access_template(id: &str, name: &str) -> Result<AccessTemplate, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template id".to_string());
    }
    let name = normalize_template_name(name)?;
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE access_templates SET name = ?1 WHERE id = ?2",
                params![name, id],
            )
            .map_err(|e| unique_template_err(&name, e))?;
        if n == 0 {
            return Err("unknown template".to_string());
        }
        template_by_id(conn, id)?.ok_or_else(|| "unknown template".to_string())
    })
}

pub fn delete_access_template(id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template id".to_string());
    }
    with_conn(|conn| {
        conn.execute(
            "DELETE FROM access_template_lines WHERE template_id = ?1",
            params![id],
        )
        .map_err(|e| format!("skin template lines delete: {e}"))?;
        let n = conn
            .execute("DELETE FROM access_templates WHERE id = ?1", params![id])
            .map_err(|e| format!("skin template delete: {e}"))?;
        Ok(n > 0)
    })
}

fn resolve_line_role(
    conn: &Connection,
    kind: &str,
    role: Option<&str>,
) -> Result<Option<String>, String> {
    let role = role.map(str::trim).filter(|s| !s.is_empty());
    if kind == "app" {
        let Some(token) = role else {
            return Err("kind=app requires roleId".to_string());
        };
        let Some(found) = role_by_id_or_name(conn, token)? else {
            return Err(format!("unknown skin role '{token}'"));
        };
        Ok(Some(found.id))
    } else if role.is_some() {
        Err("roleId is only set for kind=app".to_string())
    } else {
        Ok(None)
    }
}

pub fn create_template_line(
    template_id: &str,
    kind: &str,
    role: Option<&str>,
) -> Result<AccessTemplateLine, String> {
    let template_id = template_id.trim();
    if template_id.is_empty() {
        return Err("missing template id".to_string());
    }
    let kind = grant_kind(kind)?;
    with_conn(|conn| {
        if template_by_id(conn, template_id)?.is_none() {
            return Err("unknown template".to_string());
        }
        let role_id = resolve_line_role(conn, &kind, role)?;
        let position: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM access_template_lines WHERE template_id = ?1",
                params![template_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin template line position: {e}"))?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO access_template_lines (id, template_id, kind, role_id, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, template_id, kind, role_id, position],
        )
        .map_err(|e| format!("skin template line insert: {e}"))?;
        Ok(AccessTemplateLine {
            id,
            template_id: template_id.to_string(),
            kind,
            role_id,
            position,
        })
    })
}

pub fn update_template_line(
    id: &str,
    kind: &str,
    role: Option<&str>,
) -> Result<AccessTemplateLine, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template line id".to_string());
    }
    let kind = grant_kind(kind)?;
    with_conn(|conn| {
        let existing: Option<String> = conn
            .query_row(
                "SELECT template_id FROM access_template_lines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin template line lookup: {e}"))?;
        let Some(template_id) = existing else {
            return Err("unknown template line".to_string());
        };
        let role_id = resolve_line_role(conn, &kind, role)?;
        conn.execute(
            "UPDATE access_template_lines SET kind = ?1, role_id = ?2 WHERE id = ?3",
            params![kind, role_id, id],
        )
        .map_err(|e| format!("skin template line update: {e}"))?;
        let position: i64 = conn
            .query_row(
                "SELECT position FROM access_template_lines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin template line lookup: {e}"))?;
        Ok(AccessTemplateLine {
            id: id.to_string(),
            template_id,
            kind,
            role_id,
            position,
        })
    })
}

pub fn delete_template_line(id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template line id".to_string());
    }
    with_conn(|conn| {
        let n = conn
            .execute(
                "DELETE FROM access_template_lines WHERE id = ?1",
                params![id],
            )
            .map_err(|e| format!("skin template line delete: {e}"))?;
        Ok(n > 0)
    })
}

/// Insert grants for the person's id. Does not update those rows when
/// the template changes later. Mailbox, database, and cli require scope
/// and reject a role. App uses the line's role_id.
pub fn apply_access_template(
    principal_id: &str,
    template_id: &str,
    lines: &[TemplateApplyLine],
) -> Result<Vec<SkinGrant>, String> {
    let principal_id = principal_id.trim();
    if principal_id.is_empty() {
        return Err("missing subject id".to_string());
    }
    let template_id = template_id.trim();
    if template_id.is_empty() {
        return Err("missing template id".to_string());
    }
    if lines.is_empty() {
        return Err("missing lines".to_string());
    }
    let planned = with_conn(|conn| {
        if !principal_id_exists(conn, principal_id)? {
            return Err(format!("unknown principal id '{principal_id}'"));
        }
        let Some(template) = template_by_id(conn, template_id)? else {
            return Err("unknown template".to_string());
        };
        let mut writes = Vec::new();
        for binding in lines {
            let line_id = binding.line_id.trim();
            let Some(line) = template.lines.iter().find(|l| l.id == line_id) else {
                return Err(format!("unknown template line '{line_id}'"));
            };
            let target_id = binding.target_id.trim();
            if target_id.is_empty() {
                return Err("missing target id".to_string());
            }
            let scope = binding
                .scope
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let binding_role = binding
                .role
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if line.kind != "app" {
                if binding_role.is_some() || line.role_id.is_some() {
                    return Err("roleId is only set for kind=app".to_string());
                }
                if scope.is_none() {
                    return Err(format!("kind={} requires scope", line.kind));
                }
                writes.push(SkinGrantWrite {
                    subject_kind: "principal".to_string(),
                    subject_id: principal_id.to_string(),
                    kind: line.kind.clone(),
                    target_id: target_id.to_string(),
                    role: None,
                    scope,
                    created_by: None,
                });
            } else {
                let Some(role_id) = line.role_id.clone() else {
                    return Err("kind=app requires roleId".to_string());
                };
                writes.push(SkinGrantWrite {
                    subject_kind: "principal".to_string(),
                    subject_id: principal_id.to_string(),
                    kind: "app".to_string(),
                    target_id: target_id.to_string(),
                    role: Some(role_id),
                    scope: None,
                    created_by: None,
                });
            }
        }
        Ok(writes)
    })?;
    let mut out = Vec::new();
    for write in &planned {
        out.push(create_grant(write)?);
    }
    Ok(out)
}

/// Handle + `project_handle_aliases` + project_id UUID only.
/// Display-name / folder-basename fallback is **not** accepted.
/// Unknown or ambiguous → error (400 at HTTP).
pub fn resolve_room_tokens(tokens: &[String]) -> Result<Vec<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut out: Vec<String> = Vec::new();
    for raw in tokens {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        match resolve_room_token(&conn, t)? {
            Some(id) => {
                if !out.iter().any(|x| x == &id) {
                    out.push(id);
                }
            }
            None => {
                return Err(format!("unknown workspace handle {t:?}"));
            }
        }
    }
    Ok(out)
}

fn resolve_room_token(conn: &Connection, token: &str) -> Result<Option<String>, String> {
    use crate::workspace_session_handles::is_uuid_shape;
    if is_uuid_shape(token) {
        let found: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE id = ?1",
                params![token],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("room lookup: {e}"))?;
        return Ok(found);
    }

    let mut handles: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM projects WHERE handle = ?1 COLLATE NOCASE \
                 AND handle IS NOT NULL AND TRIM(handle) != ''",
            )
            .map_err(|e| format!("room handle lookup: {e}"))?;
        let rows = stmt
            .query_map(params![token], |r| r.get::<_, String>(0))
            .map_err(|e| format!("room handle lookup: {e}"))?;
        for row in rows {
            handles.push(row.map_err(|e| format!("room handle row: {e}"))?);
        }
    }
    match handles.len() {
        1 => return Ok(Some(handles.remove(0))),
        n if n > 1 => {
            return Err(format!("ambiguous workspace handle {token:?}"));
        }
        _ => {}
    }

    let mut aliases: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT a.project_id FROM project_handle_aliases a \
                 WHERE a.alias = ?1 COLLATE NOCASE",
            )
            .map_err(|e| format!("room alias lookup: {e}"))?;
        let rows = stmt
            .query_map(params![token], |r| r.get::<_, String>(0))
            .map_err(|e| format!("room alias lookup: {e}"))?;
        for row in rows {
            aliases.push(row.map_err(|e| format!("room alias row: {e}"))?);
        }
    }
    match aliases.len() {
        1 => Ok(Some(aliases.remove(0))),
        n if n > 1 => Err(format!("ambiguous workspace handle {token:?}")),
        _ => Ok(None),
    }
}

/// Live handles for stored room UUIDs. Missing/retired projects are skipped.
pub fn handles_for_project_ids(ids: &[String]) -> Vec<String> {
    live_agents(ids).into_iter().map(|a| a.handle).collect()
}

/// Allowed agents whose `project_id` still exists. Skip missing.
pub fn live_agents(ids: &[String]) -> Vec<SkinAgent> {
    let db = crate::db::shared();
    let conn = db.lock();
    type Pending = (String, String, Option<String>, Option<String>, Option<String>);
    let mut pending: Vec<Pending> = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        if let Ok(h) = crate::workspace::handle::project_handle(&conn, id) {
            let h = h.trim().to_string();
            if !h.is_empty() {
                let (path, color, icon): (Option<String>, Option<String>, Option<String>) = conn
                    .query_row(
                        "SELECT path, color, icon_url FROM projects WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .unwrap_or((None, None, None));
                let path = path
                    .map(|s: String| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                pending.push((id.to_string(), h, path, color, icon));
            }
        }
    }
    drop(conn);
    pending
        .into_iter()
        .map(|(project_id, handle, path, color, icon)| {
            let display_name = path
                .map(|p| crate::workspace::display::agent_display_name(&p))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| handle.clone());
            SkinAgent {
                handle,
                project_id,
                display_name,
                color: color
                    .and_then(|c| crate::workspace::agent_look::normalize_color(&c).ok()),
                image: crate::workspace::agent_look::guest_image_view(icon.as_deref()),
            }
        })
        .collect()
}

/// Resolve a presented raw key. Revoked / expired / unknown / wrong prefix → `None`.
/// A login session (`session=1`) also needs its principal and an enabled
/// `kind=app` grant **for the App the session is bound to** (S24-5; NULL
/// on a pre-0.45.5 row = `host`) at the moment of use: a backstop behind the
/// revocations the grant writers do. Its rooms and caps are that grant's at
/// the moment of use ([`live_session`]). Platform passes
/// (`session=0`) skip both.
/// An App **link pass** (`link_id` set, runtime S14) is never returned
/// here: it is valid only on its own link ([`link::resolve_link_pass`]).
pub fn resolve_skin_token(presented_raw: &str) -> Option<SkinPass> {
    try_resolve_skin_token(presented_raw).ok().flatten()
}

/// [`resolve_skin_token`] that tells "no such live pass" (`Ok(None)`) from "the
/// database couldn't say" (`Err`). An open socket must not read an error as a
/// dead pass: that tells a person to sign in again for nothing.
pub fn try_resolve_skin_token(presented_raw: &str) -> Result<Option<SkinPass>, String> {
    resolve_token(presented_raw, true)
}

/// [`try_resolve_skin_token`] for K2's own re-check of an open socket: nobody used
/// the session, so it doesn't count as "last used".
pub fn peek_skin_token(presented_raw: &str) -> Result<Option<SkinPass>, String> {
    resolve_token(presented_raw, false)
}

fn resolve_token(presented_raw: &str, used: bool) -> Result<Option<SkinPass>, String> {
    if !is_skin_token(presented_raw) || is_api_key_prefix(presented_raw) {
        return Ok(None);
    }
    resolve_token_by(TokenKey::Hash(sha256_hex(presented_raw)), used)
}

/// A pass by its id, as it would resolve now (its rooms, caps and folders
/// read again; `None` once revoked, expired or its grant is gone). For a
/// holder K2 keeps no secret of: a resumable transfer re-reads the pass
/// that made it on every request (`fs_transfer`). Never a link pass (those
/// are the App's own and rotate hourly). Doesn't count as the pass being
/// used.
pub fn resolve_skin_token_id(id: &str) -> Result<Option<SkinPass>, String> {
    let id = id.trim();
    if id.is_empty() {
        return Ok(None);
    }
    resolve_token_by(TokenKey::Id(id.to_string()), false)
}

/// Which column finds the token row.
enum TokenKey {
    Hash(String),
    Id(String),
}

fn resolve_token_by(key: TokenKey, used: bool) -> Result<Option<SkinPass>, String> {
    let (key_sql, key_value) = match key {
        TokenKey::Hash(h) => ("t.key_hash = ?1", h),
        TokenKey::Id(id) => ("t.id = ?1", id),
    };
    let now = now_secs();
    let row = with_conn(|conn| {
        let row = conn
            .query_row(
                &format!("SELECT t.id, t.principal_id, t.session, t.name, p.username, t.caps, t.rooms, t.room_policy,
                        t.app_service_id,
                        EXISTS (
                          SELECT 1 FROM grants g
                          WHERE g.subject_kind = 'principal' AND g.subject_id = t.principal_id
                            AND g.kind = 'app' AND g.target_id = COALESCE(t.app_service_id, ?3)
                            AND g.enabled != 0),
                        EXISTS (
                          SELECT 1 FROM grants g
                          WHERE g.subject_kind = 'principal' AND g.subject_id = t.principal_id
                            AND g.kind = ?4 AND g.target_id = ?5 AND g.enabled != 0),
                        tie.workspace_id, tie.host_ok, tie.reasons, tie.set_by, tie.set_at,
                        t.expires_at
                 FROM tokens t
                 LEFT JOIN principals p ON p.id = t.principal_id
                 LEFT JOIN principal_agent_ties tie ON tie.principal_id = t.principal_id AND t.session = 1
                 WHERE {key_sql} AND t.revoked_at IS NULL
                   AND (t.expires_at IS NULL OR t.expires_at > ?2)
                   AND t.link_id IS NULL
                   AND (t.session = 0 OR p.id IS NOT NULL)"),
                params![key_value, now, HOST_APP_ID, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
                |r| {
                    let principal_id: Option<String> = r.get(1)?;
                    let session: i64 = r.get(2)?;
                    let token_name: Option<String> = r.get(3)?;
                    let principal_username: Option<String> = r.get(4)?;
                    let stamp = if session != 0 {
                        principal_username.unwrap_or_default()
                    } else {
                        token_name.unwrap_or_default()
                    };
                    let caps_raw: String = r.get(5)?;
                    let rooms_raw: String = r.get(6)?;
                    let policy_raw: Option<String> = r.get(7)?;
                    let app_raw: Option<String> = r.get(8)?;
                    let caps = caps_from_json(&caps_raw);
                    let rooms = rooms_from_json(&rooms_raw);
                    let session_b = session != 0;
                    let room_policy = interpret_token_policy(
                        session_b,
                        &caps,
                        &rooms,
                        policy_raw.as_deref(),
                    );
                    let (caps, rooms) = if session_b {
                        (union_caps(&room_policy), rooms_from_policy(&room_policy))
                    } else {
                        (caps, rooms)
                    };
                    let app_ok: bool = r.get(9)?;
                    let chat_ok: bool = r.get(10)?;
                    let tie = match (principal_id.as_ref(), r.get::<_, Option<String>>(11)?) {
                        (Some(pid), Some(ws)) if session != 0 => Some(ties::Tie {
                            principal_id: pid.clone(),
                            workspace_id: ws,
                            host_ok: r.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
                            reasons: serde_json::from_str(&r.get::<_, Option<String>>(13)?.unwrap_or_default()).unwrap_or_default(),
                            set_by: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
                            set_at: r.get::<_, Option<i64>>(15)?.unwrap_or(0),
                        }),
                        _ => None,
                    };
                    let app = if session_b {
                        blank_to_none(app_raw).unwrap_or_else(|| HOST_APP_ID.to_string())
                    } else {
                        String::new()
                    };
                    // A login session needs its App's enabled grant — or,
                    // for a chat-only box login (S28 §13), an enabled
                    // `chat` grant AND an empty room policy, so a session
                    // minted under a host grant never outlives that grant
                    // with its rooms.
                    let admitted = !session_b
                        || app_ok
                        || (chat_ok && app == HOST_APP_ID && room_policy.is_empty());
                    if !admitted {
                        return Ok(None);
                    }
                    Ok(Some((
                        SkinPass {
                            id: r.get(0)?,
                            principal_id,
                            username: stamp,
                            caps,
                            rooms,
                            session: session_b,
                            room_policy,
                            app,
                            room_paths: Default::default(),
                            expires_at: r.get(16)?,
                            chat: chat_ok,
                        },
                        tie,
                    )))
                },
            )
            .optional()
            .map_err(|e| format!("skin token resolve: {e}"))?
            .flatten();
        let Some((mut pass, tie)) = row else { return Ok(None) };
        if pass.session {
            // 0.46.8 (Rosson 2026-11-02): new access at the next request, no
            // sign-out. A login's rooms and caps are its grant's, now, like
            // its folders (R3); the stored copy only says what it was when
            // last written. A role that can't be read fails closed.
            match live_session(conn, &pass)? {
                Some(live) => {
                    pass.caps = union_caps(&live.policy);
                    pass.rooms = rooms_from_policy(&live.policy);
                    pass.room_policy = live.policy;
                    pass.room_paths = live.paths;
                }
                None => pass.room_paths = paths::session_paths(conn, &pass),
            }
            // app-user-profile §3.7: last used, in memory only.
            if used {
                profile::touch(&pass.id);
            }
        }
        let app_rooms = if tie.is_some() { ties::app_rooms_conn(conn, &pass.app)? } else { Vec::new() };
        Ok(Some((pass, tie, app_rooms)))
    })?;
    Ok(row.and_then(|(pass, tie, app_rooms)| match tie {
        None => Some(pass),
        // agent-app-users R2 (A2): a tied guest's live session is held to
        // the ceiling on every use. The App's workspace is read after the
        // skin mutex is released (A21).
        Some(tie) => clamp_resolved(pass, &tie, &app_rooms),
    }))
}

/// One App's guests, each as a session of theirs on this App would resolve now
/// (`scope=guests`, `prd-live-access-changes-v1` S7). K2 doesn't hold those guests'
/// passes, so it asks by person. A guest with no enabled grant here isn't listed.
/// One person per `with_conn`, so other requests get in between. No token is read:
/// `id` is empty and `expires_at` is `None`.
pub fn app_guests_now(service_id: &str) -> Result<Vec<SkinPass>, String> {
    let guests: Vec<(String, String, bool)> = with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT p.id, p.username,
                        EXISTS (SELECT 1 FROM grants c WHERE c.subject_kind = 'principal' AND c.subject_id = p.id
                                  AND c.kind = ?2 AND c.target_id = ?3 AND c.enabled != 0)
                 FROM grants g JOIN principals p ON p.id = g.subject_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1 AND g.enabled != 0",
            )
            .map_err(|e| format!("app guests: {e}"))?;
        let rows = st
            .query_map(params![service_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| format!("app guests: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("app guests: {e}"))
    })?;
    let mut out = Vec::with_capacity(guests.len());
    for (principal_id, username, chat) in guests {
        let mut pass = SkinPass {
            id: String::new(),
            principal_id: Some(principal_id.clone()),
            username,
            caps: Vec::new(),
            rooms: Vec::new(),
            session: true,
            room_policy: RoomPolicy::new(),
            app: service_id.to_string(),
            room_paths: Default::default(),
            expires_at: None,
            chat,
        };
        let answer = with_conn(|conn| {
            let Some(live) = live_session(conn, &pass)? else { return Ok(None) };
            let tie = ties::get_conn(conn, &principal_id)?;
            let app_rooms = if tie.is_some() { ties::app_rooms_conn(conn, service_id)? } else { Vec::new() };
            Ok(Some((live, tie, app_rooms)))
        })?;
        let Some((live, tie, app_rooms)) = answer else { continue };
        pass.caps = union_caps(&live.policy);
        pass.rooms = rooms_from_policy(&live.policy);
        pass.room_policy = live.policy;
        pass.room_paths = live.paths;
        // After the skin mutex is released, as in a resolve (A21).
        let held = match tie {
            None => Some(pass),
            Some(tie) => clamp_resolved(pass, &tie, &app_rooms),
        };
        out.extend(held);
    }
    Ok(out)
}

/// What a login session has right now, on one read of its App's enabled grant:
/// the policy ([`grant_login_policy`], what a fresh sign-in would get) and its
/// folders ([`paths::session_paths`]).
struct LiveSession {
    policy: RoomPolicy,
    paths: paths::PassPaths,
}

/// One query, because this runs at every request of a signed-in person: the grant
/// and its role. `None`: no such grant (a chat-only box login, S28 §13, whose
/// stored policy is empty), or no principal; the stored row stands. A grant whose
/// role doesn't join (a name on an old row, a role that's gone) takes the slower
/// lookups, which fail closed.
fn live_session(conn: &Connection, pass: &SkinPass) -> Result<Option<LiveSession>, String> {
    let Some(principal_id) = pass.principal_id.as_deref() else { return Ok(None) };
    let row = conn
        .query_row(
            "SELECT g.id, g.subject_kind, g.subject_id, g.kind, g.target_id, g.role_id, g.scope, g.created_at, g.enabled, g.created_by,
                    r.id, r.name, r.caps, r.rooms, r.created_at, COALESCE(r.room_policy, '{}'), r.app_id, r.room_paths
             FROM grants g LEFT JOIN roles r ON r.id = g.role_id
             WHERE g.subject_kind = 'principal' AND g.subject_id = ?1 AND g.kind = 'app' AND g.target_id = ?2 AND g.enabled != 0",
            params![principal_id, pass.app],
            |r| {
                let grant = map_grant_row(r)?;
                let joined: Option<String> = r.get(10)?;
                let role = match joined {
                    Some(_) => Some((map_role_row_at(r, 10)?, r.get::<_, Option<String>>(17)?)),
                    None => None,
                };
                Ok((grant, role))
            },
        )
        .optional()
        .map_err(|e| format!("skin live session: {e}"))?;
    let Some((grant, role)) = row else { return Ok(None) };
    match (grant.role_id.as_deref(), role) {
        (None, _) => Ok(Some(LiveSession {
            policy: thread_only_policy(&parse_rooms_json(grant.scope.as_deref())),
            paths: paths::PassPaths::Whole,
        })),
        (Some(_), Some((role, raw_paths))) => Ok(Some(LiveSession {
            paths: match paths::parse_room_paths(raw_paths.as_deref()) {
                Ok(lines) => paths::PassPaths::Lines(lines),
                Err(e) => paths::PassPaths::Unreadable(e),
            },
            policy: role.room_policy,
        })),
        (Some(_), None) => Ok(Some(LiveSession {
            policy: grant_login_policy(conn, &grant)?.0,
            paths: paths::session_paths(conn, pass),
        })),
    }
}

/// [`resolve_skin_token`]'s clamp for a tied guest's session: `None` when
/// its App is outside the ceiling, else its policy cut to the tie's room
/// and the rooms the owner approved for its App (`app_rooms`).
fn clamp_resolved(mut pass: SkinPass, tie: &ties::Tie, app_rooms: &[String]) -> Option<SkinPass> {
    let app_ws = ties::app_workspace(&pass.app);
    let policy = std::mem::take(&mut pass.room_policy);
    match ties::clamp(Some(tie), &pass.app, app_ws.as_deref(), app_rooms, policy) {
        Ok(p) => {
            pass.caps = union_caps(&p);
            pass.rooms = rooms_from_policy(&p);
            pass.room_policy = p;
            Some(pass)
        }
        Err(ties::Denied) => None,
    }
}

// ── Front door ───────────────────────────────────────────────────────

pub fn get_front_door() -> Result<SkinFrontDoor, String> {
    with_conn(|conn| {
        let stored = conn
            .query_row(
                "SELECT mode, url, hint, ui_port FROM front_door WHERE id = 1",
                [],
                |r| {
                    let ui: Option<i64> = r.get(3)?;
                    Ok(SkinFrontDoor {
                        mode: r.get(0)?,
                        url: r.get(1)?,
                        hint: r.get(2)?,
                        ui_port: ui.and_then(|n| u16::try_from(n).ok()),
                    })
                },
            )
            .optional()
            .map_err(|e| format!("skin front-door: {e}"))?;
        Ok(stored.unwrap_or_else(SkinFrontDoor::default_connect))
    })
}

/// True when a front-door row has been persisted (boot apply uses this).
pub fn front_door_is_stored() -> bool {
    with_conn(|conn| {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM front_door WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        Ok(n > 0)
    })
    .unwrap_or(false)
}

pub fn set_front_door(
    mode: &str,
    url: Option<&str>,
    hint: Option<&str>,
    ui_port: Option<u16>,
) -> Result<SkinFrontDoor, String> {
    let mode = parse_front_door_mode(mode)?;
    let url_stored = url.map(str::trim).filter(|s| !s.is_empty());
    let hint_stored = hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_hint_for_mode(mode).to_string());
    let ui_stored: Option<i64> = ui_port.map(|p| p as i64);
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO front_door (id, mode, url, hint, ui_port, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               mode = excluded.mode,
               url = excluded.url,
               hint = excluded.hint,
               ui_port = excluded.ui_port,
               updated_at = excluded.updated_at",
            params![mode, url_stored, hint_stored, ui_stored, now_secs()],
        )
        .map_err(|e| format!("skin front-door persist: {e}"))?;
        Ok(SkinFrontDoor {
            mode: mode.to_string(),
            url: url_stored.map(|s| s.to_string()),
            hint: Some(hint_stored),
            ui_port,
        })
    })
}

/// Effective front-door view: persisted mode plus derived Connect URL
/// (`https://skin.<sub>.k2.dev`) when mode is `connect` and no URL was stored.
pub fn effective_front_door() -> Result<SkinFrontDoor, String> {
    let mut door = get_front_door()?;
    if door.mode != "connect" && door.mode != "direct" {
        door.mode = "connect".to_string();
    }
    if door.hint.as_deref().map(str::trim).unwrap_or("").is_empty() {
        door.hint = Some(default_hint_for_mode(&door.mode).to_string());
    }
    if door.mode == "connect" && door.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
        door.url = crate::tunnel::config::load()
            .ok()
            .and_then(|c| c.public_url())
            .and_then(|u| skin_url_from_public(&u));
    }
    Ok(door)
}

#[cfg(test)]
#[path = "skin_tests.rs"]
mod tests;

// Sew S28 K7/K8: chat grant, chat-only login, revoke ops, Connect ids.
#[cfg(test)]
#[path = "skin_sew_tests.rs"]
mod sew_tests;
