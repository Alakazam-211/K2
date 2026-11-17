//! The transport against the shell CLI's: for each call in a table, the
//! request functions of the built `cli/k2` (sourced under bash, with the
//! real curl) and the Rust client make the same call to one logging stub
//! daemon ([`common::stub`]), and the raw requests must be the same bytes.
//!
//! A Rust test, not a `tests/cli` script: the client is a library that no
//! command calls yet, so only Rust can drive it, and the stub keeps each
//! request's raw bytes (a Python `http.server` stub hands back parsed
//! headers, not the bytes).
//!
//! The normaliser treats only these as expected differences, each line
//! counted (exactly one per request, or the row fails):
//!
//! - `User-Agent`: `curl/<version>` from bash, `k5/<version>` here;
//! - `X-K5-Cli-Version: <version>`: sent here only, dropped;
//! - `X-K5-Cli-Via`: `fallthrough` from bash (the front ran it,
//!   `_K5_CLI_VIA=rust`), `native` here.
//!
//! Everything else is compared byte for byte: the request line and its
//! query, every other header and its place (curl's order is fixed, so the
//! order is compared too), and the body. Each row also says how many
//! requests both must send (so a retry row proves its retry), and checks
//! the answer the caller gets: bash's stdout against the response body,
//! bash's permission note file against [`Response::permission`], and the
//! no-answer line (its seconds checked to be in range, then masked).
//!
//! Hermetic: HOME, TMPDIR and K2_HOME are a scratch folder, the passports
//! are made up, and bash gets nothing from the environment but PATH.

#![cfg(unix)]

mod common;

use common::stub::{Logged, Reply, Stub};
use k5_cli::transport::{Auth, Call, Channel, Client, Error, Passport, Response, SecretsDir};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const OWNER: &str = "owner-tok-1111";
const PASSPORT: &str = "sessA.secret2222";
const HEALED: &str = "sessA.secret3333";
/// Escaped by both (`project=`): a space, non-ASCII, and `& = +`.
const PROJECT: &str = "/tmp/k5 parity/ws é&=+";
/// Quotes, a backslash, a newline and non-ASCII.
const BODY: &str = "{\"name\":\"bob\",\"note\":\"a \\\"q\\\" \\\\ b\\nline2 é\"}";
const REFUSED: &str = r#"{"error":"Invalid or missing auth token"}"#;

/// The shell's transport, from the built `cli/k2`: the env reader, the
/// healed passport, the curl config and `_k2_curl`, the request
/// functions, `urlencode`, `cli_post_form` and the cell-socket arms.
fn bash_lib() -> String {
    let k2 = common::cli_script();
    let next = "# Read one secret from stdin into _K2_SECRET: a hidden prompt when stdin";
    let requests = common::block(&k2, "_CLI_LOCAL_TRANSPORT_DENIED=75", next);
    let requests = requests.strip_suffix(next).expect("the block ends with its last line").to_string();
    [
        common::block(&k2, "_envn_into() {", "}"),
        common::block(&k2, "# BEGIN_CLI_HEALED_PASSPORT", "# END_CLI_HEALED_PASSPORT"),
        common::block(&k2, "# BEGIN_CLI_CURL_CFG", "# END_CLI_CURL_CFG"),
        requests,
        common::block(&k2, "urlencode() {", "}"),
        common::block(&k2, "cli_post_form() {", "}"),
        common::block(&k2, "# ── #58 Phase-1 close: scoped per-cell UDS arms ──────────────────────────", "# END_CLI_PASSPORT_PROJECT"),
    ]
    .join("\n")
}

/// A scratch folder, and the stub with its socket under `/tmp` (a socket
/// path is capped near 104 bytes on macOS, and the temp dir there is
/// long).
struct Work {
    dir: PathBuf,
    lib: PathBuf,
    stub: Stub,
}

impl Work {
    fn new(tag: &str) -> Work {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k5-tp-{tag}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        for sub in ["home", "tmp", "k2home", "secrets"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let lib = dir.join("lib.sh");
        std::fs::write(&lib, bash_lib()).unwrap();
        let stub = Stub::start(&PathBuf::from(format!("/tmp/k5tp-{}-{n}.sock", std::process::id())));
        Work { dir, lib, stub }
    }

    fn secrets(&self) -> PathBuf {
        self.dir.join("secrets")
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    /// No passport: the owner token over TCP.
    Owner,
    /// A passport, no cell socket.
    PassportTcp,
    /// A passport and the stub's socket.
    PassportUds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Exit 0; stdout is the body.
    Body,
    /// Exit 0; stdout is `<body>\n<status>` (`cli_send_json`).
    BodyAndStatus,
    /// The no-answer line and exit 1, its seconds in the range.
    NoAnswer { empty: bool, secs: (u64, u64) },
    /// Nothing accepted the connection: curl's exit 7 and no output.
    Unreachable,
}

struct Row {
    name: &'static str,
    who: Who,
    would_refuse: bool,
    replies: Vec<Reply>,
    /// The call, in bash.
    bash: &'static str,
    /// The same call, here.
    rust: fn(&mut Client) -> Result<Response, Error>,
    /// The requests each side sends.
    requests: usize,
    outcome: Outcome,
}

struct BashRun {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    rc: Option<String>,
    note: Option<String>,
}

fn run_bash(w: &Work, row: &Row, port: u16) -> BashRun {
    let (token, scoped) = match row.who {
        Who::Owner => (OWNER, ""),
        Who::PassportTcp | Who::PassportUds => (PASSPORT, PASSPORT),
    };
    let out = w.dir.join("out");
    let rc = w.dir.join("rc");
    let note = w.dir.join("note");
    for f in [&out, &rc, &note] {
        let _ = std::fs::remove_file(f);
    }
    let script = format!(
        r#"set -euo pipefail
_K2_WINDOWS=""
HOST=127.0.0.1
PORT={port}
TOKEN={token}
SCOPED_TOKEN={scoped}
PROJECT="$K5T_PROJECT"
BODY="$K5T_BODY"
. "$K5T_LIB"
BASE_URL="http://${{HOST}}:${{PORT}}"
_K5_CLI_VIA=rust
_K5_WOULD_REFUSE={wr}
rc=0
{call} >"$K5T_OUT" || rc=$?
printf '%s' "$rc" >"$K5T_RC"
if [ -f "$_K2_PERMISSION_NOTE" ]; then cat "$_K2_PERMISSION_NOTE" >"$K5T_NOTE"; fi
"#,
        port = port,
        wr = if row.would_refuse { "1" } else { "" },
        call = row.bash,
    );
    let mut cmd = Command::new("bash");
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", w.dir.join("home"))
        .env("TMPDIR", w.dir.join("tmp"))
        .env("K2_HOME", w.dir.join("k2home"))
        .env("K2_SECRETS_DIR", w.secrets())
        .env("K5T_PROJECT", PROJECT)
        .env("K5T_BODY", BODY)
        .env("K5T_LIB", &w.lib)
        .env("K5T_OUT", &out)
        .env("K5T_RC", &rc)
        .env("K5T_NOTE", &note);
    if row.who == Who::PassportUds {
        cmd.env("K2_HOOK_SOCK", &w.stub.sock);
    }
    cmd.arg("-c").arg(&script);
    let o = cmd.output().unwrap_or_else(|e| panic!("run bash: {e}"));
    BashRun {
        status: o.status.code(),
        stdout: std::fs::read(&out).unwrap_or_default(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        rc: std::fs::read_to_string(&rc).ok(),
        note: std::fs::read_to_string(&note).ok(),
    }
}

fn client(w: &Work, row: &Row, port: u16) -> Client {
    let passport = || Passport::new(PASSPORT).expect("a passport");
    let auth = match row.who {
        Who::Owner => Auth::Owner(OWNER.to_string()),
        Who::PassportTcp => Auth::Passport { passport: passport(), sock: None },
        Who::PassportUds => Auth::Passport { passport: passport(), sock: Some(w.stub.sock.clone()) },
    };
    let mut c = Client::new("127.0.0.1", port, PROJECT, auth);
    c.would_refuse = row.would_refuse;
    c.healer = Some(Box::new(SecretsDir(w.secrets())));
    c.port_file = Some(w.dir.join("k2home").join("daemon.port"));
    c
}

/// A logged request with the three expected differences masked, each
/// counted.
fn normalise(r: &Logged, rust: bool, row: &str) -> String {
    let version = k5_cli::embedded::version();
    let text = String::from_utf8(r.bytes.clone()).unwrap_or_else(|_| panic!("{row}: a request that is not UTF-8"));
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or_else(|| panic!("{row}: a request with no head: {text:?}"));
    let (mut ua, mut via, mut ver) = (0, 0, 0);
    let mut lines: Vec<String> = Vec::new();
    for line in head.split("\r\n") {
        let masked = if rust {
            if line == format!("User-Agent: k5/{version}") {
                ua += 1;
                Some("User-Agent: <ua>")
            } else if line == format!("X-K5-Cli-Version: {version}") {
                ver += 1;
                continue;
            } else if line == "X-K5-Cli-Via: native" {
                via += 1;
                Some("X-K5-Cli-Via: <via>")
            } else {
                None
            }
        } else if line.starts_with("User-Agent: curl/") {
            ua += 1;
            Some("User-Agent: <ua>")
        } else if line == "X-K5-Cli-Via: fallthrough" {
            via += 1;
            Some("X-K5-Cli-Via: <via>")
        } else {
            None
        };
        lines.push(masked.map(str::to_string).unwrap_or_else(|| line.to_string()));
    }
    let side = if rust { "rust" } else { "bash" };
    assert_eq!((ua, via, ver), (1, 1, usize::from(rust)), "{row} ({side}): the masked lines in {text:?}");
    format!("[{}]\n{}\n\n{body}", r.channel, lines.join("\n"))
}

/// `(the line with its seconds as <n>, the seconds)`.
fn no_answer_secs(line: &str) -> (String, u64) {
    let tail = " s; run k2 daemon status";
    let head = line.strip_suffix(tail).unwrap_or_else(|| panic!("not a no-answer line: {line:?}"));
    let (pre, n) = head.rsplit_once(' ').unwrap_or_else(|| panic!("not a no-answer line: {line:?}"));
    (format!("{pre} <n>{tail}"), n.parse().unwrap_or_else(|_| panic!("no seconds in {line:?}")))
}

fn check(w: &Work, row: &Row) {
    check_on(w, row, w.stub.port);
}

/// [`check`], both sides given `port` (a dead one, for the rows whose
/// first connect is refused).
fn check_on(w: &Work, row: &Row, port: u16) {
    let name = row.name;
    // The heal rows' stub writes the new passport; start without one.
    let _ = std::fs::remove_file(w.secrets().join(".k2-passport"));
    w.stub.reset(&row.replies);
    let bash = run_bash(w, row, port);
    let bash_log = w.stub.log();
    assert_eq!(w.stub.left(), 0, "{name}: bash left answers unused; stderr: {}", bash.stderr);

    let _ = std::fs::remove_file(w.secrets().join(".k2-passport"));
    w.stub.reset(&row.replies);
    let mut c = client(w, row, port);
    let got = (row.rust)(&mut c);
    let rust_log = w.stub.log();
    assert_eq!(w.stub.left(), 0, "{name}: rust left answers unused");

    let b: Vec<String> = bash_log.iter().map(|r| normalise(r, false, name)).collect();
    let r: Vec<String> = rust_log.iter().map(|r| normalise(r, true, name)).collect();
    assert_eq!(b.len(), row.requests, "{name}: bash sent {} requests:\n{}", b.len(), b.join("\n----\n"));
    assert_eq!(r, b, "{name}: the requests differ\n== rust ==\n{}\n== bash ==\n{}", r.join("\n----\n"), b.join("\n----\n"));
    // The shapes, for the log.
    println!("== {name}\n{}", b.join("\n----\n").replace('\r', ""));

    match row.outcome {
        Outcome::Body | Outcome::BodyAndStatus => {
            assert_eq!(bash.status, Some(0), "{name}: bash failed: {}", bash.stderr);
            assert_eq!(bash.rc.as_deref(), Some("0"), "{name}: bash's call failed: {}", bash.stderr);
            assert_eq!(bash.stderr, "", "{name}: bash printed on stderr");
            let resp = got.unwrap_or_else(|e| panic!("{name}: rust failed: {e}"));
            let mut want = resp.body.clone();
            if row.outcome == Outcome::BodyAndStatus {
                want.extend_from_slice(format!("\n{}", resp.status).as_bytes());
            }
            assert_eq!(String::from_utf8_lossy(&bash.stdout), String::from_utf8_lossy(&want), "{name}: the answer the caller gets");
            assert_eq!(bash.note, resp.permission.as_ref().map(|n| n.text()), "{name}: the permission note");
        }
        Outcome::NoAnswer { empty, secs } => {
            assert_eq!(bash.status, Some(1), "{name}: bash's exit; stderr: {}", bash.stderr);
            assert_eq!(bash.rc, None, "{name}: bash's command went on after no answer");
            let line = bash.stderr.strip_suffix('\n').unwrap_or_else(|| panic!("{name}: bash's stderr: {:?}", bash.stderr));
            let (bash_line, bash_secs) = no_answer_secs(line);
            let n = match got {
                Err(Error::NoAnswer(n)) => n,
                other => panic!("{name}: rust: {other:?}"),
            };
            assert_eq!(n.empty, empty, "{name}");
            let (rust_line, rust_secs) = no_answer_secs(&n.to_string());
            assert_eq!(rust_line, bash_line, "{name}: the no-answer line");
            for (side, s) in [("bash", bash_secs), ("rust", rust_secs)] {
                assert!((secs.0..=secs.1).contains(&s), "{name}: {side} waited {s} s, wanted {secs:?}");
            }
        }
        Outcome::Unreachable => {
            assert_eq!(bash.status, Some(0), "{name}: bash failed: {}", bash.stderr);
            assert_eq!(bash.rc.as_deref(), Some("7"), "{name}: bash's call; stderr: {}", bash.stderr);
            assert_eq!(bash.stdout, b"", "{name}: bash's output");
            assert_eq!(bash.stderr, "", "{name}: bash printed on stderr");
            assert!(matches!(got, Err(Error::Unreachable { .. })), "{name}: rust: {got:?}");
        }
    }
}

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

fn rows(w: &Work) -> Vec<Row> {
    let heal = |status: u16| Reply::WriteThen {
        file: w.secrets().join(".k2-passport"),
        content: format!("{HEALED}\n"),
        status,
        body: REFUSED.to_string(),
    };
    let permission_ask = r#"{"error":{"code":"owner_only"},"permission":{"command":"k2 permission request --scope dns.manage --why \"...\"","requestable":true}}"#;
    let permission_none = r#"{"error":"nope","permission":{"requestable":false,"hint":"Only a person can change this."}}"#;
    vec![
        Row {
            name: "GET with a query (cli_request)",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok(r#"{"heartbeats":[]}"#)],
            bash: r#"cli_request /cli/heartbeat/list "name=a%2Fb" "json=1""#,
            rust: |c| c.call(Channel::Tcp, "/cli/heartbeat/list", Call::Request(&["name=a%2Fb", "json=1"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "GET on an endpoint that has a query",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_request "/cli/x?y=%2F1" "a=%2Fb""#,
            rust: |c| c.call(Channel::Tcp, "/cli/x?y=%2F1", Call::Request(&["a=%2Fb"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "POST with a query (cli_request_post)",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_request_post /cli/review/approve "agent=a%20b" "branch=x""#,
            rust: |c| c.call(Channel::Tcp, "/cli/review/approve", Call::RequestPost(&["agent=a%20b", "branch=x"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "POST on an endpoint that has a query",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_request_post "/cli/x?y=%2F1" "a=%2Fb""#,
            rust: |c| c.call(Channel::Tcp, "/cli/x?y=%2F1", Call::RequestPost(&["a=%2Fb"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a cell-socket POST on an endpoint that has a query",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_send cli_request_post "/cli/inbox/claim?y=%2F1" "id=7""#,
            rust: |c| c.send("/cli/inbox/claim?y=%2F1", Call::RequestPost(&["id=7"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "POST a form (cli_post_form)",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_post_form /cli/workspace/msg "text=hi%20there" "to=ws""#,
            rust: |c| c.call(Channel::Tcp, "/cli/workspace/msg", Call::PostForm(&["text=hi%20there", "to=ws"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "POST JSON (cli_post_json)",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok(r#"{"ok":true}"#)],
            bash: r#"cli_post_json /cli/users/add "$BODY""#,
            rust: |c| c.call(Channel::Tcp, "/cli/users/add", Call::PostJson(BODY.as_bytes())),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "POST JSON from stdin (cli_post_json_stdin)",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok(r#"{"ok":true}"#)],
            bash: r#"printf '%s' "$BODY" | cli_post_json_stdin /cli/llm/accounts/add-key"#,
            rust: |c| c.post_json_stdin("/cli/llm/accounts/add-key", BODY.as_bytes()),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "cli_send_json GET over TCP",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok(r#"{"rows":[]}"#)],
            bash: r#"cli_send_json GET /cli/compute/nodes "q=%2F1&x=y""#,
            rust: |c| c.send_json(false, "/cli/compute/nodes", "q=%2F1&x=y"),
            requests: 1,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "cli_send_json POST over TCP",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Answer(201, r#"{"id":1}"#.to_string())],
            bash: r#"cli_send_json POST /cli/compute/run "$BODY""#,
            rust: |c| c.send_json(true, "/cli/compute/run", BODY),
            requests: 1,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "the would-refuse count",
            who: Who::Owner,
            would_refuse: true,
            replies: vec![ok("{}")],
            bash: r#"cli_request /cli/agents/list "json=1""#,
            rust: |c| c.call(Channel::Tcp, "/cli/agents/list", Call::Request(&["json=1"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport over TCP (cli_send, no socket)",
            who: Who::PassportTcp,
            would_refuse: false,
            replies: vec![ok(r#"{"items":[]}"#)],
            bash: r#"cli_send cli_request /cli/inbox/list "a=1""#,
            rust: |c| c.send("/cli/inbox/list", Call::Request(&["a=1"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport on the cell socket: GET",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok(r#"{"items":[]}"#)],
            bash: r#"cli_send cli_request /cli/inbox/list "a=%2Fb""#,
            rust: |c| c.send("/cli/inbox/list", Call::Request(&["a=%2Fb"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport on the cell socket: GET with no args, would refuse",
            who: Who::PassportUds,
            would_refuse: true,
            replies: vec![ok(r#"{"heartbeats":[]}"#)],
            bash: r#"cli_send cli_request /cli/heartbeat/list"#,
            rust: |c| c.send("/cli/heartbeat/list", Call::Request(&[])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport on the cell socket: POST with a query",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_send cli_request_post /cli/inbox/claim "id=7""#,
            rust: |c| c.send("/cli/inbox/claim", Call::RequestPost(&["id=7"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport on the cell socket: POST JSON",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_send cli_post_json /cli/awareness/publish "$BODY""#,
            rust: |c| c.send("/cli/awareness/publish", Call::PostJson(BODY.as_bytes())),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport on the cell socket: POST a form",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_send cli_post_form /cli/workspace/msg "text=a%20b""#,
            rust: |c| c.send("/cli/workspace/msg", Call::PostForm(&["text=a%20b"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a passport, a route the cell socket doesn't serve: TCP",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok("{}")],
            bash: r#"cli_send cli_request /cli/agents/list"#,
            rust: |c| c.send("/cli/agents/list", Call::Request(&[])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "the cell socket says not served: then TCP with the passport",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.to_string()), ok(r#"{"items":[]}"#)],
            bash: r#"cli_send cli_request /cli/inbox/list "a=1""#,
            rust: |c| c.send("/cli/inbox/list", Call::Request(&["a=1"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "cli_send_json GET on the cell socket",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok(r#"{"rows":[]}"#)],
            bash: r#"cli_send_json GET /cli/compute/nodes "q=%2F1""#,
            rust: |c| c.send_json(false, "/cli/compute/nodes", "q=%2F1"),
            requests: 1,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "cli_send_json POST on the cell socket, not served, then TCP",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![Reply::Answer(404, r#"{"error":"not served on cell socket"}"#.to_string()), Reply::Answer(403, r#"{"error":"owner_only"}"#.to_string())],
            bash: r#"cli_send_json POST /cli/compute/run "$BODY""#,
            rust: |c| c.send_json(true, "/cli/compute/run", BODY),
            requests: 2,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "a cell-socket GET query that starts with @ is sent as written",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok(r#"{"items":[]}"#)],
            bash: r#"cli_send cli_request /cli/inbox/list "@/etc/hosts""#,
            rust: |c| c.send("/cli/inbox/list", Call::Request(&["@/etc/hosts"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a cli_send_json GET payload that starts with @ is sent as written, cell socket",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![ok(r#"{"rows":[]}"#)],
            bash: r#"cli_send_json GET /cli/compute/nodes "@/etc/hosts""#,
            rust: |c| c.send_json(false, "/cli/compute/nodes", "@/etc/hosts"),
            requests: 1,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "a cli_send_json GET payload that starts with @ is sent as written, TCP",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok(r#"{"rows":[]}"#)],
            bash: r#"cli_send_json GET /cli/compute/nodes "@/etc/hosts""#,
            rust: |c| c.send_json(false, "/cli/compute/nodes", "@/etc/hosts"),
            requests: 1,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "503 migrating, then 200",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Answer(503, r#"{"state":"migrating"}"#.to_string()), ok(r#"{"items":[]}"#)],
            bash: r#"cli_request /cli/inbox/list "a=1""#,
            rust: |c| c.call(Channel::Tcp, "/cli/inbox/list", Call::Request(&["a=1"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "503 migrating on the cell socket POST, then 200",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![Reply::Answer(503, r#"{"state":"migrating"}"#.to_string()), ok("{}")],
            bash: r#"cli_send cli_request_post /cli/inbox/claim "id=7""#,
            rust: |c| c.send("/cli/inbox/claim", Call::RequestPost(&["id=7"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "405 method not allowed, then the same call as a GET",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Answer(405, r#"{"error":"method not allowed for this route"}"#.to_string()), ok(r#"{"ok":true}"#)],
            bash: r#"cli_request_post /cli/review/approve "agent=x""#,
            rust: |c| c.call(Channel::Tcp, "/cli/review/approve", Call::RequestPost(&["agent=x"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refusal with a permission command (cli_request)",
            who: Who::PassportTcp,
            would_refuse: false,
            replies: vec![Reply::Answer(403, permission_ask.to_string())],
            bash: r#"cli_request /cli/dns/list"#,
            rust: |c| c.call(Channel::Tcp, "/cli/dns/list", Call::Request(&[])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refusal that can't be requested (cli_post_json on the cell socket)",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![Reply::Answer(403, permission_none.to_string())],
            bash: r#"cli_send cli_post_json /cli/awareness/publish "$BODY""#,
            rust: |c| c.send("/cli/awareness/publish", Call::PostJson(BODY.as_bytes())),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refusal on cli_send_json keeps its note",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Answer(403, permission_ask.to_string())],
            bash: r#"cli_send_json POST /cli/compute/run "{}""#,
            rust: |c| c.send_json(true, "/cli/compute/run", "{}"),
            requests: 1,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "a refusal on cli_post_form keeps its note",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Answer(403, permission_ask.to_string())],
            bash: r#"cli_post_form /cli/workspace/msg "text=x""#,
            rust: |c| c.call(Channel::Tcp, "/cli/workspace/msg", Call::PostForm(&["text=x"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refusal on cli_post_form_uds keeps its note",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![Reply::Answer(403, permission_none.to_string())],
            bash: r#"cli_send cli_post_form /cli/inbox/compose "title=x""#,
            rust: |c| c.send("/cli/inbox/compose", Call::PostForm(&["title=x"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refused passport, healed, over TCP",
            who: Who::PassportTcp,
            would_refuse: false,
            replies: vec![heal(401), ok(r#"{"name":"a"}"#)],
            bash: r#"cli_request /cli/whoami"#,
            rust: |c| c.call(Channel::Tcp, "/cli/whoami", Call::Request(&[])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refused passport, healed, on the cell socket",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![heal(403), ok(r#"{"items":[]}"#)],
            bash: r#"cli_send cli_request_post /cli/inbox/claim "id=7""#,
            rust: |c| c.send("/cli/inbox/claim", Call::RequestPost(&["id=7"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refused passport, healed, cli_post_json on the cell socket",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![heal(403), ok("{}")],
            bash: r#"cli_send cli_post_json /cli/awareness/publish "{}""#,
            rust: |c| c.send("/cli/awareness/publish", Call::PostJson(b"{}")),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refused passport, healed, cli_post_form over TCP",
            who: Who::PassportTcp,
            would_refuse: false,
            replies: vec![heal(401), ok("{}")],
            bash: r#"cli_post_form /cli/workspace/msg "text=x""#,
            rust: |c| c.call(Channel::Tcp, "/cli/workspace/msg", Call::PostForm(&["text=x"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "a refused passport, healed, cli_post_form on the cell socket",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![heal(403), ok("{}")],
            bash: r#"cli_send cli_post_form /cli/workspace/msg "text=x""#,
            rust: |c| c.send("/cli/workspace/msg", Call::PostForm(&["text=x"])),
            requests: 2,
            outcome: Outcome::Body,
        },
        Row {
            name: "a cli_send_json refusal, healed, on the cell socket",
            who: Who::PassportUds,
            would_refuse: false,
            replies: vec![heal(401), ok(r#"{"rows":[]}"#)],
            bash: r#"cli_send_json GET /cli/compute/nodes"#,
            rust: |c| c.send_json(false, "/cli/compute/nodes", ""),
            requests: 2,
            outcome: Outcome::BodyAndStatus,
        },
        Row {
            name: "a timeout (cli_post_json, _K2_POST_MAX_TIME=2)",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Hang(Duration::from_secs(4))],
            bash: r#"_K2_POST_MAX_TIME=2 cli_post_json /cli/slow "{}""#,
            rust: |c| {
                c.post_max_time = Some(Duration::from_secs(2));
                c.call(Channel::Tcp, "/cli/slow", Call::PostJson(b"{}"))
            },
            requests: 1,
            outcome: Outcome::NoAnswer { empty: false, secs: (2, 3) },
        },
        Row {
            name: "a connection closed with nothing",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Close],
            bash: r#"cli_request /cli/inbox/list"#,
            rust: |c| c.call(Channel::Tcp, "/cli/inbox/list", Call::Request(&[])),
            requests: 1,
            outcome: Outcome::NoAnswer { empty: true, secs: (0, 1) },
        },
        Row {
            name: "a 503 with an empty body",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![Reply::Answer(503, String::new())],
            bash: r#"cli_request_post /cli/inbox/claim "id=1""#,
            rust: |c| c.call(Channel::Tcp, "/cli/inbox/claim", Call::RequestPost(&["id=1"])),
            requests: 1,
            outcome: Outcome::NoAnswer { empty: true, secs: (0, 1) },
        },
    ]
}

#[test]
fn every_call_sends_what_bash_sends() {
    let w = Work::new("rows");
    let rows = rows(&w);
    for row in &rows {
        check(&w, row);
        println!("ok: {}", row.name);
    }
    println!("{} rows match", rows.len());
}

/// A refused connect is tried again, on the port in `daemon.port`, unless
/// the call read stdin: the shell's rule is an argument that is exactly
/// `@-`, so an `@-` inside the URL keeps the retries.
#[test]
fn a_refused_connect_is_tried_again_unless_stdin_was_read() {
    let w = Work::new("refused");
    std::fs::write(w.dir.join("k2home").join("daemon.port"), format!("{}\n", w.stub.port)).unwrap();
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let rows = [
        Row {
            name: "a URL with @- in it, refused: tried again on the moved port",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![ok(r#"{"ok":true}"#)],
            bash: r#"cli_request "/cli/files/a@-b" "q=1""#,
            rust: |c| c.call(Channel::Tcp, "/cli/files/a@-b", Call::Request(&["q=1"])),
            requests: 1,
            outcome: Outcome::Body,
        },
        Row {
            name: "a body from stdin, refused: never tried again",
            who: Who::Owner,
            would_refuse: false,
            replies: vec![],
            bash: r#"printf '%s' "$BODY" | cli_post_json_stdin /cli/llm/accounts/add-key"#,
            rust: |c| c.post_json_stdin("/cli/llm/accounts/add-key", BODY.as_bytes()),
            requests: 0,
            outcome: Outcome::Unreachable,
        },
    ];
    for row in &rows {
        check_on(&w, row, dead);
        println!("ok: {}", row.name);
    }
    println!("{} rows match", rows.len());
}

/// The cell socket's routes: `_uds_eligible` and [`uds_eligible`] agree.
///
/// [`uds_eligible`]: k5_cli::transport::client::uds_eligible
#[test]
fn the_cell_socket_serves_the_shell_s_routes() {
    let paths = [
        "/cli/workspace/msg",
        "/cli/workspace/msgs",
        "/cli/awareness/publish",
        "/cli/folder-trust/status",
        "/cli/folder-trust/request",
        "/cli/folder-trust",
        "/cli/terminal/read",
        "/cli/inbox/list",
        "/cli/inbox/",
        "/cli/inbox",
        "/cli/review-checklist/x/y",
        "/cli/dns/records",
        "/cli/dns",
        "/cli/domains",
        "/cli/domains/names",
        "/cli/domains/names/remove",
        "/cli/domains/attach",
        "/cli/certs",
        "/cli/certs/renew",
        "/cli/publish/run",
        "/cli/publish",
        "/cli/connections",
        "/cli/connections/add",
        "/cli/federation/peers",
        "/cli/federation/peer-roster",
        "/cli/federation/peer-rosters",
        "/cli/federation/send",
        "/cli/federation/pair",
        "/cli/heartbeat/install-launchd",
        "/cli/heartbeat/uninstall-launchd",
        "/cli/heartbeat/apply-wake-scheduler",
        "/cli/heartbeat/list-all",
        "/cli/heartbeat/fires-list-all",
        "/cli/heartbeat/set-show-sessions",
        "/cli/heartbeat/list",
        "/cli/heartbeat/list-all/x",
        "/cli/heartbeat-log",
        "/cli/heartbeat",
        "/cli/thread",
        "/cli/thread/post",
        "/cli/threads",
        "/cli/browser/notes",
        "/cli/browser/notes/redeliver",
        "/cli/browser/open",
        "/cli/agents/list",
        "/cli/whoami",
        "",
    ];
    let k2 = common::cli_script();
    let fun = common::block(&k2, "_uds_eligible() {", "}");
    let script = format!("{fun}\nfor p in \"$@\"; do if _uds_eligible \"$p\"; then printf 1; else printf 0; fi; done");
    let shell = common::bash_args(&script, &paths);
    assert_eq!(shell.len(), paths.len());
    for (p, s) in paths.iter().zip(shell.chars()) {
        assert_eq!(k5_cli::transport::client::uds_eligible(p), s == '1', "{p:?}");
    }
}
