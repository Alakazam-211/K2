//! Hosted mail forwards: rows → Sieve (prd-hostmail-external-forwarding-v1
//! §3.2, §3.4; vs-live FW-A5, FW-A12, FW-A20, FW-A26).
//!
//! The `mail_forwards` rows are the truth (`k2_core::mail_forward`). This
//! module writes the mailbox's one K2 Sieve script (`k2`, shared with the
//! out-of-office block, `sieve_user`) FROM its live rows, never the other
//! way:
//!
//! - an `active` same-box row → a plain `redirect` (as before);
//! - an `active` outside row → a `redirect` behind the guard
//!   ([`ForwardGuard`]: bounces, loop marker, spam and DMARC failures stay
//!   home);
//! - anything else (`pending_verify`, `paused`, `removed`, no row) → no
//!   redirect for that row. With none left, mail simply stays in the mailbox.
//!
//! **Several destinations.** Stalwart's Sieve runs one `redirect` per
//! destination and queues each as its own message (own queue id, same
//! envelope sender), so the caps and the delivery outcomes count every copy.
//! But its user-script limit defaults to `maxRedirects: 1` and a `redirect`
//! over the limit is **silently skipped**, so before K2 writes a script with
//! more redirects than it has now it raises the limit server-wide
//! ([`SieveIo::ensure_redirects`], [`REDIRECT_LIMIT`]; never lowers it). If that
//! fails the old script stays and the change reports as deferred: K2 never
//! writes a script that would quietly drop a destination. A script with fewer
//! redirects always goes: a Stop or a removal must never wait on the limit.
//! Mailbox users can't write Sieve (the lockdown), so only K2's own scripts
//! use the room.
//!
//! **When:** right after every change (the route waits for it), at daemon
//! start (after the Sieve lockdown; it also imports same-box forwards that
//! predate the rows) and hourly. A script that doesn't match what K2 would
//! write is rewritten and the change is logged as a `drift` event. The hourly
//! pass also reads the mailboxes with no rows ([`plan_strip`]): an outbound
//! action in one of their scripts has no row behind it, so that action goes
//! (K2's own same-box forward becomes a row instead). Every other rule stays,
//! the old script is kept, the audit log has the targets, and the owner is told.
//!
//! **Out-of-office** changes come through here too ([`patch_ooo`]), under the
//! same lock: the forward block is written from the rows, never read back
//! from the script.
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
    /// Its source, if it has one.
    pub legacy_src: Option<String>,
}

/// The most `redirect`s one script may run: one per destination.
pub const REDIRECT_LIMIT: u64 = store::MAX_DESTINATIONS as u64;
/// The most messages one script may send: a `vacation` reply counts too.
pub const OUT_MESSAGE_LIMIT: u64 = REDIRECT_LIMIT + 1;

/// The interpreter limits to write, or `None` when they're already high
/// enough. Never lowers what an operator set; absent means Stalwart's default
/// (`maxRedirects` 1, `maxOutMessages` 3).
pub fn plan_redirect_limits(redirects: Option<u64>, out_messages: Option<u64>) -> Option<(u64, u64)> {
    let (r, o) = (redirects.unwrap_or(1), out_messages.unwrap_or(3));
    (r < REDIRECT_LIMIT || o < OUT_MESSAGE_LIMIT).then_some((r.max(REDIRECT_LIMIT), o.max(OUT_MESSAGE_LIMIT)))
}

/// Read and write a mailbox's K2 script (Stalwart; a fake in tests).
pub trait SieveIo {
    fn read(&self, account_id: &str) -> Result<Current, String>;
    fn write(&self, account_id: &str, current: &Current, next: &UserSieve) -> Result<(), String>;
    /// Write a source K2 didn't compose (someone's rules, less an outbound action).
    /// `activate`: false leaves a script that isn't running not running.
    fn write_source(&self, account_id: &str, current: &Current, source: &str, activate: bool) -> Result<(), String>;
    /// Make sure a script may run [`REDIRECT_LIMIT`] redirects, loaded and live.
    fn ensure_redirects(&self) -> Result<(), String>;
}

impl SieveIo for StalwartClient {
    fn read(&self, account_id: &str) -> Result<Current, String> {
        let (sieve, scripts) = sieve_user::load_user_sieve(self, account_id)?;
        let mut k2_src = None;
        for sc in scripts.iter().filter(|s| s.name == sieve_user::SCRIPT_NAME && !s.blob_id.trim().is_empty()) {
            k2_src = Some(String::from_utf8_lossy(&self.sieve_script_blob(account_id, &sc.blob_id)?).into_owned());
        }
        let legacy = scripts.iter().any(|s| s.name == sieve_user::LEGACY_FORWARD_NAME);
        let mut legacy_src = None;
        for sc in scripts.iter().filter(|s| s.name == sieve_user::LEGACY_FORWARD_NAME && !s.blob_id.trim().is_empty()) {
            legacy_src = Some(String::from_utf8_lossy(&self.sieve_script_blob(account_id, &sc.blob_id)?).into_owned());
        }
        Ok(Current { sieve, scripts, k2_src, legacy, legacy_src })
    }
    fn write(&self, account_id: &str, current: &Current, next: &UserSieve) -> Result<(), String> {
        sieve_user::save_user_sieve(self, account_id, &current.scripts, next)
    }
    fn write_source(&self, account_id: &str, current: &Current, source: &str, activate: bool) -> Result<(), String> {
        sieve_user::save_user_source(self, account_id, &current.scripts, source, activate)
    }
    fn ensure_redirects(&self) -> Result<(), String> {
        // "Owed" is written before the save and cleared after the reload, so a
        // restart between them can't leave Stalwart on the old limit while K2
        // shows every destination active.
        let owed = |v: bool| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_reload_owed(&conn, v)
        };
        let (redirects, out) = self.sieve_user_interpreter_limits()?;
        if let Some((r, o)) = plan_redirect_limits(redirects, out) {
            owed(true)?;
            self.sieve_user_interpreter_set_limits(r, o)?;
        }
        let reload = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::reload_owed(&conn)
        };
        if reload {
            self.action_reload_settings()?;
            owed(false)?;
        }
        Ok(())
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

/// The forward block the live rows ask for (`None`: no forward). `outside`:
/// may outside destinations be written at all (the mail server's version);
/// without a valid mail host name they aren't either (fail closed).
///
/// A copy stays in the mailbox when any active row keeps one: it never loses
/// mail to a row that said no-keep.
pub fn desired_block(rows: &[Forward], ctx: &Ctx, outside: bool) -> Option<ForwardBlock> {
    let active: Vec<&Forward> = rows.iter().filter(|f| f.is_active()).collect();
    let keep = active.iter().any(|f| f.keep);
    let same_box: Vec<String> = active.iter().filter(|f| !f.is_external()).map(|f| f.dest.clone()).collect();
    let host = ctx.host.as_deref().filter(|h| outside && guard_host_ok(h));
    let others: Vec<String> = if host.is_some() { active.iter().filter(|f| f.is_external()).map(|f| f.dest.clone()).collect() } else { Vec::new() };
    let mut others = others.into_iter();
    match (others.next(), host) {
        (Some(dest), Some(host)) => {
            Some(ForwardBlock { dest, keep, guard: Some(ForwardGuard { host: host.to_string(), stay_home: Vec::new() }), more: others.collect(), same_box })
        }
        _ => {
            let mut inside = same_box.into_iter();
            let dest = inside.next()?;
            Some(ForwardBlock { dest, keep, guard: None, more: inside.collect(), same_box: Vec::new() })
        }
    }
}

/// K2's system senders (their mail stays home), sorted so a rewrite is stable.
fn system_senders() -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out: Vec<String> = k2_core::email_notify::list_senders(&conn, false)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.is_system())
        .map(|s| s.address.to_ascii_lowercase())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The full script K2 wants: the current out-of-office block kept, the
/// forward block from the rows.
pub fn desired(current: &Current, rows: &[Forward], ctx: &Ctx, outside: bool) -> UserSieve {
    let mut forward = desired_block(rows, ctx, outside);
    if let Some(g) = forward.as_mut().and_then(|f| f.guard.as_mut()) {
        g.stay_home = system_senders();
    }
    UserSieve { ooo: current.sieve.ooo.clone(), forward }
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
    // As it is now, under this lock: a pause that landed since it was read counts.
    let f = store::get(&conn, &f.id)?.unwrap_or(f);
    // After the reasons above: a mail server too old for outside forwards
    // holds an active one visibly (paused `upgrade_required`), and lifts
    // the hold after `k2 hostmail upgrade`.
    let hold = hold_reason(&f, mb, ctx, !f.is_external() || verified_ok(&conn, &f))
        .or_else(|| (f.is_external() && f.state != store::ST_PENDING_VERIFY && !outside_forward_supported()).then_some(store::P_UPGRADE))
        // An agent's outside forward on its own mailbox follows that workspace's
        // agent sending, also when the owner turns it off later.
        .or_else(|| {
            let off = f.state != store::ST_PENDING_VERIFY
                && f.agent_ws.as_deref().is_some_and(|ws| super::access::workspace_agent_send_conn(&conn, ws, &k2_core::app_settings::load().mail_agent_send) != "on");
            off.then_some(store::P_AGENT_SEND_OFF)
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
        // K2 lets go, but someone else paused it too: it stays paused, for them.
        (store::ST_PAUSED, None, Some(cur)) if store::auto_lifts(cur) && f.held_by.is_some() => Ok((store::hold_to_manual(&conn, &f.id, ctx.now)?, Some(store::P_MANUAL.to_string()))),
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
        store::live_rows(&conn, address_id)?
    };
    // A retired mailbox forwards nothing: its rows go too.
    if !mb.active {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for f in &live {
            store::transition(
                &conn,
                &f.id,
                store::ST_REMOVED,
                &store::Change { actor: ACTOR_K2, detail: Some(json!({ "why": "mailbox_retired" })), ..Default::default() },
                ctx.now,
            )
            .map_err(|e| e.to_string())?;
        }
        return Ok(out);
    }
    let mut rows = Vec::with_capacity(live.len());
    let (mut sending_off, mut sending_on) = (Vec::new(), Vec::new());
    for f in live {
        let was = f.pause_reason.clone();
        let (f, held) = apply_holds(f, &mb, ctx)?;
        match held.as_deref() {
            Some(store::P_AGENT_SEND_OFF) => sending_off.push(f.dest.clone()),
            Some("") if was.as_deref() == Some(store::P_AGENT_SEND_OFF) => sending_on.push(f.dest.clone()),
            _ => {}
        }
        if held.is_some() {
            out.held = held;
        }
        rows.push(f);
    }
    // One notice per change. Back on restarts outside mail, so the owner hears.
    if !sending_off.is_empty() {
        super::forward_verify::tell_owner(
            &format!("Forwarding from {} is paused", mb.address),
            &format!(
                "Agent sending is off for this mailbox's workspace, so K2 paused the outside forwards its agent set: {}. New mail stays in the mailbox. They go back on by themselves if agent sending is turned on again. To keep one on anyway, resume it, and it no longer follows agent sending: k2 hostmail forward resume {} --to <address>.",
                sending_off.join(", "),
                mb.address
            ),
            &[],
        );
    }
    if !sending_on.is_empty() {
        super::forward_verify::tell_owner(
            &format!("Forwarding from {} is back on", mb.address),
            &format!(
                "Agent sending is on again for this mailbox's workspace, so K2 turned back on the outside forwards its agent set: {}. New mail goes there again. To stop it: k2 hostmail forward pause {}.",
                sending_on.join(", "),
                mb.address
            ),
            &[],
        );
    }
    let Some(account) = mb.account_id.as_deref() else {
        return Err(format!("mail forwards: mailbox {address_id} has no mail-server account"));
    };
    let current = io.read(account)?;
    // Fail closed on a mail server older than the one outside forwards are
    // proven on (or an unknown one): no outside redirect.
    let outside_ok = outside_forward_supported();
    if !outside_ok && rows.iter().any(|f| f.is_active() && f.is_external()) {
        k2_core::log_debug!("{P} WARN mailbox {address_id}: outside forward not written: {UPGRADE_HINT}");
    }
    let want = desired(&current, &rows, ctx, outside_ok);
    let written = want.forward.as_ref().map_or(0, ForwardBlock::redirects);
    let active = rows.iter().filter(|f| f.is_active()).count();
    out.blocked = written < active;
    if out.blocked {
        k2_core::log_debug!("{P} WARN mailbox {address_id}: {} active forward(s) not written (mail host name or mail server version)", active - written);
    }
    // Only more redirects than the script has now need the limit first, and
    // they wait for it. Fewer always go: a Stop must never wait on this. The
    // periodic pass also repairs a limit that went back down; on its own that
    // failing changes nothing.
    let had = current.sieve.forward.as_ref().map_or(0, ForwardBlock::redirects);
    if written > 1 && written > had {
        io.ensure_redirects()?;
    } else if written > 1 && why == Why::Periodic {
        if let Err(e) = io.ensure_redirects() {
            k2_core::log_debug!("{P} WARN mailbox {address_id}: the redirect limit: {}", crate::email_notify::send::mask_addresses(&e));
        }
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
        if let Some(f) = rows.first() {
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
            // Within the minute, not within the hour: a Stop may be waiting on it.
            request_reconcile();
            Err(ChangeError::Sieve(Box::new(row), e))
        }
    }
}

/// [`change_and_sync`] for a change that has to read the row and decide under
/// the sync lock: `f` takes the database lock itself, so it may call anything
/// that does. An `Err` from `f` changes nothing and writes nothing.
pub fn locked_change<E>(address_id: &str, f: impl FnOnce(i64) -> Result<Forward, E>) -> Result<Result<Forward, ChangeError>, E> {
    let ctx = ctx();
    let guard = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    let row = f(ctx.now)?;
    let synced = with_io(|io| sync_locked(io, address_id, Why::Change, &ctx));
    let id = row.id.clone();
    let row = reread(&id, row);
    drop(guard);
    super::forward_outcomes::after_change(&row);
    Ok(match synced {
        Ok(_) => Ok(row),
        Err(e) => {
            k2_core::log_debug!("{P} forward {} saved, Sieve write deferred: {}", row.id, crate::email_notify::send::mask_addresses(&e));
            Err(ChangeError::Sieve(Box::new(row), e))
        }
    })
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

/// Change a mailbox's out-of-office (`None`: off), under the sync lock. The
/// forward block comes from its rows, so an out-of-office write can neither
/// invent a forward nor bring back one that was stopped.
pub fn patch_ooo(address_id: &str, ooo: Option<sieve_user::OooBlock>) -> Result<UserSieve, String> {
    let ctx = ctx();
    with_io(|io| {
        let _g = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        let mb = mailbox(address_id)?.ok_or_else(|| format!("mail forwards: mailbox {address_id} is gone"))?;
        let account = mb.account_id.as_deref().ok_or_else(|| format!("mail forwards: mailbox {address_id} has no mail-server account"))?;
        let live = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::live_rows(&conn, address_id)?
        };
        let current = io.read(account)?;
        let mut want = desired(&current, &live, &ctx, outside_forward_supported());
        want.ooo = ooo;
        // This write may bring back redirects: the limit goes up first, or the
        // mail server silently skips the extra ones.
        let written = want.forward.as_ref().map_or(0, ForwardBlock::redirects);
        if written > 1 && written > current.sieve.forward.as_ref().map_or(0, ForwardBlock::redirects) {
            io.ensure_redirects()?;
        }
        if !in_sync(&current, &want) {
            io.write(account, &current, &want)?;
        }
        Ok(want)
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub checked: usize,
    pub wrote: usize,
    pub drift: usize,
    pub held: usize,
    pub imported: usize,
    pub failed: usize,
    /// Mailboxes the import couldn't read or record: it's still owed.
    pub import_failed: usize,
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
    let mut unread = std::collections::HashSet::new();
    if import {
        r.imported = import_with(io, &ids, ctx, &mut r.failed, &mut unread);
        r.import_failed = unread.len();
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
    strip_unbacked(io, ctx, &unread, &mut r);
    purge_kept_scripts(ctx.now);
    // Rows that can't arrive are listed for the owner, never silently changed.
    super::forward_verify::note_cant_arrive(ctx.host.as_deref(), ctx.now);
    // A notice that "went" and has since bounced is corrected here too.
    super::forward_verify::correct_bounced_notices(ctx.now);
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = store::purge(&conn, ctx.now) {
        k2_core::log_debug!("{P} purge: {e}");
    }
    r
}

/// What to do about the outbound actions in a mailbox with no live row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StripPlan {
    /// Nothing outbound, or only redirects to mailboxes here in rules K2 didn't write.
    Nothing,
    /// K2's own same-box forward from before the rows: it becomes a row.
    Import(ForwardBlock),
    /// K2 wrote all of it: write it again without the forward.
    Compose(UserSieve),
    /// Someone's rules: only the outbound commands go.
    Source(String),
}

/// The one decision for a mailbox with no live row. `is_here`: an active mailbox
/// on this box. A script K2 didn't write is never recomposed: that would lose
/// its other rules (the lockdown keeps those on purpose, `K2Unrecognised`).
pub(crate) fn plan_strip(current: &Current, address: &str, is_here: &dyn Fn(&str) -> bool) -> StripPlan {
    use super::sieve_lockdown::{classify, outbound_actions, remove_outbound, Owner};
    let own = |name: &str, src: &str| classify(name, src, &|_| true).0 == Owner::K2;
    let k2 = current.k2_src.as_deref();
    let legacy = current.legacy_src.as_deref();
    if ![k2, legacy].into_iter().flatten().any(|src| !outbound_actions(src).is_empty()) {
        return StripPlan::Nothing;
    }
    let k2_own = k2.is_none_or(|src| own(sieve_user::SCRIPT_NAME, src));
    let legacy_own = legacy.is_none_or(|src| own(sieve_user::LEGACY_FORWARD_NAME, src));
    let without_forward = || StripPlan::Compose(UserSieve { ooo: current.sieve.ooo.clone(), forward: None });
    if k2_own && legacy_own {
        // Only a script that's running: importing one that isn't would start it.
        let running = outbound_is_running(current);
        let same_box = current.sieve.forward.as_ref().filter(|f| running && f.guard.is_none() && f.redirects() == 1 && is_here(&f.dest) && !f.dest.eq_ignore_ascii_case(address));
        return same_box.map_or_else(without_forward, |f| StripPlan::Import(f.clone()));
    }
    let legacy_changes = legacy.is_some_and(|src| remove_outbound(src, is_here) != src);
    match k2 {
        // K2's script, and someone's rules in the old `k2-forward` beside it. The
        // old one only ever ran instead of `k2`, and its copy is kept.
        Some(_) if k2_own => {
            if legacy_changes {
                without_forward()
            } else {
                StripPlan::Nothing
            }
        }
        Some(src) => {
            let next = remove_outbound(src, is_here);
            if next == src && !legacy_changes {
                StripPlan::Nothing
            } else {
                StripPlan::Source(next)
            }
        }
        None => match legacy {
            Some(src) if legacy_changes => StripPlan::Source(remove_outbound(src, is_here)),
            _ => StripPlan::Nothing,
        },
    }
}

/// Is a script holding an outbound action the mailbox's active one?
fn outbound_is_running(current: &Current) -> bool {
    [(sieve_user::SCRIPT_NAME, &current.k2_src), (sieve_user::LEGACY_FORWARD_NAME, &current.legacy_src)].into_iter().any(|(name, src)| {
        src.as_deref().is_some_and(|src| !super::sieve_lockdown::outbound_actions(src).is_empty())
            && current.scripts.iter().any(|s| s.name == name && s.is_active)
    })
}

/// How long K2 keeps a script it changed.
const REMOVED_KEEP_SECS: i64 = 90 * 86_400;

/// Where the scripts K2 changed are kept: `~/.k2/mail-sieve-removed/`, 0600.
fn removed_dir() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(d) = TEST_REMOVED_DIR.with(|c| c.borrow().clone()) {
        return d;
    }
    if cfg!(test) {
        // House rule: tests never touch the real `~/.k2`.
        return std::env::temp_dir().join(format!("k2-mail-sieve-removed-test-{}", std::process::id()));
    }
    k2_core::paths::home_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join(".k2").join("mail-sieve-removed")
}

#[cfg(test)]
thread_local! {
    static TEST_REMOVED_DIR: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    static TEST_AUDIT: std::cell::RefCell<Vec<serde_json::Value>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: keep old scripts here on this thread.
#[cfg(test)]
pub(crate) fn set_test_removed_dir(d: Option<std::path::PathBuf>) {
    TEST_REMOVED_DIR.with(|c| *c.borrow_mut() = d);
}

/// Tests: the strip's audit lines on this thread.
#[cfg(test)]
pub(crate) fn take_strip_audit() -> Vec<serde_json::Value> {
    TEST_AUDIT.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// The start of a kept copy's file name: readable, short enough for any address
/// (a file name stops at 255 bytes), and exact through the hash.
pub(crate) fn kept_prefix(address: &str) -> String {
    let safe: String = address.chars().take(120).map(|c| if c.is_ascii_alphanumeric() || "@._-".contains(c) { c } else { '_' }).collect();
    format!("{safe}-{}-", &k2_core::email_verify::sha256_hex(address)[..12])
}

/// Keep what's about to change, whole, for the owner. The name follows the
/// content, so a write that keeps failing doesn't pile up copies.
fn keep_old_scripts(address: &str, current: &Current) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let mut text = String::new();
    for (name, src) in [(sieve_user::SCRIPT_NAME, &current.k2_src), (sieve_user::LEGACY_FORWARD_NAME, &current.legacy_src)] {
        if let Some(src) = src {
            text.push_str(&format!("# ---- {address}, script {name}, as it was before K2 changed it ----\n{src}\n"));
        }
    }
    let dir = removed_dir();
    let path = dir.join(format!("{}{}.sieve", kept_prefix(address), &k2_core::email_verify::sha256_hex(&text)[..16]));
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        open.mode(0o600);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("chmod {}: {e}", dir.display()))?;
    }
    let mut f = open.open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Drop kept copies older than [`REMOVED_KEEP_SECS`], and any whose mailbox isn't
/// active here any more.
fn purge_kept_scripts(now: i64) {
    let Ok(entries) = std::fs::read_dir(removed_dir()) else { return };
    let prefixes: Result<Vec<String>, String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.prepare("SELECT address FROM mail_addresses WHERE status = 'active'")
            .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>())
            .map(|v| v.iter().map(|a| kept_prefix(a)).collect())
            .map_err(|e| e.to_string())
    };
    let prefixes = match prefixes {
        Ok(p) => p,
        Err(e) => {
            k2_core::log_debug!("{P} kept scripts not purged: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sieve") {
            continue;
        }
        let saved = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
        let expired = saved.is_some_and(|t| now - t >= REMOVED_KEEP_SECS);
        if expired || !prefixes.iter().any(|p| name.starts_with(p.as_str())) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                k2_core::log_debug!("{P} kept script not purged: {e}");
            }
        }
    }
}

/// The outbound targets about to go, by domain only.
fn strip_targets(current: &Current, plan: &StripPlan, is_here: &dyn Fn(&str) -> bool) -> Vec<super::sieve_lockdown::Target> {
    let hosted = super::sieve_lockdown::hosted_domains().unwrap_or_default();
    // A script K2 wrote loses its whole forward; someone's rules keep their
    // redirects to mailboxes here.
    let stays: &dyn Fn(&str) -> bool = if matches!(plan, StripPlan::Source(_)) { is_here } else { &|_| false };
    let mut out = Vec::new();
    for src in [&current.k2_src, &current.legacy_src].into_iter().flatten() {
        out.extend(super::sieve_lockdown::removed_targets(src, stays, &|d| hosted.contains(d)));
    }
    out
}

/// The audit line, before the write.
fn audit_strip(id: &str, address: &str, account: &str, targets: &[super::sieve_lockdown::Target], kept: &std::path::Path, running: bool) {
    let detail = json!({
        "accountId": account,
        "address": address,
        "script": sieve_user::SCRIPT_NAME,
        "action": "remove-outbound",
        "running": running,
        "targets": targets.iter().map(super::sieve_lockdown::Target::to_json).collect::<Vec<_>>(),
        "keptAt": kept.display().to_string(),
    });
    k2_core::log_debug!("{P} mailbox {id}: removing a forward nobody set: {} target(s)", targets.len());
    #[cfg(test)]
    TEST_AUDIT.with(|c| c.borrow_mut().push(detail));
    #[cfg(not(test))]
    {
        let ev = k2_core::auth_audit::AuditEvent::new(super::sieve_lockdown::AUDIT_EVENT, address, "removing", "daemon", "-", "daemon");
        if let Err(e) = k2_core::auth_audit::try_record_detail(&ev, detail) {
            k2_core::log_debug!("{P} WARN auth-audit write failed: {e}");
        }
    }
}

/// Mailboxes the owner has already heard K2 couldn't clean, this run.
static TOLD_LEFT: Mutex<std::collections::BTreeSet<String>> = Mutex::new(std::collections::BTreeSet::new());

/// The active mailboxes with no live row: anything in their scripts that sends
/// mail out has no row behind it. It goes, whatever wrote it, and the owner
/// hears. Everything else in the script stays ([`plan_strip`]). `skip`: the
/// import couldn't read them, so it isn't known what they hold yet.
fn strip_unbacked(io: &dyn SieveIo, ctx: &Ctx, skip: &std::collections::HashSet<String>, r: &mut Report) {
    let rows: Vec<(String, String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut st) = conn.prepare(
            "SELECT a.id, a.address, a.stalwart_account_id FROM mail_addresses a \
             WHERE a.status = 'active' AND a.stalwart_account_id IS NOT NULL AND a.stalwart_account_id <> '' \
             AND NOT EXISTS (SELECT 1 FROM mail_forwards f WHERE f.address_id = a.id AND f.state <> 'removed') ORDER BY a.address",
        ) else {
            r.failed += 1;
            return;
        };
        let rows = st.query_map([], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?)));
        match rows {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => {
                r.failed += 1;
                return;
            }
        }
    };
    let mut stripped: Vec<String> = Vec::new();
    let mut imported: Vec<String> = Vec::new();
    let mut left: Vec<String> = Vec::new();
    for (id, address, account) in rows {
        if skip.contains(&id) {
            continue;
        }
        let _g = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(current) = io.read(&account) else {
            r.failed += 1;
            continue;
        };
        // The list above is old by now: a forward set since then has its row,
        // and the redirect is that row's. Rows only change under this lock.
        match has_live_row(&id) {
            Ok(false) => {}
            Ok(true) => continue,
            Err(e) => {
                r.failed += 1;
                k2_core::log_debug!("{P} mailbox {id} not cleaned: {e}");
                continue;
            }
        }
        let plan = plan_strip(&current, &address, &is_minted);
        if matches!(plan, StripPlan::Nothing) {
            continue;
        }
        if let StripPlan::Import(fwd) = &plan {
            match import_one(io, &id, &address, fwd, ctx) {
                Ok(()) => {
                    r.imported += 1;
                    imported.push(format!("{address} to {}", fwd.dest));
                }
                Err(e) => {
                    r.failed += 1;
                    k2_core::log_debug!("{P} import of mailbox {id}: {}", crate::email_notify::send::mask_addresses(&e));
                }
            }
            continue;
        }
        // Before the write: never lose what we can't keep.
        let kept = match keep_old_scripts(&address, &current) {
            Ok(p) => p,
            Err(e) => {
                r.failed += 1;
                k2_core::log_debug!("{P} mailbox {id} not cleaned: the old script can't be kept: {}", crate::email_notify::send::mask_addresses(&e));
                if TOLD_LEFT.lock().unwrap_or_else(|p| p.into_inner()).insert(address.clone()) {
                    left.push(format!("{address}: its script is as it was, because K2 couldn't save a copy of it first."));
                }
                continue;
            }
        };
        let targets = strip_targets(&current, &plan, &is_minted);
        let running = outbound_is_running(&current);
        audit_strip(&id, &address, &account, &targets, &kept, running);
        // What's active stays what's active: cleaning a script that isn't
        // running mustn't start it, or an out-of-office kept in it.
        let activate = current.scripts.iter().any(|s| super::sieve_lockdown::is_k2_name(&s.name) && s.is_active);
        let wrote = match &plan {
            StripPlan::Compose(want) => io.write_source(&account, &current, &sieve_user::compose(want).unwrap_or_else(|| "keep;\n".to_string()), activate),
            StripPlan::Source(src) => {
                if super::sieve_lockdown::removed_targets(src, &is_minted, &|_| true).is_empty() {
                    io.write_source(&account, &current, src, activate)
                } else {
                    Err("something outbound would still be in the script".to_string())
                }
            }
            StripPlan::Nothing | StripPlan::Import(_) => Ok(()),
        };
        match wrote {
            Ok(()) => {
                k2_core::log_debug!("{P} mailbox {id}: removed a redirect with no forward row behind it");
                r.drift += 1;
                r.wrote += 1;
                TOLD_LEFT.lock().unwrap_or_else(|p| p.into_inner()).remove(&address);
                let mut domains: Vec<String> = targets.into_iter().map(|t| t.domain).collect();
                domains.sort();
                domains.dedup();
                let what = if running { "it forwarded mail to" } else { "that script wasn't running, so nothing went through it to" };
                stripped.push(format!("{address}: {what} {}. The old script is kept in {} on this server for 90 days.", domains.join(", "), kept.display()));
            }
            Err(e) => {
                r.failed += 1;
                k2_core::log_debug!("{P} mailbox {id} not cleaned: {}", crate::email_notify::send::mask_addresses(&e));
                if TOLD_LEFT.lock().unwrap_or_else(|p| p.into_inner()).insert(address.clone()) {
                    left.push(format!("{address}: its script is as it was, and a copy is in {} on this server.", kept.display()));
                }
            }
        }
    }
    if !stripped.is_empty() {
        super::forward_outcomes::notice(
            "K2 removed a forward nobody set",
            &format!(
                "These mail filters had a redirect with no forward on record behind it: nobody set it through K2, and the address was never told or counted. K2 removed only that; any other rules in the script are still there.\n\n{}\n\nThe removal is in the audit log (k2 users audit --json). Check who could change these mailboxes, and their recent activity.",
                stripped.join("\n")
            ),
            &[],
        );
    }
    if !left.is_empty() {
        super::forward_outcomes::notice(
            "K2 couldn't remove a forward nobody set",
            &format!(
                "These mail filters have a redirect with no forward on record behind it, and K2 couldn't take that out. K2 keeps trying each hour. Until it works, if the script is running, mail may still leave.\n\n{}",
                left.join("\n")
            ),
            &[],
        );
    }
    if !imported.is_empty() {
        super::forward_outcomes::notice(
            "K2 found a same-box forward with no record",
            &format!(
                "K2 found these in mail filters with no forward on record, so it recorded them and they keep running. If you didn't expect one, remove it: k2 hostmail forward unset <mailbox>.\n\n{}",
                imported.join("\n")
            ),
            &[],
        );
    }
}

fn has_live_row(address_id: &str) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM mail_forwards WHERE address_id = ?1 AND state <> 'removed')",
        rusqlite::params![address_id],
        |x| x.get(0),
    )
    .map_err(|e| format!("mail forwards: rows: {e}"))
}

/// A same-box forward K2 wrote before its rows: it becomes a row, and the row
/// writes the script. The caller holds [`SYNC`].
fn import_one(io: &dyn SieveIo, id: &str, address: &str, fwd: &ForwardBlock, ctx: &Ctx) -> Result<(), String> {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let f = store::create(
            &conn,
            &store::NewForward {
                address_id: id,
                address,
                dest: &fwd.dest.to_ascii_lowercase(),
                dest_kind: store::KIND_INTERNAL,
                keep: fwd.keep,
                set_by: ACTOR_K2,
                state: store::ST_ACTIVE,
                verified_address_id: None,
            },
            ctx.now,
        )
        .map_err(|e| e.to_string())?;
        let _ = store::record_event(&conn, &f.id, store::E_IMPORTED, ACTOR_K2, &json!({ "internal": true }), ctx.now);
    }
    // Write right away (the row is the truth now).
    sync_locked(io, id, Why::Change, ctx)?;
    Ok(())
}

/// FW-A5: same-box forwards K2 wrote before 0256 become rows. Anything else in
/// a mailbox with no row is [`strip_unbacked`]'s, later in the same pass.
/// `unread`: mailboxes this couldn't read or record. The import is owed until
/// there are none, and the strip leaves them alone.
fn import_with(io: &dyn SieveIo, skip: &[String], ctx: &Ctx, failed: &mut usize, unread: &mut std::collections::HashSet<String>) -> usize {
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
                unread.insert(id);
                continue;
            }
        };
        let StripPlan::Import(fwd) = plan_strip(&cur, &address, &is_minted) else { continue };
        match import_one(io, &id, &address, &fwd, ctx) {
            Ok(()) => n += 1,
            Err(e) => {
                *failed += 1;
                k2_core::log_debug!("{P} import of mailbox {id}: {}", crate::email_notify::send::mask_addresses(&e));
                unread.insert(id);
            }
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
    // Owed until a pass has read every mailbox: one it couldn't read may hold
    // a same-box forward from before the rows.
    let mut import_owed = true;
    let mut last_air = k2_core::airgap::enabled();
    loop {
        let status = super::supervisor::current_status();
        let gated = super::defaults::gate(status.as_deref(), super::supervisor::enable_completed());
        let busy = super::supervisor::enable_running().load(Ordering::SeqCst);
        if gated.is_none() && !busy {
            let ctx = ctx();
            match with_io(|io| Ok(reconcile_with(io, import_owed, &ctx))) {
                Ok(r) => {
                    if first || r.wrote + r.held + r.failed + r.imported > 0 {
                        k2_core::log_debug!("{P} pass: {r:?}");
                    }
                    first = false;
                    import_owed = r.import_failed > 0;
                    // Delivery outcomes not arriving means the auto-pauses can't
                    // fire: try again each hour, not only at the next start.
                    let broken = {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        store::webhook_error(&conn).is_some()
                    };
                    if broken {
                        crate::email_notify::webhook::spawn_after("retry");
                    }
                }
                Err(e) => k2_core::log_debug!("{P} pass skipped: {}", crate::email_notify::send::mask_addresses(&e)),
            }
            // The owner's hourly number, if writing it right away failed or
            // something changed it since. Reads, and writes only a difference.
            match super::supervisor::mgmt_client_from_row() {
                Ok(client) => {
                    for line in super::defaults::reconcile_configured_throttles(&client).lines {
                        k2_core::log_debug!("{line} (hourly)");
                    }
                }
                Err(e) => k2_core::log_debug!("{P} throttles skipped: {e}"),
            }
        }
        // Hourly, sooner when nudged or air-gap flips (checked each minute),
        // and every 10 minutes while the import is owed.
        for minute in 0..60 {
            std::thread::sleep(Duration::from_secs(if first { 10 } else { 60 }));
            // Each minute: rows from before opt-out turning on, paced, and
            // failed notice emails due another try.
            let ready = super::defaults::gate(super::supervisor::current_status().as_deref(), super::supervisor::enable_completed()).is_none()
                && !super::supervisor::enable_running().load(Ordering::SeqCst);
            if ready && !first {
                super::forward_verify::minute_pass(now());
            }
            let air = k2_core::airgap::enabled();
            if NUDGE.swap(false, Ordering::SeqCst) || air != last_air || first || (import_owed && minute >= 9) {
                last_air = air;
                break;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
