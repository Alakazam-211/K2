//! `k5-cli app doctor|box|engine|rehost|sessions` against the shell CLI's
//! (`cli/src/app-doctor.sh`, `cli/src/app-runtime.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `doctor` and `box [doctor|status]`: ready (0), not ready (1), not
//!   Linux, automatic prep, legacy services, `--json`; `doctor <app>`
//!   (the reach doctor) and its `--json`; the words.
//! - `box podman uninstall`, `box unprep` (the plan, `--dry-run`, no
//!   terminal, `--yes`), `box auto-prepare on|off|status|retry`, `box
//!   prepare` (done, `--dry-run`, failed steps, `--json`, the words).
//! - `engine install|status`, the page; `rehost` (one App, `status`,
//!   `policy`), `sessions --live [--count]`.
//! - A list above 256 KiB; help never acts.

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

fn files(rig: &Rig) {
    owner_and_port(rig);
    std::fs::create_dir_all(rig.proj.join("engine-dir")).unwrap();
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn one(name: &'static str, args: &'static [&'static str], r: Reply, code: i32) -> Case {
    case(name, args, &[], vec![r], files, code, 1)
}

fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], files, code, 0)
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

const REFUSED_403: &str = r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#;
const BOX: &str = r#"{"ready":true,"box":{"ready":true,"runtime":"crun","runtimeRecorded":true,"checks":[{"status":"ok","id":"podman","detail":"5.0"},{"status":"fail","id":"subuid","detail":"none é","fix":"prepare"},{"status":"ok","id":"x","fix":"never shown"}]},"autoPrepare":{"enabled":true,"outcome":{"state":"done","changed":["a","b"]},"undoneAt":0,"blocked":true},"legacyServices":[{"name":"blog","workspaceHandle":"ann","label":"svc","hint":"move it"},{"name":"x"}]}"#;

#[test]
fn doctors_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "doctor");
    let reach = r#"{"app":"acme","wall":"guarded","reach":{"running":true,"ok":false,"rule":{"gwPort":4100,"boxPorts":[5432,6379],"dbPorts":[]},"dbDoor":{"inWallPort":15432,"openSessions":2,"databases":["acme","b"]},"findings":[{"id":"db-port","detail":"not applied é"}]}}"#;
    check(
        &rig,
        vec![
            // covers: app doctor
            one("doctor: ready", &["doctor"], ok(BOX), 0),
            // covers: app doctor
            one(
                "doctor: not ready, the box at the top",
                &["doctor"],
                ok(
                    r#"{"ready":false,"runtime":"youki","autoPrepare":{"enabled":false,"outcome":{"why":"no root"},"undoneAt":5}}"#,
                ),
                1,
            ),
            // covers: app doctor
            one(
                "doctor: not Linux",
                &["doctor"],
                ok(r#"{"box":{"linux":false,"ready":true},"autoPrepare":{"enabled":true}}"#),
                1,
            ),
            // covers: app doctor --json
            one("doctor --json", &["doctor", "--json"], ok(BOX), 0),
            // covers: app doctor
            traceback(one(
                "doctor: a fix that is a number",
                &["doctor"],
                ok(r#"{"checks":[{"status":"warn","fix":3}]}"#),
                1,
            )),
            // covers: app doctor
            one("doctor refused", &["doctor"], answer(403, REFUSED_403), 3),
            // covers: app doctor
            one("doctor <app>: the reach", &["doctor", "acme"], ok(reach), 1),
            // covers: app doctor
            one(
                "doctor <app>: all applied, the whole band",
                &["doctor", "acme"],
                ok(r#"{"app":"acme","reach":{"rule":{"gwPort":1}}}"#),
                0,
            ),
            // covers: app doctor --json
            one(
                "doctor <app> --json",
                &["doctor", "acme", "--json"],
                ok(reach),
                1,
            ),
            // covers: app doctor --json
            traceback(one(
                "doctor <app> --json: a reach that is a list",
                &["doctor", "--json", "acme"],
                ok(r#"{"reach":[1]}"#),
                1,
            )),
            // covers: app doctor
            words("doctor --voice without --store", &["doctor", "--voice"], 2),
            // covers: app doctor
            words("doctor, two Apps", &["doctor", "a", "b"], 2),
            // covers: app doctor
            words("doctor, an unknown flag", &["doctor", "--zap"], 2),
            // covers: app box
            one("box", &["box"], ok(BOX), 0),
            // covers: app box doctor
            one(
                "box doctor --json",
                &["box", "doctor", "--json"],
                ok(BOX),
                0,
            ),
            // covers: app box doctor
            one("box status", &["box", "status"], ok(r#"{"ready":1}"#), 0),
            // covers: app box
            words("box, an unknown verb", &["box", "zap"], 2),
        ],
    );
}

#[test]
fn box_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "boxverbs");
    let plan =
        r#"{"undoes":["the cgroup slice","subuid é"],"kept":["podman"],"runningApps":["acme"]}"#;
    let undone = r#"{"result":{"steps":[{"step":"slice","changed":true},{"step":"subuid"}],"kept":["x"]},"note":"done","box":{"ready":false}}"#;
    let dry = r#"{"podman":{"action":"install","have":"4.9","os":"ubuntu 24.04","bundle":{"version":"5.2","source":"k2","url":"https://x/p.tgz","sha256":"ab","size":10,"writes":["/opt/a","/opt/b"]}},"packages":{"manager":"apt","install":["uidmap","crun"],"alreadyInstalled":["x"],"distroPodman":"4.9 (too old)","note":"apt note"},"steps":["slice","subuid"],"box":{"ready":true}}"#;
    let done = r#"{"steps":[{"verb":"slice","result":{"changed":true,"apparmor":{"needed":true,"installed":["apparmor"],"parser":"4.0"}}},{"verb":"subuid","result":{"changed":false,"apparmor":{"needed":true,"note":"skip"}}},{"verb":"x","result":"plain"}],"notes":["one é"],"box":{"ready":true,"checks":[{"id":"coresSlice","status":"ok"},{"id":"agentsLeaf","status":"warn"}]}}"#;
    check(
        &rig,
        vec![
            // covers: app box podman uninstall
            one(
                "podman uninstall",
                &["box", "podman", "uninstall"],
                ok(r#"{"result":{"removed":["podman","conmon"],"kept":["/etc/x"]}}"#),
                0,
            ),
            // covers: app box podman uninstall
            one(
                "podman uninstall: nothing",
                &["box", "podman", "uninstall"],
                ok("{}"),
                0,
            ),
            // covers: app box podman uninstall
            one(
                "podman uninstall --json",
                &["box", "podman", "uninstall", "--json"],
                ok("{}"),
                0,
            ),
            // covers: app box podman uninstall
            words("podman, not uninstall", &["box", "podman"], 2),
            // covers: app box unprep
            one(
                "unprep --dry-run",
                &["box", "unprep", "--dry-run"],
                ok(plan),
                0,
            ),
            // covers: app box unprep
            one(
                "unprep --dry-run --json",
                &["box", "unprep", "--json", "--dry-run"],
                ok(plan),
                0,
            ),
            // covers: app box unprep
            one("unprep, no terminal", &["box", "unprep"], ok(plan), 2),
            // covers: app box unprep
            one(
                "unprep --json, no terminal: the plan text",
                &["box", "unprep", "--json"],
                ok(plan),
                2,
            ),
            // covers: app box unprep
            one("unprep --yes", &["box", "unprep", "--yes"], ok(undone), 0),
            // covers: app box unprep
            one(
                "unprep -y --json",
                &["box", "unprep", "-y", "--json"],
                ok(undone),
                0,
            ),
            // covers: app box unprep
            one(
                "unprep refused",
                &["box", "unprep", "--yes"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app box unprep
            words("unprep, an unknown word", &["box", "unprep", "now"], 2),
            // covers: app box auto-prepare
            one(
                "auto-prepare status",
                &["box", "auto-prepare", "status"],
                ok(
                    r#"{"autoPrepare":{"enabled":true,"outcome":{"state":"failed","error":"boom"}}}"#,
                ),
                0,
            ),
            // covers: app box auto-prepare
            one(
                "auto-prepare status --json",
                &["box", "auto-prepare", "status", "--json"],
                ok(r#"{"autoPrepare":{"enabled":true,"x":"é"}}"#),
                0,
            ),
            // covers: app box auto-prepare
            one(
                "auto-prepare status --json: none",
                &["box", "auto-prepare", "status", "--json"],
                ok("{}"),
                0,
            ),
            // covers: app box auto-prepare
            one(
                "auto-prepare on",
                &["box", "auto-prepare", "on"],
                ok(r#"{"autoPrepare":{"enabled":true},"setting":true,"note":"now"}"#),
                0,
            ),
            // covers: app box auto-prepare
            one(
                "auto-prepare off --json",
                &["box", "auto-prepare", "off", "--json"],
                ok("{}"),
                0,
            ),
            // covers: app box auto-prepare
            one(
                "auto-prepare retry",
                &["box", "auto-prepare", "retry"],
                ok("{}"),
                0,
            ),
            // covers: app box auto-prepare
            one(
                "auto-prepare retry refused",
                &["box", "auto-prepare", "retry"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app box auto-prepare
            words("auto-prepare, no verb", &["box", "auto-prepare"], 2),
            // covers: app box prepare
            one(
                "prepare: every flag, done",
                &[
                    "box",
                    "prepare",
                    "--youki",
                    "--cores-mem-min",
                    "512",
                    "--apps-mem-max",
                    "4096",
                    "--apps-tasks-max",
                    "100",
                    "--install-podman",
                    "--podman-from",
                    "engine-dir",
                ],
                ok(done),
                0,
            ),
            // covers: app box prepare
            one(
                "prepare --dry-run",
                &["box", "prepare", "--dry-run", "--runtime", "crun"],
                ok(dry),
                0,
            ),
            // covers: app box prepare
            one(
                "prepare --dry-run: keep, no packages",
                &["box", "prepare", "--dry-run"],
                ok(
                    r#"{"podman":{"action":"keep","have":"5.1","source":"distro"},"packages":{"note":"none here"}}"#,
                ),
                0,
            ),
            // covers: app box prepare
            one(
                "prepare --json",
                &["box", "prepare", "--json", "-y"],
                ok(done),
                0,
            ),
            // covers: app box prepare
            one(
                "prepare: failed steps",
                &["box", "prepare"],
                answer(
                    500,
                    r#"{"error":{"code":"prepare_failed","hint":"a step failed","steps":[{"ok":true,"verb":"slice"},{"verb":"subuid","error":"denied"}]}}"#,
                ),
                3,
            ),
            // covers: app box prepare
            one(
                "prepare --json: failed",
                &["box", "prepare", "--json"],
                answer(
                    409,
                    r#"{"error":{"code":"podman_install_needed","hint":"old podman"}}"#,
                ),
                3,
            ),
            // covers: app box prepare
            one(
                "prepare: podman needed, no terminal",
                &["box", "prepare"],
                answer(
                    409,
                    r#"{"error":{"code":"podman_install_needed","hint":"old podman"}}"#,
                ),
                3,
            ),
            // covers: app box prepare
            words(
                "prepare, a bad runtime",
                &["box", "prepare", "--runtime", "runc"],
                2,
            ),
            // covers: app box prepare
            words(
                "prepare, a size with a unit",
                &["box", "prepare", "--apps-mem-max", "4G"],
                2,
            ),
            // covers: app box prepare
            words(
                "prepare --runtime last: set -e",
                &["box", "prepare", "--runtime"],
                1,
            ),
        ],
    );
}

#[test]
fn engine_rehost_sessions_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "engine");
    let wave = r#"{"k2Version":"0.46.17","state":"running","reason":"an update é","apps":{"zeta":{"state":"done"},"acme":{"state":"failed","code":"E1","detail":"boom"}}}"#;
    let one_app = r#"{"app":"acme","rehost":{"policy":"auto","whenNoOverlap":"pause","fingerprint":{"k2Version":"0.46.16"},"due":true,"sessions":3,"wave":{"state":"done","code":"ok","detail":"fine","mode":"overlap","forced":true}}}"#;
    check(
        &rig,
        vec![
            // covers: app engine install
            one(
                "engine install --from",
                &["engine", "install", "--from", "engine-dir"],
                ok(r#"{"engine":{"version":"1.2","protocol":3,"runtime":"crun","podman":"5.2"}}"#),
                0,
            ),
            // covers: app engine install
            one(
                "engine install --json",
                &["engine", "install", "--json"],
                ok("{}"),
                0,
            ),
            // covers: app engine install
            one(
                "engine install refused",
                &["engine", "install"],
                answer(403, REFUSED_403),
                3,
            ),
            // covers: app engine install
            words("engine install, a word", &["engine", "install", "now"], 2),
            // covers: app engine status
            one(
                "engine status",
                &["engine", "status"],
                ok(
                    r#"{"state":"running","want":{"version":"1.2","protocol":3},"installed":{"version":"1.1"},"running":null,"linger":false,"heal":{"state":"healing","why":"old"},"apps":{"waitingEngine":2}}"#,
                ),
                0,
            ),
            // covers: app engine status
            one(
                "engine status: absent",
                &["engine", "status"],
                ok(r#"{"state":"absent"}"#),
                0,
            ),
            // covers: app engine status
            one(
                "engine status --json",
                &["engine", "status", "--json"],
                ok(r#"{"state":"x"}"#),
                0,
            ),
            // covers: app engine status
            traceback(one(
                "engine status: a want that is a string",
                &["engine", "status"],
                ok(r#"{"state":"ok","want":"x"}"#),
                1,
            )),
            // covers: app engine
            words("engine: the page", &["engine"], 0),
            // covers: app engine
            words("engine, an unknown verb", &["engine", "zap"], 2),
            // covers: app rehost
            one(
                "rehost <app>: done",
                &["rehost", "acme", "--yes"],
                ok(r#"{"app":"acme","rehost":"done","deployment":{"id":"d-9"},"mode":"pause"}"#),
                0,
            ),
            // covers: app rehost
            one(
                "rehost <app>: current",
                &["rehost", "--pause", "acme"],
                ok(r#"{"rehost":"current"}"#),
                0,
            ),
            // covers: app rehost
            one(
                "rehost <app>: failed",
                &["rehost", "acme"],
                ok(r#"{"rehost":"failed","why":"no room","mode":"overlap"}"#),
                1,
            ),
            // covers: app rehost
            one(
                "rehost --json",
                &["rehost", "acme", "--json"],
                ok(r#"{"rehost":"failed"}"#),
                0,
            ),
            // covers: app rehost
            words("rehost: the page", &["rehost"], 0),
            // covers: app rehost
            words("rehost, an unknown flag", &["rehost", "acme", "--zap"], 2),
            // covers: app rehost status
            one(
                "rehost status: the wave",
                &["rehost", "status"],
                ok(wave),
                0,
            ),
            // covers: app rehost status
            one(
                "rehost status <app>",
                &["rehost", "status", "acme é"],
                ok(one_app),
                0,
            ),
            // covers: app rehost status
            one(
                "rehost status --json",
                &["rehost", "status", "--json"],
                ok(wave),
                0,
            ),
            // covers: app rehost status
            traceback(one(
                "rehost status: apps that is a list",
                &["rehost", "status"],
                ok(r#"{"apps":[1]}"#),
                1,
            )),
            // covers: app rehost policy
            one(
                "rehost policy",
                &[
                    "rehost",
                    "policy",
                    "acme",
                    "--policy",
                    "window",
                    "--window",
                    "02:00-04:00",
                    "--when-no-overlap",
                    "defer",
                ],
                ok(
                    r#"{"app":"acme","rehost":{"policy":"window","window":"02:00-04:00","whenNoOverlap":"defer"},"warning":"late"}"#,
                ),
                0,
            ),
            // covers: app rehost policy
            one(
                "rehost policy --json",
                &["rehost", "policy", "acme", "--json"],
                ok("{}"),
                0,
            ),
            // covers: app rehost policy
            traceback(one(
                "rehost policy: no rehost",
                &["rehost", "policy", "acme"],
                ok("{}"),
                1,
            )),
            // covers: app rehost policy
            words(
                "rehost policy, no App",
                &["rehost", "policy", "--policy", "auto"],
                2,
            ),
            // covers: app sessions
            one(
                "sessions --live",
                &["sessions", "--live"],
                ok(
                    r#"{"connected":1,"signedIn":4,"apps":[{"app":"acme","connected":1,"signedIn":3},{"app":"globex"}]}"#,
                ),
                0,
            ),
            // covers: app sessions
            one(
                "sessions --live: nobody",
                &["sessions", "--live"],
                ok("{}"),
                0,
            ),
            // covers: app sessions
            one(
                "sessions --live --count",
                &["sessions", "--count", "--live"],
                ok(r#"{"connected":2}"#),
                0,
            ),
            // covers: app sessions
            one(
                "sessions --live --json",
                &["sessions", "--live", "--json"],
                ok("{}"),
                0,
            ),
            // covers: app sessions
            words("sessions without --live", &["sessions"], 2),
            // covers: app sessions
            words("sessions, an unknown word", &["sessions", "--live", "x"], 2),
        ],
    );
}

/// One answer above 256 KiB: thousands of checks.
#[test]
fn box_a_large_list() {
    let rig = Rig::new(VERB, "boxbig");
    let rows: Vec<String> = (0..4000)
        .map(|n| {
            format!(r#"{{"status":"ok","id":"check-{n:06}","detail":"a detail line é {n:030}"}}"#)
        })
        .collect();
    let body = format!(r#"{{"ready":true,"checks":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["doctor"][..], &["box", "doctor", "--json"]] {
        rig.check(&case(
            "a large list",
            args,
            &[],
            vec![ok(&body)],
            files,
            0,
            1,
        ));
    }
}

/// Help never acts.
#[test]
fn boxes_help_never_acts() {
    let rig = Rig::new(VERB, "boxhelp");
    let rows: [&'static [&'static str]; 10] = [
        &["box", "prepare", "--yes", "-h"],
        &["box", "unprep", "--yes", "--help"],
        &["box", "auto-prepare", "on", "-h"],
        &["box", "podman", "uninstall", "--help"],
        &["box", "-h"],
        &["engine", "install", "-h"],
        &["rehost", "acme", "--yes", "--help"],
        &["rehost", "policy", "acme", "--policy", "auto", "-h"],
        &["doctor", "-h"],
        &["sessions", "--live", "-h"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], files, 0, 0)
        });
    }
}

/// The lines name the command by the brand's name.
#[test]
fn boxes_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "boxbrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want, code) in [
        (
            &["engine", "x"][..],
            format!("usage: {cli} app engine install [--from DIR] [--json] | {cli} app engine status [--json]\n"),
            2,
        ),
        (
            &["box", "zap"],
            format!("Unknown: {cli} app box zap (doctor|prepare|unprep|auto-prepare on|off|status|retry|podman uninstall)\n"),
            2,
        ),
        (
            &["doctor", "--zap"],
            format!("Unknown flag: --zap ({cli} app doctor --help)\n"),
            2,
        ),
    ] {
        let c = case("brand", args, &[], vec![], files, code, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
