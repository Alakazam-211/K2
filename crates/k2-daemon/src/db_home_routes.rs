//! The home database switch, daemon side (`prd-k2-db-rename-v1`).
//!
//! The switch itself runs in `k2_core::db::home_switch` before the database
//! opens. This module is what a person sees of it:
//!
//! - `GET /cli/db-home`: what this boot did (`homeDb`), plus a refusal
//!   note, a pending choice and the hold, as on disk now (`k2 db home`);
//! - `POST /cli/db-home/keep?keep=k2.db|k2so.db`: the owner's way out of a
//!   refusal (DB-A6, `k2 db home --keep`). Owner only: the owner token or an
//!   Owner login. It writes `db-switch-choice.json`; the next start applies
//!   it (the other file goes to `db-quarantine/`, never deleted);
//! - the boot card: when the switch refuses because both files hold
//!   workspaces, one Thread card asks the owner which to keep (a person's
//!   tap runs [`run_card`]); with no Thread to put it on, an owner ticket
//!   with the two commands.

use std::path::{Path, PathBuf};

use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use k2_core::db::home_switch;

/// The card's action id ([`crate::action_cards::run`]).
pub const CARD_ACTION: &str = "db.home.keep";
/// The card's third option.
pub const NOT_NOW: &str = "Not now";

fn k2_dir() -> PathBuf {
    k2_core::paths::k2_home()
}

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// `true` for `/cli/db-home` and `/cli/db-home/keep`.
pub fn is_route(path: &str) -> bool {
    path == "/cli/db-home" || path == "/cli/db-home/keep"
}

/// The two routes. `who` is the classified caller.
pub fn handle(path: &str, is_post: bool, who: &Who, params: &std::collections::HashMap<String, String>) -> CliResponse {
    handle_at(&k2_dir(), path, is_post, who, params)
}

pub(crate) fn handle_at(
    dir: &Path,
    path: &str,
    is_post: bool,
    who: &Who,
    params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    match path {
        "/cli/db-home" => {
            if !who.is_admin() {
                return err_json(
                    "403 Forbidden",
                    "forbidden",
                    "Only the owner or an Admin can read where K2 keeps its database. Ask your human.".into(),
                );
            }
            CliResponse::ok_json(json!({ "ok": true, "homeDb": home_switch::home_json(dir) }).to_string())
        }
        "/cli/db-home/keep" => {
            if !is_post {
                return CliResponse::method_not_allowed();
            }
            if !who.is_owner() {
                return err_json(
                    "403 Forbidden",
                    "owner_only",
                    "Only the owner can choose which database K2 keeps. Ask your human.".into(),
                );
            }
            let keep = params.get("keep").map(String::as_str).unwrap_or("");
            match home_switch::write_keep_choice(dir, keep, &who.label()) {
                Ok(v) => CliResponse::ok_json(v.to_string()),
                Err(e) if e.contains("--keep must be") => err_json("400 Bad Request", "usage", e),
                Err(e) if e.contains("nothing to choose") => err_json("409 Conflict", "nothing_to_choose", e),
                Err(e) => err_json("500 Internal Server Error", "error", e),
            }
        }
        _ => CliResponse::not_found(),
    }
}

/// The card's two keep options, with each file's workspace count.
fn keep_label(file: &str, workspaces: i64) -> String {
    format!("Keep {file} ({workspaces} workspaces)")
}

/// A person tapped the refusal card (`by`: `owner` or a login). Only the
/// owner may choose; "Not now" changes nothing.
pub fn run_card(params: &J, answer: &str, by: &str) -> Result<J, String> {
    run_card_at(&k2_dir(), params, answer, by)
}

pub(crate) fn run_card_at(dir: &Path, params: &J, answer: &str, by: &str) -> Result<J, String> {
    if answer == NOT_NOW {
        return Ok(json!({ "message": "Nothing changed. K2 keeps using the same file; k2 db home --keep chooses later." }));
    }
    let keep = [home_switch::NEW_NAME, home_switch::OLD_NAME]
        .into_iter()
        .find(|f| params["labels"][f].as_str() == Some(answer))
        .ok_or_else(|| format!("'{answer}' isn't an answer to this card"))?;
    let is_owner = by == "owner"
        || k2_core::connect_users::role_for_user(by) == Some(k2_core::connect_users::Role::Owner);
    if !is_owner {
        return Err("Only the owner can choose which database K2 keeps.".into());
    }
    let v = home_switch::write_keep_choice(dir, keep, by)?;
    Ok(json!({ "keep": keep, "message": v["message"] }))
}

/// After the database opened: when this boot refused because both files
/// hold workspaces, ask the owner once (a card on a Thread, else a ticket).
pub fn after_boot(report: &home_switch::SwitchReport) {
    let home_switch::Outcome::Blocked { k2_projects, k2so_projects } = report.outcome else { return };
    let dir = k2_dir();
    if !home_switch::blocked_notice_due(&dir) {
        return;
    }
    let body = home_switch::blocked_message(&dir, &report.path, k2_projects, k2so_projects);
    if present_card(&body, k2_projects, k2so_projects) || file_ticket(&body) {
        home_switch::mark_blocked_notice_filed(&dir);
    }
}

fn present_card(prompt: &str, k2: i64, k2so: i64) -> bool {
    let labels = json!({
        home_switch::OLD_NAME: keep_label(home_switch::OLD_NAME, k2so),
        home_switch::NEW_NAME: keep_label(home_switch::NEW_NAME, k2),
    });
    let a = labels[home_switch::OLD_NAME].as_str().unwrap_or_default().to_string();
    let b = labels[home_switch::NEW_NAME].as_str().unwrap_or_default().to_string();
    let options = [a.as_str(), b.as_str(), NOT_NOW];
    let action = k2_core::overlay::CardAction {
        id: CARD_ACTION.to_string(),
        params: json!({ "labels": labels }),
        dedupe: "home-db".to_string(),
        result: None,
        view: None,
    };
    let projects = match k2_core::projects_ops::projects_list() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[db] home-db card: no workspaces to post on: {e}");
            return false;
        }
    };
    for p in projects {
        match crate::overlay_routes::present_action_card(
            None,
            &p.id,
            prompt,
            &options,
            action.clone(),
            true,
            k2_core::overlay::now_secs() + k2_core::overlay::DEFAULT_CARD_EXPIRY_SECS,
            crate::overlay_routes::CardAudience::AgentPost,
        ) {
            Ok(crate::overlay_routes::Presented::Card { id, .. }) | Ok(crate::overlay_routes::Presented::Waiting { id }) => {
                k2_core::log_debug!("[db] home-db card {id} on {}", p.id);
                return true;
            }
            Ok(crate::overlay_routes::Presented::NoThread) => {}
            Err(e) => k2_core::log_debug!("[db] home-db card on {}: {e}", p.id),
        }
    }
    false
}

fn file_ticket(body: &str) -> bool {
    let text = format!(
        "{body}\n\nThere is no Thread to put a choice card on. From a terminal outside K2 (owner):\n\n\
         ```\nk2 db home --keep k2so.db\nk2 db home --keep k2.db\n```\n\nThen restart K2."
    );
    match crate::feedback_routes::file_owners_only_notice(
        k2_core::db::AUDIT_SENTINEL_IDS[0],
        "K2",
        "K2 did not rename its database: two copies hold workspaces",
        &text,
    ) {
        Ok(_) => true,
        Err(e) => {
            k2_core::log_debug!("[db] owner notice for the blocked rename: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-db-home-routes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
        ));
        std::fs::create_dir_all(&d).expect("scratch");
        d
    }

    /// Two real SQLite files that each hold one workspace row.
    fn two_live(dir: &Path) {
        for f in [home_switch::NEW_NAME, home_switch::OLD_NAME] {
            let c = rusqlite::Connection::open(dir.join(f)).expect("open");
            c.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY); INSERT INTO projects VALUES ('p1');")
                .expect("seed");
        }
    }

    fn params(keep: &str) -> std::collections::HashMap<String, String> {
        [("keep".to_string(), keep.to_string())].into_iter().collect()
    }

    /// `k2 db home --keep`: owner only (an agent passport, a shell tab, a
    /// Member or Admin login are refused and nothing is written); the
    /// owner's choice lands on disk; POST only.
    #[test]
    fn keep_route_is_owner_only_and_post_only() {
        let dir = scratch();
        two_live(&dir);
        let refused = [
            Who::Agent { project_id: "p".into(), session_id: "s".into() },
            Who::Shell { project_id: "p".into(), session_id: "s".into() },
            Who::Login { username: "m".into(), role: k2_core::connect_users::Role::Member },
            Who::Login { username: "a".into(), role: k2_core::connect_users::Role::Admin },
        ];
        for who in &refused {
            let r = handle_at(&dir, "/cli/db-home/keep", true, who, &params("k2.db"));
            assert_eq!(r.status, "403 Forbidden", "{who:?}: {}", r.body);
            assert!(!dir.join(home_switch::CHOICE_FILE).exists(), "{who:?} wrote a choice");
        }
        let r = handle_at(&dir, "/cli/db-home/keep", false, &Who::Owner, &params("k2.db"));
        assert_eq!(r.status, "405 Method Not Allowed");
        let r = handle_at(&dir, "/cli/db-home/keep", true, &Who::Owner, &params("nope.db"));
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let r = handle_at(&dir, "/cli/db-home/keep", true, &Who::Owner, &params("k2so.db"));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: J = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["keep"], "k2so.db");
        assert_eq!(v["moveAside"], "k2.db");
        let c: J = serde_json::from_slice(&std::fs::read(dir.join(home_switch::CHOICE_FILE)).expect("choice")).expect("json");
        assert_eq!(c["keep"], "k2so.db");
        assert_eq!(c["by"], "owner");
    }

    /// With only one file there is nothing to choose: 409.
    #[test]
    fn keep_route_conflicts_without_two_files() {
        let dir = scratch();
        let c = rusqlite::Connection::open(dir.join(home_switch::OLD_NAME)).expect("open");
        c.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY);").expect("seed");
        let r = handle_at(&dir, "/cli/db-home/keep", true, &Who::Owner, &params("k2.db"));
        assert_eq!(r.status, "409 Conflict", "{}", r.body);
    }

    /// The status read: Admin and up; an agent is refused.
    #[test]
    fn status_route_is_admin_and_up() {
        let dir = scratch();
        let r = handle_at(&dir, "/cli/db-home", false, &Who::Agent { project_id: "p".into(), session_id: "s".into() }, &Default::default());
        assert_eq!(r.status, "403 Forbidden");
        let r = handle_at(&dir, "/cli/db-home", false, &Who::Owner, &Default::default());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: J = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["homeDb"]["held"], false);
    }

    /// The card: an owner's tap writes the choice for the file the label
    /// names; a login that isn't the owner, and "Not now", change nothing.
    #[test]
    fn card_answers() {
        let dir = scratch();
        two_live(&dir);
        let params = json!({ "labels": {
            "k2so.db": keep_label("k2so.db", 1),
            "k2.db": keep_label("k2.db", 1),
        }});
        let v = run_card_at(&dir, &params, NOT_NOW, "owner").expect("not now");
        assert!(v["message"].as_str().expect("message").contains("Nothing changed"));
        assert!(!dir.join(home_switch::CHOICE_FILE).exists());
        let e = run_card_at(&dir, &params, &keep_label("k2.db", 1), "nobody-here").expect_err("not the owner");
        assert!(e.contains("Only the owner"), "{e}");
        assert!(!dir.join(home_switch::CHOICE_FILE).exists());
        let e = run_card_at(&dir, &params, "Keep both", "owner").expect_err("not an option");
        assert!(e.contains("isn't an answer"), "{e}");
        let v = run_card_at(&dir, &params, &keep_label("k2.db", 1), "owner").expect("owner keeps k2.db");
        assert_eq!(v["keep"], "k2.db");
        assert!(dir.join(home_switch::CHOICE_FILE).exists());
    }
}
