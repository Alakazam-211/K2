//! `k5-cli update` against the shell CLI's `cli/k2 update`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code, and the same programs run with the same arguments.
//!
//! GitHub is never asked: a stub `curl` first on `PATH`, on every side,
//! answers `https://api.github.com/…` from the case's `UPDATE_GH_BODY`
//! and `UPDATE_GH_RC`, writes `UPDATE_GH_ERR` on stderr (noise when it is
//! not set), and logs its arguments,
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
//!   (before and after an answer, with curl's message, with none, with
//!   noise), no temp folder for mktemp, the proxy settings passed
//!   through, no daemon (no gate), the virtual desktop's guard, the forms
//!   the words pick; `--help` goes to the shell (which prints the usage
//!   and asks nothing, with or without a daemon).
//! - `--list`: releases (a pre-release, no date, a date that is a list),
//!   none, an object, a raise part way, curl failing (with and without an
//!   answer), not JSON, the flags around it, no daemon.
//! - air-gap (`update`, `--app`, `--list`): on and off by `K2_AIRGAP` (and
//!   it winning over the file), by `settings.json` (true, false, a true
//!   anywhere), the file unreadable (a value that is neither, a key with
//!   no value, a folder, a link to nothing), none; with no daemon too;
//!   `--cli` and `--help` are not refused.

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
        if [ "${{UPDATE_GH_ERR+x}}" = x ]; then
            printf '%s' "$UPDATE_GH_ERR" >&2
        else
            printf 'curl: noise on stderr\n' >&2
        fi
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

fn gh_err(body: &str, rc: i32, err: &str) -> Vec<(&'static str, String)> {
    vec![
        ("UPDATE_GH_BODY", body.to_string()),
        ("UPDATE_GH_RC", rc.to_string()),
        ("UPDATE_GH_ERR", err.to_string()),
    ]
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn no_tmpdir(c: Case) -> Case {
    Case {
        no_tmpdir: true,
        ..c
    }
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
    let port = PORT.to_string();
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
            with_passport(&[]),
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
            case("curl fails", &[], &[], vec![], owner_and_port, 1, 0),
        ),
        with(
            gh_rc(newest, 56),
            case(
                "curl fails after an answer",
                &[],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        with(
            gh_err(
                "<html>404</html>",
                22,
                "curl: (22) The requested URL returned error: 404\nmore\n",
            ),
            case(
                "curl fails with its message",
                &["--app"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        with(
            gh_err("", 28, ""),
            case(
                "curl fails and says nothing",
                &[],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        no_tmpdir(with(
            gh(newest),
            case(
                "no temp folder for mktemp",
                &[],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        )),
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
            case("no daemon: no gate", &[], &[], vec![], nothing, 0, 0),
        ),
        with(
            [gh(newest), vec![("K2_PORT", "1x".into())]].concat(),
            case(
                "--app, a port that is no port: unused",
                &["--app"],
                &[],
                vec![],
                owner_only,
                0,
                0,
            ),
        ),
        with(
            [gh(newest), vec![("K2_NO_OWNER_FALLBACK", "1".into())]].concat(),
            case(
                "the virtual desktop's guard",
                &["--app"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        with(
            gh(newest),
            case(
                "--check alone: the app",
                &["--check"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh(newest),
            case(
                "-c and -l: the app",
                &["-c", "-l"],
                &[],
                vec![],
                nothing,
                0,
                0,
            ),
        ),
        with(
            gh(newest),
            case(
                "--cli, then --app last",
                &["--cli", "-c", "--app"],
                &[],
                vec![],
                nothing,
                0,
                0,
            ),
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
        shell(with(
            gh(newest),
            case(
                "--cli -h, no daemon: the shell's",
                &["--cli", "-h"],
                &[],
                vec![],
                nothing,
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
fn update_list_answers_as_the_shell_does() {
    let rig = rig("list");
    let releases = r#"[
        {"tag_name":"v0.47.0-rc1","published_at":"2026-11-19T00:00:00Z","prerelease":true},
        {"tag_name":"v0.46.20","published_at":"2026-11-18T01:02:03Z","prerelease":false},
        {"tag_name":"v0.46.19 é","prerelease":0},
        {"published_at":["a",1,2,3,4,5,6,7,8,9,10,11],"prerelease":"yes"}]"#;
    let v = vec![
        with(
            gh(releases),
            case("--list", &["--list"], &[], vec![], owner_and_port, 0, 0),
        ),
        with(
            gh(releases),
            case(
                "--cli --list",
                &["--cli", "--list"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh(releases),
            case(
                "-c, a word, --list, --cli",
                &["-c", "x", "--list", "--cli"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh("[]"),
            case("none", &["--list"], &[], vec![], owner_and_port, 0, 0),
        ),
        with(
            gh("{}"),
            case(
                "an empty object",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            gh(r#"{"message":"Not Found"}"#),
            case("an object", &["--list"], &[], vec![], owner_and_port, 1, 0),
        ),
        with(
            gh(r#"[{"tag_name":"a"},{"published_at":null}]"#),
            case(
                "a raise part way",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        with(
            gh_rc("", 6),
            case(
                "curl fails, nothing read",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        with(
            gh_rc(releases, 56),
            case(
                "curl fails after a whole answer",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        with(
            gh_err(
                "",
                7,
                "curl: (7) Failed to connect to api.github.com port 443: Couldn't connect to server\n",
            ),
            case(
                "curl fails with its message",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        no_tmpdir(with(
            gh(releases),
            case(
                "no temp folder for mktemp",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        )),
        with(
            gh("<html>"),
            case("not JSON", &["--list"], &[], vec![], owner_and_port, 1, 0),
        ),
        with(
            gh(releases),
            case(
                "no daemon: no gate",
                &["--list"],
                &[],
                vec![],
                nothing,
                0,
                0,
            ),
        ),
        with(
            gh(releases),
            case(
                "--app, --list, --cli: the list",
                &["--app", "--list", "--cli"],
                &[],
                vec![],
                nothing,
                0,
                0,
            ),
        ),
        with(
            [gh(releases), vec![("K2_NO_OWNER_FALLBACK", "1".into())]].concat(),
            case(
                "the virtual desktop's guard",
                &["--list"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
    ];
    for c in &v {
        rig.check(c);
    }
    assert_native(&rig, &v[0]);
}

/// The K2 home's `settings.json`, made as `body` says (`None`: none);
/// what was there before is gone.
fn settings_file(r: &Rig, body: Option<&str>) -> std::path::PathBuf {
    let f = k2_client::paths::k2_home_in(&r.home).join("settings.json");
    if let Ok(m) = std::fs::symlink_metadata(&f) {
        if m.is_dir() {
            std::fs::remove_dir(&f).unwrap();
        } else {
            std::fs::remove_file(&f).unwrap();
        }
    }
    if let Some(body) = body {
        std::fs::write(&f, body).unwrap();
    }
    f
}

fn no_settings(r: &Rig) {
    owner_and_port(r);
    settings_file(r, None);
}

fn airgap_true(r: &Rig) {
    owner_and_port(r);
    settings_file(
        r,
        Some("{\n  \"theme\": \"dark\",\n  \"airgap\": true\n}\n"),
    );
}

fn airgap_false(r: &Rig) {
    owner_and_port(r);
    settings_file(
        r,
        Some("{\n  \"airgap\": false,\n  \"theme\": \"dark\"\n}\n"),
    );
}

fn airgap_true_somewhere(r: &Rig) {
    owner_and_port(r);
    settings_file(r, Some(r#"{"airgap":false,"x":{"airgap":true}}"#));
}

fn airgap_neither(r: &Rig) {
    owner_and_port(r);
    settings_file(r, Some(r#"{"airgap":"yes"}"#));
}

fn airgap_no_value(r: &Rig) {
    owner_and_port(r);
    settings_file(r, Some(r#"{"airgap":false,"keys":["airgap"]}"#));
}

fn airgap_folder(r: &Rig) {
    owner_and_port(r);
    let f = settings_file(r, None);
    std::fs::create_dir(&f).unwrap();
}

fn airgap_link_to_nothing(r: &Rig) {
    owner_and_port(r);
    let f = settings_file(r, None);
    std::os::unix::fs::symlink(r.dir.join("nowhere.json"), &f).unwrap();
}

fn airgap_true_no_daemon(r: &Rig) {
    settings_file(r, Some(r#"{"airgap":true}"#));
}

#[test]
fn update_under_air_gap_answers_as_the_shell_does() {
    let rig = rig("airgap");
    let newest = r#"{"tag_name":"v0.46.20"}"#;
    let releases = r#"[{"tag_name":"v0.46.20","published_at":"2026-11-18T01:02:03Z"}]"#;
    let env = |pairs: &[(&'static str, &str)], body: &str| -> Vec<(&'static str, String)> {
        let mut v = gh(body);
        v.extend(pairs.iter().map(|(k, v)| (*k, v.to_string())));
        v
    };
    let v = vec![
        with(
            env(&[], newest),
            case(
                "on in the file: update",
                &[],
                &[],
                vec![],
                airgap_true,
                1,
                0,
            ),
        ),
        with(
            env(&[], releases),
            case(
                "on in the file: --list",
                &["--list"],
                &[],
                vec![],
                airgap_true,
                1,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case(
                "on in the file, no daemon: refused, not the gate",
                &["--app"],
                &[],
                vec![],
                airgap_true_no_daemon,
                1,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case(
                "a true anywhere",
                &["--app"],
                &[],
                vec![],
                airgap_true_somewhere,
                1,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case("off in the file", &[], &[], vec![], airgap_false, 0, 0),
        ),
        with(
            env(&[], releases),
            case(
                "no settings file",
                &["--list"],
                &[],
                vec![],
                no_settings,
                0,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case(
                "unreadable: a value that is neither",
                &[],
                &[],
                vec![],
                airgap_neither,
                1,
                0,
            ),
        ),
        with(
            env(&[], releases),
            case(
                "unreadable: a key with no value",
                &["--list"],
                &[],
                vec![],
                airgap_no_value,
                1,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case(
                "unreadable: a folder",
                &["--app"],
                &[],
                vec![],
                airgap_folder,
                1,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case(
                "unreadable: a link to nothing",
                &[],
                &[],
                vec![],
                airgap_link_to_nothing,
                1,
                0,
            ),
        ),
        with(
            env(&[("K2_AIRGAP", "1")], newest),
            case("K2_AIRGAP=1", &[], &[], vec![], no_settings, 1, 0),
        ),
        with(
            env(&[("K2_AIRGAP", "")], releases),
            case(
                "K2_AIRGAP empty is on",
                &["--list"],
                &[],
                vec![],
                airgap_false,
                1,
                0,
            ),
        ),
        with(
            env(&[("K2_AIRGAP", "maybe")], newest),
            case(
                "K2_AIRGAP neither is on",
                &[],
                &[],
                vec![],
                no_settings,
                1,
                0,
            ),
        ),
        with(
            env(&[("K2_AIRGAP", "OFF")], newest),
            case(
                "K2_AIRGAP=OFF wins over the file",
                &["--app"],
                &[],
                vec![],
                airgap_true,
                0,
                0,
            ),
        ),
        with(
            env(&[("K2_AIRGAP", "no\n")], releases),
            case(
                "K2_AIRGAP=no and a newline is off",
                &["--list"],
                &[],
                vec![],
                airgap_neither,
                0,
                0,
            ),
        ),
        with(
            env(&[("K2_AIRGAP", " no")], newest),
            case(
                "K2_AIRGAP with a space is on",
                &[],
                &[],
                vec![],
                no_settings,
                1,
                0,
            ),
        ),
        with(
            env(&[], newest),
            case(
                "--cli is not refused",
                &["--cli"],
                &[],
                vec![version(ours())],
                airgap_true,
                0,
                1,
            ),
        ),
        shell(with(
            env(&[], newest),
            case(
                "--help is not refused",
                &["--help"],
                &[],
                vec![],
                airgap_true,
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

/// The stub curl is the one both sides run: a GitHub call reaches it,
/// and logs what it was given. Neither side leaves its temp file.
#[test]
fn the_stub_curl_answers_for_github() {
    let rig = rig("stub");
    let c = with(
        gh(r#"{"tag_name":"v9"}"#),
        case("stub", &["--app"], &[], vec![], owner_and_port, 0, 0),
    );
    (c.setup)(&rig);
    for (bash, tmp) in [(true, "tmp-bash"), (false, "tmp-rust")] {
        let out = rig.run(bash, &c);
        assert_eq!(out.code, Some(0), "{out:?}");
        assert!(out.stdout.contains("  Latest:  v9\n"), "{out:?}");
        let log = rig.take_tool_log();
        let mut want = String::from("curl");
        for a in k5_cli::families::update::CURL_ARGS
            .iter()
            .chain([&k5_cli::families::update::LATEST_URL])
        {
            want.push_str(&format!(" [{a}]"));
        }
        assert!(log.starts_with(&format!("{want}\n")), "{log}");
        assert!(!log.contains("UNEXPECTED"), "{log}");
        let left: Vec<_> = std::fs::read_dir(rig.dir.join(tmp))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "{tmp}: {left:?}");
    }
    assert!(Path::new(rig.path_first.as_ref().unwrap())
        .join("curl")
        .is_file());
}
