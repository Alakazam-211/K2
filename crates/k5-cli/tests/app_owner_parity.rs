//! `k5-cli app uninstall|expose|handover|unpark|park|release|exception`
//! against the shell CLI's (`cli/src/app-doctor.sh`), on the parity rig
//! ([`common::parity`]) and, where the owner confirms in person, the
//! vault rig ([`common::vault`]): the same requests to the daemon and the
//! vault socket, stdout, stderr and exit code.
//!
//! - `uninstall`: plain, `--drop-access`, `--json`, `--purge` with the
//!   vault password (and with no vault socket), refused, the words.
//! - `expose`: tunnel, local, held, `--json`, Python raising, the words.
//! - `handover`: its page; `status` (none, rows, `--detail`, `--json`),
//!   `brief`, each sub-act (done, held, refused), `plan` and `--dry-run`,
//!   the offer with every flag and a brief file, the legacy answer, the
//!   tip in a session, the confirm the daemon asks for (with the vault
//!   password, and with no vault socket); `unpark`.
//! - `park`, `release` (done, held, `--json`), `exception` (`off`, `on`
//!   with devices and the vault password).
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use common::vault::{VCase, VRig};

const VERB: &str = "app";

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// `NOTES.md`, a brief.
fn files(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(
        rig.proj.join("NOTES.md"),
        "Run the tests first.\nThen é.\n\n",
    )
    .unwrap();
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn one(name: &'static str, args: &'static [&'static str], r: Reply, code: i32) -> Case {
    case(name, args, &[], vec![r], files, code, 1)
}

fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], files, code, 0)
}

fn in_session(c: Case) -> Case {
    Case {
        env: with_passport(&[]),
        setup: nothing,
        ..c
    }
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

const REFUSED_403: &str = r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#;
const HELD: &str = r#"{"status":"held","request":{"id":"req-1","summary":"expose"},"hint":"ask"}"#;
const CONFIRM: &str = r#"{"error":{"code":"vault_confirm_required","action":"app-handover","secretId":"acme","hint":"the owner in person"}}"#;
const OFFER: &str = r#"{"status":"held","plan":{"lines":["the App moves","its data é"],"blockers":[{"code":"b","hint":"h"}]},"handover":{"id":"ho-1","state":"offered","toHandle":"bob"},"request":{"id":"req-9"},"hint":"bob accepts next"}"#;

#[test]
fn uninstall_expose_park_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "own1");
    check(
        &rig,
        vec![
            // covers: app uninstall
            one("uninstall", &["uninstall", "acme"], ok(r#"{"ok":true}"#), 0),
            // covers: app uninstall --json
            one(
                "uninstall --drop-access --json",
                &["uninstall", "--drop-access", "acme", "--json"],
                ok(r#"{"ok":true,"x":"é"}"#),
                0,
            ),
            // covers: app uninstall
            one(
                "uninstall refused",
                &["uninstall", "acme"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app uninstall
            case(
                "uninstall --purge, no vault socket",
                &["uninstall", "acme", "--purge"],
                &[],
                vec![],
                files,
                3,
                0,
            ),
            // covers: app uninstall
            words("uninstall, no App", &["uninstall"], 2),
            // covers: app uninstall
            words(
                "uninstall, an unknown flag",
                &["uninstall", "a", "--zap"],
                2,
            ),
            // covers: app expose
            one(
                "expose --tunnel: changed, a URL",
                &["expose", "acme", "--tunnel", "--transfer"],
                ok(
                    r#"{"app":"acme","changed":true,"expose":"tunnel","url":"https://acme.example","next":"done é"}"#,
                ),
                0,
            ),
            // covers: app expose
            one(
                "expose --local: changed, a hostname left",
                &["expose", "--local", "acme"],
                ok(
                    r#"{"app":"acme","changed":true,"expose":"local","hostnameLeft":"acme.example stays yours"}"#,
                ),
                0,
            ),
            // covers: app expose
            one(
                "expose: no change",
                &["expose", "acme", "--tunnel", "--tunnel"],
                ok(r#"{"app":"acme","changed":false,"next":null}"#),
                0,
            ),
            // covers: app expose --json
            one(
                "expose --json",
                &["expose", "acme", "--local", "--json"],
                ok(r#"{"changed":true}"#),
                0,
            ),
            // covers: app expose
            one(
                "expose held",
                &["expose", "acme", "--tunnel"],
                answer(202, HELD),
                2,
            ),
            // covers: app expose
            one(
                "expose refused",
                &["expose", "acme", "--tunnel"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app expose
            traceback(one(
                "expose: a URL that is a number",
                &["expose", "acme", "--tunnel"],
                ok(r#"{"changed":true,"expose":"tunnel","url":5}"#),
                1,
            )),
            // covers: app expose
            words("expose, no mode", &["expose", "acme"], 2),
            // covers: app expose
            words(
                "expose, both",
                &["expose", "acme", "--local", "--tunnel"],
                2,
            ),
            // covers: app expose
            words("expose, two Apps", &["expose", "a", "b", "--local"], 2),
            // covers: app expose
            words(
                "expose --transfer --local",
                &["expose", "a", "--local", "--transfer"],
                2,
            ),
            // covers: app park
            one("park", &["park", "acme", "--reason", "away é"], ok("{}"), 0),
            // covers: app park --json
            one(
                "park --json",
                &["park", "acme", "--json", "--to", "x"],
                ok(r#"{"parked":true}"#),
                0,
            ),
            // covers: app park
            one(
                "park refused",
                &["park", "acme"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app park
            words("park, no App", &["park"], 2),
            // covers: app park
            words(
                "park --reason last: set -e",
                &["park", "acme", "--reason"],
                1,
            ),
        ],
    );
}

#[test]
fn handover_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "handover");
    let rows = r#"{"handovers":[{"id":"ho-1","state":"offered","appId":"acme","toHandle":"bob","inWindow":true,"windowUntil":"2026-11-21","rekeyState":"done","requestId":"req-1","plan":{"lines":["moves é"]}},{"id":"ho-2","state":"done","appId":"globex","fromHandle":"ann","toHandle":"cy"}]}"#;
    check(
        &rig,
        vec![
            // covers: app handover
            words("the page", &["handover"], 2),
            // covers: app handover status
            one("status: none", &["handover", "status"], ok("{}"), 0),
            // covers: app handover status
            one("status: rows", &["handover", "status", "acme"], ok(rows), 0),
            // covers: app handover status
            one(
                "status --detail",
                &["handover", "status", "--detail"],
                ok(rows),
                0,
            ),
            // covers: app handover status --json
            one(
                "status --json",
                &["handover", "status", "--json"],
                ok(rows),
                0,
            ),
            // covers: app handover status
            traceback(one(
                "status: a row with no id",
                &["handover", "status"],
                ok(r#"{"handovers":[{"state":"x"}]}"#),
                1,
            )),
            // covers: app handover brief
            one(
                "brief",
                &["handover", "brief", "acme"],
                ok(r#"{"brief":"do X é"}"#),
                0,
            ),
            // covers: app handover brief
            one(
                "brief: none",
                &["handover", "brief", "acme"],
                ok(r#"{"brief":""}"#),
                0,
            ),
            // covers: app handover brief
            one(
                "brief --json",
                &["handover", "brief", "acme", "--json"],
                ok(r#"{"brief":"b"}"#),
                0,
            ),
            // covers: app handover brief
            words("brief, no App: the page", &["handover", "brief"], 2),
            // covers: app handover accept
            one(
                "accept: done",
                &["handover", "accept", "acme"],
                ok(
                    r#"{"handover":{"id":"ho-1","state":"accepted","fromHandle":"ann","toHandle":"bob"},"already":true,"workspace":"bob"}"#,
                ),
                0,
            ),
            // covers: app handover decline
            one(
                "decline --reason: held",
                &["handover", "decline", "acme", "--reason", "busy"],
                answer(202, OFFER),
                2,
            ),
            // covers: app handover undo
            one(
                "undo: held, no terminal",
                &["handover", "undo", "acme"],
                answer(202, OFFER),
                2,
            ),
            // covers: app handover cancel
            one(
                "cancel --json: held",
                &["handover", "cancel", "acme", "--json"],
                answer(202, OFFER),
                2,
            ),
            // covers: app handover finish
            one(
                "finish --json",
                &["handover", "finish", "--json", "acme"],
                ok("{}"),
                0,
            ),
            // covers: app handover rekey
            one(
                "rekey refused",
                &["handover", "rekey", "acme"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app handover accept
            words("accept, no App: the page", &["handover", "accept"], 2),
            // covers: app handover plan
            one(
                "plan: ready",
                &["handover", "plan", "acme", "--to", "bob"],
                ok(r#"{"lines":["one","two"],"ready":true}"#),
                0,
            ),
            // covers: app handover
            one(
                "--dry-run with every flag: blocked",
                &[
                    "handover",
                    "acme",
                    "--to",
                    "bob é",
                    "--db",
                    "share",
                    "--collisions",
                    "use_b",
                    "--reset-shared",
                    "--copy-source",
                    "--dry-run",
                ],
                ok(
                    r#"{"plan":{"lines":["x"],"blockers":[{"code":"db","hint":"wait"}]},"ready":false}"#,
                ),
                0,
            ),
            // covers: app handover --json
            one(
                "--dry-run --json",
                &["handover", "acme", "--to", "bob", "--dry-run", "--json"],
                ok(r#"{"ready":true}"#),
                0,
            ),
            // covers: app handover
            one(
                "the offer: every flag, a brief file",
                &[
                    "handover",
                    "acme",
                    "--to",
                    "bob",
                    "--brief",
                    "NOTES.md",
                    "--db",
                    "wait",
                    "--collisions",
                    "keep",
                    "--reset-shared",
                    "--copy-source",
                    "--reason",
                    "moving é",
                ],
                answer(202, OFFER),
                2,
            ),
            // covers: app handover
            one(
                "legacy: done at once",
                &["handover", "acme", "--to", "bob"],
                ok(r#"{"app":"acme","workspace":"bob"}"#),
                0,
            ),
            // covers: app handover --json
            one(
                "the offer --json",
                &["handover", "acme", "--to", "bob", "--json"],
                answer(202, OFFER),
                2,
            ),
            // covers: app handover
            in_session(case(
                "the offer in a session: the tip",
                &["handover", "acme", "--to", "bob"],
                &[],
                vec![answer(202, r#"{"handover":{"id":"h","state":"offered"}}"#)],
                files,
                2,
                1,
            )),
            // covers: app handover
            one(
                "refused",
                &["handover", "acme", "--to", "bob"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app handover
            one(
                "a confirm, no vault socket",
                &["handover", "acme", "--to", "bob"],
                answer(403, CONFIRM),
                3,
            ),
            // covers: app handover
            traceback(one(
                "a card with no id",
                &["handover", "acme", "--to", "bob"],
                answer(202, r#"{"request":{"x":1}}"#),
                1,
            )),
            // covers: app handover
            words("no --to: the page", &["handover", "acme"], 2),
            // covers: app handover
            words(
                "no brief file",
                &["handover", "acme", "--to", "b", "--brief", "nope.md"],
                2,
            ),
            // covers: app handover
            words("an unknown flag", &["handover", "acme", "--zap"], 2),
            // covers: app unpark
            one(
                "unpark",
                &["unpark", "acme", "--to", "bob"],
                answer(202, OFFER),
                2,
            ),
            // covers: app unpark
            words("unpark, no App: the page", &["unpark"], 2),
        ],
    );
}

#[test]
fn release_and_exception_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "release");
    check(
        &rig,
        vec![
            // covers: app release
            one(
                "release: done",
                &["release", "ws", "--apps", "uninstall", "--drop-access"],
                ok(
                    r#"{"workspace":"ws","way":"uninstall","results":[{"app":"acme"},{"app":"globex"}]}"#,
                ),
                0,
            ),
            // covers: app release
            one(
                "release: one",
                &["release", "ws", "--apps", "park"],
                ok(r#"{"workspace":"ws","way":"park","results":[{"app":"a"}]}"#),
                0,
            ),
            // covers: app release
            one(
                "release: held cards, no terminal",
                &["release", "ws", "--apps", "handover:bob"],
                answer(
                    202,
                    r#"{"results":[{"app":"a","request":"req-1"},{"app":"b"}],"hint":"approve each"}"#,
                ),
                2,
            ),
            // covers: app release --json
            one(
                "release --json: held",
                &["release", "ws", "--apps", "handover:bob", "--json"],
                answer(202, r#"{"results":[]}"#),
                2,
            ),
            // covers: app release
            one(
                "release refused",
                &["release", "ws", "--apps", "park"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app release
            traceback(one(
                "release: a row with no app",
                &["release", "ws", "--apps", "park"],
                ok(r#"{"results":[{}]}"#),
                1,
            )),
            // covers: app release
            words("release, no --apps", &["release", "ws"], 2),
            // covers: app release
            words(
                "release --drop-access, not uninstall",
                &["release", "ws", "--apps", "park", "--drop-access"],
                2,
            ),
            // covers: app exception
            one(
                "exception off",
                &["exception", "acme", "off"],
                ok(
                    r#"{"app":"acme","label":"guarded container","changed":true,"warning":"w","next":"n"}"#,
                ),
                0,
            ),
            // covers: app exception
            one(
                "exception off: no change, a reason",
                &["exception", "off", "off", "--reason", "back é"],
                ok(r#"{"app":"off","changed":false}"#),
                0,
            ),
            // covers: app exception
            one(
                "exception off --json",
                &["exception", "acme", "off", "--json"],
                ok(r#"{"app":"a"}"#),
                0,
            ),
            // covers: app exception
            one(
                "exception off refused",
                &["exception", "acme", "off"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app exception
            case(
                "exception on, no vault socket",
                &[
                    "exception",
                    "acme",
                    "on",
                    "--device",
                    "dri-render128",
                    "--device",
                    "x",
                ],
                &[],
                vec![],
                files,
                3,
                0,
            ),
            // covers: app exception
            words("exception, no mode", &["exception", "acme"], 2),
            // covers: app exception
            words(
                "exception --device last: set -e",
                &["exception", "acme", "on", "--device"],
                1,
            ),
        ],
    );
}

fn methods(m: &str) -> Reply {
    ok(&format!(
        r#"{{"methods":["{m}"],"confirmText":"the owner in person"}}"#
    ))
}

fn token(t: &str) -> Reply {
    ok(&format!(r#"{{"token":"{t}"}}"#))
}

fn confirming(name: &'static str, args: &[&'static str], daemon: Vec<Reply>, code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = VERB;
    c.daemon_requests = daemon.len();
    c.daemon = daemon;
    c.vault = Some(vec![methods("vault-password"), token("tk-own-1")]);
    c.vault_requests = 2;
    c.stdin = b"pw-own-1\n".to_vec();
    c.code = code;
    c.secrets = vec!["pw-own-1", "tk-own-1"];
    c
}

/// The owner confirms in person: the vault password typed, the token in
/// the body, no secret to a child.
#[test]
fn the_owner_confirms_in_person() {
    let rig = VRig::new();
    for c in [
        // covers: app uninstall
        confirming(
            "uninstall --purge",
            &["uninstall", "acme", "--purge", "--drop-access"],
            vec![ok("{}")],
            0,
        ),
        // covers: app exception
        confirming(
            "exception on, two devices",
            &[
                "exception",
                "acme",
                "on",
                "--device",
                "dri-render128",
                "--device",
                "kvm",
                "--reason",
                "gpu",
            ],
            vec![ok(
                r#"{"app":"acme","label":"process exception","changed":true}"#,
            )],
            0,
        ),
        // covers: app handover
        confirming(
            "handover: the confirm the daemon asks for",
            &["handover", "acme", "--to", "bob"],
            vec![answer(403, CONFIRM), answer(202, OFFER)],
            2,
        ),
        // covers: app release
        confirming(
            "release: the confirm the daemon asks for",
            &["release", "ws", "--apps", "handover:bob"],
            vec![
                answer(
                    403,
                    r#"{"error":{"code":"vault_confirm_required","action":"app-release","secretId":"ws"}}"#,
                ),
                ok(r#"{"workspace":"ws","way":"handover","results":[]}"#),
            ],
            0,
        ),
    ] {
        rig.check(&c);
        rig.no_secret_to_a_child(&c);
    }
}

/// Help never acts: a verb with `-h` or `--help` anywhere on the line
/// prints help and sends nothing.
#[test]
fn owner_help_never_acts() {
    let rig = Rig::new(VERB, "ownhelp");
    let rows: [&'static [&'static str]; 12] = [
        &["uninstall", "acme", "--purge", "-h"],
        &["uninstall", "--help", "acme"],
        &["expose", "acme", "--tunnel", "-h"],
        &["handover", "acme", "--to", "bob", "--help"],
        &["handover", "-h"],
        &["handover", "accept", "acme", "-h"],
        &["unpark", "acme", "--to", "bob", "-h"],
        &["park", "acme", "--help"],
        &["release", "ws", "--apps", "park", "-h"],
        &["exception", "acme", "on", "-h"],
        &["exception", "--help", "acme", "off"],
        &["handover", "status", "--help"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], files, 0, 0)
        });
    }
}

/// The lines name the command by the brand's name, never a typed `k2`
/// or `k5`.
#[test]
fn owner_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "ownbrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want, code) in [
        (
            &["park"][..],
            format!("usage: {cli} app park <app-id> [--reason TEXT]\n"),
            2,
        ),
        (
            &["release", "ws"],
            format!("usage: {cli} app release <workspace> --apps handover:<workspace>|uninstall|park [--drop-access]\n"),
            2,
        ),
        (
            &["exception", "acme"],
            format!("usage: {cli} app exception <app-id> on [--device dri-render128]... [--reason TEXT] | off\n"),
            2,
        ),
    ] {
        let c = case("brand", args, &[], vec![], files, code, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
