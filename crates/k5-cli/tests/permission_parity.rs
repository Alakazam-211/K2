//! `k5-cli permission ...` against the shell CLI's `cli/k2 permission ...`
//! (permission cards, Beta), on the parity rig ([`common::parity`]): the
//! same requests, stdout, stderr and exit code.
//!
//! - Whether the family exists: the owner's probe over TCP, the pages'
//!   probe with a passport (cell socket or TCP), `beta` with no probe;
//!   off (`"enabled": false`, a refusal, not JSON, no answer in time, no
//!   daemon) is the unknown command and the daily tier, exit 1.
//! - Every verb: `request` (and `ask`) with its words and every answer
//!   shape (asked, a dry run, already asked, already on, recently denied,
//!   each status's exit, the permission card's closing line, not JSON, a
//!   NUL, Python's raises), the `compute.grant` and `ci.preview` forms the
//!   daemon's "Ask with:" lines print; `update`, `status`, `cancel`,
//!   `list` (one answer above 256 KiB), `pending`, `approve` and `deny`
//!   (the lookup, the vault's confirm, not found), `beta`, `approvers`,
//!   `agent`; usage errors and `shift 2` with one word left.
//! - A person's verbs refused with a passport; a session with no passport
//!   (nothing sent as the owner); a healed passport; a POST on the cell
//!   socket that got no answer (never sent again); no answer over TCP.
//! - 0.46.17: `--assign` is the same flag as `--to` on `request` (the
//!   same body; its own name in the usage lines: last, empty, `=` empty).
//! - The bulletin line on success.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "permission";
const ON: &str = r#"{"enabled":true}"#;
const OFF: &str = r#"{"enabled":false}"#;

fn on() -> Reply {
    ok(ON)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// A line the front hands to the shell: both sides are the shell's.
fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn nul_in_answer(c: Case) -> Case {
    Case {
        compare: Compare::NulInAnswer,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn check(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

#[test]
fn the_family_exists_only_when_the_probe_says_so() {
    let rig = Rig::new(VERB, "probe");
    check(
        &rig,
        vec![
            case("on: the usage", &[], &[], vec![on()], owner_and_port, 0, 1),
            case("help", &["help"], &[], vec![on()], owner_and_port, 0, 1),
            // A help line is the family's too (its help form,
            // text::FAMILY_FORMS): the probe, then the usage.
            // native: permission --help
            case("--help", &["--help"], &[], vec![on()], owner_and_port, 0, 1),
            case("-h", &["-h"], &[], vec![on()], owner_and_port, 0, 1),
            case(
                "an unknown verb: the usage on stderr",
                &["nope", "x"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "off: an unknown command",
                &["request", "x", "--why", "w"],
                &[],
                vec![ok(OFF)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a refusal is off",
                &["status"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "not JSON is off",
                &["status"],
                &[],
                vec![ok("enabled")],
                owner_and_port,
                1,
                1,
            ),
            case(
                "enabled is not true",
                &["status"],
                &[],
                vec![ok(r#"{"enabled":1}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a 201 is off",
                &["status"],
                &[],
                vec![Reply::Answer(201, ON.into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "no answer in time is off",
                &["status"],
                &[],
                vec![Reply::Hang(Duration::from_secs(3))],
                owner_and_port,
                1,
                1,
            ),
            case(
                "an empty reply is off",
                &["status"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            ),
            case(
                "beta asks nothing first",
                &["beta", "status"],
                &[],
                vec![ok(r#"{"settings":{"permissionCards":true}}"#)],
                owner_and_port,
                0,
                1,
            ),
        ],
    );
    let port = dead_port();
    rig.check(&case(
        "no daemon: off",
        &["list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

#[test]
fn request_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "request");
    let soon = now() + 3 * 86_400 + 3600;
    let asked = format!(
        r#"{{"id":"pr_1","status":"pending","expiresAt":{soon},"routedTo":[{{"login":"ada","role":"owner"}},{{"name":"bo"}},{{}},"cy"],"addr":"k2/acme","text":"Use node-a for 2h é","duration":"2h"}}"#
    );
    let hours = format!(
        r#"{{"id":"pr_2","expiresAt":"{}","routedTo":"ab"}}"#,
        now() + 7200 + 600
    );
    check(
        &rig,
        vec![
            case(
                "the compute.grant form, as the daemon prints it",
                &[
                    "request",
                    "compute.grant",
                    "max-disk=120",
                    "max-parallel=2",
                    "max-queued=20",
                    "max-time=1h30m",
                    "node=node-a",
                    "--why",
                    "<what you need it for>",
                ],
                &[],
                vec![on(), ok(&asked)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "the ci.preview form",
                &["request", "ci.preview", "--why", "<what you need it for>"],
                &[],
                vec![on(), ok(&hours)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "ask, --why=, --to, keys twice",
                &[
                    "ask", "--why=a \"quoted\" é why", "x.y", "a=1", "b=", "a=2", "=v", "-k=v",
                    "--to", "ada,owner",
                ],
                &[],
                vec![on(), ok(r#"{"id":"pr_3","status":"pending"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "--assign, the same body as --to",
                &["request", "x", "--assign", "ada,owner", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"id":"pr_3","status":"pending"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "--assign=, then --to: the last one stands",
                &["request", "x", "--assign=bo", "--to", "ada", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"id":"pr_3","status":"pending"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "--to=",
                &["request", "x", "--to=ada", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"id":"pr_4","expiresAt":5}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "a dry run",
                &["request", "x", "--dry-run", "--why", "w"],
                &[],
                vec![
                    on(),
                    ok(r#"{"dryRun":true,"text":"Let it run","consequences":"It runs","approvers":[{"login":"ada"}],"expiresIn":"24 hours"}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "text and no id is a dry run, nobody yet",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"text":"","routedTo":[]}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "already pending",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"code":"already_pending","id":"pr_9"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "already done",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"status":"already_done"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "--json",
                &["request", "x", "--why", "w", "--json"],
                &[],
                vec![on(), ok("{\"id\":\"pr_1\"}\n\n")],
                owner_and_port,
                0,
                2,
            ),
            case(
                "--json and a refusal",
                &["request", "x", "--why", "w", "--json"],
                &[],
                vec![on(), Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                3,
                2,
            ),
            case(
                "recently denied",
                &["request", "x", "--why", "w"],
                &[],
                vec![
                    on(),
                    Reply::Answer(
                        409,
                        r#"{"error":{"code":"recently_denied"},"denied":{"by":"ada","at":1700000000,"reason":"not now","askAgainAt":"1700086400","id":"pr_7"}}"#.into(),
                    ),
                ],
                owner_and_port,
                3,
                2,
            ),
            case(
                "recently denied, from the error",
                &["request", "x", "--why", "w"],
                &[],
                vec![
                    on(),
                    Reply::Answer(
                        409,
                        r#"{"error":{"code":"recently_denied","at":"soon"},"id":"pr_8"}"#.into(),
                    ),
                ],
                owner_and_port,
                3,
                2,
            ),
            case(
                "each status's exit: 400",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(400, r#"{"error":{"code":"bad_why","message":"too long"}}"#.into())],
                owner_and_port,
                2,
                2,
            ),
            case(
                "413",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(413, r#"{"error":"too_big","hint":"smaller"}"#.into())],
                owner_and_port,
                2,
                2,
            ),
            case(
                "404",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(404, r#"{"error":5,"message":"gone"}"#.into())],
                owner_and_port,
                4,
                2,
            ),
            case(
                "429",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(429, r#"{"hint":0}"#.into())],
                owner_and_port,
                3,
                2,
            ),
            case(
                "500, not JSON",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(500, format!("  oops {}  ", "x".repeat(400)))],
                owner_and_port,
                1,
                2,
            ),
            case(
                "302",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(302, "{}".into())],
                owner_and_port,
                1,
                2,
            ),
            case(
                "a 502 with no body",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Answer(502, String::new())],
                owner_and_port,
                1,
                2,
            ),
            case(
                "a 200 that is not an object",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok("[1,2]")],
                owner_and_port,
                0,
                2,
            ),
            case(
                "the bulletin on success",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok(BULLETIN)],
                owner_and_port,
                0,
                2,
            ),
        ],
    );
    check(
        &rig,
        vec![
            traceback(case(
                "routedTo that is no list",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"id":"x","routedTo":5}"#)],
                owner_and_port,
                0,
                2,
            )),
            traceback(case(
                "a login that is no string",
                &["request", "x", "--dry-run", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"dryRun":1,"routedTo":[{"login":7}]}"#)],
                owner_and_port,
                0,
                2,
            )),
            traceback(case(
                "an addr that is no string",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok(r#"{"id":"x","addr":[1]}"#)],
                owner_and_port,
                0,
                2,
            )),
            traceback(case(
                "denied that is no object",
                &["request", "x", "--why", "w"],
                &[],
                vec![
                    on(),
                    Reply::Answer(409, r#"{"error":"recently_denied","denied":"yes"}"#.into()),
                ],
                owner_and_port,
                3,
                2,
            )),
            nul_in_answer(case(
                "a NUL in the answer",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), ok("{\"id\":\"a\u{0}b\"}")],
                owner_and_port,
                0,
                2,
            )),
        ],
    );
}

#[test]
fn request_s_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let one = |name: &'static str, args: &'static [&'static str], code: i32| {
        case(name, args, &[], vec![on()], owner_and_port, code, 1)
    };
    check(
        &rig,
        vec![
            one("no action", &["request", "--why", "w"], 2),
            one("no why", &["request", "x"], 2),
            one("an empty why", &["request", "x", "--why", ""], 2),
            one("--why last", &["request", "x", "--why"], 2),
            one("--to last", &["request", "x", "--to"], 2),
            one("--to empty", &["request", "x", "--to", "", "--why", "w"], 2),
            one("--to= empty", &["request", "x", "--to=", "--why", "w"], 2),
            one("--assign last", &["request", "x", "--assign"], 2),
            one(
                "--assign empty",
                &["request", "x", "--assign", "", "--why", "w"],
                2,
            ),
            one(
                "--assign= empty",
                &["request", "x", "--assign=", "--why", "w"],
                2,
            ),
            one("an unknown flag", &["request", "x", "--now"], 2),
            one("--dry-run=1 is a flag", &["request", "x", "--dry-run=1"], 2),
            one("two actions", &["request", "x", "y", "--why", "w"], 2),
            shell(one("--help after words", &["request", "x", "--help"], 0)),
            // A help form: the family's (text::FAMILY_FORMS).
            one("-h first", &["request", "-h"], 0),
        ],
    );
}

#[test]
fn the_other_agent_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "agentverbs");
    let rows: String = (0..6000)
        .map(|i| {
            format!(
                r#"{{"action":"compute.grant","area":"compute","label":"Row {i} é","asks":{}}}"#,
                i % 7
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let big = format!(r#"{{"actions":[{rows}]}}"#);
    assert!(big.len() > 256 * 1024);
    check(
        &rig,
        vec![
            case(
                "update",
                &["update", "pr_1", "--why", "more facts é"],
                &[],
                vec![on(), ok(r#"{"message":"Updated."}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "update with no why",
                &["update", "pr_1"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "update: --why last",
                &["update", "pr_1", "--why"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "update: another word",
                &["update", "pr_1", "--reason", "x"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "update: an empty id",
                &["update", "", "--why", "x"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "update: the hint, then OK.",
                &["update", "pr_1", "--why", "x"],
                &[],
                vec![on(), ok(r#"{"message":"","hint":0}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "status, an id that needs escaping",
                &["status", "a b/é&c"],
                &[],
                vec![
                    on(),
                    ok(r#"{"requests":[{"id":"a","status":"pending","n":1.5e16}]}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "status, all",
                &["status"],
                &[],
                vec![on(), ok(r#"{"requests":[]}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "cancel",
                &["cancel", "pr_1"],
                &[],
                vec![on(), ok(r#"{}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "cancel with no id",
                &["cancel"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "list --area --search",
                &["list", "--search", "a b", "--area", "compute"],
                &[],
                vec![
                    on(),
                    ok(r#"{"actions":[{"action":"x","nested":{"k":[1,null,true]}}]}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "list --search only",
                &["list", "--search", "é"],
                &[],
                vec![on(), ok(r#"{"actions":[]}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "list: --area last",
                &["list", "--area"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "list: another word",
                &["list", "compute"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "list: above 256 KiB",
                &["list"],
                &[],
                vec![on(), ok(&big)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "a permission card on a refusal",
                &["cancel", "pr_1"],
                &[],
                vec![on(), Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                3,
                2,
            ),
        ],
    );
}

#[test]
fn the_people_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "people");
    const PENDING: &str = r#"{"requests":[{"requestId":"pr_1","cardId":"c_1","addr":"k2/acme"},{"requestId":"pr_2","cardId":"c_2","addr":"k2/acme","proof":{"kind":"confirm"}}]}"#;
    check(
        &rig,
        vec![
            case(
                "pending",
                &["pending"],
                &[],
                vec![on(), ok(PENDING)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "approve",
                &["approve", "pr_1"],
                &[],
                vec![on(), ok(PENDING), ok(r#"{"message":"Allowed."}"#)],
                owner_and_port,
                0,
                3,
            ),
            case(
                "deny with a reason",
                &["deny", "pr_2", "--reason", "not now é"],
                &[],
                vec![on(), ok(PENDING), ok(r#"{}"#)],
                owner_and_port,
                0,
                3,
            ),
            case(
                "approve: the vault's confirm",
                &["approve", "pr_2"],
                &[],
                vec![on(), ok(PENDING)],
                owner_and_port,
                3,
                2,
            ),
            case(
                "approve: no such request",
                &["approve", "pr_9"],
                &[],
                vec![on(), ok(PENDING)],
                owner_and_port,
                4,
                2,
            ),
            case(
                "approve: the lookup is not JSON",
                &["approve", "pr_1"],
                &[],
                vec![on(), ok("nope")],
                owner_and_port,
                4,
                2,
            ),
            case(
                "approve: a request that is no object",
                &["approve", "pr_1"],
                &[],
                vec![on(), ok(r#"{"requests":"pr_1"}"#)],
                owner_and_port,
                4,
                2,
            ),
            case(
                "approve: the answer refused",
                &["approve", "pr_1"],
                &[],
                vec![
                    on(),
                    ok(PENDING),
                    Reply::Answer(403, r#"{"error":{"code":"owner_only"}}"#.into()),
                ],
                owner_and_port,
                3,
                3,
            ),
            case(
                "approve with no id",
                &["approve"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "deny: --reason last",
                &["deny", "pr_1", "--reason"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "beta on",
                &["beta", "on"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "beta off",
                &["beta", "off", "extra"],
                &[],
                vec![ok(r#"{"message":"Off."}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "beta alone is status, off",
                &["beta"],
                &[],
                vec![ok(r#"{"permissionCards":"yes"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "beta status: a refusal is read as settings",
                &["beta", "status"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                owner_and_port,
                0,
                1,
            ),
            case(
                "beta status: not JSON",
                &["beta", "status"],
                &[],
                vec![ok("")],
                owner_and_port,
                1,
                1,
            ),
            case(
                "beta status: no answer",
                &["beta", "status"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            ),
            case(
                "beta maybe",
                &["beta", "maybe"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
            case(
                "approvers",
                &["approvers"],
                &[],
                vec![
                    on(),
                    ok(r#"{"settings":{"permissionApprovers":["ada","owner"]}}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "approvers: everyone",
                &["approvers"],
                &[],
                vec![on(), ok(r#"{"settings":{"permissionApprovers":[]}}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "approvers: a string is its letters",
                &["approvers"],
                &[],
                vec![on(), ok(r#"{"permissionApprovers":"ab"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "approvers set",
                &["approvers", " ada, ,owner "],
                &[],
                vec![on(), ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "approvers set everyone",
                &["approvers", "EveryOne"],
                &[],
                vec![on(), ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "approvers: two words",
                &["approvers", "ada", "owner"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
            case(
                "agent on",
                &["agent", "acme", "on"],
                &[],
                vec![on(), ok(r#"{"message":"On for acme."}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "agent off",
                &["agent", "acme", "off"],
                &[],
                vec![on(), ok(r#"{}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "agent maybe",
                &["agent", "acme"],
                &[],
                vec![on()],
                owner_and_port,
                2,
                1,
            ),
        ],
    );
    check(
        &rig,
        vec![
            traceback(case(
                "beta status: settings that are no object",
                &["beta", "status"],
                &[],
                vec![ok(r#"{"settings":5}"#)],
                owner_and_port,
                1,
                1,
            )),
            traceback(case(
                "approvers: names that are no list",
                &["approvers"],
                &[],
                vec![on(), ok(r#"{"permissionApprovers":5}"#)],
                owner_and_port,
                1,
                2,
            )),
            traceback(case(
                "approvers: a name that is no string",
                &["approvers"],
                &[],
                vec![on(), ok(r#"{"permissionApprovers":["a",1]}"#)],
                owner_and_port,
                1,
                2,
            )),
        ],
    );
}

#[test]
fn in_a_session() {
    let rig = Rig::new(VERB, "session");
    let h = with_passport;
    check(
        &rig,
        vec![
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "the cell socket: probe and request",
                    &["request", "x", "--why", "w"],
                    &[],
                    vec![on(), ok(r#"{"id":"pr_1"}"#)],
                    port_only,
                    0,
                    2,
                ),
            ),
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a person's verb is refused",
                    &["pending"],
                    &[],
                    vec![on()],
                    port_only,
                    3,
                    1,
                ),
            ),
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "beta asks first with a passport, then is refused",
                    &["beta", "on"],
                    &[],
                    vec![on()],
                    port_only,
                    3,
                    1,
                ),
            ),
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "off in a session: the daily tier asks about reports too",
                    &["status"],
                    &[],
                    vec![ok(OFF), ok(r#"{"agents":false,"senderKind":"agent"}"#)],
                    port_only,
                    1,
                    2,
                ),
            ),
            with(
                h(&[]),
                case(
                    "the passport over TCP",
                    &["status", "pr_1"],
                    &[],
                    vec![on(), ok(r#"{"status":"pending"}"#)],
                    port_only,
                    0,
                    2,
                ),
            ),
            with(
                h(&[]),
                case(
                    "a refused passport heals once",
                    &["status"],
                    &[],
                    vec![on(), heal(&rig), ok(r#"{"status":"pending"}"#)],
                    port_only,
                    0,
                    3,
                ),
            ),
            case(
                "HOOK_SOCK and no passport: the owner asks, nothing is sent",
                &["status"],
                &[("K2_HOOK_SOCK", "{sock}")],
                vec![on()],
                owner_and_port,
                1,
                1,
            ),
            case(
                "HOOK_SOCK and no passport: beta sends nothing",
                &["beta", "on"],
                &[("K2_HOOK_SOCK", "{sock}")],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ],
    );
    rig.check(&with(
        h(&[("K2_HOOK_SOCK", "{sock}")]),
        waited(case(
            "a POST on the cell socket that got no answer is not sent again",
            &["request", "x", "--why", "w"],
            &[],
            vec![on(), Reply::Close],
            port_only,
            1,
            2,
        )),
    ));
}

#[test]
fn with_no_answer() {
    let rig = Rig::new(VERB, "noanswer");
    check(
        &rig,
        vec![
            case(
                "an empty reply",
                &["request", "x", "--why", "w"],
                &[],
                vec![on(), Reply::Close],
                owner_and_port,
                1,
                2,
            ),
            case(
                "a reset",
                &["cancel", "pr_1"],
                &[],
                vec![on(), Reply::Reset],
                owner_and_port,
                1,
                2,
            ),
            case(
                "not HTTP",
                &["status"],
                &[],
                vec![on(), Reply::Raw(GARBAGE.to_vec())],
                owner_and_port,
                1,
                2,
            ),
            case(
                "the lookup got no answer",
                &["approve", "pr_1"],
                &[],
                vec![on(), Reply::Close],
                owner_and_port,
                4,
                2,
            ),
            case(
                "approvers got no answer",
                &["approvers"],
                &[],
                vec![on(), Reply::Close],
                owner_and_port,
                1,
                2,
            ),
        ],
    );
    let port = dead_port();
    rig.check(&case(
        "beta on with no daemon",
        &["beta", "on"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn permission_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "a port that is no port: the shell's port refusal",
            &["status"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        )
    });
}

/// Every `-h` and `--help` line of the family (its help forms,
/// `text::FAMILY_FORMS`) is the family's code, as `cmd_permission` runs
/// it: the probe first (the owner token over TCP for the owner, none for
/// the owner's `beta`; the passport's probe for an agent), off is the
/// unknown command and the daily tier, and on, help wins
/// (`_permission_cmd`'s `_wants_help`): the usage, exit 0, and nothing
/// sent but the probe, whatever the verb (a person's verb with a
/// passport too).
#[test]
fn the_help_lines_are_the_family_s() {
    let rig = Rig::new(VERB, "help");
    let h = with_passport;
    let mut cases = Vec::new();
    for flag in ["--help", "-h"] {
        // Each line twice, once a flag: the args are &'static.
        let line = |a: &'static [&'static str], b: &'static [&'static str]| {
            if flag == "--help" {
                a
            } else {
                b
            }
        };
        // Each row's name once in the rig.
        let n =
            |name: &str| -> &'static str { Box::leak(format!("{name} ({flag})").into_boxed_str()) };
        // The usage after the probe: one request.
        let usage = |name: &str, a: &'static [&'static str], b: &'static [&'static str]| {
            case(n(name), line(a, b), &[], vec![on()], owner_and_port, 0, 1)
        };
        // The owner's `beta`: no probe, the usage, nothing sent.
        let beta = |name: &str, a: &'static [&'static str], b: &'static [&'static str]| {
            case(n(name), line(a, b), &[], vec![], owner_and_port, 0, 0)
        };
        cases.extend([
            // native: permission request --help
            usage(
                "request: the usage, the probe only",
                &["request", "--help"],
                &["request", "-h"],
            ),
            // native: permission ask --help
            usage(
                "ask: the usage, the probe only",
                &["ask", "--help"],
                &["ask", "-h"],
            ),
            // native: permission update --help
            usage(
                "update: the usage, the probe only",
                &["update", "--help"],
                &["update", "-h"],
            ),
            // native: permission status --help
            usage(
                "status: the usage, the probe only",
                &["status", "--help"],
                &["status", "-h"],
            ),
            // native: permission cancel --help
            usage(
                "cancel: the usage, the probe only",
                &["cancel", "--help"],
                &["cancel", "-h"],
            ),
            // native: permission list --help
            usage(
                "list: the usage, the probe only",
                &["list", "--help"],
                &["list", "-h"],
            ),
            // native: permission pending --help
            usage(
                "pending: the usage, the probe only",
                &["pending", "--help"],
                &["pending", "-h"],
            ),
            // native: permission approve --help
            usage(
                "approve: the usage, the probe only",
                &["approve", "--help"],
                &["approve", "-h"],
            ),
            // native: permission deny --help
            usage(
                "deny: the usage, the probe only",
                &["deny", "--help"],
                &["deny", "-h"],
            ),
            // native: permission beta --help
            beta(
                "beta: no probe, the usage",
                &["beta", "--help"],
                &["beta", "-h"],
            ),
            // native: permission beta on --help
            beta(
                "beta on: no probe, the usage",
                &["beta", "on", "--help"],
                &["beta", "on", "-h"],
            ),
            // native: permission beta off --help
            beta(
                "beta off: no probe, the usage",
                &["beta", "off", "--help"],
                &["beta", "off", "-h"],
            ),
            // native: permission beta status --help
            beta(
                "beta status: no probe, the usage",
                &["beta", "status", "--help"],
                &["beta", "status", "-h"],
            ),
            // native: permission approvers --help
            usage(
                "approvers: the usage, the probe only",
                &["approvers", "--help"],
                &["approvers", "-h"],
            ),
            // native: permission agent --help
            usage(
                "agent: the usage, the probe only",
                &["agent", "--help"],
                &["agent", "-h"],
            ),
            // Off: the unknown command and the daily tier.
            case(
                n("off"),
                line(&["status", "--help"], &["status", "-h"]),
                &[],
                vec![ok(OFF)],
                owner_and_port,
                1,
                1,
            ),
            case(
                n("off, the usage line"),
                line(&["--help"], &["-h"]),
                &[],
                vec![ok(OFF)],
                owner_and_port,
                1,
                1,
            ),
            // No daemon: off.
            case(
                n("no daemon"),
                line(&["--help"], &["-h"]),
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            ),
            // An agent: the passport's probe on the cell socket; on, the
            // usage, or a person's verb refused; off, the daily tier asks
            // about reports too.
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    n("agent, on"),
                    line(&["--help"], &["-h"]),
                    &[],
                    vec![on()],
                    port_only,
                    0,
                    1,
                ),
            ),
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    n("agent, a person's verb: the usage, the probe only"),
                    line(&["approvers", "--help"], &["approvers", "-h"]),
                    &[],
                    vec![on()],
                    port_only,
                    0,
                    1,
                ),
            ),
            with(
                h(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    n("agent, off"),
                    line(&["request", "--help"], &["request", "-h"]),
                    &[],
                    vec![ok(OFF), ok(r#"{"agents":true,"senderKind":"agent"}"#)],
                    port_only,
                    1,
                    2,
                ),
            ),
            // An agent over TCP.
            with(
                h(&[]),
                case(
                    n("agent over TCP, on"),
                    line(&["--help"], &["-h"]),
                    &[],
                    vec![on()],
                    port_only,
                    0,
                    1,
                ),
            ),
        ]);
    }
    check(&rig, cases);
}

/// Help later on the line, or as a flag's value, wins too: the usage,
/// exit 0, and nothing sent but the probe (before the fix, `--why` and
/// `--area` took `-h` as their value and `agent <ws> on --help` sent the
/// switch). The front hands these lines to the shell on both sides (no
/// help form names them); [`the_help_lines_are_the_family_s`] and the
/// family's unit test cover the binary's own reading.
#[test]
fn help_later_on_the_line_is_the_usage() {
    let rig = Rig::new(VERB, "helplate");
    let usage = |name: &'static str, args: &'static [&'static str]| {
        shell(case(name, args, &[], vec![on()], owner_and_port, 0, 1))
    };
    check(
        &rig,
        vec![
            usage("update --why -h", &["update", "--why", "-h"]),
            usage("update <id> --why -h", &["update", "pr_1", "--why", "-h"]),
            usage("list --area -h", &["list", "--area", "-h"]),
            usage("list --search --help", &["list", "--search", "--help"]),
            usage("request --why -h", &["request", "--why", "-h"]),
            usage("agent <ws> on --help", &["agent", "acme", "on", "--help"]),
            usage("agent <ws> off -h", &["agent", "acme", "off", "-h"]),
            usage("deny <id> --reason -h", &["deny", "pr_1", "--reason", "-h"]),
            usage("status <id> --help", &["status", "pr_1", "--help"]),
            usage("approvers ada -h", &["approvers", "ada", "-h"]),
            shell(case(
                "beta off --help: no probe",
                &["beta", "off", "--help"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )),
        ],
    );
}

/// 0.46.17: the usage's `--assign` line names the brand's command.
#[test]
fn the_assign_line_says_the_brand_s_name() {
    let rig = Rig::new(VERB, "assignbrand");
    let cli = k2_client::brand::CLI_NAME;
    let c = case(
        "brand: the usage",
        &[],
        &[],
        vec![on()],
        owner_and_port,
        0,
        1,
    );
    let (ran, _) = rig.binary(&c, false);
    assert!(
        ran.stdout
            .contains(&format!("(--assign is the same flag, as on {cli} app)\n")),
        "{}",
        ran.stdout
    );
    assert_eq!(ran.code, Some(0));
    let c = case(
        "brand: --assign= empty",
        &["request", "x", "--assign=", "--why", "w"],
        &[],
        vec![on()],
        owner_and_port,
        2,
        1,
    );
    let (ran, _) = rig.binary(&c, false);
    assert_eq!(
        ran.stderr,
        "Error: --assign needs one or more logins, comma-separated (--assign ada,owner)\n"
    );
    assert_eq!(ran.code, Some(2));
}
