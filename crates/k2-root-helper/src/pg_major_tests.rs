//! `pg` protocol 9 (several majors) over the temp root and the fakes of
//! `tests.rs`. A small fake box answers `pg_lsclusters`, `ss`, apt, dpkg,
//! `pg_createcluster`, `systemctl` and the postgres-side programs (`psql`,
//! `pg_dump`, `pg_restore`, `stat`, `rm`). Nothing real runs.

use super::*;
use serde_json::json;

use k2_root_proto::pg_major::{
    AuthRow, APT_MARK, CREATECLUSTER_DROPIN, DPKG_QUERY, ENV as ENV_BIN, PGDG_COMMON_KEY, PGDG_KEYRING, PGDG_KEY_ASC,
    PGDG_PREF, PGDG_SOURCES, PG_CREATECLUSTER, PG_LSCLUSTERS, SEW_TOML, SLICE_UNIT_PATH, SS, UNATTENDED_DROPIN,
};

const APT: &str = "/usr/bin/apt-get";
const SCRAM_A: &str = "SCRAM-SHA-256$4096:c2FsdEE=$a2V5QQ==:c2VyQQ==";
const SCRAM_B: &str = "SCRAM-SHA-256$4096:c2FsdEI=$a2V5Qg==:c2VyQg==";

#[derive(Default)]
struct World {
    /// (major, port, online)
    clusters: Vec<(u32, u16, bool)>,
    listening: Vec<u16>,
    /// port → pg_authid rows
    roles: HashMap<u16, Vec<AuthRow>>,
    /// port → databases
    dbs: HashMap<u16, Vec<String>>,
    /// postgres-owned files (dumps)
    files: HashSet<String>,
    /// every SQL text psql got: (port, sql)
    sql: Vec<(u16, String)>,
    /// `apt-get install -s` stdout
    sim: String,
    /// `apt-get install -s` exit + stderr (unreachable PGDG)
    sim_fail: Option<String>,
    /// was the createcluster drop-in on disk when `install -y` ran?
    dropin_before_install: Option<bool>,
    dpkg: Vec<String>,
    /// (port, db) → owner (default `<db>_migrator`)
    owners: HashMap<(u16, String), String>,
    /// (port, db) still accepting connections (drop-db refuses)
    open_dbs: HashSet<(u16, String)>,
    /// relations already in a restore target
    target_rels: u64,
    /// relforcerowsecurity oids, comma-separated
    forced: String,
    /// major → pg_dump/pg_restore minor
    tool_minor: HashMap<String, String>,
    moves_dir_stat: String,
    passfiles: Vec<(String, String)>,
    restore_fail: Option<String>,
    fail_create_role: bool,
    /// every program run as postgres: "ENV=… prog args"
    postgres_calls: Vec<String>,
    /// package → version (default 1.0); `bump_on_install` changes one at install
    vers: HashMap<String, String>,
    bump_on_install: Option<(String, String)>,
    /// port → `server_version_num` (default 160015 on 5432, 170011 else)
    server_num: HashMap<u16, String>,
    /// the restore target's owner is a superuser
    super_owner: bool,
    /// pgdual tests: psql answers by needle (checked first, in order)
    answers: Vec<(String, String)>,
    /// pgdual tests: psql fails (exit 3, this stderr) on SQL with the needle
    fail_on: Option<(String, String)>,
}

type W = Rc<RefCell<World>>;

fn out(status: i32, stdout: &str) -> ExecOut {
    ExecOut { status, stdout: stdout.as_bytes().to_vec(), stderr: vec![] }
}

fn touch(root: &Path, abs: &str) {
    let p = under(root, abs);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"").unwrap();
}

fn bins(root: &Path, m: u32) {
    for b in ["postgres", "pg_dump", "pg_restore", "psql"] {
        touch(root, &format!("/usr/lib/postgresql/{m}/bin/{b}"));
    }
}

fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn role(name: &str, v: Option<&str>) -> AuthRow {
    AuthRow {
        rolname: name.into(),
        rolsuper: false,
        rolinherit: false,
        rolcreaterole: false,
        rolcreatedb: false,
        rolcanlogin: v.is_some(),
        rolreplication: false,
        rolbypassrls: false,
        rolconnlimit: -1,
        rolpassword: v.map(str::to_string),
    }
}

/// A noble box with distro 16 on 5432 (online), the helper's fakes wired.
fn pg_box() -> (T, W) {
    let t = box_root();
    let w: W = Rc::new(RefCell::new(World {
        clusters: vec![(16, 5432, true)],
        listening: vec![5432, 22],
        dpkg: vec!["postgresql-16".into(), "postgresql-client-16".into(), "postgresql-common".into()],
        moves_dir_stat: "directory postgres".into(),
        ..Default::default()
    }));
    let root = t.root.clone();
    mkdirs(&root, &["/etc/apt/sources.list.d", "/etc/apt/preferences.d", "/etc/postgresql/16/main/conf.d", "/var/lib/postgresql/16/main"]);
    // Like Ubuntu: /etc/os-release is a link to /usr/lib/os-release.
    mkdirs(&root, &["/usr/lib"]);
    std::fs::write(under(&root, "/usr/lib/os-release"), "NAME=\"Ubuntu\"\nVERSION_CODENAME=noble\n").unwrap();
    symlink("../usr/lib/os-release", under(&root, "/etc/os-release")).unwrap();
    std::fs::write(under(&root, "/etc/postgresql/16/main/start.conf"), "# distro\nauto\n").unwrap();
    for b in [PG_LSCLUSTERS, PG_CREATECLUSTER, SS, APT, DPKG_QUERY, APT_MARK] {
        touch(&root, b);
    }
    bins(&root, 16);
    w.borrow_mut().dbs.insert(5432, vec!["postgres".into(), "ws_jobs".into()]);

    let mut log = t.exec.0.borrow_mut();
    let ww = w.clone();
    log.programs.insert(
        PG_LSCLUSTERS.into(),
        Rc::new(move |_| {
            let s: String = ww
                .borrow()
                .clusters
                .iter()
                .map(|(m, p, on)| {
                    format!(
                        "{m}  main    {p} {} postgres /var/lib/postgresql/{m}/main /var/log/postgresql/postgresql-{m}-main.log\n",
                        if *on { "online" } else { "down" }
                    )
                })
                .collect();
            out(0, &s)
        }),
    );
    let ww = w.clone();
    log.programs.insert(
        SS.into(),
        Rc::new(move |_| {
            let s: String = ww
                .borrow()
                .listening
                .iter()
                .map(|p| format!("LISTEN 0 200 127.0.0.1:{p} 0.0.0.0:*\n"))
                .collect();
            out(0, &s)
        }),
    );
    let (ww, r) = (w.clone(), root.clone());
    log.programs.insert(
        PG_CREATECLUSTER.into(),
        Rc::new(move |a| {
            let m: u32 = a[0].parse().unwrap();
            let p: u16 = arg_after(a, "-p").unwrap().parse().unwrap();
            ww.borrow_mut().clusters.push((m, p, false));
            ww.borrow_mut().dbs.insert(p, vec!["postgres".into()]);
            mkdirs(&r, &[format!("/etc/postgresql/{m}/main/conf.d").as_str(), format!("/var/lib/postgresql/{m}/main").as_str()]);
            std::fs::write(under(&r, &format!("/etc/postgresql/{m}/main/start.conf")), "auto\n").unwrap();
            out(0, "")
        }),
    );
    let ww = w.clone();
    log.programs.insert(
        SYSTEMCTL.into(),
        Rc::new(move |a| {
            if a.first().map(String::as_str) == Some("start") {
                let unit = &a[1];
                let m: u32 = unit.trim_start_matches("postgresql@").trim_end_matches("-main").parse().unwrap();
                for c in ww.borrow_mut().clusters.iter_mut().filter(|c| c.0 == m) {
                    c.2 = true;
                }
            }
            out(0, "")
        }),
    );
    let (ww, r) = (w.clone(), root.clone());
    log.programs.insert(
        ENV_BIN.into(),
        Rc::new(move |a| {
            if a.get(1).map(String::as_str) == Some(APT) {
                return match a.get(2).map(String::as_str) {
                    Some("update") => out(0, ""),
                    Some("install") if a.iter().any(|x| x == "-s") => match ww.borrow().sim_fail.clone() {
                        Some(e) => ExecOut { status: 100, stdout: vec![], stderr: e.into_bytes() },
                        None => out(0, &ww.borrow().sim),
                    },
                    Some("install") => {
                        ww.borrow_mut().dropin_before_install = Some(under(&r, CREATECLUSTER_DROPIN).exists());
                        assert!(a.iter().any(|x| x == "--no-remove"), "the real install never removes (M1): {a:?}");
                        let bump = ww.borrow_mut().bump_on_install.take();
                        if let Some((p, v)) = bump {
                            ww.borrow_mut().vers.insert(p, v);
                        }
                        for p in a.iter().filter(|x| x.starts_with("postgresql-")) {
                            if let Some(m) = p.strip_prefix("postgresql-").and_then(|m| m.parse::<u32>().ok()) {
                                bins(&r, m);
                            }
                            ww.borrow_mut().dpkg.push(p.clone());
                        }
                        out(0, "")
                    }
                    _ => out(1, ""),
                };
            }
            out(1, "")
        }),
    );
    let ww = w.clone();
    log.programs.insert(
        DPKG_QUERY.into(),
        Rc::new(move |a| {
            let w = ww.borrow();
            let with_v = a.iter().any(|x| x.contains("${Version}"));
            let s: String = w
                .dpkg
                .iter()
                .map(|p| {
                    if with_v {
                        format!("{p} {} install ok installed\n", w.vers.get(p).cloned().unwrap_or_else(|| "1.0".into()))
                    } else {
                        format!("{p} install ok installed\n")
                    }
                })
                .collect();
            out(0, &s)
        }),
    );
    log.programs.insert(APT_MARK.into(), Rc::new(|_| out(0, "")));
    let (ww, ex) = (w.clone(), t.exec.clone());
    log.programs_in.insert(
        RUNUSER.into(),
        Rc::new(move |a, stdin| {
            assert_eq!(&a[..3], &["-u", "postgres", "--"], "everything runs as postgres: {a:?}");
            // `/usr/bin/env KEY=VAL… prog …`: the program after the assignments.
            let mut rest: Vec<String> = a[3..].to_vec();
            let mut envs: Vec<String> = Vec::new();
            if rest[0] == ENV_BIN {
                rest.remove(0);
                while rest[0].contains('=') && !rest[0].starts_with('-') {
                    envs.push(rest.remove(0));
                }
            }
            let a = &rest;
            let prog = a[0].rsplit('/').next().unwrap().to_string();
            if prog == "psql" {
                assert!(
                    envs.iter().any(|e| e == k2_root_proto::pg_major::SAFE_PGOPTIONS),
                    "every superuser psql pins search_path (F1): {envs:?} {a:?}"
                );
            }
            if prog == "pg_lsclusters" {
                let p = ex.0.borrow().programs.get(PG_LSCLUSTERS).cloned().expect("lsclusters fake");
                return p(&a[1..]);
            }
            if a.iter().any(|x| x == "--version") {
                let major = a[0].split('/').nth(4).unwrap().to_string();
                let v = ww.borrow().tool_minor.get(&major).cloned().unwrap_or_else(|| if major == "16" { "15".into() } else { "11".into() });
                return out(0, &format!("{prog} (PostgreSQL) {major}.{v} (Ubuntu)\n"));
            }
            let mut w = ww.borrow_mut();
            w.postgres_calls.push(format!("{} {}", envs.join(" "), a.join(" ")).trim().to_string());
            match prog.as_str() {
                "psql" => {
                    let port: u16 = arg_after(a, "-p").map(|p| p.parse().unwrap()).unwrap_or(5432);
                    let sql = String::from_utf8_lossy(stdin.unwrap_or_default()).to_string();
                    w.sql.push((port, sql.clone()));
                    if let Some((_, e)) = w.fail_on.as_ref().filter(|(n, _)| sql.contains(n.as_str())) {
                        return ExecOut { status: 3, stdout: vec![], stderr: e.clone().into_bytes() };
                    }
                    if let Some((_, a)) = w.answers.iter().find(|(n, _)| sql.contains(n.as_str())) {
                        return out(0, &a.clone());
                    }
                    if sql.starts_with("SHOW server_version_num") {
                        let d = if port == 5432 { "160015" } else { "170011" };
                        return out(0, &format!("{}\n", w.server_num.get(&port).cloned().unwrap_or_else(|| d.into())));
                    }
                    if sql.contains("rolsuper::text FROM pg_catalog.pg_roles") {
                        return out(0, if w.super_owner { "true\n" } else { "false\n" });
                    }
                    if sql.contains("FROM pg_authid") {
                        let rows = w.roles.get(&port).cloned().unwrap_or_default();
                        let mut want: Vec<AuthRow> = rows.into_iter().filter(|r| sql.contains(&format!("'{}'", r.rolname))).collect();
                        // ORDER BY rolname, like the real read.
                        want.sort_by(|a, b| a.rolname.cmp(&b.rolname));
                        return out(0, &serde_json::to_string(&want).unwrap());
                    }
                    if w.fail_create_role && sql.contains("CREATE ROLE") {
                        return ExecOut { status: 3, stdout: vec![], stderr: b"ERROR:  role exists\nLINE 1: ...$4096:c2FsdEE=$a2V5QQ==:c2VyQQ=='".to_vec() };
                    }
                    if let Some(rest) = sql.split("WHERE datname = '").nth(1) {
                        let db = rest.split('\'').next().unwrap().to_string();
                        let has = w.dbs.get(&port).is_some_and(|d| d.iter().any(|x| *x == db));
                        if sql.contains("pg_get_userbyid(datdba)") {
                            let o = w.owners.get(&(port, db.clone())).cloned().unwrap_or(format!("{db}_migrator"));
                            return out(0, if has { &o } else { "" });
                        }
                        if sql.contains("datallowconn OR datconnlimit") {
                            return out(0, if w.open_dbs.contains(&(port, db)) { "false" } else { "true" });
                        }
                        if sql.starts_with("SELECT count(*) FROM pg_database") {
                            return out(0, if has { "1\n" } else { "0\n" });
                        }
                        return out(0, "2\n");
                    }
                    if sql.contains("NOT LIKE 'pg\\_%'") && sql.starts_with("SELECT count(*)") {
                        return out(0, &format!("{}\n", w.target_rels));
                    }
                    if sql.contains("relforcerowsecurity") && sql.starts_with("SELECT") {
                        return out(0, &w.forced.clone());
                    }
                    if sql.contains("CREATE ROLE") {
                        for line in sql.lines().filter(|l| l.starts_with("CREATE ROLE")) {
                            let name = line.split('"').nth(1).unwrap().to_string();
                            let v = line.split("PASSWORD '").nth(1).map(|x| x.trim_end_matches("';").to_string());
                            w.roles.entry(port).or_default().push(role(&name, v.as_deref()));
                        }
                    }
                    out(0, "")
                }
                "mkdir" | "chmod" | "du" | "find" => out(0, ""),
                "dd" => {
                    let of = a.iter().find_map(|x| x.strip_prefix("of=")).unwrap().to_string();
                    w.passfiles.push((of, String::from_utf8_lossy(stdin.unwrap_or_default()).to_string()));
                    out(0, "")
                }
                "stat" => {
                    if a.iter().any(|x| x == "%F %U") {
                        return out(0, &format!("{}\n", w.moves_dir_stat));
                    }
                    let f = a.last().unwrap();
                    if w.files.contains(f) {
                        out(0, "4096\n")
                    } else {
                        out(1, "")
                    }
                }
                "pg_dump" => {
                    w.files.insert(arg_after(a, "-f").unwrap().to_string());
                    out(0, "")
                }
                "pg_restore" => {
                    if a.iter().any(|x| x == "-l") {
                        return out(0, TOC);
                    }
                    let section = a.iter().find_map(|x| x.strip_prefix("--section=")).unwrap().to_string();
                    if w.restore_fail.as_deref() == Some(section.as_str()) {
                        return ExecOut {
                            status: 1,
                            stdout: vec![],
                            stderr: b"pg_restore: error: could not execute query: ERROR:  new row violates check constraint \"c\"\nDETAIL:  Failing row contains (secret-row-value).\nCommand was: COPY public.t (a) FROM stdin;".to_vec(),
                        };
                    }
                    out(0, "")
                }
                "rm" => {
                    let f = a.last().unwrap().clone();
                    w.files.remove(&f);
                    out(0, "")
                }
                other => panic!("unexpected program as postgres: {other}"),
            }
        }),
    );
    drop(log);
    (t, w)
}

fn json(o: &crate::Outcome) -> serde_json::Value {
    ok(o);
    serde_json::from_slice(&o.stdout).unwrap()
}

/// No K2 verb ever removes a package, drops a cluster or upgrades one in
/// place (Sew 2026-10-20 (1); PRD §6.4: never automatic, never packages).
fn never_destructive(t: &T) {
    for c in t.non_query_calls() {
        for bad in [" remove", " purge", " autoremove", "dpkg -r", "dpkg --remove", "pg_dropcluster", "pg_upgradecluster", " dist-upgrade", "apt-get upgrade"] {
            assert!(!c.contains(bad), "{c}");
        }
    }
}

// ── clusters ────────────────────────────────────────────────────────────

/// Sew 2026-10-20 (2): Sew's private Postgres is never a K2 cluster, even
/// when its data dir and unit are on the box (and even if something
/// registered it with postgresql-common).
#[test]
fn clusters_lists_k2_layout_and_never_sews_db() {
    let (t, w) = pg_box();
    mkdirs(&t.root, &["/var/lib/sew/db", "/etc/sew"]);
    std::fs::write(t.p("/etc/systemd/system/sew-db.service"), "[Service]\nExecStart=/usr/lib/postgresql/16/bin/postgres -c config_file=/etc/sew/sew-db.conf\n").unwrap();
    std::fs::write(t.p(SEW_TOML), "[db]\npg_major = 16\nport = 5441\n").unwrap();
    w.borrow_mut().listening.push(5433);
    // A row pointing at Sew's data dir (someone ran pg_createcluster on it).
    t.exec.0.borrow_mut().programs.insert(
        PG_LSCLUSTERS.into(),
        Rc::new(|_| {
            out(
                0,
                "16  main    5432 online postgres /var/lib/postgresql/16/main /var/log/x.log\n\
                 16  sew     5499 online sew-db   /var/lib/sew/db /var/log/y.log\n",
            )
        }),
    );
    // Open: an agent may read it.
    t.as_agent();
    let v = json(&t.call(&["pg", "clusters"]));
    let cl = v["clusters"].as_array().unwrap();
    assert_eq!(cl.len(), 1, "{v}");
    assert_eq!(cl[0]["major"], 16);
    assert_eq!(cl[0]["port"], 5432);
    assert_eq!(cl[0]["online"], true);
    assert_eq!(cl[0]["k2Layout"], true);
    assert_eq!(cl[0]["startConf"], "auto");
    assert_eq!(cl[0]["unit"], "postgresql@16-main.service");
    assert_eq!(cl[0]["running"], true, "the daemon's parse_helper_cluster reads `running`");
    assert!(!v.to_string().contains("/var/lib/sew"), "{v}");
    assert_eq!(v["installed"], json!([16]));
    assert_eq!(v["codename"], "noble");
    assert_eq!(v["distroMajor"], 16);
    assert_eq!(v["sew"]["present"], true);
    assert_eq!(v["sew"]["pgMajor"], 16);
    assert_eq!(v["bandListening"], json!([5432, 5433, 5441]), "Sew's configured port counts as taken (L6)");
    assert_eq!(v["sew"]["ports"], json!([5441]));
    assert_eq!(v["apt"]["source"], false);
    never_destructive(&t);
}

// ── add-major ───────────────────────────────────────────────────────────

#[test]
fn add_major_dry_run_writes_nothing_and_runs_nothing() {
    let (t, _w) = pg_box();
    t.as_agent();
    let v = json(&t.call(&["pg", "add-major", "17", "--dry-run"]));
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["fromPgdg"], true);
    assert_eq!(v["packages"], json!(["postgresql-17", "postgresql-client-17"]));
    assert_eq!(v["files"]["keyring"], PGDG_KEYRING);
    for p in [CREATECLUSTER_DROPIN, PGDG_KEYRING, PGDG_SOURCES, PGDG_PREF] {
        assert!(!t.p(p).exists(), "{p}");
    }
    assert!(t.non_query_calls().is_empty(), "{:?}", t.non_query_calls());
    // The real install is the daemon's call only.
    let o = t.call(&["pg", "add-major", "17"]);
    assert_eq!(o.code, exit::NOT_DAEMON, "{}", o.stderr);
    assert!(!t.p(CREATECLUSTER_DROPIN).exists());
}

#[test]
fn add_major_installs_17_from_pgdg_with_the_pinned_key_and_pins() {
    let (t, w) = pg_box();
    w.borrow_mut().sim = "Inst libpq5 [16.15-0ubuntu0.24.04.1] (18.6-1.pgdg24.04+2 PostgreSQL)\nInst postgresql-client-17 (17.11-1.pgdg24.04+2)\nInst postgresql-17 (17.11-1.pgdg24.04+2)\n".into();
    let v = json(&t.call(&["pg", "add-major", "17"]));
    assert_eq!(v["result"], "installed");
    assert_eq!(v["source"], "k2");
    assert_eq!(std::fs::read_to_string(t.p(PGDG_KEYRING)).unwrap(), PGDG_KEY_ASC);
    let src = std::fs::read_to_string(t.p(PGDG_SOURCES)).unwrap();
    assert_eq!(src, k2_root_proto::pg_major::pgdg_sources("noble").unwrap());
    let pref = std::fs::read_to_string(t.p(PGDG_PREF)).unwrap();
    assert_eq!(pref, k2_root_proto::pg_major::pgdg_pref("noble").unwrap());
    assert_eq!(std::fs::read_to_string(t.p(CREATECLUSTER_DROPIN)).unwrap(), k2_root_proto::pg_major::createcluster_dropin());
    assert_eq!(w.borrow().dropin_before_install, Some(true), "create_main_cluster=false lands before apt");
    let calls = t.non_query_calls();
    let apt: Vec<&String> = calls.iter().filter(|c| c.contains(APT)).collect();
    assert_eq!(apt.len(), 3, "{apt:?}");
    assert!(apt[0].ends_with("apt-get update -q"));
    assert!(apt[1].contains("install -s -q --no-install-recommends --no-upgrade") && apt[1].ends_with("postgresql-17 postgresql-client-17"));
    assert!(apt[2].contains("install -y --no-remove -q --no-install-recommends --no-upgrade") && apt[2].ends_with("postgresql-17 postgresql-client-17"));
    assert_eq!(std::fs::read_to_string(t.p(UNATTENDED_DROPIN)).unwrap(), k2_root_proto::pg_major::unattended_dropin(), "PGDG updates arrive unattended (M2)");
    assert!(t.p("/usr/lib/postgresql/17/bin/postgres").exists());
    let mark: Vec<&String> = calls.iter().filter(|c| c.starts_with(APT_MARK)).collect();
    assert_eq!(mark.len(), 1);
    assert_eq!(mark[0], &format!("{APT_MARK} manual postgresql-16 postgresql-client-16 postgresql-17 postgresql-client-17"));
    assert_eq!(v["markedManual"].as_array().unwrap().len(), 4);
    never_destructive(&t);
    let a = t.audit.0.borrow();
    let line = a.last().unwrap();
    assert!(line.contains("\"verb\":\"pg add-major 17\"") && line.contains("\"result\":\"installed\""), "{line}");
}

/// Sew 2026-10-20 (1): an install that would remove or replace the
/// installed postgresql-16 (Sew's own Postgres runs on it) is refused
/// before anything is installed.
#[test]
fn add_major_refuses_to_remove_or_change_sews_postgres() {
    for sim in [
        "Remv postgresql-16 [16.15-0ubuntu0.24.04.1]\nInst postgresql-17 (17.11-1.pgdg24.04+2)\n",
        "Inst postgresql-16 [16.15-0ubuntu0.24.04.1] (16.11-1.pgdg24.04+1 PostgreSQL)\nInst postgresql-17 (17.11)\n",
    ] {
        let (t, w) = pg_box();
        mkdirs(&t.root, &["/etc/sew"]);
        std::fs::write(t.p(SEW_TOML), "pg_major = 16\n").unwrap();
        w.borrow_mut().sim = sim.into();
        let o = t.call(&["pg", "add-major", "17"]);
        assert_eq!(o.code, exit::STATE, "{}", o.stderr);
        assert!(o.stderr.contains("Sew") && o.stderr.contains("postgresql-16"), "{}", o.stderr);
        assert!(o.stderr.contains("nothing changed"), "{}", o.stderr);
        assert!(!t.non_query_calls().iter().any(|c| c.contains("install -y")), "nothing installed");
        assert!(!t.p("/usr/lib/postgresql/17/bin/postgres").exists());
        never_destructive(&t);
    }
}

/// Review #1 M1: an installed Postgres package that changed during the
/// real install (the lists moved after the simulation) is reported loudly.
#[test]
fn add_major_reports_an_installed_package_that_changed() {
    let (t, w) = pg_box();
    mkdirs(&t.root, &["/etc/sew"]);
    std::fs::write(t.p(SEW_TOML), "pg_major = 16\n").unwrap();
    w.borrow_mut().sim = "Inst postgresql-17 (17.11)\n".into();
    w.borrow_mut().bump_on_install = Some(("postgresql-common".into(), "282".into()));
    let o = t.call(&["pg", "add-major", "17"]);
    assert_eq!(o.code, exit::STATE, "{}", o.stderr);
    assert!(
        o.stderr.starts_with("installed_changed: ") && o.stderr.contains("postgresql-common 1.0 → 282") && o.stderr.contains("Sew"),
        "{}",
        o.stderr
    );
    // libpq5 moving is expected (PGDG's 17 needs it).
    let (t, w) = pg_box();
    w.borrow_mut().dpkg.push("libpq5".into());
    w.borrow_mut().sim = "Inst postgresql-17 (17.11)\n".into();
    w.borrow_mut().bump_on_install = Some(("libpq5".into(), "18.6".into()));
    ok(&t.call(&["pg", "add-major", "17"]));
}

#[test]
fn add_major_already_installed_needs_no_network() {
    let (t, _w) = pg_box();
    bins(&t.root, 17);
    let v = json(&t.call(&["pg", "add-major", "17"]));
    assert_eq!(v["result"], "already_installed");
    assert!(!t.non_query_calls().iter().any(|c| c.contains(APT)), "{:?}", t.non_query_calls());
    assert!(!t.p(PGDG_SOURCES).exists(), "no PGDG when nothing is installed");
    assert!(t.p(CREATECLUSTER_DROPIN).exists());
    // The distro's own major is never from PGDG either.
    let (t, w) = pg_box();
    std::fs::remove_dir_all(t.p("/usr/lib/postgresql/16")).unwrap();
    w.borrow_mut().sim = "Inst postgresql-16 (16.15-0ubuntu0.24.04.1 Ubuntu)\n".into();
    let v = json(&t.call(&["pg", "add-major", "16"]));
    assert_eq!(v["source"], "distro");
    assert!(!t.p(PGDG_SOURCES).exists() && !t.p(PGDG_KEYRING).exists());
}

#[test]
fn add_major_refusals() {
    // postgresql-common's key differs from K2's: nothing written.
    let (t, _w) = pg_box();
    touch(&t.root, PGDG_COMMON_KEY);
    std::fs::write(t.p(PGDG_COMMON_KEY), "-----BEGIN PGP PUBLIC KEY BLOCK-----\nother\n").unwrap();
    let o = t.call(&["pg", "add-major", "17"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("pgdg_key_mismatch"), "{}", o.stderr);
    assert!(!t.p(PGDG_KEYRING).exists() && !t.p(PGDG_SOURCES).exists());
    // Same key (whitespace aside): fine.
    let (t, w) = pg_box();
    touch(&t.root, PGDG_COMMON_KEY);
    std::fs::write(t.p(PGDG_COMMON_KEY), PGDG_KEY_ASC.replace('\n', "\r\n")).unwrap();
    w.borrow_mut().sim = "Inst postgresql-17 (17.11)\n".into();
    ok(&t.call(&["pg", "add-major", "17"]));

    // A PGDG source K2 didn't write: kept, K2's not added, pins still apply.
    let (t, w) = pg_box();
    std::fs::write(t.p("/etc/apt/sources.list.d/pgdg.list"), "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt noble-pgdg main\n").unwrap();
    w.borrow_mut().sim = "Inst postgresql-17 (17.11)\n".into();
    let v = json(&t.call(&["pg", "add-major", "17"]));
    assert_eq!(v["source"], "foreign");
    assert!(!t.p(PGDG_SOURCES).exists() && t.p(PGDG_PREF).exists());

    // Air-gap: the mirror hint, exit 3.
    let (t, w) = pg_box();
    w.borrow_mut().sim_fail = Some("E: Unable to locate package postgresql-17\n".into());
    let o = t.call(&["pg", "add-major", "17"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("pgdg_unreachable") && o.stderr.contains("apt install ./*.deb"), "{}", o.stderr);

    // Not an allow-listed distro.
    let (t, _w) = pg_box();
    std::fs::remove_file(t.p("/etc/os-release")).unwrap();
    std::fs::write(t.p("/usr/lib/os-release"), "ID=fedora\nVERSION_CODENAME=\n").unwrap();
    let o = t.call(&["pg", "add-major", "17"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("unsupported_distro"), "{}", o.stderr);
    assert!(t.non_query_calls().is_empty());
}

// ── clusters on demand ──────────────────────────────────────────────────

#[test]
fn ensure_cluster_creates_17_on_5433_with_k2_conf_and_the_slice() {
    let (t, w) = pg_box();
    bins(&t.root, 17);
    let v = json(&t.call(&["pg", "ensure-cluster", "17", "5433"]));
    for (k, want) in [
        ("major", json!(17)),
        ("name", json!("main")),
        ("port", json!(5433)),
        ("dataDir", json!("/var/lib/postgresql/17/main")),
        ("running", json!(true)),
        ("startConf", json!("auto")),
        ("unit", json!("postgresql@17-main.service")),
        ("created", json!(true)),
        ("started", json!(true)),
    ] {
        assert_eq!(v[k], want, "{k}: {v}");
    }
    let calls = t.non_query_calls();
    assert!(calls.contains(&format!(
        "{PG_CREATECLUSTER} 17 main -p 5433 --start-conf auto -- --auth-local=peer --auth-host=scram-sha-256"
    )), "{calls:?}");
    assert!(calls.contains(&format!("{SYSTEMCTL} start postgresql@17-main")));
    let conf = std::fs::read_to_string(t.p("/etc/postgresql/17/main/conf.d/98-k2-cluster.conf")).unwrap();
    assert!(conf.contains("wal_level = logical") && conf.contains("listen_addresses = 'localhost'"));
    assert_eq!(
        std::fs::read_to_string(t.p("/etc/postgresql/17/main/conf.d/99-k2-ram.conf")).unwrap(),
        k2_root_proto::templates::pg_guc_conf()
    );
    assert_eq!(std::fs::read_to_string(t.p(SLICE_UNIT_PATH)).unwrap(), k2_root_proto::pg_major::slice_unit());
    assert_eq!(
        std::fs::read_to_string(t.p(&k2_root_proto::pg_major::instance_slice_dropin(17))).unwrap(),
        k2_root_proto::pg_major::slice_dropin(),
        "only the K2-created cluster's own instance joins the slice"
    );
    assert!(!t.p("/etc/systemd/system/postgresql@.service.d/k2-slice.conf").exists(), "never the template (L3)");
    assert!(!t.p(&k2_root_proto::pg_major::instance_slice_dropin(16)).exists());
    assert!(w.borrow().clusters.contains(&(17, 5433, true)));
    // Again: nothing created, nothing started.
    let v = json(&t.call(&["pg", "ensure-cluster", "17", "5433"]));
    assert_eq!(v["created"], false);
    assert_eq!(v["started"], false);
    // The 16 cluster is never touched.
    assert!(!t.non_query_calls().iter().any(|c| c.contains("16-main") || c.contains("/16/main/conf.d")));
    never_destructive(&t);
}

#[test]
fn ensure_cluster_refusals_change_nothing() {
    let cases: Vec<(&str, Box<dyn Fn(&T, &W)>, &str)> = vec![
        ("band full", Box::new(|t, w| {
            bins(&t.root, 17);
            w.borrow_mut().listening = (5433..=5449).collect();
        }), "band_full"),
        ("port in use", Box::new(|t, w| {
            bins(&t.root, 17);
            w.borrow_mut().listening.push(5433);
        }), "port_taken"),
        ("port of another cluster", Box::new(|t, w| {
            bins(&t.root, 17);
            w.borrow_mut().clusters.push((18, 5433, false));
        }), "port_taken"),
        ("leftover data dir", Box::new(|t, _| {
            bins(&t.root, 17);
            mkdirs(&t.root, &["/var/lib/postgresql/17/main"]);
        }), "cluster_leftover"),
        ("existing cluster elsewhere", Box::new(|t, w| {
            bins(&t.root, 17);
            w.borrow_mut().clusters.push((17, 5434, true));
        }), "port_mismatch"),
        // Review #1 L5: a link planted on the way to the new dirs.
        ("link above the data dir", Box::new(|t, _| {
            bins(&t.root, 17);
            mkdirs(&t.root, &["/srv/elsewhere"]);
            std::os::unix::fs::symlink(t.p("/srv/elsewhere"), t.p("/var/lib/postgresql/17")).unwrap();
        }), "cluster_layout_refused"),
        ("link above the config dir", Box::new(|t, _| {
            bins(&t.root, 17);
            mkdirs(&t.root, &["/srv/elsewhere"]);
            std::os::unix::fs::symlink(t.p("/srv/elsewhere"), t.p("/etc/postgresql/17")).unwrap();
        }), "cluster_layout_refused"),
    ];
    for (what, setup, want) in cases {
        let (t, w) = pg_box();
        setup(&t, &w);
        let argv: &[&str] = if what == "band full" { &["pg", "ensure-cluster", "17"] } else { &["pg", "ensure-cluster", "17", "5433"] };
        let o = t.call(argv);
        assert_eq!(o.code, exit::STATE, "{what}: {}", o.stderr);
        assert!(o.stderr.starts_with(want), "{what}: {}", o.stderr);
        assert!(!t.non_query_calls().iter().any(|c| c.starts_with(PG_CREATECLUSTER) || c.contains(" start ")), "{what}");
        assert!(!t.p(SLICE_UNIT_PATH).exists(), "{what}");
    }
    // An agent can't make one.
    let (t, _w) = pg_box();
    bins(&t.root, 17);
    t.as_agent();
    assert_eq!(t.call(&["pg", "ensure-cluster", "17", "5433"]).code, exit::NOT_DAEMON);
}

/// PG-A's resolver asks `pg ensure-cluster <M>` without a port: the
/// helper installs the major when its packages are missing and takes the
/// first free port in the band (5432 is 16's, 5433 is busy here).
#[test]
fn ensure_cluster_without_a_port_installs_and_picks_the_band_port() {
    let (t, w) = pg_box();
    w.borrow_mut().listening.push(5433);
    w.borrow_mut().sim = "Inst postgresql-17 (17.11-1.pgdg24.04+2)\n".into();
    let v = json(&t.call(&["pg", "ensure-cluster", "17"]));
    assert_eq!(v["port"], 5434, "{v}");
    assert_eq!(v["created"], true);
    assert!(t.p("/usr/lib/postgresql/17/bin/postgres").exists(), "add-major ran first");
    assert!(t.p(PGDG_PREF).exists());
    let calls = t.non_query_calls();
    let install = calls.iter().position(|c| c.contains("install -y")).unwrap();
    let create = calls.iter().position(|c| c.starts_with(PG_CREATECLUSTER)).unwrap();
    assert!(install < create, "{calls:?}");
    assert!(calls[create].contains(" -p 5434 "));
    // Again: the existing cluster, no install, no create.
    let v = json(&t.call(&["pg", "ensure-cluster", "17"]));
    assert_eq!((v["port"].as_u64(), v["created"].as_bool()), (Some(5434), Some(false)));
    never_destructive(&t);
}

#[test]
fn start_conf_and_slice() {
    let (t, _w) = pg_box();
    let v = json(&t.call(&["pg", "start-conf", "16", "manual"]));
    assert_eq!(v["changed"], true);
    let body = std::fs::read_to_string(t.p("/etc/postgresql/16/main/start.conf")).unwrap();
    assert!(body.ends_with("\nmanual\n"), "{body}");
    assert!(t.non_query_calls().contains(&format!("{SYSTEMCTL} daemon-reload")));
    assert!(!t.non_query_calls().iter().any(|c| c.contains("stop")), "manual never stops it");
    assert_eq!(json(&t.call(&["pg", "start-conf", "16", "manual"]))["changed"], false);
    ok(&t.call(&["pg", "start-conf", "16", "auto"]));
    let o = t.call(&["pg", "start-conf", "17", "auto"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("no_cluster"), "{}", o.stderr);
    // Review #1 L3: the owner's `disabled` stays; both verbs are the daemon's.
    std::fs::write(t.p("/etc/postgresql/16/main/start.conf"), "disabled\n").unwrap();
    let o = t.call(&["pg", "start-conf", "16", "auto"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("start_conf_disabled"), "{}", o.stderr);
    assert_eq!(std::fs::read_to_string(t.p("/etc/postgresql/16/main/start.conf")).unwrap(), "disabled\n");
    t.as_agent();
    for argv in [&["pg", "start-conf", "16", "manual"][..], &["pg", "start-conf", "16", "auto"], &["pg", "install-slice"]] {
        assert_eq!(t.call(argv).code, exit::NOT_DAEMON, "{argv:?}");
    }
    t.as_daemon();
    let v = json(&t.call(&["pg", "install-slice"]));
    assert_eq!((v["changed"].as_bool(), v["drift"].as_bool()), (Some(true), Some(false)));
    assert_eq!(json(&t.call(&["pg", "install-slice"]))["changed"], false);
    // A slice the owner tuned is left as it is.
    std::fs::write(t.p(SLICE_UNIT_PATH), "[Slice]\nMemoryMax=90%\n").unwrap();
    let v = json(&t.call(&["pg", "install-slice"]));
    assert_eq!((v["changed"].as_bool(), v["drift"].as_bool()), (Some(false), Some(true)));
    assert_eq!(std::fs::read_to_string(t.p(SLICE_UNIT_PATH)).unwrap(), "[Slice]\nMemoryMax=90%\n");
}

// ── roles ───────────────────────────────────────────────────────────────

fn two_clusters() -> (T, W) {
    let (t, w) = pg_box();
    bins(&t.root, 17);
    {
        let mut b = w.borrow_mut();
        b.clusters.push((17, 5433, true));
        b.dbs.insert(5433, vec!["postgres".into()]);
        b.roles.insert(
            5432,
            vec![
                role("ws_jobs_agent", Some(SCRAM_A)),
                role("ws_jobs_migrator", Some(SCRAM_B)),
                role("ws_jobs_bind", None),
                role("ws_crm_agent", Some(SCRAM_A)),
            ],
        );
    }
    (t, w)
}

fn roles_req(r: &[&str]) -> Vec<u8> {
    serde_json::to_vec(&json!({ "roles": r })).unwrap()
}

#[test]
fn copy_roles_keeps_the_verifier_and_is_idempotent() {
    let (t, w) = two_clusters();
    let req = roles_req(&["ws_jobs_agent", "ws_jobs_migrator", "ws_jobs_bind"]);
    let v = json(&t.call_in(&["pg", "copy-roles", "16", "17"], &req));
    assert_eq!(v["created"], json!(["ws_jobs_agent", "ws_jobs_bind", "ws_jobs_migrator"]));
    let sql: Vec<String> = w.borrow().sql.iter().filter(|(p, s)| *p == 5433 && s.contains("CREATE ROLE")).map(|(_, s)| s.clone()).collect();
    assert_eq!(sql.len(), 1);
    assert!(sql[0].contains("\nBEGIN;\n") && sql[0].ends_with("COMMIT;\n"));
    assert!(sql[0].contains(&format!("CREATE ROLE \"ws_jobs_agent\" LOGIN NOINHERIT NOCREATEROLE NOCREATEDB NOSUPERUSER NOREPLICATION NOBYPASSRLS CONNECTION LIMIT -1 PASSWORD '{SCRAM_A}';")));
    assert!(sql[0].contains("CREATE ROLE \"ws_jobs_bind\" NOLOGIN") && sql[0].contains("PASSWORD NULL;"));
    // The verifier never reaches stdout or the audit line.
    assert!(!String::from_utf8_lossy(&t.call_in(&["pg", "copy-roles", "16", "17"], &req).stdout).contains("SCRAM"));
    assert!(!t.audit.0.borrow().iter().any(|l| l.contains("SCRAM")));
    let v = json(&t.call_in(&["pg", "copy-roles", "16", "17"], &req));
    assert_eq!(v["created"], json!([]));
    assert_eq!(v["same"].as_array().unwrap().len(), 3);
}

#[test]
fn copy_roles_dry_run_reports_names_and_changes_nothing() {
    let (t, w) = two_clusters();
    w.borrow_mut().roles.insert(5433, vec![role("ws_crm_agent", Some(SCRAM_B)), role("ws_jobs_bind", None)]);
    w.borrow_mut().roles.get_mut(&5432).unwrap()[1].rolsuper = true;
    let v = json(&t.call_in(
        &["pg", "copy-roles", "16", "17", "--dry-run"],
        &roles_req(&["ws_jobs_agent", "ws_jobs_migrator", "ws_jobs_bind", "ws_crm_agent", "ws_ghost_agent"]),
    ));
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["wouldCreate"], json!(["ws_jobs_agent"]));
    assert_eq!(v["same"], json!(["ws_jobs_bind"]));
    assert_eq!(v["mismatch"], json!(["ws_crm_agent"]));
    assert_eq!(v["refused"], json!(["ws_jobs_migrator"]));
    assert_eq!(v["missing"], json!(["ws_ghost_agent"]));
    assert!(!v.to_string().contains("SCRAM"), "names only: {v}");
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE")));
}

/// Review #1 L2: the batch turns statement logging off, and a failure
/// answers a fixed line (psql's context could carry part of a verifier).
#[test]
fn copy_roles_never_leaks_a_verifier_on_failure() {
    let (t, w) = two_clusters();
    w.borrow_mut().fail_create_role = true;
    let o = t.call_in(&["pg", "copy-roles", "16", "17"], &roles_req(&["ws_jobs_agent"]));
    assert_eq!(o.code, exit::IO);
    assert_eq!(o.stderr, "copy-roles: creating the roles failed; the batch is one transaction, so none were created");
    let sql = w.borrow().sql.iter().find(|(_, s)| s.contains("CREATE ROLE")).cloned().unwrap().1;
    assert!(
        sql.starts_with("SET log_min_error_statement = panic;\nSET log_statement = 'none';\nSET log_min_duration_statement = -1;\n"),
        "{sql}"
    );
    assert!(sql.contains("pg_stat_statements.track_utility"));
    assert!(!t.audit.0.borrow().iter().any(|l| l.contains("SCRAM") || l.contains("c2Vy")));
}

#[test]
fn copy_roles_refusals_change_nothing() {
    // A shared grantee login with another password in the target (V32).
    let (t, w) = two_clusters();
    w.borrow_mut().roles.insert(5433, vec![role("ws_crm_agent", Some(SCRAM_B))]);
    let o = t.call_in(&["pg", "copy-roles", "16", "17"], &roles_req(&["ws_jobs_agent", "ws_crm_agent"]));
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("verifier_mismatch") && o.stderr.contains("nothing changed"), "{}", o.stderr);
    assert!(!o.stderr.contains("SCRAM"));
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE")));
    // Missing in the source.
    let (t, w) = two_clusters();
    let o = t.call_in(&["pg", "copy-roles", "16", "17"], &roles_req(&["ws_jobs_agent", "ws_ghost_agent"]));
    assert_eq!(o.code, exit::STATE);
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE")));
    // A superuser (or BYPASSRLS) source role is never copied.
    let (t, w) = two_clusters();
    w.borrow_mut().roles.get_mut(&5432).unwrap()[0].rolbypassrls = true;
    let o = t.call_in(&["pg", "copy-roles", "16", "17"], &roles_req(&["ws_jobs_agent"]));
    assert_eq!(o.code, exit::STATE);
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE")));
    // Bad stdin: refused as input (exit 2), nothing run.
    let (t, w) = two_clusters();
    for bad in [&b"{\"roles\":[\"postgres\"]}"[..], b"{\"roles\":[\"a\"],\"sql\":\"x\"}", b"not json", b"{\"roles\":[]}"] {
        let o = t.call_in(&["pg", "copy-roles", "16", "17"], bad);
        assert_eq!(o.code, exit::ARGV, "{}", String::from_utf8_lossy(bad));
    }
    assert!(w.borrow().sql.is_empty());
    // Daemon only.
    t.as_agent();
    assert_eq!(t.call_in(&["pg", "copy-roles", "16", "17"], &roles_req(&["ws_jobs_agent"])).code, exit::NOT_DAEMON);
}

// ── dump, restore, lock, drop ───────────────────────────────────────────

/// `pg_restore -l` of the fake dump: two extensions and a comment on one,
/// tenant objects, and a table whose name tries to pass for an extension
/// entry (pg_restore prints a newline in a name as a space; the rest of
/// its line still isn't an extension entry).
const TOC: &str = ";\n; Archive created at 2026-10-20 10:15:00 UTC\n;\n\
2; 3079 16385 EXTENSION - pgcrypto \n\
4321; 0 0 COMMENT - EXTENSION pgcrypto \n\
3; 3079 16420 EXTENSION - pg_trgm \n\
5; 3079 16430 EXTENSION - uuid-ossp \n\
4322; 0 0 COMMENT - EXTENSION \"uuid-ossp\" \n\
4323; 0 0 COMMENT - EXTENSION \"not-here\" \n\
6; 2615 16490 SCHEMA - k2 ws_jobs_migrator\n\
61; 1255 16491 FUNCTION k2 actor() postgres\n\
62; 1255 16492 FUNCTION k2 skin_uid() ws_jobs_migrator\n\
63; 0 0 ACL k2 FUNCTION actor() postgres\n\
64; 1255 16493 FUNCTION k2 whoami() ws_jobs_migrator\n\
7; 2615 16500 SCHEMA - app ws_jobs_migrator\n\
9; 1255 16510 FUNCTION public h1_who(integer) ws_jobs_migrator\n\
11; 1259 16520 TABLE public h1 ws_jobs_migrator\n\
12; 1259 16530 TABLE public x 13; 3079 1 EXTENSION - evil ws_jobs_migrator\n\
14; 0 0 COMMENT - EXTENSION evil x ws_jobs_migrator\n";

fn get_list(w: &W, name: &str) -> String {
    let files = w.borrow().passfiles.clone();
    files.iter().find(|(f, _)| f.ends_with(name)).map(|(_, b)| b.clone()).unwrap_or_else(|| panic!("{name}: {files:?}"))
}

fn login(role: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"role": role, "password": "pw:with\\colon"})).unwrap()
}

#[test]
fn dump_restore_use_the_targets_binaries_as_postgres() {
    let (t, w) = two_clusters();
    w.borrow_mut().dbs.get_mut(&5433).unwrap().push("ws_jobs".into());
    let tag = "20261020T101500Z";
    let v = json(&t.call(&["pg", "move-dump", "16", "17", "ws_jobs", tag]));
    let file = "/var/lib/postgresql/k2-moves/ws_jobs-20261020T101500Z.dump";
    assert_eq!(v["file"], file);
    assert_eq!(v["bytes"], 4096);
    let pc = w.borrow().postgres_calls.clone();
    assert!(pc.contains(&"/usr/bin/mkdir -p -m 0700 -- /var/lib/postgresql/k2-moves".to_string()), "{pc:?}");
    assert!(pc.contains(&"/usr/bin/chmod 0700 -- /var/lib/postgresql/k2-moves".to_string()), "L4: {pc:?}");
    assert!(
        pc.contains(&"/usr/bin/find /var/lib/postgresql/k2-moves -maxdepth 1 -type f -name .k2pass-* -delete".to_string()),
        "R1: stale passfiles swept: {pc:?}"
    );
    assert!(pc.contains(&"/usr/bin/find /var/lib/postgresql/k2-moves -maxdepth 1 -type f -name .k2toc-* -delete".to_string()), "{pc:?}");
    assert!(w.borrow().sql.iter().any(|(p, s)| *p == 5432 && s.starts_with("SHOW server_version_num")), "R2: the source server's version read");
    assert!(pc.contains(&format!(
        "PGOPTIONS=-c search_path=pg_catalog,pg_temp -c role=none /usr/lib/postgresql/17/bin/pg_dump -Fc --no-subscriptions --no-publications -h /var/run/postgresql -p 5432 -f {file} -d ws_jobs"
    )), "17's pg_dump against 16's port: {pc:?}");
    assert!(pc.contains(&format!("/usr/bin/chmod 0600 -- {file}")), "L4: {pc:?}");
    // The same tag twice: refused, nothing overwritten.
    let o = t.call(&["pg", "move-dump", "16", "17", "ws_jobs", tag]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("dump_exists"));

    // Restore: extensions alone as postgres; the rest of pre-data, data and
    // post-data as the owner (H1/F2; the 0.46.4 drill: a generated column's
    // IMMUTABLE function ran as postgres during pre-data).
    w.borrow_mut().forced = "16384,16390".into();
    w.borrow_mut().postgres_calls.clear();
    let v = json(&t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator")));
    assert_eq!(
        (v["to"].as_u64(), v["loadedAs"].as_str(), v["forcedTables"].as_u64()),
        (Some(17), Some("ws_jobs_migrator"), Some(2))
    );
    let pc = w.borrow().postgres_calls.clone();
    let restores: Vec<&String> = pc.iter().filter(|c| c.contains("pg_restore")).collect();
    assert_eq!(restores.len(), 6, "{restores:?}");
    assert!(restores[0].ends_with(&format!("/usr/lib/postgresql/17/bin/pg_restore -l {}", "/var/lib/postgresql/k2-moves/ws_jobs-20261020T101500Z.dump")), "{}", restores[0]);
    assert!(restores[1].contains("-L /var/lib/postgresql/k2-moves/.k2toc-schemas-20261020T101500Z") && restores[1].contains(" -U ws_jobs_migrator "), "schemas as the owner, first: {}", restores[1]);
    let su = restores[2];
    assert!(
        su.contains("--section=pre-data") && su.contains("-L /var/lib/postgresql/k2-moves/.k2toc-su-20261020T101500Z")
            && su.contains("-h /var/run/postgresql") && !su.contains(" -U "),
        "extensions as postgres: {su}"
    );
    for (r, section) in [(restores[1], "pre-data"), (restores[3], "pre-data"), (restores[4], "data"), (restores[5], "post-data")] {
        assert!(r.contains(&format!("--section={section}")), "{r}");
        assert!(r.contains("-h 127.0.0.1 -p 5433 -U ws_jobs_migrator -d ws_jobs"), "the owner over loopback: {r}");
        assert!(r.starts_with("PGPASSFILE=/var/lib/postgresql/k2-moves/.k2pass-20261020T101500Z "), "{r}");
        assert!(!r.contains("pw:with"), "the password is never in argv: {r}");
        assert!(!r.contains("temp_file_limit") && !r.contains("role=none"), "a non-superuser session: {r}");
    }
    assert!(restores[3].contains("-L /var/lib/postgresql/k2-moves/.k2toc-own-20261020T101500Z"), "{}", restores[3]);
    assert_eq!(get_list(&w, ".k2toc-schemas-20261020T101500Z"), "7; 2615 16500 SCHEMA - app ws_jobs_migrator\n");
    let files = w.borrow().passfiles.clone();
    let get = |name: &str| files.iter().find(|(f, _)| f.ends_with(name)).map(|(_, b)| b.clone()).unwrap_or_else(|| panic!("{name}: {files:?}"));
    assert_eq!(get(".k2pass-20261020T101500Z"), "127.0.0.1:5433:ws_jobs:ws_jobs_migrator:pw\\:with\\\\colon\n");
    assert_eq!(
        get(".k2toc-su-20261020T101500Z"),
        "2; 3079 16385 EXTENSION - pgcrypto \n4321; 0 0 COMMENT - EXTENSION pgcrypto \n3; 3079 16420 EXTENSION - pg_trgm \n\
         5; 3079 16430 EXTENSION - uuid-ossp \n4322; 0 0 COMMENT - EXTENSION \"uuid-ossp\" \n",
        "only real extension entries (a quoted name too, drill B) go to the superuser"
    );
    // Drill A: K2's own schema and helpers are never replayed; the canned
    // install runs as postgres, in the target, before the owner's pre-data.
    let sql = w.borrow().sql.clone();
    let install = sql.iter().position(|(p, s)| *p == 5433 && s.starts_with("SET check_function_bodies = off;\nCREATE SCHEMA k2;\n")).expect("K2's install");
    let s = &sql[install].1;
    assert!(s.contains("CREATE OR REPLACE FUNCTION k2.actor() RETURNS text") && s.contains("CREATE OR REPLACE FUNCTION k2.skin_uid()"), "{s}");
    assert!(!s.contains("whoami") && !s.contains("set_principal"), "only K2's listed helpers: {s}");
    assert!(s.contains("ALTER FUNCTION k2.skin_uid() OWNER TO \"ws_jobs_migrator\";") && s.contains("ALTER SCHEMA k2 OWNER TO \"ws_jobs_migrator\";"), "{s}");
    let force_read = sql.iter().position(|(_, s)| s.contains("WHERE c.relforcerowsecurity")).unwrap();
    assert!(install < force_read, "before the owner's pre-data");
    let own = get(".k2toc-own-20261020T101500Z");
    assert!(!own.contains("7; 2615"), "{own}");
    for k2 in ["6; 2615", "61; 1255", "62; 1255", "63; 0 0 ACL"] {
        assert!(!own.contains(k2), "K2's own entries are never replayed: {own}");
    }
    assert!(own.contains("64; 1255 16493 FUNCTION k2 whoami()"), "a tenant function in k2 is the owner's: {own}");
    assert!(own.contains("4323; 0 0 COMMENT - EXTENSION \"not-here\""), "a comment on no extension of the dump: {own}");
    for l in ["9; 1255", "11; 1259", "12; 1259 16530 TABLE public x 13; 3079 1 EXTENSION - evil", "14; 0 0 COMMENT - EXTENSION evil x"] {
        assert!(own.contains(l), "{l}: {own}");
    }
    assert!(!own.contains("EXTENSION - pgcrypto") && !own.contains("Archive created"), "{own}");
    assert!(pc.contains(&"/usr/bin/chmod 0600 -- /var/lib/postgresql/k2-moves/.k2pass-20261020T101500Z".to_string()), "{pc:?}");
    for f in [".k2pass-", ".k2toc-schemas-", ".k2toc-su-", ".k2toc-own-"] {
        assert!(pc.iter().any(|c| *c == format!("/usr/bin/rm -f -- /var/lib/postgresql/k2-moves/{f}20261020T101500Z")), "{f} goes: {pc:?}");
    }
    // FORCE is read and lifted only after the owner's pre-data.
    let own_pre = pc.iter().position(|c| c.contains("--section=pre-data") && c.contains(".k2toc-own-")).unwrap();
    let data = pc.iter().position(|c| c.contains("--section=data")).unwrap();
    assert!(own_pre < data);
    let sql: Vec<String> = w.borrow().sql.iter().map(|(_, s)| s.clone()).collect();
    let nof = sql.iter().position(|s| s.contains("NO FORCE ROW LEVEL SECURITY")).expect("FORCE lifted");
    let refo = sql
        .iter()
        .rposition(|s| s.contains("ARRAY[16384, 16390]::oid[]") && s.contains(" FORCE ROW LEVEL SECURITY") && !s.contains("NO FORCE"))
        .expect("FORCE back");
    assert!(nof < refo);
    assert!(sql.iter().any(|s| s == "ALTER ROLE \"ws_jobs_migrator\" IN DATABASE \"ws_jobs\" SET temp_file_limit = -1;\n"));
    assert!(sql.iter().any(|s| s == "ALTER ROLE \"ws_jobs_migrator\" IN DATABASE \"ws_jobs\" RESET temp_file_limit;\n"));

    // A failing owner pre-data (the schemas, the first owner step): the
    // passfile, the lists and the temp setting come back; FORCE untouched.
    w.borrow_mut().restore_fail = Some("pre-data".into());
    w.borrow_mut().postgres_calls.clear();
    w.borrow_mut().sql.clear();
    let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator"));
    assert_eq!(o.code, exit::IO, "{}", o.stderr);
    assert!(o.stderr.contains("pg_restore (pre-data)"), "{}", o.stderr);
    let pc = w.borrow().postgres_calls.clone();
    for f in [".k2pass-", ".k2toc-schemas-", ".k2toc-su-", ".k2toc-own-"] {
        assert!(pc.iter().any(|c| c.starts_with(&format!("/usr/bin/rm -f -- /var/lib/postgresql/k2-moves/{f}"))), "{f}: {pc:?}");
    }
    let sql: Vec<String> = w.borrow().sql.iter().map(|(_, s)| s.clone()).collect();
    assert!(!sql.iter().any(|s| s.contains("FORCE ROW LEVEL SECURITY")), "{sql:?}");
    assert!(sql.iter().any(|s| s.ends_with("RESET temp_file_limit;\n")), "{sql:?}");
    // A failing data or post-data load: only the ERROR line, never the row
    // (I4); the passfile, the temp setting and FORCE all come back in the
    // same cleanup (re-check #1 R1).
    for section in ["data", "post-data"] {
        w.borrow_mut().restore_fail = Some(section.into());
        w.borrow_mut().postgres_calls.clear();
        w.borrow_mut().sql.clear();
        let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator"));
        assert_eq!(o.code, exit::IO, "{}", o.stderr);
        assert!(o.stderr.contains(&format!("pg_restore ({section})")) && o.stderr.contains("violates check constraint"), "{}", o.stderr);
        assert!(!o.stderr.contains("secret-row-value") && !o.stderr.contains("COPY"), "{}", o.stderr);
        assert!(w.borrow().postgres_calls.iter().any(|c| c.starts_with("/usr/bin/rm -f -- /var/lib/postgresql/k2-moves/.k2pass-")));
        let sql: Vec<String> = w.borrow().sql.iter().map(|(_, s)| s.clone()).collect();
        let nof = sql.iter().position(|s| s.contains("NO FORCE ROW LEVEL SECURITY")).expect("FORCE lifted");
        let refo = sql
            .iter()
            .rposition(|s| s.contains(" FORCE ROW LEVEL SECURITY") && !s.contains("NO FORCE"))
            .unwrap_or_else(|| panic!("{section}: FORCE put back after a failed load: {sql:?}"));
        assert!(nof < refo);
        assert!(sql.iter().any(|s| s.ends_with("RESET temp_file_limit;\n")), "{section}: {sql:?}");
    }
    w.borrow_mut().restore_fail = None;
    // Backstop (re-check #1 R6): never as a superuser owner.
    w.borrow_mut().super_owner = true;
    w.borrow_mut().postgres_calls.clear();
    let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator"));
    assert!(o.stderr.starts_with("owner_superuser"), "{}", o.stderr);
    assert!(!w.borrow().postgres_calls.iter().any(|c| c.contains("pg_restore")), "nothing loaded");
    w.borrow_mut().super_owner = false;
    // R2: a source server older than 16.4 is refused before any dump.
    w.borrow_mut().server_num.insert(5432, "160003".into());
    w.borrow_mut().postgres_calls.clear();
    let o = t.call(&["pg", "move-dump", "16", "17", "ws_jobs", "20261020T131500Z"]);
    assert!(o.stderr.starts_with("server_too_old: the source server is 160003; a move needs 160004"), "{}", o.stderr);
    assert!(!w.borrow().postgres_calls.iter().any(|c| c.contains("pg_dump")), "nothing dumped");
    w.borrow_mut().server_num.insert(5432, "160004".into());
    ok(&t.call(&["pg", "move-dump", "16", "17", "ws_jobs", "20261020T131500Z"]));
    w.borrow_mut().server_num.clear();

    // Refusals: not the owner, a non-K2 database, a non-empty target, an
    // old pg_restore / pg_dump, bad stdin, a foreign moves dir.
    let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_crm_agent"));
    assert!(o.stderr.starts_with("owner_mismatch"), "{}", o.stderr);
    w.borrow_mut().owners.insert((5433, "ws_jobs".into()), "alice".into());
    let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator"));
    assert!(o.stderr.starts_with("not_k2_database"), "{}", o.stderr);
    w.borrow_mut().owners.clear();
    w.borrow_mut().target_rels = 3;
    let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator"));
    assert!(o.stderr.starts_with("target_not_empty"), "{}", o.stderr);
    w.borrow_mut().target_rels = 0;
    w.borrow_mut().tool_minor.insert("17".into(), "5".into());
    let o = t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], &login("ws_jobs_migrator"));
    assert!(o.stderr.starts_with("tool_too_old: pg_restore 17.5 is older than 17.6"), "{}", o.stderr);
    let o = t.call(&["pg", "move-dump", "16", "17", "ws_jobs", "20261020T111500Z"]);
    assert!(o.stderr.starts_with("tool_too_old: pg_dump 17.5"), "{}", o.stderr);
    w.borrow_mut().tool_minor.clear();
    for bad in [&b"{\"role\":\"postgres\",\"password\":\"x\"}"[..], b"{\"role\":\"ws_jobs_migrator\"}", b"nope"] {
        assert_eq!(t.call_in(&["pg", "move-restore", "17", "ws_jobs", tag], bad).code, exit::ARGV);
    }
    w.borrow_mut().moves_dir_stat = "directory root".into();
    let o = t.call(&["pg", "move-dump", "16", "17", "ws_jobs", "20261020T121500Z"]);
    assert!(o.stderr.starts_with("moves_dir_refused"), "{}", o.stderr);
    w.borrow_mut().moves_dir_stat = "directory postgres".into();
    // No dump / no target database.
    let o = t.call_in(&["pg", "move-restore", "17", "ws_crm", tag], &login("ws_crm_migrator"));
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("dump_missing"), "{}", o.stderr);
    w.borrow_mut().files.insert("/var/lib/postgresql/k2-moves/ws_crm-20261020T101500Z.dump".into());
    let o = t.call_in(&["pg", "move-restore", "17", "ws_crm", tag], &login("ws_crm_migrator"));
    assert!(o.stderr.starts_with("no_database"), "{}", o.stderr);

    let v = json(&t.call(&["pg", "move-file-remove", "ws_jobs", tag]));
    assert_eq!(v["removed"], true);
    assert!(!w.borrow().files.contains(file));
    assert_eq!(json(&t.call(&["pg", "move-file-remove", "ws_jobs", tag]))["removed"], false);

    let a = t.audit.0.borrow();
    assert!(a.iter().any(|l| l.contains("\"verb\":\"pg move-dump 16 17\"") && l.contains("\"db\":\"ws_jobs\"") && l.contains(tag)));
    assert!(!a.iter().any(|l| l.contains("pw:with")), "the owner's password never reaches the audit");
    drop(a);
    // Daemon only; the agent gets 77 before anything runs.
    t.as_agent();
    for argv in [
        &["pg", "move-dump", "16", "17", "ws_jobs", "20261021T000000Z"][..],
        &["pg", "move-restore", "17", "ws_jobs", tag],
        &["pg", "move-file-remove", "ws_jobs", tag],
        &["pg", "allow-conn", "16", "ws_jobs", "on"],
        &["pg", "drop-db", "16", "ws_jobs"],
    ] {
        assert_eq!(t.call(argv).code, exit::NOT_DAEMON, "{argv:?}");
    }
    never_destructive(&t);
}

#[test]
fn allow_conn_modes_and_drop() {
    let (t, w) = two_clusters();
    let last_sql = |w: &W| w.borrow().sql.last().cloned().unwrap();
    let v = json(&t.call(&["pg", "allow-conn", "16", "ws_jobs", "quiesce"]));
    assert_eq!(v["mode"], "quiesce");
    let (port, sql) = last_sql(&w);
    assert_eq!(port, 5432);
    assert!(sql.starts_with("ALTER DATABASE \"ws_jobs\" CONNECTION LIMIT 0;\n"), "{sql}");
    assert!(sql.contains("pg_terminate_backend(pid)") && sql.contains("datname = 'ws_jobs'"));
    assert!(!sql.contains("ALLOW_CONNECTIONS"), "superusers (the dump) still connect: {sql}");
    ok(&t.call(&["pg", "allow-conn", "16", "ws_jobs", "off"]));
    assert!(last_sql(&w).1.starts_with("ALTER DATABASE \"ws_jobs\" ALLOW_CONNECTIONS false;\n"));
    ok(&t.call(&["pg", "allow-conn", "16", "ws_jobs", "on"]));
    let sql = last_sql(&w).1;
    assert!(sql.contains("ALLOW_CONNECTIONS true") && sql.contains("CONNECTION LIMIT -1"), "{sql}");
    let o = t.call(&["pg", "allow-conn", "17", "ws_jobs", "on"]);
    assert!(o.stderr.starts_with("no_database"), "{}", o.stderr);
    let o = t.call(&["pg", "allow-conn", "16", "postgres", "off"]);
    assert_eq!(o.code, exit::STATE);

    // Review #1 L1: never a database that is open, nor one K2 didn't make.
    w.borrow_mut().open_dbs.insert((5432, "ws_jobs".into()));
    let o = t.call(&["pg", "drop-db", "16", "ws_jobs"]);
    assert!(o.stderr.starts_with("db_open"), "{}", o.stderr);
    w.borrow_mut().open_dbs.clear();
    w.borrow_mut().owners.insert((5432, "ws_jobs".into()), "alice".into());
    for argv in [&["pg", "drop-db", "16", "ws_jobs"][..], &["pg", "allow-conn", "16", "ws_jobs", "off"]] {
        let o = t.call(argv);
        assert!(o.stderr.starts_with("not_k2_database"), "{argv:?}: {}", o.stderr);
    }
    w.borrow_mut().owners.clear();
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.starts_with("DROP DATABASE")), "nothing dropped yet");
    let v = json(&t.call(&["pg", "drop-db", "16", "ws_jobs"]));
    assert_eq!(v["dropped"], true);
    let (port, sql) = last_sql(&w);
    assert_eq!((port, sql.as_str()), (5432, "DROP DATABASE IF EXISTS \"ws_jobs\" WITH (FORCE);\n"));
    assert_eq!(json(&t.call(&["pg", "drop-db", "17", "ws_jobs"]))["dropped"], false);
    assert_eq!(t.call(&["pg", "drop-db", "16", "postgres"]).code, exit::STATE);
    never_destructive(&t);
}

#[test]
fn psql_cluster_runs_that_majors_psql_on_its_port() {
    let (t, w) = two_clusters();
    ok(&t.call_in(&["pg", "psql", "--cluster", "17", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-tA"], b"SELECT 1;\n"));
    assert!(t.non_query_calls().contains(&format!(
        "{RUNUSER} -u postgres -- /usr/bin/env PGOPTIONS=-c search_path=pg_catalog,pg_temp -c role=none /usr/lib/postgresql/17/bin/psql -X -h /var/run/postgresql -p 5433 -d postgres -v ON_ERROR_STOP=1 -tA"
    )), "{:?}", t.non_query_calls());
    assert_eq!(w.borrow().sql.last().unwrap().0, 5433);
    // Without --cluster: pg_wrapper's psql as before (protocol 3).
    ok(&t.call_in(&["pg", "psql", "-d", "postgres"], b"SELECT 1;\n"));
    // Pinned too (F1): the boot seal and K2's helper installs run here.
    assert!(t
        .non_query_calls()
        .contains(&format!("{RUNUSER} -u postgres -- /usr/bin/env PGOPTIONS=-c search_path=pg_catalog,pg_temp -c role=none {PSQL} -X -d postgres")));
    // A major with no cluster: refused, nothing run.
    let o = t.call_in(&["pg", "psql", "--cluster", "18", "-d", "postgres"], b"SELECT 1;\n");
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("no_cluster"), "{}", o.stderr);
    let a = t.audit.0.borrow();
    assert!(a.iter().any(|l| l.contains("\"verb\":\"pg psql --cluster 17\"")));
}

#[test]
fn version_announces_pgmajor_on_protocol_9() {
    let t = box_root();
    let v = json(&t.call(&["helper", "version"]));
    assert_eq!(v["protocol"], 11, "pgmajor came in 9; 11 is 0.46.6's one bump (additive)");
    assert_eq!(v["namespaces"]["pgmajor"], true);
    assert_eq!(v["namespaces"]["pg"], true, "protocols are additive");
    assert!(PROTOCOL >= k2_root_proto::version::PGMAJOR_MIN_PROTOCOL);
}

// ── pgdual (protocol 10, prd-app-pg-choice-dualrun-v1 §9) ───────────────

const DUAL_PW: &str = "Rk2dualSecretOnlyOnStdinNeverArgv012345678";
const DUAL_ID: &str = "0123456789ab";

fn dual_pw() -> Vec<u8> {
    serde_json::to_vec(&json!({ "password": DUAL_PW })).unwrap()
}

fn answer(w: &W, needle: &str, a: &str) {
    w.borrow_mut().answers.push((needle.into(), a.into()));
}

/// The A-L2 check's needle: the reserved names in the source database.
const RESERVED: &str = "pg_namespace WHERE nspname = '_k2_dual'";

fn hba_file(t: &T, m: u32) {
    std::fs::write(
        t.p(&format!("/etc/postgresql/{m}/main/pg_hba.conf")),
        "# PostgreSQL Client Authentication\nlocal all postgres peer\nhost replication all 127.0.0.1/32 scram-sha-256\nhost all all 127.0.0.1/32 scram-sha-256\n",
    )
    .unwrap();
}

/// No password anywhere a log could keep it.
fn no_password_leak(t: &T, o: &crate::Outcome) {
    assert!(!String::from_utf8_lossy(&o.stdout).contains(DUAL_PW), "stdout");
    assert!(!o.stderr.contains(DUAL_PW), "stderr: {}", o.stderr);
    assert!(!t.audit.0.borrow().iter().any(|l| l.contains(DUAL_PW)), "audit");
    for (_, a, _) in t.calls() {
        assert!(!a.iter().any(|x| x.contains(DUAL_PW)), "argv: {a:?}");
    }
}

/// PA21/PA22/PA24/L25: the dry run is open and writes nothing; the write
/// is daemon-only; `--if-logical` never introduces `wal_level`.
#[test]
fn logical_conf_dry_run_write_and_if_logical() {
    let (t, w) = pg_box();
    hba_file(&t, 16);
    answer(&w, "SHOW wal_level", "replica\n");
    let conf = t.p("/etc/postgresql/16/main/conf.d/97-k2-logical.conf");
    let hba = t.p("/etc/postgresql/16/main/pg_hba.conf");
    let hba_before = std::fs::read_to_string(&hba).unwrap();
    t.as_agent();
    let v = json(&t.call(&["pg", "logical-conf", "16", "--dry-run"]));
    assert_eq!((v["walLevel"].as_str(), v["restartNeeded"].as_bool()), (Some("replica"), Some(true)), "{v}");
    assert_eq!(t.call(&["pg", "logical-conf", "16", "--wal-keep", "2048"]).code, exit::NOT_DAEMON);
    t.as_daemon();
    let o = t.call(&["pg", "logical-conf", "16", "--wal-keep", "2048", "--if-logical"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("not_logical"), "{}", o.stderr);
    assert!(!conf.exists(), "nothing written");
    assert_eq!(std::fs::read_to_string(&hba).unwrap(), hba_before, "hba untouched");
    // The owner's prepare: the drop-in and the guard, then a reload.
    let v = json(&t.call(&["pg", "logical-conf", "16", "--wal-keep", "2048"]));
    assert_eq!((v["changed"].as_bool(), v["guardChanged"].as_bool(), v["restartNeeded"].as_bool()), (Some(true), Some(true), Some(true)), "{v}");
    assert_eq!(std::fs::read_to_string(&conf).unwrap(), k2_root_proto::pg_dual::logical_conf(2048));
    let h = std::fs::read_to_string(&hba).unwrap();
    assert!(h.find(k2_root_proto::apps::REPL_GUARD_LINE).unwrap() < h.find("host replication all").unwrap(), "{h}");
    assert!(t.non_query_calls().iter().any(|c| c.ends_with("reload postgresql@16-main")), "{:?}", t.non_query_calls());
    // Idempotent; once logical, --if-logical may set the cap.
    w.borrow_mut().answers.clear();
    answer(&w, "SHOW wal_level", "logical\n");
    let v = json(&t.call(&["pg", "logical-conf", "16", "--wal-keep", "2048", "--if-logical"]));
    assert_eq!((v["changed"].as_bool(), v["guardChanged"].as_bool()), (Some(false), Some(false)), "{v}");
    let v = json(&t.call(&["pg", "logical-conf", "16", "--wal-keep", "1024", "--if-logical"]));
    assert_eq!(v["changed"], true);
    assert!(std::fs::read_to_string(&conf).unwrap().contains("max_slot_wal_keep_size = 1024MB"));
    never_destructive(&t);
}

fn dual_box() -> (T, W) {
    let (t, w) = two_clusters();
    mkdirs(&t.root, &["/etc/postgresql/17/main/conf.d", "/var/lib/postgresql/17/main"]);
    hba_file(&t, 16);
    hba_file(&t, 17);
    w.borrow_mut().dbs.get_mut(&5433).unwrap().push("ws_jobs".into());
    (t, w)
}

/// PA23/PA25/PA26: setup on the source, the subscription on the target,
/// from canned SQL; the password only on stdin.
#[test]
fn dual_setup_source_then_subscribe() {
    let (t, w) = dual_box();
    answer(&w, "SHOW wal_level", "logical\n");
    answer(&w, "pg_roles WHERE rolname = 'k2_repl_", "0\n");
    answer(&w, RESERVED, "0\n");
    let argv = ["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID];
    let o = t.call_in(&argv, &dual_pw());
    let v = json(&o);
    no_password_leak(&t, &o);
    assert_eq!(v["role"], "k2_repl_0123456789ab");
    let sql = w.borrow().sql.clone();
    let role = sql.iter().find(|(_, s)| s.contains("CREATE ROLE \"k2_repl_")).expect("role sql");
    assert_eq!(role.0, 5432);
    assert!(role.1.starts_with(k2_root_proto::pg_dual::QUIET_SQL));
    assert!(role.1.contains("LOGIN REPLICATION BYPASSRLS NOINHERIT") && role.1.contains(DUAL_PW));
    let db = sql.iter().find(|(_, s)| s.contains("CREATE PUBLICATION")).expect("db sql");
    assert_eq!(db.0, 5432);
    assert!(db.1.contains("CREATE EVENT TRIGGER k2_dual_freeze"));
    let h = std::fs::read_to_string(t.p("/etc/postgresql/16/main/pg_hba.conf")).unwrap();
    assert!(h.contains(k2_root_proto::apps::REPL_GUARD_LINE), "the guard is there before the role");
    // A-M2: the run's own allow lines above the guard that refuses every
    // other database, all above Debian's `host all all`.
    let allow = h.find("host ws_jobs k2_repl_0123456789ab 127.0.0.1/32 scram-sha-256").expect("allow v4");
    assert!(h.contains("host ws_jobs k2_repl_0123456789ab ::1/128 scram-sha-256"), "{h}");
    let all = h.find(k2_root_proto::apps::REPL_GUARD_ALL_LINE).expect("guard all");
    assert!(allow < all && all < h.find("host all all").unwrap(), "{h}");
    assert!(t.non_query_calls().iter().any(|c| c.ends_with("reload postgresql@16-main")), "{:?}", t.non_query_calls());
    let reserved = sql.iter().position(|(_, s)| s.contains(RESERVED)).expect("reserved names checked");
    assert!(reserved < sql.iter().position(|(_, s)| s.contains("CREATE ROLE")).unwrap());
    assert!(t.audit.0.borrow().iter().any(|l| l.contains("\"verb\":\"pg dual setup-source 16 17\"") && l.contains("\"dualId\":\"0123456789ab\"")));

    answer(&w, "datconnlimit FROM pg_catalog.pg_database", "0,0,0\n");
    answer(&w, "pg_publication WHERE pubname", "1\n");
    let o = t.call_in(&["pg", "dual", "subscribe", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
    ok(&o);
    no_password_leak(&t, &o);
    let sub = w.borrow().sql.iter().find(|(_, s)| s.contains("CREATE SUBSCRIPTION")).cloned().expect("sub sql");
    assert_eq!(sub.0, 5433, "the subscription lives on the target");
    assert!(sub.1.contains("port=5432 dbname=ws_jobs user=k2_repl_0123456789ab") && sub.1.contains("run_as_owner = false"));
    // An agent: refused before anything.
    t.as_agent();
    assert_eq!(t.call_in(&argv, &dual_pw()).code, exit::NOT_DAEMON);
}

#[test]
fn dual_refusals_change_nothing() {
    // Not logical: no role, no guard.
    let (t, w) = dual_box();
    answer(&w, "SHOW wal_level", "replica\n");
    let o = t.call_in(&["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("not_logical"), "{}", o.stderr);
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE")));
    assert!(!std::fs::read_to_string(t.p("/etc/postgresql/16/main/pg_hba.conf")).unwrap().contains("k2_repl"));
    // A second setup of the same id.
    let (t, w) = dual_box();
    answer(&w, "SHOW wal_level", "logical\n");
    answer(&w, "pg_roles WHERE rolname = 'k2_repl_", "1\n");
    let o = t.call_in(&["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
    assert!(o.stderr.starts_with("dual_exists"), "{}", o.stderr);
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE")));
    // A-L2: a tenant's own `_k2_dual` schema / `k2_dual_<id>` publication /
    // `k2_dual_freeze` trigger: refused, never taken over (teardown would
    // drop them by name).
    let (t, w) = dual_box();
    answer(&w, "SHOW wal_level", "logical\n");
    answer(&w, "pg_roles WHERE rolname = 'k2_repl_", "0\n");
    answer(&w, RESERVED, "1\n");
    let o = t.call_in(&["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("reserved_names"), "{}", o.stderr);
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE ROLE") || s.contains("CREATE PUBLICATION")));
    assert!(!std::fs::read_to_string(t.p("/etc/postgresql/16/main/pg_hba.conf")).unwrap().contains("k2_repl"), "hba untouched");
    // Not a K2 database.
    let (t, w) = dual_box();
    answer(&w, "SHOW wal_level", "logical\n");
    w.borrow_mut().owners.insert((5432, "ws_jobs".into()), "alice".into());
    let o = t.call_in(&["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
    assert!(o.stderr.starts_with("not_k2_database"), "{}", o.stderr);
    // A bad password or extra keys: refused as input.
    let (t, w) = dual_box();
    for bad in [
        &br#"{"password":"short"}"#[..],
        br#"{"password":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; DROP"}"#,
        br#"{"password":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sql":"x"}"#,
        b"nope",
    ] {
        let o = t.call_in(&["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID], bad);
        assert_eq!(o.code, exit::ARGV, "{}", String::from_utf8_lossy(bad));
    }
    assert!(w.borrow().sql.is_empty());
    // Subscribe into an open target, one with FORCE still on, or one with
    // a subscription already.
    for state in ["-1,0,0", "0,2,0", "0,0,1"] {
        let (t, w) = dual_box();
        answer(&w, "datconnlimit FROM pg_catalog.pg_database", &format!("{state}\n"));
        answer(&w, "pg_publication WHERE pubname", "1\n");
        let o = t.call_in(&["pg", "dual", "subscribe", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
        assert!(o.stderr.starts_with("target_not_ready"), "{state}: {}", o.stderr);
        assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("CREATE SUBSCRIPTION")), "{state}");
    }
    // A failing CREATE ROLE whose psql echo carries the password: masked.
    let (t, w) = dual_box();
    answer(&w, "SHOW wal_level", "logical\n");
    answer(&w, "pg_roles WHERE rolname = 'k2_repl_", "0\n");
    answer(&w, RESERVED, "0\n");
    w.borrow_mut().fail_on =
        Some(("CREATE ROLE".into(), format!("ERROR:  boom at '{DUAL_PW}'\nLINE 1: CREATE ROLE x LOGIN '{DUAL_PW}'")));
    let o = t.call_in(&["pg", "dual", "setup-source", "16", "17", "ws_jobs", DUAL_ID], &dual_pw());
    assert_eq!(o.code, exit::IO);
    assert!(o.stderr.contains("setup-source: the replication role") && o.stderr.contains("***"), "{}", o.stderr);
    assert!(!o.stderr.contains("LINE 1"), "{}", o.stderr);
    no_password_leak(&t, &o);
}

/// PA31 step 4 / PA34: detach drops the subscription (slot name cleared
/// first) then the source's objects; teardown also drops the role and
/// carries on when the target cluster is down; status is open.
#[test]
fn dual_detach_teardown_and_status() {
    let (t, w) = dual_box();
    let hba = t.p("/etc/postgresql/16/main/pg_hba.conf");
    let with_run = k2_root_proto::apps::hba_dual(&std::fs::read_to_string(&hba).unwrap(), "ws_jobs", DUAL_ID, true).unwrap().unwrap();
    std::fs::write(&hba, &with_run).unwrap();
    answer(&w, "pg_subscription WHERE subname", "1\n");
    let v = json(&t.call(&["pg", "dual", "detach", "16", "17", "ws_jobs", DUAL_ID]));
    assert_eq!((v["subscriptionDropped"].as_bool(), v["roleDropped"].as_bool()), (Some(true), Some(false)), "{v}");
    // A-M2: the run's allow lines go at detach; the guards stay.
    assert_eq!(v["hbaChanged"], true, "{v}");
    let h = std::fs::read_to_string(&hba).unwrap();
    assert!(!h.contains("k2_repl_0123456789ab") && h.contains(k2_root_proto::apps::REPL_GUARD_ALL_LINE), "{h}");
    let sql = w.borrow().sql.clone();
    let drop_sub = sql.iter().position(|(p, s)| *p == 5433 && s.contains("SET (slot_name = NONE)")).expect("subscription dropped on the target");
    let src = sql
        .iter()
        .position(|(p, s)| *p == 5432 && s.contains("DROP EVENT TRIGGER k2_dual_freeze"))
        .expect("source detached");
    assert!(drop_sub < src, "the subscription before the slot");
    assert!(!sql.iter().any(|(_, s)| s.contains("DROP ROLE")), "detach keeps the role");
    let v = json(&t.call(&["pg", "dual", "teardown", "16", "17", "ws_jobs", DUAL_ID]));
    assert_eq!(v["roleDropped"], true);
    assert!(w.borrow().sql.iter().any(|(p, s)| *p == 5432 && s.contains("DROP ROLE \"k2_repl_0123456789ab\"")));
    // The target cluster down: teardown still cleans the source, detach refuses.
    let (t, w) = dual_box();
    w.borrow_mut().clusters[1].2 = false;
    let o = t.call(&["pg", "dual", "detach", "16", "17", "ws_jobs", DUAL_ID]);
    assert_eq!(o.code, exit::STATE);
    assert!(w.borrow().sql.iter().all(|(_, s)| !s.contains("DROP EVENT TRIGGER")), "detach refused before the source");
    let v = json(&t.call(&["pg", "dual", "teardown", "16", "17", "ws_jobs", DUAL_ID]));
    assert!(v["notes"].to_string().contains("target cluster is down"), "{v}");
    assert!(w.borrow().sql.iter().any(|(p, s)| *p == 5432 && s.contains("pg_drop_replication_slot")));
    // A-M1: a closed source (the kept copy after a switch) is opened to
    // the helper alone (connection limit 0 first), cleaned, closed again,
    // and the role dropped; closed again even when the cleanup fails.
    for fail in [false, true] {
        let (t, w) = dual_box();
        answer(&w, "datallowconn::text FROM pg_catalog.pg_database", "false\n");
        answer(&w, "pg_subscription WHERE subname", "0\n");
        if fail {
            w.borrow_mut().fail_on = Some(("DROP EVENT TRIGGER".into(), "ERROR:  boom".into()));
        }
        let o = t.call(&["pg", "dual", "teardown", "16", "17", "ws_jobs", DUAL_ID]);
        let sql = w.borrow().sql.clone();
        let open = sql.iter().position(|(p, s)| *p == 5432 && s.contains("CONNECTION LIMIT 0;\nALTER DATABASE \"ws_jobs\" ALLOW_CONNECTIONS true")).expect("opened");
        let clean = sql.iter().position(|(p, s)| *p == 5432 && s.contains("DROP EVENT TRIGGER")).expect("cleanup ran");
        let close = sql.iter().position(|(p, s)| *p == 5432 && s.contains("ALLOW_CONNECTIONS false")).expect("closed again");
        assert!(open < clean && clean < close, "{fail}");
        if fail {
            assert_ne!(o.code, exit::OK);
            assert!(!sql.iter().any(|(_, s)| s.contains("DROP ROLE")), "a failed cleanup keeps the role (its grants would block the drop)");
        } else {
            let v = json(&o);
            assert_eq!(v["roleDropped"], true);
            let drop = sql.iter().position(|(p, s)| *p == 5432 && s.contains("DROP ROLE \"k2_repl_0123456789ab\"")).expect("role dropped");
            assert!(close < drop);
            assert!(v["notes"].to_string().contains("opened to the helper alone"), "{v}");
        }
    }
    // Status: read-only, each side its own answer; the daemon's alone (A-L3).
    let (t, w) = dual_box();
    answer(&w, "'walLevel', pg_catalog.current_setting", r#"{"walLevel":"logical","slot":{"active":true,"lagBytes":12}}"#);
    answer(&w, "'subscription', EXISTS", r#"{"subscription":true,"tablesReady":3,"tablesTotal":3}"#);
    t.as_agent();
    assert_eq!(t.call(&["pg", "dual", "status", "16", "17", "ws_jobs", DUAL_ID]).code, exit::NOT_DAEMON);
    assert!(w.borrow().sql.is_empty(), "refused before any query");
    t.as_daemon();
    let v = json(&t.call(&["pg", "dual", "status", "16", "17", "ws_jobs", DUAL_ID]));
    assert_eq!(v["source"]["slot"]["lagBytes"], 12, "{v}");
    assert_eq!(v["target"]["tablesReady"], 3, "{v}");
    assert!(!w.borrow().sql.iter().any(|(_, s)| s.contains("DROP") || s.contains("CREATE")), "status changes nothing");
    never_destructive(&t);
}

/// PA25/PA31: the schema-only dump, FORCE kept off then put back on the
/// recorded tables only, and the write block that spares the walsender.
#[test]
fn schema_only_dump_force_kept_off_force_back_and_quiesce_repl() {
    let (t, w) = two_clusters();
    w.borrow_mut().dbs.get_mut(&5433).unwrap().push("ws_jobs".into());
    ok(&t.call(&["pg", "move-dump", "16", "17", "ws_jobs", "20261024T101500Z", "--schema-only"]));
    assert!(
        w.borrow().postgres_calls.iter().any(|c| c.contains("pg_dump -Fc --schema-only --no-subscriptions")),
        "{:?}",
        w.borrow().postgres_calls
    );
    w.borrow_mut().forced = "16500,16501".into();
    let v = json(&t.call_in(&["pg", "move-restore", "17", "ws_jobs", "20261024T101500Z", "--keep-force-off"], &login("ws_jobs_migrator")));
    assert_eq!(v["forcedOids"], json!([16500, 16501]));
    assert_eq!(v["forceKeptOff"], true);
    let force_sql: Vec<String> = w.borrow().sql.iter().filter(|(_, s)| s.contains("FORCE ROW LEVEL SECURITY")).map(|(_, s)| s.clone()).collect();
    assert_eq!(force_sql.len(), 1, "lifted once, never put back: {force_sql:?}");
    assert!(force_sql[0].contains(" NO FORCE ROW LEVEL SECURITY"), "{force_sql:?}");
    // force-rls: exactly those tables, checked in the database.
    let v = json(&t.call_in(&["pg", "force-rls", "17", "ws_jobs", "on"], br#"{"oids":[16500,16501]}"#));
    assert_eq!(v["major"], 17);
    let f = w.borrow().sql.last().cloned().unwrap();
    assert_eq!(f.0, 5433);
    assert!(f.1.contains("ARRAY[16500, 16501]") && f.1.contains("RAISE EXCEPTION"));
    for bad in [&br#"{"oids":[1259]}"#[..], br#"{"oids":"16500"}"#, br#"{"oids":[16500],"sql":"x"}"#] {
        assert_eq!(t.call_in(&["pg", "force-rls", "17", "ws_jobs", "on"], bad).code, exit::ARGV, "{}", String::from_utf8_lossy(bad));
    }
    // quiesce-repl: CONNECTION LIMIT 0, every session but the walsender.
    ok(&t.call(&["pg", "allow-conn", "16", "ws_jobs", "quiesce-repl"]));
    let (port, sql) = w.borrow().sql.last().cloned().unwrap();
    assert_eq!(port, 5432);
    assert!(sql.starts_with("ALTER DATABASE \"ws_jobs\" CONNECTION LIMIT 0;") && sql.contains("backend_type <> 'walsender'"), "{sql}");
    assert!(sql.contains("RAISE EXCEPTION 'k2_quiesce_busy"), "B2: waits for the sessions to exit: {sql}");
    // Sessions that outlive the wait: the error reaches the daemon.
    w.borrow_mut().fail_on = Some(("k2_quiesce_busy".into(), "ERROR:  k2_quiesce_busy: sessions of ws_jobs did not exit".into()));
    let o = t.call(&["pg", "allow-conn", "16", "ws_jobs", "quiesce-repl"]);
    assert_ne!(o.code, exit::OK);
    assert!(o.stderr.contains("k2_quiesce_busy"), "{}", o.stderr);
    w.borrow_mut().fail_on = None;
    t.as_agent();
    assert_eq!(t.call_in(&["pg", "force-rls", "17", "ws_jobs", "on"], br#"{"oids":[16500]}"#).code, exit::NOT_DAEMON);
}

#[test]
fn version_announces_pgdual_on_protocol_10() {
    let t = box_root();
    let v = json(&t.call(&["helper", "version"]));
    assert_eq!(v["protocol"], 11, "pgdual came in 10 (additive)");
    assert_eq!(v["namespaces"]["pgdual"], true);
    assert_eq!(v["namespaces"]["creds"], true);
    assert_eq!(k2_root_proto::version::PGDUAL_MIN_PROTOCOL, 10);
}
