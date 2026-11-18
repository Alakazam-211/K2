//! The binary's own verbs, run as a person runs them, against a logging
//! stub daemon ([`common::stub`]): `k5 --version --check`, `k5 cli status`
//! and the one line for a server of another version.
//!
//! Every run gets an empty environment but `PATH` and what the case sets,
//! a temporary `HOME` (its K2 home holds the owner token when a case puts
//! one there) and the stub's port. No real daemon, no real K2 home.

#![cfg(unix)]

mod common;

use common::stub::{Logged, Reply, Stub};
use k5_cli::out::json::{self, Value};
use k5_cli::route::TABLE;
use k5_cli::transport::skew;
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const OWNER: &str = "owner-tok-5151";
const PASSPORT: &str = "sessA.secret5252";
const OTHER: &str = "0.46.99";

fn ours() -> &'static str {
    k5_cli::embedded::version()
}

static N: AtomicUsize = AtomicUsize::new(0);

struct Rig {
    dir: PathBuf,
    home: PathBuf,
    proj: PathBuf,
    stub: Stub,
}

#[derive(Debug)]
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k5-native-{tag}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let home = dir.join("home");
        let proj = dir.join("proj");
        // The project is the run's folder: its workspace folder stops the
        // search there.
        for d in [
            dir.join("bin"),
            k2_client::paths::k2_home_in(&home),
            k2_client::workspace_dir::workspace_dot_dir(&proj),
        ] {
            std::fs::create_dir_all(&d).unwrap();
        }
        for name in ["k2", "k5"] {
            std::os::unix::fs::symlink(BIN, dir.join("bin").join(name)).unwrap();
        }
        let stub = Stub::start(&PathBuf::from(format!(
            "/tmp/k5n-{tag}-{}-{n}.sock",
            std::process::id()
        )));
        Rig {
            dir,
            home,
            proj,
            stub,
        }
    }

    /// The owner token in the K2 home, as the daemon writes it.
    fn owner_on_disk(&self) {
        std::fs::write(
            k2_client::paths::k2_home_in(&self.home).join("heartbeat.token"),
            format!("{OWNER}\n"),
        )
        .unwrap();
    }

    fn port(&self) -> String {
        self.stub.port.to_string()
    }

    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(self.dir.join("bin").join("k5"));
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", &self.home)
            .current_dir(&self.proj)
            .envs(extra.iter().copied())
            .args(args.iter().map(OsString::from));
        let out = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        Out {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A port nothing listens on.
fn dead_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port().to_string()
}

fn headed(status: u16, version: Option<&str>, body: &str) -> Reply {
    let extra = version
        .map(|v| vec![("X-K5-Server-Version".to_string(), v.to_string())])
        .unwrap_or_default();
    Reply::Headed(status, extra, body.to_string())
}

/// The request line and the header lines of a logged request.
fn head(r: &Logged) -> Vec<String> {
    let text = String::from_utf8(r.bytes.clone()).expect("a UTF-8 request");
    let (head, _) = text.split_once("\r\n\r\n").expect("a request head");
    head.split("\r\n").map(str::to_string).collect()
}

fn rows(pairs: &[(&str, &str, i64)]) -> String {
    let rows: Vec<String> = pairs
        .iter()
        .map(|(kind, key, n)| {
            format!(r#"{{"kind":"{kind}","key":"{key}","count":{n},"first_at":1,"last_at":2}}"#)
        })
        .collect();
    format!(r#"{{"rows":[{}]}}"#, rows.join(","))
}

fn skew_lines(stderr: &str) -> usize {
    let want = skew::line(ours(), OTHER);
    stderr.lines().filter(|l| *l == want).count()
}

// ── --version --check ──────────────────────────────────────────────────

#[test]
fn version_check_same_version() {
    let rig = Rig::new("chk-same");
    rig.stub.reset(&[headed(
        200,
        None,
        &format!(r#"{{"phase":"ready","version":"{}"}}"#, ours()),
    )]);
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &rig.port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(
        out.stdout,
        format!("k2 {v}\nserver {v}: same version\n", v = ours())
    );
    assert_eq!(out.stderr, "");
    // One request: a bare GET, no credential, no query.
    let log = rig.stub.log();
    assert_eq!(log.len(), 1, "{log:?}");
    let h = head(&log[0]);
    assert_eq!(h[0], "GET /boot-status HTTP/1.1");
    assert_eq!(log[0].channel, "tcp");
    let names: Vec<&str> = h[1..]
        .iter()
        .map(|l| l.split_once(':').expect("a header").0)
        .collect();
    assert_eq!(
        names,
        [
            "Host",
            "User-Agent",
            "Accept",
            "X-K5-Cli-Version",
            "X-K5-Cli-Via"
        ]
    );
}

#[test]
fn version_check_skewed_is_one_stdout_line_and_exit_0() {
    let rig = Rig::new("chk-skew");
    rig.owner_on_disk();
    // The header names the other version too: still one line.
    rig.stub.reset(&[headed(
        200,
        Some(OTHER),
        &format!(r#"{{"version":"{OTHER}"}}"#),
    )]);
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &rig.port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(
        out.stdout,
        format!("k2 {}\n{}\n", ours(), skew::line(ours(), OTHER))
    );
    assert_eq!(
        out.stderr, "",
        "the transport's own line is not printed as well"
    );
    // The owner token on disk is not sent to a public read.
    let h = head(&rig.stub.log()[0]);
    assert_eq!(h[0], "GET /boot-status HTTP/1.1");
    assert!(
        h.iter()
            .all(|l| !l.contains(OWNER) && !l.starts_with("X-K5-Owner-Source")),
        "{h:?}"
    );
}

#[test]
fn version_check_with_the_daemon_down_is_one_line_and_exit_7() {
    let rig = Rig::new("chk-down");
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &dead_port())]);
    assert_eq!(out.code, Some(7), "{out:?}");
    assert_eq!(out.stdout, format!("k2 {}\n", ours()));
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
    assert!(
        out.stderr
            .starts_with("can't reach K2's server for /boot-status: "),
        "{out:?}"
    );
    // No port anywhere: the same exit, a line that says where it looked.
    let out = rig.run(&["--version", "--check"], &[]);
    assert_eq!(out.code, Some(7), "{out:?}");
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
    assert!(
        out.stderr
            .starts_with("can't reach K2's server: no port in K5_PORT or "),
        "{out:?}"
    );
}

#[test]
fn only_the_exact_pair_checks() {
    let rig = Rig::new("chk-pair");
    // The shell's connection gate wants a credential before its router.
    for args in [
        &["-v", "--check"][..],
        &["version", "--check"],
        &["--version", "--check", "x"],
    ] {
        let out = rig.run(
            args,
            &[("K2_PORT", &rig.port()), ("K2_HOOK_TOKEN", "test-token")],
        );
        assert_eq!(
            (out.code, out.stdout.as_str()),
            (Some(0), format!("k2 {}\n", ours()).as_str()),
            "{args:?}: {out:?}"
        );
    }
    assert!(
        rig.stub
            .log()
            .iter()
            .all(|r| !head(r)[0].contains("/boot-status")),
        "the shell's version arm never asks"
    );
}

// ── cli status ─────────────────────────────────────────────────────────

/// The table's first column, family per line, after the summary and
/// header lines.
/// The table's family rows: (family, who answers, count). A verb row
/// (`<noun> <verbs> <who>`, under its family) is not one of them
/// ([`table_verbs`]).
fn table_families(stdout: &str) -> Vec<(String, String, Option<String>)> {
    stdout
        .lines()
        .skip(2)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let fam = parts.next().expect("a family").to_string();
            let by = parts.next().expect("who answers").to_string();
            ["bash", "native", "spec", "-"]
                .contains(&by.as_str())
                .then(|| (fam, by, parts.next().map(str::to_string)))
        })
        .collect()
}

/// The table's verb rows: (noun, the verbs joined by `|`, who answers).
fn table_verbs(stdout: &str) -> Vec<(String, String, String)> {
    stdout
        .lines()
        .skip(2)
        .filter_map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            (parts.len() >= 3 && !["bash", "native", "spec", "-"].contains(&parts[1]))
                .then(|| (parts[0].into(), parts[1].into(), parts[2].into()))
        })
        .collect()
}

/// The route table's verb rows, as the table prints them.
fn route_verbs() -> Vec<(String, String, String)> {
    TABLE
        .iter()
        .flat_map(|n| {
            n.verbs.iter().map(|v| {
                (
                    n.words[0].to_string(),
                    v.words.join("|"),
                    k5_cli::cli_status::owner_name(v.owner).to_string(),
                )
            })
        })
        .collect()
}

fn field(v: &Value, key: &str) -> Value {
    let Value::Object(pairs) = v else {
        panic!("not an object: {v:?}")
    };
    pairs
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .unwrap_or_else(|| panic!("no {key}"))
        .1
        .clone()
}

fn string(v: Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.as_str().expect("UTF-8").to_string()),
        Value::Null => None,
        Value::Int(n) => Some(n),
        other => panic!("not a string, a number or null: {other:?}"),
    }
}

#[test]
fn cli_status_as_the_owner_shows_the_counts() {
    let rig = Rig::new("st-owner");
    rig.owner_on_disk();
    let body = rows(&[
        ("fallthrough", "doc", 3),
        ("fallthrough", "docs", 2),
        ("fallthrough", "msg", 4),
        ("fallthrough", "other", 1),
        ("would_refuse", "ws", 9),
    ]);
    rig.stub.reset(&[headed(200, Some(ours()), &body)]);
    let out = rig.run(&["cli", "status"], &[("K2_PORT", &rig.port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stderr, "", "{out:?}");
    let lines: Vec<&str> = out.stdout.lines().collect();
    let native = TABLE
        .iter()
        .filter(|n| n.owner == k5_cli::route::Owner::Native)
        .count();
    assert_eq!(
        lines[0],
        format!(
            "k5 {}: {native} native, 0 spec, {} bash",
            ours(),
            TABLE.len() - native
        )
    );
    assert_eq!(
        lines[1].split_whitespace().collect::<Vec<_>>(),
        ["family", "by", "fallthroughs"]
    );
    let fams = table_families(&out.stdout);
    assert_eq!(fams.len(), TABLE.len() + 1, "every row, and `other`");
    assert_eq!(table_verbs(&out.stdout), route_verbs(), "every verb row");
    let get = |name: &str| {
        fams.iter()
            .find(|f| f.0 == name)
            .unwrap_or_else(|| panic!("no {name} in\n{}", out.stdout))
            .clone()
    };
    assert_eq!(
        get("doc|docs"),
        ("doc|docs".into(), "bash".into(), Some("5".into()))
    );
    assert_eq!(get("msg"), ("msg".into(), "bash".into(), Some("4".into())));
    assert_eq!(
        get("cli"),
        ("cli".into(), "native".into(), Some("0".into()))
    );
    assert_eq!(get("version|--version|-v").1, "native");
    assert_eq!(
        fams.last().unwrap(),
        &("other".into(), "-".into(), Some("1".into()))
    );
    // One request: cli_request's shape with the owner token from disk.
    let log = rig.stub.log();
    assert_eq!(log.len(), 1, "{log:?}");
    let h = head(&log[0]);
    // The folder as the process sees it (macOS's temp folder is a link).
    let proj = k5_cli::transport::encode::quote(
        &std::fs::canonicalize(&rig.proj).unwrap().to_string_lossy(),
    );
    assert_eq!(
        h[0],
        format!(
            "GET /cli/cli-usage?token={OWNER}&project={} HTTP/1.1",
            k5_cli::transport::wire::curl_get_query(&proj)
        )
    );
    assert_eq!(
        &h[4..],
        [
            "X-K5-Cli-Version: ".to_string() + ours(),
            "X-K5-Cli-Via: native".into(),
            "X-K5-Owner-Source: disk".into()
        ]
    );
}

#[test]
fn cli_status_json_parses_and_matches_the_table() {
    let rig = Rig::new("st-json");
    rig.owner_on_disk();
    let body = rows(&[("fallthrough", "docs", 2), ("fallthrough", "nosuchverb", 6)]);
    rig.stub
        .reset(&[headed(200, None, &body), headed(200, None, &body)]);
    let table = rig.run(&["cli", "status"], &[("K2_PORT", &rig.port())]);
    let out = rig.run(&["cli", "status", "--json"], &[("K2_PORT", &rig.port())]);
    assert_eq!((out.code, out.stderr.as_str()), (Some(0), ""), "{out:?}");
    let doc =
        json::parse(&out.stdout).unwrap_or_else(|e| panic!("not JSON: {e:?}\n{}", out.stdout));
    assert_eq!(string(field(&doc, "version")).as_deref(), Some(ours()));
    assert_eq!(string(field(&doc, "counts")).as_deref(), Some("shown"));
    let Value::Array(fams) = field(&doc, "families") else {
        panic!("families is not a list")
    };
    let from_json: Vec<(String, String, Option<String>)> = fams
        .iter()
        .map(|f| {
            let owner = string(field(f, "owner")).unwrap_or_else(|| "-".into());
            (
                string(field(f, "family")).expect("a family name"),
                owner,
                string(field(f, "fallthroughs")),
            )
        })
        .collect();
    assert_eq!(from_json, table_families(&table.stdout));
    let verbs_json: Vec<(String, String, String)> = fams
        .iter()
        .flat_map(|f| {
            let fam = string(field(f, "family")).expect("a family name");
            let noun = fam.split('|').next().unwrap().to_string();
            let Value::Array(verbs) = field(f, "verbs") else {
                panic!("verbs is not a list")
            };
            verbs
                .into_iter()
                .map(|v| {
                    let Value::Array(words) = field(&v, "words") else {
                        panic!("words is not a list")
                    };
                    let words: Vec<String> =
                        words.into_iter().map(|w| string(w).unwrap()).collect();
                    (
                        noun.clone(),
                        words.join("|"),
                        string(field(&v, "owner")).unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(verbs_json, table_verbs(&table.stdout));
    assert_eq!(verbs_json, route_verbs());
    let first = table.stdout.lines().next().unwrap().to_string();
    let n = |k: &str| string(field(&doc, k)).unwrap();
    assert_eq!(
        first,
        format!(
            "k5 {}: {} native, {} spec, {} bash",
            ours(),
            n("native"),
            n("spec"),
            n("bash")
        )
    );
}

#[test]
fn cli_status_with_a_passport_is_the_table_and_one_line() {
    let rig = Rig::new("st-pass");
    rig.owner_on_disk();
    let refusal = headed(
        403,
        Some(ours()),
        r#"{"error":"only the owner can read the CLI usage counts"}"#,
    );
    rig.stub.reset(&[refusal.clone(), refusal]);
    let out = rig.run(
        &["cli", "status"],
        &[("K2_PORT", &rig.port()), ("K2_HOOK_TOKEN", PASSPORT)],
    );
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(
        out.stderr,
        format!("{}\n", k5_cli::cli_status::OWNER_ONLY_LINE)
    );
    assert_eq!(
        out.stdout
            .lines()
            .nth(1)
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["family", "by"]
    );
    assert!(
        table_families(&out.stdout).iter().all(|f| f.2.is_none()),
        "no counts column"
    );
    // The passport, never the owner token on disk (PRD 13.9 #4).
    let log = rig.stub.log();
    assert_eq!(log.len(), 1, "{log:?}");
    let h = head(&log[0]);
    assert!(
        h[0].starts_with(&format!("GET /cli/cli-usage?token={PASSPORT}&project=")),
        "{h:?}"
    );
    assert_eq!(rig.stub.left(), 1);
    assert!(
        h.iter()
            .all(|l| !l.contains(OWNER) && !l.starts_with("X-K5-Owner-Source")),
        "{h:?}"
    );
    let json = rig.run(
        &["cli", "status", "--json"],
        &[("K2_PORT", &rig.port()), ("K2_HOOK_TOKEN", PASSPORT)],
    );
    let doc = json::parse(&json.stdout).unwrap();
    assert_eq!(string(field(&doc, "counts")).as_deref(), Some("owner-only"));
}

#[test]
fn cli_status_with_no_credential_asks_nothing() {
    let rig = Rig::new("st-none");
    let out = rig.run(&["cli", "status"], &[("K2_PORT", &rig.port())]);
    assert_eq!(
        (out.code, out.stderr.as_str()),
        (Some(0), "the fallthrough counts are the owner's only\n"),
        "{out:?}"
    );
    assert!(rig.stub.log().is_empty(), "{:?}", rig.stub.log());
}

#[test]
fn cli_status_with_the_daemon_down_is_the_table_and_one_line() {
    let rig = Rig::new("st-down");
    rig.owner_on_disk();
    let out = rig.run(&["cli", "status"], &[("K2_PORT", &dead_port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
    assert!(
        out.stderr
            .starts_with("can't reach K2's server for /cli/cli-usage: "),
        "{out:?}"
    );
    assert_eq!(table_families(&out.stdout).len(), TABLE.len());
    let json = rig.run(&["cli", "status", "--json"], &[("K2_PORT", &dead_port())]);
    assert_eq!(
        string(field(&json::parse(&json.stdout).unwrap(), "counts")).as_deref(),
        Some("unavailable")
    );
}

#[test]
fn inside_a_session_with_no_passport_the_call_is_counted() {
    let rig = Rig::new("st-wr");
    rig.owner_on_disk();
    rig.stub.reset(&[headed(200, None, &rows(&[]))]);
    let out = rig.run(
        &["cli", "status"],
        &[("K2_PORT", &rig.port()), ("CLAUDECODE", "1")],
    );
    assert_eq!(out.code, Some(0), "{out:?}");
    let h = head(&rig.stub.log()[0]);
    assert_eq!(h[4], "X-K5-Would-Refuse: 1", "{h:?}");
}

#[test]
fn cli_help_is_answered_here_and_other_forms_reach_the_shell_s_pointer() {
    let rig = Rig::new("st-help");
    for args in [
        &["cli"][..],
        &["cli", "--help"],
        &["cli", "-h"],
        &["cli", "help"],
        &["cli", "status", "--help"],
    ] {
        let out = rig.run(args, &[("K2_PORT", &rig.port())]);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(0), k5_cli::cli_status::HELP, ""),
            "{args:?}"
        );
    }
    let pointer = "k5 cli status is answered by the k5 binary: run k5 cli status\n";
    for args in [&["cli", "status", "--bogus"][..], &["cli", "usage"]] {
        let out = rig.run(
            args,
            &[("K2_PORT", &rig.port()), ("K2_HOOK_TOKEN", "test-token")],
        );
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(2), "", pointer),
            "{args:?}"
        );
    }
    // The shell CLI on its own says the same.
    let mut cmd = Command::new("bash");
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &rig.home)
        .env("K2_PORT", "1")
        .env("K2_HOOK_TOKEN", "test-token");
    let o = cmd
        .arg(common::cli_src().join("../k2"))
        .args(["cli", "status"])
        .output()
        .unwrap();
    assert_eq!(
        (
            o.status.code(),
            &*String::from_utf8_lossy(&o.stderr),
            o.stdout.is_empty()
        ),
        (Some(2), pointer, true)
    );
    assert!(rig.stub.log().is_empty(), "{:?}", rig.stub.log());
}

// ── the skew line ──────────────────────────────────────────────────────

/// A migrating daemon, then its answer: two replies in one run, both of
/// another version, one line.
#[test]
fn the_skew_line_is_printed_once_per_process() {
    let rig = Rig::new("skew-once");
    rig.owner_on_disk();
    rig.stub.reset(&[
        headed(503, Some(OTHER), r#"{"state":"migrating"}"#),
        headed(200, Some(OTHER), &rows(&[("fallthrough", "msg", 1)])),
    ]);
    let out = rig.run(&["cli", "status"], &[("K2_PORT", &rig.port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(rig.stub.log().len(), 2, "the retry happened");
    assert_eq!(skew_lines(&out.stderr), 1, "{out:?}");
    assert_eq!(out.stderr, format!("{}\n", skew::line(ours(), OTHER)));
    assert!(
        table_families(&out.stdout)
            .iter()
            .any(|f| f.0 == "msg" && f.2.as_deref() == Some("1")),
        "it carried on"
    );
}

#[test]
fn no_skew_line_when_equal_absent_or_not_a_version() {
    let rig = Rig::new("skew-none");
    rig.owner_on_disk();
    for version in [Some(ours()), None, Some("0.46.99\u{1b}[2J"), Some("")] {
        rig.stub.reset(&[headed(200, version, &rows(&[]))]);
        let out = rig.run(&["cli", "status"], &[("K2_PORT", &rig.port())]);
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (Some(0), ""),
            "{version:?}: {out:?}"
        );
    }
}

/// A refusal carries the header too: the line shows, and the passport's
/// answer stands.
#[test]
fn a_refusal_of_another_version_gets_the_line_too() {
    let rig = Rig::new("skew-403");
    rig.stub.reset(&[headed(
        403,
        Some(OTHER),
        r#"{"error":"only the owner can read the CLI usage counts"}"#,
    )]);
    let out = rig.run(
        &["cli", "status"],
        &[("K2_PORT", &rig.port()), ("K2_HOOK_TOKEN", PASSPORT)],
    );
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(
        out.stderr,
        format!(
            "{}\n{}\n",
            skew::line(ours(), OTHER),
            k5_cli::cli_status::OWNER_ONLY_LINE
        )
    );
}
