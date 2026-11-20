//! `k5-cli app user|role|token …` against the shell CLI, on the parity rig
//! ([`common::parity`]): `cmd_app` hands these to `cmd_skin` and
//! `cmd_skin_token` (`cli/src/app-wallet.sh`), which the binary answers
//! with the skin and skin-token families' code (no teaching line under
//! `app`). `skin_parity.rs` and `skin_token_parity.rs` drive the same
//! code under the old names; these rows drive every form under `app`.
//!
//! - Each verb's answer, `--json` where it takes it, a held answer, a
//!   refusal and Python raising part way.
//! - The words: usage lines and a value flag as the last word.
//! - Help never acts: each write verb with `-h` or `--help` on the line.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "app";
const ACCESS: &str = r#"{"name":"staff","roomAccess":[{"handle":"sales","caps":["thread:read"],"folders":{"read":["docs/"],"write":["in/"],"upload":["drop/"],"download":["a.pdf"],"uploadDir":"in/x","hide":[".env"]}},{"handle":"ops","folders":{}}]}"#;
const ROLES: &str = r#"{"roles":[{"name":"staff","roomHandles":["sales","ops"]},{"name":"empty"}]}"#;
const WAITING: &str = r#"{"username":"bob","generatedPassword":"pw-1","waitingForReset":{"rooms":["sales","ops"],"ask":"ask them"}}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
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
fn app_user_reads_and_writes() {
    let rig = Rig::new(VERB, "users");
    check(
        &rig,
        vec![
            // covers: app user show
            one(
                "user show",
                &["user", "show", "bob", "--app", "acme/site"],
                r#"{"username":"bob","app":"acme/site","fullName":"Bob","nameSetBySelf":true,"email":"b@x","imageHash":"0123456789abcdef","role":"staff","settings":{"editNames":true},"sessions":[{"id":"s1","kind":"web","client":"Firefox on a long platform name é","push":true,"pushPlatform":"apns"}]}"#,
                0,
            ),
            // covers: app user show --json
            one(
                "user show --json",
                &["user", "show", "bob", "--app=a b", "--json"],
                r#"{"username":"bob"}"#,
                0,
            ),
            // covers: app user show
            words("user show with no --app", &["user", "show", "bob"], 1),
            // covers: app user show
            traceback(one(
                "user show: a picture hash that is a number",
                &["user", "show", "bob", "--app", "a"],
                r#"{"username":"bob","imageHash":7}"#,
                1,
            )),
            // covers: app user image
            one(
                "user image --clear",
                &["user", "image", "bob", "--clear"],
                r#"{"cleared":true,"username":"bob"}"#,
                0,
            ),
            // covers: app user image
            words("user image with no --clear", &["user", "image", "bob"], 1),
            // covers: app user sessions
            one(
                "user sessions",
                &["user", "sessions", "bob", "--app", "a"],
                r#"{"username":"bob","app":"a","sessions":[{"id":"s1","kind":"app","client":null},{"id":"s2","kind":"web","client":"Safari"}]}"#,
                0,
            ),
            // covers: app user sessions --json
            one(
                "user sessions --json",
                &["user", "sessions", "bob", "--app", "a", "--json"],
                r#"{"sessions":null,"x":1}"#,
                0,
            ),
            // covers: app user sessions
            one(
                "user sessions --revoke",
                &["user", "sessions", "bob", "--app", "a", "--revoke", "s1"],
                r#"{"ended":1,"username":"bob","app":"a"}"#,
                0,
            ),
            // covers: app user sessions --json
            one(
                "user sessions --revoke-all --json",
                &["user", "sessions", "bob", "--app", "a", "--revoke-all", "--json"],
                r#"{"ended":3}"#,
                0,
            ),
            // covers: app user invite
            one(
                "user invite sent",
                &["user", "invite", "bob", "--app", "a", "--from", "me@x", "--role", "r"],
                r#"{"delivered":"sent","sentTo":"b@x","from":"me@x","ttlSecs":259200}"#,
                0,
            ),
            // covers: app user invite
            one(
                "user invite: no mail path, waiting",
                &["user", "invite", "bob", "--app", "a"],
                WAITING,
                0,
            ),
            // covers: app user invite --json
            one(
                "user invite --json",
                &["user", "invite", "bob", "--app=a", "--json"],
                r#"{"delivered":"link","link":"https://x/y"}"#,
                0,
            ),
            // covers: app user invite
            words("user invite with no --app", &["user", "invite", "bob"], 1),
            // covers: app user reset
            one(
                "user reset --revoke",
                &["user", "reset", "bob", "--app", "a", "--revoke", "--code"],
                r#"{"revoked":2,"username":"bob"}"#,
                0,
            ),
            // covers: app user reset
            one(
                "user reset held",
                &["user", "reset", "bob", "--to", "b@x"],
                r#"{"status":"held","request":{"id":"r1","summary":"reset bob"},"hint":"wait"}"#,
                2,
            ),
            // covers: app user reset --json
            one(
                "user reset --json",
                &["user", "reset", "bob", "--json"],
                r#"{"sentTo":"b@x"}"#,
                0,
            ),
            // covers: app user reset
            words("user reset with no name", &["user", "reset"], 1),
            // covers: app user email
            one(
                "user email",
                &["user", "email", "bob", "--email", "b@x"],
                r#"{"email":"b@x","username":"bob"}"#,
                0,
            ),
            // covers: app user email
            words("user email with no name", &["user", "email"], 1),
            // covers: app user full-name
            one(
                "user full-name",
                &["user", "full-name", "bob", "--name", "Bob B é"],
                r#"{"username":"bob","fullName":"Bob B é"}"#,
                0,
            ),
            // covers: app user full-name
            one(
                "user fullname cleared",
                &["user", "fullname", "bob"],
                r#"{"username":"bob"}"#,
                0,
            ),
            // covers: app user agent
            one(
                "user agent",
                &["user", "agent", "bob", "sales"],
                r#"{"username":"bob","agentWorkspace":"sales"}"#,
                0,
            ),
            // covers: app user agent
            one(
                "user agent --clear",
                &["user", "agent", "bob", "--clear"],
                r#"{"username":"bob"}"#,
                0,
            ),
            // covers: app user agent
            words(
                "user agent with both",
                &["user", "agent", "bob", "w", "--clear"],
                1,
            ),
            // covers: app user unlock
            one(
                "user unlock",
                &["user", "unlock", "bob"],
                r#"{"cleared":true,"username":"bob"}"#,
                0,
            ),
            // covers: app user unlock --json
            one(
                "user unlock --json",
                &["user", "unlock", "bob", "--json"],
                r#"{"cleared":1}"#,
                0,
            ),
            // covers: app user remove
            one(
                "user remove",
                &["user", "remove", "bob"],
                r#"{"success":true}"#,
                0,
            ),
            // covers: app user remove
            traceback(one(
                "user rm: an answer that is a list",
                &["user", "rm", "bob"],
                "[1]",
                1,
            )),
            // covers: app user rooms
            one(
                "user rooms on an App",
                &[
                    "user", "rooms", "bob", "--agent", "sales", "--agent", " ops ", "--app", "a",
                ],
                r#"{"app":"a","roomHandles":["sales"],"username":"bob","waitingForReset":{"rooms":["sales"],"ask":"ask"}}"#,
                0,
            ),
            // covers: app user rooms
            words(
                "user rooms --agent as the last word",
                &["user", "rooms", "bob", "--agent"],
                1,
            ),
            // covers: app user role
            one(
                "user role",
                &["user", "role", "bob", "staff"],
                r#"{"username":"bob","role_name":"staff"}"#,
                0,
            ),
            // covers: app user role
            words("user role with no role", &["user", "role", "bob"], 1),
            // covers: app user unassign
            one(
                "user unassign",
                &["user", "unassign", "bob"],
                r#"{"username":"bob"}"#,
                0,
            ),
            // covers: app user unassign
            one(
                "user unassign: a refusal",
                &["user", "unassign", "bob"],
                r#"{"error":{"code":"forbidden","hint":"Not yours"}}"#,
                3,
            ),
        ],
    );
}

#[test]
fn app_roles() {
    let rig = Rig::new(VERB, "roles");
    check(
        &rig,
        vec![
            // covers: app role
            one("role alone is the list", &["role"], ROLES, 0),
            // covers: app role create
            one(
                "role create",
                &["role", "create", "staff", "--agent", "sales", "--app", "a"],
                ACCESS,
                0,
            ),
            // covers: app role create
            words("role create with no name", &["role", "create"], 1),
            // covers: app role room
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
                ACCESS,
                0,
            ),
            // covers: app role room
            one(
                "role room held",
                &["role", "room", "staff", "sales", "--clear"],
                r#"{"status":"held","request":{"id":"r9","summary":"room"},"hint":"ask"}"#,
                2,
            ),
            // covers: app role room
            words("role room with no handle", &["role", "room", "staff"], 1),
            // covers: app role paths
            one(
                "role paths",
                &[
                    "role", "paths", "staff", "sales", "--read", "docs/,b/", "--hide", ".env",
                ],
                ACCESS,
                0,
            ),
            // covers: app role paths
            words(
                "role paths with nothing",
                &["role", "paths", "staff", "sales"],
                2,
            ),
            // covers: app role update
            one(
                "role update rooms",
                &["role", "update", "staff", "--agent", "sales"],
                ACCESS,
                0,
            ),
            // covers: app role update
            one(
                "role update --clear",
                &["role", "update", "staff", "--clear"],
                r#"{"name":"staff","roomAccess":[]}"#,
                0,
            ),
            // covers: app role remove
            one("role remove", &["roles", "remove", "staff"], r#"{}"#, 0),
            // covers: app role remove
            words("role remove with no name", &["role", "remove"], 1),
        ],
    );
}

#[test]
fn app_tokens() {
    let rig = Rig::new(VERB, "tokens");
    check(
        &rig,
        vec![
            // covers: app token create
            one(
                "token create",
                &[
                    "token",
                    "create",
                    "--name",
                    "web",
                    "--agent",
                    "sales",
                    "--agent",
                    "ops",
                    "--caps",
                    "thread:read, files:read",
                ],
                r#"{"id":"t1","name":"web","caps":["thread:read","files:read"],"roomHandles":["sales","ops"],"token":"k2skn_example"}"#,
                0,
            ),
            // covers: app token create
            words(
                "token create with no --agent",
                &["token", "create", "--name", "web"],
                2,
            ),
            // covers: app token create
            words(
                "token create --name as the last word",
                &["tokens", "create", "--name"],
                1,
            ),
            // covers: app token rooms
            one(
                "token rooms",
                &["token", "rooms", "t1", "--agent", "sales"],
                r#"{"id":"t1","roomHandles":["sales"]}"#,
                0,
            ),
            // covers: app token rooms
            one(
                "token rooms --clear",
                &["token", "rooms", "t1", "--clear"],
                r#"{"id":"t1","room_handles":[]}"#,
                0,
            ),
            // covers: app token rooms
            words("token rooms with no id", &["token", "rooms"], 1),
            // covers: app token revoke
            one(
                "token revoke",
                &["token", "revoke", "t1"],
                r#"{"success":true}"#,
                0,
            ),
            // covers: app token revoke
            one(
                "token revoke, none matched",
                &["token", "revoke", "t9"],
                r#"{"success":false}"#,
                0,
            ),
            // covers: app token revoke
            one(
                "token revoke: a refusal",
                &["token", "revoke", "t1"],
                r#"{"error":{"code":"owner_only","hint":"the owner's"}}"#,
                3,
            ),
            // covers: app token revoke
            words("token revoke with no id", &["token", "revoke"], 1),
        ],
    );
}

/// Help never acts: a write verb with `-h` or `--help` anywhere on the
/// line prints help and sends nothing (the router's `app)` arm).
#[test]
fn app_people_help_never_acts() {
    let rig = Rig::new(VERB, "peoplehelp");
    let rows: [&'static [&'static str]; 12] = [
        &["user", "invite", "bob", "--app", "-h"],
        &["user", "reset", "bob", "--help"],
        &["user", "image", "bob", "--clear", "-h"],
        &["user", "sessions", "bob", "--app", "a", "--revoke-all", "--help"],
        &["user", "unlock", "-h", "bob"],
        &["user", "remove", "bob", "--help"],
        &["user", "role", "bob", "staff", "-h"],
        &["role", "create", "staff", "--agent", "-h"],
        &["role", "paths", "staff", "sales", "--read", "a/", "--help"],
        &["role", "remove", "staff", "-h"],
        &["token", "create", "--name", "web", "--agent", "s", "-h"],
        &["token", "revoke", "--help", "t1"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
