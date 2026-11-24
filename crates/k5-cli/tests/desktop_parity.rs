//! `k5-cli desktop` against the shell CLI's `cli/k2 desktop`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every form.
//!
//! - The help (`help`, `--help`, `-h` wherever the word reader reaches
//!   it), behind the guard and the gate, and `env`'s in a session with no
//!   passport (`env` is an `id` tool; the other verbs are no tool's).
//! - The words: each flag with no value, the flags that never work, an
//!   unknown flag and verb, each flag on a verb it doesn't go with, and
//!   each verb's own checks.
//! - Every verb with `--json` and without, with every line its view can
//!   print; a refusal (403 with `Ask with:`, `desktop_people_only`, the
//!   desktops that could be closed, a string error, no error, not JSON);
//!   odd answers its Python can't read; many rows, and an answer above
//!   256 KiB (`audit --json`; the views' Python reads the answer from an
//!   environment variable, which Linux caps at 128 KiB, so the rows
//!   without `--json` stay under it).
//! - `setup`'s wait (installed, failed, `--no-wait`, `--timeout`) and
//!   `env`'s (ready, on its way then ready, still waiting at
//!   `--timeout`, `--release`).
//! - A passport over TCP, the cell socket, no answer and no daemon.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing, a help flag right after a value flag too
//!   (`cmd_desktop` reads `_wants_help` first now; it used to take it as
//!   the flag's value and send).
//! - A `--timeout` that isn't digits and one unit letter is the usage
//!   line before anything is sent (0f25406366); a leading 0 is decimal.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "desktop";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

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

/// The status of a virtual desktop, every line it can print.
fn status() -> String {
    r#"{"enabled":true,"machine":"box é","desktop":"virtual","name":"Acme","kind":"agent","state":"running","message":"up",
        "backend":"xvfb","lane":"x11",
        "macPermission":{"step":"screen","screen":"denied","accessibility":"granted","line":"Allow it in System Settings","checking":true},
        "virtual":{"size":{"width":1600,"height":900},"memoryBytes":524288000,"present":[{"user":"ann"},{"user":"bob"}],
          "hold":{"by":"agent-a","until":1700000000},"nextSleepAt":1700000600,"nextCloseAt":1700086400,"canSleep":false,
          "waiting":{"position":2,"awakeLimit":3},
          "limits":{"desktops":{"used":1,"max":4},"awake":{"used":1,"max":2},"memoryBytes":{"max":4294967296,"each":1073741824},"cpuCores":2},
          "label":"Acme's desktop","manual":"apt install xvfb","suggestRecycle":true},
        "you":{"role":"owner","mayManage":true,"mayStart":true,"mayView":true},
        "hideApps":["com.acme.mail","com.acme.pass"],
        "grants":[{"user":"carol","mode":"view"}],
        "viewers":[{"user":"ann","mode":"view","ingress":"lan"}]}"#
        .to_string()
}

/// The console desktop of this computer, the plain lines.
fn console() -> String {
    r#"{"enabled":false,"desktop":"console","state":"off","backend":"screencapturekit","lane":"mac","you":{"sameMachine":true,"role":"owner","mayManage":true},"hideApps":[],"grants":[],"viewers":[]}"#.to_string()
}

/// `n` lines of the access log.
fn audit(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"ts":"2026-11-20T10:{:02}:00Z","event":"view.open","who":"user{i}","ingress":"lan","outcome":"ok","target":"virtual","ip":"10.0.0.{}"{}}}"#,
                i % 60,
                i % 250,
                if i % 3 == 0 { r#","mode":"view é""# } else { "" }
            )
        })
        .collect();
    format!(r#"{{"lines":[{}]}}"#, rows.join(","))
}

#[test]
fn desktop_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        ("help", &["help"][..]),
        ("--help", &["--help"]),
        ("-h after a verb", &["status", "-h"]),
        (
            "--help after the words",
            &["grant", "ann", "--view", "--help"],
        ),
        ("an unknown verb's --help", &["nosuch", "--help"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(case(
        "the help with no daemon set up: the gate stands",
        &["--help"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "help behind the virtual desktop's guard",
        &["help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "env's help in a session with no passport",
        &["env", "--help"],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    for (name, args) in [
        ("--desktop at the end", &["status", "--desktop"][..]),
        ("--size at the end", &["start", "--size"]),
        ("--timeout at the end", &["setup", "--timeout"]),
        ("--open at the end", &["ask", "--open"]),
        ("--workspace at the end", &["allow", "--workspace"]),
        ("--since at the end", &["audit", "--since"]),
        ("--limit at the end", &["audit", "--limit"]),
        ("--display at the end", &["open", "--display"]),
        ("--hide-app at the end", &["policy", "--hide-app"]),
        ("--control", &["grant", "ann", "--control"]),
        ("--ttl=1d", &["grant", "ann", "--ttl=1d"]),
        ("--consent", &["policy", "--consent"]),
        ("--active-window=x", &["policy", "--active-window=x"]),
        ("an unknown flag", &["status", "--bogus"]),
        ("a lone dash", &["status", "-"]),
        ("an unknown verb", &["nosuch"]),
        ("--size on status", &["status", "--size", "1x1"]),
        ("--no-wait on start", &["start", "--no-wait"]),
        ("--timeout on status", &["status", "--timeout", "2m"]),
        ("--release on setup", &["setup", "--release"]),
        ("--workspace on status", &["status", "--workspace", "w"]),
        ("--open on status", &["status", "--open", "screen"]),
        ("--check on list", &["list", "--check"]),
        ("start --size bad", &["start", "--size", "16x"]),
        ("ask with a word", &["ask", "x"]),
        (
            "ask --open and --check",
            &["ask", "--open", "screen", "--check"],
        ),
        ("ask --open elsewhere", &["ask", "--open", "dock"]),
        ("policy with nothing", &["policy"]),
        (
            "policy, both",
            &["policy", "--no-hide-apps", "--hide-app", "a"],
        ),
        ("grant with no login", &["grant"]),
        ("grant with two", &["grant", "a", "b"]),
        ("revoke with no login", &["revoke"]),
        ("kick with no one", &["kick"]),
        ("audit --limit x", &["audit", "--limit", "5x"]),
        ("env with a word", &["env", "x"]),
        ("allow with --desktop", &["allow", "--desktop", "v"]),
        ("allow with two words", &["allow", "on", "off"]),
        ("allow maybe", &["allow", "maybe"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    v.push(case(
        "a usage error with no daemon set up: the gate first",
        &["nosuch"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "the guard",
        &["status"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn desktop_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let v = vec![
        // covers: desktop
        case(
            "status, no verb: every line",
            &[],
            &[],
            vec![ok(&status())],
            owner_and_port,
            0,
            1,
        ),
        // covers: desktop --json
        case(
            "--json first is status",
            &["--json"],
            &[],
            vec![ok("{\"state\":\"off\"}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status of the console, --desktop",
            &["status", "--desktop", "virtual:ws 1/é"],
            &[],
            vec![ok(&console())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status with views only",
            &["status"],
            &[],
            vec![ok(
                r#"{"enabled":true,"state":"running","virtual":{"canSleep":true,"present":[]},"you":{"mayView":true},"macPermission":{"step":"ready"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: may drive",
            &["status"],
            &[],
            vec![ok(
                r#"{"you":{"mayControl":true,"role":"admin"},"virtual":null}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "start --size",
            &["start", "--size", "1600X900", "--desktop", "virtual"],
            &[],
            vec![ok(&status())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "start --json",
            &["start", "--json"],
            &[],
            vec![ok(r#"{"state":"starting"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list",
            &["list"],
            &[],
            vec![ok(
                r#"{"desktops":[{"id":"virtual","state":"running","name":"This server","label":"x"},{"id":"virtual:ws_é"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, none",
            &["list"],
            &[],
            vec![ok(r#"{"desktops":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stop",
            &["stop", "--desktop", "virtual"],
            &[],
            vec![ok(r#"{"closed":true,"message":"bye"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "close: it wasn't running",
            &["close"],
            &[],
            vec![ok(r#"{"closed":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable",
            &["enable"],
            &[],
            vec![ok(
                r#"{"enabled":true,"changed":true,"state":"ready","card":"Scan this:\n[]","macPermission":{"step":"screen","screen":"denied"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "on, it already was",
            &["on", "--desktop", "console"],
            &[],
            vec![ok(r#"{"enabled":true,"state":"ready","message":"m"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "disable",
            &["disable"],
            &[],
            vec![ok(r#"{"enabled":false,"changed":true,"closed":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "off --json",
            &["off", "--json"],
            &[],
            vec![ok(r#"{"enabled":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ask",
            &["ask"],
            &[],
            vec![ok(r#"{"asked":"prompt"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ask --open accessibility",
            &["ask", "--open", "accessibility"],
            &[],
            vec![ok(
                r#"{"asked":"open_accessibility","macPermission":{"step":"accessibility","screen":"granted","accessibility":"denied"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ask --check",
            &["ask", "--check"],
            &[],
            vec![ok(r#"{"asked":"other"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "policy --hide-app",
            &["policy", "--hide-app", "com.acme.é", "--hide-app=b"],
            &[],
            vec![ok(r#"{"hideApps":["com.acme.é","b"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "policy --no-hide-apps",
            &["policy", "--no-hide-apps"],
            &[],
            vec![ok(r#"{"hideApps":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "grant",
            &["grant", "ann é", "--view", "--desktop", "virtual"],
            &[],
            vec![ok(r#"{"grant":{"user":"ann é","mode":"view"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "revoke",
            &["revoke", "ann"],
            &[],
            vec![ok(r#"{"revoked":true,"closed":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "revoke, no grant",
            &["revoke", "ann"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "grants",
            &["grants", "--desktop", "v"],
            &[],
            vec![ok(
                r#"{"grants":[{"user":"ann","mode":"view","grantedBy":"owner"},{"user":"bob","mode":"view"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "grants, none",
            &["grants"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "viewers",
            &["viewers"],
            &[],
            vec![ok(
                r#"{"viewers":[{"id":"dv_1","user":"ann","mode":"view","ingress":"lan","ip":"10.0.0.2","since":1700000000123},{"id":"dv_2","user":"bob","mode":"control","ingress":"front","since":"x"},{"id":"dv_3","user":"c","mode":"view","ingress":"lan"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "viewers, none",
            &["viewers"],
            &[],
            vec![ok(r#"{"viewers":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kick a viewer",
            &["kick", "dv_1"],
            &[],
            vec![ok(r#"{"kicked":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kick a login",
            &["kick", "ann", "--desktop", "v"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit",
            &["audit", "--since", "1d é", "--limit", "7"],
            &[],
            vec![ok(&audit(5))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit, none",
            &["audit"],
            &[],
            vec![ok(r#"{"lines":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit, many lines (under the environment's 128 KiB)",
            &["audit"],
            &[],
            vec![ok(&audit(500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "audit --json above 256 KiB",
            &["audit", "--json"],
            &[],
            vec![ok(&audit(2500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "open",
            &["open", "--display", "2"],
            &[],
            vec![ok(r#"{"for":"ann"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "displays",
            &["displays"],
            &[],
            vec![ok(
                r#"{"displays":[{"id":"1","name":"Built-in","widthPt":1512,"heightPt":982,"main":true},{"id":"2","name":"LG é","widthPt":2560,"heightPt":1440}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "displays, none",
            &["displays"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "allow on",
            &["allow", "on", "--workspace", "/w/acme"],
            &[],
            vec![ok(r#"{"workspace":"/w/acme","agentsCanUseDesktop":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "allow, this workspace's status",
            &["allow"],
            &[],
            vec![ok(r#"{"agentsAllowed":false,"name":"acme"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "allow status --json",
            &["allow", "status", "--json"],
            &[],
            vec![ok(r#"{"agentsAllowed":true,"workspaceId":"w1"}"#)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn desktop_refusals_and_odd_answers() {
    let rig = Rig::new(VERB, "refused");
    let v = vec![
        case(
            "403 with Ask with:",
            &["grant", "ann"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "people only",
            &["status", "--json"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"desktop_people_only","hint":"a person's"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // 0.46.18: while agents may report, the daemon flags the error and
        // the feedback line closes it.
        case(
            "people only, agents may report: the feedback line",
            &["status"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"desktop_people_only","hint":"a person's"},"feedback":true}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "too many desktops: the ones to close",
            &["start"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"desktop_limit"},"closable":["virtual:a","virtual:b"],"permission":{"requestable":false}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a string error",
            &["list"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad desktop"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "no error in it",
            &["viewers"],
            &[],
            vec![Reply::Answer(404, r#"{"x":1}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "not JSON",
            &["displays", "--json"],
            &[],
            vec![Reply::Answer(500, "  oops\n".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "201 is not 200",
            &["stop"],
            &[],
            vec![Reply::Answer(201, r#"{"closed":true}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a refusal that is a list",
            &["kick", "dv_1"],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a 200 that is not JSON",
            &["status"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a 200 that is a list",
            &["grants"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a viewer with no id",
            &["viewers"],
            &[],
            vec![ok(r#"{"viewers":[{"user":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a size with no height",
            &["status"],
            &[],
            vec![ok(r#"{"virtual":{"size":{"width":1}}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a present viewer with no user",
            &["status"],
            &[],
            vec![ok(r#"{"virtual":{"present":[{}]}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "memory as text",
            &["status"],
            &[],
            vec![ok(r#"{"virtual":{"memoryBytes":"1"}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a grant with no mode",
            &["status"],
            &[],
            vec![ok(r#"{"grants":[{"user":"a"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "ask, a list asked",
            &["ask"],
            &[],
            vec![ok(r#"{"asked":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "setup's view raises: set -e ends it",
            &["setup"],
            &[],
            vec![ok(r#"{"state":"closed","missing":5}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn desktop_setup_and_env_wait_as_the_shell_does() {
    let rig = Rig::new(VERB, "wait");
    let installing = || ok(r#"{"state":"setting_up","message":"installing"}"#);
    let v = vec![
        case(
            "setup: installed at once",
            &["setup"],
            &[],
            vec![ok(r#"{"state":"closed","ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "setup: still missing",
            &["setup"],
            &[],
            vec![ok(r#"{"state":"not_set_up","missing":["xvfb","x11vnc"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "setup --no-wait while it installs",
            &["setup", "--no-wait"],
            &[],
            vec![installing()],
            owner_and_port,
            0,
            1,
        ),
        case(
            "setup --no-wait --json",
            &["setup", "--no-wait", "--json"],
            &[],
            vec![installing()],
            owner_and_port,
            0,
            1,
        ),
        case(
            "setup: installs, then is done",
            &["setup", "--desktop", "ignored"],
            &[],
            vec![installing(), ok(r#"{"state":"closed"}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "setup --json: it fails",
            &["setup", "--json", "--timeout", "1m"],
            &[],
            vec![installing(), ok(r#"{"state":"not_set_up"}"#)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "setup: it fails, with why",
            &["setup", "--timeout=90s"],
            &[],
            vec![
                installing(),
                ok(r#"{"state":"not_set_up","message":"apt is locked"}"#),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "setup --timeout 2: still installing",
            &["setup", "--timeout", "2"],
            &[],
            vec![installing(), installing()],
            owner_and_port,
            1,
            2,
        ),
        case(
            "setup: a refusal while it waits",
            &["setup"],
            &[],
            vec![installing(), Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            2,
        ),
        case(
            "setup: no answer while it waits",
            &["setup"],
            &[],
            vec![installing(), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "setup: refused at once",
            &["setup", "--json"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "env: ready",
            &["env"],
            &[],
            vec![ok(
                r#"{"desktop":"virtual:ws1","name":"Acme","state":"running","env":{"DISPLAY":":101","XAUTHORITY":"/home/a b/.Xauth","K2_DESKTOP_PROFILE":""},"hold":{"until":1700000000},"label":"Shared"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "env: on its way, then ready",
            &["env", "--timeout", "1m"],
            &[],
            vec![
                ok(r#"{"state":"starting","message":"booting"}"#),
                ok(
                    r#"{"desktop":"virtual:ws1","state":"running","message":"up","env":{"DISPLAY":":7"}}"#,
                ),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "env --timeout 0s: still waiting",
            &["env", "--timeout", "0s", "--json"],
            &[],
            vec![ok(r#"{"state":"waiting","position":2}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "env --release",
            &["env", "--release", "--desktop", "virtual:ws1"],
            &[],
            vec![ok(r#"{"desktop":"virtual:ws1","released":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "env --release: nothing held",
            &["env", "--release"],
            &[],
            vec![ok(r#"{"desktop":"virtual:ws1","released":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "env --json",
            &["env", "--json"],
            &[],
            vec![ok(r#"{"state":"running"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "env: refused",
            &["env"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        // A bad --timeout: the usage line, nothing sent (0f25406366).
        case(
            "env --timeout x",
            &["env", "--timeout", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "env --timeout that names a variable",
            &["env", "--timeout", "HOMEm", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "env --timeout with words: the words' line first",
            &["env", "x", "--timeout", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "setup --timeout x",
            &["setup", "--timeout", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "setup --timeout 1.5m",
            &["setup", "--timeout", "1.5m", "--no-wait"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "setup --timeout 00s: a leading 0 is decimal",
            &["setup", "--timeout", "00s"],
            &[],
            vec![installing(), installing()],
            owner_and_port,
            1,
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn desktop_with_a_passport_and_no_answer() {
    let rig = Rig::new(VERB, "pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let port = dead_port();
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a host session: the passport over TCP",
                &["env"],
                &[],
                vec![ok(r#"{"state":"running"}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket: a GET",
                &["grants"],
                &[],
                vec![ok(r#"{"grants":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "not served there: a POST goes on to TCP",
                &["env"],
                &[],
                vec![not_served(), ok(r#"{"state":"running"}"#)],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(),
            case(
                "no answer there: a POST is never sent again",
                &["revoke", "ann"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        case(
            "an empty reply",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["enable", "--json"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "env: nothing listens",
            &["env"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: each verb that changes something, with `-h` or
/// `--help`, prints the help and sends nothing.
#[test]
fn desktop_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for (name, args) in [
        ("start -h", &["start", "-h"][..]),
        ("stop --help", &["stop", "--help"]),
        ("close -h", &["close", "--desktop", "v", "-h"]),
        ("setup -h", &["setup", "-h"]),
        ("enable --help", &["enable", "--help"]),
        ("on -h", &["on", "-h"]),
        ("ask -h", &["ask", "--check", "-h"]),
        ("disable -h", &["disable", "-h"]),
        ("off --help", &["off", "--json", "--help"]),
        ("policy -h", &["policy", "--hide-app", "a", "-h"]),
        ("grant -h", &["grant", "ann", "-h"]),
        ("revoke --help", &["revoke", "ann", "--help"]),
        ("kick -h", &["kick", "dv_1", "-h"]),
        ("open -h", &["open", "-h"]),
        ("env --help", &["env", "--release", "--help"]),
        ("allow on -h", &["allow", "on", "-h"]),
        ("-h before the verb", &["-h", "start"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(shell(case(
        "grant ann --desktop -h: the help, nothing sent",
        &["grant", "ann", "--desktop", "-h"],
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    )));
    v.push(shell(case(
        "policy --hide-app --help: the help, nothing sent",
        &["policy", "--hide-app", "--help"],
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    )));
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn desktop_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["nosuch"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr
            .starts_with("usage: unknown desktop verb 'nosuch'"),
        "{}",
        out.stderr
    );
    assert!(log.is_empty());
    let c = case(
        "brand: grant's usage",
        &["grant"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: {cli} desktop grant <login> [--view]\n")
    );
    let c = case(
        "brand: no answer",
        &["status"],
        &[],
        vec![Reply::Close],
        owner_and_port,
        1,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(out.stderr, format!("{cli} desktop: daemon unreachable\n"));
}
