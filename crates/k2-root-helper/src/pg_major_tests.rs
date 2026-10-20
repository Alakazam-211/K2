//! `pg` protocol 9 (several majors) over the temp root and the fakes of
//! `tests.rs`. A small fake box answers `pg_lsclusters`, `ss`, apt, dpkg,
//! `pg_createcluster`, `systemctl` and the postgres-side programs (`psql`,
//! `pg_dump`, `pg_restore`, `stat`, `rm`). Nothing real runs.

use super::*;
use serde_json::json;

use k2_root_proto::pg_major::{
    AuthRow, APT_MARK, CREATECLUSTER_DROPIN, DPKG_QUERY, ENV as ENV_BIN, PGDG_COMMON_KEY, PGDG_KEYRING, PGDG_KEY_ASC,
    PGDG_PREF, PGDG_SOURCES, PG_CREATECLUSTER, PG_LSCLUSTERS, SEW_TOML, SLICE_DROPIN_PATH, SLICE_UNIT_PATH, SS,
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
        ..Default::default()
    }));
    let root = t.root.clone();
    mkdirs(&root, &["/etc/apt/sources.list.d", "/etc/apt/preferences.d", "/etc/postgresql/16/main/conf.d", "/var/lib/postgresql/16/main"]);
    std::fs::write(under(&root, "/etc/os-release"), "NAME=\"Ubuntu\"\nVERSION_CODENAME=noble\n").unwrap();
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
        Rc::new(move |_| {
            let s: String = ww.borrow().dpkg.iter().map(|p| format!("{p} install ok installed\n")).collect();
            out(0, &s)
        }),
    );
    log.programs.insert(APT_MARK.into(), Rc::new(|_| out(0, "")));
    let ww = w.clone();
    log.programs_in.insert(
        RUNUSER.into(),
        Rc::new(move |a, stdin| {
            assert_eq!(&a[..3], &["-u", "postgres", "--"], "everything runs as postgres: {a:?}");
            let prog = a[3].rsplit('/').next().unwrap().to_string();
            let mut w = ww.borrow_mut();
            match prog.as_str() {
                "psql" => {
                    let port: u16 = arg_after(a, "-p").map(|p| p.parse().unwrap()).unwrap_or(5432);
                    let sql = String::from_utf8_lossy(stdin.unwrap_or_default()).to_string();
                    w.sql.push((port, sql.clone()));
                    if sql.contains("FROM pg_authid") {
                        let rows = w.roles.get(&port).cloned().unwrap_or_default();
                        let want: Vec<AuthRow> = rows.into_iter().filter(|r| sql.contains(&format!("'{}'", r.rolname))).collect();
                        return out(0, &serde_json::to_string(&want).unwrap());
                    }
                    if let Some(rest) = sql.split("WHERE datname = '").nth(1) {
                        if sql.starts_with("SELECT count(*) FROM pg_database") {
                            let db = rest.split('\'').next().unwrap();
                            let has = w.dbs.get(&port).is_some_and(|d| d.iter().any(|x| x == db));
                            return out(0, if has { "1\n" } else { "0\n" });
                        }
                        return out(0, "2\n");
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
                "mkdir" => out(0, ""),
                "stat" => {
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
                "env" => out(0, ""),
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
    std::fs::write(t.p(SEW_TOML), "[db]\npg_major = 16\n").unwrap();
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
    assert_eq!(cl[0]["unit"], "postgresql@16-main");
    assert!(!v.to_string().contains("/var/lib/sew"), "{v}");
    assert_eq!(v["installed"], json!([16]));
    assert_eq!(v["codename"], "noble");
    assert_eq!(v["distroMajor"], 16);
    assert_eq!(v["sew"]["present"], true);
    assert_eq!(v["sew"]["pgMajor"], 16);
    assert_eq!(v["bandListening"], json!([5432, 5433]));
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
    assert!(apt[2].contains("install -y -q --no-install-recommends --no-upgrade") && apt[2].ends_with("postgresql-17 postgresql-client-17"));
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
    std::fs::write(t.p("/etc/os-release"), "ID=fedora\nVERSION_CODENAME=\n").unwrap();
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
    assert_eq!(v, json!({"major": 17, "port": 5433, "created": true, "started": true, "unit": "postgresql@17-main"}));
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
    assert_eq!(std::fs::read_to_string(t.p(SLICE_DROPIN_PATH)).unwrap(), k2_root_proto::pg_major::slice_dropin());
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
        ("not installed", Box::new(|_, _| {}), "major_not_installed"),
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
    ];
    for (what, setup, want) in cases {
        let (t, w) = pg_box();
        setup(&t, &w);
        let o = t.call(&["pg", "ensure-cluster", "17", "5433"]);
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
    t.as_agent();
    assert_eq!(t.call(&["pg", "start-conf", "16", "manual"]).code, exit::NOT_DAEMON);
    ok(&t.call(&["pg", "start-conf", "16", "auto"]));
    let o = t.call(&["pg", "start-conf", "17", "auto"]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("no_cluster"), "{}", o.stderr);
    let v = json(&t.call(&["pg", "install-slice"]));
    assert_eq!(v["changed"], true);
    assert_eq!(json(&t.call(&["pg", "install-slice"]))["changed"], false);
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
    assert!(sql[0].starts_with("BEGIN;\n") && sql[0].ends_with("COMMIT;\n"));
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

#[test]
fn dump_restore_use_the_targets_binaries_as_postgres() {
    let (t, w) = two_clusters();
    w.borrow_mut().dbs.get_mut(&5433).unwrap().push("ws_jobs".into());
    let tag = "20261020T101500Z";
    let v = json(&t.call(&["pg", "move-dump", "16", "17", "ws_jobs", tag]));
    let file = "/var/lib/postgresql/k2-moves/ws_jobs-20261020T101500Z.dump";
    assert_eq!(v["file"], file);
    assert_eq!(v["bytes"], 4096);
    let calls = t.non_query_calls();
    assert!(calls.contains(&format!("{RUNUSER} -u postgres -- /usr/bin/mkdir -p -m 0700 -- /var/lib/postgresql/k2-moves")), "{calls:?}");
    assert!(calls.contains(&format!(
        "{RUNUSER} -u postgres -- /usr/lib/postgresql/17/bin/pg_dump -Fc --no-subscriptions --no-publications -h /var/run/postgresql -p 5432 -f {file} -d ws_jobs"
    )), "17's pg_dump against 16's port: {calls:?}");
    // The same tag twice: refused, nothing overwritten.
    let o = t.call(&["pg", "move-dump", "16", "17", "ws_jobs", tag]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("dump_exists"));

    let v = json(&t.call(&["pg", "move-restore", "17", "ws_jobs", tag]));
    assert_eq!(v["to"], 17);
    assert!(t.non_query_calls().contains(&format!(
        "{RUNUSER} -u postgres -- /usr/bin/env PGOPTIONS=-c temp_file_limit=-1 -c statement_timeout=0 -c lock_timeout=0 -c idle_in_transaction_session_timeout=0 /usr/lib/postgresql/17/bin/pg_restore --exit-on-error -h /var/run/postgresql -p 5433 -d ws_jobs {file}"
    )));
    // No target database: refused (create it first).
    let o = t.call(&["pg", "move-restore", "17", "ws_crm", tag]);
    assert_eq!(o.code, exit::STATE);
    assert!(o.stderr.starts_with("dump_missing"), "{}", o.stderr);
    w.borrow_mut().files.insert("/var/lib/postgresql/k2-moves/ws_crm-20261020T101500Z.dump".into());
    let o = t.call(&["pg", "move-restore", "17", "ws_crm", tag]);
    assert!(o.stderr.starts_with("no_database"), "{}", o.stderr);

    let v = json(&t.call(&["pg", "move-file-remove", "ws_jobs", tag]));
    assert_eq!(v["removed"], true);
    assert!(!w.borrow().files.contains(file));
    assert_eq!(json(&t.call(&["pg", "move-file-remove", "ws_jobs", tag]))["removed"], false);

    let a = t.audit.0.borrow();
    assert!(a.iter().any(|l| l.contains("\"verb\":\"pg move-dump 16 17\"") && l.contains("\"db\":\"ws_jobs\"") && l.contains(tag)));
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
        "{RUNUSER} -u postgres -- /usr/lib/postgresql/17/bin/psql -X -h /var/run/postgresql -p 5433 -d postgres -v ON_ERROR_STOP=1 -tA"
    )), "{:?}", t.non_query_calls());
    assert_eq!(w.borrow().sql.last().unwrap().0, 5433);
    // Without --cluster: pg_wrapper's psql as before (protocol 3).
    ok(&t.call_in(&["pg", "psql", "-d", "postgres"], b"SELECT 1;\n"));
    assert!(t.non_query_calls().contains(&format!("{RUNUSER} -u postgres -- {PSQL} -X -d postgres")));
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
    assert_eq!(v["protocol"], 9);
    assert_eq!(v["namespaces"]["pgmajor"], true);
    assert_eq!(v["namespaces"]["pg"], true, "protocols are additive");
    assert!(PROTOCOL >= k2_root_proto::version::PGMAJOR_MIN_PROTOCOL);
}
