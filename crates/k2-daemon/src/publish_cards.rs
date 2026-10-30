//! K2's own cards for `k2 publish` visibility
//! (prd-publish-visibility-and-dev-mode-v1 PV-7, PV-10, PV-11; vs-live PA-17).
//!
//! - **`publish.public`**: an agent asks to make its site public. A person
//!   allows it with one click, through the route's own entry.
//! - **`publish.pin`**: an agent asks for a PIN. The card opens K2's own PIN form,
//!   and the PIN goes straight to the route as that person. It never passes
//!   through the card, the Thread or the agent. Allow only closes the card once
//!   the PIN is set, so it decides nothing by itself.
//! - **The upgrade card**: one Thread card listing the sites that were public
//!   before 0.46.7, one click each.
//!
//! The two permission cards are standard ones K2 presents itself (like
//! `zen.sign`), whether or not the owner turned on the Beta.

use k2_core::overlay;
use k2_core::permission::catalog::Floor;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use k2_core::site_visibility as sv;
use serde_json::{json, Map, Value as J};

use crate::cli_response::CliResponse;
use crate::permission::actions::Ctx;
use crate::permission::{answer, limits, present, store};

/// The catalog actions (`permission/actions/publish.rs`).
pub(crate) const PUBLIC: &str = "publish.public";
pub(crate) const PIN: &str = "publish.pin";

/// The session K2's own requests carry (never batched with an agent's asks).
pub(crate) const K2_SESSION: &str = "k2:publish";

/// The refusal codes an agent's attempt answers with, carrying its card.
pub(crate) const PUBLIC_NEEDS_PERSON: &str = "public_needs_person";
pub(crate) const PIN_NEEDS_PERSON: &str = "pin_needs_person";

/// An agent's attempt: a 403 with the card it raised (the CLI's exit 2).
fn held(code: &str, label: &str, message: String, card: J) -> CliResponse {
    let mut body = json!({
        "status": "held",
        "code": code,
        "label": label,
        "message": message,
    });
    if let (Some(o), Some(c)) = (body.as_object_mut(), card.as_object()) {
        for (k, v) in c {
            o.insert(k.clone(), v.clone());
        }
    }
    CliResponse { status: "403 Forbidden", content_type: "application/json", body: body.to_string() }
}

/// `publish.public` for `label`, asked by workspace `ws`.
pub(crate) fn present_public(ws: &str, label: &str, reason: &str) -> CliResponse {
    let mut params = Map::new();
    params.insert("label".to_string(), J::String(label.to_string()));
    params.insert("reason".to_string(), J::String(reason.to_string()));
    let handle = present::workspace_handle(ws);
    let text = format!("{handle} asks to make {label} public, so anyone with its address can open it. Its reason: {reason}");
    let card = present(ws, PUBLIC, params, reason, &text, &handle, label);
    held(
        PUBLIC_NEEDS_PERSON,
        label,
        format!("Making {label} public is a person's choice: K2 put a card on your Thread. Wait for its answer."),
        card,
    )
}

/// `publish.pin` for `label`, asked by workspace `ws`.
pub(crate) fn present_pin(ws: &str, label: &str) -> CliResponse {
    let mut params = Map::new();
    params.insert("label".to_string(), J::String(label.to_string()));
    let handle = present::workspace_handle(ws);
    let text = format!("{handle} asks you to put {label} behind a PIN. You type the PIN into K2's own form; {handle} never sees it.");
    let card = present(ws, PIN, params, "I need this site behind a PIN.", &text, &handle, label);
    held(
        PIN_NEEDS_PERSON,
        label,
        format!("An agent can't set a PIN, because it would then know it: K2 put a card on your Thread, and the person types the PIN into K2's own form. Wait for its answer."),
        card,
    )
}

/// One request with its card on `ws`'s Thread (`zen_sign_cards` is the model).
/// `{card, request, addr}`, `{card, request, waiting: true}` when the same one
/// is open, or `{card: null, note: true}` with no Thread (an owners-only note).
fn present(ws: &str, action: &str, params: Map<String, J>, why: &str, text: &str, handle: &str, label: &str) -> J {
    let ph = params_hash(&params);
    if let Ok(Some(open)) = store::open_same(ws, action, &ph) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true });
    }
    let Some(reg) = present::find_action(action) else {
        k2_core::log_debug!("[publish-cards] {action} is not registered");
        return json!({ "card": null, "asked": false });
    };
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(ws) else {
        return inbox_note(ws, handle, label, text);
    };
    let at = overlay::now_secs();
    let ctx = Ctx { workspace_id: ws.to_string(), workspace_handle: handle.to_string(), params: params.clone() };
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(handle.to_string()),
        card_id: None,
        action: action.to_string(),
        params: J::Object(params),
        params_hash: ph.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why: why.to_string(),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: None,
        batch_id: None,
        ask_number: limits::ask_number(ws, action, &ph, at),
        asked_for: None,
        created_at: at,
        expires_at: at + reg.spec.expiry.default_secs,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    if let Err(e) = store::insert(&req) {
        k2_core::log_debug!("[publish-cards] request for {label}: {e}");
        return json!({ "card": null, "asked": false });
    }
    let reqs = std::slice::from_ref(&req);
    let card_action = present::card_action(reqs, false, present::single_dedupe(&req));
    let options = [k2_core::permission::label::ALLOW, k2_core::permission::label::DENY];
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        ws,
        text,
        &options,
        card_action,
        false,
        req.expires_at,
        crate::overlay_routes::CardAudience::Logins(present::eligible_logins(Floor::Admin)),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            if let Err(e) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[publish-cards] withdraw {}: {e}", req.id);
            }
            k2_core::log_debug!("[publish-cards] card for {label}: {other:?}");
            return match other {
                Ok(crate::overlay_routes::Presented::NoThread) => inbox_note(ws, handle, label, text),
                _ => json!({ "card": null, "asked": false }),
            };
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[publish-cards] set card {card_id}: {e}");
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    json!({ "card": card_id, "request": req.display_id(), "addr": addr })
}

/// No Thread: one owners-only inbox note (none while the same is open).
fn inbox_note(ws: &str, agent: &str, label: &str, text: &str) -> J {
    let title = format!("{agent} asks about who may open {label}");
    let open = k2_core::feedback::list_for_project(ws, &k2_core::feedback::ListFilter::Open).unwrap_or_default();
    if !open.iter().any(|i| i.title == title) {
        let body = format!("{text}\n\nTo do it, from a terminal outside K2: k2 publish visibility {label} <pin|public>. To say no, do nothing.");
        if let Err(e) = crate::feedback_routes::file_owners_only_notice(ws, agent, &title, &body) {
            k2_core::log_debug!("[publish-cards] inbox note for {label}: {e}");
        }
    }
    json!({ "card": null, "note": true })
}

// ── the upgrade card (PV-11) ────────────────────────────────────────────

/// The card action ([`crate::action_cards::run`]).
pub(crate) const UPGRADE_ACTION: &str = "publish.upgrade-sites";
const MAKE_PRIVATE: &str = "Make private";
const ADD_PIN: &str = "Add a PIN";
const KEEP_PUBLIC: &str = "Keep public";
const KEEP_THE_REST: &str = "Keep them all public";
/// A card takes at most 8 options: 2 sites, one click each, then a new card for
/// the next ones, and all of them in one click.
const SITES_PER_CARD: usize = 2;

/// The sites still public from before 0.46.7 that nobody has decided about.
fn undecided(conn: &rusqlite::Connection) -> Result<Vec<String>, String> {
    Ok(sv::grandfathered_public(conn)?.into_iter().map(|v| v.label).collect())
}

fn option_words(site: &str, what: &str) -> String {
    format!("{site}: {what}")
}

fn upgrade_card(sites: &[String]) -> (String, Vec<String>) {
    let list = sites.join(", ");
    let n = sites.len();
    let prompt = format!(
        "K2 now lets you choose who may open a published site: private, behind a PIN, or public. {} from before this update stay{} public, so nothing stopped working: {list}. New sites start private.",
        if n == 1 { "1 site".to_string() } else { format!("{n} sites") },
        if n == 1 { "s" } else { "" },
    );
    let mut options = Vec::new();
    for s in sites.iter().take(SITES_PER_CARD) {
        options.push(option_words(s, MAKE_PRIVATE));
        options.push(option_words(s, ADD_PIN));
        options.push(option_words(s, KEEP_PUBLIC));
    }
    if n > SITES_PER_CARD {
        options.push(KEEP_THE_REST.to_string());
    }
    (prompt, options)
}

/// Post (or replace) the upgrade card. `true` when it is on a Thread, or when
/// there is nothing to say. Where no workspace has a Thread it stays owed.
pub(crate) fn present_upgrade_card() -> bool {
    let sites = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match undecided(&conn) {
            Ok(s) => s,
            Err(e) => {
                k2_core::log_debug!("[publish-cards] upgrade sites: {e}");
                return false;
            }
        }
    };
    if sites.is_empty() {
        return true;
    }
    let (prompt, options) = upgrade_card(&sites);
    let opts: Vec<&str> = options.iter().map(String::as_str).collect();
    let action = overlay::CardAction {
        id: UPGRADE_ACTION.to_string(),
        params: json!({}),
        dedupe: "publish-upgrade".to_string(),
        result: None,
        view: None,
    };
    let projects = match k2_core::projects_ops::projects_list() {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("[publish-cards] no workspaces to post on: {e}");
            return false;
        }
    };
    for p in projects {
        match crate::overlay_routes::present_action_card(
            None,
            &p.id,
            &prompt,
            &opts,
            action.clone(),
            true,
            overlay::now_secs() + overlay::DEFAULT_CARD_EXPIRY_SECS,
            crate::overlay_routes::CardAudience::AgentPost,
        ) {
            Ok(crate::overlay_routes::Presented::Card { .. }) | Ok(crate::overlay_routes::Presented::Waiting { .. }) => return true,
            Ok(crate::overlay_routes::Presented::NoThread) => {}
            Err(e) => k2_core::log_debug!("[publish-cards] upgrade card on {}: {e}", p.id),
        }
    }
    false
}

/// A person tapped the upgrade card. `by` is `owner` or a login. The answer
/// route lets only the owner answer an action card; this acts as that person
/// through the route's own entry. The next undecided sites get a new card.
pub(crate) fn run_upgrade_card(answer: &str, by: &str) -> Result<J, String> {
    let who = if by == "owner" {
        crate::compute_routes::Who::Owner
    } else {
        crate::compute_routes::Who::Login {
            username: by.to_string(),
            role: k2_core::connect_users::role_for_user(by).unwrap_or(k2_core::connect_users::Role::Member),
        }
    };
    let db = k2_core::db::shared();
    let keep_public = |label: &str| -> Result<(), String> {
        let conn = db.lock();
        sv::set_public(&conn, label, "kept public by the owner after 0.46.7", &who.label()).map(|_| ())
    };
    let result = if answer == KEEP_THE_REST {
        let sites = {
            let conn = db.lock();
            undecided(&conn)?
        };
        for s in &sites {
            keep_public(s)?;
        }
        json!({ "message": format!("Kept {} public. Change any time: k2 publish visibility <name>.", sites.join(", ")) })
    } else {
        let (site, what) = answer.split_once(": ").ok_or_else(|| "K2 doesn't know that answer".to_string())?;
        match what {
            MAKE_PRIVATE => {
                let mut params = std::collections::HashMap::new();
                params.insert("label".to_string(), site.to_string());
                params.insert("mode".to_string(), "private".to_string());
                params.insert("yes".to_string(), "1".to_string());
                let r = crate::publish_site_routes::handle_set(&params, &who);
                if !r.status.starts_with("200") {
                    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
                    return Err(v["error"].as_str().unwrap_or("K2 couldn't change it").to_string());
                }
                json!({ "message": format!("{site} is private now. Open it from K2, or with k2 publish open {site}.") })
            }
            ADD_PIN => json!({
                // The PIN form is K2's own and the client's: the PIN goes
                // straight to the route, never through this card.
                "openPinForm": site,
                "message": format!("Set {site}'s PIN in K2's form. From a terminal outside K2: k2 publish visibility {site} pin"),
            }),
            KEEP_PUBLIC => {
                keep_public(site)?;
                json!({ "message": format!("{site} stays public. Change any time: k2 publish visibility {site}.") })
            }
            _ => return Err("K2 doesn't know that answer".to_string()),
        }
    };
    // The rest of the list, on a new card. Not on this thread: the answer route
    // is still finishing this one.
    #[cfg(not(test))]
    if let Err(e) = std::thread::Builder::new().name("publish-upgrade-card".into()).spawn(|| {
        present_upgrade_card();
    }) {
        k2_core::log_debug!("[publish-cards] next upgrade card: {e}");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("site-{i}")).collect()
    }

    /// PV-11: one click per site, within a card's limit, and a long list is one
    /// click from done.
    #[test]
    fn the_upgrade_card_has_one_click_per_site_and_fits_a_card() {
        let (prompt, options) = upgrade_card(&names(1));
        assert!(prompt.contains("1 site from before this update stays public"), "{prompt}");
        assert_eq!(options, vec!["site-0: Make private", "site-0: Add a PIN", "site-0: Keep public"]);

        let (prompt, options) = upgrade_card(&names(2));
        assert!(prompt.contains("2 sites from before this update stay public"), "{prompt}");
        assert_eq!(options.len(), 6);

        let (prompt, options) = upgrade_card(&names(12));
        assert!(prompt.contains("site-11"), "the card names every site: {prompt}");
        assert_eq!(options.len(), 7, "{options:?}");
        assert_eq!(options.last().map(String::as_str), Some(KEEP_THE_REST));
        overlay::options::check_ask_caps(&prompt, &options).expect("a card must take these");
    }

    /// PV-11: each answer does what it says, as the person, and a site someone
    /// decided about leaves the list. "Add a PIN" changes nothing by itself: the
    /// person's form does, so the answer carries no PIN either.
    #[test]
    fn the_upgrade_card_answers_decide_one_site_or_the_rest() {
        let _home = crate::test_support::TempHome::new();
        let _ = k2_core::db::init_for_tests();
        std::fs::write(_home.path().join(".k2/tunnel.json"), json!({ "token": "t", "subdomain": "rosson", "e2e": true }).to_string()).expect("tunnel.json");
        let id = uuid::Uuid::new_v4().simple().to_string();
        let ws = format!("ws-{id}");
        let site = |n: &str| format!("{n}-{}", &id[..8]);
        let (keep, close, pin, rest) = (site("keep"), site("close"), site("pin"), site("rest"));
        let db = k2_core::db::shared();
        {
            let conn = db.lock();
            conn.execute("INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)", rusqlite::params![ws, format!("/tmp/{ws}"), ws]).expect("workspace");
            for label in [&keep, &close, &pin, &rest] {
                k2_core::published_services::insert(&conn, &ws, label, "true", &format!("/tmp/{ws}"), 41950, "tunnel", "stopped", "cmd", "").expect("service");
                sv::set_public(&conn, label, sv::REASON_BEFORE, sv::SET_BY_UPGRADE).expect("public");
            }
        }
        let mode = |l: &str| {
            let conn = db.lock();
            sv::effective(&conn, l).expect("row").mode
        };
        let left = || {
            let conn = db.lock();
            undecided(&conn).expect("list")
        };
        assert_eq!(left().len(), 4);

        let r = run_upgrade_card(&format!("{keep}: Keep public"), "owner").expect("keep");
        assert!(r["message"].as_str().expect("message").contains("stays public"), "{r}");
        assert_eq!(mode(&keep), sv::Mode::Public);
        assert!(!left().contains(&keep), "decided");

        let r = run_upgrade_card(&format!("{close}: Make private"), "owner").expect("private");
        assert!(r["message"].as_str().expect("message").contains("private now"), "{r}");
        assert_eq!(mode(&close), sv::Mode::Private);

        let r = run_upgrade_card(&format!("{pin}: Add a PIN"), "owner").expect("pin");
        assert_eq!(r["openPinForm"], pin.as_str(), "{r}");
        assert_eq!(mode(&pin), sv::Mode::Public, "only the person's form sets a PIN");
        assert!(left().contains(&pin), "not decided yet");

        let r = run_upgrade_card(KEEP_THE_REST, "owner").expect("rest");
        assert!(r["message"].as_str().expect("message").contains(&rest), "{r}");
        assert_eq!(mode(&rest), sv::Mode::Public);
        assert!(left().is_empty(), "{:?}", left());

        assert!(run_upgrade_card("nonsense", "owner").is_err());
        assert!(run_upgrade_card(&format!("{keep}: Burn it down"), "owner").is_err());
    }

    /// The card waits for the pass over the routing map, and comes once.
    #[test]
    fn the_upgrade_card_is_due_once_the_map_pass_has_settled_and_only_once() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        conn.execute_batch("CREATE TABLE site_visibility_meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);").expect("meta");
        conn.execute("INSERT INTO site_visibility_meta (key, value) VALUES ('grandfather', 'pending')", []).expect("pending");
        assert!(!sv::upgrade_card_due(&conn).expect("due"), "the pass still decides which sites");
        conn.execute("UPDATE site_visibility_meta SET value = 'done' WHERE key = 'grandfather'", []).expect("done");
        assert!(sv::upgrade_card_due(&conn).expect("due"));
        sv::mark_upgrade_card_done(&conn).expect("mark");
        assert!(!sv::upgrade_card_due(&conn).expect("due"), "once");
    }

    /// PV-7: the agent's card and its answer never carry a PIN, and neither
    /// does anything the agent is told.
    #[test]
    fn the_pin_card_words_tell_the_person_the_agent_never_sees_it() {
        assert_eq!(PIN, "publish.pin");
        assert_eq!(PUBLIC, "publish.public");
        let r = held(PIN_NEEDS_PERSON, "docs", "m".into(), json!({ "card": "c1", "request": "pr_1" }));
        assert_eq!(r.status, "403 Forbidden");
        let v: J = serde_json::from_str(&r.body).expect("json");
        assert_eq!((v["status"].as_str(), v["code"].as_str(), v["card"].as_str()), (Some("held"), Some(PIN_NEEDS_PERSON), Some("c1")));
    }
}
