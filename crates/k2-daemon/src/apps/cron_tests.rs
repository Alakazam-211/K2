//! App cron: the schedule fires headless (the daemon's own tick, no client),
//! the call reaches the App's port with the secret header, an overlapping
//! slot is skipped, missed slots follow their policy, three failures in a
//! row tell the App's workspace once, and every run is logged. The App is a
//! tiny HTTP server on loopback; nothing real is woken.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use chrono::{Local, TimeZone};
use k2_core::deploy::store;
use serde_json::json;

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    app: String,
    ws: String,
}

fn fx(tag: &str) -> Fx {
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (app, ws) = (format!("cron{tag}"), format!("ws-cron-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![ws, format!("/tmp/{ws}")]).unwrap();
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: &app,
                service_id: &format!("svc-{app}"),
                seed_id: None,
                owner_workspace: &ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
    }
    Fx { _db: db, app, ws }
}

/// A one-thread HTTP App: answers `status` to every request and keeps
/// each request's head.
fn app_server(status: u16) -> (u16, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut buf = Vec::new();
            let mut b = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match c.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&b[..n]),
                }
            }
            s2.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
            let body = if status == 200 { "done" } else { "boom: the job failed" };
            let _ = write!(c, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (port, seen)
}

fn add_ok(f: &Fx, schedule: &str, path: &str, extra: serde_json::Value) -> String {
    let mut b = json!({"app": f.app, "schedule": schedule, "path": path});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    let v = handle(ADD, &Who::Owner, b.to_string().as_bytes(), &HashMap::new()).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    v["id"].as_str().unwrap().to_string()
}

fn set_anchor_to(id: &str, at: i64) {
    super::set_anchor(id, at);
}

fn runs(id: &str) -> Vec<serde_json::Value> {
    last_runs(id, 100)
}

fn local(y: i32, mo: u32, d: u32, h: u32) -> i64 {
    Local.with_ymd_and_hms(y, mo, d, h, 0, 0).single().unwrap().timestamp()
}

#[test]
fn add_checks_what_it_is_given() {
    let f = fx("add");
    test_seam::set(&f.app, None, "unused");
    let bad = |b: serde_json::Value| handle(ADD, &Who::Owner, b.to_string().as_bytes(), &HashMap::new()).unwrap_err();
    for (b, want) in [
        (json!({"app": f.app, "schedule": "*/5 * * *", "path": "/x"}), "5 fields"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "https://evil.example/x"}), "starting with /"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "api/x"}), "starting with /"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "/a b"}), "starting with /"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "//host/x"}), "starting with /"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "/x", "method": "DELETE"}), "GET or POST"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "/x", "missed": "all"}), "skip (default) or once"),
        (json!({"app": f.app, "schedule": "*/5 * * * *", "path": "/x", "timeout": "2h"}), "at most 1h"),
        (json!({"app": f.app, "path": "/x"}), "app cron add"),
        (json!({"app": "nope", "schedule": "*/5 * * * *", "path": "/x"}), ""),
    ] {
        let e = bad(b.clone());
        assert!(e.hint.contains(want), "{b}: {}", e.hint);
    }
    let id = add_ok(&f, "*/5  *  * * *", "/api/cron/digest", json!({"method": "post", "timeout": "90s"}));
    let e = handle(ADD, &Who::Owner, json!({"app": f.app, "schedule": "*/5 * * * *", "path": "/api/cron/digest"}).to_string().as_bytes(), &HashMap::new())
        .unwrap_err();
    assert_eq!((e.code, e.status), ("cron_exists", 409), "{}", e.hint);
    let mut p = HashMap::new();
    p.insert("app".to_string(), f.app.clone());
    let v = handle(LIST, &Who::Owner, b"", &p).unwrap();
    let c = &v["crons"][0];
    assert_eq!((c["id"].as_str(), c["schedule"].as_str(), c["method"].as_str(), c["timeoutSecs"].as_i64()), (Some(id.as_str()), Some("*/5 * * * *"), Some("POST"), Some(90)), "{v}");
    // A Member reads; it never adds.
    let member = Who::Login { username: "m".into(), role: k2_core::connect_users::Role::Member };
    assert!(handle(LIST, &member, b"", &p).is_ok());
    let e = handle(ADD, &member, json!({"app": f.app, "schedule": "0 * * * *", "path": "/y"}).to_string().as_bytes(), &HashMap::new()).unwrap_err();
    assert_eq!(e.status, 403, "{}", e.hint);
    // rm.
    let v = handle(RM, &Who::Owner, json!({"app": f.app, "id": id}).to_string().as_bytes(), &HashMap::new()).unwrap();
    assert_eq!(v["removed"], id.as_str());
    assert!(entries(Some(&f.app)).is_empty());
}

/// The schedule fires from the daemon's own tick (no client), the call
/// reaches the App's port with `Authorization: Bearer <CRON_SECRET>`, and
/// the run is logged ok; a slot while that run is still going is skipped.
#[test]
fn cron_fires_headless_with_the_secret_and_refuses_overlap() {
    let f = fx("fire");
    let (port, seen) = app_server(200);
    test_seam::set(&f.app, Some(port), "s3cret-value");
    let id = add_ok(&f, "* * * * *", "/api/cron/sync?x=1", json!({}));
    let t0 = local(2026, 3, 10, 9);
    set_anchor_to(&id, t0);

    // A slot due: started, and the run (inline here) calls the App.
    let ran = Mutex::new(Vec::new());
    let r = tick_at(t0 + 61, &|app, e, run| {
        ran.lock().unwrap().push(run_claimed(&app, &e, &run).0);
    });
    assert_eq!(r, vec![Tick::Started(id.clone())]);
    assert_eq!(ran.lock().unwrap()[0].status, "ok");
    let head = seen.lock().unwrap()[0].clone();
    assert!(head.starts_with("GET /api/cron/sync?x=1 HTTP/1.1\r\n"), "{head}");
    assert!(head.to_ascii_lowercase().contains("authorization: bearer s3cret-value\r\n"), "{head}");
    assert!(head.contains("User-Agent: k2-cron/1") || head.contains("user-agent: k2-cron/1"), "{head}");
    let log = runs(&id);
    assert_eq!((log[0]["status"].as_str(), log[0]["http"].as_i64(), log[0]["trigger"].as_str()), (Some("ok"), Some(200), Some("schedule")), "{log:?}");
    assert!(!log.iter().any(|r| r.to_string().contains("s3cret")), "the secret is never logged");
    // Nothing more due in the same minute.
    assert_eq!(tick_at(t0 + 70, &|_, _, _| panic!("not due")), vec![Tick::NotYet(id.clone())]);

    // The next slot starts a run that doesn't finish (the spawn holds it)...
    let held = Mutex::new(None);
    assert_eq!(tick_at(t0 + 121, &|_, e, run| *held.lock().unwrap() = Some((e, run))), vec![Tick::Started(id.clone())]);
    // ...so the one after is skipped, and logged as such.
    assert_eq!(tick_at(t0 + 181, &|_, _, _| panic!("overlap must not start")), vec![Tick::SkippedOverlap(id.clone())]);
    assert_eq!(runs(&id)[0]["status"], "skipped_overlap");
    // run-now refuses the same way.
    let e = handle(RUN_NOW, &Who::Owner, json!({"app": f.app, "id": id}).to_string().as_bytes(), &HashMap::new()).unwrap_err();
    assert_eq!((e.code, e.status), ("cron_running", 409));
    // The held run ends; the next slot runs again.
    let (e, run) = held.lock().unwrap().take().unwrap();
    let app = { let h = k2_core::db::shared(); let c = h.lock(); store::get_app(&c, &f.app).unwrap().unwrap() };
    assert_eq!(run_claimed(&app, &e, &run).0.status, "ok");
    assert_eq!(tick_at(t0 + 241, &|app, e, run| { run_claimed(&app, &e, &run); }), vec![Tick::Started(id.clone())]);
    assert_eq!(seen.lock().unwrap().len(), 3);
    // A restart mid-run: boot ends it, the entry is free.
    let held2 = Mutex::new(None);
    tick_at(t0 + 301, &|_, e, run| *held2.lock().unwrap() = Some((e, run)));
    boot();
    assert_eq!(runs(&id)[0]["status"], "interrupted");
    assert!(entries(Some(&f.app))[0].running_run.is_none());
}

/// Three failures in a row tell the App's workspace once; an ok run resets
/// the streak; a stopped App is logged, not counted.
#[test]
fn three_failures_in_a_row_tell_the_workspace_once() {
    let f = fx("fail");
    let (port, _) = app_server(500);
    test_seam::set(&f.app, Some(port), "tok");
    let told = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let t2 = told.clone();
    super::super::alerts::set_sink(Some(Box::new(move |ws, text| t2.lock().unwrap().push((ws.to_string(), text.to_string())))));
    let id = add_ok(&f, "0 4 * * *", "/api/cron/bill", json!({}));
    let run_now = || handle(RUN_NOW, &Who::Owner, json!({"app": f.app, "id": id}).to_string().as_bytes(), &HashMap::new()).unwrap();
    for n in 1..=4 {
        let v = run_now();
        assert_eq!((v["status"].as_str(), v["http"].as_i64(), v["failuresInARow"].as_i64()), (Some("failed"), Some(500), Some(n)), "{v}");
        assert_eq!(v["detail"], "boom: the job failed");
    }
    {
        let t = told.lock().unwrap();
        assert_eq!(t.len(), 1, "once per streak: {t:?}");
        assert_eq!(t[0].0, f.ws);
        assert!(t[0].1.contains("failed 3 times in a row") && t[0].1.contains("/api/cron/bill") && t[0].1.contains("HTTP 500"), "{}", t[0].1);
    }
    // Stopped: logged, the streak unchanged.
    test_seam::set(&f.app, None, "tok");
    let v = run_now();
    assert_eq!((v["status"].as_str(), v["failuresInARow"].as_i64()), (Some("not_running"), Some(4)), "{v}");
    // Back and answering: the streak resets.
    let (ok_port, _) = app_server(200);
    test_seam::set(&f.app, Some(ok_port), "tok");
    assert_eq!(run_now()["failuresInARow"], 0);
    let mut p = HashMap::new();
    p.insert("app".to_string(), f.app.clone());
    p.insert("id".to_string(), id.clone());
    let v = handle(LOGS, &Who::Owner, b"", &p).unwrap();
    let statuses: Vec<&str> = v["logs"][0]["runs"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, vec!["ok", "not_running", "failed", "failed", "failed", "failed"], "{v}");
    super::super::alerts::set_sink(None);
}

/// A slot missed while the daemon was down: `skip` logs it, `once` runs
/// one catch-up; past 12 h it is always skipped.
#[test]
fn missed_slots_follow_their_policy() {
    let f = fx("missed");
    let (port, seen) = app_server(200);
    test_seam::set(&f.app, Some(port), "tok");
    let skip = add_ok(&f, "0 0 1 1 *", "/api/cron/yearly", json!({}));
    let once = add_ok(&f, "0 0 1 1 *", "/api/cron/yearly-once", json!({"missed": "once"}));
    let midnight = local(2027, 1, 1, 0);
    for id in [&skip, &once] {
        set_anchor_to(id, midnight - 86_400);
    }
    // Two hours after the slot: catch-up territory.
    let r = tick_at(midnight + 7200, &|app, e, run| {
        run_claimed(&app, &e, &run);
    });
    assert!(r.contains(&Tick::SkippedMissed(skip.clone())) && r.contains(&Tick::Started(once.clone())) && r.len() == 2, "{r:?}");
    assert_eq!(runs(&skip)[0]["status"], "skipped_missed");
    assert_eq!((runs(&once)[0]["status"].as_str(), runs(&once)[0]["trigger"].as_str()), (Some("ok"), Some("catch_up")));
    assert_eq!(seen.lock().unwrap().len(), 1, "only the once entry called the App");
    assert!(seen.lock().unwrap()[0].contains("/api/cron/yearly-once"));
    // The anchor moved: nothing more this year.
    let r = tick_at(midnight + 7300, &|_, e, _| panic!("{} again", e.id));
    assert!(r.iter().all(|t| matches!(t, Tick::NotYet(_))), "{r:?}");
}

#[test]
fn a_slot_older_than_twelve_hours_is_always_skipped() {
    let f = fx("stale");
    test_seam::set(&f.app, Some(1), "tok");
    let id = add_ok(&f, "0 0 1 1 *", "/api/cron/yearly", json!({"missed": "once"}));
    let midnight = local(2027, 1, 1, 0);
    set_anchor_to(&id, midnight - 86_400);
    assert_eq!(tick_at(midnight + 13 * 3600, &|_, _, _| panic!("too late to run")), vec![Tick::SkippedMissed(id.clone())]);
    assert_eq!(runs(&id)[0]["status"], "skipped_missed");
}
