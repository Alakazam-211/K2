//! `k5-cli app warnings|clean-mode|key|keys|label|test-guests|move-plan`,
//! the pages `surfaces|door|grace`, `requests|quota|token rotate`,
//! `grants <app>`, `access` and `invite-settings`, against the shell
//! CLI's (`cli/src/app-updates.sh`, `app-door.sh`, `app-grace.sh`,
//! `app-recording.sh`, `app-guests.sh`, `app-auth.sh`, `app-people.sh`),
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - Each read: its lines, `--json`, its usage line, an unknown flag, a
//!   value flag given last, Python's raise part way, a refusal.
//! - Each change: done, held for the owner (exit 2, and `--json`), a
//!   refusal; `key mint`'s confirm with no vault socket, with no service
//!   id, and in a session (no confirm); `token rotate` never in a
//!   session.
//! - `access`: the read, `--json` (its `access` as `json.dumps` writes
//!   it), each switch in the shell's order, `--users` checked after the
//!   switches went, the cores, `--open`, the read again failing.
//! - A large list (`keys`); no daemon; help never acts; the brand's name
//!   in the lines.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// One answer, the owner on disk.
fn one(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Case {
    case(name, args, &[], vec![reply], owner_and_port, code, 1)
}

/// Words that stop before anything is sent.
fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
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

const HELD: &str = r#"{"status":"held","request":{"id":"r9","summary":"label take é"},"hint":"The owner decides.","reasons":["another workspace's label"],"card":{"where":"your Thread"}}"#;
const WARNINGS: &str = r#"{"services":[{"app":{"handle":"ws/site","envMode":"inherit"},"readyForCutover":false,"breaks":2,"warnings":[{"severity":"breaks","message":"reads ~/.k2 é","fix":"use the pass"},{"severity":"expected","message":"port"},{"severity":"other","message":"x"}],"scan":{"truncated":true,"filesScanned":900}},{"app":{"name":"api","cleanMode":true},"readyForCutover":true,"ownerTokenAttribution":"off"}]}"#;
const KEYS: &str = r#"{"keys":[{"id":"k1","state":"live","label":"a label that runs past twenty-four","scopes":["users","rooms"],"rooms":[]},{"id":"k2","state":"revoked","scopes":[]}]}"#;
const MINTED: &str = r#"{"id":"k3","app":{"handle":"acme-h"},"scopes":["users","push"],"routes":"/v1/users, /v1/push","delivered":{"vaultSecret":"APP_KEY","file":"app-key","note":"read it at start"},"key":"k2ak_secret_shown_once"}"#;
const CONFIRM: &str =
    r#"{"error":{"code":"vault_confirm_required","hint":"confirm","serviceId":"svc-1"}}"#;
const PLAN: &str = r#"{"service":{"workspaceHandle":"ws","name":"site","port":8080},"app":{"id":"site","installed":false},"steps":[{"state":"done","who":"owner","title":"box prep","detail":"ready"},{"state":"todo","who":"agent","title":"copy-host","detail":"users to copy","users":["ann","bo"],"command":"k2 app grant copy-host site"},{"state":"blocked","who":"owner","title":"guests-rooms","detail":"waits","roomHandles":["r1"],"command":"k2 app grant site --kind guests-rooms"},{"state":"n/a","who":"agent","title":"data","detail":"none"},{"state":"odd","who":"x","title":"y","detail":"z"}],"ownerStepsTodo":["guests-rooms"],"request":"k2 app move-plan site --request"}"#;
const REQUESTS: &str = r#"{"app":"acme","totals":{"requests":3,"ok":1,"clientErrors":1,"serverErrors":1,"k2Answered":1,"closedEarly":1},"requests":[{"atMs":1700000000123,"method":"GET","path":"/a/very/long/path/that/runs/past/thirty/four/columns","status":200,"bytesIn":0,"bytesOut":20480,"totalMs":12},{"atMs":1700000001000,"method":"POST","path":"/up","status":502,"k2Code":"upstream_down","bytesIn":2048,"bytesOut":5000000,"totalMs":3000},{"atMs":1700000002000,"ws":"upgrade","closedEarly":"client","bytesIn":null,"totalMs":null}],"keep":{"days":7,"rowsPerApp":10000},"dropped":4}"#;
const QUOTA: &str = r#"{"app":"acme","quotas":{"rps":5,"burst":10,"sockets":2,"bodyBytes":8388608,"uploadBytes":104857600,"subscriptions":4},"override":{"burst":10,"uploadBytes":1},"live":{"connections":1,"subscriptions":0},"raised":["burst","uploads"]}"#;
const ROTATED: &str = r#"{"app":"acme","pass":"k2rp_the_pass_shown_once","prefix":"k2rp_th","caps":["push"],"rooms":["r1","r2"],"expiresAt":1700000000,"revoked":1,"withheld":["room.write"],"note":"keep it safe é"}"#;
const GRANTS: &str = r#"{"app":{"handle":"acme-h","name":"acme"},"grants":[{"kind":"chat-key","enabled":false,"caps":["post","read"],"maxLevel":"member","maxMembers":50,"uploads":true,"grantedBy":"owner","grantedAt":1700000000},{"kind":"guests-chat","maxMembers":9,"grantedBy":"ann","grantedAt":1700000100},{"kind":"guests-rooms","handsOut":["r1","r2"],"grantedBy":"owner","grantedAt":1700000200}]}"#;
const LIST: &str = r#"{"apps":[],"access":{"workspace":"ws","buildAndRun":true,"manageEndToEnd":false,"manageUsers":true,"coreList":[{"core":"db","label":"Database","ticked":true},{"core":"mail","ticked":false,"later":"after the mail core ships"}]}}"#;
const SET: &str = r#"{"workspace":"ws","toggle":"apps_build","value":true}"#;

#[test]
fn the_cross_over_reads_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "grread");
    let dead = dead_port();
    check(
        &rig,
        vec![
            // native: app warnings
            one("warnings", &["warnings"], ok(WARNINGS), 0),
            // native: app warnings
            one(
                "warnings, one App, none to check",
                &["warnings", "acme"],
                ok(r#"{"services":[],"ownerTokenAttribution":"unsupported"}"#),
                0,
            ),
            // native: app warnings
            one(
                "warnings, one report",
                &["warnings", "acme"],
                ok(r#"{"app":{"handle":"h"},"breaks":1,"ownerTokenAttribution":"on"}"#),
                0,
            ),
            // native: app warnings --json
            one("warnings --json", &["warnings", "--json"], ok(WARNINGS), 0),
            // native: app warnings
            stop("warnings: an unknown flag", &["warnings", "--zap"], 2),
            // native: app warnings
            traceback(one(
                "warnings: breaks that is a word",
                &["warnings"],
                ok(r#"{"app":{"handle":"h"},"breaks":"two"}"#),
                1,
            )),
            // native: app keys
            one("keys", &["keys"], ok(KEYS), 0),
            // native: app keys
            one("keys, none", &["keys", "acme"], ok(r#"{"keys":[]}"#), 0),
            // native: app keys --json
            one("keys --json", &["keys", "acme", "--json"], ok(KEYS), 0),
            // native: app key list
            one("key list", &["key", "list", "acme"], ok(KEYS), 0),
            // native: app key ls
            one("key ls --json", &["key", "ls", "--json"], ok(KEYS), 0),
            // native: app keys
            traceback(one(
                "keys: a key with no id",
                &["keys"],
                ok(r#"{"keys":[{"state":"live"}]}"#),
                1,
            )),
            // native: app test-guests
            one(
                "test-guests",
                &["test-guests", "acme"],
                ok(
                    r#"{"app":"acme","max":5,"testGuests":[{"username":"t1","expiresAt":1700000000,"createdBy":"agent é"},{"username":"t2","expired":true}]}"#,
                ),
                0,
            ),
            // native: app test-guest
            one(
                "test-guest: one",
                &["test-guest", "acme"],
                ok(
                    r#"{"app":"acme","testGuests":[{"username":"t1","expired":true,"createdBy":"o"}]}"#,
                ),
                0,
            ),
            // native: app test-guests --json
            one(
                "test-guests --json",
                &["test-guests", "--json", "acme"],
                ok(r#"{"app":"acme"}"#),
                0,
            ),
            // native: app test-guests
            stop("test-guests: no App", &["test-guests", "--json"], 2),
            // native: app test-guests
            traceback(one(
                "test-guests: no app in the answer",
                &["test-guests", "acme"],
                ok(r#"{"testGuests":[]}"#),
                1,
            )),
            // native: app move-plan
            one("move-plan", &["move-plan", "site"], ok(PLAN), 0),
            // native: app move-plan
            one(
                "move-plan --app, done",
                &["move-plan", "site", "--app", "acme é"],
                ok(
                    r#"{"service":{"workspaceHandle":"ws","name":"site","port":1},"app":{"id":"acme","installed":true},"steps":[],"done":true}"#,
                ),
                0,
            ),
            // native: app move-plan --json
            one(
                "move-plan --json",
                &["move-plan", "site", "--json"],
                ok(PLAN),
                0,
            ),
            // native: app move-plan
            stop("move-plan: no service", &["move-plan", "--request"], 2),
            // native: app move-plan
            stop("move-plan: --app last", &["move-plan", "site", "--app"], 1),
            // native: app move-plan
            traceback(one(
                "move-plan: no steps",
                &["move-plan", "site"],
                ok(
                    r#"{"service":{"workspaceHandle":"ws","name":"s","port":1},"app":{"id":"a","installed":true}}"#,
                ),
                1,
            )),
            // native: app requests
            one("requests", &["requests", "acme"], ok(REQUESTS), 0),
            // native: app requests
            one(
                "requests, every filter, none",
                &[
                    "requests", "acme", "--since", "1h", "--until", "10m", "--status", "5xx",
                    "--path", "/a b", "--method", "POST", "--limit", "5",
                ],
                ok(r#"{"app":"acme","totals":{"requests":1}}"#),
                0,
            ),
            // native: app requests --json
            one(
                "requests --json",
                &["requests", "--json", "acme"],
                ok(REQUESTS),
                0,
            ),
            // native: app requests
            stop("requests: no App", &["requests"], 2),
            // native: app requests
            stop(
                "requests: --since last",
                &["requests", "acme", "--since"],
                1,
            ),
            // native: app requests
            traceback(one(
                "requests: a size that is a word",
                &["requests", "acme"],
                ok(r#"{"app":"a","requests":[{"atMs":0,"bytesIn":"big"}]}"#),
                1,
            )),
            // native: app quota
            one("quota", &["quota", "acme"], ok(QUOTA), 0),
            // native: app quotas
            one(
                "quotas, no live",
                &["quotas", "acme"],
                ok(r#"{"app":"acme","quotas":{}}"#),
                0,
            ),
            // native: app quota --json
            one("quota --json", &["quota", "acme", "--json"], ok(QUOTA), 0),
            // native: app quota
            stop("quota: no App", &["quota", "--reset"], 2),
            // native: app quota
            stop(
                "quota: a number that isn't",
                &["quota", "acme", "--burst", "1.5"],
                2,
            ),
            // native: app grants
            one("grants", &["grants", "acme"], ok(GRANTS), 0),
            // native: app grants
            one(
                "grants, none",
                &["grants", "acme"],
                ok(r#"{"app":{"handle":"acme-h","name":"acme"},"grants":[]}"#),
                0,
            ),
            // native: app grants --json
            one(
                "grants --json",
                &["grants", "--json", "acme"],
                ok(GRANTS),
                0,
            ),
            // native: app grants
            stop("grants: an unknown flag", &["grants", "acme", "--zap"], 2),
            // native: app grants
            traceback(one(
                "grants: a grant with no kind",
                &["grants", "acme"],
                ok(r#"{"app":{"handle":"h"},"grants":[{}]}"#),
                1,
            )),
            // native: app surfaces
            stop("surfaces: the page", &["surfaces", "anything"], 0),
            // native: app door
            stop("door: the page", &["door"], 0),
            // native: app grace
            stop("grace: the page", &["grace", "x", "y"], 0),
            // native: app crossover
            stop("crossover: the page", &["crossover"], 0),
            // native: app keys
            one(
                "a refusal with its fix line",
                &["keys", "acme"],
                refused(),
                3,
            ),
            // native: app quota
            case(
                "quota: no daemon",
                &["quota", "acme"],
                &[("K2_PORT", dead.as_str())],
                vec![],
                owner_only,
                3,
                0,
            ),
        ],
    );
}

#[test]
fn the_cross_over_changes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "grset");
    check(
        &rig,
        vec![
            // native: app clean-mode on
            one(
                "clean-mode on --restart",
                &["clean-mode", "acme", "on", "--restart"],
                ok(r#"{"app":{"handle":"ws/acme"},"cleanMode":true,"envMode":"clean","restart":"restarted","ownerTokenAttribution":"off","ownerTokenAttributionNote":"Linux only"}"#),
                0,
            ),
            // native: app clean-mode off
            one(
                "clean-mode off",
                &["clean-mode", "off", "acme"],
                ok(r#"{"app":{"handle":"h"},"envMode":"inherit","restart":"next start"}"#),
                0,
            ),
            // native: app clean-mode --json
            one(
                "clean-mode, held, --json",
                &["clean-mode", "acme", "on", "--json"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app clean-mode
            one(
                "clean-mode, held",
                &["clean-mode", "acme", "off"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app clean-mode
            stop("clean-mode: no on or off", &["clean-mode", "acme"], 2),
            // native: app clean-mode
            one("clean-mode: refused", &["clean-mode", "acme", "on"], refused(), 3),
            // native: app clean-mode
            traceback(one(
                "clean-mode: no app",
                &["clean-mode", "acme", "on"],
                ok(r#"{"cleanMode":true}"#),
                1,
            )),
            // native: app key revoke
            one(
                "key revoke",
                &["key", "revoke", "k1"],
                ok(r#"{"revoked":true,"id":"k1","app":{"handle":"acme-h"}}"#),
                0,
            ),
            // native: app key rm
            one(
                "key rm, already, a --json word",
                &["key", "rm", "k1", "--json"],
                ok(r#"{"id":"k1","app":{"handle":"acme-h"}}"#),
                0,
            ),
            // native: app key revoke
            one(
                "key revoke: held",
                &["key", "revoke", "k1"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app key revoke
            stop("key revoke: no id", &["key", "revoke"], 2),
            // native: app key
            stop("key: the page on stderr", &["key"], 2),
            // native: app key
            stop("key: a word it doesn't take", &["key", "make"], 2),
            // native: app key mint
            one(
                "key mint --show",
                &[
                    "key", "mint", "acme", "--scopes", " users, push,", "--rooms", "r1",
                    "--role", "staff", "--label", "L é", "--show",
                ],
                ok(MINTED),
                0,
            ),
            // native: app key mint
            one(
                "key mint, in the vault",
                &["key", "mint", "--scope", "users", "acme"],
                ok(r#"{"id":"k4","app":{"handle":"h"},"scopes":["users"],"routes":"/v1/users"}"#),
                0,
            ),
            // native: app key mint --json
            one(
                "key mint --json",
                &["key", "mint", "acme", "--scopes", "users", "--json"],
                ok(MINTED),
                0,
            ),
            // native: app key mint
            one(
                "key mint: held",
                &["key", "mint", "acme", "--scopes", "users"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app key mint
            one(
                "key mint: a confirm, no vault socket",
                &["key", "mint", "acme", "--scopes", "users"],
                Reply::Answer(403, CONFIRM.into()),
                3,
            ),
            // native: app key mint
            one(
                "key mint: a confirm with no service id",
                &["key", "mint", "acme", "--scopes", "users"],
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"vault_confirm_required","hint":"confirm"}}"#.into(),
                ),
                3,
            ),
            // native: app key mint
            Case {
                env: with_passport(&[]),
                ..case(
                    "key mint: in a session, no confirm",
                    &["key", "mint", "acme", "--scopes", "users"],
                    &[],
                    vec![Reply::Answer(403, CONFIRM.into())],
                    nothing,
                    3,
                    1,
                )
            },
            // native: app key mint
            stop("key mint: no scopes", &["key", "mint", "acme"], 2),
            // native: app key mint
            stop("key mint: --label last", &["key", "mint", "acme", "--label"], 1),
            // native: app label take
            one(
                "label take, copied",
                &["label", "take", "acme", "site"],
                ok(r#"{"label":"site","app":"acme","frontPort":4100,"copied":{"added":["ann","bo"],"keptExisting":[{"user":"cy"},{}],"skippedOtherAppRole":["dee"],"skippedOtherWorkspaceAgent":["eve"],"rolesCopied":[{"from":"staff","to":"acme-staff"}]},"notCopied":["fay: no login"],"note":"guests sign in again"}"#),
                0,
            ),
            // native: app label take
            one(
                "label take, none copied, already",
                &["label", "take", "acme", "site"],
                ok(r#"{"label":"site","app":"acme","already":true}"#),
                0,
            ),
            // native: app label take
            one(
                "label take, nothing to copy",
                &["label", "take", "acme", "site"],
                ok(r#"{"label":"site","app":"acme","frontPort":1,"copied":{}}"#),
                0,
            ),
            // native: app label return
            one(
                "label return",
                &["label", "return", "acme", "site"],
                ok(r#"{"label":"site","app":"acme","returnedTo":{"port":8080},"site":true,"note":"back"}"#),
                0,
            ),
            // native: app label take --json
            one(
                "label take: held --json",
                &["label", "take", "acme", "site", "--json"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app label
            stop("label: no verb", &["label", "acme"], 2),
            // native: app label return
            stop("label return: no label", &["label", "return", "acme"], 2),
            // native: app label take
            traceback(one(
                "label take: added that is a number",
                &["label", "take", "acme", "site"],
                ok(r#"{"label":"site","copied":{"added":5}}"#),
                1,
            )),
            // native: app move-plan
            one(
                "move-plan --request: held",
                &["move-plan", "site", "--request", "--reason", "move é"],
                Reply::Answer(
                    202,
                    r#"{"status":"held","request":{"id":"r1"},"hint":"ask the owner","plan":{"service":{"workspaceHandle":"ws","name":"site","port":8080},"app":{"id":"site","installed":true},"steps":[],"ownerStepsTodo":["a","b"]}}"#.into(),
                ),
                2,
            ),
            // native: app move-plan
            one(
                "move-plan --request: nothing to ask",
                &["move-plan", "site", "--request"],
                ok(r#"{"status":"nothing_to_request","note":"all done","plan":{"service":{"workspaceHandle":"ws","name":"site","port":1},"app":{"id":"a","installed":true},"steps":[]}}"#),
                0,
            ),
            // native: app move-plan --json
            one(
                "move-plan --request --json: held",
                &["move-plan", "site", "--request", "--json"],
                Reply::Answer(202, r#"{"status":"held"}"#.into()),
                2,
            ),
            // native: app move-plan
            one(
                "move-plan --request: refused",
                &["move-plan", "site", "--request"],
                refused(),
                3,
            ),
            // native: app quota
            one(
                "quota: set",
                &[
                    "quota", "acme", "--rps", "010", "--burst", "20", "--sockets", "3",
                    "--body", "8M", "--upload", "1G", "--subscriptions", "9", "--reason", "launch é",
                ],
                ok(QUOTA),
                0,
            ),
            // native: app quota
            one(
                "quota --reset: held",
                &["quota", "acme", "--reset"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app quota --json
            one(
                "quota: an empty value still asks for a change, --json",
                &["quota", "acme", "--body", "", "--json"],
                ok(QUOTA),
                0,
            ),
            // native: app token rotate
            one(
                "token rotate",
                &["token", "rotate", "acme", "--ttl", "30d", "--keep-old", "--caps", "push, room.write"],
                ok(ROTATED),
                0,
            ),
            // native: app token rotate
            one(
                "tokens rotate, nothing withheld",
                &["tokens", "rotate", "acme"],
                ok(r#"{"app":"acme","pass":"k2rp_x","expiresAt":1700000000}"#),
                0,
            ),
            // native: app token rotate --json
            one(
                "token rotate --json",
                &["token", "rotate", "acme", "--json"],
                ok(ROTATED),
                0,
            ),
            // native: app token rotate
            Case {
                env: with_passport(&[]),
                ..case(
                    "token rotate: never in a session",
                    &["token", "rotate", "acme"],
                    &[],
                    vec![],
                    port_only,
                    3,
                    0,
                )
            },
            // native: app token rotate
            stop("token rotate: no App", &["token", "rotate"], 2),
            // native: app token rotate
            one("token rotate: refused", &["token", "rotate", "acme"], refused(), 3),
            // native: app token rotate
            traceback(one(
                "token rotate: no pass",
                &["token", "rotate", "acme"],
                ok(r#"{"app":"acme"}"#),
                1,
            )),
        ],
    );
}

#[test]
fn access_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "access");
    let three = |name: &'static str, args: &'static [&'static str], code: i32| {
        case(
            name,
            args,
            &[],
            vec![ok(SET), ok(SET), ok(LIST)],
            owner_and_port,
            code,
            3,
        )
    };
    check(
        &rig,
        vec![
            // native: app access
            one("access: the read", &["access", "ws"], ok(LIST), 0),
            // native: app access --json
            one(
                "access --json: the read",
                &["access", "ws", "--json"],
                ok(LIST),
                0,
            ),
            // native: app access --json
            traceback(one(
                "access --json: an answer that is a list",
                &["access", "ws", "--json"],
                ok("[1]"),
                1,
            )),
            // native: app access
            three(
                "access: both on",
                &["access", "ws", "--build", "on", "--manage", "on"],
                0,
            ),
            // native: app access
            three(
                "access: both off, switch 2 first",
                &["access", "ws", "--manage", "off", "--build", "off"],
                0,
            ),
            // native: app access
            case(
                "access: --manage alone, its note",
                &["access", "ws", "--manage", "on"],
                &[],
                vec![ok(SET), ok(LIST)],
                owner_and_port,
                0,
                2,
            ),
            // native: app access
            case(
                "access: --users and the cores, the read again refused",
                &[
                    "access",
                    "ws",
                    "--users",
                    "off",
                    "--core",
                    "db=on",
                    "--cores",
                    " files =off,mail=on,",
                ],
                &[],
                vec![ok(SET), ok(SET), refused()],
                owner_and_port,
                0,
                3,
            ),
            // native: app access --json
            case(
                "access --json: the last answer",
                &["access", "ws", "--build", "on", "--users", "on", "--json"],
                &[],
                vec![ok(SET), ok(r#"{"toggle":"app_users"}"#)],
                owner_and_port,
                0,
                2,
            ),
            // native: app access
            case(
                "access: --users checked after the switches went",
                &["access", "ws", "--build", "on", "--users", "maybe"],
                &[],
                vec![ok(SET)],
                owner_and_port,
                2,
                1,
            ),
            // native: app access
            one(
                "access: refused",
                &["access", "ws", "--build", "on"],
                refused(),
                3,
            ),
            // native: app access
            stop("access: no workspace", &["access", "--build", "on"], 2),
            // native: app access
            stop(
                "access: a core that isn't on or off",
                &["access", "ws", "--core", "db=yes"],
                2,
            ),
            // native: app access
            stop(
                "access: --open is retired",
                &["access", "ws", "--open", "on"],
                2,
            ),
            // native: app access
            stop(
                "access: --build maybe",
                &["access", "ws", "--build", "maybe"],
                2,
            ),
            // native: app access
            stop("access: --core last", &["access", "ws", "--core"], 1),
            // native: app invite-settings
            one(
                "invite-settings: the read",
                &["invite-settings", "acme"],
                ok(r#"{"app":"acme","settings":{}}"#),
                0,
            ),
            // native: app invite-settings
            stop("invite-settings: no App", &["invite-settings"], 2),
        ],
    );
}

/// One answer above 256 KiB: thousands of keys.
#[test]
fn keys_a_large_list() {
    let rig = Rig::new(VERB, "keyslarge");
    let rows: Vec<String> = (0..4000)
        .map(|n| {
            format!(
                r#"{{"id":"key-{n:05}","state":"live","label":"label é {n}","scopes":["users","rooms","fetch"],"rooms":["room-{n}","room-{}"]}}"#,
                n + 1
            )
        })
        .collect();
    let body = format!(r#"{{"keys":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["keys"][..], &["keys", "--json"]] {
        rig.check(&case(
            "a large list",
            args,
            &[],
            vec![ok(&body)],
            owner_and_port,
            0,
            1,
        ));
    }
}

#[test]
fn the_changes_help_never_acts() {
    let rig = Rig::new(VERB, "grhelp");
    for args in [
        &["clean-mode", "acme", "on", "-h"][..],
        &["key", "mint", "acme", "--scopes", "u", "--help"],
        &["key", "revoke", "k1", "-h"],
        &["label", "take", "acme", "site", "--help"],
        &["move-plan", "site", "--request", "-h"],
        &["quota", "acme", "--rps", "5", "--help"],
        &["token", "rotate", "acme", "-h"],
        &["access", "ws", "--build", "on", "-h"],
        &["invite-settings", "acme", "--max-devices", "3", "--help"],
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}

/// The lines name the command and the product by the brand's names (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy` and `XY`), never a
/// typed `k2`, `k5` or `K2`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "grbrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want, code) in [
        (
            &["clean-mode", "a"][..],
            format!("usage: {cli} app clean-mode <app> on|off [--restart]\n"),
            2,
        ),
        (
            &["test-guests"],
            format!("usage: {cli} app test-guests <app-id> [--json]\n"),
            2,
        ),
        (
            &["grants", "--json"],
            format!("usage: {cli} app grants <app> [--json]\n"),
            2,
        ),
        (
            &["token", "rotate"],
            format!("usage: {cli} app token rotate <app-id> [--ttl 30d] [--keep-old] [--caps a,b]\n"),
            2,
        ),
        (
            &["label", "take", "a"],
            format!("usage: {cli} app label take <app-id> <label>\n"),
            2,
        ),
        (
            &["access", "w", "--open", "x"],
            format!(
                "{cli}: --open is retired: each open-path move (a new --cmd service, --path open, extra ports, broad egress) is held\n    for the owner or an admin, who approves it in person: {cli} app approvals, then {cli} app approve <id> (outside {}).\n",
                k2_client::brand::PRODUCT_NAME
            ),
            2,
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, code, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
