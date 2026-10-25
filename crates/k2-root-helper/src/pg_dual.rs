//! `pg` namespace, protocol 10 (`namespaces.pgdual`): a live dual-run of
//! one K2 database (prd-app-pg-choice-dualrun-v1 §5, §9).
//!
//! Rules on top of `pg_major`'s:
//! - Every statement is canned in `k2_root_proto::pg_dual`, rendered from
//!   validated tokens only (majors, a K2 database name, a 12-hex id). No
//!   SQL comes from the caller.
//! - Only databases K2 made (`k2_db`: owner `<db>_migrator`) on K2-layout
//!   clusters; never `postgres`, never a template.
//! - The replication password arrives on stdin only. Errors from the two
//!   statements that carry it are reduced to their first `ERROR:` line with
//!   the password masked, so it reaches no audit line, no daemon log and no
//!   answer. psql runs with `log_statement` off for those sessions.
//! - Objects are created and dropped by their fixed names only; teardown
//!   is idempotent and works whatever subset exists.

use k2_root_proto::pg_dual::{self as pd, DualNames, DualOp, DualSecret, ForceOids};
use k2_root_proto::pg_major::PgMajor;
use k2_root_proto::paths::SYSTEMCTL;
use k2_root_proto::verbs::DbName;
use serde_json::{json, Value as J};

use crate::ctx::{Ctx, Fail, HResult};
use crate::pg_major::{cluster_of, confd, db_exists, k2_db, no_links_above, sql_at};

fn names(id: &str) -> HResult<DualNames> {
    DualNames::of(id).ok_or_else(|| Fail::argv("pg dual: the id is not 12 hex digits"))
}

/// The run id inside a validated [`DualNames`] (`k2_repl_<id>`).
fn id_of(n: &DualNames) -> &str {
    n.role.strip_prefix("k2_repl_").unwrap_or(&n.role)
}

fn secret(stdin: &[u8]) -> HResult<DualSecret> {
    let s: DualSecret = serde_json::from_slice(stdin).map_err(|_| Fail::argv("pg dual: stdin is not {\"password\": …}"))?;
    s.validate().map_err(Fail::argv)?;
    Ok(s)
}

/// A failed statement that carried the password: its first `ERROR:` line,
/// the password masked, nothing else from psql (no LINE/CONTEXT echo).
fn masked(what: &str, e: &Fail, pw: &str) -> Fail {
    let line = e.msg.lines().find(|l| l.contains("ERROR:")).unwrap_or("failed").replace(pw, "***");
    let line = crate::pg::redact_passwords(&line);
    let mut f = Fail::io(format!("{what}: {}", line.trim().chars().take(300).collect::<String>()));
    f.code = e.code;
    f
}

fn count(ctx: &Ctx, m: PgMajor, port: u16, db: &str, sql: &str) -> HResult<u64> {
    let t = sql_at(ctx, m, port, db, sql)?;
    t.trim().parse().map_err(|_| Fail::io("pg dual: a count answered no number"))
}

/// The running cluster's `wal_level`.
fn wal_level(ctx: &Ctx, m: PgMajor, port: u16) -> HResult<String> {
    Ok(sql_at(ctx, m, port, "postgres", "SHOW wal_level;\n")?.trim().to_string())
}

/// `pg logical-conf <M> --wal-keep <MB> [--if-logical]` / `--dry-run`.
pub fn logical_conf(ctx: &Ctx, m: PgMajor, wal_keep_mb: Option<u32>, if_logical: bool) -> HResult<J> {
    let row = cluster_of(ctx, m)?;
    let running = if row.online() { Some(wal_level(ctx, m, row.port)?) } else { None };
    let file_now = confd(ctx, m)?
        .read_file(std::ffi::OsStr::new(pd::LOGICAL_CONF_FILE), 4096)
        .map_err(|e| Fail::io_err("read the logical drop-in", e))?
        .map(|b| String::from_utf8_lossy(&b).to_string());
    let Some(mb) = wal_keep_mb else {
        return Ok(json!({
            "major": m.get(), "dryRun": true, "walLevel": running, "dropin": file_now,
            "restartNeeded": running.as_deref().is_some_and(|w| w != "logical"),
        }));
    };
    if !pd::wal_keep_ok(mb) {
        return Err(Fail::argv("logical-conf: --wal-keep out of range"));
    }
    // An agent's live move never changes wal_level (L25): with
    // --if-logical, only an already-logical running cluster gets the file.
    if if_logical && running.as_deref() != Some("logical") {
        return Err(Fail::state(
            "not_logical: the cluster isn't running with wal_level = logical; the owner prepares it first (k2 db upgrade --prepare-live <major>); nothing changed",
        ));
    }
    no_links_above(ctx, m)?;
    let d = confd(ctx, m)?;
    let body = pd::logical_conf(mb);
    let changed = file_now.as_deref() != Some(body.as_str());
    if changed {
        d.write_atomic(std::ffi::OsStr::new(pd::LOGICAL_CONF_FILE), body.as_bytes(), 0o644, ctx.root_owner())
            .map_err(|e| Fail::io_err("write the logical drop-in", e))?;
    }
    let guard = crate::pg::hba_guard(ctx, m)?;
    // max_slot_wal_keep_size reloads; wal_level waits for the restart.
    let mut reloaded = false;
    if changed && row.online() {
        ctx.run_ok(SYSTEMCTL, &["reload", &m.unit()], None)?;
        reloaded = true;
    }
    Ok(json!({
        "major": m.get(), "walKeepMb": mb, "changed": changed, "guardChanged": guard, "reloaded": reloaded,
        "walLevel": running, "restartNeeded": running.as_deref() != Some("logical"),
    }))
}

/// `pg force-rls <M> <db> on`, stdin [`ForceOids`].
pub fn force_rls(ctx: &Ctx, m: PgMajor, db: &DbName, stdin: &[u8]) -> HResult<J> {
    let req: ForceOids = serde_json::from_slice(stdin).map_err(|_| Fail::argv("force-rls: stdin is not {\"oids\": […]}"))?;
    req.validate().map_err(Fail::argv)?;
    let row = cluster_of(ctx, m)?;
    k2_db(ctx, m, row.port, db)?;
    if req.oids.is_empty() {
        return Ok(json!({"major": m.get(), "forced": 0}));
    }
    let out = sql_at(ctx, m, row.port, db.as_str(), &pd::force_rls_sql(&req.oids))?;
    let n: u64 = out.lines().last().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    Ok(json!({"major": m.get(), "forced": n}))
}

/// `pg dual <op> <from> <to> <db> <id>`.
pub fn run(ctx: &Ctx, op: DualOp, from: PgMajor, to: PgMajor, db: &DbName, id: &str, stdin: &[u8]) -> HResult<J> {
    let n = names(id)?;
    match op {
        DualOp::SetupSource => setup_source(ctx, from, db, &n, &secret(stdin)?),
        DualOp::Subscribe => subscribe(ctx, from, to, db, &n, &secret(stdin)?),
        DualOp::Detach => detach(ctx, from, to, db, &n, false),
        DualOp::Teardown => detach(ctx, from, to, db, &n, true),
        DualOp::Status => status(ctx, from, to, db, &n),
    }
}

fn setup_source(ctx: &Ctx, from: PgMajor, db: &DbName, n: &DualNames, s: &DualSecret) -> HResult<J> {
    let src = cluster_of(ctx, from)?;
    if !src.online() {
        return Err(Fail::state("cluster_down: the source cluster is not running"));
    }
    k2_db(ctx, from, src.port, db)?;
    if wal_level(ctx, from, src.port)? != "logical" {
        return Err(Fail::state("not_logical: the source cluster isn't running with wal_level = logical; nothing changed"));
    }
    if count(ctx, from, src.port, "postgres", &pd::role_count_sql(n))? != 0 {
        return Err(Fail::state("dual_exists: this run's replication role already exists; tear it down first; nothing changed"));
    }
    // A-L2: teardown drops these names; they must be K2's own.
    if count(ctx, from, src.port, db.as_str(), &pd::reserved_count_sql(n))? != 0 {
        return Err(Fail::state(format!(
            "reserved_names: the database already has a schema {}, a publication {} or an event trigger {}; K2 won't take them over; nothing changed",
            pd::FREEZE_SCHEMA, n.publication, pd::FREEZE_TRIGGER
        )));
    }
    // PA24 + A-M2: before the role exists, a k2_repl_* role is refused
    // physical replication and every database but this run's one.
    let guard = crate::pg::hba_dual(ctx, from, db.as_str(), id_of(n), true)?;
    sql_at(ctx, from, src.port, "postgres", &pd::setup_role_sql(n, db.as_str(), s))
        .map_err(|e| masked("setup-source: the replication role", &e, &s.password))?;
    sql_at(ctx, from, src.port, db.as_str(), &pd::setup_db_sql(n, db.as_str()))?;
    Ok(json!({
        "major": from.get(), "role": n.role, "publication": n.publication, "slot": n.slot,
        "freeze": pd::FREEZE_TRIGGER, "guardChanged": guard,
    }))
}

fn subscribe(ctx: &Ctx, from: PgMajor, to: PgMajor, db: &DbName, n: &DualNames, s: &DualSecret) -> HResult<J> {
    let src = cluster_of(ctx, from)?;
    let dst = cluster_of(ctx, to)?;
    if !src.online() || !dst.online() {
        return Err(Fail::state("cluster_down: both clusters must be running"));
    }
    k2_db(ctx, to, dst.port, db)?;
    // PA25: quiesced before the subscription exists, FORCE lifted (the
    // apply worker writes as the owner), no other subscription in it.
    let ready = sql_at(ctx, to, dst.port, db.as_str(), &pd::target_ready_sql(db.as_str()))?;
    if ready.trim() != "0,0,0" {
        return Err(Fail::state(
            "target_not_ready: the target database must be quiesced, have FORCE RLS lifted and no subscription (connlimit,forced,subscriptions answered otherwise); nothing changed",
        ));
    }
    if count(ctx, from, src.port, db.as_str(), &pd::publication_count_sql(n))? != 1 {
        return Err(Fail::state("no_publication: run setup-source first; nothing changed"));
    }
    sql_at(ctx, to, dst.port, db.as_str(), &pd::subscribe_sql(n, db.as_str(), src.port, s))
        .map_err(|e| masked("subscribe", &e, &s.password))?;
    Ok(json!({"from": from.get(), "to": to.get(), "subscription": n.subscription, "slot": n.slot}))
}

/// Detach (the cut-over) or teardown (abort, failure, cleanup). The
/// target's subscription first (when its cluster runs and the database is
/// there), then the source's slot, freeze, grants and publication;
/// teardown also drops the role. Idempotent.
fn detach(ctx: &Ctx, from: PgMajor, to: PgMajor, db: &DbName, n: &DualNames, teardown: bool) -> HResult<J> {
    let mut notes: Vec<String> = Vec::new();
    let mut dropped_subscription = false;
    match cluster_of(ctx, to) {
        Ok(dst) if dst.online() => {
            if db_exists(ctx, to, dst.port, db)? {
                k2_db(ctx, to, dst.port, db)?;
                if count(ctx, to, dst.port, db.as_str(), &pd::subscription_count_sql(n))? > 0 {
                    sql_at(ctx, to, dst.port, db.as_str(), &pd::drop_subscription_sql(n))?;
                    dropped_subscription = true;
                }
            }
        }
        Ok(_) => {
            if !teardown {
                return Err(Fail::state("cluster_down: the target cluster is not running; nothing changed"));
            }
            notes.push("the target cluster is down: its subscription (if any) was left".into());
        }
        Err(e) if teardown => notes.push(format!("target: {}", e.msg)),
        Err(e) => return Err(e),
    }
    let src = cluster_of(ctx, from)?;
    if !src.online() {
        return Err(Fail::state("cluster_down: the source cluster is not running"));
    }
    // A-M2: the role's way in goes first (the guards stay and refuse it).
    let hba_changed = crate::pg::hba_dual(ctx, from, db.as_str(), id_of(n), false)?;
    if db_exists(ctx, from, src.port, db)? {
        k2_db(ctx, from, src.port, db)?;
        // A-M1: a closed database (a kept source) still gets its slot,
        // freeze, grants and publication dropped: the helper alone goes in
        // (connection limit 0 first) and closes it again, whatever happens.
        let closed = sql_at(ctx, from, src.port, "postgres", &pd::db_open_sql(db.as_str()))?.trim() == "false";
        if closed {
            sql_at(ctx, from, src.port, "postgres", &pd::open_for_cleanup_sql(db.as_str()))?;
            notes.push("the source database was closed: opened to the helper alone for cleanup, closed again".into());
        }
        let done = sql_at(ctx, from, src.port, db.as_str(), &pd::detach_source_sql(n, db.as_str()));
        if closed {
            sql_at(ctx, from, src.port, "postgres", &pd::close_after_cleanup_sql(db.as_str()))?;
        }
        done?;
    } else {
        notes.push("the source database is gone".into());
    }
    if teardown {
        sql_at(ctx, from, src.port, "postgres", &pd::drop_role_sql(n, db.as_str()))?;
    }
    Ok(json!({
        "from": from.get(), "to": to.get(), "teardown": teardown,
        "subscriptionDropped": dropped_subscription, "roleDropped": teardown, "hbaChanged": hba_changed, "notes": notes,
    }))
}

/// Read-only: the numbers of both sides, each `{error}` when it can't be
/// read (a stopped cluster, a dropped database).
fn status(ctx: &Ctx, from: PgMajor, to: PgMajor, db: &DbName, n: &DualNames) -> HResult<J> {
    let side = |m: PgMajor, sql: &str| -> J {
        let r = (|| -> HResult<J> {
            let row = cluster_of(ctx, m)?;
            if !row.online() {
                return Err(Fail::state("cluster_down"));
            }
            if !db_exists(ctx, m, row.port, db)? {
                return Err(Fail::state("no_database"));
            }
            k2_db(ctx, m, row.port, db)?;
            let t = sql_at(ctx, m, row.port, db.as_str(), sql)?;
            serde_json::from_str(t.trim()).map_err(|_| Fail::io("status answer is not JSON"))
        })();
        r.unwrap_or_else(|e| json!({"error": e.msg}))
    };
    Ok(json!({
        "from": from.get(), "to": to.get(),
        "source": side(from, &pd::status_source_sql(n)),
        "target": side(to, &pd::status_target_sql(n)),
    }))
}
