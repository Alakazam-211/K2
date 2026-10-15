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
    self as wire, app_slice, check_start, codes, deployment_ok, gateway_port, hex_decode, image_ok, pass_hex_ok, proxy_port, seed_hash_ok,
    slot_base, unit_stem, Answer, Request, StartSpec, EXEC_TIMEOUT_BOUNDS, MAX_HEADER, MAX_IMAGE_BYTES, MAX_OUTPUT, MAX_SEED_BYTES,
    PASS_FILE_NAME, PASS_ROOT, PASS_TARGET, SECRETS_TARGET_DIR,
};
use serde_json::{json, Value};

use crate::quadlet::{self, SecretMount};
use crate::sys::{self, Out};

pub const PODMAN: &str = "/usr/bin/podman";
pub const SYSTEMCTL: &str = "/usr/bin/systemctl";

type R<T> = Result<T, (&'static str, String)>;

fn bad(m: impl Into<String>) -> (&'static str, String) {
    (codes::BAD_REQUEST, m.into())
}

/// The running engine.
pub struct Engine {
    pub home: PathBuf,
    pub runtime_dir: String,
    /// The App pass folders' root (tmpfs [`PASS_ROOT`] in production).
    pub pass_root: PathBuf,
    /// Serializes slot changes and unit-file writes + `daemon-reload`.
    lock: Mutex<()>,
    /// `/etc/subuid` and `/etc/subgid` (tests point these elsewhere).
    subid_files: (PathBuf, PathBuf),
}

/// One instance's state file (`<state>/run/<app>/<deployment>.json`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunState {
    pub app: String,
    pub deployment: String,
    pub port: u16,
    pub gw_port: u16,
    /// The App's egress proxy port (S15). 0 in a state file written by a
    /// protocol-1 engine (that container has no proxy forward).
    #[serde(default)]
    pub px_port: u16,
    pub slot: u64,
    pub unit: String,
    pub secrets: Vec<String>,
}

impl Engine {
    pub fn new(home: PathBuf, runtime_dir: String) -> Self {
        Self {
            home,
            runtime_dir,
            pass_root: PathBuf::from(PASS_ROOT),
            lock: Mutex::new(()),
            subid_files: (PathBuf::from(SUBUID_FILE), PathBuf::from(SUBGID_FILE)),
        }
    }

    /// Tests: keep the pass folders somewhere writable.
    pub fn with_pass_root(mut self, root: PathBuf) -> Self {
        self.pass_root = root;
        self
    }

    fn pass_dir(&self, app: &AppId) -> PathBuf {
        self.pass_root.join(app.as_str())
    }

    /// Write the App's pass file (S14): `pass/` 0700 (k2app only, so the
    /// daemon user's agents can't read it), `pass/<app>/` 0755 and the
    /// file 0444 (the container's root is "other" to k2app), swapped in
    /// by a rename so every mounted instance sees the whole new pass.
    fn write_pass(&self, app: &AppId, pass_hex: &str) -> R<PathBuf> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !pass_hex_ok(pass_hex) {
            return Err(bad("pass"));
        }
        let raw = hex_decode(pass_hex).ok_or_else(|| bad("pass"))?;
        let dir = self.pass_dir(app);
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        for (d, mode) in [(&self.pass_root, 0o700), (&dir, 0o755)] {
            if !d.is_dir() {
                std::fs::DirBuilder::new().recursive(true).mode(mode).create(d).map_err(|e| (codes::IO, format!("mkdir {}: {e}", d.display())))?;
            }
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(mode)).map_err(|e| (codes::IO, format!("chmod {}: {e}", d.display())))?;
        }
        let file = dir.join(PASS_FILE_NAME);
        sys::write_atomic(&file, &raw, 0o444).map_err(|e| (codes::IO, e))?;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).map_err(|e| (codes::IO, format!("chmod pass: {e}")))?;
        // The process exception reads its pass from its own process folder
        // (bound read-only into its unit; `K2_PASS_FILE`): same swap.
        let proc_dir = self.apps().join(app.as_str()).join("process");
        if proc_dir.is_dir() {
            let pf = proc_dir.join(PASS_FILE_NAME);
            sys::write_atomic(&pf, &raw, 0o644).map_err(|e| (codes::IO, e))?;
            std::fs::set_permissions(&pf, std::fs::Permissions::from_mode(0o644)).map_err(|e| (codes::IO, format!("chmod pass: {e}")))?;
        }
        Ok(dir)
    }

    /// Read the subordinate id files from elsewhere (tests).
    pub fn with_subid_files(mut self, subuid: PathBuf, subgid: PathBuf) -> Self {
        self.subid_files = (subuid, subgid);
        self
    }

    pub(crate) fn seeds(&self) -> PathBuf {
        self.home.join("seeds")
    }
    pub(crate) fn apps(&self) -> PathBuf {
        self.home.join("apps")
    }
    pub(crate) fn state(&self) -> PathBuf {
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

    /// What Podman gets as `--runtime`: the OCI shim for the configured
    /// runtime (`oci.rs`), never the runtime itself.
    pub fn runtime_arg(&self) -> String {
        crate::oci::shim_path(&self.state(), self.runtime()).to_string_lossy().to_string()
    }

    /// Make the engine's own folders, the OCI shim links and K2's seccomp
    /// profile.
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
        crate::oci::install_links(&self.state(), Path::new(k2_root_proto::apps::ENGINE_PATH))?;
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
            Request::Exec { app, deployment, argv, timeout_secs } => self.exec(&app, &deployment, &argv, timeout_secs),
            // Answered in `serve_conn` (it streams); never reaches here.
            Request::ExecTty { .. } => Err(bad("exec-tty streams; it has no one-line answer")),
            Request::ProcessStage(spec) => crate::process::stage(self, &spec),
            Request::ProcessActivate { app, deployment } => crate::process::activate(self, &app, &deployment),
            Request::ProcessState { app } => crate::process::state(self, &app),
            Request::Adopt => self.adopt(),
            Request::RemoveApp { app, data } => self.remove_app(&app, data),
            Request::PassWrite { app, pass_hex } => {
                let a = AppId::parse(&app).ok_or_else(|| bad("app id"))?;
                self.write_pass(&a, &pass_hex)?;
                Ok(json!({ "written": true }))
            }
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

    /// The App's slot, assigned now if it has none (the process
    /// exception needs only its gateway port from it).
    pub(crate) fn assign_slot(&self, app: &AppId) -> R<u64> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = crate::slots::load(&self.slots_path()).map_err(|e| (codes::IO, e))?;
        let cap = self.capacity();
        let n = s.assign(app.as_str(), cap).ok_or_else(|| {
            (codes::NO_SLOT, format!("no free id slot for another App ({cap} slots from k2app's subordinate ids are in use)"))
        })?;
        crate::slots::save(&self.slots_path(), &s).map_err(|e| (codes::IO, e))?;
        Ok(n)
    }

    fn capacity(&self) -> u64 {
        let su = std::fs::read_to_string(&self.subid_files.0).unwrap_or_default();
        let sg = std::fs::read_to_string(&self.subid_files.1).unwrap_or_default();
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
        let px = proxy_port(slot).ok_or((codes::NO_SLOT, "slot outside the proxy range".into()))?;
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
        let rt = format!("--runtime={}", self.runtime_arg());
        let t0 = std::time::Instant::now();
        self.podman(&[&rt, "create", "--pull=never", "--name", &warm, "--uidmap", &map, "--gidmap", &map, image, "true"], None, 900)?;
        let warm_ms = t0.elapsed().as_millis() as u64;
        let _ = self.run(PODMAN, &["rm", "-f", "-i", &warm], None, 60);
        Ok(json!({ "slot": slot, "gwPort": gw, "pxPort": px, "base": base, "warmMs": warm_ms }))
    }

    fn secret_name(app: &AppId, dep: &str, tag: &str) -> String {
        format!("k2-{}--{dep}--{tag}", app.as_str())
    }

    fn start(&self, s: &StartSpec) -> R<Value> {
        let app = check_start(s).map_err(|w| bad(format!("start: {w}")))?;
        let slot = self.slot_of(&app)?;
        let gw = gateway_port(slot).ok_or((codes::NO_SLOT, "slot outside the gateway range".into()))?;
        let px = proxy_port(slot).ok_or((codes::NO_SLOT, "slot outside the proxy range".into()))?;
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
        // S15: the App's only way out is its own egress proxy (the daemon
        // listens on the forwarded port and enforces the allowlist). These
        // replace any value the daemon's env carried.
        for (k, v) in wire::proxy_env(px) {
            env.insert(k.into(), v);
        }
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
        let mut pass_dir = None;
        if let Some(p) = &s.pass_hex {
            match self.write_pass(&app, p) {
                Ok(d) => pass_dir = Some(d.to_string_lossy().to_string()),
                Err(e) => {
                    cleanup(&made);
                    return Err(e);
                }
            }
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
        let runtime = self.runtime_arg();
        let unit_text = quadlet::render(&quadlet::Render {
            app: &app,
            deployment: &s.deployment,
            image: &s.image,
            runtime: &runtime,
            base: slot_base(slot),
            seed_dir: &seed_s,
            data_dir: &data_s,
            cache_dir: &cache_s,
            env_file: &env_s,
            seccomp: &sec_s,
            gw_port: gw,
            px_port: px,
            port: s.port,
            argv: &s.argv,
            memory_mib: s.memory_mib,
            drain_secs: s.drain_secs,
            secrets: &mounts,
            pass_dir: pass_dir.as_deref(),
            io: &s.io,
        });
        let qfile = self.quadlets().join(format!("{stem}.container"));
        let state = RunState { app: app.as_str().into(), deployment: s.deployment.clone(), port: s.port, gw_port: gw, px_port: px, slot, unit: unit.clone(), secrets: made.clone() };
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
            let _ = self.systemctl(&["stop", &unit], 120);
            undo(self);
            return Err((codes::SYSTEMD, format!("start {unit}: {} (its output: k2 app logs)", r.why())));
        }
        let pid = self.show(&unit, "MainPID").and_then(|p| p.parse::<i32>().ok()).filter(|p| *p > 0);
        Ok(json!({ "unit": unit, "gwPort": gw, "pxPort": px, "slot": slot, "pid": pid, "slice": app_slice(&app) }))
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

    /// S28 one-shot exec: `podman exec` into the deployment's container
    /// (the App's own user, capabilities, seccomp; the OCI shim adds its
    /// MAC profile and no-new-privileges to the exec process). No stdin,
    /// no tty, killed at the timeout. Output is raw here; the daemon frames
    /// and strips it.
    fn exec(&self, app: &str, dep: &str, argv: &[String], timeout_secs: u64) -> R<Value> {
        let (a, _) = Self::ids(app, dep)?;
        if !wire::exec_argv_ok(argv) {
            return Err(bad("argv"));
        }
        if !(EXEC_TIMEOUT_BOUNDS.0..=EXEC_TIMEOUT_BOUNDS.1).contains(&timeout_secs) {
            return Err(bad("timeout"));
        }
        let ctr = unit_stem(&a, dep);
        let mut args: Vec<&str> = vec!["exec", "--interactive=false", &ctr];
        args.extend(argv.iter().map(String::as_str));
        let t0 = std::time::Instant::now();
        let o = self.run(PODMAN, &args, None, timeout_secs)?;
        let mut out = o.stdout.clone();
        out.extend_from_slice(&o.stderr);
        let bytes = out.len();
        out.truncate(MAX_OUTPUT);
        Ok(json!({
            "exitCode": if o.timed_out { None } else { Some(o.status) },
            "timedOut": o.timed_out,
            "bytes": bytes,
            "ms": t0.elapsed().as_millis() as u64,
            "output": String::from_utf8_lossy(&out),
        }))
    }

    /// The `podman exec -it` command for `k2 app shell` (S28).
    pub fn exec_tty_command(&self, app: &str, dep: &str, argv: &[String]) -> R<Command> {
        let (a, _) = Self::ids(app, dep)?;
        if !wire::exec_argv_ok(argv) {
            return Err(bad("argv"));
        }
        let ctr = unit_stem(&a, dep);
        let mut c = self.cmd(PODMAN);
        // No detach keys: every byte goes to the command.
        c.args(["exec", "--interactive", "--tty", "--detach-keys=", &ctr]);
        c.args(argv);
        Ok(c)
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
                live.push(json!({ "app": s.app, "deployment": s.deployment, "port": s.port, "gwPort": s.gw_port, "pxPort": s.px_port, "unit": s.unit, "pid": st["pid"] }));
            }
        }
        Ok(json!({ "instances": live }))
    }

    fn remove_app(&self, app: &str, data: bool) -> R<Value> {
        let a = AppId::parse(app).ok_or_else(|| bad("app id"))?;
        // The process exception's code and stages (its state under
        // /var/lib/k2appx is the root helper's to purge).
        crate::process::remove(self, &a);
        let mut stopped = Vec::new();
        for s in self.run_states().into_iter().filter(|s| s.app == a.as_str()) {
            self.stop(&s.app, &s.deployment)?;
            stopped.push(s.deployment);
        }
        let _ = std::fs::remove_dir(self.run_dir(&a));
        // The App's pass goes with its last instance (the daemon revoked it).
        let _ = std::fs::remove_dir_all(self.pass_dir(&a));
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
        Request::Exec { .. } => "exec",
        Request::ExecTty { .. } => "exec-tty",
        Request::Adopt => "adopt",
        Request::RemoveApp { .. } => "remove-app",
        Request::PassWrite { .. } => "pass-write",
        Request::ProcessStage(_) => "process-stage",
        Request::ProcessActivate { .. } => "process-activate",
        Request::ProcessState { .. } => "process-state",
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
    // A terminal: answer, then stream frames until the command exits.
    if let Request::ExecTty { app, deployment, argv, cols, rows } = &req {
        match engine.exec_tty_command(app, deployment, argv) {
            Err((code, msg)) => {
                audit(verb, &who, false, Some(code));
                answer(&mut out, &Answer::err(code, msg));
            }
            Ok(cmd) => {
                let mut line = serde_json::to_vec(&Answer::ok(json!({ "streaming": true }))).unwrap_or_default();
                line.push(b'\n');
                if out.write_all(&line).and_then(|_| out.flush()).is_err() {
                    return;
                }
                // The daemon keeps the session (idle limit 15 min); allow
                // a little more here before the input side gives up.
                let conn = r.into_inner();
                let _ = conn.set_read_timeout(Some(Duration::from_secs(16 * 60)));
                let res = crate::tty::bridge(cmd, *cols, *rows, conn);
                audit(verb, &who, res.is_ok(), res.as_ref().err().map(|_| codes::IO));
            }
        }
        return;
    }
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

    /// S14: the pass file lands in the App's pass folder with the modes
    /// the wall needs (folder root k2app-only, App folder and file
    /// world-readable for the container's mapped root), is replaced whole
    /// on rotation, and a malformed pass is refused before any write.
    #[test]
    fn pass_write_rotates_the_file_and_refuses_junk() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("run-pass");
        let e = Engine::new(d.path().to_path_buf(), "/tmp".into()).with_pass_root(root.clone());
        let ask = |line: String| -> Answer {
            let (mut a, b) = UnixStream::pair().unwrap();
            a.write_all(line.as_bytes()).unwrap();
            a.shutdown(std::net::Shutdown::Write).unwrap();
            serve_conn(&e, b, &|_| Ok("k2".into()));
            let mut s = String::new();
            a.read_to_string(&mut s).unwrap();
            serde_json::from_str(s.trim()).unwrap()
        };
        let w = |p: &[u8]| format!("{{\"verb\":\"pass-write\",\"app\":\"jobs\",\"passHex\":\"{}\"}}\n", wire::hex_encode(p));
        // serde field names: snake_case in the enum (no rename_all on variant fields).
        let w2 = |p: &[u8]| format!("{{\"verb\":\"pass-write\",\"app\":\"jobs\",\"pass_hex\":\"{}\"}}\n", wire::hex_encode(p));
        let a = ask(w2(b"k2skn_first"));
        assert!(a.ok, "{a:?}");
        let file = root.join("jobs").join("pass");
        assert_eq!(std::fs::read(&file).unwrap(), b"k2skn_first");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700, "the root stops the daemon user's agents");
        assert_eq!(mode(&root.join("jobs")), 0o755);
        assert_eq!(mode(&file), 0o444);
        let a = ask(w2(b"k2skn_second"));
        assert!(a.ok, "{a:?}");
        assert_eq!(std::fs::read(&file).unwrap(), b"k2skn_second", "rotation replaces the whole file");
        for junk in [&b"not-a-pass"[..], b"k2skn_a\nb", b""] {
            let a = ask(w2(junk));
            assert_eq!(a.code.as_deref(), Some(codes::BAD_REQUEST), "{junk:?}");
        }
        let a = ask(w(b"k2skn_x"));
        assert_eq!(a.code.as_deref(), Some(codes::BAD_REQUEST), "unknown field");
        let a = ask(format!("{{\"verb\":\"pass-write\",\"app\":\"../x\",\"pass_hex\":\"{}\"}}\n", wire::hex_encode(b"k2skn_x")));
        assert_eq!(a.code.as_deref(), Some(codes::BAD_REQUEST));
        assert_eq!(std::fs::read(&file).unwrap(), b"k2skn_second", "refusals never touch the file");
        // A process-exception App reads its pass from its process folder
        // (`K2_PASS_FILE`): pass-write replaces it there too, readable by
        // k2appx ("other"), and only when that folder exists.
        assert!(!d.path().join("apps/jobs/process/pass").exists());
        std::fs::create_dir_all(d.path().join("apps/jobs/process")).unwrap();
        let a = ask(w2(b"k2skn_third"));
        assert!(a.ok, "{a:?}");
        let pf = d.path().join("apps/jobs/process/pass");
        assert_eq!(std::fs::read(&pf).unwrap(), b"k2skn_third");
        assert_eq!(mode(&pf), 0o644);
        assert_eq!(std::fs::read(&file).unwrap(), b"k2skn_third", "and the container mount");
    }

    #[test]
    fn run_state_round_trip() {
        let s = RunState { app: "jobs".into(), deployment: "d-0123456789ab".into(), port: 41000, gw_port: 38600, px_port: 38900, slot: 0, unit: "k2app-jobs--d-0123456789ab.service".into(), secrets: vec![] };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<RunState>(&j).unwrap(), s);
        // A protocol-1 state file (no proxy forward) still reads.
        let old = r#"{"app":"jobs","deployment":"d-0123456789ab","port":41000,"gwPort":38600,"slot":0,"unit":"u.service","secrets":[]}"#;
        assert_eq!(serde_json::from_str::<RunState>(old).unwrap().px_port, 0);
        let _ = StartSpec::clone;
    }
}
