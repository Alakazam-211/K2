use super::*;
use std::cell::{Cell, RefCell};

/// A fake box: what the doctor says before and after, what each step
/// answers, and every call made (the tests fail loudly on any root call
/// they didn't expect).
struct Fake {
    linux: bool,
    apps: bool,
    enabled: bool,
    undone: bool,
    helper: Option<u32>,
    subuid: String,
    subgid: String,
    airgap: bool,
    disk: Vec<(String, u64)>,
    /// The shared root latch is held by someone else.
    latch_held: bool,
    /// Doctors in order (the last one repeats).
    doctors: RefCell<Vec<J>>,
    prepare: Result<J, String>,
    engine: Result<J, String>,
    calls: RefCell<Vec<String>>,
    notes: RefCell<Vec<(String, String)>>,
    /// Latch claims minus releases (must end at 0).
    held: Cell<i32>,
}

fn check(id: &str, status: &str, fix: Option<&str>) -> J {
    json!({ "id": id, "status": status, "detail": format!("{id} detail"), "fix": fix })
}

fn unprepared() -> J {
    json!({ "ready": false, "checks": [
        check("platform", "ok", None),
        check("kernel", "ok", None),
        check("podman", "fail", Some("k2 app box prepare --install-podman (owner; --dry-run first to see every change)")),
        check("appUser", "fail", Some("k2 app box prepare (owner)")),
        check("nft", "fail", Some("k2 app box prepare (owner)")),
        check("engine", "fail", Some("k2 app engine install (owner)")),
    ]})
}

fn prepared_no_engine() -> J {
    json!({ "ready": false, "checks": [check("platform", "ok", None), check("appUser", "ok", None), check("engine", "fail", Some("k2 app engine install (owner)"))]})
}

fn ready() -> J {
    json!({ "ready": true, "checks": [check("platform", "ok", None), check("appUser", "ok", None), check("engine", "ok", None)]})
}

/// An unprepared box whose `id` row fails with no prep fix.
fn unpreparable(id: &str) -> J {
    let mut d = unprepared();
    for c in d["checks"].as_array_mut().unwrap() {
        if c["id"] == id {
            *c = check(id, "fail", None);
        }
    }
    if !d["checks"].as_array().unwrap().iter().any(|c| c["id"] == id) {
        d["checks"].as_array_mut().unwrap().push(check(id, "fail", None));
    }
    d
}

const PLENTY: u64 = 50 * 1024 * 1024 * 1024;

impl Fake {
    fn new(doctors: Vec<J>) -> Self {
        Fake {
            linux: true,
            apps: true,
            enabled: true,
            undone: false,
            helper: Some(8),
            subuid: "k2:100000:65536\n".into(),
            subgid: "k2:100000:65536\n".into(),
            airgap: false,
            disk: vec![("/".into(), PLENTY), ("/var".into(), PLENTY)],
            latch_held: false,
            doctors: RefCell::new(doctors),
            prepare: Ok(json!({
                "ok": true,
                "podman": {"action": "installed", "version": "5.4.2", "prefix": "/opt/k2/podman"},
                "steps": [
                    {"verb": "apps podman install", "ok": true, "result": {"changed": true}},
                    {"verb": "apps ensure-user", "ok": true, "result": {"changed": true}},
                    {"verb": "apps subid", "ok": true, "result": {"changed": true}},
                    {"verb": "apps nft apply", "ok": true, "result": {"changed": false}},
                ]
            })),
            engine: Ok(json!({"ok": true, "engine": {"version": "0.46.3"}})),
            calls: RefCell::default(),
            notes: RefCell::default(),
            held: Cell::new(0),
        }
    }

    fn root_calls(&self) -> Vec<String> {
        self.calls.borrow().iter().filter(|c| !c.starts_with("latch")).cloned().collect()
    }
}

impl Ops for Fake {
    fn linux(&self) -> bool {
        self.linux
    }
    fn apps_on(&self) -> bool {
        self.apps
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn undone(&self) -> bool {
        self.undone
    }
    fn helper_protocol(&self) -> Option<u32> {
        self.helper
    }
    fn doctor(&self) -> J {
        let mut d = self.doctors.borrow_mut();
        if d.len() > 1 {
            d.remove(0)
        } else {
            d[0].clone()
        }
    }
    fn subid_files(&self) -> (String, String) {
        (self.subuid.clone(), self.subgid.clone())
    }
    fn airgap(&self) -> bool {
        self.airgap
    }
    fn disk_free(&self) -> Vec<(String, u64)> {
        self.disk.clone()
    }
    fn begin_busy(&self) -> bool {
        if self.latch_held {
            return false;
        }
        self.calls.borrow_mut().push("latch on".into());
        self.held.set(self.held.get() + 1);
        true
    }
    fn end_busy(&self) {
        self.calls.borrow_mut().push("latch off".into());
        self.held.set(self.held.get() - 1);
    }
    fn prepare(&self) -> Result<J, String> {
        assert_eq!(self.held.get(), 1, "prepare runs inside the latch");
        self.calls.borrow_mut().push("prepare".into());
        self.prepare.clone()
    }
    fn install_engine(&self) -> Result<J, String> {
        assert_eq!(self.held.get(), 1, "the engine install runs inside the latch");
        self.calls.borrow_mut().push("engine".into());
        self.engine.clone()
    }
    fn note_owner(&self, title: &str, body: &str) {
        self.notes.borrow_mut().push((title.into(), body.into()));
    }
}

/// An unprepared box: prepare (with K2's Podman), then the engine, then one
/// owner note naming what changed and how to undo it.
#[test]
fn an_unprepared_box_is_prepared_once_and_the_owner_is_told() {
    let f = Fake::new(vec![unprepared(), prepared_no_engine()]);
    let out = run_once(&f);
    let Outcome::Prepared { changed } = &out else { panic!("{out:?}") };
    assert_eq!(*f.calls.borrow(), vec!["latch on", "prepare", "engine", "latch off"]);
    assert_eq!(f.held.get(), 0, "the latch is released");
    assert!(changed[0].contains("K2's Podman 5.4.2"), "{changed:?}");
    assert!(changed.contains(&"apps ensure-user".to_string()) && changed.contains(&"apps subid".to_string()), "{changed:?}");
    assert!(!changed.iter().any(|c| c == "apps nft apply"), "an unchanged step isn't reported: {changed:?}");
    assert!(changed.last().unwrap().contains("App engine 0.46.3"), "{changed:?}");
    let notes = f.notes.borrow();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].1.contains("k2 app box podman uninstall") && notes[0].1.contains("k2 app box unprep") && notes[0].1.contains("appsAutoPrepare"),
        "{}",
        notes[0].1
    );
}

/// Idempotent: a ready box makes no root call, takes no latch, files no note.
#[test]
fn a_ready_box_is_left_alone() {
    let f = Fake::new(vec![ready()]);
    assert_eq!(run_once(&f), Outcome::Ready);
    assert!(f.calls.borrow().is_empty());
    assert!(f.notes.borrow().is_empty());
    // A prepared box missing only the engine gets only the engine.
    let f = Fake::new(vec![prepared_no_engine(), ready()]);
    assert!(matches!(run_once(&f), Outcome::Prepared { .. }));
    assert_eq!(f.root_calls(), vec!["engine"]);
}

/// The owner's switch off, Apps off, no helper or an old one, not Linux:
/// nothing runs.
#[test]
fn it_respects_the_setting_and_needs_the_owners_helper_on_linux() {
    let cases: Vec<(Box<dyn Fn(&mut Fake)>, &str)> = vec![
        (Box::new(|f: &mut Fake| f.enabled = false), "setting_off"),
        (Box::new(|f: &mut Fake| f.apps = false), "apps_off"),
        (Box::new(|f: &mut Fake| f.helper = None), "no_helper"),
        (Box::new(|f: &mut Fake| f.helper = Some(7)), "helper_old"),
        (Box::new(|f: &mut Fake| f.linux = false), "not_linux"),
        (Box::new(|f: &mut Fake| f.airgap = true), "airgap"),
    ];
    for (set, want) in cases {
        let mut f = Fake::new(vec![unprepared()]);
        set(&mut f);
        match run_once(&f) {
            Outcome::Skipped { code, .. } => assert_eq!(code, want),
            other => panic!("{want}: {other:?}"),
        }
        assert!(f.calls.borrow().is_empty(), "{want}: no root call, no latch");
        assert!(f.notes.borrow().is_empty());
    }
}

/// Rootless Docker's sub-ids overlapping another user's: stop before any
/// change, say which, and wait for the owner (F6: blocked, no daily retry).
#[test]
fn a_subid_overlap_stops_it_before_any_change() {
    let mut f = Fake::new(vec![unprepared()]);
    f.subuid = "k2:100000:65536\ndockremap:131072:65536\n".into();
    match run_once(&f) {
        Outcome::Blocked { code, why } => {
            assert_eq!(code, "subid_overlap");
            assert!(why.contains("/etc/subuid") && why.contains("dockremap") && why.contains("k2"), "{why}");
            assert!(why.contains("Retry"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    assert!(f.calls.borrow().is_empty());
    assert_eq!(subid_conflict("a:100000:65536\nb:165536:65536\n", "a:100000:65536\nb:165536:65536\n"), None, "adjacent is fine");
    assert!(subid_conflict("a:1:10\n", "a:100000:65536\nb:100001:2\n").unwrap().contains("/etc/subgid"));
    assert_eq!(subid_conflict("", ""), None);
    assert_eq!(subid_conflict("# comment\nk2:100000:65536\nk2:200000:65536\n", ""), None, "one owner's own ranges never conflict");
}

/// A failing step: Failed with the step, no engine, no note; the retry
/// backs off 1 h → 24 h; success and skips check daily; a held latch
/// comes back in 5 minutes.
#[test]
fn a_failure_is_reported_and_retried_with_backoff() {
    let mut f = Fake::new(vec![unprepared()]);
    f.prepare = Err("box prep stopped at `apps nft apply`: /usr/sbin/nft exit 1: Operation not permitted".into());
    let out = run_once(&f);
    assert_eq!(
        out,
        Outcome::Failed { step: "apps nft apply".into(), error: "box prep stopped at `apps nft apply`: /usr/sbin/nft exit 1: Operation not permitted".into() }
    );
    assert_eq!(*f.calls.borrow(), vec!["latch on", "prepare", "latch off"]);
    assert!(f.notes.borrow().is_empty());
    let mut b = 3600;
    assert_eq!(next_wait(&out, &mut b), Some(3600));
    assert_eq!(next_wait(&out, &mut b), Some(7200));
    for _ in 0..10 {
        next_wait(&out, &mut b);
    }
    assert_eq!(next_wait(&out, &mut b), Some(86_400), "capped at a day");
    assert_eq!(next_wait(&Outcome::Ready, &mut b), Some(86_400));
    assert_eq!(b, 3600, "a good run resets the backoff");
    assert_eq!(out.json()["state"], "failed");
    assert_eq!(next_wait(&skip("busy", "x"), &mut b), Some(300));
}

/// F6: a box prep can't fix (kernel, cgroup v1, old systemd, a distro
/// with no Podman 5 and no bundle) is blocked before any root call; one
/// whose helper says so mid-run (podman_too_old, a source-built Podman,
/// no bundle for the CPU) too. Blocked never schedules a retry.
#[test]
fn an_unpreparable_box_is_blocked_and_never_retried_daily() {
    for (row, code) in [("kernel", "kernel_too_old"), ("cgroupV2", "cgroup_v1"), ("systemd", "systemd_too_old"), ("podman", "distro_unsupported")] {
        let f = Fake::new(vec![unpreparable(row)]);
        match run_once(&f) {
            Outcome::Blocked { code: c, why } => {
                assert_eq!(c, code, "{row}");
                assert!(why.contains(&format!("{row} detail")) && why.contains("Retry"), "{why}");
            }
            other => panic!("{row}: {other:?}"),
        }
        assert!(f.calls.borrow().is_empty(), "{row}: no root call, no latch");
    }
    // A kernel row alone (nothing for prep to fix) still blocks, not "ready".
    let f = Fake::new(vec![json!({"ready": false, "checks": [check("kernel", "fail", None), check("engine", "ok", None)]})]);
    assert!(run_once(&f).is_blocked());

    for (err, code) in [
        ("box prep stopped at `apps packages install`: podman_too_old: the box's Podman is 4.9.3", "distro_unsupported"),
        ("box prep stopped at `apps packages install`: customer_podman: a Podman K2 didn't install is at /usr/local/bin/podman", "customer_podman"),
        ("box prep stopped at `apps packages install`: no supported package manager (apt-get, dnf, pacman)", "distro_unsupported"),
        ("unsupported: no K2 Podman bundle for this CPU (x86_64, aarch64)", "cpu_unsupported"),
        ("box prep stopped at `apps packages install`: apt_would_remove: installing x would remove docker-ce", "apt_would_remove"),
    ] {
        let mut f = Fake::new(vec![unprepared()]);
        f.prepare = Err(err.into());
        let out = run_once(&f);
        match &out {
            Outcome::Blocked { code: c, why } => {
                assert_eq!(c, code, "{err}");
                assert!(why.contains(err), "{why}");
            }
            other => panic!("{err}: {other:?}"),
        }
        assert_eq!(f.held.get(), 0, "released");
        let mut b = 3600;
        assert_eq!(next_wait(&out, &mut b), None, "{err}: no retry until Retry or an update");
    }
    // A transient failure is not a block.
    assert_eq!(permanent_from_error("box prep stopped at `apps packages install`: /usr/bin/env exit 100: Could not resolve 'archive.ubuntu.com'"), None);
    // The owner's undo holds it the same way, before any root call.
    let mut f = Fake::new(vec![unprepared()]);
    f.undone = true;
    match run_once(&f) {
        Outcome::Blocked { code, .. } => assert_eq!(code, "owner_undid"),
        other => panic!("{other:?}"),
    }
    assert!(f.calls.borrow().is_empty());
}

/// F5: below 5 GiB free on / or /var nothing starts (no latch, no root
/// call), the doctor line says where, and it looks again in 6 hours.
#[test]
fn low_disk_refuses_before_any_step() {
    let mut f = Fake::new(vec![unprepared()]);
    f.disk = vec![("/".into(), PLENTY), ("/var".into(), 2 * 1024 * 1024 * 1024)];
    let out = run_once(&f);
    match &out {
        Outcome::Skipped { code, why } => {
            assert_eq!(code, "low_disk");
            assert!(why.contains("2.0 GiB free on /var") && why.contains("5.0 GiB"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    assert!(f.calls.borrow().is_empty());
    let mut b = 3600;
    assert_eq!(next_wait(&out, &mut b), Some(6 * 3600));
}

/// F9: the latch held (a hosted-mail upgrade, the helper heal, an owner's
/// prep): no root call, back in 5 minutes; a step that panics still
/// releases it.
#[test]
fn it_waits_for_the_shared_root_latch() {
    let mut f = Fake::new(vec![unprepared()]);
    f.latch_held = true;
    match run_once(&f) {
        Outcome::Skipped { code, .. } => assert_eq!(code, "busy"),
        other => panic!("{other:?}"),
    }
    assert!(f.calls.borrow().is_empty());

    let mut f = Fake::new(vec![unprepared()]);
    f.engine = Err("engine_install_failed: x".into());
    f.doctors = RefCell::new(vec![unprepared(), prepared_no_engine()]);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        struct Boom<'a>(&'a Fake);
        impl Ops for Boom<'_> {
            fn linux(&self) -> bool { true }
            fn apps_on(&self) -> bool { true }
            fn enabled(&self) -> bool { true }
            fn undone(&self) -> bool { false }
            fn helper_protocol(&self) -> Option<u32> { Some(8) }
            fn doctor(&self) -> J { unprepared() }
            fn subid_files(&self) -> (String, String) { (String::new(), String::new()) }
            fn airgap(&self) -> bool { false }
            fn disk_free(&self) -> Vec<(String, u64)> { vec![] }
            fn begin_busy(&self) -> bool { self.0.begin_busy() }
            fn end_busy(&self) { self.0.end_busy() }
            fn prepare(&self) -> Result<J, String> { panic!("boom") }
            fn install_engine(&self) -> Result<J, String> { unreachable!() }
            fn note_owner(&self, _: &str, _: &str) {}
        }
        run_once(&Boom(&f))
    }));
    assert!(r.is_err(), "the panic propagates");
    assert_eq!(f.held.get(), 0, "and the latch is still released");
}

/// F7: the status survives a restart (a new store at the same path reads
/// it back); a block from this version holds at boot, an update or a
/// Retry lifts it; the owner's undo is kept across runs until Retry.
#[test]
fn the_status_persists_across_a_restart() {
    let dir = crate::test_support::Scratch::new();
    let path = dir.path().join("apps-auto-prepare.json");
    let store = Store { path: path.clone() };
    assert!(status_json_from(&store).get("lastRun").is_none(), "never run");
    let out = Outcome::Prepared { changed: vec!["apps ensure-user".into()] };
    store.record(&out, Some(1_000_100), 1_000_000, "0.46.3").unwrap();

    // "Restart": a fresh store over the same file.
    let after = Store { path: path.clone() };
    let s = status_json_from(&after);
    assert_eq!(s["lastRun"], 1_000_000);
    assert_eq!(s["outcome"], out.json());
    assert_eq!(s["nextAttempt"], 1_000_100);
    assert_eq!(s["version"], "0.46.3");
    assert_eq!(s["blocked"], false);
    assert!(!after.held_at_boot("0.46.3"));
    assert_eq!(Outcome::from_json(&out.json()), Some(out));

    // A block: held at boot on the same version, lifted by an update.
    let b = blocked("kernel_too_old", "5.4: Apps need Linux 5.11 or newer");
    after.record(&b, None, 1_000_200, "0.46.3").unwrap();
    let s = status_json_from(&Store { path: path.clone() });
    assert_eq!(s["blocked"], true);
    assert_eq!(s["outcome"]["code"], "kernel_too_old");
    assert!(s["nextAttempt"].is_null(), "no retry scheduled");
    assert!(after.held_at_boot("0.46.3"));
    assert!(!after.held_at_boot("0.46.4"), "a K2 update lifts it");
    // Retry lifts it too, and survives a restart until the next run.
    after.retry(1_000_300).unwrap();
    assert!(!Store { path: path.clone() }.held_at_boot("0.46.3"));
    assert_eq!(status_json_from(&after)["blocked"], false);
    after.record(&b, None, 1_000_400, "0.46.3").unwrap();
    assert!(after.held_at_boot("0.46.3"), "the next run's block holds again");

    // The undo hold: kept by runs, cleared by Retry.
    after.mark_undone(1_000_500).unwrap();
    after.record(&skip("setting_off", "off"), Some(1), 1_000_600, "0.46.3").unwrap();
    assert_eq!(status_json_from(&Store { path: path.clone() })["undoneAt"], 1_000_500);
    after.retry(1_000_700).unwrap();
    assert!(status_json_from(&after)["undoneAt"].is_null());

    // An unreadable file is "never run", not a crash.
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(Store { path }.load(), Saved::default());
}

/// The setting's default is the reviewed one, and the env override wins.
#[test]
fn the_default_and_the_env_override() {
    assert!(!k2_core::deploy::AUTO_PREPARE_DEFAULT, "OFF until design-apps-auto-prepare-0463 is signed off");
    let _on = k2_core::test_env::EnvVar::set(k2_core::deploy::AUTO_PREPARE_ENV, "1");
    assert!(k2_core::deploy::auto_prepare_enabled());
    drop(_on);
    let _off = k2_core::test_env::EnvVar::set(k2_core::deploy::AUTO_PREPARE_ENV, "off");
    assert!(!k2_core::deploy::auto_prepare_enabled());
    let dir = crate::test_support::Scratch::new();
    let s = status_json_from(&Store { path: dir.path().join("s.json") });
    assert_eq!(s["setting"], "appsAutoPrepare");
    assert_eq!(s["retryRoute"], RETRY_ROUTE);
}

/// The Retry route is the owner's.
#[test]
fn retry_is_the_owners() {
    for w in [
        Who::Login { username: "a".into(), role: Role::Admin },
        Who::Agent { project_id: "p".into(), session_id: "s".into() },
    ] {
        assert_eq!(retry_route(&w).unwrap_err().status, 403, "{w:?}");
    }
}
