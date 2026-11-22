//! The shell's Python follows no redirect (`_K2_PY_PROXY_RULE`'s
//! `_K2NoRedirect`): every request carries a token, so a 3xx urllib would
//! follow ends the block with one line on stderr and exit 3, and nothing
//! goes to the place it names. The binary's urllib families answer the
//! same through the shared opener's `_ruled` pair
//! (`transport::urllib::Opener::open_ruled`), on the parity rig: the same
//! requests, stdout, stderr (each side's stub port masked) and exit.
//!
//! - `calendar` (`_cal_py`'s `call`, with its tries, and `link`'s
//!   `call_or_owner`), `project` (`_project_py`), `mail` (`_mail_py`'s
//!   `call`, with its tries, and the reader's own `call`).
//! - `agent`: `retire` and `restart` (their POST after the conf), `set`
//!   (its `call` after the conf), `context` (`_context_http`) and `hire`
//!   (its `http_get`); `db` (`_db_py`), `preset` (`_preset_py`), `feedback
//!   ask` (its `request`) and `heartbeat wake` (its `request`, whose
//!   `except SystemExit: raise` passes the exit on).
//! - Each code urllib's handler takes (301, 302, 303, 307, 308), to http
//!   and https, relative, with a user part, with bytes past ASCII (quoted
//!   in the line as the handler quotes them).
//! - What it doesn't take, which stays the family's HTTP error: a 3xx with
//!   no `Location`, and a scheme the handler refuses on its own.
//! - The place a redirect names is listened on, and gets no connection.
//!
//! - A python3 before 3.11 has no handler for 308: the shell's answer
//!   there is the family's HTTP error (exit 1, nothing followed), the
//!   binary's the refusal, as with 3.11 and newer.
//!
//! tests/cli/py_no_redirect.sh holds the same rule from outside, with
//! Python's audit hook on the shell side.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::net::TcpListener;

/// The line `_K2NoRedirect` writes: from the stub (`{port}`) to `there`.
fn refused(code: u16, there: &str) -> String {
    let p = k2_client::product_name!();
    format!(
        "refused: 127.0.0.1:{{port}} answered {code}, sending {p} to {there}; {p} does not follow a redirect with your token or password, so nothing was sent there\n"
    )
}

/// The daemon's error as a body: a 3xx the handler doesn't take is the
/// family's HTTP error, and dies on it.
const MOVED: &str = r#"{"error":{"code":"moved","hint":"h"}}"#;

fn redirect(code: u16, location: &str) -> Reply {
    Reply::Headed(
        code,
        vec![("Location".to_string(), location.to_string())],
        MOVED.to_string(),
    )
}

/// A row: its case, and the stderr bash must print (`None`: the family's
/// own answer, compared with the binary's only).
struct Row {
    case: Case,
    stderr: Option<String>,
}

fn refusal(name: &'static str, args: &'static [&'static str], reply: Reply, stderr: String) -> Row {
    Row {
        case: case(name, args, &[], vec![reply], owner_and_port, 3, 1),
        stderr: Some(stderr),
    }
}

/// A refusal after `before`, the answers the family reads first (over
/// curl, or urllib that isn't redirected).
fn refusal_after(
    name: &'static str,
    args: &'static [&'static str],
    before: Vec<Reply>,
    reply: Reply,
    stderr: String,
) -> Row {
    let n = before.len() + 1;
    let mut replies = before;
    replies.push(reply);
    Row {
        case: case(name, args, &[], replies, owner_and_port, 3, n),
        stderr: Some(stderr),
    }
}

fn not_followed(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Row {
    Row {
        case: case(name, args, &[], vec![reply], owner_and_port, code, 1),
        stderr: None,
    }
}

/// A row refusing a 308.
fn is_308(row: &Row) -> bool {
    row.stderr
        .as_deref()
        .is_some_and(|e| e.contains(" answered 308, "))
}

/// Each `127.0.0.1:<the stub's port>` as `127.0.0.1:{port}`.
fn mask_port(text: &str, port: u16) -> String {
    text.replace(&format!("127.0.0.1:{port}"), "127.0.0.1:{port}")
}

/// Both sides of each row, then no connection to `there`.
fn check(rig: &Rig, there: &TcpListener, rows: Vec<Row>) {
    for row in rows {
        let c = &row.case;
        let mut runs = Vec::new();
        for bash in [true, false] {
            let side = if bash { Side::Bash } else { Side::Rust };
            rig.prepare(side, c);
            let port = rig.stub().port;
            let out = rig.run(bash, c);
            assert_eq!(rig.stub().left(), 0, "{}: answers left: {out:?}", c.name);
            let log: Vec<String> = rig
                .read(side, !bash, c.name)
                .iter()
                .map(|r| {
                    normalise(r, !bash, rig.noun(), c.name)
                        .lines()
                        .map(|l| {
                            if l.starts_with("Host: ") {
                                "Host: {port}".to_string()
                            } else {
                                l.to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .collect();
            let stderr = mask_port(&out.stderr, port);
            runs.push((out, stderr, log));
        }
        let (b, r) = (&runs[0], &runs[1]);
        println!("-- {}: bash {:?}", c.name, b.0);
        assert_eq!(b.2.len(), c.requests, "{}: bash's requests", c.name);
        if is_308(&row) && python3_version() < (3, 11) {
            // urllib before 3.11 takes no 308 (its handler has none), so
            // the shell's Python never reaches _K2NoRedirect: the family's
            // HTTP error, exit 1, and nothing goes to the place it names.
            // The binary refuses it as 3.11 and newer do.
            assert_eq!(b.0.code, Some(1), "{}: bash's exit", c.name);
            assert!(!b.1.contains("refused: "), "{}: bash's stderr", c.name);
            assert_eq!(r.2, b.2, "{}: the requests", c.name);
            assert_eq!(r.0.code, Some(c.code), "{}: the exit", c.name);
            assert_eq!(r.0.stdout, "", "{}: stdout", c.name);
            assert_eq!(Some(&r.1), row.stderr.as_ref(), "{}: stderr", c.name);
            continue;
        }
        assert_eq!(b.0.code, Some(c.code), "{}: bash's exit", c.name);
        if let Some(want) = &row.stderr {
            assert_eq!(&b.1, want, "{}: bash's stderr", c.name);
            assert_eq!(b.0.stdout, "", "{}: bash's stdout", c.name);
        }
        assert_eq!(r.2, b.2, "{}: the requests", c.name);
        assert_eq!(r.0.code, b.0.code, "{}: the exit", c.name);
        assert_eq!(r.0.stdout, b.0.stdout, "{}: stdout", c.name);
        assert_eq!(r.1, b.1, "{}: stderr (the stub's port masked)", c.name);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    match there.accept() {
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok((_, from)) => panic!("a redirect was followed: a connection from {from}"),
        Err(e) => panic!("accept: {e}"),
    }
}

/// The place every redirect names, listened on.
fn there() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    l.set_nonblocking(true).expect("nonblocking");
    (l, port)
}

#[test]
fn calendar_follows_no_redirect() {
    let rig = Rig::new("calendar", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "list --json, 302 to http",
            &["list", "--json"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "list, 307 to https",
            &["list"],
            redirect(307, &format!("https://{at}/other-host")),
            refused(307, &at),
        ),
        refusal(
            "events, 303 relative",
            &["events"],
            redirect(303, "/moved?x=1"),
            refused(303, "127.0.0.1:{port}"),
        ),
        refusal(
            "link's call_or_owner, 308 with a user part",
            &[
                "link",
                "a@x.example",
                "--provider",
                "google",
                "--workspace",
                "w",
            ],
            redirect(308, &format!("http://u:pw@{at}/x")),
            refused(308, &at),
        ),
        not_followed(
            "list, a 302 with no Location",
            &["list"],
            Reply::Answer(302, MOVED.to_string()),
            1,
        ),
        not_followed(
            "list, a 302 to a scheme urllib refuses",
            &["list"],
            redirect(302, "mailto:a@x.example"),
            1,
        ),
    ];
    check(&rig, &l, rows);
}

#[test]
fn project_follows_no_redirect() {
    let rig = Rig::new("project", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "list --json, 302 to http",
            &["list", "--json"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "list, 301 to https",
            &["list"],
            redirect(301, &format!("https://{at}/other-host")),
            refused(301, &at),
        ),
        refusal(
            "list, a host past ASCII",
            &["list"],
            redirect(302, "http://h\u{e9} x.example/"),
            refused(302, "h%C3%A9%20x.example"),
        ),
        not_followed(
            "list, a 302 with no Location",
            &["list"],
            Reply::Answer(302, MOVED.to_string()),
            1,
        ),
    ];
    check(&rig, &l, rows);
}

#[test]
fn mail_follows_no_redirect() {
    let rig = Rig::new("mail", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "domains --json, 302 to http",
            &["domains", "--json"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "domains, 307 to https",
            &["domains"],
            redirect(307, &format!("https://{at}/other-host")),
            refused(307, &at),
        ),
        refusal(
            "reader inboxes, 302 to http",
            &["reader", "inboxes"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        not_followed(
            "domains, a 302 with no Location",
            &["domains"],
            Reply::Answer(302, MOVED.to_string()),
            1,
        ),
    ];
    check(&rig, &l, rows);
}

/// `agent conf`'s answer for `qa` (agent_parity.rs's `CONF`).
const CONF: &str = r#"{"ok":true,"name":"qa","path":"/srv/qa","mode":"agent","enabled":true,"personaPath":null,"connections":[],"projects":[],"live":{"active":true,"sessionId":"sess-0123456789","uptimeSec":3725},"charterProblems":[]}"#;

#[test]
fn agent_follows_no_redirect() {
    let rig = Rig::new("agent", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let conf = || vec![ok(CONF)];
    let rows = vec![
        refusal_after(
            "retire, 302 to http",
            &["retire", "qa"],
            conf(),
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal_after(
            "restart, 307 to https",
            &["restart", "qa"],
            conf(),
            redirect(307, &format!("https://{at}/other-host")),
            refused(307, &at),
        ),
        refusal_after(
            "set, 302 relative",
            &["set", "qa", "--mode", "manager"],
            conf(),
            redirect(302, "/moved"),
            refused(302, "127.0.0.1:{port}"),
        ),
        refusal(
            "context list, 308 to http",
            &["context", "list"],
            redirect(308, &format!("http://{at}/x")),
            refused(308, &at),
        ),
        refusal(
            "hire, 301 to https",
            &["hire", "newbie"],
            redirect(301, &format!("https://u:pw@{at}/y")),
            refused(301, &at),
        ),
    ];
    check(&rig, &l, rows);
}

#[test]
fn db_follows_no_redirect() {
    let rig = Rig::new("db", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "status, 302 to http",
            &["status"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "status --json, 307 to https",
            &["status", "--json"],
            redirect(307, &format!("https://{at}/other-host")),
            refused(307, &at),
        ),
    ];
    check(&rig, &l, rows);
}

#[test]
fn preset_follows_no_redirect() {
    let rig = Rig::new("preset", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "list, 302 to http",
            &["list"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "list --json, 303 to https",
            &["list", "--json"],
            redirect(303, &format!("https://{at}/other-host")),
            refused(303, &at),
        ),
    ];
    check(&rig, &l, rows);
}

#[test]
fn feedback_ask_follows_no_redirect() {
    let rig = Rig::new("feedback", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "ask, 302 to http",
            &["ask", "Ship it?"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "ask --json, 308 to https",
            &["ask", "Ship it?", "--json"],
            redirect(308, &format!("https://{at}/other-host")),
            refused(308, &at),
        ),
    ];
    check(&rig, &l, rows);
}

#[test]
fn heartbeat_wake_follows_no_redirect() {
    let rig = Rig::new("heartbeat", "noredir");
    let (l, tp) = there();
    let at = format!("127.0.0.1:{tp}");
    let rows = vec![
        refusal(
            "wake, 302 to http",
            &["wake"],
            redirect(302, &format!("http://{at}/elsewhere")),
            refused(302, &at),
        ),
        refusal(
            "wake, 307 to https",
            &["wake"],
            redirect(307, &format!("https://{at}/other-host")),
            refused(307, &at),
        ),
    ];
    check(&rig, &l, rows);
}
