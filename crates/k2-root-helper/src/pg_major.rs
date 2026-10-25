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
    LAYOUT
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

/// `pg_lsclusters -h` as postgres (it parses config files postgres owns,
/// so it never runs as root: review #1 L7). Empty when postgresql-common
/// isn't installed.
pub fn lsclusters(ctx: &Ctx) -> HResult<Vec<ClusterRow>> {
    if !present(ctx, PG_LSCLUSTERS) {
        return Ok(Vec::new());
    }
    let out = as_postgres(ctx, PG_LSCLUSTERS, &["-h"], None)?;
    Ok(parse_lsclusters(&String::from_utf8_lossy(&out.stdout)))
}

const CHMOD: &str = "/usr/bin/chmod";
const DD: &str = "/usr/bin/dd";
const DU: &str = "/usr/bin/du";
const FIND: &str = "/usr/bin/find";

/// The K2-layout cluster of `m` (`<M>/main` under /var/lib/postgresql).
pub(crate) fn cluster_of(ctx: &Ctx, m: PgMajor) -> HResult<ClusterRow> {
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

/// In-band ports Sew's config names: taken even while Sew is stopped.
fn sew_band_ports(ctx: &Ctx) -> Vec<u16> {
    read_small(ctx, SEW_TOML, 256 * 1024)
        .ok()
        .flatten()
        .map(|t| sew_ports(&String::from_utf8_lossy(&t)))
        .unwrap_or_default()
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

/// One cluster as `pg clusters` and `pg ensure-cluster` answer it (the
/// daemon's `sql::cluster::parse_helper_cluster` reads these fields).
fn cluster_json(ctx: &Ctx, r: &ClusterRow) -> J {
    json!({
        "major": r.major,
        "name": r.name,
        "port": r.port,
        "status": r.status,
        "online": r.online(),
        "running": r.online(),
        "owner": r.owner,
        "dataDir": r.data_dir,
        "k2Layout": k2_layout(r),
        "unit": format!("postgresql@{}-{}.service", r.major, r.name),
        "startConf": start_conf_of(ctx, r.major),
        "inSlice": present(ctx, &instance_slice_dropin(r.major)),
    })
}

/// `pg clusters` (open, read-only).
pub fn clusters(ctx: &Ctx) -> HResult<J> {
    let rows = lsclusters(ctx)?;
    let (sew, sew_major) = sew_status(ctx);
    let code = codename(ctx);
    let sew_ports = sew_band_ports(ctx);
    let listening_band: Vec<u16> = listening(ctx)
        .map(|v| {
            // Sew's configured ports count as taken (review #1 L6).
            let mut b: Vec<u16> = v.into_iter().chain(sew_ports.iter().copied()).filter(|p| in_band(*p)).collect();
            b.sort_unstable();
            b.dedup();
            b
        })
        .unwrap_or_default();
    // What moves keep on disk (dumps), for the check's disk line (review #2 F7).
    let moves_bytes: u64 = as_postgres(ctx, DU, &["-sb", "--", MOVES_DIR], None)
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0);
    Ok(json!({
        "clusters": rows.iter().map(|r| cluster_json(ctx, r)).collect::<Vec<_>>(),
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
        "slice": {"unit": present(ctx, SLICE_UNIT_PATH)},
        "bandListening": listening_band,
        "movesBytes": moves_bytes,
        "sew": {"present": sew, "pgMajor": sew_major, "ports": sew_ports},
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
        // PGDG's security and minor updates arrive unattended (review #1 M2).
        put_at(ctx, UNATTENDED_DROPIN, unattended_dropin().as_bytes())?;
    }
    let before = dpkg_versions(ctx)?;
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
    // `--no-remove`: whatever apt decides now (lists can change after the
    // simulation), it never removes a package (review #1 M1).
    let mut inst = vec![FE, APT_GET, "install", "-y", "--no-remove"];
    inst.extend_from_slice(&opts);
    inst.extend(pk.iter().map(String::as_str));
    ctx.run_ok(ENV, &inst, None)?;
    let changed = changed_packages(&before, &dpkg_versions(ctx)?);
    if !changed.is_empty() {
        return Err(Fail::state(format!(
            "installed_changed: the install changed installed Postgres packages: {}{}",
            changed.join(", "),
            if sew_major.is_some() { " (Sew's own Postgres runs on one of them; check sew-db)" } else { "" }
        )));
    }
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

/// Installed version of every Postgres package and libpq5.
fn dpkg_versions(ctx: &Ctx) -> HResult<std::collections::BTreeMap<String, String>> {
    if !present(ctx, DPKG_QUERY) {
        return Ok(Default::default());
    }
    let out = ctx
        .exec
        .run(
            DPKG_QUERY,
            &["-W".into(), "-f".into(), "${Package} ${Version} ${Status}\\n".into(), "postgresql*".into(), "libpq5".into()],
            None,
        )
        .map_err(|e| Fail::io(format!("dpkg-query: {}", e.kind_str())))?;
    Ok(installed_versions(&String::from_utf8_lossy(&out.stdout)))
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

/// The shared slice unit (`pg install-slice`, and every `ensure-cluster`).
/// An existing slice the owner tuned is left as it is (`drift`, review #1
/// L3); only K2-created clusters join it, through their own instance
/// drop-in ([`slice_in`]).
pub fn install_slice(ctx: &Ctx) -> HResult<J> {
    let have = read_small(ctx, SLICE_UNIT_PATH, 64 * 1024)?;
    let (changed, drift) = match have {
        None => (put_at(ctx, SLICE_UNIT_PATH, slice_unit().as_bytes())?, false),
        Some(b) => (false, b != slice_unit().as_bytes()),
    };
    if changed {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(json!({"changed": changed, "drift": drift, "slice": "k2-postgres.slice"}))
}

/// Put one K2-created cluster's instance in the slice (at its next start).
fn slice_in(ctx: &Ctx, m: PgMajor) -> HResult<bool> {
    let changed = put_at(ctx, &instance_slice_dropin(m.get()), slice_dropin().as_bytes())?;
    if changed {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(changed)
}

/// `/etc/postgresql/<M>/main/conf.d` of a cluster, opened without links.
pub(crate) fn confd(ctx: &Ctx, m: PgMajor) -> HResult<Dir> {
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
pub fn ensure_cluster(ctx: &Ctx, m: PgMajor, port: Option<u16>) -> HResult<J> {
    let rows = lsclusters(ctx)?;
    let mine = rows.iter().find(|r| r.major == m.get() && r.name == "main").cloned();
    let created = match mine {
        Some(r) => {
            if !k2_layout(&r) {
                return Err(Fail::state("cluster_layout_refused: the cluster's data dir is outside /var/lib/postgresql/<M>/main"));
            }
            if port.is_some_and(|p| p != r.port) {
                return Err(Fail::state(format!(
                    "port_mismatch: {}/main already runs on {}",
                    m.get(),
                    r.port
                )));
            }
            false
        }
        None => {
            if present(ctx, &m.data_dir()) || present(ctx, &m.etc_dir()) {
                return Err(Fail::state(
                    "cluster_leftover: a data or config dir for that major exists without a cluster; nothing changed",
                ));
            }
            no_links_above(ctx, m)?;
            let mut busy = listening(ctx)?;
            busy.extend(sew_band_ports(ctx));
            let port = match port {
                Some(p) => {
                    if rows.iter().any(|r| r.port == p) {
                        return Err(Fail::state("port_taken: another cluster is configured on that port"));
                    }
                    if busy.contains(&p) {
                        return Err(Fail::state("port_taken: something already listens on that port, or Sew's config names it"));
                    }
                    p
                }
                None => {
                    let mut taken: Vec<u16> = rows.iter().map(|r| r.port).collect();
                    taken.extend(busy);
                    next_port(&taken).ok_or_else(|| Fail::state("band_full: no free port in K2's band 5432-5449"))?
                }
            };
            // §4 step 1: the packages first when they're missing.
            if !major_installed(ctx, m) {
                add_major(ctx, m, false)?;
            }
            if present(ctx, &m.data_dir()) || present(ctx, &m.etc_dir()) {
                return Err(Fail::state(
                    "cluster_leftover: a data or config dir for that major exists without a cluster; nothing changed",
                ));
            }
            // Review #1 L5: postgres owns /var/lib/postgresql and
            // /etc/postgresql, so a process running as postgres could plant
            // a link where root's pg_createcluster is about to write. Check
            // again right before it (no link on the way), and after it (the
            // new dirs are real directories, not links). A window of a few
            // instructions between the check and pg_createcluster remains;
            // closing it would mean creating the dirs ourselves no-follow.
            no_links_above(ctx, m)?;
            put_at(ctx, CREATECLUSTER_DROPIN, createcluster_dropin().as_bytes())?;
            let mt = m.token();
            let pt = port.to_string();
            ctx.run_ok(
                PG_CREATECLUSTER,
                &[&mt, "main", "-p", &pt, "--start-conf", "auto", "--", "--auth-local=peer", "--auth-host=scram-sha-256"],
                None,
            )?;
            for d in [m.data_dir(), m.etc_dir()] {
                if !real_dir(ctx, &d) {
                    return Err(Fail::state(format!(
                        "cluster_layout_refused: {d} is not a real directory after pg_createcluster; the cluster is left stopped for the owner"
                    )));
                }
            }
            no_links_above(ctx, m)?;
            let d = confd(ctx, m)?;
            put(ctx, &d, CLUSTER_CONF_FILE, cluster_conf().as_bytes())?;
            put(ctx, &d, k2_root_proto::paths::PG_CONFD_FILE, pg_guc_conf().as_bytes())?;
            true
        }
    };
    install_slice(ctx)?;
    if created {
        slice_in(ctx, m)?;
    }
    let before = cluster_of(ctx, m)?;
    let started = !before.online();
    if started {
        ctx.run_ok(SYSTEMCTL, &["start", &m.unit()], None)?;
    }
    let row = wait_online(ctx, m)?;
    let mut v = cluster_json(ctx, &row);
    v["created"] = json!(created);
    v["started"] = json!(started);
    Ok(v)
}

/// A directory, not a link (`symlink_metadata`: never follows).
fn real_dir(ctx: &Ctx, abs: &str) -> bool {
    ctx.p(abs).symlink_metadata().is_ok_and(|m| m.file_type().is_dir())
}

/// Review #1 L5: no link on the way to `m`'s data and config dirs (each
/// component either missing or a real directory).
pub(crate) fn no_links_above(ctx: &Ctx, m: PgMajor) -> HResult<()> {
    for parent in ["/var/lib/postgresql".to_string(), format!("/var/lib/postgresql/{}", m.get()), "/etc/postgresql".to_string(), format!("/etc/postgresql/{}", m.get())] {
        if present(ctx, &parent) && !real_dir(ctx, &parent) {
            return Err(Fail::state(format!("cluster_layout_refused: {parent} is a link or a file; nothing changed")));
        }
    }
    Ok(())
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
    // The owner's "never start" stays (review #1 L3).
    if before.as_deref() == Some("disabled") {
        return Err(Fail::state("start_conf_disabled: the owner set this cluster disabled; K2 leaves it"));
    }
    if before.as_deref() != Some(word) {
        d.write_atomic(name, body.as_bytes(), (st.st_mode as u32) & 0o7777, owner)
            .map_err(|e| Fail::io_err("write start.conf", e))?;
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(json!({"major": m.get(), "startConf": word, "changed": before.as_deref() != Some(word)}))
}

// ── SQL as postgres, against one cluster ────────────────────────────────

pub(crate) fn as_postgres(ctx: &Ctx, prog: &str, args: &[&str], stdin: Option<&[u8]>) -> HResult<ExecOut> {
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

/// `psql -X -tA -v ON_ERROR_STOP=1` of `m` against `db`, SQL on stdin, with
/// `search_path` pinned to pg_catalog (review #2 F1): every superuser
/// session the helper opens goes through here or `pg::psql`.
pub(crate) fn sql_at(ctx: &Ctx, m: PgMajor, port: u16, db: &str, sql: &str) -> HResult<String> {
    let pt = port.to_string();
    let psql = m.bin("psql");
    let out = as_postgres(
        ctx,
        ENV,
        &[SAFE_PGOPTIONS, &psql, "-X", "-h", PG_SOCKET_DIR, "-p", &pt, "-d", db, "-v", "ON_ERROR_STOP=1", "-tA"],
        Some(sql.as_bytes()),
    )?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Refuse a `pg_dump`/`pg_restore` older than the CVE floor.
fn tool_floor(ctx: &Ctx, m: PgMajor, tool: &str) -> HResult<()> {
    let out = as_postgres(ctx, &m.bin(tool), &["--version"], None)?;
    let v = parse_tool_version(&String::from_utf8_lossy(&out.stdout));
    match v {
        Some((maj, min)) if maj == m.get() && min >= min_tool_minor(maj) => Ok(()),
        Some((maj, min)) => Err(Fail::state(format!(
            "tool_too_old: {tool} {maj}.{min} is older than {maj}.{}; update postgresql-client-{maj} first",
            min_tool_minor(maj)
        ))),
        None => Err(Fail::state(format!("tool_unknown: {tool} answered no version"))),
    }
}

/// The owner of a database (`None`: no such database).
fn db_owner(ctx: &Ctx, m: PgMajor, port: u16, db: &DbName) -> HResult<Option<String>> {
    let o = sql_at(
        ctx,
        m,
        port,
        "postgres",
        &format!("SELECT pg_catalog.pg_get_userbyid(datdba) FROM pg_catalog.pg_database WHERE datname = '{}';\n", db.as_str()),
    )?;
    Ok((!o.is_empty()).then_some(o))
}

/// Only databases K2 made (review #1 L1).
pub(crate) fn k2_db(ctx: &Ctx, m: PgMajor, port: u16, db: &DbName) -> HResult<String> {
    let owner = db_owner(ctx, m, port, db)?.ok_or_else(|| Fail::state("no_database: no such database in that cluster"))?;
    if !k2_stamp_ok(db.as_str(), &owner) {
        return Err(Fail::state("not_k2_database: that database isn't one K2 made (owner <name>_migrator); nothing changed"));
    }
    Ok(owner)
}

/// `pg psql --cluster <M> …` (pg.rs routes here).
pub fn psql_args(ctx: &Ctx, m: PgMajor) -> HResult<(String, Vec<String>)> {
    let row = cluster_of(ctx, m)?;
    Ok((m.bin("psql"), vec!["-h".into(), PG_SOCKET_DIR.into(), "-p".into(), row.port.to_string()]))
}

pub(crate) fn db_exists(ctx: &Ctx, m: PgMajor, port: u16, db: &DbName) -> HResult<bool> {
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
    // The verifiers never reach the server log or pg_stat_statements
    // (review #1 L2).
    let mut sql = String::from(
        "SET log_min_error_statement = panic;\n\
         SET log_statement = 'none';\n\
         SET log_min_duration_statement = -1;\n\
         DO $k2$BEGIN IF pg_catalog.current_setting('pg_stat_statements.track_utility', true) IS NOT NULL THEN \
         PERFORM pg_catalog.set_config('pg_stat_statements.track_utility', 'off', false); END IF; END$k2$;\n\
         BEGIN;\n",
    );
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
        // A fixed error: psql's context lines can carry part of a verifier.
        sql_at(ctx, to, dst.port, "postgres", &sql).map_err(|_| {
            Fail::io("copy-roles: creating the roles failed; the batch is one transaction, so none were created")
        })?;
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

/// `pg move-dump <from> <to> <db> <tag> [--schema-only]` (`--schema-only`:
/// protocol 10, a live dual-run's target gets the DDL only, PA25).
pub fn move_dump(ctx: &Ctx, from: PgMajor, to: PgMajor, db: &DbName, tag: &str, schema_only: bool) -> HResult<J> {
    if !major_installed(ctx, to) {
        return Err(Fail::state("major_not_installed: the target major's pg_dump isn't installed"));
    }
    let src = cluster_of(ctx, from)?;
    if !src.online() {
        return Err(Fail::state("cluster_down: the source cluster is not running"));
    }
    tool_floor(ctx, to, "pg_dump")?;
    // Re-check #1 R2: the CVE-2024-7348 guard needs the source server at
    // 16.4 / 17.0 or later too, not only the new pg_dump.
    let num = sql_at(ctx, from, src.port, "postgres", "SHOW server_version_num;\n")?;
    let num: u32 = num.trim().parse().map_err(|_| Fail::io("the source server answered no server_version_num"))?;
    if num < min_server_num(from.get()) || num / 10_000 != from.get() {
        return Err(Fail::state(format!(
            "server_too_old: the source server is {num}; a move needs {} or later (update postgresql-{} and restart it first); nothing changed",
            min_server_num(from.get()),
            from.get()
        )));
    }
    moves_dir(ctx)?;
    let file = dump_path(db.as_str(), tag);
    if postgres_test(ctx, &file)?.is_some() {
        return Err(Fail::state("dump_exists: a dump with that tag exists; nothing changed"));
    }
    let pt = src.port.to_string();
    let pg_dump = to.bin("pg_dump");
    let mut args: Vec<&str> =
        vec![SAFE_PGOPTIONS, &pg_dump, "-Fc", "--no-subscriptions", "--no-publications", "-h", PG_SOCKET_DIR, "-p", &pt, "-f", &file, "-d", db.as_str()];
    if schema_only {
        args.insert(3, "--schema-only");
    }
    as_postgres(ctx, ENV, &args, None)?;
    // Full tenant data: postgres only (review #1 L4).
    as_postgres(ctx, CHMOD, &["0600", "--", &file], None)?;
    let bytes = postgres_test(ctx, &file)?.ok_or_else(|| Fail::io("pg_dump finished but the dump is missing"))?;
    Ok(json!({"file": file, "bytes": bytes, "from": from.get(), "pgDump": to.get(), "schemaOnly": schema_only}))
}

/// `MOVES_DIR`, postgres-owned 0700 whatever made it first (review #1 L4).
fn moves_dir(ctx: &Ctx) -> HResult<()> {
    as_postgres(ctx, MKDIR, &["-p", "-m", "0700", "--", MOVES_DIR], None)?;
    let st = as_postgres(ctx, STAT, &["-c", "%F %U", "--", MOVES_DIR], None)?;
    if String::from_utf8_lossy(&st.stdout).trim() != "directory postgres" {
        return Err(Fail::state("moves_dir_refused: /var/lib/postgresql/k2-moves is not a postgres-owned directory"));
    }
    as_postgres(ctx, CHMOD, &["0700", "--", MOVES_DIR], None)?;
    // Re-check #1 R1: an owner passfile a killed helper left behind (one
    // move at a time, so any passfile here is stale).
    as_postgres(ctx, FIND, &[MOVES_DIR, "-maxdepth", "1", "-type", "f", "-name", ".k2pass-*", "-delete"], None)?;
    as_postgres(ctx, FIND, &[MOVES_DIR, "-maxdepth", "1", "-type", "f", "-name", ".k2toc-*", "-delete"], None)?;
    Ok(())
}

/// A pg_restore failure without row values (review #2 info / #1 I4): only
/// the first `ERROR:` line, never DETAIL, CONTEXT or the command.
fn restore_failed(section: &str, out: &ExecOut) -> Fail {
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().find(|l| l.contains("ERROR:")).unwrap_or("pg_restore failed");
    let line = crate::pg::redact_passwords(line);
    Fail::io(format!("pg_restore ({section}) exit {}: {}", out.status, line.trim().chars().take(300).collect::<String>()))
}

fn run_restore(ctx: &Ctx, section: &str, args: &[&str]) -> HResult<()> {
    let mut a: Vec<String> = vec!["-u".into(), PG_OS_USER.into(), "--".into()];
    a.extend(args.iter().map(|s| s.to_string()));
    let out = ctx
        .exec
        .run(RUNUSER, &a, None)
        .map_err(|e| Fail::io(format!("runuser: {}", e.kind_str())))?;
    if out.status != 0 {
        return Err(restore_failed(section, &out));
    }
    Ok(())
}

/// The dump's table of contents split for the restore (0.46.4 drill H1).
pub(crate) struct TocSplit {
    /// Schemas, first, as the owner.
    pub schemas: String,
    /// Extensions and their comments, as postgres.
    pub superuser: String,
    /// Everything else, as the owner.
    pub owner: String,
    /// K2's own schema `k2` and helpers ([`k2_root_proto::k2_db::is_k2_entry`]):
    /// never replayed from the dump; installed canned as postgres.
    pub k2_schema: bool,
    pub k2_functions: Vec<String>,
}

/// The `pg_restore -l` lists for the restore. Only
/// extensions and their comments go to the superuser (they need it, and run
/// no tenant code); the schemas go first, as the owner (an extension may
/// live in one of them; empty then); every other entry is restored as the
/// owner after the extensions. A line goes to the
/// superuser list only when it is exactly an extension entry as
/// `pg_restore -l` prints it (dump id; pg_extension's catalog oid; a plain
/// extension name), so no tenant-chosen object name can put an entry there.
pub(crate) fn split_toc(toc: &str, migrator: &str) -> TocSplit {
    fn ext_name_ok(n: &str) -> bool {
        !n.is_empty() && n.len() <= 63 && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    }
    // The extensions the dump creates (their comments must name one).
    let extensions: Vec<String> = toc
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 6 && f[1] == "3079" && f[3] == "EXTENSION" && f[4] == "-" && ext_name_ok(f[5])).then(|| f[5].to_string())
        })
        .collect();
    let superuser_line = |l: &str| -> bool {
        let f: Vec<&str> = l.split_whitespace().collect();
        let id_ok = f.first().and_then(|x| x.strip_suffix(';')).is_some_and(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
        if !id_ok || f.len() < 6 || f[1].bytes().any(|b| !b.is_ascii_digit()) || f[2].bytes().any(|b| !b.is_ascii_digit()) {
            return false;
        }
        // `<id>; 3079 <oid> EXTENSION - <name> [<owner>]`
        let ext = f[1] == "3079" && f[3] == "EXTENSION" && f[4] == "-" && ext_name_ok(f[5]) && f.len() <= 7;
        // `<id>; 0 0 COMMENT - EXTENSION <name> [<owner>]`; pg_restore
        // quotes a name that needs it (`"uuid-ossp"`, drill B). The name
        // must be one of the dump's own extensions.
        let named = |n: &str| {
            let bare = n.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(n);
            (bare == n || ext_name_ok(bare)) && extensions.iter().any(|e| e == bare)
        };
        let comment = f.len() >= 7 && f.len() <= 8 && f[1] == "0" && f[2] == "0" && f[3] == "COMMENT" && f[4] == "-" && f[5] == "EXTENSION" && named(f[6]);
        ext || comment
    };
    // `<id>; 2615 <oid> SCHEMA - <name> [<owner>]`: a bare CREATE SCHEMA.
    fn schema_line(l: &str) -> bool {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() == 6 || f.len() == 7) && f[0].strip_suffix(';').is_some_and(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit())) && f[1] == "2615" && f[3] == "SCHEMA" && f[4] == "-"
    }
    let (mut schemas, mut su, mut own) = (String::new(), String::new(), String::new());
    let (mut k2_schema, mut k2_functions) = (false, Vec::new());
    for l in toc.lines() {
        if l.trim_start().starts_with(';') || l.trim().is_empty() {
            continue;
        }
        if let Some(what) = k2_root_proto::k2_db::is_k2_entry(l, migrator) {
            match what.as_str() {
                "schema" => k2_schema = true,
                w if !w.contains(' ') => k2_functions.push(w.to_string()),
                _ => {} // its ACL or comment: the canned install sets K2's.
            }
            continue;
        }
        let out = if superuser_line(l) {
            &mut su
        } else if schema_line(l) {
            &mut schemas
        } else {
            &mut own
        };
        out.push_str(l);
        out.push('\n');
    }
    TocSplit { schemas, superuser: su, owner: own, k2_schema, k2_functions }
}

/// `pg move-restore <to> <db> <tag>`, stdin [`MoveLogin`] (the database
/// owner). No tenant code ever runs as the superuser (review #1 H1, #2 F2,
/// and the 0.46.4 drill: restoring a generated column plans its expression,
/// which runs an IMMUTABLE owner function with constant arguments as the
/// restoring role, so pre-data as postgres made the owner a superuser):
/// - pre-data: extensions alone as postgres ([`split_toc`]); every other
///   entry (schemas, tables, types, functions, ACLs) as the owner, over
///   loopback with its own password;
/// - FORCE ROW LEVEL SECURITY lifted from the tables that have it;
/// - data and post-data as the owner, over loopback with its own password
///   (CHECK and domain constraints, generated columns, triggers, index
///   expressions and REFRESH run as the owner, a non-superuser);
/// - FORCE put back — except with `--keep-force-off` (protocol 10, a live
///   dual-run's target, L10): on success FORCE stays lifted (the apply
///   worker writes as the owner) and the answer's `forcedOids` lists the
///   tables for `pg force-rls` at the switch. A failed restore still puts
///   it back.
pub fn move_restore(ctx: &Ctx, to: PgMajor, db: &DbName, tag: &str, keep_force_off: bool, stdin: &[u8]) -> HResult<J> {
    let login: MoveLogin =
        serde_json::from_slice(stdin).map_err(|_| Fail::argv("move-restore: stdin is not the owner's login JSON"))?;
    login.validate().map_err(Fail::argv)?;
    tool_floor(ctx, to, "pg_restore")?;
    let dst = cluster_of(ctx, to)?;
    let file = dump_path(db.as_str(), tag);
    if postgres_test(ctx, &file)?.is_none() {
        return Err(Fail::state("dump_missing: no dump with that tag"));
    }
    let owner = k2_db(ctx, to, dst.port, db)?;
    if owner != login.role {
        return Err(Fail::state("owner_mismatch: the login is not the target database's owner"));
    }
    // Backstop (re-check #1 R6): the data never loads as a superuser.
    let su = sql_at(
        ctx,
        to,
        dst.port,
        "postgres",
        &format!("SELECT rolsuper::text FROM pg_catalog.pg_roles WHERE rolname = '{}';\n", login.role),
    )?;
    if su.trim() != "false" {
        return Err(Fail::state("owner_superuser: the database's owner is a superuser (or missing); K2 never loads data as one; nothing changed"));
    }
    let rels = sql_at(
        ctx,
        to,
        dst.port,
        db.as_str(),
        "SELECT count(*) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg\\_%';\n",
    )?;
    if rels.trim() != "0" {
        return Err(Fail::state("target_not_empty: the target database already has relations; nothing changed"));
    }
    let pt = dst.port.to_string();
    let restore = to.bin("pg_restore");
    let (d, r) = (db.as_str(), login.role.as_str());

    // 1. The table of contents, split: extensions for the superuser, the
    // rest for the owner (lists in the postgres-only moves dir).
    moves_dir(ctx)?;
    let toc = as_postgres(ctx, &restore, &["-l", &file], None)?;
    let split = split_toc(&String::from_utf8_lossy(&toc.stdout), r);
    let (schema_list, su_list, own_list) = (&split.schemas, &split.superuser, &split.owner);
    let k2_install = k2_root_proto::k2_db::install_sql(split.k2_schema, &split.k2_functions, r)
        .ok_or_else(|| Fail::state("restore refused: the owner is not a role name"))?;
    let schema_file = format!("{MOVES_DIR}/.k2toc-schemas-{tag}");
    let su_file = format!("{MOVES_DIR}/.k2toc-su-{tag}");
    let own_file = format!("{MOVES_DIR}/.k2toc-own-{tag}");
    let force_sql = |forced: &[u32], on: bool| {
        let list = forced.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        format!(
            "DO $k2$DECLARE t oid; BEGIN FOREACH t IN ARRAY ARRAY[{list}]::oid[] LOOP \
             EXECUTE pg_catalog.format('ALTER TABLE %s {} FORCE ROW LEVEL SECURITY', t::regclass); END LOOP; END$k2$;\n",
            if on { "" } else { "NO" }
        )
    };
    let mut forced: Vec<u32> = Vec::new();
    // FORCE, the owner's temp-file setting and the passfile come back in
    // one cleanup path, whatever fails (re-check #1 R1). A helper killed
    // mid-load leaves the passfile to the next `moves_dir` sweep; the
    // target copy (with its FORCE and temp setting) is dropped by the
    // daemon's undo.
    let quoted = format!("\"{r}\" IN DATABASE \"{d}\"");
    let pass = format!("{MOVES_DIR}/.k2pass-{tag}");
    let (mut force_lifted, mut temp_set) = (false, false);
    let owner_steps = (|| -> HResult<()> {
        as_postgres(ctx, DD, &[&format!("of={schema_file}"), "status=none"], Some(schema_list.as_bytes()))?;
        as_postgres(ctx, DD, &[&format!("of={su_file}"), "status=none"], Some(su_list.as_bytes()))?;
        as_postgres(ctx, DD, &[&format!("of={own_file}"), "status=none"], Some(own_list.as_bytes()))?;
        // The owner's sessions may need big temp files (index builds,
        // V40): for this database only, reset after.
        temp_set = true;
        sql_at(ctx, to, dst.port, "postgres", &format!("ALTER ROLE {quoted} SET temp_file_limit = -1;\n"))?;
        // Its password in a 0600 file for every owner step.
        as_postgres(ctx, DD, &[&format!("of={pass}"), "status=none"], Some(login.passfile_line(dst.port, d).as_bytes()))?;
        as_postgres(ctx, CHMOD, &["0600", "--", &pass], None)?;
        let env_pass = format!("PGPASSFILE={pass}");
        let owner_restore = |section: &str, list: Option<&str>| -> HResult<()> {
            let sec = format!("--section={section}");
            let mut args: Vec<&str> = vec![
                ENV,
                &env_pass,
                "PGOPTIONS=-c statement_timeout=0 -c lock_timeout=0 -c idle_in_transaction_session_timeout=0",
                &restore,
                &sec,
                "--exit-on-error",
            ];
            if let Some(l) = list {
                args.extend(["-L", l]);
            }
            args.extend(["-h", "127.0.0.1", "-p", &pt, "-U", r, "-d", d, &file]);
            run_restore(ctx, section, &args)
        };
        // 2. The schemas as the owner (empty: an extension may go in one).
        if !schema_list.is_empty() {
            owner_restore("pre-data", Some(&schema_file))?;
        }
        // 3. Extensions (and their comments) as postgres: no tenant code.
        if !su_list.is_empty() {
            run_restore(
                ctx,
                "pre-data (extensions)",
                &[
                    ENV,
                    "PGOPTIONS=-c search_path=pg_catalog,pg_temp -c role=none -c statement_timeout=0 -c lock_timeout=0",
                    &restore,
                    "--section=pre-data",
                    "--exit-on-error",
                    "-L",
                    &su_file,
                    "-h",
                    PG_SOCKET_DIR,
                    "-p",
                    &pt,
                    "-d",
                    d,
                    &file,
                ],
            )?;
        }
        // 3b. K2's own schema `k2` and helpers: canned, as postgres, while
        // the target holds no tenant object yet (drill A: they are owned by
        // postgres, which the owner can't replay; and no tenant DDL ever
        // runs as postgres).
        if split.k2_schema || !split.k2_functions.is_empty() {
            sql_at(ctx, to, dst.port, d, &k2_install)?;
        }
        // 4. The rest of pre-data as the owner (never the superuser: a
        // generated column's expression is planned at CREATE TABLE).
        owner_restore("pre-data", Some(&own_file))?;
        // 5. FORCE off while the owner loads (it would hide its own rows).
        let f = sql_at(
            ctx,
            to,
            dst.port,
            d,
            "SELECT coalesce(pg_catalog.string_agg(c.oid::text, ','), '') FROM pg_catalog.pg_class c WHERE c.relforcerowsecurity;\n",
        )?;
        forced = f.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if !forced.is_empty() {
            force_lifted = true;
            sql_at(ctx, to, dst.port, d, &force_sql(&forced, false))?;
        }
        // 6. data and post-data as the owner.
        for section in ["data", "post-data"] {
            owner_restore(section, None)?;
        }
        Ok(())
    })();
    // 7. Cleanup, on success and failure alike.
    let mut undone: Vec<String> = Vec::new();
    for f in [&pass, &schema_file, &su_file, &own_file] {
        if let Err(e) = as_postgres(ctx, RM, &["-f", "--", f], None) {
            undone.push(format!("remove {f}: {}", e.msg));
        }
    }
    if temp_set {
        if let Err(e) = sql_at(ctx, to, dst.port, "postgres", &format!("ALTER ROLE {quoted} RESET temp_file_limit;\n")) {
            undone.push(format!("reset temp_file_limit: {}", e.msg));
        }
    }
    if force_lifted && !(keep_force_off && owner_steps.is_ok()) {
        if let Err(e) = sql_at(ctx, to, dst.port, d, &force_sql(&forced, true)) {
            undone.push(format!("put FORCE back: {}", e.msg));
        }
    }
    match (owner_steps, undone.is_empty()) {
        (Ok(()), true) => Ok(json!({
            "to": to.get(), "file": file, "loadedAs": r, "forcedTables": forced.len(),
            "forceKeptOff": keep_force_off && !forced.is_empty(), "forcedOids": forced,
        })),
        (Ok(()), false) => Err(Fail::io(format!("restore cleanup: {}", undone.join("; ")))),
        (Err(e), true) => Err(e),
        (Err(mut e), false) => {
            e.msg = format!("{}; cleanup: {}", e.msg, undone.join("; "));
            Err(e)
        }
    }
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
    k2_db(ctx, m, row.port, db)?;
    let n = db.as_str();
    let end = format!(
        "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE datname = '{n}' AND pid <> pg_backend_pid();\n"
    );
    let sql = match mode {
        ConnMode::On => format!("ALTER DATABASE \"{n}\" ALLOW_CONNECTIONS true;\nALTER DATABASE \"{n}\" CONNECTION LIMIT -1;\nSELECT 0;\n"),
        ConnMode::Quiesce => format!("ALTER DATABASE \"{n}\" CONNECTION LIMIT 0;\n{end}"),
        ConnMode::Off => format!("ALTER DATABASE \"{n}\" ALLOW_CONNECTIONS false;\n{end}"),
        // Protocol 10: the cut-over's write block keeps the walsender.
        ConnMode::QuiesceRepl => k2_root_proto::pg_dual::quiesce_repl_sql(n),
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
        k2_db(ctx, m, row.port, db)?;
        // Only a closed (kept source) or quiesced (aborted move) copy: a
        // live database is never dropped here (review #1 L1).
        let state = sql_at(
            ctx,
            m,
            row.port,
            "postgres",
            &format!(
                "SELECT (NOT datallowconn OR datconnlimit = 0)::text FROM pg_catalog.pg_database WHERE datname = '{}';\n",
                db.as_str()
            ),
        )?;
        if state.trim() != "true" {
            return Err(Fail::state("db_open: the database accepts connections; quiesce or close it first; nothing dropped"));
        }
        sql_at(ctx, m, row.port, "postgres", &format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE);\n", db.as_str()))?;
    }
    Ok(json!({"major": m.get(), "dropped": had}))
}
