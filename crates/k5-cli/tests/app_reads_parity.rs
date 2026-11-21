//! `k5-cli app list` and `app deployment` against the shell CLI's, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - `list`: the apps view (engines and their re-host marks, a URL or the
//!   front port, the legacy services, the notices, nothing on the box),
//!   `--workspace`, `--box` (memory, maintainers, disk, hosts, ports, box
//!   services), `--json`, a list above 256 KiB; its loop's mistakes (a
//!   word, `--workspace` last).
//! - `inspect`: a full answer (why, a warm deployment, the seed, the
//!   last deployment, a broken audit chain), a bare one, `--json`, the
//!   usage line.
//! - `inspect --resources`: the box usage view's printer on a full
//!   answer (every section, the parts biggest first, the owners, the
//!   sweep, the suggestion), a bare one, `--json`, a refusal, and its
//!   Python raising part way. The answers name `checkedAt`: with none the
//!   Python prints the minute it runs.
//! - `deployment`: with and without an id, an error, `--json`, the usage
//!   line, an unknown flag.
//! - Python's raise part way; a refusal with the registry's fix line;
//!   the cell socket; no daemon.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";
const LIST: &str = r#"{"apps":[{"appId":"acme","state":"live","workspaceHandle":"ws-a","liveSeed":"0123456789abcdef0123","url":"https://acme.example","label":"Acme é","engine":{"label":"v2","rehostDue":true,"rehost":"running"}},{"appId":"globex-long-app-id-here","state":"stopped","frontPort":4100,"engine":{"rehostDue":true,"rehost":"other"}},{"appId":"b","state":"x","engine":null,"label":null}],"legacy":[{"name":"old","desired":"running","workspaceHandle":"w","label":"L","labelMovedTo":"acme","hint":"install it again"}],"notices":[{"hint":"one"},{}]}"#;
const BOXED: &str = r#"{"memory":{"usedBytes":2147483648,"freeBytes":1073741824,"pool":{"poolBytes":3221225472}},"apps":[{"appId":"acme","state":"live","maintainer":{"kind":"agent","agent":"ann","address":"ann@box"},"memory":{"limitBytes":536870912},"disk":{"budgetBytes":1073741824},"hostnames":["a.example","b.example"],"ports":[{"port":5000,"proto":"udp"}],"boxServices":["pg"]},{"appId":"b","state":"parked","maintainer":null,"memory":{}}]}"#;
const DEPLOYMENT: &str = r#"{"app":"acme","state":"failed","who":"ann","commit":"abc123","deployment":{"id":"d-1","kind":"deploy","error":"health check failed","fromSeed":"0123456789abcdef","toSeed":"fedcba9876543210"}}"#;

/// The box usage view with every section.
const RESOURCES: &str = r#"{"box":{"host":"box1","cores":8,"memBytes":17179869184,"checkedAt":1700000000},"guard":{"mode":"on","lines":{"warnPct":85,"warnFreeBytes":21474836480,"refusePct":95,"reserveBytes":10737418240,"criticalPct":98,"criticalFreeBytes":2147483648}},"scan":{"workspacesAt":0,"appsAt":0},"disks":[{"mount":"/","usedBytes":123456789012,"totalBytes":500000000000,"usedPct":24.7,"freeBytes":376543210988,"level":"ok","holds":["apps","seeds"],"notMeasuredBytes":4500000000},{"id":"data-disk-long","usedBytes":999,"totalBytes":null,"usedPct":null,"freeBytes":1500,"level":"warn é"}],"mem":{"totalBytes":17179869184,"availableBytes":1610612736,"agentsBytes":536870912},"cpu":{"load5":1.25},"consumers":[{"kind":"app","id":"acme","path":"guarded","owner":{"handle":"ann"},"diskBytes":2500000000,"memBytes":268435456,"state":"live","parts":{"seeds":1000,"data":3000,"logs":3000,"tmp":5}},{"kind":"service","id":"pg-service-with-a-long-name","owner":{"kind":"server"},"diskBytes":15000000,"memBytes":null,"walled":false,"partial":true},{"kind":"database","id":"db1","owner":{},"diskBytes":12,"unusedBytes":2048,"folder":"/srv/db1"},{"kind":"workspace","id":"ws-a","diskBytes":null,"parts":{"a":1,"b":2,"c":3,"d":4}},{"kind":"shared","id":"cache","folder":"/var/cache é"},{"kind":"other","id":"skipped"}],"byOwner":[{"owner":{"handle":"ann","address":"ann@box1"},"diskBytes":2500000000},{"owner":{},"diskBytes":12},{"diskBytes":null},{"owner":{"handle":"zed"}}],"k2WillClear":{"seedsBytes":3200000000,"seeds":4},"suggestion":"Clear old Seeds."}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn list_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
        case(
            "the apps",
            &["list"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nothing on the box",
            &["list"],
            &[],
            vec![ok(r#"{"apps":[],"legacy":null}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--workspace",
            &["list", "--workspace", "ws a/é"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--box over --workspace",
            &["list", "--workspace", "w", "--box"],
            &[],
            vec![ok(BOXED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--box, no pool",
            &["list", "--box"],
            &[],
            vec![ok(r#"{"memory":{"pool":{"poolBytes":0}},"apps":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["list", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a word: exit 2",
            &["list", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--workspace last: set -e, exit 1",
            &["list", "--workspace"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        traceback(case(
            "Python's raise part way",
            &["list"],
            &[],
            vec![ok(r#"{"apps":[{"appId":"a","state":"s"},{"appId":"b"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "--box: a size that is not a number",
            &["list", "--box"],
            &[],
            vec![ok(r#"{"memory":{"usedBytes":"x"}}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a refusal with its fix line",
            &["list"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"x"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        with_cell(case(
            "the cell socket",
            &["list"],
            &[],
            vec![ok(LIST)],
            port_only,
            0,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

fn with_cell(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        ..c
    }
}

/// One answer above 256 KiB: thousands of rows.
#[test]
fn list_a_large_list() {
    let rig = Rig::new(VERB, "llarge");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"appId":"app-{n}","state":"live","workspaceHandle":"ws-{n}","liveSeed":"seed{n:020}","url":"https://app-{n}.example","label":"App {n} é"}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"apps":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["list"][..], &["list", "--json"]] {
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
fn deployment_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "depl");
    let dead = dead_port();
    let v = vec![
        case(
            "a deployment",
            &["deployment", "acme"],
            &[],
            vec![ok(DEPLOYMENT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "by id, the last word wins",
            &["deployment", "a b", "d-0", "d-1"],
            &[],
            vec![ok(
                r#"{"app":"a b","state":"live","who":["x"],"deployment":{"id":"d-1","kind":"rollback","fromSeed":["0","1"]}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["deployment", "--json", "acme"],
            &[],
            vec![ok(DEPLOYMENT)],
            owner_and_port,
            0,
            1,
        ),
        case("usage", &["deployment"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an unknown flag",
            &["deployment", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "an error that is not words",
            &["deployment", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","state":"s","deployment":{"id":"d","kind":"k","error":7}}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a 404",
            &["deployment", "acme", "d-9"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such deployment"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "no daemon",
            &["deployment", "acme"],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

const INSPECT: &str = r#"{"app":{"appId":"acme","state":"stopped","label":"Acme","stateReason":"crashed twice","workspaceHandle":"ws-a","url":"https://acme.example","frontPort":4100,"memoryLimitBytes":536870912},"runtime":{"live":{"deployment":"d-2","port":5002,"pid":77},"warm":{"deployment":"d-1","port":5001},"watchUntilSecs":30},"liveSeed":{"hash":"0123456789abcdef","seedId":"s-1","version":"1.2","signer":"ann","createdWith":{"k2":"0.46.17","catalog":"c1","checker":"ok"},"sourceRef":"abc"},"lastDeployment":{"kind":"deploy","id":"d-2","actor":"ann","result":"failed","error":"boom é"},"audit":{"verified":false,"brokenAt":4,"reason":"hash"}}"#;

#[test]
fn inspect_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "insp");
    let v = vec![
        case(
            "a full answer",
            &["inspect", "acme"],
            &[],
            vec![ok(INSPECT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a bare answer",
            &["inspect", "a b"],
            &[],
            vec![ok(
                r#"{"app":{"appId":"a","state":"s","label":null},"runtime":{},"audit":{"verified":1}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["inspect", "--json", "acme"],
            &[],
            vec![ok(INSPECT)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "usage",
            &["inspect", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // native: app inspect --resources
        case(
            "--resources: every section",
            &["inspect", "acme", "--resources"],
            &[],
            vec![ok(RESOURCES)],
            owner_and_port,
            0,
            1,
        ),
        // native: app inspect --resources
        case(
            "--resources: a bare answer",
            &["inspect", "--resources", "acme"],
            &[],
            vec![ok(r#"{"box":{"checkedAt":1700000000},"guard":{"modeSource":"env"},"scan":{"appsAt":5}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // native: app inspect --resources
        case(
            "--resources --json",
            &["inspect", "acme", "--resources", "--json"],
            &[],
            vec![ok(RESOURCES)],
            owner_and_port,
            0,
            1,
        ),
        // native: app inspect --resources
        case(
            "--resources: refused",
            &["inspect", "acme", "--resources"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.to_string(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // native: app inspect --resources
        traceback(case(
            "--resources: a part that is not a number",
            &["inspect", "acme", "--resources"],
            &[],
            vec![ok(
                r#"{"box":{"checkedAt":1700000000},"consumers":[{"kind":"shared","id":"s","parts":{"a":"x"}}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // native: app inspect --resources
        traceback(case(
            "--resources: an answer that is a list",
            &["inspect", "acme", "--resources"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a memory limit that is not a number",
            &["inspect", "acme"],
            &[],
            vec![ok(
                r#"{"app":{"appId":"a","state":"s","label":"l","memoryLimitBytes":1.5},"runtime":{}}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `app` alone and `app --json`: the App page on stderr through the
/// permission cards' filter, exit 1. No passport: off, nothing asked. A
/// passport: the shell's probe, over TCP or on the cell socket, on or
/// off. In a session with no passport: the id tool's refusal first.
#[test]
fn the_app_page_through_the_card_filter() {
    let rig = Rig::new(VERB, "page");
    let tcp = || with_passport(&[]);
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        // covers: app
        case("no passport: off, nothing asked", &[], &[], vec![], owner_and_port, 1, 0),
        // covers: app --json
        case("--json: the same page", &["--json"], &[], vec![], owner_and_port, 1, 0),
        // covers: app
        Case {
            env: tcp(),
            ..case(
                "a passport, cards on (TCP)",
                &[],
                &[],
                vec![ok(common::cards::ON)],
                nothing,
                1,
                1,
            )
        },
        // covers: app
        Case {
            env: tcp(),
            ..case(
                "a passport, cards off (TCP)",
                &[],
                &[],
                vec![ok(common::cards::OFF)],
                nothing,
                1,
                1,
            )
        },
        // covers: app --json
        Case {
            env: cell(),
            ..case(
                "a passport, cards on (the cell socket)",
                &["--json"],
                &[],
                vec![ok(common::cards::ON)],
                nothing,
                1,
                1,
            )
        },
        // covers: app
        Case {
            env: vec![
                ("K2_PORT", PORT.to_string()),
                ("K2_HOOK_SOCK", "{sock}".to_string()),
            ],
            ..case(
                "a session with no passport: refused",
                &[],
                &[],
                vec![],
                owner_only,
                3,
                0,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}
