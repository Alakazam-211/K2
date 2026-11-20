//! `k5-cli migrate` against the shell CLI's `cli/k2 migrate`
//! (`families::migrate`), on the vault rig ([`common::vault`]): the help,
//! every usage line, each step over the vault socket (the person's
//! channel) and over the daemon in a K2 terminal, the vault confirm and
//! its refusals, a held step, each layout, a body Python can't read, and
//! one large answer. The password typed for a confirm reaches no child.

#![cfg(unix)]

mod common;

use common::parity::PASSPORT;
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

fn case(name: &'static str, args: &[&'static str], code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = "migrate";
    c.code = code;
    c
}

/// The vault socket answers.
fn sock(name: &'static str, args: &[&'static str], replies: Vec<Reply>, code: i32) -> VCase {
    let mut c = case(name, args, code);
    c.vault_requests = replies.len();
    c.vault = Some(replies);
    c
}

/// In a K2 terminal: the daemon's cell socket answers, with the passport.
fn session(name: &'static str, args: &[&'static str], replies: Vec<Reply>, code: i32) -> VCase {
    let mut c = case(name, args, code);
    c.env = vec![
        ("K2_HOOK_SOCK", "{sock}".to_string()),
        ("K2_HOOK_TOKEN", PASSPORT.to_string()),
    ];
    c.daemon_requests = replies.len();
    c.daemon = replies;
    c
}

fn traceback(mut c: VCase) -> VCase {
    c.compare = VCompare::Traceback;
    c
}

const STATUS: &str = r#"{"vault":{"open":true,"vaultId":"v1","keyHome":"keychain"},"frozen":{"at":"t1","moveKey":"fp9"},"moveKey":{"recipient":"age1abc","fingerprint":"fp9","expiresAt":"t2"},"lastImport":{"vaultId":"v0","at":"t0"},"held":[{"id":"mv_1","step":"export","status":"pending","by":"agent é","expiresAt":"t3"},{"id":"mv_2","step":"undo","status":"done-long-word","by":null,"expiresAt":"t4"}]}"#;

const CLOSED: &str = r#"{"vault":{"open":false,"error":{"code":"vault_key_missing","hint":"no key"}},"frozen":null,"moveKey":null,"lastImport":null,"held":[]}"#;

const BEGIN: &str =
    r#"{"created":true,"moveKey":{"recipient":"age1new","fingerprint":"fpN","expiresAt":"t5"}}"#;

const EXPORT: &str = r#"{"out":"/tmp/m.k2mv","bytes":2048,"moveKey":"fpN","vaultId":"v1","gen":3,"secrets":{"user":4,"system":2},"next":"copy the file"}"#;

const DRY: &str = r#"{"vaultId":"v1","gen":3,"secrets":{"user":4,"system":2},"source":{"host":"old-box","tunnelSubdomain":"acme"},"sourceKeyHome":"tpm","exportedAt":"t6","tunnelToken":true,"systemdCreds":[{"secret":"DB","unit":"db.service"},{"secret":"API","unit":"web.service"}],"replaces":{"open":true,"vaultId":"v0","userSecrets":1},"next":"run it with --apply"}"#;

const DRY_CLOSED: &str = r#"{"vaultId":"v1","gen":3,"secrets":{},"source":{},"sourceKeyHome":"tpm","exportedAt":"t6","tunnelToken":false,"replaces":{"open":false,"error":"vault_key_missing"},"next":""}"#;

const IMPORT: &str = r#"{"vaultId":"v1","gen":4,"keyHome":"keychain","rollback":true,"secrets":{"user":4,"system":2},"tunnelToken":true,"systemdCreds":[{"secret":"DB","unit":"db.service"}],"replaced":"/r/replaced-1","next":["k2 vault status","re-place the creds"]}"#;

const CONFIRM_403: &str = r#"{"error":{"code":"vault_confirm_required","hint":"confirm it"},"confirm":{"secretId":"mv-bind-1","label":"the import","action":"vault-move"}}"#;

const EXPORT_403: &str = r#"{"error":{"code":"vault_presence_required","hint":"a person"},"confirm":{"secretId":"mv-bind-2","label":"the export","action":"box-move-export"}}"#;

const PASSWORD: &str = "pw-migrate-5151";
const TOKEN: &str = "ctok-migrate-6161";

fn methods(m: &str) -> Reply {
    ok(&format!(r#"{{"methods":["{m}"]}}"#))
}

#[test]
fn help_and_usage() {
    let rig = VRig::new();
    for c in [
        // covers: migrate
        case("help", &[], 0),
        case("help word", &["help"], 0),
        case("--help", &["--help"], 0),
        case("-h", &["-h"], 0),
        case("an unknown verb", &["nope"], 2),
        case("a flag for a verb", &["--json"], 2),
        case("approve with no id", &["approve"], 2),
        case("approve --json alone", &["approve", "--json"], 2),
        case("export with nothing", &["export-secrets"], 2),
        case("export, --to only", &["export-secrets", "--to", "age1x"], 2),
        case(
            "export, an empty --out",
            &["export-secrets", "--to", "age1x", "--out", ""],
            2,
        ),
        case(
            "export, a word it doesn't take",
            &["export-secrets", "x"],
            2,
        ),
        // The shell's `shift 2` fails under `set -e`: exit 1, nothing
        // printed.
        case("export, --to last", &["export-secrets", "--to"], 1),
        case("export, -o last", &["export-secrets", "--to", "a", "-o"], 1),
        case("import with nothing", &["import-secrets"], 2),
        case("import, a flag", &["import-secrets", "--nope"], 2),
        case("import, --apply alone", &["import-secrets", "--apply"], 2),
    ] {
        rig.check(&c);
    }
    // A line with `-h` or `--help` is the front's (a help page, or the
    // shell's): the binary hands it on.
    for (name, args, code) in [
        ("approve -h", &["approve", "-h"][..], 0),
        ("approve --json --help", &["approve", "--json", "--help"], 0),
        ("approve, a flag before -h", &["approve", "--x", "-h"], 2),
    ] {
        let mut c = case(name, args, code);
        c.handed = true;
        rig.check(&c);
    }
}

/// Help never acts (K2's rule): `-h` or `--help` on the line of a step
/// that changes something prints the help and sends nothing. The binary
/// hands these lines on (the front's rule for a family's help).
#[test]
fn help_never_acts() {
    let rig = VRig::new();
    for args in [
        &["export-secrets", "--to", "age1x", "--out", "f", "--help"][..],
        &["export-secrets", "-h", "--to", "age1x", "--out", "f"],
        &["export-secrets", "--unfreeze", "-h"],
        &["import-secrets", "f.k2mv", "--apply", "--help"],
        &["import-secrets", "--undo", "-h"],
        &["import-secrets", "begin", "--new", "--help"],
        &["approve", "mv_1", "-h"],
        &["approve", "mv_1", "--json", "--help"],
    ] {
        let mut c = case("help", args, 0);
        c.handed = true;
        rig.check(&c);
    }
}

/// A bash bug, reported: `--to` and `--out` take `-h` as their value, so
/// `k2 migrate export-secrets --to -h --out f` sends the export
/// (cli/src/migrate.sh, cmd_migrate's export-secrets arm). The binary
/// hands the line to the shell (the front's rule for `-h`), so both send
/// it. FLIP: when bash's fix lands this fails; then the line prints the
/// help and sends nothing, and this becomes a help_never_acts row.
#[test]
fn export_to_takes_help_as_its_value_bash_bug() {
    let rig = VRig::new();
    let c = sock(
        "export, -h as --to's value (bash bug)",
        &["export-secrets", "--to", "-h", "--out", "f"],
        vec![ok(EXPORT)],
        0,
    );
    let bash = rig.run(true, false, &c);
    assert_eq!(bash.out.code, Some(0), "{bash:#?}");
    assert_eq!(
        bash.vault.len(),
        1,
        "FLIP: bash no longer sends it: {bash:#?}"
    );
    assert!(bash.vault[0].contains("\"to\": \"-h\""), "{bash:#?}");
}

#[test]
fn status() {
    let rig = VRig::new();
    // covers: migrate status
    let mut big = String::from(r#"{"vault":{"open":true},"held":["#);
    for i in 0..4000 {
        if i > 0 {
            big.push(',');
        }
        big.push_str(&format!(
            r#"{{"id":"mv_{i:05}","step":"export","status":"pending","by":"agent-{i}","expiresAt":"2026-11-20T00:00:00Z"}}"#
        ));
    }
    big.push_str("]}");
    assert!(big.len() > 256 * 1024, "the large answer is large");
    let big: &'static str = Box::leak(big.into_boxed_str());
    for c in [
        sock("status", &["status"], vec![ok(STATUS)], 0),
        sock("status, closed", &["status"], vec![ok(CLOSED)], 0),
        sock("status, nothing in it", &["status"], vec![ok("{}")], 0),
        sock("status, a large answer", &["status"], vec![ok(big)], 0),
        // covers: migrate status --json
        sock("status --json", &["status", "--json"], vec![ok(STATUS)], 0),
        sock(
            "status --json, large",
            &["status", "--json"],
            vec![ok(big)],
            0,
        ),
        // Only the first word after the verb is read.
        sock(
            "status x --json",
            &["status", "x", "--json"],
            vec![ok(STATUS)],
            0,
        ),
        session("status in a K2 terminal", &["status"], vec![ok(STATUS)], 0),
        session(
            "status --json in a K2 terminal",
            &["status", "--json"],
            vec![ok(STATUS)],
            0,
        ),
        sock(
            "status refused",
            &["status"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"no"}}"#.into(),
            )],
            3,
        ),
        sock(
            "status refused --json",
            &["status", "--json"],
            vec![Reply::Answer(403, r#"{"error":"no"}"#.into())],
            3,
        ),
        sock(
            "status not found",
            &["status"],
            vec![Reply::Answer(404, "not json".into())],
            4,
        ),
        sock("status, no answer", &["status"], vec![Reply::Close], 1),
        sock(
            "status --json, no answer",
            &["status", "--json"],
            vec![Reply::Close],
            1,
        ),
        traceback(sock("status, not JSON", &["status"], vec![ok("nope")], 1)),
        traceback(sock(
            "status, a held row that is no object",
            &["status"],
            vec![ok(r#"{"held":["x"]}"#)],
            1,
        )),
        traceback(sock(
            "status, an error that is a word",
            &["status"],
            vec![ok(r#"{"vault":{"open":false,"error":"x"}}"#)],
            1,
        )),
    ] {
        rig.check(&c);
    }
    // No vault socket: the daemon, over TCP.
    let mut c = case("status, no vault socket", &["status"], 0);
    c.vault = None;
    c.daemon = vec![ok(STATUS)];
    c.daemon_requests = 1;
    rig.check(&c);
    // A session with no passport: the id tool's refusal.
    let mut c = case("a session without a passport", &["status"], 3);
    c.env = vec![("K2_HOOK_SOCK", "{sock}".to_string())];
    rig.check(&c);
    // No port, no token: the gate; the help still prints.
    let mut c = case("no port, no token", &["status"], 1);
    c.owner_on_disk = false;
    rig.check(&c);
    let mut c = case("help with no port", &["import-secrets", "-h"], 0);
    c.owner_on_disk = false;
    c.handed = true;
    rig.check(&c);
}

#[test]
fn begin_and_the_dry_run() {
    let rig = VRig::new();
    for c in [
        // covers: migrate import-secrets begin
        sock("begin", &["import-secrets", "begin"], vec![ok(BEGIN)], 0),
        sock(
            "begin again",
            &["import-secrets", "begin", "--new"],
            vec![ok(r#"{"moveKey":{"recipient":"age1old"}}"#)],
            0,
        ),
        // covers: migrate import-secrets begin --json
        sock(
            "begin --json",
            &["import-secrets", "begin", "--json"],
            vec![ok(BEGIN)],
            0,
        ),
        session(
            "begin in a K2 terminal",
            &["import-secrets", "begin"],
            vec![ok(BEGIN)],
            0,
        ),
        sock(
            "begin held",
            &["import-secrets", "begin"],
            vec![Reply::Answer(
                202,
                r#"{"status":"held","hint":"ask the owner"}"#.into(),
            )],
            2,
        ),
        traceback(sock(
            "begin, a key that is a word",
            &["import-secrets", "begin"],
            vec![ok(r#"{"moveKey":"x"}"#)],
            1,
        )),
        // covers: migrate import-secrets
        sock("dry run", &["import-secrets", "m.k2mv"], vec![ok(DRY)], 0),
        sock(
            "dry run, an absolute file",
            &["import-secrets", "/abs/m.k2mv"],
            vec![ok(DRY_CLOSED)],
            0,
        ),
        // covers: migrate import-secrets --json
        sock(
            "dry run --json",
            &["import-secrets", "m.k2mv", "--json"],
            vec![ok(DRY)],
            0,
        ),
        sock(
            "dry run refused",
            &["import-secrets", "m.k2mv"],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_file","hint":"not a move file"}}"#.into(),
            )],
            2,
        ),
        traceback(sock(
            "dry run, a next that is a number",
            &["import-secrets", "m.k2mv"],
            vec![ok(r#"{"next":5}"#)],
            1,
        )),
    ] {
        rig.check(&c);
    }
}

#[test]
fn the_steps_and_their_confirm() {
    let rig = VRig::new();
    let mut apply = sock(
        "import --apply: the confirm, the vault password",
        &["import-secrets", "m.k2mv", "--apply"],
        vec![
            Reply::Answer(403, CONFIRM_403.into()),
            methods("vault-password"),
            ok(&format!(r#"{{"token":"{TOKEN}"}}"#)),
            ok(IMPORT),
        ],
        0,
    );
    apply.stdin = format!("{PASSWORD}\n").into_bytes();
    apply.secrets = vec![PASSWORD, TOKEN];
    let mut export = sock(
        "export: the box-move-export confirm",
        &["export-secrets", "--to", "age1new", "--out", "move.k2mv"],
        vec![
            Reply::Answer(403, EXPORT_403.into()),
            ok(r#"{"methods":["terminal"],"confirmText":"export the vault"}"#),
            ok(&format!(r#"{{"token":"{TOKEN}"}}"#)),
            ok(EXPORT),
        ],
        0,
    );
    export.secrets = vec![TOKEN];
    let mut export_json = sock(
        "export --json",
        &["export-secrets", "--json", "-o", "/m", "--to", "age1new"],
        vec![
            Reply::Answer(403, EXPORT_403.into()),
            methods("terminal"),
            ok(&format!(r#"{{"token":"{TOKEN}"}}"#)),
            ok(EXPORT),
        ],
        0,
    );
    export_json.secrets = vec![TOKEN];
    for c in [
        apply,
        // covers: migrate export-secrets
        export,
        // covers: migrate export-secrets --json
        export_json,
        sock(
            "import --apply --json: done at once",
            &["import-secrets", "m.k2mv", "--apply", "--json"],
            vec![ok(IMPORT)],
            0,
        ),
        sock(
            "the confirm refused",
            &["import-secrets", "m.k2mv", "--apply"],
            vec![
                Reply::Answer(403, CONFIRM_403.into()),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"a person"}}"#.into(),
                ),
            ],
            3,
        ),
        sock(
            "no confirm method",
            &["import-secrets", "m.k2mv", "--apply"],
            vec![Reply::Answer(403, CONFIRM_403.into()), methods("none")],
            3,
        ),
        sock(
            "a 403 with no binding",
            &["import-secrets", "m.k2mv", "--apply"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"vault_confirm_required","hint":"h"},"confirm":{}}"#.into(),
            )],
            3,
        ),
        traceback(sock(
            "a 403 that names the confirm and is no JSON",
            &["import-secrets", "m.k2mv", "--apply"],
            vec![Reply::Answer(403, r#"x "vault_confirm_required""#.into())],
            1,
        )),
        sock(
            "a 403 the dry run never confirms",
            &["import-secrets", "m.k2mv"],
            vec![Reply::Answer(403, CONFIRM_403.into())],
            3,
        ),
        sock(
            "export held",
            &["export-secrets", "--to", "age1new", "--out", "/m"],
            vec![Reply::Answer(
                202,
                r#"{"status":"held","id":"mv_9","hint":"k2 migrate approve mv_9"}"#.into(),
            )],
            2,
        ),
        sock(
            "export held --json",
            &["export-secrets", "--to", "age1new", "--out", "/m", "--json"],
            vec![Reply::Answer(
                202,
                r#"{"status":"held","id":"mv_9"}"#.into(),
            )],
            2,
        ),
        traceback(sock(
            "held, a hint that is a number",
            &["export-secrets", "--to", "age1new", "--out", "/m"],
            vec![Reply::Answer(202, r#"{"hint":7}"#.into())],
            1,
        )),
        session(
            "export in a K2 terminal: held",
            &["export-secrets", "--to", "age1new", "--out", "/m"],
            vec![Reply::Answer(
                202,
                r#"{"status":"held","hint":"ask the owner"}"#.into(),
            )],
            2,
        ),
        sock(
            "unfreeze",
            &["export-secrets", "--unfreeze"],
            vec![ok(r#"{"wasFrozen":true}"#)],
            0,
        ),
        sock(
            "unfreeze, not frozen",
            &["export-secrets", "--unfreeze", "--json", "--to", "x"],
            vec![ok(r#"{"wasFrozen":false}"#)],
            0,
        ),
        sock(
            "undo",
            &["import-secrets", "--undo"],
            vec![ok(
                r#"{"undone":"v1","vault":{"open":false,"error":"vault_key_missing"},"note":"the old vault is back"}"#,
            )],
            0,
        ),
        sock(
            "undo, open",
            &["import-secrets", "--undo", "f"],
            vec![ok(r#"{"undone":"v1","vault":{"open":true}}"#)],
            0,
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn approve() {
    let rig = VRig::new();
    let mut confirmed = sock(
        "approve: confirmed, then the export it held",
        &["approve", "mv_1"],
        vec![
            Reply::Answer(403, CONFIRM_403.into()),
            methods("vault-password"),
            ok(&format!(r#"{{"token":"{TOKEN}"}}"#)),
            ok(EXPORT),
        ],
        0,
    );
    confirmed.stdin = format!("{PASSWORD}\n").into_bytes();
    confirmed.secrets = vec![PASSWORD, TOKEN];
    for c in [
        // covers: migrate approve
        confirmed,
        sock(
            "approve: an unfreeze",
            &["approve", "mv_2"],
            vec![ok(r#"{"wasFrozen":true}"#)],
            0,
        ),
        sock(
            "approve: an undo",
            &["approve", "mv_3"],
            vec![ok(r#"{"undone":"v1","vault":{}}"#)],
            0,
        ),
        sock(
            "approve: an import",
            &["approve", "mv_4"],
            vec![ok(IMPORT)],
            0,
        ),
        sock(
            "approve: something else",
            &["approve", "mv_5"],
            vec![ok(r#"{"ran":"x","n":[1,2.5,"é"],"o":{}}"#)],
            0,
        ),
        sock(
            "approve: the last id counts",
            &["approve", "mv_a", "mv_b"],
            vec![ok(r#"{"ran":"b"}"#)],
            0,
        ),
        // covers: migrate approve --json
        sock(
            "approve --json",
            &["approve", "--json", "mv_1"],
            vec![ok(EXPORT)],
            0,
        ),
        sock(
            "approve: not found",
            &["approve", "mv_x"],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such step"}}"#.into(),
            )],
            4,
        ),
        sock(
            "approve: expired",
            &["approve", "mv_x", "--json"],
            vec![Reply::Answer(410, r#"{"error":"expired"}"#.into())],
            5,
        ),
        traceback(sock(
            "approve: an answer that is a word",
            &["approve", "mv_x"],
            vec![ok(r#""out""#)],
            1,
        )),
        traceback(sock(
            "approve: an answer that is a number",
            &["approve", "mv_x"],
            vec![ok("5")],
            1,
        )),
    ] {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = VRig::new();
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    for (args, code, out, err) in [
        (&["nope"][..], 2, String::new(), format!("usage: {cli} migrate status | import-secrets … | export-secrets … | approve mv_… ({cli} migrate --help)\n")),
        (&["approve"][..], 2, String::new(), format!("usage: {cli} migrate approve <mv_…> ({cli} migrate status lists held steps)\n")),
        (&["export-secrets", "x"][..], 2, String::new(), format!("usage: {cli} migrate export-secrets --to <age1…> --out <file> | --unfreeze ({cli} migrate --help)\n")),
        (&["import-secrets"][..], 2, String::new(), format!("usage: {cli} migrate import-secrets <file> [--apply] (or: begin, --undo)\n")),
    ] {
        let c = case("brand", args, code);
        let ran = rig.run(false, false, &c);
        assert_eq!((ran.out.stdout.as_str(), ran.out.stderr.as_str()), (out.as_str(), err.as_str()), "{args:?}");
        assert_eq!(ran.out.code, Some(code), "{args:?}");
    }
    let c = sock(
        "brand: status",
        &["status"],
        vec![ok(r#"{"vault":{"open":true},"frozen":{"at":"t"}}"#)],
        0,
    );
    let ran = rig.run(false, false, &c);
    assert!(
        ran.out.stdout.contains(&format!(
            "({cli} migrate export-secrets --unfreeze to call it off)"
        )) && ran
            .out
            .stdout
            .contains(&format!("none ({cli} migrate import-secrets begin)")),
        "{ran:#?}"
    );
    let mut c = sock("brand: no answer", &["status"], vec![Reply::Close], 1);
    c.code = 1;
    let ran = rig.run(false, false, &c);
    assert_eq!(
        ran.out.stderr,
        format!("vault_unavailable: the {product} daemon didn't answer (is it running on this machine?)\n")
    );
    if cli != "k2" {
        for s in [&ran.out.stdout, &ran.out.stderr] {
            assert!(!s.contains("k2 ") && !s.contains("k5 "), "{s}");
        }
    }
}
