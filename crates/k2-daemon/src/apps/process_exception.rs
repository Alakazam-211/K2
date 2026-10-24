//! The `process` exception (`prd-k2-app-runtime-v1` §4.3, AR45): the one
//! App kind systemd runs directly. The owner moves an App out of the
//! guarded container (for a device a container can't reach, §4.1), in
//! person, with an owner confirm (`POST /cli/app/exception`); its label is
//! "Open (process)".
//!
//! It is still walled, just differently:
//! - a **system** unit `k2app-process@<id>.service` (root helper template)
//!   running as `k2appx`, never `k2app` (which owns every guarded
//!   container's user namespace) and never the daemon user;
//! - systemd's sandbox: read-only system, no homes, private /tmp and
//!   /dev, no new privileges, an empty capability set, the
//!   `@system-service` syscall filter, no namespaces, loopback only;
//! - the `k2-app` AppArmor profile where AppArmor is on;
//! - the same caps as any App (memory, disk ceilings, tasks) on a runtime
//!   drop-in the helper writes before every start (`apps process caps`);
//! - the `inet k2app` table: its user reaches only its own gateway port,
//!   the proxy and Postgres.
//!
//! One instance per App: a deploy is a health-gated **restart** (stop,
//! swap the code, start), not a blue/green switch; rollback is "start the
//! previous Seed again" (§4.1, open path). `k2 app exec`/`shell` don't
//! reach it in 0.46.0.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::deploy::codes;
use k2_core::deploy::engine::{
    AppSpec, CgroupStats, EngineCaps, EngineError, ExecStream, Isolation, Liveness, Prepared, Result, RunHandle, RuntimeEngine,
    SeedRef,
};
use k2_root_proto::apps::{AppId, IoMax, APPS_SLICE, CGROUP_ROOT};
use k2_root_proto::engine::{self as wire, ProcessSpec, Request};
use serde_json::Value;

use super::guarded;

/// `engine_ref` prefix for the process exception's handles.
pub const REF_PREFIX: &str = "procx:";

pub const SYSTEMCTL: &str = "/usr/bin/systemctl";

fn err(code: &'static str, m: impl Into<String>) -> EngineError {
    EngineError::new(code, m)
}

fn app_id(a: &str) -> Result<AppId> {
    AppId::parse(a).ok_or_else(|| err(codes::APP_ID_INVALID, format!("App id '{a}' doesn't fit the process exception's unit name")))
}

fn helper(argv: &[&str]) -> Result<String> {
    let st = crate::root_helper::state();
    if !st.usable(k2_root_proto::version::APPS_MIN_PROTOCOL) {
        return Err(err(codes::ENGINE_UNAVAILABLE, "the process exception needs the root helper (protocol 6): k2 app doctor"));
    }
    let v: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    crate::root_helper::call_text(&v, None).map_err(|e| err(codes::ENGINE_UNAVAILABLE, format!("root helper `{}`: {e}", argv.join(" "))))
}

/// The helper's `apps process caps` argv for an App's ceilings.
pub fn caps_argv(app: &AppId, memory_mib: u64, io: &IoMax, devices: &[String]) -> Vec<String> {
    let mut v = vec!["apps".to_string(), "process".into(), "caps".into(), app.as_str().into(), memory_mib.to_string(), io.token()];
    v.extend(devices.iter().cloned());
    v
}

/// `ActiveState`, `ExecMainStatus`, `MainPID` of the App's unit (a plain
/// read: no root).
fn unit_show(app: &AppId) -> Option<(String, Option<i32>, Option<i32>)> {
    let out = std::process::Command::new(SYSTEMCTL)
        .args(["show", "-p", "ActiveState", "-p", "ExecMainStatus", "-p", "MainPID", &app.process_unit()])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout).to_string();
    let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    Some((get("ActiveState"), get("ExecMainStatus").parse().ok(), get("MainPID").parse().ok().filter(|p: &i32| *p > 0)))
}

#[derive(Default)]
pub struct ProcessException {
    /// deployment → Seed hash, from `prepare` to `start`.
    prepared: Mutex<HashMap<String, String>>,
    /// App → the deployment its unit runs now.
    current: Mutex<HashMap<String, String>>,
    live: Mutex<HashMap<String, (Instant, Liveness)>>,
}

pub fn engine() -> &'static ProcessException {
    static E: OnceLock<ProcessException> = OnceLock::new();
    E.get_or_init(ProcessException::default)
}

pub fn owns(h: &RunHandle) -> bool {
    h.engine_ref.starts_with(REF_PREFIX)
}

impl ProcessException {
    fn is_current(&self, h: &RunHandle) -> bool {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).get(&h.app_id) == Some(&h.deployment_id)
    }
}

impl RuntimeEngine for ProcessException {
    fn isolation(&self) -> Isolation {
        Isolation::Process
    }

    fn caps(&self) -> EngineCaps {
        EngineCaps {
            separate_user: true,
            memory_limit: true,
            cpu_limit: false,
            read_only_seed: true,
            egress_gate: true,
            exec: false,
            adopt: true,
        }
    }

    fn prepare(&self, seed: &SeedRef, spec: &AppSpec) -> Result<Prepared> {
        if !cfg!(target_os = "linux") {
            return Err(err(codes::ENGINE_UNAVAILABLE, "the process exception runs on Linux boxes only"));
        }
        app_id(&spec.app_id)?;
        // A4: K2's own engine faults carry their own codes (never a crash).
        guarded::remember_checked(&spec.deployment_id, guarded::require_protocol()?);
        guarded::ensure_seed(seed)?;
        self.prepared.lock().unwrap_or_else(|e| e.into_inner()).insert(spec.deployment_id.clone(), seed.hash.clone());
        Ok(Prepared { workdir: seed.path.clone() })
    }

    fn start(&self, spec: &AppSpec, p: &Prepared, port: u16) -> Result<RunHandle> {
        let app = app_id(&spec.app_id)?;
        if !wire::app_port_ok(port) {
            return Err(err(codes::ENGINE_UNAVAILABLE, format!("port {port} is in K2's reserved range")));
        }
        let seed = self
            .prepared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&spec.deployment_id)
            .ok_or_else(|| err(codes::ENGINE_UNAVAILABLE, "start before prepare"))?;
        // The same environment a guarded App gets (the engine adds the
        // gateway address and the paths), and its granted secrets.
        let mut env = guarded::guarded_env(spec, std::env::var("LANG").ok(), std::env::var("TZ").ok());
        for k in ["K2_HOST", "K2_PORT", "K2_GATEWAY_URL"] {
            env.remove(k);
        }
        // Box services (0.46.0): the process exception has the host's
        // loopback, narrowed by its nft rule; it dials the host port, which
        // the rule admits only while approved.
        env.extend(super::box_service_routes::env_for(app.as_str(), true));
        let g = crate::vault::publish_env::granted(&spec.service_id, &spec.workspace_id, &p.workdir);
        let (secrets, _dropped) = guarded::secret_items(g.env, g.files);
        let memory_mib = guarded::process_memory_mib(spec.memory_limit_bytes);
        let ps = ProcessSpec {
            app: app.as_str().into(),
            deployment: spec.deployment_id.clone(),
            seed,
            port,
            argv: spec.argv.clone(),
            env,
            memory_mib,
            secrets,
            pass_hex: None,
        };
        wire::check_process(&ps).map_err(|w| err(codes::MANIFEST_INVALID, format!("the process exception can't run this: {w}")))?;
        let staged = guarded::call(&Request::ProcessStage(Box::new(ps)), None, Duration::from_secs(600))?;
        let gw = staged["gwPort"].as_u64().and_then(|g| u16::try_from(g).ok()).ok_or_else(|| err(codes::ENGINE_UNAVAILABLE, "stage gave no gateway port"))?;
        // S14: the App's gateway link on its own gateway port (host
        // loopback; no in-wall forward here), with a fresh pass. A redeploy
        // keeps the listener.
        let (link_caps, rooms, dropped_caps) = super::gateway::link_policy(&guarded::seed_caps(&p.workdir)?, &spec.workspace_id);
        if !dropped_caps.is_empty() {
            k2_core::log_debug!("[apps] {}: caps the App pass can't carry (owner widenings, 0.46.1): {}", app.as_str(), dropped_caps.join(", "));
        }
        let had_link = super::gateway::port_of(app.as_str()).is_some();
        let link = super::gateway::LinkSpec {
            app_id: app.as_str().into(),
            service_id: spec.service_id.clone(),
            workspace_id: spec.workspace_id.clone(),
            port: gw,
            caps: link_caps,
            rooms,
        };
        let deliver = guarded::deliver_for(app.as_str());
        let pass = super::gateway::open(link, Some(deliver.clone())).map_err(|e| err(codes::ENGINE_UNAVAILABLE, format!("the App's gateway link: {e}")))?;
        let close_new_link = || {
            if !had_link {
                super::gateway::close(app.as_str(), Some(&spec.service_id), true);
            }
        };
        // One instance per App: stop what runs, swap the code, start.
        let _ = helper(&["apps", "process", "stop", app.as_str()]);
        self.current.lock().unwrap_or_else(|e| e.into_inner()).remove(&spec.app_id);
        let started = (|| -> Result<()> {
            guarded::call(&Request::ProcessActivate { app: app.as_str().into(), deployment: spec.deployment_id.clone() }, None, Duration::from_secs(120))?;
            // The pass lands in the App's process folder (`K2_PASS_FILE`);
            // rotations replace it there through the same `pass-write`.
            deliver(&pass).map_err(|e| err(codes::ENGINE_UNAVAILABLE, format!("the App pass: {e}")))?;
            let io = IoMax::from_column(spec.io_max_json.as_deref());
            let caps = caps_argv(&app, memory_mib, &io, &spec.devices);
            let caps_ref: Vec<&str> = caps.iter().map(String::as_str).collect();
            helper(&caps_ref)?;
            helper(&["apps", "process", "start", app.as_str()])?;
            Ok(())
        })();
        if let Err(e) = started {
            close_new_link();
            return Err(e);
        }
        self.current.lock().unwrap_or_else(|e| e.into_inner()).insert(spec.app_id.clone(), spec.deployment_id.clone());
        if let Err(e) = guarded::install_gateway_rule(app.as_str(), gw) {
            k2_core::log_debug!("[apps] {}: process exception gateway rule not installed: {e}", app.as_str());
        }
        let pid = unit_show(&app).and_then(|(_, _, p)| p);
        Ok(RunHandle {
            app_id: spec.app_id.clone(),
            deployment_id: spec.deployment_id.clone(),
            port,
            pid,
            engine_ref: format!("{REF_PREFIX}{}", app.process_unit()),
        })
    }

    fn liveness(&self, h: &RunHandle) -> Liveness {
        if !self.is_current(h) {
            // Replaced by a later deploy (one instance per App).
            return Liveness::Exited(None);
        }
        let key = h.deployment_id.clone();
        if let Some((at, l)) = self.live.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if at.elapsed() < Duration::from_secs(2) {
                return *l;
            }
        }
        let Ok(app) = app_id(&h.app_id) else { return Liveness::Unknown };
        let l = match unit_show(&app) {
            Some((st, code, _)) => match st.as_str() {
                "active" | "activating" | "reloading" => Liveness::Running,
                "inactive" | "failed" | "deactivating" => Liveness::Exited(code),
                _ => Liveness::Unknown,
            },
            None => Liveness::Unknown,
        };
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), l));
        l
    }

    fn stop(&self, h: &RunHandle, _grace: Duration) -> Result<()> {
        // A replaced deployment has nothing left to stop.
        if !self.is_current(h) {
            return Ok(());
        }
        helper(&["apps", "process", "stop", &h.app_id])?;
        self.current.lock().unwrap_or_else(|e| e.into_inner()).remove(&h.app_id);
        self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
        Ok(())
    }

    fn exec(&self, _h: &RunHandle, _argv: &[String], _tty: bool) -> Result<ExecStream> {
        Err(err(
            codes::ENGINE_UNAVAILABLE,
            "k2 app exec and shell don't reach the process exception in this release; use k2 app logs, or move the App back to the guarded container",
        ))
    }

    fn stats(&self, h: &RunHandle) -> Result<CgroupStats> {
        let app = app_id(&h.app_id)?;
        let base = format!("{CGROUP_ROOT}/{APPS_SLICE}/{}", app.process_unit());
        let read = |f: &str| std::fs::read_to_string(Path::new(&base).join(f)).ok().and_then(|s| s.trim().parse::<u64>().ok());
        let cpu_usec = std::fs::read_to_string(Path::new(&base).join("cpu.stat"))
            .ok()
            .and_then(|s| k2_core::deploy::engine::cpu_stat_usage_usec(&s));
        Ok(CgroupStats { mem_current: read("memory.current"), pids: read("pids.current"), cpu_usec })
    }

    fn adopt(&self) -> Result<Vec<RunHandle>> {
        if !cfg!(target_os = "linux") {
            return Ok(Vec::new());
        }
        // A3: with an engine installed, a missing socket is "not answering
        // yet" (boot waits), never "nothing running".
        if !Path::new(wire::SOCKET).exists() {
            if !guarded::engine_installed() {
                return Ok(Vec::new());
            }
            return Err(err(codes::ENGINE_NOT_ANSWERING, format!("the App engine's socket {} isn't there yet", wire::SOCKET)));
        }
        let apps = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::deploy::store::list_apps(&conn, None).map_err(|e| err("db_error", e.to_string()))?
        };
        let mut out = Vec::new();
        for a in apps.iter().filter(|a| a.is_process_exception()) {
            let Ok(id) = app_id(&a.app_id) else { continue };
            let Some((st, _, pid)) = unit_show(&id) else { continue };
            if st != "active" {
                continue;
            }
            let v: Value = guarded::call(&Request::ProcessState { app: id.as_str().into() }, None, Duration::from_secs(30))?;
            let (Some(dep), Some(port)) = (v["deployment"].as_str(), v["port"].as_u64().and_then(|p| u16::try_from(p).ok())) else {
                continue;
            };
            if let Some(gw) = v["gwPort"].as_u64().and_then(|g| u16::try_from(g).ok()) {
                let _ = guarded::install_gateway_rule(id.as_str(), gw);
                // S14: the link listener is the daemon's, so it re-opens;
                // the App gets a fresh pass in its process folder.
                if super::gateway::port_of(id.as_str()).is_none() {
                    if let Some(spec) = guarded::link_spec_for(&a.app_id, gw) {
                        let deliver = guarded::deliver_for(id.as_str());
                        if let Err(e) = super::gateway::open(spec, Some(deliver.clone())).and_then(|raw| deliver(&raw)) {
                            k2_core::log_debug!("[apps] adopt {}: gateway link: {e}", a.app_id);
                        }
                    }
                }
            }
            self.current.lock().unwrap_or_else(|e| e.into_inner()).insert(a.app_id.clone(), dep.to_string());
            out.push(RunHandle {
                app_id: a.app_id.clone(),
                deployment_id: dep.to_string(),
                port,
                pid,
                engine_ref: format!("{REF_PREFIX}{}", id.process_unit()),
            });
        }
        Ok(out)
    }
}

/// Purge a stopped process App's state (uninstall --purge).
pub fn purge(app: &str) -> Result<()> {
    let id = app_id(app)?;
    helper(&["apps", "process", "purge", id.as_str()]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_argv_is_on_the_helpers_catalog() {
        let a = AppId::parse("gpu-job").unwrap();
        let v = caps_argv(&a, 2048, &IoMax::k2_default(), &["dri-render128".into()]);
        assert_eq!(v.join(" "), "apps process caps gpu-job 2048 -.41943040.-.2000 dri-render128");
        let r: Vec<&str> = v.iter().map(String::as_str).collect();
        let verb = k2_root_proto::Verb::parse(&r).unwrap();
        assert_eq!(verb.class(), k2_root_proto::Class::Daemon);
        let v = caps_argv(&a, 512, &IoMax::unlimited(), &[]);
        assert_eq!(v.join(" "), "apps process caps gpu-job 512 -.-.-.-");
        assert!(k2_root_proto::Verb::parse(&v.iter().map(String::as_str).collect::<Vec<_>>()).is_ok());
    }

    #[test]
    fn a_replaced_deployment_is_exited_and_stopping_it_is_a_no_op() {
        let e = ProcessException::default();
        e.current.lock().unwrap().insert("gpu-job".into(), "d-00000000000b".into());
        let old = RunHandle {
            app_id: "gpu-job".into(),
            deployment_id: "d-00000000000a".into(),
            port: 41_000,
            pid: None,
            engine_ref: format!("{REF_PREFIX}k2app-process@gpu-job.service"),
        };
        assert!(owns(&old));
        assert_eq!(e.liveness(&old), Liveness::Exited(None));
        assert!(e.stop(&old, Duration::from_secs(1)).is_ok(), "never stops the newer deployment's unit");
        assert_eq!(e.current.lock().unwrap().get("gpu-job").map(String::as_str), Some("d-00000000000b"));
        assert_eq!(e.exec(&old, &["ls".into()], false).unwrap_err().code, codes::ENGINE_UNAVAILABLE);
    }
}
