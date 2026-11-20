//! Who a permission card asks (0.46.17; migration 0318).
//!
//! Before, a card went to every owner or admin who may answer it, with a
//! push to each. On a server where some owners are clients that agents must
//! not contact, an agent couldn't safely ask. Now:
//!
//! - **The agent names them:** `k2 permission request … --to ada,owner`.
//!   Each name is a Connect login that may answer the action (its role meets
//!   the floor), or `owner` (the owner token's person, the box's own
//!   credential). An unknown name, a disabled login or one below the floor
//!   is refused with a clear error and nothing is asked.
//! - **The owner's default** (`permissionApprovers`, Settings → Access →
//!   Permission Cards, or the `permission.approvers` card): who agents'
//!   cards go to when the agent names no one. Filtered to who may answer
//!   each card; when no one listed may, the card goes to `owner`. Empty (the
//!   default) is today's behaviour: everyone who may answer.
//!
//! It narrows **who is asked**: the people pushed, the Tickets item's
//! assignees, the logins who get Allow and Deny on the card, and "Waiting
//! on you". Never who may answer: `k2 permission approve` and Settings still
//! take an answer from anyone at the floor, and the owner token sees and
//! answers every card (it is the server itself).
//!
//! A Garden owner's card (`garden-owner`) is that person's: the owner's
//! default never narrows it (a `--to` still may, among those who can answer).
//! The set is written once, when the request is made (no rows: everyone).

use k2_core::connect_users::{self, Role};
use k2_core::notify_prefs::People;
use k2_core::permission::catalog::Floor;
use k2_core::permission::request::PermissionRequest;
use rusqlite::params;
use serde_json::Value;

use super::actions::Answerer;
use super::present;

/// The owner token's person: always eligible, never a login.
pub(crate) const OWNER: &str = k2_core::notify_prefs::OWNER;

/// How many names one `--to` may carry.
pub(crate) const MAX_NAMES: usize = k2_core::app_settings::PERMISSION_APPROVERS_MAX;

/// A `--to` that names no one who may be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused {
    pub code: &'static str,
    pub message: String,
}

/// Error codes of [`check_named`].
pub(crate) mod code {
    pub const UNKNOWN: &str = "unknown_approver";
    pub const NOT_ELIGIBLE: &str = "approver_not_eligible";
    pub const BAD: &str = "bad_approvers";
}

/// `to` as sent: a JSON array of names or one comma-separated string.
/// Nothing (or only blanks): `None`, the default applies.
pub(crate) fn parse(raw: Option<&Value>) -> Result<Option<Vec<String>>, Refused> {
    let bad = || Refused { code: code::BAD, message: "--to takes logins, comma-separated (--to ada,owner)".to_string() };
    let names: Vec<String> = match raw {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect(),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for it in items {
                let s = it.as_str().ok_or_else(bad)?;
                out.extend(s.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()));
            }
            out
        }
        Some(_) => return Err(bad()),
    };
    if names.is_empty() {
        return Ok(None);
    }
    if names.len() > MAX_NAMES {
        return Err(Refused { code: code::BAD, message: format!("--to names at most {MAX_NAMES} people") });
    }
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(Some(out))
}

fn role_wire(r: Role) -> &'static str {
    r.as_wire()
}

/// Each of `names` may be asked for a card at `floor`: `owner`, or an
/// enabled login whose role meets the floor. The names, as stored.
pub(crate) fn check_named(names: &[String], floor: Floor) -> Result<Vec<String>, Refused> {
    let users = connect_users::list_users().map_err(|e| Refused { code: code::BAD, message: e })?;
    for n in names {
        if n == OWNER {
            continue;
        }
        let Some(u) = users.iter().find(|u| &u.username == n) else {
            return Err(Refused {
                code: code::UNKNOWN,
                message: format!("there's no login named {n} on this server: --to takes the logins of owners or admins, or owner (a dry run lists who may answer: --dry-run)"),
            });
        };
        let who = Answerer::Login { user: u.username.clone(), role: role_wire(u.role).to_string() };
        if u.disabled || connect_users::is_reserved_username(&u.username) || !present::meets_floor(floor, &who) {
            return Err(Refused {
                code: code::NOT_ELIGIBLE,
                message: format!(
                    "{n} can't answer this card: it needs {} (a dry run lists who may answer: --dry-run)",
                    if floor == Floor::Admin { "an owner or admin" } else { "an owner" }
                ),
            });
        }
    }
    Ok(names.to_vec())
}

/// The owner's default (`permissionApprovers`): empty = everyone.
pub(crate) fn server_default() -> Vec<String> {
    k2_core::app_settings::load().permission_approvers
}

/// Who a new request asks. `named`: the agent's `--to`, already checked.
/// Without it, the owner's default, kept to those who may answer at
/// `floor` (`owner` when none of them may). `None`: everyone who may answer
/// (no default, or a Garden owner's card with no `--to`).
pub(crate) fn for_new(named: Option<Vec<String>>, floor: Floor) -> Option<Vec<String>> {
    if let Some(n) = named {
        return Some(n);
    }
    if floor == Floor::GardenOwner {
        return None;
    }
    let default = server_default();
    if default.is_empty() {
        return None;
    }
    let eligible = present::eligible_logins(floor);
    let mut out: Vec<String> = default.into_iter().filter(|n| n == OWNER || eligible.contains(n)).collect();
    if out.is_empty() {
        out.push(OWNER.to_string());
    }
    Some(out)
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn).map_err(|e| format!("permission_request_approvers: {e}"))
}

/// Keep who `request_id` asks (once, when it is made).
pub(crate) fn set(request_id: &str, names: &[String]) -> Result<(), String> {
    with_conn(|c| {
        for n in names {
            c.execute("INSERT OR IGNORE INTO permission_request_approvers (request_id, approver) VALUES (?1, ?2)", params![request_id, n])?;
        }
        Ok(())
    })
}

/// Who `request_id` asks; `None`: everyone who may answer.
pub(crate) fn of(request_id: &str) -> Option<Vec<String>> {
    let names: Vec<String> = with_conn(|c| {
        let mut st = c.prepare("SELECT approver FROM permission_request_approvers WHERE request_id = ?1 ORDER BY approver")?;
        let rows = st.query_map(params![request_id], |r| r.get::<_, String>(0))?;
        rows.collect()
    })
    .unwrap_or_else(|e| {
        k2_core::log_debug!("[permission] approvers of {request_id}: {e}");
        Vec::new()
    });
    (!names.is_empty()).then_some(names)
}

/// The logins of a narrowed set (without `owner`).
pub(crate) fn logins_of(names: &[String]) -> Vec<String> {
    names.iter().filter(|n| n.as_str() != OWNER).cloned().collect()
}

/// The logins a request's card is pushed to and its Tickets item assigned
/// to: those it asks, or every login at its floor.
pub(crate) fn audience(req: &PermissionRequest) -> Vec<String> {
    match of(req.id.as_str()) {
        Some(names) => logins_of(&names),
        None => present::eligible_logins(present::floor_of(&req.floor)),
    }
}

/// [`audience`] for a request just stored, after writing the owner's
/// default for it (a card K2 raises itself, with no `--to`).
pub(crate) fn audience_for_new(req: &PermissionRequest) -> Vec<String> {
    if of(req.id.as_str()).is_none() {
        if let Some(names) = for_new(None, present::floor_of(&req.floor)) {
            if let Err(e) = set(req.id.as_str(), &names) {
                k2_core::log_debug!("[permission] approvers for {}: {e}", req.id.as_str());
            }
        }
    }
    audience(req)
}

/// The people (`People::person_key`) a narrowed set asks: each login's
/// person, and the owner token's person when `owner` is named or a named
/// login is the owner's own.
pub(crate) fn people_of(people: &People, names: &[String]) -> std::collections::BTreeSet<String> {
    names.iter().map(|n| if n == OWNER { OWNER.to_string() } else { people.person_key(n) }).collect()
}

/// Was `who` asked by `request_id`? Always, when it asks everyone; the
/// owner token always (the server itself); a login when it is named, or
/// when it is the owner's own login and `owner` is named.
pub(crate) fn asked(request_id: &str, who: &Answerer) -> bool {
    let Some(names) = of(request_id) else { return true };
    match who {
        Answerer::OwnerToken => true,
        Answerer::Login { user, .. } => {
            let user = user.trim().to_ascii_lowercase();
            if names.contains(&user) {
                return true;
            }
            names.iter().any(|n| n == OWNER) && People::load().is_ok_and(|p| p.person_key(&user) == OWNER)
        }
    }
}

/// `routedTo` for a request: `[{login, role}]` of who is asked (`owner`
/// with role `owner` for the owner token's person).
pub(crate) fn routed_json(floor: Floor, names: Option<&[String]>) -> Value {
    let roles: std::collections::HashMap<String, String> =
        connect_users::list_users().unwrap_or_default().into_iter().map(|u| (u.username, role_wire(u.role).to_string())).collect();
    let list: Vec<String> = match names {
        Some(n) => n.to_vec(),
        None => present::eligible_logins(floor),
    };
    Value::Array(
        list.into_iter()
            .map(|l| {
                let role = if l == OWNER { "owner".to_string() } else { roles.get(&l).cloned().unwrap_or_default() };
                serde_json::json!({ "role": role, "login": l })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_is_a_list_or_one_comma_string_and_nothing_is_the_default() {
        assert_eq!(parse(None).unwrap(), None);
        assert_eq!(parse(Some(&json!(""))).unwrap(), None);
        assert_eq!(parse(Some(&json!(" , "))).unwrap(), None);
        assert_eq!(parse(Some(&json!("Ada, owner,ada"))).unwrap(), Some(vec!["ada".to_string(), "owner".to_string()]));
        assert_eq!(parse(Some(&json!(["ada", "bo,cy"]))).unwrap(), Some(vec!["ada".to_string(), "bo".to_string(), "cy".to_string()]));
        assert_eq!(parse(Some(&json!(7))).unwrap_err().code, code::BAD);
        assert_eq!(parse(Some(&json!([1]))).unwrap_err().code, code::BAD);
        let many: Vec<String> = (0..=MAX_NAMES).map(|i| format!("u{i}")).collect();
        assert_eq!(parse(Some(&json!(many.join(",")))).unwrap_err().code, code::BAD);
    }
}
