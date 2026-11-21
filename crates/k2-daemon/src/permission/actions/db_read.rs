//! `db.read db=<name|id>` (0.46.17): let the asking workspace READ another
//! workspace's database. The reader's side asks (`k2 db request-read`, `k2
//! app db connect <app> --read-only`, or `k2 permission request db.read
//! db=…`); before, only a person could, with `k2 db grant <ws> --db <name>
//! --level read`.
//!
//! - **Who asks:** any agent, for its own workspace only (the grantee is
//!   never a value). The database may be any active one on the server
//!   except its own (`k2 db browse` lists them by name; names aren't a
//!   secret, a DSN never leaves the daemon).
//! - **Who answers:** the owner token, an Owner or an Admin login (the card's
//!   floor; `/cli/db/grant`'s own route floor runs again at the click). Never
//!   an agent: a passport can't answer a card.
//! - **Proof:** a click (`k2 db grant` takes no confirm).
//! - **On Allow:** `/cli/db/grant`'s own entry ([`crate::sql::routes::handle_grant`])
//!   as the person who clicked, with `level: read`, no `--manage` and no
//!   relation fence, for exactly the database the card names (by its id).
//!   Deny changes nothing.
//! - **Stale, never wrong:** the card's words carry the database's id and
//!   owner, so a name that comes to mean another database between the ask
//!   and the click changes the text hash (`stale`), and the grant never goes
//!   to the other one. Already read (or write) access: `already_done`.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo, WallCoverage,
    Words,
};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::db_read_routes::{self as dbr, Db};
use k2_core::brand::CLI_NAME;

/// The card's id.
pub(crate) const ACTION: &str = dbr::ACTION;
/// `k2 db grant <ws> --db <name> --level read`.
pub(crate) const ROUTE: &str = "/cli/db/grant";

pub(crate) static READ: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Misc,
    params: &[ParamSpec { key: "db", kind: ParamKind::Database, required: true, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Read access to the {name} database",
        what_changes: "Let {ws} read the {name} database (owned by {owner})",
        consequences: "{effect}",
        scope: "{name} (id {id}) only, read-only, for {ws}",
        duration: "until someone revokes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "data", path: "Settings → Database → Database access" },
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

fn spec_of(ctx: &Ctx) -> Result<&str, String> {
    text_param(ctx, "db").ok_or_else(|| format!("name the database: db=<name> ({CLI_NAME} db browse lists them)"))
}

/// The database the card names, checked: active, and not the asker's own.
fn target(ctx: &Ctx) -> Result<Db, String> {
    let db = dbr::find(spec_of(ctx)?).map_err(|(_, _, hint)| hint)?;
    if db.owner_id == ctx.workspace_id {
        return Err(format!(
            "{} owns {}: it reads and writes it already ({CLI_NAME} db dsn --db {}), no card needed",
            ctx.workspace_handle, db.name, db.name
        ));
    }
    Ok(db)
}

/// The card's computed words (`{name}`, `{id}`, `{owner}`, `{effect}`),
/// from K2's own records, never the agent's.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let ws = &ctx.workspace_handle;
    let (name, id, owner, owner_ws) = match spec_of(ctx).ok().map(dbr::find) {
        Some(Ok(db)) => (db.name.clone(), db.id.clone(), dbr::owner_words(&db), db.owner_handle()),
        _ => {
            let spec = text_param(ctx, "db").unwrap_or("?").to_string();
            (spec.clone(), spec, "a workspace this server can't find".to_string(), "its owner".to_string())
        }
    };
    out.params.insert("name".into(), json!(name));
    out.params.insert("id".into(), json!(id));
    out.params.insert("owner".into(), json!(owner));
    out.params.insert(
        "effect".into(),
        json!(format!(
            "{ws} gets its own read-only login on {name}: it can read every table there (never the _k2_* tables the server keeps for itself), and can't add, change or delete rows or change \
             the tables. Nothing else of {owner_ws}'s opens to it. It lasts until someone revokes it: Settings → Database → Database \
             access, or {CLI_NAME} db revoke {ws} --db {name}."
        )),
    );
    Some(out)
}

fn check(ctx: &Ctx) -> Check {
    let db = match target(ctx) {
        Ok(d) => d,
        Err(e) => return Check::Moved(e),
    };
    if dbr::access_of(&ctx.workspace_id, &db).is_some() {
        return Check::AlreadyDone;
    }
    Check::Needed
}

/// Through `/cli/db/grant`'s own entry, as the person who clicked: read, for
/// the asking workspace, on exactly this database.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let db = match target(ctx) {
        Ok(d) => d,
        Err(e) => return refused("bad_params", e),
    };
    if let Err(r) = route_floor(ROUTE, who) {
        return r;
    }
    // A card is answered by a person, whose request carries no passport: the
    // route then acts as them (the owner's rules). Never from a session.
    if crate::caller_workspace::request_principal().is_some() {
        return refused("person_only", "a person answers this card, never an agent's session".into());
    }
    let body = json!({ "project": ctx.workspace_id, "db": db.id, "level": "read", "manage": false, "relations": [] });
    let r = crate::sql::routes::handle_grant(body.to_string().as_bytes());
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "db_grant_refused");
        return Applied::Refused { code, message };
    }
    if v["level"] != "read" || v["databaseId"] != json!(db.id) || v["projectId"] != json!(ctx.workspace_id) {
        return refused("failed", format!("the grant came back for something else: {v}"));
    }
    Applied::Done {
        message: format!(
            "{} can read {} now (read-only, until someone revokes it): {CLI_NAME} db dsn --db {} gives your read-only DSN (no password shown), and {CLI_NAME} db run --db {} -- <cmd> runs a command with it.",
            ctx.workspace_handle, db.name, db.name, db.name
        ),
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &READ, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "db_read_tests.rs"]
mod tests;
