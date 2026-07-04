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

use serde::Serialize;
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
fn deregister(conn_id: u64) {
    let removed = {
        let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&conn_id)
    };
    if removed.is_some() {
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
