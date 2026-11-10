//! Sew's recorder on K2 (0.46.11, K-a, `prd-k2-bobbin-in-apps-v1` §4):
//! `k2 hostchat recording status|preflight|enable|disable|uninstall`.
//!
//! - **Two switches** (Sew 2026-11-10). Sew's own `[recording] enabled`
//!   (`sew recording enable`) means "installed and available"; the org
//!   policy `recording.enabled` turns the feature on for people. Enable
//!   installs when needed (stage, `sew recording install`, `sew recording
//!   enable`), then sets the policy on. Disable sets the policy off only:
//!   live recordings stop, the sidecars stay, every recording is kept.
//!   Uninstall removes the sidecars and the model; recordings stay in Sew.
//! - **Who.** Enable: an Owner or Admin, in person, with a fresh vault
//!   confirm bound to this install ([`CONFIRM_ENABLE`]). An agent, even
//!   with "Manage Chat / Voice Hosting", gets `person_only` and asks on the
//!   `sew.recording-enable` card (Admin floor); it never installs anything.
//!   Disable and uninstall: Manage Sew (they only narrow), uninstall with
//!   `confirm: true`.
//! - **Gate.** Every write is refused `sew_feature_missing` until Sew
//!   reports `app_recording` (Sew 0.9.0), so this is inert on 0.8.x; and
//!   `helper_outdated` on a root helper below protocol 15
//!   (`namespaces.sewrecording`).
//! - **Preflight** (Sew `docs/recording.md`): 8 GiB of RAM, the fit rule on
//!   a `shared` box, voice installed, disk headroom over the App disk
//!   guard's reserve, the pins, and Sew's own `sew recording plan` when it
//!   has one. A failed check names its fix; Sew's install refusals map to
//!   named fixes too ([`refusal_of`]).
//! - **Trust.** Every file is checked against the sha256 in K2's code before
//!   the helper sees it; the helper checks again, then Sew. A model that
//!   isn't the pin is a hard failure: no install, no other model, no retry
//!   (the Kessel Steward's condition R28).
//!
//! Stand-in: Sew's flag `app_recording` is [`FEATURE_APP_RECORDING`] here
//! until the K-h prep slice adds `tool_hook::features::APP_RECORDING`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use k2_root_proto::sew_recording::{self as rec, Sidecar};
use k2_root_proto::version::SEW_RECORDING_MIN_PROTOCOL;

use super::fetch::ReleaseFile;
use super::routes::{need_manage, need_person, PersonFloor, Who};
use super::supervisor::{self as sup, no, Env, Fail, R};

/// Sew 0.9.0's flag for Apps' recording and the recorder install (a
/// stand-in for `k2_core::tool_hook::features::APP_RECORDING`, K-h prep).
pub const FEATURE_APP_RECORDING: &str = "app_recording";
/// The four 0.9.0 recording flags `GET /cli/sew/recording` reports.
pub const FLAGS: [&str; 4] = ["app_recording", "voice_record_cap", "recording_required", "mgmt_recording_reads"];

pub const ROUTE: &str = "/cli/sew/recording";
pub const PREFLIGHT_ROUTE: &str = "/cli/sew/recording/preflight";
pub const ENABLE_ROUTE: &str = "/cli/sew/recording/enable";
pub const DISABLE_ROUTE: &str = "/cli/sew/recording/disable";
pub const UNINSTALL_ROUTE: &str = "/cli/sew/recording/uninstall";
pub const ROUTES: [&str; 5] = [ROUTE, PREFLIGHT_ROUTE, ENABLE_ROUTE, DISABLE_ROUTE, UNINSTALL_ROUTE];
pub const POST_ROUTES: [&str; 3] = [ENABLE_ROUTE, DISABLE_ROUTE, UNINSTALL_ROUTE];

/// The vault confirm a person's enable takes (`secretId` = this install's
/// id, or `none`); the card binds its own to the request id.
pub const CONFIRM_ENABLE: &str = "sew-recording-enable";
/// The card an agent asks on.
pub const CARD: &str = "sew.recording-enable";
/// The org policy K2 sets.
pub const POLICY_ENABLED: &str = "recording.enabled";
const POLICIES: &str = "/mgmt/v1/policies";
/// K2's record (`~/.k2/sew/recording.json`; holds no secret).
pub const RECORD: &str = "recording";
/// Free disk kept for the first recordings beyond the staged files and the
/// disk guard's reserve.
pub const RECORDINGS_HEADROOM: u64 = 2 * sup::GIB;
/// Where Sew keeps recordings (its blob store) and where the model goes.
pub const DISK_PATH: &str = "/var/lib";

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── the record ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// `sew recording install` + `enable` succeeded.
    pub installed: bool,
    /// The org policy `recording.enabled` as K2 last set it.
    pub enabled: bool,
    pub recorder_version: Option<String>,
    pub transcriber_version: Option<String>,
    pub model: Option<String>,
    pub installed_at: Option<i64>,
    /// `{step, result: running|done|error, error?, at}`.
    pub progress: Option<Value>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

pub fn load(env: &Env) -> State {
    env.ops.record_load(RECORD).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save(env: &Env, st: &mut State) {
    st.updated_at = env.ops.now();
    let v = serde_json::to_value(&*st).unwrap_or(Value::Null);
    if let Err(e) = env.ops.record_save(RECORD, Some(&v)) {
        k2_core::log_debug!("[sew] recording record not saved: {e}");
    }
}

fn update(env: &Env, f: impl FnOnce(&mut State)) {
    let mut st = load(env);
    f(&mut st);
    save(env, &mut st);
}

fn step(env: &Env, name: &str) {
    let now = env.ops.now();
    update(env, |st| st.progress = Some(json!({"step": name, "result": "running", "at": now})));
}

/// `k2 hostchat status`'s `recording` (K2's record; no helper call).
pub fn status_brief(env: &Env) -> Value {
    let st = load(env);
    json!({"installed": st.installed, "enabled": st.enabled, "version": st.recorder_version})
}

// ── gates ────────────────────────────────────────────────────────────────

/// Sew reports `app_recording`.
pub fn feature_on(env: &Env) -> bool {
    env.ops.sew_features().iter().any(|f| f == FEATURE_APP_RECORDING)
}

fn need_feature(env: &Env) -> R<()> {
    if feature_on(env) {
        return Ok(());
    }
    Err(no(
        "sew_feature_missing",
        409,
        "this server's Sew can't record yet (it needs Sew 0.9.0: app_recording). Nothing changed. Once K2 pins Sew 0.9.0, \
         k2 hostchat upgrade, then try again",
    ))
}

/// Root helper protocol 15 with `namespaces.sewrecording`.
pub fn helper_ready(env: &Env) -> bool {
    let st = env.ops.helper_state();
    st.usable(SEW_RECORDING_MIN_PROTOCOL)
        && st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/sewrecording")).and_then(Value::as_bool) == Some(true)
}

fn need_helper(env: &Env) -> R<()> {
    if helper_ready(env) {
        return Ok(());
    }
    let st = env.ops.helper_state();
    Err(no(
        "helper_outdated",
        409,
        format!(
            "{} Nothing changed: this root helper can't install Sew's recorder (it needs protocol {SEW_RECORDING_MIN_PROTOCOL}).",
            crate::root_helper::helper_needed("Recording", SEW_RECORDING_MIN_PROTOCOL, &st)
        ),
    ))
}

/// Everything a write needs that a person can't wave away.
fn gates(env: &Env) -> R<()> {
    sup::need_released(env)?;
    if !env.ops.linux() {
        return Err(no("unsupported", 409, "Chat / Voice Hosting runs on Linux boxes only"));
    }
    need_feature(env)?;
    need_helper(env)
}

/// An agent's refusal (`person_only`): its Ask-with line is the card's.
pub fn agent_refusal() -> Fail {
    no(
        "person_only",
        403,
        "nothing changed: an Owner or Admin installs Sew's recorder in person (Touch ID, the vault password or their \
         login), never an agent, even with Manage Chat / Voice Hosting. Ask on a permission card: k2 permission request \
         sew.recording-enable --why \"<why>\"",
    )
}

// ── preflight ────────────────────────────────────────────────────────────

fn check(id: &str, status: &str, detail: impl Into<String>, hint: impl Into<String>) -> Value {
    json!({"id": id, "status": status, "detail": detail.into(), "hint": hint.into()})
}

const MIB: u64 = 1024 * 1024;

fn gib(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / sup::GIB as f64)
}

/// Read-only checks (`GET /cli/sew/recording/preflight`; enable refuses on
/// any `fail`). Never changes anything.
pub fn preflight(env: &Env) -> Vec<Value> {
    let mut c = Vec::new();
    if !env.ops.linux() {
        c.push(check("os", "fail", "not Linux", "Chat / Voice Hosting runs on Linux boxes only"));
        return c;
    }
    if feature_on(env) {
        c.push(check("sew", "ok", "Sew reports app_recording", ""));
    } else {
        c.push(check("sew", "fail", "this Sew has no app_recording", "recording needs Sew 0.9.0: k2 hostchat upgrade once K2 pins it"));
    }
    if helper_ready(env) {
        c.push(check("helper", "ok", format!("k2-root-helper protocol {}", env.ops.helper_state().protocol), ""));
    } else {
        c.push(check(
            "helper",
            "fail",
            format!("k2-root-helper protocol {} without the recorder verbs", env.ops.helper_state().protocol),
            format!("update it: k2 root-helper update (recording needs protocol {SEW_RECORDING_MIN_PROTOCOL})"),
        ));
    }
    match super::store::load() {
        Some(r) if matches!(r.status.as_str(), "running" | "degraded") => c.push(check("chat", "ok", format!("Chat / Voice Hosting is {}", r.status), "")),
        Some(r) => c.push(check("chat", "fail", format!("Chat / Voice Hosting is {}", r.status), "recording needs it running: k2 hostchat enable")),
        None => c.push(check("chat", "fail", "Chat / Voice Hosting is not installed", "k2 hostchat enable --voice first")),
    }
    if super::voice::load(env).installed {
        c.push(check("voice", "ok", "voice is installed", ""));
    } else {
        c.push(check("voice", "fail", "voice is not installed", "the recorder joins calls: k2 hostchat media install first"));
    }
    let arch = env.ops.arch();
    for s in Sidecar::BOTH {
        match rec::pinned_sidecar(s, &arch) {
            Ok((v, _)) => c.push(check(s.key(), "ok", format!("{} {v}", s.member()), "")),
            Err(e) => c.push(check(s.key(), "fail", e, "this K2 build pins no recorder for this CPU")),
        }
    }
    let model = rec::pinned_asr_model();
    match &model {
        Ok(m) => c.push(check("model", "ok", format!("speech model {} ({} MB)", m.name, m.size / 1_000_000), "")),
        Err(e) => c.push(check("model", "fail", e, "update K2 once it pins Sew's speech model (k2 update)")),
    }
    let ram = env.ops.mem_total();
    match ram {
        Some(b) if b >= rec::MIN_RAM_BYTES => c.push(check("ram", "ok", format!("{} of RAM", gib(b)), "")),
        Some(b) => c.push(check(
            "ram",
            "fail",
            format!("{} of RAM", gib(b)),
            "recording needs at least 8 GiB of RAM: move Chat / Voice Hosting to a box with 8 GiB or more",
        )),
        None => c.push(check("ram", "fail", "K2 couldn't read this box's memory", "recording needs at least 8 GiB of RAM")),
    }
    c.push(fit_check(env, ram, model.as_ref().ok().copied()));
    c.push(disk_check(env, model.as_ref().ok().copied()));
    if let Some(p) = plan_check(env, &c) {
        c.push(p);
    }
    c
}

/// Sew's fit rule on a `shared` (K2) box, from the live units' MemoryMax.
fn fit_check(env: &Env, ram: Option<u64>, model: Option<&rec::SewAsrModel>) -> Value {
    let (Some(ram), Some(m)) = (ram, model) else {
        return check("fit", "info", "not checked (no RAM reading or no model pin)", "");
    };
    let units: Option<Vec<u64>> =
        [k2_root_proto::sew::UNIT_DB, k2_root_proto::sew::UNIT_SEW, k2_root_proto::sew::UNIT_MEDIA].iter().map(|u| env.ops.unit(u).memory_max).collect();
    let Some(units) = units else {
        return check("fit", "info", "K2 can't read every Sew unit's MemoryMax (is voice running?)", "Sew checks the fit itself at install");
    };
    let (fits, sum, budget) = rec::shared_fit(ram / MIB, units.iter().sum::<u64>() / MIB, m.memory_max_mib);
    let detail = format!("Sew's units with the recorder and {} need {sum} MiB; this shared box gives them {budget} MiB", m.name);
    if fits {
        check("fit", "ok", detail, "")
    } else {
        check(
            "fit",
            "fail",
            detail,
            "free memory or move to a bigger box: on a shared box 40% of RAM stays with K2's own database, and every Sew \
             unit (chat, its database, voice, the recorder, the transcriber) must fit beside it",
        )
    }
}

/// The staged files and the first recordings, over the disk guard's reserve.
fn disk_check(env: &Env, model: Option<&rec::SewAsrModel>) -> Value {
    let reserve = env.ops.disk_reserve();
    let need = model.map(|m| m.size).unwrap_or(0) + RECORDINGS_HEADROOM;
    match env.ops.disk_free(DISK_PATH) {
        Some(free) if free >= need + reserve => {
            check("disk", "ok", format!("{} free on {DISK_PATH} (the disk guard keeps {})", gib(free), gib(reserve)), "")
        }
        Some(free) => check(
            "disk",
            "fail",
            format!("{} free on {DISK_PATH}; recording needs {} beyond the disk guard's {} reserve", gib(free), gib(need), gib(reserve)),
            "free disk on /var/lib, where Sew keeps recordings: k2 app resources shows what uses it",
        ),
        None => check("disk", "warn", format!("statvfs({DISK_PATH}) failed"), ""),
    }
}

/// Sew's own read-only plan (`sew recording plan`), when the helper and
/// Sew are ready for it. A Sew without the verb is info, never a fail.
fn plan_check(env: &Env, so_far: &[Value]) -> Option<Value> {
    let ready = |id: &str| so_far.iter().any(|c| c["id"] == id && c["status"] == "ok");
    if !(ready("sew") && ready("helper") && ready("chat") && ready("model")) {
        return None;
    }
    let v = match sup::helper_json(env, &["recording", "plan"]) {
        Ok(v) => v,
        Err(e) => return Some(check("plan", "info", format!("Sew's plan didn't answer: {e}"), "Sew checks again at install")),
    };
    Some(match sup::sew_exit(&v) {
        Some(0) => check("plan", "ok", "Sew's own recording plan passes", ""),
        Some(3) => {
            let msg = sup::sew_message(&v);
            let (_, fix) = refusal_of(&msg);
            check("plan", "fail", msg, fix)
        }
        _ => check("plan", "info", "this Sew has no recording plan yet", "Sew checks again at install"),
    })
}

// ── Sew's install refusals, in Sew's order (2026-11-10) ──────────────────

/// `(needles, code, fix)`: the first row whose needles all appear in Sew's
/// message wins. Specific codes first; then Sew's check order.
pub const SEW_REFUSALS: &[(&[&str], &str, &str)] = &[
    (&["k2_managed"], "sew_k2_managed", "this Sew refused K2's call without --i-am-k2: update the root helper (k2 root-helper update), then retry"),
    (&["model_sha256_mismatch"], "model_sha256_mismatch", "the speech model isn't the one Sew pins: K2 and Sew disagree, so nothing was installed. k2 hostchat upgrade so both are the same release"),
    (&["model_does_not_fit"], "model_does_not_fit", "the speech model doesn't fit beside chat and voice on this box: free memory, or move to a box with more RAM (on a shared box 40% stays with K2's database)"),
    (&["service_account_unsafe"], "service_account_unsafe", "an existing sew-recorder or sew-transcriber account can log in: lock it (usermod -L <name>; usermod -s /usr/sbin/nologin <name>), then retry"),
    (&["voice is not installed"], "voice_not_installed", "install voice first: k2 hostchat media install, then k2 hostchat recording enable"),
    (&["recording_needs_loopback_media"], "recording_needs_loopback_media", "the voice engine must bind 0.0.0.0 or a loopback address (voice.ops.bind_v4) so the recorder can reach it: k2 hostchat media status, then k2 hostchat doctor"),
    (&["recording_unpinned_build"], "recording_unpinned_build", "this Sew build carries no recorder pins (a dev build): k2 hostchat upgrade to the Sew this K2 pins"),
    (&["tarball", "sha256"], "sidecar_sha256_mismatch", "the recorder or transcriber isn't the one this Sew pins: k2 hostchat upgrade so K2 and Sew are the same release, then retry"),
    (&["8 GiB"], "recording_needs_8gib", "recording needs at least 8 GiB of RAM: move Chat / Voice Hosting to a box with 8 GiB or more"),
    (&["not a pinned, selectable model"], "model_unknown", "this Sew doesn't offer the model K2 pins (or hasn't measured it yet): k2 hostchat upgrade so K2 and Sew are the same release"),
];

/// Sew's refusal message → K2's code and named fix.
pub fn refusal_of(message: &str) -> (&'static str, &'static str) {
    SEW_REFUSALS
        .iter()
        .find(|(needles, _, _)| needles.iter().all(|n| message.contains(n)))
        .map(|(_, code, fix)| (*code, *fix))
        .unwrap_or(("recording_install_failed", "k2 hostchat doctor, and the journal: journalctl -u sew-recorder -u sew-transcriber -e"))
}

fn sew_refused(v: &Value, what: &str) -> Fail {
    let msg = sup::sew_message(v);
    let (code, fix) = refusal_of(&msg);
    let mut f = no(code, 409, format!("{what}: Sew said: {msg}. {fix}"));
    f.extra = Some(json!({"sewExit": sup::sew_exit(v), "step": v["step"]}));
    f
}

// ── the files ────────────────────────────────────────────────────────────

/// Optional local files for an offline box (each absolute); else K2's
/// cache or Sew's release download.
#[derive(Debug, Clone, Default)]
pub struct Files {
    pub recorder: Option<PathBuf>,
    pub transcriber: Option<PathBuf>,
    pub model: Option<PathBuf>,
}

fn sidecar_file(which: Sidecar, arch: &str) -> R<ReleaseFile> {
    let (v, a) = rec::pinned_sidecar(which, arch).map_err(|e| no("recording_unpinned", 409, e))?;
    Ok(ReleaseFile {
        tag: k2_root_proto::sew::pinned().version.into(),
        name: format!("{}-{v}-{}.tar.gz", which.member(), a.triple),
        sha256: a.tarball_sha256.into(),
        cap: k2_root_proto::sew::MAX_TARBALL as u64,
        label: format!("{} {v}", which.member()),
    })
}

fn model_file(m: &rec::SewAsrModel) -> ReleaseFile {
    ReleaseFile {
        tag: m.sew_version.into(),
        name: m.file.into(),
        sha256: m.sha256.into(),
        cap: m.size,
        label: format!("Sew's speech model {}", m.name),
    }
}

/// One of the model's licence files, a release asset beside it.
fn licence_file(m: &rec::SewAsrModel, l: &rec::SewAsrLicence) -> ReleaseFile {
    ReleaseFile {
        tag: m.sew_version.into(),
        name: l.file.into(),
        sha256: l.sha256.into(),
        cap: l.size,
        label: format!("the speech model's licence file {}", l.file),
    }
}

/// The bytes of one pinned file, checked here against the sha256 in K2's
/// code before the helper ever sees them.
fn obtain(env: &Env, f: &ReleaseFile, path: Option<&Path>, flag: &str, code: &'static str) -> R<Vec<u8>> {
    let fetched;
    let p = match path {
        Some(p) => p,
        None => {
            let note = |_: Value| {};
            fetched = sup::obtain_release(env, f, &format!("k2 hostchat recording enable {flag}"), &note)
                .map_err(|e| no("recording_file_needed", 409, e))?;
            fetched.as_path()
        }
    };
    let bytes = env.ops.read_file(p, f.cap as usize).map_err(|e| no("bad_request", 400, e))?;
    if !crate::update_routes::verify_sha256(&bytes, &f.sha256) {
        return Err(no(code, 409, format!("{} is not {} (sha256 {}): nothing was installed", p.display(), f.label, f.sha256)));
    }
    Ok(bytes)
}

/// Every file checked, then staged. The model is a hard failure on a
/// mismatch: no install, no other model, no retry.
fn stage(env: &Env, files: &Files) -> R<&'static rec::SewAsrModel> {
    let arch = env.ops.arch();
    let model = rec::pinned_asr_model().map_err(|e| no("recording_model_unpinned", 409, e))?;
    let rf = sidecar_file(Sidecar::Recorder, &arch)?;
    let tf = sidecar_file(Sidecar::Transcriber, &arch)?;
    let m = obtain(env, &model_file(model), files.model.as_deref(), "--model-file", "model_sha256_mismatch")?;
    // Its licence texts and NOTICE: with --model-file, from the same folder
    // (Sew publishes them beside the model); else downloaded. Each is a
    // hard sha256 check, as the model is.
    let mut licences = Vec::new();
    for l in model.licences {
        let beside = files.model.as_deref().and_then(Path::parent).map(|d| d.join(l.file));
        let b = obtain(env, &licence_file(model, l), beside.as_deref(), "--model-file (its licence files beside it)", "model_sha256_mismatch")?;
        licences.push((l.file, b));
    }
    let r = obtain(env, &rf, files.recorder.as_deref(), "--recorder-tarball", "sha256_mismatch")?;
    let t = obtain(env, &tf, files.transcriber.as_deref(), "--transcriber-tarball", "sha256_mismatch")?;
    for (what, bytes) in [("sew-recorder", &r), ("sew-transcriber", &t), ("speech model", &m)] {
        sup::helper_stdin(env, &["recording", "stage"], bytes).map_err(|e| no("helper_error", 500, format!("staging {what}: {e}")))?;
    }
    for (file, bytes) in &licences {
        sup::helper_stdin(env, &["recording", "stage"], bytes).map_err(|e| no("helper_error", 500, format!("staging {file}: {e}")))?;
    }
    Ok(model)
}

/// Sew's recording status through the helper (`None` when it can't say).
fn live(env: &Env) -> Option<Value> {
    if !helper_ready(env) || super::store::load().is_none() {
        return None;
    }
    sup::helper_json(env, &["recording", "status"]).ok().filter(|v| sup::sew_exit(v) == Some(0))
}

fn version_of(live: &Value, s: Sidecar) -> Option<String> {
    live.pointer(&format!("/versions/{}/version", s.key())).and_then(Value::as_str).map(str::to_string)
}

fn set_policy(env: &Env, on: bool) -> R<()> {
    sup::mgmt(env, "POST", POLICIES, Some(&json!({"changes": {POLICY_ENABLED: on}})))
        .map(|_| ())
        .map_err(|e| no("policy_failed", 502, format!("Sew didn't take {POLICY_ENABLED} = {on}: {e}")))
}

fn policies(env: &Env) -> Value {
    match sup::mgmt(env, "GET", POLICIES, None) {
        Ok(v) => {
            let all = v.get("policies").and_then(Value::as_object).cloned().unwrap_or_default();
            Value::Object(all.into_iter().filter(|(k, _)| k.starts_with("recording.")).collect())
        }
        Err(_) => Value::Null,
    }
}

// ── the verbs ────────────────────────────────────────────────────────────

/// `GET /cli/sew/recording`: installed, version, org policies, flags.
pub fn status(env: &Env) -> Value {
    let st = load(env);
    let features = env.ops.sew_features();
    let flags: serde_json::Map<String, Value> = FLAGS.iter().map(|f| (f.to_string(), json!(features.iter().any(|x| x == f)))).collect();
    let live = live(env);
    let pol = if live.is_some() || st.installed { policies(env) } else { Value::Null };
    let installed = live.as_ref().and_then(|l| l.pointer("/sew/installed")).and_then(Value::as_bool).unwrap_or(st.installed);
    let enabled = pol.get(POLICY_ENABLED).and_then(Value::as_bool).unwrap_or(st.enabled);
    let version = |s: Sidecar| live.as_ref().and_then(|l| version_of(l, s)).or(match s {
        Sidecar::Recorder => st.recorder_version.clone(),
        Sidecar::Transcriber => st.transcriber_version.clone(),
    });
    let ready = match gates(env) {
        Ok(()) => json!({"ok": true}),
        Err(f) => json!({"ok": false, "code": f.code, "hint": f.hint}),
    };
    json!({
        "ok": true,
        "installed": installed,
        "enabled": enabled,
        "version": {"recorder": version(Sidecar::Recorder), "transcriber": version(Sidecar::Transcriber)},
        "model": live.as_ref().and_then(|l| l.pointer("/sew/model").cloned()).filter(|m| !m.is_null()).or(st.model.clone().map(Value::from)),
        "units": live.as_ref().map(|l| json!({"recorder": l.pointer("/sew/recorder"), "transcriber": l.pointer("/sew/transcriber")})),
        "policies": pol,
        "flags": flags,
        "helper": {"protocol": env.ops.helper_state().protocol, "recording": helper_ready(env)},
        "ready": ready,
        "progress": st.progress,
        "lastError": st.last_error,
        "rules": "recording needs 8 GiB of RAM; disable keeps the recorder and every recording; uninstall removes the recorder and never deletes recordings",
    })
}

/// The enable machine: install when needed (stage, `sew recording install`
/// + `enable`), then the org policy on.
pub fn enable_machine(env: &Env, files: &Files) -> R<Value> {
    let r = enable_steps(env, files);
    let now = env.ops.now();
    match &r {
        Ok(_) => {
            update(env, |st| {
                st.progress = Some(json!({"step": null, "result": "done", "at": now}));
                st.last_error = None;
            });
            sup::changed("recording-enabled");
        }
        Err(f) => {
            let msg = format!("{}: {}", f.code, f.hint);
            update(env, |st| {
                st.progress = Some(json!({"step": st.progress.as_ref().and_then(|p| p.get("step").cloned()), "result": "error", "error": msg, "at": now}));
                st.last_error = Some(msg.clone());
            });
            sup::changed("recording-failed");
        }
    }
    r
}

fn enable_steps(env: &Env, files: &Files) -> R<Value> {
    let arch = env.ops.arch();
    let (pin_v, _) = rec::pinned_sidecar(Sidecar::Recorder, &arch).map_err(|e| no("recording_unpinned", 409, e))?;
    let current = live(env);
    let installed = current.as_ref().is_some_and(|l| {
        l.pointer("/sew/installed").and_then(Value::as_bool) == Some(true) && version_of(l, Sidecar::Recorder).as_deref() == Some(pin_v)
    });
    let mut model = current.as_ref().and_then(|l| l.pointer("/sew/model")).and_then(Value::as_str).map(str::to_string);
    if !installed {
        step(env, "stage");
        let m = stage(env, files)?;
        step(env, "install");
        let body = json!({"model": m.name}).to_string();
        let v = sup::helper_stdin(env, &["recording", "install"], body.as_bytes()).map_err(|e| no("helper_error", 500, e))?;
        if sup::sew_exit(&v) != Some(0) {
            return Err(sew_refused(&v, "the recorder wasn't installed"));
        }
        model = Some(m.name.to_string());
    }
    step(env, "policy");
    set_policy(env, true)?;
    let after = live(env);
    let now = env.ops.now();
    update(env, |st| {
        st.installed = true;
        st.enabled = true;
        st.recorder_version = after.as_ref().and_then(|l| version_of(l, Sidecar::Recorder)).or(Some(pin_v.into()));
        st.transcriber_version = after.as_ref().and_then(|l| version_of(l, Sidecar::Transcriber)).or(st.transcriber_version.clone());
        st.model = model.clone();
        st.installed_at = st.installed_at.or(Some(now));
    });
    Ok(json!({
        "ok": true,
        "state": "enabled",
        "installed": true,
        "enabled": true,
        "alreadyInstalled": installed,
        "version": pin_v,
        "model": model,
        "next": "Apps and K5 rooms still choose who may record (Settings → Chat / Voice Hosting); each person consents for themselves",
    }))
}

/// `POST /cli/sew/recording/disable`: the org policy off. The sidecars stay
/// and every recording is kept.
pub fn disable(env: &Env) -> R<Value> {
    gates(env)?;
    set_policy(env, false)?;
    update(env, |st| st.enabled = false);
    sup::changed("recording-disabled");
    Ok(json!({
        "ok": true,
        "enabled": false,
        "installed": load(env).installed,
        "kept": "the recorder stays installed and every recording is kept; live recordings stopped",
    }))
}

/// `POST /cli/sew/recording/uninstall {confirm: true}`: the policy off,
/// then `sew recording uninstall`. Recordings stay in Sew.
pub fn uninstall(env: &Env) -> R<Value> {
    gates(env)?;
    let mut warnings = Vec::new();
    if let Err(f) = set_policy(env, false) {
        warnings.push(f.hint);
    }
    let v = sup::helper_json(env, &["recording", "uninstall"]).map_err(|e| no("helper_error", 500, e))?;
    if sup::sew_exit(&v) != Some(0) {
        return Err(sew_refused(&v, "the recorder wasn't removed"));
    }
    update(env, |st| {
        st.installed = false;
        st.enabled = false;
        st.recorder_version = None;
        st.transcriber_version = None;
        st.model = None;
    });
    sup::changed("recording-uninstalled");
    Ok(json!({
        "ok": true,
        "installed": false,
        "enabled": false,
        "kept": "every recording stays in Sew (k2 hostchat recording enable puts the recorder back)",
        "warnings": warnings,
    }))
}

/// After a Sew upgrade on a box with the recorder (Sew 2026-11-10): the
/// new release's sidecars, `sew recording upgrade --when-idle`. `Ok(None)`:
/// nothing to do. A Sew without the verb is a note, never a failed upgrade
/// (`sew doctor` flags the mismatch).
pub fn after_upgrade(env: &Env) -> Result<Option<String>, String> {
    if !load(env).installed || !helper_ready(env) {
        return Ok(None);
    }
    let arch = env.ops.arch();
    let mut staged = Vec::new();
    for s in Sidecar::BOTH {
        let f = sidecar_file(s, &arch).map_err(|f| f.hint)?;
        let b = obtain(env, &f, None, "", "sha256_mismatch").map_err(|f| f.hint)?;
        staged.push(b);
    }
    for b in &staged {
        sup::helper_stdin(env, &["recording", "stage"], b)?;
    }
    let v = sup::helper_json(env, &["recording", "upgrade"])?;
    match sup::sew_exit(&v) {
        Some(0) => {
            let after = live(env);
            update(env, |st| {
                st.recorder_version = after.as_ref().and_then(|l| version_of(l, Sidecar::Recorder));
                st.transcriber_version = after.as_ref().and_then(|l| version_of(l, Sidecar::Transcriber));
            });
            Ok(Some("the recorder and transcriber are upgraded (when idle)".into()))
        }
        Some(2) => Ok(Some("this Sew has no recording upgrade yet: the recorder stays on its version; k2 hostchat doctor shows it".into())),
        _ => Err(format!("the recorder wasn't upgraded: {}", sup::sew_message(&v))),
    }
}

// ── routes ───────────────────────────────────────────────────────────────

fn abs(v: &Value, k: &str) -> R<Option<PathBuf>> {
    match v.get(k).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()) {
        None => Ok(None),
        Some(p) if p.starts_with('/') && !p.split('/').any(|seg| seg == "..") => Ok(Some(PathBuf::from(p))),
        Some(_) => Err(no("bad_request", 400, format!("{k} must be an absolute path"))),
    }
}

fn need_credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(no("forbidden", 403, "invalid or missing token")),
        Who::Wrong => Err(no("wrong_credential", 403, "App passes, API keys and API sessions can't manage Chat / Voice Hosting")),
        _ => Ok(()),
    }
}

/// `/cli/sew/recording*`. `background = false` runs enable inline (tests).
pub fn route(env: Env<'static>, path: &str, who: &Who, v: &Value, background: bool) -> R<Value> {
    match path {
        ROUTE => {
            need_credential(who)?;
            Ok(status(&env))
        }
        PREFLIGHT_ROUTE => {
            need_credential(who)?;
            let checks = preflight(&env);
            let ready = !checks.iter().any(|c| c["status"] == "fail");
            Ok(json!({"ok": true, "ready": ready, "checks": checks}))
        }
        ENABLE_ROUTE => {
            // An agent never installs, Manage Sew or not: the card.
            if who.passport().is_some() {
                need_manage(who)?;
                return Err(agent_refusal());
            }
            need_manage(who)?;
            gates(&env)?;
            let files = Files { recorder: abs(v, "recorderTarballPath")?, transcriber: abs(v, "transcriberTarballPath")?, model: abs(v, "modelFilePath")? };
            let audit = |outcome: &str| crate::vault::auth_audit(CONFIRM_ENABLE, &who.label(), outcome, "cli");
            let method = match need_person(who, v, CONFIRM_ENABLE, &super::routes::confirm_secret_id(), PersonFloor::Admin) {
                Ok(m) => m,
                Err(f) => {
                    audit(&format!("refused {}", f.code));
                    return Err(f);
                }
            };
            audit(&format!("confirmed via {method}"));
            let latch = sup::begin(&env)?;
            let checks = preflight(&env);
            if checks.iter().any(|c| c["status"] == "fail") {
                let why: Vec<String> = checks
                    .iter()
                    .filter(|c| c["status"] == "fail")
                    .map(|c| format!("{}. {}", c["detail"].as_str().unwrap_or(""), c["hint"].as_str().unwrap_or("")))
                    .collect();
                let mut f = no("preflight_failed", 409, format!("the box isn't ready for recording; nothing was changed. {}", why.join(" ")));
                f.extra = Some(json!({"report": checks}));
                return Err(f);
            }
            if background {
                std::thread::Builder::new()
                    .name("sew-recording".into())
                    .spawn(move || {
                        let _l = latch;
                        if let Err(f) = enable_machine(&env, &files) {
                            k2_core::log_debug!("[sew] recording enable: {}", f.code);
                        }
                    })
                    .map_err(|e| no("spawn", 500, e.to_string()))?;
                return Ok(json!({"ok": true, "state": "installing", "confirmedBy": method, "hint": "poll GET /cli/sew/recording (progress)"}));
            }
            let mut r = enable_machine(&env, &files)?;
            drop(latch);
            r["confirmedBy"] = json!(method);
            Ok(r)
        }
        DISABLE_ROUTE => {
            need_manage(who)?;
            disable(&env)
        }
        UNINSTALL_ROUTE => {
            need_manage(who)?;
            if v.get("confirm").and_then(Value::as_bool) != Some(true) {
                return Err(no(
                    "confirm_required",
                    409,
                    "Remove Sew's recorder? Live recordings stop; every recording is kept. Send {\"confirm\": true} \
                     (k2 hostchat recording uninstall --yes)",
                ));
            }
            let _latch = sup::begin(&env)?;
            uninstall(&env)
        }
        _ => Err(no("not_found", 404, "no such Sew route")),
    }
}

#[cfg(all(test, unix))]
#[path = "recording_tests.rs"]
mod tests;
