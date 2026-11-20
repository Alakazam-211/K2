//! `k5-cli app validate|status|sdk|recording|room-names` against the shell
//! CLI's (`cli/src/talk-and-workspace.sh`, `cli/src/app-doctor.sh`,
//! `cli/src/send-feedback.sh`, `cli/src/app-sdk.sh`,
//! `cli/src/app-recording.sh`), on the parity rig ([`common::parity`]):
//! the same requests, stdout, stderr and exit code.
//!
//! - `validate`: the code pass (nothing, an App name, `--dir`), a folder
//!   with `manifest.json`, `--manifest`, a Seed hash; Apps off (the
//!   folder's fallback, the Seed's refusal, `--types`); `--json`; the
//!   daemon's errors, an answer that is not JSON, empty or a list; Python
//!   raising (twice where the shell's `set -e` is off); the words.
//! - `status`: each verdict's exit, `--json`, `--no-checks`, `--public`,
//!   no verdict, a raise before and after the verdict, a refusal, words.
//! - `sdk`: `add` (`--sew`, `--json`), `status`, `kit`, the page, the
//!   checks on the folder or file, a refusal, a raise, words.
//! - `recording`: the read, the home client, `--json`; `set` with each
//!   setting, the confirm the daemon asks for (no vault socket here, a
//!   confirm with no action, in a session), a raise, and every usage line.
//! - `room-names`: hide, show, already, `--json`, in a session, words.
//! - Help never acts: each write verb with `-h` or `--help` anywhere.

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

fn in_session(c: Case) -> Case {
    Case {
        env: with_passport(&[]),
        setup: nothing,
        ..c
    }
}

/// Folders and files the verbs check: `site` has a `manifest.json`,
/// `plain` has none; `m.json`; `kits/sew-app-kit-1.0.tgz`.
fn files(rig: &Rig) {
    owner_and_port(rig);
    for d in ["site", "plain", "kits"] {
        std::fs::create_dir_all(rig.proj.join(d)).unwrap();
    }
    std::fs::write(rig.proj.join("site/manifest.json"), "{}\n").unwrap();
    std::fs::write(rig.proj.join("m.json"), "{}\n").unwrap();
    std::fs::write(rig.proj.join("kits/sew-app-kit-1.0.tgz"), "bytes").unwrap();
}

/// One answer, the folders on disk.
fn one(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], files, code, 1)
}

/// Words that stop before anything is sent.
fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], files, code, 0)
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

const APPS_OFF: &str = r#"{"error":{"code":"apps_disabled","hint":"Apps are off"}}"#;
const REPORT_OK: &str = r#"{"ok":true,"report":{"ok":true,"checks":["manifest","contract"],"warnings":[{"code":"W1","at":"manifest.json","message":"hm é"},{"code":"W2","message":"no at"}]}}"#;

#[test]
fn validate_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "validate");
    check(
        &rig,
        vec![
            // covers: app validate
            one(
                "validate, nothing: this workspace",
                &["validate"],
                r#"{"ok":true,"files":3}"#,
                0,
            ),
            // covers: app validate
            one(
                "validate an App name: errors, warnings, skipped",
                &["validate", "acme"],
                r#"{"ok":false,"files":2,"target":"acme é","errors":[{"file":"a.js","line":3,"col":1,"message":"leak","suggestion":"use fromEnv"}],"warnings":[{"file":"b.ts","line":1,"col":2,"message":"hm"}],"skipped":["big.js"]}"#,
                1,
            ),
            // covers: app validate
            one(
                "validate --dir with no manifest: the code pass, pending",
                &["validate", "--dir", "plain"],
                r#"{"status":"pending","files":2,"warnings":[{"file":"x","line":1,"col":1,"message":"w"}]}"#,
                2,
            ),
            // covers: app validate
            one(
                "validate --dir as the last word",
                &["validate", "--dir"],
                r#"{"ok":true,"files":0}"#,
                0,
            ),
            // covers: app validate --json
            one(
                "validate --json",
                &["validate", "--json", "acme"],
                r#"{"ok":true,"files":1,"x":"é","n":[1,{"a":null}]}"#,
                0,
            ),
            // covers: app validate --json
            one(
                "validate --json: the daemon's error",
                &["validate", "acme", "--json"],
                r#"{"error":{"message":"no such App é"},"suggestion":"try x"}"#,
                1,
            ),
            // covers: app validate
            one(
                "validate: the error is a string",
                &["validate", "acme"],
                r#"{"error":"bad","message":null}"#,
                1,
            ),
            // covers: app validate
            one("validate: not JSON", &["validate", "acme"], "nope", 1),
            // covers: app validate
            one("validate: a list", &["validate", "acme"], "[1]", 1),
            // covers: app validate
            one("validate: an empty answer", &["validate", "acme"], "", 1),
            // covers: app validate
            traceback(one(
                "validate: an error that is a string",
                &["validate", "acme"],
                r#"{"ok":false,"errors":["x"]}"#,
                1,
            )),
            // covers: app validate
            one(
                "validate a folder with manifest.json",
                &["validate", "site"],
                REPORT_OK,
                0,
            ),
            // covers: app validate
            one(
                "validate a folder: the report fails",
                &["validate", "site"],
                r#"{"ok":false,"error":{"report":{"ok":false,"errors":[{"code":"E1","at":"x","message":"bad"}]}}}"#,
                1,
            ),
            // covers: app validate --json
            one(
                "validate a folder --types --json",
                &["validate", "site", "--types", "--json"],
                REPORT_OK,
                0,
            ),
            // covers: app validate
            traceback(one(
                "validate a folder: a list, two tracebacks",
                &["validate", "--dir", "site"],
                "[]",
                1,
            )),
            // covers: app validate
            case(
                "validate a folder, Apps off: the code pass",
                &["validate", "site"],
                &[],
                vec![answer(404, APPS_OFF), ok(r#"{"ok":true,"files":4}"#)],
                files,
                0,
                2,
            ),
            // covers: app validate
            one(
                "validate a folder --types, Apps off",
                &["validate", "site", "--types"],
                APPS_OFF,
                2,
            ),
            // covers: app validate
            case(
                "validate a folder: refused",
                &["validate", "site"],
                &[],
                vec![answer(403, r#"{"error":{"code":"owner_only","hint":"x"}}"#)],
                files,
                3,
                1,
            ),
            // covers: app validate
            one(
                "validate a Seed",
                &["validate", "0123456789abcdef"],
                REPORT_OK,
                0,
            ),
            // covers: app validate
            case(
                "validate a Seed, Apps off",
                &["validate", "0123456789abcdef", "--types"],
                &[],
                vec![answer(404, APPS_OFF)],
                files,
                3,
                1,
            ),
            // covers: app validate
            one(
                "validate --dir plain --manifest m.json",
                &["validate", "--dir", "plain", "--manifest", "m.json"],
                REPORT_OK,
                0,
            ),
            // covers: app validate
            words("validate: an unknown flag", &["validate", "--zap"], 2),
            // covers: app validate
            words("validate: two targets", &["validate", "a", "b"], 2),
            // covers: app validate
            words(
                "validate: a target and --dir",
                &["validate", "a", "--dir=plain"],
                2,
            ),
            // covers: app validate
            words(
                "validate: --manifest is not a file",
                &["validate", "site", "--manifest", "nope.json"],
                2,
            ),
            // covers: app validate
            words(
                "validate: --manifest with no folder",
                &["validate", "acme", "--manifest=m.json"],
                2,
            ),
            // covers: app validate
            words(
                "validate: --types with an App name",
                &["validate", "acme", "--types"],
                2,
            ),
            // covers: app validate
            words(
                "validate: --types with a folder with no manifest",
                &["validate", "plain", "--types"],
                2,
            ),
        ],
    );
}

const STATUS: &str = r#"{"app":"acme","verdict":"green","deploy":{"state":"live","stateReason":"all good é","seed":{"hash":"0123456789abcdef0123","version":"1.2"},"live":{"liveAt":"2026-11-20T01:00:00Z","actor":"ann","commit":"abcdef0123456789"},"last":{"kind":"deploy","id":"d-1","result":"failed","error":"boom"}},"health":{"path":"/health","probe":{"status":200,"ms":12},"uptimeSecs":300,"recentRestarts":0},"checks":{"failed":1,"declared":2,"source":"manifest","results":[{"name":"home","path":"/","expect":200,"status":200,"ok":true},{"name":"a-very-long-check-name","path":"/x","expect":200,"status":null,"ok":false,"why":"timeout"}]},"errors":{"byKind":{"5xx":3,"js":1}},"resources":{"app":{"diskBytes":12345678},"level":"ok"},"door":{"egressDenied":2,"heldApprovals":0},"hosts":{"url":"https://acme.example","domains":{"domains":[{"d":1},{"d":2}]}},"phone":{"push":{"on":true,"keys":{"apns":true},"devices":3},"pairing":{"on":false}},"updates":{"feed":"stable"},"next":[{"level":"warn","what":"a check fails","run":"k2 app status acme"}]}"#;

#[test]
fn status_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "status");
    check(
        &rig,
        vec![
            // covers: app status
            one("status, green", &["status", "acme"], STATUS, 0),
            // covers: app status
            one(
                "status, yellow, the bare answer",
                &["status", "a b", "--no-checks"],
                r#"{"app":"a b","verdict":"yellow"}"#,
                2,
            ),
            // covers: app status
            one(
                "status, red: recorded checks, errors and resources unavailable",
                &["status", "acme", "--public"],
                r#"{"app":"acme","verdict":"red","deploy":{"state":"failed"},"health":{"probe":{"ms":5}},"checks":{"recorded":{"results":[1,2,3]},"failed":2},"errors":{"unavailable":"no log"},"resources":{"unavailable":"off"},"hosts":{"label":"lab","domains":[]},"phone":{}}"#,
                1,
            ),
            // covers: app status
            one(
                "status: the checks' error, a disk that is a float",
                &["status", "acme"],
                r#"{"app":"acme","verdict":"green","checks":{"error":"no manifest"},"resources":{"app":{"diskBytes":1.5e6},"level":3}}"#,
                0,
            ),
            // covers: app status
            one(
                "status: no verdict",
                &["status", "acme"],
                r#"{"app":"acme"}"#,
                1,
            ),
            // covers: app status --json
            one(
                "status --json, yellow",
                &["status", "acme", "--json"],
                r#"{"app":"acme","verdict":"yellow","x":"é"}"#,
                2,
            ),
            // covers: app status --json
            one(
                "status --json, no verdict",
                &["status", "--json", "acme"],
                r#"{"verdict":null}"#,
                1,
            ),
            // covers: app status
            traceback(one(
                "status: a list, before the verdict",
                &["status", "acme"],
                "[1]",
                1,
            )),
            // covers: app status
            traceback(one(
                "status: no app in the answer",
                &["status", "acme"],
                r#"{"verdict":"green"}"#,
                1,
            )),
            // covers: app status
            traceback(one(
                "status: a verdict that is a number",
                &["status", "acme"],
                r#"{"app":"a","verdict":7}"#,
                1,
            )),
            // covers: app status
            traceback(one(
                "status: next with no run",
                &["status", "acme"],
                r#"{"app":"a","verdict":"green","next":[{"level":"x","what":"y"}]}"#,
                1,
            )),
            // covers: app status
            case(
                "status: refused",
                &["status", "acme"],
                &[],
                vec![answer(
                    403,
                    r#"{"error":{"code":"forbidden","hint":"not yours"}}"#,
                )],
                files,
                3,
                1,
            ),
            // covers: app status
            words("status with no app", &["status"], 2),
            // covers: app status
            words("status: an unknown flag", &["status", "acme", "--zap"], 2),
            // covers: app status
            words("status: two apps", &["status", "a", "b"], 2),
            // covers: app status
            words(
                "status --no-checks --public",
                &["status", "a", "--no-checks", "--public"],
                2,
            ),
        ],
    );
}

#[test]
fn sdk_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "sdk");
    let added = r#"{"folder":"/w/site","vendored":[{"name":"@k2/server","version":"0.46.17","file":"vendor/k2-server-0.46.17.tgz","written":true},{"name":"@sew/client","version":"0.9.0","file":"vendor/sew-client.tgz","written":false}],"removed":["vendor/old.tgz"],"packageJson":"updated","overrides":["@sew/voice"],"install":"npm i x y"}"#;
    check(
        &rig,
        vec![
            // covers: app sdk add
            one("sdk add --sew", &["sdk", "add", "site", "--sew"], added, 0),
            // covers: app sdk add
            one(
                "sdk add: no removed, no overrides, no install",
                &["sdk", "add", "plain"],
                r#"{"folder":"/w/plain","vendored":[{"name":"@k2/server","version":"1","file":"f","written":true}],"packageJson":"made"}"#,
                0,
            ),
            // covers: app sdk add --json
            one(
                "sdk add --json",
                &["sdk", "add", "--json", "site"],
                r#"{"folder":"x","é":1}"#,
                0,
            ),
            // covers: app sdk add
            traceback(one(
                "sdk add: nothing vendored",
                &["sdk", "add", "site"],
                r#"{"folder":"/w","vendored":[]}"#,
                1,
            )),
            // covers: app sdk add
            case(
                "sdk add: refused",
                &["sdk", "add", "site"],
                &[],
                vec![answer(
                    400,
                    r#"{"error":{"code":"bad_folder","hint":"no package.json"}}"#,
                )],
                files,
                2,
                1,
            ),
            // covers: app sdk add
            words("sdk add: not a folder", &["sdk", "add", "nope"], 2),
            // covers: app sdk add
            words("sdk add: two folders", &["sdk", "add", "site", "plain"], 2),
            // covers: app sdk add
            words("sdk add: no folder", &["sdk", "add", "--sew"], 2),
            // covers: app sdk add
            words("sdk add: an unknown flag", &["sdk", "add", "site", "-x"], 2),
            // covers: app sdk status
            one(
                "sdk status: older, with Sew",
                &["sdk", "status", "site"],
                r#"{"folder":"/w/site","box":{"k2Server":"0.46.17","sewKit":"0.9.0"},"packages":[{"name":"@k2/server","vendored":"0.46.16","onBox":"0.46.17","state":"older"},{"name":"@sew/client","state":"missing"}]}"#,
                0,
            ),
            // covers: app sdk status
            one(
                "sdk status: current, no kit pinned",
                &["sdk", "status", "plain"],
                r#"{"folder":"/w/plain","box":{"k2Server":"0.46.17","sewPin":"0.4.0"},"packages":[{"name":"@k2/server","vendored":"0.46.17","onBox":"0.46.17","state":"current"}]}"#,
                0,
            ),
            // covers: app sdk status
            one(
                "sdk status: a kit pinned, none kept",
                &["sdk", "status", "plain"],
                r#"{"folder":"/w/plain","box":{"k2Server":"1","sewKitPinned":true},"packages":[]}"#,
                0,
            ),
            // covers: app sdk status --json
            one(
                "sdk status --json",
                &["sdk", "status", "site", "--json"],
                r#"{"box":{}}"#,
                0,
            ),
            // covers: app sdk status
            traceback(one(
                "sdk status: no box",
                &["sdk", "status", "site"],
                r#"{"packages":[]}"#,
                1,
            )),
            // covers: app sdk status
            words("sdk status: --sew is no flag of its", &["sdk", "status", "site", "--sew"], 2),
            // covers: app sdk status
            words("sdk status: not a folder", &["sdk", "status", "m.json"], 2),
            // covers: app sdk kit
            one(
                "sdk kit",
                &["sdk", "kit", "kits/sew-app-kit-1.0.tgz"],
                r#"{"version":"1.0","sha256":"0123456789abcdef0123","packages":[{"name":"@sew/client"},{"name":"@sew/voice"}]}"#,
                0,
            ),
            // covers: app sdk kit --json
            one(
                "sdk kit --json",
                &["sdk", "kit", "--json", "kits/sew-app-kit-1.0.tgz"],
                r#"{"version":"1.0"}"#,
                0,
            ),
            // covers: app sdk kit
            traceback(one(
                "sdk kit: a package name that is a number",
                &["sdk", "kit", "kits/sew-app-kit-1.0.tgz"],
                r#"{"version":"1.0","sha256":"ab","packages":[{"name":1}]}"#,
                1,
            )),
            // covers: app sdk kit
            words("sdk kit: not a file", &["sdk", "kit", "kits"], 2),
            // covers: app sdk kit
            words("sdk kit: no file", &["sdk", "kit"], 2),
            words("sdk alone is its page", &["sdk"], 0),
            words("sdk help is its page", &["sdk", "help"], 0),
            words("sdk: an unknown verb", &["sdk", "nope"], 2),
        ],
    );
}

const CONFIRM: &str = r#"{"error":{"code":"vault_confirm_required","hint":"confirm","serviceId":"svc-1","action":"app-recording-widen"}}"#;
const REC: &str = r#"{"app":{"name":"Acme é","serviceId":"svc-1"},"flags":{"appRecording":true,"voiceRecordCap":true,"recordingRequired":false,"mgmtRecordingReads":true},"recording":{"start_level":"admin","required":true,"server_read":false,"audio_max_days":30},"orgAudioMaxDays":90,"delivered":true,"spaces":[{"spaceId":"sp-1","governance":"app","locked":true},{"spaceId":"sp-2","startLevel":"member","required":false},{"spaceId":"sp-3"}]}"#;

#[test]
fn recording_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "recording");
    check(
        &rig,
        vec![
            // covers: app recording
            one("recording", &["recording", "acme"], REC, 0),
            // covers: app recording
            one(
                "recordings: Sew can't, nothing set, no Spaces",
                &["recordings", "acme"],
                r#"{"app":{"serviceId":"svc-1"},"orgAudioMaxDays":90}"#,
                0,
            ),
            // covers: app recording
            one(
                "recording: the home client",
                &["recording", "acme"],
                r#"{"app":{"name":"Home"},"flags":{"appRecording":true},"homeClient":true}"#,
                0,
            ),
            // covers: app recording --json
            one(
                "recording --json",
                &["recording", "--json", "acme"],
                r#"{"app":{},"é":1}"#,
                0,
            ),
            // covers: app recording
            traceback(one(
                "recording: Spaces that are text",
                &["recording", "acme"],
                r#"{"app":{"name":"a"},"spaces":"sp"}"#,
                1,
            )),
            // covers: app recording
            one(
                "recording set, every setting",
                &[
                    "recording",
                    "acme",
                    "set",
                    "--start-level",
                    "admin",
                    "--audio-max-days",
                    "030d",
                    "--required=on",
                    "--server-read",
                    "no",
                ],
                &REC.replacen('{', r#"{"changed":true,"#, 1),
                0,
            ),
            // covers: app recording --json
            one(
                "recording set --audio-max-days org --json",
                &[
                    "recording",
                    "set",
                    "acme",
                    "--audio-max-days=org",
                    "--json",
                ],
                r#"{"changed":false}"#,
                0,
            ),
            // covers: app recording
            one(
                "recording set: nothing changed",
                &["recording", "acme", "set", "--start-level", "none"],
                r#"{"app":{"name":"a"},"changed":false,"recording":{}}"#,
                0,
            ),
            // covers: app recording
            case(
                "recording set: a confirm, no vault socket",
                &["recording", "acme", "set", "--required", "on"],
                &[],
                vec![answer(403, CONFIRM)],
                files,
                3,
                1,
            ),
            // covers: app recording
            case(
                "recording set: a confirm with no action",
                &["recording", "acme", "set", "--required", "on"],
                &[],
                vec![answer(
                    403,
                    r#"{"error":{"code":"owner_confirm_required","serviceId":"svc-1"}}"#,
                )],
                files,
                3,
                1,
            ),
            // covers: app recording
            in_session(case(
                "recording set: in a session, no confirm",
                &["recording", "acme", "set", "--required", "on"],
                &[],
                vec![answer(403, CONFIRM)],
                files,
                3,
                1,
            )),
            // covers: app recording
            traceback(case(
                "recording set: a confirm whose error is a list",
                &["recording", "acme", "set", "--required", "on"],
                &[],
                vec![answer(403, r#"{"error":["vault_confirm_required"]}"#)],
                files,
                1,
                1,
            )),
            // covers: app recording
            words("recording with no App: the page", &["recording"], 2),
            // covers: app recording
            words("recording: two Apps", &["recording", "a", "b"], 2),
            // covers: app recording
            words(
                "recording: a change with no set",
                &["recording", "a", "--required", "on"],
                2,
            ),
            // covers: app recording
            words("recording set with no change", &["recording", "a", "set"], 2),
            // covers: app recording
            words(
                "recording set: a level",
                &["recording", "a", "set", "--start-level", "owner"],
                2,
            ),
            // covers: app recording
            words(
                "recording set: no days",
                &["recording", "a", "set", "--audio-max-days", "0"],
                2,
            ),
            // covers: app recording
            words(
                "recording set: too many days",
                &["recording", "a", "set", "--audio-max-days", "3651"],
                2,
            ),
            // covers: app recording
            words(
                "recording set: days that are words",
                &["recording", "a", "set", "--audio-max-days="],
                2,
            ),
            // covers: app recording
            words(
                "recording set: required inherit",
                &["recording", "a", "set", "--required", "inherit"],
                2,
            ),
            // covers: app recording
            words(
                "recording set: server-read maybe",
                &["recording", "a", "set", "--server-read=maybe"],
                2,
            ),
            // covers: app recording
            words(
                "recording: --server-read as the last word",
                &["recording", "a", "set", "--server-read"],
                2,
            ),
            // covers: app recording
            words(
                "recording: --start-level as the last word",
                &["recording", "a", "--start-level"],
                2,
            ),
            // covers: app recording
            words("recording: an unknown flag", &["recording", "a", "--zap"], 2),
        ],
    );
}

#[test]
fn room_names_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "roomnames");
    check(
        &rig,
        vec![
            // covers: app room-names
            one(
                "room-names hide",
                &["room-names", "sales", "hide"],
                r#"{"workspace":"sales","namesHidden":true,"changed":true}"#,
                0,
            ),
            // covers: app room-names
            one(
                "room-names show, already",
                &["room-names", "sales", "show"],
                r#"{"workspace":"sales","namesHidden":false}"#,
                0,
            ),
            // covers: app room-names
            one(
                "room-names --json",
                &["room-names", "--json", "sales", "show"],
                r#"{"workspace":"sales","é":1}"#,
                0,
            ),
            // covers: app room-names
            traceback(one(
                "room-names: no namesHidden",
                &["room-names", "sales", "hide"],
                r#"{"workspace":"sales"}"#,
                1,
            )),
            // covers: app room-names
            in_session(case(
                "room-names in a session: the owner's",
                &["room-names", "sales", "hide"],
                &[],
                vec![],
                files,
                3,
                0,
            )),
            // covers: app room-names
            words("room-names: no mode", &["room-names", "sales"], 2),
            // covers: app room-names
            words(
                "room-names: a mode that isn't one",
                &["room-names", "sales", "on"],
                2,
            ),
            // covers: app room-names
            words("room-names: an unknown flag", &["room-names", "-x"], 2),
        ],
    );
}

/// Help never acts: a write verb with `-h` or `--help` anywhere on the
/// line prints help and sends nothing.
#[test]
fn runtime_help_never_acts() {
    let rig = Rig::new(VERB, "rthelp");
    let rows: [&'static [&'static str]; 8] = [
        &["recording", "acme", "set", "--required", "on", "-h"],
        &["recording", "--help", "acme", "set", "--start-level", "admin"],
        &["room-names", "sales", "hide", "-h"],
        &["room-names", "--help", "sales", "show"],
        &["sdk", "add", "site", "--sew", "-h"],
        &["sdk", "kit", "--help", "kits/sew-app-kit-1.0.tgz"],
        &["validate", "site", "-h"],
        &["status", "acme", "--help"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], files, 0, 0)
        });
    }
}
