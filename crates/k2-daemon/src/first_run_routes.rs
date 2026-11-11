//! `/cli/first-run/*`: the setup checklist (prd-setup-checklist-v1 C1;
//! the daemon–app contract is `setup-checklist-contract.md`).
//! `k2_core::first_run` holds the rows and the store.
//!
//! | Method | Path | Body | Answer |
//! |---|---|---|---|
//! | GET  | `/cli/first-run/status` | | `{enabled, hidden, finished, rows[{id, state, opens, note?}]}` |
//! | POST | `/cli/first-run/hide` | | the same body |
//! | POST | `/cli/first-run/show` | | the same body (hidden, finished and skips cleared) |
//! | POST | `/cli/first-run/skip` | `{"row": "<id>"}` | the same body |
//!
//! **Who.** A person only: the owner token or a Connect login at the
//! Member floor (`route_policy`), resolved to the person key from the
//! credential, never the body (`homes_routes::resolve_person`: the owner
//! token and the logins the owner linked are `owner`; a reserved-name
//! login is nobody). Agent passports are on the denylist
//! (`session_token`, `/cli/first-run/`), app passes and API keys fail
//! `validate_session`, and a Voice & Chat only login is refused by the
//! Member floor. The GET chain (an agent's cell socket) refuses as a belt.
//!
//! **Frames.** `first_run_changed` to that person's sockets on hide, show,
//! skip and finish.

use k2_core::first_run::{self, Action, Viewer};
use k2_core::notify_prefs::People;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::homes_routes::PersonRefusal;
use crate::notify_routes::Actor;
use crate::session_events::SessionEvent;

pub const STATUS_PATH: &str = "/cli/first-run/status";
pub const HIDE_PATH: &str = "/cli/first-run/hide";
pub const SHOW_PATH: &str = "/cli/first-run/show";
pub const SKIP_PATH: &str = "/cli/first-run/skip";

/// Body cap for the POSTs (`{"row": "<id>"}` at most).
pub const MAX_BODY: usize = 4 * 1024;

pub fn is_route(path: &str) -> bool {
    matches!(path, STATUS_PATH | HIDE_PATH | SHOW_PATH | SKIP_PATH)
}

pub fn is_post_only(path: &str) -> bool {
    matches!(path, HIDE_PATH | SHOW_PATH | SKIP_PATH)
}

fn error(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

/// 403 for anything that is not a person.
pub fn not_a_person() -> CliResponse {
    error(
        "403 Forbidden",
        "first_run_person_only",
        "the setup list belongs to a person: sign in with your own login (agents, app passes and API keys have none)",
    )
}

/// GET chain door (`cli::dispatch`): never served there.
pub fn refuse_outside_person_arm(path: &str, is_post: bool) -> Option<CliResponse> {
    if !is_route(path) {
        return None;
    }
    if is_post_only(path) && !is_post {
        return Some(CliResponse::method_not_allowed());
    }
    Some(not_a_person())
}

/// Tell `person`'s sockets (every socket when `None`) to re-read
/// `GET /cli/first-run/status`.
pub fn emit_changed(person: Option<String>) {
    let _ = crate::session_events::emit(SessionEvent::FirstRunChanged { person });
}

fn person_for(actor: &Actor) -> Result<String, CliResponse> {
    crate::homes_routes::resolve_person(actor).map_err(|r| match r {
        PersonRefusal::NoLogin => not_a_person(),
        PersonRefusal::Store(e) => error("500 Internal Server Error", "store", e),
        PersonRefusal::Reserved => error(
            "403 Forbidden",
            "first_run_person_only",
            "this login's name is one K2 uses for the server's owner and its agents, so it has no setup list; the owner can remove it in Settings → Admin Access and add the person under another name",
        ),
    })
}

fn viewer_for(actor: &Actor) -> Result<Viewer, CliResponse> {
    if actor.owner_token {
        return Ok(Viewer::OwnerToken);
    }
    match k2_core::connect_users::role_for_user(&actor.login) {
        // The Member floor already refused it; a belt for any other door.
        Some(k2_core::connect_users::Role::VoiceChat) => {
            Err(error("403 Forbidden", "role_required", "the setup list needs a Member, Admin or Owner login"))
        }
        Some(role) => Ok(Viewer::Login(role)),
        None => Err(not_a_person()),
    }
}

fn action_for(path: &str, body: &[u8]) -> Result<Option<Action>, CliResponse> {
    Ok(match path {
        STATUS_PATH => None,
        HIDE_PATH => Some(Action::Hide),
        SHOW_PATH => Some(Action::Show),
        SKIP_PATH => {
            let v: Value = serde_json::from_slice(body)
                .map_err(|e| error("400 Bad Request", "usage", format!("send {{\"row\": \"<id>\"}}: {e}")))?;
            let row = v
                .get("row")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| error("400 Bad Request", "usage", "send {\"row\": \"<id>\"}"))?;
            Some(Action::Skip(row.to_string()))
        }
        _ => return Err(CliResponse::not_found()),
    })
}

/// Handle one `/cli/first-run/*` request for `actor` (the dispatcher's
/// person arm already resolved the credential).
pub fn handle(is_post: bool, path: &str, body: &[u8], actor: &Actor) -> CliResponse {
    if !is_route(path) {
        return CliResponse::not_found();
    }
    if is_post_only(path) != is_post {
        return CliResponse::method_not_allowed();
    }
    let person = match person_for(actor) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let viewer = match viewer_for(actor) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let action = match action_for(path, body) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    let settings = k2_core::app_settings::load();
    let now = chrono::Utc::now().timestamp();
    let result = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let facts = match first_run::load_facts(&conn, &people, &settings, &person) {
            Ok(f) => f,
            Err(e) => return error("500 Internal Server Error", "store", e),
        };
        match &action {
            None => first_run::status_on(&conn, &people, &person, viewer, &facts, settings.setup_checklist, now),
            Some(a) => {
                first_run::apply_on(&conn, &people, &person, viewer, &facts, settings.setup_checklist, a, now)
            }
        }
    };
    let status = match result {
        Ok(s) => s,
        Err(first_run::Error::UnknownRow(row)) => {
            return error(
                "400 Bad Request",
                "unknown_row",
                format!("{row:?} is not a row on your setup list (k2 setup shows them)"),
            )
        }
        Err(first_run::Error::Store(e)) => return error("500 Internal Server Error", "store", e),
    };
    if action.is_some() || status.newly_finished {
        emit_changed(Some(person));
    }
    match serde_json::to_string(&status) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => error("500 Internal Server Error", "serialize", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_shape_and_doors() {
        for p in [STATUS_PATH, HIDE_PATH, SHOW_PATH, SKIP_PATH] {
            assert!(is_route(p), "{p}");
        }
        assert!(!is_route("/cli/first-run/statusx") && !is_route("/cli/first-run/"));
        for p in [HIDE_PATH, SHOW_PATH, SKIP_PATH] {
            assert_eq!(refuse_outside_person_arm(p, false).expect("door").status, "405 Method Not Allowed", "{p}");
            let r = refuse_outside_person_arm(p, true).expect("door");
            assert_eq!(r.status, "403 Forbidden", "{p}");
            assert!(r.body.contains("first_run_person_only"), "{}", r.body);
        }
        let r = refuse_outside_person_arm(STATUS_PATH, false).expect("door");
        assert_eq!(r.status, "403 Forbidden");
        assert!(refuse_outside_person_arm("/cli/thread", false).is_none());
        // The handler's own guards: a GET to a POST route, a POST to status.
        let h = handle(false, HIDE_PATH, b"", &Actor::owner_token());
        assert_eq!(h.status, "405 Method Not Allowed");
        let h = handle(true, STATUS_PATH, b"{}", &Actor::owner_token());
        assert_eq!(h.status, "405 Method Not Allowed");
    }

    #[test]
    fn a_login_that_is_not_a_person_is_refused() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::test_add_legacy_reserved_user("owner", "password123").expect("user named owner");
        let r = handle(false, STATUS_PATH, b"", &Actor::login("ghost"));
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("first_run_person_only"), "{}", r.body);
        let r = handle(false, STATUS_PATH, b"", &Actor::login("owner"));
        assert_eq!(r.status, "403 Forbidden", "a reserved name is nobody: {}", r.body);
    }

    #[test]
    fn skip_needs_a_row() {
        for body in [&b""[..], b"{}", br#"{"row": "  "}"#, br#"{"row": 3}"#] {
            let r = action_for(SKIP_PATH, body).expect_err("usage");
            assert_eq!(r.status, "400 Bad Request");
        }
        let ok = |path: &str, body: &[u8]| match action_for(path, body) {
            Ok(a) => a,
            Err(r) => panic!("{path}: {} {}", r.status, r.body),
        };
        assert_eq!(ok(SKIP_PATH, br#"{"row":"phone"}"#), Some(Action::Skip("phone".into())));
        assert_eq!(ok(STATUS_PATH, b""), None);
    }

    /// End to end through the handler on a temp home: the owner token's
    /// list, a skip, the frame, and a Member's narrower list.
    #[test]
    fn the_owner_token_and_a_member_get_their_own_lists() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let mut rx = crate::session_events::sender().subscribe();
        let r = handle(false, STATUS_PATH, b"", &Actor::owner_token());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["enabled"], json!(false), "the switch is off by default");
        let ids: Vec<&str> = v["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
        assert_eq!(ids, vec!["llm_sign_in", "first_agent", "tunnel", "phone", "own_sign_in"]);

        let r = handle(true, SKIP_PATH, br#"{"row":"tunnel"}"#, &Actor::owner_token());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["rows"][2], json!({ "id": "tunnel", "state": "skipped", "opens": "tunnel" }));
        let mut saw = false;
        while let Ok(ev) = rx.try_recv() {
            // Other tests in this process may send their own frames.
            if let SessionEvent::FirstRunChanged { person } = ev {
                saw |= person.as_deref() == Some("owner");
            }
        }
        assert!(saw, "skip sends first_run_changed to the owner");

        // show clears the skip: the row is to do again.
        let r = handle(true, SHOW_PATH, b"", &Actor::owner_token());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["rows"][2], json!({ "id": "tunnel", "state": "todo", "opens": "tunnel" }));
        assert_eq!(v["finished"], json!(false));

        let r = handle(true, SKIP_PATH, br#"{"row":"nope"}"#, &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("unknown_row"), "{}", r.body);

        k2_core::connect_users::add_user("anna", "password123").expect("add");
        let r = handle(false, STATUS_PATH, b"", &Actor::login("anna"));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        let ids: Vec<&str> = v["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
        assert_eq!(ids, vec!["first_agent", "phone"], "a Member's rows");
        let r = handle(true, SKIP_PATH, br#"{"row":"tunnel"}"#, &Actor::login("anna"));
        assert_eq!(r.status, "400 Bad Request", "a Member can't skip a row it doesn't see");

        let r = handle(true, HIDE_PATH, b"", &Actor::login("anna"));
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["hidden"], json!(true));
        let r = handle(false, STATUS_PATH, b"", &Actor::owner_token());
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["hidden"], json!(false), "anna's hide is anna's");
    }
}
