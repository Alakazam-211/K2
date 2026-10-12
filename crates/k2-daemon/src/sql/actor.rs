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
//! | kind    | actor                  | when                                      |
//! |---------|------------------------|-------------------------------------------|
//! | `guest` | `guest:<principal>`    | a guest principal's own login session     |
//! | `app`   | `app:<pass name>`      | reserved: platform / App pass (refused in |
//! |         |                        | this lane today)                          |
//! | `agent` | `agent:<workspace>`    | reserved for S43 delegation: an agent     |
//! |         |                        | acting for a user (`on_behalf_of`)        |

use k2_core::skin::SkinPass;

use super::ops::OpsError;

/// A guest principal using its own login session (password sign-in).
pub const ACTOR_KIND_GUEST: &str = "guest";
/// Reserved: a platform / App pass (`k2 app token`). Not built: platform
/// passes are refused by the guest doors.
#[allow(dead_code)]
pub const ACTOR_KIND_APP: &str = "app";
/// Reserved for S43: an agent workspace acting on behalf of a user.
#[allow(dead_code)]
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
}

impl GuestActor {
    /// A guest's own login session. No validation: [`Self::from_pass`]
    /// validates, and the guest connection re-checks the principal.
    pub fn session(principal: &str, pass_id: &str) -> Self {
        let principal = principal.trim().to_string();
        Self {
            actor: format!("{ACTOR_KIND_GUEST}:{principal}"),
            principal,
            kind: ACTOR_KIND_GUEST,
            pass: pass_id.trim().to_string(),
        }
    }

    /// Derive the actor from the resolved pass. Only the credential
    /// counts; nothing in the request body can change it.
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
        Ok(Self::session(principal, &pass.id))
    }
}

fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The actor lock, run as the migrator in the same `-c` (one implicit
/// transaction) as the principal lock and before `SET ROLE`. No grant to
/// the guest login: `k2.actor_info()` is SECURITY DEFINER and only reads
/// a lock owned by its own owner (the migrator), so a DSN holder's own
/// temp table of that name is ignored.
pub(crate) fn actor_lock_sql(actor: &GuestActor) -> String {
    format!(
        "CREATE TEMP TABLE pg_temp.k2_actor_lock (kind text NOT NULL, actor text NOT NULL, principal uuid NOT NULL, pass text) ON COMMIT PRESERVE ROWS;\n\
         INSERT INTO pg_temp.k2_actor_lock (kind, actor, principal, pass) VALUES ({kind}, {actor}, {principal}::uuid, {pass});",
        kind = quote_literal(actor.kind),
        actor = quote_literal(&actor.actor),
        principal = quote_literal(&actor.principal),
        pass = if actor.pass.is_empty() {
            "NULL".to_string()
        } else {
            quote_literal(&actor.pass)
        },
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
           'pass', l.pass)
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
        }
    }

    const UID: &str = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";

    #[test]
    fn login_session_is_a_guest_actor_named_by_its_principal() {
        let a = GuestActor::from_pass(&pass(true, Some(UID))).expect("session");
        assert_eq!(a.kind, "guest");
        assert_eq!(a.actor, format!("guest:{UID}"));
        assert_eq!(a.principal, UID);
        assert_eq!(a.pass, "7d1f3c1e-0b5a-4d55-9a0e-1f2a3b4c5d6e");
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
                "VALUES ('guest', 'guest:{UID}', '{UID}'::uuid, 'p''1')"
            )),
            "{sql}"
        );
        assert!(
            !sql.to_ascii_uppercase().contains("GRANT"),
            "no grant to the guest: {sql}"
        );
        a.pass.clear();
        assert!(
            actor_lock_sql(&a).contains("::uuid, NULL)"),
            "{}",
            actor_lock_sql(&a)
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
