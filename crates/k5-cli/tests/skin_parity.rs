//! `k5-cli skin` (and `app user|role|grant|front-door|hydra|token`, the
//! same `cmd_skin` and `cmd_skin_token`) against the shell CLI, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - Every native verb with an answer, the daemon's errors through
//!   `_skin_die_if_err`, a held answer, `--json`, Python raising part
//!   way, and one list above 256 KiB (`user list`, `role list`).
//! - The words: usage lines, a value flag as the last word, the `--role`
//!   and `--user` checks.
//! - A passport (`grant list` asks for the agent's own logins).
//! - No answer: the line and exit 1; nothing listening: curl's code.
//! - What the binary hands to the shell, and help that never acts.
//! - A password on stdin (`user add|password --password-stdin`), on the
//!   vault rig ([`common::vault`]), which has room for stdin.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use common::vault::{VCase, VRig};

const VERB: &str = "skin";
const USERS: &str = r#"{"users":[{"username":"bob","roleName":"staff","email":"b@x","fullName":"Bob B","nameSetBySelf":true},{"username":"cat","defaultRoomHandles":["ops"],"agentHandle":"sales","tiedTo":"sales","pendingReset":true},{"username":"dan","appGrants":[{"role":"member","app":"acme/site","rooms":["a","b"]},{"rooms":["b","c"]},{"app":"x"}],"test":{"app":"acme/site","expired":true}}]}"#;
const ROLES: &str = r#"{"roles":[{"name":"staff","roomHandles":["sales","ops"],"roomAccess":[{"handle":"sales","caps":["thread:read","files:read"],"folders":{"read":["docs/"],"write":["in/"],"uploadDir":"in/drop","hide":[".env"]}},{"handle":"ops"}]},{"name":"empty"}]}"#;

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

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// One answer, the owner on disk.
fn one(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

/// Words that stop before anything is sent.
fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn skin_user_reads() {
    let rig = Rig::new(VERB, "ureads");
    let big: String = {
        let rows: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    r#"{{"username":"user{i:05}","fullName":"Name {i} é","email":"u{i}@example.com","defaultRoomHandles":["room-{i}","sales"]}}"#
                )
            })
            .collect();
        format!(r#"{{"users":[{}]}}"#, rows.join(","))
    };
    assert!(
        big.len() > 256 * 1024,
        "the large list is {} bytes",
        big.len()
    );
    check(
        &rig,
        vec![
            // covers: skin user list
            case(
                "user list",
                &["user", "list"],
                &[],
                vec![ok(USERS), ok(ROLES)],
                owner_and_port,
                0,
                2,
            ),
            // covers: skin user list
            case(
                "user alone is the list",
                &["users"],
                &[],
                vec![ok(r#"{"users":[]}"#), ok(ROLES)],
                owner_and_port,
                0,
                2,
            ),
            // covers: skin user list
            case(
                "a large user list, above 256 KiB",
                &["user", "list"],
                &[],
                vec![ok(&big), ok(ROLES)],
                owner_and_port,
                0,
                2,
            ),
            // covers: skin user list
            case(
                "user list: the roles answer refused",
                &["user", "list"],
                &[],
                vec![
                    ok(USERS),
                    Reply::Answer(403, r#"{"error":{"code":"owner_only"}}"#.into()),
                ],
                owner_and_port,
                3,
                2,
            ),
            // covers: skin user list
            traceback(case(
                "user list: an answer over two lines",
                &["user", "list"],
                &[],
                vec![ok("{\n\"users\": []\n}"), ok(ROLES)],
                owner_and_port,
                1,
                2,
            )),
            // covers: skin user list
            traceback(case(
                "user list: a role name that is a list",
                &["user", "list"],
                &[],
                vec![ok(USERS), ok(r#"{"roles":[{"name":["x"]}]}"#)],
                owner_and_port,
                1,
                2,
            )),
            one(
                "user show",
                &["user", "show", "bob", "--app", "acme/site"],
                r#"{"username":"bob","app":"acme/site","fullName":"Bob","nameSetBySelf":true,"email":"b@x","imageHash":"0123456789abcdef","role":"staff","settings":{"editNames":true},"sessions":[{"id":"s1","kind":"web","client":"Firefox on a very long platform name","push":true,"pushPlatform":"apns"}]}"#,
                0,
            ),
            one(
                "user show --json",
                &["user", "show", "bob", "--app", "a b", "--json"],
                r#"{"username":"bob"}"#,
                0,
            ),
            traceback(one(
                "user show: a picture hash that is a number",
                &["user", "show", "bob", "--app", "a"],
                r#"{"username":"bob","imageHash":7}"#,
                1,
            )),
            traceback(one(
                "user show: a time that is text",
                &["user", "show", "bob", "--app", "a"],
                r#"{"username":"bob","createdAt":"soon"}"#,
                1,
            )),
            one(
                "user sessions",
                &["user", "sessions", "bob", "--app", "a"],
                r#"{"username":"bob","app":"a","sessions":[{"id":"s1","kind":"app","client":null}]}"#,
                0,
            ),
            one(
                "user sessions: none",
                &["user", "sessions", "bob", "--app", "a"],
                r#"{"username":"bob","app":"a","sessions":[]}"#,
                0,
            ),
            one(
                "user sessions --json",
                &["user", "sessions", "bob", "--app", "a", "--json"],
                r#"{"sessions":null,"x":1}"#,
                0,
            ),
            words("user show with no --app", &["user", "show", "bob"], 1),
            // covers: skin user password
            words(
                "user password with no --password-stdin",
                &["user", "password", "bob"],
                1,
            ),
            words(
                "user sessions with no user",
                &["user", "sessions", "--app", "a"],
                1,
            ),
            words("an unknown user verb", &["user", "nope"], 1),
            // covers: skin user add
            words(
                "--role with no --app",
                &["user", "add", "bob", "--role", "r"],
                2,
            ),
        ],
    );
}

#[test]
fn skin_user_writes() {
    let rig = Rig::new(VERB, "uwrites");
    let waiting = r#""waitingForReset":{"rooms":["sales","ops"],"ask":"ask them"}"#;
    let added = format!(r#"{{"username":"bob","generatedPassword":"pw-1",{waiting}}}"#);
    let added: &'static str = Box::leak(added.into_boxed_str());
    check(
        &rig,
        vec![
            // covers: skin user add
            one(
                "user add",
                &["user", "add", "bob", "--temp-password"],
                added,
                0,
            ),
            one(
                "user add on an App",
                &[
                    "user", "add", "bob", "--app", " acme ", "--role=r", "--email", "b@x",
                ],
                r#"{"username":"bob"}"#,
                0,
            ),
            // covers: skin user add
            one(
                "user add held",
                &["user", "add", "bob"],
                r#"{"status":"held","request":{"id":"r1","summary":"add bob"},"hint":"wait","afterApproval":"then go"}"#,
                2,
            ),
            // covers: skin user add
            traceback(one(
                "user add: waiting rooms that are a number",
                &["user", "add", "bob"],
                r#"{"username":"bob","waitingForReset":{"rooms":5}}"#,
                1,
            )),
            // covers: skin user add
            words("user add with no name", &["user", "add"], 1),
            // covers: skin user add
            words(
                "user add with both password forms",
                &["user", "add", "b", "--password-stdin", "--temp-password"],
                2,
            ),
            one(
                "user image",
                &["user", "image", "bob", "--clear"],
                r#"{"cleared":false,"username":"bob"}"#,
                0,
            ),
            words("user image with no --clear", &["user", "image", "bob"], 1),
            one(
                "user sessions --revoke",
                &["user", "sessions", "bob", "--app", "a", "--revoke", "s1"],
                r#"{"ended":1,"username":"bob","app":"a"}"#,
                0,
            ),
            traceback(one(
                "user sessions --revoke-all: ended is text",
                &["user", "sessions", "bob", "--app", "a", "--revoke-all"],
                r#"{"ended":"x"}"#,
                1,
            )),
            one(
                "user invite sent",
                &["user", "invite", "bob", "--app", "a", "--from", "me@x"],
                r#"{"delivered":"sent","sentTo":"b@x","from":"me@x","ttlSecs":259200}"#,
                0,
            ),
            one(
                "user invite with no mail path",
                &["user", "invite", "bob", "--app=a", "--json"],
                r#"{"delivered":"link","link":"https://x/y"}"#,
                0,
            ),
            one(
                "user invite: no mail path, waiting",
                &["user", "invite", "bob", "--app", "a"],
                added,
                0,
            ),
            one(
                "user reset --revoke",
                &["user", "reset", "bob", "--app", "a", "--revoke", "--code"],
                r#"{"revoked":2,"username":"bob"}"#,
                0,
            ),
            one(
                "user reset held",
                &["user", "reset", "bob", "--to", "b@x"],
                r#"{"status":"held","request":{}}"#,
                2,
            ),
            // covers: skin user unlock
            one(
                "user unlock",
                &["user", "unlock", "bob"],
                r#"{"cleared":true,"username":"bob"}"#,
                0,
            ),
            // covers: skin user unlock --json
            one(
                "user unlock --json",
                &["user", "unlock", "bob", "--json"],
                r#"{"cleared":1}"#,
                0,
            ),
            // covers: skin user email
            one(
                "user email",
                &["user", "email", "bob", "--email=b@x"],
                r#"{"email":"b@x","username":"bob"}"#,
                0,
            ),
            one(
                "user full-name cleared",
                &["user", "full-name", "bob"],
                r#"{"username":"bob"}"#,
                0,
            ),
            one(
                "user agent",
                &["user", "agent", "bob", "sales"],
                r#"{"username":"bob","agentWorkspace":"sales"}"#,
                0,
            ),
            one(
                "user agent --clear",
                &["user", "agent", "bob", "--clear"],
                r#"{"username":"bob"}"#,
                0,
            ),
            words(
                "user agent with both",
                &["user", "agent", "bob", "w", "--clear"],
                1,
            ),
            // covers: skin user remove
            one(
                "user remove",
                &["user", "rm", "bob"],
                r#"{"success":true}"#,
                0,
            ),
            one(
                "user rooms on an App",
                &[
                    "user", "rooms", "bob", "--agent", "sales", "--agent", " ops ", "--app", "a",
                ],
                &format!(r#"{{"app":"a","roomHandles":["sales"],"username":"bob",{waiting}}}"#),
                0,
            ),
            // covers: skin user rooms
            one(
                "user rooms, default rooms",
                &["user", "rooms", "bob"],
                r#"{"username":"bob","default_room_handles":[]}"#,
                0,
            ),
            // covers: skin user rooms
            words(
                "user rooms --agent as the last word",
                &["user", "rooms", "bob", "--agent"],
                2,
            ),
            // covers: skin user rooms
            words(
                "user rooms --apply-tokens",
                &["user", "rooms", "bob", "--apply-tokens"],
                2,
            ),
            // covers: skin user role
            one(
                "user role",
                &["user", "role", "bob", "staff"],
                r#"{"username":"bob","role_name":"staff"}"#,
                0,
            ),
            // covers: skin user unassign
            one(
                "user unassign",
                &["user", "unassign", "bob"],
                r#"{"username":"bob"}"#,
                0,
            ),
            // covers: skin user unassign
            one(
                "a refusal",
                &["user", "unassign", "bob"],
                r#"{"error":{"code":"forbidden","hint":"Not yours"}}"#,
                3,
            ),
            // covers: skin user remove
            traceback(one(
                "an answer that is a list",
                &["user", "remove", "bob"],
                "[1]",
                1,
            )),
        ],
    );
}

#[test]
fn skin_roles() {
    let rig = Rig::new(VERB, "roles");
    let big: String = {
        let rows: Vec<String> = (0..2500)
            .map(|i| {
                format!(
                    r#"{{"name":"role-{i}","roomAccess":[{{"handle":"room-{i}","caps":["thread:read","thread:post"],"folders":{{"read":["a/","b/"],"hide":[".env"]}}}}]}}"#
                )
            })
            .collect();
        format!(r#"{{"roles":[{}]}}"#, rows.join(","))
    };
    assert!(
        big.len() > 256 * 1024,
        "the large list is {} bytes",
        big.len()
    );
    let access = r#"{"name":"staff","roomAccess":[{"handle":"sales","caps":["thread:read"],"folders":{"read":["docs/"],"write":["in/"],"upload":["drop/"],"download":["a.pdf"],"uploadDir":"in/x","hide":[".env"]}},{"handle":"ops","folders":{}}]}"#;
    check(
        &rig,
        vec![
            // covers: skin role list
            one("role list", &["role", "list"], ROLES, 0),
            // covers: skin role list
            one("roles alone is the list", &["roles"], r#"{"roles":[]}"#, 0),
            // covers: skin role list
            one(
                "a large role list, above 256 KiB",
                &["role", "list"],
                &big,
                0,
            ),
            // covers: skin role create
            one(
                "role create",
                &["role", "create", "staff", "--agent", "sales", "--app", "a"],
                access,
                0,
            ),
            // covers: skin role create
            one(
                "role create, no rooms",
                &["role", "create", "staff"],
                r#"{"name":"staff"}"#,
                0,
            ),
            // covers: skin role create
            words(
                "role create --caps",
                &["role", "create", "staff", "--caps", "x"],
                2,
            ),
            // covers: skin role create
            words("role --user", &["role", "create", "x", "--user", "bob"], 2),
            one(
                "role room",
                &[
                    "role",
                    "room",
                    "staff",
                    "sales",
                    "--caps",
                    "thread:read, files:read",
                ],
                access,
                0,
            ),
            one(
                "role room held",
                &["role", "room", "staff", "sales", "--clear"],
                r#"{"status":"held","request":{"id":"r9","summary":"room"},"hint":"ask"}"#,
                2,
            ),
            traceback(one(
                "role room: folders read is text",
                &["role", "room", "staff", "sales"],
                r#"{"name":"staff","roomAccess":[{"handle":"sales","folders":{"read":"docs","write":["w"]}}]}"#,
                1,
            )),
            one(
                "role paths",
                &[
                    "role", "paths", "staff", "sales", "--read", "docs/,b/", "--hide", ".env",
                    "--unhide", "x",
                ],
                access,
                0,
            ),
            one(
                "role paths, the whole room",
                &["role", "paths", "staff", "ops", "--clear"],
                access,
                0,
            ),
            words(
                "role paths with nothing",
                &["role", "paths", "staff", "sales"],
                2,
            ),
            words(
                "role paths --clear with folders",
                &["role", "paths", "staff", "sales", "--clear", "--read", "a"],
                2,
            ),
            words(
                "role paths: a third word",
                &["role", "paths", "staff", "sales", "x"],
                1,
            ),
            // covers: skin role update
            one(
                "role update --clear",
                &["role", "update", "staff", "--clear"],
                r#"{"name":"staff","roomAccess":[]}"#,
                0,
            ),
            // covers: skin role update
            one(
                "role update rooms",
                &["role", "update", "staff", "--agent", "sales"],
                access,
                0,
            ),
            // covers: skin role remove
            one("role remove", &["role", "rm", "staff"], r#"{}"#, 0),
            one(
                "role add from a template",
                &[
                    "role", "add", "--from", "viewer", "--app", "a", "--agent", "sales",
                ],
                r#"{"template":"viewer","role":{"name":"viewers","roomAccess":[{"handle":"sales","caps":["thread:read"]}]}}"#,
                0,
            ),
            words("role add with no --from", &["role", "add", "--app", "a"], 2),
            words("role add: an unknown word", &["role", "add", "x"], 2),
            words("an unknown role verb", &["role", "nope"], 1),
        ],
    );
}

#[test]
fn skin_grants_doors_and_hydra() {
    let rig = Rig::new(VERB, "grants");
    check(
        &rig,
        vec![
            one(
                "grant list",
                &["grant", "list"],
                r#"{"grants":[{"id":"g1","subjectKind":"principal","subjectId":"bob","kind":"app","targetId":"site","roleId":"staff","hookState":"failed","hookError":"no sew"},{"id":"g2","subject_kind":"workspace","scope":"member"}]}"#,
                0,
            ),
            one("grants alone, none", &["grants"], r#"{"grants":[]}"#, 0),
            traceback(one(
                "grant list: a role that is a number",
                &["grant"],
                r#"{"grants":[{"roleId":5}]}"#,
                1,
            )),
            one(
                "grant add",
                &[
                    "grant",
                    "add",
                    "--subject-kind",
                    "principal",
                    "--subject",
                    "bob",
                    "--kind",
                    "chat",
                    "--scope",
                    "guest",
                ],
                r#"{"id":"g3","subjectKind":"principal","subjectId":"bob","kind":"chat","targetId":"sew","scope":"guest"}"#,
                0,
            ),
            words(
                "grant add, missing words",
                &["grant", "add", "--kind", "app"],
                1,
            ),
            words("grant add: a word", &["grant", "add", "x"], 1),
            one(
                "grant rm with cascades",
                &["grant", "rm", "--id=g1"],
                r#"{"success":true,"cascaded":[{"kind":"chat","id":"g9","subjectKind":"principal","subjectId":"bob","scope":"guest"}]}"#,
                0,
            ),
            one(
                "grant set",
                &["grant", "set", "--id", "g1", "--scope", "member"],
                r#"{"id":"g1","scope":"member"}"#,
                0,
            ),
            one(
                "grant role",
                &["grant", "role", "g1", "--role", "staff"],
                r#"{"id":"g1","changed":true,"subjectId":"bob","targetId":"site"}"#,
                0,
            ),
            words("an unknown grant verb", &["grant", "nope"], 1),
            // covers: skin front-door
            one(
                "front-door",
                &["front-door"],
                r#"{"mode":"connect","url":"https://x","applied":true,"caddy":{"running":true,"pid":9},"nested":{"label":"n"},"hint":"ok"}"#,
                0,
            ),
            // covers: skin front-door --json
            one(
                "front-door --json",
                &["frontdoor", "--json"],
                r#"{"mode":"direct"}"#,
                0,
            ),
            one(
                "front-door direct",
                &[
                    "front-door",
                    "direct",
                    "--url",
                    "https://x",
                    "--ui-port",
                    "8080",
                ],
                r#"{"mode":"direct","url":"https://x","listen":":443","applied":true}"#,
                0,
            ),
            // covers: skin front-door
            one(
                "front-door direct: caddy missing",
                &["front-door", "direct"],
                r#"{"mode":"direct","error":"caddy_missing","applied":false}"#,
                1,
            ),
            // covers: skin front-door
            one(
                "front-door connect: an owner refusal",
                &["front-door", "connect"],
                r#"{"error":{"code":"owner_only","hint":"No"}}"#,
                3,
            ),
            // covers: skin front-door --json
            one(
                "front-door connect --json: a string refusal",
                &["front-door", "connect", "--json"],
                r#"{"error":"Owner token needed"}"#,
                3,
            ),
            // covers: skin front-door
            one(
                "front-door connect: not JSON",
                &["front-door", "connect"],
                "<html>",
                1,
            ),
            // covers: skin front-door
            words(
                "front-door: two modes",
                &["front-door", "connect", "direct"],
                1,
            ),
            // covers: skin front-door
            words(
                "front-door: --url last",
                &["front-door", "direct", "--url"],
                2,
            ),
            one(
                "hydra",
                &["hydra"],
                r#"{"supported":true,"enabled":false,"running":false,"hint":"try"}"#,
                0,
            ),
            one(
                "hydra on --json",
                &["hydra", "on", "--json"],
                r#"{"enabled":true}"#,
                0,
            ),
            words("hydra: a word", &["hydra", "x"], 2),
        ],
    );
}

#[test]
fn skin_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    check(
        &rig,
        vec![
            with(
                h(&[]),
                case(
                    "grant list: the agent's own logins",
                    &["grant", "list"],
                    &[],
                    vec![ok(
                        r#"{"scope":"own","grants":[{"id":"g1","username":"bob","app":"site","rooms":["a","b"]},{"id":"g2","subjectId":"cat","role":"staff","enabled":false}]}"#,
                    )],
                    port_only,
                    0,
                    1,
                ),
            ),
            with(
                h(&[]),
                shell(case(
                    "grant add with a passport: the shell's",
                    &[
                        "grant",
                        "add",
                        "--subject-kind",
                        "p",
                        "--subject",
                        "s",
                        "--kind",
                        "app",
                        "--target",
                        "t",
                    ],
                    &[],
                    vec![ok(r#"{"id":"g1"}"#)],
                    port_only,
                    0,
                    1,
                )),
            ),
            // covers: skin role list
            case(
                "HOOK_SOCK set and no passport: the require-passport lines",
                &["role", "list"],
                &[("K2_HOOK_SOCK", "{sock}")],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ],
    );
}

#[test]
fn skin_with_no_answer() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    check(
        &rig,
        vec![
            // covers: skin role list
            case(
                "nothing listens (7)",
                &["role", "list"],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                7,
                0,
            ),
            // covers: skin user remove
            case(
                "nothing listens (7), a POST",
                &["user", "remove", "bob"],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                7,
                0,
            ),
            case(
                "no port: the gate",
                &["hydra"],
                &[],
                vec![],
                owner_only,
                1,
                0,
            ),
        ],
    );
    let rig = Rig::new(VERB, "noanswer");
    rig.check(&waited(case(
        "a 5xx with no body to a GET: the line, exit 1",
        &["hydra"],
        &[],
        vec![Reply::Answer(502, String::new())],
        owner_and_port,
        1,
        1,
    )));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's. Help never acts (K2's rule):
/// the router's `skin)` arm prints the family's page for -h or --help
/// anywhere, and sends nothing.
#[test]
fn skin_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    check(
        &rig,
        vec![
            shell(words("--help", &["--help"], 0)),
            shell(words("user add -h", &["user", "add", "bob", "-h"], 0)),
            shell(words(
                "user remove --help",
                &["user", "remove", "bob", "--help"],
                0,
            )),
            shell(words("role create -h", &["role", "create", "r", "-h"], 0)),
            shell(words(
                "grant rm --help",
                &["grant", "rm", "--id", "g", "--help"],
                0,
            )),
            shell(words(
                "front-door direct -h",
                &["front-door", "direct", "-h"],
                0,
            )),
            shell(words("hydra on --help", &["hydra", "on", "--help"], 0)),
            shell(case(
                "a port that is no port",
                &["role", "list"],
                &[("K2_PORT", "41o0")],
                vec![],
                owner_only,
                3,
                0,
            )),
        ],
    );
}

const CHAT: &str = r#"{"username":"bob","scope":"guest","enabled":true,"managedBy":"app","app":"acme","members":3,"maxMembers":null,"principalId":"p-bob","warning":"near the cap"}"#;

/// `user chat` (`_compute_call`) through `skin` (the teaching line first)
/// and through `app` (none): the read, a change, `--json`, the words'
/// stops, a refusal, Python raising part way.
#[test]
fn user_chat_as_the_shell_does() {
    for (noun, tag) in [("skin", "chat"), ("app", "appchat")] {
        let rig = Rig::new(noun, tag);
        check(
            &rig,
            vec![
                // native: skin user chat
                // native: app user chat
                one("user chat: the read", &["user", "chat", "bob"], CHAT, 0),
                one(
                    "user chat --app: the read for an App",
                    &["user", "chat", "bob é", "--app", "acme site"],
                    r#"{"username":"bob","scope":null,"principalId":"p"}"#,
                    0,
                ),
                one(
                    "users chat bob none: a change, unchanged, switched off",
                    &["users", "chat", "bob", "none", "--app", "acme"],
                    r#"{"username":"bob","scope":"member","enabled":false,"managedBy":"owner","app":"acme","members":2,"maxMembers":10,"changed":false,"principalId":"p"}"#,
                    0,
                ),
                one(
                    "user chat --json",
                    &["user", "chat", "--json", "bob", "member"],
                    CHAT,
                    0,
                ),
                words("user chat: no username", &["user", "chat"], 2),
                words(
                    "user chat: a bad scope",
                    &["user", "chat", "bob", "admin"],
                    2,
                ),
                words(
                    "user chat: a third word",
                    &["user", "chat", "bob", "guest", "x"],
                    2,
                ),
                words(
                    "user chat: a flag it doesn't take",
                    &["user", "chat", "bob", "-q"],
                    2,
                ),
                words(
                    "user chat: --app last",
                    &["user", "chat", "bob", "--app"],
                    2,
                ),
                words(
                    "user chat: --role with no --app",
                    &["user", "chat", "bob", "--role", "r"],
                    2,
                ),
                case(
                    "user chat: a refusal",
                    &["user", "chat", "bob", "guest"],
                    &[],
                    vec![Reply::Answer(
                        403,
                        r#"{"error":{"code":"owner_only","hint":"the owner's"}}"#.into(),
                    )],
                    owner_and_port,
                    3,
                    1,
                ),
                traceback(one(
                    "user chat: a warning that is no string",
                    &["user", "chat", "bob"],
                    r#"{"username":"bob","principalId":"p","warning":["x"]}"#,
                    1,
                )),
                traceback(one(
                    "user chat: an answer that is a list",
                    &["user", "chat", "bob"],
                    "[1]",
                    1,
                )),
                case(
                    "user chat, no daemon: the gate",
                    &["user", "chat", "bob"],
                    &[],
                    vec![],
                    nothing,
                    1,
                    0,
                ),
            ],
        );
    }
}

const COPIED_HOST: &str = r#"{"dryRun":false,"added":["ann"],"appInfo":{"handle":"acme-h"},"skippedOtherAppRoleCount":1,"skippedOtherAppRole":["dee"]}"#;

/// `skin grant copy-host` is `cmd_skin`'s arm, the one `app grant
/// copy-host` runs: the teaching line, then the same requests and lines
/// (app_guests_parity.rs has the rest of its rows).
#[test]
fn skin_grant_copy_host_as_the_shell_does() {
    let rig = Rig::new(VERB, "copyhost");
    check(
        &rig,
        vec![
            // native: skin grant copy-host
            one(
                "grant copy-host",
                &["grant", "copy-host", "acme"],
                COPIED_HOST,
                0,
            ),
            // native: skin grant copyhost
            one(
                "grants copyhost --dry-run --json",
                &["grants", "copyhost", "acme", "--dry-run", "--json"],
                COPIED_HOST,
                0,
            ),
            words("grant copy-host: no app", &["grant", "copy-host"], 1),
            words(
                "grant copy-host: a flag it doesn't take",
                &["grant", "copy-host", "acme", "-q"],
                1,
            ),
            case(
                "grant copy-host: held for the owner",
                &["grant", "copy-host", "acme"],
                &[],
                vec![Reply::Answer(
                    202,
                    r#"{"status":"held","request":{"id":"r9","summary":"copy-host on acme"},"hint":"The owner decides."}"#.into(),
                )],
                owner_and_port,
                2,
                1,
            ),
            // From `skin` the refusal carries no registry fix: cmd_skin
            // leaves _K2_REFUSAL_AREA empty (app_guests_parity.rs has it
            // from `app`).
            case(
                "grant copy-host: a refusal",
                &["grant", "copy-host", "acme"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"the owner's"}}"#.into(),
                )],
                owner_and_port,
                3,
                1,
            ),
        ],
    );
}

/// `app` alone and `app --json`: `cmd_app` hands them to `cmd_skin`,
/// which prints the App family's page through the permission-card
/// filter, with no teaching line (the family's bare row).
#[test]
fn app_alone_is_skin_s_page() {
    let rig = Rig::new("app", "bare");
    check(
        &rig,
        vec![
            // covers: app
            words("app alone: the page, exit 1", &[], 1),
            words("app --json: the page", &["--json"], 1),
            case("app alone, no daemon", &[], &[], vec![], nothing, 1, 0),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "app alone, cards on",
                    &[],
                    &[],
                    vec![ok(r#"{"enabled":true}"#)],
                    port_only,
                    1,
                    1,
                ),
            ),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "app --json, cards off",
                    &["--json"],
                    &[],
                    vec![ok(r#"{"enabled":false}"#)],
                    port_only,
                    1,
                    1,
                ),
            ),
            shell(words("app --help: the shell's", &["--help"], 0)),
        ],
    );
}

const TEMPLATES: &str = r#"{"templates":[{"id":"viewer","label":"Viewer","description":"Reads é","caps":["read"],"folders":{"read":["docs"],"write":[],"uploadDir":"up"},"note":"safe"},{"id":"a-long-id","label":"Editor","description":"Writes","caps":[]}]}"#;

/// `skin` alone and an unknown verb print the App family's page through
/// the permission-card filter; `role suggest` is a `_compute_call`.
#[test]
fn skin_alone_and_role_suggest() {
    let rig = Rig::new(VERB, "usage");
    check(
        &rig,
        vec![
            // covers: skin
            words("skin alone: the page, exit 1", &[], 1),
            words("an unknown verb", &["nope"], 1),
            words("--json is no verb", &["--json"], 1),
            case("skin alone, no daemon", &[], &[], vec![], nothing, 1, 0),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "skin alone, cards on",
                    &[],
                    &[],
                    vec![ok(r#"{"enabled":true}"#)],
                    port_only,
                    1,
                    1,
                ),
            ),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "skin alone, cards off",
                    &["nope"],
                    &[],
                    vec![ok(r#"{"enabled":false}"#)],
                    port_only,
                    1,
                    1,
                ),
            ),
            // covers: skin role
            one("role suggest", &["role", "suggest"], TEMPLATES, 0),
            one(
                "roles templates --json",
                &["roles", "templates", "--json"],
                TEMPLATES,
                0,
            ),
            one(
                "role suggestions, none",
                &["role", "suggestions"],
                r#"{"templates":[]}"#,
                0,
            ),
            traceback(one(
                "role suggest, a template with no id",
                &["role", "suggest"],
                r#"{"templates":[{"label":"x"}]}"#,
                1,
            )),
            traceback(one(
                "role suggest, not JSON",
                &["role", "suggest"],
                "<html>",
                1,
            )),
            case(
                "role suggest, refused",
                &["role", "suggest"],
                &[],
                vec![Reply::Answer(403, r#"{"error":"owner_only"}"#.into())],
                owner_and_port,
                3,
                1,
            ),
        ],
    );
}

/// `app user|role|grant|front-door|hydra` are `cmd_skin`, with no
/// teaching line; `app token` is `cmd_skin_token`.
#[test]
fn app_reaches_the_same_code() {
    let rig = Rig::new("app", "skin");
    check(
        &rig,
        vec![
            // covers: app user list
            case(
                "app user list",
                &["user", "list"],
                &[],
                vec![ok(USERS), ok(ROLES)],
                owner_and_port,
                0,
                2,
            ),
            // covers: app role list
            one("app role list", &["roles"], ROLES, 0),
            // covers: app grant list
            one("app grant list", &["grant"], r#"{"grants":[]}"#, 0),
            // covers: app hydra
            one(
                "app hydra off",
                &["hydra", "off"],
                r#"{"enabled":false}"#,
                0,
            ),
            // covers: app front-door
            one(
                "app front-door",
                &["front-door"],
                r#"{"mode":"connect"}"#,
                0,
            ),
            // covers: app token list
            one("app token list", &["token", "list"], r#"{"tokens":[]}"#, 0),
            // covers: app token
            words("app tokens with no verb", &["tokens"], 1),
            words("app user: an unknown verb", &["user", "nope"], 1),
            shell(words("app user add -h", &["user", "add", "bob", "-h"], 0)),
        ],
    );
}

/// A password on stdin: the body the daemon gets, the printer, and no
/// child that sees it. `noun`: `skin` or `app`.
fn on_stdin(
    noun: &'static str,
    name: &'static str,
    args: &[&'static str],
    stdin: &[u8],
    answer: Reply,
    code: i32,
) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = noun;
    c.daemon = vec![answer];
    c.daemon_requests = 1;
    c.stdin = stdin.to_vec();
    c.code = code;
    c
}

#[test]
fn skin_password_on_stdin() {
    let rig = VRig::new();
    let mut v = vec![
        // covers: skin user add
        on_stdin(
            "skin",
            "user add --password-stdin: all of stdin, the trailing newlines off",
            &["user", "add", "bob", "--password-stdin", "--email", "b@x"],
            b"pw one\n\ntwo\n\n",
            ok(r#"{"username":"bob"}"#),
            0,
        ),
        // covers: app user add
        on_stdin(
            "app",
            "app user add --password-stdin --app: a byte that isn't UTF-8 and a CR",
            &[
                "user",
                "add",
                "bob",
                "--password-stdin",
                "--app",
                "acme/site",
                "--role",
                "member",
            ],
            b"p\xffw\"d\r\n",
            ok(r#"{"username":"bob"}"#),
            0,
        ),
        // covers: skin user password
        on_stdin(
            "skin",
            "user password --password-stdin",
            &["user", "password", "bob", "--password-stdin"],
            b"pw-reset-1\n",
            ok(r#"{"username":"bob","hasPassword":true,"passwordMustChange":true}"#),
            0,
        ),
        // covers: app user password
        on_stdin(
            "app",
            "app user password --password-stdin, nothing on stdin: cleared",
            &["user", "password", "--password-stdin", "bob"],
            b"",
            ok(r#"{"username":"bob","hasPassword":false}"#),
            0,
        ),
        // covers: app user password
        on_stdin(
            "app",
            "app user password: the daemon's error",
            &["user", "password", "bob", "--password-stdin"],
            b"pw\n",
            Reply::Answer(404, r#"{"error":"no such user"}"#.into()),
            1,
        ),
        // covers: app user password
        on_stdin(
            "app",
            "app user password: a list, Python's AttributeError",
            &["user", "password", "bob", "--password-stdin"],
            b"pw\n",
            ok("[]"),
            1,
        ),
    ];
    v[5].compare = common::vault::VCompare::Traceback;
    v[0].secrets = vec!["pw one"];
    v[2].secrets = vec!["pw-reset-1"];
    for c in v {
        rig.check(&c);
    }
}

/// `skin` alone, an empty verb or one `cmd_skin` has no arm for: the
/// teaching line, then `_skin_usage` (the App family's page through
/// `_perm_lines`, the cards asked with a passport), exit 1. Each row is
/// the binary's own answer.
#[test]
fn skin_usage_as_the_shell_prints_it() {
    let rig = Rig::new(VERB, "usage");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    for c in [
        // native: skin
        words("skin alone", &[], 1),
        words("an empty verb", &[""], 1),
        words("an unknown verb", &["nope", "x"], 1),
        words("--json is no verb", &["--json"], 1),
        case("the connection gate", &[], &[], vec![], nothing, 1, 0),
        case(
            "a session with no passport: the locked tool",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(),
            case(
                "cards on, asked on the cell socket",
                &[],
                &[],
                vec![ok(common::cards::ON)],
                port_only,
                1,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "cards off",
                &["nope"],
                &[],
                vec![ok(common::cards::OFF)],
                port_only,
                1,
                1,
            ),
        ),
    ] {
        rig.check(&c);
        rig.binary(&c, false);
        assert!(
            !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
            "{}: the binary handed the line to the shell",
            c.name
        );
    }
}
