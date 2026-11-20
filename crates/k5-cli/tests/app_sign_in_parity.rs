//! `k5-cli app auth|login settings|chat|init|doctor --store` against the
//! shell CLI's (`cli/src/app-auth.sh`, `cli/src/app-people.sh`,
//! `cli/src/app-recording.sh`, `cli/src/app-init.sh`,
//! `cli/src/app-doctor.sh`), on the parity rig ([`common::parity`]): the
//! same requests, stdout, stderr and exit code.
//!
//! - `auth`: the box alone, an App, `--json`, `door on|off`, `origins`
//!   (the read, `--origin`, `--clear`), each usage line, a refusal,
//!   Python raising.
//! - `login settings`: the read, every flag (body files, `--default`,
//!   `--max-devices`), `--json`, the usage lines, a missing body file.
//! - `chat`: Spaces with steps, channels, people and leavers; none;
//!   `--json`; the usage lines.
//! - `init` and `doctor --store`: a folder (relative, `.`), `--json`, every
//!   usage line, a folder that is not one, ready and not ready.
//! - Help never acts: the write verbs with `-h` or `--help` anywhere.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";
const AUTH: &str = r#"{"box":{"on":true},"app":{"name":"acme","door":"on","origins":["https://a.example"],"pairingOrigins":["https://p.example"],"hosts":["acme.example"],"labelNote":"a label","notRuntime":true},"warnings":["w1"]}"#;
const LOGIN: &str = r#"{"app":"acme","settings":{"inviteTtlSecs":345600,"inviteTtlSet":true,"resetTtlSecs":5400,"setPasswordUrl":"https://acme.example/set","invite":{"subject":"Hi"}},"maxDevices":20}"#;
const CHAT: &str = r#"{"name":"acme","chat":{"appId":"a1","feature":true,"key":{"enabled":true,"maxLevel":"member","maxMembers":500},"manifestChannels":2,"spaces":[{"spaceId":"sp1","governance":"app","governanceConfirmed":"sew","sync":[{"id":"st1","op":"add","subject":"ann","state":"blocked","attempts":3,"lastStatus":409,"lastError":"taken"},{"op":"rm","subject":"bo","state":"done","attempts":1}],"channels":[{"slug":"general","type":"text","private":false,"state":"made","channelId":"c1"},{"slug":"ops é","type":"voice","private":true,"state":"waiting","why":"no Sew yet"}],"waiting":{"channels":["ops é"],"agents":["sales"],"refusedAgents":["x"]},"people":[{"username":"ann","level":"member","state":"in","channels":["general"]},{"externalId":"e9","level":"guest","state":"waiting","why":"pending"}],"leaving":[{"externalId":"u1","state":"gone"},{"externalId":"u2"}]}]}}"#;
const INIT: &str = r#"{"dryRun":true,"files":[{"action":"write","path":"src/k2.ts","note":"new","content":"line 1\nline 2\n\n"},{"action":"snippet","path":"main.rs","note":"by hand","content":"fn main() {}"},{"action":"other","path":"x","note":null}],"notes":["n1"],"next":["npm i","npm run dev"]}"#;
const STORE: &str = r#"{"folder":"/x","voice":true,"voiceWhy":"mic used","tauri":"2.1","wry":"0.4","findings":[{"level":"error","platform":"ios","path":"Info.plist","problem":"no mic text","fix":"add it"},{"level":"warn","platform":"android","path":"a.xml","problem":"p","fix":"f"}],"notes":["n"],"passed":["a"],"ok":false}"#;

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

fn one(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Case {
    case(name, args, &[], vec![reply], owner_and_port, code, 1)
}

fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn refused() -> Reply {
    Reply::Answer(
        403,
        r#"{"error":{"code":"owner_only","hint":"the owner's"}}"#.into(),
    )
}

/// The owner and port, a folder `site` and two body files.
fn files(rig: &Rig) {
    owner_and_port(rig);
    std::fs::create_dir_all(rig.proj.join("site")).unwrap();
    std::fs::write(rig.proj.join("invite.txt"), "Hello {name} \u{e9}\n\n").unwrap();
    std::fs::write(rig.proj.join("reset.txt"), "Reset\nhere").unwrap();
}

fn with_files(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Case {
    case(name, args, &[], vec![reply], files, code, 1)
}

fn words_with_files(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], files, code, 0)
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn auth_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "auth");
    check(
        &rig,
        vec![
            // covers: app auth
            one("auth: the box", &["auth"], ok(r#"{"box":{"on":false}}"#), 0),
            // covers: app auth
            one(
                "auth: an override",
                &["auth"],
                ok(r#"{"box":{"on":false,"envOverride":"off"}}"#),
                0,
            ),
            // covers: app auth
            one("auth acme", &["auth", "acme"], ok(AUTH), 0),
            // covers: app auth --json
            one("auth --json", &["auth", "--json"], ok(AUTH), 0),
            // covers: app auth --json
            one("auth acme --json", &["auth", "off", "--json"], ok(AUTH), 0),
            // covers: app auth
            one(
                "auth door on acme",
                &["auth", "door", "acme", "on"],
                ok(r#"{"box":{"on":true}}"#),
                0,
            ),
            // covers: app auth --json
            one(
                "auth door off --json",
                &["auth", "door", "off", "--json"],
                ok(r#"{"box":{"on":false}}"#),
                0,
            ),
            // covers: app auth
            words("auth door: no on|off", &["auth", "door", "acme"], 2),
            // covers: app auth
            one(
                "auth origins: the read",
                &["auth", "origins", "acme"],
                ok(AUTH),
                0,
            ),
            // covers: app auth
            one(
                "auth origins: set",
                &[
                    "auth",
                    "origins",
                    "acme",
                    "--origin",
                    " https://a.example ",
                    "--origin",
                    "",
                    "--origin",
                    "https://b é",
                ],
                ok(AUTH),
                0,
            ),
            // covers: app auth
            one(
                "auth origins --clear",
                &["auth", "origins", "acme", "--clear"],
                ok(r#"{"box":{"on":true},"app":{"app":"acme","door":"off","appOff":true}}"#),
                0,
            ),
            // covers: app auth
            words(
                "auth origins: both",
                &["auth", "origins", "acme", "--origin", "x", "--clear"],
                2,
            ),
            // covers: app auth
            words("auth origins: no App", &["auth", "origins"], 2),
            // covers: app auth
            words(
                "auth: --origin with no origins",
                &["auth", "acme", "--origin", "x"],
                2,
            ),
            // covers: app auth
            words(
                "auth: --clear with door",
                &["auth", "door", "on", "--clear"],
                2,
            ),
            // covers: app auth
            words(
                "auth: --origin last",
                &["auth", "origins", "acme", "--origin"],
                1,
            ),
            // covers: app auth
            words("auth: a flag first", &["auth", "--zap"], 2),
            // covers: app auth
            words("auth: a flag", &["auth", "acme", "-x"], 2),
            // covers: app auth
            words("auth: two Apps", &["auth", "a", "b"], 2),
            // covers: app auth
            one("auth: a refusal", &["auth", "door", "on"], refused(), 3),
            // covers: app auth
            traceback(one(
                "auth: hosts that are not words",
                &["auth", "acme"],
                ok(r#"{"app":{"name":"a","hosts":[1]}}"#),
                1,
            )),
        ],
    );
}

#[test]
fn login_settings_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "login");
    check(
        &rig,
        vec![
            // covers: app login settings
            with_files(
                "login settings",
                &["login", "settings", "acme"],
                ok(LOGIN),
                0,
            ),
            // covers: app login settings
            with_files(
                "login settings: every flag",
                &[
                    "login",
                    "settings",
                    "acme",
                    "--invite-ttl",
                    "72h",
                    "--reset-ttl",
                    "24h",
                    "--set-password-url",
                    "https://acme.example/set",
                    "--invite-subject",
                    "Hi {name}",
                    "--invite-body-file",
                    "invite.txt",
                    "--reset-subject",
                    "R",
                    "--reset-body-file",
                    "reset.txt",
                    "--max-devices",
                    "07",
                    "--default",
                    "resetSubject inviteTtl",
                ],
                ok(
                    r#"{"app":"acme","settings":{"inviteTtlSecs":"7200","resetTtlSecs":90},"maxDevices":7}"#,
                ),
                0,
            ),
            // covers: app login settings --json
            with_files(
                "login settings --json",
                &["login", "settings", "--json", "acme"],
                ok(LOGIN),
                0,
            ),
            // covers: app login settings
            words_with_files("login: no settings", &["login", "acme"], 2),
            // covers: app login settings
            words_with_files("login settings: no App", &["login", "settings"], 2),
            // covers: app login settings
            words_with_files(
                "login settings: --max-devices too big",
                &["login", "settings", "acme", "--max-devices", "100"],
                2,
            ),
            // covers: app login settings
            words_with_files(
                "login settings: no such body file",
                &[
                    "login",
                    "settings",
                    "acme",
                    "--invite-body-file",
                    "nope.txt",
                ],
                2,
            ),
            // covers: app login settings
            words_with_files(
                "login settings: a body file given last",
                &["login", "settings", "acme", "--reset-body-file"],
                2,
            ),
            // covers: app login settings
            words_with_files(
                "login settings: --reset-ttl last",
                &["login", "settings", "acme", "--reset-ttl"],
                1,
            ),
            // covers: app login settings
            words_with_files(
                "login settings: a flag",
                &["login", "settings", "acme", "--zap"],
                2,
            ),
            // covers: app login settings
            with_files(
                "login settings: a refusal",
                &["login", "settings", "acme"],
                refused(),
                3,
            ),
            // covers: app login settings
            traceback(with_files(
                "login settings: a life that is not a number",
                &["login", "settings", "acme"],
                ok(r#"{"settings":{"inviteTtlSecs":"soon"}}"#),
                1,
            )),
        ],
    );
}

#[test]
fn chat_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "chat");
    check(
        &rig,
        vec![
            // covers: app chat
            one("chat", &["chat", "acme"], ok(CHAT), 0),
            // covers: app chat
            one(
                "chat: no key, no feature, no Space",
                &["chat", "acme"],
                ok(r#"{"name":"acme","chat":{"appId":"a1"}}"#),
                0,
            ),
            // covers: app chat --json
            one("chat --json", &["chat", "--json", "acme"], ok(CHAT), 0),
            // covers: app chat
            words("chat: no App", &["chat"], 2),
            // covers: app chat
            words("chat: two Apps", &["chat", "a", "b"], 2),
            // covers: app chat
            words("chat: a flag", &["chat", "a", "--x"], 2),
            // covers: app chat
            one("chat: a refusal", &["chat", "acme"], refused(), 3),
            // covers: app chat
            traceback(one(
                "chat: waiting channels that are not words",
                &["chat", "acme"],
                ok(r#"{"chat":{"spaces":[{"waiting":{"channels":[1]}}]}}"#),
                1,
            )),
        ],
    );
}

#[test]
fn init_and_store_doctor_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "init");
    check(
        &rig,
        vec![
            // covers: app init
            with_files(
                "init tauri, a dry run",
                &[
                    "init",
                    "--template",
                    "tauri",
                    "site",
                    "--app",
                    "acme",
                    "--voice",
                    "--dry-run",
                ],
                ok(INIT),
                0,
            ),
            // covers: app init
            with_files(
                "init static here",
                &["init", "--template=static", "--name=Acme"],
                ok(
                    r#"{"files":[{"action":"write","path":"index.html","note":"n","content":"x"}]}"#,
                ),
                0,
            ),
            // covers: app init --json
            with_files(
                "init --json",
                &["init", "--json", "--template", "node-api", "./site/"],
                ok(INIT),
                0,
            ),
            // covers: app init
            words_with_files("init: no template", &["init"], 2),
            // covers: app init
            words_with_files("init: a bad template", &["init", "--template", "go"], 2),
            // covers: app init
            words_with_files(
                "init: --voice with static",
                &["init", "--template", "static", "--voice"],
                2,
            ),
            // covers: app init
            words_with_files(
                "init: --app and --name",
                &["init", "--template", "tauri", "--app", "a", "--name", "n"],
                2,
            ),
            // covers: app init
            words_with_files(
                "init: not a folder",
                &["init", "--template", "tauri", "invite.txt"],
                2,
            ),
            // covers: app init
            words_with_files("init: two folders", &["init", "a", "b"], 2),
            // covers: app init
            words_with_files("init: --template last", &["init", "--template"], 1),
            // covers: app init
            with_files(
                "init: a refusal",
                &["init", "--template", "static", "site"],
                refused(),
                3,
            ),
            // covers: app init
            traceback(with_files(
                "init: content that is not words",
                &["init", "--template", "static"],
                ok(r#"{"dryRun":true,"files":[{"action":"write","content":5}]}"#),
                1,
            )),
            // covers: app doctor store
            with_files(
                "doctor --store: not ready",
                &["doctor", "--store", "site", "--voice"],
                ok(STORE),
                1,
            ),
            // covers: app doctor store
            with_files(
                "doctor --store: ready, here",
                &["doctor", "--no-voice", "--store"],
                ok(
                    r#"{"folder":"/x","voiceWhy":"no mic","findings":[{"level":"warn","platform":"ios","path":"p","problem":"q","fix":"r"}],"passed":["a","b"],"ok":true}"#,
                ),
                0,
            ),
            // covers: app doctor store --json
            with_files(
                "doctor --store --json: not ready",
                &["doctor", "--store", "--json", "site"],
                ok(STORE),
                1,
            ),
            // covers: app doctor store --json
            traceback(with_files(
                "doctor --store --json: not JSON",
                &["doctor", "--store", "--json"],
                ok("<html>"),
                1,
            )),
            // covers: app doctor store
            words_with_files(
                "doctor --store: not a folder",
                &["doctor", "--store", "nope"],
                2,
            ),
            // covers: app doctor store
            words_with_files(
                "doctor --store: two folders",
                &["doctor", "--store", "a", "b"],
                2,
            ),
            // covers: app doctor store
            with_files(
                "doctor --store: a refusal",
                &["doctor", "--store"],
                refused(),
                3,
            ),
        ],
    );
}

/// Help never acts (K2's rule): the write verbs with `-h` or `--help`
/// anywhere print help and send nothing; both sides are the shell's.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "signhelp");
    check(
        &rig,
        vec![
            shell(words("auth door on -h", &["auth", "door", "on", "-h"], 0)),
            shell(words(
                "auth origins --help",
                &["auth", "origins", "acme", "--clear", "--help"],
                0,
            )),
            shell(words(
                "login settings -h",
                &["login", "settings", "acme", "--max-devices", "3", "-h"],
                0,
            )),
            shell(words(
                "init --help",
                &["init", "--template", "static", "--help"],
                0,
            )),
            shell(words("doctor --store -h", &["doctor", "--store", "-h"], 0)),
        ],
    );
}
