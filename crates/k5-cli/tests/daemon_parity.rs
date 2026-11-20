//! `k5-cli daemon` and `daemon status [--json]` against the shell CLI's
//! `cli/k2 daemon`, on the parity rig ([`common::parity`]). Every row puts
//! stand-ins for `systemctl` and `launchctl` first on `PATH`, so no row
//! reads (or could touch) this machine's real service: each answers from
//! the files the row writes.
//!
//! - The bare line (the status) and `status --json`: a unit found by
//!   name (user and system scope), none found, a pid of 0, an inactive
//!   unit; the k2-ptyd lines from `/boot-status` (an answer, odd answers,
//!   none); `daemon.port` and `daemon.token` there or not.
//! - The gates, and the `daemon-admin` tool in a session with no
//!   passport.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::path::PathBuf;

const VERB: &str = "daemon";

/// A folder of stand-ins: `systemctl` answers from `$K2_TEST_SYSTEMD`
/// (a file of `scope unit state pid` words), `launchctl print` from
/// `$K2_TEST_LAUNCHD` (its text).
fn stand_ins(rig: &Rig) -> PathBuf {
    let dir = rig.dir.join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let systemctl = r#"#!/bin/sh
scope=system
if [ "$1" = --user ]; then scope=user; shift; fi
f="${K2_TEST_SYSTEMD:-/nonexistent}"
[ -f "$f" ] || exit 1
read s u st pid < "$f"
[ "$s" = "$scope" ] || exit 1
case "$1" in
  cat) [ "$2" = "$u" ] ;;
  is-active) echo "$st"; [ "$st" = active ] ;;
  show) echo "$pid" ;;
  *) exit 1 ;;
esac
"#;
    let launchctl = r#"#!/bin/sh
f="${K2_TEST_LAUNCHD:-/nonexistent}"
[ -f "$f" ] || exit 1
cat "$f"
"#;
    for (name, text) in [("systemctl", systemctl), ("launchctl", launchctl)] {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

fn files(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.k2home.join("daemon.port"), format!("{}\n", r.stub().port)).unwrap();
    std::fs::write(r.k2home.join("daemon.token"), "tok\n").unwrap();
    std::fs::write(
        r.dir.join("user-unit"),
        "user k2-daemon.service active 4242\n",
    )
    .unwrap();
    std::fs::write(
        r.dir.join("system-unit"),
        "system k2so-daemon.service inactive 0\n",
    )
    .unwrap();
    std::fs::write(
        r.dir.join("launchd"),
        "gui/501/dev.k2.daemon = {\n\tstate = running\n\tpid = 4242\n\tlast exit code = 1 \n}\n",
    )
    .unwrap();
}

fn no_port_file(r: &Rig) {
    owner_and_port(r);
    // The rows before leave theirs.
    let _ = std::fs::remove_file(r.k2home.join("daemon.port"));
    std::fs::write(r.k2home.join("daemon.token"), "").unwrap();
}

fn rig(tag: &str) -> Rig {
    let mut rig = Rig::new(VERB, tag);
    rig.path_first = Some(stand_ins(&rig));
    rig
}

fn env(rig: &Rig, unit: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "K2_TEST_SYSTEMD",
            rig.dir.join(unit).to_string_lossy().into_owned(),
        ),
        (
            "K2_TEST_LAUNCHD",
            rig.dir.join("launchd").to_string_lossy().into_owned(),
        ),
    ]
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// The rig's check, each side on its own stub's port: `daemon.port` holds
/// that side's port, which the status prints, so it is masked as `{port}`
/// in what each side prints and sends. Bash, the binary, and the binary
/// with no `python3` on `PATH`.
fn check_on_own_port(rig: &Rig, c: &Case) {
    let mask = |side: Side, t: &str| t.replace(&rig.stub_of(side).port.to_string(), "{port}");
    rig.prepare(Side::Bash, c);
    let out = rig.run(true, c);
    let log: Vec<String> = rig
        .read(Side::Bash, false, c.name)
        .iter()
        .map(|r| mask(Side::Bash, &normalise(r, false, VERB, c.name)))
        .collect();
    let bash = (
        out.code,
        mask(Side::Bash, &out.stdout),
        mask(Side::Bash, &out.stderr),
        log,
    );
    assert_eq!(bash.0, Some(c.code), "{}: bash's exit", c.name);
    assert_eq!(bash.3.len(), c.requests, "{}: bash's requests", c.name);
    for (side, without) in [(Side::Rust, false), (Side::NoPython3, true)] {
        let (out, log) = rig.binary(c, without);
        let log: Vec<String> = log.iter().map(|l| mask(side, l)).collect();
        let got = (
            out.code,
            mask(side, &out.stdout),
            mask(side, &out.stderr),
            log,
        );
        assert_eq!(got, bash, "{}: {side:?} against bash", c.name);
    }
}

#[test]
fn daemon_status_answers_as_the_shell_does() {
    let rig = rig("status");
    let ptyd = || ok(r#"{"ptyd":"on\tnow","adopted":3}"#);
    let v = vec![
        // covers: daemon
        // A bash bug, copied: the router's `shift 2` with the noun alone
        // fails under `set -e` (cli/src/router.sh, the daemon arm), so
        // `daemon` alone ends with 1 and prints nothing.
        with(
            env(&rig, "user-unit"),
            case(
                "daemon alone: the router's shift 2 ends it",
                &[],
                &[],
                vec![],
                files,
                1,
                0,
            ),
        ),
        with(
            env(&rig, "user-unit"),
            case(
                "status: a user unit",
                &["status"],
                &[],
                vec![ptyd()],
                files,
                0,
                1,
            ),
        ),
        with(
            env(&rig, "user-unit"),
            case(
                "status --json",
                &["status", "--json"],
                &[],
                vec![ptyd()],
                files,
                0,
                1,
            ),
        ),
        with(
            env(&rig, "system-unit"),
            case(
                "a system unit, inactive, pid 0",
                &["status"],
                &[],
                vec![ok(r#"{"ptyd":"off","adopted":true}"#)],
                files,
                0,
                1,
            ),
        ),
        with(
            env(&rig, "none"),
            case(
                "no unit, no ptyd in the answer",
                &["status", "x", "--json"],
                &[],
                vec![ok(r#"{"phase":"ready"}"#)],
                files,
                0,
                1,
            ),
        ),
        with(
            env(&rig, "user-unit"),
            case(
                "an answer that is not JSON",
                &["status"],
                &[],
                vec![Reply::Answer(500, "nope".into())],
                files,
                0,
                1,
            ),
        ),
        with(
            env(&rig, "user-unit"),
            case(
                "no answer to /boot-status",
                &["status", "--json"],
                &[],
                vec![Reply::Close],
                files,
                0,
                1,
            ),
        ),
        with(
            env(&rig, "user-unit"),
            case(
                "no daemon.port: no probe; an empty token",
                &["status"],
                &[],
                vec![],
                no_port_file,
                0,
                0,
            ),
        ),
    ];
    for c in v {
        check_on_own_port(&rig, &c);
    }
}

#[test]
fn daemon_gates() {
    let rig = rig("gates");
    let v = vec![
        case(
            "no daemon set up: the gate",
            &["status"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "the guard",
            &["status"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            port_only,
            3,
            0,
        ),
        case(
            "a session with no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the product and the command by the brand's names.
#[test]
fn daemon_lines_say_the_brand_s_name() {
    let rig = rig("brand");
    let c = with(
        env(&rig, "user-unit"),
        case(
            "brand",
            &["status"],
            &[],
            vec![ok(r#"{"ptyd":"on","adopted":1}"#)],
            files,
            0,
            1,
        ),
    );
    let (out, _) = rig.binary(&c, false);
    let product = k2_client::brand::PRODUCT_NAME;
    let cli = k2_client::brand::CLI_NAME;
    assert!(
        out.stdout.starts_with(&format!("{product} Daemon\n")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains(&format!("(k2-ptyd; {cli} daemon survive status)")),
        "{}",
        out.stdout
    );
}
