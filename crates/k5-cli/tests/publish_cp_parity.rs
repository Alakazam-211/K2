//! `k5-cli publish` against the shell CLI's `cli/k2 publish`, on the
//! parity rig ([`common::parity`]), for the verbs made native last:
//!
//! - `status` with no name and `subdomain create`, `list`, `point`
//!   (`target`), `rm` (`remove`, `delete`) and any other word: the K2
//!   Connect control plane through the daemon (`/cli/connect/proxy`),
//!   every reader's lines, the daemon's refusals, no answer, the air-gap,
//!   the base check, and the control plane itself when no daemon makes the
//!   call (`tunnel.json`'s token, only as the `Authorization` header).
//! - `status <name>`, `client-ip`, `secret` (`secrets`) and `env`.
//! - Help never acts; a large list (above 256 KiB).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::sync::{Arc, Mutex};

const VERB: &str = "publish";

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The control plane's answer as the daemon relays it.
fn relayed(status: u16, body: &str) -> Reply {
    let b = body.replace('\\', "\\\\").replace('"', "\\\"");
    ok(leak(format!(r#"{{"status":{status},"body":"{b}"}}"#)))
}

fn on(
    name: &'static str,
    args: &'static [&'static str],
    replies: Vec<Reply>,
    code: i32,
    n: usize,
) -> Case {
    case(name, args, &[], replies, owner_and_port, code, n)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn subdomain_acme(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.k2home.join("tunnel.json"), r#"{"subdomain":" ACME "}"#).unwrap();
}

fn tunnel_token(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.k2home.join("tunnel.json"),
        r#"{"subdomain":"acme","token":"tun-tok"}"#,
    )
    .unwrap();
}

fn check_all(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

const SUBS: &str = r#"{"subdomains":[{"host":"acme.k2.dev","label":"acme","target":"localhost:3000","tier":"pro","primary":true,"connected":true,"effective":"active"},{"label":"staging","target":"localhost:4000","status":"pending","tier":"Single"},{"host":"x.acme.k2.dev","target":null}]}"#;
const CHECKED: &str = r#"{"success":true}"#;

#[test]
fn status_and_list_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "cpread");
    check_all(
        &rig,
        vec![
            on(
                "status: the connected row",
                &["status"],
                vec![relayed(200, SUBS)],
                0,
                1,
            ),
            on(
                "status --json",
                &["status", "--json"],
                vec![relayed(200, SUBS)],
                0,
                1,
            ),
            on(
                "status: single",
                &["status"],
                vec![relayed(
                    200,
                    r#"{"subdomains":[{"label":"a","current":1,"tier":" FREE "}]}"#,
                )],
                0,
                1,
            ),
            on(
                "status: a tier of its own",
                &["status"],
                vec![relayed(
                    200,
                    r#"{"subdomains":[{"host":"h","is_current":true,"tier":"gOLD"}]}"#,
                )],
                0,
                1,
            ),
            case(
                "status: this computer's label",
                &["status", "--json"],
                &[],
                vec![relayed(
                    200,
                    r#"{"subdomains":[{"host":"Acme.k2.dev","tier":"single"}]}"#,
                )],
                subdomain_acme,
                0,
                1,
            ),
            traceback(case(
                "status: no row for this computer (the snippet's sys.exit raises)",
                &["status"],
                &[],
                vec![relayed(200, r#"{"subdomains":[]}"#)],
                subdomain_acme,
                1,
                1,
            )),
            traceback(on(
                "status: not connected (the snippet's sys.exit raises)",
                &["status"],
                vec![relayed(200, r#"{}"#)],
                1,
                1,
            )),
            on(
                "status --json: not connected",
                &["status", "--json"],
                vec![relayed(200, "[]")],
                0,
                1,
            ),
            traceback(on(
                "status: a row that is no dict",
                &["status"],
                vec![relayed(200, r#"{"subdomains":["a"]}"#)],
                1,
                1,
            )),
            traceback(on(
                "status --json: a tier that is no string",
                &["status", "--json"],
                vec![relayed(200, r#"{"subdomains":[{"connected":1,"tier":5}]}"#)],
                1,
                1,
            )),
            on(
                "status: the control plane refuses the token",
                &["status"],
                vec![relayed(401, r#"{"error":"invalid_token"}"#)],
                1,
                1,
            ),
            on(
                "status --json: the control plane refuses",
                &["status", "--json"],
                vec![relayed(403, r#"{"error":"nope","detail":{"a":1}}"#)],
                1,
                1,
            ),
            on(
                "status: an empty body (no newline left)",
                &["status"],
                vec![relayed(204, "")],
                1,
                1,
            ),
            on(
                "status --json: an empty body",
                &["status", "--json"],
                vec![ok(r#"{"status":204,"body":"\n"}"#)],
                1,
                1,
            ),
            on(
                "status: the daemon's 200 is not JSON",
                &["status"],
                vec![ok("nope")],
                1,
                1,
            ),
            on(
                "status: no body in the 200",
                &["status"],
                vec![ok(r#"{"status":200}"#)],
                1,
                1,
            ),
            on(
                "status: a status that is no number",
                &["status"],
                vec![ok(r#"{"status":"x","body":""}"#)],
                1,
                1,
            ),
            on(
                "status: refused with no code (needs the owner)",
                &["status"],
                vec![Reply::Answer(403, r#"{"error":"no"}"#.into())],
                3,
                1,
            ),
            on(
                "status --json: refused with no code",
                &["status", "--json"],
                vec![Reply::Answer(403, "".into())],
                3,
                1,
            ),
            on(
                "status: a code and an error",
                &["status"],
                vec![Reply::Answer(
                    500,
                    r#"{"code":"cp_down","error":{"hint":"try later"}}"#.into(),
                )],
                1,
                1,
            ),
            on(
                "status: a code alone",
                &["status"],
                vec![Reply::Answer(502, r#"{"code":"cp_down"}"#.into())],
                1,
                1,
            ),
            on(
                "status: nothing to read",
                &["status"],
                vec![Reply::Answer(500, "x".into())],
                1,
                1,
            ),
            on(
                "status: no route, no tunnel token",
                &["status"],
                vec![Reply::Answer(404, "not found".into())],
                1,
                1,
            ),
            on(
                "status --json: no answer to a GET, no tunnel token",
                &["status", "--json"],
                vec![Reply::Close],
                1,
                1,
            ),
            with(
                vec![("K2_CONNECT_BASE", "cp.example.invalid".into())],
                on("status: a base with no scheme", &["status"], vec![], 2, 0),
            ),
            with(
                vec![("K2_CONNECT_BASE", "http://x.localhost:9".into())],
                on(
                    "subdomain list: plain http to another machine",
                    &["subdomain", "list"],
                    vec![],
                    2,
                    0,
                ),
            ),
            with(
                vec![("K2_AIRGAP", "1".into())],
                on("status: air-gap on", &["status"], vec![], 1, 0),
            ),
            with(
                vec![("K2_AIRGAP", "1".into())],
                case(
                    "subdomain: air-gap on, before the gates (no daemon)",
                    &["subdomain"],
                    &[],
                    vec![],
                    nothing,
                    1,
                    0,
                ),
            ),
            on("status: a flag", &["status", "-x"], vec![], 2, 0),
            on(
                "status: a flag after --json",
                &["status", "--json", "--long"],
                vec![],
                2,
                0,
            ),
            on("status: two names", &["status", "a", "b"], vec![], 2, 0),
            on(
                "subdomain alone: the list",
                &["subdomain"],
                vec![relayed(200, SUBS)],
                0,
                1,
            ),
            case(
                "subdomain list",
                &["subdomain", "list"],
                &[],
                vec![relayed(200, SUBS)],
                subdomain_acme,
                0,
                1,
            ),
            case(
                "subdomain list --json",
                &["subdomain", "list", "--json"],
                &[],
                vec![relayed(200, SUBS)],
                subdomain_acme,
                0,
                1,
            ),
            on(
                "subdomain list: none",
                &["subdomain", "list"],
                vec![relayed(200, r#"{"subdomains":[]}"#)],
                0,
                1,
            ),
            on(
                "subdomain list: not JSON",
                &["subdomain", "list"],
                vec![relayed(200, "x")],
                0,
                1,
            ),
            traceback(on(
                "subdomain list: a host that is no string",
                &["subdomain", "list"],
                vec![relayed(200, r#"{"subdomains":[{"host":5}]}"#)],
                1,
                1,
            )),
            traceback(on(
                "subdomain list --json: subdomains is a number",
                &["subdomain", "list", "--json"],
                vec![relayed(200, r#"{"subdomains":3}"#)],
                1,
                1,
            )),
            on(
                "subdomain list: Pro only",
                &["subdomain", "list"],
                vec![relayed(403, r#"{"error":"pro_required","detail":"acme"}"#)],
                1,
                1,
            ),
            on(
                "subdomain list: a known error with a detail",
                &["subdomain", "list"],
                vec![relayed(400, r#"{"error":"bad_label","detail":" x "}"#)],
                1,
                1,
            ),
            on(
                "subdomain list: unauthorized",
                &["subdomain", "list"],
                vec![relayed(401, "{}")],
                1,
                1,
            ),
            on(
                "subdomain list: an error of its own",
                &["subdomain", "list"],
                vec![relayed(409, r#"{"error":"weird","detail":"d"}"#)],
                1,
                1,
            ),
            on(
                "subdomain list: plain text",
                &["subdomain", "list"],
                vec![relayed(500, "boom")],
                1,
                1,
            ),
            traceback(on(
                "subdomain list: an error that is no string",
                &["subdomain", "list"],
                vec![relayed(500, r#"{"error":7}"#)],
                1,
                1,
            )),
            on(
                "subdomain list --json: an error",
                &["subdomain", "list", "--json"],
                vec![relayed(500, r#"{"error":"e"}"#)],
                1,
                1,
            ),
            on(
                "subdomain list: a flag",
                &["subdomain", "list", "--all"],
                vec![],
                2,
                0,
            ),
            on(
                "subdomain list: a word",
                &["subdomain", "list", "x"],
                vec![],
                2,
                0,
            ),
            on(
                "subdomain: an unknown word",
                &["subdomain", "nope"],
                vec![],
                1,
                0,
            ),
        ],
    );
}

const CREATED: &str = r#"{"label":"staging","host":"staging.acme.k2.dev","target":"example.invalid:80","status":"active","cert_status":"issued","effective":"active"}"#;

#[test]
fn create_point_and_rm_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "cpwrite");
    check_all(
        &rig,
        vec![
            on(
                "create",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![
                    ok(CHECKED),
                    relayed(201, CREATED),
                    ok(r#"{"success":true,"projectId":"p1"}"#),
                    ok("{}"),
                ],
                0,
                4,
            ),
            on(
                "create --transfer, a local target nothing listens on",
                &[
                    "subdomain",
                    "create",
                    "--transfer",
                    "staging",
                    "--json",
                    "--target",
                    "localhost:1",
                ],
                vec![
                    ok(CHECKED),
                    relayed(201, r#"{"label":"staging","target":null}"#),
                    ok(CHECKED),
                    ok("{}"),
                ],
                0,
                4,
            ),
            on(
                "create: the preflight refuses",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![ok(
                    r#"{"error":"owned by another workspace","hint":"ask its owner"}"#,
                )],
                1,
                1,
            ),
            on(
                "create: the preflight has no answer",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![ok("")],
                1,
                1,
            ),
            on(
                "create: the label is taken",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![ok(CHECKED), relayed(409, r#"{"error":"label_taken"}"#)],
                1,
                2,
            ),
            on(
                "create: the stamp fails",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![
                    ok(CHECKED),
                    relayed(201, CREATED),
                    ok(r#"{"error":"busy"}"#),
                ],
                1,
                3,
            ),
            traceback(on(
                "create: the answer is a list",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![ok(CHECKED), relayed(201, "[1]")],
                1,
                2,
            )),
            waited(on(
                "create: no answer from the proxy (never sent again)",
                &[
                    "subdomain",
                    "create",
                    "staging",
                    "--target",
                    "example.invalid:80",
                ],
                vec![ok(CHECKED), Reply::Close],
                1,
                2,
            )),
            with(
                vec![("K2_AIRGAP", "yes".into())],
                on(
                    "create: air-gap on, before the gates",
                    &[
                        "subdomain",
                        "create",
                        "staging",
                        "--target",
                        "example.invalid:80",
                    ],
                    vec![],
                    1,
                    0,
                ),
            ),
            on(
                "create: no target",
                &["subdomain", "create", "staging"],
                vec![],
                1,
                0,
            ),
            on(
                "create: --target with nothing after it",
                &["subdomain", "create", "staging", "--target"],
                vec![],
                1,
                0,
            ),
            on(
                "create: a reserved label",
                &["subdomain", "create", " Skin", "--target", "x:1"],
                vec![],
                1,
                0,
            ),
            on(
                "create: meet",
                &["subdomain", "create", "MEET", "--target", "x:1"],
                vec![],
                1,
                0,
            ),
            with(
                vec![("K2_CONNECT_BASE", "ftp://cp.example".into())],
                on(
                    "create: a base that is not https",
                    &["subdomain", "create", "a", "--target", "x:1"],
                    vec![],
                    2,
                    0,
                ),
            ),
            on(
                "point",
                &[
                    "subdomain",
                    "point",
                    "staging",
                    "--target",
                    "example.invalid:81",
                ],
                vec![
                    ok(CHECKED),
                    relayed(
                        200,
                        r#"{"label":"staging","target":"example.invalid:81","status":"active"}"#,
                    ),
                    ok(CHECKED),
                    ok("{}"),
                ],
                0,
                4,
            ),
            on(
                "target <label> <endpoint>",
                &[
                    "subdomain",
                    "target",
                    "st ag",
                    "example.invalid:81",
                    "extra",
                ],
                vec![ok(CHECKED), relayed(200, "{}"), ok(CHECKED), ok("{}")],
                0,
                4,
            ),
            on(
                "point: no endpoint",
                &["subdomain", "point", "staging"],
                vec![],
                1,
                0,
            ),
            on(
                "target: chat",
                &["subdomain", "target", "chat", "x:1"],
                vec![],
                1,
                0,
            ),
            on(
                "rm",
                &["subdomain", "rm", "staging", "extra"],
                vec![
                    ok(CHECKED),
                    relayed(200, r#"{"removed":"staging.acme.k2.dev"}"#),
                    ok(CHECKED),
                    ok("{}"),
                ],
                0,
                4,
            ),
            on(
                "remove skin (no preflight)",
                &["subdomain", "remove", "SKIN"],
                vec![relayed(200, "{}"), ok(CHECKED), ok("{}")],
                0,
                3,
            ),
            on(
                "delete: the unclaim fails",
                &["subdomain", "delete", "staging"],
                vec![ok(CHECKED), relayed(200, "{}"), ok("x")],
                1,
                3,
            ),
            on(
                "rm: not found",
                &["subdomain", "rm", "staging"],
                vec![ok(CHECKED), relayed(404, r#"{"error":"not_found"}"#)],
                1,
                2,
            ),
            on("rm: no label", &["subdomain", "rm"], vec![], 1, 0),
        ],
    );
}

/// A control plane on loopback that answers `status` and `body` to
/// every request and keeps each request's bytes.
fn control_plane(status: &'static str, body: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let mut c = conn.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = c.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let t = String::from_utf8_lossy(&buf).into_owned();
                if let Some((head, rest)) = t.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|h| h.strip_prefix("Content-Length: "))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    if rest.len() >= len {
                        break;
                    }
                }
            }
            keep.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf).into_owned());
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            c.write_all(reply.as_bytes()).unwrap();
        }
    });
    (port, seen)
}

/// Every side sent the control plane the same request (the User-Agent
/// aside), starting with `first`, the token only as its Authorization
/// header and nothing of the daemon's.
fn same_requests(seen: &Arc<Mutex<Vec<String>>>, first: &str) {
    let got = seen.lock().unwrap().clone();
    assert!(got.len() >= 2, "one request from each run: {got:?}");
    let masked: Vec<String> = got
        .iter()
        .map(|r| {
            r.split("\r\n")
                .map(|l| {
                    if l.starts_with("User-Agent: ") {
                        "User-Agent: <ua>"
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\r\n")
        })
        .collect();
    for m in &masked[1..] {
        assert_eq!(&masked[0], m, "the shell's, then the binary's");
    }
    for r in &got {
        assert!(r.starts_with(first), "{r}");
        let with_token: Vec<&str> = r.split("\r\n").filter(|l| l.contains("tun-tok")).collect();
        assert_eq!(with_token, ["Authorization: Bearer tun-tok"], "{r}");
        assert!(!r.contains("X-K5-"), "{r}");
    }
}

/// One direct control-plane row: the name, the words, the control
/// plane's status line and body, the daemon's answers and how many, and
/// how the control plane's request starts.
type CpRow = (
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
    Vec<Reply>,
    usize,
    &'static str,
);

/// No daemon route (404 from /cli/connect/proxy): the control plane
/// itself with `tunnel.json`'s token, for each method.
#[test]
fn the_control_plane_itself_when_no_daemon_makes_the_call() {
    let rig = Rig::new(VERB, "cplane");
    let rows: [CpRow; 4] = [
        (
            "list: GET",
            &["subdomain", "list"],
            "200 OK",
            r#"{"subdomains":[{"label":"acme","tier":"pro"}]}"#,
            vec![Reply::Answer(404, "not found".into())],
            1,
            "GET /subdomains HTTP/1.1\r\n",
        ),
        (
            "create: POST",
            &[
                "subdomain",
                "create",
                "staging",
                "--target",
                "example.invalid:80",
            ],
            "201 Created",
            CREATED,
            vec![
                ok(CHECKED),
                Reply::Answer(405, "".into()),
                ok(CHECKED),
                ok("{}"),
            ],
            4,
            "POST /subdomains HTTP/1.1\r\n",
        ),
        (
            "point: PUT",
            &[
                "subdomain",
                "point",
                "st/ag",
                "--target",
                "example.invalid:80",
            ],
            "200 OK",
            CREATED,
            vec![
                ok(CHECKED),
                Reply::Answer(404, "".into()),
                ok(CHECKED),
                ok("{}"),
            ],
            4,
            "PUT /subdomains/st%2Fag HTTP/1.1\r\n",
        ),
        (
            "rm: DELETE",
            &["subdomain", "rm", "staging"],
            "200 OK",
            r#"{"removed":"staging"}"#,
            vec![
                ok(CHECKED),
                Reply::Answer(404, "".into()),
                ok(CHECKED),
                ok("{}"),
            ],
            4,
            "DELETE /subdomains/staging HTTP/1.1\r\n",
        ),
    ];
    for (name, args, status, body, replies, n, first) in rows {
        let (port, seen) = control_plane(status, body);
        let base = format!("http://127.0.0.1:{port}");
        rig.check(&with(
            vec![("K2_CONNECT_BASE", base)],
            case(name, args, &[], replies, tunnel_token, 0, n),
        ));
        same_requests(&seen, first);
    }
    // The control plane's own error, and no control plane at all.
    let (port, seen) = control_plane("401 Unauthorized", r#"{"error":"invalid_token"}"#);
    rig.check(&with(
        vec![("K2_CONNECT_BASE", format!("http://127.0.0.1:{port}"))],
        case(
            "status: the control plane refuses",
            &["status", "--json"],
            &[],
            vec![Reply::Answer(404, "".into())],
            tunnel_token,
            1,
            1,
        ),
    ));
    same_requests(&seen, "GET /subdomains HTTP/1.1\r\n");
    rig.check(&with(
        vec![(
            "K2_CONNECT_BASE",
            format!("http://127.0.0.1:{}", dead_port()),
        )],
        case(
            "status: no control plane either",
            &["status"],
            &[],
            vec![Reply::Answer(404, "".into())],
            tunnel_token,
            1,
            1,
        ),
    ));
}

/// With a token in `tunnel.json`, a proxy setting the binary doesn't
/// read: the shell's (it might have to call the control plane itself).
#[test]
fn a_proxy_it_does_not_read_is_the_shell_s() {
    let rig = Rig::new(VERB, "cpshell");
    rig.check(&shell(with(
        vec![("HTTPS_PROXY", "socks5://127.0.0.1:9".into())],
        case(
            "status, a socks proxy",
            &["status"],
            &[],
            vec![relayed(200, SUBS)],
            tunnel_token,
            0,
            1,
        ),
    )));
}

#[test]
fn app_status_and_client_ip_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "local");
    let list = r#"{"services":[{"name":"Web","status":"running","url":"https://web.acme.k2.dev","desired":"running","probeTimeoutMs":90000},{"name":"api","status":"starting","startingLeftMs":61500,"port":3047,"hostnamePending":true},{"name":"slow","status":"starting","startingLeftMs":-5},{"name":"off","status":"stopped"},{"name":"sick","status":"unhealthy","target":"localhost:9"},{"name":"dead","status":"exited","lastExitCode":3,"error":"boom"},{"name":"gone","status":null},"x"]}"#;
    let list = leak(list.to_string());
    let mut v = Vec::new();
    for (name, args, code) in [
        ("status <name>: running", &["status", " WEB "][..], 0),
        (
            "status <name> --json: running",
            &["status", "web", "--json"],
            0,
        ),
        ("status <name>: starting", &["status", "api"], 2),
        (
            "status <name> --json: starting",
            &["status", "--json", "api"],
            2,
        ),
        (
            "status <name>: starting, past its time",
            &["status", "slow"],
            2,
        ),
        ("status <name>: stopped", &["status", "off"], 1),
        ("status <name>: unhealthy", &["status", "sick"], 1),
        ("status <name>: exited", &["status", "dead"], 1),
        (
            "status <name> --json: exited",
            &["status", "dead", "--json"],
            1,
        ),
        ("status <name>: no status", &["status", "gone"], 1),
        ("status <name>: missing", &["status", "nope"], 1),
        (
            "status <name> --json: missing",
            &["status", "nope", "--json"],
            1,
        ),
    ] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        v.push(on(name, args, vec![ok(list)], code, 1));
    }
    v.extend([
        on(
            "status <name>: an error",
            &["status", "web"],
            vec![ok(r#"{"error":"not_pro"}"#)],
            1,
            1,
        ),
        on(
            "status <name>: an error of null",
            &["status", "web"],
            vec![ok(r#"{"error":null,"x":"é"}"#)],
            1,
            1,
        ),
        on(
            "status <name>: not JSON",
            &["status", "web"],
            vec![ok("x")],
            1,
            1,
        ),
        on(
            "status <name>: a list",
            &["status", "web"],
            vec![ok("[1]")],
            1,
            1,
        ),
        traceback(on(
            "status <name>: a time that is no number",
            &["status", "web"],
            vec![ok(
                r#"{"services":[{"name":"web","status":"starting","startingLeftMs":"soon"}]}"#,
            )],
            1,
            1,
        )),
        traceback(on(
            "status <name>: services is a number",
            &["status", "web"],
            vec![ok(r#"{"services":5}"#)],
            1,
            1,
        )),
        waited(on(
            "status <name>: no answer",
            &["status", "web"],
            vec![Reply::Close],
            1,
            1,
        )),
        on(
            "client-ip",
            &["client-ip"],
            vec![ok(r#"{"labels":["a","b"]}"#)],
            0,
            1,
        ),
        on(
            "client-ip: none",
            &["client-ip"],
            vec![ok(r#"{"labels":[]}"#)],
            0,
            1,
        ),
        on(
            "client-ip --json",
            &["client-ip", "--json"],
            vec![ok(r#"{"labels":["é"]}"#)],
            0,
            1,
        ),
        on(
            "client-ip <label> on",
            &["client-ip", "web", "on"],
            vec![ok(r#"{"label":"web","clientIp":true,"labels":["web"]}"#)],
            0,
            1,
        ),
        on(
            "client-ip <label> false",
            &["client-ip", "web", "false", "--json"],
            vec![ok(r#"{"label":"web","clientIp":false}"#)],
            0,
            1,
        ),
        on(
            "client-ip: a state it doesn't know",
            &["client-ip", "web", "maybe"],
            vec![],
            2,
            0,
        ),
        on("client-ip: no state", &["client-ip", "web"], vec![], 2, 0),
        on("client-ip: a flag", &["client-ip", "-x"], vec![], 2, 0),
        on(
            "client-ip: three words",
            &["client-ip", "a", "on", "b"],
            vec![],
            2,
            0,
        ),
        on(
            "client-ip: an error",
            &["client-ip"],
            vec![ok(r#"{"error":{"code":"x"}}"#)],
            2,
            1,
        ),
        on("client-ip: a list", &["client-ip"], vec![ok("[1]")], 2, 1),
        on(
            "client-ip: not JSON",
            &["client-ip"],
            vec![ok("  nope ")],
            1,
            1,
        ),
        on(
            "client-ip: an empty answer",
            &["client-ip"],
            vec![ok("")],
            1,
            1,
        ),
        traceback(on(
            "client-ip: a label and no clientIp",
            &["client-ip"],
            vec![ok(r#"{"label":"w"}"#)],
            1,
            1,
        )),
        traceback(on(
            "client-ip: a label that is a number",
            &["client-ip"],
            vec![ok(r#"{"labels":[1]}"#)],
            1,
            1,
        )),
    ]);
    check_all(&rig, v);
}

#[test]
fn secret_and_env_hand_over_to_the_vault_as_the_shell_does() {
    let rig = Rig::new(VERB, "secret");
    let grant = r#"{"grant":{"id":"g1","expiresAt":"t"},"reach":"they get it now"}"#;
    let gone = r#"{"recall":"1 file was deleted","stillRunning":[]}"#;
    let service = r#"{"service":"api","mode":"legacy","envFile":".env.k2","secrets":[{"name":"K","delivery":"env","envName":"K"}],"inheritedNames":["PATH"],"envRemovedIn":"0.47","restart":"restart api"}"#;
    check_all(
        &rig,
        vec![
            on(
                "secret alone: the page on stderr",
                &["secret"],
                vec![],
                2,
                0,
            ),
            on("secrets help: the page", &["secrets", "help"], vec![], 0, 0),
            on(
                "secret add",
                &[
                    "secret",
                    "add",
                    "api",
                    "K",
                    "--as",
                    "file",
                    "--env-name",
                    "API_KEY",
                    "--scope",
                    "server",
                ],
                vec![ok(grant)],
                0,
                1,
            ),
            on(
                "secret add --json",
                &["secrets", "add", "api", "K", "--json"],
                vec![ok(grant)],
                0,
                1,
            ),
            on(
                "secret add: no name",
                &["secret", "add", "api"],
                vec![],
                2,
                0,
            ),
            on(
                "secret add --system (vault's refusal)",
                &["secret", "add", "api", "K", "--system"],
                vec![],
                2,
                0,
            ),
            on(
                "secret remove",
                &["secret", "remove", "api", "K", "--scope", "server"],
                vec![ok(gone)],
                0,
                1,
            ),
            on(
                "secret rm: --as is vault's refusal",
                &["secret", "rm", "api", "K", "--as", "file"],
                vec![],
                2,
                0,
            ),
            on(
                "secret remove: no service",
                &["secret", "remove", ""],
                vec![],
                2,
                0,
            ),
            on(
                "secret list",
                &["secret", "list", "api", "extra"],
                vec![ok(service)],
                0,
                1,
            ),
            on("secret ls: no service", &["secret", "ls"], vec![], 2, 0),
            on(
                "secret: a verb it doesn't know",
                &["secret", "show", "api"],
                vec![],
                2,
                0,
            ),
            on("env", &["env", "api"], vec![ok(service)], 0, 1),
            on(
                "env clean --env-file",
                &["env", "api", "clean", "--env-file", ".env.k2"],
                vec![ok(r#"{"service":"api","mode":"clean"}"#)],
                0,
                1,
            ),
            on("env: no service", &["env"], vec![], 2, 0),
        ],
    );
}

/// The verbs that change something, with `-h` or `--help`: the help, no
/// request (the shell's page).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let lines: [&'static [&'static str]; 6] = [
        &["client-ip", "web", "on", "-h"],
        &["secret", "add", "api", "K", "--help"],
        &["secrets", "remove", "api", "K", "-h"],
        &["secret", "--help"],
        &["env", "api", "clean", "--help"],
        &["status", "web", "-h"],
    ];
    let v = lines
        .into_iter()
        .map(|a| shell(on("help", a, vec![], 0, 0)))
        .collect();
    check_all(&rig, v);
}

/// `subdomain create|point|rm` with `-h` or `--help` anywhere: the help,
/// no request (the shell's fix: help never acts; this row checked the
/// shell's bug on bash alone until it landed).
#[test]
fn subdomain_writes_on_help_never_act() {
    let rig = Rig::new(VERB, "helpbug");
    let lines: [&'static [&'static str]; 3] = [
        &["subdomain", "rm", "--help"],
        &[
            "subdomain",
            "create",
            "a",
            "--target",
            "example.invalid:1",
            "-h",
        ],
        &[
            "subdomain",
            "point",
            "a",
            "--target",
            "example.invalid:1",
            "--help",
        ],
    ];
    let v = lines
        .into_iter()
        .map(|a| shell(on("help", a, vec![], 0, 0)))
        .collect();
    check_all(&rig, v);
}

/// A label beginning with `-` is an unknown flag, exit 2, nothing sent
/// (the shell's f523e1e546 and f267f23585: so `-h` can never be a label),
/// and a bare `--` anywhere on a publish line is the prelude's usage error,
/// exit 2, nothing sent (7a07949682 and 865243bd9e: publish takes none).
#[test]
fn a_dash_label_and_a_bare_dashdash_are_usage_errors() {
    let rig = Rig::new(VERB, "dash");
    let lines: [&'static [&'static str]; 17] = [
        // covers: publish subdomain claim
        &["subdomain", "claim", "-x"],
        &["subdomain", "claim", "--transfer", "-x"],
        &["subdomain", "unclaim", "-x"],
        &["subdomain", "create", "-x", "--target", "x"],
        &["subdomain", "create", "--target", "x", "-x"],
        &["subdomain", "point", "-x", "--target", "x"],
        &["subdomain", "point", "--target", "x", "-x"],
        &["subdomain", "target", "-x", "x"],
        &["subdomain", "target", "acme", "-x"],
        &["subdomain", "rm", "-x"],
        &["subdomain", "remove", "-"],
        &["subdomain", "delete", "-x"],
        &["subdomain", "claim", "--", "acme"],
        &["subdomain", "create", "--", "-x", "--target", "x"],
        &["subdomain", "create", "staging", "--target", "x", "--"],
        &["ps", "--"],
        &["--"],
    ];
    let v = lines
        .into_iter()
        .map(|a| on("a usage error", a, vec![], 2, 0))
        .collect();
    check_all(&rig, v);
    // The binary's own words for the two rules.
    let (out, log) = rig.binary(&on("dash", &["subdomain", "rm", "-x"], vec![], 2, 0), false);
    assert_eq!(out.stderr, "Error: unknown flag '-x'\n");
    assert!(log.is_empty(), "{log:?}");
    let (out, _) = rig.binary(&on("dashdash", &["subdomain", "claim", "--", "acme"], vec![], 2, 0), false);
    let cli = k2_client::brand::CLI_NAME;
    assert_eq!(out.stderr, format!("usage: publish takes no '--' ({cli} help publish)\n"));
}

/// The binary's lines name the brand's command and product.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    let rows: Vec<(&'static [&'static str], Vec<Reply>, i32, String)> = vec![
        (&["subdomain", "rm"], vec![], 1, format!("Usage: {cli} publish subdomain rm <label>\n")),
        (&["secret", "ls"], vec![], 2, format!("usage: {cli} publish secret list <service>\n")),
        (&["client-ip", "-x"], vec![], 2, format!("unknown flag '-x' \u{2014} usage: {cli} publish client-ip [<label> on|off] [--json]\n")),
        (
            &["status"],
            vec![Reply::Answer(403, "{}".into())],
            3,
            format!("Error: {product} Connect commands need the server owner, an Owner/Admin login, or a {product} agent session's own passport; this credential was refused.\n"),
        ),
    ];
    for (args, answers, code, err) in rows {
        let c = on("brand", args, answers, code, 0);
        let (ran, _) = rig.binary(&c, false);
        assert_eq!(
            (ran.code, ran.stderr.as_str()),
            (Some(code), err.as_str()),
            "{args:?}"
        );
    }
}

/// The large list: above 256 KiB, thousands of rows.
#[test]
fn subdomain_list_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| format!(r#"{{"host":"s{i:05}.acme.k2.dev","target":"localhost:{}","tier":"pro","effective":"active"}}"#, 10000 + i))
        .collect();
    let body = format!(r#"{{"subdomains":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&on(
        "subdomain list, 3000 rows",
        &["subdomain", "list"],
        vec![relayed(200, &body)],
        0,
        1,
    ));
    rig.check(&on(
        "subdomain list --json, 3000 rows",
        &["subdomain", "list", "--json"],
        vec![relayed(200, &body)],
        0,
        1,
    ));
}
