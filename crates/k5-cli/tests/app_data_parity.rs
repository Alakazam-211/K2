//! `k5-cli app domain|domains`, `app data put|ls|get|rm|mv` and `app
//! resources [settings|clean|trends]` against the shell CLI's
//! (`cli/src/app-updates.sh`, `cli/src/app-grace.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `domain add|rm`: a label's lines, a custom domain's, held, the plain
//!   printer after the custom check raised; `check|primary|external`;
//!   `domains` with every kind of row; alone, the usage line; usage
//!   lines.
//! - `data ls`; `data get` to a file (two pieces), to stdout, `--json`, a
//!   size that changed, a piece the reader can't read, a refusal mid-way
//!   (no part file left); `rm`, `mv`, `put` (a path made absolute).
//! - `resources`: the view, `--app`/`--owner`, `--rescan` (each answer),
//!   `--rescan --wait`; `settings` read and set (sizes, a bad size);
//!   `clean`; `trends`.
//! - Help never acts; the brand's name in the lines.

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

fn one(name: &'static str, args: &'static [&'static str], reply: Reply, code: i32) -> Case {
    case(name, args, &[], vec![reply], owner_and_port, code, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn refused() -> Reply {
    Reply::Answer(
        403,
        r#"{"error":{"code":"owner_only","hint":"the owner's"}}"#.into(),
    )
}

fn check(rig: &Rig, v: Vec<Case>) {
    for c in v {
        rig.check(&c);
    }
}

const HELD: &str =
    r#"{"status":"held","request":{"id":"r9","summary":"domain add"},"hint":"The owner decides."}"#;
const CUSTOM: &str = r#"{"custom":{"hostname":"shop.example.com","state":"pending","words":"waiting for DNS é","lastError":"no A record","relay":{"state":"claimed"},"setup":{"message":"set these","records":"advanced","advanced":"Advanced: both"},"records":[{"type":"A","name":"shop","value":"203.0.113.5"},{"type":"ALIAS","name":"@","value":"box.example","note":"if your DNS has ALIAS"}],"next":"run check","ownServer":"or your own server"},"txt":{"planted":true},"note":"bound"}"#;
const DOMAINS: &str = r#"{"own":{"label":"acme"},"taken":[{"host":"site.box.example","from":"ws/site"}],"attached":[{"label":"shop","addedBy":"ann","previousTarget":"ws/old"},{"host":"x.example","addedBy":"bo"}],"custom":[{"hostname":"shop.example.com","state":"live","primary":true,"words":"serving","path":"connect","setup":{"pathB":{"proxy":{"state":"waiting","words":"proxy waits"}}},"relay":{"state":"verified","error":"slow"},"lastError":"old"},{"hostname":"own.example","state":"external","words":"yours","path":"direct"},{"hostname":"new.example","state":"pending","words":"set DNS","setup":{"message":"m","records":"plain"},"records":[{"type":"CNAME","name":"new","value":"acme.box"}],"relay":{"state":"odd"},"next":"wait"}],"pathB":{"available":true,"meter":"2 of 5"},"maxCustomNames":5}"#;
const RESOURCES: &str = r#"{"box":{"host":"box1","cores":8,"memBytes":17179869184,"checkedAt":1700000000},"guard":{"mode":"on","modeSource":"setting","lines":{"warnPct":85,"warnFreeBytes":21474836480,"refusePct":95,"reserveBytes":10737418240,"criticalPct":98,"criticalFreeBytes":2147483648}},"disks":[{"mount":"/","usedBytes":123456789012,"totalBytes":500000000000,"usedPct":24.7,"freeBytes":376543210988,"level":"ok"}],"mem":{"totalBytes":17179869184,"availableBytes":1610612736},"consumers":[],"byOwner":[]}"#;
const PIECE_1: &str = r#"{"data":"aGVs","size":5,"mtime":10,"eof":false,"offset":0,"length":3}"#;
const PIECE_2: &str = r#"{"data":"bG8=","size":5,"mtime":10,"eof":true,"offset":3,"length":2}"#;
const TRENDS: &str = r#"{"window":"24h","resSecs":60,"box":{"cores":8,"memTotalBytes":17179869184,"summary":{"cpuAvgCores":1.234,"cpuPeakCores":3,"memAvgBytes":8589934592,"memPeakBytes":12884901888},"points":[{"cpuCores":1,"memBytes":8589934592},{"cpuCores":null,"memPeakBytes":12884901888},{"cpuCores":3.5,"memBytes":1073741824}]},"apps":[{"app":"acme","owner":{"handle":"ann"},"memBudgetBytes":2147483648,"summary":{"memAvgBytes":536870912},"points":[{"cpuCores":0.5,"memBytes":536870912}]}],"services":[{"service":"site","summary":null,"points":[]}]}"#;

#[test]
fn domains_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "domains");
    check(
        &rig,
        vec![
            // native: app domain add
            one(
                "domain add, a label",
                &["domain", "add", "acme", "shop", "--transfer"],
                ok(r#"{"label":"shop","app":"acme","frontPort":4100,"note":"next start"}"#),
                0,
            ),
            // native: app domain add
            one(
                "domain add, already",
                &["domain", "add", "acme", "shop"],
                ok(r#"{"label":"shop","app":"acme","already":true,"note":"yes é"}"#),
                0,
            ),
            // native: app domain rm
            one(
                "domain rm, back to before",
                &["domain", "rm", "acme", "shop"],
                ok(r#"{"label":"shop","app":"acme","now":"ws/old"}"#),
                0,
            ),
            // native: app domain add
            one(
                "domain add, a custom domain",
                &["domain", "add", "acme", "shop.example.com", "--external"],
                ok(CUSTOM),
                0,
            ),
            // native: app domain rm
            one(
                "domain rm, a custom domain",
                &["domain", "rm", "acme", "shop.example.com"],
                ok(r#"{"app":"acme","removed":"shop.example.com"}"#),
                0,
            ),
            // native: app domain add --json
            one(
                "domain add --json, held",
                &["domain", "add", "acme", "shop", "--json"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app domain add
            traceback(one(
                "domain add, an answer that is a number",
                &["domain", "add", "acme", "shop"],
                ok("5"),
                1,
            )),
            // native: app domain check
            one(
                "domain check",
                &["domain", "check", "acme", "shop.example.com"],
                ok(CUSTOM),
                0,
            ),
            // native: app domain primary
            one(
                "domain primary",
                &["domain", "primary", "acme", "shop.example.com"],
                ok(r#"{"primary":"https://shop.example.com","note":"at the next start"}"#),
                0,
            ),
            // native: app domain external
            one(
                "domain external on, no change",
                &["domain", "external", "acme", "own.example", "on"],
                ok(
                    r#"{"custom":{"hostname":"own.example","state":"external","words":"yours"},"already":true,"external":true,"txt":{"why":"not your zone"}}"#,
                ),
                0,
            ),
            // native: app domain external --json
            one(
                "domain external off --json",
                &["domain", "external", "acme", "own.example", "off", "--json"],
                ok(r#"{"custom":{}}"#),
                0,
            ),
            // native: app domain external
            stop(
                "domain external: no mode",
                &["domain", "external", "acme", "own.example"],
                2,
            ),
            // native: app domains
            one("domains", &["domains", "acme"], ok(DOMAINS), 0),
            // native: app domain list
            one(
                "domain list, Path B closed",
                &["domain", "list", "acme"],
                ok(r#"{"own":{"host":"acme.box.example"},"pathB":{"why":"no relay"}}"#),
                0,
            ),
            // native: app domains --json
            one(
                "domains --json",
                &["domains", "--json", "acme"],
                ok(DOMAINS),
                0,
            ),
            // native: app domains
            traceback(one(
                "domains: its own label missing",
                &["domains", "acme"],
                ok(r#"{"own":{}}"#),
                1,
            )),
            // native: app domain
            stop("domain: alone is the usage line", &["domain"], 2),
            // native: app domain
            stop("domain: --json alone", &["domain", "--json"], 2),
            // native: app domain
            stop("domain: a word it doesn't take", &["domain", "move"], 2),
            // native: app domain rm
            stop(
                "domain rm --external",
                &["domain", "rm", "acme", "shop", "--external"],
                2,
            ),
            // native: app domain add
            stop("domain add: no label", &["domain", "add", "acme"], 2),
            // native: app domains
            stop("domains: no App", &["domains"], 2),
            // native: app domain add
            one(
                "domain add: refused",
                &["domain", "add", "acme", "shop"],
                refused(),
                3,
            ),
        ],
    );
}

#[test]
fn data_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "data");
    check(
        &rig,
        vec![
            // native: app data ls
            one(
                "data ls",
                &["data", "ls", "acme"],
                ok(
                    r#"{"at":"/","kind":"dir","entries":[{"type":"dir","name":"logs","size":4096,"mtime":1700000000},{"type":"file","name":"big.bin","size":5000000000,"mtime":1700000000.5},{"type":"link","name":"l","size":null},{"type":"file","name":"tiny","size":12}],"truncated":true,"max":1000}"#,
                ),
                0,
            ),
            // native: app data list
            one(
                "data list, a path, one entry",
                &["data", "list", "acme", "logs é"],
                ok(r#"{"at":"/logs","kind":"dir","entries":[{"type":"file","name":"a"}]}"#),
                0,
            ),
            // native: app data ls --json
            one(
                "data ls --json",
                &["data", "ls", "--json", "acme"],
                ok(r#"{"entries":[]}"#),
                0,
            ),
            // native: app data ls
            stop("data ls: three words", &["data", "ls", "a", "b", "c"], 2),
            // native: app data ls
            traceback(one(
                "data ls: a size that is a word",
                &["data", "ls", "acme"],
                ok(r#"{"entries":[{"type":"file","name":"a","size":"big"}]}"#),
                1,
            )),
            // native: app data get
            case(
                "data get: two pieces into a file",
                &["data", "get", "acme", "notes/hello.txt"],
                &[],
                vec![ok(PIECE_1), ok(PIECE_2)],
                owner_and_port,
                0,
                2,
            ),
            // native: app data get --json
            case(
                "data get --to --json",
                &[
                    "data",
                    "get",
                    "acme",
                    "notes/hello.txt",
                    "--to",
                    "copy.txt",
                    "--json",
                ],
                &[],
                vec![ok(PIECE_1), ok(PIECE_2)],
                owner_and_port,
                0,
                2,
            ),
            // native: app data get
            case(
                "data get: to stdout",
                &["data", "get", "acme", "notes/hello.txt", "-"],
                &[],
                vec![ok(PIECE_1), ok(PIECE_2)],
                owner_and_port,
                0,
                2,
            ),
            // native: app data get
            case(
                "data get: the size changed",
                &["data", "get", "acme", "notes/changed.txt"],
                &[],
                vec![
                    ok(PIECE_1),
                    ok(r#"{"data":"bG8=","size":6,"mtime":10,"eof":true,"offset":3,"length":2}"#),
                ],
                owner_and_port,
                3,
                2,
            ),
            // native: app data get
            case(
                "data get: the size changed, a quote and a backslash in the path",
                &["data", "get", "acme", r#"notes/say "hi"\back.txt"#],
                &[],
                vec![
                    ok(PIECE_1),
                    ok(r#"{"data":"bG8=","size":6,"mtime":10,"eof":true,"offset":3,"length":2}"#),
                ],
                owner_and_port,
                3,
                2,
            ),
            // native: app data get
            traceback(case(
                "data get: a piece with no data",
                &["data", "get", "acme", "notes/bad.txt"],
                &[],
                vec![ok(r#"{"size":1}"#)],
                owner_and_port,
                3,
                1,
            )),
            // native: app data get
            case(
                "data get: refused mid-way",
                &["data", "get", "acme", "notes/gone.txt"],
                &[],
                vec![ok(PIECE_1), refused()],
                owner_and_port,
                3,
                2,
            ),
            // native: app data get
            stop(
                "data get: --to empty",
                &["data", "get", "acme", "p", "--to", ""],
                2,
            ),
            // native: app data get
            stop(
                "data get: --json and -",
                &["data", "get", "acme", "p", "-", "--json"],
                2,
            ),
            // native: app data get
            stop("data get: no path", &["data", "get", "acme"], 2),
            // native: app data rm
            one(
                "data rm -r",
                &["data", "rm", "acme", "logs", "-r"],
                ok(r#"{"removed":"dir","at":"/logs","app":"acme"}"#),
                0,
            ),
            // native: app data remove
            one(
                "data remove, held",
                &["data", "remove", "acme", "x"],
                Reply::Answer(202, HELD.into()),
                2,
            ),
            // native: app data mv
            one(
                "data mv --force",
                &["data", "mv", "acme", "a", "b", "--force"],
                ok(r#"{"from":"/a","to":"/b","app":"acme","replaced":true}"#),
                0,
            ),
            // native: app data move --json
            one(
                "data move --json",
                &["data", "move", "acme", "a", "b", "--json"],
                ok(r#"{"from":"/a"}"#),
                0,
            ),
            // native: app data mv
            stop(
                "data mv: four words",
                &["data", "mv", "a", "b", "c", "d"],
                2,
            ),
            // native: app data put
            one(
                "data put: a path made absolute",
                &[
                    "data",
                    "put",
                    "acme",
                    ".",
                    "--to",
                    "in",
                    "--disk-override=space é",
                ],
                ok(
                    r#"{"files":3,"from":"/w","to":"in","app":"acme","diskWarning":{"hint":"85% used"}}"#,
                ),
                0,
            ),
            // native: app data put
            one(
                "data put: a path that isn't there",
                &["data", "put", "acme", "missing/x"],
                ok(r#"{"files":0}"#),
                0,
            ),
            // native: app data put
            stop("data put: no path", &["data", "put", "acme"], 2),
            // native: app data put
            stop(
                "data put: --to last",
                &["data", "put", "acme", "x", "--to"],
                2,
            ),
            // native: app data
            stop("data: the page", &["data"], 0),
            // native: app data
            stop("data: a word it doesn't take", &["data", "cp"], 2),
        ],
    );
    // The last side's file is there whole, and no part file is left.
    assert_eq!(std::fs::read(rig.proj.join("hello.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::read(rig.proj.join("copy.txt")).unwrap(), b"hello");
    let left: Vec<_> = std::fs::read_dir(&rig.proj)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains("k2-part"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn resources_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "boxuse");
    check(
        &rig,
        vec![
            // native: app resources
            one("resources", &["resources"], ok(RESOURCES), 0),
            // native: app resources --json
            one(
                "resources --app --owner --json",
                &["resources", "--app", "acme", "--owner", "ann é", "--json"],
                ok(RESOURCES),
                0,
            ),
            // native: app resources
            case(
                "resources --rescan: started",
                &["resources", "--rescan"],
                &[],
                vec![ok(r#"{"started":true}"#), ok(RESOURCES)],
                owner_and_port,
                0,
                2,
            ),
            // native: app resources
            case(
                "resources --rescan: skipped",
                &["resources", "--rescan", "--wait"],
                &[],
                vec![ok(r#"{"skipped":"too soon"}"#), ok(RESOURCES)],
                owner_and_port,
                0,
                2,
            ),
            // native: app resources
            case(
                "resources --rescan: no scanner",
                &["resources", "--rescan"],
                &[],
                vec![ok(r#"{}"#), ok(RESOURCES)],
                owner_and_port,
                0,
                2,
            ),
            // native: app resources
            case(
                "resources --rescan --wait: done at the first look",
                &["resources", "--rescan", "--wait"],
                &[],
                vec![
                    ok(r#"{"running":true}"#),
                    ok(r#"{"scan":{"running":false}}"#),
                    ok(RESOURCES),
                ],
                owner_and_port,
                0,
                3,
            ),
            // native: app resources
            stop("resources: --wait alone", &["resources", "--wait"], 2),
            // native: app resources
            stop("resources: a word", &["resources", "acme"], 2),
            // native: app resources
            stop("resources: --app last", &["resources", "--app"], 2),
            // native: app resources trends
            stop(
                "resources trends: --window last",
                &["resources", "trends", "--window"],
                2,
            ),
            // native: app resources settings
            stop(
                "resources settings: --mode last",
                &["resources", "settings", "--mode"],
                2,
            ),
            // native: app resources settings
            one(
                "resources settings: the read",
                &["resources", "settings"],
                ok(RESOURCES),
                0,
            ),
            // native: app resources settings --json
            one(
                "resources settings --json: the read",
                &["resources", "settings", "--json"],
                ok(RESOURCES),
                0,
            ),
            // native: app resources settings
            one(
                "resources settings: set",
                &[
                    "resources",
                    "settings",
                    "--mode",
                    "warn",
                    "--warn",
                    "80",
                    "--warn-free",
                    "20G",
                    "--refuse",
                    "09",
                    "--reserve",
                    "1.5 GiB",
                    "--critical-free",
                    "512m",
                ],
                ok(
                    r#"{"changed":true,"guard":{"mode":"warn","modeSource":"setting","lines":{"warnPct":80,"warnFreeBytes":21474836480,"reserveBytes":1610612736}},"note":"saved é"}"#,
                ),
                0,
            ),
            // native: app resources settings
            one(
                "resources settings: refused",
                &["resources", "settings", "--mode", "off"],
                refused(),
                3,
            ),
            // native: app resources settings
            stop(
                "resources settings: a bad size",
                &["resources", "settings", "--reserve", "10X"],
                2,
            ),
            // native: app resources settings
            stop(
                "resources settings: a bad mode",
                &["resources", "settings", "--mode", "auto"],
                2,
            ),
            // native: app resources settings
            stop(
                "resources settings: a bad percent",
                &["resources", "settings", "--warn", "100"],
                2,
            ),
            // native: app resources settings
            stop(
                "resources settings: a word",
                &["resources", "settings", "now"],
                2,
            ),
            // native: app resources clean
            one(
                "resources clean --dry-run",
                &["resources", "clean", "--dry-run"],
                ok(
                    r#"{"dryRun":true,"scope":"box","items":[{"kind":"seed","bytes":3200000000,"app":"acme","id":"seed-1"},{"kind":"odd","bytes":null,"id":"x","error":"busy"}],"totals":{"bytes":3200000000},"never":"never your data","hint":"run it for real"}"#,
                ),
                0,
            ),
            // native: app resources clean
            one(
                "resources clean",
                &["resources", "clean"],
                ok(
                    r#"{"items":[{"kind":"image","bytes":999,"id":"img","removed":false}],"totals":{"freedBytes":0},"engine":{"status":"error: down"}}"#,
                ),
                0,
            ),
            // native: app resources clean --json
            one(
                "resources clean --json",
                &["resources", "clean", "--json"],
                ok(r#"{"items":[]}"#),
                0,
            ),
            // native: app resources clean
            stop("resources clean: a word", &["resources", "clean", "all"], 2),
            // native: app resources trends
            one("resources trends", &["resources", "trends"], ok(TRENDS), 0),
            // native: app resources trends
            one(
                "resources trends: no samples",
                &["resources", "trends", "--app", "acme", "--window", "7d"],
                ok(r#"{"window":"7d","resSecs":3600}"#),
                0,
            ),
            // native: app resources trends --json
            one(
                "resources trends --json",
                &["resources", "trends", "--json"],
                ok(TRENDS),
                0,
            ),
            // native: app resources trends
            traceback(one(
                "resources trends: a point that is a word",
                &["resources", "trends"],
                ok(r#"{"box":{"points":["x"]}}"#),
                1,
            )),
            // native: app resources trends
            stop(
                "resources trends: a word",
                &["resources", "trends", "now"],
                2,
            ),
        ],
    );
}

#[test]
fn the_changes_help_never_acts() {
    let rig = Rig::new(VERB, "datahelp");
    for args in [
        &["domain", "add", "acme", "shop", "-h"][..],
        &["domain", "external", "acme", "h", "on", "--help"],
        &["data", "rm", "acme", "x", "-h"],
        &["data", "put", "acme", ".", "--help"],
        &["data", "put", "-h"],
        &["data", "put", "--help"],
        &["data", "put", "acme", "--json", "-h", "x"],
        // A value flag never takes -h or --help: help wins.
        &["data", "put", "acme", "x", "--to", "-h"],
        &["data", "put", "acme", "--to", "--help", "x"],
        &["data", "put", "acme", "x", "--disk-override", "-h"],
        &["domain", "-h"],
        &["domain", "--help"],
        &["domain", "rm", "--help", "acme", "shop"],
        &["data", "get", "acme", "p", "-h"],
        &["resources", "settings", "--mode", "off", "-h"],
        &["resources", "clean", "--help"],
        &["resources", "--rescan", "-h"],
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "databrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want, code) in [
        (
            &["domains"][..],
            format!("usage: {cli} app domains <app-id> [--json]\n"),
            2,
        ),
        (
            &["domain"],
            format!("usage: {cli} app domain add|rm <app-id> <subdomain> [--json]\n"),
            2,
        ),
        (
            &["domain", "rm", "a", "b", "--external"],
            format!("--external goes with {cli} app domain add (to switch a bound name: {cli} app domain external <app-id> <host> on|off)\n"),
            2,
        ),
        (
            &["data", "cp"],
            format!("usage: {cli} app data put|ls|get|rm|mv <app-id> \u{2026} ({cli} app data --help)\n"),
            2,
        ),
        (
            &["data", "get", "a", "p", "-", "--json"],
            format!("{cli} app data get: --json and - (stdout) don't go together\n"),
            2,
        ),
        (
            &["resources", "clean", "x"],
            format!("usage: {cli} app resources clean [--dry-run] [--json]\n"),
            2,
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, code, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
