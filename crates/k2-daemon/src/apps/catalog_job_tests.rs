//! The one-click install job against a fake box (catalog_job.rs): every
//! step to Ready, the refusals that install nothing, the named waits, Sew's
//! Meet setup against a fake mgmt API, and a daemon restart mid-install.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use k2_core::suite::jobs::{self, JobRow};
use k2_core::suite::{self, testkit, Catalog};
use serde_json::{json, Value as J};

use super::*;
use crate::apps::catalog_routes::{install_with, Facts};

pub(crate) const WS: &str = "ws-suite";

pub(crate) struct Fx {
    pub _home: crate::test_support::TempHome,
    pub _db: k2_core::db::TestDbGuard,
    pub dir: PathBuf,
}

pub(crate) fn fx() -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![WS, format!("/tmp/{WS}")]).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("k2-suite-job-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    Fx { _home: home, _db: db, dir }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Switch 1 (and 2) for the workspace's agents.
pub(crate) fn set_switches(build: bool, full: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute(
        "UPDATE projects SET agents_can_manage_skin = ?2, agents_can_manage_apps_full = ?3 WHERE id = ?1",
        rusqlite::params![WS, build as i64, full as i64],
    )
    .unwrap();
}

pub(crate) fn agent() -> Who {
    Who::Agent { project_id: WS.into(), session_id: "sess-suite".into() }
}

/// A Seed tarball in Sew's layout and its K2 tree hash (stored the way the
/// job stores it, under the temp home).
pub(crate) struct Seed {
    pub bytes: Vec<u8>,
    pub seed_hash: String,
}

pub(crate) fn seed(app: &str, version: &str, variant: &str, caps: &[&str], boxes: &[&str], commit: &str) -> Seed {
    let m = testkit::manifest(app, version, variant, caps, boxes);
    let files = testkit::seed_files(&m, &testkit::build_json(app, version, variant, commit));
    let bytes = testkit::tar_gz(&files);
    let scratch = std::env::temp_dir().join(format!("k2-suite-hash-{}", uuid::Uuid::new_v4().simple()));
    let tgz = scratch.with_extension("tgz");
    std::fs::write(&tgz, &bytes).unwrap();
    suite::unpack::unpack_tar_gz(&tgz, &scratch, suite::unpack::Limits::seed()).unwrap();
    let stored = k2_core::deploy::seed::store_folder(&scratch, &k2_core::deploy::seed::build_excludes(&[])).unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let _ = std::fs::remove_file(&tgz);
    Seed { bytes, seed_hash: stored.hash }
}

/// The compiled catalog plus one pinned version of `entry` for `s`.
pub(crate) fn catalog_with(entry: &str, s: &Seed, caps: &[&str], boxes: &[&str]) -> Catalog {
    let mut c = suite::catalog().unwrap().clone();
    let e = c.entries.iter_mut().find(|e| e.entry == entry).unwrap();
    let setup: &[&str] = if entry == "sew/meet" { &[suite::SETUP_MEET_INIT, suite::SETUP_APP_SPACE_RECORD, suite::SETUP_MEETINGS_ENABLED] } else { &[] };
    e.versions.push(testkit::version(&e.app.clone(), "0.2.4", "k2", &s.bytes, &s.seed_hash, caps, boxes, setup));
    suite::check(&c).unwrap();
    c
}

pub(crate) fn facts() -> Facts {
    Facts { linux: true, sew: Ok(("k2".into(), Some("0.7.3".into()))), pro: Some(true) }
}

/// The fake box.
#[derive(Default)]
pub(crate) struct Fake {
    pub tarball: Mutex<Option<PathBuf>>,
    pub fetch_err: Mutex<Option<FetchError>>,
    pub fetches: AtomicUsize,
    pub pro_err: Mutex<Option<(&'static str, String)>>,
    pub pointed: Mutex<Vec<(String, Option<u16>)>>,
    pub attached: Mutex<Vec<String>>,
    pub installs: AtomicUsize,
    pub install_held: Mutex<Option<String>>,
    pub deploys: Mutex<Vec<J>>,
    pub deploy_held: Mutex<Option<String>>,
    pub box_held: Mutex<Option<String>>,
    pub panic_on_deploy: AtomicBool,
    /// id → (seed, dep)
    pub deps: Mutex<HashMap<String, (String, Dep)>>,
    pub requests: Mutex<HashMap<String, String>>,
    pub manage_sew: AtomicBool,
    pub mgmt: Mutex<Vec<(String, J)>>,
    pub mgmt_answers: Mutex<VecDeque<Result<J, MgmtError>>>,
    pub spaces: Mutex<Vec<(String, String)>>,
    pub ready: Mutex<VecDeque<Result<(u16, J), String>>>,
    pub probed: Mutex<Vec<(u16, String)>>,
}

pub(crate) const FRONT: u16 = 40123;
pub(crate) const PORT: u16 = 41000;

impl Fake {
    pub(crate) fn new(tarball: PathBuf) -> Fake {
        let f = Fake::default();
        *f.tarball.lock().unwrap() = Some(tarball);
        f.manage_sew.store(true, Ordering::SeqCst);
        f
    }
    pub(crate) fn add_dep(&self, id: &str, seed: &str, result: &str) {
        self.deps.lock().unwrap().insert(id.into(), (seed.into(), Dep { id: id.into(), result: result.into(), port: Some(PORT), error: None }));
    }
}

impl Hooks for Fake {
    fn fetch(&self, _f: &ReleaseFile) -> Result<PathBuf, FetchError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.fetch_err.lock().unwrap().clone() {
            return Err(e);
        }
        Ok(self.tarball.lock().unwrap().clone().expect("a tarball"))
    }
    fn pro_check(&self) -> Result<(), Refusal> {
        match self.pro_err.lock().unwrap().clone() {
            Some((c, h)) => Err(no(c, 409, h)),
            None => Ok(()),
        }
    }
    fn point_suite_label(&self, label: &str, port: Option<u16>) -> Result<(), String> {
        self.pointed.lock().unwrap().push((label.into(), port));
        Ok(())
    }
    fn attach_label(&self, _who: &Who, _app_id: &str, label: &str, _front: u16) -> R<J> {
        self.attached.lock().unwrap().push(label.into());
        Ok(json!({ "ok": true }))
    }
    fn install(&self, _who: &Who, body: &J) -> R<J> {
        if let Some(r) = self.install_held.lock().unwrap().clone() {
            return Ok(json!({ "status": "held", "request": { "id": r } }));
        }
        self.installs.fetch_add(1, Ordering::SeqCst);
        let app = body["app"].as_str().unwrap();
        let h = k2_core::db::shared();
        let conn = h.lock();
        let svc = k2_core::published_services::insert(&conn, WS, app, "(app)", "/tmp/x", 0, "local", "stopped", "cmd", "").unwrap();
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: app,
                service_id: &svc.id,
                seed_id: None,
                owner_workspace: WS,
                isolation: "container",
                memory_limit_bytes: None,
                front_port: FRONT,
                created_by: "test",
            },
        )
        .unwrap();
        Ok(json!({ "ok": true, "frontPort": FRONT }))
    }
    fn deploy(&self, _who: &Who, body: &J) -> R<J> {
        if self.panic_on_deploy.load(Ordering::SeqCst) {
            panic!("the daemon dies mid-deploy");
        }
        self.deploys.lock().unwrap().push(body.clone());
        if let Some(r) = self.deploy_held.lock().unwrap().clone() {
            return Ok(json!({ "status": "held", "request": { "id": r } }));
        }
        let id = format!("dep-{}", self.deps.lock().unwrap().len() + 1);
        self.add_dep(&id, body["seed"].as_str().unwrap(), "live");
        let mut out = json!({ "ok": true, "deployment": { "id": id } });
        if let Some(r) = self.box_held.lock().unwrap().clone() {
            out["boxServices"] = json!({ "status": "held", "request": { "id": r } });
        }
        Ok(out)
    }
    fn deployment(&self, _app_id: &str, id: Option<&str>, seed: &str) -> Option<Dep> {
        let d = self.deps.lock().unwrap();
        match id {
            Some(id) => d.get(id).map(|(_, d)| d.clone()),
            None => d.values().find(|(s, _)| s == seed).map(|(_, d)| d.clone()),
        }
    }
    fn request_status(&self, id: &str) -> Option<String> {
        self.requests.lock().unwrap().get(id).cloned()
    }
    fn may_manage_sew(&self, _who: &Who) -> bool {
        self.manage_sew.load(Ordering::SeqCst)
    }
    fn sew_mgmt(&self, method: &str, path: &str, body: &J) -> Result<J, MgmtError> {
        assert_eq!(method, "POST");
        self.mgmt.lock().unwrap().push((path.into(), body.clone()));
        if let Some(a) = self.mgmt_answers.lock().unwrap().pop_front() {
            return a;
        }
        if path == MGMT_MEET_INIT {
            return Ok(json!({ "space_id": "sp-meetings", "created": true, "app_id": body["app_id"] }));
        }
        Ok(json!({ "ok": true }))
    }
    fn record_app_space(&self, service_id: &str, space_id: &str, _by: &str) -> Result<(), String> {
        self.spaces.lock().unwrap().push((service_id.into(), space_id.into()));
        Ok(())
    }
    fn readyz(&self, port: u16, path: &str) -> Result<(u16, J), String> {
        self.probed.lock().unwrap().push((port, path.into()));
        if let Some(a) = self.ready.lock().unwrap().pop_front() {
            return a;
        }
        let app = if self.mgmt.lock().unwrap().is_empty() { "hearth" } else { "meet" };
        Ok((200, json!({ "ok": true, "app": app, "version": "0.2.4", "variant": "k2", "commit": testkit::COMMIT })))
    }
}

pub(crate) fn write_tarball(fx: &Fx, s: &Seed) -> PathBuf {
    let p = fx.dir.join("seed.tar.gz");
    std::fs::write(&p, &s.bytes).unwrap();
    p
}

fn get(id: &str) -> JobRow {
    let h = k2_core::db::shared();
    let conn = h.lock();
    jobs::get(&conn, id).unwrap().unwrap()
}

fn job_id(v: &J) -> String {
    v["job"]["id"].as_str().unwrap().to_string()
}

fn app_exists(app: &str) -> bool {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::store::get_app(&conn, app).unwrap().is_some()
}

fn done_steps(j: &JobRow) -> Vec<String> {
    j.steps.iter().map(|s| s.step.clone()).collect()
}

fn audit_events(app: &str) -> Vec<String> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut st = conn.prepare("SELECT event FROM deploy_audit WHERE subject_id = ?1 ORDER BY rowid").unwrap();
    st.query_map(rusqlite::params![app], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect()
}

fn hearth(fx: &Fx) -> (Seed, Catalog, Fake) {
    let s = seed("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"], testkit::COMMIT);
    let c = catalog_with("sew/hearth", &s, testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    let f = Fake::new(write_tarball(fx, &s));
    (s, c, f)
}

fn start(c: &Catalog, f: &Fake, who: &Who, extra: J) -> String {
    let mut body = json!({ "entry": "sew/hearth", "workspace": WS });
    if let J::Object(m) = extra {
        for (k, v) in m {
            body[k] = v;
        }
    }
    let v = install_with(who, &body, c, &facts(), f).expect("the job starts");
    job_id(&v)
}

// ── the whole way ────────────────────────────────────────────────────

/// R4/A5: Download → Check → Install → Address → Deploy → Approve → Sew
/// setup → Healthy → Ready, in that order, the address bound before the
/// first deploy, and an audit event for every step (R35).
#[test]
fn a_hearth_install_runs_every_step_to_ready() {
    let fx = fx();
    let (s, c, f) = hearth(&fx);
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_DONE, "{j:?}");
    assert_eq!(done_steps(&j), jobs::STEPS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!((j.app_id.as_str(), j.address_kind.as_str(), j.address.as_str()), ("hearth", jobs::ADDRESS_SUITE_LABEL, "chat"));
    assert_eq!(j.seed_hash.as_deref(), Some(s.seed_hash.as_str()), "the stored Seed is the pinned tree");
    assert!(app_exists("hearth"));
    // The address before the first deploy, at the front port.
    assert_eq!(*f.pointed.lock().unwrap(), vec![("chat".to_string(), Some(FRONT))]);
    let deploys = f.deploys.lock().unwrap().clone();
    assert_eq!(deploys.len(), 1);
    assert_eq!(deploys[0]["seed"], json!(s.seed_hash), "deployed by hash, never the folder");
    assert_eq!(*f.probed.lock().unwrap(), vec![(PORT, "/readyz".to_string())]);
    assert!(f.mgmt.lock().unwrap().is_empty(), "Hearth needs no Sew setup");
    let h = k2_core::db::shared();
    {
        let conn = h.lock();
        let l = jobs::lineage_get(&conn, "hearth").unwrap().unwrap();
        assert_eq!((l.entry.as_str(), l.version.as_str(), l.variant.as_str(), l.job_id.as_str()), ("sew/hearth", "0.2.4", "k2", id.as_str()));
        assert_eq!(jobs::label_get(&conn, "chat").unwrap().unwrap().app_id, "hearth");
    }
    let ev = audit_events("hearth");
    for step in jobs::STEPS {
        assert!(ev.contains(&format!("suite.install.{step}")), "audit for {step}: {ev:?}");
    }
    assert!(ev.contains(&"suite.install.requested".to_string()) && ev.contains(&"suite.install.done".to_string()), "{ev:?}");
    // The view the CLI and Settings render.
    let v = job_view(&j);
    assert_eq!(v["state"], "done");
    assert!(v["next"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("k2 app user invite hearth")), "{v}");
    // People read a sentence; the command rides apart for agents.
    let steps = v["nextSteps"].as_array().unwrap();
    assert_eq!(steps.len(), v["next"].as_array().unwrap().len(), "{v}");
    let invite = &steps[0];
    assert_eq!(invite["text"], "Invite people to Hearth from Settings → Cores → Apps → Hearth → People.", "{v}");
    assert_eq!(invite["hint"], "k2 app user invite hearth <email>", "{v}");
    for st in steps {
        let text = st["text"].as_str().unwrap();
        assert!(!text.contains("k2 ") && !text.contains("<email>") && !text.contains("--"), "a command in the people's words: {text}");
        assert!(st["hint"].as_str().unwrap().starts_with("k2 "), "{st}");
    }
    assert_plain(&v);
}

/// A string a person reads never carries an internal reference: a review
/// id (`vs-live A4`), a PRD name or an open question (`Q13`), or a
/// requirement / step id (`R27`, `S3`, `A12`, `AT5`).
pub(crate) fn internal_ref(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for bad in ["vs-live", "prd-", "prd "] {
        if lower.contains(bad) {
            return Some(bad.to_string());
        }
    }
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| {
            let letters: String = w.chars().take_while(char::is_ascii_uppercase).collect();
            let digits = &w[letters.len()..];
            matches!(letters.as_str(), "A" | "AT" | "Q" | "R" | "S" | "H") && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_string)
}

/// Every string anywhere in `v` is free of internal references.
pub(crate) fn assert_plain(v: &J) {
    fn walk(v: &J, at: &str) {
        match v {
            J::String(s) => {
                if let Some(r) = internal_ref(s) {
                    panic!("{at}: {s:?} carries the internal reference {r:?}");
                }
            }
            J::Array(a) => a.iter().enumerate().for_each(|(i, x)| walk(x, &format!("{at}[{i}]"))),
            J::Object(m) => m.iter().for_each(|(k, x)| walk(x, &format!("{at}.{k}"))),
            _ => {}
        }
    }
    walk(v, "$");
}

#[test]
fn the_internal_reference_check_catches_what_it_should() {
    for bad in ["the owner (vs-live A4)", "see prd-one-click-suite-apps-v1", "open question Q13", "R27 says", "PRD 3"] {
        assert!(internal_ref(bad).is_some(), "{bad}");
    }
    for ok in ["Hearth 0.2.4", "K2 Connect Pro", "sha256 abc", "chat.your-server.k2.dev", "E2E", "Settings → Apps"] {
        assert_eq!(internal_ref(ok), None, "{ok}");
    }
}

// ── refusals that install nothing ────────────────────────────────────

/// AT5: one changed byte → `catalog_mismatch`, no App row, no install.
#[test]
fn a_changed_byte_is_refused_and_nothing_is_installed() {
    let fx = fx();
    let (s, c, f) = hearth(&fx);
    let mut bad = s.bytes.clone();
    let i = bad.len() / 2;
    bad[i] ^= 1;
    std::fs::write(f.tarball.lock().unwrap().clone().unwrap(), &bad).unwrap();
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_FAILED);
    assert_eq!(j.error_code.as_deref(), Some(suite::codes::CATALOG_MISMATCH), "{:?}", j.error_hint);
    assert!(j.error_hint.as_deref().unwrap().contains("(sha256:"), "{:?}", j.error_hint);
    assert_eq!(f.installs.load(Ordering::SeqCst), 0);
    assert!(!app_exists("hearth"), "no App row");
    // The fetch's own pin check (a download that isn't the pinned file).
    let fx2 = fx;
    let (_s, c, f) = hearth(&fx2);
    *f.fetch_err.lock().unwrap() = Some(FetchError::Mismatch("the download has sha256 x".into()));
    let h = k2_core::db::shared();
    {
        let conn = h.lock();
        conn.execute("DELETE FROM suite_install_jobs", []).unwrap();
    }
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.error_code.as_deref(), Some(suite::codes::CATALOG_MISMATCH));
    assert_eq!(f.installs.load(Ordering::SeqCst), 0);
}

/// AT6: the Seed's manifest asks for other caps than the catalog pins →
/// `catalog_mismatch (caps)`, nothing installed.
#[test]
fn a_manifest_that_disagrees_with_the_catalog_is_refused() {
    let fx = fx();
    let s = seed("hearth", "0.2.4", "k2", &["chat:read", "chat:post", "voice:join"], &["sew-api", "sew-signal"], testkit::COMMIT);
    let c = catalog_with("sew/hearth", &s, testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    let f = Fake::new(write_tarball(&fx, &s));
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.error_code.as_deref(), Some(suite::codes::CATALOG_MISMATCH));
    assert!(j.error_hint.as_deref().unwrap().contains("(caps:"), "{:?}", j.error_hint);
    assert!(j.error_hint.as_deref().unwrap().ends_with("so nothing was installed"), "{:?}", j.error_hint);
    assert_eq!(f.installs.load(Ordering::SeqCst), 0);
    assert!(!app_exists("hearth"));
    // build.json naming another commit is refused the same way.
    let other = seed("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"], &"2".repeat(40));
    let c = catalog_with("sew/hearth", &other, testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    *f.tarball.lock().unwrap() = Some(write_tarball(&fx, &other));
    {
        let h = k2_core::db::shared();
        h.lock().execute("DELETE FROM suite_install_jobs", []).unwrap();
    }
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert!(j.error_hint.as_deref().unwrap().contains("(build.json:"), "{:?}", j.error_hint);
}

/// A12: the stored Seed's tree hash must be the pinned `seedHash`.
#[test]
fn a_stored_tree_that_isnt_the_pinned_seed_hash_is_refused() {
    let fx = fx();
    let mut s = seed("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"], testkit::COMMIT);
    let real = s.seed_hash.clone();
    s.seed_hash = "0".repeat(64);
    let c = catalog_with("sew/hearth", &s, testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    let f = Fake::new(write_tarball(&fx, &s));
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    let hint = j.error_hint.clone().unwrap();
    assert!(hint.contains("(seedHash:") && hint.contains(&real), "{hint}");
    assert_eq!(f.installs.load(Ordering::SeqCst), 0);
}

// ── a restart mid-install (AT12) ─────────────────────────────────────

/// The daemon dies in the middle of the deploy step. On restart the job
/// resumes at that step: nothing before it runs again (no second download,
/// install or label bind), and it reaches Ready.
#[test]
fn the_job_resumes_after_a_restart_mid_install() {
    let fx = fx();
    let (_s, c, f) = hearth(&fx);
    f.panic_on_deploy.store(true, Ordering::SeqCst);
    let id = start(&c, &f, &Who::Owner, json!({}));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| advance(&f, &id)));
    assert!(crashed.is_err(), "the step died");
    let j = get(&id);
    assert_eq!((j.state.as_str(), j.step.as_str()), (jobs::STATE_RUNNING, jobs::STEP_DEPLOY), "left at the step it was on");
    assert_eq!(done_steps(&j), vec!["download", "check", "install", "address"]);
    // "Restart": a fresh daemon (new hooks), the driver's pass.
    let tarball = f.tarball.lock().unwrap().clone().unwrap();
    let f2 = Fake::new(tarball);
    advance_all(&f2);
    let j = get(&id);
    assert_eq!(j.state, jobs::STATE_DONE, "{j:?}");
    assert_eq!(f2.fetches.load(Ordering::SeqCst), 0, "no second download");
    assert_eq!(f2.installs.load(Ordering::SeqCst), 0, "no second install");
    assert!(f2.pointed.lock().unwrap().is_empty(), "no second label bind");
    assert_eq!(f2.deploys.lock().unwrap().len(), 1);
    assert_eq!(f.installs.load(Ordering::SeqCst) + f2.installs.load(Ordering::SeqCst), 1);
    // A deploy that already went out before the death is not sent again.
    let fx3 = fx;
    drop(fx3);
}

/// A deployment the dead daemon already started is found, not repeated.
#[test]
fn a_deploy_that_went_out_before_the_restart_is_not_sent_again() {
    let fx = fx();
    let (s, c, f) = hearth(&fx);
    f.panic_on_deploy.store(true, Ordering::SeqCst);
    let id = start(&c, &f, &Who::Owner, json!({}));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| advance(&f, &id)));
    let f2 = Fake::new(f.tarball.lock().unwrap().clone().unwrap());
    f2.add_dep("dep-before", &s.seed_hash, "live");
    let j = advance(&f2, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_DONE);
    assert!(f2.deploys.lock().unwrap().is_empty(), "found, not deployed again");
    assert_eq!(j.deployment_id.as_deref(), Some("dep-before"));
}

// ── waits ────────────────────────────────────────────────────────────

/// Q13 (A4): an agent with "Build and run Apps" but not Manage Sew waits
/// at Address for a suite label (`waiting_sew_rights`), and goes on by
/// itself once it has it. Q5 (A8/A9): a deploy held for the owner waits at
/// Approve (`waiting_approval`) on the filed request, and goes on when the
/// approval's replay deploys.
#[test]
fn waiting_states_name_their_question_and_clear_by_themselves() {
    let fx = fx();
    set_switches(true, false);
    let (s, c, f) = hearth(&fx);
    f.manage_sew.store(false, Ordering::SeqCst);
    *f.deploy_held.lock().unwrap() = Some("req-caps".into());
    f.requests.lock().unwrap().insert("req-caps".into(), "pending".into());
    let id = start(&c, &f, &agent(), json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.state.as_str(), j.step.as_str()), (jobs::STATE_WAITING, jobs::STEP_ADDRESS));
    assert_eq!((j.wait_reason.as_deref(), j.wait_question.as_deref()), (Some(jobs::WAIT_SEW_RIGHTS), None), "no open question is ever named");
    assert!(f.pointed.lock().unwrap().is_empty(), "nothing bound while waiting");
    let v = job_view(&j);
    assert_eq!(v["waiting"]["reason"], jobs::WAIT_SEW_RIGHTS);
    // The person reads which permission; the agent gets the command.
    let text = v["waiting"]["text"].as_str().unwrap();
    assert!(text.contains("Manage chat and voice") && !text.contains("k2 ") && !text.contains("chat_manage"), "{text}");
    assert_eq!(v["waiting"]["hint"], format!("k2 agent access set {WS} chat_manage on"), "{v}");
    assert!(v["waiting"].get("question").is_none(), "{v}");
    assert_plain(&v);
    // Still waiting on the next pass.
    assert_eq!(advance(&f, &id).unwrap().state, jobs::STATE_WAITING);
    // The owner turns Manage Sew on: the job goes on, to the held deploy.
    f.manage_sew.store(true, Ordering::SeqCst);
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.state.as_str(), j.step.as_str()), (jobs::STATE_WAITING, jobs::STEP_APPROVE), "{j:?}");
    assert_eq!((j.wait_reason.as_deref(), j.wait_question.as_deref()), (Some(jobs::WAIT_APPROVAL), None));
    assert_eq!(j.wait_requests, vec!["req-caps"]);
    let v = job_view(&j);
    let text = v["waiting"]["text"].as_str().unwrap();
    assert!(text.contains("Settings → App Approvals") && !text.contains("k2 "), "{text}");
    assert_eq!(v["waiting"]["hint"], "k2 app approve req-caps", "{v}");
    assert_plain(&v);
    assert_eq!(f.deploys.lock().unwrap().len(), 1, "one deploy filed the request");
    // Approved, but the replay hasn't started: pending, never a second deploy.
    f.requests.lock().unwrap().insert("req-caps".into(), "approved".into());
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.state.as_str(), j.step.as_str()), (jobs::STATE_RUNNING, jobs::STEP_APPROVE));
    assert_eq!(f.deploys.lock().unwrap().len(), 1);
    // The approval's replay deploys the Seed: the job observes it.
    f.add_dep("dep-replayed", &s.seed_hash, "live");
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_DONE, "{j:?}");
    assert_eq!(j.deployment_id.as_deref(), Some("dep-replayed"));
    assert_eq!(f.deploys.lock().unwrap().len(), 1);
    let ev = audit_events("hearth");
    assert!(ev.contains(&"suite.install.address.waiting".to_string()) && ev.contains(&"suite.install.approve.waiting".to_string()), "{ev:?}");
}

/// Box services filed beside a deploy that went out: Approve waits on that
/// request (an owner's job: no open question), and a denial ends the job.
#[test]
fn a_denied_box_service_request_ends_the_job() {
    let fx = fx();
    let (_s, c, f) = hearth(&fx);
    *f.box_held.lock().unwrap() = Some("req-box".into());
    f.requests.lock().unwrap().insert("req-box".into(), "pending".into());
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.step.as_str(), j.wait_reason.as_deref(), j.wait_question.as_deref()), (jobs::STEP_APPROVE, Some(jobs::WAIT_APPROVAL), None));
    f.requests.lock().unwrap().insert("req-box".into(), "denied".into());
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.state.as_str(), j.error_code.as_deref()), (jobs::STATE_FAILED, Some("approval_denied")));
}

// ── Meet's Sew setup against a fake mgmt API ─────────────────────────

fn meet(fx: &Fx) -> (Catalog, Fake) {
    let s = seed("meet", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"], testkit::COMMIT);
    let c = catalog_with("sew/meet", &s, testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    (c, Fake::new(write_tarball(fx, &s)))
}

fn start_meet(c: &Catalog, f: &Fake, who: &Who, extra: J) -> String {
    let mut body = json!({ "entry": "sew/meet", "workspace": WS });
    if let J::Object(m) = extra {
        for (k, v) in m {
            body[k] = v;
        }
    }
    job_id(&install_with(who, &body, c, &facts(), f).expect("the job starts"))
}

fn service_id(app: &str) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::store::get_app(&conn, app).unwrap().unwrap().service_id
}

/// A1/A2 (Sew 2026-11-04): `meet init` gets the App's service id (the
/// value the App pass's `app` claim and `members/add` use), lower-case and
/// in Sew's grammar; its `space_id` is recorded in `tool_app_spaces`; then
/// `meetings.enabled`. A second pass never calls `meet init` again.
#[test]
fn meet_setup_binds_the_meetings_space_to_the_service_id() {
    let fx = fx();
    let (c, f) = meet(&fx);
    let id = start_meet(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_DONE, "{j:?}");
    assert_eq!((j.app_id.as_str(), j.address.as_str()), ("sew-meet", "meet"));
    let svc = service_id("sew-meet");
    assert_eq!(svc, svc.to_ascii_lowercase(), "lower-case");
    assert!(sew_app_id_ok(&svc), "{svc} fits Sew's App id grammar");
    let calls = f.mgmt.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0], (MGMT_MEET_INIT.to_string(), json!({ "app_id": svc })));
    assert_eq!(calls[1], (k2_core::tool_hook::policy::MGMT_POLICIES.to_string(), json!({ "changes": { "meetings.enabled": true } })));
    assert_eq!(*f.spaces.lock().unwrap(), vec![(svc.clone(), "sp-meetings".to_string())], "the Space is recorded for the App's service id");
    assert_eq!(j.space_id.as_deref(), Some("sp-meetings"));
    // Re-running the step (a resume) records the Space again (idempotent)
    // but never sends meet init twice.
    let mut again = get(&id);
    again.step = jobs::STEP_SEW_SETUP.into();
    again.state = jobs::STATE_RUNNING.into();
    {
        let h = k2_core::db::shared();
        jobs::save(&h.lock(), &again).unwrap();
    }
    advance(&f, &id).unwrap();
    let inits = f.mgmt.lock().unwrap().iter().filter(|(p, _)| p == MGMT_MEET_INIT).count();
    assert_eq!(inits, 1);
}

/// Q13: Meet's Sew steps wait for Manage Sew like the label does.
#[test]
fn meet_setup_waits_for_manage_sew() {
    let fx = fx();
    set_switches(true, true);
    let (c, f) = meet(&fx);
    let id = start_meet(&c, &f, &agent(), json!({ "address": "team-meet" }));
    f.manage_sew.store(false, Ordering::SeqCst);
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.step.as_str(), j.wait_reason.as_deref(), j.wait_question.as_deref()), (jobs::STEP_SEW_SETUP, Some(jobs::WAIT_SEW_RIGHTS), None));
    assert_plain(&job_view(&j));
    assert_eq!(*f.attached.lock().unwrap(), vec!["team-meet"], "an ordinary label needs no Manage Sew");
    assert!(f.mgmt.lock().unwrap().is_empty(), "no Sew call while waiting");
}

/// R40.3: a Meetings Space bound to another App stops the job; it is
/// re-bound only when a person asked (`rebindMeet`), by releasing the
/// Space of the removed install this box recorded, then `meet init`.
#[test]
fn a_bound_meetings_space_stops_the_job_unless_a_person_rebinds() {
    let fx = fx();
    let (c, f) = meet(&fx);
    f.mgmt_answers.lock().unwrap().push_back(Err(MgmtError::Status(409, "{\"error\":\"space_bound\"}".into())));
    let id = start_meet(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.error_code.as_deref(), Some("meet_space_bound"), "{j:?}");
    let v = job_view(&j);
    let text = v["error"]["text"].as_str().unwrap();
    assert!(text.contains("only the owner") && !text.contains("--rebind-meet"), "{text}");
    assert_eq!(v["error"]["hint"], "k2 app install --catalog sew/meet --rebind-meet", "{v}");
    assert_plain(&v);
    // An earlier install's job recorded the Space, and its App is gone.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let mut old = jobs::get(&conn, &id).unwrap().unwrap();
        old.id = "sj-old".into();
        old.app_id = "sew-meet-old".into();
        old.space_id = Some("sp-old".into());
        old.created_at -= 100;
        jobs::insert(&conn, &old).unwrap();
        conn.execute("DELETE FROM suite_install_jobs WHERE id = ?1", rusqlite::params![id]).unwrap();
        conn.execute("DELETE FROM apps WHERE app_id = 'sew-meet'", []).unwrap();
        conn.execute("DELETE FROM published_services WHERE name = 'sew-meet'", []).unwrap();
        conn.execute("DELETE FROM suite_apps", []).unwrap();
        conn.execute("DELETE FROM suite_labels", []).unwrap();
    }
    // An agent can't ask for it.
    set_switches(true, true);
    let e = install_with(&agent(), &json!({ "entry": "sew/meet", "rebindMeet": true }), &c, &facts(), &f).unwrap_err();
    assert_eq!(e.code, k2_core::deploy::codes::FORBIDDEN);
    f.mgmt.lock().unwrap().clear();
    f.mgmt_answers.lock().unwrap().push_back(Err(MgmtError::Status(409, "{\"error\":\"space_bound\"}".into())));
    let id = start_meet(&c, &f, &Who::Owner, json!({ "rebindMeet": true }));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_DONE, "{j:?}");
    let paths: Vec<String> = f.mgmt.lock().unwrap().iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(paths, vec![MGMT_MEET_INIT, k2_core::tool_hook::sew_wire::MGMT_SPACES_RELEASE, MGMT_MEET_INIT, k2_core::tool_hook::policy::MGMT_POLICIES]);
    assert_eq!(f.mgmt.lock().unwrap()[1].1, json!({ "space_id": "sp-old", "confirm": true }));
}

// ── Ready ────────────────────────────────────────────────────────────

/// `/readyz` 503 (held box services stall) keeps the job at Ready; a
/// version other than the pin's fails it (the update didn't land).
#[test]
fn ready_waits_on_503_and_must_name_the_pinned_version() {
    let fx = fx();
    let (_s, c, f) = hearth(&fx);
    f.ready.lock().unwrap().push_back(Ok((503, json!({ "ok": false }))));
    let id = start(&c, &f, &Who::Owner, json!({}));
    let j = advance(&f, &id).unwrap();
    assert_eq!((j.state.as_str(), j.step.as_str()), (jobs::STATE_RUNNING, jobs::STEP_READY));
    f.ready.lock().unwrap().push_back(Ok((200, json!({ "ok": true, "app": "hearth", "version": "0.2.3", "variant": "k2", "commit": testkit::COMMIT }))));
    let j = advance(&f, &id).unwrap();
    assert_eq!(j.error_code.as_deref(), Some("version_mismatch"), "{j:?}");
}

// ── uninstall ────────────────────────────────────────────────────────

/// A7: uninstall releases the App's suite labels and its lineage, and a
/// job still running for it fails.
#[test]
fn forgetting_an_app_releases_its_suite_labels_and_lineage() {
    let fx = fx();
    let (_s, c, f) = hearth(&fx);
    let id = start(&c, &f, &Who::Owner, json!({}));
    assert_eq!(advance(&f, &id).unwrap().state, jobs::STATE_DONE);
    let released = forget_app("hearth");
    assert_eq!(released, vec!["chat"]);
    let h = k2_core::db::shared();
    let conn = h.lock();
    assert!(jobs::label_get(&conn, "chat").unwrap().is_none());
    assert!(jobs::lineage_get(&conn, "hearth").unwrap().is_none());
}

// ── who, kept on the row ─────────────────────────────────────────────

#[test]
fn the_caller_round_trips_without_a_token() {
    for w in [Who::Owner, Who::Login { username: "ana".into(), role: Role::Admin }, agent(), Who::Shell { project_id: WS.into(), session_id: "s".into() }] {
        let v = who_json(&w);
        assert!(!v.to_string().contains("k2_"), "no token in {v}");
        assert_eq!(format!("{:?}", who_from(&v)), format!("{w:?}"));
    }
    assert!(matches!(who_from(&json!({ "kind": "login", "role": "viewer" })), Who::Invalid));
}

// ── the download itself ──────────────────────────────────────────────

/// Sew 2026-11-04: the Seeds ship in their own release, tagged
/// `seeds-0.2.4` (no `v`). The live fetch takes them from that tag, keeps
/// the pinned bytes, and refuses other bytes under the same name (the pin
/// is the only check; nothing is unpacked).
#[test]
fn a_seed_downloads_from_its_named_release_tag_against_the_pin() {
    use crate::sew::fetch::test_server::{Reply, Scratch, Server};
    use crate::sew::fetch::{obtain, Source};
    let bytes = b"pretend this is hearth-k2-0.2.4.tar.gz".to_vec();
    let f = ReleaseFile::suite("seeds-0.2.4", "hearth-k2-0.2.4.tar.gz", &testkit::sha256_hex(&bytes), bytes.len() as u64, "Hearth 0.2.4 (k2)");
    assert_eq!(f.url(), "https://github.com/Alakazam-211/sew-releases/releases/download/seeds-0.2.4/hearth-k2-0.2.4.tar.gz");
    let core = ReleaseFile::suite("0.7.3", "sew-0.7.3-x86_64-unknown-linux-gnu.tar.gz", &"0".repeat(64), 1, "Sew 0.7.3");
    assert!(core.url().ends_with("/download/v0.7.3/sew-0.7.3-x86_64-unknown-linux-gnu.tar.gz"), "a core version keeps its v");
    let dir = Scratch::new("suite-fetch");
    let s = Server::start();
    s.serve("/seeds-0.2.4/hearth-k2-0.2.4.tar.gz", Reply::Bytes(bytes.clone()));
    let src = Source { origin: s.origin(), cache_root: Some(dir.0.join("cache")) };
    let got = obtain(&src, &f, &|_, _| {}).expect("the pinned bytes");
    assert_eq!(std::fs::read(&got.path).unwrap(), bytes);
    let s2 = Server::start();
    s2.serve("/seeds-0.2.4/hearth-k2-0.2.4.tar.gz", Reply::Bytes(b"pretend this is hearth-k2-0.2.4.tar.gx".to_vec()));
    let src2 = Source { origin: s2.origin(), cache_root: Some(dir.0.join("cache2")) };
    assert!(matches!(obtain(&src2, &f, &|_, _| {}), Err(FetchError::Mismatch(_))), "other bytes under the pin");
}
