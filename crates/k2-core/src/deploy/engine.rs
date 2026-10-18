//! `RuntimeEngine` (§3.3): the one trait every App wall plugs into.
//!
//! - Stage 1: the **process** engine (`k2-daemon` `apps::process_engine`),
//!   today's publish supervisor promoted, behind the preview flag, Apps
//!   running as the daemon user (the one exception to SR1, §16 C9).
//! - Stage 3: the **container** engine (rootless Podman/Quadlet through
//!   `k2-app-engine`, S11–S12) implements the same trait; `isolation()`
//!   says `Container` and `caps()` says what it can enforce.
//! - Stage V: `Vm` (crun `krun` inside the container).
//!
//! Not merged with `terminal::sandbox::SandboxBackend` (PTY cells), but it
//! adopts that trait's rule: **never silently fall back to a weaker
//! isolation** (AR24). A deploy names the isolation it wants; an engine
//! that can't give it refuses (`engine_unavailable`).
//!
//! HTTP health, the switch, drain, the watch window and backoff live above
//! the trait (shared code), so every engine gets them unchanged.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

/// The App's wall (§2, K12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    Container,
    Vm,
    Process,
    None,
}

impl Isolation {
    pub fn as_str(self) -> &'static str {
        match self {
            Isolation::Container => "container",
            Isolation::Vm => "vm",
            Isolation::Process => "process",
            Isolation::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "container" => Some(Isolation::Container),
            "vm" => Some(Isolation::Vm),
            "process" => Some(Isolation::Process),
            "none" => Some(Isolation::None),
            _ => None,
        }
    }
}

/// What an engine can enforce on this box (shown by `k2 app inspect`,
/// used by the label function and admission later).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineCaps {
    /// Runs App code as a user that isn't the daemon's.
    pub separate_user: bool,
    /// Enforces a memory limit (cgroup `memory.max`).
    pub memory_limit: bool,
    pub cpu_limit: bool,
    /// Read-only Seed mount.
    pub read_only_seed: bool,
    /// Egress through the K2 proxy only.
    pub egress_gate: bool,
    /// `k2 app exec` / `shell` (S28).
    pub exec: bool,
    /// Instances survive a daemon restart and are re-adopted.
    pub adopt: bool,
}

/// A stored Seed handed to an engine (already verified by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedRef {
    pub hash: String,
    pub path: PathBuf,
}

/// Everything an engine needs to run one instance (naming C13:
/// `InstanceSpec` → `AppSpec`).
#[derive(Debug, Clone, PartialEq)]
pub struct AppSpec {
    pub app_id: String,
    /// `published_services.id` (AR5).
    pub service_id: String,
    pub workspace_id: String,
    /// The deployment this instance belongs to (logs and adopt key).
    pub deployment_id: String,
    pub isolation: Isolation,
    /// The server argv (`host.start`), run with `$PORT` set.
    pub argv: Vec<String>,
    /// Declared, non-secret variables (manifest `env`) plus K2's own.
    pub env: BTreeMap<String, String>,
    /// `None` = no per-App memory limit (the user decides App memory,
    /// Rosson 2026-10-10). Stored; enforced only by engines whose caps say
    /// `memory_limit` (stage R sizes the protected reserve).
    pub memory_limit_bytes: Option<i64>,
    /// The App's drain window (manifest `host.drainSecs`, else the
    /// default): a walled engine's stop timeout must cover it (AR45).
    pub drain_secs: u64,
    /// Per-App disk ceilings as stored (`apps.io_max_json`; NULL = K2's
    /// default ceiling). Walled engines parse and enforce it.
    pub io_max_json: Option<String>,
    /// The `process` exception's device grants (enum tokens).
    pub devices: Vec<String>,
    /// The App's public https origins, primary first (`K2_APP_ORIGIN` /
    /// `K2_APP_ORIGINS`); empty = no public URL, neither is set.
    pub public_origins: Vec<String>,
    /// The App folder's writable parts.
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub home_dir: PathBuf,
    pub log_path: PathBuf,
}

/// What `prepare` produced (the working directory the start runs in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub workdir: PathBuf,
}

/// One running instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunHandle {
    pub app_id: String,
    pub deployment_id: String,
    pub port: u16,
    /// The process (group) id for the process engine; the container's main
    /// pid for others, when known.
    pub pid: Option<i32>,
    /// The engine's own reference (unit name, container id).
    pub engine_ref: String,
}

/// Engine-level liveness. HTTP health is shared code above the trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "code")]
pub enum Liveness {
    Running,
    Exited(Option<i32>),
    Unknown,
}

/// Resource numbers for the sampler (stage R). Engines without cgroups
/// return `None`s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CgroupStats {
    pub mem_current: Option<u64>,
    pub pids: Option<u64>,
}

/// An interactive or one-shot exec into an instance (S28).
#[derive(Debug)]
pub struct ExecStream {
    pub exit_code: Option<i32>,
    pub output: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngineError {
    pub code: &'static str,
    pub message: String,
}

impl EngineError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

pub trait RuntimeEngine: Send + Sync {
    /// The wall this engine gives.
    fn isolation(&self) -> Isolation;
    /// What it can enforce on this box.
    fn caps(&self) -> EngineCaps;
    /// Ready a Seed for `spec` (mounts, image, unit file).
    fn prepare(&self, seed: &SeedRef, spec: &AppSpec) -> Result<Prepared>;
    /// Start an instance listening on `port`.
    fn start(&self, spec: &AppSpec, p: &Prepared, port: u16) -> Result<RunHandle>;
    /// Engine-level liveness.
    fn liveness(&self, h: &RunHandle) -> Liveness;
    /// SIGTERM (or the engine's equivalent), wait `grace`, then kill.
    fn stop(&self, h: &RunHandle, grace: Duration) -> Result<()>;
    /// Run a command inside an instance (S28).
    fn exec(&self, h: &RunHandle, argv: &[String], tty: bool) -> Result<ExecStream>;
    /// Resource numbers (stage R).
    fn stats(&self, h: &RunHandle) -> Result<CgroupStats>;
    /// Instances still running from before a daemon restart.
    fn adopt(&self) -> Result<Vec<RunHandle>>;
}

/// Refuse a weaker wall than asked for (AR24: never a silent fallback).
pub fn require_isolation(engine: &dyn RuntimeEngine, wanted: Isolation) -> Result<()> {
    if engine.isolation() == wanted {
        Ok(())
    } else {
        Err(EngineError::new(
            super::codes::ENGINE_UNAVAILABLE,
            format!(
                "this App wants isolation={} but this box's engine gives {}; K2 never runs an App behind a weaker wall than asked",
                wanted.as_str(),
                engine.isolation().as_str()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Isolation);
    impl RuntimeEngine for Fake {
        fn isolation(&self) -> Isolation {
            self.0
        }
        fn caps(&self) -> EngineCaps {
            EngineCaps::default()
        }
        fn prepare(&self, s: &SeedRef, _: &AppSpec) -> Result<Prepared> {
            Ok(Prepared { workdir: s.path.clone() })
        }
        fn start(&self, spec: &AppSpec, _: &Prepared, port: u16) -> Result<RunHandle> {
            Ok(RunHandle { app_id: spec.app_id.clone(), deployment_id: spec.deployment_id.clone(), port, pid: None, engine_ref: "fake".into() })
        }
        fn liveness(&self, _: &RunHandle) -> Liveness {
            Liveness::Running
        }
        fn stop(&self, _: &RunHandle, _: Duration) -> Result<()> {
            Ok(())
        }
        fn exec(&self, _: &RunHandle, _: &[String], _: bool) -> Result<ExecStream> {
            Err(EngineError::new("unsupported", "no exec"))
        }
        fn stats(&self, _: &RunHandle) -> Result<CgroupStats> {
            Ok(CgroupStats::default())
        }
        fn adopt(&self) -> Result<Vec<RunHandle>> {
            Ok(vec![])
        }
    }

    #[test]
    fn never_a_weaker_wall_than_asked() {
        let e = Fake(Isolation::Process);
        assert!(require_isolation(&e, Isolation::Process).is_ok());
        let err = require_isolation(&e, Isolation::Container).unwrap_err();
        assert_eq!(err.code, super::super::codes::ENGINE_UNAVAILABLE);
        assert_eq!(Isolation::parse("vm"), Some(Isolation::Vm));
        assert_eq!(Isolation::parse("isolate"), None, "isolate is reserved, not an engine");
    }
}
