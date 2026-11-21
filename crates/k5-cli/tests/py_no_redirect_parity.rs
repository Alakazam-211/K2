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
//! - Each code urllib's handler takes (301, 302, 303, 307, 308), to http
//!   and https, relative, with a user part, with bytes past ASCII (quoted
//!   in the line as the handler quotes them).
//! - What it doesn't take, which stays the family's HTTP error: a 3xx with
//!   no `Location`, and a scheme the handler refuses on its own.
//! - The place a redirect names is listened on, and gets no connection.
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

fn not_followed(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Row {
    Row {
        case: case(name, args, &[], vec![reply], owner_and_port, code, 1),
        stderr: None,
    }
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
