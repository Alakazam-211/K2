//! Voice rooms S6 (PRD v1.9 §7): Sew set up on every Linux box behind the
//! preview, the Pro start, the small-box floor, opt-outs, and that nothing
//! happens with the preview off. Machines run inline over the fake box.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use super::*;
use crate::sew::ops::fake::{FakeOps, GIB};
use crate::sew::ops::SewOps;
use crate::sew::secrets::{MemSecrets, SewSecrets};
use crate::sew::supervisor::{self as sup, Env};

const DAY: i64 = 86_400;

struct T {
    env: Env<'static>,
    ops: &'static FakeOps,
    secrets: &'static MemSecrets,
    _db: k2_core::db::TestDbGuard,
}

/// A prepared Linux box: the pinned Sew binary already staged on disk
/// (the release download is `fetch`'s own tests), preview on.
fn t() -> T {
    let ops: &'static FakeOps = Box::leak(Box::new(FakeOps::new()));
    *ops.on_disk.lock().unwrap() = Some(k2_root_proto::sew::pinned().version.into());
    *ops.preview.lock().unwrap() = true;
    let secrets: &'static MemSecrets = Box::leak(Box::new(MemSecrets::default()));
    let env = Env {
        ops,
        secrets,
        latch: Box::leak(Box::new(AtomicBool::new(false))),
        token: Box::leak(Box::new(Mutex::new(None))),
    };
    T { env, ops, secrets, _db: k2_core::db::scoped_for_test() }
}

impl T {
    fn now(&self) -> i64 {
        *self.ops.clock.lock().unwrap()
    }
    fn rec(&self) -> serde_json::Value {
        self.ops.records.lock().unwrap().get(RECORD).cloned().unwrap_or(serde_json::Value::Null)
    }
    fn argvs(&self) -> Vec<String> {
        self.ops.calls.lock().unwrap().iter().map(|c| c.argv.clone()).collect()
    }
}

#[test]
fn the_floor_table() {
    let ok = floor(Some(64 * GIB), Some(500 * GIB));
    assert_eq!((ok.set_up, ok.chat, ok.voice, ok.code, ok.line.clone()), (true, true, true, "ok", None));
    let disk = floor(Some(64 * GIB), Some(400_000_000));
    assert_eq!((disk.set_up, disk.chat, disk.voice, disk.code), (false, false, false, "disk"));
    assert_eq!(disk.line.as_deref(), Some("Voice rooms need 1 GiB of free disk; this box has 400 MB free."));
    let tiny = floor(Some(GIB), Some(500 * GIB));
    assert_eq!((tiny.set_up, tiny.chat, tiny.voice, tiny.code), (true, false, false, "ram_chat"));
    assert_eq!(tiny.line.as_deref(), Some("Voice rooms need 2 GiB of memory; this box has 1 GiB. They start when it has enough."));
    let small = floor(Some(3 * GIB), Some(500 * GIB));
    assert_eq!((small.set_up, small.chat, small.voice, small.code), (true, true, false, "ram_voice"));
    assert_eq!(small.line.as_deref(), Some("Voice needs 4 GiB of memory; this box has 3 GiB."));
    let edge = floor(Some(4 * GIB), Some(GIB));
    assert_eq!((edge.set_up, edge.chat, edge.voice), (true, true, true), "the floors are inclusive");
    let half = floor(Some(3 * GIB + GIB / 2), Some(500 * GIB));
    assert_eq!(half.line.as_deref(), Some("Voice needs 4 GiB of memory; this box has 3.5 GiB."));
    let unknown = floor(None, Some(500 * GIB));
    assert_eq!((unknown.set_up, unknown.code), (false, "unreadable"));
}

/// The 0.46.8 promise: with the preview off nothing changes on any box:
/// no set-up, no start (even on Pro), no record, no status field, no
/// helper call.
#[test]
fn nothing_happens_with_the_preview_off() {
    let t = t();
    *t.ops.preview.lock().unwrap() = false;
    t.ops.set_tier(Some("pro"));
    for boot in [true, false] {
        assert_eq!(due(&t.env, t.now(), boot), None);
    }
    assert!(t.ops.records.lock().unwrap().get(RECORD).is_none());
    assert!(status(&t.env).is_none());
    let s = sup::status(&t.env, false);
    assert!(s.get("autoSetup").is_none(), "{s}");
    assert_eq!(s["state"], "not-installed");
    assert!(t.argvs().iter().all(|a| a == "engine sew status"), "{:?}", t.argvs());
    assert!(store::load().is_none());
}

/// Set up (not started) on a Linux box, "Set up · waiting for Pro", then
/// started by the tier watch on the first `pro`. Below Pro it never starts.
#[test]
fn set_up_waits_for_pro_then_starts() {
    let t = t();
    let now = t.now();
    assert_eq!(due(&t.env, now, true), Some(Action::SetUp));
    let out = run(&t.env, Action::SetUp);
    assert_eq!(out["state"], "set_up", "{out}");
    let argv = t.argvs();
    assert!(argv.iter().any(|a| a == "engine sew install"), "{argv:?}");
    for never in ["engine sew systemctl enable", "engine sew systemctl start-db", "engine sew systemctl migrate", "engine sew systemctl start"] {
        assert!(!argv.iter().any(|a| a == never), "set-up started something ({never}): {argv:?}");
    }
    assert!(!t.ops.unit(k2_root_proto::sew::UNIT_DB).is_active() && !t.ops.unit(k2_root_proto::sew::UNIT_SEW).is_active());
    let row = store::load().expect("row");
    assert_eq!(row.status, "disabled");
    assert!(waiting_for_pro(&row), "{:?}", row.enable_progress);
    // The master key and the owner's launch key are in the vault.
    let master = t.secrets.resolve(row.master_key_ref.as_deref().expect("master ref")).unwrap().expect("master key");
    assert_eq!(t.secrets.recovery().unwrap().as_deref(), Some(master.as_str()), "sew-launch-key = the master key");
    let s = sup::status(&t.env, false);
    assert_eq!(s["state"], "disabled", "{s}");
    assert_eq!(s["autoSetup"]["state"], "set_up", "{s}");
    assert_eq!(s["autoSetup"]["line"], "Set up · waiting for Pro");
    assert_eq!(s["autoSetup"]["tier"]["state"], "not_pro");

    // Below Pro: never started, whatever the tick.
    t.ops.calls.lock().unwrap().clear();
    for tier in [None, Some("free"), Some("single")] {
        t.ops.set_tier(tier);
        assert_eq!(due(&t.env, now + 60, false), None, "{tier:?}");
        assert_eq!(due(&t.env, now + 60, true), None, "{tier:?}");
    }
    assert!(t.argvs().iter().all(|a| a == "engine sew status"), "{:?}", t.argvs());

    // The first `pro`: a normal enable resumes after install and starts it.
    t.ops.set_tier(Some("pro"));
    assert_eq!(due(&t.env, now + 120, false), Some(Action::Start));
    let out = run(&t.env, Action::Start);
    assert_eq!(out["state"], "started", "{out}");
    let argv = t.argvs();
    assert!(!argv.iter().any(|a| a == "engine sew install" || a == "engine sew stage"), "resumed, not reinstalled: {argv:?}");
    for want in ["engine sew systemctl enable", "engine sew systemctl start-db", "engine sew systemctl start"] {
        assert!(argv.iter().any(|a| a == want), "{want}: {argv:?}");
    }
    let row = store::load().unwrap();
    assert_eq!(row.status, "running");
    assert!(!waiting_for_pro(&row));
    let p = row.enable_progress.unwrap();
    assert_eq!((p["auto"].as_bool(), p["setupOnly"].as_bool(), p["result"].as_str()), (Some(true), Some(false), Some("done")));
    let s = sup::status(&t.env, false);
    assert_eq!(s["autoSetup"]["state"], "started", "{s}");
    assert_eq!(s["autoSetup"]["line"], STARTED_LINE);
    assert_eq!(s["autoSetup"]["tier"]["state"], "pro");
    // Running now: the tick has nothing left to do, and losing Pro stops
    // nothing here (Sew stays set up; rooms close in lane B).
    assert_eq!(due(&t.env, now + 180, true), None);
    t.ops.set_tier(Some("free"));
    assert_eq!(due(&t.env, now + 240, true), None);
    assert_eq!(store::load().unwrap().status, "running");
}

/// §7.3: under 1 GiB free disk, not set up (re-checked daily and at
/// boot); under 2 GiB of RAM, set up but never started, even on Pro;
/// 2–4 GiB, started, and status says voice needs 4 GiB.
#[test]
fn small_boxes_follow_the_floor() {
    let t = t();
    let now = t.now();
    *t.ops.disk.lock().unwrap() = Some(400_000_000);
    assert_eq!(due(&t.env, now, true), None);
    assert_eq!(t.rec()["state"], "below_floor");
    assert_eq!(t.rec()["line"], "Voice rooms need 1 GiB of free disk; this box has 400 MB free.");
    assert_eq!(t.rec()["nextTryAt"], now + FLOOR_RECHECK_SECS);
    assert!(store::load().is_none());
    let s = sup::status(&t.env, false);
    assert_eq!(s["autoSetup"]["line"], "Voice rooms need 1 GiB of free disk; this box has 400 MB free.", "{s}");
    // Disk freed: the next daily check (or a boot) sets it up.
    *t.ops.disk.lock().unwrap() = Some(500 * GIB);
    assert_eq!(due(&t.env, now + 60, false), None, "not before the daily re-check");
    assert_eq!(due(&t.env, now + 60, true), Some(Action::SetUp), "a boot re-checks at once");
    assert_eq!(due(&t.env, now + DAY, false), Some(Action::SetUp));

    // 1 GiB of RAM: set up (the preflight's 4 GiB rule only warns for K2's
    // own run), never started, even on Pro.
    *t.ops.mem.lock().unwrap() = Some(GIB);
    assert_eq!(run(&t.env, Action::SetUp)["state"], "set_up");
    t.ops.set_tier(Some("pro"));
    assert_eq!(due(&t.env, now + DAY + 60, true), None);
    assert_eq!(t.rec()["state"], "below_floor");
    assert_eq!(t.rec()["line"], "Voice rooms need 2 GiB of memory; this box has 1 GiB. They start when it has enough.");
    assert_eq!(store::load().unwrap().status, "disabled", "never started");

    // 3 GiB: chat and the Space start; voice waits for 4 GiB.
    *t.ops.mem.lock().unwrap() = Some(3 * GIB);
    assert_eq!(due(&t.env, now + DAY + 120, true), Some(Action::Start));
    assert_eq!(run(&t.env, Action::Start)["state"], "started");
    assert_eq!(store::load().unwrap().status, "running");
    let s = sup::status(&t.env, false);
    assert_eq!(s["autoSetup"]["line"], "Voice needs 4 GiB of memory; this box has 3 GiB.", "{s}");
    assert_eq!(s["autoSetup"]["floor"]["voice"], false);
    assert_eq!(s["autoSetup"]["floor"]["chat"], true);
}

/// `K2_SEW_DEFAULT=off` and a purge opt the box out; an install a person
/// made (running or disabled) is never touched, Pro or not.
#[test]
fn opt_outs_and_a_persons_install_are_left_alone() {
    let t = t();
    let now = t.now();
    {
        let _e = k2_core::test_env::EnvVar::set(OPT_OUT_ENV, "off");
        assert_eq!(due(&t.env, now, true), None);
        assert_eq!(t.rec()["state"], "opted_out");
        assert!(t.rec()["line"].as_str().unwrap().contains("K2_SEW_DEFAULT=off"), "{}", t.rec());
    }
    opt_out(&t.env, "purge");
    assert_eq!(due(&t.env, now, true), None);
    assert!(t.rec()["line"].as_str().unwrap().contains("purged"), "{}", t.rec());
    assert_eq!(opted_out(&t.env).as_deref(), Some("purge"));
    // The opt-out survives K2's own records.
    note(&t.env, "set_up", None, None, now);
    assert_eq!(t.rec()["optedOut"], true);
    t.ops.records.lock().unwrap().remove(RECORD);

    // A person's install (running), then the same disabled: never K2's.
    let req = EnableRequest::default();
    let latch = sup::enable_begin(&t.env, &req).unwrap().expect("begins");
    sup::enable_machine(&t.env, &req).unwrap();
    drop(latch);
    t.ops.set_tier(Some("pro"));
    assert_eq!(due(&t.env, now, true), None);
    sup::disable(&t.env).unwrap();
    assert_eq!(due(&t.env, now, true), None, "a person's disabled install stays off");
    assert_eq!(store::load().unwrap().status, "disabled");
}

/// A failed set-up (a preflight stop, or a helper step) is recorded with
/// why and retried daily; it never leaves a half-made row behind a
/// preflight stop.
#[test]
fn a_failed_set_up_is_retried_daily() {
    let t = t();
    let now = t.now();
    *t.ops.pg.lock().unwrap() = vec![];
    assert_eq!(due(&t.env, now, true), Some(Action::SetUp));
    let out = run(&t.env, Action::SetUp);
    assert_eq!(out["state"], "failed", "{out}");
    assert!(out["error"].as_str().unwrap().starts_with("preflight_failed"), "{out}");
    assert!(store::load().is_none(), "a preflight stop makes no row");
    assert_eq!(t.rec()["nextTryAt"], now + SETUP_RETRY_SECS);
    assert!(t.rec()["line"].as_str().unwrap().starts_with("Not set up yet: preflight_failed"), "{}", t.rec());
    assert_eq!(due(&t.env, now + 60, false), None, "not before tomorrow");
    *t.ops.pg.lock().unwrap() = vec![16];
    assert_eq!(due(&t.env, now + SETUP_RETRY_SECS, false), Some(Action::SetUp));

    // A helper step fails mid-way: the row is K2's failed set-up, retried.
    t.ops.queue("engine sew install", Err("root helper engine sew install: exit 5: boom".into()));
    let out = run(&t.env, Action::SetUp);
    assert_eq!(out["state"], "failed", "{out}");
    assert_eq!(store::load().unwrap().status, "error");
    let later = t.now() + SETUP_RETRY_SECS;
    assert_eq!(due(&t.env, t.now() + 60, false), None);
    assert_eq!(due(&t.env, later, false), Some(Action::SetUp));
    assert_eq!(run(&t.env, Action::SetUp)["state"], "set_up");
    assert!(waiting_for_pro(&store::load().unwrap()));
}
