//! `k5-cli hostchat` against the shell CLI's `cli/k2 hostchat`, on the
//! parity rig ([`common::parity`]) and, for a person's steps, the vault
//! rig ([`common::vault`]): the same requests, stdout, stderr and exit.
//!
//! - The main verbs' lines and `--json`: status (every line it can say),
//!   preflight, enable and its wait, disable, rollback, doctor, restart,
//!   adopt, backup, backups, job, upgrade and its wait, restore,
//!   rotate-master, config (the list, the policy and the public URL),
//!   uninstall, app-url, ship, mirror; deliveries and agents ride chat.
//! - A person's steps, with a confirm over the vault socket: recovery-key,
//!   purge, rotate-master --drop-prev, upgrade --accept-binary, rollback
//!   --accept-data-loss, open --print and open (the browser gets a
//!   private page that holds the link); each refuses in a session.
//! - The refusals, Python's raise, the usage lines, help and the gate, a
//!   passport and the cell socket, one large list, and the hand-backs.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};
use std::path::{Path, PathBuf};

const VERB: &str = "hostchat";
const STATUS: &str = r#"{"state":"running","released":false,"notice":"preview","identityProvider":"k2","installedVersion":"0.6.0","installId":"inst1","publicUrl":"https://chat.example","appUrl":"https://app.example","owner":"ann","backup":{"ship":{"kind":"box-backup","boxBackup":{"line":"nightly to /mnt/b"}}},"mirror":{"kind":"dir","path":"/mnt/m","tied":true,"tie":{"state":"pending"}},"recoveryKeyInVault":true,"moveOffK2":{"line":"export it"},"autoSetup":{"line":"done"},"meet":{"app":"meet1","url":"https://meet.example"},"upgradeAvailable":true,"alarms":["disk","cert"],"sewDbPostgres":{"stale":true,"package":"postgresql-16","installedVersion":"16.4","fix":"run x"},"warnings":[{"message":"w1"},{"code":"c2"}],"lastError":"boom","recording":{"installed":true,"enabled":false,"version":"0.9.0"},"voice":{"state":"on","publicUrl":"wss://v.example","turnHost":"turn.example","nodeIp":"192.0.2.10","cert":{"daysLeft":12},"turnTls":{"served":"ok"},"progress":{"step":"cert","result":"running"},"lastError":"late","exposure":{"note":"public é"}}}"#;
const STATUS2: &str = r#"{"state":"stopped","pinnedVersion":"0.5.0","owner":"none","backup":{"ship":{"kind":"dir","path":"/mnt/b","lastShippedId":"s1","lastError":"e"}},"mirror":{"kind":"none","tie":{"state":"pending","error":"no disk"}},"recoveryKeyVaultName":"lk","recoveryKeyInVault":1,"alarms":[],"warnings":[],"voice":{"cert":{"daysLeft":0},"progress":{"result":"done"}}}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn one(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

/// The shell's Python raises on the answer: exit 1, the traceback.
fn tb(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    traceback(case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1))
}

fn many(name: &'static str, args: &'static [&'static str], bodies: &[&str], code: i32) -> Case {
    let replies: Vec<Reply> = bodies.iter().map(|b| ok(b)).collect();
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, code, n)
}

/// The daemon's walled refusal of an agent's `policy set dm_enabled on`.
const DM_CARD: &str = r#"{"ok":false,"error":{"code":"person_only","hint":"nothing changed: a person does this, never an agent or a K2 terminal (the server's recording policies)"},"permission":{"requestable":true,"action":"voice.recording-dm","approvers":"owner or admin","command":"k2 permission request voice.recording-dm dm=on --why \"<what you need it for>\""}}"#;

fn bad(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

#[test]
fn hostchat_reads_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "reads");
    let v = vec![
        // covers: hostchat
        one("status: every line", &["status"], STATUS),
        // covers: hostchat --json
        one("status --json", &["status", "--json"], STATUS),
        one(
            "status: a pin, a folder, no mirror",
            &["status", "--health"],
            STATUS2,
        ),
        one(
            "status: not in this release",
            &["status"],
            r#"{"released":false,"state":"not-installed"}"#,
        ),
        one(
            "status: not in this release, its notice",
            &["status"],
            r#"{"released":false,"state":"not-installed","notice":"soon"}"#,
        ),
        one(
            "status: not supported",
            &["status"],
            r#"{"supported":false}"#,
        ),
        one(
            "status: the box backup with no line",
            &["status"],
            r#"{"state":"running","backup":{"ship":{"kind":"box-backup"}},"mirror":{"kind":"dir","path":"/m"}}"#,
        ),
        // The box backup carrying the folder: an external ship, its line
        // first, nothing else of the ship.
        one(
            "status: an external ship",
            &["status"],
            r#"{"state":"running","backup":{"ship":{"kind":"external","setting":"none","external":{"line":"K2 backups (restic), last 3 h ago","fresh":true},"lastShippedId":"x"}}}"#,
        ),
        one(
            "status: an external ship with no line",
            &["status"],
            r#"{"state":"running","backup":{"ship":{"kind":"none","external":{"fresh":false},"lastError":"e"}}}"#,
        ),
        tb(
            "status: an external ship that is no object",
            &["status"],
            r#"{"state":"running","backup":{"ship":{"external":"restic"}}}"#,
        ),
        one(
            "preflight, a relative tarball",
            &["preflight", "--tarball", "sew 1.tgz"],
            r#"{"ready":false,"checks":[{"status":"ok","id":"disk","detail":"9 GB"},{"status":"fail","id":"port","detail":"443 taken","hint":"stop nginx"}]}"#,
        ),
        one("preflight: ready", &["preflight"], r#"{"ready":true}"#),
        one(
            "backups",
            &["backups"],
            r#"{"ok":true,"backups":[{"id":"b1","at":1},{"id":"b2"}],"dir":"/var/b","count":2,"none":null,"empty":"","list":[]}"#,
        ),
        one(
            "job",
            &["job", "j 1"],
            r#"{"id":"j 1","state":"done","n":1.5,"t":true}"#,
        ),
        one("doctor", &["doctor"], r#"{"ok":false,"checks":{"a":1}}"#),
        one(
            "reconfigure",
            &["reconfigure"],
            r#"{"ok":true,"state":"running"}"#,
        ),
        one("restart", &["restart"], r#"{"state":"running"}"#),
        one("disable", &["disable"], r#"{"state":"stopped"}"#),
        one(
            "adopt",
            &["adopt"],
            r#"{"state":"running","installedVersion":{"from":"0.5.0","to":"0.6.0"},"clearedError":"x","alarms":["a","b"]}"#,
        ),
        one(
            "adopt: the same version",
            &["adopt", "--json"],
            r#"{"state":"running","installedVersion":{"from":"0.6.0","to":"0.6.0"}}"#,
        ),
        one(
            "adopt: the same version, the lines",
            &["adopt"],
            r#"{"state":"running","installedVersion":{"from":"0.6.0","to":"0.6.0"}}"#,
        ),
        one("backup", &["backup"], r#"{"ok":true,"bundle":"b9"}"#),
        one(
            "restore",
            &["restore", "b1", "--confirm"],
            r#"{"ok":true,"restored":"b1"}"#,
        ),
        one(
            "restore: not confirmed",
            &["restore", "b1"],
            r#"{"ok":false,"plan":"x"}"#,
        ),
        one("rotate-master", &["rotate-master"], r#"{"phase":"start"}"#),
        one(
            "uninstall --drop-grants",
            &["uninstall", "--drop-grants"],
            r#"{"state":"removed"}"#,
        ),
        one(
            "rollback: Sew put its voice engine back",
            &["rollback", "--media-tarball", "m.tgz"],
            r#"{"state":"running","version":"0.5.0","media":{"state":"rolled-back","from":"1.1","to":"1.0"},"warnings":["w"],"doctor":{"ok":true}}"#,
        ),
        one(
            "rollback: the command to run",
            &["rollback"],
            r#"{"state":"running","media":{"result":"manual","to":"1.0"},"mediaCommand":"sudo x","doctor":{"ok":false},"error":"e"}"#,
        ),
        one(
            "rollback: a note",
            &["rollback"],
            r#"{"state":"running","mediaNote":"n"}"#,
        ),
        case(
            "rollback: ok false exits 3 with its code",
            &["rollback", "--json"],
            &[],
            vec![ok(
                r#"{"ok":false,"code":"media_rollback_failed","error":"no","mediaCommand":"sudo y"}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "rollback: ok false, no code",
            &["rollback"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            3,
            1,
        ),
        // covers: hostchat ship
        one(
            "ship box-backup",
            &["ship", "box-backup"],
            r#"{"ship":{"boxBackup":{"line":"nightly","coveredBy":"k2-backup","timer":"02:00"}},"mirror":{"state":"tied","path":"/m"},"note":"n"}"#,
        ),
        one(
            "ship dir, relative",
            &["ship", "dir", "off box"],
            r#"{"ship":{"kind":"dir","path":"/x/off box"},"mirror":{"state":"waiting","error":"no disk"}}"#,
        ),
        one(
            "ship none",
            &["ship", "none"],
            r#"{"ship":{"kind":"none"},"mirror":{}}"#,
        ),
        // covers: hostchat mirror
        one(
            "mirror none",
            &["mirror", "none"],
            r#"{"ok":true,"mirror":{"kind":"none"}}"#,
        ),
        one(
            "mirror dir",
            &["mirror", "dir", "/mnt/m", "--json"],
            r#"{"ok":true}"#,
        ),
        // covers: hostchat app-url
        one("app-url", &["app-url"], STATUS),
        one(
            "app-url show: none",
            &["app-url", "show"],
            r#"{"state":"x"}"#,
        ),
        one("app-url --json", &["app-url", "--json"], STATUS),
        one(
            "app-url set",
            &["app-url", "set", "https://app.example/x"],
            r#"{"ok":true,"appUrl":"https://app.example/x"}"#,
        ),
        one(
            "app-url clear",
            &["app-url", "clear"],
            r#"{"ok":true,"appUrl":null}"#,
        ),
        // covers: hostchat config
        one(
            "config: the list",
            &["config"],
            r#"{"approvalsBy":"people","allowed":["people","people+agents"],"defaultSpace":null}"#,
        ),
        one(
            "config: the list, none allowed",
            &["config"],
            r#"{"defaultSpace":"ops"}"#,
        ),
        one(
            "config --json",
            &["config", "--json"],
            r#"{"approvalsBy":"people"}"#,
        ),
        one(
            "config set approvals.by",
            &["config", "set", "approvals.by", "people+agents"],
            r#"{"approvalsBy":"people+agents"}"#,
        ),
        one(
            "config set default_space",
            &["config", "set", "default-space", "ops", "--json"],
            r#"{"defaultSpace":"ops"}"#,
        ),
        one(
            "config --public-url",
            &["config", "--public-url", "https://chat.example"],
            r#"{"ok":true,"publicUrl":"https://chat.example"}"#,
        ),
        one(
            "config set server.public_url",
            &["config", "set", "server.public_url", "https://c.example"],
            r#"{"publicUrl":"https://c.example"}"#,
        ),
        // covers: hostchat deliveries
        one(
            "deliveries",
            &["deliveries"],
            r#"{"deliveries":[{"workspace":"w","quietHours":"22:00-07:00","tz":"box","batch":"off"}]}"#,
        ),
        // covers: hostchat agents
        one(
            "agents",
            &["agents"],
            r#"{"access":[{"workspace":"w","space":"s","level":"read","approval":"required"}]}"#,
        ),
        tb("status: not JSON", &["status"], "<html>"),
        tb("status: a list", &["status"], "[1]"),
        tb(
            "status: a mirror path that is no string",
            &["status"],
            r#"{"state":"x","mirror":{"kind":"dir","path":3}}"#,
        ),
        tb("backups: a list", &["backups"], "[]"),
        tb("config: not JSON", &["config"], "nope"),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_waits_as_the_shell_does() {
    let rig = Rig::new(VERB, "waits");
    let v = vec![
        // covers: hostchat enable
        many(
            "enable: waits through the download",
            &["enable", "--tarball", "sew.tgz", "--identity", "standalone"],
            &[
                r#"{"state":"installing"}"#,
                r#"{"state":"installing","enableProgress":{"current":"stage","download":{"source":"download","bytes":1234567,"total":24700000,"label":"Sew 0.5.0"}}}"#,
                r#"{"state":"installing","enableProgress":{"current":"stage","download":{"source":"cache"}}}"#,
                r#"{"state":"running","installedVersion":"0.5.0"}"#,
            ],
            0,
        ),
        many(
            "enable --voice: waits on voice, then it failed",
            &[
                "enable",
                "--voice-url",
                "wss://v.example",
                "--node-ip",
                "192.0.2.10",
                "--media-tarball",
                "/m.tgz",
            ],
            &[
                r#"{"state":"installing"}"#,
                r#"{"state":"running","voice":{"progress":{"step":"turn","result":"running"}}}"#,
                r#"{"state":"running","voice":{"progress":{"step":"turn","result":"error"}}}"#,
            ],
            3,
        ),
        many(
            "enable --json: Sew is stopping",
            &["enable", "--json", "--voice"],
            &[
                r#"{"state":"installing"}"#,
                r#"{"state":"running","apiHealth":{"reason":"stopping"}}"#,
            ],
            3,
        ),
        many(
            "enable: Sew upgrading counts as busy",
            &["enable"],
            &[
                r#"{"state":"installing"}"#,
                r#"{"state":"running","apiHealth":{"reason":"upgrading"}}"#,
                r#"{"state":"error"}"#,
            ],
            3,
        ),
        one(
            "enable --no-wait",
            &["enable", "--no-wait"],
            r#"{"state":"installing","ok":true}"#,
        ),
        one(
            "enable: done at once",
            &["enable"],
            r#"{"state":"running"}"#,
        ),
        many(
            "upgrade: each step once, then done",
            &["upgrade", "--tarball", "s.tgz", "--dry-run"],
            &[
                r#"{"state":"upgrading"}"#,
                r#"{"state":"upgrading","upgradeProgress":{"current":"upgrade","steps":{"stage":{"seq":1}}}}"#,
                r#"{"state":"running","upgradeProgress":{"result":"done","from":"0.5.0","to":"0.6.0","steps":{"health":{"seq":3},"stage":{"seq":1},"upgrade":{"seq":2}}}}"#,
            ],
            0,
        ),
        many(
            "upgrade: rolled back, the reason",
            &["upgrade"],
            &[
                r#"{"state":"upgrading"}"#,
                r#"{"state":"running","lastError":"old","upgradeProgress":{"result":"rolled_back","from":"0.5.0","to":"0.6.0","lastError":"health failed","steps":{"rollback":{"at":5},"stage":{"at":5}}}}"#,
            ],
            3,
        ),
        many(
            "upgrade --json: no reason recorded",
            &["upgrade", "--json"],
            &[
                r#"{"state":"upgrading"}"#,
                r#"{"state":"running","upgradeProgress":{"result":"aborted"}}"#,
            ],
            3,
        ),
        one(
            "upgrade --no-wait",
            &["upgrade", "--no-wait"],
            r#"{"state":"upgrading"}"#,
        ),
        one(
            "upgrade: refused at once",
            &["upgrade"],
            r#"{"state":"running","refused":"x"}"#,
        ),
        traceback(many(
            "enable: a status that is no JSON",
            &["enable"],
            &[r#"{"state":"installing"}"#, "nope"],
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_refusals_as_the_shell_says_them() {
    let rig = Rig::new(VERB, "refusals");
    let v = vec![
        case(
            "a 400",
            &["job", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_id","hint":"no such job"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a permission card",
            &["disable"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 502 that is no JSON",
            &["status"],
            &[],
            vec![Reply::Answer(502, "<html>bad</html>".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 500 with no body",
            &["backups"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "config: chat's refusal line",
            &["config"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"sew_down","hint":"h","details":{"key":"k"}}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the wait: a refusal mid-way",
            &["enable"],
            &[],
            vec![
                ok(r#"{"state":"installing"}"#),
                Reply::Answer(403, r#"{"error":{"code":"nope","hint":"h"}}"#.into()),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "a reset: status 000",
            &["status"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        case("hostchat alone: its help", &[], &[], vec![], nothing, 0, 0),
        case("help", &["help"], &[], vec![], owner_only, 0, 0),
        case(
            "an empty verb, with no port: the unknown line",
            &[""],
            &[],
            vec![],
            owner_only,
            2,
            0,
        ),
        case(
            "status with no port: the gate",
            &["status"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        bad("an unknown verb", &["nope"]),
        bad("an unknown flag, as it is", &["status", "--x\"y"]),
        bad("--setup-url", &["config", "--setup-url", "x", "--bogus"]),
        bad("--identity", &["enable", "--identity", "ldap"]),
        bad("--voice on status", &["status", "--voice"]),
        bad(
            "--node-ip on upgrade",
            &["upgrade", "--node-ip", "192.0.2.1"],
        ),
        bad(
            "--media-tarball on status",
            &["status", "--media-tarball", "m"],
        ),
        bad(
            "--accept-data-loss on upgrade",
            &["upgrade", "--accept-data-loss"],
        ),
        bad("act-as", &["act-as", "ann"]),
        bad("job: no id", &["job"]),
        bad("restore: no bundle", &["restore", "--confirm"]),
        bad("app-url set: no url", &["app-url", "set"]),
        bad("app-url: a bad word", &["app-url", "nope"]),
        bad("ship: a bad kind", &["ship", "s3"]),
        bad("ship dir: no path", &["ship", "dir"]),
        bad("mirror box-backup", &["mirror", "box-backup"]),
        bad("mirror: no kind", &["mirror"]),
        bad("config set: a bad key", &["config", "set", "color", "red"]),
        bad(
            "config set approvals.by: a bad value",
            &["config", "set", "approvals.by", "all"],
        ),
        bad(
            "config set default_space: none given",
            &["config", "set", "default_space"],
        ),
        bad(
            "config set server.public_url: none given",
            &["config", "set", "public_url"],
        ),
        bad("config: a word", &["config", "x"]),
        bad("purge: no hostname", &["purge"]),
        bad(
            "upgrade --accept-binary: not hex",
            &["upgrade", "--accept-binary", "xyz"],
        ),
        bad(
            "upgrade --accept-binary: too short",
            &["upgrade", "--accept-binary", "ABCDEF"],
        ),
        bad(
            "deliveries set: nothing to set",
            &["deliveries", "set", "w"],
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["status"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "open, HOOK_SOCK set and no passport: the require-passport lines",
            &["open"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        with(
            with_passport(&[]),
            one("a host session: TCP", &["status"], STATUS2),
        ),
        with(
            cell(),
            case(
                "the cell socket",
                &["backups"],
                &[],
                vec![ok(r#"{"backups":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        waited(with(
            cell(),
            case(
                "a POST with no answer there: not sent again",
                &["restart"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        with(
            with_passport(&[]),
            case(
                "recovery-key in a session: a person's step",
                &["recovery-key"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "purge in a session: a person's step",
                &["purge", "--confirm-hostname", "box.example"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "rollback --accept-data-loss in a session: the daemon decides",
                &["rollback", "--accept-data-loss"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "open --print in a session: a person's step",
                &["open", "--print"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "open in a session: a person's step",
                &["open"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The large list: above 256 KiB. `backups` prints the list as one
/// `json.dumps` line, as the shell's Python does.
#[test]
fn hostchat_backups_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"id":"bundle-{i:05}","at":{},"size":{},"kind":"nightly"}}"#,
                1_790_000_000 + i,
                1000 + i
            )
        })
        .collect();
    let body = format!(r#"{{"backups":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&one("a large list", &["backups"], &body));
}

/// `_CP_404_EXIT` exported: nothing in the shell reads it any more
/// (`_compute_fail` takes its exit for a 404 from the caller), so the
/// binary answers and a 404 is 3.
#[test]
fn hostchat_an_exported_404_exit_changes_nothing() {
    let rig = Rig::new(VERB, "cp404");
    for c in [
        case(
            "_CP_404_EXIT set: nothing reads it",
            &["status"],
            &[("_CP_404_EXIT", "4")],
            vec![ok(r#"{"state":"x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "_CP_404_EXIT set: a 404 is 3",
            &["status"],
            &[("_CP_404_EXIT", "4")],
            vec![Reply::Answer(404, r#"{"error":"x"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
    ] {
        rig.check(&c);
    }
}

/// What the binary hands to the shell.
#[test]
fn hostchat_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    rig.check(&shell(case(
        "_K2_REFUSAL_AREA set: the shell reads it",
        &["status"],
        &[("_K2_REFUSAL_AREA", "app")],
        vec![ok(r#"{"state":"x"}"#)],
        owner_and_port,
        0,
        1,
    )));
}

/// A value flag as the last word: both print the shell's `_need_value`
/// line, "Error: <flag> takes a value", and exit 2. With an empty value:
/// bash stops on its own `${2:?…}` line (`<path to cli/k2>: line <n>: 2:
/// <flag> <what>`) and exits 1; the binary prints one plain line and
/// exits 2 (K2's D6, `scripts/cli-parity.sh`'s named difference
/// `edge-usage-exit-2`). Neither sends anything.
#[test]
fn hostchat_value_flag_with_no_value_is_the_d6_line() {
    let rig = Rig::new(VERB, "d6");
    let cli = k2_client::brand::CLI_NAME;
    let rows: &[(&'static [&'static str], &str, &str)] = &[
        (&["enable", "--tarball"], "--tarball", "--tarball FILE"),
        (
            &["enable", "--recovery-key-file"],
            "--recovery-key-file",
            "--recovery-key-file FILE",
        ),
        (
            &["enable", "--identity"],
            "--identity",
            "--identity k2|standalone",
        ),
        (
            &["enable", "--voice-url"],
            "--voice-url",
            "--voice-url wss://HOST",
        ),
        (
            &["enable", "--media-tarball"],
            "--media-tarball",
            "--media-tarball FILE",
        ),
        (&["enable", "--node-ip"], "--node-ip", "--node-ip IP"),
        (
            &["upgrade", "--accept-binary", ""],
            "--accept-binary",
            "--accept-binary SHA256",
        ),
        (
            &["purge", "--confirm-hostname", ""],
            "--confirm-hostname",
            "--confirm-hostname HOST",
        ),
        (
            &["config", "--public-url"],
            "--public-url",
            "--public-url https://HOST",
        ),
        (&["media", "install", "--url"], "--url", "--url wss://HOST"),
        (
            &["media", "url", "--voice-url"],
            "--voice-url",
            "--url wss://HOST",
        ),
        (
            &["media", "url", "--public-url", ""],
            "--public-url",
            "--url wss://HOST",
        ),
        (
            &["media", "install", "--node-ip"],
            "--node-ip",
            "--node-ip IP",
        ),
        (
            &["media", "install", "--tarball"],
            "--tarball",
            "--tarball FILE",
        ),
        (
            &["media", "install", "--media-tarball"],
            "--media-tarball",
            "--tarball FILE",
        ),
        (
            &["media", "probe", "--udp", "", "--tcp", "ok"],
            "--udp",
            "--udp ok|fail",
        ),
        (&["media", "probe", "--tcp"], "--tcp", "--tcp ok|fail"),
        (
            &["media", "probe", "--turn-tls"],
            "--turn-tls",
            "--turn-tls ok|fail",
        ),
        (
            &["users", "invite", "ann", "--display-name"],
            "--display-name",
            "--display-name NAME",
        ),
        (
            &["users", "role", "ann", "--role", ""],
            "--role",
            "--role member|admin|guest",
        ),
        (
            &["user", "invite", "ann", "--email"],
            "--email",
            "--email EMAIL",
        ),
    ];
    for &(args, flag, hint) in rows {
        let c = case(
            leak(format!("{} with no value", args.join(" "))),
            args,
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        );
        let name = c.name;
        // The flag as the last word: both print the shell's _need_value
        // line and exit 2. An empty value: bash's own `${2:?…}` line and
        // exit 1, the binary's D6 line and exit 2.
        let last = args.last() == Some(&flag);
        rig.prepare(Side::Bash, &c);
        let b = rig.run(true, &c);
        assert!(b.stdout.is_empty(), "{name}: bash's stdout: {b:?}");
        if last {
            assert_eq!(b.code, Some(2), "{name}: bash's exit: {b:?}");
            assert_eq!(b.stderr, format!("Error: {flag} takes a value\n"), "{name}");
        } else {
            assert_eq!(b.code, Some(1), "{name}: bash's exit: {b:?}");
            assert_eq!(b.stderr.lines().count(), 1, "{name}: bash's stderr: {b:?}");
            assert!(
                b.stderr.ends_with(&format!(": 2: {hint}\n")),
                "{name}: bash's stderr: {b:?}"
            );
        }
        assert!(
            rig.read(Side::Bash, false, name).is_empty(),
            "{name}: bash sent"
        );
        let (r, sent) = rig.binary(&c, false);
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{name}: {r:?}");
        assert!(r.stdout.is_empty(), "{name}: {r:?}");
        let want = if last {
            format!("Error: {flag} takes a value\n")
        } else {
            format!("Error: {flag} needs a value (try '{cli} hostchat --help')\n")
        };
        assert_eq!(r.stderr, want, "{name}");
    }
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell. Where bash acts anyway (a value flag takes
/// `-h`/`--help` as its value, so the loop never sees it: a bash bug,
/// cli/src/hostchat-voice.sh:802-819), the rows copy bash and say so.
#[test]
fn hostchat_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str,
                 args: &'static [&'static str],
                 replies: Vec<Reply>,
                 requests: usize| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], replies, owner_and_port, 0, requests)
    };
    for c in [
        shell("enable -h", &["enable", "--voice", "-h"], vec![], 0),
        shell("disable --help", &["disable", "--help"], vec![], 0),
        shell(
            "upgrade -h",
            &["upgrade", "--tarball", "t", "-h"],
            vec![],
            0,
        ),
        shell("rollback --help", &["rollback", "--help"], vec![], 0),
        shell(
            "restore -h",
            &["restore", "b1", "--confirm", "-h"],
            vec![],
            0,
        ),
        shell(
            "purge -h",
            &["purge", "--confirm-hostname", "h", "-h"],
            vec![],
            0,
        ),
        shell(
            "recovery-key --help",
            &["recovery-key", "--help"],
            vec![],
            0,
        ),
        shell(
            "rotate-master -h",
            &["rotate-master", "--drop-prev", "-h"],
            vec![],
            0,
        ),
        shell("uninstall -h", &["uninstall", "-h"], vec![], 0),
        shell("ship dir -h", &["ship", "dir", "/x", "-h"], vec![], 0),
        shell("mirror --help", &["mirror", "none", "--help"], vec![], 0),
        shell("app-url set -h", &["app-url", "set", "u", "-h"], vec![], 0),
        shell(
            "config set -h",
            &["config", "set", "approvals.by", "people", "-h"],
            vec![],
            0,
        ),
        shell("backup --help", &["backup", "--help"], vec![], 0),
        shell("restart -h", &["-h", "restart"], vec![], 0),
        // covers: hostchat recording policy
        shell(
            "recording policy set -h",
            &["recording", "policy", "set", "dm_enabled", "on", "-h"],
            vec![],
            0,
        ),
        shell(
            "recording policies set --help",
            &["recording", "policies", "set", "enabled", "off", "--help"],
            vec![],
            0,
        ),
        // Before the verb: bash said "unexpected 'policy'" (exit 2) until
        // its recording dispatcher looked for -h anywhere first.
        shell(
            "recording -h policy set",
            &["recording", "-h", "policy", "set", "dm_enabled", "on"],
            vec![],
            0,
        ),
        shell(
            "recording --help policy",
            &["recording", "--help", "policy"],
            vec![],
            0,
        ),
        // A value flag no longer eats the help word: bash installed with
        // "-h" as the file until the same fix.
        shell(
            "recording install --recorder-tarball -h",
            &["recording", "install", "--recorder-tarball", "-h"],
            vec![],
            0,
        ),
        // The bash bug, copied: a value flag eats the help word and the
        // verb runs (flips when the fix lands).
        shell(
            "BASH BUG: enable --tarball -h enables",
            &["enable", "--no-wait", "--tarball", "-h"],
            vec![ok(r#"{"state":"installing"}"#)],
            1,
        ),
        shell(
            "BASH BUG: config --public-url --help sets it",
            &["config", "--public-url", "--help"],
            vec![ok(r#"{"ok":true}"#)],
            1,
        ),
        // chat's value flags, already reported (flips with chat's fix):
        // cli/src/chat.sh:543-545.
        shell(
            "BASH BUG: deliveries set --tz -h sets it",
            &["deliveries", "set", "w", "--tz", "-h"],
            vec![ok(r#"{"workspace":"w","tz":"-h"}"#)],
            1,
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn hostchat_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens: status 000, exit 3",
        &["status"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        3,
        0,
    ));
}

/// A case on the vault rig whose command line starts with `hostchat`.
fn vcase(name: &'static str, args: &[&'static str]) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = VERB;
    c
}

/// A person's step: the install id, then the confirm on the vault socket
/// (the vault password, typed on stdin), then the step with the token.
fn person(name: &'static str, args: &[&'static str], daemon: Vec<Reply>, code: i32) -> VCase {
    let mut c = vcase(name, args);
    c.daemon_requests = daemon.len();
    c.daemon = daemon;
    c.vault = Some(vec![
        ok(r#"{"methods":["vault-password"]}"#),
        ok(r#"{"token":"ctok-5150"}"#),
    ]);
    c.vault_requests = 2;
    c.stdin = b"pass-9191\n".to_vec();
    c.code = code;
    c.secrets = vec!["ctok-5150", "pass-9191"];
    c
}

const INSTALL: &str = r#"{"installId":"inst-7"}"#;

#[test]
fn hostchat_a_person_s_steps() {
    let rig = VRig::new();
    let mut v = vec![
        person(
            "recovery-key",
            &["recovery-key"],
            vec![
                ok(INSTALL),
                ok(r#"{"recoveryKey":"rk-1","warning":"keep it safe"}"#),
            ],
            0,
        ),
        person(
            "recovery-key --json, no install yet",
            &["recovery-key", "--json"],
            vec![ok(r#"{"installId":null}"#), ok(r#"{"key":"rk-2"}"#)],
            0,
        ),
        person(
            "purge",
            &[
                "purge",
                "--confirm-hostname",
                "box.example",
                "--keep-backups",
            ],
            vec![ok(INSTALL), ok(r#"{"state":"purged"}"#)],
            0,
        ),
        person(
            "rotate-master --drop-prev",
            &["rotate-master", "--drop-prev"],
            vec![ok(INSTALL), ok(r#"{"phase":"drop-prev","ok":true}"#)],
            0,
        ),
        person(
            "upgrade --accept-binary",
            &[
                "upgrade",
                "--no-wait",
                "--accept-binary",
                "ABCDEF0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789",
            ],
            vec![ok(INSTALL), ok(r#"{"state":"running","upgraded":true}"#)],
            0,
        ),
        person(
            "rollback --accept-data-loss",
            &["rollback", "--accept-data-loss", "--media-tarball", "m.tgz"],
            vec![
                ok(r#"{"bundle":"b7","to":"0.5.0","from":"0.6.0","since":"2026-11-01"}"#),
                ok(INSTALL),
                ok(
                    r#"{"result":"rolled_back","message":"rolled back","keptBackup":"k1","bundle":"b7","version":"0.5.0","doctorFails":["a","b"]}"#,
                ),
            ],
            0,
        ),
        person(
            "rollback --accept-data-loss --json: incomplete",
            &["rollback", "--accept-data-loss", "--json"],
            vec![
                ok(r#"{"bundle":"b7","to":"0.5.0","from":"0.6.0","since":"s"}"#),
                ok(INSTALL),
                ok(r#"{"result":"restore_failed","message":"no"}"#),
            ],
            3,
        ),
        // covers: hostchat open
        person(
            "open --print",
            &["open", "--print"],
            vec![
                ok(r#"{"appUrl":"https://chat.example/"}"#),
                ok(r#"{"code":"k2sho_abc123"}"#),
            ],
            0,
        ),
        person(
            "open --print: no code",
            &["open", "--print"],
            vec![
                ok(r#"{"appUrl":"https://chat.example"}"#),
                ok(r#"{"code":"x"}"#),
            ],
            3,
        ),
    ];
    let mut unset = vcase("open --print: no app URL", &["open", "--print"]);
    unset.daemon = vec![ok(r#"{"state":"running"}"#)];
    unset.daemon_requests = 1;
    unset.code = 3;
    v.push(unset);
    let mut no_point = vcase(
        "rollback --accept-data-loss: no point to roll back to",
        &["rollback", "--accept-data-loss"],
    );
    no_point.daemon = vec![ok(r#"{"to":"0.5.0"}"#)];
    no_point.daemon_requests = 1;
    no_point.code = 3;
    v.push(no_point);
    let mut refused = vcase(
        "purge: the confirm refused",
        &["purge", "--confirm-hostname", "h"],
    );
    refused.daemon = vec![ok(INSTALL)];
    refused.daemon_requests = 1;
    refused.vault = Some(vec![Reply::Answer(
        403,
        r#"{"error":{"code":"denied","hint":"no"}}"#.into(),
    )]);
    refused.vault_requests = 1;
    refused.code = 3;
    v.push(refused);
    let mut tb = vcase(
        "recovery-key: an install id that is no JSON",
        &["recovery-key"],
    );
    tb.daemon = vec![ok("nope")];
    tb.daemon_requests = 1;
    tb.code = 1;
    tb.compare = VCompare::Traceback;
    v.push(tb);
    for c in &v {
        rig.check(c);
    }
}

/// Stand-in openers, `open` and `xdg-open`, in a folder of their own:
/// each writes what it was given to `<rig>/<name>.txt` (how many words,
/// the page's path with mktemp's six characters masked, the page's mode
/// and its folder's, and the page), then exits `code`. Never a browser.
fn openers(rig: &VRig, name: &str, code: i32) -> (String, PathBuf) {
    let dir = rig.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let rec = rig.path().join(format!("{name}.txt"));
    let script = format!(
        r#"#!/bin/sh
p="$1"
{{
  printf 'argc: %s\n' "$#"
  case "$p" in
    "$TMPDIR"/k2-hostchat-open.??????/open.html) printf 'page: $TMPDIR/k2-hostchat-open.XXXXXX/open.html\n' ;;
    *) printf 'page: %s\n' "$p" ;;
  esac
  printf 'mode: %s\n' "$(ls -ld "$p" | cut -c1-10)"
  printf 'folder: %s\n' "$(ls -ld "${{p%/*}}" | cut -c1-10)"
  cat "$p"
}} > '{}'
exit {code}
"#,
        rec.display()
    );
    for prog in ["open", "xdg-open"] {
        let p = dir.join(prog);
        std::fs::write(&p, &script).unwrap();
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let path = std::env::var("PATH").expect("a PATH");
    (format!("{}:{path}", dir.display()), rec)
}

/// This `PATH` with no `open` or `xdg-open` on it: each folder that has
/// one is replaced by a folder of links to its other files.
fn path_without_openers(rig: &VRig) -> String {
    let path = std::env::var_os("PATH").expect("a PATH");
    let mut dirs = Vec::new();
    for (i, dir) in std::env::split_paths(&path).enumerate() {
        let has = ["open", "xdg-open"]
            .iter()
            .any(|p| std::fs::symlink_metadata(dir.join(p)).is_ok());
        if !has {
            dirs.push(dir);
            continue;
        }
        let shadow = rig.path().join(format!("no-opener-{i}"));
        std::fs::create_dir_all(&shadow).unwrap();
        for e in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let name = e.unwrap().file_name();
            if name != "open" && name != "xdg-open" {
                std::os::unix::fs::symlink(dir.join(&name), shadow.join(&name)).unwrap();
            }
        }
        dirs.push(shadow);
    }
    std::env::join_paths(dirs)
        .expect("the folders join")
        .into_string()
        .expect("a UTF-8 PATH")
}

/// `open` without `--print`: the browser gets the path of a private page
/// (0600, in a fresh 0700 folder under TMPDIR) that holds the link, and
/// the link goes on no command line. The page each side made must be the
/// same bytes. The test switch makes the removal's wait 0 s.
#[test]
fn hostchat_open_through_a_private_page() {
    let rig = VRig::new();
    let (ok_path, ok_rec) = openers(&rig, "opener-ok", 0);
    let (fail_path, fail_rec) = openers(&rig, "opener-fails", 1);
    let opened = |name: &'static str, app: &str, path: &str, rec: &Path, code: i32| {
        let mut c = person(
            name,
            &["open"],
            vec![
                ok(&format!(r#"{{"appUrl":"{app}"}}"#)),
                ok(r#"{"code":"k2sho_OPEN0001"}"#),
            ],
            code,
        );
        c.env = vec![
            ("PATH", path.to_string()),
            ("K2_TEST_HOSTCHAT_OPEN_WAIT", "0".to_string()),
        ];
        c.files = vec![rec.to_path_buf()];
        c.secrets.push("k2sho_OPEN0001");
        c
    };
    let mut v = vec![
        opened(
            "open: the opener gets one private page",
            "https://chat.example/",
            &ok_path,
            &ok_rec,
            0,
        ),
        opened(
            "open: the page escapes a link with quotes and markup",
            r#"https://chat.example/a&b<\"'>\u00e9/"#,
            &ok_path,
            &ok_rec,
            0,
        ),
        opened(
            "open: the opener fails: exit 3, nothing more said",
            "https://chat.example",
            &fail_path,
            &fail_rec,
            3,
        ),
        opened(
            "open: no opener here",
            "https://chat.example",
            &path_without_openers(&rig),
            &ok_rec,
            3,
        ),
    ];
    let mut no_tmp = opened(
        "open: TMPDIR can't hold the page's folder: mktemp's line",
        "https://chat.example",
        &ok_path,
        &ok_rec,
        3,
    );
    no_tmp.env.push((
        "TMPDIR",
        rig.path().join("no-such-tmp").display().to_string(),
    ));
    no_tmp.compare = VCompare::MktempTried {
        stem: "k2-hostchat-open.",
    };
    v.push(no_tmp);
    for c in &v {
        rig.check(c);
    }
}

const REC: &str = r#"{"installed":true,"enabled":true,"version":{"recorder":"0.9.0","transcriber":"1.2"},"model":"base","policies":{"b":true,"a":{"x":1}},"flags":{"appRecording":true,"x":false,"c":1},"progress":{"step":"fetch","result":"error"},"lastError":"e","ready":{"ok":false,"code":"no_gpu","hint":"h"}}"#;
const POLICIES: &str = r#"{"flags":{"appRecording":false,"voiceRecordCap":true},"policies":[{"key":"recording.enabled","want":true,"seen":false,"delivered":false,"setBy":"ann"},{"key":"recording.audio_max_days","want":30,"seen":30,"delivered":true},{"key":"recording.required","seen":null}],"locks":[{"spaceId":"s1","lockedBy":"ann","lockedAt":1790000000},{"spaceId":"s2"}],"k5SpaceId":"k5s"}"#;

#[test]
fn hostchat_recording_as_the_shell_does() {
    let rig = Rig::new(VERB, "rec");
    let utc = |c: Case| with(vec![("TZ", "UTC".to_string())], c);
    let v = vec![
        // covers: hostchat recording
        one("recording status", &["recording", "status"], REC),
        one(
            "recording status --json",
            &["recording", "status", "--json"],
            REC,
        ),
        one(
            "recording status: not installed",
            &["recording", "status"],
            r#"{"installed":false,"policies":{},"flags":{}}"#,
        ),
        one(
            "recording preflight",
            &["recording", "preflight"],
            r#"{"ready":false,"checks":[{"status":"fail","id":"disk","detail":"2 GB","hint":"free 10 GB"}]}"#,
        ),
        one(
            "recording install: the files, no wait needed",
            &[
                "recording",
                "install",
                "--recorder-tarball",
                "rec.tgz",
                "--model-file",
                "/m/base.bin",
                "--transcriber-tarball",
                "",
            ],
            r#"{"state":"installed","enabled":false,"version":"0.9.0","model":"base","next":"enable it"}"#,
        ),
        many(
            "recording install: waits until done",
            &["recording", "install"],
            &[
                r#"{"state":"installing"}"#,
                r#"{"installed":false,"progress":{"step":"fetch","result":"running"}}"#,
                r#"{"installed":true,"enabled":false,"progress":{"step":"done","result":"done"}}"#,
            ],
            0,
        ),
        many(
            "recording install --json: the wait ends in an error",
            &["recording", "install", "--json"],
            &[
                r#"{"state":"installing"}"#,
                r#"{"installed":false,"progress":{"step":"fetch","result":"error"},"lastError":"no net"}"#,
            ],
            3,
        ),
        one(
            "recording install --no-wait",
            &["recording", "install", "--no-wait"],
            r#"{"state":"installing"}"#,
        ),
        one(
            "recording disable",
            &["recording", "disable"],
            r#"{"installed":true,"kept":"recordings","warnings":["w1"]}"#,
        ),
        one(
            "recording uninstall --yes",
            &["recording", "uninstall", "-y"],
            r#"{"installed":false}"#,
        ),
        case(
            "recording uninstall with no terminal: confirm_required",
            &["recording", "uninstall"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "recording enable: not ready",
            &["recording", "enable"],
            &[],
            vec![ok(REC)],
            owner_and_port,
            3,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "recording enable in a session: the daemon answers",
                &["recording", "enable"],
                &[],
                vec![ok(r#"{"state":"installing","version":"0.9.0"}"#)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "recording alone: its help",
            &["recording"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "recording help",
            &["recording", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        bad(
            "recording: --yes on status",
            &["recording", "status", "--yes"],
        ),
        bad(
            "recording: --model-file on status",
            &["recording", "status", "--model-file", "m"],
        ),
        bad(
            "recording: --model-file with no file",
            &["recording", "install", "--model-file"],
        ),
        bad(
            "recording: an unknown flag",
            &["recording", "status", "--x"],
        ),
        bad("recording: a word", &["recording", "status", "now"]),
        bad("recording: an unknown verb", &["recording", "start"]),
        // covers: hostchat recording policy
        utc(one("recording policy", &["recording", "policy"], POLICIES)),
        one(
            "recording policies --json",
            &["recording", "policies", "--json"],
            POLICIES,
        ),
        one(
            "recording policy: no flags, no locks list",
            &["recording", "policy"],
            r#"{"policies":[],"locks":[]}"#,
        ),
        utc(one(
            "recording policy set",
            &[
                "recording",
                "policy",
                "set",
                "enabled",
                "on",
                "recording.audio_max_days",
                "030d",
                "required",
                "multi_person",
                "enabled",
                "off",
            ],
            r#"{"changed":["recording.enabled","recording.audio_max_days"],"policies":[]}"#,
        )),
        one(
            "recording policy set: nothing changed",
            &["recording", "policy", "set", "dm_enabled", "no"],
            r#"{"changed":[]}"#,
        ),
        case(
            "recording policy set: refused",
            &["recording", "policy", "set", "required", "off"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"the owner"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "recording policy set in a session: held, no confirm",
                &["recording", "policy", "set", "enabled", "on"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"vault_confirm_required","hint":"in person"}}"#.into(),
                )],
                port_only,
                3,
                1,
            ),
        ),
        // 0.46.17: `dm_enabled` alone, in a session: person_only with the
        // `voice.recording-dm` card's Ask-with line, no confirm.
        with(
            with_passport(&[]),
            case(
                "recording policy set dm_enabled in a session: the card",
                &["recording", "policy", "set", "dm_enabled", "on"],
                &[],
                vec![Reply::Answer(403, DM_CARD.into())],
                port_only,
                3,
                1,
            ),
        ),
        bad("recording policy: a word", &["recording", "policy", "x"]),
        bad(
            "recording policy: an unknown flag",
            &["recording", "policy", "--x"],
        ),
        bad(
            "recording policy set: nothing",
            &["recording", "policy", "set"],
        ),
        bad(
            "recording policy set: odd",
            &["recording", "policy", "set", "enabled"],
        ),
        bad(
            "recording policy set: unknown key",
            &["recording", "policy", "set", "color", "é"],
        ),
        bad(
            "recording policy set: not on or off",
            &["recording", "policy", "set", "enabled", "maybe"],
        ),
        bad(
            "recording policy set: required",
            &["recording", "policy", "set", "required", "all"],
        ),
        bad(
            "recording policy set: days 0",
            &["recording", "policy", "set", "audio_max_days", "0"],
        ),
        bad(
            "recording policy set: days 3651",
            &["recording", "policy", "set", "audio_max_days", "3651"],
        ),
        bad(
            "recording policy set: days x",
            &["recording", "policy", "set", "audio_max_days", "d"],
        ),
        tb("recording status: a list", &["recording", "status"], "[]"),
        tb(
            "recording policy: a lock time that is no number",
            &["recording", "policy"],
            r#"{"locks":[{"lockedAt":"x"}]}"#,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_recording_a_person_s_steps() {
    let rig = VRig::new();
    let mut v = vec![
        person(
            "recording enable",
            &["recording", "enable", "--model-file", "m.bin"],
            vec![
                ok(r#"{"installed":true,"ready":{"ok":true}}"#),
                ok(INSTALL),
                ok(r#"{"state":"on","version":"0.9.0","model":"m","next":"n"}"#),
            ],
            0,
        ),
        person(
            "recording policy set: held for a confirm",
            &["recording", "policy", "set", "required", "multi_person"],
            vec![
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_confirm_required","hint":"in person"}}"#.into(),
                ),
                ok(INSTALL),
                ok(r#"{"changed":["recording.required"]}"#),
            ],
            0,
        ),
    ];
    let mut tty = vcase(
        "recording uninstall: no terminal",
        &["recording", "uninstall"],
    );
    tty.code = 2;
    v.push(tty);
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn hostchat_media_as_the_shell_does() {
    let rig = Rig::new(VERB, "media");
    let v = vec![
        // covers: hostchat media
        one(
            "media preflight",
            &["media", "preflight"],
            r#"{"ready":true,"checks":[{"status":"ok","id":"udp","detail":"open"}]}"#,
        ),
        one("media status", &["media", "status"], STATUS),
        one(
            "media status --json",
            &["media", "status", "--json"],
            STATUS,
        ),
        one(
            "media status --json: no voice",
            &["media", "status", "--json"],
            r#"{"state":"x"}"#,
        ),
        one(
            "media install",
            &[
                "media",
                "install",
                "--url",
                "wss://v.example",
                "--node-ip",
                "192.0.2.10",
                "--tarball",
                "media.tgz",
            ],
            r#"{"voice":{"state":"installing"},"ok":true}"#,
        ),
        many(
            "media install: waits on voice",
            &["media", "install", "--media-tarball", "/m.tgz"],
            &[
                r#"{"voice":"installing"}"#,
                r#"{"state":"running","voice":{"progress":{"step":"cert","result":"running"}}}"#,
                r#"{"state":"running","voice":{"state":"on","progress":{"step":"cert","result":"done"}}}"#,
            ],
            0,
        ),
        one(
            "media url",
            &["media", "url", "wss://v.example"],
            r#"{"ok":true,"publicUrl":"wss://v.example"}"#,
        ),
        one(
            "media url --url",
            &["media", "url", "--public-url", "wss://w.example"],
            r#"{"ok":true}"#,
        ),
        one(
            "media renew",
            &["media", "renew"],
            r#"{"renewed":true,"daysLeft":89}"#,
        ),
        one("media rebind", &["media", "rebind"], r#"{"rebound":true}"#),
        one(
            "media tier",
            &["media", "tier", "tunnel"],
            r#"{"tier":"tunnel"}"#,
        ),
        one(
            "media probe",
            &[
                "media",
                "probe",
                "--udp",
                "ok",
                "--tcp",
                "fail",
                "--turn-tls",
                "ok",
            ],
            r#"{"exposure":{"note":"tcp blocked"}}"#,
        ),
        one(
            "media uninstall --yes",
            &["media", "uninstall", "--yes"],
            r#"{"removed":true}"#,
        ),
        case(
            "media uninstall with no terminal",
            &["media", "uninstall"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "media alone: the help",
            &["media"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        bad("media url: none", &["media", "url"]),
        bad("media tier: a bad tier", &["media", "tier", "fast"]),
        bad(
            "media probe: a flag missing",
            &["media", "probe", "--udp", "ok"],
        ),
        bad("media: --yes on renew", &["media", "renew", "-y"]),
        bad("media: an unknown flag", &["media", "renew", "--x"]),
        bad("media: an unknown verb", &["media", "start"]),
        tb(
            "media status --json: a list",
            &["media", "status", "--json"],
            "[]",
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_users_as_the_shell_does() {
    let rig = Rig::new(VERB, "users");
    let link = r#"{"username":"ann","installRole":"admin","setupLink":{"url":"https://chat.example/setup#t","expiresAt":"2026-12-01"}}"#;
    let v = vec![
        // covers: hostchat users
        one(
            "users",
            &["users"],
            r#"{"users":[{"username":"ann","installRole":"owner","kind":"person","displayName":"Ann é"},{"externalId":"x1","installRole":"member","kind":"guest","deactivated":true}]}"#,
        ),
        one("users list: none", &["users", "list"], r#"{"users":[]}"#),
        one(
            "user list --json",
            &["user", "list", "--json"],
            r#"{"users":[]}"#,
        ),
        one(
            "users owner",
            &["users", "owner", "ann", "--display-name", "Ann"],
            link,
        ),
        one(
            "users invite",
            &[
                "users",
                "invite",
                "bob",
                "--email",
                "bob@example.test",
                "--admin",
            ],
            link,
        ),
        one(
            "users invite: a member",
            &["users", "invite", "cy"],
            r#"{"username":"cy","installRole":"member"}"#,
        ),
        one("users reset", &["users", "reset", "ann"], link),
        one(
            "users revoke",
            &["users", "revoke", "ann"],
            r#"{"username":"ann","sessionsRevoked":0}"#,
        ),
        one(
            "users reactivate --json",
            &["users", "reactivate", "ann", "--json"],
            link,
        ),
        one(
            "users role",
            &["users", "role", "ann", "member"],
            r#"{"username":"ann","installRole":"member"}"#,
        ),
        one(
            "users role --role",
            &["users", "role", "ann", "--role", "admin"],
            r#"{"username":"ann"}"#,
        ),
        bad("users owner: no name", &["users", "owner"]),
        bad("users role: no role", &["users", "role", "ann"]),
        bad("users: an unknown verb", &["users", "kick", "ann"]),
        bad("users: an unknown flag", &["users", "list", "--x"]),
        case(
            "users help",
            &["users", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        tb("users list: a list", &["users"], "[1]"),
    ];
    for c in v {
        rig.check(&c);
    }
}

const HELD: &str = r#"{"error":{"code":"confirm_required","hint":"in person","details":{"action":"chat-approve","secretId":"out_1"}}}"#;

#[test]
fn hostchat_chat_administration_as_the_shell_does() {
    let rig = Rig::new(VERB, "admin");
    let utc = |c: Case| with(vec![("TZ", "UTC".to_string())], c);
    let v = vec![
        // covers: hostchat people
        one(
            "people",
            &["people"],
            r#"{"people":[{"user":"ann","subjectKind":"person","scope":"box"},{"user":"bot","subjectKind":"agent","scope":"ws","enabled":false}]}"#,
        ),
        one("people: none", &["person", "list"], r#"{"people":[]}"#),
        bad("people --json", &["people", "--json"]),
        one(
            "people add, a guest",
            &["people", "add", "@ann", "--guest"],
            r#"{"ok":true,"user":"ann","guest":true}"#,
        ),
        one(
            "people grant --guest off",
            &["people", "grant", "ann", "--guest", "off"],
            r#"{"ok":true}"#,
        ),
        one(
            "people set",
            &["people", "set", "ann", "--guest", "yes"],
            r#"{"user":"ann"}"#,
        ),
        one(
            "people remove",
            &["people", "remove", "ann", "--json"],
            r#"{"ok":true}"#,
        ),
        bad("people add: no user", &["people", "add"]),
        bad("people set: no --guest", &["people", "set", "ann"]),
        bad("people: a bad verb", &["people", "kick"]),
        bad("people: an unknown flag", &["people", "--x"]),
        // covers: hostchat approvals
        one(
            "approvals",
            &["approvals"],
            r#"{"items":[{"id":"out_1","status":"queued","addr":"s/c","body":"hi"}]}"#,
        ),
        one(
            "approvals: an id first is a list",
            &["approvals", "out_1", "--workspace", "w b"],
            r#"{"items":[]}"#,
        ),
        one(
            "approvals approve",
            &["approvals", "approve", "out_1", "--note", "ok"],
            r#"{"ok":true,"id":"out_1"}"#,
        ),
        one(
            "approvals approve: two",
            &["approval", "approve", "out_1", "out_2"],
            r#"{"ok":true,"approved":["out_1","out_2"]}"#,
        ),
        case(
            "approvals approve: refused",
            &["approvals", "approve", "out_1"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"not_yours","hint":"h"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "approvals approve: held, no action named",
            &["approvals", "approve", "out_1"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"confirm_required","hint":"h"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "approvals approve in a session: held",
                &["approvals", "approve", "out_1"],
                &[],
                vec![Reply::Answer(403, HELD.into())],
                port_only,
                3,
                1,
            ),
        ),
        one(
            "approvals deny",
            &["approvals", "deny", "out_1", "--note", "no \"way\""],
            r#"{"ok":true}"#,
        ),
        bad("approvals deny: no note", &["approvals", "deny", "out_1"]),
        bad("approvals approve: none", &["approvals", "approve"]),
        bad(
            "approvals --note: empty",
            &["approvals", "approve", "out_1", "--note", ""],
        ),
        bad("approvals: a bad verb", &["approvals", "skip"]),
        // covers: hostchat on-behalf
        one(
            "on-behalf",
            &["on-behalf", "--state", "all", "--limit", "5"],
            r#"{"person":"ann","items":[{"write":"post","messageId":"m1","action":"posted","state":"approved","workspace":"w","decidedBy":"ann","decidedKind":"person","note":7},{"write":"edit","state":"pending"}]}"#,
        ),
        one(
            "on-behalf: none",
            &["behalf", "list"],
            r#"{"person":"ann","items":[]}"#,
        ),
        one(
            "on-behalf approve",
            &["on-behalf", "approve", "act_1", "--note", "fine"],
            r#"{"ok":true,"state":"approved"}"#,
        ),
        one(
            "on-behalf decline --json",
            &["on-behalf", "decline", "msg_1", "--json"],
            r#"{"ok":true}"#,
        ),
        with(
            with_passport(&[]),
            case(
                "on-behalf approve in a session: a person's step",
                &["on-behalf", "approve", "act_1"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        bad("on-behalf approve: no id", &["on-behalf", "approve"]),
        bad("on-behalf: --state empty", &["on-behalf", "--state", ""]),
        bad("on-behalf: a bad verb", &["on-behalf", "undo"]),
        // covers: hostchat log
        utc(one(
            "log",
            &["log", "--limit", "5"],
            r#"{"entries":[{"at":1790000000,"outcome":"ok","actor":"ann","op":"people.grant","workspace":"w","target":"bob","reason":"asked"},{"at":"x","outcome":"denied"},{"at":null}]}"#,
        )),
        one("log: none", &["log"], r#"{"entries":[]}"#),
        one("log --json", &["log", "--json"], r#"{"entries":[]}"#),
        bad("log: a word", &["log", "x"]),
        bad("log --limit: none", &["log", "--limit"]),
        tb(
            "log: a reason that is no string",
            &["log"],
            r#"{"entries":[{"at":1,"reason":5}]}"#,
        ),
        tb("people: a list", &["people"], "[]"),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_chat_administration_a_person_s_steps() {
    let rig = VRig::new();
    let v = vec![
        person(
            "approvals approve: held, then confirmed",
            &["approvals", "approve", "out_1", "--note", "ok"],
            vec![
                Reply::Answer(403, HELD.into()),
                ok(r#"{"ok":true,"id":"out_1"}"#),
            ],
            0,
        ),
        person(
            "on-behalf approve: held, then confirmed",
            &["on-behalf", "approve", "act_1"],
            vec![
                Reply::Answer(403, HELD.into()),
                ok(r#"{"ok":true,"state":"approved"}"#),
            ],
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// A request as the rig logs it: its request line and its body.
fn line_and_body(r: &str) -> (String, String) {
    let line = r.lines().nth(1).unwrap_or_default().to_string();
    let body = r
        .split_once("\n\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (line, body)
}

/// What an `owner-sessions` row must print and send, on top of the
/// shell's answer: the rows pin the answer itself, so both sides being
/// wrong the same way still fails.
struct Want {
    stdout: String,
    /// stderr exactly, or (for a traceback) its last line.
    stderr: String,
    traceback: bool,
    sent: Vec<(&'static str, &'static str)>,
}

fn want(stdout: &str, stderr: &str, sent: &[(&'static str, &'static str)]) -> Want {
    Want {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        traceback: false,
        sent: sent.to_vec(),
    }
}

fn check_want(name: &str, out: &Out, sent: &[String], code: i32, w: &Want) {
    assert_eq!(
        (out.code, out.signal),
        (Some(code), None),
        "{name}: {out:?}"
    );
    assert_eq!(out.stdout, w.stdout, "{name}: stdout");
    if w.traceback {
        assert!(
            out.stderr
                .starts_with("Traceback (most recent call last):\n")
                && out.stderr.ends_with(&w.stderr),
            "{name}: stderr: {:?}",
            out.stderr
        );
    } else {
        assert_eq!(out.stderr, w.stderr, "{name}: stderr");
    }
    let got: Vec<(String, String)> = sent.iter().map(|r| line_and_body(r)).collect();
    let want: Vec<(String, String)> = w
        .sent
        .iter()
        .map(|(l, b)| (l.to_string(), b.to_string()))
        .collect();
    assert_eq!(got, want, "{name}: requests");
}

const OWNER_SESSIONS: &str = r#"{"sessions":[{"sid":"own_a","mintedAt":1790000000,"mintedBy":"ann","redeemedAt":1790000060},{"sid":"own_b","mintedAt":1790000000,"mintedBy":"ann","redeemedAt":null,"end":{"queuedAt":1790000120,"by":"bob"},"delivered":true},{"sid":"own_c","mintedAt":"x","mintedBy":"ann","end":{"queuedAt":1790000120,"by":"bob","via":"all","outbox":"ob_1"}},{"sid":"own_d","end":{"queuedAt":null,"by":null}}]}"#;

/// `hostchat owner-sessions` (`_hostchat_owner_sessions`,
/// cli/src/hostmail-app-cert-backup.sh): the list and its `--json`, the
/// alias, the usage lines, the refusals, a session's person-only stop
/// and Python's raise, each compared with the shell's answer and pinned
/// to what it must be.
#[test]
fn hostchat_owner_sessions_as_the_shell_does() {
    let rig = Rig::new(VERB, "owner-sessions");
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    let get = ("GET /cli/sew/owner-sessions HTTP/1.1", "");
    let post = "POST /cli/sew/owner-sessions/revoke HTTP/1.1";
    let utc = |c: Case| with(vec![("TZ", "UTC".to_string())], c);
    let usage = |hint: String| {
        want(
            "",
            &format!("{{\"error\": \"usage\", \"hint\": \"{hint}\"}}\n"),
            &[],
        )
    };
    let end_usage = || {
        usage(format!(
            "{cli} hostchat owner-sessions end <own_\\u2026> | end --all"
        ))
    };
    let person_only = format!("{{\"error\":\"person_only\",\"hint\":\"a person ends the owner's chat sessions, from a terminal outside {product} (Touch ID or the vault password)\"}}\n");
    let mut raise = want("", "TypeError: 'int' object is not iterable\n", &[get]);
    raise.traceback = true;
    let rows: Vec<(Case, Want)> = vec![
        // covers: hostchat owner-sessions
        (
            utc(one("list: every state", &["owner-sessions"], OWNER_SESSIONS)),
            want(
                concat!(
                    "own_a  minted 2026-09-21 14:13 by ann  opened 2026-09-21 14:14  live until Sew ends it\n",
                    "own_b  minted 2026-09-21 14:13 by ann  opened None  ended (Sew took the end 2026-09-21 14:15)\n",
                    "own_c  minted x by ann  opened None  end queued 2026-09-21 14:15 by bob (all), outbox ob_1\n",
                    "own_d  minted None by None  opened None  end queued None by None, outbox -\n",
                ),
                "",
                &[get],
            ),
        ),
        // native: hostchat owner-sessions list
        (
            one("list --json", &["owner-sessions", "list", "--json"], OWNER_SESSIONS),
            want(&format!("{OWNER_SESSIONS}\n"), "", &[get]),
        ),
        (
            one("--json alone is the list", &["owner-sessions", "--json"], r#"{"x":1}"#),
            want("{\"x\":1}\n", "", &[get]),
        ),
        // native: hostchat owner-session
        (
            one("owner-session \"\": none", &["owner-session", ""], r#"{"sessions":[]}"#),
            want(
                "(no owner chat sessions recorded in the last 30 days; end --all still ends any owner session)\n",
                "",
                &[get],
            ),
        ),
        (
            utc(one(
                "list: times as text, numbers and a float",
                &["owner-sessions", "list"],
                r#"{"sessions":[{"sid":"own_t","mintedAt":"1790000000","mintedBy":7,"redeemedAt":1790000060.9,"end":{"queuedAt":true,"by":"x","via":"sid","outbox":""},"delivered":0}]}"#,
            )),
            want(
                "own_t  minted 2026-09-21 14:13 by 7  opened 2026-09-21 14:14  end queued 1970-01-01 00:00 by x, outbox -\n",
                "",
                &[get],
            ),
        ),
        (
            traceback(case(
                "list: sessions that are a number",
                &["owner-sessions"],
                &[],
                vec![ok(r#"{"sessions":5}"#)],
                owner_and_port,
                1,
                1,
            )),
            raise,
        ),
        (
            case(
                "list refused",
                &["owner-sessions"],
                &[],
                vec![Reply::Answer(403, r#"{"error":{"code":"forbidden","hint":"the owner or an admin"}}"#.into())],
                owner_and_port,
                3,
                1,
            ),
            want("", "{\"error\": \"forbidden\", \"hint\": \"the owner or an admin\"}\n", &[get]),
        ),
        (
            case(
                "list: a 500 that is no JSON",
                &["owner-sessions"],
                &[],
                vec![Reply::Answer(500, "boom".into())],
                owner_and_port,
                1,
                1,
            ),
            want("", "{\"error\": \"daemon_error\", \"hint\": \"boom\"}\n", &[get]),
        ),
        (
            bad("list with a word", &["owner-sessions", "list", "x"]),
            usage(format!("{cli} hostchat owner-sessions [list] [--json]")),
        ),
        (
            bad("list --all", &["owner-sessions", "list", "--all"]),
            usage(format!("{cli} hostchat owner-sessions [list] [--json]")),
        ),
        (
            bad("--all alone is the list's", &["owner-sessions", "--all"]),
            usage(format!("{cli} hostchat owner-sessions [list] [--json]")),
        ),
        // native: hostchat owner-sessions end
        (
            bad("end with no id", &["owner-sessions", "end"]),
            end_usage(),
        ),
        (
            bad("end with two ids", &["owner-sessions", "end", "own_1", "own_2"]),
            end_usage(),
        ),
        // native: hostchat owner-sessions revoke
        (
            bad("end --all with an id", &["owner-sessions", "revoke", "--all", "own_1"]),
            end_usage(),
        ),
        (
            bad("end, not an own_ id", &["owner-sessions", "end", "sid_1"]),
            usage(format!("an owner session's id starts with own_ ({cli} hostchat owner-sessions)")),
        ),
        (
            bad("end, an empty id", &["owner-sessions", "end", ""]),
            usage(format!("an owner session's id starts with own_ ({cli} hostchat owner-sessions)")),
        ),
        (
            bad("an unknown verb", &["owner-sessions", "nope"]),
            usage(format!("{cli} hostchat owner-sessions [list] | end <own_\\u2026> | end --all")),
        ),
        (
            bad("help is no verb here", &["owner-sessions", "help"]),
            usage(format!("{cli} hostchat owner-sessions [list] | end <own_\\u2026> | end --all")),
        ),
        (
            bad("an unknown flag", &["owner-sessions", "end", "--x"]),
            usage(format!("unknown flag --x ({cli} hostchat owner-sessions --help)")),
        ),
        (
            bad("a lone dash is a flag", &["owner-sessions", "-"]),
            usage(format!("unknown flag - ({cli} hostchat owner-sessions --help)")),
        ),
        (
            // An answer at once (no confirm asked): what was queued, and
            // the id as one field, a newline and `Token=` in it as they
            // are (_compute_json reads every value on stdin).
            case(
                "end: an id with a newline and Token= in it, answered at once",
                &["owner-sessions", "end", "own_a\nToken=b"],
                &[],
                vec![ok(r#"{"queued":"own_a\nToken=b","ended":[]}"#)],
                owner_and_port,
                0,
                1,
            ),
            want(
                "end queued: own_a\nToken=b\n\n",
                "",
                &[(post, r#"{"sid": "own_a\nToken=b"}"#)],
            ),
        ),
        (
            case(
                "end: refused outright (no confirm named)",
                &["owner-sessions", "end", "--all"],
                &[],
                vec![Reply::Answer(403, r#"{"error":{"code":"forbidden","hint":"the owner or an admin"}}"#.into())],
                owner_and_port,
                3,
                1,
            ),
            want(
                "",
                "{\"error\": \"forbidden\", \"hint\": \"the owner or an admin\"}\n",
                &[(post, r#"{"all": true}"#)],
            ),
        ),
        (
            traceback(case(
                "end: marked ids that are numbers",
                &["owner-sessions", "end", "own_a"],
                &[],
                vec![ok(r#"{"queued":1,"ended":[1,2]}"#)],
                owner_and_port,
                1,
                1,
            )),
            {
                let mut w = want(
                    "end queued: 1\n",
                    "TypeError: sequence item 0: expected str instance, int found\n",
                    &[(post, r#"{"sid": "own_a"}"#)],
                );
                w.traceback = true;
                w
            },
        ),
        (
            with(
                with_passport(&[]),
                case(
                    "end in a session: a person's step",
                    &["owner-sessions", "end", "own_1"],
                    &[],
                    vec![],
                    port_only,
                    3,
                    0,
                ),
            ),
            want("", &person_only, &[]),
        ),
        (
            with(
                with_passport(&[]),
                case(
                    "end --all in a session: a person's step",
                    &["owner-sessions", "end", "--all"],
                    &[],
                    vec![],
                    port_only,
                    3,
                    0,
                ),
            ),
            want("", &person_only, &[]),
        ),
        (
            with(
                with_passport(&[]),
                case(
                    "end in a session with a bad id: the usage line first",
                    &["owner-sessions", "end", "x"],
                    &[],
                    vec![],
                    port_only,
                    2,
                    0,
                ),
            ),
            usage(format!("an owner session's id starts with own_ ({cli} hostchat owner-sessions)")),
        ),
        (
            with(
                with_passport(&[]),
                case(
                    "list in a session: the daemon decides",
                    &["owner-sessions"],
                    &[],
                    vec![Reply::Answer(403, r#"{"error":{"code":"person_only","hint":"never an agent"}}"#.into())],
                    port_only,
                    3,
                    1,
                ),
            ),
            want("", "{\"error\": \"person_only\", \"hint\": \"never an agent\"}\n", &[get]),
        ),
    ];
    for (c, w) in &rows {
        rig.check(c);
        let (r, sent) = rig.binary(c, false);
        check_want(c.name, &r, &sent, c.code, w);
    }
}

/// `owner-sessions` with no answer: a GET on the cell socket goes again
/// over TCP; a POST with no answer is not sent again; nothing listening
/// is `_chat_fail`'s line for status 000, exit 1.
#[test]
fn hostchat_owner_sessions_with_no_answer() {
    let rig = Rig::new(VERB, "owner-sessions-gone");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let port = dead_port();
    for c in [
        with(
            cell(),
            case(
                "list: no answer on the cell socket, then TCP",
                &["owner-sessions"],
                &[],
                vec![Reply::Close, ok(r#"{"sessions":[]}"#)],
                port_only,
                0,
                2,
            ),
        ),
        case(
            "end: no answer (curl 52): not sent again",
            &["owner-sessions", "end", "own_a"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "end --all: a reset (curl 56): not sent again",
            &["owner-sessions", "end", "--all"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "list: nothing listens (curl 7): status 000, exit 1",
            &["owner-sessions"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ] {
        rig.check(&c);
    }
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on an
/// `owner-sessions` line prints its help and sends nothing, on both
/// sides. The exact help forms are the front's captured pages; the rest
/// the binary hands to the shell, whose loop prints the same page.
#[test]
fn hostchat_owner_sessions_help_never_acts() {
    let rig = Rig::new(VERB, "owner-sessions-help");
    let lines: &[&'static [&'static str]] = &[
        &["owner-sessions", "--help"],
        &["owner-sessions", "list", "-h"],
        &["owner-sessions", "end", "--help"],
        &["owner-sessions", "end", "own_a", "-h"],
        &["owner-sessions", "end", "own_a", "--help"],
        &["owner-sessions", "end", "--all", "-h"],
        &["owner-sessions", "end", "-h", "--all"],
        &["owner-sessions", "-h", "end", "--all"],
        &["owner-sessions", "revoke", "--all", "--help"],
        &["owner-session", "end", "own_a", "--json", "-h"],
        // A bad id before the help word: the loop reads every word before
        // it looks at the id, so this is help too.
        &["owner-sessions", "end", "x", "-h"],
    ];
    for &args in lines {
        let name = leak(args.join(" "));
        rig.check(&case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    // An unknown flag before the help word is the usage line: the loop
    // stops there.
    rig.check(&bad(
        "end --x -h: the unknown flag first",
        &["owner-sessions", "end", "--x", "-h"],
    ));
}

/// The large list: above 256 KiB, thousands of sessions, one line each.
#[test]
fn hostchat_owner_sessions_large() {
    let rig = Rig::new(VERB, "owner-sessions-large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            let q = 1_790_000_100 + i;
            let end = match i % 3 {
                0 => String::new(),
                1 => format!(r#","end":{{"queuedAt":{q},"by":"login:ann"}},"delivered":true"#),
                _ => format!(
                    r#","end":{{"queuedAt":{q},"by":"owner","via":"all","outbox":"ob_{i}"}}"#
                ),
            };
            format!(
                r#"{{"sid":"own_{i:032x}","mintedAt":{},"mintedBy":"owner-token","redeemedAt":{}{end}}}"#,
                1_790_000_000 + i,
                1_790_000_010 + i
            )
        })
        .collect();
    let body = format!(r#"{{"sessions":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    let c = with(
        vec![("TZ", "UTC".to_string())],
        one("a large list", &["owner-sessions"], &body),
    );
    rig.check(&c);
    let (r, _) = rig.binary(&c, false);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert_eq!(r.stdout.lines().count(), 3000, "one line a session");
    rig.check(&with(
        vec![("TZ", "UTC".to_string())],
        one("a large list --json", &["owner-sessions", "--json"], &body),
    ));
}

const HELD_OWNER_SESSION: &str = r#"{"error":{"code":"confirm_required","hint":"in person","details":{"action":"chat-owner-session-end","secretId":"own_a"}}}"#;

/// `owner-sessions end` as a person: the daemon holds it for a confirm,
/// the vault gives the token (the vault password typed on stdin, the
/// house rig), the same body goes again with it.
#[test]
fn hostchat_owner_sessions_a_person_s_steps() {
    let rig = VRig::new();
    let post = "POST /cli/sew/owner-sessions/revoke HTTP/1.1";
    let mut rows: Vec<(VCase, Want)> = vec![
        (
            person(
                "end own_a: held, then confirmed",
                &["owner-sessions", "end", "own_a"],
                vec![
                    Reply::Answer(403, HELD_OWNER_SESSION.into()),
                    ok(
                        r#"{"queued":"own_a","ended":["own_a","own_b"],"takesEffect":"as soon as Sew receives it"}"#,
                    ),
                ],
                0,
            ),
            want(
                "end queued: own_a\nmarked: own_a, own_b\nas soon as Sew receives it\n",
                "Vault password: \n",
                &[
                    (post, r#"{"sid": "own_a"}"#),
                    (post, r#"{"sid": "own_a", "confirmToken": "ctok-5150"}"#),
                ],
            ),
        ),
        (
            person(
                "end --all --json: held, then confirmed",
                &["owner-sessions", "end", "--all", "--json"],
                vec![
                    Reply::Answer(403, HELD_OWNER_SESSION.replace("own_a", "all")),
                    ok(r#"{"queued":"all"}"#),
                ],
                0,
            ),
            want(
                "{\"queued\":\"all\"}\n",
                "Vault password: \n",
                &[
                    (post, r#"{"all": true}"#),
                    (post, r#"{"all": true, "confirmToken": "ctok-5150"}"#),
                ],
            ),
        ),
        (
            person(
                "owner-session revoke own_a: confirmed, then refused",
                &["owner-session", "revoke", "own_a"],
                vec![
                    Reply::Answer(403, HELD_OWNER_SESSION.into()),
                    Reply::Answer(
                        403,
                        r#"{"error":{"code":"confirm_required","hint":"wrong confirm"}}"#.into(),
                    ),
                ],
                3,
            ),
            want(
                "",
                "Vault password: \n{\"error\": \"confirm_required\", \"hint\": \"wrong confirm\"}\n",
                &[
                    (post, r#"{"sid": "own_a"}"#),
                    (post, r#"{"sid": "own_a", "confirmToken": "ctok-5150"}"#),
                ],
            ),
        ),
    ];
    let mut at_once = vcase(
        "end own_a: answered at once",
        &["owner-sessions", "end", "own_a"],
    );
    at_once.daemon = vec![ok(r#"{"queued":true,"ended":[]}"#)];
    at_once.daemon_requests = 1;
    rows.push((
        at_once,
        want("end queued: True\n\n", "", &[(post, r#"{"sid": "own_a"}"#)]),
    ));
    let mut nameless = vcase(
        "end own_a: held, but no action named: the refusal",
        &["owner-sessions", "end", "own_a"],
    );
    nameless.daemon = vec![Reply::Answer(
        403,
        r#"{"error":{"code":"confirm_required","hint":"in person"}}"#.into(),
    )];
    nameless.daemon_requests = 1;
    nameless.code = 3;
    rows.push((
        nameless,
        want(
            "",
            "{\"error\": \"confirm_required\", \"hint\": \"in person\"}\n",
            &[(post, r#"{"sid": "own_a"}"#)],
        ),
    ));
    let mut refused = vcase(
        "end --all: the vault refuses the confirm",
        &["owner-sessions", "end", "--all"],
    );
    refused.daemon = vec![Reply::Answer(
        403,
        HELD_OWNER_SESSION.replace("own_a", "all"),
    )];
    refused.daemon_requests = 1;
    refused.vault = Some(vec![Reply::Answer(
        403,
        r#"{"error":{"code":"denied","hint":"no"}}"#.into(),
    )]);
    refused.vault_requests = 1;
    refused.code = 3;
    rig.check(&refused);
    for (c, w) in &rows {
        rig.check(c);
        let r = rig.run(false, false, c);
        check_want(c.name, &r.out, &r.daemon, c.code, w);
        assert_eq!(
            r.vault.len(),
            c.vault_requests,
            "{}: vault requests",
            c.name
        );
        rig.no_secret_to_a_child(c);
    }
}

const APP_REC: &str = r#"{"app":{"name":"Acme","serviceId":"svc-1"},"spaces":[{"spaceId":5,"governance":"app","startLevel":"admin","required":true,"locked":true},{"spaceId":"s2"}]}"#;

#[test]
fn hostchat_apps_as_the_shell_does() {
    let rig = Rig::new(VERB, "apps");
    let utc = |c: Case| with(vec![("TZ", "UTC".to_string())], c);
    let v = vec![
        // covers: hostchat app-space
        one(
            "app-spaces",
            &["app-spaces"],
            r#"{"spaces":[{"appId":"acme","spaceId":"s1","members":{"added":3,"held":1},"governance":"app","governanceConfirmed":"sew"},{"appId":"b","spaceId":"s2","governance":"sew","governanceConfirmed":"sew"}],"unbound":[{"appId":"old","spaceId":"s0","doneAt":1},{"appId":"o2","spaceId":"s9"}],"note":"a note"}"#,
        ),
        one(
            "app-space list: none",
            &["app-space", "list"],
            r#"{"spaces":[]}"#,
        ),
        one(
            "app-space --json",
            &["app-space", "--json"],
            r#"{"spaces":[]}"#,
        ),
        one(
            "app-space bind",
            &["app-space", "bind", "acme", "s1"],
            r#"{"ok":true,"bound":true}"#,
        ),
        one(
            "app-space unbind --remove-app-members",
            &["app-space", "unbind", "acme", "s1", "--remove-app-members"],
            r#"{"ok":true}"#,
        ),
        with(
            with_passport(&[]),
            // covers: hostchat app-space governance
            case(
                "app-space governance in a session: no confirm",
                &["app-space", "governance", "acme", "s1", "app"],
                &[],
                vec![ok(
                    r#"{"spaceId":"s1","governance":"app","changed":true,"hint":"h"}"#,
                )],
                port_only,
                0,
                1,
            ),
        ),
        bad("app-space bind: one word", &["app-spaces", "bind", "acme"]),
        bad(
            "app-space bind --remove-app-members",
            &["app-space", "bind", "a", "s", "--remove-app-members"],
        ),
        bad(
            "app-space governance: no value",
            &["app-space", "governance", "a", "s"],
        ),
        bad(
            "app-space governance: a bad value",
            &["app-space", "governance", "a", "s", "both"],
        ),
        bad("app-space: an unknown flag", &["app-space", "bind", "--x"]),
        // covers: hostchat app-space recording
        one(
            "app-space recording: the rule",
            &["app-space", "recording", "acme", "5"],
            APP_REC,
        ),
        one(
            "app-space recording: another Space",
            &["app-space", "recording", "acme", "s2"],
            APP_REC,
        ),
        case(
            "app-space recording: not bound",
            &["app-space", "recording", "acme", "s9"],
            &[],
            vec![ok(APP_REC)],
            owner_and_port,
            3,
            1,
        ),
        one(
            "app-space recording --json",
            &["app-space", "recording", "acme", "5", "--json"],
            APP_REC,
        ),
        one(
            "app-space recording: a change",
            &[
                "app-space",
                "recording",
                "acme",
                "s1",
                "--start-level",
                "member",
                "--required=inherit",
            ],
            r#"{"spaceId":"s1","app":"acme","changed":true,"startLevel":"member","required":null,"locked":true}"#,
        ),
        one(
            "app-space recording: down, already so",
            &[
                "app-space",
                "recording",
                "acme",
                "s1",
                "--start-level=none",
                "--required",
                "off",
            ],
            r#"{"spaceId":"s1","app":"acme","required":false}"#,
        ),
        with(
            with_passport(&[]),
            case(
                "app-space recording in a session: held, refused",
                &["app-space", "recording", "acme", "s1", "--required", "on"],
                &[],
                vec![Reply::Answer(
                    403,
                    r#"{"error":{"code":"vault_confirm_required","hint":"in person"}}"#.into(),
                )],
                port_only,
                3,
                1,
            ),
        ),
        bad(
            "app-space recording: one word",
            &["app-space", "recording", "acme"],
        ),
        bad(
            "app-space recording: three words",
            &["app-space", "recording", "a", "s", "x"],
        ),
        bad(
            "app-space recording: a bad level",
            &["app-space", "recording", "a", "s", "--start-level", "owner"],
        ),
        bad(
            "app-space recording: a bad required",
            &["app-space", "recording", "a", "s", "--required", "maybe"],
        ),
        bad(
            "app-space recording: --required with nothing",
            &["app-space", "recording", "a", "s", "--required"],
        ),
        // covers: hostchat app-space recording-lock
        one(
            "app-space recording-lock",
            &["app-space", "recording-lock", "s1", "on"],
            r#"{"spaceId":"s1","locked":true,"changed":true,"hint":"nobody records there"}"#,
        ),
        one(
            "app-space recording-lock: already so",
            &["app-space", "recording-lock", "s1", "no"],
            r#"{"spaceId":"s1","locked":false}"#,
        ),
        bad(
            "app-space recording-lock: maybe",
            &["app-space", "recording-lock", "s1", "maybe"],
        ),
        one(
            "home",
            &["home"],
            r#"{"sew":{"appScopedSessions":false,"homeClient":true},"homeClient":"hearth","homeClientDelivered":false,"homeClientError":"no flag","apps":[{"app":"acme","name":"Acme","hiddenFromHome":true,"origin":"https://acme.example","delivered":true},{"app":"b"}]}"#,
        ),
        one(
            "home: not ok",
            &["home", "--json"],
            r#"{"ok":false,"error":"x"}"#,
        ),
        one(
            "home: not ok, the lines",
            &["home"],
            r#"{"ok":false,"error":"x"}"#,
        ),
        one(
            "home client",
            &["home", "client", "acme"],
            r#"{"sew":{"appScopedSessions":true},"homeClient":"acme","homeClientDelivered":false}"#,
        ),
        one(
            "home client none",
            &["home", "client", "none"],
            r#"{"sew":{},"homeClient":null}"#,
        ),
        one(
            "home app",
            &["home", "app", "acme", "--hidden", "on"],
            r#"{"sew":{},"apps":[]}"#,
        ),
        bad("home: a bad verb", &["home", "list"]),
        bad("home app: no --hidden", &["home", "app", "acme"]),
        bad("home client: no app", &["home", "client"]),
        case(
            "home app --hidden with nothing after: the shell's shift fails",
            &["home", "app", "acme", "--hidden"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: hostchat pass-keys
        utc(one(
            "pass-keys",
            &["pass-keys"],
            r#"{"keys":[{"state":"current","kid":"k2","publishedAt":1790000000,"rotateDueAt":1792000000},{"state":"retiring","kid":"k1","publishedAt":1780000000,"retireAfter":1791000000}]}"#,
        )),
        one("pass-keys: none", &["pass-keys"], r#"{"keys":[]}"#),
        one(
            "pass-keys rotate",
            &["pass-keys", "rotate"],
            r#"{"keys":[]}"#,
        ),
        one(
            "pass-keys drop-previous --json",
            &["pass-keys", "drop-previous", "--json"],
            r#"{"ok":true}"#,
        ),
        bad("pass-keys: a word", &["pass-keys", "list"]),
        bad("pass-keys: a flag", &["pass-keys", "--x"]),
        // covers: hostchat meet
        one(
            "meet bind",
            &["meet", "bind", "acme"],
            r#"{"ok":true,"app":"acme","url":"https://meet.example"}"#,
        ),
        one("meet unbind", &["meet", "unbind"], r#"{"ok":true}"#),
        case(
            "meet alone: the help",
            &["meet"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        bad("meet bind: no app", &["meet", "bind"]),
        bad("meet: a bad verb", &["meet", "start"]),
        tb(
            "pass-keys: a time that is no number",
            &["pass-keys"],
            r#"{"keys":[{"publishedAt":"x"}]}"#,
        ),
        tb("home: a list", &["home"], "[]"),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn hostchat_apps_a_person_s_steps() {
    let rig = VRig::new();
    let held = || {
        Reply::Answer(
            403,
            r#"{"error":{"code":"vault_confirm_required","hint":"in person"}}"#.into(),
        )
    };
    let mut gov = person(
        "app-space governance",
        &["app-space", "governance", "acme", "s1", "sew"],
        vec![ok(r#"{"spaceId":"s1","governance":"sew","changed":false}"#)],
        0,
    );
    gov.daemon_requests = 1;
    let v = vec![
        gov,
        person(
            "app-space recording: up, held for a confirm",
            &[
                "app-space",
                "recording",
                "acme",
                "s1",
                "--start-level",
                "admin",
            ],
            vec![
                held(),
                ok(APP_REC),
                ok(r#"{"spaceId":"s1","app":"acme","changed":true,"startLevel":"admin"}"#),
            ],
            0,
        ),
        person(
            "app-space recording-lock: held for a confirm",
            &["app-space", "recording-lock", "s1", "on", "--json"],
            vec![
                held(),
                ok(r#"{"spaceId":"s1","locked":true,"changed":true}"#),
            ],
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
    let mut none = vcase(
        "app-space recording: held, no App by that name",
        &["app-space", "recording", "nope", "s1", "--required", "on"],
    );
    none.daemon = vec![held(), ok(r#"{"app":null}"#)];
    none.daemon_requests = 2;
    none.code = 3;
    rig.check(&none);
}

/// `media`, `people` and `approvals` with one help flag after them
/// (`text::FAMILY_FORMS`): the flag is the sub-command, as the shell's
/// parsers read it: the unknown-command line or the usage, exit 2,
/// nothing sent. A help line a gate stops is the shell's (it skips its
/// connection gate for a help line).
#[test]
fn hostchat_help_as_a_sub_command() {
    let rig = Rig::new(VERB, "helpsub");
    for c in [
        // native: hostchat media --help
        bad("media --help: an unknown media command", &["media", "--help"]),
        bad("media -h: an unknown media command", &["media", "-h"]),
        // native: hostchat people --help
        bad("people --help: the usage", &["people", "--help"]),
        bad("people -h: the usage", &["people", "-h"]),
        // native: hostchat approvals --help
        bad("approvals --help: the usage", &["approvals", "--help"]),
        bad("approvals -h: the usage", &["approvals", "-h"]),
        with(
            with_passport(&[]),
            case(
                "people --help, an agent over TCP",
                &["people", "--help"],
                &[],
                vec![],
                nothing,
                2,
                0,
            ),
        ),
        Case {
            compare: Compare::Shell,
            ..case(
                "media --help, no port: the shell skips its gate",
                &["media", "--help"],
                &[],
                vec![],
                owner_only,
                2,
                0,
            )
        },
    ] {
        rig.check(&c);
    }
}
