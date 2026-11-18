//! `k5-cli update` against the shell CLI's `cli/k2 update`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code, and the same programs run with the same arguments.
//!
//! GitHub is never asked: a stub `curl` first on `PATH`, on every side,
//! answers `https://api.github.com/…` from the case's `UPDATE_GH_BODY`
//! and `UPDATE_GH_RC`, writes noise on stderr, and logs its arguments,
//! its folder and the settings curl reads (proxies, CA files, its config
//! file's places) to the rig's tool log, which must be the same on every
//! side. Any other `https://` address is logged as unexpected and
//! refused; anything else (the daemon's `/boot-status`) runs the real
//! curl against the stub daemon.
//!
//! - `--cli [--check|-c]`: the daemon on our version, another one (with a
//!   `v`, Python's whitespace around it, on two lines, a key twice), none
//!   in the answer, not a string, not JSON, a 500 that says one, an empty
//!   reply, a reset, not HTTP, nothing listening, an answer too slow (4 s),
//!   a passport, another host setting (still 127.0.0.1), the bulletin
//!   (no line), the flags in any order, the gates; a port that is no port
//!   goes to the shell.
//! - `update`, `--app`: the newest tag, none (`unknown`), not a string,
//!   with newlines after it, not JSON, a list, nothing, curl failing
//!   (before and after an answer), the proxy settings passed through, the
//!   gate; `--help` goes to the shell (which asks GitHub for it).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

const VERB: &str = "update";

/// What the stub curl writes down for a GitHub call.
const LOGGED: &[&str] = &[
    "HOME",
    "CURL_HOME",
    "XDG_CONFIG_HOME",
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "no_proxy",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "CURL_SSL_BACKEND",
    "SSLKEYLOGFILE",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
];

/// A rig whose `PATH` starts with the stub curl.
fn rig(tag: &str) -> Rig {
    let mut rig = Rig::new(VERB, tag);
    let shim = rig.dir.join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    let path = std::env::var_os("PATH").expect("a PATH to run the tests with");
    let real = std::env::split_paths(&path)
        .map(|d| d.join("curl"))
        .find(|p| p.is_file())
        .expect("a curl on PATH");
    let log = rig.dir.join("tools.log");
    let vars: Vec<String> = LOGGED
        .iter()
        .map(|n| format!("printf '{n}=%s\\n' \"${{{n}-<unset>}}\""))
        .collect();
    let script = format!(
        r#"#!/bin/sh
url=""
for a in "$@"; do
    case "$a" in https://*) url="$a" ;; esac
done
case "$url" in
    "") exec '{real}' "$@" ;;
    https://api.github.com/*)
        {{
            printf 'curl'
            for a in "$@"; do printf ' [%s]' "$a"; done
            printf '\n'
            printf 'pwd=%s\n' "$(pwd)"
            {vars}
        }} >> '{log}'
        printf 'curl: noise on stderr\n' >&2
        printf '%s' "${{UPDATE_GH_BODY-}}"
        exit "${{UPDATE_GH_RC:-0}}"
        ;;
    *)
        printf 'UNEXPECTED %s\n' "$url" >> '{log}'
        exit 97
        ;;
esac
"#,
        real = real.display(),
        log = log.display(),
        vars = vars.join("\n            "),
    );
    let curl = shim.join("curl");
    std::fs::write(&curl, script).unwrap();
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    rig.path_first = Some(shim);
    rig.tool_log = Some(log);
    rig
}

fn ours() -> &'static str {
    k5_cli::embedded::version()
}

fn version(v: &str) -> Reply {
    ok(&format!(r#"{{"phase":"ready","version":"{v}"}}"#))
}

fn gh(body: &str) -> Vec<(&'static str, String)> {
    vec![("UPDATE_GH_BODY", body.to_string())]
}

fn gh_rc(body: &str, rc: i32) -> Vec<(&'static str, String)> {
    vec![
        ("UPDATE_GH_BODY", body.to_string()),
        ("UPDATE_GH_RC", rc.to_string()),
    ]
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

/// The binary answers `c` itself: with no python3 on `PATH` it prints
/// what it prints with one, where a hand-off says Python is missing (69).
fn assert_native(rig: &Rig, c: &Case) {
    let (out, _) = rig.binary(c, true);
    rig.take_tool_log();
    assert_ne!(
        out.code,
        Some(69),
        "{}: handed to the shell: {out:?}",
        c.name
    );
}

#[test]
fn update_cli_answers_as_the_shell_does() {
    let rig = rig("cli");
    let port = rig.stub.port.to_string();
    let v = vec![
        case(
            "--cli, the daemon on our version",
            &["--cli"],
            &[],
            vec![version(ours())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli --check, the daemon on another version",
            &["--cli", "--check"],
            &[],
            vec![version("v0.0.1")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "-c before --cli, another version",
            &["-c", "--cli", "x"],
            &[],
            vec![version("0.0.2")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--app then --cli",
            &["--app", "--cli"],
            &[],
            vec![version(ours())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, Python's whitespace and a v around it",
            &["--cli"],
            &[],
            vec![ok(r#"{"version":"  v0.0.3 \u001c\n"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, a version on two lines",
            &["--cli"],
            &[],
            vec![ok(r#"{"version":"0.1\n0.2"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, the key twice",
            &["--cli"],
            &[],
            vec![ok(r#"{"version":"0.0.4","version":"0.0.5"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, only a v",
            &["--cli"],
            &[],
            vec![ok(r#"{"version":"v"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, no version in the answer",
            &["--cli"],
            &[],
            vec![ok(r#"{"view":"public","phase":"ready"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, a version that is not a string",
            &["--cli"],
            &[],
            vec![ok(r#"{"version":46}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, not JSON",
            &["--cli"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, a list",
            &["--cli"],
            &[],
            vec![ok(r#"["version"]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, a 500 that says a version",
            &["--cli"],
            &[],
            vec![Reply::Answer(500, r#"{"version":"0.0.6"}"#.into())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, the bulletin in the answer",
            &["--cli"],
            &[],
            vec![ok(&format!(
                r#"{{"version":"{}","bulletin":{{"day":"2026-11-17","unread":true,"key":"ab12"}}}}"#,
                ours()
            ))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, an empty reply",
            &["--cli"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, a reset",
            &["--cli", "--check"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, not HTTP",
            &["--cli"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--cli, an answer too slow",
            &["--cli"],
            &[],
            vec![Reply::Hang(Duration::from_secs(5))],
            owner_and_port,
            0,
            1,
        ),
        with(
            vec![("K2_PORT", dead_port())],
            case(
                "--cli -c, nothing listening",
                &["--cli", "-c"],
                &[],
                vec![],
                owner_only,
                0,
                0,
            ),
        ),
        with(
            with_passport(rig.stub.port, &[]),
            case(
                "--cli, a passport",
                &["--cli"],
                &[],
                vec![version(ours())],
                nothing,
                0,
                1,
            ),
        ),
        with(
            vec![("K2_HOST", "127.0.0.9".into()), ("K2_PORT", port.clone())],
            case(
                "--cli, another host setting: still 127.0.0.1",
                &["--cli"],
                &[],
                vec![version("0.0.7")],
                owner_only,
                0,
                1,
            ),
        ),
        case("--cli, the gate", &["--cli"], &[], vec![], nothing, 1, 0),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".into())],
            case(
                "--cli, the virtual desktop's guard",
                &["--cli"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        shell(with(
            vec![("K2_PORT", "1x".into())],
            case(
                "--cli, a port that is no port",
                &["--cli"],
                &[],
                vec![],
                owner_only,
                0,
                0,
            ),
        )),
    ];
    for c in &v {
        rig.check(c);
    }
    assert_native(&rig, &v[0]);
}

#[test]
fn update_app_answers_as_the_shell_does() {
    let rig = rig("app");
    let newest = r#"{"tag_name":"v0.46.20","name":"K2 0.46.20","prerelease":false}"#;
    let v = vec![
        with(
            gh(newest),
            case("the newest release", &[], &[], vec![], owner_and_port, 0, 0),
        ),
        with(
            gh(newest),
            case("--app", &["--app"], &[], vec![], owner_and_port, 0, 0),
        ),
        with(
            gh(newest),
            case(
                "--cli then --app, and words skipped",
                &["--cli", "x", "--app", "--check"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh(r#"{"message":"API rate limit exceeded"}"#),
            case("no tag: unknown", &[], &[], vec![], owner_and_port, 0, 0),
        ),
        with(
            gh(r#"{"tag_name":null}"#),
            case(
                "a tag that is not a string",
                &[],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh("{\"tag_name\":\"v1 \\u00e9\\n\\n\"}\n\n"),
            case(
                "newlines after the tag",
                &[],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh("<html>"),
            case("not JSON", &[], &[], vec![], owner_and_port, 1, 0),
        ),
        with(
            gh("[1]"),
            case("a list", &[], &[], vec![], owner_and_port, 1, 0),
        ),
        with(
            gh(""),
            case("nothing", &[], &[], vec![], owner_and_port, 1, 0),
        ),
        with(
            gh_rc("", 6),
            case("curl fails", &[], &[], vec![], owner_and_port, 6, 0),
        ),
        with(
            gh_rc(newest, 56),
            case(
                "curl fails after an answer",
                &[],
                &[],
                vec![],
                owner_and_port,
                56,
                0,
            ),
        ),
        with(
            [
                gh(newest),
                vec![
                    ("https_proxy", "http://127.0.0.1:9".into()),
                    ("NO_PROXY", "example.com".into()),
                    ("CURL_HOME", "/nowhere/curl-home".into()),
                    ("SSL_CERT_FILE", "/nowhere/ca.pem".into()),
                ],
            ]
            .concat(),
            case(
                "curl's settings reach curl",
                &["--app"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh(newest),
            case("the gate", &[], &[], vec![], nothing, 1, 0),
        ),
        shell(with(
            gh(newest),
            case(
                "--help: the shell's",
                &["--help"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        )),
    ];
    for c in &v {
        rig.check(c);
    }
    assert_native(&rig, &v[0]);
}

#[test]
fn the_stub_curl_answers_for_github() {
    let rig = rig("stub");
    let c = with(
        gh(r#"{"tag_name":"v9"}"#),
        case("stub", &["--app"], &[], vec![], owner_and_port, 0, 0),
    );
    (c.setup)(&rig);
    let out = rig.run(true, &c);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert!(out.stdout.contains("  Latest:  v9\n"), "{out:?}");
    let log = rig.take_tool_log();
    assert!(
        log.starts_with(&format!(
            "curl [-sH] [Accept: application/vnd.github.v3+json] [{}]\n",
            k5_cli::families::update::LATEST_URL
        )),
        "{log}"
    );
    assert!(!log.contains("UNEXPECTED"), "{log}");
    assert!(Path::new(rig.path_first.as_ref().unwrap())
        .join("curl")
        .is_file());
}
