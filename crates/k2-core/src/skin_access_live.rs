//! The one place skin.db's access writes are noticed (`prd-live-access-changes-v1`
//! S2, D3).
//!
//! Every skin.db write goes through one connection in `with_conn`, and only
//! `open_db` opens it. So an SQLite update hook on that connection sees every
//! access write, whoever makes it, including writers added next year and foreign
//! key cascades. The hook only says "an access table changed". It doesn't work out
//! who is affected: a hub in the daemon re-resolves each open pass and compares
//! the answers, so a no-op write or someone else's change sends nothing.
//!
//! Three things come out of it:
//! - [`super::write_generation`] moves, so an open socket resolves before its next
//!   frame. Only access writes move it: a failed login writes `login_lockouts` and
//!   a new session INSERTs into `tokens`, and neither moves it, so an outsider
//!   can't.
//! - A listener is told, after the mutex is released. It must not block or take a
//!   lock: callers may hold the main database lock.
//! - [`take_wake`] says how much to re-resolve. A `tokens` write names its rows
//!   (a device refresh or a link rotation shouldn't walk every pass), anything else
//!   is a full walk.
//!
//! What the hook can't see (SQLite's truncate optimisation on a `DELETE` with no
//! `WHERE`, a `WITHOUT ROWID` table, the other daemon's writes during a handoff):
//! the hub's sweep is the backstop, and a test pins that skin.db has neither.
//!
//! What isn't in skin.db at all gets an explicit [`hint`]: a project row removed,
//! a project's members, and an App's suspension, all in the main database.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::hooks::Action;
use rusqlite::Connection;

/// The tables whose rows decide what a live pass may do: everything
/// [`super::try_resolve_skin_token`], the role and folder reads behind it and
/// `resolve_link_pass` look at. `the_access_tables_are_what_a_resolve_reads` runs a
/// resolve under an authorizer and fails if it reads a table not listed here, so a
/// new access table can't be forgotten.
pub const ACCESS_TABLES: &[&str] =
    &["grants", "roles", "principals", "principal_agent_ties", "agent_written_access", "app_owner_grants", "tokens"];

/// Rows a `tokens` write may name before it just counts as a full walk.
const MAX_NAMED_TOKENS: usize = 256;

/// A watched write happened during this `with_conn` call.
static DIRTY: AtomicBool = AtomicBool::new(false);
/// Since the last [`take_wake`]: something other than named `tokens` rows moved.
static FULL: AtomicBool = AtomicBool::new(false);
/// Since the last [`take_wake`]: the `tokens` rows written.
static TOKEN_ROWS: Mutex<Vec<i64>> = Mutex::new(Vec::new());
/// See [`super::write_generation`].
static GENERATION: AtomicU64 = AtomicU64::new(0);
static LISTENER: OnceLock<fn()> = OnceLock::new();

/// Is this write one that can change what a live pass may do? A new row in
/// `tokens` or `principals` can't change an existing pass's answer: the grant that
/// gives it access is its own write.
fn watched(action: Action, table: &str) -> bool {
    ACCESS_TABLES.contains(&table) && !(matches!(action, Action::SQLITE_INSERT) && matches!(table, "tokens" | "principals"))
}

/// Set on the one connection, after the migrations (`open_db`). Runs inside
/// SQLite, under the skin mutex: an atomic and a short list, nothing else. It fires
/// before the commit, so it only marks; [`settle`] acts once the call returns.
pub(super) fn install(conn: &Connection) {
    conn.update_hook(Some(|action: Action, _db: &str, table: &str, rowid: i64| {
        if !watched(action, table) {
            return;
        }
        DIRTY.store(true, Ordering::SeqCst);
        if table == "tokens" {
            let mut rows = TOKEN_ROWS.lock();
            if rows.len() < MAX_NAMED_TOKENS {
                rows.push(rowid);
                return;
            }
        }
        FULL.store(true, Ordering::SeqCst);
    }));
}

/// After a `with_conn` closure returned, still under the mutex: whoever sees the
/// generation move and then reads, reads the write. `true`: tell the listener once
/// the mutex is released. A rollback or a closure that wrote and then failed lands
/// here too: one extra walk, never a wrong answer.
pub(super) fn settle() -> bool {
    if DIRTY.swap(false, Ordering::SeqCst) {
        GENERATION.fetch_add(1, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub(super) fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Tell the listener. The skin mutex is free by now.
pub(super) fn tell() {
    if let Some(f) = LISTENER.get() {
        f();
    }
}

/// Register the change listener (first call wins). It must not block or take a
/// lock: set a flag and wake a thread.
pub fn set_listener(f: fn()) {
    let _ = LISTENER.set(f);
}

/// What isn't in skin.db changed something a live pass can see: a project row, a
/// project's members, an App's suspension. Open sockets ask again before their
/// next frame, and every open pass is compared.
pub fn hint() {
    FULL.store(true, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    tell();
}

/// How much to re-resolve.
#[derive(Debug, PartialEq, Eq)]
pub enum Wake {
    /// Nothing since the last call.
    Nothing,
    /// Only these `tokens` rows.
    Tokens(Vec<i64>),
    /// Every open pass.
    Full,
}

/// What moved since the last call.
pub fn take_wake() -> Wake {
    let rows = std::mem::take(&mut *TOKEN_ROWS.lock());
    if FULL.swap(false, Ordering::SeqCst) {
        Wake::Full
    } else if rows.is_empty() {
        Wake::Nothing
    } else {
        Wake::Tokens(rows)
    }
}

/// The row a raw pass lives in, so a `tokens` write can name it. `None`: no such
/// pass.
pub fn token_rowid(raw: &str) -> Result<Option<i64>, String> {
    use rusqlite::OptionalExtension;
    let hash = super::sha256_hex(raw);
    super::with_conn(|conn| {
        conn.query_row("SELECT rowid FROM tokens WHERE key_hash = ?1", [&hash], |r| r.get(0)).optional().map_err(|e| format!("token rowid: {e}"))
    })
}

/// One App's login sessions that have ended since `since` (unix seconds), as `(token
/// id, person)`: revoked, or past their expiry. A session ending changes nothing about
/// the person's access on paper, but the server holding that pass needs to hear.
pub fn sessions_ended_since(service_id: &str, since: i64) -> Result<Vec<(String, String)>, String> {
    let now = super::now_secs();
    super::with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT id, principal_id FROM tokens
                 WHERE app_service_id = ?1 AND session = 1 AND principal_id IS NOT NULL
                   AND (revoked_at >= ?2 OR (expires_at >= ?2 AND expires_at <= ?3))",
            )
            .map_err(|e| format!("ended sessions: {e}"))?;
        let found = st.query_map(rusqlite::params![service_id, since, now], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| format!("ended sessions: {e}"))?;
        found.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("ended sessions: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex as StdMutex};

    fn tables_in_skin_db() -> BTreeSet<String> {
        skin::with_conn(|c| {
            let mut st = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'").map_err(|e| e.to_string())?;
            let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            rows.collect::<Result<BTreeSet<_>, _>>().map_err(|e| e.to_string())
        })
        .expect("tables")
    }

    fn a_guest_with_a_role() -> (String, String, String) {
        crate::db::init_for_tests();
        let room = uuid::Uuid::new_v4().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                rusqlite::params![room, format!("g{}", &room[..8]), format!("/tmp/{room}")],
            )
            .expect("project");
        }
        let (user, role) = (format!("gu{}", &room[..8]), format!("gr{}", &room[..8]));
        let mut policy = skin::RoomPolicy::new();
        policy.insert(room.clone(), vec![skin::CAP_THREAD_READ.to_string()]);
        skin::create_role_for_app(&role, &policy, None).expect("role");
        skin::add_principal(&user).expect("person");
        skin::assign_role(&user, &role).expect("assign");
        let (_, raw) = skin::create_session_token(&user).expect("session");
        (user, role, raw)
    }

    /// Runs `f` under an authorizer and returns every table it read. A cached
    /// statement wouldn't ask the authorizer again, so the cache is flushed first.
    fn tables_read_by(f: impl FnOnce()) -> BTreeSet<String> {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        let seen = Arc::new(StdMutex::new(BTreeSet::new()));
        let s2 = seen.clone();
        skin::with_conn(|c| {
            c.flush_prepared_statement_cache();
            c.authorizer(Some(move |ctx: AuthContext<'_>| {
                if let AuthAction::Read { table_name, .. } = ctx.action {
                    s2.lock().expect("seen").insert(table_name.to_string());
                }
                Authorization::Allow
            }));
            Ok(())
        })
        .expect("authorizer");
        f();
        skin::with_conn(|c| {
            c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
            Ok(())
        })
        .expect("authorizer off");
        let out = seen.lock().expect("seen").clone();
        out
    }

    /// The tables K2 knows don't decide access, each with why. A new table has to
    /// be put on one list or the other.
    const NOT_ACCESS: &[(&str, &str)] = &[
        ("access_template_lines", "a template only writes roles and grants when applied"),
        ("access_templates", "same"),
        ("app_chat_grants", "guests-chat: chat rests on the person's grants row"),
        ("app_chat_keys", "Sew's App key, not a person's access"),
        ("app_auth_door", "the sign-in door's per-App switch and page origins; a pass never reads it"),
        ("app_auth_door_box", "the sign-in door's box switch"),
        ("app_handover_journal", "the handover's own log; what it moves is in watched tables"),
        ("app_link_files", "where a link's pass file lives"),
        ("app_login_settings", "login form settings"),
        ("app_pairing_config", "pairing settings"),
        ("app_profile_settings", "profile settings"),
        // Sew 0.9.0 recording (0.46.11). The CUI/FCI lock never enters a
        // pass resolve: no K2 pass, cap or room changes with it (voice:record
        // comes from role room lines, which are watched). It only narrows the
        // Space row the sweep sends Sew (recording_start_level none, required
        // false; `tool_hook::recording::locked_body`), and a lock write kicks
        // that sweep (`recording_set_lock`). Sew refuses the start. A walk on
        // it would re-resolve every pass and find nothing changed.
        ("app_recording_locks", "the CUI/FCI lock: narrows the Space row sent to Sew, which refuses the start; no pass resolve reads it"),
        ("app_push_config", "push settings"),
        ("app_push_devices", "push devices"),
        ("app_test_guests", "marks test guests"),
        // 0.46.9: account deletion. A request turns the person's grants off
        // and ends their sessions through the access writers; its own rows
        // only remember the request.
        ("app_account_deletion_settings", "an App's deletion mode and grace period"),
        ("app_account_deletions", "pending deletion requests (the grant change is the access write)"),
        ("app_user_invites", "invites not yet consumed; consuming writes principals"),
        ("device_login_pairings", "pairing state"),
        ("device_logins", "device refresh state; its session is a tokens row"),
        ("device_refresh_spent", "spent refresh tokens"),
        ("front_door", "the box's front door"),
        ("host_grant_backfill", "a one-time marker"),
        ("login_lockouts", "failed logins: an outsider writes this"),
        ("password_change_tokens", "a must-change token"),
        ("password_reset_throttle", "reset throttle: an outsider writes this"),
        ("password_reset_tokens", "a reset token; using it writes principals"),
        ("principal_agent_creators", "who created a login"),
        ("principal_email_proven", "email proof"),
        ("principal_email_unconfirmed", "email proof"),
        ("principal_profiles", "a person's profile"),
        ("room_privacy", "who shows on a member list, not what a person may do"),
        ("session_meta", "what a session looks like in a list"),
        ("principal_person_claims", "an older marker, read by a migration only"),
        ("skin_app_backfill", "a one-time marker"),
        // K2's outbox and ledgers toward Sew: Sew's own events cover that side.
        ("tool_admin_members", "toward Sew"),
        ("tool_admin_rooms", "toward Sew"),
        ("tool_admin_space", "toward Sew"),
        ("tool_app_chat_facts", "toward Sew"),
        ("tool_app_member_lists", "toward Sew"),
        ("tool_app_service_retire", "toward Sew"),
        ("tool_app_space_channels", "toward Sew"),
        ("tool_app_space_kept", "toward Sew"),
        ("tool_app_space_leaving", "toward Sew"),
        ("tool_app_space_member_channels", "toward Sew"),
        ("tool_app_space_members", "toward Sew"),
        ("tool_app_space_unbinds", "toward Sew"),
        ("tool_app_spaces", "toward Sew"),
        ("tool_chat_avatars", "toward Sew"),
        ("tool_chat_members", "toward Sew"),
        ("tool_outbox", "toward Sew"),
        ("tool_recording_feed", "an App's recording feed cursor toward Sew; the read gate is server_read in tool_sew_settings"),
        ("tool_recording_policy", "toward Sew: the org recording policies asked for and seen"),
        ("tool_recording_rollout", "toward Sew: the cap rollout's ledger; its grandfather step's role lines are the access write"),
        ("tool_sew_policy", "toward Sew"),
        ("tool_sew_settings", "toward Sew"),
        ("tool_target_features", "toward Sew"),
        ("tool_targets", "toward Sew"),
    ];

    #[test]
    fn the_access_tables_are_what_a_resolve_reads() {
        let _home = crate::test_env::TempHome::new();
        let (user, _role, raw) = a_guest_with_a_role();
        let pid = skin::with_conn(|c| c.query_row("SELECT id FROM principals WHERE username = ?1", [&user], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())).expect("pid");
        // The tied path reads more than the plain one.
        let read = tables_read_by(|| {
            assert!(skin::resolve_skin_token(&raw).is_some(), "live");
        });
        skin::with_conn(|c| {
            c.execute(
                "INSERT INTO principal_agent_ties (principal_id, workspace_id, host_ok, reasons, set_by, set_at) VALUES (?1, 'ws', 1, '[]', 'test', 1)",
                [&pid],
            )
            .map_err(|e| e.to_string())
        })
        .expect("tie");
        let tied = tables_read_by(|| {
            skin::resolve_skin_token(&raw);
        });
        for t in read.iter().chain(tied.iter()) {
            assert!(
                ACCESS_TABLES.contains(&t.as_str()) || t.starts_with("sqlite_") || t == "pragma_table_info",
                "a resolve reads `{t}`, and a write to it wakes nothing: add it to ACCESS_TABLES (read {read:?} {tied:?})"
            );
        }
        for t in ["tokens", "grants", "principals"] {
            assert!(read.contains(t), "the authorizer saw too little to mean anything: {read:?}");
        }
    }

    #[test]
    fn every_table_in_skin_db_is_on_one_list() {
        let _home = crate::test_env::TempHome::new();
        crate::db::init_for_tests();
        for t in tables_in_skin_db() {
            let on_list = ACCESS_TABLES.contains(&t.as_str()) || NOT_ACCESS.iter().any(|(n, _)| *n == t);
            assert!(on_list, "skin.db has a new table `{t}`: if it decides what a live pass may do, add it to ACCESS_TABLES; if not, add it to NOT_ACCESS with why");
        }
    }

    /// The update hook can't see either of these, so the sweep would be the only
    /// thing that notices. Keep them out of the code. Reads the sources, because
    /// they're the claim.
    #[test]
    fn nothing_writes_an_access_table_where_the_hook_cant_see() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut files = vec![];
        for e in std::fs::read_dir(root).expect("src") {
            let p = e.expect("entry").path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if name.starts_with("skin") && name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "skin_access_live.rs" {
                files.push(p);
            }
        }
        assert!(files.len() > 10, "the source scan found too little: {files:?}");
        for f in files {
            let text = std::fs::read_to_string(&f).expect("read");
            let cut = text.find("#[cfg(test)]").unwrap_or(text.len());
            let text = &text[..cut];
            assert!(!text.contains("WITHOUT ROWID"), "{}: the update hook doesn't fire on a WITHOUT ROWID table", f.display());
            for t in ACCESS_TABLES {
                assert!(!text.contains(&format!("OR REPLACE INTO {t}")), "{}: INSERT OR REPLACE into {t} reports only the insert to the hook", f.display());
                let needle = format!("DELETE FROM {t}");
                for (at, _) in text.match_indices(&needle) {
                    let rest = text[at + needle.len()..].trim_start();
                    assert!(rest.starts_with("WHERE"), "{}: `{needle}` with no WHERE doesn't fire the hook (truncate optimisation): {}", f.display(), &rest[..rest.len().min(40)]);
                }
            }
        }
    }

    #[test]
    fn the_generation_moves_for_access_writes_only() {
        let _home = crate::test_env::TempHome::new();
        let (user, role, raw) = a_guest_with_a_role();
        let g = |what: &str, before: u64, moves: bool| {
            assert_eq!(generation() != before, moves, "{what}");
        };

        // Reads, and what an outsider can write.
        let before = generation();
        assert!(skin::resolve_skin_token(&raw).is_some());
        g("a read", before, false);
        assert!(skin::resolve_skin_token("k2skn_not_a_real_pass").is_none());
        g("a bad pass", before, false);
        skin::with_conn(|c| {
            c.execute("INSERT INTO login_lockouts (username, failed_count) VALUES ('outsider', 1)", []).map_err(|e| e.to_string())
        })
        .expect("lockout");
        g("a failed login's lockout row", before, false);

        // A new session is an INSERT: it changes nobody's answer.
        let (meta, second) = skin::create_session_token(&user).expect("session");
        g("a new session", before, false);
        assert!(skin::resolve_skin_token(&second).is_some());

        // A revoke names its row.
        let _ = take_wake();
        let row = token_rowid(&second).expect("rowid").expect("row");
        assert!(skin::revoke_token(&meta.id).expect("revoke"));
        g("a revoke", before, true);
        assert_eq!(take_wake(), Wake::Tokens(vec![row]), "a tokens write names its row");
        assert_eq!(take_wake(), Wake::Nothing, "once");

        // Anything else is a full walk, and it's in by the time the call returns.
        let before = generation();
        skin::unassign_role(&user).expect("unassign");
        g("a role change", before, true);
        assert_eq!(take_wake(), Wake::Full, "not a token write");
        let _ = role;

        // A hint covers what isn't in skin.db.
        let before = generation();
        hint();
        g("a hint", before, true);
        assert_eq!(take_wake(), Wake::Full);
    }

    #[test]
    fn a_rolled_back_write_is_a_harmless_extra_walk_and_a_failing_closure_still_counts() {
        let _home = crate::test_env::TempHome::new();
        let (user, _role, _raw) = a_guest_with_a_role();
        let _ = take_wake();
        let before = generation();
        let r: Result<(), String> = skin::with_conn(|c| {
            c.execute("UPDATE principals SET full_name = 'x' WHERE username = ?1", [&user]).map_err(|e| e.to_string())?;
            Err("it wrote, then failed".to_string())
        });
        assert!(r.is_err());
        assert_ne!(generation(), before, "an autocommitted write before an Err still counts");
        assert_eq!(take_wake(), Wake::Full);
    }

    /// PRD §5, by writer: the ones `members::changed` never heard (finding 8) and the
    /// rest. Whoever writes, however they write, the generation moves and a walk is
    /// owed. With `the_access_tables_are_what_a_resolve_reads` and
    /// `every_table_in_skin_db_is_on_one_list`, this is the proof no trigger is
    /// missed: a new writer lands on a watched table, and a new table fails the list.
    #[test]
    fn every_writer_on_the_list_moves_the_generation() {
        let _home = crate::test_env::TempHome::new();
        let (user, role, raw) = a_guest_with_a_role();
        let room = skin::resolve_skin_token(&raw).expect("live").rooms[0].clone();
        let pid = skin::host_login_detail(&user).expect("guest").principal_id;
        let svc = "svc-access-live";
        let check = |what: &str, f: &mut dyn FnMut()| {
            let _ = take_wake();
            let before = generation();
            f();
            assert_ne!(generation(), before, "{what}: the generation didn't move");
            assert_ne!(take_wake(), Wake::Nothing, "{what}: no walk owed");
        };
        // Folder lines.
        check("persist_role_paths", &mut || {
            let scope = skin::RoomPathScope { read: vec!["docs".into()], ..Default::default() };
            skin::paths::set_role_room_paths(&role, &room, &scope).expect("folders");
        });
        // guests-rooms and its marks.
        check("guests-rooms grant", &mut || {
            skin::guest_rooms::grant(svc, std::slice::from_ref(&room), "test").expect("grant");
        });
        check("guests-rooms revoke", &mut || {
            skin::guest_rooms::revoke(svc, "test").expect("revoke");
        });
        check("agent_written_access", &mut || {
            skin::with_conn(|c| skin::guest_rooms::mark_written_conn(c, skin::guest_rooms::WRITTEN_ROLE, "role-x", "agent:x")).expect("mark");
        });
        // Ties.
        check("tie set", &mut || {
            skin::ties::set(&pid, skin::ties::TieWrite { workspace_id: &room, set_by: "agent:x", reason: "test" }).expect("tie");
        });
        check("tie dropped", &mut || {
            skin::with_conn(|c| skin::ties::drop_conn(c, &pid)).expect("drop");
        });
        // A login made an agent login; the rooms on a person; a role's rooms.
        check("set_principal_agent", &mut || {
            skin::set_principal_agent(&user, Some(&room)).expect("agent");
        });
        check("clear it", &mut || {
            skin::set_principal_agent(&user, None).expect("agent");
        });
        check("unassign", &mut || {
            skin::unassign_role(&user).expect("unassign");
        });
        check("set_principal_default_rooms", &mut || {
            skin::set_principal_default_rooms(&user, std::slice::from_ref(&room), false).expect("rooms");
        });
        // Platform pass rooms, and a platform pass's revoke.
        let (meta, _) = skin::create_token("access-live-platform", None, std::slice::from_ref(&room)).expect("token");
        check("set_token_rooms", &mut || {
            skin::set_token_rooms(&meta.id, &[]).expect("rooms");
        });
        check("revoke a platform pass", &mut || {
            assert!(skin::revoke_token(&meta.id).expect("revoke"));
        });
        // Sessions ended.
        check("revoke a person's sessions", &mut || {
            skin::with_conn(|c| skin::revoke_sessions_for_principal(c, &pid)).expect("revoke");
        });
        // A person, then a role.
        let _ = skin::create_session_token(&user).expect("session");
        check("person removed", &mut || {
            assert!(skin::remove_principal(&user).expect("remove"));
        });
        check("role removed", &mut || {
            assert!(skin::remove_role(&role).expect("remove"));
        });
    }

    #[test]
    fn the_listener_can_call_back_into_skin_db() {
        // The listener runs after the mutex is released. If it didn't, this would
        // deadlock (the mutex isn't re-entrant). This is the one test in this
        // binary that sets one.
        static CALLS: AtomicU64 = AtomicU64::new(0);
        fn listener() {
            CALLS.fetch_add(1, Ordering::SeqCst);
            let _ = skin::with_conn(|_| Ok(()));
        }
        set_listener(listener);
        let _home = crate::test_env::TempHome::new();
        let (user, _role, _raw) = a_guest_with_a_role();
        let before = CALLS.load(Ordering::SeqCst);
        skin::unassign_role(&user).expect("unassign");
        assert!(CALLS.load(Ordering::SeqCst) > before, "the listener heard the write");
    }
}
