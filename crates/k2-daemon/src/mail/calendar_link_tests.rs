//! Calendar S1 (prd-google-calendar-link-v1 + vs-live-2) against the shared
//! test DB: the stored link type caps the grant (CV-2), existing links keep
//! their access, the per-link calendar opt-ins (owner only), per-workspace
//! description privacy (a person's choice), the access-log rows for agent
//! reads and governance changes, and the route wiring (CV-17). Provider
//! calls go to a scripted fake provider; nothing reaches Google or
//! Microsoft.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::mail::access::{Level, Source};
use crate::mail::calendar::{resolve_target, CalTarget};
use crate::mail::calendar_provider::{
    CalErr, CalendarProvider, Changes, DraftEvent, DraftPatch, LinkedCalInbox, StateErr,
    StoredEvent, ViewOpts,
};
use crate::mail::oauth::{scope_for, CalAccess, Features, OauthProvider};
use crate::session_token::HookPrincipal;

fn insert_project(tag: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, format!("cals1-{tag}-{}", &id[..8]), format!("/tmp/k2-cals1-{tag}-{}", &id[..8])],
    )
    .expect("insert project");
    id
}

fn addr(tag: &str) -> String {
    format!("{tag}-{}@cals1.example.com", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

/// An OAuth linked row stored as `features` with `granted` scopes.
fn seed(owner: &str, address: &str, provider: &str, features: &str, granted: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let kind = if provider == "microsoft" { "graph" } else { "imap" };
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, kind, host, port, \
         username, created_at, primary_level, auth_kind, provider, features, granted_scopes) \
         VALUES (?1, ?2, ?3, ?4, 'h.example', 443, ?3, 100, 'draft', 'oauth', ?5, ?6, ?7)",
        rusqlite::params![id, owner, address, kind, provider, features, granted],
    )
    .expect("seed link");
    id
}

fn cleanup(projects: &[&str]) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for p in projects {
        let _ = conn.execute("DELETE FROM mail_inbox_grants WHERE project_id = ?1", [p]);
        let _ = conn.execute("DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1", [p]);
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", [p]);
    }
}

fn body_of(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("non-JSON {e}: {}", r.body))
}

fn code_of(r: &CliResponse) -> String {
    body_of(r)["error"]["code"].as_str().expect("error.code").to_string()
}

fn audit_rows(address: &str, action: &str) -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn
        .prepare("SELECT actor, actor_role FROM mail_reader_audit WHERE address = ?1 AND action = ?2 ORDER BY id")
        .expect("prepare");
    st.query_map(rusqlite::params![address, action], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
}

fn must(r: Result<(), CliResponse>, what: &str) {
    if let Err(e) = r {
        panic!("{what}: {} {}", e.status, e.body);
    }
}

fn principal(project: &str) -> HookPrincipal {
    HookPrincipal { workspace_uuid: project.to_string(), agent_address: "agent".to_string() }
}

/// CV-2: a row STORED `calendar-read` is read-only even when its scope
/// string holds Google's write scope (incremental auth can hand one back):
/// a draft-level caller gets 409 read_only_link with no provider call.
/// A 0.46.1 `calendar` row keeps read and write (Rosson: existing links
/// keep their access).
#[test]
fn the_stored_link_type_caps_the_grant_and_old_links_keep_write() {
    let _home = k2_core::test_env::TempHome::new();
    let owner = insert_project("cap");
    let ro = addr("ro");
    let wider = format!(
        "{} https://www.googleapis.com/auth/calendar.events",
        scope_for(OauthProvider::Gmail, Features::CALENDAR_READ)
    );
    seed(&owner, &ro, "gmail", "calendar-read", &wider);
    let r = resolve_target(&owner, Some(&ro), Level::Draft).err().expect("read-only refuses a write");
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    assert_eq!(body_of(&r)["error"]["reason"], "read_only_link");
    assert_eq!(body_of(&r)["error"]["cap"], "calendar.write");
    match resolve_target(&owner, Some(&ro), Level::Read) {
        Ok(CalTarget::Linked(g)) => {
            assert_eq!(g.cal, CalAccess::Read);
            assert_eq!(g.opted_in, Some(Vec::new()), "the primary only, until the owner adds more");
            assert!(!g.omit_description);
            assert_eq!(g.project_id, owner);
        }
        _ => panic!("read routes to the provider"),
    }
    let old = addr("old");
    seed(&owner, &old, "gmail", "mail,calendar", &scope_for(OauthProvider::Gmail, Features { mail: true, calendar: CalAccess::Write }));
    match resolve_target(&owner, Some(&old), Level::Draft) {
        Ok(CalTarget::Linked(g)) => {
            assert_eq!(g.cal, CalAccess::Write, "a 0.46.1 link keeps write");
            assert_eq!(g.opted_in, None, "and every calendar (GC-23)");
        }
        Err(r) => panic!("old link: {} {}", r.status, r.body),
        _ => panic!("linked"),
    }
    // A read-only grant without the stored type (a row upgraded before
    // 0316 by hand, say) is read-only by its scopes alone.
    let scoped = addr("scoped");
    seed(&owner, &scoped, "microsoft", "mail", &scope_for(OauthProvider::Microsoft, Features { mail: true, calendar: CalAccess::Read }));
    let r = resolve_target(&owner, Some(&scoped), Level::Draft).err().expect("Calendars.Read can't write");
    assert_eq!(body_of(&r)["error"]["reason"], "read_only_link");
    // A Microsoft calendar-only read link routes, and has no mail.
    let ms = addr("ms");
    let id = seed(&owner, &ms, "microsoft", "calendar-read", &scope_for(OauthProvider::Microsoft, Features::CALENDAR_READ));
    assert!(matches!(resolve_target(&owner, Some(&ms), Level::Read), Ok(CalTarget::Linked(_))));
    assert!(!crate::mail::capabilities::linked_row_has_mail(&id));
    cleanup(&[&owner]);
}

/// The scope sets and the stored spelling (§3.1, CV-3).
#[test]
fn calendar_read_scopes_spelling_and_upgrade_union() {
    let g = scope_for(OauthProvider::Gmail, Features::CALENDAR_READ);
    assert_eq!(
        g,
        "openid email https://www.googleapis.com/auth/calendar.events.readonly \
         https://www.googleapis.com/auth/calendar.calendarlist.readonly"
    );
    let m = scope_for(OauthProvider::Microsoft, Features::CALENDAR_READ);
    assert!(m.ends_with("https://graph.microsoft.com/Calendars.Read"), "{m}");
    assert!(!m.contains("ReadWrite") && !m.contains("Mail."), "{m}");
    assert_eq!(crate::mail::oauth::features_granted(OauthProvider::Gmail, &g).calendar, CalAccess::Read);
    assert_eq!(crate::mail::oauth::features_granted(OauthProvider::Microsoft, &m).calendar, CalAccess::Read);
    assert!(!crate::mail::oauth::features_granted(OauthProvider::Gmail, &g).mail);
    let both = Features::parse("mail, calendar-read").unwrap().unwrap();
    assert_eq!(both, Features { mail: true, calendar: CalAccess::Read });
    assert_eq!(both.as_db(), "mail,calendar-read");
    assert_eq!(Features::CALENDAR_READ.as_db(), "calendar-read");
    assert_eq!(Features::parse("calendar-read,calendar").unwrap().unwrap().calendar, CalAccess::Write);
    assert_eq!(Features::MAIL.union(Features::CALENDAR_READ).as_db(), "mail,calendar-read");
    let e = Features::parse("calendar-write").unwrap_err();
    assert!(e.contains("calendar-read"), "{e}");
    assert_eq!(crate::mail::oauth::calendar_level(OauthProvider::Gmail, "calendar-read", "https://www.googleapis.com/auth/calendar.events"), CalAccess::Read);
    assert_eq!(crate::mail::oauth::calendar_level(OauthProvider::Gmail, "mail", "openid email"), CalAccess::None);
}

/// §3.9 (GC-13, CV-16): an agent passport's read of a linked calendar is
/// logged as `agent:<project>` / `agent`, list-shaped once per window; a
/// call with no passport (background work) writes nothing.
#[test]
fn agent_reads_are_logged_and_background_work_is_not() {
    let owner = insert_project("aud");
    let a = addr("aud");
    let id = seed(&owner, &a, "gmail", "calendar-read", &scope_for(OauthProvider::Gmail, Features::CALENDAR_READ));
    must(crate::mail::mailbox_guard::log_agent_read(&owner, Source::Linked, &id, &a, "calendar/events", None), "no passport");
    assert!(audit_rows(&a, "calendar/events").is_empty(), "no passport, no row");
    crate::caller_workspace::with_request_principal(Some(principal(&owner)), || {
        for _ in 0..3 {
            must(crate::mail::mailbox_guard::log_agent_read(&owner, Source::Linked, &id, &a, "calendar/events", None), "logged");
        }
        must(crate::mail::mailbox_guard::log_agent_read(&owner, Source::Linked, &id, &a, "calendar/show", Some("ev_x")), "logged");
        must(crate::mail::mailbox_guard::log_agent_read(&owner, Source::Linked, &id, &a, "calendar/show", Some("ev_y")), "logged");
    });
    assert_eq!(audit_rows(&a, "calendar/events"), vec![(format!("agent:{owner}"), "agent".to_string())], "list-shaped: one row per window");
    assert_eq!(audit_rows(&a, "calendar/show").len(), 2, "every show is a row");
    // A hosted inbox is not a person's linked calendar: nothing here.
    crate::caller_workspace::with_request_principal(Some(principal(&owner)), || {
        must(crate::mail::mailbox_guard::log_agent_read(&owner, Source::Hosted, "h1", "hosted@x.example", "calendar/events", None), "hosted");
    });
    assert!(audit_rows("hosted@x.example", "calendar/events").is_empty());
    cleanup(&[&owner]);
}

/// §3.2 + §3.9: description privacy is a person's choice (a passport gets
/// 403 owner_only, an IT agent too), is logged first, needs the workspace to
/// reach the inbox, and shows in the inbox catalog; grant and set-level
/// write governance rows.
#[test]
fn calendar_privacy_is_owner_only_logged_and_listed() {
    let owner = insert_project("prv");
    let other = insert_project("prv2");
    let a = addr("prv");
    let id = seed(&owner, &a, "gmail", "calendar-read", &scope_for(OauthProvider::Gmail, Features::CALENDAR_READ));
    let body = |p: &str, on: bool| json!({ "address": a, "project": p, "omitDescription": on }).to_string();
    let r = crate::caller_workspace::with_request_principal(Some(principal(&owner)), || {
        crate::mail::routes_access::handle_calendar_privacy(body(&owner, true).as_bytes())
    });
    assert!(r.status.starts_with("403"), "{} {}", r.status, r.body);
    assert_eq!(code_of(&r), "owner_only");
    assert!(!crate::mail::calendar_link::omit_description(&id, &owner).unwrap());
    let r = crate::mail::routes_access::handle_calendar_privacy(body(&other, true).as_bytes());
    assert_eq!(code_of(&r), "not_found", "a workspace with no access: {}", r.body);
    let r = crate::mail::routes_access::handle_calendar_privacy(body(&owner, true).as_bytes());
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    assert_eq!(body_of(&r)["changed"], true);
    assert!(crate::mail::calendar_link::omit_description(&id, &owner).unwrap());
    assert_eq!(audit_rows(&a, "omit_description_on").len(), 1);
    // grant with the flag: logged `grant`, then the choice.
    let g = json!({ "address": a, "project": other, "level": "read", "calendarOmitDescription": true }).to_string();
    let r = crate::mail::routes_access::handle_grant(g.as_bytes());
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    assert_eq!(audit_rows(&a, "grant").len(), 1);
    assert!(crate::mail::calendar_link::omit_description(&id, &other).unwrap());
    let cat = crate::mail::access::catalog_json(None);
    let row = cat["inboxes"].as_array().unwrap().iter().find(|i| i["address"] == json!(a)).expect("listed").clone();
    assert_eq!(row["primary"]["calendarOmitDescription"], true);
    assert_eq!(row["grants"][0]["calendarOmitDescription"], true);
    // revoke: logged, and the pref goes with the grant.
    let rv = json!({ "address": a, "project": other }).to_string();
    let r = crate::mail::routes_access::handle_revoke(rv.as_bytes());
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    assert_eq!(audit_rows(&a, "revoke").len(), 1);
    assert!(!crate::mail::calendar_link::omit_description(&id, &other).unwrap(), "the pref left with the grant");
    // set-level with the flag off.
    let sl = json!({ "address": a, "project": owner, "level": "read", "calendarOmitDescription": false }).to_string();
    let r = crate::mail::routes_access::handle_set_level(sl.as_bytes());
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    assert_eq!(audit_rows(&a, "set_level").len(), 1);
    assert_eq!(audit_rows(&a, "omit_description_off").len(), 1);
    assert!(!crate::mail::calendar_link::omit_description(&id, &owner).unwrap());
    cleanup(&[&owner, &other]);
}

// ── A scripted provider for the opt-in route ────────────────────────────

/// Lists three calendars; every other call is a test failure.
struct ListOnly;

impl CalendarProvider for ListOnly {
    fn source(&self) -> &'static str {
        "google"
    }
    fn label(&self) -> &'static str {
        "Google"
    }
    fn calendars(&self, g: &LinkedCalInbox) -> Result<Vec<Value>, CalErr> {
        Ok(vec![
            json!({ "id": g.address, "name": g.address, "isDefault": true }),
            json!({ "id": "team@group.calendar.google.com", "name": "Consults", "isDefault": false }),
            json!({ "id": "a@group.calendar.google.com", "name": "Twin", "isDefault": false }),
            json!({ "id": "b@group.calendar.google.com", "name": "Twin", "isDefault": false }),
        ])
    }
    fn view(&self, _: &LinkedCalInbox, _: &DateTime<Utc>, _: &DateTime<Utc>, _: Option<&str>, _: usize, _: ViewOpts) -> Result<(Vec<Value>, bool), CalErr> {
        panic!("the opt-in route never lists events")
    }
    fn calendar_of(&self, _: &LinkedCalInbox, _: &str) -> Result<Option<String>, CalErr> {
        panic!("unused")
    }
    fn event(&self, _: &LinkedCalInbox, _: &str, _: bool) -> Result<Option<StoredEvent>, CalErr> {
        panic!("unused")
    }
    fn insert(&self, _: &LinkedCalInbox, _: Option<&str>, _: &DraftEvent) -> Result<(String, Value), CalErr> {
        panic!("never a write")
    }
    fn patch(&self, _: &LinkedCalInbox, _: &str, _: &StoredEvent, _: &DraftPatch) -> Result<Value, CalErr> {
        panic!("never a write")
    }
    fn remove(&self, _: &LinkedCalInbox, _: &str) -> Result<(), CalErr> {
        panic!("never a write")
    }
    fn busy(&self, _: &LinkedCalInbox, _: &DateTime<Utc>, _: &DateTime<Utc>) -> Result<(Vec<crate::mail::calendar::BusySpan>, &'static str, bool), CalErr> {
        panic!("unused")
    }
    fn changes(&self, _: &LinkedCalInbox, _: Option<&str>, _: DateTime<Utc>) -> Result<Changes, StateErr> {
        panic!("unused")
    }
    fn wait_poll_secs(&self) -> u64 {
        10
    }
    fn max_reminders(&self) -> usize {
        5
    }
    fn too_many_reminders(&self) -> String {
        String::new()
    }
}

/// §3.2 (CV-18): the owner opts calendars in by id or name, the primary is
/// always on and can't be removed, a name two calendars share asks for an
/// id, a read-write link has nothing to pick, and each change is logged.
#[test]
fn the_owner_opts_calendars_in_and_out() {
    let _home = k2_core::test_env::TempHome::new();
    let owner = insert_project("opt");
    let a = addr("opt");
    let id = seed(&owner, &a, "gmail", "calendar-read", &scope_for(OauthProvider::Gmail, Features::CALENDAR_READ));
    let g = match resolve_target(&owner, Some(&a), Level::Read) {
        Ok(CalTarget::Linked(g)) => g,
        _ => panic!("routes"),
    };
    let p = ListOnly;
    let r = crate::mail::calendar_link_routes::calendars_post_with(&p, &g, &json!(["Consults", "primary"]), &Value::Null);
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    let v = body_of(&r);
    assert_eq!(v["added"], json!(["team@group.calendar.google.com"]), "the primary is skipped: always on");
    let on: HashMap<String, bool> = v["calendars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap().to_string(), c["optedIn"].as_bool().unwrap()))
        .collect();
    assert_eq!(on["Consults"], true);
    assert_eq!(on[a.as_str()], true);
    assert_eq!(crate::mail::calendar_link::opted_in(&id).unwrap().len(), 1);
    assert_eq!(audit_rows(&a, "calendar_add").len(), 1);
    let r = crate::mail::calendar_link_routes::calendars_post_with(&p, &g, &json!("Twin"), &Value::Null);
    assert_eq!(code_of(&r), "usage", "an ambiguous name: {}", r.body);
    assert!(r.body.contains("a@group.calendar.google.com") && r.body.contains("b@group.calendar.google.com"));
    let r = crate::mail::calendar_link_routes::calendars_post_with(&p, &g, &json!("Nope"), &Value::Null);
    assert_eq!(code_of(&r), "not_found");
    let r = crate::mail::calendar_link_routes::calendars_post_with(&p, &g, &Value::Null, &json!("primary"));
    assert_eq!(code_of(&r), "usage", "the primary can't be removed");
    let r = crate::mail::calendar_link_routes::calendars_post_with(&p, &g, &Value::Null, &json!(["Consults"]));
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    assert_eq!(body_of(&r)["removed"], json!(["team@group.calendar.google.com"]));
    assert!(crate::mail::calendar_link::opted_in(&id).unwrap().is_empty());
    assert_eq!(audit_rows(&a, "calendar_remove").len(), 1);
    // The GET views: the owner sees optedIn on all; anyone else only what shows.
    let v = body_of(&crate::mail::calendar_link_routes::calendars_get_with(&p, &g, true));
    assert_eq!(v["calendars"].as_array().unwrap().len(), 4);
    let v = body_of(&crate::mail::calendar_link_routes::calendars_get_with(&p, &g, false));
    assert_eq!(v["calendars"].as_array().unwrap().len(), 1, "the primary only");
    // A read-write link has nothing to pick.
    let rw = LinkedCalInbox { cal: CalAccess::Write, opted_in: None, ..g.clone() };
    let r = crate::mail::calendar_link_routes::calendars_post_with(&p, &rw, &json!("Consults"), &Value::Null);
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    // The POST route itself is owner/Admin: no viewer (a passport) → 403.
    let r = crate::mail::calendar_link_routes::handle_calendars_post(json!({ "address": a, "add": ["Consults"] }).to_string().as_bytes());
    assert_eq!(code_of(&r), "owner_only");
    cleanup(&[&owner]);
}

/// CV-17: each new route has a dispatch arm, a policy row, and POST-only
/// routes answer 405 to a GET.
#[test]
fn the_new_routes_are_wired() {
    let empty = HashMap::new();
    for p in ["/cli/mail/calendar/link-request", "/cli/mail/access/calendar-privacy"] {
        let r = crate::mail_routes::dispatch(p, &empty).expect("mail family");
        assert!(r.status.starts_with("405"), "GET {p} must 405, got {}", r.status);
        assert!(crate::routes::route_policy::post_allowed(p), "{p} on the POST allowlist");
        let r = crate::mail_routes::dispatch_post(p, b"not json");
        assert_eq!(code_of(&r), "usage", "{p}: {}", r.body);
    }
    let r = crate::mail_routes::dispatch("/cli/mail/calendar/calendars", &empty).expect("mail family");
    assert_eq!(code_of(&r), "usage", "GET calendars reaches its handler: {}", r.body);
    assert!(crate::routes::route_policy::post_allowed("/cli/mail/calendar/calendars"));
    assert!(crate::session_token::is_agent_verb("/cli/mail/calendar/link-request"), "agents may ask");
    assert!(!"/cli/mail/calendar/link-request".starts_with("/cli/mail/link/"), "not under the passport-denied prefix");
    // link-request: a bad provider is a usage error before anything posts.
    let r = crate::mail::calendar_link_routes::handle_link_request(
        json!({ "project": "x", "address": "a@example.com", "provider": "yahoo" }).to_string().as_bytes(),
    );
    assert_eq!(code_of(&r), "usage");
}

/// The default level stays the capability truth: `k2 mail capabilities`
/// reports calendar.write off with read_only_link on a read-only link.
#[test]
fn capabilities_report_read_only_link() {
    let owner = insert_project("capr");
    let a = addr("capr");
    seed(&owner, &a, "gmail", "calendar-read", &scope_for(OauthProvider::Gmail, Features::CALENDAR_READ));
    let cat = crate::mail::access::catalog_json(Some(&owner));
    let row = cat["inboxes"].as_array().unwrap().iter().find(|i| i["address"] == json!(a)).expect("listed").clone();
    let caps = &row["capabilities"];
    assert_eq!(caps["calendar"]["read"], true);
    assert_eq!(caps["calendar"]["write"], false);
    let missing = caps["missing"].as_array().unwrap();
    assert!(missing.iter().any(|m| m["cap"] == "calendar.write" && m["reason"] == "read_only_link"), "{caps}");
    assert!(missing.iter().any(|m| m["cap"] == "calendar.rsvp" && m["reason"] == "read_only_link"), "{caps}");
    cleanup(&[&owner]);
}
