//! `k5-cli db` against the shell CLI's `cli/k2 db`, on the parity rig
//! ([`common::parity`]): the same requests (urllib's, from `_db_py`, and
//! curl's for `home`, `cap` and `--workspace`), stdout, stderr and exit
//! code.
//!
//! - Every `_db_py` verb, plain and `--json`: status (and `--health`),
//!   enable, disable, uninstall, doctor, create, clusters (two calls),
//!   default-major, list, dsn (`creds` too), migrate, dump, restore, drop,
//!   grant, revoke, bind.
//! - `home` (read, `--keep`, `--json`, the error and message answers) and
//!   `cap` (show and set).
//! - `--workspace`/`--project`: a folder, a name the daemon resolves, a
//!   name it doesn't, a flag with no value.
//! - Usage errors, help, `help` as a verb, a value flag at the end
//!   (`set -e`).
//! - The daemon's errors: `ok:false`, a 4xx with an error object, a body
//!   that is not JSON, an empty one; the owner-only hint; the permission
//!   note; answers Python raises on.
//! - No answer: nothing listening, closed with nothing, a reset.
//! - A passport over TCP, the `db` id tool's refusal, the gates.
//! - One answer above 256 KiB: thousands of databases.
//! - The shell's: `run`, `upgrade`, `cluster`, `versions-access`,
//!   `create --cluster`, and `-h` on a verb that changes something.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "db";

const STATUS: &str = r#"{"ok":true,"supported":true,"state":"running","installedMajor":17,"listen":"127.0.0.1","port":5432,
 "publishHint":"open 5432","lastError":"none é","defaultMajor":17,"defaultMajorSource":"set",
 "clusters":[{"major":17,"port":5432,"role":"primary","state":"online"},{"major":16,"port":5433,"role":"foreign"}],
 "upgradeAvailable":[{"name":"app","from":16,"to":[17,18],"check":"ok"},{"name":"b","from":15}],
 "upgradeBlocked":{"hint":"stop the app first"},
 "pgStale":[{"name":"main","package":"postgresql-17","installedVersion":"17.2","fix":"restart it"},{"name":"x","fix":null}]}"#;

const CLUSTERS_STATUS: &str = r#"{"ok":true,"defaultMajor":17,"defaultMajorPinned":true,
 "clusters":[{"major":17}],"databases":[{"name":"a","major":17},{"name":"b","major":17,"cluster":"17/feedback"},{"name":"c","major":16}],
 "upgradeAvailable":[{"name":"c","from":16,"to":[17]}]}"#;

const CLUSTERS: &str = r#"{"clusters":[
 {"label":"17/main","major":17,"name":"main","port":5432,"unit":"pg@17-main","role":"primary","state":"online",
  "health":{"connections":3,"maxConnections":100,"archiveMode":"on","databases":[{"name":"a","bytes":3145728},{"name":"z","bytes":1572864}],
   "archiver":{"archivedCount":4,"failedCount":0,"lastArchivedAt":"t1"}}},
 {"label":"17/feedback","major":17,"name":"feedback","port":5434,"unit":"pg@17-feedback","role":"named","state":"online","walArchive":true,
  "health":{"online":false}},
 {"major":16,"port":5433,"role":"foreign","state":"online","health":{"error":"no access"}},
 {"label":"","major":15,"name":null,"port":5435,"health":{"databases":[]}}],
 "helperView":{"ok":true}}"#;

const CREATE: &str = r#"{"ok":true,"name":"app_db","major":17,"majorNote":"newest","existing":true,"dsn":"postgres://x","migratorDsn":"postgres://m",
 "dsnHint":"k2 db dsn --reveal","kept":{"ref":"vault:db/app","name":"DATABASE_URL"},"cap":{"used":2,"cap":0},
 "apps":[{"note":"restart app"}],"next":"k2 app db check"}"#;

const DOCTOR: &str = r#"{"ok":true,"baked":true,"unit":"pg","select1":1,"installedMajor":17,"liveMajor":16,"majorMismatch":true,
 "clusters":[{"major":17,"port":5432,"unit":"u","unitState":"active","databases":2}],
 "ram":{"currentBytes":10,"maxBytes":20,"high":"1G","max":"2G","withinCap":true,"unit":"pg.slice","oomTarget":"pg","dropinOk":false},
 "gucsOk":false,"gucs":{"work_mem":"4MB","shared_buffers":"128MB","other":1},"hint":"tune it"}"#;

const HOME: &str = r#"{"homeDb":{"file":"k2.db","outcome":"kept","reason":"more workspaces","markerAt":"t","held":true,
 "blocked":{"message":"two copies"},"choice":{"keep":"k2.db"}}}"#;

fn ok_owner(name: &'static str, args: &'static [&'static str], replies: Vec<Reply>) -> Case {
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, 0, n)
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
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

fn many_databases(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"name":"database_{i}_{}","status":"{}","id":"{i:08}"}}"#,
                "é".repeat(i % 20),
                if i % 3 == 0 { "active" } else { "test" }
            )
        })
        .collect();
    format!(r#"{{"ok":true,"databases":[{}]}}"#, rows.join(","))
}

/// The case's folder holds `sub`, a folder `--workspace sub` names.
fn with_sub(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("sub")).unwrap();
}

#[test]
fn db_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let big = many_databases(4000);
    assert!(
        big.len() > 256 * 1024,
        "the large answer is {} bytes",
        big.len()
    );
    let cases = vec![
        // Help and usage: nothing sent.
        case("no verb", &[], &[], vec![], owner_and_port, 0, 0),
        case(
            "help is no verb",
            &["help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("unknown verb", &["nope"], &[], vec![], owner_and_port, 2, 0),
        case(
            "status, a stray flag",
            &["status", "--x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "create --id at the end",
            &["create", "--id"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "create, a bad major",
            &["create", "--major", "9"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "create, a bad access",
            &["create", "--access", "all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "create --test --name",
            &["create", "--test", "--name", "n"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "default-major, none",
            &["default-major"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "dsn --as at the end",
            &["dsn", "--as"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "dsn --db -x",
            &["dsn", "--db", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "dsn --actor x",
            &["dsn", "--actor", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "dsn --ref --reveal",
            &["dsn", "--ref", "--reveal"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "restore, no file",
            &["restore"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "restore, two files",
            &["restore", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "grant, no level",
            &["grant", "w", "--db", "d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "grant, a bad level",
            &["grant", "w", "--db", "d", "--level", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "grant, --relation -x",
            &["grant", "w", "--relation", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "revoke, no db",
            &["revoke", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "bind, no role",
            &["bind"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cap, a word",
            &["cap", "x5"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cap, too big",
            &["cap", "1234567"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cap help",
            &["cap", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "home, a stray word",
            &["home", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "home --keep x",
            &["home", "--keep", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "home help",
            &["home", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // The verbs.
        // covers: db status
        ok_owner("status", &["status"], vec![ok(STATUS)]),
        // covers: db status --json
        ok_owner(
            "status --health --json",
            &["status", "--health", "--json"],
            vec![ok(STATUS)],
        ),
        ok_owner(
            "status, bare",
            &["status"],
            vec![ok(r#"{"supported":false}"#)],
        ),
        ok_owner(
            "enable",
            &["enable"],
            vec![ok(r#"{"ok":true,"state":"starting"}"#)],
        ),
        ok_owner(
            "disable --json",
            &["disable", "--json"],
            vec![ok(r#"{"ok":true,"state":"off"}"#)],
        ),
        ok_owner("uninstall", &["uninstall"], vec![ok(r#"{"ok":true}"#)]),
        ok_owner("doctor", &["doctor"], vec![ok(DOCTOR)]),
        // covers: db create
        ok_owner(
            "create",
            &[
                "create", "--id", "i 1", "--name", "app é", "--access", "write", "--major", "17",
            ],
            vec![ok(CREATE)],
        ),
        // covers: db create --json
        ok_owner(
            "create --test",
            &["create", "--test", "--major=16", "--json"],
            vec![ok(r#"{"ok":true,"name":"t"}"#)],
        ),
        ok_owner(
            "create, small",
            &["create"],
            vec![ok(
                r#"{"ok":true,"name":"x","kept":{"error":"no vault"},"cap":{"used":1}}"#,
            )],
        ),
        // covers: db clusters
        ok_owner(
            "clusters",
            &["clusters"],
            vec![ok(CLUSTERS_STATUS), ok(CLUSTERS)],
        ),
        // covers: db clusters --json
        ok_owner(
            "clusters --health --json",
            &["clusters", "--health", "--json"],
            vec![ok(CLUSTERS_STATUS), ok(CLUSTERS)],
        ),
        ok_owner(
            "clusters, the helper says no",
            &["clusters"],
            vec![ok(CLUSTERS_STATUS), answer(500, "oops")],
        ),
        case(
            "clusters, refused first",
            &["clusters"],
            &[],
            vec![ok(r#"{"ok":false,"error":{"code":"forbidden"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        // covers: db default-major
        ok_owner(
            "default-major",
            &["default-major", "17"],
            vec![ok(r#"{"ok":true,"defaultMajor":17}"#)],
        ),
        // covers: db list
        ok_owner(
            "list",
            &["list"],
            vec![ok(
                r#"{"ok":true,"databases":[{"name":"a","status":"active"},{"name":"b"}]}"#,
            )],
        ),
        // covers: db list --json
        ok_owner(
            "list --json",
            &["list", "--json"],
            vec![ok(r#"{"ok":true,"databases":[]}"#)],
        ),
        // covers: db dsn
        ok_owner(
            "dsn",
            &["dsn"],
            vec![ok(
                r#"{"ok":true,"dsn":"postgres://u@h/d","dsnHasPassword":false,"dsnHint":"use --reveal"}"#,
            )],
        ),
        ok_owner(
            "dsn --reveal",
            &[
                "dsn", "--reveal", "--db", "d1", "--test", "--actor", "migrator",
            ],
            vec![ok(
                r#"{"ok":true,"dsn":"postgres://u:p@h/d","dsnHasPassword":true}"#,
            )],
        ),
        ok_owner(
            "creds --ref --as",
            &["creds", "--ref", "--as", "PG_URL", "--db=d2"],
            vec![ok(r#"{"ok":true,"ref":"vault:db/d2","name":"PG_URL"}"#)],
        ),
        ok_owner("dsn, none", &["dsn"], vec![ok(r#"{"ok":true}"#)]),
        // covers: db migrate
        ok_owner(
            "migrate",
            &["migrate", "--dir", "m", "--db", "d"],
            vec![ok(r#"{"ok":true,"applied":["001","002"]}"#)],
        ),
        ok_owner(
            "migrate, done before",
            &["migrate"],
            vec![ok(r#"{"ok":true,"applied":[],"noop":true,"discovered":3}"#)],
        ),
        ok_owner("migrate, nothing", &["migrate"], vec![ok(r#"{"ok":true}"#)]),
        // covers: db dump
        ok_owner(
            "dump",
            &["dump", "--out", "/tmp/x.sql"],
            vec![ok(r#"{"ok":true,"path":"/tmp/x.sql"}"#)],
        ),
        // covers: db restore
        ok_owner(
            "restore",
            &["restore", "f.sql", "--db", "d"],
            vec![ok(r#"{"ok":true,"restored":"d"}"#)],
        ),
        // covers: db drop
        ok_owner(
            "drop",
            &["drop", "--yes", "--test", "--db=d"],
            vec![ok(r#"{"ok":true,"dropped":"d","apps":[{"note":"n"}]}"#)],
        ),
        // covers: db grant
        ok_owner(
            "grant",
            &[
                "grant",
                "w2",
                "--db",
                "d",
                "--level",
                "read",
                "--relation",
                "public.t",
                "--relation=é",
                "--manage",
            ],
            vec![ok(
                r#"{"ok":true,"level":"read","name":"d","projectId":"p2","canManage":true}"#,
            )],
        ),
        // covers: db revoke --json
        ok_owner(
            "revoke --json",
            &["revoke", "w2", "--db", "d", "--json"],
            vec![ok(r#"{"ok":true,"name":"d","projectId":"p2"}"#)],
        ),
        // covers: db revoke
        ok_owner(
            "revoke",
            &["revoke", "w2", "--db", "d"],
            vec![ok(r#"{"ok":true,"name":"d","projectId":"p2","apps":[]}"#)],
        ),
        // covers: db bind
        ok_owner(
            "bind",
            &["bind", "--role", "app_rw", "--db", "d"],
            vec![ok(r#"{"ok":true,"bindRole":"app_rw"}"#)],
        ),
        // covers: db default-major --json
        ok_owner(
            "default-major --json",
            &["default-major", "16", "--json"],
            vec![ok(r#"{"ok":true,"defaultMajor":16}"#)],
        ),
        // covers: db grant --json
        ok_owner(
            "grant --json",
            &["grant", "w2", "--db", "d", "--level", "write", "--json"],
            vec![ok(r#"{"ok":true,"level":"write"}"#)],
        ),
        // covers: db bind --json
        ok_owner(
            "bind --json",
            &["bind", "--role", "r", "--json"],
            vec![ok(r#"{"ok":true,"bindRole":"r"}"#)],
        ),
        // covers: db dsn --json
        ok_owner(
            "dsn --json",
            &["dsn", "--json"],
            vec![ok(r#"{"ok":true,"dsn":"postgres://u@h/d"}"#)],
        ),
        // covers: db migrate --json
        ok_owner(
            "migrate --json",
            &["migrate", "--json"],
            vec![ok(r#"{"ok":true,"applied":[]}"#)],
        ),
        // covers: db dump --json
        ok_owner(
            "dump --json",
            &["dump", "--json"],
            vec![ok(r#"{"ok":true,"path":"p"}"#)],
        ),
        // covers: db restore --json
        ok_owner(
            "restore --json",
            &["restore", "f.sql", "--json"],
            vec![ok(r#"{"ok":true,"restored":"d"}"#)],
        ),
        // covers: db drop --json
        ok_owner(
            "drop --json",
            &["drop", "--yes", "--json"],
            vec![ok(r#"{"ok":true,"dropped":"d"}"#)],
        ),
        ok_owner(
            "a string answer, --json",
            &["list", "--json"],
            vec![ok(r#""just text""#)],
        ),
        // One answer above 256 KiB.
        ok_owner("list, thousands", &["list"], vec![ok(&big)]),
        ok_owner(
            "list --json, thousands",
            &["list", "--json"],
            vec![ok(&big)],
        ),
        // home and cap.
        ok_owner("home", &["home"], vec![ok(HOME)]),
        ok_owner("home, bare", &["home"], vec![ok(r#"{"homeDb":{}}"#)]),
        ok_owner(
            "home --json",
            &["home", "--json"],
            vec![ok(r#"{"ok":false,"homeDb":{}}"#)],
        ),
        ok_owner(
            "home --keep",
            &["home", "--keep", "k2so.db"],
            vec![ok(r#"{"message":"kept k2so.db"}"#)],
        ),
        case(
            "home, an error",
            &["home"],
            &[],
            vec![ok(r#"{"error":{"code":"forbidden","hint":"owner only"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "home, not JSON",
            &["home", "--keep=k2.db"],
            &[],
            vec![answer(502, "bad gateway\n\n")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "home, an error as text",
            &["home"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: db cap
        ok_owner("cap", &["cap"], vec![ok(r#"{"ok":true,"cap":0}"#)]),
        // covers: db cap --json
        ok_owner(
            "cap --json",
            &["cap", "--json"],
            vec![ok(r#"{"ok":true,"cap":7}"#)],
        ),
        case(
            "cap, refused",
            &["cap"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"no é"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        ok_owner("cap 007", &["cap", "007"], vec![ok(r#"{"ok":true}"#)]),
        ok_owner(
            "cap default --json",
            &["cap", "default", "--json"],
            vec![ok(r#"{"ok":true}"#)],
        ),
        ok_owner("cap 0", &["cap", "0"], vec![ok(r#"{"ok":true}"#)]),
        case(
            "cap 5, an agent",
            &["cap", "5"],
            &[],
            vec![answer(403, r#"{"error":"bad token"}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "cap 5, a bad field",
            &["cap", "5"],
            &[],
            vec![answer(
                400,
                r#"{"ok":false,"error":{"code":"usage","hint":"db_active_cap must be a number"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "cap 5, not JSON",
            &["cap", "5"],
            &[],
            vec![answer(500, "")],
            owner_and_port,
            1,
            1,
        ),
        // --workspace.
        Case {
            setup: with_sub,
            ..ok_owner(
                "--workspace, a folder",
                &["--workspace", "sub", "list"],
                vec![ok(r#"{"ok":true,"databases":[]}"#)],
            )
        },
        Case {
            setup: with_sub,
            ..ok_owner(
                "--project=, a folder",
                &["list", "--project=./sub/../sub", "--json"],
                vec![ok(r#"{"ok":true,"databases":[]}"#)],
            )
        },
        ok_owner(
            "--workspace, resolved",
            &["--workspace", "acme", "dsn"],
            vec![ok(r#"{"path":"/w/acme"}"#), ok(r#"{"ok":true,"dsn":"x"}"#)],
        ),
        ok_owner(
            "--workspace, not resolved",
            &["create", "--workspace", "globex é"],
            vec![
                ok(r#"{"error":"not_found"}"#),
                ok(r#"{"ok":true,"name":"g"}"#),
            ],
        ),
        ok_owner(
            "two --workspace flags",
            &["--workspace", "a", "--project", "b", "cap"],
            vec![
                ok(r#"{"projectPath":"/w/a"}"#),
                ok(r#"{"path":"/w/b"}"#),
                ok(r#"{"ok":true,"cap":3}"#),
            ],
        ),
        case(
            "--workspace, no value",
            &["--workspace", "a", "list", "--project"],
            &[],
            vec![ok(r#"{"path":"/w/a"}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "--workspace, then help",
            &["--workspace", "a"],
            &[],
            vec![ok(r#"{"path":"/w/a"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // The daemon's errors.
        case(
            "ok false, forbidden",
            &["status"],
            &[],
            vec![ok(
                r#"{"ok":false,"error":{"code":"forbidden","hint":"agents can't"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "owner_only, asked already",
            &["enable"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"ask your human"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "usage from the daemon",
            &["create"],
            &[],
            vec![answer(400, r#"{"error":{"code":"usage","hint":"bad"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a refusal code",
            &["create", "--major", "18"],
            &[],
            vec![answer(409, r#"{"error":{"code":"preview_major"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an error code",
            &["drop"],
            &[],
            vec![answer(
                500,
                r#"{"error":{"code":"engine","hint":"pg down","extra":[1]}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 500, not JSON",
            &["list"],
            &[],
            vec![answer(500, &"x".repeat(500))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 404, empty",
            &["list"],
            &[],
            vec![answer(404, "")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the permission note",
            &["enable"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "it can't be requested",
            &["disable"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"no"},"permission":{"requestable":false,"hint":"Only a person can."}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        ok_owner("a bulletin in the answer", &["list"], vec![ok(BULLETIN)]),
        traceback(case(
            "a list answer",
            &["status"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "clusters as a number",
            &["status"],
            &[],
            vec![ok(r#"{"clusters":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "applied as numbers",
            &["migrate"],
            &[],
            vec![ok(r#"{"applied":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "forbidden with a number hint",
            &["list"],
            &[],
            vec![answer(403, r#"{"error":{"code":"forbidden","hint":5}}"#)],
            owner_and_port,
            1,
            1,
        )),
        // No answer.
        case(
            "nothing listening",
            &["list"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "closed with nothing",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["enable"],
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
        // Who it is.
        Case {
            env: with_passport(&[]),
            ..case(
                "a passport over TCP",
                &["list"],
                &[],
                vec![ok(r#"{"ok":true,"databases":[]}"#)],
                nothing,
                0,
                1,
            )
        },
        case(
            "the id tool in a session with no passport",
            &["list"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "no port: the gate",
            &["list"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        // The shell's.
        shell(case(
            "run goes to the shell",
            &["run"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "upgrade goes to the shell",
            &["upgrade", "d", "--to", "9"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "cluster goes to the shell",
            &["cluster"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "versions-access goes to the shell",
            &["versions-access", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "create --cluster goes to the shell",
            &["create", "--cluster", "c", "--test"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        // Help never acts.
        shell(case(
            "create -h",
            &["create", "--name", "n", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "drop --help",
            &["drop", "--yes", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "grant --help",
            &["grant", "w", "--db", "d", "--level", "read", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "cap 5 -h",
            &["cap", "5", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "home --keep -h",
            &["home", "--keep", "k2.db", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "enable --help",
            &["enable", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "default-major -h",
            &["default-major", "17", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "--help first",
            &["--help", "drop", "--yes"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
    ];
    for c in &cases {
        rig.check(c);
    }
}
