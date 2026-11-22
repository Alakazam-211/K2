//! `k5-cli app user rooms-ok` and `skin user rooms-ok` (a test login into
//! the rooms its role names, on an owner's card) against the shell CLI's
//! `_app_test_guest_rooms_ok` (`cli/src/app-updates.sh`, through
//! `cmd_skin` in `cli/src/app-wallet.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The card asked (exit 2 while it waits), opened and already open
//!   (exit 0), `--reason`, `--json` (its exit too), `users`.
//! - `--assign` from `app`: the pre-scan takes it, anywhere before `--`,
//!   and the POST carries `?assign=`; its usage lines. From `skin` there
//!   is no pre-scan: an unknown flag.
//! - The words: usage lines, a value flag as the last word, an unknown
//!   flag, `cmd_skin`'s `--role` check first.
//! - A refusal (with the registry's fix from `app`, none from `skin`), an
//!   answer above 256 KiB, Python raising part way, nothing listening.
//! - Help never acts: `-h` or `--help` anywhere prints help and sends
//!   nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const ASKED: &str = r#"{"ok":true,"status":"asked","username":"tess","role":"tester","rooms":["sales","ops é"],"card":{"id":"pr_1","card":"c1","thread":"ops","routedTo":["ada"]},"where":"a permission card pr_1 is on ops's Thread for ada"}"#;
const OPENED: &str = r#"{"ok":true,"status":"opened","username":"owned","rooms":["sales"],"note":"owned can now use sales"}"#;
const OPENED_BARE: &str = r#"{"status":"opened","username":"owned","rooms":["sales","b"]}"#;
const ALREADY: &str = r#"{"status":"already_open","username":"tess","rooms":["sales"]}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

/// One answer, the owner on disk.
fn one(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Case {
    case(name, args, &[], vec![reply], owner_and_port, code, 1)
}

/// Words that stop before anything is sent.
fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn refused() -> Reply {
    Reply::Answer(
        403,
        r#"{"error":{"code":"owner_only","hint":"the owner's"}}"#.into(),
    )
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn app_user_rooms_ok_as_the_shell_does() {
    let rig = Rig::new("app", "roomsok");
    let port = dead_port();
    let big = {
        let rooms: Vec<String> = (0..20000).map(|i| format!("\"room-{i:05} é\"")).collect();
        format!(
            r#"{{"status":"asked","username":"tess","role":"tester","rooms":[{}],"card":{{"id":"pr_2"}},"where":"on ops's Thread"}}"#,
            rooms.join(",")
        )
    };
    assert!(big.len() > 256 * 1024, "the answer is {} bytes", big.len());
    check(
        &rig,
        vec![
            // native: app user rooms-ok
            one(
                "rooms-ok --assign: asked, exit 2",
                &["user", "rooms-ok", "qa1", "tess", "--assign", "ada"],
                ok(ASKED),
                2,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: --assign= a comma list, before the verb",
                &["user", "--assign=ada,owner é", "rooms-ok", "qa1", "tess"],
                ok(ASKED),
                2,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: the last --assign stands, a reason",
                &[
                    "users",
                    "rooms-ok",
                    "--assign",
                    "ada",
                    "qa1",
                    "--reason",
                    "QA of sales é",
                    "tess",
                    "--assign=bo",
                ],
                ok(ASKED),
                2,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: opened, the note",
                &["user", "rooms-ok", "qa1", "owned", "--reason="],
                ok(OPENED),
                0,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: opened, no note",
                &["user", "rooms-ok", "qa1", "owned", "--reason=r"],
                ok(OPENED_BARE),
                0,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: already open",
                &["user", "rooms-ok", "", "qa1", "tess"],
                ok(ALREADY),
                0,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: asked, no card id",
                &["user", "rooms-ok", "qa1", "tess"],
                ok(r#"{"status":"asked","username":"tess","card":{},"where":"x"}"#),
                2,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: an answer above 256 KiB",
                &["user", "rooms-ok", "qa1", "tess"],
                ok(&big),
                2,
            ),
            // native: app user rooms-ok --json
            one(
                "rooms-ok --json: asked, exit 2",
                &[
                    "user", "rooms-ok", "qa1", "tess", "--json", "--assign", "ada",
                ],
                ok(ASKED),
                2,
            ),
            // native: app user rooms-ok --json
            one(
                "rooms-ok --json: opened",
                &["user", "rooms-ok", "--json", "qa1", "owned"],
                ok(OPENED),
                0,
            ),
            // native: app user rooms-ok
            one(
                "rooms-ok: a refusal, with its fix",
                &["user", "rooms-ok", "qa1", "tess", "--assign", "ada"],
                refused(),
                3,
            ),
            // native: app user rooms-ok --json
            one(
                "rooms-ok: the --assign value is --json, a refusal as JSON",
                &["user", "rooms-ok", "qa1", "tess", "--assign", "--json"],
                refused(),
                3,
            ),
            // native: app user rooms-ok
            traceback(one(
                "rooms-ok: an answer that is a list",
                &["user", "rooms-ok", "qa1", "tess"],
                ok("[1]"),
                1,
            )),
            // native: app user rooms-ok --json
            traceback(one(
                "rooms-ok --json: no JSON, the exit's Python raises",
                &["user", "rooms-ok", "qa1", "tess", "--json"],
                ok("not json"),
                1,
            )),
            // native: app user rooms-ok
            traceback(one(
                "rooms-ok: a card that is a list, after the first line",
                &["user", "rooms-ok", "qa1", "tess"],
                ok(r#"{"status":"asked","username":"tess","rooms":["a"],"card":[1]}"#),
                1,
            )),
            // native: app user rooms-ok
            traceback(one(
                "rooms-ok: a room that is no string",
                &["user", "rooms-ok", "qa1", "tess"],
                ok(r#"{"status":"already_open","username":"tess","rooms":[1]}"#),
                1,
            )),
            // native: app user rooms-ok
            words("rooms-ok: no login", &["user", "rooms-ok", "qa1"], 2),
            // native: app user rooms-ok
            words("rooms-ok: nothing", &["user", "rooms-ok"], 2),
            // native: app user rooms-ok
            words(
                "rooms-ok: a third word",
                &["user", "rooms-ok", "qa1", "tess", "x"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: --reason given last",
                &["user", "rooms-ok", "qa1", "tess", "--reason"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: an unknown flag",
                &["user", "rooms-ok", "qa1", "tess", "-q"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: --test is not its flag",
                &["user", "rooms-ok", "qa1", "tess", "--test", "--role", "r"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: cmd_skin's --role check first",
                &["user", "rooms-ok", "qa1", "tess", "--role", "r"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: --assign given last",
                &["user", "rooms-ok", "qa1", "tess", "--assign"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: --assign= with none",
                &["user", "rooms-ok", "--assign=", "qa1", "tess"],
                2,
            ),
            // native: app user rooms-ok
            words(
                "rooms-ok: --assign's usage before the verb's own",
                &["user", "rooms-ok", "-q", "--assign", ""],
                2,
            ),
            // native: app user rooms-ok
            case(
                "rooms-ok: nothing listens (7)",
                &["user", "rooms-ok", "qa1", "tess"],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                3,
                0,
            ),
            // native: app user rooms-ok
            case(
                "rooms-ok: no daemon, the gate",
                &["user", "rooms-ok", "qa1", "tess"],
                &[],
                vec![],
                nothing,
                1,
                0,
            ),
            // native: app user rooms-ok --help
            shell(words(
                "rooms-ok: --help alone",
                &["user", "rooms-ok", "--help"],
                0,
            )),
            shell(words(
                "rooms-ok -h after the words",
                &["user", "rooms-ok", "qa1", "tess", "--assign", "ada", "-h"],
                0,
            )),
            shell(words(
                "rooms-ok: --assign -h is help",
                &["user", "rooms-ok", "qa1", "tess", "--assign", "-h"],
                0,
            )),
        ],
    );
}

#[test]
fn skin_user_rooms_ok_as_the_shell_does() {
    let rig = Rig::new("skin", "skinroomsok");
    check(
        &rig,
        vec![
            // native: skin user rooms-ok
            one(
                "skin rooms-ok: asked, exit 2",
                &["user", "rooms-ok", "qa1", "tess", "--reason", "r"],
                ok(ASKED),
                2,
            ),
            // native: skin user rooms-ok
            one(
                "skin rooms-ok: opened",
                &["users", "rooms-ok", "qa1", "owned"],
                ok(OPENED),
                0,
            ),
            // native: skin user rooms-ok --json
            one(
                "skin rooms-ok --json",
                &["user", "rooms-ok", "qa1", "tess", "--json"],
                ok(ASKED),
                2,
            ),
            // From `skin` the refusal carries no registry fix: cmd_skin
            // leaves _K2_REFUSAL_AREA empty.
            // native: skin user rooms-ok
            one(
                "skin rooms-ok: a refusal",
                &["user", "rooms-ok", "qa1", "tess"],
                refused(),
                3,
            ),
            // native: skin user rooms-ok
            words(
                "skin rooms-ok: --assign is an unknown flag here",
                &["user", "rooms-ok", "qa1", "tess", "--assign", "ada"],
                2,
            ),
            // native: skin user rooms-ok
            words("skin rooms-ok: no login", &["user", "rooms-ok", "qa1"], 2),
            // native: skin user rooms-ok --help
            shell(words(
                "skin rooms-ok --help",
                &["user", "rooms-ok", "qa1", "--help"],
                0,
            )),
        ],
    );
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn rooms_ok_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let usage = format!(
        "usage: {cli} app user rooms-ok <app-id> <test-login> [--assign <login>] [--reason TEXT] [--json]"
    );
    let rig = Rig::new("app", "roomsokbrand");
    for (args, want) in [
        (&["user", "rooms-ok", "qa1"][..], format!("{usage}\n")),
        (
            &["user", "rooms-ok", "qa1", "tess", "-q"],
            format!("Unknown flag: -q ({usage})\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
    let rig = Rig::new("skin", "skinroomsokbrand");
    let c = case(
        "brand",
        &["user", "rooms-ok", "qa1", "tess", "--assign", "ada"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "{cli} skin is now {cli} app \u{2014} this command still works.\nUnknown flag: --assign ({usage})\n"
        )
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
}
