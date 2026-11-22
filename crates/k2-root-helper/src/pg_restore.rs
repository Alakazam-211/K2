//! `pg basebackup|backups|restore-to|drop-drill` (protocol 19,
//! `namespaces.pgrestore`; prd-app-supabase-compat-v1 §11.1, lane E).
//!
//! Point-in-time restore of a named cluster the helper made
//! ([`crate::pg_major::named_of`]), from base backups and the WAL spool
//! archiving already fills. The rules ([`k2_root_proto::pg_restore`]):
//!
//! - Everything that reads or writes cluster data runs as postgres
//!   (`runuser`), with the major's own binaries; root writes only the new
//!   cluster's config (as `ensure-cluster` does) and the helper's state.
//! - A restore is ALWAYS a new cluster beside the source, never in place.
//!   The source's spool is only read (`cp` out of it).
//! - Refused before anything is made: no archiving or spool, no base
//!   backup ended by the time, a hole in the spool after that backup, a
//!   time later than the newest WAL in the spool (or than now).
//! - Failed loudly after: Postgres ending recovery before the time (it
//!   stops; the cluster is left stopped and droppable with `drop-drill`).
//! - `drop-drill` removes only a cluster `restore-to` made as a drill or
//!   one whose restore failed; the helper still never drops any other.
//! - These verbs run long: each holds the `pgrestore` lock for itself and
//!   the `pg` lock only around its root writes.

use k2_root_proto::paths::SYSTEMCTL;
use k2_root_proto::pg_major::{ClusterRow, PgMajor, MKDIR, PG_SOCKET_DIR, RM};
use k2_root_proto::pg_named::{self as named, ClusterName, NamedRec};
use k2_root_proto::pg_restore::{self as pr, BackupInfo, RestoreRec, RestoresState, SpoolSeg, UtcTime, WalSeg};
use serde_json::{json, Value as J};

use crate::apps::put;
use crate::ctx::{Ctx, Fail, HResult};
use crate::pg_major::{
    as_postgres, create_named_layout, lsclusters, named_json, named_of, named_state, named_wait_online, present,
    read_small, real_dir, save_named_state, sql_at, subdir, system_id, CHMOD, DD, DU, FIND,
};

pub const PG_DROPCLUSTER: &str = "/usr/bin/pg_dropcluster";
const TAR: &str = "/usr/bin/tar";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn pause(ctx: &Ctx) {
    if ctx.real_owners || ctx.waits {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

// ── state ───────────────────────────────────────────────────────────────

pub(crate) fn restores_state(ctx: &Ctx) -> HResult<RestoresState> {
    match read_small(ctx, pr::RESTORES_STATE, pr::RESTORES_STATE_CAP)? {
        None => Ok(RestoresState::default()),
        Some(b) => serde_json::from_slice(&b).map_err(|_| Fail::state("pg-restores.json unreadable; nothing changed")),
    }
}

fn save_restores(ctx: &Ctx, s: &RestoresState) -> HResult<()> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|_| Fail::io("encode pg-restores.json"))?;
    let d = subdir(ctx, k2_root_proto::paths::STATE_DIR)?;
    put(ctx, &d, pr::RESTORES_STATE.rsplit('/').next().expect("file"), &bytes).map(|_| ())
}

// ── the spool and the base backups (read as root, never following links) ─

/// The WAL segments in a cluster's spool with their archive times (empty
/// when there is no spool).
fn spool_segs(ctx: &Ctx, m: u32, n: &str) -> HResult<Vec<SpoolSeg>> {
    let d = match ctx.dir(&named::wal_spool(m, n)) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Fail::io_err("open the WAL spool", e)),
    };
    let mut out = Vec::new();
    for f in d.entries().map_err(|e| Fail::io_err("list the WAL spool", e))? {
        let Some(seg) = f.to_str().and_then(WalSeg::parse) else { continue };
        let Some(st) = d.lstat(&f).map_err(|e| Fail::io_err("stat a WAL file", e))? else { continue };
        if crate::fsx::kind_of(crate::fsx::mode(&st)) != crate::fsx::Kind::File {
            continue;
        }
        out.push(SpoolSeg { seg, archived_at: st.st_mtime });
    }
    Ok(out)
}

/// One base backup directory: its stamp and its info (`None` = no info
/// file yet: unfinished, or a failed run).
fn base_backups(ctx: &Ctx, m: u32, n: &str) -> HResult<Vec<(String, Option<BackupInfo>)>> {
    let root = pr::base_dir(m, n);
    let d = match ctx.dir(&root) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Fail::io_err("open the base backups", e)),
    };
    let mut out = Vec::new();
    for f in d.entries().map_err(|e| Fail::io_err("list the base backups", e))? {
        let Some(stamp) = f.to_str().filter(|s| pr::stamp_ok(s)).map(str::to_string) else { continue };
        if !real_dir(ctx, &format!("{root}/{stamp}")) {
            continue;
        }
        let info = read_small(ctx, &format!("{root}/{stamp}/{}", pr::BACKUP_INFO_FILE), 64 * 1024)?
            .and_then(|b| serde_json::from_slice::<BackupInfo>(&b).ok())
            .filter(|i| i.valid() && i.stamp == stamp && i.major == m && i.name == n);
        out.push((stamp, info));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn dir_bytes(ctx: &Ctx, abs: &str) -> u64 {
    as_postgres(ctx, DU, &["-sb", "--", abs], None)
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

fn backup_json(ctx: &Ctx, m: u32, n: &str, stamp: &str, info: Option<&BackupInfo>) -> J {
    let dir = format!("{}/{stamp}", pr::base_dir(m, n));
    match info {
        Some(i) => json!({
            "stamp": stamp, "dir": dir, "complete": true, "bytes": dir_bytes(ctx, &dir),
            "startedAt": UtcTime::from_epoch(i.started_at).token(), "endedAt": UtcTime::from_epoch(i.ended_at).token(),
            "startWal": i.start_wal, "endWal": i.end_wal,
        }),
        None => json!({"stamp": stamp, "dir": dir, "complete": false}),
    }
}

// ── pg backups ──────────────────────────────────────────────────────────

/// `pg backups <M>-<name>` (open, read-only): base backups, the spool, and
/// the window a restore can reach (`restorableFrom` .. `restorableTo`).
pub fn backups(ctx: &Ctx, m: PgMajor, name: &ClusterName) -> HResult<J> {
    let n = name.as_str();
    let (_, rec) = named_of(ctx, m, name)?;
    let list = base_backups(ctx, m.get(), n)?;
    let spool = spool_segs(ctx, m.get(), n)?;
    let mine: Vec<&BackupInfo> = list.iter().filter_map(|(_, i)| i.as_ref()).filter(|i| i.system_id == rec.system_id).collect();
    let oldest = mine.iter().min_by_key(|i| i.ended_at);
    let (range, gap) = match oldest {
        Some(o) => match pr::wal_range(&spool, o.start()) {
            Ok(r) => (Some(r), None),
            Err(miss) => (None, Some(miss.name())),
        },
        None => (None, None),
    };
    let newest_any = spool.iter().map(|s| s.archived_at).max();
    let mut segs: Vec<&SpoolSeg> = spool.iter().collect();
    segs.sort_by_key(|s| s.seg);
    Ok(json!({
        "major": m.get(),
        "name": n,
        "walArchive": rec.wal_archive,
        "baseDir": pr::base_dir(m.get(), n),
        "baseBackups": list.iter().map(|(s, i)| backup_json(ctx, m.get(), n, s, i.as_ref())).collect::<Vec<_>>(),
        "wal": {
            "spool": named::wal_spool(m.get(), n),
            "count": spool.len(),
            "first": segs.first().map(|s| s.seg.name()),
            "last": segs.last().map(|s| s.seg.name()),
            "newestAt": newest_any.map(|t| UtcTime::from_epoch(t).token()),
            "bytes": crate::pg_major::spool_bytes(ctx, m.get(), n),
            "gap": gap,
        },
        "restorableFrom": range.as_ref().and(oldest).map(|o| UtcTime::from_epoch(o.ended_at).token()),
        "restorableTo": range.as_ref().map(|r| UtcTime::from_epoch(r.newest_at).token()),
    }))
}

// ── pg basebackup ───────────────────────────────────────────────────────

/// `pg basebackup <M>-<name>`: a base backup of a running, archiving named
/// cluster into `<base_dir>/<stamp>/` (postgres 0700), its info file, then
/// the oldest beyond [`pr::BASE_KEEP`] (and unfinished ones) removed.
pub fn basebackup(ctx: &Ctx, m: PgMajor, name: &ClusterName) -> HResult<J> {
    let _own = crate::take_lock(ctx, pr::LOCK_NS)?;
    let n = name.as_str();
    let (row, rec) = named_of(ctx, m, name)?;
    if !rec.wal_archive {
        return Err(Fail::state(format!(
            "no_archive: {}/{n} doesn't archive WAL, so a base backup can't be replayed to a time; turn archiving on first; nothing written",
            m.get()
        )));
    }
    if !row.online() {
        return Err(Fail::state(format!("cluster_down: {}/{n} isn't running; start it first; nothing written", m.get())));
    }
    let root = pr::base_dir(m.get(), n);
    as_postgres(ctx, MKDIR, &["-p", "-m", "0700", "--", &root], None)?;
    let st = as_postgres(ctx, k2_root_proto::pg_major::STAT, &["-c", "%F %U", "--", &root], None)?;
    if String::from_utf8_lossy(&st.stdout).trim() != "directory postgres" {
        return Err(Fail::state("base_dir_refused: the base backup folder is not a postgres-owned directory"));
    }
    as_postgres(ctx, CHMOD, &["0700", "--", &root], None)?;
    let started = now();
    let stamp = UtcTime::from_epoch(started).stamp();
    let dir = format!("{root}/{stamp}");
    if present(ctx, &dir) {
        return Err(Fail::state("busy: a base backup of this second already exists; try again"));
    }
    let pt = row.port.to_string();
    let label = format!("k2-{stamp}");
    as_postgres(
        ctx,
        &m.bin("pg_basebackup"),
        &["-h", PG_SOCKET_DIR, "-p", &pt, "-U", "postgres", "-w", "-D", &dir, "-Ft", "-z", "-X", "stream", "-c", "fast", "-l", &label],
        None,
    )?;
    let ended = now();
    let manifest = read_small(ctx, &format!("{dir}/{}", pr::BACKUP_MANIFEST_FILE), pr::MANIFEST_CAP)?
        .ok_or_else(|| Fail::state("backup_incomplete: pg_basebackup wrote no backup_manifest"))?;
    let (start_wal, end_wal) =
        pr::manifest_wal(&manifest).ok_or_else(|| Fail::state("backup_incomplete: the backup_manifest has no WAL range"))?;
    for f in ["base.tar.gz", "pg_wal.tar.gz"] {
        if !present(ctx, &format!("{dir}/{f}")) {
            return Err(Fail::state(format!("backup_incomplete: pg_basebackup wrote no {f}")));
        }
    }
    let info = BackupInfo {
        stamp: stamp.clone(),
        major: m.get(),
        name: n.to_string(),
        system_id: rec.system_id.clone(),
        started_at: started,
        ended_at: ended,
        start_wal: start_wal.name(),
        end_wal: end_wal.name(),
    };
    let body = serde_json::to_vec_pretty(&info).map_err(|_| Fail::io("encode the backup info"))?;
    let info_path = format!("{dir}/{}", pr::BACKUP_INFO_FILE);
    as_postgres(ctx, DD, &[&format!("of={info_path}"), "status=none"], Some(&body))?;
    // Prune: unfinished runs, then the oldest finished beyond the keep.
    let mut pruned = Vec::new();
    let all = base_backups(ctx, m.get(), n)?;
    let mut done: Vec<&String> = all.iter().filter(|(_, i)| i.is_some()).map(|(s, _)| s).collect();
    done.sort();
    let extra = done.len().saturating_sub(pr::BASE_KEEP);
    let old: Vec<String> = done.into_iter().take(extra).cloned().collect();
    for s in all.iter().filter(|(s, i)| i.is_none() && *s != stamp).map(|(s, _)| s.clone()).chain(old) {
        as_postgres(ctx, RM, &["-rf", "--", &format!("{root}/{s}")], None)?;
        pruned.push(s);
    }
    let mut v = backup_json(ctx, m.get(), n, &stamp, Some(&info));
    v["major"] = json!(m.get());
    v["name"] = json!(n);
    v["kept"] = json!(pr::BASE_KEEP);
    v["pruned"] = json!(pruned);
    Ok(v)
}

// ── pg restore-to ───────────────────────────────────────────────────────

/// The source closes its current segment after a fresh commit and waits
/// for the archiver to copy it (so everything up to now is in the spool).
fn flush_source(ctx: &Ctx, m: PgMajor, row: &ClusterRow) -> HResult<()> {
    let out = sql_at(ctx, m, row.port, "postgres", pr::SWITCH_SQL)?;
    let closed = out.lines().last().map(str::trim).unwrap_or("").to_string();
    let closed = WalSeg::parse(&closed).ok_or_else(|| Fail::state("archive_unknown: the source answered no WAL file name"))?;
    for i in 0..pr::ARCHIVE_WAIT_SECS {
        let last = sql_at(ctx, m, row.port, "postgres", pr::LAST_ARCHIVED_SQL)?;
        if WalSeg::parse(last.trim()).is_some_and(|l| l.tli == closed.tli && l >= closed) {
            return Ok(());
        }
        if i + 1 < pr::ARCHIVE_WAIT_SECS {
            pause(ctx);
        }
    }
    Err(Fail::state(format!(
        "archive_behind: the source hasn't archived {} after {} s (is archive_command failing? the cluster health shows the archiver); nothing restored",
        closed.name(),
        pr::ARCHIVE_WAIT_SECS
    )))
}

/// The last FATAL/PANIC lines of a cluster's log (no data: recovery's own
/// lines), for a loud failure.
fn log_tail(ctx: &Ctx, m: u32, n: &str) -> String {
    let path = format!("/var/log/postgresql/postgresql-{m}-{n}.log");
    let Ok(Some(b)) = read_small(ctx, &path, 4 * 1024 * 1024) else { return String::new() };
    let text = String::from_utf8_lossy(&b);
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("FATAL") || l.contains("PANIC") || l.contains("recovery")).collect();
    lines[lines.len().saturating_sub(4)..].join(" | ").chars().take(1200).collect()
}

fn set_restore(ctx: &Ctx, m: u32, n: &str, f: impl FnOnce(&mut RestoreRec)) -> HResult<()> {
    let mut st = restores_state(ctx)?;
    if let Some(r) = st.restores.iter_mut().find(|r| r.major == m && r.name == n) {
        f(r);
    }
    save_restores(ctx, &st)
}

/// `pg restore-to <M>-<name> <new> <time> [--drill]`.
pub fn restore_to(ctx: &Ctx, m: PgMajor, name: &ClusterName, new: &ClusterName, target: UtcTime, drill: bool) -> HResult<J> {
    let _own = crate::take_lock(ctx, pr::LOCK_NS)?;
    let (src, nn) = (name.as_str(), new.as_str());
    if target.epoch() > now() {
        return Err(Fail::state(format!("target_in_future: {} is later than now; nothing restored", target.token())));
    }
    let (row, rec) = named_of(ctx, m, name)?;
    if !present(ctx, &named::wal_spool(m.get(), src)) {
        return Err(Fail::state(format!(
            "no_archive: {}/{src} has no WAL spool, so there is nothing to replay; nothing restored",
            m.get()
        )));
    }
    if row.online() && rec.wal_archive {
        flush_source(ctx, m, &row)?;
    }
    let infos: Vec<BackupInfo> = base_backups(ctx, m.get(), src)?.into_iter().filter_map(|(_, i)| i).collect();
    let base = pr::choose_backup(&infos, &rec.system_id, target).cloned().ok_or_else(|| {
        Fail::state(format!(
            "no_base_backup: no base backup of {}/{src} ended at or before {}; take a base backup first or pick a later time; nothing restored",
            m.get(),
            target.token()
        ))
    })?;
    let spool = spool_segs(ctx, m.get(), src)?;
    let range = pr::wal_range(&spool, base.start()).map_err(|miss| {
        Fail::state(format!(
            "wal_gap: WAL segment {} is missing from the spool after base backup {}; the restore can't replay past it; nothing restored",
            miss.name(),
            base.stamp
        ))
    })?;
    if target.epoch() > range.newest_at {
        return Err(Fail::state(format!(
            "target_after_wal: the newest WAL in the spool was archived at {}; {} is later, and a restore never silently stops at the end; pick a time at or before it; nothing restored",
            UtcTime::from_epoch(range.newest_at).token(),
            target.token()
        )));
    }
    // The new cluster's config and the records, under the pg lock.
    let port = {
        let _pg = crate::take_lock(ctx, "pg")?;
        let st = named_state(ctx)?;
        let port = create_named_layout(ctx, m, new, None, &st)?;
        let d = ctx
            .dir(&format!("{}/conf.d", named::etc_dir(m.get(), nn)))
            .map_err(|e| Fail::io_err("open the cluster's conf.d", e))?;
        put(ctx, &d, pr::RESTORE_CONF_FILE, pr::recovery_conf(m.get(), src, target).as_bytes())?;
        let mut rs = restores_state(ctx)?;
        rs.restores.retain(|r| !(r.major == m.get() && r.name == nn));
        rs.restores.push(RestoreRec {
            major: m.get(),
            name: nn.to_string(),
            from: src.to_string(),
            target: target.token(),
            base_backup: base.stamp.clone(),
            drill,
            ok: false,
            created_at: now(),
        });
        save_restores(ctx, &rs)?;
        port
    };
    // The data: the base backup over a cleared data dir, as postgres.
    let data = named::data_dir(m.get(), nn);
    let bdir = format!("{}/{}", pr::base_dir(m.get(), src), base.stamp);
    as_postgres(ctx, FIND, &[&data, "-mindepth", "1", "-delete"], None)?;
    as_postgres(ctx, TAR, &["-xzf", &format!("{bdir}/base.tar.gz"), "-C", &data], None)?;
    as_postgres(ctx, MKDIR, &["-p", "-m", "0700", "--", &format!("{data}/pg_wal")], None)?;
    as_postgres(ctx, TAR, &["-xzf", &format!("{bdir}/pg_wal.tar.gz"), "-C", &format!("{data}/pg_wal")], None)?;
    as_postgres(ctx, DD, &[&format!("of={data}/recovery.signal"), "status=none"], Some(b""))?;
    as_postgres(ctx, CHMOD, &["0700", "--", &data], None)?;
    // Replay, then promotion.
    let unit = named::unit(m.get(), nn);
    let fail = |why: String| -> Fail {
        let _ = ctx.run_ok(SYSTEMCTL, &["stop", &unit], None);
        let tail = log_tail(ctx, m.get(), nn);
        Fail::state(format!(
            "recovery_failed: {why}{}; {}/{nn} is left stopped (remove it with `pg drop-drill`)",
            if tail.is_empty() { String::new() } else { format!(" — {tail}") },
            m.get()
        ))
    };
    if let Err(e) = ctx.run_ok(SYSTEMCTL, &["start", &unit], None) {
        return Err(fail(format!("the cluster didn't start ({})", e.msg)));
    }
    let mut promoted = false;
    let mut seen_up = false;
    for i in 0..pr::RECOVERY_WAIT_SECS {
        let r = lsclusters(ctx)?.into_iter().find(|r| r.major == m.get() && r.name == nn);
        match r {
            Some(r) if r.online() => {
                seen_up = true;
                if let Ok(a) = sql_at(ctx, m, port, "postgres", pr::IN_RECOVERY_SQL) {
                    if a.trim() == "false" {
                        promoted = true;
                        break;
                    }
                }
            }
            _ if seen_up || i > 30 => {
                return Err(fail("Postgres stopped before reaching the time (a missing WAL segment, or no commit after the time in the WAL)".into()));
            }
            _ => {}
        }
        if i + 1 < pr::RECOVERY_WAIT_SECS {
            pause(ctx);
        }
    }
    if !promoted {
        return Err(fail(format!("recovery didn't finish within {} s", pr::RECOVERY_WAIT_SECS)));
    }
    // Promoted: the recovery settings go; the cluster is recorded as one
    // the helper made.
    let rec_new = {
        let _pg = crate::take_lock(ctx, "pg")?;
        crate::apps::unput(ctx, &format!("{}/conf.d", named::etc_dir(m.get(), nn)), None, pr::RESTORE_CONF_FILE)?;
        let row = named_wait_online(ctx, m, nn)?;
        if row.port != port || row.data_dir != data {
            return Err(fail("the restored cluster answers another port or data dir".into()));
        }
        let rec_new = NamedRec {
            major: m.get(),
            name: nn.to_string(),
            port,
            data_dir: data.clone(),
            system_id: system_id(ctx, m, port)?,
            wal_archive: false,
            created_at: now(),
        };
        let mut st = named_state(ctx)?;
        st.clusters.retain(|r| !(r.major == m.get() && r.name == nn));
        st.clusters.push(rec_new.clone());
        save_named_state(ctx, &st)?;
        set_restore(ctx, m.get(), nn, |r| r.ok = true)?;
        rec_new
    };
    let row = named_wait_online(ctx, m, nn)?;
    let mut v = named_json(ctx, &row, &rec_new);
    v["created"] = json!(true);
    v["restoredFrom"] = json!(named::label(m.get(), src));
    v["target"] = json!(target.token());
    v["baseBackup"] = json!(base.stamp);
    v["walReplayed"] = json!({"first": range.first.name(), "last": range.last.name(), "count": range.count});
    v["drill"] = json!(drill);
    Ok(v)
}

// ── pg drop-drill ───────────────────────────────────────────────────────

/// `pg drop-drill <M>-<name>`: stop and remove a cluster `restore-to` made
/// as a drill, or one whose restore failed. Anything else: `not_drill`.
pub fn drop_drill(ctx: &Ctx, m: PgMajor, name: &ClusterName) -> HResult<J> {
    let _own = crate::take_lock(ctx, pr::LOCK_NS)?;
    let n = name.as_str();
    let mut rs = restores_state(ctx)?;
    let Some(r) = rs.get(m.get(), n).cloned() else {
        return Err(Fail::state(format!(
            "not_drill: {}/{n} isn't a cluster `restore-to` made; the helper never drops any other cluster; nothing changed",
            m.get()
        )));
    };
    if !r.drill && r.ok {
        return Err(Fail::state(format!(
            "not_drill: {}/{n} is a finished restore, not a drill; the helper doesn't drop it; nothing changed",
            m.get()
        )));
    }
    let _pg = crate::take_lock(ctx, "pg")?;
    let data = named::data_dir(m.get(), n);
    let row = lsclusters(ctx)?.into_iter().find(|x| x.major == m.get() && x.name == n);
    let mut st = named_state(ctx)?;
    if st.get(m.get(), n).is_some() {
        named_of(ctx, m, name)?;
    }
    let dropped = match row {
        Some(row) => {
            if row.data_dir != data || !real_dir(ctx, &data) {
                return Err(Fail::state("cluster_layout_refused: the drill's data dir isn't the named-cluster layout; nothing changed"));
            }
            crate::pg_major::no_links_above(ctx, m)?;
            ctx.run_ok(SYSTEMCTL, &["stop", &named::unit(m.get(), n)], None)?;
            ctx.run_ok(PG_DROPCLUSTER, &[&m.token(), n], None)?;
            true
        }
        None => false,
    };
    if crate::apps::unput(ctx, &named::slice_dropin(m.get(), n).rsplit_once('/').expect("dir").0.to_string(), None, "k2-slice.conf")? {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    st.clusters.retain(|c| !(c.major == m.get() && c.name == n));
    save_named_state(ctx, &st)?;
    rs.restores.retain(|x| !(x.major == m.get() && x.name == n));
    save_restores(ctx, &rs)?;
    Ok(json!({"major": m.get(), "name": n, "dropped": dropped, "from": named::label(m.get(), &r.from), "drill": r.drill}))
}
