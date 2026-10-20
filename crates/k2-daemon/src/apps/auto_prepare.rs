//! Automatic App box prep (0.46.3, Rosson 2026-10-20: "I should be able to
//! turn on a permissions toggle to let him use apps/runtimes"): a Linux box
//! with the owner's root helper gets ready for walled Apps by itself after
//! an update — K2's Podman bundle where the distro's Podman is < 5, the
//! k2app user and its sub-ids, delegation, linger, the nft table, the
//! AppArmor profile (exactly `box_prep::plan`, the owner's `k2 app box
//! prepare`), then the App engine (`engine_install`) — so an agent with
//! "Build and run Apps" can install and deploy with nobody running sudo.
//!
//! **Consent.** The owner installed the root helper (that is what lets K2
//! run root steps at all; the helper's own allowlist bounds them) and the
//! owner-only setting `appsAutoPrepare` is on (`K2_APPS_AUTO_PREPARE`
//! overrides; default `deploy::AUTO_PREPARE_DEFAULT`, OFF until the design
//! `design-apps-auto-prepare-0463` is signed off). Box prep never needed a
//! vault confirm (it is not on the AR43 person-present list: `may_prepare`
//! is an owner-caller check, and the daemon itself is the caller here).
//!
//! **Never breaks the box.** Before any change it reads `/etc/subuid` and
//! `/etc/subgid` (world-readable) and stops on any overlap between owners
//! (rootless Docker under the daemon user, a hand-made range): the doctor
//! and the owner's note say which. A box that is already ready is left
//! alone (no root call). macOS and other OSes: nothing.
//!
//! **When.** One thread from boot (= after every update), ~3 minutes in
//! (after the root helper's own heal), then daily; a failure retries after
//! 1 h, doubling to 24 h. Every outcome is in the doctor
//! (`GET /cli/app/box` → `autoPrepare`) and Settings → Apps → This server;
//! a run that changed the box files one owner notice (what was installed,
//! how to undo it).

use std::sync::Mutex;

use serde_json::{json, Value as J};

/// The helper protocol the whole prep needs (K2's Podman bundle install is
/// protocol 8; the rest is 6).
pub const MIN_PROTOCOL: u32 = k2_root_proto::version::PODMAN_INSTALL_MIN_PROTOCOL;
/// Who the audit and the notice name.
pub const ACTOR: &str = "k2:auto-prepare";

/// What one run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not run: `code` (`not_linux`, `apps_off`, `setting_off`,
    /// `no_helper`, `helper_old`, `subid_overlap`, `airgap`) and why.
    Skipped { code: &'static str, why: String },
    /// The box was ready already: nothing changed.
    Ready,
    /// Prepared: what changed, in order.
    Prepared { changed: Vec<String> },
    /// A step failed; the box is where the doctor says.
    Failed { step: String, error: String },
}

impl Outcome {
    pub fn json(&self) -> J {
        match self {
            Outcome::Skipped { code, why } => json!({ "state": "skipped", "code": code, "why": why }),
            Outcome::Ready => json!({ "state": "ready" }),
            Outcome::Prepared { changed } => json!({ "state": "prepared", "changed": changed }),
            Outcome::Failed { step, error } => json!({ "state": "failed", "step": step, "error": error }),
        }
    }
}

/// The box as the run sees it (tests drive a fake).
pub trait Ops {
    fn linux(&self) -> bool;
    fn apps_on(&self) -> bool;
    fn enabled(&self) -> bool;
    /// `None` = no root helper installed (or not root-owned); else its
    /// protocol.
    fn helper_protocol(&self) -> Option<u32>;
    /// `GET /cli/app/box`'s doctor (`ready`, `checks`).
    fn doctor(&self) -> J;
    /// `/etc/subuid`, `/etc/subgid` (empty when missing).
    fn subid_files(&self) -> (String, String);
    fn airgap(&self) -> bool;
    /// The owner's prepare, with K2's Podman when the box needs it.
    fn prepare(&self) -> Result<J, String>;
    fn install_engine(&self) -> Result<J, String>;
    fn note_owner(&self, title: &str, body: &str);
}

// ── sub-id overlaps ──────────────────────────────────────────────────

fn parse_subid(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split(':').collect();
            if f.len() != 3 || f[0].starts_with('#') {
                return None;
            }
            let start: u64 = f[1].trim().parse().ok()?;
            let count: u64 = f[2].trim().parse().ok()?;
            (count > 0).then(|| (f[0].trim().to_string(), start, count))
        })
        .collect()
}

/// Two owners' ranges that overlap in one file, named (`None` = none).
/// The prep's own `apps subid` allocates above every range; an overlap
/// that is already there (rootless Docker's range across someone else's)
/// means the box's sub-ids are hand-made and K2 stays out.
pub fn subid_conflict(subuid: &str, subgid: &str) -> Option<String> {
    for (file, text) in [("/etc/subuid", subuid), ("/etc/subgid", subgid)] {
        let rows = parse_subid(text);
        for (i, a) in rows.iter().enumerate() {
            for b in rows.iter().skip(i + 1) {
                if a.0 != b.0 && a.1 < b.1 + b.2 && b.1 < a.1 + a.2 {
                    return Some(format!(
                        "{file}: {}'s range {}+{} overlaps {}'s {}+{}",
                        a.0, a.1, a.2, b.0, b.1, b.2
                    ));
                }
            }
        }
    }
    None
}

// ── one run ──────────────────────────────────────────────────────────

fn needs(doctor: &J) -> (bool, bool) {
    let checks = doctor["checks"].as_array().cloned().unwrap_or_default();
    let not_ok = |c: &J| c["status"].as_str() != Some("ok");
    let prepare = checks
        .iter()
        .any(|c| not_ok(c) && c["fix"].as_str().is_some_and(|f| f.starts_with("k2 app box prepare")));
    let engine = checks.iter().any(|c| c["id"] == "engine" && not_ok(c));
    (prepare, engine)
}

/// One run: decide, then prepare and install the engine where needed.
pub fn run_once(ops: &dyn Ops) -> Outcome {
    let skip = |code: &'static str, why: String| Outcome::Skipped { code, why };
    if !ops.linux() {
        return skip("not_linux", "walled Apps run on Linux boxes only".into());
    }
    if !ops.apps_on() {
        return skip("apps_off", "Apps (beta) are off on this server".into());
    }
    if !ops.enabled() {
        return skip(
            "setting_off",
            "the owner's \"Prepare this box automatically\" (appsAutoPrepare) is off".into(),
        );
    }
    match ops.helper_protocol() {
        None => {
            return skip(
                "no_helper",
                "K2's root helper isn't installed: the owner installs it once (Settings → K2 Server), or prepares the box by hand".into(),
            )
        }
        Some(p) if p < MIN_PROTOCOL => {
            return skip(
                "helper_old",
                format!("the root helper speaks protocol {p}; automatic prep needs {MIN_PROTOCOL} (it updates itself after a K2 update)"),
            )
        }
        Some(_) => {}
    }
    let doctor = ops.doctor();
    let (prepare, engine) = needs(&doctor);
    if !prepare && !engine {
        return Outcome::Ready;
    }
    let (subuid, subgid) = ops.subid_files();
    if let Some(c) = subid_conflict(&subuid, &subgid) {
        return skip(
            "subid_overlap",
            format!("{c}. K2 changes nothing on a box whose sub-ids already overlap (rootless Docker, a hand-made range): fix them, then prepare"),
        );
    }
    if ops.airgap() {
        return skip(
            "airgap",
            "air-gapped: K2 can't fetch its Podman or the App engine; the owner prepares with k2 app box prepare --podman-from and k2 app engine install --from".into(),
        );
    }
    let mut changed: Vec<String> = Vec::new();
    if prepare {
        match ops.prepare() {
            Ok(v) => {
                if v["podman"]["action"] == "installed" {
                    changed.push(format!(
                        "installed K2's Podman {} in {} for the k2app user (k2 app box podman uninstall removes it)",
                        v["podman"]["version"].as_str().unwrap_or("?"),
                        v["podman"]["prefix"].as_str().unwrap_or("/opt/k2/podman")
                    ));
                }
                for s in v["steps"].as_array().cloned().unwrap_or_default() {
                    let verb = s["verb"].as_str().unwrap_or("").to_string();
                    if s["result"]["changed"] != json!(false) && verb != "apps podman install" {
                        changed.push(verb);
                    }
                }
            }
            Err(e) => {
                let step = e.split_once("box prep stopped at `").and_then(|(_, r)| r.split_once('`')).map(|(s, _)| s.to_string());
                return Outcome::Failed { step: step.unwrap_or_else(|| "box prepare".into()), error: e };
            }
        }
    }
    // The engine after the box: prepare may have just made its user.
    let (_, engine_now) = if prepare { needs(&ops.doctor()) } else { (false, engine) };
    if engine_now {
        match ops.install_engine() {
            Ok(v) => changed.push(format!(
                "installed the App engine {} (k2-app-engine, signed for this K2 version)",
                v["engine"]["version"].as_str().or(v["checked"]["version"].as_str()).unwrap_or("")
            )),
            Err(e) => return Outcome::Failed { step: "apps install-engine".into(), error: e },
        }
    }
    if !changed.is_empty() {
        ops.note_owner(
            "K2 prepared this server for Apps",
            &format!(
                "\"Prepare this box automatically\" is on, so after the update K2 set this server up for walled Apps through its \
                 root helper:\n\n- {}\n\nUndo: k2 app box podman uninstall (K2's Podman); the rest is K2's own k2app user and its \
                 firewall table, which Apps need. Turn automatic prep off in Settings → Apps → This server (appsAutoPrepare).",
                changed.join("\n- ")
            ),
        );
    }
    Outcome::Prepared { changed }
}

// ── status ───────────────────────────────────────────────────────────

struct Status {
    last_run: i64,
    outcome: Outcome,
    next_attempt: Option<i64>,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);

/// What the doctor and Settings show.
pub fn status_json() -> J {
    let s = STATUS.lock().unwrap_or_else(|p| p.into_inner());
    let mut out = json!({
        "enabled": k2_core::deploy::auto_prepare_enabled(),
        "default": k2_core::deploy::AUTO_PREPARE_DEFAULT,
        "setting": "appsAutoPrepare",
    });
    if let Some(st) = s.as_ref() {
        out["lastRun"] = json!(st.last_run);
        out["outcome"] = st.outcome.json();
        out["nextAttempt"] = json!(st.next_attempt);
    }
    out
}

fn record(o: &Outcome, next: Option<i64>) {
    *STATUS.lock().unwrap_or_else(|p| p.into_inner()) = Some(Status { last_run: k2_core::deploy::now(), outcome: o.clone(), next_attempt: next });
}

/// Seconds until the next run after `o`; `backoff` is the failure backoff
/// (1 h, doubling to 24 h).
pub fn next_wait(o: &Outcome, backoff: &mut u64) -> u64 {
    match o {
        Outcome::Failed { .. } => {
            let w = *backoff;
            *backoff = (*backoff * 2).min(86_400);
            w
        }
        Outcome::Skipped { code: "helper_old", .. } => {
            *backoff = 3600;
            6 * 3600
        }
        _ => {
            *backoff = 3600;
            86_400
        }
    }
}

// ── the live box ─────────────────────────────────────────────────────

struct LiveOps;

impl Ops for LiveOps {
    fn linux(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn apps_on(&self) -> bool {
        k2_core::deploy::enabled()
    }
    fn enabled(&self) -> bool {
        k2_core::deploy::auto_prepare_enabled()
    }
    fn helper_protocol(&self) -> Option<u32> {
        let st = crate::root_helper::state();
        st.usable(0).then_some(st.protocol)
    }
    fn doctor(&self) -> J {
        super::box_prep::doctor(&super::box_prep::gather())
    }
    fn subid_files(&self) -> (String, String) {
        (std::fs::read_to_string("/etc/subuid").unwrap_or_default(), std::fs::read_to_string("/etc/subgid").unwrap_or_default())
    }
    fn airgap(&self) -> bool {
        k2_core::airgap::enabled()
    }
    fn prepare(&self) -> Result<J, String> {
        let opts = super::box_prep::PrepareOpts { runtime: k2_root_proto::apps::OciRuntime::Crun, cores_mem_min_mib: None, budget: None };
        // The owner's setting is the opt-in to K2's Podman (lane P's
        // --install-podman); prepare installs it only where the box needs it.
        let pod = super::podman_install::PodmanOpts { install: true, dry_run: false, from: None };
        super::box_prep::prepare_core(&opts, &pod, ACTOR).map_err(|r| {
            let step = r.extra.as_ref().and_then(|e| e["failedStep"].as_str()).unwrap_or("").to_string();
            if step.is_empty() {
                format!("{}: {}", r.code, r.hint)
            } else {
                format!("box prep stopped at `{step}`: {}", r.hint)
            }
        })
    }
    fn install_engine(&self) -> Result<J, String> {
        super::engine_install::install_core(None).map_err(|r| format!("{}: {}", r.code, r.hint))
    }
    fn note_owner(&self, title: &str, body: &str) {
        if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-apps", title, body) {
            k2_core::log_debug!("[apps] auto-prepare notice: {e}");
        }
    }
}

/// Boot (= after every update): one detached thread on Linux.
pub fn spawn() {
    if cfg!(test) || !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::thread::Builder::new().name("app-auto-prepare".into()).spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(180));
        let mut backoff = 3600u64;
        loop {
            let out = run_once(&LiveOps);
            super::grace::audit("app-auto-prepare", ACTOR, &out.json().to_string());
            let wait = next_wait(&out, &mut backoff);
            record(&out, Some(k2_core::deploy::now() + wait as i64));
            k2_core::log_debug!("[apps] auto-prepare: {}; next in {wait}s", out.json());
            std::thread::sleep(std::time::Duration::from_secs(wait));
        }
    });
}

#[cfg(test)]
#[path = "auto_prepare_tests.rs"]
mod tests;
