//! `pg` namespace, protocol 9 (`namespaces.pgmajor`): several Postgres
//! majors side by side, one Debian cluster per major
//! (`prd-pg17-and-db-upgrade-v1` rev 2 §4–§6, S2).
//!
//! Rules on top of the namespace's:
//! - Majors come from `k2_root_proto::pg_major::INSTALLABLE`; ports from
//!   K2's band; paths are fixed or built from validated tokens only.
//! - Everything that touches a database runs as `postgres` through
//!   `runuser`, with the major's own binaries (`/usr/lib/postgresql/<M>/bin`)
//!   against the port `pg_lsclusters` reports. Never a shell, never SQL
//!   from the caller (the one exception is `pg psql`, as before).
//! - K2 never removes or changes an installed Postgres server package
//!   (Sew's own Postgres runs on one, Sew 2026-10-20) and never drops a
//!   cluster.
//! - Sew's private Postgres (`/var/lib/sew/db`, `sew-db.service`) is
//!   never a cluster here: not listed, not counted, not touched.

use std::ffi::OsStr;

use k2_root_proto::apps::APT_GET;
use k2_root_proto::paths::{PG_OS_USER, RUNUSER, SYSTEMCTL};
use k2_root_proto::pg_major::*;
use k2_root_proto::templates::pg_guc_conf;
use k2_root_proto::verbs::{ConnMode, DbName};
use serde_json::{json, Value as J};

use crate::apps::{ensure_subdir, put};
use crate::ctx::{Ctx, ExecOut, Fail, HResult, KindStr};
use crate::fsx::{Dir, Kind};

const FE: &str = "DEBIAN_FRONTEND=noninteractive";

/// Seconds the helper waits for a cluster it just started to answer.
const START_WAIT_TRIES: u32 = 30;

// ── small fs helpers (fixed paths only) ─────────────────────────────────

fn present(ctx: &Ctx, abs: &str) -> bool {
    ctx.p(abs).symlink_metadata().is_ok()
}

fn split_dyn(abs: &str) -> (&str, &str) {
    let i = abs.rfind('/').expect("absolute");
    (if i == 0 { "/" } else { &abs[..i] }, &abs[i + 1..])
}

/// A small text file under a fixed directory, without following links
/// (`None` when missing).
fn read_small(ctx: &Ctx, abs: &str, cap: usize) -> HResult<Option<Vec<u8>>> {
    let (parent, name) = split_dyn(abs);
    let d = match ctx.dir(parent) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Fail::io_err("open", e)),
    };
    d.read_file(OsStr::new(name), cap).map_err(|e| Fail::io_err("read", e))
}

/// A fixed directory, each missing component created 0755 root (a link
/// or a file anywhere on the way is a refusal).
fn subdir(ctx: &Ctx, abs: &str) -> HResult<Dir> {
    match ctx.dir(abs) {
        Ok(d) => return Ok(d),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Fail::io_err("open", e)),
    }
    let (parent, name) = split_dyn(abs);
    if parent != "/" {
        subdir(ctx, parent)?;
    }
    ensure_subdir(ctx, parent, name)
}

/// Canned bytes into a fixed file (0644 root), `Ok(true)` when changed.
fn put_at(ctx: &Ctx, abs: &str, bytes: &[u8]) -> HResult<bool> {
    let (parent, name) = split_dyn(abs);
    let d = subdir(ctx, parent)?;
    put(ctx, &d, name, bytes)
}

fn today_days() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    secs.div_euclid(86_400)
}

fn trimmed(err: &[u8]) -> String {
    let e = crate::pg::redact_passwords(&String::from_utf8_lossy(err));
    e.trim().chars().take(1500).collect()
}

// ── what the box has ────────────────────────────────────────────────────

/// Majors whose server binaries are installed.
pub fn installed_majors(ctx: &Ctx) -> Vec<u32> {
    INSTALLABLE
        .iter()
        .copied()
        .chain([12, 13, 14, 15])
        .filter(|m| {
            let mm = format!("{PG_LIB}/{m}/bin");
            present(ctx, &format!("{mm}/postgres")) && present(ctx, &format!("{mm}/pg_dump"))
        })
        .collect::<std::collections::BTreeSet<u32>>()
        .into_iter()
        .collect()
}

fn major_installed(ctx: &Ctx, m: PgMajor) -> bool {
    present(ctx, &m.bin("postgres")) && present(ctx, &m.bin("pg_dump")) && present(ctx, &m.bin("psql"))
}

/// `pg_lsclusters -h` (empty when postgresql-common isn't installed).
pub fn lsclusters(ctx: &Ctx) -> HResult<Vec<ClusterRow>> {
    if !present(ctx, PG_LSCLUSTERS) {
        return Ok(Vec::new());
    }
    let out = ctx.run_ok(PG_LSCLUSTERS, &["-h"], None)?;
    Ok(parse_lsclusters(&String::from_utf8_lossy(&out.stdout)))
}

/// The K2-layout cluster of `m` (`<M>/main` under /var/lib/postgresql).
fn cluster_of(ctx: &Ctx, m: PgMajor) -> HResult<ClusterRow> {
    let rows = lsclusters(ctx)?;
    let row = rows
        .into_iter()
        .find(|r| r.major == m.get() && r.name == "main")
        .ok_or_else(|| Fail::state(format!("no_cluster: no {}/main cluster on this box", m.get())))?;
    if !k2_layout(&row) {
        return Err(Fail::state("cluster_layout_refused: the cluster's data dir is outside /var/lib/postgresql/<M>/main"));
    }
    Ok(row)
}

fn sew_status(ctx: &Ctx) -> (bool, Option<u32>) {
    let toml = read_small(ctx, SEW_TOML, 256 * 1024).ok().flatten();
    let present = toml.is_some() || SEW_DB_UNITS.iter().any(|u| present(ctx, u));
    (present, toml.and_then(|t| sew_pg_major(&String::from_utf8_lossy(&t))))
}

/// `/etc/os-release`, else `/usr/lib/os-release` (os-release(5)): on
/// Ubuntu and Debian the first is a link to the second, and the helper
/// never follows a link, so the vendor file is read directly.
fn codename(ctx: &Ctx) -> Option<String> {
    let t = read_small(ctx, OS_RELEASE, 64 * 1024)
        .ok()
        .flatten()
        .or_else(|| read_small(ctx, OS_RELEASE_VENDOR, 64 * 1024).ok().flatten())?;
    os_release_codename(&String::from_utf8_lossy(&t))
}

fn listening(ctx: &Ctx) -> HResult<Vec<u16>> {
    if !present(ctx, SS) {
        return Err(Fail::state("ss (iproute2) is missing: can't check the port"));
    }
    let out = ctx.run_ok(SS, &["-ltnH"], None)?;
    Ok(listening_ports(&String::from_utf8_lossy(&out.stdout)))
}

fn start_conf_of(ctx: &Ctx, m: u32) -> Option<String> {
    let t = read_small(ctx, &format!("/etc/postgresql/{m}/main/start.conf"), 4096).ok().flatten()?;
    String::from_utf8_lossy(&t)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
}

/// `pg clusters` (open, read-only).
pub fn clusters(ctx: &Ctx) -> HResult<J> {
    let rows = lsclusters(ctx)?;
    let (sew, sew_major) = sew_status(ctx);
    let code = codename(ctx);
    let listening_band: Vec<u16> = listening(ctx)
        .map(|v| {
            let mut b: Vec<u16> = v.into_iter().filter(|p| port_in_band(*p)).collect();
            b.sort_unstable();
            b.dedup();
            b
        })
        .unwrap_or_default();
    Ok(json!({
        "clusters": rows.iter().map(|r| json!({
            "major": r.major,
            "name": r.name,
            "port": r.port,
            "status": r.status,
            "online": r.online(),
            "owner": r.owner,
            "dataDir": r.data_dir,
            "k2Layout": k2_layout(r),
            "unit": format!("postgresql@{}-{}", r.major, r.name),
            "startConf": start_conf_of(ctx, r.major),
        })).collect::<Vec<_>>(),
        "installed": installed_majors(ctx),
        "codename": code,
        "distroMajor": code.as_deref().and_then(distro_major),
        "apt": {
            "keyring": present(ctx, PGDG_KEYRING),
            "source": present(ctx, PGDG_SOURCES),
            "foreignSource": PGDG_FOREIGN_SOURCES.iter().any(|p| present(ctx, p)),
            "pins": present(ctx, PGDG_PREF),
            "createclusterDropin": present(ctx, CREATECLUSTER_DROPIN),
        },
        "slice": {"unit": present(ctx, SLICE_UNIT_PATH), "dropin": present(ctx, SLICE_DROPIN_PATH)},
        "bandListening": listening_band,
        "sew": {"present": sew, "pgMajor": sew_major},
    }))
}

// ── add-major ───────────────────────────────────────────────────────────

fn key_norm(b: &[u8]) -> String {
    String::from_utf8_lossy(b).split_whitespace().collect::<Vec<_>>().join("\n")
}

/// `pg add-major <M> [--dry-run]`.
pub fn add_major(ctx: &Ctx, m: PgMajor, dry_run: bool) -> HResult<J> {
    if past_eol(m.get(), today_days()) {
        return Err(Fail::state("major_past_eol: that Postgres major is past its end of life"));
    }
    let installed = major_installed(ctx, m);
    let code = codename(ctx);
    let distro = code.as_deref().and_then(distro_major);
    let (sew, sew_major) = sew_status(ctx);
    let pgdg = !installed && distro != Some(m.get());
    let plan = json!({
        "major": m.get(),
        "codename": code,
        "distroMajor": distro,
        "installed": installed,
        "fromPgdg": pgdg,
        "packages": if installed { Vec::new() } else { packages(m.get()).to_vec() },
        "files": {
            "createclusterDropin": CREATECLUSTER_DROPIN,
            "keyring": pgdg.then_some(PGDG_KEYRING),
            "source": pgdg.then_some(PGDG_SOURCES),
            "pins": pgdg.then_some(PGDG_PREF),
        },
        "sew": {"present": sew, "pgMajor": sew_major},
    });
    if !installed && distro.is_none() {
        return Err(Fail::state(
            "unsupported_distro: K2 installs Postgres majors on Ubuntu jammy/noble and Debian bookworm/trixie; \
             install postgresql-<M> and postgresql-client-<M> yourself, then re-run",
        ));
    }
    if dry_run {
        let mut p = plan;
        p["dryRun"] = json!(true);
        return Ok(p);
    }
    // Before any install: apt never makes a surprise cluster (V9).
    put_at(ctx, CREATECLUSTER_DROPIN, createcluster_dropin().as_bytes())?;
    if installed {
        let marked = mark_manual(ctx)?;
        let mut p = plan;
        p["result"] = json!("already_installed");
        p["markedManual"] = json!(marked);
        return Ok(p);
    }
    if !present(ctx, APT_GET) {
        return Err(Fail::state("no apt-get on this box"));
    }
    let code = code.expect("distro checked above");
    let mut source = "distro";
    if pgdg {
        if let Some(theirs) = read_small(ctx, PGDG_COMMON_KEY, 64 * 1024)? {
            if key_norm(&theirs) != key_norm(PGDG_KEY_ASC.as_bytes()) {
                return Err(Fail::state(
                    "pgdg_key_mismatch: postgresql-common's copy of the PGDG key differs from the one K2 carries; nothing changed",
                ));
            }
        }
        put_at(ctx, PGDG_KEYRING, PGDG_KEY_ASC.as_bytes())?;
        if PGDG_FOREIGN_SOURCES.iter().any(|p| present(ctx, p)) {
            // apt refuses one URI with two Signed-By values: keep theirs.
            source = "foreign";
        } else {
            put_at(ctx, PGDG_SOURCES, pgdg_sources(&code).expect("allow-listed").as_bytes())?;
            source = "k2";
        }
        put_at(ctx, PGDG_PREF, pgdg_pref(&code).expect("allow-listed").as_bytes())?;
    }
    ctx.run_ok(ENV, &[FE, APT_GET, "update", "-q"], None)?;
    let pk = packages(m.get());
    let opts = [
        "-q",
        "--no-install-recommends",
        "--no-upgrade",
        "-o",
        "Dpkg::Options::=--force-confdef",
        "-o",
        "Dpkg::Options::=--force-confold",
    ];
    let mut sim = vec![FE, APT_GET, "install", "-s"];
    sim.extend_from_slice(&opts);
    sim.extend(pk.iter().map(String::as_str));
    let out = ctx
        .exec
        .run(ENV, &sim.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None)
        .map_err(|e| Fail::io(format!("apt-get: {}", e.kind_str())))?;
    if out.status != 0 {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("Unable to locate package") || err.contains("has no installation candidate") {
            return Err(Fail::state(format!(
                "pgdg_unreachable: apt has no postgresql-{0} (no network to apt.postgresql.org?). Install \
                 postgresql-{0} and postgresql-client-{0} from your mirror (apt install ./*.deb), then re-run",
                m.get()
            )));
        }
        return Err(Fail::io(format!("apt-get install -s exit {}: {}", out.status, trimmed(&out.stderr))));
    }
    if let Some(why) = apt_sim_refusal(&String::from_utf8_lossy(&out.stdout), sew_major) {
        return Err(Fail::state(why));
    }
    let mut inst = vec![FE, APT_GET, "install", "-y"];
    inst.extend_from_slice(&opts);
    inst.extend(pk.iter().map(String::as_str));
    ctx.run_ok(ENV, &inst, None)?;
    if !major_installed(ctx, m) {
        return Err(Fail::io(format!(
            "apt finished but {} is missing",
            m.bin("postgres")
        )));
    }
    let marked = mark_manual(ctx)?;
    let mut p = plan;
    p["result"] = json!("installed");
    p["source"] = json!(source);
    p["markedManual"] = json!(marked);
    Ok(p)
}

/// `apt-mark manual` every installed `postgresql-<n>` server and client:
/// an autoremove never takes one away.
fn mark_manual(ctx: &Ctx) -> HResult<Vec<String>> {
    if !present(ctx, k2_root_proto::pg_major::DPKG_QUERY) || !present(ctx, APT_MARK) {
        return Ok(Vec::new());
    }
    let out = ctx
        .exec
        .run(
            DPKG_QUERY,
            &["-W".into(), "-f".into(), "${Package} ${Status}\\n".into(), "postgresql-*".into()],
            None,
        )
        .map_err(|e| Fail::io(format!("dpkg-query: {}", e.kind_str())))?;
    let pkgs: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.ends_with("install ok installed"))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|p| {
            let rest = p.strip_prefix("postgresql-client-").or_else(|| p.strip_prefix("postgresql-"));
            rest.is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_string)
        .collect();
    if !pkgs.is_empty() {
        let mut a = vec!["manual".to_string()];
        a.extend(pkgs.iter().cloned());
        let refs: Vec<&str> = a.iter().map(String::as_str).collect();
        ctx.run_ok(APT_MARK, &refs, None)?;
    }
    Ok(pkgs)
}

// ── clusters ────────────────────────────────────────────────────────────

/// The shared slice (`pg install-slice`, and every `ensure-cluster`).
pub fn install_slice(ctx: &Ctx) -> HResult<J> {
    let a = put_at(ctx, SLICE_UNIT_PATH, slice_unit().as_bytes())?;
    let b = put_at(ctx, SLICE_DROPIN_PATH, slice_dropin().as_bytes())?;
    if a || b {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    // A running instance moves into the slice at its next start.
    Ok(json!({"changed": a || b, "slice": "k2-postgres.slice"}))
}

/// `/etc/postgresql/<M>/main/conf.d` of a cluster, opened without links.
fn confd(ctx: &Ctx, m: PgMajor) -> HResult<Dir> {
    ctx.dir(&format!("{}/conf.d", m.etc_dir()))
        .map_err(|e| Fail::io_err("open the cluster's conf.d", e))
}

fn wait_online(ctx: &Ctx, m: PgMajor) -> HResult<ClusterRow> {
    for i in 0..START_WAIT_TRIES {
        if let Ok(r) = cluster_of(ctx, m) {
            if r.online() {
                return Ok(r);
            }
        }
        if i + 1 < START_WAIT_TRIES && ctx.real_owners {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    Err(Fail::io(format!("{} did not come online", m.unit())))
}

/// `pg ensure-cluster <M> <port>`.
pub fn ensure_cluster(ctx: &Ctx, m: PgMajor, port: u16) -> HResult<J> {
    if !major_installed(ctx, m) {
        return Err(Fail::state(format!(
            "major_not_installed: postgresql-{0} isn't installed (pg add-major {0} first)",
            m.get()
        )));
    }
    let rows = lsclusters(ctx)?;
    let mine = rows.iter().find(|r| r.major == m.get() && r.name == "main").cloned();
    let created = match mine {
        Some(r) => {
            if !k2_layout(&r) {
                return Err(Fail::state("cluster_layout_refused: the cluster's data dir is outside /var/lib/postgresql/<M>/main"));
            }
            if r.port != port {
                return Err(Fail::state(format!(
                    "port_mismatch: {}/main already runs on {}",
                    m.get(),
                    r.port
                )));
            }
            false
        }
        None => {
            if rows.iter().any(|r| r.port == port) {
                return Err(Fail::state("port_taken: another cluster is configured on that port"));
            }
            if listening(ctx)?.contains(&port) {
                return Err(Fail::state("port_taken: something already listens on that port"));
            }
            if present(ctx, &m.data_dir()) || present(ctx, &m.etc_dir()) {
                return Err(Fail::state(
                    "cluster_leftover: a data or config dir for that major exists without a cluster; nothing changed",
                ));
            }
            put_at(ctx, CREATECLUSTER_DROPIN, createcluster_dropin().as_bytes())?;
            let mt = m.token();
            let pt = port.to_string();
            ctx.run_ok(
                PG_CREATECLUSTER,
                &[&mt, "main", "-p", &pt, "--start-conf", "auto", "--", "--auth-local=peer", "--auth-host=scram-sha-256"],
                None,
            )?;
            let d = confd(ctx, m)?;
            put(ctx, &d, CLUSTER_CONF_FILE, cluster_conf().as_bytes())?;
            put(ctx, &d, k2_root_proto::paths::PG_CONFD_FILE, pg_guc_conf().as_bytes())?;
            true
        }
    };
    install_slice(ctx)?;
    let before = cluster_of(ctx, m)?;
    let started = !before.online();
    if started {
        ctx.run_ok(SYSTEMCTL, &["start", &m.unit()], None)?;
    }
    let row = wait_online(ctx, m)?;
    Ok(json!({"major": m.get(), "port": row.port, "created": created, "started": started, "unit": m.unit()}))
}

/// `pg start-conf <M> auto|manual`.
pub fn start_conf(ctx: &Ctx, m: PgMajor, auto: bool) -> HResult<J> {
    cluster_of(ctx, m)?;
    let d = ctx.dir(&m.etc_dir()).map_err(|e| Fail::io_err("open the cluster's config dir", e))?;
    let name = OsStr::new("start.conf");
    let st = d
        .lstat(name)
        .map_err(|e| Fail::io_err("stat start.conf", e))?
        .ok_or_else(|| Fail::state("start.conf is missing"))?;
    if crate::fsx::kind_of(st.st_mode as u32) != Kind::File {
        return Err(Fail::state("start.conf is not a regular file"));
    }
    let word = if auto { "auto" } else { "manual" };
    let body = format!("# K2 (pg start-conf): auto | manual | disabled\n{word}\n");
    let owner = ctx.real_owners.then_some((st.st_uid, st.st_gid));
    let before = start_conf_of(ctx, m.get());
    if before.as_deref() != Some(word) {
        d.write_atomic(name, body.as_bytes(), (st.st_mode as u32) & 0o7777, owner)
            .map_err(|e| Fail::io_err("write start.conf", e))?;
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(json!({"major": m.get(), "startConf": word, "changed": before.as_deref() != Some(word)}))
}

// ── SQL as postgres, against one cluster ────────────────────────────────

fn as_postgres(ctx: &Ctx, prog: &str, args: &[&str], stdin: Option<&[u8]>) -> HResult<ExecOut> {
    let mut a: Vec<String> = vec!["-u".into(), PG_OS_USER.into(), "--".into(), prog.into()];
    a.extend(args.iter().map(|s| s.to_string()));
    let out = ctx
        .exec
        .run(RUNUSER, &a, stdin)
        .map_err(|e| Fail::io(format!("runuser: {}", e.kind_str())))?;
    if out.status != 0 {
        let name = prog.rsplit('/').next().unwrap_or(prog);
        return Err(Fail::io(format!("{name} exit {}: {}", out.status, trimmed(&out.stderr))));
    }
    Ok(out)
}

/// `psql -X -tA -v ON_ERROR_STOP=1` of `m` against `db`, SQL on stdin.
fn sql_at(ctx: &Ctx, m: PgMajor, port: u16, db: &str, sql: &str) -> HResult<String> {
    let pt = port.to_string();
    let out = as_postgres(
        ctx,
        &m.bin("psql"),
        &["-X", "-h", PG_SOCKET_DIR, "-p", &pt, "-d", db, "-v", "ON_ERROR_STOP=1", "-tA"],
        Some(sql.as_bytes()),
    )?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `pg psql --cluster <M> …` (pg.rs routes here).
pub fn psql_args(ctx: &Ctx, m: PgMajor) -> HResult<(String, Vec<String>)> {
    let row = cluster_of(ctx, m)?;
    Ok((m.bin("psql"), vec!["-h".into(), PG_SOCKET_DIR.into(), "-p".into(), row.port.to_string()]))
}

fn db_exists(ctx: &Ctx, m: PgMajor, port: u16, db: &DbName) -> HResult<bool> {
    let n = sql_at(ctx, m, port, "postgres", &format!("SELECT count(*) FROM pg_database WHERE datname = '{}';\n", db.as_str()))?;
    Ok(n == "1")
}

/// `pg copy-roles <from> <to> [--dry-run]`. A dry run answers the same
/// comparison (`missing`, `refused`, `mismatch` by name) and changes
/// nothing; a real run refuses on any of them before any change.
pub fn copy_roles(ctx: &Ctx, from: PgMajor, to: PgMajor, dry_run: bool, stdin: &[u8]) -> HResult<J> {
    let req: CopyRolesRequest =
        serde_json::from_slice(stdin).map_err(|_| Fail::argv("copy-roles: stdin is not the request JSON"))?;
    req.validate().map_err(Fail::argv)?;
    let src = cluster_of(ctx, from)?;
    let dst = cluster_of(ctx, to)?;
    let sel = authid_select_sql(&req.roles);
    let read = |m: PgMajor, port: u16| -> HResult<Vec<AuthRow>> {
        let t = sql_at(ctx, m, port, "postgres", &sel)?;
        serde_json::from_str(&t).map_err(|_| Fail::io("copy-roles: pg_authid answer is not JSON"))
    };
    let have = read(from, src.port)?;
    let there = read(to, dst.port)?;
    let missing: Vec<&String> = req.roles.iter().filter(|r| !have.iter().any(|h| &h.rolname == *r)).collect();
    let mut sql = String::from("BEGIN;\n");
    let (mut created, mut same, mut refused, mut mismatch) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for row in &have {
        let Some(create) = row.create_sql() else {
            refused.push(row.rolname.clone());
            continue;
        };
        match there.iter().find(|t| t.rolname == row.rolname) {
            Some(t) if t.same_as(row) => same.push(row.rolname.clone()),
            // One login can't hold two passwords (V32).
            Some(_) => mismatch.push(row.rolname.clone()),
            None => {
                sql.push_str(&create);
                created.push(row.rolname.clone());
            }
        }
    }
    if dry_run {
        return Ok(json!({
            "from": from.get(), "to": to.get(), "dryRun": true,
            "wouldCreate": created, "same": same, "missing": missing, "refused": refused, "mismatch": mismatch,
        }));
    }
    if !missing.is_empty() {
        return Err(Fail::state("copy-roles: a role is missing in the source cluster; nothing changed"));
    }
    if !refused.is_empty() {
        return Err(Fail::state(
            "copy-roles: a role is superuser, replication or BYPASSRLS, or has an unsafe stored password; nothing changed",
        ));
    }
    // Refuse before any change.
    if !mismatch.is_empty() {
        return Err(Fail::state(
            "verifier_mismatch: a role already exists in the target cluster with another password or other attributes; nothing changed",
        ));
    }
    sql.push_str("COMMIT;\n");
    if !created.is_empty() {
        sql_at(ctx, to, dst.port, "postgres", &sql)?;
    }
    Ok(json!({"from": from.get(), "to": to.get(), "created": created, "same": same}))
}

fn postgres_test(ctx: &Ctx, path: &str) -> HResult<Option<u64>> {
    let mut a: Vec<String> = vec!["-u".into(), PG_OS_USER.into(), "--".into(), STAT.into()];
    a.extend(["-c".into(), "%s".into(), "--".into(), path.into()]);
    let out = ctx
        .exec
        .run(RUNUSER, &a, None)
        .map_err(|e| Fail::io(format!("runuser: {}", e.kind_str())))?;
    if out.status != 0 {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse().ok())
}

/// `pg move-dump <from> <to> <db> <tag>`.
pub fn move_dump(ctx: &Ctx, from: PgMajor, to: PgMajor, db: &DbName, tag: &str) -> HResult<J> {
    if !major_installed(ctx, to) {
        return Err(Fail::state("major_not_installed: the target major's pg_dump isn't installed"));
    }
    let src = cluster_of(ctx, from)?;
    if !src.online() {
        return Err(Fail::state("cluster_down: the source cluster is not running"));
    }
    as_postgres(ctx, MKDIR, &["-p", "-m", "0700", "--", MOVES_DIR], None)?;
    let file = dump_path(db.as_str(), tag);
    if postgres_test(ctx, &file)?.is_some() {
        return Err(Fail::state("dump_exists: a dump with that tag exists; nothing changed"));
    }
    let pt = src.port.to_string();
    as_postgres(
        ctx,
        &to.bin("pg_dump"),
        &["-Fc", "--no-subscriptions", "--no-publications", "-h", PG_SOCKET_DIR, "-p", &pt, "-f", &file, "-d", db.as_str()],
        None,
    )?;
    let bytes = postgres_test(ctx, &file)?.ok_or_else(|| Fail::io("pg_dump finished but the dump is missing"))?;
    Ok(json!({"file": file, "bytes": bytes, "from": from.get(), "pgDump": to.get()}))
}

/// `pg move-restore <to> <db> <tag>`.
pub fn move_restore(ctx: &Ctx, to: PgMajor, db: &DbName, tag: &str) -> HResult<J> {
    let dst = cluster_of(ctx, to)?;
    let file = dump_path(db.as_str(), tag);
    if postgres_test(ctx, &file)?.is_none() {
        return Err(Fail::state("dump_missing: no dump with that tag"));
    }
    if !db_exists(ctx, to, dst.port, db)? {
        return Err(Fail::state("no_database: create the target database first"));
    }
    let pt = dst.port.to_string();
    let restore = to.bin("pg_restore");
    as_postgres(
        ctx,
        ENV,
        &[
            // The box's 1 GB temp cap and 60 s statement cap would fail a
            // big index build (V40); for this session only.
            "PGOPTIONS=-c temp_file_limit=-1 -c statement_timeout=0 -c lock_timeout=0 -c idle_in_transaction_session_timeout=0",
            &restore,
            "--exit-on-error",
            "-h",
            PG_SOCKET_DIR,
            "-p",
            &pt,
            "-d",
            db.as_str(),
            &file,
        ],
        None,
    )?;
    Ok(json!({"to": to.get(), "file": file}))
}

/// `pg move-file-remove <db> <tag>`.
pub fn move_file_remove(ctx: &Ctx, db: &DbName, tag: &str) -> HResult<J> {
    let file = dump_path(db.as_str(), tag);
    let had = postgres_test(ctx, &file)?.is_some();
    if had {
        as_postgres(ctx, RM, &["-f", "--", &file], None)?;
    }
    Ok(json!({"file": file, "removed": had}))
}

/// `pg allow-conn <M> <db> on|quiesce|off`.
pub fn allow_conn(ctx: &Ctx, m: PgMajor, db: &DbName, mode: ConnMode) -> HResult<J> {
    if db.as_str() == "postgres" {
        return Err(Fail::state("refused: the postgres database"));
    }
    let row = cluster_of(ctx, m)?;
    if !db_exists(ctx, m, row.port, db)? {
        return Err(Fail::state("no_database: no such database in that cluster"));
    }
    let n = db.as_str();
    let end = format!(
        "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE datname = '{n}' AND pid <> pg_backend_pid();\n"
    );
    let sql = match mode {
        ConnMode::On => format!("ALTER DATABASE \"{n}\" ALLOW_CONNECTIONS true;\nALTER DATABASE \"{n}\" CONNECTION LIMIT -1;\nSELECT 0;\n"),
        ConnMode::Quiesce => format!("ALTER DATABASE \"{n}\" CONNECTION LIMIT 0;\n{end}"),
        ConnMode::Off => format!("ALTER DATABASE \"{n}\" ALLOW_CONNECTIONS false;\n{end}"),
    };
    let out = sql_at(ctx, m, row.port, "postgres", &sql)?;
    let ended: u64 = out.lines().last().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    Ok(json!({"major": m.get(), "mode": mode.token(), "ended": ended}))
}

/// `pg drop-db <M> <db>`.
pub fn drop_db(ctx: &Ctx, m: PgMajor, db: &DbName) -> HResult<J> {
    if db.as_str() == "postgres" {
        return Err(Fail::state("refused: the postgres database"));
    }
    let row = cluster_of(ctx, m)?;
    let had = db_exists(ctx, m, row.port, db)?;
    if had {
        // A kept source is ALLOW_CONNECTIONS false; DROP needs no connection to it.
        sql_at(ctx, m, row.port, "postgres", &format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE);\n", db.as_str()))?;
    }
    Ok(json!({"major": m.get(), "dropped": had}))
}
