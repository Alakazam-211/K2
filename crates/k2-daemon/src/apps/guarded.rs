//! The guarded engine (`prd-k2-app-runtime-v1` §4.2, S12): `RuntimeEngine`
//! over `k2-app-engine`, the binary that runs as `k2app` and drives
//! rootless Podman + Quadlet (S11). Linux only, behind the runtime
//! preview; the process engine keeps serving every other App.
//!
//! A guarded deploy:
//! 1. the base image (a pinned digest, fetched by the daemon with skopeo
//!    into an OCI archive and streamed to the engine; `k2app` itself has
//!    no network at all) and the Seed (a tar of the stored Seed) reach the
//!    engine once each;
//! 2. `prepare` gives the App its id slot and gateway port and pays the
//!    first start of its id mapping (W §5.5) — before any switch;
//! 3. `start` opens the App's gateway link (S14, `gateway.rs`: a listener
//!    on the slot's gateway port and a fresh one-hour App pass), renders
//!    the Quadlet unit (the floor, §4.2) and starts it; the daemon then
//!    installs the App's gateway rule (root helper `apps nft app`, which
//!    nft resolves to the cgroup inode now, W §2.12);
//! 4. health, the switch, drain, the watch window and rollback are the
//!    shared pipeline's (`supervisor.rs`), probing the App's port on host
//!    loopback from the daemon (AR45: daemon-side HTTP probing).
//!
//! Secrets granted to the service travel as Podman secrets, never in the
//! unit or the env file. The App pass (S14) is a file the engine keeps in
//! the App's pass folder (tmpfs), mounted read-only at `/run/k2`; the
//! gateway rotates it through the engine's `pass-write`.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::deploy::engine::{
    AppSpec, CgroupStats, EngineCaps, EngineError, ExecStream, Isolation, Liveness, Prepared, Result, RunHandle, RuntimeEngine,
    SeedRef,
};
use k2_core::deploy::codes;
use k2_root_proto::apps::AppId;
use k2_root_proto::engine::{self as wire, Answer, Request, SecretItem, StartSpec};
use serde_json::{json, Value};

/// `engine_ref` prefix that marks a handle as the guarded engine's.
pub const REF_PREFIX: &str = "podman:";

/// Default per-App memory cap when the App has none set (§6.2: 512 MiB
/// for a Next BFF). The guarded floor always has a cap (§4.2 item 10).
pub const DEFAULT_MEMORY_MIB: u64 = 512;

/// The pinned base image (docker.io `node:24-bookworm-slim`, 2026-10-14):
/// the platform manifest digest per arch. The local name carries the
/// digest's first 12 hex, so a new pin is a new image.
pub struct BaseImage {
    pub name: &'static str,
    pub digest: &'static str,
}

pub const NODE24_AMD64: &str = "sha256:ec857ea6bb60cb3d0fa4d97495590c5fe7d51be8572c307b55957eeb06569cf7";
pub const NODE24_ARM64: &str = "sha256:156e9bc7a0a267cc6ca77b4283e32b123479342df684ce297455d7b95ff5a6b5";

pub fn base_image_for(arch: &str) -> Option<BaseImage> {
    match arch {
        "x86_64" => Some(BaseImage { name: "node24", digest: NODE24_AMD64 }),
        "aarch64" => Some(BaseImage { name: "node24", digest: NODE24_ARM64 }),
        _ => None,
    }
}

impl BaseImage {
    pub fn local(&self) -> String {
        let hex = self.digest.trim_start_matches("sha256:");
        format!("localhost/k2-base/{}:{}", self.name, &hex[..12])
    }
    pub fn remote(&self) -> String {
        format!("docker://docker.io/library/node@{}", self.digest)
    }
}

// ── the client ───────────────────────────────────────────────────────

fn eng_err(code: &'static str, m: impl Into<String>) -> EngineError {
    EngineError::new(code, m)
}

/// Engine answer codes → the daemon's stable codes.
fn code_of(engine_code: Option<&str>) -> &'static str {
    match engine_code {
        Some(wire::codes::BAD_REQUEST) => codes::BAD_REQUEST,
        Some(wire::codes::TOO_LARGE) => codes::BAD_REQUEST,
        Some(wire::codes::NOT_FOUND) => "engine_not_found",
        Some(wire::codes::NO_SLOT) => "engine_no_slot",
        Some(wire::codes::CALLER_NOT_DAEMON) => "engine_caller_refused",
        _ => codes::ENGINE_UNAVAILABLE,
    }
}

/// One request to the engine at `socket` (one connection, a header line,
/// an optional body, one answer line).
#[cfg(unix)]
pub fn call_at(socket: &Path, req: &Request, body: Option<(&mut dyn Read, u64)>, timeout: Duration) -> Result<Value> {
    use std::os::unix::net::UnixStream;
    let mut c = UnixStream::connect(socket).map_err(|e| {
        eng_err(
            codes::ENGINE_UNAVAILABLE,
            format!(
                "the App engine isn't answering at {} ({e}); install it as the owner: k2 app engine install",
                socket.display()
            ),
        )
    })?;
    let _ = c.set_read_timeout(Some(timeout));
    let _ = c.set_write_timeout(Some(Duration::from_secs(120)));
    let mut line = serde_json::to_vec(req).map_err(|e| eng_err(codes::BAD_REQUEST, e.to_string()))?;
    line.push(b'\n');
    c.write_all(&line).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("engine write: {e}")))?;
    if let Some((r, len)) = body {
        let n = std::io::copy(&mut r.take(len), &mut c).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("engine body: {e}")))?;
        if n != len {
            return Err(eng_err(codes::ENGINE_UNAVAILABLE, format!("body ended at {n} of {len} bytes")));
        }
    }
    let _ = c.shutdown(std::net::Shutdown::Write);
    let mut ans = String::new();
    BufReader::new(c)
        .take(4 * wire::MAX_OUTPUT as u64)
        .read_line(&mut ans)
        .map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("engine answer: {e}")))?;
    let a: Answer = serde_json::from_str(ans.trim()).map_err(|_| eng_err(codes::ENGINE_UNAVAILABLE, "the App engine gave no answer"))?;
    if a.ok {
        Ok(a.data)
    } else {
        Err(eng_err(code_of(a.code.as_deref()), format!("App engine: {}", a.message.unwrap_or_default())))
    }
}

#[cfg(not(unix))]
pub fn call_at(_: &Path, _: &Request, _: Option<(&mut dyn Read, u64)>, _: Duration) -> Result<Value> {
    Err(eng_err(codes::ENGINE_UNAVAILABLE, "the guarded engine runs on Linux boxes only"))
}

pub fn call(req: &Request, body: Option<(&mut dyn Read, u64)>, timeout: Duration) -> Result<Value> {
    if !cfg!(target_os = "linux") {
        return Err(eng_err(codes::ENGINE_UNAVAILABLE, "the guarded engine runs on Linux boxes only"));
    }
    call_at(Path::new(wire::SOCKET), req, body, timeout)
}

/// The engine's `version` answer (None when it isn't there).
pub fn engine_version() -> std::result::Result<Value, String> {
    call(&Request::Version, None, Duration::from_secs(10)).map_err(|e| e.message)
}

// ── env and secrets ──────────────────────────────────────────────────

/// Names the engine or K2 sets itself; a manifest can't declare them.
const OWN: &[&str] = &[
    "PATH",
    "HOME",
    "PORT",
    "NODE_ENV",
    "TMPDIR",
    "LANG",
    "TZ",
    "NODE_OPTIONS",
    // The egress proxy (S15): the engine sets these.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "NODE_USE_ENV_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// The container's environment from the daemon's side: the manifest's
/// declared variables, then the App identity (S24/AR37: `K2_APP_ID` = the
/// published service UUID, `K2_APP_NAME` = the slug) and the gateway link
/// (`K2_HOST`/`K2_PORT` point at the link inside the wall, AR37; the same
/// in-wall `127.0.0.1:8730` for every App, S14). The engine adds `PORT`,
/// `HOME`, `TMPDIR`, `NODE_OPTIONS`, `K2_APP_DATA`, `K2_APP_CACHE` and the
/// secret/pass file variables.
pub fn guarded_env(spec: &AppSpec, host_lang: Option<String>, host_tz: Option<String>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (k, v) in &spec.env {
        if !OWN.contains(&k.as_str()) && !k.starts_with("K2_") && wire::env_name_ok(k) && wire::value_ok(v, 32 * 1024) {
            env.insert(k.clone(), v.clone());
        }
    }
    if let Some(l) = host_lang.filter(|l| wire::value_ok(l, 64)) {
        env.insert("LANG".into(), l);
    }
    if let Some(t) = host_tz.filter(|t| wire::value_ok(t, 64)) {
        env.insert("TZ".into(), t);
    }
    env.insert("NODE_ENV".into(), "production".into());
    env.insert(k2_core_app_id_env().into(), spec.service_id.clone());
    env.insert(super::process_engine::APP_NAME_ENV.into(), spec.app_id.clone());
    env.insert("K2_APP_DEPLOYMENT".into(), spec.deployment_id.clone());
    env.insert("K2_HOST".into(), "127.0.0.1".into());
    env.insert("K2_PORT".into(), wire::IN_WALL_GATEWAY_PORT.to_string());
    env.insert("K2_GATEWAY_URL".into(), super::gateway::in_wall_url());
    env
}

/// The live Seed's manifest caps (the ceiling of the App's link pass).
pub(crate) fn seed_caps(seed_dir: &Path) -> Result<Vec<String>> {
    let path = seed_dir.join(k2_core::deploy::manifest::MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| eng_err(codes::SEED_NOT_FOUND, format!("{}: {e}", path.display())))?;
    let m: k2_core::deploy::manifest::Manifest =
        serde_json::from_str(&text).map_err(|e| eng_err(codes::MANIFEST_INVALID, format!("the Seed's manifest: {e}")))?;
    Ok(m.caps)
}

/// Rotations reach the App through the engine (`pass-write` swaps the
/// file every instance of the App mounts).
pub(crate) fn deliver_for(app: &str) -> super::gateway::Deliver {
    let app = app.to_string();
    std::sync::Arc::new(move |raw: &str| {
        call(&Request::PassWrite { app: app.clone(), pass_hex: wire::hex_encode(raw.as_bytes()) }, None, Duration::from_secs(30))
            .map(|_| ())
            .map_err(|e| e.message)
    })
}

/// The link an installed App gets on `gw` (adopt after a daemon restart).
pub(crate) fn link_spec_for(app_id: &str, gw: u16) -> Option<super::gateway::LinkSpec> {
    let (app, seed) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let app = k2_core::deploy::store::get_app(&conn, app_id).ok().flatten()?;
        let seed = app.live_seed.as_deref().and_then(|h| k2_core::deploy::store::get_seed(&conn, h).ok().flatten());
        (app, seed)
    };
    let caps: Vec<String> = seed
        .and_then(|s| serde_json::from_value::<k2_core::deploy::manifest::Manifest>(s.manifest).ok())
        .map(|m| m.caps)
        .unwrap_or_default();
    let (caps, rooms, _) = super::gateway::link_policy(&caps, &app.owner_workspace);
    Some(super::gateway::LinkSpec { app_id: app.app_id, service_id: app.service_id, workspace_id: app.owner_workspace, port: gw, caps, rooms })
}

fn k2_core_app_id_env() -> &'static str {
    crate::vault::publish_env::APP_ID_ENV
}

/// Vault grants as Podman secrets. A name the wire refuses is dropped
/// (logged), never renamed.
pub fn secret_items(env: Vec<(String, String)>, files: Vec<(String, Vec<u8>)>) -> (Vec<SecretItem>, Vec<String>) {
    let mut out = Vec::new();
    let mut dropped = Vec::new();
    for (k, v) in env {
        if wire::env_name_ok(&k) && !k.starts_with("K2_") && !OWN.contains(&k.as_str()) {
            out.push(SecretItem { env: Some(k), file: None, value_hex: wire::hex_encode(v.as_bytes()) });
        } else {
            dropped.push(k);
        }
    }
    for (n, b) in files {
        if wire::secret_file_ok(&n) {
            out.push(SecretItem { env: None, file: Some(n), value_hex: wire::hex_encode(&b) });
        } else {
            dropped.push(n);
        }
    }
    (out, dropped)
}

pub fn memory_mib(limit_bytes: Option<i64>) -> u64 {
    match limit_bytes {
        Some(b) if b > 0 => ((b as u64) / (1024 * 1024)).clamp(64, 65_536),
        _ => DEFAULT_MEMORY_MIB,
    }
}

// ── the engine ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct GuardedEngine {
    /// deployment → (slot, gateway port) from `prepare`.
    prepared: Mutex<HashMap<String, (u64, u16)>>,
    /// unit → (when, answer): the supervisor asks every 250 ms.
    live: Mutex<HashMap<String, (Instant, Liveness)>>,
}

pub fn engine() -> &'static GuardedEngine {
    static E: OnceLock<GuardedEngine> = OnceLock::new();
    E.get_or_init(GuardedEngine::default)
}

fn app_id(spec_app: &str) -> Result<AppId> {
    AppId::parse(spec_app).ok_or_else(|| {
        eng_err(
            codes::APP_ID_INVALID,
            format!("App id '{spec_app}' doesn't fit the guarded engine (at most 32 characters, starting with a letter, no '--' and no trailing '-')"),
        )
    })
}

fn images_dir() -> PathBuf {
    k2_core::paths::k2_home().join("apps").join("images")
}

fn tmp_dir() -> PathBuf {
    k2_core::paths::k2_home().join("apps").join("tmp")
}

pub const SKOPEO: &str = "/usr/bin/skopeo";

/// The base image on the engine: present, or fetched (skopeo, by digest,
/// as the daemon user) and streamed in.
pub fn ensure_image() -> Result<String> {
    let base = base_image_for(std::env::consts::ARCH).ok_or_else(|| eng_err(codes::ENGINE_UNAVAILABLE, "no pinned base image for this CPU"))?;
    let image = base.local();
    let has = call(&Request::ImageHas { image: image.clone() }, None, Duration::from_secs(60))?;
    if has["present"] == true {
        return Ok(image);
    }
    let dir = images_dir();
    std::fs::create_dir_all(&dir).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("mkdir {}: {e}", dir.display())))?;
    let hex = base.digest.trim_start_matches("sha256:");
    let archive = dir.join(format!("{}-{}.oci.tar", base.name, &hex[..12]));
    if !archive.is_file() {
        if k2_core::airgap::enabled() {
            return Err(eng_err(
                codes::ENGINE_UNAVAILABLE,
                format!(
                    "air-gapped: put the base image archive at {} (skopeo copy {} oci-archive:<file>:{image}) and deploy again",
                    archive.display(),
                    base.remote()
                ),
            ));
        }
        if !Path::new(SKOPEO).exists() {
            return Err(eng_err(codes::ENGINE_UNAVAILABLE, "skopeo is missing (box prep installs it: k2 app box prepare)"));
        }
        let tmp = dir.join(format!(".fetch-{}.oci.tar", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let dest = format!("oci-archive:{}:{image}", tmp.display());
        // Anonymous: the base image is public and pinned by digest. Without
        // --src-no-creds skopeo reads /run/containers/<uid>/auth.json (no
        // XDG_RUNTIME_DIR here), and where root has run the distro's Podman
        // that folder is root's 0700: "permission denied", deploy failed
        // (Ubuntu 24.04 e2e, lane P).
        let out = std::process::Command::new(SKOPEO)
            .args(["copy", "--quiet", "--retry-times", "3", "--src-no-creds", &base.remote(), &dest])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", k2_core::paths::k2_home())
            .env("TMPDIR", &dir)
            .output()
            .map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("skopeo: {e}")))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(eng_err(
                codes::ENGINE_UNAVAILABLE,
                format!("skopeo copy {}: {}", base.remote(), String::from_utf8_lossy(&out.stderr).trim()),
            ));
        }
        std::fs::rename(&tmp, &archive).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("place image archive: {e}")))?;
    }
    let len = std::fs::metadata(&archive).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e.to_string()))?.len();
    let mut f = std::fs::File::open(&archive).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
    call(&Request::ImageLoad { image: image.clone(), len }, Some((&mut f, len)), Duration::from_secs(900))?;
    Ok(image)
}

/// The Seed on the engine: present, or tarred (links kept as links) and
/// streamed in.
pub fn ensure_seed(seed: &SeedRef) -> Result<()> {
    let has = call(&Request::SeedHas { hash: seed.hash.clone() }, None, Duration::from_secs(30))?;
    if has["present"] == true {
        return Ok(());
    }
    let dir = tmp_dir();
    std::fs::create_dir_all(&dir).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("mkdir {}: {e}", dir.display())))?;
    let tar_path = dir.join(format!("seed-{}-{}.tar", &seed.hash[..12.min(seed.hash.len())], std::process::id()));
    let res = (|| -> Result<()> {
        {
            let f = std::fs::File::create(&tar_path).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
            let mut b = tar::Builder::new(f);
            b.follow_symlinks(false);
            b.mode(tar::HeaderMode::Deterministic);
            b.append_dir_all(".", &seed.path).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("tar the Seed: {e}")))?;
            b.into_inner().and_then(|f| f.sync_all()).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
        }
        let len = std::fs::metadata(&tar_path).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e.to_string()))?.len();
        let mut f = std::fs::File::open(&tar_path).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
        call(&Request::SeedPut { hash: seed.hash.clone(), len }, Some((&mut f, len)), Duration::from_secs(900)).map(|_| ())
    })();
    let _ = std::fs::remove_file(&tar_path);
    res
}

fn log_line(spec: &AppSpec, line: &str) {
    if let Some(p) = spec.log_path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&spec.log_path) {
        let _ = writeln!(f, "[k2] {line}");
    }
}

/// The App's gateway rule (root helper, protocol 6). Without it the App
/// can't reach its gateway port at all (the table drops it): fail closed.
/// It carries the App's approved box services' host ports every time (the
/// helper replaces the App's list), so every re-apply keeps them.
pub fn install_gateway_rule(app: &str, gw: u16) -> std::result::Result<(), String> {
    let st = crate::root_helper::state();
    if !st.usable(k2_root_proto::version::APPS_MIN_PROTOCOL) {
        return Err("the root helper is older than protocol 6".into());
    }
    crate::root_helper::call_text(&gateway_rule_argv(app, gw, &super::box_service_routes::allowed_ports(app)), None).map(|_| ())
}

/// `apps nft app <id> <gw> [<box port>…]`.
pub fn gateway_rule_argv(app: &str, gw: u16, box_ports: &[u16]) -> Vec<String> {
    let mut a: Vec<String> = vec!["apps".into(), "nft".into(), "app".into(), app.into(), gw.to_string()];
    a.extend(box_ports.iter().map(u16::to_string));
    a
}

/// Does the engine take box forwards (`version` lists the feature)?
fn engine_has_box_forward() -> bool {
    engine_version().is_ok_and(|v| {
        v["features"].as_array().is_some_and(|f| f.iter().any(|x| x.as_str() == Some(wire::FEATURE_BOX_FORWARD)))
    })
}

pub fn remove_gateway_rule(app: &str) {
    let st = crate::root_helper::state();
    if st.usable(k2_root_proto::version::APPS_MIN_PROTOCOL) {
        let _ = crate::root_helper::call_text(&["apps".into(), "nft".into(), "app-remove".into(), app.into()], None);
    }
}

/// A guarded deployment's output: its unit's journal, read by the root
/// helper (`apps logs`; k2app is a system user, so journald files its
/// lines in the system journal).
pub fn logs(app: &str, deployment: &str, lines: u32) -> Result<String> {
    let st = crate::root_helper::state();
    if !st.usable(k2_root_proto::version::APPS_MIN_PROTOCOL) {
        return Err(eng_err(codes::ENGINE_UNAVAILABLE, "reading a guarded App's journal needs the root helper (protocol 6)"));
    }
    let out = crate::root_helper::call_text(&["apps".into(), "logs".into(), app.into(), deployment.into(), lines.clamp(1, 5000).to_string()], None)
        .map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e))?;
    let v: Value = serde_json::from_str(&out).map_err(|_| eng_err(codes::ENGINE_UNAVAILABLE, "apps logs: not JSON"))?;
    Ok(v["text"].as_str().unwrap_or("").to_string())
}

/// Uninstall: every instance stopped; `purge` also deletes its data and
/// frees its id slot. Then the gateway rule goes.
pub fn remove_app(app: &str, purge: bool) -> Result<Value> {
    let v = call(&Request::RemoveApp { app: app.into(), data: purge }, None, Duration::from_secs(1200))?;
    remove_gateway_rule(app);
    super::box_service_routes::forget_live(app);
    super::egress_proxy::stop(app);
    Ok(v)
}

/// The App's egress proxy port from an engine answer (`pxPort`, engine
/// protocol 2), or the one its gateway port implies.
pub fn proxy_port_of(answer: &Value, gw: Option<u16>) -> Option<u16> {
    answer["pxPort"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .or_else(|| gw.and_then(k2_root_proto::apps::proxy_port_for_gateway))
}

/// Open the App's egress proxy listener (S15). Without it the App has no
/// way out at all (fail closed), so a failure is logged, never fatal.
pub fn ensure_proxy(app: &str, px: Option<u16>) -> std::result::Result<u16, String> {
    let px = px.ok_or_else(|| "no proxy port for this App's slot".to_string())?;
    if k2_root_proto::apps::PROXY_PORTS.lo > px || px > k2_root_proto::apps::PROXY_PORTS.hi {
        return Err(format!("proxy port {px} is outside K2's proxy range"));
    }
    super::egress_proxy::ensure(app, px).map(|_| px)
}

impl RuntimeEngine for GuardedEngine {
    fn isolation(&self) -> Isolation {
        Isolation::Container
    }

    fn caps(&self) -> EngineCaps {
        EngineCaps {
            separate_user: true,
            memory_limit: true,
            cpu_limit: false,
            read_only_seed: true,
            // Egress only through the App's own proxy (S15); the table
            // drops everything else.
            egress_gate: true,
            exec: true,
            adopt: true,
        }
    }

    fn prepare(&self, seed: &SeedRef, spec: &AppSpec) -> Result<Prepared> {
        let app = app_id(&spec.app_id)?;
        let v = engine_version().map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, e))?;
        if v["protocol"].as_u64() != Some(wire::ENGINE_PROTOCOL as u64) {
            return Err(eng_err(
                codes::ENGINE_UNAVAILABLE,
                format!("the App engine speaks protocol {} and this daemon {}: k2 app engine install", v["protocol"], wire::ENGINE_PROTOCOL),
            ));
        }
        let image = ensure_image()?;
        ensure_seed(seed)?;
        let t0 = Instant::now();
        let p = call(&Request::Prepare { app: app.as_str().into(), image, seed: seed.hash.clone() }, None, Duration::from_secs(960))?;
        let slot = p["slot"].as_u64().ok_or_else(|| eng_err(codes::ENGINE_UNAVAILABLE, "prepare gave no slot"))?;
        let gw = p["gwPort"].as_u64().and_then(|g| u16::try_from(g).ok()).ok_or_else(|| eng_err(codes::ENGINE_UNAVAILABLE, "prepare gave no gateway port"))?;
        log_line(spec, &format!("guarded: slot {slot}, gateway port {gw}, id mapping warmed in {} ms", t0.elapsed().as_millis()));
        self.prepared.lock().unwrap_or_else(|e| e.into_inner()).insert(spec.deployment_id.clone(), (slot, gw));
        Ok(Prepared { workdir: seed.path.clone() })
    }

    fn start(&self, spec: &AppSpec, p: &Prepared, port: u16) -> Result<RunHandle> {
        let app = app_id(&spec.app_id)?;
        if !wire::app_port_ok(port) {
            return Err(eng_err(codes::ENGINE_UNAVAILABLE, format!("port {port} is in K2's reserved range")));
        }
        let (_, gw) = self
            .prepared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&spec.deployment_id)
            .ok_or_else(|| eng_err(codes::ENGINE_UNAVAILABLE, "start before prepare"))?;
        let seed_hash = spec_seed_hash(p).ok_or_else(|| eng_err(codes::SEED_NOT_FOUND, "the prepared Seed has no hash"))?;
        let mut env = guarded_env(spec, std::env::var("LANG").ok(), std::env::var("TZ").ok());
        // Box services (0.46.0): approved and held ones are forwarded into
        // the wall; the App's nft rule opens only the approved ones.
        let (mut box_forwards, skipped) = super::box_service_routes::forwards_for(app.as_str(), port);
        if !skipped.is_empty() {
            log_line(spec, &format!("guarded: box services not forwarded (refused port, or on the App's own port {port}): {}", skipped.join(", ")));
        }
        if !box_forwards.is_empty() && !engine_has_box_forward() {
            log_line(spec, "guarded: the App engine predates box services; reinstall it (k2 app engine install) and restart the App");
            box_forwards.clear();
        }
        if !box_forwards.is_empty() {
            for (k, v) in super::box_service_routes::env_for(app.as_str(), false) {
                if box_forwards.iter().any(|f| v.ends_with(&format!(":{}", f.app_port))) {
                    env.insert(k, v);
                }
            }
            log_line(
                spec,
                &format!(
                    "guarded: box services in the wall: {} (k2 app box-service {})",
                    box_forwards.iter().map(|f| format!("127.0.0.1:{} -> host {}", f.app_port, f.host_port)).collect::<Vec<_>>().join(", "),
                    spec.app_id
                ),
            );
        }
        // S14: the App's gateway link and a fresh pass, before the App
        // starts (it may call K2 on boot). A redeploy keeps the listener.
        let (caps, rooms, dropped_caps) = super::gateway::link_policy(&seed_caps(&p.workdir)?, &spec.workspace_id);
        if !dropped_caps.is_empty() {
            log_line(spec, &format!("guarded: caps the App pass can't carry (owner widenings, 0.46.1): {}", dropped_caps.join(", ")));
        }
        let had_link = super::gateway::port_of(app.as_str()).is_some();
        let link = super::gateway::LinkSpec {
            app_id: app.as_str().into(),
            service_id: spec.service_id.clone(),
            workspace_id: spec.workspace_id.clone(),
            port: gw,
            caps,
            rooms,
        };
        let pass = super::gateway::open(link, Some(deliver_for(app.as_str())))
            .map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("the App's gateway link: {e}")))?;
        let close_new_link = || {
            if !had_link {
                super::gateway::close(app.as_str(), Some(&spec.service_id), true);
            }
        };
        let g = crate::vault::publish_env::granted(&spec.service_id, &spec.workspace_id, &p.workdir);
        let (secrets, dropped) = secret_items(g.env, g.files);
        if !dropped.is_empty() {
            log_line(spec, &format!("guarded: granted secrets not delivered (names the wall refuses): {}", dropped.join(", ")));
        }
        let base = base_image_for(std::env::consts::ARCH).ok_or_else(|| eng_err(codes::ENGINE_UNAVAILABLE, "no pinned base image for this CPU"))?;
        let s = StartSpec {
            app: app.as_str().into(),
            deployment: spec.deployment_id.clone(),
            seed: seed_hash,
            image: base.local(),
            port,
            argv: spec.argv.clone(),
            env,
            memory_mib: memory_mib(spec.memory_limit_bytes),
            drain_secs: spec.drain_secs.min(600),
            secrets,
            pass_hex: Some(wire::hex_encode(pass.as_bytes())),
            io: k2_root_proto::apps::IoMax::from_column(spec.io_max_json.as_deref()),
            box_forwards,
        };
        if let Err(w) = wire::check_start(&s) {
            close_new_link();
            return Err(eng_err(codes::MANIFEST_INVALID, format!("the guarded engine can't run this: {w}")));
        }
        // S15: the App's egress proxy listens before the App starts, so a
        // request at boot finds it (same slot offset as the gateway port).
        let px_planned = k2_root_proto::apps::proxy_port_for_gateway(gw);
        match ensure_proxy(app.as_str(), px_planned) {
            Ok(px) => log_line(spec, &format!("guarded: egress proxy on 127.0.0.1:{px} (k2 app egress {})", spec.app_id)),
            Err(e) => log_line(spec, &format!("guarded: egress proxy not listening ({e}); the App has no egress until it is")),
        }
        let v = match call(&Request::Start(Box::new(s)), None, Duration::from_secs(420)) {
            Ok(v) => v,
            Err(mut e) => {
                close_new_link();
                // The unit's own words (Podman / the OCI runtime) are in the
                // journal, which only the root helper reads for k2app.
                if let Ok(t) = logs(app.as_str(), &spec.deployment_id, 12) {
                    let t: String = t.trim().chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
                    if !t.is_empty() {
                        e.message.push_str(&format!("; the unit said: {t}"));
                    }
                }
                return Err(e);
            }
        };
        let unit = v["unit"].as_str().unwrap_or_default().to_string();
        super::box_service_routes::record_live(app.as_str(), &v);
        log_line(spec, &format!("guarded container {unit} started on 127.0.0.1:{port}; its output is in the unit's journal (k2 app logs)"));
        if let Err(e) = install_gateway_rule(app.as_str(), gw) {
            log_line(spec, &format!("guarded: gateway rule not installed ({e}); the App can't reach K2 until it is"));
        }
        // The engine's answer is the truth for the forward it rendered.
        if let Some(px) = proxy_port_of(&v, Some(gw)).filter(|p| Some(*p) != px_planned) {
            if let Err(e) = ensure_proxy(app.as_str(), Some(px)) {
                log_line(spec, &format!("guarded: egress proxy not listening ({e}); the App has no egress until it is"));
            }
        }
        Ok(RunHandle {
            app_id: spec.app_id.clone(),
            deployment_id: spec.deployment_id.clone(),
            port,
            pid: v["pid"].as_i64().and_then(|p| i32::try_from(p).ok()),
            engine_ref: format!("{REF_PREFIX}{unit}"),
        })
    }

    fn liveness(&self, h: &RunHandle) -> Liveness {
        let key = h.engine_ref.clone();
        if let Some((at, l)) = self.live.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if at.elapsed() < Duration::from_secs(2) {
                return *l;
            }
        }
        let l = match call(&Request::Liveness { app: h.app_id.clone(), deployment: h.deployment_id.clone() }, None, Duration::from_secs(15)) {
            Ok(v) => match v["state"].as_str() {
                Some("running") => Liveness::Running,
                Some("exited") => Liveness::Exited(v["code"].as_i64().and_then(|c| i32::try_from(c).ok())),
                _ => Liveness::Unknown,
            },
            Err(_) => Liveness::Unknown,
        };
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (Instant::now(), l));
        l
    }

    fn stop(&self, h: &RunHandle, grace: Duration) -> Result<()> {
        call(&Request::Stop { app: h.app_id.clone(), deployment: h.deployment_id.clone() }, None, grace + Duration::from_secs(180))?;
        self.live.lock().unwrap_or_else(|e| e.into_inner()).remove(&h.engine_ref);
        Ok(())
    }

    fn exec(&self, h: &RunHandle, argv: &[String], tty: bool) -> Result<ExecStream> {
        if tty {
            return Err(eng_err(codes::ENGINE_UNAVAILABLE, "a terminal goes through exec_tty (k2 app shell)"));
        }
        let v = exec_once(&h.app_id, &h.deployment_id, argv, wire::EXEC_TIMEOUT_DEFAULT)?;
        Ok(ExecStream {
            exit_code: v["exitCode"].as_i64().and_then(|c| i32::try_from(c).ok()),
            output: v["output"].as_str().unwrap_or("").as_bytes().to_vec(),
        })
    }

    fn stats(&self, h: &RunHandle) -> Result<CgroupStats> {
        let v = call(&Request::Stats { app: h.app_id.clone(), deployment: h.deployment_id.clone() }, None, Duration::from_secs(15))?;
        Ok(CgroupStats { mem_current: v["memCurrent"].as_u64(), pids: v["pids"].as_u64() })
    }

    fn adopt(&self) -> Result<Vec<RunHandle>> {
        if !cfg!(target_os = "linux") || !Path::new(wire::SOCKET).exists() {
            return Ok(Vec::new());
        }
        let v = call(&Request::Adopt, None, Duration::from_secs(120))?;
        let mut out = Vec::new();
        for i in v["instances"].as_array().cloned().unwrap_or_default() {
            let (Some(app), Some(dep), Some(port), Some(unit)) =
                (i["app"].as_str(), i["deployment"].as_str(), i["port"].as_u64(), i["unit"].as_str())
            else {
                continue;
            };
            let Ok(port) = u16::try_from(port) else { continue };
            let gw = i["gwPort"].as_u64().and_then(|g| u16::try_from(g).ok());
            super::box_service_routes::record_live(app, &i);
            if let Some(gw) = gw {
                // After a daemon restart the rule may point at an old inode.
                let _ = install_gateway_rule(app, gw);
                // S14: the link listener is the daemon's, so it re-opens;
                // the App gets a fresh pass (the one it holds stays valid
                // until it expires).
                if super::gateway::port_of(app).is_none() {
                    if let Some(spec) = link_spec_for(app, gw) {
                        let deliver = deliver_for(app);
                        match super::gateway::open(spec, Some(deliver.clone())).and_then(|raw| deliver(&raw)) {
                            Ok(()) => {}
                            Err(e) => k2_core::log_debug!("[apps] adopt {app}: gateway link: {e}"),
                        }
                    }
                }
            }
            // The proxy is the daemon's: re-open it for the running App.
            if let Err(e) = ensure_proxy(app, proxy_port_of(&i, gw)) {
                k2_core::log_debug!("[apps] {app}: egress proxy after adopt: {e}");
            }
            out.push(RunHandle {
                app_id: app.into(),
                deployment_id: dep.into(),
                port,
                pid: i["pid"].as_i64().and_then(|p| i32::try_from(p).ok()),
                engine_ref: format!("{REF_PREFIX}{unit}"),
            });
        }
        Ok(out)
    }
}

/// `Prepared.workdir` is the daemon's Seed folder, `<seeds>/<hash>`.
fn spec_seed_hash(p: &Prepared) -> Option<String> {
    let h = p.workdir.file_name()?.to_str()?.to_string();
    wire::seed_hash_ok(&h).then_some(h)
}

/// S28 one-shot exec into a guarded deployment's container (`k2 app
/// exec`): `{exitCode, timedOut, bytes, ms, output}` (output raw; the route
/// strips and frames it).
pub fn exec_once(app: &str, deployment: &str, argv: &[String], timeout_secs: u64) -> Result<Value> {
    call(
        &Request::Exec { app: app.into(), deployment: deployment.into(), argv: argv.to_vec(), timeout_secs },
        None,
        Duration::from_secs(timeout_secs + 30),
    )
}

/// S28 `k2 app shell`: ask the engine for a terminal; on `ok` the returned
/// stream carries `wire::tty` frames until the command exits.
#[cfg(unix)]
pub fn exec_tty_at(socket: &Path, app: &str, deployment: &str, argv: &[String], cols: u16, rows: u16) -> Result<std::os::unix::net::UnixStream> {
    use std::os::unix::net::UnixStream;
    let mut c = UnixStream::connect(socket)
        .map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("the App engine isn't answering at {} ({e})", socket.display())))?;
    let _ = c.set_write_timeout(Some(Duration::from_secs(60)));
    let req = Request::ExecTty { app: app.into(), deployment: deployment.into(), argv: argv.to_vec(), cols, rows };
    let mut line = serde_json::to_vec(&req).map_err(|e| eng_err(codes::BAD_REQUEST, e.to_string()))?;
    line.push(b'\n');
    c.write_all(&line).map_err(|e| eng_err(codes::ENGINE_UNAVAILABLE, format!("engine write: {e}")))?;
    // The answer line, byte by byte: nothing of the frame stream after it
    // may be swallowed by a buffer.
    let _ = c.set_read_timeout(Some(Duration::from_secs(60)));
    let mut ans = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match c.read(&mut b) {
            Ok(1) if b[0] == b'\n' => break,
            Ok(1) if ans.len() < 64 * 1024 => ans.push(b[0]),
            Ok(_) => return Err(eng_err(codes::ENGINE_UNAVAILABLE, "the App engine gave no answer")),
            Err(e) => return Err(eng_err(codes::ENGINE_UNAVAILABLE, format!("engine answer: {e}"))),
        }
    }
    let a: Answer = serde_json::from_slice(&ans).map_err(|_| eng_err(codes::ENGINE_UNAVAILABLE, "the App engine gave no answer"))?;
    if !a.ok {
        return Err(eng_err(code_of(a.code.as_deref()), format!("App engine: {}", a.message.unwrap_or_default())));
    }
    let _ = c.set_read_timeout(None);
    Ok(c)
}

#[cfg(unix)]
pub fn exec_tty(app: &str, deployment: &str, argv: &[String], cols: u16, rows: u16) -> Result<std::os::unix::net::UnixStream> {
    if !cfg!(target_os = "linux") {
        return Err(eng_err(codes::ENGINE_UNAVAILABLE, "the guarded engine runs on Linux boxes only"));
    }
    exec_tty_at(Path::new(wire::SOCKET), app, deployment, argv, cols, rows)
}

/// Is this handle the guarded engine's?
pub fn owns(h: &RunHandle) -> bool {
    h.engine_ref.starts_with(REF_PREFIX)
}

/// `GET /cli/app/box`'s view of the engine (doctor).
pub fn doctor_json() -> Value {
    match engine_version() {
        Ok(v) => json!({ "answering": true, "version": v["version"], "protocol": v["protocol"], "runtime": v["runtime"], "podman": v["podman"] }),
        Err(e) => json!({ "answering": false, "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AppSpec {
        AppSpec {
            app_id: "nsi-skin".into(),
            service_id: "6f1c0d7e-1111-4222-8333-944455556666".into(),
            workspace_id: "ws-1".into(),
            deployment_id: "d-0123456789ab".into(),
            isolation: Isolation::Container,
            argv: vec!["node".into(), "server.mjs".into()],
            env: [
                ("API_BASE".to_string(), "https://x".to_string()),
                ("K2_HOOK_TOKEN".to_string(), "stolen".to_string()),
                ("PATH".to_string(), "/evil".to_string()),
                ("NODE_OPTIONS".to_string(), "--inspect".to_string()),
                ("BAD-NAME".to_string(), "x".to_string()),
                ("HTTPS_PROXY".to_string(), "http://evil:1".to_string()),
                ("MULTI".to_string(), "a\nb".to_string()),
            ]
            .into_iter()
            .collect(),
            memory_limit_bytes: None,
            drain_secs: 30,
            io_max_json: None,
            devices: vec![],
            data_dir: "/x/data".into(),
            cache_dir: "/x/cache".into(),
            home_dir: "/x/home".into(),
            log_path: "/x/log".into(),
        }
    }

    #[test]
    fn env_is_the_manifest_plus_identity_and_the_link() {
        let e = guarded_env(&spec(), Some("C.UTF-8".into()), Some("bad\nzone".into()));
        assert_eq!(e["API_BASE"], "https://x");
        for never in ["K2_HOOK_TOKEN", "PATH", "NODE_OPTIONS", "BAD-NAME", "MULTI", "TZ", "HTTPS_PROXY"] {
            assert!(!e.contains_key(never), "{never}");
        }
        assert_eq!(e["K2_APP_ID"], "6f1c0d7e-1111-4222-8333-944455556666", "AR37: the service UUID");
        assert_eq!(e["K2_APP_NAME"], "nsi-skin");
        assert_eq!(e["K2_HOST"], "127.0.0.1");
        assert_eq!(e["K2_PORT"], "8730", "inside the wall K2_PORT is the link, the same port for every App");
        assert_eq!(e["K2_GATEWAY_URL"], "http://127.0.0.1:8730");
        assert!(!e.contains_key("K2_PASS_FILE"), "the engine sets the pass file variable");
        assert_eq!(e["LANG"], "C.UTF-8");
        assert_eq!(e["NODE_ENV"], "production");
    }

    #[test]
    fn secrets_become_items_and_bad_names_are_dropped() {
        let (items, dropped) = secret_items(
            vec![("STRIPE_KEY".into(), "sk_x".into()), ("K2_X".into(), "y".into()), ("PATH".into(), "z".into())],
            vec![("db.json".into(), b"{}".to_vec()), ("../shadow".into(), b"x".to_vec())],
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].env.as_deref(), Some("STRIPE_KEY"));
        assert_eq!(wire::hex_decode(&items[0].value_hex).unwrap(), b"sk_x");
        assert_eq!(items[1].file.as_deref(), Some("db.json"));
        assert_eq!(dropped, vec!["K2_X", "PATH", "../shadow"]);
    }

    #[test]
    fn memory_and_base_image() {
        assert_eq!(memory_mib(None), 512);
        assert_eq!(memory_mib(Some(1 << 30)), 1024);
        assert_eq!(memory_mib(Some(1)), 64);
        let b = base_image_for("x86_64").unwrap();
        assert_eq!(b.local(), "localhost/k2-base/node24:ec857ea6bb60");
        assert!(wire::image_ok(&b.local()));
        assert!(b.remote().starts_with("docker://docker.io/library/node@sha256:"));
        assert!(wire::image_ok(&base_image_for("aarch64").unwrap().local()));
        assert!(base_image_for("riscv64").is_none());
        assert_eq!(code_of(Some("caller_not_daemon")), "engine_caller_refused");
        // S15: the proxy port from the engine's answer, else the gateway's offset.
        assert_eq!(proxy_port_of(&json!({"pxPort": 38_905}), Some(38_601)), Some(38_905));
        assert_eq!(proxy_port_of(&json!({"pxPort": 0}), Some(38_601)), Some(38_901), "a protocol-1 state file");
        assert_eq!(proxy_port_of(&json!({}), Some(38_601)), Some(38_901));
        assert_eq!(proxy_port_of(&json!({}), None), None);
        assert!(ensure_proxy("x", None).is_err());
        assert!(ensure_proxy("x", Some(38_601)).unwrap_err().contains("outside"));
        assert_eq!(code_of(Some("podman_failed")), codes::ENGINE_UNAVAILABLE);
    }

    /// The client against a stub engine on a temp socket: request line,
    /// streamed body, answer mapping.
    #[cfg(unix)]
    #[test]
    fn client_speaks_the_wire() {
        use std::os::unix::net::UnixListener;
        // Short path: a Unix socket path is at most ~104 bytes on macOS.
        let d = std::path::PathBuf::from(format!("/tmp/k2gt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let sock = d.join("e.sock");
        let l = UnixListener::bind(&sock).unwrap();
        let t = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (c, _) = l.accept().unwrap();
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let req: Request = serde_json::from_str(line.trim()).unwrap();
                let mut w = c;
                match &req {
                    Request::SeedPut { len, .. } => {
                        let mut b = vec![0u8; *len as usize];
                        r.read_exact(&mut b).unwrap();
                        seen.push(b);
                        w.write_all(b"{\"ok\":true,\"data\":{\"stored\":true}}\n").unwrap();
                    }
                    _ => w.write_all(b"{\"ok\":false,\"code\":\"caller_not_daemon\",\"message\":\"nope\"}\n").unwrap(),
                }
            }
            seen
        });
        let body = b"tarbytes";
        let v = call_at(&sock, &Request::SeedPut { hash: "a".repeat(64), len: 8 }, Some((&mut &body[..], 8)), Duration::from_secs(5)).unwrap();
        assert_eq!(v["stored"], true);
        let e = call_at(&sock, &Request::Adopt, None, Duration::from_secs(5)).unwrap_err();
        assert_eq!(e.code, "engine_caller_refused");
        assert!(e.message.contains("nope"));
        assert_eq!(t.join().unwrap(), vec![body.to_vec()]);
        let gone = call_at(&d.join("none.sock"), &Request::Version, None, Duration::from_secs(1)).unwrap_err();
        assert_eq!(gone.code, codes::ENGINE_UNAVAILABLE);
        assert!(gone.message.contains("k2 app engine install"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
