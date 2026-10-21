//! "Allow this agent to manage Postgres versions" (`db_upgrade`, migration
//! 0201; prd-pg17-and-db-upgrade-v1 rev 2 §12, Rosson 2026-10-20).
//!
//! A per-agent switch on the Agent tab, stored in
//! `projects.agents_can_upgrade_postgres`, written only by
//! `POST /cli/agent-access/set {toggle: "db_upgrade"}` through
//! [`set_switch`]: the owner raises, the owner or an Admin lowers, an
//! agent never (that route is not an agent verb, so an agent can't grant
//! it to itself or anyone). Every change writes an Access Audit line
//! (`db-upgrade-toggle`), an inbox note for the agent and a settings
//! event.
//!
//! [`decide`] is the one gate the `k2 db upgrade` / `create --major` /
//! `clusters` routes ask (S5 builds them on it). With the switch on, a
//! **local, non-sandboxed** agent passport (or a K2 shell tab, which
//! follows the agent rules) from workspace W may, on W's own databases and
//! W's own Apps' databases only:
//!
//! - `--check`, a move, `--rollback --yes`;
//! - `k2 db create --major <M>`;
//! - `k2 db clusters tidy`.
//!
//! Other workspaces' databases and `--all` are the owner's in 0.46.3 (Q9).
//! Always the owner's: `--forget-old`, `--allow-adopted`, `--now`,
//! `--whole-cluster`, the dual-run `wal_level` restart,
//! `--acknowledge-failed` and `k2 db default-major`. An Admin login may
//! only `--check`. A passport from anywhere but the loopback listener, a
//! sandboxed agent, an App pass, an API key and every federated or Connect
//! caller get nothing.

use serde_json::{json, Value as J};

/// Who is asking, as far as this switch cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// The owner daemon token or an Owner-role Connect login.
    Owner,
    /// An Admin-role Connect login.
    Admin { username: String },
    /// A Member-role Connect login.
    Member { username: String },
    /// A harness session's passport or a K2 shell tab's (same rules).
    /// `local` = it reached the TCP loopback listener; `sandboxed` = a
    /// microVM agent.
    Agent { project_id: String, session_id: String, local: bool, sandboxed: bool },
    /// App pass, API key, API session, federated, invalid.
    Other,
}

impl Who {
    /// From the compute routes' caller shape (the same identification the
    /// `compute_manage` switch uses). `local` = the request reached the
    /// loopback listener.
    pub fn from_compute(who: &crate::compute_routes::Who, local: bool) -> Self {
        use crate::compute_routes::Who as C;
        use k2_core::connect_users::Role;
        match who {
            C::Owner => Who::Owner,
            C::Login { role: Role::Owner, .. } => Who::Owner,
            C::Login { username, role: Role::Admin } => Who::Admin { username: username.clone() },
            C::Login { username, role: Role::Member } => Who::Member { username: username.clone() },
            C::Agent { project_id, session_id } | C::Shell { project_id, session_id } => Who::Agent {
                project_id: project_id.clone(),
                session_id: session_id.clone(),
                local,
                sandboxed: sandboxed(session_id),
            },
            C::Wrong | C::Invalid => Who::Other,
        }
    }

    /// The Access Audit "who": `owner`, `admin:<u>`, `agent:<project>`.
    pub fn label(&self) -> String {
        match self {
            Who::Owner => "owner".into(),
            Who::Admin { username } => format!("admin:{username}"),
            Who::Member { username } => format!("member:{username}"),
            Who::Agent { project_id, .. } => format!("agent:{project_id}"),
            Who::Other => "-".into(),
        }
    }
}

/// Is the passport's live session a sandboxed (microVM) agent? A session
/// K2 can't find counts as sandboxed: fail closed (PG-B security review #2
/// F10), since an ended session's passport may still be valid.
fn sandboxed(session_id: &str) -> bool {
    crate::v2_session_map::snapshot()
        .into_iter()
        .find(|(_, s)| s.session_id.to_string() == session_id)
        .is_none_or(|(_, s)| matches!(s.sandbox, k2_core::terminal::SandboxSpec::Microvm))
}

/// What the caller wants to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionsAct {
    /// `k2 db upgrade --check` (read-only).
    Check,
    /// A move (`k2 db upgrade <db> [--to M]`).
    Move,
    /// `k2 db upgrade <db> --rollback --yes`.
    Rollback,
    /// `k2 db create --major <M>`.
    CreateOnMajor,
    /// `k2 db clusters tidy`.
    ClustersTidy,
    /// `--forget-old` (drops the kept source).
    ForgetOld,
    /// `--allow-adopted`.
    AllowAdopted,
    /// `--now` (skips the 60 s notice).
    Now,
    /// `--whole-cluster` (0.46.4).
    WholeCluster,
    /// The dual-run `wal_level` restart of a shared cluster (0.46.4).
    WalLevelRestart,
    /// `--acknowledge-failed`.
    AcknowledgeFailed,
    /// `k2 db default-major <M>`.
    DefaultMajor,
    /// `k2 db upgrade --all`.
    All,
}

impl VersionsAct {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Move => "move",
            Self::Rollback => "rollback",
            Self::CreateOnMajor => "create-on-major",
            Self::ClustersTidy => "clusters-tidy",
            Self::ForgetOld => "forget-old",
            Self::AllowAdopted => "allow-adopted",
            Self::Now => "now",
            Self::WholeCluster => "whole-cluster",
            Self::WalLevelRestart => "wal-level-restart",
            Self::AcknowledgeFailed => "acknowledge-failed",
            Self::DefaultMajor => "default-major",
            Self::All => "all",
        }
    }

    /// The acts the switch hands an agent (on its own scope).
    pub fn agent_may(self) -> bool {
        matches!(self, Self::Check | Self::Move | Self::Rollback | Self::CreateOnMajor | Self::ClustersTidy)
    }

    /// Acts that don't name one database (no scope to check).
    fn unscoped(self) -> bool {
        matches!(self, Self::ClustersTidy)
    }
}

/// A refusal: `code` is the CLI/JSON reason, `status` the HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
}

pub const OWNER_ONLY: &str = "owner_only";
pub const SWITCH_OFF: &str = "db_upgrade_off";
pub const OUT_OF_SCOPE: &str = "not_your_database";
pub const NOT_LOCAL: &str = "not_local";

fn no(code: &'static str, status: u16, hint: impl Into<String>) -> Refusal {
    Refusal { code, status, hint: hint.into() }
}

fn owner_only(act: VersionsAct) -> Refusal {
    no(
        OWNER_ONLY,
        403,
        format!(
            "only the server's owner may run `{}` (from a terminal outside K2, or Settings → Data)",
            act.as_str()
        ),
    )
}

/// The one gate. `target_db_project` is the workspace that owns the target
/// database (None for acts without one, or a database K2 doesn't know);
/// `target_is_callers_app_db` = the database belongs to one of the
/// caller's own Apps; `switch_on` = the caller workspace's
/// `agents_can_upgrade_postgres` (read with [`switch_on`]).
pub fn decide(
    who: &Who,
    act: VersionsAct,
    target_db_project: Option<&str>,
    target_is_callers_app_db: bool,
    switch_on: bool,
) -> Result<(), Refusal> {
    match who {
        Who::Owner => Ok(()),
        Who::Admin { .. } if act == VersionsAct::Check => Ok(()),
        Who::Admin { .. } => Err(no(
            OWNER_ONLY,
            403,
            format!("an Admin login may only check (`k2 db upgrade --check`); `{}` is the owner's", act.as_str()),
        )),
        Who::Member { .. } | Who::Other => Err(owner_only(act)),
        Who::Agent { project_id, local, sandboxed, .. } => {
            if !*local || *sandboxed {
                return Err(no(
                    NOT_LOCAL,
                    403,
                    "Postgres versions are managed only by a local, non-sandboxed agent on this server (or the owner)",
                ));
            }
            if !act.agent_may() {
                return Err(owner_only(act));
            }
            if !switch_on {
                return Err(no(
                    SWITCH_OFF,
                    403,
                    "\"Allow this agent to manage Postgres versions\" is off for this agent: the owner turns it on in \
                     Settings → Workspaces → <agent> → Agent",
                ));
            }
            if act.unscoped() {
                return Ok(());
            }
            let own = target_db_project == Some(project_id.as_str()) || target_is_callers_app_db;
            if own {
                Ok(())
            } else {
                Err(no(
                    OUT_OF_SCOPE,
                    403,
                    "an agent manages Postgres versions for its own databases and its own Apps' databases only; \
                     other workspaces' databases are the owner's",
                ))
            }
        }
    }
}

/// Does `project_id`'s agent have the switch? Unknown workspace → false.
pub fn switch_on(project_id: &str) -> bool {
    k2_core::workspace::settings::agents_can_upgrade_postgres(project_id)
}

/// The one writer for the switch (`POST /cli/agent-access/set {toggle:
/// "db_upgrade"}`; the route has already refused agents and Members):
/// the owner raises, the owner or an Admin lowers. A change writes the
/// `db-upgrade-toggle` Access Audit line, the agent's inbox note and a
/// settings event; the route adds its own `agent-access-set` line.
pub fn set_switch(ws_id: &str, value: bool, caller_is_owner: bool, by: &str) -> Result<J, Refusal> {
    let (path, handle) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::db::schema::Project::get(&conn, ws_id) {
            Ok(p) => {
                let handle = if p.handle.trim().is_empty() { p.name.clone() } else { p.handle.clone() };
                (p.path, handle)
            }
            Err(_) => return Err(no("workspace_not_found", 404, format!("no workspace '{ws_id}'"))),
        }
    };
    let before = switch_on(ws_id);
    if value && !before && !caller_is_owner {
        return Err(no(
            OWNER_ONLY,
            403,
            "only the server's owner turns \"Allow this agent to manage Postgres versions\" on (an Admin may turn it off): \
             Settings → Workspaces → <agent> → Agent, or k2 db versions-access on --workspace <ws> from a terminal outside K2",
        ));
    }
    if value != before {
        k2_core::workspace::settings::set_agents_can_upgrade_postgres(ws_id, value)
            .map_err(|e| no("workspace_not_found", 404, e))?;
        changed(&path, &handle, value, by);
    }
    Ok(json!({
        "ok": true,
        "workspace": handle,
        "workspaceId": ws_id,
        "toggle": "db_upgrade",
        "agentsCanUpgradePostgres": value,
        "was": before,
        "changed": value != before,
    }))
}

/// The audit line, the agent's note and the settings event for a change.
fn changed(path: &str, handle: &str, on: bool, by: &str) {
    let state = if on { "on" } else { "off" };
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "db-upgrade-toggle",
        by,
        format!("ok db_upgrade={state} workspace={handle}"),
        "-",
        "-",
        "cli",
    ));
    let title = format!("Postgres versions: {state}");
    let text = if on {
        format!(
            "\"Allow this agent to manage Postgres versions\" is on (by {by}).\n\n\
             You may check, move and roll back your own databases and your Apps' databases to a newer Postgres \
             (k2 db upgrade <db> --check, then k2 db upgrade <db>), create a database on a chosen version \
             (k2 db create --major <M>) and tidy empty clusters (k2 db clusters tidy). Every act is in the Access \
             Audit and the owner gets a note for each move. Deleting the old copy (--forget-old), other workspaces' \
             databases and --all stay with the owner. Check with k2 db status."
        )
    } else {
        format!(
            "\"Allow this agent to manage Postgres versions\" is off (by {by}).\n\n\
             Moves, rollbacks and create --major now need the owner. Your databases keep the version they are on."
        )
    };
    if let Err(e) = k2_core::inbox::compose(std::path::Path::new(path), &title, &text, Some("normal"), Some("db"), Some("k2-db")) {
        k2_core::log_debug!("[db] versions switch note to {handle}: {e}");
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncProjects, J::Null);
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        json!({ "db": "versions_access", "workspace": handle, "agentsCanUpgradePostgres": on }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use VersionsAct::*;

    const ALL_ACTS: [VersionsAct; 13] = [
        Check,
        Move,
        Rollback,
        CreateOnMajor,
        ClustersTidy,
        ForgetOld,
        AllowAdopted,
        Now,
        WholeCluster,
        WalLevelRestart,
        AcknowledgeFailed,
        DefaultMajor,
        All,
    ];

    fn agent(local: bool, sandboxed: bool) -> Who {
        Who::Agent { project_id: "w".into(), session_id: "s".into(), local, sandboxed }
    }

    fn code(r: Result<(), Refusal>) -> &'static str {
        match r {
            Ok(()) => "ok",
            Err(e) => e.code,
        }
    }

    #[test]
    fn the_owner_may_do_everything_switch_or_not() {
        for act in ALL_ACTS {
            for sw in [false, true] {
                assert_eq!(decide(&Who::Owner, act, Some("other"), false, sw), Ok(()), "{act:?}");
                assert_eq!(decide(&Who::Owner, act, None, false, sw), Ok(()), "{act:?}");
            }
        }
    }

    #[test]
    fn an_admin_login_may_only_check() {
        let admin = Who::Admin { username: "ana".into() };
        for act in ALL_ACTS {
            let want = if act == Check { "ok" } else { OWNER_ONLY };
            assert_eq!(code(decide(&admin, act, Some("w"), false, true)), want, "{act:?}");
        }
    }

    #[test]
    fn members_and_other_credentials_get_nothing() {
        for who in [Who::Member { username: "m".into() }, Who::Other] {
            for act in ALL_ACTS {
                assert_eq!(code(decide(&who, act, Some("w"), true, true)), OWNER_ONLY, "{who:?} {act:?}");
            }
        }
    }

    #[test]
    fn an_agent_with_the_switch_manages_its_own_databases_only() {
        let a = agent(true, false);
        for act in [Check, Move, Rollback, CreateOnMajor] {
            assert_eq!(decide(&a, act, Some("w"), false, true), Ok(()), "own db: {act:?}");
            assert_eq!(decide(&a, act, Some("other"), true, true), Ok(()), "own App's db: {act:?}");
            assert_eq!(code(decide(&a, act, Some("other"), false, true)), OUT_OF_SCOPE, "other ws: {act:?}");
            assert_eq!(code(decide(&a, act, None, false, true)), OUT_OF_SCOPE, "unknown db: {act:?}");
        }
        assert_eq!(decide(&a, ClustersTidy, None, false, true), Ok(()), "tidy has no database");
    }

    #[test]
    fn owner_only_acts_stay_the_owners_even_with_the_switch() {
        let a = agent(true, false);
        for act in [ForgetOld, AllowAdopted, Now, WholeCluster, WalLevelRestart, AcknowledgeFailed, DefaultMajor, All] {
            assert_eq!(code(decide(&a, act, Some("w"), false, true)), OWNER_ONLY, "{act:?}");
            assert!(!act.agent_may());
        }
    }

    #[test]
    fn without_the_switch_an_agent_gets_nothing_not_even_check() {
        let a = agent(true, false);
        for act in [Check, Move, Rollback, CreateOnMajor, ClustersTidy] {
            assert_eq!(code(decide(&a, act, Some("w"), false, false)), SWITCH_OFF, "{act:?}");
        }
    }

    #[test]
    fn remote_and_sandboxed_passports_never_qualify() {
        for who in [agent(false, false), agent(true, true), agent(false, true)] {
            for act in ALL_ACTS {
                assert_eq!(code(decide(&who, act, Some("w"), false, true)), NOT_LOCAL, "{who:?} {act:?}");
            }
        }
    }

    #[test]
    fn from_compute_maps_roles_and_passports() {
        use crate::compute_routes::Who as C;
        use k2_core::connect_users::Role;
        assert_eq!(Who::from_compute(&C::Owner, true), Who::Owner);
        assert_eq!(Who::from_compute(&C::Login { username: "o".into(), role: Role::Owner }, false), Who::Owner);
        assert_eq!(
            Who::from_compute(&C::Login { username: "a".into(), role: Role::Admin }, true),
            Who::Admin { username: "a".into() }
        );
        assert_eq!(
            Who::from_compute(&C::Login { username: "m".into(), role: Role::Member }, true),
            Who::Member { username: "m".into() }
        );
        let got = Who::from_compute(&C::Shell { project_id: "w".into(), session_id: "gone".into() }, true);
        // A session K2 can't find is sandboxed (fail closed, PG-B review #2 F10).
        assert_eq!(got, Who::Agent { project_id: "w".into(), session_id: "gone".into(), local: true, sandboxed: true });
        assert_eq!(Who::from_compute(&C::Wrong, true), Who::Other);
        assert_eq!(Who::from_compute(&C::Invalid, true), Who::Other);
    }

    fn ws(dir: &std::path::Path) -> String {
        let path = dir.join("ops");
        std::fs::create_dir_all(&path).unwrap();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES ('w-pg', 'ops', ?1, 'ops')",
            [path.to_string_lossy().to_string()],
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_owner_raises_an_admin_only_lowers() {
        let home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let path = ws(home.path());
        assert!(!switch_on("w-pg"), "0201 default is off");
        assert!(!switch_on("no-such-ws"), "unknown workspace fails closed");

        let r = set_switch("w-pg", true, false, "admin:ana").unwrap_err();
        assert_eq!((r.code, r.status), (OWNER_ONLY, 403));
        assert!(!switch_on("w-pg"), "an Admin can't raise");

        let out = set_switch("w-pg", true, true, "owner").unwrap();
        assert_eq!(out["agentsCanUpgradePostgres"], true);
        assert_eq!(out["changed"], true);
        assert!(switch_on("w-pg"));
        let notes = k2_core::inbox::list_all(std::path::Path::new(&path));
        assert_eq!(notes.len(), 1, "the agent gets one note");
        assert!(notes[0].title.contains("Postgres versions: on"), "{:?}", notes[0].title);

        // Raising again is a no-op: no second note.
        let out = set_switch("w-pg", true, true, "owner").unwrap();
        assert_eq!(out["changed"], false);
        assert_eq!(k2_core::inbox::list_all(std::path::Path::new(&path)).len(), 1);
        // An Admin re-asking for "on" while it is on is not a raise.
        assert!(set_switch("w-pg", true, false, "admin:ana").is_ok());

        let out = set_switch("w-pg", false, false, "admin:ana").unwrap();
        assert_eq!(out["agentsCanUpgradePostgres"], false);
        assert!(!switch_on("w-pg"), "an Admin may lower");
        assert_eq!(k2_core::inbox::list_all(std::path::Path::new(&path)).len(), 2);

        let tail = k2_core::auth_audit::tail(10).unwrap();
        let lines: Vec<String> = tail
            .iter()
            .filter(|e| e["event"] == "db-upgrade-toggle")
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(lines, vec!["ok db_upgrade=on workspace=ops", "ok db_upgrade=off workspace=ops"], "{tail:?}");

        let r = set_switch("no-such-ws", false, true, "owner").unwrap_err();
        assert_eq!((r.code, r.status), ("workspace_not_found", 404));
    }
}
