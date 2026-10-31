//! Hosted mail forwards: rows → Sieve (prd-hostmail-external-forwarding-v1
//! §3.2, §3.4; vs-live FW-A5, FW-A12, FW-A20, FW-A26).
//!
//! The `mail_forwards` row is the truth (`k2_core::mail_forward`). This
//! module writes the mailbox's one K2 Sieve script (`k2`, shared with the
//! out-of-office block, `sieve_user`) FROM the live row, never the other
//! way:
//!
//! - an `active` same-box forward → a plain `redirect` (as before);
//! - an `active` outside forward → the guarded block ([`ForwardGuard`]:
//!   bounces, loop marker, spam and DMARC failures stay home);
//! - anything else (`pending_verify`, `paused`, `removed`, no row) → no
//!   forward block. Mail simply stays in the mailbox (keep-only).
//!
//! **When:** right after every change (the route waits for it), at daemon
//! start (after the Sieve lockdown; it also imports same-box forwards that
//! predate the rows) and hourly. A script that doesn't match what K2 would
//! write is rewritten and the change is logged as a `drift` event.
//!
//! **Holds K2 applies itself** ([`hold_reason`]): air-gap on, or the
//! mailbox's domain `receive-only`, pauses an active outside forward;
//! when the cause goes away the reconcile resumes the forwards it paused
//! for it (FW-A12, FW-A20). Pauses for other reasons are lifted only by a
//! person (`resume`).
//!
//! **Fail closed:** an outside forward is written only when this box's mail
//! host name is known and valid ([`guard_host_ok`]); without it the block
//! is left out (no forward) and the log says why.
//!
//! The daemon log carries ids and counts, never addresses.

use std::sync::Mutex;

use k2_core::mail_forward::{self as store, Forward};
use serde_json::json;

use super::jmap::{SieveScriptInfo, StalwartClient};
use super::sieve_user::{self, guard_host_ok, ForwardBlock, ForwardGuard, UserSieve};

const P: &str = "[mail/forward-sync]";

/// The actor K2 writes for its own changes.
pub const ACTOR_K2: &str = "k2";

/// Removed rows younger than this still get their mailbox checked (a
/// failed Sieve write at unset time is retried).
const REMOVED_RECHECK_SECS: i64 = 7 * 86_400;

/// One forward change or reconcile at a time (the route and the hourly
/// pass never interleave their read-modify-write of a script).
static SYNC: Mutex<()> = Mutex::new(());

// ── the Sieve seam ──────────────────────────────────────────────────

/// What a mailbox's Sieve holds now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Current {
    pub sieve: UserSieve,
    pub scripts: Vec<SieveScriptInfo>,
    /// The `k2` script's source, if there is one.
    pub k2_src: Option<String>,
    /// A leftover `k2-forward` script is present.
    pub legacy: bool,
}

/// Read and write a mailbox's K2 script (Stalwart; a fake in tests).
pub trait SieveIo {
    fn read(&self, account_id: &str) -> Result<Current, String>;
    fn write(&self, account_id: &str, current: &Current, next: &UserSieve) -> Result<(), String>;
}

impl SieveIo for StalwartClient {
    fn read(&self, account_id: &str) -> Result<Current, String> {
        let (sieve, scripts) = sieve_user::load_user_sieve(self, account_id)?;
        let mut k2_src = None;
        for sc in scripts.iter().filter(|s| s.name == sieve_user::SCRIPT_NAME && !s.blob_id.trim().is_empty()) {
            k2_src = Some(String::from_utf8_lossy(&self.sieve_script_blob(account_id, &sc.blob_id)?).into_owned());
        }
        let legacy = scripts.iter().any(|s| s.name == sieve_user::LEGACY_FORWARD_NAME);
        Ok(Current { sieve, scripts, k2_src, legacy })
    }
    fn write(&self, account_id: &str, current: &Current, next: &UserSieve) -> Result<(), String> {
        sieve_user::save_user_sieve(self, account_id, &current.scripts, next)
    }
}

#[cfg(test)]
thread_local! {
    static TEST_IO: std::cell::RefCell<Option<std::rc::Rc<dyn SieveIo>>> = const { std::cell::RefCell::new(None) };
    static TEST_CTX: std::cell::RefCell<Option<Ctx>> = const { std::cell::RefCell::new(None) };
}

/// Tests: this thread's Sieve (`None`: no mail server).
#[cfg(test)]
pub(crate) fn set_test_io(io: Option<std::rc::Rc<dyn SieveIo>>) {
    TEST_IO.with(|c| *c.borrow_mut() = io);
}

/// Tests: this thread's host name and air-gap.
#[cfg(test)]
pub(crate) fn set_test_ctx(ctx: Option<Ctx>) {
    TEST_CTX.with(|c| *c.borrow_mut() = ctx);
}

/// Run `f` with the mail server's Sieve.
fn with_io<T>(f: impl FnOnce(&dyn SieveIo) -> Result<T, String>) -> Result<T, String> {
    #[cfg(test)]
    {
        let io = TEST_IO.with(|c| c.borrow().clone()).ok_or_else(|| "tests: no fake Sieve installed".to_string())?;
        f(io.as_ref())
    }
    #[cfg(not(test))]
    {
        let (client, _) = super::domains::engine_from_db()?;
        f(&client)
    }
}

// ── the context ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ctx {
    /// This box's mail host name (`mail_server.hostname`).
    pub host: Option<String>,
    pub airgap: bool,
    pub now: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// This box's mail host name (lower case), if set.
pub fn mail_host() -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT hostname FROM mail_server WHERE id = 1", [], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

pub fn ctx() -> Ctx {
    #[cfg(test)]
    if let Some(c) = TEST_CTX.with(|c| c.borrow().clone()) {
        return Ctx { now: now(), ..c };
    }
    Ctx { host: mail_host(), airgap: crate::push_routes::airgap_blocks(), now: now() }
}

// ── the mail server's version ───────────────────────────────────────

/// The oldest Stalwart outside forwards are proven on (the live probe,
/// tests/mail/stalwart-forward-external-probe.py, every check green on
/// 0.16.25; on 0.16.10 its control redirect never reached the sink).
/// Same-box forwards work on every version.
pub const V_OUTSIDE_FORWARD: super::defaults::Ver = (0, 16, 25);

/// The refusal hint for an older (or unknown) mail server.
pub const UPGRADE_HINT: &str =
    "outside forwarding needs the mail server upgraded: k2 hostmail upgrade (same-box forwards work as they are)";

#[cfg(test)]
thread_local! {
    static TEST_VERSION: std::cell::RefCell<Option<Option<String>>> = const { std::cell::RefCell::new(None) };
}

/// Tests: this thread's installed mail server version (`Some(None)`:
/// unknown; `None`: the default, the pinned version).
#[cfg(test)]
pub(crate) fn set_test_version(v: Option<Option<String>>) {
    TEST_VERSION.with(|c| *c.borrow_mut() = v);
}

fn installed_version() -> Option<String> {
    #[cfg(test)]
    {
        TEST_VERSION.with(|c| c.borrow().clone()).unwrap_or_else(|| Some(super::defaults::ver_str(V_OUTSIDE_FORWARD)))
    }
    #[cfg(not(test))]
    {
        super::supervisor::row_field("installed_version")
    }
}

/// Can this server's mail server run an outside forward? Unknown → no
/// (fail closed).
pub fn outside_forward_supported() -> bool {
    installed_version().as_deref().and_then(super::defaults::parse_version).is_some_and(|v| v >= V_OUTSIDE_FORWARD)
}

// ── the mailbox ─────────────────────────────────────────────────────

/// The forwarding mailbox as the reconcile needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: String,
    pub address: String,
    pub account_id: Option<String>,
    pub active: bool,
    /// The domain's send mode (`direct` | `relay` | `receive-only`).
    pub send_mode: Option<String>,
}

pub fn mailbox(address_id: &str) -> Result<Option<Mailbox>, String> {
    use rusqlite::OptionalExtension;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT a.id, a.address, a.stalwart_account_id, a.status, d.send_mode FROM mail_addresses a \
         LEFT JOIN mail_domains d ON d.id = a.domain_id WHERE a.id = ?1",
        rusqlite::params![address_id],
        |r| {
            Ok(Mailbox {
                id: r.get(0)?,
                address: r.get(1)?,
                account_id: r.get::<_, Option<String>>(2)?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                active: r.get::<_, String>(3)? == "active",
                send_mode: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("mail forwards: mailbox: {e}"))
}

// ── decisions (pure) ────────────────────────────────────────────────

/// Why K2 itself holds an outside forward paused now, if it does.
/// `verified_ok`: the destination's confirmed address is still confirmed
/// (not stopped); a stopped one pauses the forward for good (`revoked`).
pub fn hold_reason(f: &Forward, mb: &Mailbox, ctx: &Ctx, verified_ok: bool) -> Option<&'static str> {
    if !f.is_external() {
        return None;
    }
    if f.state != store::ST_PENDING_VERIFY && !verified_ok {
        return Some(store::P_REVOKED);
    }
    if ctx.airgap {
        return Some(store::P_AIR_GAP);
    }
    if mb.send_mode.as_deref() == Some("receive-only") {
        return Some(store::P_RECEIVE_ONLY);
    }
    None
}

/// The forward block the live row asks for (`None`: no forward).
pub fn desired_block(f: Option<&Forward>, ctx: &Ctx) -> Option<ForwardBlock> {
    let f = f.filter(|f| f.is_active())?;
    if !f.is_external() {
        return Some(ForwardBlock::internal(f.dest.clone(), f.keep));
    }
    let host = ctx.host.as_deref().filter(|h| guard_host_ok(h))?;
    Some(ForwardBlock { dest: f.dest.clone(), keep: f.keep, guard: Some(ForwardGuard { host: host.to_string() }) })
}

/// The full script K2 wants: the current out-of-office block kept, the
/// forward block from the row.
pub fn desired(current: &Current, f: Option<&Forward>, ctx: &Ctx) -> UserSieve {
    UserSieve { ooo: current.sieve.ooo.clone(), forward: desired_block(f, ctx) }
}

/// Does the mailbox's Sieve already say exactly `want`?
pub fn in_sync(current: &Current, want: &UserSieve) -> bool {
    if current.legacy {
        return false;
    }
    match sieve_user::compose(want) {
        // An empty remainder: no `k2` script, or K2's no-op `keep;`.
        None => current.k2_src.as_deref().is_none_or(|s| s == "keep;\n"),
        Some(src) => current.k2_src.as_deref() == Some(src.as_str()),
    }
}

// ── one mailbox ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Right after a change K2 made (no drift event).
    Change,
    /// The boot or hourly pass (a mismatch is drift).
    Periodic,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Synced {
    /// The script was rewritten.
    pub wrote: bool,
    /// The rewrite fixed drift (Periodic only).
    pub drift: bool,
    /// K2 paused (Some(reason)) or resumed (Some("")) the forward itself.
    pub held: Option<String>,
    /// An outside forward is active but its block was left out (no valid
    /// mail host name).
    pub blocked: bool,
}

/// Is the forward's destination still confirmed (an outside forward's
/// `verified_address_id` row is `verified`, for this mailbox and address)?
pub fn verified_ok(conn: &rusqlite::Connection, f: &Forward) -> bool {
    let Some(id) = f.verified_address_id.as_deref() else { return false };
    match k2_core::email_verify::get(conn, id) {
        Ok(Some(v)) => {
            v.is_verified()
                && v.purpose == k2_core::email_verify::PURPOSE_FORWARD
                && v.subject_ref == f.address
                && v.address == f.dest
        }
        _ => false,
    }
}

fn apply_holds(f: Forward, mb: &Mailbox, ctx: &Ctx) -> Result<(Forward, Option<String>), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // After the reasons above: a mail server too old for outside forwards
    // holds an active one visibly (paused `upgrade_required`), and lifts
    // the hold after `k2 hostmail upgrade`.
    let hold = hold_reason(&f, mb, ctx, !f.is_external() || verified_ok(&conn, &f)).or_else(|| {
        (f.is_external() && f.state != store::ST_PENDING_VERIFY && !outside_forward_supported()).then_some(store::P_UPGRADE)
    });
    let mv = |to: &str, reason: Option<&str>| {
        store::transition(&conn, &f.id, to, &store::Change { reason, actor: ACTOR_K2, ..Default::default() }, ctx.now)
            .map_err(|e| e.to_string())
    };
    match (f.state.as_str(), hold, f.pause_reason.as_deref()) {
        (store::ST_ACTIVE, Some(r), _) => Ok((mv(store::ST_PAUSED, Some(r))?, Some(r.to_string()))),
        (store::ST_PAUSED, Some(r), Some(cur)) if (store::auto_lifts(cur) || r == store::P_REVOKED) && cur != r => {
            Ok((mv(store::ST_PAUSED, Some(r))?, Some(r.to_string())))
        }
        (store::ST_PAUSED, None, Some(cur)) if store::auto_lifts(cur) => Ok((mv(store::ST_ACTIVE, None)?, Some(String::new()))),
        _ => Ok((f, None)),
    }
}

/// Bring one mailbox's Sieve in line with its live row.
pub fn sync_mailbox_with(io: &dyn SieveIo, address_id: &str, why: Why, ctx: &Ctx) -> Result<Synced, String> {
    let _g = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    sync_locked(io, address_id, why, ctx)
}

fn sync_locked(io: &dyn SieveIo, address_id: &str, why: Why, ctx: &Ctx) -> Result<Synced, String> {
    let mut out = Synced::default();
    let mb = mailbox(address_id)?.ok_or_else(|| format!("mail forwards: mailbox {address_id} is gone"))?;
    let live = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::live_for_address(&conn, address_id)?
    };
    let live = match live {
        // A retired mailbox forwards nothing: its row goes too.
        Some(f) if !mb.active => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::transition(
                &conn,
                &f.id,
                store::ST_REMOVED,
                &store::Change { actor: ACTOR_K2, detail: Some(json!({ "why": "mailbox_retired" })), ..Default::default() },
                ctx.now,
            )
            .map_err(|e| e.to_string())?;
            return Ok(out);
        }
        Some(f) => {
            let (f, held) = apply_holds(f, &mb, ctx)?;
            out.held = held;
            Some(f)
        }
        None => None,
    };
    if !mb.active {
        return Ok(out);
    }
    let Some(account) = mb.account_id.as_deref() else {
        return Err(format!("mail forwards: mailbox {address_id} has no mail-server account"));
    };
    let current = io.read(account)?;
    let mut want = desired(&current, live.as_ref(), ctx);
    // Fail closed on a mail server older than the one outside forwards are
    // proven on (or an unknown one): no outside forward block.
    if want.forward.as_ref().is_some_and(|f| f.guard.is_some()) && !outside_forward_supported() {
        want.forward = None;
        k2_core::log_debug!("{P} WARN mailbox {address_id}: outside forward not written: {UPGRADE_HINT}");
    }
    out.blocked = live.as_ref().is_some_and(|f| f.is_active()) && want.forward.is_none();
    if out.blocked {
        k2_core::log_debug!("{P} WARN forward {} is active but not written (mail host name or mail server version)", live.as_ref().map(|f| f.id.as_str()).unwrap_or(""));
    }
    if in_sync(&current, &want) {
        return Ok(out);
    }
    io.write(account, &current, &want)?;
    out.wrote = true;
    if why == Why::Periodic {
        out.drift = true;
        let had = current.sieve.forward.is_some();
        let wants = want.forward.is_some();
        k2_core::log_debug!("{P} drift on mailbox {address_id}: restored (had forward: {had}, wants forward: {wants})");
        if let Some(f) = &live {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::record_event(&conn, &f.id, store::E_DRIFT, ACTOR_K2, &json!({ "hadForward": had, "wantsForward": wants }), ctx.now)?;
        }
    }
    Ok(out)
}

/// [`sync_mailbox_with`] on the mail server.
pub fn sync_mailbox(address_id: &str, why: Why) -> Result<Synced, String> {
    let ctx = ctx();
    with_io(|io| sync_mailbox_with(io, address_id, why, &ctx))
}

// ── state changes other modules use ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeError {
    /// The row didn't move (unknown id, a move the diagram doesn't have).
    Row(store::TransitionError),
    /// The row moved (here as it is now) but the mail server wasn't
    /// updated; the reconcile retries.
    Sieve(Box<Forward>, String),
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeError::Row(e) => write!(f, "{e}"),
            ChangeError::Sieve(_, e) => write!(f, "saved, but the mail server wasn't updated yet ({e}); K2 retries within the hour"),
        }
    }
}

fn reread(id: &str, fallback: Forward) -> Forward {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get(&conn, id).ok().flatten().unwrap_or(fallback)
}

/// Run `f` (a row change) under the sync lock, then write the mailbox's
/// Sieve. Returns the row as it is after the write (a hold may have moved
/// it again).
pub fn change_and_sync(
    address_id: &str,
    f: impl FnOnce(&rusqlite::Connection, i64) -> Result<Forward, store::TransitionError>,
) -> Result<Forward, ChangeError> {
    let ctx = ctx();
    let _g = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        f(&conn, ctx.now).map_err(ChangeError::Row)?
    };
    let synced = with_io(|io| sync_locked(io, address_id, Why::Change, &ctx));
    let id = row.id.clone();
    let row = reread(&id, row);
    // Outside the lock: the webhook follows the outside forwards, and the
    // box brake catches a forward activated while it holds (FW3).
    drop(_g);
    super::forward_outcomes::after_change(&row);
    match synced {
        Ok(_) => Ok(row),
        Err(e) => {
            k2_core::log_debug!("{P} forward {} saved, Sieve write deferred: {}", row.id, crate::email_notify::send::mask_addresses(&e));
            Err(ChangeError::Sieve(Box::new(row), e))
        }
    }
}

/// Move a forward and write its mailbox's Sieve.
pub fn transition_and_sync(id: &str, to: &str, change: &store::Change<'_>) -> Result<Forward, ChangeError> {
    let address_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get(&conn, id)
            .map_err(|e| ChangeError::Row(store::TransitionError::Db(e)))?
            .ok_or(ChangeError::Row(store::TransitionError::NotFound))?
            .address_id
    };
    change_and_sync(&address_id, |conn, now| store::transition(conn, id, to, change, now))
}

/// Pause a forward (keep-only) for `reason`.
pub fn pause(id: &str, reason: &str, actor: &str, detail: Option<serde_json::Value>) -> Result<Forward, ChangeError> {
    transition_and_sync(id, store::ST_PAUSED, &store::Change { reason: Some(reason), actor, detail, ..Default::default() })
}

/// Resume a paused forward.
pub fn resume(id: &str, actor: &str) -> Result<Forward, ChangeError> {
    transition_and_sync(id, store::ST_ACTIVE, &store::Change { actor, ..Default::default() })
}

// ── the whole box ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub checked: usize,
    pub wrote: usize,
    pub drift: usize,
    pub held: usize,
    pub imported: usize,
    pub failed: usize,
}

/// Every mailbox with a forward row (and, with `import`, every active
/// mailbox without one: a forward K2 wrote before the rows existed).
pub fn reconcile_with(io: &dyn SieveIo, import: bool, ctx: &Ctx) -> Report {
    let mut r = Report::default();
    let ids = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::address_ids_with_rows(&conn, ctx.now - REMOVED_RECHECK_SECS) {
            Ok(v) => v,
            Err(e) => {
                k2_core::log_debug!("{P} FAILED: {e}");
                r.failed += 1;
                return r;
            }
        }
    };
    if import {
        r.imported = import_with(io, &ids, ctx, &mut r.failed);
    }
    for id in &ids {
        r.checked += 1;
        match sync_mailbox_with(io, id, Why::Periodic, ctx) {
            Ok(s) => {
                r.wrote += s.wrote as usize;
                r.drift += s.drift as usize;
                r.held += s.held.is_some() as usize;
            }
            Err(e) => {
                r.failed += 1;
                k2_core::log_debug!("{P} mailbox {id} not reconciled: {}", crate::email_notify::send::mask_addresses(&e));
            }
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = store::purge(&conn, ctx.now) {
        k2_core::log_debug!("{P} purge: {e}");
    }
    r
}

/// FW-A5: same-box forwards K2 wrote before 0256 become rows. A forward
/// block on a mailbox with no row that isn't a same-box mailbox (or that
/// carries a guard with no row behind it) is not K2's to keep: it's
/// stripped and recorded as a removed row.
fn import_with(io: &dyn SieveIo, skip: &[String], ctx: &Ctx, failed: &mut usize) -> usize {
    let mailboxes: Vec<(String, String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut st = match conn.prepare(
            "SELECT id, address, stalwart_account_id FROM mail_addresses \
             WHERE status = 'active' AND stalwart_account_id IS NOT NULL AND stalwart_account_id <> '' ORDER BY address",
        ) {
            Ok(s) => s,
            Err(e) => {
                k2_core::log_debug!("{P} import: {e}");
                *failed += 1;
                return 0;
            }
        };
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)));
        match rows {
            Ok(it) => it.filter_map(Result::ok).filter(|(id, _, _): &(String, String, String)| !skip.contains(id)).collect(),
            Err(e) => {
                k2_core::log_debug!("{P} import: {e}");
                *failed += 1;
                return 0;
            }
        }
    };
    let mut n = 0;
    for (id, address, account) in mailboxes {
        let _g = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        let cur = match io.read(&account) {
            Ok(c) => c,
            Err(_) => {
                *failed += 1;
                continue;
            }
        };
        let Some(fwd) = cur.sieve.forward.clone() else { continue };
        let internal = fwd.guard.is_none() && is_minted(&fwd.dest) && !fwd.dest.eq_ignore_ascii_case(&address);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let made = store::create(
            &conn,
            &store::NewForward {
                address_id: &id,
                address: &address,
                dest: &fwd.dest.to_ascii_lowercase(),
                dest_kind: if internal { store::KIND_INTERNAL } else { store::KIND_EXTERNAL },
                keep: fwd.keep,
                set_by: ACTOR_K2,
                state: if internal { store::ST_ACTIVE } else { store::ST_PENDING_VERIFY },
                verified_address_id: None,
            },
            ctx.now,
        );
        let f = match made {
            Ok(f) => f,
            Err(e) => {
                k2_core::log_debug!("{P} import of mailbox {id}: {e}");
                *failed += 1;
                continue;
            }
        };
        let _ = store::record_event(&conn, &f.id, store::E_IMPORTED, ACTOR_K2, &json!({ "internal": internal }), ctx.now);
        if !internal {
            // Never forwarded outside without a confirm: strip it.
            let _ = store::transition(
                &conn,
                &f.id,
                store::ST_REMOVED,
                &store::Change { actor: ACTOR_K2, detail: Some(json!({ "why": "unverified_outside_forward" })), ..Default::default() },
                ctx.now,
            );
            k2_core::log_debug!("{P} mailbox {id}: a forward block with no confirmed destination was removed");
        }
        drop(conn);
        n += 1;
        // Write right away (the row is the truth now).
        if let Err(e) = sync_locked(io, &id, Why::Change, ctx) {
            *failed += 1;
            k2_core::log_debug!("{P} import sync of mailbox {id}: {}", crate::email_notify::send::mask_addresses(&e));
        }
    }
    n
}

fn is_minted(address: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM mail_addresses WHERE address = ?1 AND status = 'active'",
        rusqlite::params![address.trim().to_ascii_lowercase()],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

// ── background ──────────────────────────────────────────────────────

/// Boot: wait for the mail server (and the Sieve lockdown, which holds
/// the enable lock), import and reconcile once, then hourly. Linux only;
/// one detached thread; never blocks boot.
pub fn spawn_background() {
    if !super::supervisor::mail_supported() || cfg!(test) {
        return;
    }
    let spawned = std::thread::Builder::new().name("mail-forward-sync".into()).spawn(|| {
        if std::panic::catch_unwind(run_live).is_err() {
            k2_core::log_debug!("{P} FAILED (daemon carries on): panicked");
        }
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("{P} FAILED (daemon carries on): thread spawn: {e}");
    }
}

/// Ask for a reconcile soon (after `hostmail enable`, an air-gap flip…).
pub fn request_reconcile() {
    NUDGE.store(true, std::sync::atomic::Ordering::SeqCst);
}

static NUDGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg_attr(test, allow(dead_code))]
fn run_live() {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    let mut first = true;
    let mut last_air = k2_core::airgap::enabled();
    loop {
        let status = super::supervisor::current_status();
        let gated = super::defaults::gate(status.as_deref(), super::supervisor::enable_completed());
        let busy = super::supervisor::enable_running().load(Ordering::SeqCst);
        if gated.is_none() && !busy {
            let ctx = ctx();
            match with_io(|io| Ok(reconcile_with(io, first, &ctx))) {
                Ok(r) => {
                    if first || r.wrote + r.held + r.failed + r.imported > 0 {
                        k2_core::log_debug!("{P} pass: {r:?}");
                    }
                    first = false;
                }
                Err(e) => k2_core::log_debug!("{P} pass skipped: {}", crate::email_notify::send::mask_addresses(&e)),
            }
        }
        // Hourly, sooner when nudged or air-gap flips (checked each minute).
        for _ in 0..60 {
            std::thread::sleep(Duration::from_secs(if first { 10 } else { 60 }));
            let air = k2_core::airgap::enabled();
            if NUDGE.swap(false, Ordering::SeqCst) || air != last_air || first {
                last_air = air;
                break;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
