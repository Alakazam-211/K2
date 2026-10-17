//! Thread push prefs and recipients (prd-thread-push-notifications-v1
//! §3, §4, §6.2). Daemon-owned: every client renders this truth.
//!
//! **Person key (§3, TP9).** Prefs, devices, recipients and viewers all use
//! one key per human:
//! - the owner daemon token → [`OWNER`];
//! - a Connect user the owner LINKED as one of their accounts
//!   (`ownerAccounts` setting, `POST /cli/owner/accounts`) → [`OWNER`],
//!   whatever its role — identity only, never privilege;
//! - a Connect user with role Owner → [`OWNER`] when it is the server's
//!   ONLY Owner-role Connect user (Q6); with two or more, each keeps its
//!   own username;
//! - any other Connect user → their username.
//!
//! The mapping is computed when a key is read and never stored, so a role
//! change takes effect at once. It is what makes the owner's desktop
//! (owner token) and phone (Connect login) one person.
//!
//! **Storage (migration 0157).** `notify_workspace_prefs(principal,
//! project_id, thread_push)` where principal `*` is the workspace default an
//! Admin sets, and `notify_server_prefs(principal, enabled)` — the bell,
//! which gates EVERY push kind for that person (Q5).
//!
//! Resolution (§4): own row → `*` row → on (Q1). Bell: own row → on.
//!
//! The evaluators are pure ([`People`] + [`PrefsSnapshot`] + devices in,
//! answers out) so they are tested without `$HOME` or the shared DB.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::{params, Connection};

use crate::connect_users::{ConnectUserView, Role};
use crate::push_devices::PushDevice;

/// The person key of the server owner.
pub const OWNER: &str = "owner";
/// The workspace-default principal (`notify_workspace_prefs` only).
pub const DEFAULT_PRINCIPAL: &str = "*";

/// The Connect users, read once. Pure helpers answer person-key questions
/// from it.
#[derive(Debug, Clone, Default)]
pub struct People {
    pub users: Vec<ConnectUserView>,
    /// Owner-linked accounts: Connect usernames that ARE the owner.
    pub linked: Vec<String>,
}

impl People {
    /// Read `connect-users.json` and the `ownerAccounts` setting.
    pub fn load() -> Result<Self, String> {
        Ok(Self {
            users: crate::connect_users::list_users()?,
            linked: crate::app_settings::load().owner_accounts,
        })
    }

    /// True when `login` is an owner-linked account.
    pub fn is_linked(&self, login: &str) -> bool {
        self.linked.iter().any(|l| l.trim().eq_ignore_ascii_case(login.trim()))
    }

    /// Every Connect login that resolves to the `owner` person: the linked
    /// accounts that exist, plus the sole Owner-role login. Sorted, unique.
    pub fn owner_aliases(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .users
            .iter()
            .filter(|u| self.person_key(&u.username) == OWNER && !u.username.eq_ignore_ascii_case(OWNER))
            .map(|u| u.username.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn user(&self, username: &str) -> Option<&ConnectUserView> {
        self.users.iter().find(|u| u.username.eq_ignore_ascii_case(username))
    }

    /// The server's only Owner-role Connect user (Q6), if exactly one.
    pub fn sole_owner(&self) -> Option<&str> {
        let mut owners = self.users.iter().filter(|u| u.role == Role::Owner);
        let first = owners.next()?;
        if owners.next().is_some() {
            return None;
        }
        Some(first.username.as_str())
    }

    /// The person key for a login identity (`"owner"` for the owner token,
    /// else a Connect username).
    pub fn person_key(&self, login: &str) -> String {
        let login = login.trim();
        if login.eq_ignore_ascii_case(OWNER) || self.is_linked(login) {
            return OWNER.to_string();
        }
        if self.sole_owner().is_some_and(|o| o.eq_ignore_ascii_case(login)) {
            return OWNER.to_string();
        }
        login.to_ascii_lowercase()
    }

    /// The person a registered device belongs to, or `None` when the
    /// device must not get pushes: its login was removed or is disabled
    /// (TP11). A device registered by the owner token always counts.
    pub fn device_person(&self, device_username: &str) -> Option<String> {
        let name = device_username.trim();
        if name.eq_ignore_ascii_case(OWNER) && self.user(name).is_none() {
            return Some(OWNER.to_string());
        }
        let user = self.user(name)?;
        if user.disabled {
            return None;
        }
        Some(self.person_key(&user.username))
    }

    /// The role a person key stands for (for `can_act_on`): `owner` is an
    /// Owner; a username has its stored role. `None` = no such person.
    pub fn role_of(&self, person: &str) -> Option<Role> {
        if person == OWNER {
            return Some(Role::Owner);
        }
        self.user(person).map(|u| u.role)
    }

    /// True when `from` names a human on this server: the owner, the
    /// owner's display name (`owner_display`), or any Connect username
    /// (TP14). Case-insensitive.
    pub fn is_human_from(&self, from: &str, owner_display: &str) -> bool {
        let f = from.trim();
        if f.is_empty() {
            return false;
        }
        f.eq_ignore_ascii_case(OWNER)
            || (!owner_display.trim().is_empty() && f.eq_ignore_ascii_case(owner_display.trim()))
            || self.user(f).is_some()
    }

    /// Every person, owner first, then Connect users by name — the admin
    /// page's `people`. A sole Owner-role user folds into `owner`.
    pub fn roster(&self) -> Vec<PersonRow> {
        let mut out = vec![PersonRow {
            principal: OWNER.to_string(),
            role: Role::Owner,
            disabled: false,
            logins: {
                let mut l = vec![OWNER.to_string()];
                l.extend(self.owner_aliases());
                l
            },
        }];
        let mut users: Vec<&ConnectUserView> = self.users.iter().collect();
        users.sort_by(|a, b| a.username.cmp(&b.username));
        for u in users {
            if self.person_key(&u.username) == OWNER {
                continue;
            }
            out.push(PersonRow {
                principal: u.username.clone(),
                role: u.role,
                disabled: u.disabled,
                logins: vec![u.username.clone()],
            });
        }
        out
    }
}

/// One person on the admin page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonRow {
    pub principal: String,
    pub role: Role,
    pub disabled: bool,
    /// The login names whose devices are this person's.
    pub logins: Vec<String>,
}

/// Where an effective value came from (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The person's own row.
    User,
    /// The workspace's `*` row.
    Default,
    /// No row: on (Q1).
    Fallback,
}

impl Source {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Default => "default",
            Self::Fallback => "fallback",
        }
    }
}

/// An effective switch value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub on: bool,
    pub source: Source,
    /// Present only when `source` is [`Source::User`].
    pub updated_by: Option<String>,
    pub updated_at: Option<i64>,
}

impl Effective {
    fn fallback() -> Self {
        Self { on: true, source: Source::Fallback, updated_by: None, updated_at: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    on: bool,
    updated_at: i64,
    updated_by: String,
}

/// Every pref row, read once.
#[derive(Debug, Clone, Default)]
pub struct PrefsSnapshot {
    workspace: HashMap<(String, String), Row>,
    server: HashMap<String, Row>,
}

impl PrefsSnapshot {
    /// Read both tables.
    pub fn load(conn: &Connection) -> Result<Self, String> {
        let mut snap = Self::default();
        let mut stmt = conn
            .prepare(
                "SELECT principal, project_id, thread_push, updated_at, updated_by \
                 FROM notify_workspace_prefs",
            )
            .map_err(|e| format!("notify prefs read: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    Row { on: r.get::<_, i64>(2)? != 0, updated_at: r.get(3)?, updated_by: r.get(4)? },
                ))
            })
            .map_err(|e| format!("notify prefs read: {e}"))?;
        for row in rows {
            let (p, project, row) = row.map_err(|e| format!("notify prefs read: {e}"))?;
            snap.workspace.insert((p, project), row);
        }
        let mut stmt = conn
            .prepare("SELECT principal, enabled, updated_at, updated_by FROM notify_server_prefs")
            .map_err(|e| format!("notify prefs read: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Row { on: r.get::<_, i64>(1)? != 0, updated_at: r.get(2)?, updated_by: r.get(3)? },
                ))
            })
            .map_err(|e| format!("notify prefs read: {e}"))?;
        for row in rows {
            let (p, row) = row.map_err(|e| format!("notify prefs read: {e}"))?;
            snap.server.insert(p, row);
        }
        Ok(snap)
    }

    /// A person's Thread push for a workspace (§4): own row → `*` row → on.
    pub fn effective(&self, person: &str, project_id: &str) -> Effective {
        if let Some(r) = self.workspace.get(&(person.to_string(), project_id.to_string())) {
            return Effective {
                on: r.on,
                source: Source::User,
                updated_by: Some(r.updated_by.clone()),
                updated_at: Some(r.updated_at),
            };
        }
        self.default_for(project_id)
    }

    /// A workspace's default: its `*` row (`default`) or on (`fallback`).
    pub fn default_for(&self, project_id: &str) -> Effective {
        match self.workspace.get(&(DEFAULT_PRINCIPAL.to_string(), project_id.to_string())) {
            Some(r) => Effective {
                on: r.on,
                source: Source::Default,
                updated_by: Some(r.updated_by.clone()),
                updated_at: Some(r.updated_at),
            },
            None => Effective::fallback(),
        }
    }

    /// A person's server bell: own row → on.
    pub fn bell(&self, person: &str) -> Effective {
        match self.server.get(person) {
            Some(r) => Effective {
                on: r.on,
                source: Source::User,
                updated_by: Some(r.updated_by.clone()),
                updated_at: Some(r.updated_at),
            },
            None => Effective::fallback(),
        }
    }
}

/// True when migration 0157 has run (the `thread-push-v1` feature gate).
pub fn tables_exist(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
         AND name IN ('notify_workspace_prefs', 'notify_server_prefs')",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 2)
    .unwrap_or(false)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Write (`Some`) or delete (`None`) a workspace row. `principal` is a
/// person key or [`DEFAULT_PRINCIPAL`]. Returns whether anything changed
/// on disk (a delete of a missing row is `false`).
pub fn set_workspace(
    conn: &Connection,
    principal: &str,
    project_id: &str,
    thread_push: Option<bool>,
    by: &str,
) -> Result<bool, String> {
    let n = match thread_push {
        Some(on) => conn
            .execute(
                "INSERT INTO notify_workspace_prefs (principal, project_id, thread_push, updated_at, updated_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(principal, project_id) DO UPDATE SET thread_push = excluded.thread_push, \
                 updated_at = excluded.updated_at, updated_by = excluded.updated_by",
                params![principal, project_id, on as i64, now_secs(), by],
            )
            .map_err(|e| format!("notify prefs write: {e}"))?,
        None => conn
            .execute(
                "DELETE FROM notify_workspace_prefs WHERE principal = ?1 AND project_id = ?2",
                params![principal, project_id],
            )
            .map_err(|e| format!("notify prefs write: {e}"))?,
    };
    Ok(n > 0)
}

/// Write a person's server bell. `principal` must be a person key, never
/// [`DEFAULT_PRINCIPAL`].
pub fn set_server(conn: &Connection, principal: &str, enabled: bool, by: &str) -> Result<(), String> {
    if principal == DEFAULT_PRINCIPAL || principal.trim().is_empty() {
        return Err("usage: the server bell belongs to a person, not a default".into());
    }
    conn.execute(
        "INSERT INTO notify_server_prefs (principal, enabled, updated_at, updated_by) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(principal) DO UPDATE SET enabled = excluded.enabled, \
         updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![principal, enabled as i64, now_secs(), by],
    )
    .map_err(|e| format!("notify prefs write: {e}"))?;
    Ok(())
}

/// Delete every pref row of a person (user removal, TP11).
pub fn delete_person(conn: &Connection, principal: &str) -> Result<usize, String> {
    let a = conn
        .execute("DELETE FROM notify_workspace_prefs WHERE principal = ?1", params![principal])
        .map_err(|e| format!("notify prefs delete: {e}"))?;
    let b = conn
        .execute("DELETE FROM notify_server_prefs WHERE principal = ?1", params![principal])
        .map_err(|e| format!("notify prefs delete: {e}"))?;
    Ok(a + b)
}

/// TP11 cleanup after a Connect user is removed: their pref rows and their
/// registered devices go. The owner person is never touched (a removed
/// sole-Owner login's rows were keyed `owner`; the owner token keeps them).
pub fn cleanup_removed_user(conn: &Connection, username: &str) -> Result<(), String> {
    let name = username.trim().to_ascii_lowercase();
    if name.is_empty() || name == OWNER {
        return Ok(());
    }
    delete_person(conn, &name)?;
    conn.execute("DELETE FROM push_devices WHERE username = ?1", params![name])
        .map_err(|e| format!("push device cleanup: {e}"))?;
    Ok(())
}

/// One recipient of a push: a person and their device logins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub person: String,
    pub device_usernames: Vec<String>,
}

/// Group devices by person, skipping devices whose login is removed or
/// disabled (TP11).
fn devices_by_person(people: &People, devices: &[PushDevice]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for d in devices {
        if let Some(person) = people.device_person(&d.username) {
            out.entry(person).or_default().insert(d.username.clone());
        }
    }
    out
}

/// Who receives a Thread push for `project_id` (§6.2): every person with
/// at least one eligible device on this daemon, whose bell is on, whose
/// effective Thread push for the workspace is on, and who is not viewing
/// (`viewing(person)` is checked at send time, TP26). Guests never appear:
/// they have no devices (TP13). Per-user workspace scopes (TP10) would
/// filter here.
pub fn recipients(
    people: &People,
    prefs: &PrefsSnapshot,
    devices: &[PushDevice],
    project_id: &str,
    viewing: &dyn Fn(&str) -> bool,
) -> Vec<Recipient> {
    devices_by_person(people, devices)
        .into_iter()
        .filter(|(person, _)| prefs.bell(person).on)
        .filter(|(person, _)| prefs.effective(person, project_id).on)
        .filter(|(person, _)| !viewing(person))
        .map(|(person, logins)| Recipient { person, device_usernames: logins.into_iter().collect() })
        .collect()
}

/// The devices a ticket or project push may reach (§10): drop devices of
/// removed or disabled logins and of people whose server bell is off.
pub fn bell_allowed_devices(
    people: &People,
    prefs: &PrefsSnapshot,
    devices: Vec<PushDevice>,
) -> Vec<PushDevice> {
    devices
        .into_iter()
        .filter(|d| match people.device_person(&d.username) {
            Some(person) => prefs.bell(&person).on,
            None => false,
        })
        .collect()
}

/// How many devices a person has on this daemon.
pub fn device_count(people: &People, devices: &[PushDevice], person: &str) -> usize {
    devices
        .iter()
        .filter(|d| people.device_person(&d.username).as_deref() == Some(person))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn user(name: &str, role: Role, disabled: bool) -> ConnectUserView {
        ConnectUserView {
            username: name.to_string(),
            created_at: Utc::now(),
            disabled,
            role,
            was_viewer: false,
            can_clone: true,
            color: None,
        }
    }

    fn device(id: &str, username: &str) -> PushDevice {
        PushDevice {
            device_id: id.to_string(),
            platform: "apns".to_string(),
            token: format!("tok-{id}"),
            username: username.to_string(),
            created_at: 1,
            updated_at: 1,
            last_seen_at: 1,
        }
    }

    /// A fresh migrated DB on this thread with two projects.
    fn db() -> (crate::db::TestDbGuard, String, String) {
        let g = crate::db::scoped_for_test();
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        {
            let h = g.handle();
            let conn = h.lock();
            for (id, name) in [(&a, "alpha"), (&b, "beta")] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                    params![id, name, format!("/tmp/np-{id}")],
                )
                .expect("seed project");
            }
        }
        (g, a, b)
    }

    #[test]
    fn migration_0157_creates_both_tables() {
        let (g, _, _) = db();
        let h = g.handle();
        let conn = h.lock();
        assert!(tables_exist(&conn));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations WHERE name = '0157_notify_prefs'", [], |r| r.get(0))
            .expect("read _migrations");
        assert_eq!(n, 1);
    }

    #[test]
    fn effective_resolves_user_then_default_then_fallback() {
        let (g, a, b) = db();
        let h = g.handle();
        let conn = h.lock();
        let snap = PrefsSnapshot::load(&conn).expect("load");
        let e = snap.effective("gina", &a);
        assert_eq!((e.on, e.source), (true, Source::Fallback), "no rows: on (Q1)");
        assert!(e.updated_by.is_none());

        set_workspace(&conn, DEFAULT_PRINCIPAL, &a, Some(false), "owner").expect("default");
        let snap = PrefsSnapshot::load(&conn).expect("load");
        let e = snap.effective("gina", &a);
        assert_eq!((e.on, e.source), (false, Source::Default));
        assert_eq!(snap.effective("gina", &b).source, Source::Fallback, "other workspace untouched");

        set_workspace(&conn, "gina", &a, Some(true), "admin1").expect("own row");
        let snap = PrefsSnapshot::load(&conn).expect("load");
        let e = snap.effective("gina", &a);
        assert_eq!((e.on, e.source), (true, Source::User));
        assert_eq!(e.updated_by.as_deref(), Some("admin1"));
        assert!(e.updated_at.expect("updated_at") > 0);
        assert_eq!(snap.effective("other", &a).source, Source::Default, "rows are per person");

        // threadPush:null deletes the row → the default again.
        assert!(set_workspace(&conn, "gina", &a, None, "gina").expect("clear"));
        assert!(!set_workspace(&conn, "gina", &a, None, "gina").expect("clear again"), "nothing left");
        let snap = PrefsSnapshot::load(&conn).expect("load");
        assert_eq!(snap.effective("gina", &a).source, Source::Default);
    }

    #[test]
    fn server_bell_defaults_on_and_never_takes_the_default_principal() {
        let (g, _, _) = db();
        let h = g.handle();
        let conn = h.lock();
        assert_eq!(PrefsSnapshot::load(&conn).expect("load").bell("owner"), Effective::fallback());
        set_server(&conn, "owner", false, "owner").expect("bell off");
        let b = PrefsSnapshot::load(&conn).expect("load").bell("owner");
        assert_eq!((b.on, b.source), (false, Source::User));
        assert!(set_server(&conn, DEFAULT_PRINCIPAL, false, "owner").is_err());
    }

    #[test]
    fn deleting_a_project_cascades_its_rows() {
        let (g, a, b) = db();
        let h = g.handle();
        let conn = h.lock();
        set_workspace(&conn, "gina", &a, Some(false), "gina").expect("row a");
        set_workspace(&conn, DEFAULT_PRINCIPAL, &a, Some(false), "owner").expect("default a");
        set_workspace(&conn, "gina", &b, Some(false), "gina").expect("row b");
        conn.execute("DELETE FROM projects WHERE id = ?1", params![a]).expect("delete project");
        let left: Vec<String> = conn
            .prepare("SELECT project_id FROM notify_workspace_prefs")
            .expect("prep")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(left, vec![b.clone()]);
        // A row for a project that does not exist is refused (FK).
        assert!(set_workspace(&conn, "gina", "no-such-project", Some(true), "gina").is_err());
    }

    #[test]
    fn person_key_maps_the_sole_owner_role_user_to_owner() {
        let one = People {
            linked: vec![],
            users: vec![user("rosson", Role::Owner, false), user("gina", Role::Admin, false)],
        };
        assert_eq!(one.person_key("owner"), OWNER, "owner token");
        assert_eq!(one.person_key("rosson"), OWNER, "the only Owner-role user is the owner");
        assert_eq!(one.person_key("gina"), "gina");
        assert_eq!(one.device_person("rosson").as_deref(), Some(OWNER));

        // Two Owner-role users: neither maps (Q6).
        let two = People {
            linked: vec![],
            users: vec![user("rosson", Role::Owner, false), user("wes", Role::Owner, false)],
        };
        assert_eq!(two.person_key("rosson"), "rosson");
        assert_eq!(two.person_key("wes"), "wes");
        assert_eq!(two.person_key("owner"), OWNER);

        // A role change takes effect on the next read (nothing stored).
        let demoted = People {
            linked: vec![],
            users: vec![user("rosson", Role::Admin, false), user("gina", Role::Admin, false)],
        };
        assert_eq!(demoted.person_key("rosson"), "rosson");
        assert_eq!(demoted.role_of(OWNER), Some(Role::Owner));
        assert_eq!(demoted.role_of("gina"), Some(Role::Admin));
        assert_eq!(demoted.role_of("ghost"), None);
    }

    /// Owner-linked accounts: a linked login IS the owner person, whatever
    /// its role (two Owner-role users or an Admin), and an unlinked Admin
    /// stays a separate person. Identity only: `role_of` the LOGIN is
    /// still read from the store by the routes, never from the person.
    #[test]
    fn linked_accounts_resolve_to_the_owner_person() {
        let p = People {
            linked: vec!["Z3thon".into()],
            users: vec![
                user("z3thon", Role::Admin, false),
                user("gina", Role::Admin, false),
                user("rosson", Role::Owner, false),
                user("wes", Role::Owner, false),
            ],
        };
        assert_eq!(p.person_key("z3thon"), OWNER, "linked Admin login → owner person");
        assert_eq!(p.person_key("gina"), "gina", "an unlinked Admin is a separate person");
        assert_eq!(p.person_key("rosson"), "rosson", "two Owner-role users: no automatic mapping");
        assert_eq!(p.device_person("z3thon").as_deref(), Some(OWNER));
        assert_eq!(p.owner_aliases(), vec!["z3thon".to_string()]);
        assert_eq!(p.roster()[0].logins, vec!["owner".to_string(), "z3thon".to_string()]);
        assert!(p.roster().iter().all(|r| r.principal != "z3thon"), "folded into owner");
        // The login's own role is untouched by the link.
        assert_eq!(p.user("z3thon").map(|u| u.role), Some(Role::Admin));
        // A disabled linked login's devices still get nothing.
        let d = People { linked: vec!["z3thon".into()], users: vec![user("z3thon", Role::Admin, true)] };
        assert_eq!(d.device_person("z3thon"), None);
    }

    #[test]
    fn roster_lists_owner_first_and_folds_the_sole_owner() {
        let p = People {
            linked: vec![],
            users: vec![
                user("zed", Role::Member, true),
                user("rosson", Role::Owner, false),
                user("gina", Role::Admin, false),
            ],
        };
        let r = p.roster();
        let names: Vec<&str> = r.iter().map(|x| x.principal.as_str()).collect();
        assert_eq!(names, vec!["owner", "gina", "zed"]);
        assert_eq!(r[0].logins, vec!["owner".to_string(), "rosson".to_string()]);
        assert!(r[2].disabled, "disabled people are listed and marked");
    }

    #[test]
    fn recipients_skip_disabled_no_device_bell_off_pref_off_and_viewers() {
        let (g, a, _) = db();
        let h = g.handle();
        let conn = h.lock();
        let people = People {
            linked: vec![],
            users: vec![
                user("rosson", Role::Owner, false),
                user("gina", Role::Admin, false),
                user("mo", Role::Member, false),
                user("dis", Role::Member, true),
                user("nodev", Role::Member, false),
                user("bell", Role::Member, false),
                user("view", Role::Member, false),
            ],
        };
        let devices = vec![
            device("d1", "owner"),
            device("d2", "rosson"),
            device("d3", "gina"),
            device("d4", "mo"),
            device("d5", "dis"),
            device("d6", "bell"),
            device("d7", "view"),
            device("d8", "removed-user"),
        ];
        set_workspace(&conn, "mo", &a, Some(false), "mo").expect("mo off");
        set_server(&conn, "bell", false, "bell").expect("bell off");
        let prefs = PrefsSnapshot::load(&conn).expect("load");
        let r = recipients(&people, &prefs, &devices, &a, &|p| p == "view");
        assert_eq!(
            r,
            vec![
                Recipient { person: "gina".into(), device_usernames: vec!["gina".into()] },
                Recipient {
                    person: OWNER.into(),
                    device_usernames: vec!["owner".into(), "rosson".into()],
                },
            ],
            "owner's desktop + phone are one person; disabled, removed, bell-off, pref-off and viewers drop"
        );

        // A default off for the workspace silences everyone without a row.
        set_workspace(&conn, DEFAULT_PRINCIPAL, &a, Some(false), "owner").expect("default off");
        set_workspace(&conn, "gina", &a, Some(true), "gina").expect("gina on");
        let prefs = PrefsSnapshot::load(&conn).expect("load");
        let r = recipients(&people, &prefs, &devices, &a, &|_| false);
        let persons: Vec<&str> = r.iter().map(|x| x.person.as_str()).collect();
        assert_eq!(persons, vec!["gina"]);
    }

    #[test]
    fn ticket_and_project_pushes_follow_the_bell_and_skip_disabled_users() {
        let (g, _, _) = db();
        let h = g.handle();
        let conn = h.lock();
        let people = People {
            linked: vec![],
            users: vec![user("gina", Role::Admin, false), user("dis", Role::Member, true), user("mo", Role::Member, false)],
        };
        set_server(&conn, "mo", false, "mo").expect("mo bell off");
        let prefs = PrefsSnapshot::load(&conn).expect("load");
        let kept: Vec<String> = bell_allowed_devices(
            &people,
            &prefs,
            vec![device("a", "owner"), device("b", "gina"), device("c", "dis"), device("d", "mo"), device("e", "gone")],
        )
        .into_iter()
        .map(|d| d.username)
        .collect();
        assert_eq!(kept, vec!["owner".to_string(), "gina".to_string()]);
        assert_eq!(device_count(&people, &[device("a", "owner"), device("b", "owner")], OWNER), 2);
    }

    #[test]
    fn human_from_matches_owner_display_and_connect_users() {
        let p = People { linked: vec![], users: vec![user("gina", Role::Member, false)] };
        assert!(p.is_human_from("owner", "Rosson"));
        assert!(p.is_human_from("rosson", "Rosson"));
        assert!(p.is_human_from("GINA", ""));
        assert!(!p.is_human_from("k2", "Rosson"), "a room handle is an agent");
        assert!(!p.is_human_from("k2/reviewer", "Rosson"));
        assert!(!p.is_human_from("", "Rosson"));
    }

    #[test]
    fn removed_user_cleanup_drops_rows_and_devices_but_never_the_owner() {
        let (g, a, _) = db();
        let h = g.handle();
        let conn = h.lock();
        set_workspace(&conn, "gina", &a, Some(false), "gina").expect("row");
        set_server(&conn, "gina", false, "gina").expect("bell");
        set_server(&conn, OWNER, true, "owner").expect("owner bell");
        crate::push_devices::upsert_device("np-dev-gina", "apns", "np-tok-gina", "gina").expect("device");
        conn.execute(
            "INSERT INTO push_devices (device_id, platform, token, username, created_at, updated_at, last_seen_at) \
             VALUES ('np-dev-g2', 'fcm', 'np-tok-g2', 'gina', 1, 1, 1)",
            [],
        )
        .expect("device");
        cleanup_removed_user(&conn, "Gina").expect("cleanup");
        let snap = PrefsSnapshot::load(&conn).expect("load");
        assert_eq!(snap.bell("gina").source, Source::Fallback);
        assert_eq!(snap.effective("gina", &a).source, Source::Fallback);
        assert_eq!(snap.bell(OWNER).source, Source::User, "owner untouched");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM push_devices WHERE username = 'gina'", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0);
        cleanup_removed_user(&conn, "owner").expect("no-op for owner");
        assert_eq!(PrefsSnapshot::load(&conn).expect("load").bell(OWNER).source, Source::User);
    }
}
