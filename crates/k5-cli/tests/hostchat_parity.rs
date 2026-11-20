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
//!   --accept-data-loss and open --print; each refuses in a session.
//! - The refusals, Python's raise, the usage lines, help and the gate, a
//!   passport and the cell socket, one large list, and the hand-backs.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};

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

fn one(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn many(name: &'static str, args: &'static [&'static str], bodies: &[&str], code: i32) -> Case {
    let replies: Vec<Reply> = bodies.iter().map(|b| ok(b)).collect();
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, code, n)
}

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
        one("status: a pin, a folder, no mirror", &["status", "--health"], STATUS2),
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
        one("status: not supported", &["status"], r#"{"supported":false}"#),
        one(
            "status: the box backup with no line",
            &["status"],
            r#"{"state":"running","backup":{"ship":{"kind":"box-backup"}},"mirror":{"kind":"dir","path":"/m"}}"#,
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
        one("job", &["job", "j 1"], r#"{"id":"j 1","state":"done","n":1.5,"t":true}"#),
        one("doctor", &["doctor"], r#"{"ok":false,"checks":{"a":1}}"#),
        one("reconfigure", &["reconfigure"], r#"{"ok":true,"state":"running"}"#),
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
            vec![ok(r#"{"ok":false,"code":"media_rollback_failed","error":"no","mediaCommand":"sudo y"}"#)],
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
        one("ship none", &["ship", "none"], r#"{"ship":{"kind":"none"},"mirror":{}}"#),
        // covers: hostchat mirror
        one("mirror none", &["mirror", "none"], r#"{"ok":true,"mirror":{"kind":"none"}}"#),
        one("mirror dir", &["mirror", "dir", "/mnt/m", "--json"], r#"{"ok":true}"#),
        // covers: hostchat app-url
        one("app-url", &["app-url"], STATUS),
        one("app-url show: none", &["app-url", "show"], r#"{"state":"x"}"#),
        one("app-url --json", &["app-url", "--json"], STATUS),
        one(
            "app-url set",
            &["app-url", "set", "https://app.example/x"],
            r#"{"ok":true,"appUrl":"https://app.example/x"}"#,
        ),
        one("app-url clear", &["app-url", "clear"], r#"{"ok":true,"appUrl":null}"#),
        // covers: hostchat config
        one(
            "config: the list",
            &["config"],
            r#"{"approvalsBy":"people","allowed":["people","people+agents"],"defaultSpace":null}"#,
        ),
        one("config: the list, none allowed", &["config"], r#"{"defaultSpace":"ops"}"#),
        one("config --json", &["config", "--json"], r#"{"approvalsBy":"people"}"#),
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
        traceback(one("status: not JSON", &["status"], "<html>")),
        traceback(one("status: a list", &["status"], "[1]")),
        traceback(one(
            "status: a mirror path that is no string",
            &["status"],
            r#"{"state":"x","mirror":{"kind":"dir","path":3}}"#,
        )),
        traceback(one("backups: a list", &["backups"], "[]")),
        traceback(one(
            "config: not JSON",
            &["config"],
            "nope",
        )),
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
        one("enable --no-wait", &["enable", "--no-wait"], r#"{"state":"installing","ok":true}"#),
        one("enable: done at once", &["enable"], r#"{"state":"running"}"#),
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
        one("upgrade --no-wait", &["upgrade", "--no-wait"], r#"{"state":"upgrading"}"#),
        one("upgrade: refused at once", &["upgrade"], r#"{"state":"running","refused":"x"}"#),
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
            vec![Reply::Answer(400, r#"{"error":{"code":"bad_id","hint":"no such job"}}"#.into())],
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
            vec![Reply::Answer(409, r#"{"error":{"code":"sew_down","hint":"h","details":{"key":"k"}}}"#.into())],
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
        case("status with no port: the gate", &["status"], &[], vec![], owner_only, 1, 0),
        bad("an unknown verb", &["nope"]),
        bad("an unknown flag, as it is", &["status", "--x\"y"]),
        bad("--setup-url", &["config", "--setup-url", "x", "--bogus"]),
        bad("--identity", &["enable", "--identity", "ldap"]),
        bad("--voice on status", &["status", "--voice"]),
        bad("--node-ip on upgrade", &["upgrade", "--node-ip", "192.0.2.1"]),
        bad("--media-tarball on status", &["status", "--media-tarball", "m"]),
        bad("--accept-data-loss on upgrade", &["upgrade", "--accept-data-loss"]),
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
        bad("config set approvals.by: a bad value", &["config", "set", "approvals.by", "all"]),
        bad("config set default_space: none given", &["config", "set", "default_space"]),
        bad("config set server.public_url: none given", &["config", "set", "public_url"]),
        bad("config: a word", &["config", "x"]),
        bad("purge: no hostname", &["purge"]),
        bad("upgrade --accept-binary: not hex", &["upgrade", "--accept-binary", "xyz"]),
        bad(
            "upgrade --accept-binary: too short",
            &["upgrade", "--accept-binary", "ABCDEF"],
        ),
        bad("deliveries set: nothing to set", &["deliveries", "set", "w"]),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["status"],
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
        with(with_passport(&[]), one("a host session: TCP", &["status"], STATUS2)),
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
        .map(|i| format!(r#"{{"id":"bundle-{i:05}","at":{},"size":{},"kind":"nightly"}}"#, 1_790_000_000 + i, 1000 + i))
        .collect();
    let body = format!(r#"{{"backups":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&one("a large list", &["backups"], &body));
}

/// What the binary hands to the shell.
#[test]
fn hostchat_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    for c in [
        shell(case(
            "_CP_404_EXIT set: the shell reads it",
            &["status"],
            &[("_CP_404_EXIT", "4")],
            vec![ok(r#"{"state":"x"}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "open: a browser with the owner link (waits for a ruling)",
            &["open"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        )),
    ] {
        rig.check(&c);
    }
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell. Where bash acts anyway (a value flag takes
/// `-h`/`--help` as its value, so the loop never sees it: a bash bug,
/// cli/src/hostchat-voice.sh:463-480), the rows copy bash and say so.
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
        shell("upgrade -h", &["upgrade", "--tarball", "t", "-h"], vec![], 0),
        shell("rollback --help", &["rollback", "--help"], vec![], 0),
        shell("restore -h", &["restore", "b1", "--confirm", "-h"], vec![], 0),
        shell("purge -h", &["purge", "--confirm-hostname", "h", "-h"], vec![], 0),
        shell("recovery-key --help", &["recovery-key", "--help"], vec![], 0),
        shell("rotate-master -h", &["rotate-master", "--drop-prev", "-h"], vec![], 0),
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
fn person(
    name: &'static str,
    args: &[&'static str],
    daemon: Vec<Reply>,
    code: i32,
) -> VCase {
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
            vec![ok(INSTALL), ok(r#"{"recoveryKey":"rk-1","warning":"keep it safe"}"#)],
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
            &["purge", "--confirm-hostname", "box.example", "--keep-backups"],
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
                ok(r#"{"result":"rolled_back","message":"rolled back","keptBackup":"k1","bundle":"b7","version":"0.5.0","doctorFails":["a","b"]}"#),
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
            vec![ok(r#"{"appUrl":"https://chat.example"}"#), ok(r#"{"code":"x"}"#)],
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
    let mut refused = vcase("purge: the confirm refused", &["purge", "--confirm-hostname", "h"]);
    refused.daemon = vec![ok(INSTALL)];
    refused.daemon_requests = 1;
    refused.vault = Some(vec![Reply::Answer(
        403,
        r#"{"error":{"code":"denied","hint":"no"}}"#.into(),
    )]);
    refused.vault_requests = 1;
    refused.code = 3;
    v.push(refused);
    let mut tb = vcase("recovery-key: an install id that is no JSON", &["recovery-key"]);
    tb.daemon = vec![ok("nope")];
    tb.daemon_requests = 1;
    tb.code = 1;
    tb.compare = VCompare::Traceback;
    v.push(tb);
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
        one("recording status --json", &["recording", "status", "--json"], REC),
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
        case("recording alone: its help", &["recording"], &[], vec![], owner_and_port, 0, 0),
        case("recording help", &["recording", "help"], &[], vec![], owner_and_port, 0, 0),
        bad("recording: --yes on status", &["recording", "status", "--yes"]),
        bad("recording: --model-file on status", &["recording", "status", "--model-file", "m"]),
        bad("recording: --model-file with no file", &["recording", "install", "--model-file"]),
        bad("recording: an unknown flag", &["recording", "status", "--x"]),
        bad("recording: a word", &["recording", "status", "now"]),
        bad("recording: an unknown verb", &["recording", "start"]),
        // covers: hostchat recording policy
        utc(one("recording policy", &["recording", "policy"], POLICIES)),
        one("recording policies --json", &["recording", "policies", "--json"], POLICIES),
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
            vec![Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"the owner"}}"#.into())],
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
                vec![Reply::Answer(403, r#"{"error":{"code":"vault_confirm_required","hint":"in person"}}"#.into())],
                port_only,
                3,
                1,
            ),
        ),
        bad("recording policy: a word", &["recording", "policy", "x"]),
        bad("recording policy: an unknown flag", &["recording", "policy", "--x"]),
        bad("recording policy set: nothing", &["recording", "policy", "set"]),
        bad("recording policy set: odd", &["recording", "policy", "set", "enabled"]),
        bad("recording policy set: unknown key", &["recording", "policy", "set", "color", "é"]),
        bad("recording policy set: not on or off", &["recording", "policy", "set", "enabled", "maybe"]),
        bad("recording policy set: required", &["recording", "policy", "set", "required", "all"]),
        bad("recording policy set: days 0", &["recording", "policy", "set", "audio_max_days", "0"]),
        bad("recording policy set: days 3651", &["recording", "policy", "set", "audio_max_days", "3651"]),
        bad("recording policy set: days x", &["recording", "policy", "set", "audio_max_days", "d"]),
        traceback(one("recording status: a list", &["recording", "status"], "[]")),
        traceback(one(
            "recording policy: a lock time that is no number",
            &["recording", "policy"],
            r#"{"locks":[{"lockedAt":"x"}]}"#,
        )),
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
                Reply::Answer(403, r#"{"error":{"code":"owner_confirm_required","hint":"in person"}}"#.into()),
                ok(INSTALL),
                ok(r#"{"changed":["recording.required"]}"#),
            ],
            0,
        ),
    ];
    let mut tty = vcase("recording uninstall: no terminal", &["recording", "uninstall"]);
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
        one("media status --json", &["media", "status", "--json"], STATUS),
        one("media status --json: no voice", &["media", "status", "--json"], r#"{"state":"x"}"#),
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
        one("media url", &["media", "url", "wss://v.example"], r#"{"ok":true,"publicUrl":"wss://v.example"}"#),
        one("media url --url", &["media", "url", "--public-url", "wss://w.example"], r#"{"ok":true}"#),
        one("media renew", &["media", "renew"], r#"{"renewed":true,"daysLeft":89}"#),
        one("media rebind", &["media", "rebind"], r#"{"rebound":true}"#),
        one("media tier", &["media", "tier", "tunnel"], r#"{"tier":"tunnel"}"#),
        one(
            "media probe",
            &["media", "probe", "--udp", "ok", "--tcp", "fail", "--turn-tls", "ok"],
            r#"{"exposure":{"note":"tcp blocked"}}"#,
        ),
        one("media uninstall --yes", &["media", "uninstall", "--yes"], r#"{"removed":true}"#),
        case(
            "media uninstall with no terminal",
            &["media", "uninstall"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("media alone: the help", &["media"], &[], vec![], owner_and_port, 0, 0),
        bad("media url: none", &["media", "url"]),
        bad("media tier: a bad tier", &["media", "tier", "fast"]),
        bad("media probe: a flag missing", &["media", "probe", "--udp", "ok"]),
        bad("media: --yes on renew", &["media", "renew", "-y"]),
        bad("media: an unknown flag", &["media", "renew", "--x"]),
        bad("media: an unknown verb", &["media", "start"]),
        traceback(one("media status --json: a list", &["media", "status", "--json"], "[]")),
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
        one("user list --json", &["user", "list", "--json"], r#"{"users":[]}"#),
        one("users owner", &["users", "owner", "ann", "--display-name", "Ann"], link),
        one(
            "users invite",
            &["users", "invite", "bob", "--email", "bob@example.test", "--admin"],
            link,
        ),
        one("users invite: a member", &["users", "invite", "cy"], r#"{"username":"cy","installRole":"member"}"#),
        one("users reset", &["users", "reset", "ann"], link),
        one(
            "users revoke",
            &["users", "revoke", "ann"],
            r#"{"username":"ann","sessionsRevoked":0}"#,
        ),
        one("users reactivate --json", &["users", "reactivate", "ann", "--json"], link),
        one("users role", &["users", "role", "ann", "member"], r#"{"username":"ann","installRole":"member"}"#),
        one("users role --role", &["users", "role", "ann", "--role", "admin"], r#"{"username":"ann"}"#),
        bad("users owner: no name", &["users", "owner"]),
        bad("users role: no role", &["users", "role", "ann"]),
        bad("users: an unknown verb", &["users", "kick", "ann"]),
        bad("users: an unknown flag", &["users", "list", "--x"]),
        case("users help", &["users", "help"], &[], vec![], owner_and_port, 0, 0),
        traceback(one("users list: a list", &["users"], "[1]")),
    ];
    for c in v {
        rig.check(&c);
    }
}
