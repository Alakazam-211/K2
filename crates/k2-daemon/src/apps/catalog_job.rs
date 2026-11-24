//! The one-click install job (0.46.9, prd-one-click-suite-apps-v1 R4/R41,
//! vs-live A5/A9): one daemon job per `k2 app install --catalog` (or the
//! Settings sheet), in this order:
//!
//! `Download → Check → Install → Address → Deploy → Approve → Sew setup →
//! Healthy → Ready`
//!
//! - **Daemon-owned.** The job is a row (`k2_core::suite::jobs`, migration
//!   0265) and one background driver thread ([`spawn_driver`]) moves every
//!   running or waiting job forward. Closing the window, quitting the app or
//!   a headless box changes nothing; a daemon restart resumes each job at
//!   the step it was on (every step is idempotent: a stored Seed is reused,
//!   an installed App is recognised by its lineage row, a bound label is a
//!   no-op, a recorded deployment is not deployed again, Sew's `meet init`
//!   answers the same Space).
//! - **Download / Check**: the Seed tarball from the pinned release only
//!   (`sew::fetch`, https to GitHub's release hosts, the pinned sha256 and
//!   size), unpacked with A13's rules, its inner `SHA256SUMS`, the manifest
//!   and `build.json` against the pin (R26, A12), validated and stored as a
//!   Seed whose tree hash must equal the pin's `seedHash`. Any difference
//!   fails the job with `catalog_mismatch` naming the field; it never tries
//!   another file.
//! - **Install** without a public label (R7: the install id isn't the
//!   address), **Address** before the first deploy (A5: Hearth and Meet
//!   refuse to boot without `K2_APP_ORIGIN`), then **Deploy** the stored
//!   Seed by hash through the normal pipeline (every hold applies).
//! - **Approve** waits (`waiting_approval`) while a held request the job
//!   filed is pending, and resumes by itself once the owner approves (A9:
//!   the approval replays the deploy; the job only observes it).
//! - **Sew setup** (Meet: `meet init` with the App's service id, the Space
//!   recorded in `tool_app_spaces`, `meetings.enabled`) and binding a suite
//!   label are Manage Sew acts today (A4). Without it the job waits
//!   (`waiting_sew_rights`).
//! - **Words.** A wait or a failure keeps one plain sentence for people
//!   (`text`); the command an agent runs (`hint`) is worked out from the
//!   code when the job is shown ([`wait_command`], [`error_command`]), so
//!   people never read a command and no internal reference ever ships.
//! - **Healthy** is the deploy's own health gate (`/healthz`); **Ready**
//!   probes the instance's `/readyz` on loopback and checks it names the
//!   app, version, variant and commit the catalog pinned (Sew's step 7).
//!
//! Every step and outcome is an event on the App's hash-chained audit
//! (`suite.install.<step>`, R35). No secret is ever in a job row, an audit
//! payload or an answer.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use k2_core::connect_users::Role;
use k2_core::deploy::{self, store};
use k2_core::suite::jobs::{self, JobRow, StepDone};
use k2_core::suite::{self, codes as sc, Pin};
use serde_json::{json, Value as J};

use super::routes::{actor, no, Refusal, Who, R};
use crate::sew::fetch::{FetchError, ReleaseFile};
use crate::sew::ops::MgmtError;
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::cli_name;

/// `POST /mgmt/v1/meet/init {app_id}` (Sew 0.3.0+): the hidden Meetings
/// Space, bound to that App. Idempotent: the same `space_id` again.
pub const MGMT_MEET_INIT: &str = "/mgmt/v1/meet/init";

/// How long a step may stay pending before the job fails.
const HEALTHY_DEADLINE: i64 = 15 * 60;
const READY_DEADLINE: i64 = 10 * 60;
const REPLAY_DEADLINE: i64 = 10 * 60;
/// The driver's pace.
const TICK: Duration = Duration::from_secs(3);

/// Failure codes the job can retry from (`k2 app catalog retry <job>`).
pub const RETRYABLE: &[&str] = &["download_failed", "sew_unreachable", "not_ready", "address_failed", "deploy_failed"];

/// One deployment, as the job reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dep {
    pub id: String,
    pub result: String,
    pub port: Option<u16>,
    pub error: Option<String>,
}

/// Everything the job does outside its own row. [`Live`] is the daemon;
/// tests hand a fake.
pub(crate) trait Hooks: Send + Sync {
    /// The pinned release file (cache or download), its local path.
    fn fetch(&self, f: &ReleaseFile) -> Result<PathBuf, FetchError>;
    /// K2 Connect Pro, E2E and the token (`nested_preflight`, live).
    fn pro_check(&self) -> Result<(), Refusal>;
    /// Point a box-level suite label at the App's front port (`None` =
    /// remove it), with the box's tunnel token.
    fn point_suite_label(&self, label: &str, port: Option<u16>) -> Result<(), String>;
    /// Attach an ordinary label before the first deploy (A5).
    fn attach_label(&self, who: &Who, app_id: &str, label: &str, front: u16) -> R<J>;
    /// `/cli/app/install` as `who`.
    fn install(&self, who: &Who, body: &J) -> R<J>;
    /// `/cli/app/deploy` as `who` (every hold applies).
    fn deploy(&self, who: &Who, body: &J) -> R<J>;
    /// A deployment: by id, else the newest of `app` to `seed`.
    fn deployment(&self, app_id: &str, id: Option<&str>, seed: &str) -> Option<Dep>;
    /// An approval request's status (`pending`, `approved`, …).
    fn request_status(&self, id: &str) -> Option<String>;
    /// Manage chat and voice for this caller (A4).
    fn may_manage_sew(&self, who: &Who) -> bool;
    /// Sew's mgmt API.
    fn sew_mgmt(&self, method: &str, path: &str, body: &J) -> Result<J, MgmtError>;
    /// Record the App's Space in `tool_app_spaces` (A2; idempotent).
    fn record_app_space(&self, service_id: &str, space_id: &str, by: &str) -> Result<(), String>;
    /// GET `http://127.0.0.1:<port><path>` → (status, JSON body).
    fn readyz(&self, port: u16, path: &str) -> Result<(u16, J), String>;
}

// ── who, kept on the row (never a token) ─────────────────────────────

pub(crate) fn who_json(w: &Who) -> J {
    match w {
        Who::Owner => json!({ "kind": "owner" }),
        Who::Login { username, role } => json!({ "kind": "login", "username": username, "role": role.as_wire() }),
        Who::Agent { project_id, session_id } => json!({ "kind": "agent", "projectId": project_id, "sessionId": session_id }),
        Who::Shell { project_id, session_id } => json!({ "kind": "shell", "projectId": project_id, "sessionId": session_id }),
        Who::Wrong | Who::Invalid => json!({ "kind": "invalid" }),
    }
}

pub(crate) fn who_from(v: &J) -> Who {
    let s = |k: &str| v.get(k).and_then(J::as_str).unwrap_or_default().to_string();
    match v.get("kind").and_then(J::as_str) {
        Some("owner") => Who::Owner,
        Some("login") => match Role::from_wire(&s("role")) {
            Some(role) => Who::Login { username: s("username"), role },
            None => Who::Invalid,
        },
        Some("agent") => Who::Agent { project_id: s("projectId"), session_id: s("sessionId") },
        Some("shell") => Who::Shell { project_id: s("projectId"), session_id: s("sessionId") },
        _ => Who::Invalid,
    }
}

/// A person (not an agent passport): the owner token, a login or a shell tab.
pub(crate) fn is_person(w: &Who) -> bool {
    matches!(w, Who::Owner | Who::Login { .. } | Who::Shell { .. })
}

// ── the live hooks ───────────────────────────────────────────────────

pub(crate) struct Live;

impl Hooks for Live {
    fn fetch(&self, f: &ReleaseFile) -> Result<PathBuf, FetchError> {
        let src = crate::sew::fetch::Source {
            origin: crate::sew::fetch::Origin::github(),
            cache_root: Some(k2_core::paths::k2_home().join("cache").join("suite")),
        };
        crate::sew::fetch::obtain(&src, f, &|_, _| {}).map(|o| o.path)
    }
    fn pro_check(&self) -> Result<(), Refusal> {
        crate::publish_runtime::nested_preflight().map(|_| ()).map_err(|e| pro_refusal(&e.message))
    }
    fn point_suite_label(&self, label: &str, port: Option<u16>) -> Result<(), String> {
        let target = port.map(|p| format!("127.0.0.1:{p}"));
        crate::sew::supervisor::live().ops.nested_label(label, target.as_deref())
    }
    fn attach_label(&self, who: &Who, app_id: &str, label: &str, front: u16) -> R<J> {
        as_caller(who, || super::domains::attach_before_first_deploy(who, app_id, label, front))
    }
    fn install(&self, who: &Who, body: &J) -> R<J> {
        let b = body.to_string();
        as_caller(who, || super::routes::install(who, b.as_bytes()))
    }
    fn deploy(&self, who: &Who, body: &J) -> R<J> {
        as_caller(who, || super::routes::deploy_inner(who, body))
    }
    fn deployment(&self, app_id: &str, id: Option<&str>, seed: &str) -> Option<Dep> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = match id {
            Some(id) => store::get_deployment(&conn, id).ok().flatten(),
            None => store::list_deployments(&conn, app_id, 20)
                .ok()?
                .into_iter()
                .filter(|d| d.to_seed.as_deref() == Some(seed) && d.kind == store::KIND_DEPLOY)
                .max_by_key(|d| d.started_at),
        }?;
        Some(Dep { id: row.id, result: row.result, port: row.port.and_then(|p| u16::try_from(p).ok()), error: row.error })
    }
    fn request_status(&self, id: &str) -> Option<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::app_grace::requests::get(&conn, id).ok().flatten().map(|r| r.status)
    }
    fn may_manage_sew(&self, who: &Who) -> bool {
        crate::sew::routes::need_manage(who).is_ok()
    }
    fn sew_mgmt(&self, method: &str, path: &str, body: &J) -> Result<J, MgmtError> {
        crate::sew::supervisor::mgmt(&crate::sew::supervisor::live(), method, path, Some(body))
    }
    fn record_app_space(&self, service_id: &str, space_id: &str, by: &str) -> Result<(), String> {
        k2_core::tool_hook::app_space_bind(service_id, space_id, by).map(|_| ())
    }
    fn readyz(&self, port: u16, path: &str) -> Result<(u16, J), String> {
        let c = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(4))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let r = c.get(format!("http://127.0.0.1:{port}{path}")).send().map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        let text = r.text().map_err(|e| e.to_string())?;
        Ok((status, serde_json::from_str(&text).unwrap_or(J::Null)))
    }
}

/// Run `f` with the label caller set for an agent (the hostname step's
/// fail-closed default otherwise), as `routes::handle` does.
fn as_caller<T>(who: &Who, f: impl FnOnce() -> T) -> T {
    if matches!(who, Who::Agent { .. }) {
        crate::label_owner::with_caller(crate::label_owner::Caller::Agent, f)
    } else {
        f()
    }
}

/// `nested_preflight`'s text → a coded refusal (A17).
pub(crate) fn pro_refusal(msg: &str) -> Refusal {
    let code = if msg.starts_with("pro_required") {
        sc::PRO_REQUIRED
    } else if msg.starts_with("e2e_off") {
        "e2e_off"
    } else if msg.starts_with("no_token") {
        "no_tunnel"
    } else if msg.contains("air-gap") || msg.contains("airgap") {
        "air_gapped"
    } else {
        "pro_check_failed"
    };
    let hint = if code == sc::PRO_REQUIRED {
        format!(
            "An address like {} needs K2 Connect Pro on this server: see {}",
            k2_core::hosts::box_host("chat.<your server>"),
            k2_core::hosts::DASHBOARD_URL
        )
    } else {
        msg.to_string()
    };
    no(code, 409, hint)
}

// ── the driver ───────────────────────────────────────────────────────

static KICK: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Wake the driver now (a new job, a retry, an approval).
pub fn kick() {
    let (m, c) = &KICK;
    *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
    c.notify_all();
}

/// The one background driver (daemon boot). Resumes every running or
/// waiting job, then moves them forward every few seconds, or at once on
/// [`kick`]. Headless: nothing here needs a client.
pub fn spawn_driver() {
    let _ = std::thread::Builder::new().name("k2-suite-jobs".into()).spawn(|| loop {
        if k2_core::db::try_shared().is_some() && deploy::enabled() {
            let any = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                jobs::active(&conn).map(|v| !v.is_empty()).unwrap_or(false)
            };
            if any {
                super::supervisor::ensure_started();
                advance_all(&Live);
            }
        }
        let (m, c) = &KICK;
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        let (mut g, _) = c.wait_timeout_while(g, TICK, |k| !*k).unwrap_or_else(|e| e.into_inner());
        *g = false;
    });
}

/// Move every running or waiting job forward, oldest first.
pub(crate) fn advance_all(h: &dyn Hooks) {
    let ids: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        jobs::active(&conn).map(|v| v.into_iter().map(|j| j.id).collect()).unwrap_or_default()
    };
    for id in ids {
        if let Err(e) = advance(h, &id) {
            k2_core::log_debug!("[suite] job {id}: {}: {}", e.code, e.hint);
        }
    }
}

struct Busy(String);

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(s) = BUSY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            s.remove(&self.0);
        }
    }
}

fn claim(id: &str) -> Option<Busy> {
    let mut g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    set.insert(id.to_string()).then(|| Busy(id.to_string()))
}

/// What one step says.
#[derive(Debug)]
enum Out {
    /// Done; the job goes to the next step (a short note for the record).
    Next(Option<String>),
    /// Not yet (a deploy starting, a probe answering 503): next tick.
    Pending,
    /// A named wait (R4): why, the held requests it watches, and what
    /// happens next in plain words (the command is [`wait_command`]'s).
    Wait { reason: &'static str, requests: Vec<String>, text: String },
    /// A failure: its code and a plain sentence (the command, when there is
    /// one, is [`error_command`]'s).
    Fail { code: String, text: String },
}

fn fail(code: &str, text: impl Into<String>) -> Out {
    Out::Fail { code: code.to_string(), text: text.into() }
}

fn load(id: &str) -> R<JobRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    jobs::get(&conn, id).map_err(super::routes::db_err)?.ok_or_else(|| no("job_not_found", 404, format!("no install job '{id}'")))
}

fn save(j: &JobRow) -> R<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    jobs::save(&conn, j).map_err(super::routes::db_err)
}

fn audit(j: &JobRow, who: &Who, event: &str, extra: J) {
    let mut p = json!({ "job": j.id, "entry": j.entry, "version": j.version, "variant": j.variant, "step": j.step });
    if let (Some(o), J::Object(m)) = (p.as_object_mut(), extra) {
        o.extend(m);
    }
    super::routes::audit_human(&j.app_id, event, who, p);
}

/// Drive one job as far as it goes now. Returns the row as it stands.
pub(crate) fn advance(h: &dyn Hooks, id: &str) -> R<JobRow> {
    let Some(_busy) = claim(id) else { return load(id) };
    loop {
        let mut j = load(id)?;
        if !j.active() {
            return Ok(j);
        }
        let pin: Pin = serde_json::from_str(&j.pin_json).map_err(|e| no("job_corrupt", 500, format!("job {id}: its pin: {e}")))?;
        let who = who_from(&serde_json::from_str(&j.who_json).unwrap_or(J::Null));
        let now = deploy::now();
        let out = run_step(h, &mut j, &pin, &who, now);
        match out {
            Out::Next(note) => {
                audit(&j, &who, &format!("suite.install.{}", j.step), json!({ "note": note, "seedHash": j.seed_hash }));
                j.steps.push(StepDone { step: j.step.clone(), at: now, note });
                j.step = jobs::next_step(&j.step).to_string();
                j.state = jobs::STATE_RUNNING.into();
                j.wait_reason = None;
                j.wait_question = None;
                j.wait_hint = None;
                j.step_started_at = now;
                j.updated_at = now;
                if j.step == jobs::STEP_DONE {
                    j.state = jobs::STATE_DONE.into();
                    j.finished_at = Some(now);
                    save(&j)?;
                    audit(&j, &who, "suite.install.done", json!({ "address": j.address }));
                    return Ok(j);
                }
                save(&j)?;
            }
            Out::Pending => {
                if j.state != jobs::STATE_RUNNING {
                    j.state = jobs::STATE_RUNNING.into();
                    j.wait_reason = None;
                    j.wait_question = None;
                    j.wait_hint = None;
                    j.updated_at = now;
                    save(&j)?;
                } else {
                    // A step may have recorded progress (the space id).
                    save(&j)?;
                }
                return Ok(j);
            }
            Out::Wait { reason, requests, text } => {
                let first = j.state != jobs::STATE_WAITING || j.wait_reason.as_deref() != Some(reason);
                j.state = jobs::STATE_WAITING.into();
                j.wait_reason = Some(reason.into());
                j.wait_question = None;
                j.wait_requests = requests;
                j.wait_hint = Some(text);
                j.updated_at = now;
                save(&j)?;
                if first {
                    audit(&j, &who, &format!("suite.install.{}.waiting", j.step), json!({ "reason": reason, "requests": j.wait_requests }));
                }
                return Ok(j);
            }
            Out::Fail { code, text } => {
                j.state = jobs::STATE_FAILED.into();
                j.error_code = Some(code.clone());
                j.error_hint = Some(text.clone());
                j.finished_at = Some(now);
                j.updated_at = now;
                save(&j)?;
                audit(&j, &who, "suite.install.failed", json!({ "code": code, "text": text }));
                return Ok(j);
            }
        }
    }
}

fn opt_str<'a>(j: &'a JobRow, k: &str) -> Option<&'a str> {
    j.options.get(k).and_then(J::as_str)
}

fn app_row(app_id: &str) -> Option<store::AppRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::get_app(&conn, app_id).ok().flatten()
}

fn front_of(app: &store::AppRow) -> Option<u16> {
    app.front_port.and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0)
}

/// The held requests a job watches: `None` = none pending (approved ones
/// are dropped); else the wait or the failure.
fn requests_gate(h: &dyn Hooks, j: &mut JobRow) -> Option<Out> {
    if j.wait_requests.is_empty() {
        return None;
    }
    let mut pending = Vec::new();
    for r in &j.wait_requests {
        match h.request_status(r).as_deref() {
            Some(k2_core::app_grace::requests::STATUS_APPROVED) => {}
            Some(k2_core::app_grace::requests::STATUS_PENDING) => pending.push(r.clone()),
            Some(other) => {
                return Some(fail("approval_denied", format!("The approval request {r} was {other}, so nothing more was deployed. To try again, deploy it again")))
            }
            None => return Some(fail("approval_missing", format!("The approval request {r} is gone, so the deploy can't go on"))),
        }
    }
    if pending.is_empty() {
        j.wait_requests.clear();
        return None;
    }
    // TODO(Q5): for verified catalog entries, may a switch-1 agent's Sew
    // core pre-approve the preset box services sew-api / sew-signal? Until
    // that is decided, the owner approves them in person (switch 2 already
    // opens them itself, A8).
    Some(Out::Wait {
        reason: jobs::WAIT_APPROVAL,
        requests: pending.clone(),
        text: format!(
            "Waiting for the owner to approve what this App uses (request {}). The owner approves it in Settings → App Approvals; the deploy goes on by itself once it's approved",
            pending.join(", ")
        ),
    })
}

fn run_step(h: &dyn Hooks, j: &mut JobRow, pin: &Pin, who: &Who, now: i64) -> Out {
    match j.step.as_str() {
        jobs::STEP_DOWNLOAD => step_download(h, j, pin),
        jobs::STEP_CHECK => step_check(j, pin),
        jobs::STEP_INSTALL => step_install(h, j, who),
        jobs::STEP_ADDRESS => step_address(h, j, who),
        jobs::STEP_DEPLOY => step_deploy(h, j, pin, who),
        jobs::STEP_APPROVE => step_approve(h, j, now),
        jobs::STEP_SEW_SETUP => step_sew_setup(h, j, pin, who),
        jobs::STEP_HEALTHY => step_healthy(h, j, now),
        jobs::STEP_READY => step_ready(h, j, pin, now),
        other => fail("job_corrupt", format!("unknown step {other}")),
    }
}

pub(crate) fn release_file(pin: &Pin) -> ReleaseFile {
    ReleaseFile::suite(&pin.tag, &pin.seed.file, &pin.seed.sha256, pin.seed.size, &format!("{} {} ({})", pin.name, pin.version, pin.variant))
}

fn step_download(h: &dyn Hooks, j: &mut JobRow, pin: &Pin) -> Out {
    let f = release_file(pin);
    match h.fetch(&f) {
        Ok(path) => {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if len != pin.seed.size {
                let _ = std::fs::remove_file(&path);
                return fail(sc::CATALOG_MISMATCH, suite::Mismatch::new("size", format!("{} is {len} bytes, the catalog pins {}", f.name, pin.seed.size)).text());
            }
            j.options["tarball"] = json!(path.to_string_lossy());
            Out::Next(Some(f.name.clone()))
        }
        Err(FetchError::Mismatch(m)) => fail(sc::CATALOG_MISMATCH, suite::Mismatch::new("sha256", m).text()),
        Err(FetchError::Refused(m)) => fail("download_refused", m),
        Err(e) => fail("download_failed", format!("Couldn't download {}: {e}. Try again in a little while", f.name)),
    }
}

fn step_check(j: &mut JobRow, pin: &Pin) -> Out {
    let Some(tarball) = opt_str(j, "tarball").map(PathBuf::from) else {
        return fail("job_corrupt", "the download's path is missing");
    };
    // The file again: nothing may change it between the steps.
    match suite::unpack::sha256_file(&tarball) {
        Ok(got) if got == pin.seed.sha256 => {}
        Ok(got) => return fail(sc::CATALOG_MISMATCH, suite::Mismatch::new("sha256", format!("{} has sha256 {got}, the catalog pins {}", pin.seed.file, pin.seed.sha256)).text()),
        Err(e) => return fail("download_failed", e),
    }
    let tmp = deploy::seeds_dir().join(format!(".tmp-suite-{}", uuid::Uuid::new_v4().simple()));
    let out = check_unpacked(&tarball, &tmp, j, pin);
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    out
}

fn check_unpacked(tarball: &std::path::Path, tmp: &std::path::Path, j: &mut JobRow, pin: &Pin) -> Out {
    if let Err(e) = suite::unpack::unpack_tar_gz(tarball, tmp, suite::unpack::Limits::seed()) {
        return fail(sc::UNPACK_REFUSED, format!("{PRODUCT_NAME} refused to unpack {}: {e}. Nothing was installed", pin.seed.file));
    }
    if let Err((f, d)) = suite::unpack::verify_sums(tmp) {
        return fail(sc::CATALOG_MISMATCH, suite::Mismatch::new("SHA256SUMS", format!("{f}: {d}")).text());
    }
    let manifest = match std::fs::read_to_string(tmp.join(deploy::manifest::MANIFEST_FILE)).map_err(|e| e.to_string()).and_then(|t| {
        deploy::manifest::parse(&t).map_err(|e| format!("{}:{}: {}", e.line, e.column, e.message))
    }) {
        Ok(m) => m,
        Err(e) => return fail(sc::CATALOG_MISMATCH, suite::Mismatch::new("manifest", e).text()),
    };
    let build = std::fs::read_to_string(tmp.join("build.json")).ok().and_then(|t| serde_json::from_str::<J>(&t).ok());
    if let Err(m) = suite::agree(pin, &manifest, build.as_ref()) {
        return fail(sc::CATALOG_MISMATCH, m.text());
    }
    let built = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        deploy::build::build_folder(&conn, tmp, &j.actor, &deploy::validate::Options { types: false, checker: None })
    };
    let built = match built {
        Ok(b) => b,
        Err(f) => return fail(f.code, format!("{PRODUCT_NAME}'s checks refused the downloaded App: {}", f.message)),
    };
    if built.seed.hash != pin.seed.seed_hash {
        return fail(
            sc::CATALOG_MISMATCH,
            suite::Mismatch::new("seedHash", format!("the stored Seed is {}, the catalog pins {}", built.seed.hash, pin.seed.seed_hash)).text(),
        );
    }
    j.seed_hash = Some(built.seed.hash.clone());
    Out::Next(Some(built.seed.hash))
}

fn step_install(h: &dyn Hooks, j: &mut JobRow, who: &Who) -> Out {
    if let Some(o) = requests_gate(h, j) {
        return o;
    }
    let lineage = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        jobs::lineage_get(&conn, &j.app_id).ok().flatten()
    };
    if let Some(app) = app_row(&j.app_id) {
        // Ours: this job's lineage, or the App an approved install replayed
        // for this job's workspace after the job started.
        let ours = lineage.as_ref().is_some_and(|l| l.job_id == j.id)
            || (lineage.is_none() && app.owner_workspace == j.workspace_id && app.created_at >= j.created_at);
        if !ours {
            return fail("app_exists", format!("An App called '{}' already exists on this server; deploy it again under another App id", j.app_id));
        }
        return record_lineage(j, "already installed");
    }
    let body = json!({ "app": j.app_id, "workspace": j.workspace_id, "tunnel": false });
    match h.install(who, &body) {
        Ok(v) if v.get("status").and_then(J::as_str) == Some("held") => {
            let rid = v["request"]["id"].as_str().unwrap_or_default().to_string();
            if rid.is_empty() {
                return fail("install_held", v["hint"].as_str().unwrap_or("the install is held for the owner").to_string());
            }
            j.wait_requests = vec![rid];
            requests_gate(h, j).unwrap_or(Out::Pending)
        }
        Ok(_) => record_lineage(j, "installed"),
        Err(r) => fail(r.code, r.hint),
    }
}

fn record_lineage(j: &mut JobRow, note: &str) -> Out {
    let l = jobs::Lineage {
        app_id: j.app_id.clone(),
        entry: j.entry.clone(),
        variant: j.variant.clone(),
        version: j.version.clone(),
        seed_hash: j.seed_hash.clone().unwrap_or_default(),
        // Q7 (follow by default) is open; recorded as the PRD recommends,
        // read by nothing in this slice.
        follow: true,
        forked: false,
        bad_versions: vec![],
        job_id: j.id.clone(),
        installed_by: j.actor.clone(),
        installed_at: deploy::now(),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match jobs::lineage_insert(&conn, &l) {
        Ok(()) => Out::Next(Some(note.into())),
        Err(e) => fail("db_error", e.to_string()),
    }
}

/// The App holding suite label `label` now (the table, and the older
/// `k2 hostchat meet bind` record for `meet`).
pub(crate) fn suite_label_holder(label: &str) -> Option<String> {
    let held = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        jobs::label_get(&conn, label).ok().flatten().map(|l| l.app_id)
    };
    held.or_else(|| (label == crate::sew::meet::MEET_LABEL).then(crate::sew::meet::bound_live).flatten().map(|(a, _)| a))
}

/// The wait for "Manage chat and voice" (binding a suite label, Sew
/// setup): the plain sentence and the command that ends it.
pub(crate) fn sew_rights_wait(what: &str, workspace: &str) -> (String, String) {
    (
        format!(
            "Setting up {what} on this server's chat needs the \"Manage chat and voice\" permission. The owner can give it to the agent that looks after this App, or deploy it from Settings → Cores → Apps → Add an App; the deploy goes on by itself then"
        ),
        format!("{CLI_NAME} agent access set {workspace} chat_manage on"),
    )
}

fn step_address(h: &dyn Hooks, j: &mut JobRow, who: &Who) -> Out {
    let Some(app) = app_row(&j.app_id) else { return fail("app_not_found", format!("App '{}' is gone", j.app_id)) };
    let Some(front) = front_of(&app) else { return fail("app_not_ready", format!("App '{}' has no front port", j.app_id)) };
    if j.address_kind == jobs::ADDRESS_SUITE_LABEL {
        if suite_label_holder(&j.address).as_deref() == Some(j.app_id.as_str()) {
            return Out::Next(Some(format!("{} already bound", j.address)));
        }
        // TODO(Q13): may the catalog job bind its own suite label (and run
        // its closed Sew steps) without Manage Sew? Until Rosson answers, a
        // caller without it waits here.
        if !h.may_manage_sew(who) {
            return Out::Wait { reason: jobs::WAIT_SEW_RIGHTS, requests: vec![], text: sew_rights_wait(&app_words(j), &j.workspace_id).0 };
        }
        if let Some(other) = suite_label_holder(&j.address) {
            return fail(sc::ADDRESS_TAKEN, format!("The address '{}' already belongs to the App {other}", j.address));
        }
        if let Err(r) = h.pro_check() {
            return fail(r.code, r.hint);
        }
        if let Err(e) = h.point_suite_label(&j.address, Some(front)) {
            return fail("address_failed", format!("The address {} couldn't be pointed at {}: {e}", k2_core::hosts::box_host(format_args!("{}.<your server>", j.address)), j.app_id));
        }
        let rec = jobs::SuiteLabel { label: j.address.clone(), app_id: j.app_id.clone(), front_port: front as i64, bound_at: deploy::now(), bound_by: j.actor.clone() };
        let bound = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            jobs::label_bind(&conn, &rec)
        };
        if let Err(holder) = bound {
            return fail(sc::ADDRESS_TAKEN, format!("The address '{}' already belongs to the App {holder}", j.address));
        }
        return Out::Next(Some(format!("{} → front port {front}", j.address)));
    }
    if super::labels::taken(&j.app_id).iter().any(|l| l == &j.address) {
        return Out::Next(Some(format!("{} already attached", j.address)));
    }
    if let Err(r) = h.pro_check() {
        return fail(r.code, r.hint);
    }
    match h.attach_label(who, &j.app_id, &j.address, front) {
        Ok(_) => Out::Next(Some(format!("{} → front port {front}", j.address))),
        Err(r) => fail(r.code, r.hint),
    }
}

fn step_deploy(h: &dyn Hooks, j: &mut JobRow, pin: &Pin, who: &Who) -> Out {
    let Some(seed) = j.seed_hash.clone() else { return fail("job_corrupt", "no stored Seed") };
    if j.deployment_id.is_some() {
        return Out::Next(Some("already deployed".into()));
    }
    if let Some(d) = h.deployment(&j.app_id, None, &seed) {
        j.deployment_id = Some(d.id.clone());
        return Out::Next(Some(format!("deployment {}", d.id)));
    }
    let body = json!({ "app": j.app_id, "seed": seed, "reason": format!("one-click install of {} {}", pin.name, pin.version) });
    match h.deploy(who, &body) {
        Ok(v) if v.get("status").and_then(J::as_str) == Some("held") => {
            // A9: the job files nothing else; Approve watches this request
            // and the approval's replay deploys.
            let rid = v["request"]["id"].as_str().unwrap_or_default().to_string();
            if rid.is_empty() {
                return fail("deploy_held", v["hint"].as_str().unwrap_or("the deploy is held for the owner").to_string());
            }
            j.wait_requests = vec![rid.clone()];
            Out::Next(Some(format!("held for approval ({rid})")))
        }
        Ok(v) => {
            let id = v["deployment"]["id"].as_str().unwrap_or_default().to_string();
            if id.is_empty() {
                return fail("deploy_failed", format!("the deploy answered no deployment: {v}"));
            }
            j.deployment_id = Some(id.clone());
            // Box services the Seed asks for, filed beside the deploy.
            let b = &v["boxServices"];
            if b["status"].as_str() == Some("held") {
                if let Some(r) = b["request"]["id"].as_str() {
                    j.wait_requests = vec![r.to_string()];
                }
            }
            Out::Next(Some(format!("deployment {id}")))
        }
        Err(r) => fail(r.code, r.hint),
    }
}

fn step_approve(h: &dyn Hooks, j: &mut JobRow, now: i64) -> Out {
    if let Some(o) = requests_gate(h, j) {
        return o;
    }
    if j.deployment_id.is_none() {
        let seed = j.seed_hash.clone().unwrap_or_default();
        match h.deployment(&j.app_id, None, &seed) {
            Some(d) => j.deployment_id = Some(d.id),
            None if now - j.step_started_at > REPLAY_DEADLINE => {
                return fail("deploy_not_replayed", "The approval went through, but no deploy of the App started after it")
            }
            None => return Out::Pending,
        }
    }
    Out::Next(Some("nothing held".into()))
}

/// Sew's App id grammar (1–64 bytes, `[a-z0-9][a-z0-9._-]*`): the service
/// id K2 also puts in the App pass's `app` claim and `members/add` (A1).
pub(crate) fn sew_app_id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

fn mgmt_fail(what: &str, e: MgmtError) -> Out {
    match e {
        MgmtError::Status(c, b) => fail("sew_refused", format!("The chat server refused {what} ({c}): {b}")),
        other => fail("sew_unreachable", format!("The chat server didn't answer {what}: {other}")),
    }
}

fn step_sew_setup(h: &dyn Hooks, j: &mut JobRow, pin: &Pin, who: &Who) -> Out {
    if pin.setup.is_empty() {
        return Out::Next(Some("no Sew setup".into()));
    }
    // TODO(Q13): may the catalog job run its closed Sew steps (meet init
    // for its own App, the Space record, meetings.enabled) without Manage
    // Sew? Until Rosson answers, a caller without it waits here.
    if !h.may_manage_sew(who) {
        return Out::Wait { reason: jobs::WAIT_SEW_RIGHTS, requests: vec![], text: sew_rights_wait(&pin.name, &j.workspace_id).0 };
    }
    let Some(app) = app_row(&j.app_id) else { return fail("app_not_found", format!("App '{}' is gone", j.app_id)) };
    let app_id = app.service_id.to_ascii_lowercase();
    if !sew_app_id_ok(&app_id) {
        return fail("sew_app_id_invalid", format!("the App's service id {app_id} is not an id Sew takes"));
    }
    for step in &pin.setup {
        match step.as_str() {
            suite::SETUP_MEET_INIT => {
                if j.space_id.is_some() {
                    continue;
                }
                match meet_init(h, j, &app_id) {
                    Ok(space) => {
                        j.space_id = Some(space);
                        if let Err(e) = save(j) {
                            return fail(e.code, e.hint);
                        }
                    }
                    Err(o) => return o,
                }
            }
            suite::SETUP_APP_SPACE_RECORD => {
                let Some(space) = j.space_id.clone() else { return fail("job_corrupt", "no Meetings Space to record") };
                if let Err(e) = h.record_app_space(&app_id, &space, &j.actor) {
                    return fail("space_record_failed", e);
                }
            }
            suite::SETUP_MEETINGS_ENABLED => {
                let body = json!({ "changes": { "meetings.enabled": true } });
                if let Err(e) = h.sew_mgmt("POST", k2_core::tool_hook::policy::MGMT_POLICIES, &body) {
                    return mgmt_fail("meetings.enabled", e);
                }
            }
            other => return fail(sc::SETUP_UNKNOWN, format!("setup step {other}")),
        }
    }
    Out::Next(Some(format!("{} step(s)", pin.setup.len())))
}

/// `meet init` for this App. A Meetings Space bound to another App stops
/// the job (never re-bound on its own, R40.3) unless a person asked to
/// rebind it (`rebindMeet`): then the Space of the removed install this box
/// knows is released in Sew first.
fn meet_init(h: &dyn Hooks, j: &JobRow, app_id: &str) -> Result<String, Out> {
    let body = json!({ "app_id": app_id });
    let first = h.sew_mgmt("POST", MGMT_MEET_INIT, &body);
    let v = match first {
        Ok(v) => v,
        Err(MgmtError::Status(409, b)) if b.contains("slug_taken") => {
            return Err(fail("meet_slug_taken", format!("Another chat space already uses the name 'meetings'; the owner decides what to do with it in the chat server (it said: {b})")))
        }
        Err(MgmtError::Status(409, b)) => {
            let rebind = j.options.get("rebindMeet").and_then(J::as_bool) == Some(true);
            let old = previous_meet_space(&j.entry, &j.id);
            match (rebind, old) {
                (true, Some(space)) => {
                    if let Err(e) = h.sew_mgmt("POST", k2_core::tool_hook::sew_wire::MGMT_SPACES_RELEASE, &json!({ "space_id": space, "confirm": true })) {
                        return Err(mgmt_fail("spaces/release", e));
                    }
                    h.sew_mgmt("POST", MGMT_MEET_INIT, &body).map_err(|e| mgmt_fail("meet init", e))?
                }
                (true, None) => {
                    return Err(fail(
                        "meet_space_bound",
                        format!("The chat server's Meetings space belongs to another App, and this server knows of no earlier Meet to release it from; release it in the chat server (it said: {b})"),
                    ))
                }
                (false, _) => {
                    return Err(fail(
                        "meet_space_bound",
                        format!(
                            "The chat server's Meetings space still belongs to a removed Meet, which keeps it read-only. {PRODUCT_NAME} never moves it on its own: only the owner can, by deploying Meet again and choosing to release it (it said: {b})"
                        ),
                    ))
                }
            }
        }
        Err(e) => return Err(mgmt_fail("meet init", e)),
    };
    v.get("space_id").and_then(J::as_str).filter(|s| !s.is_empty()).map(str::to_string).ok_or_else(|| fail("sew_refused", format!("meet init answered no space_id: {v}")))
}

/// The Meetings Space an earlier (finished) catalog install of `entry`
/// recorded, whose App is gone.
fn previous_meet_space(entry: &str, not_job: &str) -> Option<String> {
    let all = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        jobs::list(&conn, None).ok()?
    };
    all.into_iter()
        .filter(|o| o.entry == entry && o.id != not_job && o.space_id.is_some())
        .find(|o| app_row(&o.app_id).is_none())
        .and_then(|o| o.space_id)
}

fn step_healthy(h: &dyn Hooks, j: &mut JobRow, now: i64) -> Out {
    let seed = j.seed_hash.clone().unwrap_or_default();
    let Some(d) = h.deployment(&j.app_id, j.deployment_id.as_deref(), &seed) else {
        return if now - j.step_started_at > HEALTHY_DEADLINE { fail("deploy_failed", "the deployment is gone") } else { Out::Pending };
    };
    match d.result.as_str() {
        store::RESULT_LIVE => Out::Next(Some(format!("live on port {}", d.port.map(|p| p.to_string()).unwrap_or_else(|| "?".into())))),
        store::RESULT_PENDING | store::RESULT_STARTING if now - j.step_started_at <= HEALTHY_DEADLINE => Out::Pending,
        r => fail("deploy_failed", format!("The deploy ({}) ended {r}: {}. The App's logs say more", d.id, d.error.unwrap_or_default())),
    }
}

fn step_ready(h: &dyn Hooks, j: &mut JobRow, pin: &Pin, now: i64) -> Out {
    let seed = j.seed_hash.clone().unwrap_or_default();
    let late = now - j.step_started_at > READY_DEADLINE;
    let Some(port) = h.deployment(&j.app_id, j.deployment_id.as_deref(), &seed).and_then(|d| d.port) else {
        return if late { fail("not_ready", "the live deployment has no port") } else { Out::Pending };
    };
    match h.readyz(port, &pin.health.ready) {
        Ok((200, v)) => match suite::health_mismatch(pin, &v) {
            None => Out::Next(Some(format!("{} answers {}", pin.health.ready, pin.version))),
            Some(m) => fail("version_mismatch", format!("The App answered, but the version running isn't the one {PRODUCT_NAME} installed ({m})")),
        },
        Ok((code, _)) if late => fail(
            "not_ready",
            format!(
                "The App still isn't ready after {} minutes (it answers {code}): something it uses may still wait for the owner's approval, or chat hosting is down",
                READY_DEADLINE / 60
            ),
        ),
        Err(e) if late => fail("not_ready", format!("The App didn't answer whether it is ready: {e}")),
        _ => Out::Pending,
    }
}

// ── uninstall (A7) ───────────────────────────────────────────────────

/// An App going away: its suite labels are released (the nested label
/// removed, best effort, and the record dropped), the older `meet` record
/// too when it names this App, its lineage goes, and a job still running
/// for it fails. Its Meetings Space is unbound in Sew by the existing
/// `app_removed` path, because the job recorded it in `tool_app_spaces`.
pub(crate) fn forget_app(app_id: &str) -> Vec<String> {
    let labels = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        jobs::labels_of(&conn, app_id).unwrap_or_default()
    };
    let mut released = Vec::new();
    for l in labels {
        if let Err(e) = Live.point_suite_label(&l.label, None) {
            k2_core::log_debug!("[suite] uninstall {app_id}: remove {}: {e}", l.label);
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        if jobs::label_release(&conn, &l.label).unwrap_or(false) {
            released.push(l.label);
        }
    }
    if crate::sew::meet::bound_live().is_some_and(|(a, _)| a == app_id) {
        match crate::sew::meet::unbind(&crate::sew::supervisor::live()) {
            Ok(_) => released.push(crate::sew::meet::MEET_LABEL.to_string()),
            Err(e) => k2_core::log_debug!("[suite] uninstall {app_id}: meet unbind: {}", e.hint),
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = jobs::lineage_delete(&conn, app_id);
    if let Ok(list) = jobs::list(&conn, Some(app_id)) {
        for mut j in list.into_iter().filter(JobRow::active) {
            j.state = jobs::STATE_FAILED.into();
            j.error_code = Some("app_uninstalled".into());
            j.error_hint = Some(format!("App '{app_id}' was uninstalled"));
            j.finished_at = Some(deploy::now());
            j.updated_at = deploy::now();
            let _ = jobs::save(&conn, &j);
        }
    }
    released
}

// ── the view ─────────────────────────────────────────────────────────

/// One thing to do once the job is done (R5, A10): `text` for people (where
/// in Settings), `hint` the command an agent runs, and `line`, the CLI's
/// older one-line form (`next`).
pub(crate) struct NextStep {
    pub text: String,
    pub hint: String,
    pub line: String,
}

/// The App as people see it in Settings → Apps: its name, with the App id
/// when that isn't simply the name.
fn app_words(j: &JobRow) -> String {
    let name = serde_json::from_str::<Pin>(&j.pin_json).map(|p| p.name).unwrap_or_else(|_| j.app_id.clone());
    if name.eq_ignore_ascii_case(&j.app_id) {
        name
    } else {
        format!("{name} ({})", j.app_id)
    }
}

/// What a person does next once the job is done (R5, A10).
pub(crate) fn next_steps(j: &JobRow) -> Vec<NextStep> {
    let app = app_words(j);
    let mut out = vec![];
    if j.variant == suite::VARIANT_K2 {
        let cmd = format!("{CLI_NAME} app user invite {} <email>", j.app_id);
        out.push(NextStep { text: format!("Invite people to {app} from Settings → Cores → Apps → {app} → People."), line: format!("Invite people: {cmd}"), hint: cmd });
        // TODO(Q14): offer "Let this App give chat to its users"
        // (guests-chat) at deploy, and the default member role (A10). Until
        // that is decided, each person also needs chat and a role by hand.
        let cmd = format!("{CLI_NAME} app user chat {} <user> member --app", j.app_id);
        out.push(NextStep {
            text: format!("Each person also needs chat turned on in {app}, with a role that includes chat. The agent that looks after {app} can do this for you."),
            line: format!("Give each person chat: {cmd} (and a role whose room line has the chat caps)"),
            hint: cmd,
        });
    } else {
        let cmd = concat!(cli_name!(), " hostchat users invite <email>").to_string();
        out.push(NextStep {
            text: "Invite people to this server's chat: it signs people in itself. The agent that looks after chat hosting can send the invites.".into(),
            line: format!("Invite people: {cmd} (this Sew signs people in itself)"),
            hint: cmd,
        });
    }
    if j.entry == "sew/hearth" {
        // TODO(Q6): an owner-only card "Make Hearth this box's home client"
        // for deploys the owner didn't click. Today: by hand.
        let cmd = format!("{CLI_NAME} hostchat home client {}", j.app_id);
        out.push(NextStep {
            text: format!("The owner can make {app} this server's main chat app, so it shows every chat space a person is in."),
            line: format!("The owner can make it this box's home client: {cmd}"),
            hint: cmd,
        });
    }
    out
}

/// The command that ends a wait, for agents and the CLI (people read the
/// wait's `text`).
pub(crate) fn wait_command(j: &JobRow) -> Option<String> {
    match j.wait_reason.as_deref()? {
        jobs::WAIT_APPROVAL => j.wait_requests.first().map(|r| format!("{CLI_NAME} app approve {r}")),
        jobs::WAIT_SEW_RIGHTS => Some(sew_rights_wait("", &j.workspace_id).1),
        _ => None,
    }
}

/// The command that moves a failed job on, for agents and the CLI (people
/// read the error's `text` and use the Try again button).
pub(crate) fn error_command(j: &JobRow) -> Option<String> {
    let code = j.error_code.as_deref()?;
    if RETRYABLE.contains(&code) {
        return Some(format!("{CLI_NAME} app catalog retry {}", j.id));
    }
    match code {
        "approval_denied" => Some(format!("{CLI_NAME} app install --catalog {}", j.entry)),
        "approval_missing" => Some(concat!(cli_name!(), " app approvals").into()),
        "app_exists" => Some(format!("{CLI_NAME} app install --catalog {} --id <another-id>", j.entry)),
        "version_mismatch" => Some(format!("{CLI_NAME} app logs {}", j.app_id)),
        "meet_space_bound" => Some(format!("{CLI_NAME} app install --catalog {} --rebind-meet", j.entry)),
        "deploy_not_replayed" => j.seed_hash.as_ref().map(|seed| format!("{CLI_NAME} app deploy {} {seed}", j.app_id)),
        "sew_not_running" | "sew_unreachable" => Some(concat!(cli_name!(), " hostchat status").into()),
        _ => None,
    }
}

pub(crate) fn job_view(j: &JobRow) -> J {
    let sub = super::origins::box_subdomain();
    let url = (!sub.is_empty()).then(|| format!("https://{}.{sub}.{}", j.address, k2_core::tunnel::config::SUBDOMAIN_HOST));
    let steps: Vec<J> = jobs::STEPS
        .iter()
        .map(|s| {
            let done = j.steps.iter().find(|d| d.step == *s);
            let state = if done.is_some() {
                "done"
            } else if *s == j.step {
                j.state.as_str()
            } else {
                "todo"
            };
            json!({ "step": s, "state": state, "at": done.map(|d| d.at), "note": done.and_then(|d| d.note.clone()) })
        })
        .collect();
    let mut v = json!({
        "id": j.id,
        "entry": j.entry,
        "version": j.version,
        "variant": j.variant,
        "app": j.app_id,
        "workspace": j.workspace_id,
        "address": { "kind": j.address_kind, "label": j.address, "url": url },
        "actor": j.actor,
        "step": j.step,
        "state": j.state,
        "steps": steps,
        "seedHash": j.seed_hash,
        "deployment": j.deployment_id,
        "spaceId": j.space_id,
        "createdAt": j.created_at,
        "updatedAt": j.updated_at,
        "finishedAt": j.finished_at,
    });
    // Waits and failures: `text` is the plain sentence for people, `hint`
    // the command for agents (or null).
    if j.state == jobs::STATE_WAITING {
        v["waiting"] = json!({ "reason": j.wait_reason, "requests": j.wait_requests, "text": j.wait_hint, "hint": wait_command(j) });
    }
    if j.state == jobs::STATE_FAILED {
        let code = j.error_code.clone().unwrap_or_default();
        v["error"] = json!({ "code": code, "text": j.error_hint, "hint": error_command(j), "retry": RETRYABLE.contains(&code.as_str()) });
    }
    if j.state == jobs::STATE_DONE {
        let next = next_steps(j);
        // `next`: the CLI's lines, each with its command (agents read these);
        // `nextSteps`: a plain sentence for people plus the command apart.
        v["next"] = json!(next.iter().map(|n| n.line.clone()).collect::<Vec<_>>());
        v["nextSteps"] = json!(next.iter().map(|n| json!({ "text": n.text, "hint": n.hint })).collect::<Vec<_>>());
    }
    v
}

/// The actor string for a caller (the audit's).
pub(crate) fn actor_of(w: &Who) -> String {
    actor(w)
}

#[cfg(test)]
#[path = "catalog_job_tests.rs"]
pub(crate) mod tests;
