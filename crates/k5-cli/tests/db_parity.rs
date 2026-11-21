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
//! - `run`: the DSN in the command's environment, never on its argv
//!   (a spy program checks), and its errors.
//! - The shell's: `-h` on a verb that changes something.
//! - The compute verbs (`_compute_send`): `versions-access`, `cluster
//!   create`, `restart` and `archive`, `create --cluster`, `upgrade` (its status,
//!   check, live, no-wait and watch forms), their refusals, answers the
//!   readers raise on, no answer, the cell socket, and the environment
//!   settings that hand them to the shell.

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
   "archiver":{"archivedCount":4,"failedCount":0,"lastArchivedAt":"t1","currentLsn":"0/1A2B3C4D"}}},
 {"label":"17/feedback","major":17,"name":"feedback","port":5434,"unit":"pg@17-feedback","role":"named","state":"online","walArchive":true,
  "health":{"online":false}},
 {"major":16,"port":5433,"role":"foreign","state":"online","health":{"error":"no access"}},
 {"label":"","major":15,"name":null,"port":5435,"health":{"databases":[]}}],
 "helperView":{"ok":true}}"#;

/// `--health` with no WAL archive: the current WAL position still prints;
/// a null one, an empty one and no archiver print nothing.
const CLUSTERS_LSN: &str = r#"{"clusters":[
 {"label":"17/main","port":5432,"health":{"connections":1,"maxConnections":100,"archiveMode":"off","archiver":{"currentLsn":"0/16B3748"}}},
 {"label":"17/b","port":5434,"health":{"connections":1,"maxConnections":100,"archiver":{"currentLsn":null}}},
 {"label":"17/c","port":5435,"health":{"connections":1,"maxConnections":100,"archiver":{"currentLsn":""}}},
 {"label":"17/d","port":5436,"health":{"connections":1,"maxConnections":100}},
 {"label":"17/e","port":5437,"health":{"connections":1,"maxConnections":100,"archiver":{"currentLsn":["0/1",2]}}}]}"#;

const CREATE: &str = r#"{"ok":true,"name":"app_db","major":17,"majorNote":"newest","existing":true,"dsn":"postgres://x","migratorDsn":"postgres://m",
 "dsnHint":"k2 db dsn --reveal","kept":{"ref":"vault:db/app","name":"DATABASE_URL"},"cap":{"used":2,"cap":0},
 "apps":[{"note":"restart app"}],"next":"k2 app db check"}"#;

const DOCTOR: &str = r#"{"ok":true,"baked":true,"unit":"pg","select1":1,"installedMajor":17,"liveMajor":16,"majorMismatch":true,
 "clusters":[{"major":17,"port":5432,"unit":"u","unitState":"active","databases":2}],
 "ram":{"currentBytes":10,"maxBytes":20,"high":"1G","max":"2G","withinCap":true,"unit":"pg.slice","oomTarget":"pg","dropinOk":false},
 "gucsOk":false,"gucs":{"work_mem":"4MB","shared_buffers":"128MB","other":1},"hint":"tune it"}"#;

const HOME: &str = r#"{"homeDb":{"file":"k2.db","outcome":"kept","reason":"more workspaces","markerAt":"t","held":true,
 "blocked":{"message":"two copies"},"choice":{"keep":"k2.db"}}}"#;

const ARCHIVE_ON: &str = r#"{"ok":true,"cluster":"17/fb é","walArchive":true,"changed":true,"restarted":true,"walSpool":"/var/lib/postgresql/k2-wal/17-fb"}"#;

const ARCHIVE_OFF: &str = r#"{"ok":true,"cluster":"17/fb","walArchive":false,"changed":true,"restarted":true,"walSpool":"/var/lib/postgresql/k2-wal/17-fb","walSpoolKept":true}"#;

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
            2,
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
            "clusters --health, the current WAL position",
            &["clusters", "--health"],
            vec![ok(CLUSTERS_STATUS), ok(CLUSTERS_LSN)],
        ),
        // `a.get("currentLsn")` on an archiver that is no dict raises,
        // archive mode on or not.
        traceback(case(
            "clusters --health, an archiver that is a list",
            &["clusters", "--health"],
            &[],
            vec![
                ok(CLUSTERS_STATUS),
                ok(r#"{"clusters":[{"label":"x","health":{"connections":1,"archiver":[1]}}]}"#),
            ],
            owner_and_port,
            1,
            2,
        )),
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
        case(
            "home --json",
            &["home", "--json"],
            &[],
            vec![ok(r#"{"ok":false,"homeDb":{}}"#)],
            owner_and_port,
            3,
            1,
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
        case(
            "run, no command",
            &["run"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "upgrade, a bad --to",
            &["upgrade", "d", "--to", "9"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster, no op",
            &["cluster"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "versions-access, a bad action",
            &["versions-access", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "create --cluster --test",
            &["create", "--cluster", "c", "--test"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
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

const VA_ON: &str = r#"{"ok":true,"agentsCanUpgradePostgres":true,"workspace":"/w/é"}"#;

const NAMED: &str = r#"{"ok":true,"cluster":{"label":"17/fb","port":5434,"unit":"pg@17-fb","state":"online","walSpool":"/var/wal é","name":"fb"}}"#;

const UP_STATUS: &str = r#"{"live":[{"db":"app","from":16,"to":17,"state":"streaming","tables":{"ready":3,"total":4},"lagBytes":5242880,"walStatus":"ok","lastError":"slow é"},
 {"db":"b","from":15,"to":16,"state":"syncing"}],
 "moves":[{"kind":"move","from":16,"to":17,"state":"succeeded","step":"done"},{"method":"live","from":15,"to":16,"state":"failed","step":"copy","lastError":"boom"},{},{"x":1}]}"#;

const UP_CHECK: &str = r#"{"check":[{"db":"app","from":16,"to":17,"pauseSecs":[2,5],"lines":[
 {"level":"ok","id":"ext","text":"extensions fine"},{"level":"refuse","id":"size","text":"too big","hint":"free space é"},{"level":"note","id":"averyverylongid","text":"y"}]},
 {"db":"b","error":"not found"}]}"#;

fn cell(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

#[test]
fn db_compute_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "cp");
    let running = || ok(r#"{"moves":[{"step":"copy","state":"running"}]}"#);
    let cases = vec![
        // versions-access: over TCP, `_compute_fail` on a refusal.
        case(
            "versions-access on",
            &["versions-access", "on"],
            &[],
            vec![ok(VA_ON)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "versions-access off --json",
            &["versions-access", "off", "--json"],
            &[],
            vec![ok(r#"{"agentsCanUpgradePostgres":false,"workspace":"w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "versions-access, the permission note",
            &["versions-access", "on"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "versions-access, not found",
            &["versions-access", "off", "--workspace", "sub"],
            &[],
            vec![answer(404, r#"{"error":"not_found","message":"no such"}"#)],
            with_sub,
            3,
            1,
        ),
        traceback(case(
            "versions-access, an answer with no switch",
            &["versions-access", "on"],
            &[],
            vec![ok(r#"{"workspace":"w"}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "versions-access, the action is the first word",
            &["versions-access", "--json", "on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "versions-access with a passport",
                &["versions-access", "on"],
                &[],
                vec![ok(VA_ON)],
                nothing,
                0,
                1,
            )
        },
        shell(case(
            "versions-access with K2_CP_MAX_TIME set",
            &["versions-access", "on"],
            &[("K2_CP_MAX_TIME", "30")],
            vec![ok(VA_ON)],
            owner_and_port,
            0,
            1,
        )),
        // Named clusters: `_db_named_post`.
        // covers: db cluster create
        case(
            "cluster create",
            &[
                "cluster",
                "create",
                "17",
                "--name",
                "fb",
                "--port",
                "5434",
                "--wal-archive",
            ],
            &[],
            vec![ok(NAMED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: db cluster create --json
        case(
            "cluster create --json",
            &["cluster", "create", "17", "--name=fb", "--json"],
            &[],
            vec![ok(NAMED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: db cluster restart
        case(
            "cluster restart",
            &["cluster", "restart", "fb"],
            &[],
            vec![ok(r#"{"ok":true,"cluster":"17/fb"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: db cluster restart --json
        case(
            "cluster restart --json",
            &["cluster", "restart", "fb", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"cluster":"17/fb"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // `cluster archive`: POST /cli/db/clusters/archive.
        // covers: db cluster archive
        case(
            "cluster archive on, restarted",
            &["cluster", "archive", "fb é", "on"],
            &[],
            vec![ok(ARCHIVE_ON)],
            owner_and_port,
            0,
            1,
        ),
        // covers: db cluster archive --json
        case(
            "cluster archive --json",
            &["cluster", "archive", "fb", "off", "--json"],
            &[],
            vec![ok(ARCHIVE_OFF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cluster archive off, the spool kept",
            &["cluster", "archive", "fb", "off"],
            &[],
            vec![ok(ARCHIVE_OFF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cluster archive on, a stopped cluster",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![ok(
                r#"{"cluster":"17/fb","walArchive":true,"changed":true,"restarted":false,"walSpool":"/s"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cluster archive, already in that state",
            &["cluster", "archive", "fb", "off"],
            &[],
            vec![ok(
                r#"{"cluster":"17/fb","walArchive":false,"changed":false}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "cluster archive, an empty answer",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "cluster archive, an answer that is a list",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        case(
            "cluster archive, no name",
            &["cluster", "archive"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster archive, a flag for a name",
            &["cluster", "archive", "--json", "on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster archive, no state",
            &["cluster", "archive", "fb"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster archive, a state that is neither",
            &["cluster", "archive", "fb", "yes"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster archive, a word after the state",
            &["cluster", "archive", "fb", "on", "now"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster archive, a helper below 18",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![answer(
                409,
                r#"{"error":{"code":"helper_needed","hint":"update the root helper"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "cluster archive, refused as usage",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![answer(400, r#"{"error":{"code":"usage","hint":"bad"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "cluster archive, not found",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![answer(
                404,
                r#"{"error":{"code":"unknown_cluster","hint":"none"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "cluster archive, the permission note",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "cluster archive, closed with nothing",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        cell(case(
            "cluster archive on the cell socket",
            &["cluster", "archive", "fb", "on"],
            &[],
            vec![ok(ARCHIVE_ON)],
            nothing,
            0,
            1,
        )),
        case(
            "cluster create, a bad port",
            &["cluster", "create", "17", "--name", "fb", "--port", "5450"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster create --name at the end",
            &["cluster", "create", "17", "--name"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cluster create, refused as usage",
            &["cluster", "create", "17", "--name", "fb"],
            &[],
            vec![answer(400, r#"{"error":{"code":"usage","hint":"bad é"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "cluster restart, not found",
            &["cluster", "restart", "fb"],
            &[],
            vec![answer(404, r#"{"error":"no_cluster","hint":"none"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "cluster restart, the permission note",
            &["cluster", "restart", "fb"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "cluster restart, a refusal that is not JSON",
            &["cluster", "restart", "fb"],
            &[],
            vec![answer(500, "oops")],
            owner_and_port,
            3,
            1,
        ),
        case(
            "cluster restart, an empty refusal",
            &["cluster", "restart", "fb"],
            &[],
            vec![answer(503, "")],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "cluster restart, a refusal that is a list",
            &["cluster", "restart", "fb"],
            &[],
            vec![answer(500, "[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "cluster create, a cluster that is a word",
            &["cluster", "create", "17", "--name", "fb"],
            &[],
            vec![ok(r#"{"cluster":"x"}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "cluster restart, closed with nothing",
            &["cluster", "restart", "fb"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        cell(case(
            "cluster create on the cell socket",
            &["cluster", "create", "17", "--name", "fb"],
            &[],
            vec![ok(NAMED)],
            nothing,
            0,
            1,
        )),
        waited(cell(case(
            "cluster restart on the cell socket, closed with nothing",
            &["cluster", "restart", "fb"],
            &[],
            vec![Reply::Close],
            nothing,
            1,
            1,
        ))),
        case(
            "create --cluster",
            &[
                "create",
                "--cluster",
                "17/fb",
                "--name",
                "app",
                "--id",
                "i1",
            ],
            &[],
            vec![ok(
                r#"{"ok":true,"name":"app","cluster":"17/fb","existing":true,"dsn":"postgres://x","dsnHint":"run dsn --reveal"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "create --cluster --json",
            &["create", "--cluster=17/fb", "--major", "17", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"name":"x"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // upgrade.
        case(
            "upgrade --status",
            &["upgrade", "app é", "--status"],
            &[],
            vec![ok(UP_STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --status --json",
            &["upgrade", "app", "--status", "--json"],
            &[],
            vec![ok(UP_STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --status, nothing yet",
            &["upgrade", "app", "--status"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --status, the permission note",
            &["upgrade", "app", "--status"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "upgrade --status, a lag that is a float",
            &["upgrade", "app", "--status"],
            &[],
            vec![ok(
                r#"{"live":[{"db":"a","from":1,"to":2,"state":"x","lagBytes":1.5}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "upgrade --status, nothing listening",
            &["upgrade", "app", "--status"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            3,
            0,
        ),
        // covers: db upgrade
        case(
            "upgrade --check, refused lines",
            &["upgrade", "app", "--to", "17", "--check"],
            &[],
            vec![ok(UP_CHECK)],
            owner_and_port,
            3,
            1,
        ),
        // covers: db upgrade --json
        case(
            "upgrade --check --json",
            &["upgrade", "app", "--to=17", "--check", "--json"],
            &[],
            vec![ok(UP_CHECK)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "upgrade --check, no pause",
            &["upgrade", "app", "--check"],
            &[],
            vec![ok(r#"{"check":[{"db":"a","from":1,"to":2,"lines":[]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "upgrade --rollback, no check in the answer",
            &["upgrade", "app", "--rollback", "--yes"],
            &[],
            vec![ok(
                r#"{"ok":true,"rolledBack":"app","n":[1,"é",{"a":null}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --live",
            &["upgrade", "app", "--live", "--to", "17", "--app", "web"],
            &[],
            vec![ok(
                r#"{"live":{"db":"app","from":16,"to":17,"state":"syncing"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --abort",
            &["upgrade", "app", "--abort"],
            &[],
            vec![ok(
                r#"{"live":{"db":"app","from":16,"to":17,"state":"aborted","lastError":"stopped"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --prepare-live",
            &["upgrade", "--prepare-live", "16", "--all", "--now"],
            &[],
            vec![ok(r#"{"major":16,"walLevel":"logical","restarted":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade --no-wait",
            &[
                "upgrade",
                "app",
                "--no-wait",
                "--force-now",
                "--allow-adopted",
            ],
            &[],
            vec![ok(r#"{"started":["app"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade, watched until it succeeds",
            &["upgrade", "app", "--to", "17"],
            &[],
            vec![
                ok(r#"{"started":["app"]}"#),
                running(),
                answer(500, "{}"),
                ok(
                    r#"{"moves":[{"step":"done","state":"succeeded","warnings":["w1"],"diff":["d1"]}]}"#,
                ),
            ],
            owner_and_port,
            0,
            4,
        ),
        case(
            "upgrade --switch --json, the name from the answer, rolled back",
            &["upgrade", "--switch", "--json"],
            &[],
            vec![
                ok(r#"{"switching":"app é"}"#),
                ok(r#"{"moves":[{"step":"switch","state":"rolled_back","lastError":"x"}]}"#),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "upgrade, watched until it fails",
            &["upgrade", "app"],
            &[],
            vec![
                ok(r#"{"started":["app"]}"#),
                ok(
                    r#"{"moves":[{"step":"copy","state":"failed","lastError":"disk full","warnings":["w"]}]}"#,
                ),
            ],
            owner_and_port,
            1,
            2,
        ),
        traceback(case(
            "upgrade, no name in the answer",
            &["upgrade", "--to", "17"],
            &[],
            vec![ok(r#"{"started":[]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "upgrade, refused as usage",
            &["upgrade", "app"],
            &[],
            vec![answer(400, r#"{"error":"bad_major","hint":"no é"}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "upgrade, a refusal that is not JSON",
            &["upgrade", "app"],
            &[],
            vec![answer(502, "bad gateway")],
            owner_and_port,
            3,
            1,
        ),
        case(
            "upgrade, one database",
            &["upgrade", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "upgrade --status, no database",
            &["upgrade", "--status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // `_compute_json` reads only its arguments: a `K2_CJ_` variable in
        // the environment never joins the body, and the binary answers.
        case(
            "a K2_CJ_ variable: not in the body",
            &["cluster", "restart", "fb"],
            &[("K2_CJ_extra", "1")],
            vec![ok(r#"{"ok":true,"cluster":"17/fb"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // Help never acts.
        shell(case(
            "versions-access on -h",
            &["versions-access", "on", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "cluster create --help",
            &["cluster", "create", "17", "--name", "n", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "cluster restart -h",
            &["cluster", "restart", "fb", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "cluster archive on -h",
            &["cluster", "archive", "fb", "on", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "cluster archive --help",
            &["cluster", "archive", "--help", "fb", "off"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "create --cluster -h",
            &["create", "--cluster", "c", "--name", "n", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "upgrade --help",
            &["upgrade", "app", "--to", "17", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "upgrade --rollback -h",
            &["upgrade", "app", "--rollback", "--yes", "-h"],
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

const BROWSE: &str = r#"{"ok":true,"databases":[{"id":"d1","name":"ws_o_jobs","owner":"ops","apps":["jobs"],"yours":null},{"id":"d2","name":"ws_o_notes é","owner":"ops","apps":[],"yours":"read"},{"id":"d3","name":"a_database_name_longer_than_thirty","owner":null,"apps":["a","b"],"yours":"write"},{"id":"d4"}],"hint":"names only: no DSN"}"#;

const CARD_UP: &str = r#"{"id":"pr_1","status":"pending","action":"db.read","text":"Let reader read the ws_o_jobs database (owned by ops, App jobs).","approvers":"owner or admin","routedTo":[{"login":"ada","role":"admin"},{"role":"owner"},{"login":"bo"}],"addr":"reader","next":"check once: k2 db requests"}"#;

const CARD_WAITING: &str = r#"{"id":"pr_1","status":"already_pending","text":"","approvers":"owner or admin","routedTo":[]}"#;

const DRY_RUN: &str = r#"{"status":"dry_run","action":"db.read","text":"Let reader read the ws_o_jobs database (owned by ops, App jobs).","routedTo":[{"login":"ada","role":"admin"}]}"#;

const READ_ONLY: &str = r#"{"ok":true,"status":"read_only","readOnly":true,"name":"ws_o_notes","dsn":"postgres://ws_r_agent@127.0.0.1:5432/ws_o_notes","dsnHasPassword":false,"dsnHint":"no password shown"}"#;

const REQUESTS: &str = r#"{"requests":[{"id":"pr_1","action":"db.read","params":{"db":"ws_o_jobs"},"state":"approved","message":"reader can read ws_o_jobs now"},{"id":"pr_2","params":null,"state":"pending","reason":"verify é"},{"id":"pr_3","params":{"db":null},"state":"denied"}]}"#;

/// Thousands of databases, above 256 KiB.
fn many_browsed(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"d{i}","name":"ws_owner_{i}_{}","owner":"workspace_{}","apps":["app_{i}","é"],"yours":{}}}"#,
                "x".repeat(i % 40),
                i % 7,
                if i % 2 == 0 { "null" } else { r#""read""# }
            )
        })
        .collect();
    format!(r#"{{"ok":true,"databases":[{}]}}"#, rows.join(","))
}

/// q17's `db browse` (and `list --all`), `db request-read` and `db
/// requests`: another workspace's database, read-only, on a card.
#[test]
fn db_reads_another_workspace_s_database() {
    let rig = Rig::new(VERB, "dbread");
    let big = many_browsed(3000);
    assert!(
        big.len() > 256 * 1024,
        "the large answer is {} bytes",
        big.len()
    );
    let cases = vec![
        // browse.
        // covers: db browse
        ok_owner("browse", &["browse"], vec![ok(BROWSE)]),
        // covers: db browse --json
        ok_owner(
            "browse --app --json",
            &["browse", "--app", "jobs", "--json"],
            vec![ok(BROWSE)],
        ),
        ok_owner(
            "browse --app= with a space and an accent",
            &["browse", "--app=a b/é"],
            vec![ok(r#"{"databases":[]}"#)],
        ),
        ok_owner("list --all is browse", &["list", "--all"], vec![ok(BROWSE)]),
        ok_owner(
            "list --json --all",
            &["list", "--json", "--all"],
            vec![ok(BROWSE)],
        ),
        case(
            "browse --workspace a folder",
            &["browse", "--workspace", "sub"],
            &[],
            vec![ok(BROWSE)],
            with_sub,
            0,
            1,
        ),
        ok_owner(
            "browse, thousands of databases",
            &["browse"],
            vec![ok(&big)],
        ),
        ok_owner(
            "browse --json, thousands of databases",
            &["browse", "--json"],
            vec![ok(&big)],
        ),
        case(
            "browse, an unknown App: exit 4",
            &["browse", "--app", "nope"],
            &[],
            vec![answer(
                404,
                r#"{"ok":false,"error":{"code":"app_not_found","hint":"this server has no App nope"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "browse, a 400 whose error is a word: exit 2",
            &["browse"],
            &[],
            vec![answer(400, r#"{"error":"bad_app","hint":"h é"}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "browse, an error with a message only",
            &["browse"],
            &[],
            vec![answer(403, r#"{"error":null,"message":"nope"}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "browse, an answer that is no JSON",
            &["browse"],
            &[],
            vec![answer(502, "<html>bad gateway</html>")],
            owner_and_port,
            3,
            1,
        ),
        case(
            "browse, a list for an answer",
            &["browse"],
            &[],
            vec![answer(500, "[1]")],
            owner_and_port,
            3,
            1,
        ),
        case(
            "browse, the permission note",
            &["browse"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "browse, closed with nothing",
            &["browse"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "browse, rows that are a number",
            &["browse"],
            &[],
            vec![ok(r#"{"databases":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "browse, an App that is no word",
            &["browse"],
            &[],
            vec![ok(r#"{"databases":[{"name":"a","apps":["x"]},{"name":"b","apps":[1]}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "browse, an answer that is no JSON",
            &["browse"],
            &[],
            vec![ok("not json")],
            owner_and_port,
            1,
            1,
        )),
        case("browse --app with no value", &["browse", "--app"], &[], vec![], owner_and_port, 2, 0),
        case(
            "browse --app then a flag",
            &["browse", "--app", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("browse, a stray word", &["browse", "x"], &[], vec![], owner_and_port, 2, 0),
        case(
            "list --all, a stray flag",
            &["list", "--all", "--frob"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // request-read.
        // covers: db request-read
        ok_owner(
            "request-read: the card is up",
            &[
                "request-read",
                "--db",
                "ws_o_jobs",
                "--why",
                "verify the import",
                "--assign",
                "ada",
            ],
            vec![ok(CARD_UP)],
        ),
        // covers: db request-read --json
        ok_owner(
            "request-read --json: the card is up",
            &["request-read", "--json", "--db", "ws_o_jobs"],
            vec![ok(CARD_UP)],
        ),
        ok_owner(
            "request-read --app --dry-run",
            &["request-read", "--app", "jobs", "--dry-run"],
            vec![ok(DRY_RUN)],
        ),
        ok_owner(
            "request-read, a dry run nobody is routed to",
            &["request-read", "--app=jobs", "--dry-run"],
            vec![ok(r#"{"status":"dry_run","text":null}"#)],
        ),
        ok_owner(
            "request-read, the card already waiting",
            &["request-read", "--to=ada", "--app=jobs", "--why=é"],
            vec![ok(CARD_WAITING)],
        ),
        ok_owner(
            "request-read, already read",
            &["request-read", "--db=ws_o_jobs"],
            vec![ok(r#"{"status":"already_done","text":"you already read ws_o_jobs"}"#)],
        ),
        ok_owner(
            "request-read, an answer with no text",
            &["request-read", "--db", "d1"],
            vec![ok(r#"{"status":"done","db":"é"}"#)],
        ),
        ok_owner(
            "request-read, read-only",
            &["request-read", "--db", "d2"],
            vec![ok(READ_ONLY)],
        ),
        case(
            "request-read, not found: exit 4",
            &["request-read", "--db", "missing"],
            &[],
            vec![answer(
                404,
                r#"{"ok":false,"error":{"code":"not_found","hint":"this server has no database missing"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "request-read, your own: exit 3",
            &["request-read", "--db", "mine"],
            &[],
            vec![answer(
                409,
                r#"{"ok":false,"error":{"code":"db_read_own","hint":"you own it"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "request-read, cards off: exit 3",
            &["request-read", "--db", "off"],
            &[],
            vec![answer(
                403,
                r#"{"ok":false,"error":{"code":"permission_cards_off","hint":"an owner or admin turns on \"Let this agent ask for permission\""}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "request-read, a 400: exit 2",
            &["request-read", "--db", "x"],
            &[],
            vec![answer(400, r#"{"error":{"code":"usage"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "request-read, the permission note",
            &["request-read", "--db", "x"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "request-read, closed with nothing",
            &["request-read", "--db", "x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "request-read, an answer that is no JSON",
            &["request-read", "--db", "x"],
            &[],
            vec![ok("not json")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "request-read --json, an answer that is a list",
            &["request-read", "--db", "x", "--json"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "request-read, an addr that is a number",
            &["request-read", "--db", "x"],
            &[],
            vec![ok(r#"{"id":"p","status":"pending","addr":7}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "request-read, a login that is a number",
            &["request-read", "--db", "x"],
            &[],
            vec![ok(r#"{"id":"p","status":"pending","routedTo":[{"login":"a"},{"login":2}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "request-read, a routed row that is a word",
            &["request-read", "--db", "x", "--dry-run"],
            &[],
            vec![ok(r#"{"status":"dry_run","routedTo":["ada"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        Case {
            env: with_passport(&[]),
            ..case(
                "request-read with a passport",
                &["request-read", "--db", "ws_o_jobs"],
                &[],
                vec![ok(CARD_UP)],
                nothing,
                0,
                1,
            )
        },
        cell(case(
            "request-read on the cell socket",
            &["request-read", "--db", "ws_o_jobs"],
            &[],
            vec![ok(CARD_UP)],
            nothing,
            0,
            1,
        )),
        waited(cell(case(
            "request-read on the cell socket, closed with nothing",
            &["request-read", "--db", "ws_o_jobs"],
            &[],
            vec![Reply::Close],
            nothing,
            1,
            1,
        ))),
        case(
            "request-read with K2_CP_MAX_TIME set",
            &["request-read", "--db", "ws_o_jobs"],
            &[("K2_CP_MAX_TIME", "30")],
            vec![ok(CARD_UP)],
            owner_and_port,
            0,
            1,
        ),
        shell(case(
            "request-read with a K2_CP_MAX_TIME curl reads its own way",
            &["request-read", "--db", "ws_o_jobs"],
            &[("K2_CP_MAX_TIME", "1e3")],
            vec![ok(CARD_UP)],
            owner_and_port,
            0,
            1,
        )),
        case("request-read alone", &["request-read"], &[], vec![], owner_and_port, 2, 0),
        case(
            "request-read --db with no value",
            &["request-read", "--db"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "request-read --why with no value",
            &["request-read", "--why"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "request-read --db then a flag",
            &["request-read", "--db", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "request-read, a stray flag",
            &["request-read", "--db", "x", "--frob"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "request-read --why alone",
            &["request-read", "--why", "w", "--db="],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // requests.
        // covers: db requests
        ok_owner("requests", &["requests"], vec![ok(REQUESTS)]),
        // covers: db requests --json
        ok_owner("requests --json", &["requests", "--json"], vec![ok(REQUESTS)]),
        ok_owner(
            "requests, none asked",
            &["requests"],
            vec![ok(r#"{"requests":[]}"#)],
        ),
        case(
            "requests, cards off: exit 3",
            &["requests"],
            &[],
            vec![answer(404, "")],
            owner_and_port,
            3,
            1,
        ),
        case(
            "requests, cards off with the permission note",
            &["requests", "--json"],
            &[],
            vec![answer(404, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "requests, a refusal",
            &["requests"],
            &[],
            vec![answer(500, r#"{"error":{"code":"boom","hint":"h"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "requests, params that are a word",
            &["requests"],
            &[],
            vec![ok(r#"{"requests":[{"id":"a","params":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case("requests, a stray word", &["requests", "extra"], &[], vec![], owner_and_port, 2, 0),
        // Help never acts.
        shell(case(
            "request-read --help",
            &["request-read", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "request-read --db x -h",
            &["request-read", "--db", "x", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "browse --help",
            &["browse", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "requests --help",
            &["requests", "--help"],
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

const DSN: &str = r#"{"ok":true,"dsn":"postgres://u:pw@127.0.0.1:5432/app"}"#;

/// A rig whose `PATH` starts with `dbkid`: it logs its argv and the
/// DSN names it was given, prints a line, and fails (98) when the DSN is
/// on its argv.
fn run_rig() -> Rig {
    use std::os::unix::fs::PermissionsExt;
    let mut rig = Rig::new(VERB, "run");
    let shim = rig.dir.join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    let log = rig.dir.join("kid.log");
    let script = format!(
        r#"#!/bin/sh
for a in "$0" "$@"; do printf 'argv=%s\n' "$a" >> '{log}'; done
printf 'DATABASE_URL=%s PG_URL=%s\n' "${{DATABASE_URL-unset}}" "${{PG_URL-unset}}" >> '{log}'
echo "kid ran with $#"
for a in "$@"; do case "$a" in *postgres://*) exit 98 ;; esac; done
exit "${{KID_RC:-0}}"
"#,
        log = log.display()
    );
    let kid = shim.join("dbkid");
    std::fs::write(&kid, script).unwrap();
    std::fs::set_permissions(&kid, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plain = shim.join("plainkid");
    std::fs::write(
        &plain,
        format!(
            "printf 'plain=%s %s\\n' \"${{DATABASE_URL-unset}}\" \"$1\" >> '{}'\necho plain kid\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
    rig.path_first = Some(shim);
    rig.tool_log = Some(log);
    rig
}

#[test]
fn db_run_answers_as_the_shell_does() {
    let rig = run_rig();
    let cases = vec![
        // covers: db run
        case(
            "run a command",
            &["run", "--", "dbkid", "a b", "--json"],
            &[],
            vec![ok(DSN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "run --as, --db, --test --actor migrator",
            &[
                "run", "--db", "app", "--test", "--actor", "migrator", "--as", "PG_URL", "--",
                "dbkid",
            ],
            &[],
            vec![ok(DSN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "run, the command's own exit",
            &["run", "--db=app", "--", "dbkid", "x"],
            &[("KID_RC", "7")],
            vec![ok(DSN)],
            owner_and_port,
            7,
            1,
        ),
        case(
            "run, --workspace after -- is the shell's",
            &["run", "--", "dbkid", "--workspace", "sub", "y"],
            &[],
            vec![ok(DSN)],
            with_sub,
            0,
            1,
        ),
        case(
            "run, no DSN in the answer",
            &["run", "--", "dbkid"],
            &[],
            vec![ok(r#"{"ok":true,"dsn":""}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "run, the permission note",
            &["run", "--", "dbkid"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "run, an answer that is a list",
            &["run", "--", "dbkid"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        case(
            "run, no answer",
            &["run", "--", "dbkid"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "run, no --",
            &["run", "dbkid"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "run, a bad --as",
            &["run", "--as", "pg-url", "--", "dbkid"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "run, a bad --actor",
            &["run", "--actor", "x", "--", "dbkid"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "run --as at the end",
            &["run", "--as"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // A file execve won't run (no `#!`): bash runs it itself, so the
        // binary hands the line to the shell.
        shell(case(
            "run, a script with no #! line",
            &["run", "--", "plainkid", "z"],
            &[],
            vec![ok(DSN)],
            owner_and_port,
            0,
            1,
        )),
        // A -h after the bare -- is the command's (the shell's help scans
        // stop at it): the DSN is asked and the command runs. The front
        // still hands the line to the shell (a help word on the line).
        shell(case(
            "run -- dbkid -h: the command's -h",
            &["run", "--", "dbkid", "-h"],
            &[],
            vec![ok(DSN)],
            owner_and_port,
            0,
            1,
        )),
    ];
    for c in &cases {
        rig.check(c);
    }
}
