//! `k5-cli vault run` (`families::vault::run`) against `cli/k2 vault run`
//! on the vault rig ([`common::vault`]), with a fake run helper the stub
//! daemon names. Each side must print the same, exit the same, send the
//! same requests and leave the same helper log: the helper's argv, the
//! passport it read on fd 3, and whether any passport name or value is
//! in its environment. K2's ruling 1b, the binary's side of it:
//!
//! - the passport reaches the helper on fd 3, never in its environment
//!   (the log, and the rig's spies: the helper's own children see its
//!   environment, and no passport may be in it);
//! - nothing is sent and nothing runs for a line the shell refuses, and
//!   an old helper (no `"tokenFd": true`) never runs;
//! - the helper's exit code is the command's.
//!
//! A run line with `-h` or `--help` anywhere is the front's, and so the
//! shell's when it is no page ([`help_runs_nothing`]), even when the
//! `--help` is the command's own, after `--`.
//!
//! What the command itself gets (no passport, nothing K2-private,
//! `K2_NO_OWNER_FALLBACK=1`) is the real helper's work:
//! `tests/cli/vault_run_child_identity.sh` proves it through the binary
//! (cli-parity runs it with the binary as the CLI under test).

#![cfg(unix)]

mod common;

use common::parity::{PASSPORT, PERMISSION_ASK};
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};
use std::path::{Path, PathBuf};

const K5_PASSPORT: &str = "sessB.passportK5x";
const K2SO_PASSPORT: &str = "sessC.passportSOx";

/// The fake helper: writes what it was given to `log`, exits
/// `$FAKE_EXIT` (0 when unset).
fn helper(dir: &Path) -> (PathBuf, PathBuf) {
    let path = dir.join("fake-helper");
    let log = dir.join("helper.log");
    let script = format!(
        r#"#!/bin/sh
fd=""
prev=""
for a in "$@"; do [ "$prev" = "--token-fd" ] && fd="$a"; prev="$a"; done
{{
    for a in "$@"; do printf 'ARG=%s\n' "$a"; done
    if [ "${{K2_HOOK_SOCK+set}}" = set ]; then
        if [ "$K2_HOOK_SOCK" = "${{EXPECT_SOCK:-}}" ]; then echo "SOCK=the session's"; else echo "SOCK=another"; fi
    else
        echo "SOCK=unset"
    fi
    printf 'K2_HOOK_TOKEN=%s\n' "${{K2_HOOK_TOKEN-unset}}"
    printf 'K5_HOOK_TOKEN=%s\n' "${{K5_HOOK_TOKEN-unset}}"
    printf 'K2SO_HOOK_TOKEN=%s\n' "${{K2SO_HOOK_TOKEN-unset}}"
    case "$(export -p)" in
        *{PASSPORT}*|*{K5_PASSPORT}*|*{K2SO_PASSPORT}*) echo "A PASSPORT IS IN THE ENVIRONMENT" ;;
        *) echo "no passport in the environment" ;;
    esac
    if [ "$fd" = 3 ]; then printf 'FD3=%s\n' "$(cat <&3)"; fi
}} >'{log}'
exit "${{FAKE_EXIT:-0}}"
"#,
        log = log.display()
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    (path, log)
}

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

/// A line that asks the daemon once and gets `reply`.
fn asks(name: &'static str, args: &[&'static str], reply: Reply, log: &Path, code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.daemon = vec![reply];
    c.daemon_requests = 1;
    c.code = code;
    c.files = vec![log.to_path_buf()];
    c.secrets = vec![PASSPORT, K5_PASSPORT, K2SO_PASSPORT];
    c
}

/// Nothing sent, nothing run: the lines, exit 2.
fn refused(name: &'static str, args: &[&'static str], log: &Path) -> VCase {
    let mut c = VCase::new(name, args);
    c.code = 2;
    c.files = vec![log.to_path_buf()];
    c
}

fn session(mut c: VCase) -> VCase {
    c.env.push(("K2_HOOK_TOKEN", PASSPORT.to_string()));
    c
}

#[test]
fn the_passport_goes_on_fd_3_and_nowhere_else() {
    let rig = VRig::new();
    let (h, log) = helper(rig.path());
    let named = format!(r#"{{"helper":"{}","tokenFd":true}}"#, h.display());
    // covers: vault run
    let mut cell = session(asks(
        "in a session: the cell socket",
        &["run", "--secret", "A", "--", "true"],
        ok(&named),
        &log,
        0,
    ));
    cell.env.push(("K2_HOOK_SOCK", "{sock}".to_string()));
    cell.env.push(("EXPECT_SOCK", "{sock}".to_string()));
    let mut three = session(asks(
        "a passport under each name: K5_ is the one",
        &["run", "--secret", "A", "--", "true"],
        ok(&named),
        &log,
        0,
    ));
    three.env.push(("K5_HOOK_TOKEN", K5_PASSPORT.to_string()));
    three
        .env
        .push(("K2SO_HOOK_TOKEN", K2SO_PASSPORT.to_string()));
    let mut exit7 = session(asks(
        "the helper's exit is the command's",
        &["run", "A", "--", "false"],
        ok(&named),
        &log,
        7,
    ));
    exit7.env.push(("FAKE_EXIT", "7".to_string()));
    for c in [
        session(asks(
            "every flag, and the command's own words",
            &[
                "run",
                "--secret",
                "A",
                "--secret",
                "B=OTHER",
                "--env-file",
                ".env.k2",
                "--scope",
                "server",
                "--no-mask",
                "--",
                "grep",
                "--secret",
                "x",
                "--",
                "y",
            ],
            ok(&named),
            &log,
            0,
        )),
        cell,
        three,
        asks(
            "the owner on disk: an empty fd 3",
            &["run", "NAME", "--", "ls"],
            ok(&named),
            &log,
            0,
        ),
        exit7,
    ] {
        rig.check(&c);
    }
}

#[test]
fn what_runs_nothing() {
    let rig = VRig::new();
    let (h, log) = helper(rig.path());
    let old = format!(r#"{{"helper":"{}"}}"#, h.display());
    let not_true = format!(r#"{{"helper":"{}","tokenFd":"yes"}}"#, h.display());
    let gone = format!(
        r#"{{"helper":"{}","tokenFd":true}}"#,
        rig.path().join("no-such-helper").display()
    );
    let mut cases = vec![
        session(asks(
            "an old helper: refused, not run",
            &["run", "--secret", "A", "--", "true"],
            ok(&old),
            &log,
            1,
        )),
        session(asks(
            "tokenFd that isn't true",
            &["run", "--secret", "A", "--", "true"],
            ok(&not_true),
            &log,
            1,
        )),
        session(asks(
            "a helper that isn't there",
            &["run", "--secret", "A", "--", "true"],
            ok(&gone),
            &log,
            1,
        )),
        session(asks(
            "no helper named",
            &["run", "--secret", "A", "--", "true"],
            ok(r#"{"helper":"","tokenFd":true}"#),
            &log,
            1,
        )),
        session(asks(
            "a helper that is a number",
            &["run", "--secret", "A", "--", "true"],
            ok(r#"{"helper":5,"tokenFd":true}"#),
            &log,
            1,
        )),
        session(asks(
            "refused",
            &["run", "--secret", "A", "--", "true"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            &log,
            3,
        )),
        session(asks(
            "not found",
            &["run", "--secret", "A", "--", "true"],
            Reply::Answer(404, r#"{"error":{"code":"nope","hint":"h"}}"#.into()),
            &log,
            4,
        )),
        session(asks(
            "no answer",
            &["run", "--secret", "A", "--", "true"],
            Reply::Close,
            &log,
            1,
        )),
    ];
    for body in ["not json", "[1]"] {
        let mut c = session(asks(
            "a 200 Python can't read",
            &["run", "--secret", "A", "--", "true"],
            ok(body),
            &log,
            1,
        ));
        c.compare = VCompare::Traceback;
        cases.push(c);
    }
    for (name, args) in [
        ("no --", &["run", "--secret", "A"][..]),
        ("no command", &["run", "--secret", "A", "--"][..]),
        ("no secret", &["run", "--", "ls"][..]),
        (
            "only --scope",
            &["run", "--scope", "server", "--", "ls"][..],
        ),
        ("names and no --", &["run", "node", "app.js"][..]),
        ("nothing", &["run"][..]),
        ("--secret with no name", &["run", "--secret"][..]),
        ("--secret ''", &["run", "--secret", "", "--", "ls"][..]),
        ("--env-file with no file", &["run", "--env-file"][..]),
        ("--scope with no scope", &["run", "--scope"][..]),
        (
            "an unknown flag",
            &["run", "--bogus", "--secret", "A", "--", "ls"][..],
        ),
        ("--value", &["run", "--value", "hunter2", "--", "ls"][..]),
        ("--value=", &["run", "--value=hunter2", "--", "ls"][..]),
        ("--secret-value", &["run", "--secret-value", "hunter2"][..]),
        (
            "--system",
            &["run", "--system", "--secret", "A", "--", "ls"][..],
        ),
    ] {
        cases.push(session(refused(name, args, &log)));
    }
    // The command's own -h, and no secret: the usage, no gate.
    for c in cases {
        rig.check(&c);
    }
}

/// A run line with `-h` or `--help` anywhere is the front's: a page, or
/// the shell (the binary hands it on), the `-h` before `--` or after it.
/// On both sides it sends nothing and runs nothing, with an owner or with
/// none (the shell skips its connection gate for these).
#[test]
fn help_runs_nothing() {
    let rig = VRig::new();
    let (_, log) = helper(rig.path());
    for (name, args, code) in [
        ("-h", &["run", "-h"][..], 0),
        (
            "--help after a name",
            &["run", "--secret", "A", "--help", "--", "ls"][..],
            0,
        ),
        (
            "-h as the last word before --",
            &["run", "A", "-h", "--", "ls"][..],
            0,
        ),
        ("an unknown flag, then -h", &["run", "--bogus", "-h"][..], 2),
        (
            "no secret, and the command's --help",
            &["run", "--", "ls", "--help"][..],
            2,
        ),
    ] {
        for owner in [true, false] {
            let mut c = refused(name, args, &log);
            c.code = code;
            c.owner_on_disk = owner;
            c.handed = true;
            rig.check(&c);
        }
    }
}
