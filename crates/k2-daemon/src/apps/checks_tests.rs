//! `host.checks` (TC-16): the parse, the judge, a run against a loopback
//! server, the wake text (no App output), and the recorded run.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use serde_json::json;

use super::*;

fn check(v: serde_json::Value) -> Check {
    serde_json::from_value(v).expect("a check")
}

#[test]
fn the_parse_takes_every_field_and_refuses_bad_ones() {
    let m = json!({ "host": { "checks": [
        { "name": "health", "path": "/api/health", "expect": 200 },
        { "name": "login csp", "path": "/login", "expect": 200, "header": { "name": "Content-Security-Policy", "contains": "default-src 'self'" }, "bodyContains": "Sign in" },
        { "name": "config-401", "path": "/api/config", "expect": 401, "as": "anonymous" },
        { "name": "me", "path": "/api/me", "expect": 200, "as": "testGuest" },
    ] } });
    let c = parse(&m).expect("parses");
    assert_eq!(c.len(), 4);
    assert_eq!(c[1].header.as_ref().map(|h| h.name.as_str()), Some("Content-Security-Policy"));
    assert_eq!(c[1].body_contains.as_deref(), Some("Sign in"));
    assert_eq!(c[3].probe_as(), ProbeAs::TestGuest);
    assert_eq!(parse(&json!({ "host": {} })).expect("none"), vec![], "no checks: none");
    let eleven: Vec<_> = (0..11).map(|i| json!({ "name": format!("c{i}"), "path": "/", "expect": 200 })).collect();
    for (bad, why) in [
        (json!([{ "name": "x", "path": "/", "expect": 200, "extra": 1 }]), "unknown field"),
        (json!([{ "name": "x", "path": "nope", "expect": 200 }]), "absolute path"),
        (json!([{ "name": "x", "path": "/", "expect": 99 }]), "one HTTP status"),
        (json!([{ "name": "x", "path": "/", "expect": 200, "as": "admin" }]), "anonymous or testGuest"),
        (json!([{ "name": "x", "path": "/", "expect": 200 }, { "name": "x", "path": "/a", "expect": 200 }]), "two checks"),
        (json!([{ "name": "x", "path": "/", "expect": 200, "bodyContains": "a".repeat(201) }]), "bodyContains"),
        (json!([{ "name": "x", "path": "/", "expect": 200, "header": { "name": "bad name", "contains": "x" } }]), "header"),
        (json!(eleven), "at most 10"),
    ] {
        let e = parse(&json!({ "host": { "checks": bad } })).expect_err(why);
        assert!(e.contains(why), "{why}: {e}");
    }
}

#[test]
fn the_judge_names_what_did_not_match_and_reads_64_kib_only() {
    let c = check(json!({ "name": "login", "path": "/login", "expect": 200, "header": { "name": "content-security-policy", "contains": "frame-ancestors" }, "bodyContains": "Sign in" }));
    let good = Answer { status: 200, headers: vec![("Content-Security-Policy".into(), "default-src 'self'; frame-ancestors 'none'".into())], body: b"<h1>Sign in</h1>".to_vec() };
    assert_eq!(judge(&c, &good), (true, Some(true), Some(true), None));
    let wrong_status = Answer { status: 500, ..Answer { status: 0, headers: good.headers.clone(), body: good.body.clone() } };
    assert_eq!(judge(&c, &wrong_status).3, Some("status"));
    let no_header = Answer { status: 200, headers: vec![], body: good.body.clone() };
    assert_eq!(judge(&c, &no_header), (false, Some(false), Some(true), Some("header")));
    let mut late = vec![b'x'; BODY_WINDOW];
    late.extend_from_slice(b"Sign in");
    let late = Answer { status: 200, headers: good.headers.clone(), body: late };
    assert_eq!(judge(&c, &late), (false, Some(true), Some(false), Some("body")), "past 64 KiB doesn't count");
    let plain = check(json!({ "name": "h", "path": "/", "expect": 204 }));
    assert_eq!(judge(&plain, &Answer { status: 204, headers: vec![], body: vec![] }), (true, None, None, None));
}

/// A loopback server answering each path with a fixed status, header and
/// body (one request per connection).
fn serve(routes: Vec<(&'static str, u16, &'static str, &'static str)>, n: usize) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for s in l.incoming().take(n) {
            let mut s = s.expect("accept");
            let mut line = String::new();
            let mut r = BufReader::new(s.try_clone().expect("clone"));
            r.read_line(&mut line).expect("request line");
            loop {
                let mut h = String::new();
                r.read_line(&mut h).expect("header");
                if h == "\r\n" || h.is_empty() {
                    break;
                }
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (status, header, body) = routes.iter().find(|(p, ..)| *p == path).map(|(_, s, h, b)| (*s, *h, *b)).unwrap_or((404, "", "nope"));
            let extra = if header.is_empty() { String::new() } else { format!("{header}\r\n") };
            let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}", body.len());
        }
    });
    port
}

#[test]
fn a_run_reports_each_check_and_never_app_output() {
    let port = serve(
        vec![
            ("/api/health", 200, "", "ok"),
            ("/login", 200, "Content-Security-Policy: default-src 'self'", "<b>SECRET-BODY</b> Sign in"),
            ("/api/config", 200, "", "{}"),
        ],
        3,
    );
    let checks = parse(&json!({ "host": { "checks": [
        { "name": "health", "path": "/api/health", "expect": 200 },
        { "name": "login", "path": "/login", "expect": 200, "header": { "name": "content-security-policy", "contains": "frame-ancestors" } },
        { "name": "config", "path": "/api/config", "expect": 401 },
    ] } }))
    .expect("parse");
    let out = run("jobs", &format!("http://127.0.0.1:{port}"), &checks);
    let by: std::collections::HashMap<&str, &Outcome> = out.iter().map(|o| (o.name.as_str(), o)).collect();
    assert!(by["health"].ok && by["health"].status == Some(200));
    assert_eq!((by["login"].ok, by["login"].header, by["login"].why), (false, Some(false), Some("header")));
    assert_eq!((by["config"].ok, by["config"].status, by["config"].why), (false, Some(200), Some("status")));
    let json_text = serde_json::to_string(&out.iter().map(Outcome::to_json).collect::<Vec<_>>()).expect("json");
    assert!(!json_text.contains("SECRET-BODY") && !json_text.contains("default-src"), "no body or header value: {json_text}");
    let failed: Vec<&Outcome> = out.iter().filter(|o| !o.ok).collect();
    let wake = wake_text("jobs", "d-1", &failed);
    assert!(wake.contains("2 of its host.checks failed") && wake.contains("k2 app status jobs"), "{wake}");
    assert!(!wake.contains("SECRET-BODY") && !wake.contains("default-src"), "{wake}");
    // Nothing listening: no_answer, not a panic.
    let dead = TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let out = run("jobs", &format!("http://127.0.0.1:{dead}"), &checks[..1]);
    assert_eq!((out[0].ok, out[0].status, out[0].why), (false, None, Some("no_answer")));
}

#[test]
fn a_recorded_run_is_found_by_its_deployment() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let o = Outcome { name: "h".into(), path: "/".into(), expect: 200, as_test_guest: false, status: Some(503), header: None, body: None, ok: false, why: Some("status"), ms: 3 };
    record("jobs", "d-1", std::slice::from_ref(&o));
    record("jobs", "d-2", &[Outcome { status: Some(200), ok: true, why: None, ..o.clone() }]);
    let r = recorded("jobs", "d-1").expect("recorded");
    assert_eq!(r["results"][0]["status"], 503);
    assert_eq!(recorded("jobs", "d-2").expect("d-2")["results"][0]["ok"], true);
    assert!(recorded("jobs", "d-3").is_none());
}
