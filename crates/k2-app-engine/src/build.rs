//! S16 (protocol 3): the manifest's `build` runs inside the wall.
//!
//! K2 never infers an install: it runs only the argv the manifest
//! declares (lockfile required, the daemon checks), and never on the host.
//! The engine:
//! 1. copies the source Seed into `apps/<id>/build/<b>`, owned by the
//!    App's ids;
//! 2. runs the argv there as a one-off Quadlet unit in the App's own slot
//!    and slice: its ids, no capabilities, K2's seccomp profile, the MAC
//!    shim, a read-only root, and no network but the App's egress proxy
//!    (the daemon lets the package registries through for the build). No
//!    gateway link, no pass, no secrets, no published port;
//! 3. on exit 0, tars the built tree (inside k2app's namespace, where the
//!    App's files are k2app's) for the daemon, which stores it as the Seed.
//!
//! On the guarded path package install scripts are off
//! (`wire::no_scripts_env`): a dependency's `postinstall` never runs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_root_proto::engine::{self as wire, check_build, codes, proxy_port, slot_base, BuildSpec, MAX_OUTPUT};
use serde_json::{json, Value};

use crate::quadlet;
use crate::server::Engine;
use crate::sys;

type R<T> = Result<T, (&'static str, String)>;

pub const TAR: &str = "/usr/bin/tar";

/// What a finished build hands back: the answer and, on success, the tar
/// of the built tree (a temp file the caller streams and removes).
pub struct Built {
    pub answer: Value,
    pub tar: Option<PathBuf>,
}

/// The build's environment: the declared variables, then the engine's
/// own (they win): home and tmp, the App's proxy, a package cache in
/// `/tmp`, `CI=1`, scripts off on the guarded path, Node's heap only
/// when capped.
pub fn build_env(s: &BuildSpec, px: u16) -> std::collections::BTreeMap<String, String> {
    let mut env = s.env.clone();
    for k in ["PATH", "HOME", "TMPDIR", "NODE_OPTIONS", "PORT", "CI"] {
        env.remove(k);
    }
    env.retain(|k, _| !k.starts_with("K2_"));
    env.insert("HOME".into(), "/app-home".into());
    env.insert("TMPDIR".into(), "/tmp".into());
    env.insert("CI".into(), "1".into());
    env.insert("npm_config_cache".into(), "/tmp/.npm".into());
    for (k, v) in wire::proxy_env(px) {
        env.insert(k.into(), v);
    }
    if !s.scripts {
        for (k, v) in wire::no_scripts_env() {
            env.insert(k.into(), v.into());
        }
    }
    match s.resources.node_heap_mib() {
        Some(h) => env.insert("NODE_OPTIONS".into(), format!("--max-old-space-size={h}")),
        None => None,
    };
    env
}

/// The unit's state: `(ActiveState, ExecMainStatus, Result)`.
fn unit_state(e: &Engine, unit: &str) -> R<(String, Option<i32>, String)> {
    let o = e.systemctl(&["show", "-p", "ActiveState", "-p", "ExecMainStatus", "-p", "Result", unit], 30)?;
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    Ok((get("ActiveState"), get("ExecMainStatus").parse().ok(), get("Result")))
}

/// The last `MAX_OUTPUT` bytes of the build's log.
fn tail(path: &Path) -> String {
    let b = std::fs::read(path).unwrap_or_default();
    let start = b.len().saturating_sub(MAX_OUTPUT);
    String::from_utf8_lossy(&b[start..]).to_string()
}

pub fn run(e: &Engine, s: &BuildSpec) -> R<Built> {
    let app = check_build(s).map_err(|w| (codes::BAD_REQUEST, format!("build: {w}")))?;
    let slot = e.slot_of(&app)?;
    let px = proxy_port(slot).ok_or((codes::NO_SLOT, "slot outside the proxy range".into()))?;
    let base = slot_base(slot);
    let seed_dir = e.seeds().join(&s.seed);
    if !seed_dir.is_dir() {
        return Err((codes::NOT_FOUND, "source tree not on the engine (seed-put first)".into()));
    }
    let dir = e.builds_dir().join(app.as_str());
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|er| (codes::IO, format!("mkdir: {er}")))?;
    }
    let work = e.work_copy(&app, &seed_dir, &format!("build/{}", s.build), base)?;
    let env_file = dir.join(format!("{}.env", s.build));
    let log_file = dir.join(format!("{}.log", s.build));
    let stem = quadlet::build_stem(&app, &s.build);
    let unit = format!("{stem}.service");
    let qfile = e.quadlets().join(format!("{stem}.container"));
    let cleanup = |e: &Engine| {
        let _ = e.systemctl(&["stop", &unit], 120);
        let _ = e.run(crate::server::podman_bin(), &["rm", "-f", "-i", &stem], None, 60);
        let _ = std::fs::remove_file(&qfile);
        let _ = std::fs::remove_file(&env_file);
        {
            let _g = e.locked();
            let _ = e.systemctl(&["daemon-reload"], 120);
        }
        let _ = e.systemctl(&["reset-failed", &unit], 30);
        e.remove_owned(&work);
    };
    let env_text: String = build_env(s, px).iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let work_s = work.to_string_lossy().to_string();
    let env_s = env_file.to_string_lossy().to_string();
    let log_s = log_file.to_string_lossy().to_string();
    let sec_s = e.seccomp_path().to_string_lossy().to_string();
    let runtime = e.runtime_arg();
    let text = quadlet::render_build(&quadlet::BuildRender {
        app: &app,
        build: &s.build,
        image: &s.image,
        runtime: &runtime,
        base,
        work_dir: &work_s,
        env_file: &env_s,
        seccomp: &sec_s,
        px_port: px,
        argv: &s.argv,
        resources: &s.resources,
        log_file: &log_s,
        timeout_secs: s.timeout_secs,
    });
    {
        let _g = e.locked();
        let w = sys::write_atomic(&env_file, env_text.as_bytes(), 0o600)
            .and_then(|_| sys::write_atomic(&log_file, b"", 0o600))
            .and_then(|_| sys::write_atomic(&qfile, text.as_bytes(), 0o600));
        if let Err(er) = w {
            drop(_g);
            cleanup(e);
            return Err((codes::IO, er));
        }
        let r = e.systemctl(&["daemon-reload"], 120)?;
        if !r.ok() {
            drop(_g);
            cleanup(e);
            return Err((codes::SYSTEMD, format!("daemon-reload: {}", r.why())));
        }
    }
    let t0 = Instant::now();
    // Blocks until the container runs (or the unit failed at once).
    let _ = e.systemctl(&["start", &unit], s.timeout_secs + 120)?;
    let deadline = Duration::from_secs(s.timeout_secs + 90);
    let (code, timed_out) = loop {
        let (active, status, result) = unit_state(e, &unit)?;
        if matches!(active.as_str(), "inactive" | "failed") {
            let code = if result == "success" { Some(0) } else { status.filter(|c| *c != 0).or(Some(-1)) };
            break (code, result == "timeout");
        }
        if t0.elapsed() > deadline {
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let output = tail(&log_file);
    let ms = t0.elapsed().as_millis() as u64;
    let _ = std::fs::remove_file(&log_file);
    if code != Some(0) {
        cleanup(e);
        return Ok(Built { answer: json!({ "exitCode": code, "timedOut": timed_out, "output": output, "ms": ms, "tarLen": null }), tar: None });
    }
    // The built tree, tarred where the App's files are k2app's.
    let tar = e.builds_dir().join(format!(".tar-{}-{}.tar", app.as_str(), s.build));
    let _ = std::fs::remove_file(&tar);
    let tar_s = tar.to_string_lossy().to_string();
    let o = e.podman(&["unshare", TAR, "--numeric-owner", "--owner=0", "--group=0", "-C", &work_s, "-cf", &tar_s, "."], None, 900);
    if let Err(er) = o {
        let _ = std::fs::remove_file(&tar);
        cleanup(e);
        return Err(er);
    }
    cleanup(e);
    let len = std::fs::metadata(&tar).map_err(|er| (codes::IO, format!("built tree: {er}")))?.len();
    if len > wire::MAX_BUILD_TAR {
        let _ = std::fs::remove_file(&tar);
        return Err((codes::TOO_LARGE, format!("the built tree is {len} bytes; the cap is {}", wire::MAX_BUILD_TAR)));
    }
    Ok(Built { answer: json!({ "exitCode": 0, "timedOut": false, "output": output, "ms": ms, "tarLen": len }), tar: Some(tar) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_env_is_the_engines() {
        let s = BuildSpec {
            app: "hearth".into(),
            build: "b-0123456789ab".into(),
            seed: "c".repeat(64),
            image: "localhost/k2-base/node24:ec857ea6bb60".into(),
            argv: vec!["npm".into(), "ci".into()],
            env: [
                ("NODE_ENV".to_string(), "production".to_string()),
                ("HTTPS_PROXY".to_string(), "http://evil:1".to_string()),
                ("K2_HOOK_TOKEN".to_string(), "x".to_string()),
                ("npm_config_ignore_scripts".to_string(), "false".to_string()),
            ]
            .into(),
            scripts: false,
            timeout_secs: 900,
            resources: wire::Resources::k2_default(),
        };
        let e = build_env(&s, 38_902);
        assert_eq!(e["NODE_ENV"], "production");
        assert_eq!(e["HTTPS_PROXY"], "http://127.0.0.1:38902", "only the App's own proxy");
        assert_eq!(e["npm_config_ignore_scripts"], "true", "guarded: install scripts off, whatever the manifest says");
        assert!(!e.contains_key("K2_HOOK_TOKEN"));
        assert!(!e.contains_key("NODE_OPTIONS"), "no cap, no heap flag");
        assert_eq!(e["CI"], "1");
        let mut open = s.clone();
        open.scripts = true;
        open.resources.memory_mib = Some(2048);
        let e = build_env(&open, 38_902);
        assert_eq!(e["npm_config_ignore_scripts"], "false", "open path: scripts as the App asks");
        assert_eq!(e["NODE_OPTIONS"], "--max-old-space-size=1024");
    }
}
