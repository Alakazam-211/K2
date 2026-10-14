//! `pg` namespace (§7.4): the legacy `k2-pg-helper` script ported to Rust
//! with box-secrets B1's rules and the RH9/RH10 fixes.
//!
//! - `systemctl`: fixed verbs, the unit by regex, `--now` only with
//!   enable/disable.
//! - `install-ram-fence`: canned drop-ins for `postgresql.service` and
//!   `postgresql@.service`, and canned `99-k2-ram.conf` into each
//!   `/etc/postgresql/<n>/main/conf.d` — found WITHOUT following links,
//!   with a pre-scan that refuses (nothing written) when any conf.d or an
//!   existing `99-k2-ram.conf` is a symlink (RH9: that directory is
//!   normally postgres-owned).
//! - `psql`: daemon-only (class table). `runuser -u postgres --
//!   /usr/bin/psql -X …` with an argv array — no `su -c`, no shell.

use std::ffi::{OsStr, OsString};

use k2_root_proto::apps::{hba_edit, AppId, HBA_MAX_BYTES};
use k2_root_proto::paths::*;
use k2_root_proto::templates::{pg_guc_conf, pg_ram_dropin};
use k2_root_proto::verbs::{DbName, PgOp, PgUnit, PsqlFlags};

use crate::ctx::{Ctx, Fail, HResult, KindStr};
use crate::fsx::{Dir, Kind};
use crate::mail::{parent_dir, split};

pub fn systemctl(ctx: &Ctx, op: PgOp, now: bool, unit: &PgUnit) -> HResult<()> {
    let verb = match op {
        PgOp::Start => "start",
        PgOp::Restart => "restart",
        PgOp::Enable => "enable",
        PgOp::Stop => "stop",
        PgOp::Disable => "disable",
        PgOp::IsActive => "is-active",
    };
    let mut args = vec![verb];
    if now {
        args.push("--now");
    }
    args.push(unit.as_str());
    if op == PgOp::IsActive {
        // An answer, not an error: print systemd's word, exit 0.
        let out = ctx
            .exec
            .run(
                SYSTEMCTL,
                &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                None,
            )
            .map_err(|e| Fail::io(format!("systemctl: {}", e.kind_str())))?;
        ctx.print(String::from_utf8_lossy(&out.stdout).trim().as_bytes());
        ctx.print(b"\n");
        return Ok(());
    }
    ctx.run_ok(SYSTEMCTL, &args, None).map(|_| ())
}

pub fn daemon_reload(ctx: &Ctx) -> HResult<()> {
    ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None).map(|_| ())
}

fn version_dir_ok(name: &OsStr) -> bool {
    let b = name.as_encoded_bytes();
    !b.is_empty()
        && b.len() <= 8
        && b.iter().all(|c| c.is_ascii_digit() || *c == b'.')
        && b[0] != b'.'
}

/// One `conf.d` to write: (`<n>/main/conf.d` handle).
fn scan_confds(ctx: &Ctx) -> HResult<Vec<(OsString, Dir)>> {
    let (parent, name) = split(PG_ETC);
    let etc = ctx.dir(parent).map_err(|e| Fail::io_err("open /etc", e))?;
    let pg_name = OsStr::new(name);
    match etc.kind(pg_name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => return Ok(Vec::new()),
        Kind::Dir => {}
        Kind::Symlink => {
            return Err(Fail::state(
                "install-ram-fence refused: /etc/postgresql is a symlink",
            ))
        }
        _ => {
            return Err(Fail::state(
                "install-ram-fence refused: /etc/postgresql is not a directory",
            ))
        }
    }
    let pg = etc
        .sub(pg_name)
        .map_err(|e| Fail::io_err("open /etc/postgresql", e))?;
    let mut out = Vec::new();
    for ver in pg
        .entries()
        .map_err(|e| Fail::io_err("read /etc/postgresql", e))?
    {
        if !version_dir_ok(&ver) {
            continue;
        }
        // The shell glob skipped anything that wasn't a directory chain;
        // here a link anywhere in the chain is a refusal, never a skip.
        let refuse =
            || Fail::state("install-ram-fence refused: a cluster config path is a symlink");
        match pg.kind(&ver).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Dir => {}
            Kind::Symlink => return Err(refuse()),
            _ => continue,
        }
        let vd = pg.sub(&ver).map_err(|e| Fail::io_err("open cluster", e))?;
        match vd
            .kind(OsStr::new("main"))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Dir => {}
            Kind::Symlink => return Err(refuse()),
            _ => continue,
        }
        let main = vd
            .sub(OsStr::new("main"))
            .map_err(|e| Fail::io_err("open main", e))?;
        match main
            .kind(OsStr::new("conf.d"))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Dir => {}
            Kind::Symlink => return Err(refuse()),
            _ => continue,
        }
        let confd = main
            .sub(OsStr::new("conf.d"))
            .map_err(|e| Fail::io_err("open conf.d", e))?;
        match confd
            .kind(OsStr::new(PG_CONFD_FILE))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Missing | Kind::File => {}
            _ => {
                return Err(Fail::state(
                    "install-ram-fence refused: 99-k2-ram.conf is not a regular file",
                ))
            }
        }
        out.push((ver, confd));
    }
    Ok(out)
}

fn ensure_dropin_dir(ctx: &Ctx, abs: &'static str) -> HResult<Dir> {
    let (d, name) = parent_dir(ctx, abs)?;
    match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => d.mkdir(name, 0o755).map_err(|e| Fail::io_err("mkdir", e))?,
        Kind::Dir => {}
        _ => {
            return Err(Fail::state(
                "install-ram-fence refused: drop-in dir is not a directory",
            ))
        }
    }
    let sub = d
        .sub(name)
        .map_err(|e| Fail::io_err("open drop-in dir", e))?;
    if let Some((u, g)) = ctx.root_owner() {
        sub.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
    }
    sub.fchmod(0o755).map_err(|e| Fail::io_err("chmod", e))?;
    Ok(sub)
}

/// Canned D29 fence. Pre-scan first: any symlink → refuse with nothing
/// written. Then the two systemd drop-ins (0644 root) and each conf.d file
/// (0644; owner root, readable by postgres).
pub fn install_ram_fence(ctx: &Ctx) -> HResult<serde_json::Value> {
    let confds = scan_confds(ctx)?;
    // Refuse a symlinked drop-in dir before writing anything.
    for abs in [PG_RAM_DROPIN_DIR, PG_RAM_TEMPLATE_DROPIN_DIR] {
        let (d, name) = parent_dir(ctx, abs)?;
        if matches!(
            d.kind(name).map_err(|e| Fail::io_err("stat", e))?,
            Kind::Symlink | Kind::File | Kind::Other
        ) {
            return Err(Fail::state(
                "install-ram-fence refused: drop-in dir is not a directory",
            ));
        }
    }
    for abs in [PG_RAM_DROPIN_DIR, PG_RAM_TEMPLATE_DROPIN_DIR] {
        let dir = ensure_dropin_dir(ctx, abs)?;
        dir.write_atomic(
            OsStr::new("k2-ram.conf"),
            pg_ram_dropin().as_bytes(),
            0o644,
            ctx.root_owner(),
        )
        .map_err(|e| Fail::io_err("write drop-in", e))?;
    }
    let mut clusters = Vec::new();
    for (ver, confd) in &confds {
        confd
            .write_atomic(
                OsStr::new(PG_CONFD_FILE),
                pg_guc_conf().as_bytes(),
                0o644,
                ctx.root_owner(),
            )
            .map_err(|e| Fail::io_err("write conf.d", e))?;
        clusters.push(ver.to_string_lossy().to_string());
    }
    Ok(serde_json::json!({ "dropins": 2, "clusters": clusters }))
}

/// Every `/etc/postgresql/<n>/main` (no link anywhere in the chain: a
/// link is a refusal, never a skip; that tree is normally postgres-owned).
fn cluster_mains(ctx: &Ctx) -> HResult<Vec<(OsString, Dir)>> {
    let (parent, name) = split(PG_ETC);
    let etc = ctx.dir(parent).map_err(|e| Fail::io_err("open /etc", e))?;
    let pg_name = OsStr::new(name);
    match etc.kind(pg_name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => return Ok(Vec::new()),
        Kind::Dir => {}
        _ => return Err(Fail::state("hba-app refused: /etc/postgresql is not a plain directory")),
    }
    let pg = etc.sub(pg_name).map_err(|e| Fail::io_err("open /etc/postgresql", e))?;
    let refuse = || Fail::state("hba-app refused: a cluster config path is a symlink");
    let mut out = Vec::new();
    for ver in pg.entries().map_err(|e| Fail::io_err("read /etc/postgresql", e))? {
        if !version_dir_ok(&ver) {
            continue;
        }
        match pg.kind(&ver).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Dir => {}
            Kind::Symlink => return Err(refuse()),
            _ => continue,
        }
        let vd = pg.sub(&ver).map_err(|e| Fail::io_err("open cluster", e))?;
        match vd.kind(OsStr::new("main")).map_err(|e| Fail::io_err("stat", e))? {
            Kind::Dir => {}
            Kind::Symlink => return Err(refuse()),
            _ => continue,
        }
        out.push((ver, vd.sub(OsStr::new("main")).map_err(|e| Fail::io_err("open main", e))?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// `pg hba-app add|remove <id>` (protocol 6): the App's canned line in
/// the K2 block of every cluster's `pg_hba.conf` (owner and mode kept),
/// then a reload of a running Postgres. Every file is checked before any
/// is written; a link, a damaged block or a foreign line in the block
/// refuses the whole call.
pub fn hba_app(ctx: &Ctx, add: bool, id: &AppId) -> HResult<serde_json::Value> {
    let mains = cluster_mains(ctx)?;
    if mains.is_empty() {
        return Err(Fail::state("no Postgres cluster here (/etc/postgresql/<n>/main)"));
    }
    let hba = OsStr::new("pg_hba.conf");
    let mut plan: Vec<(String, &Dir, Option<String>, u32, Option<(u32, u32)>)> = Vec::new();
    for (ver, main) in &mains {
        let st = match main.lstat(hba).map_err(|e| Fail::io_err("stat pg_hba.conf", e))? {
            None => return Err(Fail::state("hba-app refused: a cluster has no pg_hba.conf")),
            Some(st) => st,
        };
        if crate::fsx::kind_of(st.st_mode as u32) != Kind::File {
            return Err(Fail::state("hba-app refused: pg_hba.conf is not a regular file"));
        }
        let bytes = main
            .read_file(hba, HBA_MAX_BYTES)
            .map_err(|e| Fail::io_err("read pg_hba.conf", e))?
            .ok_or_else(|| Fail::state("hba-app refused: pg_hba.conf vanished"))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Fail::state("hba-app refused: pg_hba.conf is not UTF-8"))?;
        let next = hba_edit(text, add, id).map_err(Fail::state)?;
        let owner = ctx.real_owners.then_some((st.st_uid, st.st_gid));
        plan.push((ver.to_string_lossy().to_string(), main, next, (st.st_mode as u32) & 0o7777, owner));
    }
    let mut changed = Vec::new();
    for (ver, main, next, mode, owner) in &plan {
        if let Some(text) = next {
            main.write_atomic(hba, text.as_bytes(), *mode, *owner)
                .map_err(|e| Fail::io_err("write pg_hba.conf", e))?;
            changed.push(ver.clone());
        }
    }
    let mut reloaded = false;
    if !changed.is_empty() {
        let active = ctx
            .exec
            .run(SYSTEMCTL, &["is-active".into(), "--quiet".into(), "postgresql.service".into()], None)
            .is_ok_and(|o| o.status == 0);
        if active {
            ctx.run_ok(SYSTEMCTL, &["reload", "postgresql.service"], None)?;
            reloaded = true;
        }
    }
    Ok(serde_json::json!({
        "app": id.as_str(),
        "role": id.pg_name(),
        "line": add,
        "clusters": plan.iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
        "changed": changed,
        "reloaded": reloaded,
    }))
}

/// `runuser -u postgres -- /usr/bin/psql -X -d <db> …`, SQL on stdin.
/// psql's own stdout goes to stdout; its stderr (Postgres's words) is
/// relayed on failure so the daemon can show the SQL error.
pub fn psql(ctx: &Ctx, db: &DbName, flags: PsqlFlags, sql: &[u8]) -> HResult<()> {
    let mut args: Vec<String> = ["-u", PG_OS_USER, "--", PSQL, "-X", "-d", db.as_str()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if flags.on_error_stop {
        args.push("-v".into());
        args.push("ON_ERROR_STOP=1".into());
    }
    match (flags.tuples_only, flags.unaligned) {
        (true, true) => args.push("-tA".into()),
        (true, false) => args.push("-t".into()),
        (false, true) => args.push("-A".into()),
        (false, false) => {}
    }
    let out = ctx
        .exec
        .run(RUNUSER, &args, Some(sql))
        .map_err(|e| Fail::io(format!("runuser: {}", e.kind_str())))?;
    ctx.print(&out.stdout);
    if out.status != 0 {
        let err = redact_passwords(&String::from_utf8_lossy(&out.stderr));
        let err: String = err.trim().chars().take(2000).collect();
        return Err(Fail::io(format!("psql exit {}: {err}", out.status)));
    }
    Ok(())
}

/// Postgres echoes the failing statement (`LINE 1: … PASSWORD '…'`); drop
/// every line that mentions a password before it reaches the daemon's
/// errors and HTTP answers (security review #10).
pub fn redact_passwords(stderr: &str) -> String {
    stderr
        .lines()
        .map(|l| {
            if l.to_ascii_lowercase().contains("password") {
                "[line with a password redacted]"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn password_lines_are_redacted() {
        let e = "ERROR:  syntax error at or near \"x\"\nLINE 1: CREATE ROLE a PASSWORD 'hunter2' x;\n        ^";
        let r = super::redact_passwords(e);
        assert!(!r.contains("hunter2"));
        assert!(r.contains("syntax error"));
        assert!(r.contains("[line with a password redacted]"));
    }
}
