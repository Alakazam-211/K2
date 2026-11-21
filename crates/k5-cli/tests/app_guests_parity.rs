//! `k5-cli app guest …`, `app grant …` (the kinds, copy-host, hooks and
//! the list words `cmd_skin` answers) and `app hydra|front-door --json`
//! against the shell CLI's (`cli/src/app-guests.sh`, `cmd_skin` in
//! `cli/src/app-wallet.sh`, `_app_grant_hooks` in
//! `cli/src/talk-and-workspace.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `guest` alone (the rooms page), an empty or unknown word.
//! - `guest rooms|ceiling|list`: each read and `--json`, a set (rooms,
//!   `-`, `--json` before the rooms), the usage lines, a value flag given
//!   last, a list above 256 KiB, a refusal, Python raising part way.
//! - `grant <app> --kind …`: guests-rooms (rooms, held for the owner,
//!   `--json`), guests-chat, `--revoke`, chat-key (set, revoke, a bad
//!   `--uploads`, the confirm with no vault socket, no service id, and in
//!   a session), the usage lines.
//! - `grant copy-host`: the copy, held, `--json`, the usage lines.
//! - `grant hooks`: status (and `--json`), log (a list above 256 KiB),
//!   retry, skip, reconcile, target, their usage lines, a refusal.
//! - `grant`, `grant add|rm|role|set`, `hydra --json`, `front-door
//!   --json` through `app`.
//! - No answer: nothing listening (curl 7).
//! - Help never acts: each write verb with `-h` or `--help` anywhere
//!   (both sides the shell's: the binary hands help forms over).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";
const ROOMS: &str = r#"{"username":"ann","app":"acme","roomHandles":["a","b"],"role":"staff","changed":true,"before":["x"],"beforeHandles":["x"]}"#;
const CEILING: &str = r#"{"app":"acme","appInfo":{"handle":"acme-h","name":"acme"},"ownRoom":{"handle":"acme-room"},"granted":true,"grantRooms":[{"handle":"r1"},{"handle":"r2"}]}"#;
const LIST: &str = r#"{"app":"acme","guests":[{"username":"ann","roomHandles":["a"],"role":"staff"},{"username":"bo é","enabled":false,"agent":true}],"nextCursor":"bo"}"#;
const GRANTED: &str =
    r#"{"app":{"handle":"acme-h","serviceId":"s1"},"grant":{"handsOut":["acme-room","r1"]}}"#;
const HELD: &str = r#"{"status":"held","request":{"id":"r9","summary":"guests-rooms on acme"},"hint":"The owner decides.","reasons":["another workspace's room"],"card":{"where":"your Thread"}}"#;
const CHAT_KEY: &str = r#"{"app":{"handle":"acme-h"},"key":{"caps":["post","read"],"maxLevel":"member","maxMembers":500,"uploads":true}}"#;
const CONFIRM: &str =
    r#"{"error":{"code":"vault_confirm_required","hint":"confirm","serviceId":"svc-1"}}"#;
const COPIED: &str = r#"{"dryRun":true,"added":["ann","bo"],"appInfo":{"handle":"acme-h"},"keptExistingCount":1,"note":"done","keptExisting":[{"username":"cy","enabled":true}],"skippedOtherAppRole":["dee"],"skippedOtherWorkspaceAgent":["eve"]}"#;
const HOOKS: &str = r#"{"targets":[{"kind":"chat","enabled":true,"healthy":false,"lastError":"down","socket":"/run/sew.sock","lastReconcile":{"provisioned":2,"updated":0},"lastReconcileAt":1700000000,"lastChange":{"deprovisioned":1},"lastChangeAt":1700000100.5,"reconcileHistory":[{"at":1699999000,"provisioned":1},{"at":1700000000}]}],"outbox":{"pending":3,"blocked":1,"held":0,"oldestPendingAgeSecs":42},"blocked":[{"id":"o1","op":"add","subjectKey":"p:ann","lastError":"no"}],"held":[{"id":"o2","op":"rm","subjectKey":"p:bo"}]}"#;
const HOOK_LOG: &str = r#"{"rows":[{"id":"o1","state":"done","op":"provision","subjectKey":"p:ann","actor":"owner","cause":"grant","attempts":1},{"id":"o2","state":"blocked","op":"rm","subjectKey":"p:bo","actor":"agent","cause":"revoke","attempts":3,"lastError":"sew said no"}]}"#;

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
fn guest_reads_and_sets() {
    let rig = Rig::new(VERB, "guest");
    check(
        &rig,
        vec![
            // covers: app guest rooms
            one(
                "guest rooms",
                &["guest", "rooms", "acme", "ann"],
                ok(ROOMS),
                0,
            ),
            // covers: app guest rooms --json
            one(
                "guest rooms --json",
                &["guest", "rooms", "--json", "acme", "ann"],
                ok(ROOMS),
                0,
            ),
            // covers: app guest rooms
            one(
                "guest rooms set",
                &["guest", "rooms", "acme", "ann", "set", " a, ,b "],
                ok(
                    r#"{"username":"ann","app":"acme","roomHandles":["a","b"],"changed":false,"before":[]}"#,
                ),
                0,
            ),
            // covers: app guest rooms
            one(
                "guest rooms set -: no rooms, disabled",
                &["guest", "rooms", "acme", "ann", "set", "-"],
                ok(r#"{"username":"ann","app":"acme","enabled":false}"#),
                0,
            ),
            // covers: app guest rooms --json
            one(
                "guest rooms set --json then -",
                &["guest", "rooms", "acme", "ann", "set", "--json", "-"],
                ok(ROOMS),
                0,
            ),
            // native: app guest
            words("guest alone: the rooms page", &["guest"], 0),
            words("guest, an empty word", &["guest", ""], 0),
            case(
                "guest alone: HOOK_SOCK set and no passport",
                &["guest"],
                &[("K2_HOOK_SOCK", "{sock}")],
                vec![],
                owner_and_port,
                3,
                0,
            ),
            case(
                "guest alone, no daemon: the connection gate",
                &["guest"],
                &[],
                vec![],
                nothing,
                1,
                0,
            ),
            words("guest, an unknown word", &["guest", "nope"], 2),
            // covers: app guest rooms
            words("guest rooms: no user", &["guest", "rooms", "acme"], 2),
            // covers: app guest rooms
            words(
                "guest rooms set: no rooms",
                &["guest", "rooms", "acme", "ann", "set"],
                2,
            ),
            // covers: app guest rooms
            words(
                "guest rooms: a third word",
                &["guest", "rooms", "a", "b", "c"],
                2,
            ),
            // covers: app guest rooms
            words("guest rooms: a flag", &["guest", "rooms", "a", "-x"], 2),
            // covers: app guest rooms
            words("guest: an unknown word", &["guest", "nope"], 2),
            // covers: app guest rooms
            traceback(one(
                "guest rooms: a role that is a number",
                &["guest", "rooms", "acme", "ann"],
                ok(r#"{"role":5}"#),
                1,
            )),
            // covers: app guest rooms
            one(
                "guest rooms: a refusal with its fix line",
                &["guest", "rooms", "acme", "ann"],
                refused(),
                3,
            ),
            // covers: app guest ceiling
            one(
                "guest ceiling",
                &["guest", "ceiling", "acme"],
                ok(CEILING),
                0,
            ),
            // covers: app guest ceiling
            one(
                "guest ceiling: no grant",
                &["guest", "ceiling", "acme"],
                ok(r#"{"app":"acme","appInfo":{"name":"acme"},"granted":false}"#),
                0,
            ),
            // covers: app guest ceiling --json
            one(
                "guest ceiling --json",
                &["guest", "ceiling", "acme", "--json"],
                ok(CEILING),
                0,
            ),
            // covers: app guest ceiling
            traceback(one(
                "guest ceiling: an own room with no handle",
                &["guest", "ceiling", "acme"],
                ok(r#"{"ownRoom":{"x":1},"granted":true}"#),
                1,
            )),
            // covers: app guest ceiling
            words("guest ceiling: no App", &["guest", "ceiling"], 2),
            // covers: app guest list
            one(
                "guest list",
                &["guest", "list", "acme", "--limit", "2", "--cursor", "a b"],
                ok(LIST),
                0,
            ),
            // covers: app guest list
            one(
                "guest list: none",
                &["guest", "list", "acme"],
                ok(r#"{"app":"acme","guests":[]}"#),
                0,
            ),
            // covers: app guest list --json
            one(
                "guest list --json",
                &["guest", "list", "--json", "acme"],
                ok(LIST),
                0,
            ),
            // covers: app guest list
            words(
                "guest list: --cursor last",
                &["guest", "list", "acme", "--cursor"],
                2,
            ),
            // covers: app guest list
            words("guest list: a flag", &["guest", "list", "acme", "-z"], 2),
        ],
    );
}

/// One answer above 256 KiB: thousands of guests.
#[test]
fn guest_list_a_large_list() {
    let rig = Rig::new(VERB, "guestbig");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"username":"guest{i:05} é","roomHandles":["room-{i}","sales","ops"],"role":"staff-{i}","enabled":{}}}"#,
                i % 7 != 0
            )
        })
        .collect();
    let big = format!(r#"{{"app":"acme","guests":[{}]}}"#, rows.join(","));
    assert!(
        big.len() > 256 * 1024,
        "the large list is {} bytes",
        big.len()
    );
    // covers: app guest list
    rig.check(&one(
        "a large guest list, above 256 KiB",
        &["guest", "list", "acme"],
        Reply::Answer(200, big),
        0,
    ));
}

#[test]
fn grant_kinds() {
    let rig = Rig::new(VERB, "kinds");
    check(
        &rig,
        vec![
            // covers: app grant guests-rooms
            one(
                "guests-rooms",
                &[
                    "grant",
                    "acme",
                    "--kind",
                    "guests-rooms",
                    "--room",
                    "r1",
                    "--room=r2",
                    "--agent",
                    " ",
                ],
                ok(GRANTED),
                0,
            ),
            // covers: app grant guests-rooms
            one(
                "guests-rooms, no rooms: its own room",
                &["grant", "--kind=guests-rooms", "acme"],
                ok(r#"{"app":{"serviceId":"s1"},"grant":{}}"#),
                0,
            ),
            // covers: app grant guests-rooms --json
            one(
                "guests-rooms --json",
                &["grant", "acme", "--kind", "guests-rooms", "--json"],
                ok(GRANTED),
                0,
            ),
            // covers: app grant guests-rooms
            one(
                "guests-rooms: held for the owner",
                &["grant", "acme", "--kind", "guests-rooms", "--room", "x"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // covers: app grant guests-rooms --json
            one(
                "guests-rooms --json: held for the owner",
                &["grant", "acme", "--kind", "guests-rooms", "--json"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // covers: app grant guests-rooms
            traceback(one(
                "guests-rooms: held, a card error that is words",
                &["grant", "acme", "--kind", "guests-rooms"],
                Reply::Answer(202, r#"{"card":{"error":"x"}}"#.into()),
                1,
            )),
            // covers: app grant guests-rooms
            one(
                "guests-rooms: a refusal",
                &["grant", "acme", "--kind", "guests-rooms"],
                refused(),
                3,
            ),
            // covers: app grant guests-rooms
            one(
                "guests-rooms --revoke",
                &["grant", "acme", "--kind", "guests-rooms", "--revoke"],
                ok(r#"{"app":{"handle":"acme-h"},"revoked":false}"#),
                0,
            ),
            // covers: app grant guests-rooms
            traceback(one(
                "guests-rooms: no app in the answer",
                &["grant", "acme", "--kind", "guests-rooms"],
                ok(r#"{"grant":{}}"#),
                1,
            )),
            // covers: app grant guests-rooms
            one(
                "guests-chat",
                &["grant", "acme", "--kind", "guests-chat", "--max", "050"],
                ok(
                    r#"{"app":{"serviceId":"s1","name":"acme"},"grant":{"maxMembers":50},"warning":"sew is old"}"#,
                ),
                0,
            ),
            // covers: app grant guests-rooms
            words("a kind but no App", &["grant", "--kind", "guests-rooms"], 2),
            // covers: app grant guests-rooms
            words("--kind given last", &["grant", "acme", "--kind"], 2),
            // covers: app grant guests-rooms
            words("an unknown flag", &["grant", "acme", "--zap"], 2),
            // covers: app grant chat-key
            one(
                "chat-key",
                &[
                    "grant",
                    "acme",
                    "--kind",
                    "chat-key",
                    "--caps",
                    "post, read",
                    "--max-level",
                    "member",
                    "--uploads",
                    "yes",
                    "--max",
                    "500",
                ],
                ok(CHAT_KEY),
                0,
            ),
            // covers: app grant chat-key
            one(
                "chat-key --json, uploads off",
                &[
                    "grant",
                    "acme",
                    "--kind",
                    "chat-key",
                    "--uploads",
                    "off",
                    "--json",
                ],
                ok(CHAT_KEY),
                0,
            ),
            // covers: app grant chat-key
            one(
                "chat-key --revoke",
                &["grant", "acme", "--kind", "chat-key", "--revoke"],
                ok(r#"{"app":{"serviceId":"s1"},"revoked":true,"sessionsEnded":false}"#),
                0,
            ),
            // covers: app grant chat-key
            one(
                "chat-key --revoke: already off",
                &["grant", "acme", "--kind", "chat-key", "--revoke"],
                ok(r#"{"app":{"handle":"h"},"revoked":false}"#),
                0,
            ),
            // covers: app grant chat-key
            words(
                "chat-key: a bad --uploads",
                &["grant", "acme", "--kind", "chat-key", "--uploads", "maybe"],
                2,
            ),
            // covers: app grant chat-key
            one(
                "chat-key: a confirm, no vault socket",
                &["grant", "acme", "--kind", "chat-key"],
                Reply::Answer(403, CONFIRM.into()),
                3,
            ),
            // covers: app grant chat-key
            one(
                "chat-key: a confirm with no service id",
                &["grant", "acme", "--kind", "chat-key"],
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"vault_confirm_required","hint":"confirm"}}"#.into(),
                ),
                3,
            ),
            // covers: app grant chat-key
            Case {
                env: with_passport(&[]),
                ..case(
                    "chat-key: in a session, no confirm",
                    &["grant", "acme", "--kind", "chat-key"],
                    &[],
                    vec![Reply::Answer(403, CONFIRM.into())],
                    nothing,
                    3,
                    1,
                )
            },
        ],
    );
}

#[test]
fn grant_copy_host() {
    let rig = Rig::new(VERB, "copyhost");
    check(
        &rig,
        vec![
            // covers: app grant copy-host
            one(
                "copy-host --dry-run",
                &["grant", "copy-host", "acme", "other", "--dry-run"],
                ok(COPIED),
                0,
            ),
            // covers: app grant copy-host
            one(
                "copyhost: a count, no list",
                &["grants", "copyhost", "acme"],
                ok(r#"{"app":"acme","addedCount":"3"}"#),
                0,
            ),
            // covers: app grant copy-host --json
            one(
                "copy-host --json",
                &["grant", "copy-host", "acme", "--json"],
                ok(COPIED),
                0,
            ),
            // covers: app grant copy-host
            one(
                "copy-host: held for the owner",
                &["grant", "copy-host", "acme"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // covers: app grant copy-host --json
            one(
                "copy-host --json: held for the owner",
                &["grant", "copy-host", "acme", "--json"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // covers: app grant copy-host
            one(
                "copy-host: a refusal",
                &["grant", "copy-host", "acme"],
                refused(),
                3,
            ),
            // covers: app grant copy-host
            traceback(one(
                "copy-host: a count that is not a number",
                &["grant", "copy-host", "acme"],
                ok(r#"{"addedCount":"many"}"#),
                1,
            )),
            // covers: app grant copy-host
            words("copy-host: no App", &["grant", "copy-host"], 1),
            // covers: app grant copy-host
            words("copy-host: a flag", &["grant", "copy-host", "a", "--x"], 1),
        ],
    );
}

#[test]
fn grant_hooks() {
    let rig = Rig::new(VERB, "hooks");
    let rows: Vec<String> = (0..2500)
        .map(|i| {
            format!(
                r#"{{"id":"o{i:05}","state":"pending","op":"provision","subjectKey":"p:user{i} é","actor":"owner","cause":"grant","attempts":{i}}}"#
            )
        })
        .collect();
    let big = format!(r#"{{"rows":[{}]}}"#, rows.join(","));
    assert!(
        big.len() > 256 * 1024,
        "the large log is {} bytes",
        big.len()
    );
    check(
        &rig,
        vec![
            // covers: app grant hooks
            one("hooks", &["grant", "hooks"], ok(HOOKS), 0),
            // covers: app grant hooks
            one(
                "hooks status: none",
                &["grant", "hook", "status"],
                ok(r#"{"targets":[]}"#),
                0,
            ),
            // covers: app grant hooks --json
            one("hooks --json", &["grant", "hooks", "--json"], ok(HOOKS), 0),
            // covers: app grant hooks --json
            one(
                "hooks status --json",
                &["grants", "hooks", "status", "--json"],
                ok(HOOKS),
                0,
            ),
            // covers: app grant hooks
            traceback(one(
                "hooks: a time that is words",
                &["grant", "hooks"],
                ok(r#"{"targets":[{"lastReconcile":{},"lastReconcileAt":"x"}]}"#),
                1,
            )),
            // covers: app grant hooks
            one("hooks: a refusal", &["grant", "hooks"], refused(), 3),
            // covers: app grant hooks
            one(
                "hooks log",
                &["grant", "hooks", "log", "--subject", "p:ann", "--limit=5"],
                ok(HOOK_LOG),
                0,
            ),
            // covers: app grant hooks
            one(
                "hooks log: none",
                &["grant", "hooks", "log", "--limit"],
                ok(r#"{"rows":[]}"#),
                0,
            ),
            // covers: app grant hooks --json
            one(
                "hooks log --json",
                &["grant", "hooks", "log", "--json"],
                ok(HOOK_LOG),
                0,
            ),
            // covers: app grant hooks
            one(
                "a large hooks log, above 256 KiB",
                &["grant", "hooks", "log"],
                Reply::Answer(200, big),
                0,
            ),
            // covers: app grant hooks
            words("hooks log: a word", &["grant", "hooks", "log", "x"], 1),
            // covers: app grant hooks
            one(
                "hooks retry",
                &["grant", "hooks", "retry", "o1 é"],
                ok(r#"{"id":"o1","state":"pending"}"#),
                0,
            ),
            // covers: app grant hooks
            one(
                "hooks skip",
                &["grant", "hooks", "skip", "o1"],
                ok(r#"{"id":"o1","state":"skipped"}"#),
                0,
            ),
            // covers: app grant hooks
            words("hooks retry: no id", &["grant", "hooks", "retry"], 1),
            // covers: app grant hooks
            one(
                "hooks reconcile",
                &["grant", "hooks", "reconcile"],
                ok("{}"),
                0,
            ),
            // covers: app grant hooks
            one(
                "hooks target",
                &[
                    "grant",
                    "hooks",
                    "target",
                    "chat",
                    "--off",
                    "--socket",
                    "/run/s.sock",
                    "--token-file=/t",
                ],
                ok(r#"{"kind":"chat","enabled":false,"socket":"/run/s.sock"}"#),
                0,
            ),
            // covers: app grant hooks
            words(
                "hooks target: no switch",
                &["grant", "hooks", "target", "chat"],
                1,
            ),
            // covers: app grant hooks
            words("hooks: an unknown action", &["grant", "hooks", "nope"], 1),
        ],
    );
}

/// `cmd_skin`'s grant words, `hydra` and `front-door` through `app`.
#[test]
fn grant_words_hydra_and_front_door_through_app() {
    let rig = Rig::new(VERB, "skinw");
    check(
        &rig,
        vec![
            // covers: app grant
            one(
                "grant alone is the list",
                &["grant"],
                ok(
                    r#"{"grants":[{"id":"g1","subjectKind":"principal","subjectId":"bob","kind":"app","targetId":"site","roleId":"staff"}]}"#,
                ),
                0,
            ),
            // covers: app grant add
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
                    "app",
                    "--target",
                    "site",
                    "--role",
                    "staff",
                ],
                ok(
                    r#"{"id":"g3","subjectKind":"principal","subjectId":"bob","kind":"app","targetId":"site"}"#,
                ),
                0,
            ),
            // covers: app grant add
            words(
                "grant add: missing words",
                &["grant", "add", "--kind", "app"],
                1,
            ),
            // covers: app grant rm
            one(
                "grant rm",
                &["grant", "rm", "--id", "g1"],
                ok(r#"{"success":true}"#),
                0,
            ),
            // covers: app grant rm
            one(
                "grant rm: a refusal",
                &["grant", "remove", "--id", "g1"],
                Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"No"}}"#.into()),
                3,
            ),
            // covers: app grant role
            one(
                "grant role",
                &["grant", "role", "g1", "--role", "staff"],
                ok(r#"{"id":"g1","changed":true,"subjectId":"bob","targetId":"site"}"#),
                0,
            ),
            // covers: app grant set
            one(
                "grant set",
                &["grant", "set", "--id", "g1", "--scope", "member"],
                ok(r#"{"id":"g1","scope":"member"}"#),
                0,
            ),
            // covers: app hydra --json
            one(
                "hydra --json",
                &["hydra", "--json"],
                ok(r#"{"supported":true,"enabled":false}"#),
                0,
            ),
            // covers: app front-door --json
            one(
                "front-door --json",
                &["front-door", "--json"],
                ok(r#"{"mode":"connect","url":"https://x"}"#),
                0,
            ),
        ],
    );
}

#[test]
fn nothing_listening() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    check(
        &rig,
        vec![
            // covers: app guest list
            case(
                "guest list: nothing listens (7)",
                &["guest", "list", "acme"],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                3,
                0,
            ),
            // covers: app grant chat-key
            case(
                "chat-key: nothing listens (7)",
                &["grant", "acme", "--kind", "chat-key"],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                3,
                0,
            ),
            // covers: app grant hooks
            case(
                "hooks: nothing listens (7)",
                &["grant", "hooks"],
                &[("K2_PORT", port.as_str())],
                vec![],
                owner_only,
                7,
                0,
            ),
        ],
    );
}

/// Help never acts (K2's rule): the write verbs with `-h` or `--help`
/// anywhere print help and send nothing. The binary hands help forms to
/// the shell, so both sides are the shell's here.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    check(
        &rig,
        vec![
            shell(words(
                "guest rooms set -h",
                &["guest", "rooms", "acme", "ann", "-h", "set", "a"],
                0,
            )),
            shell(words(
                "grant kind --help",
                &["grant", "acme", "--kind", "guests-rooms", "--help"],
                0,
            )),
            shell(words(
                "grant chat-key -h",
                &["grant", "acme", "-h", "--kind", "chat-key"],
                0,
            )),
            shell(words(
                "copy-host -h",
                &["grant", "copy-host", "acme", "-h"],
                0,
            )),
            shell(words(
                "hooks retry --help",
                &["grant", "hooks", "retry", "o1", "--help"],
                0,
            )),
            shell(words(
                "hooks reconcile -h",
                &["grant", "hooks", "reconcile", "-h"],
                0,
            )),
            shell(words(
                "hooks target -h",
                &["grant", "hooks", "target", "chat", "--on", "-h"],
                0,
            )),
        ],
    );
}
