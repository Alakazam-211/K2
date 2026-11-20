//! `k5-cli hostmail cert`, `outbound` and `backup` against the shell
//! CLI's `cli/k2 hostmail …`, on the parity rig ([`common::parity`]): the
//! same requests (urllib's, from `_mail_py`), stdout, stderr and exit
//! code.
//!
//! - The words: each group's help with no words, each usage line, exit 2
//!   and nothing sent.
//! - cert renew, owner (each owner and ACME kind) and names (list, issue
//!   with a failure); outbound show and dane on; backup status, plan and
//!   set, plain and `--json`.
//! - A refusal with the permission card, answers the Python raises on,
//!   and no answer.
//! - Help never acts: each verb that changes something with `-h` or
//!   `--help` sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "hostmail";

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

fn help_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 0, 0)
}

fn traceback(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    compare(
        Compare::Traceback,
        case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1),
    )
}

#[test]
fn hostmail_cert_verbs_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "cwords");
    let v = vec![
        help_case("cert alone: the help", &["cert"]),
        help_case("backup alone: the help", &["backup"]),
        help_case("outbound -h", &["outbound", "-h"]),
        help_case("cert names --help", &["cert", "names", "--help"]),
        usage_case("cert, an unknown sub-command", &["cert", "show"]),
        usage_case("cert renew, a stray word", &["cert", "renew", "now"]),
        usage_case("cert owner, an unknown flag", &["cert", "owner", "--k2"]),
        usage_case(
            "cert names, --name with list",
            &["cert", "names", "--name", "x"],
        ),
        usage_case(
            "cert names issue, --name with no value",
            &["cert", "names", "issue", "--name"],
        ),
        usage_case("cert names, a stray word", &["cert", "names", "renew", "x"]),
        usage_case("outbound, dane with no on", &["outbound", "dane"]),
        usage_case("outbound, dane off", &["outbound", "dane", "off"]),
        usage_case("outbound, a stray word", &["outbound", "list"]),
        usage_case("backup, an unknown sub-command", &["backup", "run"]),
        usage_case("backup status, a stray word", &["backup", "status", "x"]),
        usage_case(
            "backup plan, a count not a number",
            &["backup", "plan", "--keep-daily", "7d"],
        ),
        usage_case(
            "backup plan, a value missing",
            &["backup", "plan", "--max-local-gb"],
        ),
        usage_case(
            "backup plan, an unknown flag",
            &["backup", "plan", "--mode", "local"],
        ),
        usage_case(
            "backup set, no mode",
            &["backup", "set", "--confirm-plan", "h"],
        ),
        usage_case(
            "backup set, a mode it doesn't know",
            &["backup", "set", "--mode", "cloud"],
        ),
        usage_case(
            "backup set, none with no reason",
            &["backup", "set", "--mode", "none", "--confirm-plan", "h"],
        ),
        usage_case(
            "backup set, no plan hash",
            &["backup", "set", "--mode", "local"],
        ),
        usage_case(
            "backup set, an offsite count not a number",
            &[
                "backup",
                "set",
                "--mode",
                "both",
                "--offsite-keep-monthly",
                "-1",
            ],
        ),
        usage_case("backup set, a value missing", &["backup", "set", "--mode"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

const OWNER_K2: &str =
    r#"{"ok":true,"hostname":"mail.acme.example","owner":"k2","stalwartAcme":{"type":"Manual"}}"#;

const NAMES: &str = r#"{"ok":true,"names":[{"name":"autoconfig.acme.example","pointsHere":true,"issuer":"le","cert":{"state":"valid","expiresAt":"2027-01-01","lastError":"once"}},{"name":"mta-sts.acme.example","pointsHere":false,"dns":"add an A record","reason":"not here"}]}"#;

const BACKUP: &str = r#"{"ok":true,"backup":{"mode":"local","configuredBy":"owner","configuredWorkspace":"alpha","configuredAt":1700000000,"reason":"nightly","window":"02:00-04:00","retention":{"local":{"keepDaily":7,"keepWeekly":4,"maxLocalGb":50},"offsite":{"keepDaily":7}},"observer":{"status":"ok","lastSuccess":1700000000,"days":3,"churnPerDay":{"deletedBytes":2097152,"maxDeletedBytes":3221225472,"createdBytes":10,"measured":true},"storeBytes":5368709120,"mutableBytes":1048576,"fsFree":1,"fsTotal":2,"sameFs":false,"lastError":"slow"},"configError":"bad window","note":"no backups run yet"}}"#;

const PLAN: &str = r#"{"ok":true,"store":{"bytes":1073741824,"path":"/var/lib/stalwart","immutableFiles":120,"mutableBytes":1024,"configBytes":2048,"k2HalfBytes":0,"fsFree":10737418240,"fsTotal":21474836480,"floorBytes":1048576,"viewRoot":"/var/lib/k2-backup","sameFs":true},"churn":{"label":"low","deletedPerDay":1,"maxDeletedPerDay":2,"createdPerDay":3},"retention":{"local":{"keepDaily":7,"keepWeekly":4,"maxLocalGb":20},"offsite":{"keepDaily":7,"keepWeekly":4,"keepMonthly":6}},"window":"02:00-04:00","bandwidthMbps":50,"modes":{"local":{"verdict":"ok","firstNightLocalBytes":1073741824,"steadyLocalBytes":10,"peakBytes":20,"freeAfterBytes":30,"downtimeSecs":45,"downtimeLabel":"a short stop","protects":"a bad delete"},"offsite-only":{"verdict":"warn","available":false,"unavailableReason":"B2","offsite":{"firstUploadBytes":{"low":1,"high":2},"firstUploadSecs":5000,"nightlyUploadBytes":3,"nightlyUploadSecs":90000,"firstTempBytes":4,"nightlyTempBytes":5}},"both":{}},"notes":["one note"],"planHash":"ph_1"}"#;

#[test]
fn hostmail_cert_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "canswer");
    let v = vec![
        // covers: hostmail cert renew
        ok_case(
            "cert renew",
            &["cert", "renew"],
            r#"{"ok":true,"hostname":"mail.acme.example"}"#,
        ),
        // covers: hostmail cert renew --json
        ok_case(
            "cert renew --json",
            &["cert", "renew", "--json"],
            r#"{"ok":true}"#,
        ),
        ok_case(
            "cert renew, a hint",
            &["cert", "renew"],
            r#"{"ok":true,"hint":"queued"}"#,
        ),
        // covers: hostmail cert owner
        ok_case("cert owner, the daemon's", &["cert", "owner"], OWNER_K2),
        ok_case(
            "cert owner, Stalwart's automatic ACME",
            &["cert", "owner"],
            r#"{"ok":true,"owner":"stalwart-acme","stalwartAcme":{"type":"Automatic","names":["mail.acme.example","autoconfig.acme.example"]}}"#,
        ),
        ok_case(
            "cert owner, automatic with no names",
            &["cert", "owner"],
            r#"{"ok":true,"owner":"stalwart-acme","stalwartAcme":{"type":"Automatic"}}"#,
        ),
        ok_case(
            "cert owner, manual and not the daemon's",
            &["cert", "owner"],
            r#"{"ok":true,"owner":"unknown","stalwartAcme":{"type":"Manual"}}"#,
        ),
        ok_case(
            "cert owner, an error reading Stalwart",
            &["cert", "owner"],
            r#"{"ok":true,"stalwartAcme":{"error":"locked"}}"#,
        ),
        // covers: hostmail cert owner --json
        ok_case("cert owner --json", &["cert", "owner", "--json"], OWNER_K2),
        ok_case(
            "cert owner --stalwart-acme",
            &["cert", "owner", "--stalwart-acme"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail cert names
        ok_case("cert names", &["cert", "names"], NAMES),
        ok_case(
            "cert names list, none",
            &["cert", "names", "list"],
            r#"{"ok":true,"names":[]}"#,
        ),
        // covers: hostmail cert names --json
        ok_case("cert names --json", &["cert", "names", "--json"], NAMES),
        // covers: hostmail cert names issue
        case(
            "cert names issue, one failed",
            &[
                "cert",
                "names",
                "issue",
                "--name",
                "autoconfig.acme.example",
            ],
            &[],
            vec![ok(
                r#"{"ok":true,"failed":1,"names":[{"name":"autoconfig.acme.example","result":"failed","detail":"no NS"},{"name":"b","result":"issued","expiresAt":"2027"}],"reload":{"method":"api","error":"late"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        // covers: hostmail cert names issue --json
        case(
            "cert names issue --json, failed",
            &["cert", "names", "issue", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"failed":2}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: hostmail cert names renew
        ok_case(
            "cert names renew",
            &["cert", "names", "renew"],
            r#"{"ok":true,"names":[]}"#,
        ),
        // covers: hostmail cert names renew --json
        ok_case(
            "cert names renew --json",
            &["cert", "names", "renew", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail outbound
        ok_case(
            "outbound",
            &["outbound"],
            r#"{"ok":true,"ipStrategy":"ipv4_then_ipv6","ipStrategySetBy":"k2","dane":[{"name":"default","dane":"optional","setBy":"default"},{"name":"invalid-tls","dane":"disable","setBy":"hand"},{"setBy":"other"}],"pendingReload":{"at":"10:00"}}"#,
        ),
        ok_case(
            "outbound show, little known",
            &["outbound", "show"],
            r#"{"ok":true,"dnssec":"ok"}"#,
        ),
        // covers: hostmail outbound --json
        ok_case("outbound --json", &["outbound", "--json"], r#"{"ok":true}"#),
        ok_case(
            "outbound dane on",
            &["outbound", "dane", "on"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail backup status
        ok_case("backup status", &["backup", "status"], BACKUP),
        ok_case(
            "backup status, nothing set",
            &["backup", "status"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail backup status --json
        ok_case(
            "backup status --json",
            &["backup", "status", "--json"],
            BACKUP,
        ),
        // covers: hostmail backup plan
        ok_case(
            "backup plan",
            &[
                "backup",
                "plan",
                "--bandwidth-mbps",
                "50",
                "--keep-daily",
                "7",
                "--max-local-gb",
                "20",
            ],
            PLAN,
        ),
        ok_case(
            "backup plan, little known",
            &["backup", "plan"],
            r#"{"ok":true}"#,
        ),
        // covers: hostmail backup plan --json
        ok_case(
            "backup plan --json",
            &["backup", "plan", "--json", "--keep-weekly", "4"],
            PLAN,
        ),
        // covers: hostmail backup set
        ok_case(
            "backup set",
            &[
                "backup",
                "set",
                "--mode",
                "local",
                "--keep-daily",
                "7",
                "--keep-weekly",
                "04",
                "--window",
                "02:00-04:00",
                "--max-local-gb",
                "20.5",
                "--confirm-plan",
                "ph_1",
            ],
            r#"{"ok":true,"from":"none","backup":{"mode":"local"}}"#,
        ),
        // covers: hostmail backup set --json
        ok_case(
            "backup set none --json",
            &[
                "backup",
                "set",
                "--mode",
                "none",
                "--reason",
                "provider snapshots",
                "--confirm-plan",
                "ph_1",
                "--json",
            ],
            r#"{"ok":true}"#,
        ),
        case(
            "backup set, a size Python can't read",
            &[
                "backup",
                "set",
                "--mode",
                "local",
                "--max-local-gb",
                "lots",
                "--confirm-plan",
                "h",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "outbound dane on, refused",
            &["outbound", "dane", "on"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        traceback(
            "cert owner, names not strings",
            &["cert", "owner"],
            r#"{"ok":true,"stalwartAcme":{"type":"Automatic","names":[1]}}"#,
        ),
        traceback(
            "cert names, a row not a dict",
            &["cert", "names"],
            r#"{"ok":true,"names":["x"]}"#,
        ),
        traceback(
            "outbound, a setter not hashable",
            &["outbound"],
            r#"{"ok":true,"ipStrategySetBy":["k2"]}"#,
        ),
        traceback(
            "backup status, the observer not a dict",
            &["backup", "status"],
            r#"{"ok":true,"backup":{"observer":"x"}}"#,
        ),
        traceback(
            "backup plan, a verdict not a string",
            &["backup", "plan"],
            r#"{"ok":true,"modes":{"local":{"verdict":5}}}"#,
        ),
        case(
            "cert renew, no answer",
            &["cert", "renew"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "backup status, the daemon down",
            &["backup", "status"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "chelp");
    const LINES: &[&[&str]] = &[
        &["cert", "renew", "-h"],
        &["cert", "owner", "--stalwart-acme", "--help"],
        &["cert", "names", "issue", "--name", "x", "-h"],
        &["cert", "names", "renew", "--help"],
        &["outbound", "dane", "on", "-h"],
        &[
            "backup",
            "set",
            "--mode",
            "local",
            "--confirm-plan",
            "h",
            "--help",
        ],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case("help never acts", line, &[], vec![], owner_and_port, 0, 0),
        ));
    }
}
