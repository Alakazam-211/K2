use super::py::{line, Ask, Verb};
use super::*;

fn source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/db-store.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `_db_py`'s script as python3 reads it from stdin: line 1 is the one
/// after `<<'PYEOF'`.
fn script() -> Vec<String> {
    let text = source();
    let body = text
        .split_once("python3 - <<'PYEOF'\n")
        .expect("_db_py's heredoc")
        .1;
    let end = body.find("\nPYEOF\n").expect("the heredoc's end");
    body[..end].split('\n').map(str::to_string).collect()
}

#[test]
fn the_lines_that_raise_are_the_shell_s() {
    let s = script();
    let at = |n: u32| s[n as usize - 1].trim().to_string();
    let pins: &[(u32, &str)] = &[
        (line::DIE_IN, "if code in (\"forbidden\", \"owner_only\")"),
        (line::DIE_ADD, "hint = (hint + \" — \" if hint else \"\")"),
        (line::REQUEST_HTTP_READ, "raw = e.read().decode("),
        (
            line::REQUEST_DIE,
            "die({\"code\": \"error\", \"hint\": \"daemon unreachable",
        ),
        (
            line::PUT_JSON,
            "die({\"code\": \"usage\", \"hint\": \"--json body must be JSON\"})",
        ),
        (line::NOTE, "note_permission(d)"),
        (line::DIE_OK_FALSE, "die(d[\"error\"])"),
        (line::NOTE_STATUS, "note_permission(d)"),
        (line::DIE_STATUS_ERROR, "die(d[\"error\"])"),
        (
            line::DIE_STATUS,
            "die({\"code\": \"error\", \"hint\": raw[:400]",
        ),
        (
            line::STATUS,
            "print(\"supported: %s\" % d.get(\"supported\"))",
        ),
        (line::STATUS_SRC, "print(\"new dbs:   Postgres %s%s\""),
        (line::STATUS_CLUSTERS, "for c in d.get(\"clusters\") or []:"),
        (line::STATUS_CLUSTER, "print(\"cluster:   Postgres"),
        (
            line::STATUS_UPGRADES,
            "for u in d.get(\"upgradeAvailable\") or []:",
        ),
        (line::STATUS_UPGRADE, "if u.get(\"check\"):"),
        (
            line::STATUS_UPGRADE_CHECK,
            "print(\"upgrade:   %s on %s → %s available: %s\"",
        ),
        (
            line::STATUS_UPGRADE_LATER,
            "print(\"upgrade:   %s on %s → %s available, once",
        ),
        (line::STATUS_FIRST, "if isinstance(blocked, dict)"),
        (line::STATUS_STALE, "for s in d.get(\"pgStale\") or []:"),
        (
            line::STATUS_STALE_LINE,
            "s.get(\"name\"), s.get(\"package\")",
        ),
        (line::CLUSTERS, "cl = d.get(\"clusters\") or []"),
        (line::CLUSTERS_HEAD, "print(\"%s cluster(s) · new databases"),
        (line::CLUSTERS_FOR, "for c in cl:"),
        (line::CLUSTERS_LABEL, "label = c.get(\"label\")"),
        (
            line::CLUSTERS_NAMED_N,
            "n = len([x for x in d.get(\"databases\") or [] if x.get(\"cluster\") == label])",
        ),
        (
            line::CLUSTERS_MAIN_N,
            "n = len([x for x in d.get(\"databases\") or [] if x.get(\"major\")",
        ),
        (line::CLUSTERS_ROLE, "if c.get(\"role\") == \"foreign\":"),
        (line::CLUSTERS_ROW, "print(\"  %-14s port %s"),
        (line::CLUSTERS_HEALTH, "h = c.get(\"health\")"),
        (line::CLUSTERS_SIZES, "sizes = \", \".join("),
        (line::CLUSTERS_ARCHIVER, "a = h.get(\"archiver\") or {}"),
        (
            line::CLUSTERS_CONNECTIONS,
            "print(\"      %s/%s connections",
        ),
        (line::CLUSTERS_WAL, "print(\"      WAL archive:"),
        (
            line::CLUSTERS_UPGRADES,
            "for u in d.get(\"upgradeAvailable\") or []:",
        ),
        (line::CLUSTERS_UPGRADE, "print(\"  upgrade available:"),
        (
            line::DEFAULT_MAJOR,
            "print(\"new databases go on Postgres %s\"",
        ),
        (line::STATE, "print(\"state: %s\" % d.get(\"state\"))"),
        (line::CREATE, "print(\"created %s%s\""),
        (line::CREATE_KEPT, "if kept.get(\"ref\"):"),
        (line::CREATE_CAP, "print(\"cap %s/%s used\""),
        (line::CREATE_APPS, "for a in d.get(\"apps\") or []:"),
        (
            line::CREATE_APP,
            "print(\"app:      %s\" % a.get(\"note\"))",
        ),
        (line::DSN, "if d.get(\"ref\"):"),
        (line::LIST, "dbs = d.get(\"databases\") or []"),
        (line::LIST_LEN, "print(\"%s database(s)\" % len(dbs))"),
        (line::LIST_FOR, "for row in dbs:"),
        (line::LIST_ROW, "print(\"  %s  %s\" % (row.get(\"name\")"),
        (line::MIGRATE, "applied = d.get(\"applied\") or []"),
        (
            line::MIGRATE_APPLIED,
            "print(\"applied: %s\" % \", \".join(applied))",
        ),
        (
            line::MIGRATE_NOOP,
            "elif d.get(\"noop\") and discovered > 0:",
        ),
        (
            line::MIGRATE_NONE,
            "print(\"applied: %s\" % \", \".join(applied))",
        ),
        (line::DUMP, "print(\"dump: %s\""),
        (line::RESTORE, "print(\"restored %s\""),
        (line::DROP, "print(\"dropped %s\""),
        (line::DROP_APPS, "for a in d.get(\"apps\") or []:"),
        (line::DROP_APP, "print(\"app:      %s\""),
        (line::GRANT, "print(\"granted %s on %s to %s\""),
        (line::GRANT_APPS, "for a in d.get(\"apps\") or []:"),
        (line::GRANT_APP, "print(\"app:      %s\""),
        (line::REVOKE, "print(\"revoked %s from %s\""),
        (line::REVOKE_APPS, "for a in d.get(\"apps\") or []:"),
        (line::REVOKE_APP, "print(\"app:      %s\""),
        (line::BIND, "print(\"bind role: %s\""),
        (line::DOCTOR, "print(\"ok:       %s\" % d.get(\"ok\"))"),
        (line::DOCTOR_CLUSTERS, "for c in d.get(\"clusters\") or []:"),
        (line::DOCTOR_CLUSTER, "c.get(\"major\"), c.get(\"port\")"),
        (line::DOCTOR_RAM, "cur = ram.get(\"currentBytes\")"),
        (line::DOCTOR_GUCS, "if k in gucs:"),
        (line::DOCTOR_GUC, "print(\"  %s = %s\" % (k, gucs.get(k)))"),
        (line::DOCTOR_HINT, "if d.get(\"hint\"):"),
        (line::OTHER, "print(dumps(d))"),
    ];
    for (n, want) in pins {
        assert!(
            at(*n).starts_with(want),
            "line {n}: {:?}, not {want:?}",
            at(*n)
        );
    }
}

/// Each `request(...)` is called from the line the verb names.
#[test]
fn each_verb_calls_from_its_line() {
    let s = script();
    for (verb, route) in [
        (Verb::Status, "/cli/db/status"),
        (Verb::Enable, "/cli/db/server/enable"),
        (Verb::Create, "/cli/db/create"),
        (Verb::Clusters, "/cli/db/status"),
        (Verb::DefaultMajor, "/cli/db/server/default-major"),
        (Verb::Dsn, "/cli/db/dsn"),
        (Verb::Grant, "/cli/db/grant"),
        (Verb::StorePut, "/cli/store/put"),
        (Verb::StoreDrop, "/cli/store/drop"),
    ] {
        let n = verb_line(verb);
        let text = &s[n as usize - 1];
        assert!(
            text.contains("request(") && text.contains(route),
            "{verb:?}: line {n} is {text:?}"
        );
    }
}

fn verb_line(v: Verb) -> u32 {
    // The same table `Py::request`'s frames use.
    match v {
        Verb::Status => 62,
        Verb::Enable => 65,
        Verb::Create => 88,
        Verb::Clusters => 91,
        Verb::DefaultMajor => 107,
        Verb::Dsn => 126,
        Verb::Grant => 172,
        Verb::StorePut => 201,
        Verb::StoreDrop => 216,
        other => panic!("{other:?}"),
    }
}

/// The hints the shell's `cmd_db` writes, with the command's name from
/// the brand: each is in the source once the brand's name is `k2`.
#[test]
fn the_usage_hints_are_the_shell_s() {
    let text = source();
    let hints = [
        concat!(cli_name!(), " db create --test does not take --name"),
        concat!(cli_name!(), " db default-major <major> (e.g. 17)"),
        concat!(cli_name!(), " db restore <file> [--db <name>]"),
        concat!(cli_name!(), " db cap [<n>|default] [--workspace WS]"),
        concat!(cli_name!(), " db revoke <workspace> --db …"),
        concat!(cli_name!(), " db bind --role <pg_role>"),
        concat!(cli_name!(), " db home [--keep k2.db|k2so.db] [--json]"),
        concat!("--db needs a database name or id (", cli_name!(), " db list)"),
        "--ref keeps the DSN in the vault and prints its reference; --reveal prints the DSN. Pick one",
        "--relation requires schema.name or a bare name",
    ];
    for h in hints {
        let shell = h.replace(cli_name!(), "k2");
        assert!(text.contains(&shell), "{shell:?} is not in db-store.sh");
    }
}

fn form(words: &[&str]) -> Form {
    parse(words).form
}

fn hint(words: &[&str]) -> String {
    match form(words) {
        Form::Usage(h) => h,
        other => panic!("{words:?}: not a usage error: {other:?}"),
    }
}

fn ask(words: &[&str]) -> Ask {
    match form(words) {
        Form::Py(a) => *a,
        other => panic!("{words:?}: not a call: {other:?}"),
    }
}

#[test]
fn the_words_come_to_the_shell_s_forms() {
    assert_eq!(form(&[]), Form::Help);
    assert_eq!(form(&["list", "-h"]), Form::Help);
    assert_eq!(
        hint(&["help"]),
        concat!(
            "unknown db command 'help' — see ",
            cli_name!(),
            " db --help"
        )
    );
    assert_eq!(hint(&["status", "--x"]), "unexpected '--x'");
    assert_eq!(form(&["create", "--id"]), Form::Shift("--id".into()));
    assert_eq!(
        hint(&["create", "--cluster", "c", "--bad"]),
        "unexpected '--bad'"
    );
    assert_eq!(
        hint(&["run", "x"]),
        concat!(
            cli_name!(),
            " db run [--db <name>] [--test [--actor migrator]] [--as NAME] -- <command…> (unexpected 'x')"
        )
    );
    match form(&["run", "--as", "PG", "--", "psql", "-c", "select 1"]) {
        Form::Run(r) => assert_eq!((r.env_as.as_str(), r.cmd.len()), ("PG", 3)),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        hint(&["create", "--major", "9"]),
        "--major takes a Postgres major, e.g. 16 or 17"
    );
    let a = ask(&[
        "create",
        "--id",
        "i",
        "--name",
        "n",
        "--access",
        "read",
        "--major=17",
    ]);
    assert_eq!(
        (
            a.id.as_str(),
            a.name.as_str(),
            a.access.as_str(),
            a.major.as_str()
        ),
        ("i", "n", "read", "17")
    );
    assert_eq!(
        form(&["default-major"]),
        Form::Usage(concat!(cli_name!(), " db default-major <major> (e.g. 17)").into())
    );
    assert_eq!(hint(&["dsn", "--as"]), "--as needs a name");
    assert_eq!(form(&["dsn", "--actor"]), Form::Shift("--actor".into()));
    assert_eq!(
        hint(&["dsn", "--db", "-x"]),
        concat!(
            "--db needs a database name or id (",
            cli_name!(),
            " db list)"
        )
    );
    assert!(hint(&["dsn", "--ref", "--reveal"]).starts_with("--ref keeps"));
    let a = ask(&["creds", "--db=x", "--as=Y", "--test", "--actor", "migrator"]);
    assert_eq!(
        (a.db.as_str(), a.as_name.as_str(), a.test, a.actor.as_str()),
        ("x", "Y", true, "migrator")
    );
    assert_eq!(
        hint(&["restore"]),
        concat!(cli_name!(), " db restore <file> [--db <name>]")
    );
    assert_eq!(hint(&["restore", "a", "b"]), "unexpected 'b'");
    assert_eq!(form(&["cap", "5", "help"]), Form::Help);
    assert_eq!(
        form(&["cap", "007"]),
        Form::Cap {
            val: "7".into(),
            json: false
        }
    );
    assert_eq!(
        form(&["cap", "1234567"]),
        Form::Usage(concat!(cli_name!(), " db cap: at most 999999 (0 = unlimited)").into())
    );
    assert_eq!(
        hint(&["grant", "w", "--db", "d"]),
        "grant requires --level read|write"
    );
    assert_eq!(
        hint(&["grant", "w", "--db", "d", "--level", "x"]),
        "--level must be read or write"
    );
    let a = ask(&[
        "grant",
        "w",
        "--db",
        "d",
        "--level",
        "read",
        "--relation",
        "a.b",
        "--relation=c",
        "--manage",
    ]);
    assert_eq!(a.relations, vec!["a.b".to_string(), "c".to_string()]);
    assert!(a.manage);
    assert_eq!(hint(&["grant", "w", "v"]), "unexpected extra argument 'v'");
    assert_eq!(
        hint(&["bind"]),
        concat!(cli_name!(), " db bind --role <pg_role>")
    );
    assert_eq!(
        form(&["home", "--keep"]),
        Form::Usage("--keep needs k2.db or k2so.db".into())
    );
    assert_eq!(
        form(&["home", "--keep", "x"]),
        Form::Usage("--keep must be k2.db or k2so.db".into())
    );
    assert_eq!(form(&["home", "help"]), Form::Help);
    assert_eq!(
        form(&["home", "--keep=k2.db", "--json"]),
        Form::Home {
            keep: "k2.db".into(),
            json: true
        }
    );
}

#[test]
fn the_workspace_flags_come_out_in_order() {
    let l = parse(&["--workspace", "a", "list", "--project=b", "--json"]);
    assert_eq!(l.sels, vec!["a".to_string(), "b".to_string()]);
    assert!(matches!(l.form, Form::Py(ref a) if a.verb == Verb::List && a.json));
    let l = parse(&["--workspace", "a", "--project", "-x"]);
    assert_eq!(l.sels, vec!["a".to_string()]);
    assert_eq!(
        l.form,
        Form::Usage("--project requires a workspace path, handle, or uuid".into())
    );
    let l = parse(&["--workspace="]);
    assert_eq!(
        l.form,
        Form::Usage("--workspace requires a workspace path, handle, or uuid".into())
    );
    assert_eq!(parse(&["--workspace", "a"]).form, Form::Help);
}

#[test]
fn the_resolve_reader_is_the_shell_s() {
    assert_eq!(resolved(br#"{"path":"/w/a"}"#), Some("/w/a".into()));
    assert_eq!(
        resolved(br#"{"path":"","projectPath":"/p"}"#),
        Some("/p".into())
    );
    assert_eq!(resolved(br#"{"project_path":5}"#), Some("5".into()));
    assert_eq!(resolved(br#"{"path":"/x\n\n"}"#), Some("/x".into()));
    assert_eq!(resolved(b"[]"), None);
    assert_eq!(resolved(b""), None);
    assert_eq!(resolved(b"nope"), None);
}

#[test]
fn a_python_eq_is_python_s() {
    use crate::out::json::parse as p;
    let v = |t: &str| p(t).expect("json");
    assert!(crate::out::pylib::ops::py_eq(&v("1"), &v("1.0")));
    assert!(crate::out::pylib::ops::py_eq(&v("true"), &v("1")));
    assert!(!crate::out::pylib::ops::py_eq(&v("\"1\""), &v("1")));
    assert!(crate::out::pylib::ops::py_eq(
        &v(r#"{"a":1,"b":2}"#),
        &v(r#"{"b":2,"a":1}"#)
    ));
    assert!(!crate::out::pylib::ops::py_eq(&v("null"), &v("0")));
}

fn cp_ask(words: &[&str]) -> cp::Ask {
    match form(words) {
        Form::Cp(a) => *a,
        other => panic!("{words:?}: not a compute call: {other:?}"),
    }
}

#[test]
fn the_compute_verbs_words_are_the_shell_s() {
    use cp::{Ask as C, What};
    match cp_ask(&[
        "create",
        "--cluster=c",
        "--json",
        "--name",
        "n",
        "--major",
        "17",
    ]) {
        C::Named(k) => {
            assert_eq!((k.what, k.json), (What::CreateDb, true));
            assert_eq!(
                (k.cluster.as_str(), k.name.as_str(), k.major.as_str()),
                ("c", "n", "17")
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        hint(&["create", "--cluster", "c", "--test"]),
        concat!(
            cli_name!(),
            " db create --cluster takes --name, --id and --major only (no --test, no --db-access)"
        )
    );
    assert_eq!(
        hint(&["create", "--cluster", "c", "--major", "9"]),
        "--major takes a Postgres major, e.g. 16 or 17"
    );
    assert_eq!(
        cp_ask(&["versions-access", "on", "--json"]),
        C::VersionsAccess {
            on: true,
            json: true
        }
    );
    // The action is the first word, whatever it is.
    assert_eq!(
        hint(&["versions-access", "--json", "on"]),
        concat!(
            cli_name!(),
            " db versions-access on|off [--workspace WS] [--json]"
        )
    );
    assert_eq!(
        hint(&["cluster", "create", "17", "--name", "n", "--port", "5450"]),
        "--port takes a port in 5433-5449"
    );
    assert_eq!(form(&["cluster", "create", "17", "--name"]), Form::Shift("--name".into()));
    assert_eq!(
        hint(&["cluster", "create", "17", "--x"]),
        concat!(
            "unexpected '--x' — ",
            cli_name!(),
            " db cluster create <major> --name <name> [--port <port>] [--wal-archive]"
        )
    );
    assert_eq!(
        hint(&["cluster", "restart"]),
        concat!(cli_name!(), " db cluster restart <name>")
    );
    match cp_ask(&["cluster", "restart", "--json"]) {
        C::Named(k) => assert_eq!((k.name.as_str(), k.json), ("--json", false)),
        other => panic!("{other:?}"),
    }
    assert_eq!(hint(&["upgrade", "a", "b"]), "one database");
    assert_eq!(
        hint(&["upgrade", "--to", "9"]),
        "--to takes a Postgres major, e.g. 17"
    );
    assert_eq!(
        hint(&["upgrade", "--status"]),
        concat!(cli_name!(), " db upgrade <db> --status")
    );
    assert_eq!(form(&["upgrade", "d", "--app"]), Form::Shift("--app".into()));
    match cp_ask(&["upgrade", "d", "--to=17", "--live", "--no-wait"]) {
        C::Upgrade(u) => assert!(u.live && u.no_wait && u.to == "17" && u.db == "d"),
        other => panic!("{other:?}"),
    }
}
