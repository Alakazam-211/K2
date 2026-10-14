//! The engine: one Unix socket, one request per connection, a closed verb
//! list (`k2_root_proto::engine::Request`), every caller checked to be the
//! K2 daemon's main process before a byte of the request is read.
//!
//! Runs as `k2app` in its lingering user manager. Drives Podman (rootless)
//! and that manager's units through the CLI only — never `podman system
//! service`, never a shared Podman socket (§10.6 option A was rejected:
//! any daemon-user process, so every agent, would get the whole engine).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use k2_root_proto::apps::{AppId, APP_HOME, APP_USER, CONTAINERS_CONF, SUBGID_FILE, SUBUID_FILE};
use k2_root_proto::engine::{
    self as wire, app_slice, check_start, codes, deployment_ok, gateway_port, hex_decode, image_ok, seed_hash_ok, slot_base,
    unit_stem, Answer, Request, StartSpec, MAX_HEADER, MAX_IMAGE_BYTES, MAX_OUTPUT, MAX_SEED_BYTES, PASS_TARGET, SECRETS_TARGET_DIR,
};
use serde_json::{json, Value};

use crate::quadlet::{self, SecretMount};
use crate::sys::{self, Out};

pub const PODMAN: &str = "/usr/bin/podman";
pub const SYSTEMCTL: &str = "/usr/bin/systemctl";
pub const JOURNALCTL: &str = "/usr/bin/journalctl";

type R<T> = Result<T, (&'static str, String)>;

fn bad(m: impl Into<String>) -> (&'static str, String) {
    (codes::BAD_REQUEST, m.into())
}

/// The running engine.
pub struct Engine {
    pub home: PathBuf,
    pub runtime_dir: String,
    /// Serializes slot changes and unit-file writes + `daemon-reload`.
    lock: Mutex<()>,
}

/// One instance's state file (`<state>/run/<app>/<deployment>.json`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunState {
    pub app: String,
    pub deployment: String,
    pub port: u16,
    pub gw_port: u16,
    pub slot: u64,
    pub unit: String,
    pub secrets: Vec<String>,
}

impl Engine {
    pub fn new(home: PathBuf, runtime_dir: String) -> Self {
        Self { home, runtime_dir, lock: Mutex::new(()) }
    }

    fn seeds(&self) -> PathBuf {
        self.home.join("seeds")
    }
    fn apps(&self) -> PathBuf {
        self.home.join("apps")
    }
    fn state(&self) -> PathBuf {
        self.home.join("engine")
    }
    fn run_dir(&self, app: &AppId) -> PathBuf {
        self.state().join("run").join(app.as_str())
    }
    fn quadlets(&self) -> PathBuf {
        self.home.join(".config/containers/systemd")
    }
    pub fn seccomp_path(&self) -> PathBuf {
        self.state().join(crate::seccomp::file_name())
    }

    fn cmd(&self, prog: &str) -> Command {
        let mut c = Command::new(prog);
        c.current_dir(&self.home);
        sys::base_env(&mut c, &self.home.to_string_lossy(), &self.runtime_dir);
        c
    }

    fn run(&self, prog: &str, args: &[&str], stdin: Option<Vec<u8>>, secs: u64) -> R<Out> {
        let mut c = self.cmd(prog);
        c.args(args);
        sys::run(c, stdin, Duration::from_secs(secs), MAX_OUTPUT).map_err(|e| (codes::IO, format!("{prog}: {e}")))
    }

    fn podman(&self, args: &[&str], stdin: Option<Vec<u8>>, secs: u64) -> R<Out> {
        let o = self.run(PODMAN, args, stdin, secs)?;
        if !o.ok() {
            return Err((codes::PODMAN, format!("podman {}: {}", args.first().unwrap_or(&""), o.why())));
        }
        Ok(o)
    }

    fn systemctl(&self, args: &[&str], secs: u64) -> R<Out> {
        let mut a = vec!["--user"];
        a.extend_from_slice(args);
        self.run(SYSTEMCTL, &a, None, secs)
    }

    /// `crun` (default) or `youki`, as box prep recorded it.
    pub fn runtime(&self) -> &'static str {
        let conf = std::fs::read_to_string(CONTAINERS_CONF).unwrap_or_default();
        if conf.lines().any(|l| l.trim() == "runtime = \"youki\"") {
            "youki"
        } else {
            "crun"
        }
    }

    /// Make the engine's own folders and write K2's seccomp profile.
    pub fn init(&self) -> Result<(), String> {
        use std::os::unix::fs::DirBuilderExt;
        for (d, mode) in [
            (self.seeds(), 0o755),
            (self.apps(), 0o700),
            (self.state(), 0o700),
            (self.state().join("run"), 0o700),
            (self.state().join("images"), 0o700),
            (self.quadlets(), 0o700),
        ] {
            std::fs::DirBuilder::new().recursive(true).mode(mode).create(&d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
        }
        // Leftovers of an interrupted Seed put or image load.
        for dir in [self.seeds(), self.state().join("images")] {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with(".tmp-") || n.starts_with(".load-") {
                        let p = e.path();
                        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                    }
                }
            }
        }
        let default = std::fs::read_to_string(crate::seccomp::DISTRO_DEFAULT)
            .map_err(|e| format!("{}: {e} (install containers-common; the engine never runs Apps without a seccomp profile)", crate::seccomp::DISTRO_DEFAULT))?;
        let profile = crate::seccomp::transform(&default)?;
        sys::write_atomic(&self.seccomp_path(), profile.as_bytes(), 0o644)
    }

    // ── verbs ────────────────────────────────────────────────────────

    pub fn handle(&self, req: Request, body: &mut dyn Read) -> R<Value> {
        match req {
            Request::Version => self.version(),
            Request::SeedHas { hash } => {
                if !seed_hash_ok(&hash) {
                    return Err(bad("seed hash"));
                }
                Ok(json!({ "present": self.seeds().join(&hash).is_dir() }))
            }
            Request::SeedPut { hash, len } => {
                if !seed_hash_ok(&hash) {
                    return Err(bad("seed hash"));
                }
                if len > MAX_SEED_BYTES {
                    return Err((codes::TOO_LARGE, format!("Seed tar is {len} bytes; the cap is {MAX_SEED_BYTES}")));
                }
                let stored = crate::seeds::put(&self.seeds(), &hash, len, body).map_err(|e| (codes::IO, e))?;
                Ok(json!({ "stored": stored }))
            }
            Request::ImageHas { image } => {
                if !image_ok(&image) {
                    return Err(bad("image"));
                }
                let o = self.run(PODMAN, &["image", "exists", &image], None, 60)?;
                Ok(json!({ "present": o.ok() }))
            }
            Request::ImageLoad { image, len } => self.image_load(&image, len, body),
            Request::Prepare { app, image, seed } => self.prepare(&app, &image, &seed),
            Request::Start(spec) => self.start(&spec),
            Request::Stop { app, deployment } => self.stop(&app, &deployment),
            Request::Liveness { app, deployment } => self.liveness(&app, &deployment),
            Request::Stats { app, deployment } => self.stats(&app, &deployment),
            Request::Logs { app, deployment, lines } => self.logs(&app, &deployment, lines),
            Request::Exec { app, deployment, argv } => self.exec(&app, &deployment, &argv),
            Request::Adopt => self.adopt(),
            Request::RemoveApp { app, data } => self.remove_app(&app, data),
        }
    }

    fn version(&self) -> R<Value> {
        let podman = self.run(PODMAN, &["--version"], None, 30).ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        Ok(json!({
            "name": "k2-app-engine",
            "version": env!("K2_PRODUCT_VERSION"),
            "protocol": wire::ENGINE_PROTOCOL,
            "runtime": self.runtime(),
            "podman": podman,
            "seccomp": crate::seccomp::file_name(),
            "uid": unsafe { libc::getuid() },
        }))
    }

    fn image_load(&self, image: &str, len: u64, body: &mut dyn Read) -> R<Value> {
        if !image_ok(image) {
            return Err(bad("image"));
        }
        if len > MAX_IMAGE_BYTES {
            return Err((codes::TOO_LARGE, format!("image archive is {len} bytes; the cap is {MAX_IMAGE_BYTES}")));
        }
        let tmp = self.state().join("images").join(format!(".load-{}-{}.tar", std::process::id(), rand_suffix()));
        let res = (|| -> R<Value> {
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&tmp)
                    .map_err(|e| (codes::IO, format!("image temp: {e}")))?;
                let n = std::io::copy(&mut body.take(len), &mut f).map_err(|e| (codes::IO, format!("image body: {e}")))?;
                if n != len {
                    return Err((codes::IO, format!("image body ended at {n} of {len} bytes")));
                }
            }
            let tmp_s = tmp.to_string_lossy().to_string();
            let o = self.podman(&["load", "-q", "-i", &tmp_s], None, 900)?;
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let loaded = text
                .lines()
                .filter_map(|l| l.trim().strip_prefix("Loaded image:").or_else(|| l.trim().strip_prefix("Loaded image(s):")))
                .map(|s| s.trim().split(',').next().unwrap_or("").trim().to_string())
                .find(|s| !s.is_empty())
                .ok_or_else(|| (codes::PODMAN, format!("podman load named no image: {}", text.trim())))?;
            if loaded != image {
                self.podman(&["tag", &loaded, image], None, 60)?;
            }
            Ok(json!({ "image": image, "loaded": loaded }))
        })();
        let _ = std::fs::remove_file(&tmp);
        res
    }

    fn slots_path(&self) -> PathBuf {
        self.state().join("slots.json")
    }

    fn capacity(&self) -> u64 {
        let su = std::fs::read_to_string(SUBUID_FILE).unwrap_or_default();
        let sg = std::fs::read_to_string(SUBGID_FILE).unwrap_or_default();
        crate::slots::capacity(&su, &sg, APP_USER)
    }

    fn slot_of(&self, app: &AppId) -> R<u64> {
        crate::slots::load(&self.slots_path())
            .map_err(|e| (codes::IO, e))?
            .get(app.as_str())
            .ok_or_else(|| (codes::NOT_FOUND, format!("App {} has no slot yet (prepare first)", app.as_str())))
    }

    fn prepare(&self, app: &str, image: &str, seed: &str) -> R<Value> {
        let app = AppId::parse(app).ok_or_else(|| bad("app id"))?;
        if !image_ok(image) {
            return Err(bad("image"));
        }
        if !seed_hash_ok(seed) {
            return Err(bad("seed hash"));
        }
        if !self.seeds().join(seed).is_dir() {
            return Err((codes::NOT_FOUND, "Seed not on the engine (seed-put first)".into()));
        }
        if !self.run(PODMAN, &["image", "exists", image], None, 60)?.ok() {
            return Err((codes::NOT_FOUND, "image not on the engine (image-load first)".into()));
        }
        let slot = {
            let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut s = crate::slots::load(&self.slots_path()).map_err(|e| (codes::IO, e))?;
            let cap = self.capacity();
            let n = s.assign(app.as_str(), cap).ok_or_else(|| {
                (codes::NO_SLOT, format!("no free id slot for another App ({cap} slots from k2app's subordinate ids are in use)"))
            })?;
            crate::slots::save(&self.slots_path(), &s).map_err(|e| (codes::IO, e))?;
            n
        };
        let gw = gateway_port(slot).ok_or((codes::NO_SLOT, "slot outside the gateway range".into()))?;
        let base = slot_base(slot);
        let dir = self.apps().join(app.as_str());
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| (codes::IO, format!("mkdir: {e}")))?;
            for sub in ["data", "cache"] {
                let p = dir.join(sub);
                if !p.exists() {
                    std::fs::DirBuilder::new().mode(0o700).create(&p).map_err(|e| (codes::IO, format!("mkdir {sub}: {e}")))?;
                }
            }
        }
        // The App's root (its slot's first id) owns data/ and cache/. Only
        // the top directories: what the App wrote is already its own.
        let ids = format!("{base}:{base}");
        let data = dir.join("data").to_string_lossy().to_string();
        let cache = dir.join("cache").to_string_lossy().to_string();
        self.podman(&["unshare", "chown", &ids, &data, &cache], None, 60)?;
        // Pay the first start of this id mapping now, before any switch
        // (W §5.5: 13–20 s per new mapping on the first start).
        let warm = format!("k2warm-{}", app.as_str());
        let _ = self.run(PODMAN, &["rm", "-f", "-i", &warm], None, 60);
        let map = format!("0:{base}:{}", wire::SLOT_SIZE);
        let rt = format!("--runtime={}", self.runtime());
        let t0 = std::time::Instant::now();
        self.podman(&[&rt, "create", "--pull=never", "--name", &warm, "--uidmap", &map, "--gidmap", &map, image, "true"], None, 900)?;
        let warm_ms = t0.elapsed().as_millis() as u64;
        let _ = self.run(PODMAN, &["rm", "-f", "-i", &warm], None, 60);
        Ok(json!({ "slot": slot, "gwPort": gw, "base": base, "warmMs": warm_ms }))
    }

    fn secret_name(app: &AppId, dep: &str, tag: &str) -> String {
        format!("k2-{}--{dep}--{tag}", app.as_str())
    }

    fn start(&self, s: &StartSpec) -> R<Value> {
        let app = check_start(s).map_err(|w| bad(format!("start: {w}")))?;
        let slot = self.slot_of(&app)?;
        let gw = gateway_port(slot).ok_or((codes::NO_SLOT, "slot outside the gateway range".into()))?;
        let seed_dir = self.seeds().join(&s.seed);
        if !seed_dir.is_dir() {
            return Err((codes::NOT_FOUND, "Seed not on the engine".into()));
        }
        let stem = unit_stem(&app, &s.deployment);
        let unit = format!("{stem}.service");
        let run_dir = self.run_dir(&app);
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&run_dir).map_err(|e| (codes::IO, format!("mkdir: {e}")))?;
        }
        // Environment: the daemon's (identity, declared) + the engine's own.
        let mut env = s.env.clone();
        for k in ["PORT", "HOME", "TMPDIR", "NODE_OPTIONS", "K2_PASS_FILE", "K2_SECRETS_DIR", "K2_APP_DATA", "K2_APP_CACHE"] {
            env.remove(k);
        }
        env.insert("PORT".into(), s.port.to_string());
        env.insert("HOME".into(), "/app-home".into());
        env.insert("TMPDIR".into(), "/tmp".into());
        env.insert("K2_APP_DATA".into(), "/data".into());
        env.insert("K2_APP_CACHE".into(), "/cache".into());
        // AR45: V8 heap at 50% of the cap, so the heap error (with a stack)
        // comes before the kernel's kill; a fatal-error report for agents.
        env.insert(
            "NODE_OPTIONS".into(),
            format!("--max-old-space-size={} --report-on-fatalerror --report-directory=/data/.k2-diagnostics", s.memory_mib / 2),
        );
        let mut mounts = Vec::new();
        let mut made = Vec::new();
        let cleanup = |made: &[String]| {
            for n in made {
                let _ = self.run(PODMAN, &["secret", "rm", n], None, 30);
            }
        };
        let mut files = false;
        for (i, it) in s.secrets.iter().enumerate() {
            let name = Self::secret_name(&app, &s.deployment, &i.to_string());
            let val = hex_decode(&it.value_hex).ok_or_else(|| bad("secret value"))?;
            if let Err(e) = self.podman(&["secret", "create", "--replace", &name, "-"], Some(val), 60) {
                cleanup(&made);
                return Err(e);
            }
            made.push(name.clone());
            match (&it.env, &it.file) {
                (Some(var), _) => mounts.push(SecretMount::Env { secret: name, var: var.clone() }),
                (None, Some(f)) => {
                    files = true;
                    mounts.push(SecretMount::File { secret: name, target: format!("{SECRETS_TARGET_DIR}/{f}") })
                }
                _ => {}
            }
        }
        if files {
            env.insert("K2_SECRETS_DIR".into(), SECRETS_TARGET_DIR.into());
        }
        if let Some(p) = &s.pass_hex {
            let name = Self::secret_name(&app, &s.deployment, "pass");
            let val = hex_decode(p).ok_or_else(|| bad("pass"))?;
            if let Err(e) = self.podman(&["secret", "create", "--replace", &name, "-"], Some(val), 60) {
                cleanup(&made);
                return Err(e);
            }
            made.push(name.clone());
            mounts.push(SecretMount::File { secret: name, target: PASS_TARGET.into() });
            env.insert("K2_PASS_FILE".into(), PASS_TARGET.into());
        }
        let env_file = run_dir.join(format!("{}.env", s.deployment));
        let env_text: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        if let Err(e) = sys::write_atomic(&env_file, env_text.as_bytes(), 0o600) {
            cleanup(&made);
            return Err((codes::IO, e));
        }
        let seed_s = seed_dir.to_string_lossy().to_string();
        let data_s = self.apps().join(app.as_str()).join("data").to_string_lossy().to_string();
        let cache_s = self.apps().join(app.as_str()).join("cache").to_string_lossy().to_string();
        let env_s = env_file.to_string_lossy().to_string();
        let sec_s = self.seccomp_path().to_string_lossy().to_string();
        let unit_text = quadlet::render(&quadlet::Render {
            app: &app,
            deployment: &s.deployment,
            image: &s.image,
            runtime: self.runtime(),
            base: slot_base(slot),
            seed_dir: &seed_s,
            data_dir: &data_s,
            cache_dir: &cache_s,
            env_file: &env_s,
            seccomp: &sec_s,
            gw_port: gw,
            port: s.port,
            argv: &s.argv,
            memory_mib: s.memory_mib,
            drain_secs: s.drain_secs,
            secrets: &mounts,
        });
        let qfile = self.quadlets().join(format!("{stem}.container"));
        let state = RunState { app: app.as_str().into(), deployment: s.deployment.clone(), port: s.port, gw_port: gw, slot, unit: unit.clone(), secrets: made.clone() };
        let undo = |this: &Self| {
            let _ = std::fs::remove_file(&qfile);
            let _ = std::fs::remove_file(&env_file);
            let _ = std::fs::remove_file(run_dir.join(format!("{}.json", s.deployment)));
            cleanup(&made);
            let _g = this.lock.lock().unwrap_or_else(|e| e.into_inner());
            let _ = this.systemctl(&["daemon-reload"], 60);
        };
        {
            let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = sys::write_atomic(&qfile, unit_text.as_bytes(), 0o600)
                .and_then(|_| sys::write_atomic(&run_dir.join(format!("{}.json", s.deployment)), &serde_json::to_vec(&state).unwrap_or_default(), 0o600))
            {
                drop(_g);
                undo(self);
                return Err((codes::IO, e));
            }
            let r = self.systemctl(&["daemon-reload"], 120)?;
            if !r.ok() {
                drop(_g);
                undo(self);
                return Err((codes::SYSTEMD, format!("daemon-reload: {}", r.why())));
            }
        }
        // Quadlet generated the unit; start blocks until conmon reports the
        // container running (the daemon's HTTP probe decides health).
        let r = self.systemctl(&["start", &unit], 360)?;
        if !r.ok() {
            let tail = self.journal_tail(&unit, 30);
            let _ = self.systemctl(&["stop", &unit], 120);
            undo(self);
            return Err((codes::SYSTEMD, format!("start {unit}: {}; last log lines: {tail}", r.why())));
        }
        let pid = self.show(&unit, "MainPID").and_then(|p| p.parse::<i32>().ok()).filter(|p| *p > 0);
        Ok(json!({ "unit": unit, "gwPort": gw, "slot": slot, "pid": pid, "slice": app_slice(&app) }))
    }

    fn journal_tail(&self, unit: &str, n: u32) -> String {
        let lines = n.to_string();
        self.run(JOURNALCTL, &["--user", "-u", unit, "-n", &lines, "-o", "cat", "--no-pager"], None, 30)
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect())
            .unwrap_or_default()
    }

    fn show(&self, unit: &str, prop: &str) -> Option<String> {
        let o = self.systemctl(&["show", "-p", prop, "--value", unit], 30).ok()?;
        o.ok().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    fn ids(app: &str, dep: &str) -> R<(AppId, String)> {
        let a = AppId::parse(app).ok_or_else(|| bad("app id"))?;
        if !deployment_ok(dep) {
            return Err(bad("deployment id"));
        }
        let unit = format!("{}.service", unit_stem(&a, dep));
        Ok((a, unit))
    }

    fn stop(&self, app: &str, dep: &str) -> R<Value> {
        let (a, unit) = Self::ids(app, dep)?;
        // StopTimeout (drain + 10 s) lets the App drain; systemd's own
        // TimeoutStopSec bounds it.
        let r = self.systemctl(&["stop", &unit], 900)?;
        let st_path = self.run_dir(&a).join(format!("{dep}.json"));
        let secrets = std::fs::read(&st_path).ok().and_then(|b| serde_json::from_slice::<RunState>(&b).ok()).map(|s| s.secrets).unwrap_or_default();
        for n in &secrets {
            let _ = self.run(PODMAN, &["secret", "rm", n], None, 30);
        }
        let _ = std::fs::remove_file(self.quadlets().join(format!("{}.container", unit_stem(&a, dep))));
        let _ = std::fs::remove_file(self.run_dir(&a).join(format!("{dep}.env")));
        let _ = std::fs::remove_file(&st_path);
        {
            let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let _ = self.systemctl(&["daemon-reload"], 120);
        }
        Ok(json!({ "stopped": r.ok(), "unit": unit }))
    }

    fn liveness(&self, app: &str, dep: &str) -> R<Value> {
        let (_, unit) = Self::ids(app, dep)?;
        let o = self.systemctl(&["show", "-p", "LoadState", "-p", "ActiveState", "-p", "ExecMainStatus", "-p", "MainPID", &unit], 30)?;
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
        let state = match (get("LoadState").as_str(), get("ActiveState").as_str()) {
            ("not-found", _) => "exited",
            (_, "active" | "activating" | "reloading") => "running",
            (_, "inactive" | "failed" | "deactivating") => "exited",
            _ => "unknown",
        };
        let code: Option<i32> = get("ExecMainStatus").parse().ok();
        Ok(json!({ "state": state, "code": code, "activeState": get("ActiveState"), "pid": get("MainPID").parse::<i32>().ok().filter(|p| *p > 0) }))
    }

    fn stats(&self, app: &str, dep: &str) -> R<Value> {
        let (_, unit) = Self::ids(app, dep)?;
        let cg = self.show(&unit, "ControlGroup").unwrap_or_default();
        if cg.is_empty() || !cg.starts_with('/') || cg.contains("..") {
            return Ok(json!({ "memCurrent": null, "pids": null }));
        }
        let read = |f: &str| std::fs::read_to_string(format!("/sys/fs/cgroup{cg}/{f}")).ok().and_then(|s| s.trim().parse::<u64>().ok());
        Ok(json!({ "memCurrent": read("memory.current"), "pids": read("pids.current"), "memMax": read("memory.max"), "cgroup": cg }))
    }

    fn logs(&self, app: &str, dep: &str, lines: u32) -> R<Value> {
        let (_, unit) = Self::ids(app, dep)?;
        let n = lines.clamp(1, 5000).to_string();
        let o = self.run(JOURNALCTL, &["--user", "-u", &unit, "-n", &n, "-o", "cat", "--no-pager"], None, 60)?;
        if !o.ok() {
            return Err((codes::IO, format!("journalctl: {}", o.why())));
        }
        Ok(json!({ "text": String::from_utf8_lossy(&o.stdout), "truncated": o.stdout.len() >= MAX_OUTPUT }))
    }

    fn exec(&self, app: &str, dep: &str, argv: &[String]) -> R<Value> {
        let (a, _) = Self::ids(app, dep)?;
        if argv.is_empty() || argv.len() > 64 || !argv.iter().all(|x| !x.is_empty() && wire::value_ok(x, 4096)) {
            return Err(bad("argv"));
        }
        let ctr = unit_stem(&a, dep);
        let mut args: Vec<&str> = vec!["exec", &ctr];
        args.extend(argv.iter().map(String::as_str));
        let o = self.run(PODMAN, &args, None, 120)?;
        let mut out = o.stdout.clone();
        out.extend_from_slice(&o.stderr);
        out.truncate(MAX_OUTPUT);
        Ok(json!({ "exitCode": if o.timed_out { None } else { Some(o.status) }, "output": String::from_utf8_lossy(&out) }))
    }

    fn run_states(&self) -> Vec<RunState> {
        let mut out = Vec::new();
        let Ok(apps) = std::fs::read_dir(self.state().join("run")) else { return out };
        for a in apps.flatten() {
            let Ok(files) = std::fs::read_dir(a.path()) else { continue };
            for f in files.flatten() {
                if f.path().extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Some(s) = std::fs::read(f.path()).ok().and_then(|b| serde_json::from_slice::<RunState>(&b).ok()) {
                    if AppId::parse(&s.app).is_some() && deployment_ok(&s.deployment) {
                        out.push(s);
                    }
                }
            }
        }
        out
    }

    fn adopt(&self) -> R<Value> {
        let mut live = Vec::new();
        for s in self.run_states() {
            let st = self.liveness(&s.app, &s.deployment)?;
            if st["state"] == "running" {
                live.push(json!({ "app": s.app, "deployment": s.deployment, "port": s.port, "gwPort": s.gw_port, "unit": s.unit, "pid": st["pid"] }));
            }
        }
        Ok(json!({ "instances": live }))
    }

    fn remove_app(&self, app: &str, data: bool) -> R<Value> {
        let a = AppId::parse(app).ok_or_else(|| bad("app id"))?;
        let mut stopped = Vec::new();
        for s in self.run_states().into_iter().filter(|s| s.app == a.as_str()) {
            self.stop(&s.app, &s.deployment)?;
            stopped.push(s.deployment);
        }
        let _ = std::fs::remove_dir(self.run_dir(&a));
        let mut purged = false;
        if data {
            let dir = self.apps().join(a.as_str());
            if dir.exists() {
                // Files there belong to the App's mapped ids: remove them
                // inside k2app's namespace.
                let d = dir.to_string_lossy().to_string();
                self.podman(&["unshare", "rm", "-rf", "--one-file-system", &d], None, 600)?;
            }
            let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut s = crate::slots::load(&self.slots_path()).map_err(|e| (codes::IO, e))?;
            if s.free(a.as_str()) {
                crate::slots::save(&self.slots_path(), &s).map_err(|e| (codes::IO, e))?;
            }
            purged = true;
        }
        Ok(json!({ "stopped": stopped, "purged": purged }))
    }
}

fn rand_suffix() -> u64 {
    let mut b = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    u64::from_le_bytes(b)
}

// ── the socket ──────────────────────────────────────────────────────────

/// One line of the engine's own journal per request (never secrets).
fn audit(verb: &str, who: &str, ok: bool, code: Option<&str>) {
    eprintln!("k2-app-engine: {}", json!({ "verb": verb, "caller": who, "ok": ok, "code": code }));
}

fn verb_name(r: &Request) -> &'static str {
    match r {
        Request::Version => "version",
        Request::SeedHas { .. } => "seed-has",
        Request::SeedPut { .. } => "seed-put",
        Request::ImageHas { .. } => "image-has",
        Request::ImageLoad { .. } => "image-load",
        Request::Prepare { .. } => "prepare",
        Request::Start(_) => "start",
        Request::Stop { .. } => "stop",
        Request::Liveness { .. } => "liveness",
        Request::Stats { .. } => "stats",
        Request::Logs { .. } => "logs",
        Request::Exec { .. } => "exec",
        Request::Adopt => "adopt",
        Request::RemoveApp { .. } => "remove-app",
    }
}

fn answer(conn: &mut UnixStream, a: &Answer) {
    let mut line = serde_json::to_vec(a).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    line.push(b'\n');
    let _ = conn.write_all(&line);
    let _ = conn.flush();
    // Close gently: closing with unread request bytes makes the kernel send
    // a reset, and the caller would lose this answer (a refusal answers
    // before the request is read). Shut our side, then discard what the
    // caller still sends (bounded in bytes and time) — never parsed.
    let _ = conn.shutdown(std::net::Shutdown::Write);
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = std::io::copy(&mut (&mut *conn).take(8 << 20), &mut std::io::sink());
}

/// Read one header line (at most `MAX_HEADER` bytes).
fn read_header(r: &mut BufReader<UnixStream>) -> Result<Vec<u8>, &'static str> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(MAX_HEADER as u64 + 1).read_until(b'\n', &mut buf).map_err(|_| "read failed")?;
    if n == 0 {
        return Err("empty request");
    }
    if buf.last() != Some(&b'\n') {
        return Err("request header too long or unterminated");
    }
    buf.pop();
    Ok(buf)
}

pub fn serve_conn(engine: &Engine, conn: UnixStream, admit: &dyn Fn(i32) -> Result<String, &'static str>) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(600)));
    let _ = conn.set_write_timeout(Some(Duration::from_secs(60)));
    let mut out = match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    // The caller check comes first: nothing of the request is read before.
    let who = match admit(conn.as_raw_fd()) {
        Ok(u) => u,
        Err(why) => {
            audit("-", &format!("refused: {why}"), false, Some(codes::CALLER_NOT_DAEMON));
            answer(&mut out, &Answer::err(codes::CALLER_NOT_DAEMON, why));
            return;
        }
    };
    let mut r = BufReader::new(conn);
    let req = match read_header(&mut r).and_then(|h| serde_json::from_slice::<Request>(&h).map_err(|_| "unknown or malformed request")) {
        Ok(q) => q,
        Err(e) => {
            audit("-", &who, false, Some(codes::BAD_REQUEST));
            answer(&mut out, &Answer::err(codes::BAD_REQUEST, e));
            return;
        }
    };
    let verb = verb_name(&req);
    let res = engine.handle(req, &mut r);
    match res {
        Ok(v) => {
            audit(verb, &who, true, None);
            answer(&mut out, &Answer::ok(v));
        }
        Err((code, msg)) => {
            audit(verb, &who, false, Some(code));
            answer(&mut out, &Answer::err(code, msg));
        }
    }
}

/// Bind the socket (replacing a stale one) and serve forever.
pub fn serve(engine: Engine, socket: &Path) -> Result<(), String> {
    if let Ok(m) = std::fs::symlink_metadata(socket) {
        use std::os::unix::fs::FileTypeExt;
        if m.file_type().is_socket() {
            let _ = std::fs::remove_file(socket);
        } else {
            return Err(format!("{} exists and is not a socket", socket.display()));
        }
    }
    let l = UnixListener::bind(socket).map_err(|e| format!("bind {}: {e}", socket.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        // The directory's ACL decides who can reach the socket (k2app and
        // the daemon users); every connection is then caller-checked.
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o666)).map_err(|e| format!("chmod socket: {e}"))?;
    }
    let engine = std::sync::Arc::new(engine);
    let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for c in l.incoming() {
        let Ok(mut c) = c else { continue };
        if active.load(std::sync::atomic::Ordering::SeqCst) >= 64 {
            answer(&mut c, &Answer::err(codes::IO, "busy"));
            continue;
        }
        let e = engine.clone();
        let a = active.clone();
        a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let spawned = std::thread::Builder::new().name("engine-conn".into()).spawn(move || {
            let passwd = std::fs::read_to_string(k2_root_proto::apps::PASSWD_FILE).unwrap_or_default();
            let q = crate::peer::EngineQuery { passwd };
            serve_conn(&e, c, &|fd| crate::peer::admit(fd, &q));
            a.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        });
        if spawned.is_err() {
            active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    Ok(())
}

/// The production engine: must run as `k2app`, in its user manager.
pub fn production() -> Result<Engine, String> {
    let passwd = std::fs::read_to_string(k2_root_proto::apps::PASSWD_FILE).map_err(|e| format!("read /etc/passwd: {e}"))?;
    let (uid, _, home) = sys::passwd_entry(&passwd, APP_USER).ok_or("no k2app user (box prep: apps ensure-user)")?;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    if me != uid {
        return Err(format!("k2-app-engine runs only as {APP_USER} (uid {uid}), not uid {me}"));
    }
    if home != APP_HOME {
        return Err(format!("{APP_USER}'s home is {home}, expected {APP_HOME}"));
    }
    let rt = format!("/run/user/{uid}");
    if !Path::new(&rt).is_dir() {
        return Err(format!("{rt} missing: the engine runs in k2app's lingering user manager"));
    }
    Ok(Engine::new(PathBuf::from(APP_HOME), rt))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire end to end over a socket pair with a fake caller check:
    /// a refused caller gets `caller_not_daemon` and nothing is read; a
    /// malformed or unknown verb is refused; a real verb answers.
    #[test]
    fn caller_check_comes_first_and_bad_requests_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(d.path().to_path_buf(), "/tmp".into());
        let ask = |line: &str, admitted: bool| -> Answer {
            let (mut a, b) = UnixStream::pair().unwrap();
            a.write_all(line.as_bytes()).unwrap();
            a.shutdown(std::net::Shutdown::Write).unwrap();
            let admit = move |_fd: i32| if admitted { Ok("k2".to_string()) } else { Err("caller is not the daemon unit's main process") };
            serve_conn(&e, b, &admit);
            let mut s = String::new();
            a.read_to_string(&mut s).unwrap();
            serde_json::from_str(s.trim()).unwrap()
        };
        let a = ask(&format!("{{\"verb\":\"seed-has\",\"hash\":\"{}\"}}\n", "a".repeat(64)), false);
        assert!(!a.ok);
        assert_eq!(a.code.as_deref(), Some(codes::CALLER_NOT_DAEMON));
        let a = ask(&format!("{{\"verb\":\"seed-has\",\"hash\":\"{}\"}}\n", "a".repeat(64)), true);
        assert!(a.ok, "{a:?}");
        assert_eq!(a.data["present"], false);
        std::fs::create_dir_all(d.path().join("seeds").join("a".repeat(64))).unwrap();
        let a = ask(&format!("{{\"verb\":\"seed-has\",\"hash\":\"{}\"}}\n", "a".repeat(64)), true);
        assert_eq!(a.data["present"], true);
        for bad_line in [
            "{\"verb\":\"shell\",\"cmd\":\"id\"}\n",
            "{\"verb\":\"seed-has\",\"hash\":\"../../etc\"}\n",
            "not json\n",
            "{\"verb\":\"seed-has\",\"hash\":\"aa\"}",
        ] {
            let a = ask(bad_line, true);
            assert!(!a.ok, "{bad_line}");
            assert_eq!(a.code.as_deref(), Some(codes::BAD_REQUEST), "{bad_line}");
        }
        let a = ask("{\"verb\":\"stop\",\"app\":\"../x\",\"deployment\":\"d-0123456789ab\"}\n", true);
        assert_eq!(a.code.as_deref(), Some(codes::BAD_REQUEST));
        let a = ask("{\"verb\":\"exec\",\"app\":\"jobs\",\"deployment\":\"d-0123456789ab\",\"argv\":[]}\n", true);
        assert_eq!(a.code.as_deref(), Some(codes::BAD_REQUEST));
    }

    #[test]
    fn a_seed_put_streams_the_body() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(d.path().to_path_buf(), "/tmp".into());
        std::fs::create_dir_all(d.path().join("seeds")).unwrap();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(2);
        h.set_mode(0o644);
        b.append_data(&mut h, "index.js", &b"1;"[..]).unwrap();
        let t = b.into_inner().unwrap();
        let hash = "b".repeat(64);
        let (mut a, s) = UnixStream::pair().unwrap();
        let header = format!("{{\"verb\":\"seed-put\",\"hash\":\"{hash}\",\"len\":{}}}\n", t.len());
        a.write_all(header.as_bytes()).unwrap();
        a.write_all(&t).unwrap();
        a.shutdown(std::net::Shutdown::Write).unwrap();
        serve_conn(&e, s, &|_| Ok("k2".into()));
        let mut out = String::new();
        a.read_to_string(&mut out).unwrap();
        let ans: Answer = serde_json::from_str(out.trim()).unwrap();
        assert!(ans.ok, "{ans:?}");
        assert_eq!(std::fs::read_to_string(d.path().join("seeds").join(&hash).join("index.js")).unwrap(), "1;");
    }

    #[test]
    fn run_state_round_trip() {
        let s = RunState { app: "jobs".into(), deployment: "d-0123456789ab".into(), port: 41000, gw_port: 38600, slot: 0, unit: "k2app-jobs--d-0123456789ab.service".into(), secrets: vec![] };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<RunState>(&j).unwrap(), s);
        let _ = StartSpec::clone;
    }
}
