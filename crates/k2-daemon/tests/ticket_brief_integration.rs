//! Ticket HTML brief (prd-ticket-html-brief-v1.md, vs-live H29–H44).
//!
//! The k2-core half (`k2_core::feedback_brief` + `feedback::create_with_brief`
//! + migration 0126) is tested HERE, from a k2-daemon test binary, because
//! `cargo test -p k2-core` must not run on the dev Mac. This binary links
//! k2-core with `test-util`, so `db::shared()` is the in-memory DB that ran
//! every migration (0126 included).
//!
//!   T1  sanitizer: exact output strings for kept and stripped markup
//!   T2  caps: 1 MiB + 1 → TooLarge, invalid UTF-8 → Invalid, empty → Empty
//!   T3  one transaction: a failed brief insert leaves no ticket row
//!   H9  the FK cascade removes a brief with its ticket
//!   T14 no migration after 0126 rebuilds `feedback` (H10/H35)
//!   H30 a 3 MiB create body gets 413 through the REAL dispatcher
//!   H29 the door comes from the token: owner warns, a Connect member does not
//!   A1  assignee policy through the real dispatcher: no assignee warns,
//!       a Connect user / owner does not, an unknown name warns, fyi exempt
//!   T12 teaching: the brief AND `--assign <user>`
//!
//! ISOLATION: `$HOME`, connect-user stores and the in-memory DB are
//! process-wide, so dispatcher tests serialize on `TEST_LOCK`. Rows are
//! found by id, never by count.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users;
use k2_core::feedback::{self, NewFeedback};
use k2_core::feedback_brief::{self as brief, BriefError};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-ticket-brief";

// ── T1: sanitizer ───────────────────────────────────────────────────────

#[test]
fn sanitize_keeps_the_allowlist_exactly() {
    let cases: &[(&str, &str)] = &[
        (
            "<h1>T</h1><p><strong>a</strong> <em>b</em> <code>c</code> <kbd>k</kbd></p>\
             <ul><li>x</li></ul><ol class=\"k2-options\"><li>y</li></ol>",
            "<h1>T</h1><p><strong>a</strong> <em>b</em> <code>c</code> <kbd>k</kbd></p>\
             <ul><li>x</li></ul><ol class=\"k2-options\"><li>y</li></ol>",
        ),
        (
            "<table><tr><th rowspan=\"2\">h</th><td colspan=\"2\" title=\"t\">y</td></tr></table>",
            "<table><tbody><tr><th rowspan=\"2\">h</th><td colspan=\"2\" title=\"t\">y</td></tr></tbody></table>",
        ),
        (
            "<details open><summary>s</summary><pre>  log\n  line</pre></details>",
            "<details open=\"\"><summary>s</summary><pre>  log\n  line</pre></details>",
        ),
        (
            "<section class=\"k2-need\"><p>need</p></section><div class=\"k2-callout\">c</div>",
            "<section class=\"k2-need\"><p>need</p></section><div class=\"k2-callout\">c</div>",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(brief::sanitize(input), *want, "input: {input}");
        assert!(!brief::removed_anything(input), "nothing removed from: {input}");
    }
}

#[test]
fn sanitize_strips_everything_outside_the_allowlist() {
    let cases: &[(&str, &str)] = &[
        ("<script>alert(1)</script><p>ok</p>", "<p>ok</p>"),
        ("<style>p{color:red}</style><p style=\"color:red\" onclick=\"x()\">ok</p>", "<p>ok</p>"),
        ("<iframe src=\"https://e.com\"></iframe><p>ok</p>", "<p>ok</p>"),
        ("<form action=\"/x\"><input name=\"a\">hi<button>go</button></form>", "higo"),
        (
            "<meta http-equiv=\"refresh\" content=\"0;url=https://e.com\"><base href=\"https://e.com/\"><p>ok</p>",
            "<p>ok</p>",
        ),
        ("<svg><circle r=\"1\"/><text>t</text></svg><p>ok</p>", "<p>ok</p>"),
        ("<object data=\"x\"></object><embed src=\"x\"><p>ok</p>", "<p>ok</p>"),
        ("<!-- secret --><p>ok</p>", "<p>ok</p>"),
        ("<div class=\"k2-callout evil\" id=\"x\">c</div>", "<div class=\"k2-callout\">c</div>"),
        (
            "<a href=\"javascript:alert(1)\">j</a><a href=\"data:text/html,x\">d</a>\
             <a href=\"https://k2.dev/x\">k</a><a href=\"mailto:a@b.c\">m</a><a href=\"/rel\">r</a>",
            "<a rel=\"noopener noreferrer\">j</a><a rel=\"noopener noreferrer\">d</a>\
             <a href=\"https://k2.dev/x\" rel=\"noopener noreferrer\">k</a>\
             <a href=\"mailto:a@b.c\" rel=\"noopener noreferrer\">m</a><a rel=\"noopener noreferrer\">r</a>",
        ),
        (
            "<img src=\"https://evil.example/x.png\" alt=\"r\">\
             <img src=\"data:image/png;base64,iVBORw0KGgo=\" alt=\"ok\">\
             <img src=\"data:image/svg+xml;base64,PHN2Zz4=\" alt=\"s\">",
            "<img alt=\"r\"><img src=\"data:image/png;base64,iVBORw0KGgo=\" alt=\"ok\"><img alt=\"s\">",
        ),
        (
            "<!doctype html><html><head><title>T</title><meta charset=\"utf-8\">\
             <link rel=\"stylesheet\" href=\"https://e.com/x.css\"></head><body><p>ok</p></body></html>",
            "<p>ok</p>",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(brief::sanitize(input), *want, "input: {input}");
    }
    // Every stripping case except the full document reports a removal
    // (the link tag in the full document does too).
    for (input, _) in cases {
        assert!(brief::removed_anything(input), "must report a removal: {input}");
    }
    // A plain full document (doctype, title, meta charset) is accepted
    // as-is: its wrappers are not a removal.
    assert!(!brief::removed_anything(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>T</title></head>\
         <body><p>ok</p></body></html>"
    ));
}

#[test]
fn extract_text_reads_like_the_page() {
    let html = "<h2>Problem</h2><p>Deploy   is\n<b>blocked</b> &amp; stuck.</p>\
        <ol class=\"k2-options\"><li>Retry</li><li>Roll back</li></ol>\
        <ul><li>a</li><li>b</li></ul>\
        <p>See <a href=\"https://k2.dev/x\" rel=\"noopener noreferrer\">the log</a>.</p>\
        <pre>  line 1\n  line 2</pre>\
        <table><tbody><tr><td>k</td><td>v</td></tr></tbody></table>\
        <img src=\"data:image/png;base64,AA==\" alt=\"graph\">";
    assert_eq!(
        brief::extract_text(html),
        "Problem\nDeploy is blocked & stuck.\n1. Retry\n2. Roll back\n- a\n- b\n\
         See the log (https://k2.dev/x).\n  line 1\n  line 2\nk | v\n[image: graph]"
    );
}

// ── T2: caps and refusals ──────────────────────────────────────────────

#[test]
fn clean_enforces_the_cap_utf8_and_visible_text() {
    let over = vec![b'a'; brief::MAX_BRIEF_BYTES + 1];
    assert_eq!(
        brief::clean_bytes(&over),
        Err(BriefError::TooLarge { bytes: brief::MAX_BRIEF_BYTES + 1 })
    );
    let at_cap = format!("<p>{}</p>", "a".repeat(brief::MAX_BRIEF_BYTES - 7));
    assert_eq!(at_cap.len(), brief::MAX_BRIEF_BYTES);
    let ok = brief::clean_bytes(at_cap.as_bytes()).expect("exactly 1 MiB is allowed");
    assert_eq!(ok.bytes as usize, brief::MAX_BRIEF_BYTES);
    assert!(ok.text.len() <= brief::MAX_TEXT_BYTES, "text extract is capped: {}", ok.text.len());
    assert!(ok.text.ends_with("open the brief for the rest)"), "truncation marker");

    assert_eq!(brief::clean_bytes(&[0x3c, 0x70, 0x3e, 0xff, 0xfe]), Err(BriefError::Invalid));
    assert_eq!(brief::clean("<div> <br> </div>"), Err(BriefError::Empty));
    assert_eq!(brief::clean("<script>only()</script><style>x{}</style>"), Err(BriefError::Empty));

    assert_eq!(BriefError::TooLarge { bytes: 1 }.code(), "brief_too_large");
    assert_eq!(BriefError::Empty.code(), "brief_empty");
    assert_eq!(BriefError::Invalid.code(), "brief_invalid");
}

#[test]
fn clean_fills_every_stored_field() {
    let c = brief::clean("<p>Need a call.</p><section class=\"k2-need\"><p>Yes/no</p></section>")
        .expect("clean");
    assert_eq!(c.html, "<p>Need a call.</p><section class=\"k2-need\"><p>Yes/no</p></section>");
    assert_eq!(c.text, "Need a call.\nYes/no");
    assert_eq!(c.bytes as usize, c.html.len());
    assert_eq!(c.sanitizer, "k2-brief-v1");
    assert_eq!(c.sha256.len(), 64);
    assert!(c.has_need);
    assert!(!c.removed);
    assert!(c.warnings().is_empty());
}

// ── T3 / H9: storage ────────────────────────────────────────────────────

fn seed_project(label: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, format!("tb-{label}-{id}"), format!("/tmp/tb-{label}-{id}")],
    )
    .expect("insert project");
    id
}

fn ask(project_id: &str) -> NewFeedback {
    NewFeedback {
        project_id: project_id.to_string(),
        agent_name: "scout".to_string(),
        title: "Deploy?".to_string(),
        ..Default::default()
    }
}

#[test]
fn create_with_brief_writes_both_rows() {
    let pid = seed_project("both");
    let clean = brief::clean("<p>Why</p><section class=\"k2-need\"><p>Go?</p></section>").expect("clean");
    let item = feedback::create_with_brief(ask(&pid), Some(clean.clone())).expect("create");
    assert!(item.has_brief);
    assert_eq!(item.brief_bytes, Some(clean.bytes));
    assert_eq!(item.comment_count, 1, "seed comment still written");
    let stored = brief::get(&item.id).expect("brief row");
    assert_eq!(stored.html, clean.html);
    assert_eq!(stored.text, clean.text);
    assert_eq!(stored.sha256, clean.sha256);
    assert_eq!(stored.sanitizer, "k2-brief-v1");

    let bare = feedback::create(ask(&pid)).expect("create without brief");
    assert!(!bare.has_brief);
    assert_eq!(bare.brief_bytes, None);
    assert!(brief::get(&bare.id).is_none());
}

#[test]
fn a_failed_brief_insert_leaves_no_ticket() {
    let pid = seed_project("rollback");
    let clean = brief::clean("<p>Why</p>").expect("clean");
    brief::fail_next_insert_for_test();
    let err = feedback::create_with_brief(ask(&pid), Some(clean)).expect_err("forced failure");
    assert!(err.contains("feedback brief insert failed"), "{err}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let tickets: i64 = conn
        .query_row("SELECT COUNT(*) FROM feedback WHERE project_id = ?1", [&pid], |r| r.get(0))
        .expect("count tickets");
    assert_eq!(tickets, 0, "the ticket row rolled back with the brief");
    let comments: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feedback_comments c JOIN feedback f ON f.id = c.feedback_id \
             WHERE f.project_id = ?1",
            [&pid],
            |r| r.get(0),
        )
        .expect("count comments");
    assert_eq!(comments, 0);
}

#[test]
fn the_brief_cascades_with_its_ticket() {
    let pid = seed_project("cascade");
    let clean = brief::clean("<p>Why</p>").expect("clean");
    let item = feedback::create_with_brief(ask(&pid), Some(clean)).expect("create");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM feedback WHERE id = ?1", [&item.id]).expect("delete ticket");
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM feedback_briefs WHERE feedback_id = ?1", [&item.id], |r| {
            r.get(0)
        })
        .expect("count briefs");
    assert_eq!(left, 0, "ON DELETE CASCADE removes the brief");
}

// ── T14: no `feedback` rebuild after 0126 ───────────────────────────────

/// Migrations reviewed to rebuild `feedback` with foreign keys OFF
/// outside the migration transaction. Empty: none exist.
const REBUILD_EXEMPT: &[&str] = &[];

/// Does this migration SQL drop the `feedback` table itself (not a
/// `feedback_*` child)? Comment lines are ignored.
fn drops_feedback(sql: &str) -> bool {
    let code: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let squashed = code.split_whitespace().collect::<Vec<_>>().join(" ");
    ["drop table feedback", "drop table if exists feedback"].iter().any(|needle| {
        squashed.match_indices(needle).any(|(at, _)| {
            let next = squashed[at + needle.len()..].chars().next();
            !matches!(next, Some(c) if c.is_ascii_alphanumeric() || c == '_')
        })
    })
}

#[test]
fn no_migration_after_0126_rebuilds_feedback() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-core/drizzle_sql");
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).expect("read migration");
    // The guard catches the two historical rebuilds (H35) and passes 0126.
    assert!(drops_feedback(&read("0090_feedback_tickets.sql")));
    assert!(drops_feedback(&read("0098_feedback_needs_discussion.sql")));
    assert!(!drops_feedback(&read("0126_feedback_briefs.sql")));
    assert!(!drops_feedback("DROP TABLE feedback_comments;"));

    let mut seen_0126 = false;
    for entry in std::fs::read_dir(&dir).expect("read drizzle_sql") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().expect("name").to_string_lossy().to_string();
        let Some(num) = name.get(..4).and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        seen_0126 |= num == 126;
        if num < 126 || REBUILD_EXEMPT.contains(&name.as_str()) {
            continue;
        }
        assert!(
            !drops_feedback(&std::fs::read_to_string(&path).expect("read migration")),
            "{name} drops `feedback`: a rebuild under foreign_keys=ON cascades into \
             feedback_briefs/comments/assignees (H10). Use a new table, or list it in \
             REBUILD_EXEMPT after turning FKs off outside the transaction."
        );
    }
    assert!(seen_0126, "0126_feedback_briefs.sql is in drizzle_sql");
}

// ── Real dispatcher: H30 413 and the H29 door ───────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn try_parse(raw: &[u8]) -> Option<Resp> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers.lines().next()?.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    let content_len = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    })?;
    if body.len() < content_len {
        return None;
    }
    Some(Resp { status, body: body.to_string() })
}

fn read_resp(stream: &mut TcpStream) -> Resp {
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(resp) = try_parse(&raw) {
            return resp;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    try_parse(&raw).unwrap_or_else(|| panic!("incomplete response: {:?}", String::from_utf8_lossy(&raw)))
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write");
    read_resp(&mut stream)
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-ticket-brief-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&tmp).expect("remove temp HOME");
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn seed_workspace_path(label: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/tb-ws-{label}-{id}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, format!("tbws-{label}-{id}"), path],
    )
    .expect("insert project");
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversize_create_body_is_413_before_it_is_read() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let mut stream = TcpStream::connect(("127.0.0.1", d.port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
        // Declare 3 MiB, send only a few bytes: the daemon must answer
        // from the head alone instead of waiting for (and buffering) it.
        let head = format!(
            "POST /cli/feedback/create?token={OWNER_TOKEN} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{{\"title\":",
            3 * 1024 * 1024
        );
        stream.write_all(head.as_bytes()).expect("write head");
        let r = read_resp(&mut stream);
        assert_eq!(r.status, 413, "{}", r.body);
        let v = json(&r.body);
        assert_eq!(v["error"]["code"], "brief_too_large", "{}", r.body);
        // The socket is closed, so the unread body is never parsed as
        // the next keep-alive request.
        let mut rest = [0u8; 64];
        match stream.read(&mut rest) {
            Ok(0) => {}
            Ok(n) => panic!("socket stayed open and sent {n} more bytes"),
            Err(e) => assert!(
                matches!(e.kind(), std::io::ErrorKind::ConnectionReset),
                "expected close, got {e:?}"
            ),
        }

        // A normal create on a fresh socket still works.
        let path = seed_workspace_path("413-after");
        let ok = http(
            d.port,
            "POST",
            &format!("/cli/feedback/create?token={OWNER_TOKEN}"),
            Some(
                &serde_json::json!({ "project": path, "title": "after", "briefHtml": "<p>ok</p>" })
                    .to_string(),
            ),
        );
        assert_eq!(ok.status, 200, "{}", ok.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_door_comes_from_the_token() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("tb_member", "password123").expect("add_user");
        let member = connect_users::create_session("tb_member");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let path = seed_workspace_path("door");

        // Owner token = the agent door. 0.43.2 warns and still files.
        // Self-declared body fields can't change the door.
        let body = serde_json::json!({
            "project": path,
            "title": "owner, no brief",
            "sessionKind": "canonical",
            "agentName": "a person, honest",
        })
        .to_string();
        let r = http(d.port, "POST", &format!("/cli/feedback/create?token={OWNER_TOKEN}"), Some(&body));
        assert_eq!(r.status, 200, "{}", r.body);
        let v = json(&r.body);
        assert_eq!(v["warnings"][0]["code"], "brief_missing", "{}", r.body);
        assert_eq!(v["hasBrief"], false);

        // A Connect member is a person: no brief needed, no warning.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/feedback/create?token={member}"),
            Some(&serde_json::json!({ "project": path, "title": "member" }).to_string()),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r.body)["warnings"], serde_json::json!([]), "{}", r.body);

        // Owner with a brief → stored, and show?brief=1 returns it.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/feedback/create?token={OWNER_TOKEN}"),
            Some(
                &serde_json::json!({
                    "project": path,
                    "title": "with brief",
                    "briefHtml": "<p>Why<script>x()</script></p><section class=\"k2-need\"><p>Go?</p></section>",
                })
                .to_string(),
            ),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let v = json(&r.body);
        assert_eq!(v["hasBrief"], true, "{}", r.body);
        assert_eq!(v["warnings"][0]["code"], "brief_sanitized", "{}", r.body);
        let id = v["id"].as_str().expect("id");

        let plain = http(d.port, "GET", &format!("/cli/feedback/show?id={id}&token={OWNER_TOKEN}"), None);
        assert_eq!(plain.status, 200, "{}", plain.body);
        assert!(json(&plain.body).get("brief").is_none(), "{}", plain.body);
        let full = http(
            d.port,
            "GET",
            &format!("/cli/feedback/show?id={id}&brief=1&token={OWNER_TOKEN}"),
            None,
        );
        assert_eq!(full.status, 200, "{}", full.body);
        assert_eq!(
            json(&full.body)["brief"]["html"],
            "<p>Why</p><section class=\"k2-need\"><p>Go?</p></section>",
            "{}",
            full.body
        );
        // A GET to create is still 405 (post-only guard).
        let get = http(d.port, "GET", &format!("/cli/feedback/create?token={OWNER_TOKEN}"), None);
        assert_eq!(get.status, 405, "{}", get.body);
    });
}

/// A1: the assignee policy through the real dispatcher with the owner
/// token (the agent door). "A user on this server" = the owner (literal
/// `owner` or the owner display name) or a stored Connect user.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assignee_policy_through_the_dispatcher() {
    let _g = lock();
    with_temp_home(|| {
        connect_users::add_user("tb_julie", "password123").expect("add_user");
        connect_users::add_user("tb_gone", "password123").expect("add_user");
        connect_users::set_disabled("tb_gone", true).expect("disable");
        let mut settings = k2_core::app_settings::load();
        settings.owner_display_name = Some("Rosson".to_string());
        k2_core::app_settings::save(&settings).expect("save owner display name");
        let member = connect_users::create_session("tb_julie");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let path = seed_workspace_path("assignee");
        let create = |token: &str, extra: serde_json::Value| -> Resp {
            let mut body = serde_json::json!({
                "project": path,
                "title": "assignee",
                "briefHtml": "<p>Why</p><section class=\"k2-need\"><p>Go?</p></section>",
            });
            for (k, v) in extra.as_object().expect("extra is an object") {
                body[k] = v.clone();
            }
            http(d.port, "POST", &format!("/cli/feedback/create?token={token}"), Some(&body.to_string()))
        };
        let codes = |r: &Resp| -> Vec<String> {
            json(&r.body)["warnings"]
                .as_array()
                .expect("warnings array")
                .iter()
                .map(|w| w["code"].as_str().expect("code").to_string())
                .collect()
        };

        // No assignee: filed, with assignee_required and its wording.
        let r = create(OWNER_TOKEN, serde_json::json!({}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(codes(&r), vec!["assignee_required"], "{}", r.body);
        let hint = json(&r.body)["warnings"][0]["hint"].as_str().expect("hint").to_string();
        assert!(
            hint.starts_with(
                "A ticket must be assigned to a user on this server (`--assign <user>`). \
                 This will be required in a future update."
            ),
            "{hint}"
        );

        // A Connect user, any case; the literal owner; the owner display
        // name; a disabled Connect user (still a user here): no warning.
        for who in [
            serde_json::json!(["tb_julie"]),
            serde_json::json!(["TB_Julie"]),
            serde_json::json!(["owner"]),
            serde_json::json!(["rosson"]),
            serde_json::json!(["tb_gone"]),
        ] {
            let r = create(OWNER_TOKEN, serde_json::json!({ "assignees": who }));
            assert_eq!(r.status, 200, "{}", r.body);
            assert!(codes(&r).is_empty(), "{who}: {}", r.body);
            assert_eq!(json(&r.body)["assignees"], who, "{}", r.body);
        }

        // Not a user here: filed, snapshot kept, assignee_unknown.
        let r = create(OWNER_TOKEN, serde_json::json!({ "assignees": ["tb_julie", "tb_nobody"] }));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(codes(&r), vec!["assignee_unknown"], "{}", r.body);
        let v = json(&r.body);
        assert!(
            v["warnings"][0]["hint"]
                .as_str()
                .expect("hint")
                .starts_with("`tb_nobody` is not a user on this server"),
            "{}",
            r.body
        );
        assert_eq!(v["assignees"], serde_json::json!(["tb_julie", "tb_nobody"]), "{}", r.body);

        // fyi is exempt.
        let r = create(OWNER_TOKEN, serde_json::json!({ "kind": "fyi" }));
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(codes(&r).is_empty(), "fyi is exempt: {}", r.body);

        // A Connect member is a person: no assignee needed.
        let r = create(&member, serde_json::json!({}));
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(codes(&r).is_empty(), "people never must assign: {}", r.body);

        // assign: POST warns on an unknown name; GET is 405.
        let id = json(&create(OWNER_TOKEN, serde_json::json!({ "assignees": ["owner"] })).body)["id"]
            .as_str()
            .expect("id")
            .to_string();
        let r = http(
            d.port,
            "POST",
            &format!("/cli/feedback/assign?token={OWNER_TOKEN}"),
            Some(&serde_json::json!({ "id": id, "usernames": ["tb_nobody"] }).to_string()),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(codes(&r), vec!["assignee_unknown"], "{}", r.body);
        let r = http(d.port, "GET", &format!("/cli/feedback/assign?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 405, "{}", r.body);
    });
}

/// `k2 tickets template` (static, no daemon) passes the cleaner with no
/// warnings: nothing stripped, and it has the `.k2-need` section.
#[test]
fn the_cli_template_cleans_without_warnings() {
    let cli = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let empty_home = std::env::temp_dir().join(format!("tb-tpl-home-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&empty_home).expect("empty HOME");
    let out = std::process::Command::new("bash")
        .arg(&cli)
        .args(["tickets", "template"])
        .env_clear()
        .env("HOME", &empty_home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .output()
        .expect("run cli/k2 tickets template");
    std::fs::remove_dir_all(&empty_home).expect("cleanup");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let tpl = String::from_utf8(out.stdout).expect("utf-8 template");
    let c = brief::clean(&tpl).expect("the template is a valid brief");
    assert!(!c.removed, "the cleaner removed something from the template:\n{tpl}");
    assert!(c.has_need, "the template has a k2-need section");
    assert!(c.warnings().is_empty(), "{:?}", c.warnings());
}

// ── T12: teaching ───────────────────────────────────────────────────────

/// Every generator that teaches `tickets ask` also teaches the brief,
/// and the skill versions moved so existing SKILL.md files re-roll.
#[test]
fn teaching_sites_teach_the_brief() {
    use k2_core::skills::content::{
        generate_custom_agent_skill_content, generate_k2so_agent_skill_content,
        generate_manager_skill_content,
    };
    use k2_core::skills::version;

    let tooling = k2_core::workspace::skill_regen::AGENTS_MD_TOOLING_SECTION;
    assert!(tooling.contains("with an HTML brief (`k2 study ticket-brief`)"), "{tooling}");
    assert!(
        tooling.contains("assigned to a user on this server (`--assign <user>`; required in a future update)"),
        "{tooling}"
    );

    let pp = format!("/tmp/tb-teach-{}", uuid::Uuid::new_v4());
    for (name, body) in [
        ("manager", generate_manager_skill_content(&pp, "P")),
        ("custom", generate_custom_agent_skill_content("P", "a")),
        ("k2so-agent", generate_k2so_agent_skill_content("P", "a")),
    ] {
        assert!(body.contains("k2so tickets template > brief.html"), "{name}");
        assert!(body.contains("--html brief.html"), "{name}");
        assert!(body.contains("k2 study ticket-brief"), "{name}");
        assert!(body.contains("--html brief.html --assign <user>"), "{name}");
        assert!(body.contains("Assign every ticket to a user on this server"), "{name}");
        assert!(body.contains("required in a future update"), "{name}");
        assert!(body.contains("k2 connections list --users"), "{name}");
    }
    assert_eq!(version::SKILL_VERSION_MANAGER, 13);
    assert_eq!(version::SKILL_VERSION_K2SO_AGENT, 13);
    assert_eq!(version::SKILL_VERSION_CUSTOM_AGENT, 13);
    assert_eq!(version::SKILL_VERSION_WORKSPACE, 30);

    // The loadable k2-cli skill the compose path writes into a workspace
    // (temp HOME so nothing lands in the real one).
    let _g = lock();
    with_temp_home(|| {
        let ws = std::env::temp_dir().join(format!("tb-teach-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&ws).expect("workspace dir");
        let ws_s = ws.to_string_lossy().to_string();
        k2_core::workspace::skill_regen::write_workspace_skill_file(&ws_s);
        let skill =
            std::fs::read_to_string(ws.join(".k2/skills/k2-cli/SKILL.md")).expect("k2-cli skill");
        assert!(skill.contains("k2 tickets template > brief.html"), "{skill}");
        assert!(skill.contains("k2 tickets ask \"<title>\" --html brief.html"), "{skill}");
        assert!(skill.contains("k2 study ticket-brief"), "{skill}");
        assert!(
            skill.contains("k2 tickets ask \"<title>\" --html brief.html --assign <user>"),
            "{skill}"
        );
        assert!(skill.contains("k2 tickets assign <id> <user>"), "{skill}");
        assert!(skill.contains("required in a future update"), "{skill}");
        std::fs::remove_dir_all(&ws).expect("cleanup");
    });
}
