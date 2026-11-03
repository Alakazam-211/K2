//! Rows → Sieve against a fake mail server (no Stalwart, no mail).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use k2_core::mail_forward as store;
use rusqlite::params;

use super::*;
use crate::mail::jmap::SieveScriptInfo;
use crate::mail::sieve_user::{self, OooBlock, UserSieve};

/// One mailbox's `k2` script per account; counts writes.
#[derive(Default)]
pub(crate) struct FakeSieve {
    pub scripts: RefCell<HashMap<String, String>>,
    /// A leftover `k2-forward` script, per account.
    pub legacy: RefCell<HashMap<String, String>>,
    /// Refuse a source K2 didn't compose (Stalwart wouldn't compile it).
    pub source_fail: RefCell<bool>,
    /// Accounts whose K2 scripts aren't the active one.
    pub inactive: RefCell<std::collections::HashSet<String>>,
    pub writes: RefCell<usize>,
    pub fail: RefCell<bool>,
    /// How many times K2 raised the redirect limit, and whether it can.
    pub limit_calls: RefCell<usize>,
    pub limit_fail: RefCell<bool>,
}

impl FakeSieve {
    pub(crate) fn src(&self, account: &str) -> Option<String> {
        self.scripts.borrow().get(account).cloned()
    }
    pub(crate) fn forward(&self, account: &str) -> Option<sieve_user::ForwardBlock> {
        self.src(account).and_then(|s| sieve_user::parse(&s).forward)
    }
    pub(crate) fn put(&self, account: &str, src: &str) {
        self.scripts.borrow_mut().insert(account.to_string(), src.to_string());
    }
}

impl SieveIo for FakeSieve {
    fn read(&self, account_id: &str) -> Result<Current, String> {
        let src = self.src(account_id);
        let is_active = !self.inactive.borrow().contains(account_id);
        let mut scripts = src
            .as_ref()
            .map(|_| vec![SieveScriptInfo { id: "s1".into(), name: "k2".into(), blob_id: "b1".into(), is_active }])
            .unwrap_or_default();
        if self.legacy.borrow().contains_key(account_id) {
            scripts.push(SieveScriptInfo { id: "s2".into(), name: "k2-forward".into(), blob_id: "b2".into(), is_active: is_active && src.is_none() });
        }
        let legacy_src = self.legacy.borrow().get(account_id).cloned();
        let mut sieve = src.as_deref().map(sieve_user::parse).unwrap_or_default();
        if sieve.forward.is_none() {
            sieve.forward = legacy_src.as_deref().and_then(sieve_user::parse_legacy_forward);
        }
        Ok(Current { sieve, scripts, k2_src: src, legacy: legacy_src.is_some(), legacy_src })
    }
    fn write(&self, account_id: &str, _current: &Current, next: &UserSieve) -> Result<(), String> {
        if *self.fail.borrow() {
            return Err("fake: the mail server is down".into());
        }
        *self.writes.borrow_mut() += 1;
        let src = sieve_user::compose(next).unwrap_or_else(|| "keep;\n".to_string());
        self.put(account_id, &src);
        self.legacy.borrow_mut().remove(account_id);
        self.inactive.borrow_mut().remove(account_id);
        Ok(())
    }
    fn write_source(&self, account_id: &str, _current: &Current, source: &str, activate: bool) -> Result<(), String> {
        if *self.fail.borrow() || *self.source_fail.borrow() {
            return Err("fake: the mail server refused the script".into());
        }
        if activate {
            self.inactive.borrow_mut().remove(account_id);
        }
        *self.writes.borrow_mut() += 1;
        self.put(account_id, source);
        self.legacy.borrow_mut().remove(account_id);
        Ok(())
    }
    fn ensure_redirects(&self) -> Result<(), String> {
        if *self.limit_fail.borrow() {
            return Err("fake: the redirect limit can't be raised".into());
        }
        *self.limit_calls.borrow_mut() += 1;
        Ok(())
    }
}

pub(crate) const HOST: &str = "mail.brand.test";

/// Install a fresh fake Sieve and context for this thread.
pub(crate) fn install(airgap: bool) -> Rc<FakeSieve> {
    let io = Rc::new(FakeSieve::default());
    set_test_io(Some(io.clone()));
    set_test_ctx(Some(Ctx { host: Some(HOST.to_string()), airgap, now: 0 }));
    io
}

pub(crate) fn uninstall() {
    set_test_io(None);
    set_test_ctx(None);
    set_test_version(None);
}

/// A mailbox on a fresh domain (`send_mode`), account `acc-<local>`.
pub(crate) fn seed_mailbox(addr: &str, send_mode: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let domain = addr.rsplit_once('@').map(|(_, d)| d).expect("domain");
    let did = format!("d-{domain}");
    conn.execute(
        "INSERT OR IGNORE INTO mail_domains (id, domain, stalwart_domain_id, send_mode, status, created_at) VALUES (?1, ?2, 'sw', ?3, 'verified', 1)",
        params![did, domain, send_mode],
    )
    .expect("domain");
    let id = format!("ma-{addr}");
    let local = addr.split('@').next().expect("local");
    conn.execute(
        "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, 'ws-fw', 'active', 1)",
        params![id, addr, did, format!("acc-{local}")],
    )
    .expect("mailbox");
    id
}

fn now() -> i64 {
    super::now()
}

fn create(aid: &str, addr: &str, dest: &str, kind: &str, state: &str, verified: Option<&str>) -> Forward {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::create(
        &conn,
        &store::NewForward {
            address_id: aid,
            address: addr,
            dest,
            dest_kind: kind,
            keep: true,
            set_by: "owner",
            state,
            verified_address_id: verified,
        },
        now(),
    )
    .expect("create")
}

/// A `forward` verified-address row for (mailbox, dest), in `state`.
pub(crate) fn verified_row(mailbox: &str, dest: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = format!("eva_{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO email_verified_addresses (id, address, purpose, subject_ref, requested_by, state, created_at, expires_at, verified_at) \
         VALUES (?1, ?2, 'forward', ?3, 'owner', 'verified', 1, 2, 1)",
        params![id, dest, mailbox],
    )
    .expect("verified");
    id
}

struct Guard(#[allow(dead_code)] k2_core::db::TestDbGuard);
impl Drop for Guard {
    fn drop(&mut self) {
        uninstall();
    }
}

fn setup() -> Guard {
    Guard(k2_core::db::scoped_for_test())
}

#[test]
fn pending_and_paused_write_no_forward_active_writes_the_guarded_block() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("sales@brand.test", "direct");
    let f = create(&aid, "sales@brand.test", "me@outside.test", store::KIND_EXTERNAL, store::ST_PENDING_VERIFY, None);
    sync_mailbox("ma-sales@brand.test", Why::Change).expect("sync");
    assert_eq!(io.forward("acc-sales"), None, "pending_verify forwards nothing");
    let v = verified_row("sales@brand.test", "me@outside.test");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::transition(&conn, &f.id, store::ST_ACTIVE, &store::Change { verified_address_id: Some(&v), actor: "recipient", ..Default::default() }, now()).expect("active");
    }
    sync_mailbox(&aid, Why::Change).expect("sync");
    let fwd = io.forward("acc-sales").expect("forward written");
    assert_eq!((fwd.dest.as_str(), fwd.keep), ("me@outside.test", true));
    assert_eq!(fwd.guard.as_ref().map(|g| g.host.as_str()), Some(HOST), "outside = guarded");
    pause(&f.id, store::P_MANUAL, "owner", None).expect("pause");
    assert_eq!(io.forward("acc-sales"), None, "paused = keep-only");
    resume(&f.id, "owner").expect("resume");
    assert!(io.forward("acc-sales").is_some());
}

#[test]
fn the_ooo_block_survives_every_forward_change() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("ooo@brand.test", "direct");
    let ooo = OooBlock { days: 7, text: "Away.".into(), until: None, from: "ooo@brand.test".into() };
    io.put("acc-ooo", &sieve_user::compose(&UserSieve { ooo: Some(ooo.clone()), forward: None }).expect("ooo"));
    let f = create(&aid, "ooo@brand.test", "team@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None);
    sync_mailbox(&aid, Why::Change).expect("sync");
    let s = sieve_user::parse(&io.src("acc-ooo").expect("script"));
    assert_eq!(s.ooo, Some(ooo.clone()));
    assert_eq!(s.forward.as_ref().map(|f| (f.dest.as_str(), f.guard.is_none())), Some(("team@brand.test", true)));
    transition_and_sync(&f.id, store::ST_REMOVED, &store::Change { actor: "owner", ..Default::default() }).expect("remove");
    let s = sieve_user::parse(&io.src("acc-ooo").expect("script"));
    assert_eq!((s.ooo, s.forward), (Some(ooo), None));
}

#[test]
fn reconcile_restores_a_tampered_script_and_logs_drift() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("drift@brand.test", "direct");
    let v = verified_row("drift@brand.test", "me@outside.test");
    create(&aid, "drift@brand.test", "me@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&v));
    sync_mailbox(&aid, Why::Change).expect("sync");
    let good = io.src("acc-drift").expect("script");
    // Someone rewrote it (a bare redirect elsewhere, no guard).
    io.put("acc-drift", "redirect \"thief@elsewhere.test\";\n");
    let r = reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!((r.drift, r.failed), (1, 0), "{r:?}");
    assert_eq!(io.src("acc-drift").as_deref(), Some(good.as_str()), "restored byte for byte");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let ev = store::events_for_address(&conn, &aid, 10).expect("events");
    assert_eq!(ev[0].event, store::E_DRIFT);
    drop(conn);
    // A second pass finds nothing.
    let writes = *io.writes.borrow();
    let r = reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!((r.drift, *io.writes.borrow()), (0, writes), "{r:?}");
}

#[test]
fn air_gap_and_receive_only_pause_outside_forwards_and_lift_when_gone() {
    let _g = setup();
    let io = install(true);
    let aid = seed_mailbox("air@brand.test", "direct");
    let v = verified_row("air@brand.test", "me@outside.test");
    let f = create(&aid, "air@brand.test", "me@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&v));
    let internal_aid = seed_mailbox("in@brand.test", "direct");
    create(&internal_aid, "in@brand.test", "air@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None);
    let r = reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(r.failed, 0, "{r:?}");
    let get = |id: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get(&conn, id).expect("get").expect("row")
    };
    let row = get(&f.id);
    assert_eq!((row.state.as_str(), row.pause_reason.as_deref()), (store::ST_PAUSED, Some(store::P_AIR_GAP)));
    assert_eq!(io.forward("acc-air"), None, "nothing leaves under air-gap");
    assert!(io.forward("acc-in").is_some(), "a same-box forward is not held");
    // Air-gap off → it resumes by itself.
    set_test_ctx(Some(Ctx { host: Some(HOST.into()), airgap: false, now: 0 }));
    reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(get(&f.id).state, store::ST_ACTIVE);
    assert!(io.forward("acc-air").is_some());
    // The domain goes receive-only → paused for that.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_domains SET send_mode = 'receive-only' WHERE domain = 'brand.test'", []).expect("mode");
    }
    reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(get(&f.id).pause_reason.as_deref(), Some(store::P_RECEIVE_ONLY));
    // A person's pause is never lifted by K2.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_domains SET send_mode = 'direct' WHERE domain = 'brand.test'", []).expect("mode");
        store::transition(&conn, &f.id, store::ST_PAUSED, &store::Change { reason: Some(store::P_MANUAL), actor: "owner", ..Default::default() }, now()).expect("manual");
    }
    reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(get(&f.id).pause_reason.as_deref(), Some(store::P_MANUAL));
}

#[test]
fn a_stopped_destination_pauses_for_good_and_no_host_fails_closed() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("stop@brand.test", "direct");
    let v = verified_row("stop@brand.test", "me@outside.test");
    let f = create(&aid, "stop@brand.test", "me@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&v));
    // No valid mail host name: the block is left out.
    set_test_ctx(Some(Ctx { host: None, airgap: false, now: 0 }));
    let s = sync_mailbox(&aid, Why::Change).expect("sync");
    assert!(s.blocked);
    assert_eq!(io.forward("acc-stop"), None, "fail closed without a host name");
    set_test_ctx(Some(Ctx { host: Some(HOST.into()), airgap: false, now: 0 }));
    sync_mailbox(&aid, Why::Change).expect("sync");
    assert!(io.forward("acc-stop").is_some());
    // The verified row is revoked behind K2's back: the next pass pauses it.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::email_verify::revoke(&conn, &v, "recipient", now()).expect("revoke");
    }
    reconcile_with(io.as_ref(), false, &ctx());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = store::get(&conn, &f.id).expect("get").expect("row");
    assert_eq!(row.pause_reason.as_deref(), Some(store::P_REVOKED));
    drop(conn);
    assert_eq!(io.forward("acc-stop"), None);
}

#[test]
fn import_keeps_same_box_forwards_and_strips_unconfirmed_outside_ones() {
    let _g = setup();
    let io = install(false);
    let a = seed_mailbox("old@brand.test", "direct");
    let b = seed_mailbox("team@brand.test", "direct");
    let c = seed_mailbox("leak@brand.test", "direct");
    io.put("acc-old", &sieve_user::compose(&UserSieve { ooo: None, forward: Some(sieve_user::ForwardBlock::internal("team@brand.test", true)) }).expect("s"));
    io.put("acc-leak", "require [\"copy\"];\n# k2-forward\nredirect :copy \"x@outside.test\";\n");
    let r = reconcile_with(io.as_ref(), true, &ctx());
    assert_eq!((r.imported, r.failed, r.import_failed), (1, 0, 0), "{r:?}");
    assert_eq!(r.drift, 1, "the strip took the outside one: {r:?}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let kept = store::live_for_address(&conn, &a).expect("live").expect("row");
    assert_eq!((kept.dest.as_str(), kept.dest_kind.as_str(), kept.state.as_str()), ("team@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE));
    assert!(store::live_for_address(&conn, &c).expect("live").is_none(), "an unconfirmed outside forward is not kept");
    assert!(store::live_for_address(&conn, &b).expect("live").is_none());
    drop(conn);
    assert_eq!(io.forward("acc-leak"), None, "and its block is gone");
    assert!(io.forward("acc-old").is_some());
}

#[test]
fn a_failed_sieve_write_keeps_the_row_change_and_says_so() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("down@brand.test", "direct");
    let f = create(&aid, "down@brand.test", "team@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None);
    sync_mailbox(&aid, Why::Change).expect("first write");
    assert!(io.forward("acc-down").is_some());
    *io.fail.borrow_mut() = true;
    match transition_and_sync(&f.id, store::ST_REMOVED, &store::Change { actor: "owner", ..Default::default() }) {
        Err(ChangeError::Sieve(row, _)) => assert_eq!(row.state, store::ST_REMOVED),
        other => panic!("expected a deferred write, got {other:?}"),
    }
    assert!(io.forward("acc-down").is_some(), "the write failed");
    *io.fail.borrow_mut() = false;
    // The removed row's mailbox is still on the reconcile's list.
    reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(io.forward("acc-down"), None);
}

/// A mail server older than 0.16.25 (or unknown) never gets an outside
/// forward block, even for an active row (fail closed); same-box blocks are
/// written as before.
#[test]
fn an_older_mail_server_gets_no_outside_block() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("old@brand.test", "direct");
    let v = verified_row("old@brand.test", "me@outside.test");
    create(&aid, "old@brand.test", "me@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&v));
    let inside = seed_mailbox("in@brand.test", "direct");
    create(&inside, "in@brand.test", "old@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None);
    let row = || {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::live_for_address(&conn, &aid).expect("live").expect("row")
    };
    for ver in [Some("0.16.10".to_string()), None] {
        set_test_version(Some(ver.clone()));
        let s = sync_mailbox(&aid, Why::Change).expect("sync");
        assert_eq!(s.held.as_deref(), if ver.is_some() { Some(store::P_UPGRADE) } else { None }, "{ver:?}: {s:?}");
        assert_eq!(io.forward("acc-old"), None, "{ver:?}");
        // Visible: the row is paused for the upgrade, not "active".
        let r = row();
        assert_eq!((r.state.as_str(), r.pause_reason.as_deref()), (store::ST_PAUSED, Some(store::P_UPGRADE)), "{ver:?}");
        sync_mailbox(&inside, Why::Change).expect("sync");
        assert!(io.forward("acc-in").is_some(), "same-box on {ver:?}");
    }
    // A person can't resume it by hand; `k2 hostmail upgrade` lifts it.
    set_test_version(Some(Some("0.16.25-k2.1".to_string())));
    let s = sync_mailbox(&aid, Why::Change).expect("sync");
    assert_eq!(s.held.as_deref(), Some(""), "lifted: {s:?}");
    assert_eq!(row().state, store::ST_ACTIVE);
    assert!(io.forward("acc-old").is_some_and(|f| f.guard.is_some()));
}

// ── several destinations ────────────────────────────────────────────

fn add_row(aid: &str, addr: &str, dest: &str, kind: &str, state: &str, verified: Option<&str>, keep: bool) -> Forward {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::add(
        &conn,
        &store::NewForward { address_id: aid, address: addr, dest, dest_kind: kind, keep, set_by: "owner", state, verified_address_id: verified },
        now(),
    )
    .expect("add")
}

#[test]
fn stalwart_s_default_limit_is_raised_only_when_low_and_never_lowered() {
    assert_eq!(REDIRECT_LIMIT, store::MAX_DESTINATIONS as u64);
    assert_eq!(OUT_MESSAGE_LIMIT, REDIRECT_LIMIT + 1, "a vacation reply counts as an out message too");
    // Absent: Stalwart's user-script defaults (1 and 3).
    assert_eq!(plan_redirect_limits(None, None), Some((REDIRECT_LIMIT, OUT_MESSAGE_LIMIT)));
    assert_eq!(plan_redirect_limits(Some(1), Some(3)), Some((REDIRECT_LIMIT, OUT_MESSAGE_LIMIT)));
    assert_eq!(plan_redirect_limits(Some(REDIRECT_LIMIT), Some(OUT_MESSAGE_LIMIT)), None, "already high enough: no write, no reload");
    assert_eq!(plan_redirect_limits(Some(50), Some(2)), Some((50, OUT_MESSAGE_LIMIT)), "an operator's higher value stays");
    assert_eq!(plan_redirect_limits(Some(50), Some(60)), None);
}

#[test]
fn each_active_destination_gets_a_redirect_and_a_waiting_or_paused_one_gets_none() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("many@brand.test", "direct");
    let (a, b) = (verified_row("many@brand.test", "a@outside.test"), verified_row("many@brand.test", "b@outside.test"));
    add_row(&aid, "many@brand.test", "a@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&a), true);
    let fb = add_row(&aid, "many@brand.test", "b@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&b), true);
    add_row(&aid, "many@brand.test", "waiting@outside.test", store::KIND_EXTERNAL, store::ST_PENDING_VERIFY, None, true);
    add_row(&aid, "many@brand.test", "team@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, true);
    sync_mailbox(&aid, Why::Change).expect("sync");
    let fwd = io.forward("acc-many").expect("written");
    assert_eq!(fwd.dest, "a@outside.test");
    assert_eq!(fwd.more, vec!["b@outside.test".to_string()]);
    assert_eq!(fwd.same_box, vec!["team@brand.test".to_string()]);
    assert!(fwd.guard.is_some());
    let src = io.src("acc-many").expect("script");
    assert!(!src.contains("waiting@outside.test"), "an address that isn't on gets nothing: {src}");
    assert_eq!(*io.limit_calls.borrow(), 1, "several redirects: the limit is checked first");
    // Pausing one leaves the others written.
    pause(&fb.id, store::P_DEST_GONE, "k2", None).expect("pause");
    let fwd = io.forward("acc-many").expect("written");
    assert_eq!((fwd.dest.as_str(), fwd.more.len(), fwd.same_box.len()), ("a@outside.test", 0, 1));
    // A rewrite is byte-stable: the reconcile finds nothing.
    let writes = *io.writes.borrow();
    let r = reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!((r.drift, *io.writes.borrow()), (0, writes), "{r:?}");
}

/// Stalwart silently skips a redirect over its limit, so K2 never writes a
/// script it can't run: the old script stands and the change reads deferred.
#[test]
fn when_the_limit_can_t_be_raised_the_old_script_stands() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("lim@brand.test", "direct");
    let a = verified_row("lim@brand.test", "a@outside.test");
    add_row(&aid, "lim@brand.test", "a@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&a), true);
    sync_mailbox(&aid, Why::Change).expect("one destination needs no limit");
    assert_eq!(*io.limit_calls.borrow(), 0, "one redirect never touches the server's limit");
    let good = io.src("acc-lim").expect("script");
    *io.limit_fail.borrow_mut() = true;
    let b = verified_row("lim@brand.test", "b@outside.test");
    add_row(&aid, "lim@brand.test", "b@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&b), true);
    let e = sync_mailbox(&aid, Why::Change).expect_err("deferred");
    assert!(e.contains("redirect limit"), "{e}");
    assert_eq!(io.src("acc-lim").as_deref(), Some(good.as_str()), "untouched: the first destination keeps forwarding");
    *io.limit_fail.borrow_mut() = false;
    reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(io.forward("acc-lim").expect("written").more, vec!["b@outside.test".to_string()], "the reconcile finishes it");
}

#[test]
fn a_copy_stays_if_any_active_row_keeps_one() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("k@brand.test", "direct");
    add_row(&aid, "k@brand.test", "t1@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, false);
    add_row(&aid, "k@brand.test", "t2@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, false);
    sync_mailbox(&aid, Why::Change).expect("sync");
    assert!(!io.forward("acc-k").expect("written").keep);
    add_row(&aid, "k@brand.test", "t3@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, true);
    sync_mailbox(&aid, Why::Change).expect("sync");
    let src = io.src("acc-k").expect("script");
    assert_eq!(src.matches("redirect :copy").count(), 3, "all copies or none: {src}");
    assert!(!src.contains("redirect \""), "{src}");
}

/// No mail host name, or a mail server too old: the outside rows aren't
/// written, the same-box ones still are, and the sync says so.
#[test]
fn outside_rows_fail_closed_without_touching_same_box_ones() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("mix@brand.test", "direct");
    let a = verified_row("mix@brand.test", "a@outside.test");
    add_row(&aid, "mix@brand.test", "a@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&a), true);
    add_row(&aid, "mix@brand.test", "team@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, true);
    set_test_ctx(Some(Ctx { host: None, airgap: false, now: 0 }));
    let s = sync_mailbox(&aid, Why::Change).expect("sync");
    assert!(s.blocked, "{s:?}");
    let fwd = io.forward("acc-mix").expect("the same-box one");
    assert_eq!((fwd.dest.as_str(), fwd.guard.is_none(), fwd.more.len()), ("team@brand.test", true, 0));
    assert!(!io.src("acc-mix").expect("script").contains("a@outside.test"));
}

#[test]
fn a_retired_mailbox_drops_all_its_rows() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("gone@brand.test", "direct");
    add_row(&aid, "gone@brand.test", "t1@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, true);
    add_row(&aid, "gone@brand.test", "t2@brand.test", store::KIND_INTERNAL, store::ST_ACTIVE, None, true);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE mail_addresses SET status = 'retired' WHERE id = ?1", params![aid]).expect("retire");
    }
    sync_mailbox(&aid, Why::Change).expect("sync");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(store::live_rows(&conn, &aid).expect("live").is_empty());
    drop(conn);
    drop(io);
}

/// Review finding 8: a Stop or removal never waits on the limit: fewer redirects
/// always go, and a failed limit call only blocks growing.
#[test]
fn fewer_redirects_always_go_even_when_the_limit_call_fails() {
    let _g = setup();
    let io = install(false);
    let aid = seed_mailbox("few@brand.test", "direct");
    let mut ids = Vec::new();
    for d in ["a@outside.test", "b@outside.test", "c@outside.test"] {
        let v = verified_row("few@brand.test", d);
        ids.push(add_row(&aid, "few@brand.test", d, store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&v), true).id);
    }
    sync_mailbox(&aid, Why::Change).expect("sync");
    assert_eq!(*io.limit_calls.borrow(), 1);
    *io.limit_fail.borrow_mut() = true;
    pause(&ids[2], store::P_REVOKED, "recipient", None).expect("the removal is written");
    let src = io.src("acc-few").expect("script");
    assert!(!src.contains("c@outside.test"), "{src}");
    assert!(src.contains("b@outside.test"), "{src}");
    // Same count, nothing to do: no call needed (a periodic pass tries, and shrugs).
    assert_eq!(*io.limit_calls.borrow(), 1);
    let r = reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!(r.failed, 0, "{r:?}");
    // Growing still waits for the limit.
    resume(&ids[2], "owner").expect_err("deferred");
    assert!(!io.src("acc-few").expect("script").contains("c@outside.test"));
}

/// Review finding 10: with a system sender, the guard keeps its mail home.
#[test]
fn a_system_sender_s_mail_stays_home() {
    let _g = setup();
    let io = install(false);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO email_senders (id, address, domain, path, owner_kind, from_name, state, created_at, updated_at) \
             VALUES ('s1', 'K2@Brand.Test', 'brand.test', 'hosted', 'system', 'K2', 'ready', 1, 1)",
            [],
        )
        .expect("sender");
    }
    let aid = seed_mailbox("h@brand.test", "direct");
    let v = verified_row("h@brand.test", "a@outside.test");
    add_row(&aid, "h@brand.test", "a@outside.test", store::KIND_EXTERNAL, store::ST_ACTIVE, Some(&v), true);
    sync_mailbox(&aid, Why::Change).expect("sync");
    let g = io.forward("acc-h").expect("written").guard.expect("guard");
    assert_eq!(g.stay_home, vec!["k2@brand.test".to_string()]);
    // A rewrite is stable.
    let writes = *io.writes.borrow();
    let r = reconcile_with(io.as_ref(), false, &ctx());
    assert_eq!((r.drift, *io.writes.borrow()), (0, writes), "{r:?}");
}
