//! `/cli/sources/triggers/*` handlers (contract §4.2, §4.3 "Triggers").
//! Owner: B2.
//!
//! `crate::sources_routes::handle` has already classified the caller and
//! refused app passes and invalid tokens; both transports refuse a GET to a
//! POST-only path with 405. It calls [`handle`] for: list, show, log,
//! propose, add, edit, approve, pause, resume, remove, test.
//!
//! Who may do what (PRD §6.4): people (Owner/Admin) create, enable, edit,
//! pause, remove and test. Agents (and K2 shell tabs) only **propose**, with
//! the default cooldown and cap, naming themselves; a proposed trigger never
//! fires; an agent may withdraw its own proposal.

use std::collections::HashMap;

use k2_core::sources::trigger_store::{self as ts, InsertErr, NewTrigger, TriggerEdit, TriggerRow};
use k2_core::sources::triggers::{self, TriggerOn, TriggerStatus};
use k2_core::sources::{store, SourceRow, TEST_WAKES_PER_HOUR};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::compute_routes::Who;

use super::approve_routes::util::*;
use super::SourceErr;

pub fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    match path {
        "/cli/sources/triggers/list" => list(who, params),
        "/cli/sources/triggers/show" => show(who, params),
        "/cli/sources/triggers/log" => log(who, params),
        "/cli/sources/triggers/propose" => create(who, body, false),
        "/cli/sources/triggers/add" => {
            need_person(who)?;
            create(who, body, true)
        }
        "/cli/sources/triggers/edit" => {
            need_person(who)?;
            edit(who, body)
        }
        "/cli/sources/triggers/approve" => {
            need_person(who)?;
            set_status(who, body, "approve")
        }
        "/cli/sources/triggers/pause" => {
            need_person(who)?;
            set_status(who, body, "pause")
        }
        "/cli/sources/triggers/resume" => {
            need_person(who)?;
            set_status(who, body, "resume")
        }
        "/cli/sources/triggers/remove" => remove(who, body),
        "/cli/sources/triggers/test" => {
            need_person(who)?;
            test(who, body)
        }
        _ => Err(SourceErr::new("not_found", 404, "no such sources route")),
    }
}

// ── views ────────────────────────────────────────────────────────────

/// `TriggerView` (contract §4.3).
pub fn view(conn: &Connection, t: &TriggerRow, source_name: &str, now: i64) -> Value {
    let ws = crate::compute_routes::ws_by_id(&t.target_project_id);
    let fires_today = ts::fires_since(conn, &t.id, triggers::utc_day_start(now)).unwrap_or(0);
    json!({
        "id": t.id,
        "source": source_name,
        "on": t.on,
        "agent": {
            "projectId": t.target_project_id,
            "handle": ws.as_ref().map_or_else(|| t.target_project_id.clone(), |w| w.handle.clone()),
            "missing": ws.is_none(),
        },
        "note": t.note,
        "cooldownSecs": t.cooldown_secs,
        "dailyCap": t.daily_cap,
        "status": t.status,
        "madeBy": t.made_by,
        "approvedBy": t.approved_by,
        "approvedAt": t.approved_at,
        "lastFireAt": t.last_fire_at,
        "firesToday": fires_today,
        "createdAt": t.created_at,
    })
}

/// The trigger and its (readable) source.
fn trigger_and_source(conn: &Connection, who: &Who, id: &str) -> Result<(TriggerRow, SourceRow), SourceErr> {
    let t = ts::get(conn, id)
        .map_err(db_err)?
        .ok_or_else(|| SourceErr::new("trigger_not_found", 404, format!("no trigger {id}")))?;
    let src = store::get_by_id(conn, &t.source_id)
        .map_err(db_err)?
        .ok_or_else(|| SourceErr::new("trigger_not_found", 404, format!("no trigger {id}")))?;
    check_viewers(who, &src)?;
    Ok((t, src))
}

fn list(who: &Who, params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let now = now();
    let mut out = Vec::new();
    match params.get("source").filter(|s| !s.is_empty()) {
        Some(name) => {
            let src = source_by_name(&conn, name)?;
            check_viewers(who, &src)?;
            for t in ts::list(&conn, Some(&src.id)).map_err(db_err)? {
                out.push(view(&conn, &t, &src.name, now));
            }
        }
        None => {
            let sources: HashMap<String, SourceRow> =
                store::list_live(&conn).map_err(db_err)?.into_iter().map(|r| (r.id.clone(), r)).collect();
            for t in ts::list(&conn, None).map_err(db_err)? {
                let Some(src) = sources.get(&t.source_id) else { continue };
                if can_read(who, src) {
                    out.push(view(&conn, &t, &src.name, now));
                }
            }
        }
    }
    Ok(json!({"triggers": out}))
}

fn show(who: &Who, params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    let id = param(params, "id")?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (t, src) = trigger_and_source(&conn, who, id)?;
    let fires = ts::list_fires(&conn, &t.id, 20).map_err(db_err)?;
    Ok(json!({"trigger": view(&conn, &t, &src.name, now()), "fires": fires}))
}

fn log(who: &Who, params: &HashMap<String, String>) -> Result<Value, SourceErr> {
    let id = param(params, "id")?;
    let limit = match params.get("limit").filter(|s| !s.is_empty()) {
        Some(l) => l.parse::<usize>().map_err(|_| bad_request("limit is a number"))?.clamp(1, 200),
        None => 50,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (t, _) = trigger_and_source(&conn, who, id)?;
    Ok(json!({"fires": ts::list_fires(&conn, &t.id, limit).map_err(db_err)?}))
}

// ── create / edit ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateBody {
    source: String,
    on: TriggerOn,
    /// Name, handle, path or project id (the `k2 msg` resolver). Agents:
    /// absent = themselves.
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    cooldown_secs: Option<u32>,
    #[serde(default)]
    daily_cap: Option<u16>,
}

fn check_on(on: &TriggerOn) -> Result<(), SourceErr> {
    if let TriggerOn::StaleFor { secs } = on {
        if !(triggers::MIN_STALE_FOR_SECS..=triggers::MAX_STALE_FOR_SECS).contains(secs) {
            return Err(bad_request("stale_for is 5 minutes to 7 days"));
        }
    }
    Ok(())
}

fn check_note(note: Option<&str>) -> Result<Option<String>, SourceErr> {
    let n = note.map(str::trim).filter(|n| !n.is_empty());
    match n {
        Some(n) if n.chars().count() > triggers::MAX_TRIGGER_NOTE => Err(bad_request("the note is at most 500 characters")),
        Some(n) if n.chars().any(|c| c.is_control() && c != '\n') => Err(bad_request("the note is plain text")),
        other => Ok(other.map(str::to_string)),
    }
}

fn check_caps(cooldown: Option<u32>, cap: Option<u16>) -> Result<(), SourceErr> {
    if cooldown.is_some_and(|c| !(triggers::MIN_COOLDOWN_SECS..=triggers::MAX_COOLDOWN_SECS).contains(&c)) {
        return Err(bad_request("cooldown is 5 minutes to 1 day"));
    }
    if cap.is_some_and(|c| !(1..=triggers::MAX_DAILY_CAP).contains(&c)) {
        return Err(bad_request("the daily cap is 1 to 48"));
    }
    Ok(())
}

/// Resolve the trigger's agent to a project id on this server.
fn resolve_agent(who: &Who, agent: Option<&str>) -> Result<String, SourceErr> {
    let own = own_project(who);
    let token = match (agent.map(str::trim).filter(|a| !a.is_empty()), own) {
        (Some(a), _) => a.to_string(),
        (None, Some(p)) => p.to_string(),
        (None, None) => return Err(bad_request("agent is required (a workspace name, handle, path or id)")),
    };
    if token.contains("::") {
        return Err(SourceErr::new("agent_not_found", 404, "triggers wake agents on this server only"));
    }
    let ws = resolve_ws(&token).ok_or_else(|| SourceErr::new("agent_not_found", 404, format!("no agent {token} on this server")))?;
    if let Some(p) = own {
        if ws != p {
            return Err(SourceErr::new("other_agent", 403, "an agent can only propose triggers that wake itself"));
        }
    }
    Ok(ws)
}

fn resolve_ws(token: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(f) = test_seams::resolver() {
        return f(token);
    }
    crate::compute_routes::ws_by_token(token).map(|w| w.id)
}

fn insert_err(e: InsertErr) -> SourceErr {
    match e {
        InsertErr::Exists => SourceErr::new("trigger_exists", 409, "that source already has this trigger for this agent"),
        InsertErr::Db(m) => db_err(m),
    }
}

fn create(who: &Who, body: &[u8], active: bool) -> Result<Value, SourceErr> {
    let b: CreateBody = parse(body)?;
    if !is_person(who) && (b.cooldown_secs.is_some() || b.daily_cap.is_some()) {
        return Err(SourceErr::new("field_person_only", 403, "only a person sets a trigger's cooldown and daily cap"));
    }
    check_on(&b.on)?;
    check_caps(b.cooldown_secs, b.daily_cap)?;
    let note = check_note(b.note.as_deref())?;
    let project = resolve_agent(who, b.agent.as_deref())?;
    let (made_by, made_by_project) = maker(who);
    let (by, _) = actor(who);
    let now = now();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let src = source_by_name(&conn, &b.source)?;
    check_viewers(who, &src)?;
    let t = ts::insert(
        &conn,
        &NewTrigger {
            source_id: &src.id,
            on: &b.on,
            target_project_id: &project,
            note: note.as_deref(),
            cooldown_secs: b.cooldown_secs.unwrap_or(triggers::DEFAULT_COOLDOWN_SECS),
            daily_cap: b.daily_cap.unwrap_or(triggers::DEFAULT_DAILY_CAP),
            status: if active { TriggerStatus::Active } else { TriggerStatus::Proposed },
            made_by: &made_by,
            made_by_project: made_by_project.as_deref(),
            approved_by: active.then_some(by.as_str()),
            now,
        },
    )
    .map_err(insert_err)?;
    audit(
        &conn,
        who,
        if active { "trigger_add" } else { "trigger_propose" },
        Some(&src.id),
        Some(&t.id),
        json!({"source": src.name, "on": t.on.code(), "agent": project}),
    );
    changed(&conn);
    Ok(json!({"id": t.id, "status": t.status}))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EditSet {
    #[serde(default)]
    on: Option<TriggerOn>,
    #[serde(default)]
    agent: Option<String>,
    /// `null` clears it.
    #[serde(default, deserialize_with = "some_or_null")]
    note: Option<Option<String>>,
    #[serde(default)]
    cooldown_secs: Option<u32>,
    #[serde(default)]
    daily_cap: Option<u16>,
}

fn some_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditBody {
    id: String,
    set: EditSet,
}

fn edit(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: EditBody = parse(body)?;
    if let Some(on) = &b.set.on {
        check_on(on)?;
    }
    check_caps(b.set.cooldown_secs, b.set.daily_cap)?;
    let note = match &b.set.note {
        Some(n) => Some(check_note(n.as_deref())?),
        None => None,
    };
    let project = match b.set.agent.as_deref() {
        Some(a) => Some(resolve_agent(who, Some(a))?),
        None => None,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (t, src) = trigger_and_source(&conn, who, &b.id)?;
    let e = TriggerEdit {
        on: b.set.on.as_ref(),
        target_project_id: project.as_deref(),
        note: note.as_ref().map(|n| n.as_deref()),
        cooldown_secs: b.set.cooldown_secs,
        daily_cap: b.set.daily_cap,
    };
    ts::edit(&conn, &t.id, &e, now()).map_err(insert_err)?;
    // A trigger aimed at a missing workspace becomes deliverable again when
    // a person points it somewhere real.
    if project.is_some() && t.status == TriggerStatus::Undeliverable {
        ts::set_status(&conn, &t.id, TriggerStatus::Active, None, now()).map_err(db_err)?;
    }
    audit(&conn, who, "trigger_edit", Some(&src.id), Some(&t.id), json!({"source": src.name}));
    changed(&conn);
    let after = ts::get(&conn, &t.id).map_err(db_err)?.ok_or_else(|| SourceErr::new("trigger_not_found", 404, "gone"))?;
    Ok(json!({"id": after.id, "status": after.status, "trigger": view(&conn, &after, &src.name, now())}))
}

// ── status changes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdBody {
    id: String,
}

fn set_status(who: &Who, body: &[u8], verb: &str) -> Result<Value, SourceErr> {
    let b: IdBody = parse(body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (t, src) = trigger_and_source(&conn, who, &b.id)?;
    let now = now();
    let (by, _) = actor(who);
    let (to, approved_by, action) = match (verb, t.status) {
        ("approve", TriggerStatus::Proposed) => (TriggerStatus::Active, Some(by.as_str()), "trigger_approve"),
        ("approve", s) => return Err(SourceErr::new("not_proposed", 409, format!("this trigger is {}, not proposed", s.code()))),
        ("pause", TriggerStatus::Active | TriggerStatus::Undeliverable) => (TriggerStatus::Paused, None, "trigger_pause"),
        ("pause", s) => return Err(SourceErr::new("bad_state", 409, format!("this trigger is {}", s.code()))),
        ("resume", TriggerStatus::Paused | TriggerStatus::Undeliverable) => {
            if resolve_ws(&t.target_project_id).is_none() {
                return Err(SourceErr::new("agent_not_found", 404, "its workspace is gone; edit the trigger to wake another agent"));
            }
            (TriggerStatus::Active, None, "trigger_resume")
        }
        ("resume", s) => return Err(SourceErr::new("bad_state", 409, format!("this trigger is {}", s.code()))),
        _ => return Err(bad_request("unknown change")),
    };
    ts::set_status(&conn, &t.id, to, approved_by, now).map_err(db_err)?;
    audit(&conn, who, action, Some(&src.id), Some(&t.id), json!({"source": src.name}));
    changed(&conn);
    Ok(json!({"id": t.id, "status": to}))
}

fn remove(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: IdBody = parse(body)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (t, src) = trigger_and_source(&conn, who, &b.id)?;
    if !is_person(who) {
        let mine = own_project(who).is_some_and(|p| t.made_by_project.as_deref() == Some(p));
        if !mine {
            return Err(SourceErr::new("not_own", 403, "an agent can only withdraw a trigger it proposed"));
        }
        if t.status != TriggerStatus::Proposed {
            return Err(person_only());
        }
    }
    ts::delete(&conn, &t.id).map_err(db_err)?;
    audit(&conn, who, "trigger_remove", Some(&src.id), Some(&t.id), json!({"source": src.name}));
    changed(&conn);
    Ok(json!({"id": t.id, "removed": true}))
}

fn test(who: &Who, body: &[u8]) -> Result<Value, SourceErr> {
    let b: IdBody = parse(body)?;
    let now = now();
    let (t, src) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (t, src) = trigger_and_source(&conn, who, &b.id)?;
        let recent = ts::test_fires_since(&conn, &t.id, now - 3600).map_err(db_err)?;
        if recent >= TEST_WAKES_PER_HOUR {
            return Err(SourceErr::new("rate_limited", 429, "at most 5 test wakes an hour per trigger").with("retryAfterSecs", 600));
        }
        audit(&conn, who, "trigger_test", Some(&src.id), Some(&t.id), json!({"source": src.name}));
        (t, src)
    };
    let (outcome, woke) = super::wake::test_wake(&t, &src, now);
    Ok(json!({"outcome": outcome, "woke": woke}))
}

/// Test seam: a fake workspace resolver.
#[cfg(test)]
pub mod test_seams {
    use std::cell::RefCell;

    pub type Resolver = fn(&str) -> Option<String>;

    thread_local! {
        static RESOLVER: RefCell<Option<Resolver>> = const { RefCell::new(None) };
    }

    pub fn set_resolver(f: Option<Resolver>) {
        RESOLVER.with(|c| *c.borrow_mut() = f);
    }

    pub(super) fn resolver() -> Option<Resolver> {
        RESOLVER.with(|c| *c.borrow())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::approve_routes::tests::{admin, agent, env, insert_source, member};

    fn call(path: &str, who: &Who, body: Value) -> Result<Value, SourceErr> {
        handle(path, who, body.to_string().as_bytes(), &HashMap::new())
    }

    fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> Result<Value, SourceErr> {
        let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        handle(path, who, b"", &p)
    }

    /// Workspaces `p1` and `p2` exist (by id or by the handles `k2-web` /
    /// `other`); nothing else does.
    fn resolver(t: &str) -> Option<String> {
        match t {
            "p1" | "k2-web" => Some("p1".into()),
            "p2" | "other" => Some("p2".into()),
            _ => None,
        }
    }

    struct Guard(#[allow(dead_code)] crate::sources::approve_routes::tests::Env);
    impl Drop for Guard {
        fn drop(&mut self) {
            test_seams::set_resolver(None);
        }
    }
    fn setup() -> Guard {
        let e = env();
        test_seams::set_resolver(Some(resolver));
        insert_source("fleet", "active", None, false, "member");
        Guard(e)
    }

    #[test]
    fn agents_propose_for_themselves_with_defaults_only() {
        let _g = setup();
        let r = call("/cli/sources/triggers/propose", &agent("p1"), json!({"source": "fleet", "on": {"kind": "item_went_crit"}}))
            .unwrap();
        assert_eq!(r["status"], "proposed");
        let id = r["id"].as_str().unwrap().to_string();
        let e = call(
            "/cli/sources/triggers/propose",
            &agent("p1"),
            json!({"source": "fleet", "on": {"kind": "went_crit"}, "agent": "other"}),
        )
        .unwrap_err();
        assert_eq!((e.code, e.status), ("other_agent", 403));
        let e = call(
            "/cli/sources/triggers/propose",
            &agent("p1"),
            json!({"source": "fleet", "on": {"kind": "went_crit"}, "cooldownSecs": 300}),
        )
        .unwrap_err();
        assert_eq!((e.code, e.status), ("field_person_only", 403));
        let e = call("/cli/sources/triggers/propose", &agent("p1"), json!({"source": "fleet", "on": {"kind": "item_went_crit"}}))
            .unwrap_err();
        assert_eq!((e.code, e.status), ("trigger_exists", 409));
        // The agent can't enable, pause or test it; it can withdraw it.
        for p in ["/cli/sources/triggers/approve", "/cli/sources/triggers/add", "/cli/sources/triggers/test", "/cli/sources/triggers/edit"] {
            let e = call(p, &agent("p1"), json!({"id": id})).unwrap_err();
            assert_eq!(e.code, "person_only", "{p}");
        }
        let e = call("/cli/sources/triggers/remove", &agent("p2"), json!({"id": id})).unwrap_err();
        assert_eq!(e.code, "not_own");
        let list = get("/cli/sources/triggers/list", &agent("p1"), &[("source", "fleet")]).unwrap();
        let v = &list["triggers"][0];
        assert_eq!(v["agent"]["projectId"], "p1");
        assert_eq!((v["cooldownSecs"].as_u64(), v["dailyCap"].as_u64()), (Some(1800), Some(12)));
        assert_eq!(v["madeBy"], "agent:p1");
        call("/cli/sources/triggers/remove", &agent("p1"), json!({"id": id})).unwrap();
        assert_eq!(get("/cli/sources/triggers/list", &admin(), &[]).unwrap()["triggers"], json!([]));
    }

    #[test]
    fn people_add_edit_approve_pause_and_resume() {
        let _g = setup();
        let e = call("/cli/sources/triggers/add", &admin(), json!({"source": "fleet", "on": {"kind": "went_crit"}}))
            .unwrap_err();
        assert_eq!(e.code, "bad_request", "a person names the agent");
        let e = call("/cli/sources/triggers/add", &admin(), json!({"source": "fleet", "on": {"kind": "went_crit"}, "agent": "ghost"}))
            .unwrap_err();
        assert_eq!((e.code, e.status), ("agent_not_found", 404));
        let e = call(
            "/cli/sources/triggers/add",
            &admin(),
            json!({"source": "fleet", "on": {"kind": "stale_for", "secs": 60}, "agent": "k2-web"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "bad_request", "stale_for is at least 5 minutes");
        let r = call(
            "/cli/sources/triggers/add",
            &admin(),
            json!({"source": "fleet", "on": {"kind": "went_crit"}, "agent": "k2-web", "note": "check the relay", "cooldownSecs": 600, "dailyCap": 3}),
        )
        .unwrap();
        assert_eq!(r["status"], "active");
        let id = r["id"].as_str().unwrap().to_string();
        let r = call("/cli/sources/triggers/edit", &admin(), json!({"id": id, "set": {"note": null, "dailyCap": 5, "agent": "other"}}))
            .unwrap();
        assert_eq!(r["trigger"]["note"], Value::Null);
        assert_eq!(r["trigger"]["dailyCap"], 5);
        assert_eq!(r["trigger"]["agent"]["projectId"], "p2");
        assert_eq!(call("/cli/sources/triggers/pause", &admin(), json!({"id": id})).unwrap()["status"], "paused");
        assert_eq!(call("/cli/sources/triggers/resume", &admin(), json!({"id": id})).unwrap()["status"], "active");
        let e = call("/cli/sources/triggers/approve", &admin(), json!({"id": id})).unwrap_err();
        assert_eq!(e.code, "not_proposed");
        let p = call("/cli/sources/triggers/propose", &admin(), json!({"source": "fleet", "on": {"kind": "recovered"}, "agent": "p1"}))
            .unwrap();
        let pid = p["id"].as_str().unwrap();
        assert_eq!(call("/cli/sources/triggers/approve", &admin(), json!({"id": pid})).unwrap()["status"], "active");
        let show = get("/cli/sources/triggers/show", &admin(), &[("id", pid)]).unwrap();
        assert_eq!(show["trigger"]["approvedBy"], "login:ada");
        assert_eq!(show["fires"], json!([]));
        let log = get("/cli/sources/triggers/log", &admin(), &[("id", pid), ("limit", "5")]).unwrap();
        assert_eq!(log["fires"], json!([]));
        let e = get("/cli/sources/triggers/show", &admin(), &[("id", "nope")]).unwrap_err();
        assert_eq!((e.code, e.status), ("trigger_not_found", 404));
    }

    #[test]
    fn members_cant_see_triggers_of_admin_sources() {
        let _g = setup();
        insert_source("money", "active", None, false, "admin");
        call("/cli/sources/triggers/add", &admin(), json!({"source": "money", "on": {"kind": "went_crit"}, "agent": "p1"})).unwrap();
        call("/cli/sources/triggers/add", &admin(), json!({"source": "fleet", "on": {"kind": "went_crit"}, "agent": "p1"})).unwrap();
        let all = get("/cli/sources/triggers/list", &member(), &[]).unwrap();
        assert_eq!(all["triggers"].as_array().unwrap().len(), 1);
        assert_eq!(all["triggers"][0]["source"], "fleet");
        let e = get("/cli/sources/triggers/list", &member(), &[("source", "money")]).unwrap_err();
        assert_eq!(e.code, "cap_not_granted");
    }

    #[test]
    fn test_wakes_are_limited_to_five_an_hour() {
        let _g = setup();
        let db = k2_core::db::shared();
        let fake: std::sync::Arc<crate::sources::wake::Pipe> = std::sync::Arc::new(crate::sources::wake::Pipe::new(Box::new(Answer)));
        crate::sources::wake::test_seams::set_pipe(Some(fake));
        let r = call("/cli/sources/triggers/add", &admin(), json!({"source": "fleet", "on": {"kind": "went_crit"}, "agent": "p1"}))
            .unwrap();
        let id = r["id"].as_str().unwrap();
        for _ in 0..5 {
            let r = call("/cli/sources/triggers/test", &admin(), json!({"id": id})).unwrap();
            assert_eq!(r, json!({"outcome": "inbox", "woke": false}));
        }
        let e = call("/cli/sources/triggers/test", &admin(), json!({"id": id})).unwrap_err();
        assert_eq!((e.code, e.status), ("rate_limited", 429));
        crate::sources::wake::test_seams::set_pipe(None);
        drop(db);
    }

    struct Answer;
    impl k2_core::sources::Waker for Answer {
        fn deliver(&self, _req: &k2_core::sources::WakeRequest) -> k2_core::sources::WakeOutcome {
            k2_core::sources::WakeOutcome::Inbox
        }
    }
}
