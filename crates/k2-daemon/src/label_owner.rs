//! One rule for nested URL labels (`<label>.<host>.k2.dev`): a label
//! belongs to ONE workspace on this server (the 0074 attribution row).
//!
//! Every path that claims, points or runs a label asks [`decide`] first:
//!
//! - unattributed, or already this workspace's → go ahead;
//! - held by ANOTHER workspace → `409 label_owned_by_other_workspace`.
//!   Every caller is told which agent holds it (`owner: {agent, handle,
//!   address}`, N6: so agents can `k2 msg` each other); only an
//!   owner/admin caller also gets the workspace's id, name and path;
//! - an owner/admin may move it deliberately with an explicit transfer
//!   (`transfer=1`, CLI `--transfer`). Every transfer is audited
//!   (`~/.k2/auth-audit.jsonl`, event `label-transfer`).
//!
//! The caller tier is request-scoped: the HTTP dispatcher installs it
//! with [`with_caller`]. An agent passport (the request principal) always
//! wins and is [`Caller::Agent`]. Nothing installed = [`Caller::Member`]
//! (fail closed: no owner hint, no transfer) — boot respawns and direct
//! handler calls land here.

use std::cell::RefCell;

use k2_core::db::schema::{SelfStamp, SubdomainWorkspace};
use rusqlite::Connection;

use crate::cli_response::CliResponse;

pub const CODE: &str = "label_owned_by_other_workspace";
pub const TRANSFER_CODE: &str = "transfer_owner_only";
const STATUS_CONFLICT: &str = "409 Conflict";
const STATUS_FORBIDDEN: &str = "403 Forbidden";

/// Who is asking, for label-ownership purposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// An agent passport (scoped hook / cell UDS).
    Agent,
    /// A Connect Member login, or no tier installed (fail closed).
    Member,
    /// The owner token or an Owner/Admin Connect login. `actor` is a
    /// non-secret label for the audit line.
    Manager { actor: String },
}

impl Caller {
    pub fn is_manager(&self) -> bool {
        matches!(self, Caller::Manager { .. })
    }

    fn actor(&self) -> String {
        match self {
            Caller::Agent => "agent".to_string(),
            Caller::Member => "member".to_string(),
            Caller::Manager { actor } => actor.clone(),
        }
    }
}

thread_local! {
    static CALLER: RefCell<Option<Caller>> = const { RefCell::new(None) };
}

/// Run `f` with `caller` as the request-scoped label caller. Afterward
/// (including on panic) the slot holds what it held before: a nested call
/// (an approved agent request replayed inside the owner's request) hands
/// the rest of the outer request back its own caller, never the
/// fail-closed default.
pub fn with_caller<R>(caller: Caller, f: impl FnOnce() -> R) -> R {
    struct RestoreOnDrop(Option<Caller>);
    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            let previous = self.0.take();
            CALLER.with(|c| *c.borrow_mut() = previous);
        }
    }
    let previous = CALLER.with(|c| c.borrow_mut().replace(caller));
    let _guard = RestoreOnDrop(previous);
    f()
}

/// The current caller. An installed agent passport always means
/// [`Caller::Agent`], whatever tier was installed; nothing installed
/// means [`Caller::Member`].
pub fn current() -> Caller {
    if crate::caller_workspace::request_principal().is_some() {
        return Caller::Agent;
    }
    CALLER.with(|c| c.borrow().clone()).unwrap_or(Caller::Member)
}

/// What a claim of `label` for `project_id` will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Unattributed or already this workspace's.
    Claim,
    /// Owner/admin moving it from another workspace (audited on apply).
    Transfer { from: String },
}

/// A refusal: structured JSON body + HTTP status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: &'static str,
    pub body: serde_json::Value,
}

impl Refusal {
    pub fn message(&self) -> String {
        self.body["error"].as_str().unwrap_or(CODE).to_string()
    }

    pub fn into_response(self) -> CliResponse {
        CliResponse {
            status: self.status,
            content_type: "application/json",
            body: self.body.to_string(),
        }
    }
}

fn workspace_label(conn: &Connection, project_id: &str) -> (Option<String>, Option<String>) {
    conn.query_row(
        "SELECT name, path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .unwrap_or((None, None))
}

/// The 409 for a label held by another workspace. Every caller hears
/// which agent holds it, by display name and `k2 msg` address (N6, F7:
/// agents coordinate instead of guessing); only an owner/admin caller also
/// gets the workspace's id, name and folder path.
pub fn owned_by_other(conn: &Connection, label: &str, owner: &str, caller: &Caller) -> Refusal {
    let label = label.trim().to_ascii_lowercase();
    let m = crate::apps::coordination::maintainer_with(conn, owner);
    let mut owner_json = m.json();
    let ask = match (&m.address, m.kind) {
        (Some(a), "agent") => format!(" Ask it: k2 msg {a} \"…\""),
        _ => String::new(),
    };
    let body = if caller.is_manager() {
        let (name, path) = workspace_label(conn, owner);
        let who = match (&name, &path) {
            (Some(n), Some(p)) => format!("workspace '{n}' ({p})"),
            (Some(n), None) => format!("workspace '{n}'"),
            _ => format!("workspace {owner}"),
        };
        owner_json["projectId"] = serde_json::json!(owner);
        owner_json["name"] = serde_json::json!(name);
        owner_json["path"] = serde_json::json!(path);
        serde_json::json!({
            "error": format!("{CODE}: '{label}' belongs to {who}, {}.", m.phrase()),
            "code": CODE,
            "label": label,
            "hint": "To move it to this workspace on purpose, rerun with --transfer \
                     (owner/admin only; recorded in the access audit log).",
            "owner": owner_json,
        })
    } else {
        serde_json::json!({
            "error": format!("{CODE}: '{label}' belongs to another workspace on this server, {}.{ask}", m.phrase()),
            "code": CODE,
            "label": label,
            "hint": "Pick another name, ask the agent that holds it, or ask the server owner to transfer this one.",
            "owner": owner_json,
        })
    };
    Refusal {
        status: STATUS_CONFLICT,
        body,
    }
}

fn transfer_owner_only(label: &str) -> Refusal {
    let label = label.trim().to_ascii_lowercase();
    Refusal {
        status: STATUS_FORBIDDEN,
        body: serde_json::json!({
            "error": format!(
                "{TRANSFER_CODE}: only the server owner or an admin can transfer '{label}'."
            ),
            "code": TRANSFER_CODE,
            "label": label,
            "hint": "Pick another name, or ask the server owner to transfer this one.",
        }),
    }
}

/// Decide (read only) whether `project_id` may claim/point/run `label`.
pub fn decide(
    conn: &Connection,
    label: &str,
    project_id: &str,
    want_transfer: bool,
    caller: &Caller,
) -> Result<Decision, Refusal> {
    let owner = SubdomainWorkspace::owner_of(conn, label).map_err(|e| Refusal {
        status: "500 Internal Server Error",
        body: serde_json::json!({ "error": e.to_string() }),
    })?;
    match owner {
        None => Ok(Decision::Claim),
        Some(o) if o == project_id.trim() => Ok(Decision::Claim),
        Some(o) if want_transfer && caller.is_manager() => Ok(Decision::Transfer { from: o }),
        Some(_) if want_transfer => Err(transfer_owner_only(label)),
        Some(o) => Err(owned_by_other(conn, label, &o, caller)),
    }
}

/// Write a [`decide`] outcome. `Claim` is the fail-on-conflict insert
/// (a race that lost to another workspace still refuses); `Transfer` is a
/// compare-and-swap on the owner, audited.
pub fn apply(
    conn: &Connection,
    label: &str,
    project_id: &str,
    target: Option<&str>,
    decision: &Decision,
    caller: &Caller,
) -> Result<(), Refusal> {
    let internal = |e: rusqlite::Error| Refusal {
        status: "400 Bad Request",
        body: serde_json::json!({ "error": e.to_string() }),
    };
    match decision {
        Decision::Claim => {
            match SubdomainWorkspace::self_stamp(conn, label, project_id, target)
                .map_err(internal)?
            {
                SelfStamp::Written => Ok(()),
                SelfStamp::Foreign { owner } => Err(owned_by_other(conn, label, &owner, caller)),
            }
        }
        Decision::Transfer { from } => {
            let moved = SubdomainWorkspace::transfer(conn, label, from, project_id, target)
                .map_err(internal)?;
            if !moved {
                // Owner changed since the decision: re-decide without transfer.
                return match SubdomainWorkspace::owner_of(conn, label).map_err(internal)? {
                    Some(o) if o == project_id => Ok(()),
                    Some(o) => Err(owned_by_other(conn, label, &o, caller)),
                    None => match SubdomainWorkspace::self_stamp(conn, label, project_id, target)
                        .map_err(internal)?
                    {
                        SelfStamp::Written => Ok(()),
                        SelfStamp::Foreign { owner } => {
                            Err(owned_by_other(conn, label, &owner, caller))
                        }
                    },
                };
            }
            audit_transfer(label, from, project_id, caller);
            Ok(())
        }
    }
}

/// [`decide`] + [`apply`] in one step on the shared DB.
pub fn claim_checked(
    label: &str,
    project_id: &str,
    target: Option<&str>,
    want_transfer: bool,
) -> Result<Decision, Refusal> {
    let caller = current();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let decision = decide(&conn, label, project_id, want_transfer, &caller)?;
    apply(&conn, label, project_id, target, &decision, &caller)?;
    Ok(decision)
}

/// [`decide`] on the shared DB with the current caller (read only).
pub fn check(label: &str, project_id: &str, want_transfer: bool) -> Result<Decision, Refusal> {
    let caller = current();
    let db = k2_core::db::shared();
    let conn = db.lock();
    decide(&conn, label, project_id, want_transfer, &caller)
}

fn audit_transfer(label: &str, from: &str, to: &str, caller: &Caller) {
    let label = label.trim().to_ascii_lowercase();
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "label-transfer",
        &caller.actor(),
        format!("ok label={label} from={from} to={to}"),
        "-",
        "-",
        "cli",
    ));
    k2_core::log_debug!("[publish] label transfer label={label} from={from} to={to} by={}", caller.actor());
}

/// Reserved nested labels (`skin`, `meet`, `chat`) on the claim routes.
///
/// Run this AFTER the strict `skin` check (which stays first so its 400
/// is unchanged) and after the project id is resolved. A NEW claim of
/// `meet` / `chat` (unattributed, or held by another workspace, with or
/// without `transfer`) is a 400 `reserved_label`. The one thing that
/// still goes through is an idempotent re-stamp by the workspace that
/// already held the label before it was reserved, so an old row keeps
/// working. Box-level doors never come through here.
pub fn refuse_reserved_claim(
    conn: &Connection,
    label: &str,
    project_id: &str,
) -> Result<(), CliResponse> {
    if !k2_core::skin::is_reserved_nested_label(label) {
        return Ok(());
    }
    let refuse = || CliResponse::bad_request(k2_core::skin::reserved_nested_label_error(label));
    if !k2_core::skin::is_grandfathered_reserved_label(label) {
        return Err(refuse());
    }
    match SubdomainWorkspace::owner_of(conn, label) {
        Ok(Some(owner)) if owner == project_id.trim() => Ok(()),
        Ok(_) => Err(refuse()),
        Err(e) => Err(CliResponse::internal_error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nested caller (an approved agent request replayed inside the
    /// owner's request) gives the outer request its own caller back, also
    /// after a panic; outside every call the slot is empty again.
    #[test]
    fn a_nested_caller_restores_the_outer_one() {
        let owner = Caller::Manager { actor: "owner".into() };
        assert_eq!(current(), Caller::Member, "nothing installed");
        with_caller(owner.clone(), || {
            assert_eq!(current(), owner);
            with_caller(Caller::Agent, || assert_eq!(current(), Caller::Agent));
            assert_eq!(current(), owner, "back to the owner after the nested call");
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| with_caller(Caller::Agent, || panic!("boom"))));
            assert!(panicked.is_err());
            assert_eq!(current(), owner, "back to the owner after a nested panic");
        });
        assert_eq!(current(), Caller::Member, "empty again outside");
    }

    fn make_project(tag: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/label-owner-{tag}-{id}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("ws-{tag}"), path],
        )
        .expect("insert project");
        id
    }

    fn owner(label: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        SubdomainWorkspace::owner_of(&conn, label).expect("owner_of")
    }

    fn drop_label(label: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        SubdomainWorkspace::unclaim(&conn, label).expect("unclaim");
    }

    fn label(tag: &str) -> String {
        format!("lo-{tag}-{}", uuid::Uuid::new_v4().simple())[..24].to_string()
    }

    #[test]
    fn current_defaults_to_member_and_scopes_manager() {
        assert_eq!(current(), Caller::Member);
        let inside = with_caller(Caller::Manager { actor: "owner-token".into() }, current);
        assert!(inside.is_manager());
        assert_eq!(current(), Caller::Member, "slot must clear after the request");
    }

    /// N6: agents and members hear which agent holds the label and its
    /// `k2 msg` address, never the workspace's id or folder path.
    #[test]
    fn other_workspace_is_refused_naming_the_agent_never_the_path_for_agents_and_members() {
        let a = make_project("a");
        let b = make_project("b");
        let l = label("n");
        claim_checked(&l, &a, None, false).expect("A claims a free label");
        let a_path = format!("/tmp/label-owner-a-{a}");
        for caller in [Caller::Agent, Caller::Member] {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let r = decide(&conn, &l, &b, false, &caller).expect_err("B must be refused");
            assert_eq!(r.status, "409 Conflict");
            assert_eq!(r.body["code"], CODE);
            assert_eq!(r.body["owner"]["agent"], "ws-a", "{caller:?}: {}", r.body);
            assert_eq!(r.body["owner"]["kind"], "agent", "{}", r.body);
            let addr = r.body["owner"]["address"].as_str().expect("an address to k2 msg");
            assert!(addr.starts_with("ws-a"), "the address is the agent's handle (::host when Connected): {addr}");
            assert!(r.body["owner"].get("path").is_none() && r.body["owner"].get("projectId").is_none(), "{}", r.body);
            let text = r.body.to_string();
            assert!(!text.contains(&a), "the owner's id must not leak: {text}");
            assert!(!text.contains(&a_path) && !text.contains("/tmp/"), "never a folder path: {text}");
        }
        assert_eq!(owner(&l).as_deref(), Some(a.as_str()));
        drop_label(&l);
    }

    #[test]
    fn manager_sees_owner_and_non_manager_cannot_transfer() {
        let a = make_project("a");
        let b = make_project("b");
        let l = label("m");
        claim_checked(&l, &a, None, false).expect("A claims");
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mgr = Caller::Manager { actor: "owner-token".into() };
        let r = decide(&conn, &l, &b, false, &mgr).expect_err("no transfer flag → 409");
        assert_eq!(r.status, "409 Conflict");
        assert_eq!(r.body["owner"]["projectId"].as_str(), Some(a.as_str()));
        assert_eq!(r.body["owner"]["name"].as_str(), Some("ws-a"));

        let r = decide(&conn, &l, &b, true, &Caller::Agent).expect_err("agent transfer");
        assert_eq!(r.status, "403 Forbidden");
        assert_eq!(r.body["code"], TRANSFER_CODE);
        drop(conn);
        assert_eq!(owner(&l).as_deref(), Some(a.as_str()));
        drop_label(&l);
    }

    #[test]
    fn same_workspace_reclaim_is_fine() {
        let a = make_project("a");
        let l = label("s");
        assert_eq!(claim_checked(&l, &a, Some("localhost:1"), false), Ok(Decision::Claim));
        assert_eq!(claim_checked(&l, &a, Some("localhost:2"), false), Ok(Decision::Claim));
        assert_eq!(owner(&l).as_deref(), Some(a.as_str()));
        drop_label(&l);
    }

    #[test]
    fn manager_transfer_moves_and_is_audited() {
        let _home = crate::test_support::TempHome::new();
        let a = make_project("a");
        let b = make_project("b");
        let l = label("t");
        claim_checked(&l, &a, None, false).expect("A claims");
        let d = with_caller(Caller::Manager { actor: "owner-token".into() }, || {
            claim_checked(&l, &b, Some("localhost:7"), true)
        })
        .expect("owner transfer");
        assert_eq!(d, Decision::Transfer { from: a.clone() });
        assert_eq!(owner(&l).as_deref(), Some(b.as_str()));

        let events = k2_core::auth_audit::tail(50).expect("audit tail");
        let hit = events
            .iter()
            .find(|e| {
                e["event"] == "label-transfer"
                    && e["outcome"].as_str().is_some_and(|o| o.contains(&l))
            })
            .unwrap_or_else(|| panic!("no label-transfer audit line for {l}: {events:?}"));
        let outcome = hit["outcome"].as_str().expect("outcome");
        assert!(outcome.contains(&format!("from={a}")), "{outcome}");
        assert!(outcome.contains(&format!("to={b}")), "{outcome}");
        assert_eq!(hit["user"], "owner-token");
        drop_label(&l);
    }
}
