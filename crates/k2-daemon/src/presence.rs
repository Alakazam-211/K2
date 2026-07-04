//! S1 — daemon presence registry (presence/multiplayer arc).
//!
//! Process-wide "who is connected to this daemon right now" registry.
//! **"Connected" = holds a `/cli/sessions/events` WebSocket** — every
//! window (local owner or K2 Connect remote) opens the app-level one at
//! boot, and the per-workspace subscriptions announce *where* the user
//! is working via their `?path=` param. `session_events_ws.rs` resolves
//! the connection's identity at upgrade time ([`resolve_identity`]) and
//! registers/deregisters here; every membership change re-broadcasts the
//! whole aggregated roster as an app-level
//! [`SessionEvent::PresenceChanged`] (whole-set, last-write-wins — the
//! ActiveChanged convention), and `GET /cli/presence/roster`
//! ([`handle_roster`]) serves the identical JSON as the
//! reconnect-reconcile snapshot.
//!
//! Storage follows the `session_events.rs` OnceLock/static pattern:
//! a `Mutex<HashMap<conn_id, PresenceEntry>>` singleton plus an
//! (S4-filled) `granted` username set for ephemeral edit grants — the
//! roster carries `grantedEdit` NOW so the wire shape is stable before
//! the grant routes exist.
//!
//! Each entry holds a `tokio::sync::watch` close handle so a moderator
//! action (S3 kick) can tear down a user's live sockets immediately via
//! [`close_connections_for`] instead of waiting for the WS loop's 5s
//! re-auth tick. Registration hands back a [`PresenceGuard`] whose
//! `Drop` deregisters — the WS handler holds it across the connection
//! loop so panics/early-returns can never leak an entry.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use k2_core::connect_users::Role;
use k2_core::log_debug;

use crate::session_events::{self, SessionEvent};

/// Who is on the other end of a registered connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceIdentity {
    /// The host machine's owner — authorized by the daemon token, which
    /// maps to no user row / no session record (see `actor_role`). The
    /// roster synthesizes the `"owner"` row from these entries.
    Owner,
    /// A K2 Connect user authorized by a live session token.
    ConnectUser { username: String, role: Role },
}

/// What kind of `/cli/sessions/events` subscription the entry is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceKind {
    /// The app-level subscription (`?path=` empty) every window opens at
    /// boot. `windowCount` in the roster counts exactly these.
    AppSocket,
    /// A per-workspace subscription (`?path=<workspace>`) — the signal
    /// for *where* the user is working. Deduped paths feed the roster's
    /// `workspaces` list.
    WorkspaceSocket { path: String },
}

/// One live registered connection.
struct PresenceEntry {
    identity: PresenceIdentity,
    kind: PresenceKind,
    /// Unix seconds at registration. The roster surfaces the EARLIEST
    /// across a user's entries as `connectedAt`.
    connected_at: i64,
    /// Close handle: `send(true)` asks the owning WS loop to shut the
    /// socket down now (it selects on the paired receiver). Fired by
    /// [`close_connections_for`] (S3 kick).
    close: watch::Sender<bool>,
}

/// One aggregated PER-USER roster row. **Wire-frozen** (serialized both
/// inside `SessionEvent::PresenceChanged` and by `GET
/// /cli/presence/roster`): `{ "user": "owner"|username, "role":
/// "owner"|"admin"|"member", "windowCount": number, "workspaces":
/// string[], "grantedEdit": bool, "connectedAt": number }`. camelCase
/// field names follow the SessionEvent convention.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RosterUser {
    /// `"owner"` for the synthesized owner row, else the username.
    pub user: String,
    /// Wire role string (`Role::as_wire`): `"owner"|"admin"|"member"`
    /// (+ `"viewer"` once S4 lands the role).
    pub role: String,
    /// Number of app-level sockets — i.e. open windows (each window
    /// holds exactly one). Workspace subscriptions don't count.
    #[serde(rename = "windowCount")]
    pub window_count: usize,
    /// Deduped, sorted workspace paths from the user's
    /// `WorkspaceSocket` entries.
    pub workspaces: Vec<String>,
    /// Whether the user currently holds an ephemeral edit grant (S4
    /// fills the set; always `false` until then; always `false` for
    /// the owner, who needs no grant).
    #[serde(rename = "grantedEdit")]
    pub granted_edit: bool,
    /// Unix seconds of the user's EARLIEST live connection.
    #[serde(rename = "connectedAt")]
    pub connected_at: i64,
}

static REGISTRY: OnceLock<Mutex<HashMap<u64, PresenceEntry>>> = OnceLock::new();

/// Ephemeral edit grants (S4): usernames currently granted edit access.
/// Empty in S1 — declared now so the roster's `grantedEdit` field ships
/// with a stable wire shape before the grant routes exist.
static GRANTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<HashMap<u64, PresenceEntry>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn granted() -> &'static Mutex<HashSet<String>> {
    GRANTED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// RAII deregistration handle. The WS handler holds this for the life
/// of the connection; dropping it (normal exit, `break`, early return,
/// panic unwind, or the task being dropped at runtime shutdown) removes
/// the entry and re-broadcasts the roster. Entries therefore cannot
/// leak past their connection.
pub struct PresenceGuard {
    conn_id: u64,
}

impl PresenceGuard {
    pub fn conn_id(&self) -> u64 {
        self.conn_id
    }
}

impl Drop for PresenceGuard {
    fn drop(&mut self) {
        deregister(self.conn_id);
    }
}

/// Resolve a connection's identity from the `?token=` it authorized
/// with (the dispatcher already gated the upgrade, so this is a
/// classification, not an auth check — precedent: `/cli/auth/whoami`):
/// owner token → [`PresenceIdentity::Owner`]; a live connect-user
/// session → username + stored role; anything else (shouldn't happen
/// post-gate, but a session can be revoked between gate and here) →
/// `None`, and the caller skips registration.
pub fn resolve_identity(token: &str, owner_token: &str) -> Option<PresenceIdentity> {
    if token.is_empty() {
        return None;
    }
    if crate::routes::http::ct_eq_token(token, owner_token) {
        return Some(PresenceIdentity::Owner);
    }
    let username = k2_core::connect_users::validate_session(token)?;
    let role = k2_core::connect_users::role_for_user(&username)
        .unwrap_or(Role::Member);
    Some(PresenceIdentity::ConnectUser { username, role })
}

/// Register a live connection. `close` is the handler-owned watch
/// sender whose paired receiver the WS loop selects on —
/// [`close_connections_for`] fires it. Recomputes + broadcasts the
/// roster. Returns the [`PresenceGuard`] whose `Drop` deregisters.
pub fn register(
    identity: PresenceIdentity,
    kind: PresenceKind,
    close: watch::Sender<bool>,
) -> PresenceGuard {
    let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
    {
        let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
        map.insert(
            conn_id,
            PresenceEntry {
                identity,
                kind,
                connected_at: unix_now_secs(),
                close,
            },
        );
    }
    broadcast_roster();
    PresenceGuard { conn_id }
}

/// Remove a connection and re-broadcast the roster. No-op (no
/// broadcast) when the id is already gone. Called from
/// [`PresenceGuard::drop`]; not part of the handler-facing API.
///
/// S4 auto-revoke: grants live only as long as the connection (PRD §4
/// grant lifecycle) — when the departing entry was the user's LAST live
/// connection, their ephemeral edit grant is dropped here, so a viewer
/// who disconnects always reconnects ungranted.
fn deregister(conn_id: u64) {
    let removed = {
        let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&conn_id)
    };
    let Some(entry) = removed else { return };
    if let PresenceIdentity::ConnectUser { username, .. } = &entry.identity {
        if !has_live_connection(username) {
            let revoked = {
                let mut set = granted().lock().unwrap_or_else(|e| e.into_inner());
                set.remove(username)
            };
            if revoked {
                log_debug!(
                    "[daemon/presence] auto-revoked edit grant for '{username}' \
                     (last connection deregistered)"
                );
            }
        }
    }
    broadcast_roster();
}

/// Whether `username` currently holds ANY live registered connection
/// (app-level or workspace). The grant route uses this to enforce
/// "a grant must attach to a live presence".
pub fn has_live_connection(username: &str) -> bool {
    let map = registry().lock().unwrap_or_else(|e| e.into_inner());
    map.values().any(|e| {
        matches!(&e.identity, PresenceIdentity::ConnectUser { username: u, .. } if u == username)
    })
}

/// Grant or revoke `username`'s ephemeral edit access (S4) and
/// re-broadcast the roster so every window's `grantedEdit` updates live.
/// No-op (no broadcast) when the set already matches. Target validation
/// (viewer role, live connection) lives in [`handle_grant`]; this is the
/// raw mutation — the auto-revoke in [`deregister`] is the only other
/// writer of the set.
pub fn set_granted(username: &str, granted_flag: bool) {
    let changed = {
        let mut set = granted().lock().unwrap_or_else(|e| e.into_inner());
        if granted_flag {
            set.insert(username.to_string())
        } else {
            set.remove(username)
        }
    };
    if changed {
        log_debug!(
            "[daemon/presence] edit grant for '{username}' set to {granted_flag}"
        );
        broadcast_roster();
    }
}

/// Fire the close handle of every live connection belonging to
/// `username` (connect-users only — the owner is not kickable). Returns
/// how many were fired. Entries are NOT removed here: each WS loop
/// observes its handle, closes the socket, and its guard deregisters —
/// so the roster update reflects sockets that actually died. Unused by
/// routes in S1; S3's kick route calls it so revocation doesn't wait
/// for the 5s re-auth tick.
pub fn close_connections_for(username: &str) -> usize {
    let map = registry().lock().unwrap_or_else(|e| e.into_inner());
    let mut fired = 0usize;
    for entry in map.values() {
        if let PresenceIdentity::ConnectUser { username: u, .. } = &entry.identity {
            if u == username {
                let _ = entry.close.send(true);
                fired += 1;
            }
        }
    }
    if fired > 0 {
        log_debug!(
            "[daemon/presence] close_connections_for({username}) fired {fired} handle(s)"
        );
    }
    fired
}

/// Compute the aggregated PER-USER roster (see [`RosterUser`] for the
/// wire shape). Multiple windows/sockets for one user collapse into one
/// row: `windowCount` counts app-level sockets, `workspaces` dedupes
/// the workspace-socket paths, `connectedAt` is the earliest. All owner
/// windows aggregate into the single synthesized `"owner"` row. Rows
/// are sorted owner-first then by username so snapshots are
/// deterministic.
pub fn roster() -> Vec<RosterUser> {
    let map = registry().lock().unwrap_or_else(|e| e.into_inner());
    let granted_set = granted().lock().unwrap_or_else(|e| e.into_inner());

    // Aggregate: user key → (role, window_count, workspaces, earliest).
    let mut agg: HashMap<String, (String, usize, HashSet<String>, i64)> = HashMap::new();
    for entry in map.values() {
        let (user, role) = match &entry.identity {
            PresenceIdentity::Owner => ("owner".to_string(), "owner".to_string()),
            PresenceIdentity::ConnectUser { username, role } => {
                (username.clone(), role.as_wire().to_string())
            }
        };
        let slot = agg
            .entry(user)
            .or_insert_with(|| (role, 0, HashSet::new(), entry.connected_at));
        match &entry.kind {
            PresenceKind::AppSocket => slot.1 += 1,
            PresenceKind::WorkspaceSocket { path } => {
                slot.2.insert(path.clone());
            }
        }
        if entry.connected_at < slot.3 {
            slot.3 = entry.connected_at;
        }
    }

    let mut rows: Vec<RosterUser> = agg
        .into_iter()
        .map(|(user, (role, window_count, workspaces, connected_at))| {
            let granted_edit = user != "owner" && granted_set.contains(&user);
            let mut workspaces: Vec<String> = workspaces.into_iter().collect();
            workspaces.sort();
            RosterUser {
                user,
                role,
                window_count,
                workspaces,
                granted_edit,
                connected_at,
            }
        })
        .collect();
    // Owner first, then usernames ascending — deterministic snapshots.
    rows.sort_by(|a, b| {
        let a_owner = a.user == "owner";
        let b_owner = b.user == "owner";
        b_owner.cmp(&a_owner).then_with(|| a.user.cmp(&b.user))
    });
    rows
}

/// Recompute + emit the whole-set [`SessionEvent::PresenceChanged`]
/// (app-level; `event_matches_workspace` forwards it to every
/// subscriber). `let _ =` swallows the zero-subscriber case per the
/// session_events convention.
fn broadcast_roster() {
    let _ = session_events::emit(SessionEvent::PresenceChanged { roster: roster() });
}

/// `GET /cli/presence/roster` — snapshot of the live roster, identical
/// JSON to the `presence_changed` event payload (`{ "roster": [...] }`)
/// so clients fetched-on-`hello` reconcile against the same shape.
/// Auth (owner token OR live connect session) is gated in the
/// dispatcher, mirroring `/cli/auth/whoami`.
pub fn handle_roster() -> crate::cli_response::CliResponse {
    match serde_json::to_string(&serde_json::json!({ "roster": roster() })) {
        Ok(body) => crate::cli_response::CliResponse::ok_json(body),
        Err(e) => crate::cli_response::CliResponse::internal_error(format!(
            "serialize roster: {e}"
        )),
    }
}

/// Body of `POST /cli/presence/kick`.
#[derive(Deserialize)]
struct KickReq {
    username: String,
}

/// `POST /cli/presence/kick` `{"username"}` (S3 — presence/multiplayer
/// §5.2) → `{"success":true,"revokedSessions":n,"closedConnections":n}`.
///
/// The dispatcher already gated the route via `require_manage` (owner
/// token OR a managing Admin/Owner session) and hands us the actor's
/// resolved role. Here we enforce the KICK matrix (PRD §4):
///
/// - the host owner is never a target — `"owner"` names the synthesized
///   roster row backed by the daemon token, not a connect-user row;
/// - unknown username → 400 (consistent with set-disabled's neighbors);
/// - `can_act_on(actor, target)` must hold, AND — stricter than the
///   disable/remove routes — an Admin may NOT kick a fellow Admin
///   ("✓ (not owner/admin)" in the matrix). `can_act_on` itself is shared
///   by set-disabled where Admin→Admin IS allowed, so the extra tier
///   check lives here rather than in the shared predicate.
///
/// Action = both halves of revocation: delete the target's persisted
/// sessions (durable — the next login/validate fails) then fire the
/// live close handles (immediate — no 5s re-auth wait). Roster
/// broadcasts follow automatically as the WS loops deregister.
pub fn handle_kick(
    actor: Role,
    body: &[u8],
) -> crate::cli_response::CliResponse {
    use crate::cli_response::CliResponse;

    let req: KickReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if req.username == "owner" {
        return CliResponse::bad_request("the owner cannot be kicked");
    }
    let target = match k2_core::connect_users::role_for_user(&req.username) {
        Some(r) => r,
        None => {
            return CliResponse::bad_request(format!(
                "unknown user '{}'",
                req.username
            ))
        }
    };
    let admin_on_admin = actor == Role::Admin && target == Role::Admin;
    if !k2_core::connect_users::can_act_on(actor, target) || admin_on_admin {
        return CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({
                "error": format!(
                    "a {} cannot kick '{}' ({})",
                    actor.as_wire(),
                    req.username,
                    target.as_wire()
                )
            })
            .to_string(),
        };
    }

    // Durable half first (a racing reconnect can't re-auth), then the
    // immediate socket teardown.
    let revoked = k2_core::connect_users::revoke_user_sessions(&req.username);
    let closed = close_connections_for(&req.username);
    log_debug!(
        "[daemon/presence] kick '{}' by {}: revoked {revoked} session(s), closed {closed} connection(s)",
        req.username,
        actor.as_wire()
    );
    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "revokedSessions": revoked,
            "closedConnections": closed,
        })
        .to_string(),
    )
}

#[derive(serde::Deserialize)]
struct GrantReq {
    username: String,
    granted: bool,
}

/// `POST /cli/presence/grant` (S4) — body `{"username":..,"granted":bool}`
/// → `{"success":true,"username":..,"granted":..}`.
///
/// The dispatcher gates method (POST-only) + authorization
/// (`require_manage`: owner token OR Admin/Owner session) and passes the
/// actor's resolved role in. Here we validate the TARGET:
///
/// - must exist AND have role Viewer — grants only apply to viewer-role
///   users; Member and above can already edit (400 otherwise);
/// - must hold a live presence connection — grants attach to a live
///   connection and die with it (the [`deregister`] auto-revoke), so
///   granting a disconnected user is meaningless (400);
/// - `can_act_on(actor, target)` re-checked for defense in depth (403) —
///   trivially true today since the target is always Viewer.
pub fn handle_grant(
    actor_role: Role,
    body: &[u8],
) -> crate::cli_response::CliResponse {
    use crate::cli_response::CliResponse;

    let req: GrantReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let Some(target_role) = k2_core::connect_users::role_for_user(&req.username) else {
        return CliResponse::bad_request(format!("user '{}' not found", req.username));
    };
    if target_role != Role::Viewer {
        return CliResponse::bad_request(format!(
            "user '{}' has role '{}' — edit grants only apply to viewer-role users \
             (members and above can already edit)",
            req.username,
            target_role.as_wire()
        ));
    }
    if !k2_core::connect_users::can_act_on(actor_role, target_role) {
        return CliResponse::forbidden();
    }
    if !has_live_connection(&req.username) {
        return CliResponse::bad_request(format!(
            "user '{}' is not connected — an edit grant attaches to a live \
             connection and is revoked when it closes",
            req.username
        ));
    }
    set_granted(&req.username, req.granted);
    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "username": req.username,
            "granted": req.granted,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is process-global; serialize the unit tests in this
    /// module so aggregation assertions don't see each other's entries.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ch() -> watch::Sender<bool> {
        watch::channel(false).0
    }

    fn member(name: &str) -> PresenceIdentity {
        PresenceIdentity::ConnectUser {
            username: name.into(),
            role: Role::Member,
        }
    }

    /// FROZEN WIRE CONTRACT: RosterUser serializes to EXACTLY
    /// `{ user, role, windowCount, workspaces, grantedEdit, connectedAt }`.
    /// S2's renderer store codes against these names.
    #[test]
    fn roster_user_serializes_to_frozen_contract() {
        let row = RosterUser {
            user: "alice".into(),
            role: "member".into(),
            window_count: 2,
            workspaces: vec!["/x/foo".into()],
            granted_edit: false,
            connected_at: 1_700_000_000,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&row).unwrap()).unwrap();
        assert_eq!(json["user"], "alice");
        assert_eq!(json["role"], "member");
        assert_eq!(json["windowCount"], 2);
        assert_eq!(json["workspaces"], serde_json::json!(["/x/foo"]));
        assert_eq!(json["grantedEdit"], false);
        assert_eq!(json["connectedAt"], 1_700_000_000i64);
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 6, "unexpected key set: {obj:?}");
        // No snake_case leakage.
        assert!(json.get("window_count").is_none());
        assert!(json.get("granted_edit").is_none());
        assert!(json.get("connected_at").is_none());
    }

    #[test]
    fn owner_windows_aggregate_into_one_row_with_window_count() {
        let _g = lock();
        let g1 = register(PresenceIdentity::Owner, PresenceKind::AppSocket, ch());
        let g2 = register(PresenceIdentity::Owner, PresenceKind::AppSocket, ch());
        let rows = roster();
        let owner_rows: Vec<_> = rows.iter().filter(|r| r.user == "owner").collect();
        assert_eq!(owner_rows.len(), 1, "one aggregated owner row: {rows:?}");
        assert_eq!(owner_rows[0].role, "owner");
        assert_eq!(owner_rows[0].window_count, 2);
        assert!(!owner_rows[0].granted_edit);
        drop(g1);
        drop(g2);
        assert!(
            roster().iter().all(|r| r.user != "owner"),
            "guards must deregister on drop"
        );
    }

    #[test]
    fn workspace_sockets_dedupe_paths_and_do_not_count_as_windows() {
        let _g = lock();
        let g1 = register(member("bob"), PresenceKind::AppSocket, ch());
        let g2 = register(
            member("bob"),
            PresenceKind::WorkspaceSocket { path: "/x/foo".into() },
            ch(),
        );
        let g3 = register(
            member("bob"),
            PresenceKind::WorkspaceSocket { path: "/x/foo".into() },
            ch(),
        );
        let g4 = register(
            member("bob"),
            PresenceKind::WorkspaceSocket { path: "/x/bar".into() },
            ch(),
        );
        let rows = roster();
        let bob = rows
            .iter()
            .find(|r| r.user == "bob")
            .expect("bob must be in the roster");
        assert_eq!(bob.window_count, 1, "workspace sockets aren't windows");
        assert_eq!(bob.workspaces, vec!["/x/bar".to_string(), "/x/foo".to_string()]);
        assert_eq!(bob.role, "member");
        drop((g1, g2, g3, g4));
        assert!(roster().iter().all(|r| r.user != "bob"));
    }

    #[test]
    fn close_connections_for_fires_only_that_users_handles() {
        let _g = lock();
        let (tx_a, rx_a) = watch::channel(false);
        let (tx_b, rx_b) = watch::channel(false);
        let (tx_o, rx_o) = watch::channel(false);
        let ga = register(member("carol"), PresenceKind::AppSocket, tx_a);
        let gb = register(member("carol"), PresenceKind::AppSocket, tx_b);
        let go = register(PresenceIdentity::Owner, PresenceKind::AppSocket, tx_o);

        let fired = close_connections_for("carol");
        assert_eq!(fired, 2, "both of carol's connections must fire");
        assert!(*rx_a.borrow(), "carol's first close handle must be set");
        assert!(*rx_b.borrow(), "carol's second close handle must be set");
        assert!(!*rx_o.borrow(), "the owner's handle must NOT fire");

        // Entries are not removed by the fire itself — the WS loops
        // deregister when they observe the handle.
        assert!(roster().iter().any(|r| r.user == "carol"));
        assert_eq!(close_connections_for("nobody-here"), 0);
        drop((ga, gb, go));
    }

    fn viewer(name: &str) -> PresenceIdentity {
        PresenceIdentity::ConnectUser {
            username: name.into(),
            role: Role::Viewer,
        }
    }

    #[test]
    fn set_granted_flips_roster_granted_edit_and_is_idempotent() {
        let _g = lock();
        let guard = register(viewer("vera"), PresenceKind::AppSocket, ch());
        let vera = |rows: &[RosterUser]| -> RosterUser {
            rows.iter().find(|r| r.user == "vera").expect("vera row").clone()
        };
        // Viewer role rides the wire; starts ungranted.
        let row = vera(&roster());
        assert_eq!(row.role, "viewer");
        assert!(!row.granted_edit);

        set_granted("vera", true);
        assert!(vera(&roster()).granted_edit, "grant must surface in the roster");
        // Idempotent re-grant is a no-op (no state corruption).
        set_granted("vera", true);
        assert!(vera(&roster()).granted_edit);

        set_granted("vera", false);
        assert!(!vera(&roster()).granted_edit, "revoke must surface in the roster");

        drop(guard);
        assert!(roster().iter().all(|r| r.user != "vera"));
    }

    #[test]
    fn owner_row_granted_edit_stays_hard_false() {
        let _g = lock();
        let guard = register(PresenceIdentity::Owner, PresenceKind::AppSocket, ch());
        // Even a (nonsensical) direct set never surfaces on the owner row —
        // the owner needs no grant.
        set_granted("owner", true);
        let rows = roster();
        let owner = rows.iter().find(|r| r.user == "owner").expect("owner row");
        assert!(!owner.granted_edit, "owner grantedEdit is hard-false");
        set_granted("owner", false); // clean the set back up
        drop(guard);
    }

    #[test]
    fn last_disconnect_auto_revokes_the_grant() {
        let _g = lock();
        let g1 = register(viewer("vlad"), PresenceKind::AppSocket, ch());
        let g2 = register(
            viewer("vlad"),
            PresenceKind::WorkspaceSocket { path: "/x/ws".into() },
            ch(),
        );
        set_granted("vlad", true);
        assert!(has_live_connection("vlad"));

        // Dropping ONE of two connections keeps the grant (grants are
        // per-USER; they die only with the LAST connection).
        drop(g2);
        let rows = roster();
        let vlad = rows.iter().find(|r| r.user == "vlad").expect("vlad row");
        assert!(vlad.granted_edit, "grant survives while a connection remains");

        // Dropping the LAST connection auto-revokes: a reconnect comes
        // back ungranted.
        drop(g1);
        assert!(!has_live_connection("vlad"));
        let g3 = register(viewer("vlad"), PresenceKind::AppSocket, ch());
        let rows = roster();
        let vlad = rows.iter().find(|r| r.user == "vlad").expect("vlad row");
        assert!(
            !vlad.granted_edit,
            "grant must be auto-revoked when the last connection deregisters"
        );
        drop(g3);
    }

    #[test]
    fn handle_grant_validates_target_and_wire_shape() {
        let _g = lock();
        // Malformed body → 400 regardless of registry state.
        let r = handle_grant(Role::Owner, b"not json");
        assert_eq!(r.status, "400 Bad Request");
        // A connected NON-viewer target → 400 with the role in the message
        // (role_for_user hits the connect-users store; an unseeded HOME
        // yields not-found which is also a 400 — both reject non-viewers).
        let guard = register(member("marge"), PresenceKind::AppSocket, ch());
        let r = handle_grant(Role::Owner, br#"{"username":"marge","granted":true}"#);
        assert_eq!(r.status, "400 Bad Request", "body: {}", r.body);
        drop(guard);
    }

    #[test]
    fn roster_orders_owner_first_then_usernames() {
        let _g = lock();
        let g1 = register(member("zeta"), PresenceKind::AppSocket, ch());
        let g2 = register(PresenceIdentity::Owner, PresenceKind::AppSocket, ch());
        let g3 = register(member("alpha"), PresenceKind::AppSocket, ch());
        let users: Vec<String> = roster().into_iter().map(|r| r.user).collect();
        assert_eq!(users, vec!["owner", "alpha", "zeta"]);
        drop((g1, g2, g3));
    }
}
