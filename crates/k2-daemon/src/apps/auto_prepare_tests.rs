use super::*;
use std::cell::RefCell;

/// A fake box: what the doctor says before and after, what each step
/// answers, and every call made (the tests fail loudly on any root call
/// they didn't expect).
struct Fake {
    linux: bool,
    apps: bool,
    enabled: bool,
    helper: Option<u32>,
    subuid: String,
    subgid: String,
    airgap: bool,
    /// Doctors in order (the last one repeats).
    doctors: RefCell<Vec<J>>,
    prepare: Result<J, String>,
    engine: Result<J, String>,
    calls: RefCell<Vec<String>>,
    notes: RefCell<Vec<(String, String)>>,
}

fn check(id: &str, status: &str, fix: Option<&str>) -> J {
    json!({ "id": id, "status": status, "detail": "", "fix": fix })
}

fn unprepared() -> J {
    json!({ "ready": false, "checks": [
        check("platform", "ok", None),
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

impl Fake {
    fn new(doctors: Vec<J>) -> Self {
        Fake {
            linux: true,
            apps: true,
            enabled: true,
            helper: Some(8),
            subuid: "k2:100000:65536\n".into(),
            subgid: "k2:100000:65536\n".into(),
            airgap: false,
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
        }
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
    fn prepare(&self) -> Result<J, String> {
        self.calls.borrow_mut().push("prepare".into());
        self.prepare.clone()
    }
    fn install_engine(&self) -> Result<J, String> {
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
    assert_eq!(*f.calls.borrow(), vec!["prepare", "engine"]);
    assert!(changed[0].contains("K2's Podman 5.4.2"), "{changed:?}");
    assert!(changed.contains(&"apps ensure-user".to_string()) && changed.contains(&"apps subid".to_string()), "{changed:?}");
    assert!(!changed.iter().any(|c| c == "apps nft apply"), "an unchanged step isn't reported: {changed:?}");
    assert!(changed.last().unwrap().contains("App engine 0.46.3"), "{changed:?}");
    let notes = f.notes.borrow();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].1.contains("k2 app box podman uninstall") && notes[0].1.contains("appsAutoPrepare"), "{}", notes[0].1);
}

/// Idempotent: a ready box makes no root call and files no note.
#[test]
fn a_ready_box_is_left_alone() {
    let f = Fake::new(vec![ready()]);
    assert_eq!(run_once(&f), Outcome::Ready);
    assert!(f.calls.borrow().is_empty());
    assert!(f.notes.borrow().is_empty());
    // A prepared box missing only the engine gets only the engine.
    let f = Fake::new(vec![prepared_no_engine(), ready()]);
    assert!(matches!(run_once(&f), Outcome::Prepared { .. }));
    assert_eq!(*f.calls.borrow(), vec!["engine"]);
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
        assert!(f.calls.borrow().is_empty(), "{want}: no root call");
        assert!(f.notes.borrow().is_empty());
    }
}

/// Rootless Docker's sub-ids overlapping another user's: stop before any
/// change and say which.
#[test]
fn a_subid_overlap_stops_it_before_any_change() {
    let mut f = Fake::new(vec![unprepared()]);
    f.subuid = "k2:100000:65536\ndockremap:131072:65536\n".into();
    match run_once(&f) {
        Outcome::Skipped { code: "subid_overlap", why } => {
            assert!(why.contains("/etc/subuid") && why.contains("dockremap") && why.contains("k2"), "{why}");
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
/// backs off 1 h → 24 h; success and skips check daily.
#[test]
fn a_failure_is_reported_and_retried_with_backoff() {
    let mut f = Fake::new(vec![unprepared()]);
    f.prepare = Err("box prep stopped at `apps nft apply`: box prep stopped at `apps nft apply`".into());
    let out = run_once(&f);
    assert_eq!(out, Outcome::Failed { step: "apps nft apply".into(), error: "box prep stopped at `apps nft apply`: box prep stopped at `apps nft apply`".into() });
    assert_eq!(*f.calls.borrow(), vec!["prepare"]);
    assert!(f.notes.borrow().is_empty());
    let mut b = 3600;
    assert_eq!(next_wait(&out, &mut b), 3600);
    assert_eq!(next_wait(&out, &mut b), 7200);
    for _ in 0..10 {
        next_wait(&out, &mut b);
    }
    assert_eq!(next_wait(&out, &mut b), 86_400, "capped at a day");
    assert_eq!(next_wait(&Outcome::Ready, &mut b), 86_400);
    assert_eq!(b, 3600, "a good run resets the backoff");
    assert_eq!(out.json()["state"], "failed");
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
    assert_eq!(status_json()["setting"], "appsAutoPrepare");
}
