//! `k5-cli app build|install|deploy|rollback|stop|start|restart|link|seal|config`
//! against the shell CLI's (`cli/src/app-doctor.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `build`: the Seed, its size note, the warnings, `--json`, a 422
//!   report (and `--json`), a held build, a refusal, the disk notes, the
//!   folder and `--manifest` checks, Python raising, the words.
//! - `install`: everything set, the least answer, a database already
//!   there, `--json`, held (with and without `frontPort`), the words.
//! - `deploy`: a folder and a Seed, each result's exit, `--json`, the
//!   dev-site note, a 422 report, held, the words.
//! - `rollback`, `stop|start|restart`: the deployment's exit, the state
//!   line, `--json`, a raise in the test for a deployment, the words.
//! - `link`, `seal`: open or not (exit 1), `--rotate`, `rotate --yes`,
//!   `rotate` with no terminal, `--json`.
//! - `config`: every flag into the body, a name given twice, held,
//!   `--json`, the words; a list above 256 KiB.
//! - Help never acts: each verb with `-h` or `--help` anywhere.

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

/// `site` and `seedsrc` are folders; `m.json` a file.
fn files(rig: &Rig) {
    owner_and_port(rig);
    for d in ["site", "seedsrc"] {
        std::fs::create_dir_all(rig.proj.join(d)).unwrap();
    }
    std::fs::write(rig.proj.join("m.json"), "{}\n").unwrap();
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

/// One answer, the folders on disk.
fn one(name: &'static str, args: &'static [&'static str], r: Reply, code: i32) -> Case {
    case(name, args, &[], vec![r], files, code, 1)
}

/// Words that stop before anything is sent.
fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], files, code, 0)
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

const SEED: &str = r#"{"seed":{"hash":"0123456789abcdef0123","seedId":"acme","version":"1.2","sizeBytes":4096,"sourceRef":"abc123","report":{"warnings":[{"code":"W1","message":"big é"},{"message":"no code"}]}},"entries":12,"reused":true,"seedSize":{"bytes":30000000,"files":12,"largest":[{"path":"a.bin","bytes":2048},{"path":"b","bytes":10}]},"diskWarning":{"hint":"disk 90% é"},"memoryWarning":{"nohint":1},"diskOverride":{"applied":true,"via":"flag"}}"#;
const HELD: &str = r#"{"status":"held","request":{"id":"req-1","summary":"a disk override"},"reasons":["disk"],"hint":"ask the owner"}"#;
const REFUSED_403: &str = r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#;
const REPORT: &str = r#"{"error":{"code":"invalid","report":{"ok":false,"errors":[{"code":"E1","at":"manifest.json","message":"bad é"}],"warnings":[{"code":"W","message":"w"}],"checks":["manifest"]}}}"#;
const LIVE: &str = r#"{"deployment":{"kind":"deploy","id":"d-1","fromSeed":"0123456789abcdef","toSeed":"fedcba9876543210","actor":"ann é","commitRef":"c0ffee0123456789","result":"live","port":4100,"appId":"acme"},"instant":true,"seedSize":{"bytes":100,"files":1,"ignoreFile":true},"devSite":"acme-dev","diskWarning":{"hint":"low"}}"#;

#[test]
fn build_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "build");
    check(
        &rig,
        vec![
            // covers: app build
            one("a Seed", &["build", "site"], ok(SEED), 0),
            // covers: app build
            one(
                "the least Seed, --types, --manifest, --disk-override=",
                &[
                    "build",
                    "--types",
                    "site",
                    "--manifest",
                    "m.json",
                    "--disk-override=space é",
                ],
                ok(r#"{"seed":{"hash":"ab","seedId":"x","sizeBytes":1}}"#),
                0,
            ),
            // covers: app build --json
            one("--json", &["build", "site", "--json"], ok(SEED), 0),
            // covers: app build
            one("a 422 report", &["build", "site"], answer(422, REPORT), 1),
            // covers: app build --json
            one(
                "a 422 report, --json",
                &["build", "--json", "site"],
                answer(422, REPORT),
                1,
            ),
            // covers: app build
            one("held", &["build", "site", "--disk-override", "r"], answer(202, HELD), 2),
            // covers: app build --json
            one("held, --json", &["build", "site", "--json"], answer(202, HELD), 2),
            // covers: app build
            one("refused", &["build", "site"], answer(403, REFUSED_403), 3),
            // covers: app build
            one(
                "a Seed in kilobytes, no ignore file, small",
                &["build", "seedsrc"],
                ok(r#"{"seed":{"hash":"0123456789abcdef","seedId":"s","sizeBytes":2},"seedSize":{"bytes":2048.5,"files":3,"largest":[{"path":"x","bytes":5000000000000}]}}"#),
                0,
            ),
            // covers: app build
            traceback(one(
                "no seed: KeyError",
                &["build", "site"],
                ok(r#"{"entries":1}"#),
                1,
            )),
            // covers: app build
            traceback(one(
                "a size that is a string",
                &["build", "site"],
                ok(r#"{"seed":{"hash":"ab","seedId":"x","sizeBytes":1},"seedSize":{"bytes":"x"}}"#),
                1,
            )),
            // covers: app build
            words("no folder", &["build"], 2),
            // covers: app build
            words("not a folder", &["build", "nope é"], 2),
            // covers: app build
            words("--manifest not a file", &["build", "site", "--manifest", "x.json"], 2),
            // covers: app build
            words("an unknown flag", &["build", "site", "--zap"], 2),
            // covers: app build
            words("--manifest last: set -e", &["build", "site", "--manifest"], 1),
        ],
    );
}

#[test]
fn install_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "install");
    check(
        &rig,
        vec![
            // covers: app install
            one(
                "everything",
                &[
                    "install",
                    "acme",
                    "--workspace",
                    "ws é",
                    "--tunnel",
                    "--transfer",
                    "--memory",
                    "8G",
                    "--engine",
                    "guarded",
                    "--seed",
                    "abc",
                    "--pg-major=17",
                    "--disk-override",
                    "why",
                ],
                ok(r#"{"app":{"appId":"acme","memoryLimitBytes":8589934592},"frontPort":4100,"database":{"created":true,"name":"acme","major":17,"ref":"vault:db"},"deploy":{"deployment":{"id":"d-1","result":"live"},"seedSize":{"bytes":2000001,"files":4}},"memoryWarning":{"hint":"tight"}}"#),
                0,
            ),
            // covers: app install
            one(
                "the least: no limit, no deploy",
                &["install", "acme", "--folder", "site", "--pg-major", "16"],
                ok(r#"{"app":{"appId":"acme","memoryLimitBytes":null},"frontPort":1}"#),
                0,
            ),
            // covers: app install
            one(
                "a database already there, a failed deploy",
                &["install", "acme"],
                ok(r#"{"app":{"appId":"acme"},"frontPort":1,"database":{"name":"d","major":16},"deploy":{"deployment":{"id":"d","result":"failed","error":"boom"}}}"#),
                0,
            ),
            // covers: app install --json
            one(
                "--json",
                &["install", "acme", "--json"],
                ok(r#"{"app":{"appId":"acme"},"frontPort":1}"#),
                0,
            ),
            // covers: app install
            one("held", &["install", "acme", "--engine", "process"], answer(202, HELD), 2),
            // covers: app install
            one(
                "a 202 that names frontPort: a refusal",
                &["install", "acme"],
                answer(202, r#"{"frontPort":1,"error":"odd"}"#),
                3,
            ),
            // covers: app install
            one("refused", &["install", "acme"], answer(403, REFUSED_403), 3),
            // covers: app install
            traceback(one(
                "a memory limit that is a float",
                &["install", "acme"],
                ok(r#"{"app":{"appId":"acme","memoryLimitBytes":1.5},"frontPort":1}"#),
                1,
            )),
            // covers: app install
            words("no App", &["install"], 2),
            // covers: app install
            words("--pg-major one digit", &["install", "acme", "--pg-major", "9"], 2),
            // covers: app install
            words("--pg-major=0x", &["install", "acme", "--pg-major=07"], 2),
            // covers: app install
            words("an unknown flag", &["install", "acme", "--zap"], 2),
            // covers: app install
            words("--seed last: set -e", &["install", "acme", "--seed"], 1),
        ],
    );
}

#[test]
fn deploy_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "deploy");
    let pending = r#"{"deployment":{"kind":"deploy","id":"d-2","result":"pending","appId":"acme"}}"#;
    check(
        &rig,
        vec![
            // covers: app deploy
            one("a folder, live", &["deploy", "acme", "site"], ok(LIVE), 0),
            // covers: app deploy
            one(
                "a folder, --manifest, --types, --engine=, --reason",
                &[
                    "deploy",
                    "acme",
                    "site",
                    "--manifest=m.json",
                    "--types",
                    "--engine=guarded",
                    "--reason",
                    "fix é",
                    "--disk-override=d",
                ],
                ok(LIVE),
                0,
            ),
            // covers: app deploy
            one(
                "a Seed, pending: exit 2",
                &["deploy", "acme", "0123abcd", "--no-wait", "--engine", "process"],
                ok(pending),
                2,
            ),
            // covers: app deploy
            one(
                "stalled: exit 3",
                &["deploy", "acme", "s1"],
                ok(r#"{"deployment":{"result":"stalled"}}"#),
                3,
            ),
            // covers: app deploy
            one(
                "failed: exit 1",
                &["deploy", "acme", "s1"],
                ok(r#"{"deployment":{"result":"failed","error":"health"}}"#),
                1,
            ),
            // covers: app deploy
            one("no deployment: exit 1", &["deploy", "acme", "s1"], ok("{}"), 1),
            // covers: app deploy --json
            one("--json, live", &["deploy", "acme", "site", "--json"], ok(LIVE), 0),
            // covers: app deploy --json
            one("--json, pending", &["deploy", "--json", "acme", "s"], ok(pending), 2),
            // covers: app deploy
            one("a 422 report", &["deploy", "acme", "site"], answer(422, REPORT), 1),
            // covers: app deploy
            one("held", &["deploy", "acme", "site"], answer(202, HELD), 2),
            // covers: app deploy
            one("refused", &["deploy", "acme", "s"], answer(403, REFUSED_403), 3),
            // covers: app deploy
            traceback(one(
                "a deployment that is a list",
                &["deploy", "acme", "s"],
                ok(r#"{"deployment":[1]}"#),
                1,
            )),
            // covers: app deploy
            traceback(one("an answer that is a list", &["deploy", "acme", "s"], ok("[1]"), 1)),
            // covers: app deploy
            words("one word", &["deploy", "acme"], 2),
            // covers: app deploy
            words("a bad engine", &["deploy", "acme", "s", "--engine", "x"], 2),
            // covers: app deploy
            words("--manifest with a Seed", &["deploy", "acme", "s", "--manifest", "m.json"], 2),
            // covers: app deploy
            words("--manifest not a file", &["deploy", "acme", "site", "--manifest", "no"], 2),
            // covers: app deploy
            words("--reason last: set -e", &["deploy", "acme", "s", "--reason"], 1),
        ],
    );
}

#[test]
fn rollback_and_lifecycle_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "life");
    check(
        &rig,
        vec![
            // covers: app rollback
            one("rollback, live", &["rollback", "acme"], ok(LIVE), 0),
            // covers: app rollback
            one(
                "rollback --to --no-wait, starting",
                &["rollback", "acme", "--to", "seed é", "--no-wait"],
                ok(r#"{"deployment":{"kind":"rollback","id":"d","result":"starting"}}"#),
                2,
            ),
            // covers: app rollback --json
            one("rollback --json", &["rollback", "acme", "--json"], ok(LIVE), 0),
            // covers: app rollback
            one(
                "rollback refused",
                &["rollback", "acme"],
                answer(404, r#"{"error":{"code":"no_such_app","hint":"none"}}"#),
                3,
            ),
            // covers: app rollback
            words("rollback, no App", &["rollback"], 2),
            // covers: app stop
            one("stop", &["stop", "acme"], ok(r#"{"app":"acme","state":"stopped"}"#), 0),
            // covers: app stop
            one("stop, already", &["stop", "acme"], ok(r#"{"app":"acme","already":"stopped"}"#), 0),
            // covers: app start
            one("start: ok", &["start", "acme"], ok(r#"{"app":["a"]}"#), 0),
            // covers: app restart
            one("restart: a deployment", &["restart", "acme"], ok(LIVE), 0),
            // covers: app restart --json
            one("restart --json: a deployment", &["restart", "acme", "--json"], ok(LIVE), 0),
            // covers: app stop --json
            one("stop --json", &["stop", "--json", "acme"], ok(r#"{"app":"acme"}"#), 0),
            // covers: app start
            traceback(one("start: a number", &["start", "acme"], ok("5"), 1)),
            // covers: app start
            one("start refused", &["start", "acme"], answer(403, REFUSED_403), 3),
            // covers: app stop
            words("stop, no App", &["stop"], 2),
            // covers: app start
            words("start, an unknown flag", &["start", "a", "--zap"], 2),
            // covers: app restart
            words("restart, no App", &["restart", "--json"], 2),
        ],
    );
}

#[test]
fn link_and_seal_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "linkseal");
    let open = r#"{"app":"acme","link":{"open":true,"linkId":"l-1","port":4200,"inWallUrl":"http://gw","passFile":"/p","caps":["a","b"],"rooms":[],"passExpiresAt":1700000000,"rotateAt":0,"rotations":3,"livePasses":1,"lastError":"timeout é","requests":10,"refused":1,"wrongLink":0,"connections":2}}"#;
    check(
        &rig,
        vec![
            // covers: app link
            one("link, open", &["link", "acme"], ok(open), 0),
            // covers: app link
            one(
                "link, not open: exit 1",
                &["link", "acme"],
                ok(r#"{"app":"acme","link":{"open":false}}"#),
                1,
            ),
            // covers: app link
            one(
                "link, no caps",
                &["link", "acme"],
                ok(r#"{"app":"a","link":{"open":1,"caps":[]}}"#),
                0,
            ),
            // covers: app link
            one("link --rotate", &["link", "acme", "--rotate"], ok(open), 0),
            // covers: app link --json
            one("link --json, open", &["link", "acme", "--json"], ok(open), 0),
            // covers: app link --json
            one("link --json, none", &["link", "--json", "acme"], ok("{}"), 1),
            // covers: app link
            one("link refused", &["link", "acme", "--rotate"], answer(403, REFUSED_403), 3),
            // covers: app link
            traceback(one(
                "link: caps not strings",
                &["link", "acme"],
                ok(r#"{"link":{"open":true,"caps":[1]}}"#),
                1,
            )),
            // covers: app link
            words("link, no App", &["link"], 2),
            // covers: app seal
            one(
                "seal: vault closed",
                &["seal", "acme"],
                ok(r#"{"app":"acme","seal":{"vaultOpen":false,"error":"locked"}}"#),
                1,
            ),
            // covers: app seal
            one(
                "seal: no key yet",
                &["seal", "acme"],
                ok(r#"{"app":"acme","seal":{"vaultOpen":true}}"#),
                1,
            ),
            // covers: app seal
            one(
                "seal: granted",
                &["seal", "acme"],
                ok(r#"{"app":"acme","seal":{"vaultOpen":true,"present":true,"granted":true,"version":2,"file":"/run/seal"}}"#),
                0,
            ),
            // covers: app seal
            one(
                "seal: disabled",
                &["seal", "acme"],
                ok(r#"{"app":"acme","seal":{"vaultOpen":true,"present":true,"disabled":true}}"#),
                0,
            ),
            // covers: app seal --json
            one("seal --json", &["seal", "acme", "--json"], ok(r#"{"seal":{"present":true}}"#), 0),
            // covers: app seal
            one(
                "seal rotate --yes",
                &["seal", "rotate", "acme", "--yes"],
                ok(r#"{"app":"acme","version":3,"warning":"everyone is out","next":"tell them"}"#),
                0,
            ),
            // covers: app seal
            one(
                "seal rotate -y, the least answer",
                &["seal", "acme", "rotate", "-y"],
                ok("{}"),
                0,
            ),
            // covers: app seal --json
            one(
                "seal rotate --yes --json",
                &["seal", "rotate", "acme", "--yes", "--json"],
                ok(r#"{"app":"acme"}"#),
                0,
            ),
            // covers: app seal
            one("seal rotate refused", &["seal", "rotate", "acme", "--yes"], answer(403, REFUSED_403), 3),
            // covers: app seal
            words("seal rotate, no terminal", &["seal", "rotate", "acme"], 2),
            // covers: app seal
            words("seal, no App", &["seal", "rotate"], 2),
        ],
    );
}

#[test]
fn config_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "config");
    let done = r#"{"app":{"appId":"acme","memoryLimitBytes":536870912},"applied":["memory 512 MiB","egress é"],"io":{"rbps":10485760,"wbps":null,"riops":100,"wiops":null},"note":"applied now"}"#;
    check(
        &rig,
        vec![
            // covers: app config
            one(
                "every flag",
                &[
                    "config",
                    "acme",
                    "--memory",
                    "512M",
                    "--cpu-weight",
                    "50",
                    "--tasks",
                    "64",
                    "--disk",
                    "2G",
                    "--egress",
                    "add",
                    "a.com:443",
                    "--egress",
                    "add",
                    "b.com:80",
                    "--egress",
                    "rm",
                    "c:1",
                    "--egress-mode",
                    "allowlist",
                    "--port",
                    "add",
                    "5000/udp",
                    "--port",
                    "rm",
                    "22",
                    "--recycle-every",
                    "24h",
                    "--recycle-at",
                    "85%",
                    "--isolation",
                    "container",
                    "--path",
                    "guarded",
                    "--risk",
                    "high",
                    "--db-private",
                    "on",
                    "--db-raw",
                    "off",
                    "--io-read",
                    "10M",
                    "--io-write-iops",
                    "50",
                ],
                ok(done),
                0,
            ),
            // covers: app config
            one(
                "a name twice, the last empty; --io none",
                &["config", "acme", "--memory", "8G", "--io", "none", "--memory", "", "--tasks", "1"],
                ok(r#"{"app":{"appId":"acme","memoryLimitBytes":null}}"#),
                0,
            ),
            // covers: app config
            one(
                "--io default and a ceiling: the ceiling wins",
                &["config", "--io", "default", "acme", "--io-read-iops", "9", "--io-write", "1G"],
                ok(r#"{"app":{"appId":"acme"},"io":{"rbps":null,"wbps":1073741824,"riops":9,"wiops":"x"},"note":null}"#),
                0,
            ),
            // covers: app config --json
            one("--json", &["config", "acme", "--risk", "normal", "--json"], ok(done), 0),
            // covers: app config
            one("held", &["config", "acme", "--memory", "none"], answer(202, HELD), 2),
            // covers: app config
            one("refused", &["config", "acme", "--memory", "none"], answer(403, REFUSED_403), 3),
            // covers: app config
            traceback(one(
                "no app: KeyError",
                &["config", "acme", "--memory", "1G"],
                ok(r#"{"applied":[]}"#),
                1,
            )),
            // covers: app config
            words("no flags", &["config", "acme"], 2),
            // covers: app config
            words("no App", &["config", "--memory", "1G"], 2),
            // covers: app config
            words("--egress, not add or rm", &["config", "acme", "--egress", "x", "y"], 2),
            // covers: app config
            words("--port add, no port: set -e", &["config", "acme", "--port", "add"], 1),
            // covers: app config
            words("--io, a bad value", &["config", "acme", "--io", "max"], 2),
            // covers: app config
            words("an unknown flag", &["config", "acme", "--zap"], 2),
            // covers: app config
            words("--memory last: set -e", &["config", "acme", "--memory"], 1),
        ],
    );
}

/// One answer above 256 KiB: thousands of applied lines.
#[test]
fn config_a_large_list() {
    let rig = Rig::new(VERB, "configbig");
    let rows: Vec<String> = (0..7000)
        .map(|n| format!(r#""egress host-{n:06}.example.com:443 added é""#))
        .collect();
    let body = format!(
        r#"{{"app":{{"appId":"acme"}},"applied":[{}]}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [
        &["config", "acme", "--memory", "1G"][..],
        &["config", "acme", "--memory", "1G", "--json"],
    ] {
        rig.check(&case("a large list", args, &[], vec![ok(&body)], files, 0, 1));
    }
}

/// Help never acts: a verb with `-h` or `--help` anywhere on the line
/// prints help and sends nothing.
#[test]
fn deploy_help_never_acts() {
    let rig = Rig::new(VERB, "dephelp");
    let rows: [&'static [&'static str]; 14] = [
        &["build", "site", "-h"],
        &["build", "--help", "site"],
        &["install", "acme", "--tunnel", "-h"],
        &["install", "--help", "acme"],
        &["deploy", "acme", "site", "--help"],
        &["deploy", "-h", "acme", "site"],
        &["rollback", "acme", "-h"],
        &["stop", "acme", "--help"],
        &["start", "-h", "acme"],
        &["restart", "acme", "-h"],
        &["link", "acme", "--rotate", "-h"],
        &["seal", "rotate", "acme", "--yes", "--help"],
        &["config", "acme", "--memory", "8G", "-h"],
        &["config", "--help", "acme", "--memory", "8G"],
    ];
    for args in rows {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], files, 0, 0)
        });
    }
}

/// The lines name the command and the product by the brand's names (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2`
/// or `k5`.
#[test]
fn deploy_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "depbrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want, code) in [
        (
            &["rollback"][..],
            format!("usage: {cli} app rollback <app-id> [--to SEED]\n"),
            2,
        ),
        (
            &["seal"],
            format!("usage: {cli} app seal <app-id> | {cli} app seal rotate <app-id> [--yes]\n"),
            2,
        ),
        (
            &["config", "a", "--zap"],
            format!("Unknown flag: --zap ({cli} app --help)\n"),
            2,
        ),
        (
            &["seal", "rotate", "acme"],
            format!("Rotating acme's seal key signs EVERYONE out of it (members and guests); their passwords still work.\nNot rotated: run {cli} app seal rotate acme --yes\n"),
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
