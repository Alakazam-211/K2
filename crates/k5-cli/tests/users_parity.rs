//! `k5-cli users` against the shell CLI's `cli/k2 users`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for every verb of `cmd_users` and the phone verbs.
//!
//! - Each verb: `list` (bare, `--json`, none, a refusal as the owner and
//!   as an agent, answers it can't read, one above 256 KiB), `add` (each
//!   role, the shares, a password on stdin, the set-role after an owner or
//!   admin, its words), `set-role`, `share` (show and set), `set-password`,
//!   `audit`, `set-can-clone`, `unlock`, `pair-phone` (the QR, a login's
//!   password again), `phones` and `unpair-phone`; `help`; an unknown verb.
//! - The gates and the locked tool's session checks.
//! - No answer: an empty reply and nothing listening.
//! - Help with `-h` or `--help` on a verb that changes something: the
//!   shell answers it (the router prints the help page), and it sends
//!   nothing.
//!
//! The two password questions read a terminal; they are not driven here.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "users";

const USERS: &str = r#"{"users":[{"username":"ann","role":"owner","createdAt":"2026-01-02"},{"username":"bo","firstName":"Bo","displayName":"Bo Brown","mustChangePassword":true,"disabled":true,"wasViewer":true,"canClone":false},{"username":"vc","role":"voice_chat","created_at":"2026-03-04","shares":{"agents":[{"address":"a@w"},{"id":"ag2"}],"gardens":[{"name":"G"}],"sources":[{"name":"s1","missing":true}]}},"skip me"],"doctor":["login 'k2' acts as nobody"]}"#;

const PHONES: &str = r#"{"pairAs":{"user":"ann"},"devices":[{"id":"d1","username":"ann","name":"Pixel\u0007 9","current":true,"createdAt":"2026-11-01T10:00:00.123Z","expiresAt":"2026-12-01T10:00:00Z"},{"id":"d2","username":"bo","name":"iPhone","pairedBy":"ann","createdAt":"2026-11-02","expiresAt":"2027"}]}"#;

const PAIR: &str = r#"{"user":"ann","code":"123-456","pairUrl":"https://x.example/p#c","expiresInSecs":300,"expiresAt":"2026-11-20T10:05:00Z","qr":{"rows":["1110","1001","1011","0001"]}}"#;

const AGAIN: &str = r#"{"error":{"code":"password_required","hint":"Your login: type your password again to make a pairing code."}}"#;

fn reply(body: &str) -> Reply {
    ok(body)
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

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

/// Logins enough that the answer is above 256 KiB.
fn many_users() -> String {
    let rows: Vec<String> = (0..2500)
        .map(|i| {
            format!(
                r#"{{"username":"user-{:05}-{}","role":"{}","firstName":"F{i}","displayName":"Name {}","createdAt":"2026-11-{:02}T00:00:00Z","shares":{{"agents":[],"gardens":[]}}}}"#,
                (i * 7919) % 2500,
                "\u{e9}".repeat(i % 17),
                ["owner", "admin", "member"][i % 3],
                "x".repeat(i % 40),
                i % 28 + 1
            )
        })
        .collect();
    format!(r#"{{"users":[{}]}}"#, rows.join(","))
}

#[test]
fn users_list() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
        // covers: users
        case("bare: the table", &[], &[], vec![reply(USERS)], owner_and_port, 0, 1),
        case("list", &["list"], &[], vec![reply(USERS)], owner_and_port, 0, 1),
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![reply(USERS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: users --json
        case(
            "--json is no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "none yet",
            &["list"],
            &[],
            vec![reply(r#"{"users":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "only rows that are not logins",
            &["list"],
            &[],
            vec![reply(r#"{"users":"ab"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an error that is no refusal",
            &["list", "--json"],
            &[],
            vec![reply(r#"{"error":"boom"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![reply("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a list answer",
            &["list"],
            &[],
            vec![reply("[1]")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "an error code that is a number",
            &["list"],
            &[],
            vec![reply(r#"{"error":{"code":7}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a shares list that is a number",
            &["list"],
            &[],
            vec![reply(
                r#"{"users":[{"username":"vc","role":"voice_chat","shares":{"agents":3}}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a word it doesn't take",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("help: the page", &["help"], &[], vec![], owner_and_port, 0, 0),
        case(
            "an unknown verb: the line and the page",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let big = many_users();
    let big: &'static str = Box::leak(big.into_boxed_str());
    assert!(big.len() > 256 * 1024);
    for c in [
        case("large: the table", &["list"], &[], vec![reply(big)], owner_and_port, 0, 1),
        case(
            "large: --json",
            &["list", "--json"],
            &[],
            vec![reply(big)],
            owner_and_port,
            0,
            1,
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn users_add() {
    let rig = Rig::new(VERB, "add");
    let v = vec![
        // covers: users add
        stdin(
            b"s3cret \xc3\xa9\r\nnext\n",
            case(
                "a member, temporary",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![reply(r#"{"success":true,"mustChangePassword":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"pw",
            case(
                "member, kept, fixed",
                &["add", "ann", "--role=member", "--keep-password", "--fixed-password", "--password-stdin"],
                &[],
                vec![reply(r#"{"success":true,"fixedPassword":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"pw\n",
            case(
                "voice_chat with shares, forced",
                &[
                    "add", "vc", "--role", "voice-chat", "--share-agent", "a@w", "--share-agent=b@w",
                    "--share-garden", "G", "--share-source", "s1", "--force", "--no-fixed-password",
                    "--password-stdin",
                ],
                &[],
                vec![reply(
                    r#"{"success":true,"role":"voice_chat","shares":{"agents":[{"address":"a@w"},{"id":"b"}],"gardens":[]},"warnings":["b@w is private"],"notice":"note"}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"pw\n",
            case(
                "an admin: the add, then set-role",
                &["add", "ad", "--role", "admin", "--password-stdin"],
                &[],
                vec![
                    reply(r#"{"success":true}"#),
                    reply(r#"{"success":true,"role":"admin"}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
        ),
        stdin(
            b"pw\n",
            case(
                "an owner: the check is asked and there is no vault socket",
                &["add", "own", "--role", "owner", "--keep-password", "--password-stdin"],
                &[],
                vec![
                    reply(r#"{"success":true}"#),
                    Reply::Answer(
                        409,
                        r#"{"error":{"code":"vault_presence_required"},"confirm":{"action":"owner-login","secretId":"s1"}}"#
                            .into(),
                    ),
                ],
                owner_and_port,
                1,
                2,
            ),
        ),
        stdin(
            b"pw\n",
            case(
                "the daemon's error, with its risks",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![Reply::Answer(
                    409,
                    r#"{"error":{"code":"exists","risks":["r1","r2"]}}"#.into(),
                )],
                owner_and_port,
                1,
                1,
            ),
        ),
        stdin(
            b"pw\n",
            case(
                "no success",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![reply(r#"{"ok":1}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        stdin(
            b"\n",
            case(
                "an empty password",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
        ),
        traceback(stdin(
            b"p\xffw\n",
            case(
                "a password that is not UTF-8",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![reply(r#"{"success":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        )),
        case(
            "a password on the command line",
            &["add", "ann", "--password", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("no username", &["add"], &[], vec![], owner_and_port, 2, 0),
        case(
            "the viewer role",
            &["add", "ann", "--role", "Viewer"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a role it doesn't know",
            &["add", "ann", "--role", "boss"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--role with no value",
            &["add", "ann", "--role"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "shares for a member",
            &["add", "ann", "--share-garden", "G"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag: the line and the page",
            &["add", "ann", "--share-source=x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a second username",
            &["add", "ann", "bo"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn users_roles_shares_and_passwords() {
    let rig = Rig::new(VERB, "roles");
    let v = vec![
        // covers: users set-role
        case(
            "set-role",
            &["set-role", "ann", "admin"],
            &[],
            vec![reply(r#"{"success":true,"from":"member","live":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set-role voice-chat, not fixed",
            &["set-role", "ann", "voice-chat", "--no-fixed-password"],
            &[],
            vec![reply(r#"{"success":true,"from":null,"role":"voice_chat"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set-role owner: the check, and no vault socket",
            &["set-role", "ann", "owner"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"vault_presence_required"},"confirm":{"action":"owner-login","secretId":"s1"}}"#
                    .into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "set-role refused",
            &["set-role", "ann", "member"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"role_change_not_allowed"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "set-role: an empty answer",
            &["set-role", "ann", "member"],
            &[],
            vec![reply("")],
            owner_and_port,
            1,
            1,
        ),
        case("set-role: no role", &["set-role", "ann"], &[], vec![], owner_and_port, 2, 0),
        case(
            "set-role: a role it doesn't know",
            &["set-role", "ann", "viewer"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: users share
        case(
            "share: show",
            &["share", "VC"],
            &[],
            vec![reply(USERS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: users share --json
        case(
            "share: show --json",
            &["shares", "vc", "--json"],
            &[],
            vec![reply(USERS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "share: set",
            &["share", "vc", "--agent", "a@w", "--garden", "G", "--source", "s1", "--force"],
            &[],
            vec![reply(
                r#"{"ok":true,"shares":{"agents":[{"address":"a@w"}],"gardens":[{"id":"g1"}]},"warnings":["w1"],"notice":"n"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "share: clear",
            &["share", "vc", "--clear"],
            &[],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "share: not a voice_chat login",
            &["share", "ann"],
            &[],
            vec![reply(USERS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "share: no such login",
            &["share", "zed"],
            &[],
            vec![reply(USERS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "share: refused, with risks",
            &["share", "vc", "--clear"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"role_required","hint":"Owner or admin","risks":["r"]}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case("share: no login", &["share"], &[], vec![], owner_and_port, 2, 0),
        case(
            "share: a flag with no value",
            &["share", "vc", "--garden"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: users set-password
        stdin(
            b"new pw\n",
            case(
                "set-password",
                &["set-password", "bo", "--password-stdin"],
                &[],
                vec![reply(r#"{"success":true,"mustChangePassword":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"new pw\n",
            case(
                "set-password, kept",
                &["set-password", "bo", "--keep-password", "--password-stdin"],
                &[],
                vec![reply(r#"{"success":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"new pw\n",
            case(
                "set-password refused",
                &["set-password", "bo", "--password-stdin"],
                &[],
                vec![Reply::Answer(403, r#"{"error":{"hint":"Forbidden here"}}"#.into())],
                owner_and_port,
                3,
                1,
            ),
        ),
        case(
            "set-password: no login",
            &["set-password"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "set-password: an unknown flag",
            &["set-password", "bo", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn users_audit_clone_and_unlock() {
    let rig = Rig::new(VERB, "audit");
    let events = r#"{"events":[{"ts":"2026-11-20T01:02:03Z","event":"login","outcome":"ok","user":"ann","ingress":"lan","ip":"10.0.0.2","client":"web"},{"event":"logout"},5]}"#;
    let v = vec![
        // covers: users audit
        case("audit", &["audit"], &[], vec![reply(events)], owner_and_port, 0, 1),
        // covers: users audit --json
        case(
            "audit --json: as it came",
            &["audit", "--json"],
            &[],
            vec![reply("{\"events\":[]}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit --tail",
            &["audit", "--tail", "5"],
            &[],
            vec![reply(r#"{"events":[],"path":"/x/auth-audit.jsonl"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit --tail=: none",
            &["audit", "--tail="],
            &[],
            vec![reply(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit: a tail that is no number",
            &["audit", "--tail=x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "audit: the daemon's error",
            &["audit"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"hint":"owner only"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "audit: a word it doesn't take",
            &["audit", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: users set-can-clone
        case(
            "set-can-clone on",
            &["set-can-clone", "ann", "on"],
            &[],
            vec![reply(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "set-can-clone refused",
            &["set-can-clone", "ann", "0"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"hint":"owner only"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "set-can-clone: not JSON",
            &["set-can-clone", "ann", "off"],
            &[],
            vec![reply("oops")],
            owner_and_port,
            3,
            1,
        ),
        case(
            "set-can-clone: no value",
            &["set-can-clone", "ann"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: users unlock
        case(
            "unlock",
            &["unlock", "ann"],
            &[],
            vec![reply(r#"{"ok":true,"username":"ann","cleared":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: users unlock --json
        case(
            "unlock --json",
            &["unlock", "ann", "--json"],
            &[],
            vec![reply(r#"{"ok":true,"username":"ann","cleared":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unlock: ok false",
            &["unlock", "ann"],
            &[],
            vec![reply(r#"{"ok":false,"message":"no such login"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "unlock: an empty answer",
            &["unlock", "ann"],
            &[],
            vec![reply("")],
            owner_and_port,
            1,
            1,
        ),
        case("unlock: no login", &["unlock"], &[], vec![], owner_and_port, 2, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn users_phones() {
    let rig = Rig::new(VERB, "phones");
    let v = vec![
        // covers: users phones
        case("phones", &["phones"], &[], vec![reply(PHONES)], owner_and_port, 0, 1),
        // covers: users phones --json
        case(
            "phones --json",
            &["phones", "--json"],
            &[],
            vec![reply(PHONES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "phones: none, and who a code pairs as",
            &["phones"],
            &[],
            vec![reply(
                r#"{"pairAs":{"error":{"code":"owner_login_not_linked"}},"devices":[]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "phones: refused",
            &["phones"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"login_required","hint":"Sign in"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "phones: an empty answer",
            &["phones"],
            &[],
            vec![reply("\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "phones: a word it doesn't take",
            &["phones", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: users pair-phone
        case(
            "pair-phone: the QR",
            &["pair-phone"],
            &[],
            vec![reply(PAIR)],
            owner_and_port,
            0,
            1,
        ),
        // covers: users pair-phone --json
        case(
            "pair-phone --json",
            &["pair-phone", "--json", "--user", "bo"],
            &[],
            vec![reply(PAIR)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "pair-phone --invert, no link",
            &["pair-phone", "--invert", "--user=bo"],
            &[],
            vec![reply(
                r#"{"user":"bo","code":"9","qr":{"rows":["101","010","111"]}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        stdin(
            b"my pw\nrest\n",
            case(
                "pair-phone from a login: the password again",
                &["pair-phone"],
                &[],
                vec![reply(AGAIN), reply(PAIR)],
                owner_and_port,
                0,
                2,
            ),
        ),
        case(
            "pair-phone from a login, no password",
            &["pair-phone"],
            &[],
            vec![reply(AGAIN)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "pair-phone: --user with no value",
            &["pair-phone", "--user"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: users unpair-phone
        case(
            "unpair-phone",
            &["unpair-phone", "d1"],
            &[],
            vec![reply(r#"{"revoked":"d1","username":"ann"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: users unpair-phone --json
        case(
            "unpair-phone --json",
            &["unpair-phone", "d1", "--json"],
            &[],
            vec![reply(r#"{"revoked":"d1","username":"é"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unpair-phone: no such phone",
            &["unpair-phone", "d9"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"no_such_device"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "unpair-phone: no id",
            &["unpair-phone"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn users_gates_sessions_and_no_answer() {
    let rig = Rig::new(VERB, "gates");
    let v = vec![
        case("no port or token: the gate", &["list"], &[], vec![], nothing, 1, 0),
        case(
            "a session with no passport: the locked tool",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            with_passport(&[]),
            case(
                "an agent's passport, refused: where agents look",
                &["list"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"role_required","hint":"People only"}}"#.into(),
                )],
                nothing,
                3,
                1,
            ),
        ),
        waited(case(
            "list: an empty reply",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(stdin(
            b"pw\n",
            case(
                "add: an empty reply",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    let dead = vec![("K2_PORT", dead_port())];
    for c in [
        case(
            "list: nothing listening reads as none yet",
            &["list"],
            &[],
            vec![],
            owner_only,
            0,
            0,
        ),
        stdin(
            b"pw\n",
            case(
                "add: nothing listening",
                &["add", "ann", "--password-stdin"],
                &[],
                vec![],
                owner_only,
                1,
                0,
            ),
        ),
        case(
            "phones: nothing listening",
            &["phones"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
    ] {
        rig.check(&with(dead.clone(), c));
    }
}

/// `-h` or `--help` on a verb that changes something: the shell's (the
/// binary hands the line over); its router prints the help page and
/// sends nothing.
#[test]
fn users_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for args in [
        &["add", "ann", "--password-stdin", "-h"][..],
        &["set-role", "ann", "owner", "--help"],
        &["share", "vc", "--clear", "-h"],
        &["set-password", "bo", "--help"],
        &["set-can-clone", "ann", "on", "-h"],
        &["unlock", "ann", "--help"],
        &["pair-phone", "-h"],
        &["unpair-phone", "d1", "--help"],
        &["--help"],
    ] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&shell(case(
            "help with -h or --help acts on nothing",
            args,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )));
    }
}
