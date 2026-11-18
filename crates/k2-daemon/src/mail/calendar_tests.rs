use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

// ── Fixture JMAP (no network) ────────────────────────────────────────

#[derive(Default)]
struct Fake {
    calendars: Vec<Value>,
    /// jmap id → stored event (what CalendarEvent/get returns).
    events: HashMap<String, Value>,
    query_ids: Vec<String>,
    query_err: Option<String>,
    availability: Option<Result<Vec<Value>, String>>,
    state: String,
    changes: RefCell<VecDeque<Result<Value, String>>>,
    set_reply: Option<Value>,
    /// (method, account, args)
    calls: RefCell<Vec<(String, String, Value)>>,
    // ── S4: other accounts (calendars shared to the inbox) ──
    /// account → its calendars WITH shareWith.
    shared: HashMap<String, Vec<Value>>,
    /// The inbox account's groups.
    groups: Vec<String>,
    /// account → jmap id → stored event (overrides `events`).
    acct_events: HashMap<String, HashMap<String, Value>>,
    /// account → query ids (overrides `query_ids`).
    acct_query: HashMap<String, Vec<String>>,
    /// account → state (overrides `state`).
    acct_state: HashMap<String, String>,
    /// account → queued `changes` replies (overrides `changes`).
    acct_changes: RefCell<HashMap<String, VecDeque<Result<Value, String>>>>,
}

impl Fake {
    fn record(&self, m: &str, account: &str, args: Value) {
        self.calls.borrow_mut().push((m.to_string(), account.to_string(), args));
    }
    fn calls_of(&self, m: &str) -> Vec<(String, Value)> {
        self.calls
            .borrow()
            .iter()
            .filter(|(n, _, _)| n == m)
            .map(|(_, a, v)| (a.clone(), v.clone()))
            .collect()
    }
}

impl CalendarApi for Fake {
    fn calendars(&self, account: &str) -> Result<Vec<Value>, String> {
        self.record("Calendar/get", account, Value::Null);
        Ok(self.calendars.clone())
    }
    fn query(&self, account: &str, args: Value) -> Result<Vec<String>, String> {
        self.record("CalendarEvent/query", account, args);
        match &self.query_err {
            Some(e) => Err(e.clone()),
            None => Ok(self
                .acct_query
                .get(account)
                .cloned()
                .unwrap_or_else(|| self.query_ids.clone())),
        }
    }
    fn get(&self, account: &str, ids: &[String], props: Option<&[&str]>) -> Result<Value, String> {
        self.record("CalendarEvent/get", account, json!({ "ids": ids, "properties": props }));
        let store = self.acct_events.get(account).unwrap_or(&self.events);
        let list: Vec<Value> = ids.iter().filter_map(|i| store.get(i).cloned()).collect();
        let state = self.acct_state.get(account).unwrap_or(&self.state);
        Ok(json!({ "list": list, "state": state }))
    }
    fn changes(&self, account: &str, since: &str) -> Result<Value, String> {
        self.record("CalendarEvent/changes", account, json!({ "sinceState": since }));
        if let Some(q) = self.acct_changes.borrow_mut().get_mut(account) {
            return q.pop_front().unwrap_or_else(|| {
                Ok(json!({ "newState": since, "created": [], "updated": [], "destroyed": [] }))
            });
        }
        self.changes
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(json!({ "newState": since, "created": [], "updated": [], "destroyed": [] })))
    }
    fn calendars_shared(&self, account: &str) -> Result<Vec<Value>, String> {
        self.record("Calendar/get+shareWith", account, Value::Null);
        Ok(self.shared.get(account).cloned().unwrap_or_default())
    }
    fn member_groups(&self, account: &str) -> Result<Vec<String>, String> {
        self.record("x:Account/get memberGroupIds", account, Value::Null);
        Ok(self.groups.clone())
    }
    fn set_no_scheduling(&self, account: &str, args: Value) -> Result<Value, String> {
        self.record("CalendarEvent/set", account, args);
        Ok(self.set_reply.clone().unwrap_or_else(|| json!({})))
    }
    fn availability(&self, account: &str, s: &str, e: &str) -> Result<Vec<Value>, String> {
        self.record("Principal/getAvailability", account, json!({ "utcStart": s, "utcEnd": e }));
        self.availability
            .clone()
            .unwrap_or_else(|| Err("unknownMethod".to_string()))
    }
}

const OWNER: &str = "cal-owner@example.com";

fn inbox() -> CalInbox {
    CalInbox {
        address: OWNER.to_string(),
        account_id: "acct-1".to_string(),
        level: Level::Send,
        peers: Vec::new(),
    }
}

/// `CliResponse` isn't `Debug`: unwrap with the status + body shown.
fn must<T>(r: Result<T, CliResponse>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{what}: {} {}", e.status, e.body),
    }
}

fn body_of(resp: &CliResponse) -> Value {
    serde_json::from_str(&resp.body).expect("JSON body")
}

fn code_of(resp: &CliResponse) -> String {
    body_of(resp)["error"]["code"].as_str().unwrap_or("").to_string()
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn now() -> DateTime<Utc> {
    parse_instant("2026-10-07T12:00:00Z", "now").expect("now")
}

fn write_body(v: Value) -> WriteBody {
    serde_json::from_value(v).expect("write body")
}

fn plain_event(id: &str) -> Value {
    json!({
        "id": id,
        "calendarIds": { "cal-1": true },
        "title": "Prep",
        "start": "2026-10-08T16:00:00",
        "duration": "PT30M",
        "timeZone": "Etc/UTC",
        "utcStart": "2026-10-08T16:00:00Z",
        "utcEnd": "2026-10-08T16:30:00Z",
    })
}

fn assert_no_set(fake: &Fake) {
    assert!(
        fake.calls_of("CalendarEvent/set").is_empty(),
        "a refused draft write must never reach CalendarEvent/set"
    );
}

// ── DB fixtures (shared test DB; unique rows per test) ──────────────

fn insert_project(tag: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/k2-cal-{tag}-{}", &id[..8]);
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, format!("cal-{tag}-{}", &id[..8]), path],
    )
    .expect("insert project");
    (id, path)
}

fn seed_hosted(owner: &str, address: &str, level: &str, account: Option<&str>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, \
             owner_project_id, status, created_at, primary_level) \
             VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100, ?5)",
        rusqlite::params![id, address, account, owner, level],
    )
    .expect("seed hosted");
    id
}

fn seed_linked(owner: &str, address: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, \
             port, username, created_at, primary_level) \
             VALUES (?1, ?2, ?3, 'imap.example.com', 993, ?3, 100, 'draft')",
        rusqlite::params![id, owner, address],
    )
    .expect("seed linked");
    id
}

fn grant(source: &str, inbox_id: &str, project_id: &str, level: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at) \
             VALUES (?1, ?2, ?3, ?4, 100)",
        rusqlite::params![source, inbox_id, project_id, level],
    )
    .expect("seed grant");
}

fn cleanup(project_ids: &[&str]) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for p in project_ids {
        let _ = conn.execute(
            "DELETE FROM mail_inbox_grants WHERE project_id = ?1",
            rusqlite::params![p],
        );
        let _ = conn.execute(
            "DELETE FROM mail_addresses WHERE owner_project_id = ?1",
            rusqlite::params![p],
        );
        let _ = conn.execute(
            "DELETE FROM mail_external_inboxes WHERE owner_project_id = ?1",
            rusqlite::params![p],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![p]);
    }
}

fn addr(tag: &str) -> String {
    format!("{tag}-{}@cal.example.com", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

// ── JMAP envelope (loopback mock server) ────────────────────────────

#[test]
fn calendars_call_envelope_pins_using_account_and_no_scheduling() {
    use crate::mail::jmap::tests::{body_json, spawn_mock_server, NORMAL_SESSION_FIXTURE};
    let set_reply = json!({
        "methodResponses": [["CalendarEvent/set", { "accountId": "acct-u1", "created": {} }, "0"]]
    })
    .to_string();
    let cal_reply = json!({
        "methodResponses": [["Calendar/get", { "accountId": "acct-u1", "list": [], "state": "s1" }, "0"]]
    })
    .to_string();
    let avail_reply = json!({
        "methodResponses": [["Principal/getAvailability", { "list": [] }, "0"]]
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        set_reply,
        cal_reply,
        avail_reply,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "test-key");
    // A caller-built accountId and sendSchedulingMessages:true must
    // both be overwritten by the wrapper.
    client
        .calendar_event_set_no_scheduling(
            "acct-u1",
            json!({ "accountId": "acct-evil", "sendSchedulingMessages": true, "destroy": ["e1"] }),
        )
        .expect("set");
    client.calendar_list("acct-u1").expect("calendar list");
    client
        .principal_availability("acct-u1", "2026-10-08T00:00:00Z", "2026-10-09T00:00:00Z")
        .expect("availability");
    let _session = rx.recv().expect("session request");
    let set_req = body_json(&rx.recv().expect("set request"));
    assert_eq!(
        set_req["using"],
        json!([
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
            "urn:ietf:params:jmap:principals"
        ])
    );
    let call = &set_req["methodCalls"][0];
    assert_eq!(call[0], "CalendarEvent/set");
    assert_eq!(call[1]["accountId"], "acct-u1", "never a client accountId (CAL16)");
    assert_eq!(call[1]["sendSchedulingMessages"], false, "draft never schedules (CAL4)");
    let cal_req = body_json(&rx.recv().expect("calendar request"));
    assert_eq!(cal_req["methodCalls"][0][0], "Calendar/get");
    let props = cal_req["methodCalls"][0][1]["properties"].as_array().expect("properties");
    assert!(props.contains(&json!("isDefault")), "isDefault must be asked for: {props:?}");
    let av_req = body_json(&rx.recv().expect("availability request"));
    assert_eq!(av_req["methodCalls"][0][0], "Principal/getAvailability");
    assert_eq!(av_req["methodCalls"][0][1]["id"], "acct-u1");
    assert_eq!(av_req["methodCalls"][0][1]["accountId"], "acct-u1");
    assert_eq!(av_req["methodCalls"][0][1]["utcStart"], "2026-10-08T00:00:00Z");
}

// ── Inbox resolution (CAL19) + gating ───────────────────────────────

#[test]
fn no_address_uses_the_single_owned_hosted_inbox_zero_and_many_refused() {
    let (p, _) = insert_project("prim");
    // 0 owned → 404 not_found.
    let r = resolve_inbox(&p, None, Level::Read).expect_err("none owned");
    assert!(r.status.starts_with("404"), "{}", r.status);
    assert_eq!(code_of(&r), "not_found");
    // 1 owned → it, with the account from the ROW.
    let a1 = addr("one");
    seed_hosted(&p, &a1, "send", Some("acct-row-1"));
    let got = must(resolve_inbox(&p, None, Level::Read), "one owned");
    assert_eq!((got.address.as_str(), got.account_id.as_str()), (a1.as_str(), "acct-row-1"));
    assert_eq!(got.level, Level::Send, "the owner's primary level");
    assert!(got.peers.iter().all(|p| p.account_id != "acct-row-1"), "peers exclude itself");
    // >1 owned → 400 usage naming both.
    let a2 = addr("two");
    seed_hosted(&p, &a2, "send", Some("acct-row-2"));
    let r = resolve_inbox(&p, None, Level::Read).expect_err("two owned");
    assert!(r.status.starts_with("400"));
    assert_eq!(code_of(&r), "usage");
    let hint = body_of(&r)["error"]["hint"].as_str().unwrap_or("").to_string();
    assert!(hint.contains(&a1) && hint.contains(&a2), "{hint}");
    // Explicit address still works with two.
    assert_eq!(
        must(resolve_inbox(&p, Some(&a2), Level::Read), "explicit").account_id,
        "acct-row-2"
    );
    cleanup(&[&p]);
}

#[test]
/// MCS3: an app-password IMAP inbox has no calendar API — 409
/// `capability_missing` / `backend_unsupported`, but only after the
/// access gate passed (a stranger still gets the masked not_found).
fn linked_imap_inbox_is_capability_missing_but_only_after_access_passes() {
    let (owner, _) = insert_project("lnk-own");
    let (other, _) = insert_project("lnk-oth");
    let linked = addr("linked");
    seed_linked(&owner, &linked);
    let r = resolve_inbox(&owner, Some(&linked), Level::Read).expect_err("linked");
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    assert_eq!(code_of(&r), "capability_missing");
    assert!(r.body.contains("backend_unsupported"), "{}", r.body);
    // A workspace with no access never learns it is linked.
    let r = resolve_inbox(&other, Some(&linked), Level::Read).expect_err("masked");
    assert!(r.status.starts_with("404"));
    assert_eq!(code_of(&r), "not_found");
    cleanup(&[&owner, &other]);
}

/// An OAuth linked row (`kind` graph for Microsoft, imap for Gmail).
fn seed_oauth_link(owner: &str, address: &str, provider: &str, calendar: bool) -> String {
    use crate::mail::oauth::{scope_for, Features, OauthProvider};
    let (kind, host, p) = match provider {
        "microsoft" => ("graph", "graph.microsoft.com", OauthProvider::Microsoft),
        _ => ("imap", "imap.gmail.com", OauthProvider::Gmail),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, kind, host, \
             port, username, created_at, primary_level, auth_kind, provider, features, \
             granted_scopes) \
             VALUES (?1, ?2, ?3, ?4, ?5, 443, ?3, 100, 'draft', 'oauth', ?6, ?7, ?8)",
        rusqlite::params![
            id,
            owner,
            address,
            kind,
            host,
            provider,
            if calendar { "mail,calendar" } else { "mail" },
            scope_for(p, Features { mail: true, calendar: if calendar { crate::mail::oauth::CalAccess::Write } else { crate::mail::oauth::CalAccess::None } }),
        ],
    )
    .expect("seed oauth link");
    id
}

fn reason_of(resp: &CliResponse) -> String {
    body_of(resp)["error"]["reason"].as_str().expect("error.reason").to_string()
}

#[test]
/// MCS3: a Microsoft link routes to Graph only with the calendar grant
/// and the owner's switch on; otherwise `capability_missing` with the
/// reason and the fix — always AFTER the access gate (a stranger gets
/// the masked not_found, never the reason).
fn microsoft_link_routes_to_graph_or_names_the_missing_capability() {
    let _home = k2_core::test_env::TempHome::new();
    let (owner, _) = insert_project("ms-own");
    let (other, _) = insert_project("ms-oth");
    let mail_only = addr("ms-mail");
    let with_cal = addr("ms-cal");
    seed_oauth_link(&owner, &mail_only, "microsoft", false);
    seed_oauth_link(&owner, &with_cal, "microsoft", true);

    let r = resolve_target(&owner, Some(&mail_only), Level::Read).err().expect("mail only");
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    assert_eq!(code_of(&r), "capability_missing");
    assert_eq!(reason_of(&r), "scope_not_granted");
    assert_eq!(body_of(&r)["error"]["cap"], "calendar.read");
    assert_eq!(
        body_of(&r)["error"]["fix"],
        format!("k2 mail link upgrade {mail_only} --features calendar-read (owner)")
    );
    let r = resolve_target(&owner, Some(&mail_only), Level::Draft).err().expect("mail only");
    assert_eq!(body_of(&r)["error"]["cap"], "calendar.write");

    match resolve_target(&owner, Some(&with_cal), Level::Draft) {
        Ok(CalTarget::Linked(g)) => {
            assert_eq!(g.address, with_cal);
            assert_eq!(g.level, Level::Draft, "the owner's primary level");
        }
        Ok(CalTarget::Hosted(_)) => panic!("a Microsoft link is never hosted"),
        Err(r) => panic!("routable: {} {}", r.status, r.body),
    }
    // The hosted-only callers (S4 sharing) still refuse it.
    let r = resolve_inbox(&owner, Some(&with_cal), Level::Read).expect_err("hosted only");
    assert_eq!(code_of(&r), "hosted_only");

    // Its event ids resolve to Graph; the shared-event form never does.
    let tok = encode_event_id(&with_cal, "AAMkAGI2-long-graph-id==");
    match resolve_event_target(&owner, &tok, Level::Read) {
        Ok(EventTarget::Linked(g, id)) => {
            assert_eq!(g.address, with_cal);
            assert_eq!(id, "AAMkAGI2-long-graph-id==");
        }
        _ => panic!("graph event"),
    }
    let shared = encode_shared_event_id(&with_cal, "AAMk", "boss@contoso.com");
    let r = resolve_event_target(&owner, &shared, Level::Read).err().expect("no shared form");
    assert_eq!(code_of(&r), "not_found");
    // An oversized token never decodes (MCS-A23).
    let huge = encode_event_id(&with_cal, &"x".repeat(1100));
    let r = resolve_event_target(&owner, &huge, Level::Read).err().expect("too long");
    assert_eq!(code_of(&r), "usage");

    // The owner's switch off → owner_disabled, still after the gate.
    k2_core::app_settings::update(json!({ "linkedCalendars": false })).expect("switch off");
    let r = resolve_target(&owner, Some(&with_cal), Level::Read).err().expect("switched off");
    assert_eq!(code_of(&r), "capability_missing");
    assert_eq!(reason_of(&r), "owner_disabled");
    assert_eq!(body_of(&r)["error"]["fix"], "k2 mail link calendars on (owner)");
    k2_core::app_settings::update(json!({ "linkedCalendars": true })).expect("switch on");
    assert!(matches!(resolve_target(&owner, Some(&with_cal), Level::Read), Ok(CalTarget::Linked(_))));

    // A stranger learns nothing.
    for a in [&mail_only, &with_cal] {
        let r = resolve_target(&other, Some(a), Level::Read).err().expect("masked");
        assert!(r.status.starts_with("404"), "{}", r.body);
        assert_eq!(code_of(&r), "not_found");
        assert!(!r.body.contains("capability_missing"));
    }
    cleanup(&[&owner, &other]);
}

#[test]
/// MCS4: a Google link routes to the Google Calendar provider with the
/// calendar grant (a calendar-only link too) and the owner's switch on;
/// a mail-only link answers `scope_not_granted` with the upgrade fix —
/// always after the access gate.
fn google_link_routes_to_google_or_names_the_missing_capability() {
    use crate::mail::calendar_provider::LinkedBackend;
    let _home = k2_core::test_env::TempHome::new();
    let (owner, _) = insert_project("g-own");
    let (other, _) = insert_project("g-oth");
    let consented = addr("g-cal");
    let mail_only = addr("g-mail");
    let cal_only = addr("g-calonly");
    seed_oauth_link(&owner, &consented, "gmail", true);
    seed_oauth_link(&owner, &mail_only, "gmail", false);
    let id = seed_oauth_link(&owner, &cal_only, "gmail", true);
    {
        use crate::mail::oauth::{scope_for, Features, OauthProvider};
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_external_inboxes SET features = 'calendar', granted_scopes = ?2 WHERE id = ?1",
            rusqlite::params![id, scope_for(OauthProvider::Gmail, Features { mail: false, calendar: crate::mail::oauth::CalAccess::Write })],
        )
        .expect("calendar-only");
    }
    for a in [&consented, &cal_only] {
        match resolve_target(&owner, Some(a), Level::Draft) {
            Ok(CalTarget::Linked(g)) => {
                assert_eq!(&g.address, a);
                assert_eq!(g.backend, LinkedBackend::Google);
            }
            Ok(CalTarget::Hosted(_)) => panic!("a Google link is never hosted"),
            Err(r) => panic!("routable {a}: {} {}", r.status, r.body),
        }
    }
    let tok = encode_event_id(&consented, "abc123/primary");
    match resolve_event_target(&owner, &tok, Level::Read) {
        Ok(EventTarget::Linked(g, id)) => {
            assert_eq!(g.backend, LinkedBackend::Google);
            assert_eq!(id, "abc123/primary");
        }
        _ => panic!("google event"),
    }
    let r = resolve_target(&owner, Some(&mail_only), Level::Read).err().expect("google");
    assert_eq!(code_of(&r), "capability_missing");
    assert_eq!(reason_of(&r), "scope_not_granted");
    assert_eq!(
        body_of(&r)["error"]["fix"],
        format!("k2 mail link upgrade {mail_only} --features calendar-read (owner)")
    );
    k2_core::app_settings::update(json!({ "linkedCalendars": false })).expect("switch off");
    let r = resolve_target(&owner, Some(&consented), Level::Read).err().expect("switched off");
    assert_eq!(reason_of(&r), "owner_disabled");
    k2_core::app_settings::update(json!({ "linkedCalendars": true })).expect("switch on");
    for a in [&consented, &mail_only, &cal_only] {
        let r = resolve_target(&other, Some(a), Level::Read).err().expect("masked");
        assert!(r.status.starts_with("404"), "{}", r.body);
        assert!(!r.body.contains("capability_missing"));
    }
    cleanup(&[&owner, &other]);
}

#[test]
/// MCS4: a calendar-only Google link answers mail verbs with
/// `capability_missing` (mail scope not granted, upgrade fix) and drops
/// out of the all-inbox mail sweep; a mail link passes the gate.
fn calendar_only_google_link_has_no_mail() {
    let _home = k2_core::test_env::TempHome::new();
    let (owner, _) = insert_project("gm-own");
    let cal_only = addr("gm-calonly");
    let with_mail = addr("gm-mail");
    let id = seed_oauth_link(&owner, &cal_only, "gmail", true);
    let mail_id = seed_oauth_link(&owner, &with_mail, "gmail", false);
    {
        use crate::mail::oauth::{scope_for, Features, OauthProvider};
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE mail_external_inboxes SET features = 'calendar', granted_scopes = ?2 WHERE id = ?1",
            rusqlite::params![id, scope_for(OauthProvider::Gmail, Features { mail: false, calendar: crate::mail::oauth::CalAccess::Write })],
        )
        .expect("calendar-only");
    }
    assert!(!crate::mail::capabilities::linked_row_has_mail(&id));
    assert!(crate::mail::capabilities::linked_row_has_mail(&mail_id));
    let row = crate::mail::external::inbox_for_address(&cal_only).expect("row");
    let r = crate::mail::capabilities::mail_scope_gate(&row, "mail.read").expect_err("no mail");
    assert!(r.status.starts_with("409"), "{}", r.body);
    assert_eq!(code_of(&r), "capability_missing");
    assert_eq!(reason_of(&r), "scope_not_granted");
    assert_eq!(body_of(&r)["error"]["cap"], "mail.read");
    assert_eq!(
        body_of(&r)["error"]["fix"],
        format!("k2 mail link upgrade {cal_only} --features mail (owner)")
    );
    let row = crate::mail::external::inbox_for_address(&with_mail).expect("row");
    assert!(crate::mail::capabilities::mail_scope_gate(&row, "mail.read").is_ok());
    cleanup(&[&owner]);
}

#[test]
fn read_grant_reads_but_cannot_draft_and_no_grant_is_masked() {
    let (owner, _) = insert_project("g-own");
    let (reader, _) = insert_project("g-read");
    let (drafter, _) = insert_project("g-draft");
    let (stranger, _) = insert_project("g-none");
    let a = addr("granted");
    let row = seed_hosted(&owner, &a, "send", Some("acct-g"));
    grant("hosted", &row, &reader, "read");
    grant("hosted", &row, &drafter, "draft");
    assert!(resolve_inbox(&reader, Some(&a), Level::Read).is_ok());
    let r = resolve_inbox(&reader, Some(&a), Level::Draft).expect_err("read can't write");
    assert_eq!(code_of(&r), "not_found", "a read grant answers the masked not_found");
    assert!(resolve_inbox(&drafter, Some(&a), Level::Draft).is_ok());
    for need in [Level::Read, Level::Draft] {
        let r = resolve_inbox(&stranger, Some(&a), need).expect_err("no grant");
        assert!(r.status.starts_with("404"));
        assert_eq!(code_of(&r), "not_found");
    }
    // The owner's own primary_level applies too (read-only owner).
    let (ro_owner, _) = insert_project("g-ro");
    let ro = addr("readonly");
    seed_hosted(&ro_owner, &ro, "read", Some("acct-ro"));
    assert!(resolve_inbox(&ro_owner, None, Level::Read).is_ok());
    assert_eq!(
        code_of(&resolve_inbox(&ro_owner, None, Level::Draft).expect_err("ro owner")),
        "not_found"
    );
    cleanup(&[&owner, &reader, &drafter, &stranger, &ro_owner]);
}

#[test]
fn hosted_row_without_account_fails_loudly() {
    let (p, _) = insert_project("noacct");
    let a = addr("degraded");
    seed_hosted(&p, &a, "send", None);
    let r = resolve_inbox(&p, Some(&a), Level::Read).expect_err("degraded mint");
    assert!(r.status.starts_with("502"));
    cleanup(&[&p]);
}

#[test]
fn event_ids_carry_their_inbox_and_foreign_ids_are_masked() {
    let tok = encode_event_id("a@example.com", "Jx9");
    assert!(tok.starts_with("ev_"));
    assert_eq!(decode_event_id(&tok), Some(("a@example.com".into(), "Jx9".into())));
    for bad in ["", "ev_", "m_abc", "ev_!!!", &format!("ev_{}", B64URL.encode("noline"))] {
        assert_eq!(decode_event_id(bad), None, "{bad}");
    }
    let (owner, _) = insert_project("id-own");
    let (other, _) = insert_project("id-oth");
    let a = addr("ids");
    seed_hosted(&owner, &a, "send", Some("acct-ids"));
    let tok = encode_event_id(&a, "e1");
    let (inbox, jid, shared_owner) = must(resolve_event(&owner, &tok, Level::Read), "own");
    assert_eq!((inbox.account_id.as_str(), jid.as_str()), ("acct-ids", "e1"));
    assert!(shared_owner.is_none());
    let r = resolve_event(&other, &tok, Level::Read).expect_err("foreign");
    assert!(r.status.starts_with("404"));
    let hint = body_of(&r)["error"]["hint"].as_str().unwrap_or("").to_string();
    assert!(hint.contains("no event"), "event-level mask, not the address: {hint}");
    assert!(!hint.contains(&a), "the inbox address must not leak: {hint}");
    assert_eq!(code_of(&resolve_event(&owner, "junk", Level::Read).expect_err("bad")), "usage");
    cleanup(&[&owner, &other]);
}

#[test]
fn an_event_id_not_on_the_resolved_account_is_not_found() {
    // The admin key can read ANY account; the id must exist on the
    // RESOLVED one (CAL16) — the fake account has no such event.
    let fake = Fake::default();
    let r = show_with(&fake, &inbox(), "other-acct-event", "ev_x");
    assert!(r.status.starts_with("404"), "{}", r.body);
    let gets = fake.calls_of("CalendarEvent/get");
    assert_eq!(gets[0].0, "acct-1", "looked up on the resolved account only");
    // Same for update/delete: masked, never a set.
    let b = write_body(json!({ "project": "x", "id": "ev_x", "title": "t" }));
    assert!(update_with(&fake, &inbox(), "nope", "ev_x", &b).status.starts_with("404"));
    assert!(delete_with(&fake, &inbox(), "nope", "ev_x").status.starts_with("404"));
    assert_no_set(&fake);
}

#[test]
fn unknown_calendar_is_rejected_with_the_inbox_calendars() {
    let fake = Fake {
        calendars: vec![json!({ "id": "cal-1", "name": "Work", "isDefault": true })],
        ..Default::default()
    };
    let r = events_with(&fake, &inbox(), &params(&[("calendar", "cal-of-someone-else")]), now());
    assert_eq!(code_of(&r), "usage");
    assert!(r.body.contains("Work (cal-1)"), "{}", r.body);
    assert!(fake.calls_of("CalendarEvent/query").is_empty());
    // By name works and becomes the singular inCalendar filter.
    let r = events_with(&fake, &inbox(), &params(&[("calendar", "work")]), now());
    assert!(r.status.starts_with("200"), "{}", r.body);
    let q = &fake.calls_of("CalendarEvent/query")[0].1;
    assert_eq!(q["filter"]["inCalendar"], "cal-1");
}

// ── Draft guard (CAL4 / CAL20) ──────────────────────────────────────

fn create_body(extra: Value) -> WriteBody {
    let mut b = json!({
        "project": "x",
        "title": "Prep call",
        "start": "2026-10-08T09:00:00-07:00",
        "end": "2026-10-08T09:30:00-07:00",
    });
    if let (Value::Object(m), Value::Object(e)) = (&mut b, extra) {
        m.extend(e);
    }
    write_body(b)
}

#[test]
fn create_with_other_participants_or_email_alert_is_refused_before_any_call() {
    let fake = Fake {
        calendars: vec![json!({ "id": "cal-1", "isDefault": true })],
        ..Default::default()
    };
    let r = create_with(
        &fake,
        &inbox(),
        &create_body(json!({ "participants": {
            "p1": { "calendarAddress": "mailto:guest@example.org" }
        }})),
    );
    assert!(r.status.starts_with("409"), "{}", r.body);
    assert_eq!(code_of(&r), "invites_need_send_level");
    let r = create_with(&fake, &inbox(), &create_body(json!({ "participants": ["guest@example.org"] })));
    assert_eq!(code_of(&r), "invites_need_send_level");
    let r = create_with(
        &fake,
        &inbox(),
        &create_body(json!({ "alerts": [{ "action": "email", "offset": "-PT15M" }] })),
    );
    assert_eq!(code_of(&r), "email_alerts_need_send_level");
    assert_no_set(&fake);
    assert!(fake.calls.borrow().is_empty(), "refused before any JMAP call");
}

#[test]
fn create_owner_only_participant_is_fine_and_sets_never_schedule() {
    let fake = Fake {
        calendars: vec![
            json!({ "id": "cal-2", "name": "Other", "sortOrder": 0 }),
            json!({ "id": "cal-1", "name": "Main", "isDefault": true }),
        ],
        set_reply: Some(json!({ "created": { "k2new": { "id": "new-1" } } })),
        ..Default::default()
    };
    let r = create_with(
        &fake,
        &inbox(),
        &create_body(json!({ "participants": [format!("mailto:{}", OWNER.to_uppercase())] })),
    );
    assert!(r.status.starts_with("200"), "{}", r.body);
    let b = body_of(&r);
    assert_eq!(b["id"], json!(encode_event_id(OWNER, "new-1")));
    assert_eq!(b["emailSent"], false);
    let sets = fake.calls_of("CalendarEvent/set");
    assert_eq!(sets.len(), 1);
    let (account, args) = &sets[0];
    assert_eq!(account, "acct-1");
    assert_eq!(args["sendSchedulingMessages"], false);
    let ev = &args["create"]["k2new"];
    assert_eq!(ev["calendarIds"], json!({ "cal-1": true }), "the isDefault calendar");
    // No zone → Etc/UTC wall time + duration, never an offset or `end`.
    assert_eq!(ev["timeZone"], "Etc/UTC");
    assert_eq!(ev["start"], "2026-10-08T16:00:00");
    assert_eq!(ev["duration"], "PT30M");
    assert!(ev.get("end").is_none() && ev.get("participants").is_none());
}

#[test]
fn create_with_a_zone_sends_utc_start_end_for_the_server_to_convert() {
    let fake = Fake {
        calendars: vec![json!({ "id": "cal-1", "isDefault": true })],
        set_reply: Some(json!({ "created": { "k2new": { "id": "new-2" } } })),
        ..Default::default()
    };
    let r = create_with(
        &fake,
        &inbox(),
        &create_body(json!({ "timeZone": "America/Los_Angeles", "location": "Room 2",
                             "notes": "agenda", "alerts": [{ "offset": "-PT10M" }] })),
    );
    assert!(r.status.starts_with("200"), "{}", r.body);
    let args = &fake.calls_of("CalendarEvent/set")[0].1;
    let ev = &args["create"]["k2new"];
    assert_eq!(ev["timeZone"], "America/Los_Angeles");
    assert_eq!(ev["utcStart"], "2026-10-08T16:00:00Z");
    assert_eq!(ev["utcEnd"], "2026-10-08T16:30:00Z");
    assert!(ev.get("start").is_none());
    assert_eq!(ev["locations"]["l1"]["name"], "Room 2");
    assert_eq!(ev["alerts"]["a1"]["action"], "display");
    // Bad inputs are usage errors with no set.
    let fake2 = Fake {
        calendars: vec![json!({ "id": "cal-1", "isDefault": true })],
        ..Default::default()
    };
    for bad in [
        json!({ "timeZone": "Not a zone!" }),
        json!({ "start": "2026-10-08 09:00" }),
        json!({ "end": "2026-10-08T08:00:00-07:00" }),
        json!({ "title": "  " }),
    ] {
        let r = create_with(&fake2, &inbox(), &create_body(bad.clone()));
        assert_eq!(code_of(&r), "usage", "{bad}: {}", r.body);
    }
    assert_no_set(&fake2);
}

#[test]
fn update_and_delete_refuse_a_stored_event_with_other_participants() {
    let mut stored = plain_event("e-inv");
    stored["participants"] = json!({
        "o": { "calendarAddress": format!("mailto:{OWNER}"), "roles": { "owner": true } },
        "g": { "calendarAddress": "mailto:guest@example.org", "roles": { "attendee": true } }
    });
    let mut organized_elsewhere = plain_event("e-org");
    organized_elsewhere["organizerCalendarAddress"] = json!("mailto:boss@example.org");
    let fake = Fake {
        events: HashMap::from([
            ("e-inv".to_string(), stored),
            ("e-org".to_string(), organized_elsewhere),
        ]),
        ..Default::default()
    };
    // The REQUEST has no participants — the stored event decides.
    let b = write_body(json!({ "project": "x", "id": "ev_t", "title": "renamed" }));
    for id in ["e-inv", "e-org"] {
        let r = update_with(&fake, &inbox(), id, "ev_t", &b);
        assert!(r.status.starts_with("409"), "{id}: {}", r.body);
        assert_eq!(code_of(&r), "invites_need_send_level");
        let r = delete_with(&fake, &inbox(), id, "ev_t");
        assert_eq!(code_of(&r), "invites_need_send_level", "{id}: delete would skip the CANCEL");
    }
    assert_no_set(&fake);
}

#[test]
fn update_patches_only_given_fields_and_keeps_duration_on_a_new_start() {
    let fake = Fake {
        events: HashMap::from([("e1".to_string(), plain_event("e1"))]),
        ..Default::default()
    };
    let b = write_body(json!({ "project": "x", "id": "ev_t", "start": "2026-10-09T10:00:00Z",
                               "location": "" }));
    let r = update_with(&fake, &inbox(), "e1", "ev_t", &b);
    assert!(r.status.starts_with("200"), "{}", r.body);
    let args = &fake.calls_of("CalendarEvent/set")[0].1;
    assert_eq!(args["sendSchedulingMessages"], false);
    let patch = &args["update"]["e1"];
    assert_eq!(patch["start"], "2026-10-09T10:00:00");
    assert_eq!(patch["duration"], "PT30M", "duration kept");
    assert_eq!(patch["locations"], Value::Null, "empty location clears");
    assert!(patch.get("title").is_none(), "untouched fields stay out of the patch");
    // An email alert in an update is refused too.
    let b = write_body(json!({ "project": "x", "id": "ev_t",
                               "alerts": { "x": { "action": "email" } } }));
    assert_eq!(
        code_of(&update_with(&fake, &inbox(), "e1", "ev_t", &b)),
        "email_alerts_need_send_level"
    );
}

#[test]
fn delete_of_an_owner_only_event_destroys_without_scheduling() {
    let fake = Fake {
        events: HashMap::from([("e1".to_string(), plain_event("e1"))]),
        set_reply: Some(json!({ "destroyed": ["e1"] })),
        ..Default::default()
    };
    let r = delete_with(&fake, &inbox(), "e1", "ev_t");
    assert!(r.status.starts_with("200"), "{}", r.body);
    let args = &fake.calls_of("CalendarEvent/set")[0].1;
    assert_eq!(args["destroy"], json!(["e1"]));
    assert_eq!(args["sendSchedulingMessages"], false);
}

#[test]
fn every_write_body_rejects_an_account_id() {
    // deny_unknown_fields: there is no way to smuggle an accountId.
    let err = serde_json::from_value::<WriteBody>(json!({ "project": "x", "accountId": "acct-evil" }))
        .expect_err("accountId must be refused");
    assert!(err.to_string().contains("accountId"), "{err}");
    assert_eq!(
        code_of(&handle_create(br#"{"project":"x","accountId":"a"}"#)),
        "usage"
    );
}

// ── Reads ───────────────────────────────────────────────────────────

#[test]
fn events_query_uses_offset_free_window_expansion_and_stable_ids() {
    let mut inst = plain_event("syn-7");
    inst["baseEventId"] = json!("master-1");
    inst["recurrenceId"] = json!("2026-10-08T16:00:00");
    let fake = Fake {
        query_ids: vec!["syn-7".into(), "syn-8".into()],
        events: HashMap::from([("syn-7".to_string(), inst)]),
        ..Default::default()
    };
    let p = params(&[("start", "2026-10-08T00:00:00-07:00"), ("end", "2026-10-10"), ("limit", "1")]);
    let r = events_with(&fake, &inbox(), &p, now());
    assert!(r.status.starts_with("200"), "{}", r.body);
    let q = &fake.calls_of("CalendarEvent/query")[0].1;
    // Stalwart drops offsets in after/before: UTC wall time + timeZone.
    assert_eq!(q["filter"]["after"], "2026-10-08T07:00:00");
    assert_eq!(q["filter"]["before"], "2026-10-10T00:00:00");
    assert_eq!(q["timeZone"], "Etc/UTC");
    assert_eq!(q["expandRecurrences"], true);
    assert_eq!(q["limit"], 2, "page + 1 to detect truncation");
    let b = body_of(&r);
    assert_eq!(b["truncated"], true);
    let ev = &b["events"][0];
    assert_eq!(ev["id"], json!(encode_event_id(OWNER, "master-1")), "series id, not synthetic");
    assert_eq!(ev["start"], "2026-10-08T16:00:00Z");
    assert_eq!(ev["end"], "2026-10-08T16:30:00Z");
    assert_eq!(ev["recurring"], true);
    assert_eq!(b["start"], "2026-10-08T07:00:00Z");
}

#[test]
fn window_defaults_caps_and_errors() {
    let (s, e) = must(window(&params(&[]), now()), "default");
    assert_eq!((s, e - s), (now(), Duration::days(7)));
    for (p, want) in [
        (vec![("start", "2026-10-08"), ("end", "2026-10-08")], "after"),
        (vec![("start", "2026-01-01"), ("end", "2027-06-01")], "366"),
        (vec![("start", "yesterday")], "RFC 3339"),
    ] {
        let r = window(&params(&p), now()).expect_err("bad window");
        assert_eq!(code_of(&r), "usage");
        assert!(r.body.contains(want), "{p:?}: {}", r.body);
    }
}

/// CAL18: the identity stamp overwrites `from` (and project) on every
/// scoped GET — over TCP (dispatcher) and over the cell UDS (which
/// adds `cell_session_id` then calls the same stamp). The calendar
/// window rides `start`/`end`, so it must survive both untouched.
#[test]
fn window_survives_the_identity_stamp_over_tcp_and_cell_uds() {
    let principal = crate::session_token::HookPrincipal {
        workspace_uuid: uuid::Uuid::new_v4().to_string(),
        agent_address: "agent".to_string(),
    };
    let client = params(&[
        ("start", "2026-10-08T00:00:00Z"),
        ("end", "2026-10-09T00:00:00Z"),
        ("from", "2026-01-01T00:00:00Z"),
        ("project", "/tmp/whatever"),
    ]);
    // TCP: the dispatcher restores `from` ONLY for messages/wait.
    let mut tcp = client.clone();
    crate::caller_workspace::stamp_principal(&mut tcp, &principal);
    // Cell UDS: cell_session_id first, then the same stamp.
    let mut uds = client.clone();
    uds.insert("cell_session_id".into(), "sess-1".into());
    crate::caller_workspace::stamp_principal(&mut uds, &principal);
    for (label, p) in [("tcp", &tcp), ("uds", &uds)] {
        assert_ne!(
            p.get("from").map(String::as_str),
            Some("2026-01-01T00:00:00Z"),
            "{label}: the stamp clobbers `from` — why the wire uses start/end"
        );
        let fake = Fake::default();
        let r = events_with(&fake, &inbox(), p, now());
        assert!(r.status.starts_with("200"), "{label}: {}", r.body);
        let q = &fake.calls_of("CalendarEvent/query")[0].1;
        assert_eq!(q["filter"]["after"], "2026-10-08T00:00:00", "{label}");
        assert_eq!(q["filter"]["before"], "2026-10-09T00:00:00", "{label}");
    }
}

#[test]
fn list_shapes_calendars() {
    let fake = Fake {
        calendars: vec![json!({ "id": "c1", "name": "Main", "isDefault": true, "color": "#0a0" })],
        ..Default::default()
    };
    let b = body_of(&list_with(&fake, &inbox()));
    assert_eq!(b["calendars"][0], json!({
        "id": "c1", "name": "Main", "description": null, "color": "#0a0",
        "isDefault": true, "timeZone": null,
        "owner": OWNER, "shared": false,
        "rights": { "freeBusy": true, "read": true, "write": true, "delete": true },
    }));
    assert_eq!(fake.calls_of("Calendar/get")[0].0, "acct-1");
}

#[test]
fn show_wraps_full_event_with_participants() {
    let mut ev = plain_event("e1");
    ev["description"] = json!("notes");
    ev["participants"] = json!({ "p": { "calendarAddress": "mailto:Guest@Example.org",
                                        "participationStatus": "accepted" } });
    let fake = Fake { events: HashMap::from([("e1".to_string(), ev)]), ..Default::default() };
    let b = body_of(&show_with(&fake, &inbox(), "e1", "ev_t"));
    assert_eq!(b["event"]["participantList"][0]["email"], "guest@example.org");
    assert_eq!(b["event"]["description"], "notes");
    let props = &fake.calls_of("CalendarEvent/get")[0].1["properties"];
    assert!(props.as_array().expect("props").contains(&json!("utcStart")));
}

#[test]
fn freebusy_prefers_server_availability_and_merges() {
    let fake = Fake {
        availability: Some(Ok(vec![
            json!({ "utcStart": "2026-10-08T09:00:00Z", "utcEnd": "2026-10-08T10:00:00Z", "busyStatus": "tentative" }),
            json!({ "utcStart": "2026-10-08T09:30:00Z", "utcEnd": "2026-10-08T11:00:00Z", "busyStatus": "confirmed" }),
            json!({ "utcStart": "2026-10-07T23:00:00Z", "utcEnd": "2026-10-08T01:00:00Z", "busyStatus": "unavailable" }),
        ])),
        ..Default::default()
    };
    let p = params(&[("start", "2026-10-08"), ("end", "2026-10-09")]);
    let b = body_of(&freebusy_with(&fake, &inbox(), &p, now()));
    assert_eq!(b["source"], "availability");
    assert_eq!(b["busy"], json!([
        { "start": "2026-10-08T00:00:00Z", "end": "2026-10-08T01:00:00Z", "status": "unavailable" },
        { "start": "2026-10-08T09:00:00Z", "end": "2026-10-08T11:00:00Z", "status": "busy" },
    ]));
    assert!(fake.calls_of("CalendarEvent/query").is_empty());
}

#[test]
fn freebusy_falls_back_to_events_skipping_free_cancelled_secret() {
    let mut free = plain_event("f");
    free["freeBusyStatus"] = json!("free");
    let mut cancelled = plain_event("c");
    cancelled["status"] = json!("cancelled");
    let mut secret = plain_event("s");
    secret["privacy"] = json!("secret");
    let fake = Fake {
        availability: Some(Err("Principal/getAvailability: JMAP error 'forbidden'".into())),
        query_ids: vec!["b".into(), "f".into(), "c".into(), "s".into()],
        events: HashMap::from([
            ("b".to_string(), plain_event("b")),
            ("f".to_string(), free),
            ("c".to_string(), cancelled),
            ("s".to_string(), secret),
        ]),
        ..Default::default()
    };
    let p = params(&[("start", "2026-10-08"), ("end", "2026-10-09")]);
    let b = body_of(&freebusy_with(&fake, &inbox(), &p, now()));
    assert_eq!(b["source"], "events");
    assert_eq!(b["busy"], json!([
        { "start": "2026-10-08T16:00:00Z", "end": "2026-10-08T16:30:00Z", "status": "busy" }
    ]));
    assert!(!b.to_string().contains("Prep"), "titles never leave freebusy");
}

#[test]
fn too_many_expansions_is_a_usage_error() {
    let fake = Fake {
        query_err: Some("CalendarEvent/query: JMAP error 'invalidArguments': exceeds the server limit".into()),
        ..Default::default()
    };
    let r = events_with(&fake, &inbox(), &params(&[]), now());
    assert_eq!(code_of(&r), "usage");
}

// ── wait ────────────────────────────────────────────────────────────

#[test]
fn wait_times_out_with_the_state_and_reports_changes() {
    let fake = Fake { state: "s0".into(), ..Default::default() };
    // A fake clock the fake sleep advances.
    let clock = std::cell::Cell::new(0u64);
    let mut elapsed = || clock.get();
    let mut sleep = |d: std::time::Duration| clock.set(clock.get() + d.as_secs());
    let r = wait_with(&fake, &inbox(), None, 5, &mut elapsed, &mut sleep);
    let b = body_of(&r);
    assert_eq!(b["timedOut"], true);
    assert_eq!(b["state"], "s0", "state read first when none given");
    assert_eq!(fake.calls_of("CalendarEvent/get")[0].1["ids"], json!([]));
    assert!(clock.get() <= 5, "never sleeps past the budget: {}", clock.get());
    assert!(fake.calls_of("CalendarEvent/changes").len() >= 2, "it polled");

    let fake = Fake::default();
    fake.changes.borrow_mut().push_back(Ok(json!({
        "newState": "s1", "created": ["n1"], "updated": [], "destroyed": [], "hasMoreChanges": true
    })));
    fake.changes.borrow_mut().push_back(Ok(json!({
        "newState": "s2", "created": [], "updated": ["u1"], "destroyed": ["d1"]
    })));
    let mut elapsed = || 0u64;
    let mut sleep = |_d: std::time::Duration| {};
    let b = body_of(&wait_with(&fake, &inbox(), Some("s0"), 30, &mut elapsed, &mut sleep));
    assert_eq!(b["timedOut"], false);
    assert_eq!(b["state"], "s2");
    assert_eq!(b["created"], json!([encode_event_id(OWNER, "n1")]));
    assert_eq!(b["updated"], json!([encode_event_id(OWNER, "u1")]));
    assert_eq!(b["destroyed"], json!([encode_event_id(OWNER, "d1")]));
    assert_eq!(fake.calls_of("CalendarEvent/changes")[0].1["sinceState"], "s0");

    let fake = Fake::default();
    fake.changes.borrow_mut().push_back(Err(
        "CalendarEvent/changes: JMAP error 'cannotCalculateChanges': gone".into(),
    ));
    let r = wait_with(&fake, &inbox(), Some("old"), 30, &mut elapsed, &mut sleep);
    assert!(r.status.starts_with("409"));
    assert_eq!(code_of(&r), "state_expired");
}

#[test]
fn wait_timeout_bounds() {
    assert_eq!(must(parse_wait_timeout(&params(&[])), "default"), 300);
    assert_eq!(must(parse_wait_timeout(&params(&[("timeout", "900")])), "max"), 900);
    for bad in ["0", "901", "x"] {
        let r = parse_wait_timeout(&params(&[("timeout", bad)])).expect_err(bad);
        assert_eq!(code_of(&r), "usage", "{bad}");
    }
}

// ── Owner switch + routing ──────────────────────────────────────────

#[test]
fn calendars_disabled_policy_answers_409_on_every_verb() {
    let _g = crate::mail::mail_server_test_lock();
    let (p, path) = insert_project("dis");
    let a = addr("dis");
    seed_hosted(&p, &a, "send", Some("acct-dis"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", []).expect("clear");
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, hostname, dav_policy_json, updated_at) \
                 VALUES (1, 'running', '0.16.20', 'mail.example.com', \
                 '{\"calendars\":\"off\",\"files\":\"off\",\"appliedAt\":1,\"backfilledAt\":1}', 1)",
            [],
        )
        .expect("seed policy");
    }
    let q = params(&[("project", &path), ("id", &encode_event_id(&a, "e1"))]);
    for (label, r) in [
        ("list", handle_list(&q)),
        ("events", handle_events(&q)),
        ("show", handle_show(&q)),
        ("freebusy", handle_freebusy(&q)),
        ("wait", handle_wait(&q)),
    ] {
        assert!(r.status.starts_with("409"), "{label}: {} {}", r.status, r.body);
        assert_eq!(code_of(&r), "calendars_disabled", "{label}");
        assert!(r.body.contains("k2 hostmail calendar enable"), "{label}");
    }
    let body = json!({ "project": path, "title": "t", "start": "2026-10-08T09:00:00Z",
                       "end": "2026-10-08T10:00:00Z" })
    .to_string();
    assert_eq!(code_of(&handle_create(body.as_bytes())), "calendars_disabled");
    let body = json!({ "project": path, "id": encode_event_id(&a, "e1") }).to_string();
    assert_eq!(code_of(&handle_delete(body.as_bytes())), "calendars_disabled");
    assert_eq!(code_of(&handle_update(body.as_bytes())), "calendars_disabled");
    // Calendars on (or never applied) → past the switch; with the
    // server row gone the engine answers 503 not_ready.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", []).expect("clear");
    }
    let r = handle_list(&q);
    assert!(r.status.starts_with("503"), "{} {}", r.status, r.body);
    cleanup(&[&p]);
}

#[test]
fn routes_are_registered_get_post_and_405() {
    let empty = HashMap::new();
    for p in ["/cli/mail/calendar/create", "/cli/mail/calendar/update", "/cli/mail/calendar/delete"] {
        let r = crate::mail_routes::dispatch(p, &empty).expect("mail family");
        assert!(r.status.starts_with("405"), "GET {p} must 405, got {}", r.status);
        assert!(crate::routes::route_policy::post_allowed(p), "{p} on the POST allowlist");
        assert!(!crate::mail_routes::is_mail_manage_surface(p), "{p} is an agent verb");
        assert!(!crate::mail_routes::is_owner_level_mutation(p), "{p} is an agent verb");
        assert!(crate::session_token::is_agent_verb(p), "{p} rides the /cli/mail/ agent prefix");
    }
    for p in [
        "/cli/mail/calendar/list",
        "/cli/mail/calendar/events",
        "/cli/mail/calendar/show",
        "/cli/mail/calendar/freebusy",
        "/cli/mail/calendar/wait",
    ] {
        let r = crate::mail_routes::dispatch(p, &empty).expect("mail family");
        // Served (not the family 404/405): no identity → usage.
        assert_eq!(code_of(&r), "usage", "GET {p} reaches its handler: {} {}", r.status, r.body);
        assert!(!crate::routes::route_policy::post_allowed(p), "{p} is GET-only");
        assert!(!crate::mail_routes::is_mail_manage_surface(p));
        assert!(!crate::mail_routes::is_owner_level_mutation(p));
        assert!(crate::session_token::is_agent_verb(p));
    }
    // POST dispatch reaches the handlers (bad JSON → usage, not 404).
    for p in ["/cli/mail/calendar/create", "/cli/mail/calendar/update", "/cli/mail/calendar/delete"] {
        let r = crate::mail_routes::dispatch_post(p, b"not json");
        assert_eq!(code_of(&r), "usage", "{p}: {}", r.body);
    }
}

// ── Pure helpers ────────────────────────────────────────────────────

#[test]
fn durations_round_trip() {
    for (s, secs) in [("PT30M", 1800), ("PT1H30M", 5400), ("P1D", 86_400), ("P1DT2H", 93_600),
                      ("P2W", 1_209_600), ("PT0S", 0), ("PT45S", 45)] {
        assert_eq!(parse_duration_secs(s), Some(secs), "{s}");
    }
    for bad in ["", "P", "PT", "1H", "PT1X", "P1H"] {
        assert_eq!(parse_duration_secs(bad), None, "{bad}");
    }
    assert_eq!(format_duration(1800), "PT30M");
    assert_eq!(format_duration(93_600), "P1DT2H");
    assert_eq!(format_duration(86_400), "P1D");
    assert_eq!(format_duration(0), "PT0S");
}

#[test]
fn participant_owner_matching_is_case_and_mailto_insensitive() {
    let parts = json!({
        "a": { "calendarAddress": "MAILTO:Cal-Owner@Example.com" },
        "b": { "sendTo": { "imip": "mailto:x@example.org" } },
        "c": { "name": "no address" }
    });
    assert_eq!(
        non_owner_participants(Some(&parts), OWNER),
        vec!["x@example.org".to_string(), "(participant without an address)".to_string()]
    );
    assert!(non_owner_participants(None, OWNER).is_empty());
    assert!(has_email_alert(Some(&json!([{ "action": "EMAIL" }]))));
    assert!(!has_email_alert(Some(&json!({ "a": { "action": "display" } }))));
}

// ── S4: calendars shared TO the inbox ───────────────────────────────

const BOSS: &str = "boss@example.com";

fn peer(address: &str, account: &str) -> HostedAddr {
    HostedAddr { address: address.to_string(), account_id: account.to_string() }
}

fn shared_inbox(level: Level) -> CalInbox {
    CalInbox {
        address: OWNER.to_string(),
        account_id: "acct-1".to_string(),
        level,
        peers: vec![peer(BOSS, "acct-b"), peer("other@example.com", "acct-x")],
    }
}

fn share(level: &str) -> Value {
    crate::mail::calendar_share::ShareLevel::parse(level).expect("level").rights()
}

/// boss@ (acct-b) shares: c-r read to the inbox, c-w write to group
/// g-team (the inbox is a member), c-e editor to the inbox, c-fb
/// free/busy to the inbox; c-priv is not shared and c-other is shared
/// with someone else. other@ (acct-x) shares nothing with the inbox.
fn shared_fake() -> Fake {
    let cal = |id: &str, name: &str, sw: Value| json!({ "id": id, "name": name, "shareWith": sw });
    let mut f = Fake {
        calendars: vec![json!({ "id": "own-1", "name": "Mine", "isDefault": true })],
        groups: vec!["g-team".to_string()],
        ..Default::default()
    };
    f.shared.insert(
        "acct-b".to_string(),
        vec![
            cal("c-r", "Boss read", json!({ "acct-1": share("read") })),
            cal("c-w", "Team", json!({ "g-team": share("write") })),
            cal("c-e", "Boss editor", json!({ "acct-1": share("editor") })),
            cal("c-fb", "Boss busy", json!({ "acct-1": share("freebusy") })),
            cal("c-priv", "Private", Value::Null),
            cal("c-other", "Someone else's", json!({ "acct-z": share("read") })),
        ],
    );
    f.shared.insert(
        "acct-x".to_string(),
        vec![cal("x-1", "X", json!({ "acct-q": share("editor") }))],
    );
    f
}

fn boss_event(id: &str, cal: &str) -> Value {
    let mut e = plain_event(id);
    e["calendarIds"] = json!({ cal: true });
    e
}

fn with_boss_events(f: &mut Fake, evs: &[(&str, &str)]) {
    let m = evs.iter().map(|(i, c)| (i.to_string(), boss_event(i, c))).collect();
    f.acct_events.insert("acct-b".to_string(), m);
}

#[test]
fn shared_event_tokens_round_trip_and_stay_apart_from_own_ids() {
    let own = encode_event_id(OWNER, "e1");
    assert_eq!(decode_event_ref(&own), Some((OWNER.into(), "e1".into(), None)));
    let sh = encode_shared_event_id(OWNER, "e1", BOSS);
    assert_eq!(decode_event_ref(&sh), Some((OWNER.into(), "e1".into(), Some(BOSS.into()))));
    assert_eq!(decode_event_id(&sh), None, "an S3 decoder never mistakes a shared id");
    let selfish = encode_shared_event_id(OWNER, "e1", OWNER);
    assert_eq!(decode_event_ref(&selfish), Some((OWNER.into(), "e1".into(), None)));
    let four = format!("ev_{}", B64URL.encode("a\nb\nc\nd"));
    assert_eq!(decode_event_ref(&four), None);
}

#[test]
fn list_shows_own_and_shared_calendars_with_effective_rights() {
    let f = shared_fake();
    let b = body_of(&list_with(&f, &shared_inbox(Level::Send)));
    let cals = b["calendars"].as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = cals.iter().filter_map(|c| c["id"].as_str()).collect();
    assert_eq!(
        ids,
        vec!["own-1", "boss@example.com/c-r", "boss@example.com/c-w", "boss@example.com/c-e"],
        "free/busy-only, private and other people's shares are not listed"
    );
    let rights = |id: &str| {
        cals.iter().find(|c| c["id"] == id).map(|c| c["rights"].clone()).unwrap_or(Value::Null)
    };
    assert_eq!(rights("own-1"), json!({ "freeBusy": true, "read": true, "write": true, "delete": true }));
    assert_eq!(
        rights("boss@example.com/c-r"),
        json!({ "freeBusy": true, "read": true, "write": false, "delete": false })
    );
    assert_eq!(
        rights("boss@example.com/c-w"),
        json!({ "freeBusy": true, "read": true, "write": true, "delete": false }),
        "group-mediated write share"
    );
    assert_eq!(
        rights("boss@example.com/c-e"),
        json!({ "freeBusy": true, "read": true, "write": true, "delete": true })
    );
    let team = cals.iter().find(|c| c["id"] == "boss@example.com/c-w").cloned().unwrap_or_default();
    assert_eq!(team["owner"], BOSS);
    assert_eq!(team["shared"], true);
    assert_eq!(team["name"], "Team");
    assert_eq!(cals[0]["owner"], OWNER);
    assert_eq!(cals[0]["shared"], false);

    // A read grant caps every share (and the own calendars) at read.
    let b = body_of(&list_with(&f, &shared_inbox(Level::Read)));
    for c in b["calendars"].as_array().cloned().unwrap_or_default() {
        assert_eq!(c["rights"]["write"], false, "{c}");
        assert_eq!(c["rights"]["delete"], false, "{c}");
    }
    // Not in the group → the group share is gone.
    let mut f = shared_fake();
    f.groups.clear();
    let b = body_of(&list_with(&f, &shared_inbox(Level::Send)));
    assert!(!b.to_string().contains("c-w"), "{b}");
    // No peers → S3 behaviour, no extra calls.
    let f = shared_fake();
    let _ = list_with(&f, &inbox());
    assert!(f.calls_of("Calendar/get+shareWith").is_empty());
    assert!(f.calls_of("x:Account/get memberGroupIds").is_empty());
}

#[test]
fn events_merge_own_and_readable_shared_calendars_only() {
    let mut f = shared_fake();
    f.query_ids = vec!["o1".to_string()];
    f.events = HashMap::from([("o1".to_string(), plain_event("o1"))]);
    f.acct_query.insert("acct-b".to_string(), vec!["b1".to_string()]);
    with_boss_events(&mut f, &[("b1", "c-r")]);
    let b = body_of(&events_with(&f, &shared_inbox(Level::Read), &params(&[]), now()));
    let targets: Vec<(String, Value)> = f
        .calls_of("CalendarEvent/query")
        .into_iter()
        .map(|(a, q)| (a, q["filter"]["inCalendar"].clone()))
        .collect();
    assert_eq!(
        targets,
        vec![
            ("acct-1".to_string(), Value::Null),
            ("acct-b".to_string(), json!("c-r")),
            ("acct-b".to_string(), json!("c-w")),
            ("acct-b".to_string(), json!("c-e")),
        ],
        "own account + each readable shared calendar; never c-fb, c-priv or acct-x"
    );
    let evs = b["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(evs.len(), 2, "own + one shared, deduplicated: {b}");
    let sh = evs.iter().find(|e| e["shared"] == true).cloned().unwrap_or_default();
    assert_eq!(sh["owner"], BOSS);
    assert_eq!(sh["calendarIds"], json!(["boss@example.com/c-r"]));
    assert_eq!(
        decode_event_ref(sh["id"].as_str().unwrap_or("")),
        Some((OWNER.to_string(), "b1".to_string(), Some(BOSS.to_string())))
    );

    // --calendar: a free/busy-only share is not visible; a shared one by name is.
    let mut f = shared_fake();
    f.acct_query.insert("acct-b".to_string(), Vec::new());
    let r = events_with(
        &f,
        &shared_inbox(Level::Read),
        &params(&[("calendar", "boss@example.com/c-fb")]),
        now(),
    );
    assert_eq!(code_of(&r), "usage", "{}", r.body);
    let r = events_with(&f, &shared_inbox(Level::Read), &params(&[("calendar", "Team")]), now());
    assert!(r.status.starts_with("200"), "{}", r.body);
    assert_eq!(body_of(&r)["calendar"], "boss@example.com/c-w");
    let last = f.calls_of("CalendarEvent/query").pop().map(|(a, q)| (a, q["filter"]["inCalendar"].clone()));
    assert_eq!(last, Some(("acct-b".to_string(), json!("c-w"))));
}

#[test]
fn show_of_a_shared_event_needs_a_readable_calendar() {
    let mut f = shared_fake();
    with_boss_events(&mut f, &[("b-r", "c-r"), ("b-priv", "c-priv"), ("b-fb", "c-fb")]);
    let inbox = shared_inbox(Level::Read);
    let boss = peer(BOSS, "acct-b");
    let r = show_shared_with(&f, &inbox, &boss, "b-r", "ev_t");
    assert!(r.status.starts_with("200"), "{}", r.body);
    assert_eq!(body_of(&r)["event"]["owner"], BOSS);
    for id in ["b-priv", "b-fb", "nope"] {
        let r = show_shared_with(&f, &inbox, &boss, id, "ev_t");
        assert!(r.status.starts_with("404"), "{id}: {}", r.body);
    }
    // Nothing shared from that owner: masked without reading its events.
    let r = show_shared_with(&f, &inbox, &peer("other@example.com", "acct-x"), "x-ev", "ev_t");
    assert!(r.status.starts_with("404"));
    assert!(!f.calls_of("CalendarEvent/get").iter().any(|(a, _)| a == "acct-x"));
}

#[test]
fn shared_writes_need_the_share_right_and_the_draft_grant() {
    let boss = peer(BOSS, "acct-b");
    let body = write_body(json!({ "project": "x", "id": "ev_t", "title": "Moved" }));
    // (calendar, grant level, update allowed, delete allowed)
    for (cal, level, upd, del) in [
        ("c-r", Level::Draft, false, false),
        ("c-w", Level::Draft, true, false),
        ("c-e", Level::Draft, true, true),
        ("c-w", Level::Read, false, false),
        ("c-e", Level::Read, false, false),
    ] {
        let inbox = shared_inbox(level);
        let mut f = shared_fake();
        with_boss_events(&mut f, &[("ev1", cal)]);
        let r = update_at(&f, &inbox, Some(&boss), "ev1", "ev_t", &body);
        if upd {
            assert!(r.status.starts_with("200"), "update {cal} {level:?}: {}", r.body);
            let sets = f.calls_of("CalendarEvent/set");
            assert_eq!(sets.len(), 1);
            assert_eq!(sets[0].0, "acct-b", "the write goes to the owner's account");
            assert_eq!(sets[0].1["sendSchedulingMessages"], false);
        } else {
            assert_eq!(code_of(&r), "share_rights", "update {cal} {level:?}: {}", r.body);
            assert_no_set(&f);
        }
        let mut f = shared_fake();
        with_boss_events(&mut f, &[("ev1", cal)]);
        let r = delete_at(&f, &inbox, Some(&boss), "ev1", "ev_t");
        if del {
            assert!(r.status.starts_with("200"), "delete {cal} {level:?}: {}", r.body);
            let sets = f.calls_of("CalendarEvent/set");
            assert_eq!(sets[0].0, "acct-b");
            assert_eq!(sets[0].1["destroy"], json!(["ev1"]));
        } else {
            assert_eq!(code_of(&r), "share_rights", "delete {cal} {level:?}: {}", r.body);
            assert_no_set(&f);
        }
    }
    // Not on a calendar shared to the inbox: the masked 404.
    let mut f = shared_fake();
    with_boss_events(&mut f, &[("ev1", "c-priv")]);
    let r = update_at(&f, &shared_inbox(Level::Send), Some(&boss), "ev1", "ev_t", &body);
    assert!(r.status.starts_with("404"), "{}", r.body);
    assert_no_set(&f);
    // A participant other than the calendar's owner still needs send.
    let mut f = shared_fake();
    let mut ev = boss_event("ev1", "c-e");
    ev["participants"] = json!({ "p": { "calendarAddress": "mailto:guest@example.org" } });
    f.acct_events.insert("acct-b".to_string(), HashMap::from([("ev1".to_string(), ev)]));
    let r = delete_at(&f, &shared_inbox(Level::Send), Some(&boss), "ev1", "ev_t");
    assert_eq!(code_of(&r), "invites_need_send_level", "{}", r.body);
    assert_no_set(&f);
}

#[test]
fn create_on_a_shared_calendar_lands_on_the_owners_account() {
    let f = Fake {
        set_reply: Some(json!({ "created": { "k2new": { "id": "new1" } } })),
        ..shared_fake()
    };
    let r = create_with(&f, &shared_inbox(Level::Draft), &create_body(json!({ "calendar": "Team" })));
    assert!(r.status.starts_with("200"), "{}", r.body);
    let sets = f.calls_of("CalendarEvent/set");
    assert_eq!(sets[0].0, "acct-b");
    assert_eq!(sets[0].1["create"]["k2new"]["calendarIds"], json!({ "c-w": true }));
    assert_eq!(sets[0].1["sendSchedulingMessages"], false);
    let b = body_of(&r);
    assert_eq!(b["calendarId"], "boss@example.com/c-w");
    assert_eq!(b["owner"], BOSS);
    assert_eq!(
        decode_event_ref(b["id"].as_str().unwrap_or("")),
        Some((OWNER.to_string(), "new1".to_string(), Some(BOSS.to_string())))
    );
    // A read-only share refuses before any write.
    let f = shared_fake();
    let r = create_with(
        &f,
        &shared_inbox(Level::Draft),
        &create_body(json!({ "calendar": "boss@example.com/c-r" })),
    );
    assert_eq!(code_of(&r), "share_rights", "{}", r.body);
    assert_no_set(&f);
    // On someone else's calendar, even the inbox itself is an invitee.
    let f = shared_fake();
    let r = create_with(
        &f,
        &shared_inbox(Level::Draft),
        &create_body(json!({ "calendar": "Team", "participants": [OWNER] })),
    );
    assert_eq!(code_of(&r), "invites_need_send_level", "{}", r.body);
    assert_no_set(&f);
}

#[test]
fn freebusy_unions_own_and_every_calendar_shared_with_free_busy() {
    let mut f = shared_fake();
    f.availability = Some(Ok(vec![json!({
        "utcStart": "2026-10-08T09:00:00Z", "utcEnd": "2026-10-08T10:00:00Z", "busyStatus": "confirmed"
    })]));
    f.acct_query.insert("acct-b".to_string(), vec!["fb1".to_string()]);
    let mut ev = boss_event("fb1", "c-fb");
    ev["utcStart"] = json!("2026-10-08T14:00:00Z");
    ev["utcEnd"] = json!("2026-10-08T15:00:00Z");
    ev["title"] = json!("Secret board meeting");
    f.acct_events.insert("acct-b".to_string(), HashMap::from([("fb1".to_string(), ev)]));
    let p = params(&[("start", "2026-10-08"), ("end", "2026-10-09")]);
    let b = body_of(&freebusy_with(&f, &shared_inbox(Level::Read), &p, now()));
    assert_eq!(b["source"], "availability+shared");
    assert_eq!(b["sharedCalendars"], 4, "c-r, c-w, c-e and c-fb all allow free/busy");
    assert_eq!(b["busy"], json!([
        { "start": "2026-10-08T09:00:00Z", "end": "2026-10-08T10:00:00Z", "status": "busy" },
        { "start": "2026-10-08T14:00:00Z", "end": "2026-10-08T15:00:00Z", "status": "busy" },
    ]));
    assert!(!b.to_string().contains("Secret"), "titles never leave freebusy");
    let shared_q: Vec<Value> = f
        .calls_of("CalendarEvent/query")
        .into_iter()
        .filter(|(a, _)| a == "acct-b")
        .map(|(_, q)| q["filter"]["inCalendar"].clone())
        .collect();
    assert!(shared_q.contains(&json!("c-fb")), "{shared_q:?}");
    assert!(!shared_q.contains(&json!("c-priv")) && !shared_q.contains(&json!("c-other")));
}

#[test]
fn wait_watches_shared_owners_and_filters_to_visible_calendars() {
    let mut f = shared_fake();
    f.state = "own0".to_string();
    f.acct_state.insert("acct-b".to_string(), "b0".to_string());
    with_boss_events(&mut f, &[("e-vis", "c-r"), ("e-priv", "c-priv")]);
    f.acct_changes.borrow_mut().insert(
        "acct-b".to_string(),
        VecDeque::from([Ok(json!({
            "newState": "b1", "created": ["e-vis", "e-priv"], "updated": [], "destroyed": ["gone"]
        }))]),
    );
    let mut elapsed = || 0u64;
    let mut sleep = |_d: std::time::Duration| {};
    let b = body_of(&wait_with(&f, &shared_inbox(Level::Read), None, 30, &mut elapsed, &mut sleep));
    assert_eq!(b["timedOut"], false, "{b}");
    assert_eq!(
        b["created"],
        json!([encode_shared_event_id(OWNER, "e-vis", BOSS)]),
        "an event on an unshared calendar never surfaces"
    );
    assert_eq!(b["destroyed"], json!([encode_shared_event_id(OWNER, "gone", BOSS)]));
    let state = b["state"].as_str().unwrap_or("").to_string();
    let m = decode_states(&state).expect("composite state");
    assert_eq!(m.get("acct-1").map(String::as_str), Some("own0"));
    assert_eq!(m.get("acct-b").map(String::as_str), Some("b1"));

    // The next call resumes each account from that state (no re-read).
    let f2 = shared_fake();
    let clock = std::cell::Cell::new(0u64);
    let mut elapsed = || clock.get();
    let mut sleep = |d: std::time::Duration| clock.set(clock.get() + d.as_secs());
    let b = body_of(&wait_with(&f2, &shared_inbox(Level::Read), Some(&state), 3, &mut elapsed, &mut sleep));
    assert_eq!(b["timedOut"], true, "{b}");
    let since: Vec<(String, Value)> = f2
        .calls_of("CalendarEvent/changes")
        .into_iter()
        .map(|(a, v)| (a, v["sinceState"].clone()))
        .take(2)
        .collect();
    assert_eq!(since, vec![("acct-1".to_string(), json!("own0")), ("acct-b".to_string(), json!("b1"))]);
    assert!(f2.calls_of("CalendarEvent/get").is_empty(), "no state re-read");

    // A composite state on an inbox with nothing shared falls back to its own part.
    let f3 = Fake::default();
    let mut elapsed = || 99u64;
    let mut sleep = |_d: std::time::Duration| {};
    let b = body_of(&wait_with(&f3, &inbox(), Some(&state), 3, &mut elapsed, &mut sleep));
    assert_eq!(b["state"], "own0");
    assert_eq!(f3.calls_of("CalendarEvent/changes")[0].1["sinceState"], "own0");
    // A broken composite token is a usage error.
    let r = wait_with(&f3, &inbox(), Some("k2s_!!"), 3, &mut elapsed, &mut sleep);
    assert_eq!(code_of(&r), "usage");
}

#[test]
fn shared_event_ids_resolve_the_owner_from_k2_rows_never_the_client() {
    let (p, _) = insert_project("sh-ev");
    let (q, _) = insert_project("sh-own");
    let (other, _) = insert_project("sh-oth");
    let me = addr("me");
    let boss = addr("boss");
    seed_hosted(&p, &me, "send", Some("acct-me-x"));
    seed_hosted(&q, &boss, "send", Some("acct-boss-x"));
    let tok = encode_shared_event_id(&me, "e1", &boss);
    let (inbox, jid, owner) = must(resolve_event(&p, &tok, Level::Read), "shared");
    assert_eq!((inbox.account_id.as_str(), jid.as_str()), ("acct-me-x", "e1"));
    assert_eq!(
        owner.map(|o| o.account_id),
        Some("acct-boss-x".to_string()),
        "the owner's account comes from its address row"
    );
    // An owner that isn't an active hosted mailbox: masked.
    let tok2 = encode_shared_event_id(&me, "e1", "stranger@example.org");
    assert!(resolve_event(&p, &tok2, Level::Read).expect_err("unknown owner").status.starts_with("404"));
    // Another workspace's inbox as the viewer: masked like S3.
    assert!(resolve_event(&other, &tok, Level::Read).expect_err("foreign").status.starts_with("404"));
    // Write bodies never carry an account.
    assert!(serde_json::from_value::<WriteBody>(json!({ "project": "x", "accountId": "acct-boss-x" })).is_err());
    cleanup(&[&p, &q, &other]);
}
