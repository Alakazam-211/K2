//! Who is making a brokered `k2 db` call (the actor stamp).
//!
//! The daemon derives the actor from the credential that reached the
//! guest doors (`POST /cli/db/query` and the rows doors), never from the
//! request body. It is locked into the connection next to the principal
//! lock, before `SET ROLE`, in a `pg_temp` table the migrator owns. App
//! RLS policies and audit tables read it through `k2.actor_kind()`,
//! `k2.actor()` and `k2.actor_info()`. Outside the brokered lane (an
//! owner or agent DSN, a BFF's own `k2.set_principal` lane) all three
//! are NULL.
//!
//! Kinds (the forward shape follows the App runtime PRD §5.5 / §5.11):
//!
//! | kind    | actor               | skin_uid      | workspace | on_behalf_of  | when |
//! |---------|---------------------|---------------|-----------|---------------|------|
//! | `guest` | `guest:<principal>` | the principal | null      | null          | a person's own login session |
//! | `agent` | `agent:<handle>`    | the principal | ws id     | null          | the owner marked this guest principal as a workspace's agent (`k2 app user agent`) |
//! | `agent` | `agent:<handle>`    | the user      | ws id     | `guest:<uid>` | reserved for S43 delegation (an agent acting for a user); not built |
//! | `app`   | `app:<pass name>`   | —             | —         | —             | reserved: platform / App pass (refused in this lane today) |
//!
//! A marked agent principal is the agent acting as itself, so
//! `on_behalf_of` is null; S43's delegated call is the only shape where
//! it is set. Policies that want "a person, in person" check
//! `k2.actor_kind() = 'guest'`; that refuses both agent shapes.

use k2_core::skin::SkinPass;

use super::ops::OpsError;

/// A person: a guest principal using its own login session.
pub const ACTOR_KIND_GUEST: &str = "guest";
/// Reserved: a platform / App pass (`k2 app token`). Not built: platform
/// passes are refused by the guest doors.
#[allow(dead_code)]
pub const ACTOR_KIND_APP: &str = "app";
/// An agent: a guest principal the owner marked as a workspace's agent
/// (built), or, from S43, an agent acting on behalf of a user (reserved).
pub const ACTOR_KIND_AGENT: &str = "agent";

/// The actor locked into one guest connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestActor {
    /// Locked principal (`k2.skin_uid()`).
    pub principal: String,
    /// One of the `ACTOR_KIND_*` values.
    pub kind: &'static str,
    /// `<kind>:<id>`.
    pub actor: String,
    /// The pass row id (`skin.db` `tokens.id`). Never the secret.
    pub pass: String,
    /// Kind `agent`: the workspace (`projects.id`). `None` for a person.
    pub workspace: Option<String>,
    /// Reserved for S43 delegation. Always `None` today; the guest
    /// connection refuses a value.
    pub on_behalf_of: Option<String>,
}

fn label_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

impl GuestActor {
    /// A person's own login session. No validation: [`Self::from_pass`]
    /// validates, and the guest connection re-checks ([`Self::check`]).
    pub fn session(principal: &str, pass_id: &str) -> Self {
        let principal = principal.trim().to_string();
        Self {
            actor: format!("{ACTOR_KIND_GUEST}:{principal}"),
            principal,
            kind: ACTOR_KIND_GUEST,
            pass: pass_id.trim().to_string(),
            workspace: None,
            on_behalf_of: None,
        }
    }

    /// A guest principal the owner marked as the agent of `workspace_id`.
    /// The actor names the live handle; with no handle (workspace gone or
    /// unnamed) it names the id, and the kind stays `agent`.
    pub fn marked_agent(
        principal: &str,
        pass_id: &str,
        workspace_id: &str,
        handle: Option<&str>,
    ) -> Self {
        let workspace_id = workspace_id.trim().to_string();
        let label = handle
            .map(str::trim)
            .filter(|h| label_ok(h))
            .unwrap_or(workspace_id.as_str())
            .to_string();
        Self {
            actor: format!("{ACTOR_KIND_AGENT}:{label}"),
            kind: ACTOR_KIND_AGENT,
            workspace: Some(workspace_id),
            ..Self::session(principal, pass_id)
        }
    }

    /// Derive the actor from the resolved pass and the owner's mark on its
    /// principal (read live from `skin.db`). Only the credential and the
    /// owner's mark count; nothing in the request body can change it. A
    /// failed mark lookup fails the call (never falls back to `guest`).
    pub fn from_pass(pass: &SkinPass) -> Result<Self, OpsError> {
        if !pass.session {
            return Err(OpsError::Forbidden(
                "platform passes have no actor in the brokered db lane".into(),
            ));
        }
        let principal = pass
            .principal_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| OpsError::Usage("session has no principal id".into()))?;
        if uuid::Uuid::parse_str(principal).is_err() {
            return Err(OpsError::Usage("session principal id is not a uuid".into()));
        }
        let mark = k2_core::skin::principal_agent_workspace(principal)
            .map_err(|e| OpsError::Engine(format!("actor lookup failed: {e}")))?;
        Ok(match mark {
            None => Self::session(principal, &pass.id),
            Some(ws) => {
                let handle = k2_core::skin::handles_for_project_ids(std::slice::from_ref(&ws))
                    .into_iter()
                    .next();
                Self::marked_agent(principal, &pass.id, &ws, handle.as_deref())
            }
        })
    }

    /// The shapes the guest connection may lock. Anything else is a daemon
    /// bug and is never run.
    pub(crate) fn check(&self) -> Result<(), OpsError> {
        let principal = self.principal.trim();
        if uuid::Uuid::parse_str(principal).is_err() {
            return Err(OpsError::Usage("session principal id is not a uuid".into()));
        }
        let unsupported = || Err(OpsError::Engine("internal: unsupported actor".into()));
        if self.on_behalf_of.is_some() {
            // S43 delegation is not built.
            return unsupported();
        }
        match (self.kind, self.workspace.as_deref()) {
            (ACTOR_KIND_GUEST, None) if self.actor == format!("{ACTOR_KIND_GUEST}:{principal}") => {
                Ok(())
            }
            (ACTOR_KIND_AGENT, Some(ws)) if uuid::Uuid::parse_str(ws).is_ok() => {
                match self.actor.strip_prefix("agent:") {
                    Some(label) if label_ok(label) => Ok(()),
                    _ => unsupported(),
                }
            }
            _ => unsupported(),
        }
    }
}

fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn quote_opt(s: Option<&str>) -> String {
    match s.filter(|s| !s.is_empty()) {
        Some(v) => quote_literal(v),
        None => "NULL".to_string(),
    }
}

/// The actor lock, run as the migrator in the same `-c` (one implicit
/// transaction) as the principal lock and before `SET ROLE`. No grant to
/// the guest login: `k2.actor_info()` is SECURITY DEFINER and only reads
/// a lock owned by its own owner (the migrator), so a DSN holder's own
/// temp table of that name is ignored.
pub(crate) fn actor_lock_sql(actor: &GuestActor) -> String {
    format!(
        "CREATE TEMP TABLE pg_temp.k2_actor_lock (kind text NOT NULL, actor text NOT NULL, principal uuid NOT NULL, pass text, workspace text, on_behalf_of text) ON COMMIT PRESERVE ROWS;\n\
         INSERT INTO pg_temp.k2_actor_lock (kind, actor, principal, pass, workspace, on_behalf_of) VALUES ({kind}, {actor}, {principal}::uuid, {pass}, {workspace}, {obo});",
        kind = quote_literal(actor.kind),
        actor = quote_literal(&actor.actor),
        principal = quote_literal(&actor.principal),
        pass = quote_opt(Some(actor.pass.as_str())),
        workspace = quote_opt(actor.workspace.as_deref()),
        obo = quote_opt(actor.on_behalf_of.as_deref()),
    )
}

/// `k2.actor_info()` / `k2.actor_kind()` / `k2.actor()`. Part of
/// [`super::ops::ensure_k2_helpers_sql`]; `k2.actor_info()` must be owned
/// by the migrator (see [`super::ops::k2_helper_owner_sql`]).
pub(crate) const ACTOR_HELPERS_SQL: &str = r#"CREATE OR REPLACE FUNCTION k2.actor_info() RETURNS jsonb
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $k2fn$
DECLARE
  rel oid;
  info jsonb;
BEGIN
  BEGIN
    rel := 'pg_temp.k2_actor_lock'::regclass;
  EXCEPTION
    WHEN undefined_table OR invalid_schema_name THEN
      RETURN NULL;
  END;
  IF NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_class c
    WHERE c.oid = rel
      AND c.relpersistence = 't'
      AND c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)
  ) THEN
    RETURN NULL;
  END IF;
  SELECT pg_catalog.jsonb_build_object(
           'kind', l.kind,
           'actor', l.actor,
           'principal', l.principal,
           'pass', l.pass,
           'workspace', l.workspace,
           'on_behalf_of', l.on_behalf_of)
    INTO info
    FROM pg_temp.k2_actor_lock l
    LIMIT 1;
  RETURN info;
END;
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.actor_info() TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.actor_kind() RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT k2.actor_info() ->> 'kind'
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.actor_kind() TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.actor() RETURNS text
LANGUAGE sql
STABLE
SET search_path = pg_catalog, pg_temp
AS $k2fn$
  SELECT k2.actor_info() ->> 'actor'
$k2fn$;
GRANT EXECUTE ON FUNCTION k2.actor() TO PUBLIC;
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::skin::RoomPolicy;

    fn pass(session: bool, principal: Option<&str>) -> SkinPass {
        SkinPass {
            id: "7d1f3c1e-0b5a-4d55-9a0e-1f2a3b4c5d6e".into(),
            principal_id: principal.map(str::to_string),
            username: "ada".into(),
            caps: vec![],
            rooms: vec![],
            session,
            room_policy: RoomPolicy::new(),
            app: if session { "host".to_string() } else { String::new() },
            room_paths: Default::default(),
        }
    }

    const UID: &str = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
    const WS: &str = "6f9619ff-8b86-4011-b42d-00c04fc964ff";

    #[test]
    fn unmarked_login_session_is_a_guest_actor_named_by_its_principal() {
        let _home = crate::test_support::TempHome::new();
        let a = GuestActor::from_pass(&pass(true, Some(UID))).expect("session");
        assert_eq!(a.kind, "guest");
        assert_eq!(a.actor, format!("guest:{UID}"));
        assert_eq!(a.principal, UID);
        assert_eq!(a.pass, "7d1f3c1e-0b5a-4d55-9a0e-1f2a3b4c5d6e");
        assert_eq!(a.workspace, None);
        assert_eq!(a.on_behalf_of, None);
        a.check().expect("guest shape");
    }

    #[test]
    fn owner_marked_principal_is_an_agent_actor_and_unmark_flips_it_back() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let ws = uuid::Uuid::new_v4().to_string();
        let handle = format!("salesbot{}", &ws[..8]);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![ws, "Sales bot", format!("/tmp/actor-ws-{ws}"), handle],
            )
            .expect("project");
        }
        let p = k2_core::skin::add_principal("salesbot").expect("principal");
        let session = pass(true, Some(&p.id));

        let before = GuestActor::from_pass(&session).expect("unmarked");
        assert_eq!(before.kind, "guest");

        k2_core::skin::set_principal_agent("salesbot", Some(&handle)).expect("mark");
        let a = GuestActor::from_pass(&session).expect("marked");
        assert_eq!(a.kind, "agent");
        assert_eq!(a.actor, format!("agent:{handle}"));
        assert_eq!(a.principal, p.id, "k2.skin_uid() stays the principal");
        assert_eq!(a.workspace.as_deref(), Some(ws.as_str()));
        assert_eq!(a.on_behalf_of, None, "a marked agent acts as itself, not for a user");
        a.check().expect("marked agent shape");
        let sql = actor_lock_sql(&a);
        assert!(
            sql.contains(&format!(
                "VALUES ('agent', 'agent:{handle}', '{}'::uuid, '7d1f3c1e-0b5a-4d55-9a0e-1f2a3b4c5d6e', '{ws}', NULL)",
                p.id
            )),
            "{sql}"
        );

        k2_core::skin::set_principal_agent("salesbot", None).expect("unmark");
        let after = GuestActor::from_pass(&session).expect("unmarked again");
        assert_eq!(after.kind, "guest");
        assert_eq!(after.actor, format!("guest:{}", p.id));
    }

    #[test]
    fn marked_agent_without_a_handle_names_the_workspace_id() {
        let a = GuestActor::marked_agent(UID, "p", WS, None);
        assert_eq!(a.actor, format!("agent:{WS}"));
        a.check().expect("id label");
        let a = GuestActor::marked_agent(UID, "p", WS, Some("bad handle"));
        assert_eq!(a.actor, format!("agent:{WS}"), "unsafe handle falls back to the id");
    }

    #[test]
    fn check_refuses_delegation_and_mismatched_shapes() {
        let mut d = GuestActor::marked_agent(UID, "p", WS, Some("sales"));
        d.on_behalf_of = Some(format!("guest:{UID}"));
        assert_eq!(d.check().unwrap_err().hint(), "internal: unsupported actor");
        let mut g = GuestActor::session(UID, "p");
        g.workspace = Some(WS.into());
        assert_eq!(g.check().unwrap_err().hint(), "internal: unsupported actor");
        let mut g = GuestActor::session(UID, "p");
        g.actor = "guest:00000000-0000-0000-0000-000000000000".into();
        assert_eq!(g.check().unwrap_err().hint(), "internal: unsupported actor");
        let mut a = GuestActor::marked_agent(UID, "p", WS, Some("sales"));
        a.workspace = Some("sales".into());
        assert_eq!(a.check().unwrap_err().hint(), "internal: unsupported actor");
        let mut a = GuestActor::marked_agent(UID, "p", WS, Some("sales"));
        a.workspace = None;
        assert_eq!(a.check().unwrap_err().hint(), "internal: unsupported actor");
        let mut a = GuestActor::marked_agent(UID, "p", WS, Some("sales"));
        a.kind = ACTOR_KIND_APP;
        assert_eq!(a.check().unwrap_err().hint(), "internal: unsupported actor");
        let g = GuestActor::session("ada", "p");
        assert_eq!(g.check().unwrap_err().hint(), "session principal id is not a uuid");
    }

    #[test]
    fn platform_pass_has_no_actor_in_this_lane() {
        let err = GuestActor::from_pass(&pass(false, None)).expect_err("platform");
        assert_eq!(err.code(), "forbidden", "{}", err.hint());
        // Even a malformed platform row carrying a principal is refused.
        let err = GuestActor::from_pass(&pass(false, Some(UID))).expect_err("platform+principal");
        assert_eq!(err.code(), "forbidden", "{}", err.hint());
    }

    #[test]
    fn session_without_a_uuid_principal_is_refused() {
        let err = GuestActor::from_pass(&pass(true, None)).expect_err("none");
        assert_eq!(err.hint(), "session has no principal id");
        let err = GuestActor::from_pass(&pass(true, Some("  "))).expect_err("blank");
        assert_eq!(err.hint(), "session has no principal id");
        let err = GuestActor::from_pass(&pass(true, Some("ada"))).expect_err("username");
        assert_eq!(err.hint(), "session principal id is not a uuid");
    }

    #[test]
    fn lock_sql_is_typed_quoted_and_grants_nothing() {
        let mut a = GuestActor::session(UID, "p'1");
        let sql = actor_lock_sql(&a);
        assert!(
            sql.starts_with("CREATE TEMP TABLE pg_temp.k2_actor_lock"),
            "{sql}"
        );
        assert!(sql.contains("ON COMMIT PRESERVE ROWS"), "{sql}");
        assert!(
            sql.contains(&format!(
                "VALUES ('guest', 'guest:{UID}', '{UID}'::uuid, 'p''1', NULL, NULL)"
            )),
            "{sql}"
        );
        assert!(
            !sql.to_ascii_uppercase().contains("GRANT"),
            "no grant to the guest: {sql}"
        );
        a.pass.clear();
        assert!(
            actor_lock_sql(&a).contains("::uuid, NULL, NULL, NULL)"),
            "{}",
            actor_lock_sql(&a)
        );
        let agent = GuestActor::marked_agent(UID, "p", WS, Some("o'brien"));
        assert!(
            actor_lock_sql(&agent).contains("'agent:o''brien'"),
            "{}",
            actor_lock_sql(&agent)
        );
    }

    #[test]
    fn helper_sql_reads_only_a_migrator_owned_temp_lock() {
        let sql = ACTOR_HELPERS_SQL;
        let up = sql.to_ascii_uppercase();
        assert!(!up.contains("CREATE ROLE"), "{sql}");
        assert!(
            !up.contains("SET_CONFIG"),
            "actor never comes from a GUC: {sql}"
        );
        assert!(
            !sql.contains("current_setting"),
            "actor never comes from a GUC: {sql}"
        );
        assert!(
            sql.contains("CREATE OR REPLACE FUNCTION k2.actor_info() RETURNS jsonb"),
            "{sql}"
        );
        assert!(
            sql.contains("CREATE OR REPLACE FUNCTION k2.actor_kind() RETURNS text"),
            "{sql}"
        );
        assert!(
            sql.contains("CREATE OR REPLACE FUNCTION k2.actor() RETURNS text"),
            "{sql}"
        );
        for key in ["'kind'", "'actor'", "'principal'", "'pass'", "'workspace'", "'on_behalf_of'"] {
            assert!(sql.contains(key), "actor_info key {key}: {sql}");
        }
        assert_eq!(
            sql.matches("SECURITY DEFINER").count(),
            1,
            "only actor_info: {sql}"
        );
        assert_eq!(sql.matches("STABLE").count(), 3, "{sql}");
        assert_eq!(
            sql.matches("SET search_path = pg_catalog, pg_temp").count(),
            3,
            "pg_catalog first on every helper: {sql}"
        );
        assert!(sql.contains("c.relpersistence = 't'"), "{sql}");
        assert!(
            sql.contains("c.relowner = (SELECT r.oid FROM pg_catalog.pg_roles r WHERE r.rolname = current_user)"),
            "lock must be owned by the definer (migrator): {sql}"
        );
        assert!(
            sql.contains("WHEN undefined_table OR invalid_schema_name"),
            "{sql}"
        );
        for f in ["k2.actor_info()", "k2.actor_kind()", "k2.actor()"] {
            assert!(
                sql.contains(&format!("GRANT EXECUTE ON FUNCTION {f} TO PUBLIC")),
                "{f}: {sql}"
            );
        }
        assert!(!sql.contains("GRANT SELECT"), "{sql}");
    }
}
