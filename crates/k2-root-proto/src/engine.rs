//! `k2-app-engine` (`prd-k2-app-runtime-v1` §10.6 option C, S11/S12): the
//! small binary that runs as `k2app` and drives rootless Podman for the
//! guarded App path, on the daemon's orders only.
//!
//! This module is the contract the daemon and the engine share:
//! - fixed paths (the socket, the engine's state under `k2app`'s home);
//! - signed builds: the engine is installed by the root helper
//!   (`apps install-engine`) and is signed with the **root-helper key**,
//!   like `k2-node` (G8). The trusted comment's file name keeps the three
//!   binaries apart: `file:k2-app-engine-linux-<arch> version:<v>
//!   protocol:<n>`; the build marker is
//!   `K2_APP_ENGINE_BUILD v=<v> proto=<n> arch=<arch>;`;
//! - the wire: one request per connection, a JSON header line, an optional
//!   raw body of a declared length, one JSON answer line;
//! - the validators both sides run on every field (the engine trusts the
//!   daemon process, which is the trust boundary, but still refuses any
//!   path, id or value outside these shapes).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::apps::{AppId, IoMax, APP_HOME, GATEWAY_PORTS, RESERVED_PORTS};
use crate::keys::{key_id_of, TrustedKey};
use crate::sig::parse_trusted_comment;
use crate::version::{arch_ok, elf_matches, Semver};

// ── paths ───────────────────────────────────────────────────────────────

/// Root-owned `k2app:k2app 0750` + an ACL `u:<daemon user>:x` (tmpfiles.d,
/// written by `apps install-engine`). The socket inside is the engine's.
pub const RUN_DIR: &str = "/run/k2app";
pub const SOCKET: &str = "/run/k2app/engine.sock";
pub const TMPFILES_FILE: &str = "/etc/tmpfiles.d/k2app.conf";
/// The engine's user unit (for `k2app` only: `ConditionUser=`).
pub const USER_UNIT_DIR: &str = "/etc/systemd/user";
pub const USER_UNIT_FILE: &str = "k2-app-engine.service";
pub const USER_UNIT: &str = "k2-app-engine.service";

/// Under `k2app`'s home (the engine writes here; the helper never does).
pub fn seeds_dir() -> String {
    format!("{APP_HOME}/seeds")
}
pub fn apps_dir() -> String {
    format!("{APP_HOME}/apps")
}
pub fn state_dir() -> String {
    format!("{APP_HOME}/engine")
}
/// Rootless Quadlet's per-user unit dir for `k2app`.
pub fn quadlet_dir() -> String {
    format!("{APP_HOME}/.config/containers/systemd")
}

/// Container, unit, Quadlet file and per-deployment state stem:
/// `k2app-<app>--<deployment>` (an App id never holds `--`, a deployment
/// id is `d-<12 hex>`, so the split is unambiguous).
pub fn unit_stem(app: &AppId, deployment: &str) -> String {
    format!("k2app-{}--{deployment}", app.as_str())
}

/// The App's own slice inside `user@<k2app>`: nested in `k2apps.slice`
/// by systemd's dash rule, with the App id's dashes as underscores so App
/// `nsi-skin` never nests inside App `nsi`.
pub fn app_slice(app: &AppId) -> String {
    format!("k2apps-{}.slice", app.unit_safe())
}

// ── per-App user namespace slots ────────────────────────────────────────

/// Ids per App user namespace (W: a Next BFF image needs < 65536).
pub const SLOT_SIZE: u64 = 65_536;
/// Box prep gives `k2app` 64 × 65536 subordinate ids.
pub const MAX_SLOTS: u64 = crate::apps::SUBID_COUNT / SLOT_SIZE;

/// The first id of slot `n` in `k2app`'s rootless namespace (0 = `k2app`
/// itself, 1.. = its subordinate ids in order).
pub fn slot_base(slot: u64) -> u64 {
    1 + slot * SLOT_SIZE
}

/// The App's gateway link port on host loopback (S14 listens there; the
/// per-App nft rule admits only that App's cgroup to it).
pub fn gateway_port(slot: u64) -> Option<u16> {
    let p = GATEWAY_PORTS.lo as u64 + slot;
    (p <= GATEWAY_PORTS.hi as u64).then_some(p as u16)
}

/// The gateway link's port **inside** every guarded container: pasta
/// (`-T 8730:<gateway port>`) forwards the container's `127.0.0.1:8730` to
/// the App's own link on host loopback, so every App's code dials the same
/// `K2_GATEWAY_URL=http://127.0.0.1:8730` (§5.1). Never an App's own port.
pub const IN_WALL_GATEWAY_PORT: u16 = 8730;

/// Where the engine keeps each App's current pass on the host (tmpfs, under
/// [`RUN_DIR`]): `pass/` is `k2app` 0700 (the daemon user's ACL on
/// `/run/k2app` stops there, so no agent reads an App's pass), and
/// `pass/<app>/` (0755) is bind-mounted read-only at [`PASS_MOUNT`] in the
/// App's containers. The daemon rotates the file through `pass-write`; a
/// rename swaps it, so the container sees the new pass at once.
pub const PASS_ROOT: &str = "/run/k2app/pass";
pub const PASS_FILE_NAME: &str = "pass";
/// The App's egress proxy port (S15: the daemon listens there; pasta `-T`
/// forwards it from the container's loopback; the per-App nft rule
/// admits it). Same offset as the gateway port.
pub fn proxy_port(slot: u64) -> Option<u16> {
    gateway_port(slot).and_then(crate::apps::proxy_port_for_gateway)
}

/// The proxy variables the engine sets in every guarded container (S15).
/// Engine-owned: a manifest or the daemon's env can't replace them.
/// `NO_PROXY` keeps the gateway link (container loopback) direct; Node 24
/// honours the variables with `NODE_USE_ENV_PROXY=1`.
pub fn proxy_env(px_port: u16) -> Vec<(&'static str, String)> {
    let url = format!("http://127.0.0.1:{px_port}");
    let no = "127.0.0.1,localhost,::1".to_string();
    vec![
        ("HTTP_PROXY", url.clone()),
        ("HTTPS_PROXY", url.clone()),
        ("http_proxy", url.clone()),
        ("https_proxy", url.clone()),
        ("ALL_PROXY", url),
        ("NO_PROXY", no.clone()),
        ("no_proxy", no),
        ("NODE_USE_ENV_PROXY", "1".to_string()),
    ]
}

// ── signed builds ───────────────────────────────────────────────────────

/// What this build of the engine speaks on its socket (the daemon refuses
/// any other; boxes reinstall the engine with `k2 app engine install`):
/// - 2 (0.46.0) = S14 `pass-write` and the pass folder (the in-wall
///   gateway at 8730); S15 the egress proxy forward and proxy variables,
///   `pxPort` in the prepare/start/adopt answers; lane D the MAC shim, `io`
///   on start, exec timeouts, `exec-tty` terminals and the process
///   exception's staging; box services' `boxForwards` (a 2 extension
///   announced in `version`'s `features`).
/// - 3 (0.46.1, lane 1 "Wall") = one bump for the wall: `wall` on start
///   (`guarded` | `relaxed`, the open path's container, S13) with the
///   relaxed container's own port and extra ports; `resources` (an
///   optional memory cap: no cap by default, L1; tasks, open files, CPU
///   weight and the per-deployment log cap, S17); box forwards first-class;
///   `dbPort` (the App's database door forwarded to `127.0.0.1:5432` in
///   the wall, for lane 3); the `build` verb (the manifest's build inside
///   the wall, S16) and the `logs` verb (each deployment's capped log).
pub const ENGINE_PROTOCOL: u32 = 3;

pub fn asset_name(arch: &str) -> String {
    format!("k2-app-engine-linux-{arch}")
}

pub fn trusted_comment(arch: &str, version: &str, protocol: u32) -> String {
    format!("file:{} version:{version} protocol:{protocol}", asset_name(arch))
}

/// The marker needle, stored shifted by one byte so the plain prefix
/// appears in a binary only inside its one embedded marker.
pub fn marker_prefix() -> String {
    let shifted: &[u8] = std::hint::black_box(b"L3`BQQ`FOHJOF`CVJME!w>");
    shifted.iter().map(|b| (b - 1) as char).collect()
}

pub fn format_marker(version: &str, protocol: u32, arch: &str) -> String {
    format!("{}{version} proto={protocol} arch={arch};", marker_prefix())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineMarker {
    pub version: Semver,
    pub protocol: u32,
    pub arch: String,
}

/// Every engine marker in `bytes`; `Err` on any doctored occurrence.
pub fn find_markers(bytes: &[u8]) -> Result<Vec<EngineMarker>, &'static str> {
    let needle = marker_prefix();
    let needle = needle.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        let rest = &bytes[i + needle.len()..];
        let end = rest.iter().take(80).position(|&b| b == b';').ok_or("marker not terminated")?;
        let body = std::str::from_utf8(&rest[..end]).map_err(|_| "marker not utf-8")?;
        let mut it = body.split(' ');
        let version = it.next().and_then(Semver::parse).ok_or("marker malformed")?;
        let protocol = it.next().and_then(|p| p.strip_prefix("proto=")).ok_or("marker malformed")?;
        let arch = it.next().and_then(|a| a.strip_prefix("arch=")).ok_or("marker malformed")?;
        if it.next().is_some() || !arch_ok(arch) || protocol.is_empty() || protocol.len() > 4 || !protocol.bytes().all(|b| b.is_ascii_digit()) {
            return Err("marker malformed");
        }
        out.push(EngineMarker { version, protocol: protocol.parse().map_err(|_| "marker malformed")?, arch: arch.into() });
        i += needle.len() + end + 1;
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEngineBuild {
    pub version: Semver,
    pub protocol: u32,
    pub arch: String,
    pub key_id: String,
    pub key_role: &'static str,
}

/// The one check (the helper before installing, the daemon before
/// spending a root call): a minisign signature by one of `keys` (incl.
/// the global signature over the trusted comment); the comment names this
/// arch's engine asset; a 64-bit ELF for that arch; exactly one marker,
/// equal to the comment. Static refusal strings only.
pub fn verify_engine_build(bytes: &[u8], sig_text: &str, keys: &[TrustedKey], arch: &str) -> Result<VerifiedEngineBuild, &'static str> {
    if !arch_ok(arch) {
        return Err("unsupported arch");
    }
    let sig = minisign_verify::Signature::decode(sig_text).map_err(|_| "signature unreadable")?;
    let mut verified: Option<&TrustedKey> = None;
    for k in keys {
        let Ok(pk) = minisign_verify::PublicKey::from_base64(k.public_key_b64) else { continue };
        if pk.verify(bytes, &sig, false).is_ok() {
            verified = Some(k);
            break;
        }
    }
    let key = verified.ok_or("signature not valid for any trusted key")?;
    let comment = parse_trusted_comment(sig.trusted_comment()).ok_or("trusted comment malformed")?;
    if comment.asset != asset_name(arch) {
        return Err("trusted comment names another file");
    }
    if !elf_matches(bytes, arch) {
        return Err("not an ELF for this arch");
    }
    let markers = find_markers(bytes)?;
    if markers.len() != 1 {
        return Err("build marker missing or repeated");
    }
    let m = &markers[0];
    if m.version != comment.version || m.protocol != comment.protocol || m.arch != arch {
        return Err("build marker does not match the trusted comment");
    }
    Ok(VerifiedEngineBuild {
        version: comment.version,
        protocol: comment.protocol,
        arch: arch.into(),
        key_id: key_id_of(key.public_key_b64).unwrap_or_else(|| key.key_id.to_string()),
        key_role: key.role,
    })
}

// ── canned bytes the helper writes ──────────────────────────────────────

/// `/etc/systemd/user/k2-app-engine.service`. A user unit that only
/// `k2app`'s manager runs (`ConditionUser=`); enabled by the helper with
/// `systemctl --user` as `k2app`. No root anywhere on this path.
///
/// No `NoNewPrivileges=`: rootless Podman maps each App's ids through the
/// setuid `newuidmap`/`newgidmap`, which that flag disarms (Linux e2e:
/// "write to uid_map failed: Operation not permitted"). The Apps
/// themselves always run with no-new-privileges (the Quadlet floor).
pub fn user_unit() -> &'static str {
    "# K2 App engine (prd-k2-app-runtime-v1 §10.6). Written by\n\
     # k2-root-helper `apps install-engine`. Runs only in k2app's manager.\n\
     [Unit]\n\
     Description=K2 App engine\n\
     ConditionUser=k2app\n\
     After=default.target\n\
     \n\
     [Service]\n\
     Type=simple\n\
     ExecStart=/usr/local/libexec/k2-app-engine serve\n\
     Restart=always\n\
     RestartSec=2\n\
     UMask=0077\n\
     LimitNOFILE=8192\n\
     \n\
     [Install]\n\
     WantedBy=default.target\n"
}

/// `/etc/tmpfiles.d/k2app.conf`: the socket dir, traversable only by
/// `k2app` and each configured daemon user (ACL), recreated every boot.
pub fn tmpfiles_conf(daemon_users: &[&str]) -> String {
    let mut s = String::from(
        "# K2 App engine socket dir. Written by k2-root-helper `apps install-engine`.\n\
         d /run/k2app 0750 k2app k2app -\n",
    );
    for u in daemon_users {
        s.push_str(&format!("a+ /run/k2app - - - - u:{u}:x\n"));
    }
    s
}

// ── the wire ────────────────────────────────────────────────────────────

/// Largest request header line (a start carries the App's secrets).
pub const MAX_HEADER: usize = 1024 * 1024;
/// Largest Seed tarball and image archive streamed to the engine.
pub const MAX_SEED_BYTES: u64 = 4 << 30;
pub const MAX_IMAGE_BYTES: u64 = 4 << 30;
/// Largest logs / exec output the engine returns.
pub const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// `{engine version, protocol, runtime, podman}`.
    Version,
    SeedHas { hash: String },
    /// Followed by `len` bytes of a tar of the Seed folder.
    SeedPut { hash: String, len: u64 },
    ImageHas { image: String },
    /// Followed by `len` bytes of an OCI archive (made by the daemon with
    /// skopeo from the pinned digest); loaded and tagged `image`.
    ImageLoad { image: String, len: u64 },
    /// Allocate the App's slot, make its folders, and pay the first-start
    /// cost of its id mapping (W: 13–20 s) before any switch.
    Prepare { app: String, image: String, seed: String },
    Start(Box<StartSpec>),
    Stop { app: String, deployment: String },
    Liveness { app: String, deployment: String },
    Stats { app: String, deployment: String },
    /// One command in the App's container, as the App (S28): its own
    /// user, capabilities, seccomp and MAC. Killed at `timeoutSecs`
    /// ([`EXEC_TIMEOUT_BOUNDS`]).
    #[serde(rename_all = "camelCase")]
    Exec { app: String, deployment: String, argv: Vec<String>, timeout_secs: u64 },
    /// An interactive command on a terminal (S28 `k2 app shell`). After an
    /// `ok` answer line the connection carries [`tty`] frames both ways
    /// until the command exits.
    ExecTty { app: String, deployment: String, argv: Vec<String>, cols: u16, rows: u16 },
    Adopt,
    /// Stop every instance of an App; `data` also deletes its folders and
    /// frees its slot (uninstall --purge).
    RemoveApp { app: String, data: bool },
    /// Replace the App's pass file (S14 rotation). Every running instance
    /// of the App mounts the same folder and reads the new pass.
    PassWrite { app: String, pass_hex: String },
    /// The `process` exception (§4.3): stage a deployment's code (a copy
    /// of the Seed) and its spec under `apps/<id>/stage-<deployment>/`,
    /// world-readable inside the App's own folder (the unit runs as
    /// `k2appx` and sees only that folder). Gives the App its gateway
    /// slot. Starts nothing: the root helper starts the system unit.
    ProcessStage(Box<ProcessSpec>),
    /// Make a staged deployment the one `k2app-process@<id>` runs (the
    /// unit must be stopped: the daemon stops it first).
    ProcessActivate { app: String, deployment: String },
    /// Which deployment the App's process folder holds (adopt, liveness).
    ProcessState { app: String },
    /// S16: run a manifest's build inside the wall (protocol 3). The Seed
    /// named is the source tree (seed-put first); the engine copies it into
    /// a fresh work folder owned by the App's ids, runs `argv` there in a
    /// container in the App's slot (its own ids, the floor, egress only
    /// through the App's proxy), and on success answers `{exitCode, output,
    /// tarLen}` followed by `tarLen` bytes: a tar of the built tree. A
    /// failed build answers `ok` with its exit code and output, no tar.
    Build(Box<BuildSpec>),
    /// The last `lines` lines of one deployment's own log (protocol 3:
    /// each container logs to a capped file, [`Resources::log_bytes`]).
    Logs { app: String, deployment: String, lines: u32 },
}

/// The wall a container start renders (protocol 3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wall {
    /// The guarded floor (§4.2): the Seed read-only at `/app`, a
    /// read-only root, one port, egress only to listed hosts.
    #[default]
    Guarded,
    /// The open path's relaxed container (§4.3, S13): everything in the
    /// floor except a writable root, a writable **copy** of the code at
    /// `/app` made at each start (Rosson Q3, 2026-10-17: the persistent
    /// state is `/data`), extra TCP ports on host loopback, and egress
    /// broad but still only through the App's proxy (logged, K24).
    Relaxed,
}

impl Wall {
    pub fn as_str(self) -> &'static str {
        match self {
            Wall::Guarded => "guarded",
            Wall::Relaxed => "relaxed",
        }
    }
}

/// Resource lines on every container (§6.2, S17; protocol 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resources {
    /// The App's memory cap. `None` = no per-App cap (L1, Rosson
    /// 2026-10-10, confirmed 2026-10-17): the Apps slice's ceiling is the
    /// bound and an OOM there takes the biggest App, never the cores. A cap
    /// also sets Node's heap to half of it ([`Resources::node_heap_mib`]).
    pub memory_mib: Option<u64>,
    /// `PidsLimit` (fork bombs, thread leaks).
    pub tasks: u32,
    /// Open files (`Ulimit=nofile`).
    pub nofile: u32,
    /// `CPUWeight` on the unit (`None` = systemd's default, 100).
    pub cpu_weight: Option<u32>,
    /// The deployment's log file cap (bytes): a log flood can't fill the
    /// disk.
    pub log_bytes: u64,
}

/// Bounds for [`Resources`] (inclusive).
pub const MEMORY_MIB_BOUNDS: (u64, u64) = (64, 4_194_304);
pub const TASKS_BOUNDS: (u32, u32) = (16, 65_536);
pub const NOFILE_BOUNDS: (u32, u32) = (256, 1_048_576);
pub const CPU_WEIGHT_BOUNDS: (u32, u32) = (1, 10_000);
pub const LOG_BYTES_BOUNDS: (u64, u64) = (1 << 20, 1 << 30);
/// §6.2 defaults: 512 tasks, 4096 open files, a 50 MiB log per deployment.
pub const DEFAULT_TASKS: u32 = 512;
pub const DEFAULT_NOFILE: u32 = 4096;
pub const DEFAULT_LOG_BYTES: u64 = 50 << 20;

impl Resources {
    /// K2's defaults: no memory cap (L1), §6.2's tasks, files and log cap.
    pub fn k2_default() -> Self {
        Self { memory_mib: None, tasks: DEFAULT_TASKS, nofile: DEFAULT_NOFILE, cpu_weight: None, log_bytes: DEFAULT_LOG_BYTES }
    }

    pub fn check(&self) -> Result<(), &'static str> {
        if self.memory_mib.is_some_and(|m| !(MEMORY_MIB_BOUNDS.0..=MEMORY_MIB_BOUNDS.1).contains(&m)) {
            return Err("memory");
        }
        if !(TASKS_BOUNDS.0..=TASKS_BOUNDS.1).contains(&self.tasks) {
            return Err("tasks");
        }
        if !(NOFILE_BOUNDS.0..=NOFILE_BOUNDS.1).contains(&self.nofile) {
            return Err("nofile");
        }
        if self.cpu_weight.is_some_and(|w| !(CPU_WEIGHT_BOUNDS.0..=CPU_WEIGHT_BOUNDS.1).contains(&w)) {
            return Err("cpu weight");
        }
        if !(LOG_BYTES_BOUNDS.0..=LOG_BYTES_BOUNDS.1).contains(&self.log_bytes) {
            return Err("log cap");
        }
        Ok(())
    }

    /// Node's `--max-old-space-size`: half the cap, only when there is one
    /// (Q6/AR45: the heap error, with a stack, comes before the kernel's
    /// kill). No cap, no flag: V8 sizes its own heap.
    pub fn node_heap_mib(&self) -> Option<u64> {
        self.memory_mib.map(|m| m / 2)
    }
}

/// One manifest build inside the wall (S16, protocol 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildSpec {
    /// The App whose slot (ids, egress proxy, nft rule) the build runs in.
    pub app: String,
    /// `b-<12 hex>`: names the build's unit and work folder.
    pub build: String,
    /// The source tree (a Seed hash the engine holds).
    pub seed: String,
    pub image: String,
    /// The manifest's `build` argv, run in `/app` (the work copy).
    pub argv: Vec<String>,
    /// Declared, non-secret variables (no secrets reach a build).
    pub env: BTreeMap<String, String>,
    /// Package install scripts: off on the guarded path
    /// (`npm_config_ignore_scripts=true` and friends), on for the open path.
    pub scripts: bool,
    pub timeout_secs: u64,
    pub resources: Resources,
}

/// A build's bounds (seconds): up to an hour, 15 minutes by default.
pub const BUILD_TIMEOUT_BOUNDS: (u64, u64) = (10, 3600);
pub const BUILD_TIMEOUT_DEFAULT: u64 = 900;
/// Largest built tree the engine streams back.
pub const MAX_BUILD_TAR: u64 = 4 << 30;
/// Most lines a `logs` call returns.
pub const MAX_LOG_LINES: u32 = 5000;

/// `b-<12 lowercase hex>`, the build id.
pub fn build_id_ok(b: &str) -> bool {
    b.len() == 14 && b.starts_with("b-") && b[2..].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Every field of a build (the engine runs this before anything).
pub fn check_build(s: &BuildSpec) -> Result<AppId, &'static str> {
    let app = AppId::parse(&s.app).ok_or("app id")?;
    if !build_id_ok(&s.build) {
        return Err("build id");
    }
    if !seed_hash_ok(&s.seed) {
        return Err("seed hash");
    }
    if !image_ok(&s.image) {
        return Err("image");
    }
    if !exec_argv_ok(&s.argv) {
        return Err("argv");
    }
    if s.env.len() > 256 || !s.env.iter().all(|(k, v)| env_name_ok(k) && value_ok(v, 32 * 1024)) {
        return Err("env");
    }
    if !(BUILD_TIMEOUT_BOUNDS.0..=BUILD_TIMEOUT_BOUNDS.1).contains(&s.timeout_secs) {
        return Err("timeout");
    }
    s.resources.check()?;
    Ok(app)
}

/// The environment a guarded build runs with on top of the declared one:
/// package managers' install scripts off (S16: a `postinstall` in a
/// dependency never runs on the guarded path), no audit or update calls
/// home, the cache inside the work tree's tmp.
pub fn no_scripts_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("npm_config_ignore_scripts", "true"),
        ("YARN_ENABLE_SCRIPTS", "false"),
        ("npm_config_audit", "false"),
        ("npm_config_fund", "false"),
        ("npm_config_update_notifier", "false"),
    ]
}

/// One `process` exception deployment (no image: the host's own
/// programs; the Seed is the code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessSpec {
    pub app: String,
    pub deployment: String,
    pub seed: String,
    pub port: u16,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub memory_mib: u64,
    pub secrets: Vec<SecretItem>,
    pub pass_hex: Option<String>,
}

/// What `k2-app-engine process-run` reads inside the unit
/// (`apps/<id>/process/spec.json`): the command and its whole
/// environment, already resolved by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessRun {
    pub app: String,
    pub deployment: String,
    pub port: u16,
    pub gw_port: u16,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// The `process` exception's paths, inside and outside the unit.
pub mod process {
    use crate::apps::{AppId, APP_HOME, PROCESS_HOME};
    /// `apps/<id>/process` (bound read-only into the unit, same path).
    pub fn dir(app: &AppId) -> String {
        format!("{APP_HOME}/apps/{}/process", app.as_str())
    }
    pub fn spec(app: &AppId) -> String {
        format!("{}/spec.json", dir(app))
    }
    pub fn code(app: &AppId) -> String {
        format!("{}/code", dir(app))
    }
    /// The App's writable state (`StateDirectory=k2appx/<id>`).
    pub fn state(app: &AppId) -> String {
        format!("{PROCESS_HOME}/{}", app.as_str())
    }
}

/// Bounds for one-shot exec timeouts (§5.10: 60 s default, up to 10 min).
pub const EXEC_TIMEOUT_BOUNDS: (u64, u64) = (1, 600);
pub const EXEC_TIMEOUT_DEFAULT: u64 = 60;

/// The terminal stream after an `exec-tty` answer: `[kind u8][len u32 BE]
/// [payload]`, payload ≤ [`tty::MAX_FRAME`].
pub mod tty {
    /// Daemon → engine: bytes for the command's terminal.
    pub const INPUT: u8 = 1;
    /// Daemon → engine: `[cols u16 BE][rows u16 BE]`.
    pub const RESIZE: u8 = 2;
    /// Daemon → engine: hang up (the command gets SIGHUP from its tty).
    pub const HANGUP: u8 = 3;
    /// Engine → daemon: bytes the command wrote.
    pub const OUTPUT: u8 = 17;
    /// Engine → daemon: `[code i32 BE]` (-1 = killed or unknown); last.
    pub const EXIT: u8 = 18;
    pub const MAX_FRAME: usize = 64 * 1024;

    pub fn encode(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(5 + payload.len());
        v.push(kind);
        v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// Read one frame; `Ok(None)` at a clean end of stream.
    pub fn read(r: &mut dyn std::io::Read) -> std::io::Result<Option<(u8, Vec<u8>)>> {
        let mut h = [0u8; 5];
        let mut got = 0;
        while got < 5 {
            match r.read(&mut h[got..])? {
                0 if got == 0 => return Ok(None),
                0 => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "short frame header")),
                n => got += n,
            }
        }
        let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
        if len > MAX_FRAME {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
        }
        let mut p = vec![0u8; len];
        r.read_exact(&mut p)?;
        Ok(Some((h[0], p)))
    }
}

/// One secret for a container: a Podman secret, never in the unit file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretItem {
    /// `Some(VAR)`: an environment variable. `None`: a file
    /// `/run/secrets/<file>` (mode 0400).
    pub env: Option<String>,
    pub file: Option<String>,
    /// The value, hex (file secrets may be binary).
    pub value_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartSpec {
    pub app: String,
    pub deployment: String,
    pub seed: String,
    pub image: String,
    /// The wall rendered (protocol 3): the guarded floor or the open
    /// path's relaxed container.
    pub wall: Wall,
    /// Host loopback port the App's `$PORT` is published on.
    pub port: u16,
    /// Relaxed only: the port the App listens on inside its wall (`$PORT`;
    /// default = `port`). An old `--cmd` service keeps the port it always
    /// used inside, behind a fresh host port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_port: Option<u16>,
    /// Relaxed only: more TCP ports the App listens on, published on host
    /// loopback under the same number (never public in 0.46.1; direct
    /// public ports and UDP are 0.46.2, Rosson Q3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_ports: Vec<u16>,
    pub argv: Vec<String>,
    /// Declared, non-secret variables plus K2's own (an env file; never
    /// systemd-expanded).
    pub env: BTreeMap<String, String>,
    /// Memory (optional), tasks, files, CPU weight, log cap (S17).
    pub resources: Resources,
    /// The App's drain window; the container's stop timeout is longer
    /// (AR45: `StopTimeout` ≥ drain).
    pub drain_secs: u64,
    pub secrets: Vec<SecretItem>,
    /// The App pass (S14), hex: written to the App's pass folder
    /// ([`PASS_ROOT`]) and mounted at [`PASS_MOUNT`] (never a Podman
    /// secret: a secret is copied once at start and can't rotate).
    pub pass_hex: Option<String>,
    /// The App's disk ceilings (§6.2, `io.max` through the unit).
    pub io: IoMax,
    /// Box services (owner-approved): pasta forwards the container's
    /// `127.0.0.1:<appPort>` to host `127.0.0.1:<hostPort>`, like the
    /// gateway link. Approved or still held; the per-App nft rule (root
    /// helper) admits only the approved ones, so a held or revoked one is
    /// a forward to a shut door (refused at once, never a stall).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub box_forwards: Vec<BoxForward>,
    /// The App's database door on host loopback (lane 3, S21): forwarded
    /// to [`IN_WALL_DB_PORT`] in the wall, like the gateway link. Inside
    /// [`crate::apps::DB_PORTS`] (the range the nft table admits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_port: Option<u16>,
}

/// The database door's port inside the wall: the App dials
/// `127.0.0.1:5432` like any Postgres client.
pub const IN_WALL_DB_PORT: u16 = 5432;
/// Most extra ports a relaxed container publishes.
pub const MAX_EXTRA_PORTS: usize = 8;

/// The ports the App itself listens on inside its wall: `$PORT` (or the
/// relaxed `appPort`) and the extra ports.
pub fn own_ports(s: &StartSpec) -> Vec<u16> {
    let mut v = vec![s.app_port.unwrap_or(s.port)];
    v.extend(s.extra_ports.iter().copied());
    v
}

/// One box service forward into the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoxForward {
    pub app_port: u16,
    pub host_port: u16,
}

/// What an engine build supports beyond its protocol number (the
/// `version` answer's `features`). Protocol 3 carries everything the
/// 0.46.1 wall needs; the list names it for the doctor.
pub const FEATURE_BOX_FORWARD: &str = "box-forward";
pub const FEATURE_RELAXED: &str = "relaxed";
pub const FEATURE_BUILD: &str = "build";
pub const FEATURE_DB_FORWARD: &str = "db-forward";
pub const ENGINE_FEATURES: &[&str] = &[FEATURE_BOX_FORWARD, FEATURE_RELAXED, FEATURE_BUILD, FEATURE_DB_FORWARD];

/// A box forward's port inside the wall: an App port shape (≥ 1024, not
/// 8730, not K2's range), never the App's own port, never the database
/// door's in-wall port.
pub fn box_forward_ok(f: &BoxForward, app_port: u16) -> bool {
    app_port_ok(f.app_port) && f.app_port != app_port && f.app_port != IN_WALL_DB_PORT && crate::apps::box_port_ok(f.host_port)
}

/// A database door port on host loopback: inside the range the nft table
/// admits for every App ([`crate::apps::DB_PORTS`]).
pub fn db_port_ok(p: u16) -> bool {
    (crate::apps::DB_PORTS.lo..=crate::apps::DB_PORTS.hi).contains(&p)
}

/// The pass folder inside the container, read-only.
pub const PASS_MOUNT: &str = "/run/k2";
/// Where the App pass appears inside the container (`K2_PASS_FILE`).
pub const PASS_TARGET: &str = "/run/k2/pass";
pub const SECRETS_TARGET_DIR: &str = "/run/secrets";

/// One answer line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Answer {
    pub fn ok(data: serde_json::Value) -> Self {
        Self { ok: true, code: None, message: None, data }
    }
    pub fn err(code: &str, message: impl Into<String>) -> Self {
        Self { ok: false, code: Some(code.into()), message: Some(message.into()), data: serde_json::Value::Null }
    }
}

/// Error codes the engine answers with.
pub mod codes {
    pub const CALLER_NOT_DAEMON: &str = "caller_not_daemon";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const NOT_FOUND: &str = "not_found";
    pub const NO_SLOT: &str = "no_slot";
    pub const PODMAN: &str = "podman_failed";
    pub const SYSTEMD: &str = "systemd_failed";
    pub const IO: &str = "io_failed";
    pub const TOO_LARGE: &str = "too_large";
}

// ── validators ──────────────────────────────────────────────────────────

pub fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect()
}

/// `d-<12 lowercase hex>` (k2-core `store::insert_deployment`).
pub fn deployment_ok(d: &str) -> bool {
    d.len() == 14 && d.starts_with("d-") && d[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A Seed hash: 64 lowercase hex.
pub fn seed_hash_ok(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// K2's local base-image names: `localhost/k2-base/<name>:<tag>`, lower
/// alnum and `-` in the name, `[a-z0-9.-]` in the tag.
pub fn image_ok(i: &str) -> bool {
    let Some(rest) = i.strip_prefix("localhost/k2-base/") else { return false };
    let Some((name, tag)) = rest.split_once(':') else { return false };
    !name.is_empty()
        && name.len() <= 40
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !tag.is_empty()
        && tag.len() <= 64
        && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

/// The App's own published port: ≥ 1024, outside K2's reserved range, and
/// never the in-wall gateway port.
pub fn app_port_ok(p: u16) -> bool {
    p >= 1024 && p != IN_WALL_GATEWAY_PORT && !(RESERVED_PORTS.0..=RESERVED_PORTS.1).contains(&p)
}

/// A pass on the wire: hex of a `k2skn_` pass (`k2skn_` + base62), bounded.
/// The engine writes it to a file and never parses it further.
pub fn pass_hex_ok(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && hex_decode(p).is_some_and(|b| b.starts_with(b"k2skn_") && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_'))
}

/// POSIX-ish env name, not one K2 sets for the container itself.
pub fn env_name_ok(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_uppercase() || b[0] == b'_' || b[0].is_ascii_lowercase())
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// No control characters (an env file line, a unit line), bounded.
pub fn value_ok(v: &str, max: usize) -> bool {
    v.len() <= max && !v.chars().any(|c| c.is_control())
}

/// A file secret's name under `/run/secrets`.
pub fn secret_file_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Every field of a start, checked (the engine runs this before anything).
pub fn check_start(s: &StartSpec) -> Result<AppId, &'static str> {
    let app = AppId::parse(&s.app).ok_or("app id")?;
    if !deployment_ok(&s.deployment) {
        return Err("deployment id");
    }
    if !seed_hash_ok(&s.seed) {
        return Err("seed hash");
    }
    if !image_ok(&s.image) {
        return Err("image");
    }
    if !app_port_ok(s.port) {
        return Err("port");
    }
    if s.wall == Wall::Guarded && (s.app_port.is_some() || !s.extra_ports.is_empty()) {
        return Err("a guarded container has one port");
    }
    if s.app_port.is_some_and(|p| !app_port_ok(p)) {
        return Err("app port");
    }
    if s.extra_ports.len() > MAX_EXTRA_PORTS {
        return Err("extra ports");
    }
    let own = own_ports(s);
    for (i, p) in own.iter().enumerate() {
        if !app_port_ok(*p) || own[..i].contains(p) || (s.db_port.is_some() && *p == IN_WALL_DB_PORT) {
            return Err("extra ports");
        }
    }
    // An extra port is published on host loopback under its own number:
    // never the host port K2 picked for `$PORT`.
    if s.extra_ports.contains(&s.port) {
        return Err("extra ports");
    }
    if s.argv.is_empty() || s.argv.len() > 64 || !s.argv.iter().all(|a| !a.is_empty() && value_ok(a, 4096)) {
        return Err("argv");
    }
    if s.env.len() > 256 || !s.env.iter().all(|(k, v)| env_name_ok(k) && value_ok(v, 32 * 1024)) {
        return Err("env");
    }
    s.resources.check()?;
    if s.db_port.is_some_and(|p| !db_port_ok(p)) {
        return Err("db port");
    }
    if s.drain_secs > 600 {
        return Err("drain");
    }
    if s.secrets.len() > 64 {
        return Err("secrets");
    }
    for it in &s.secrets {
        match (&it.env, &it.file) {
            (Some(e), None) if env_name_ok(e) => {}
            (None, Some(f)) if secret_file_ok(f) => {}
            _ => return Err("secret target"),
        }
        if it.value_hex.len() > 2 * 1024 * 1024 || hex_decode(&it.value_hex).is_none() {
            return Err("secret value");
        }
    }
    if let Some(p) = &s.pass_hex {
        if !pass_hex_ok(p) {
            return Err("pass");
        }
    }
    if s.io.check().is_err() {
        return Err("io");
    }
    if s.box_forwards.len() > crate::apps::MAX_BOX_SERVICES {
        return Err("box forwards");
    }
    for (i, f) in s.box_forwards.iter().enumerate() {
        if !own.iter().all(|p| box_forward_ok(f, *p)) || s.box_forwards[..i].iter().any(|g| g.app_port == f.app_port) {
            return Err("box forward");
        }
    }
    Ok(app)
}

/// A one-shot or terminal exec's argv (no empty word, no control chars).
pub fn exec_argv_ok(argv: &[String]) -> bool {
    !argv.is_empty() && argv.len() <= 64 && argv.iter().all(|a| !a.is_empty() && value_ok(a, 4096))
}

/// Every field of a process deployment (the engine runs this first).
pub fn check_process(s: &ProcessSpec) -> Result<AppId, &'static str> {
    let app = AppId::parse(&s.app).ok_or("app id")?;
    if !deployment_ok(&s.deployment) {
        return Err("deployment id");
    }
    if !seed_hash_ok(&s.seed) {
        return Err("seed hash");
    }
    if !app_port_ok(s.port) {
        return Err("port");
    }
    if !exec_argv_ok(&s.argv) {
        return Err("argv");
    }
    if s.env.len() > 256 || !s.env.iter().all(|(k, v)| env_name_ok(k) && value_ok(v, 32 * 1024)) {
        return Err("env");
    }
    if !(64..=65_536).contains(&s.memory_mib) {
        return Err("memory");
    }
    if s.secrets.len() > 64 {
        return Err("secrets");
    }
    for it in &s.secrets {
        match (&it.env, &it.file) {
            (Some(e), None) if env_name_ok(e) => {}
            (None, Some(f)) if secret_file_ok(f) => {}
            _ => return Err("secret target"),
        }
        if it.value_hex.len() > 2 * 1024 * 1024 || hex_decode(&it.value_hex).is_none() {
            return Err("secret value");
        }
    }
    if let Some(p) = &s.pass_hex {
        if p.len() > 64 * 1024 || hex_decode(p).is_none() {
            return Err("pass");
        }
    }
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> StartSpec {
        StartSpec {
            app: "nsi-skin".into(),
            deployment: "d-0123456789ab".into(),
            seed: "a".repeat(64),
            image: "localhost/k2-base/node24:ec857ea6bb60".into(),
            wall: Wall::Guarded,
            port: 41234,
            app_port: None,
            extra_ports: vec![],
            argv: vec!["node".into(), "server.mjs".into()],
            env: [("NODE_ENV".to_string(), "production".to_string())].into(),
            resources: Resources::k2_default(),
            drain_secs: 30,
            secrets: vec![SecretItem { env: None, file: Some("db.json".into()), value_hex: hex_encode(b"{}") }],
            pass_hex: Some(hex_encode(b"k2skn_x")),
            io: IoMax::k2_default(),
            box_forwards: vec![],
            db_port: None,
        }
    }

    fn relaxed() -> StartSpec {
        let mut s = spec();
        s.wall = Wall::Relaxed;
        s.app_port = Some(3000);
        s.extra_ports = vec![9000, 9001];
        s
    }

    #[test]
    fn names_and_slots() {
        let a = AppId::parse("nsi-skin").unwrap();
        assert_eq!(unit_stem(&a, "d-0123456789ab"), "k2app-nsi-skin--d-0123456789ab");
        assert_eq!(app_slice(&a), "k2apps-nsi_skin.slice");
        assert_eq!(slot_base(0), 1);
        assert_eq!(slot_base(1), 65_537);
        assert_eq!(MAX_SLOTS, 64);
        assert_eq!(slot_base(MAX_SLOTS - 1) + SLOT_SIZE - 1, crate::apps::SUBID_COUNT);
        assert_eq!(gateway_port(0), Some(38_600));
        assert_eq!(gateway_port(MAX_SLOTS - 1), Some(38_663));
        assert_eq!(gateway_port(400), None);
        assert_eq!(proxy_port(0), Some(38_900));
        assert_eq!(proxy_port(MAX_SLOTS - 1), Some(38_963));
        assert_eq!(proxy_port(400), None);
    }

    #[test]
    fn proxy_env_points_at_the_own_proxy_and_keeps_loopback_direct() {
        let e: BTreeMap<&str, String> = proxy_env(38_901).into_iter().collect();
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY"] {
            assert_eq!(e[k], "http://127.0.0.1:38901", "{k}");
        }
        assert_eq!(e["NO_PROXY"], "127.0.0.1,localhost,::1");
        assert_eq!(e["no_proxy"], e["NO_PROXY"]);
        assert_eq!(e["NODE_USE_ENV_PROXY"], "1");
        assert!(e.iter().all(|(k, v)| env_name_ok(k) && value_ok(v, 256)));
    }

    #[test]
    fn start_checks() {
        assert!(check_start(&spec()).is_ok());
        type Tweak = Box<dyn Fn(&mut StartSpec)>;
        let bad: Vec<Tweak> = vec![
            Box::new(|s| s.app = "../x".into()),
            Box::new(|s| s.app = "a--b".into()),
            Box::new(|s| s.deployment = "d-0123456789AB".into()),
            Box::new(|s| s.deployment = "d-../../etc".into()),
            Box::new(|s| s.seed = "../".repeat(10)),
            Box::new(|s| s.image = "docker.io/library/node:24".into()),
            Box::new(|s| s.image = "localhost/k2-base/node24:latest;rm".into()),
            Box::new(|s| s.port = 80),
            Box::new(|s| s.port = 38_700),
            Box::new(|s| s.argv = vec![]),
            Box::new(|s| s.argv = vec!["node\n[Service]".into()]),
            Box::new(|s| {
                s.env.insert("BAD-NAME".into(), "x".into());
            }),
            Box::new(|s| {
                s.env.insert("X".into(), "a\nb".into());
            }),
            Box::new(|s| s.resources.memory_mib = Some(1)),
            Box::new(|s| s.resources.tasks = 1),
            Box::new(|s| s.resources.nofile = 10),
            Box::new(|s| s.resources.log_bytes = 10),
            Box::new(|s| s.resources.cpu_weight = Some(0)),
            Box::new(|s| s.drain_secs = 3600),
            Box::new(|s| s.secrets[0].file = Some("../shadow".into())),
            Box::new(|s| s.secrets[0].env = Some("X".into())),
            Box::new(|s| s.secrets[0].value_hex = "zz".into()),
            Box::new(|s| s.pass_hex = Some("abc".into())),
            Box::new(|s| s.pass_hex = Some(hex_encode(b"not-a-pass"))),
            Box::new(|s| s.pass_hex = Some(hex_encode(b"k2skn_a\nb"))),
            Box::new(|s| s.port = IN_WALL_GATEWAY_PORT),
            Box::new(|s| s.io.wbps = Some(1)),
            Box::new(|s| s.io.wiops = Some(u64::MAX)),
            // Box forwards: the deny-list on the host side; an App-port
            // shape inside, never the App's own port; no repeats; at most 8.
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 8470, host_port: 22 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 8470, host_port: 5432 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 8470, host_port: 38_601 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: IN_WALL_GATEWAY_PORT, host_port: 8470 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 38_901, host_port: 8470 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 41_234, host_port: 8470 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 80, host_port: 8470 }]),
            Box::new(|s| s.box_forwards = vec![BoxForward { app_port: 8470, host_port: 8470 }, BoxForward { app_port: 8470, host_port: 7880 }]),
            Box::new(|s| s.box_forwards = (0..9).map(|i| BoxForward { app_port: 9000 + i, host_port: 9000 + i }).collect()),
        ];
        for (i, f) in bad.iter().enumerate() {
            let mut s = spec();
            f(&mut s);
            assert!(check_start(&s).is_err(), "case {i}");
        }
    }

    #[test]
    fn box_forwards_are_first_class_in_protocol_3() {
        let plain = serde_json::to_string(&Request::Start(Box::new(spec()))).unwrap();
        assert!(!plain.contains("boxForwards") && !plain.contains("dbPort") && !plain.contains("extraPorts"), "{plain}");
        assert!(plain.contains("\"wall\":\"guarded\""), "{plain}");
        let mut s = spec();
        s.box_forwards = vec![BoxForward { app_port: 8470, host_port: 8470 }, BoxForward { app_port: 7880, host_port: 7880 }];
        assert!(check_start(&s).is_ok());
        let line = serde_json::to_string(&Request::Start(Box::new(s.clone()))).unwrap();
        assert!(line.contains("\"boxForwards\":[{\"appPort\":8470,\"hostPort\":8470}"), "{line}");
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back, Request::Start(Box::new(s)));
        // Remapped inside the wall is fine.
        let mut s = spec();
        s.box_forwards = vec![BoxForward { app_port: 9000, host_port: 8470 }];
        assert!(check_start(&s).is_ok());
        assert!(serde_json::from_str::<BoxForward>(r#"{"appPort":1,"hostPort":2,"proto":"udp"}"#).is_err());
        assert_eq!(ENGINE_PROTOCOL, 3, "0.46.1's wall is one engine bump");
        for f in [FEATURE_BOX_FORWARD, FEATURE_RELAXED, FEATURE_BUILD, FEATURE_DB_FORWARD] {
            assert!(ENGINE_FEATURES.contains(&f), "{f}");
        }
        // A relaxed container's box forwards stay off every port the App
        // itself listens on (its own port and the extra ones).
        let mut s = relaxed();
        s.box_forwards = vec![BoxForward { app_port: 9001, host_port: 8470 }];
        assert_eq!(check_start(&s), Err("box forward"));
        s.box_forwards = vec![BoxForward { app_port: 3000, host_port: 8470 }];
        assert_eq!(check_start(&s), Err("box forward"));
        s.box_forwards = vec![BoxForward { app_port: 8470, host_port: 8470 }];
        assert!(check_start(&s).is_ok());
    }

    #[test]
    fn the_relaxed_wall_and_its_ports() {
        assert!(check_start(&relaxed()).is_ok());
        assert_eq!(own_ports(&relaxed()), vec![3000, 9000, 9001]);
        assert_eq!(own_ports(&spec()), vec![41234], "guarded: one port, the same inside and out");
        type Tweak = Box<dyn Fn(&mut StartSpec)>;
        let bad: Vec<(Tweak, &str)> = vec![
            // A guarded container has exactly one port.
            (Box::new(|s| { s.wall = Wall::Guarded; }), "a guarded container has one port"),
            (Box::new(|s| { s.app_port = Some(80); }), "app port"),
            (Box::new(|s| { s.app_port = Some(IN_WALL_GATEWAY_PORT); }), "app port"),
            (Box::new(|s| { s.app_port = Some(38_650); }), "app port"),
            (Box::new(|s| { s.extra_ports = vec![9000, 9000]; }), "extra ports"),
            (Box::new(|s| { s.extra_ports = vec![3000]; }), "extra ports"),
            (Box::new(|s| { s.extra_ports = vec![41234]; }), "extra ports"),
            (Box::new(|s| { s.extra_ports = vec![1000]; }), "extra ports"),
            (Box::new(|s| { s.extra_ports = (0..9).map(|i| 9100 + i).collect(); }), "extra ports"),
        ];
        for (i, (f, why)) in bad.iter().enumerate() {
            let mut s = relaxed();
            f(&mut s);
            assert_eq!(check_start(&s), Err(*why), "case {i}");
        }
        assert_eq!(Wall::default(), Wall::Guarded);
        assert_eq!(serde_json::to_string(&Wall::Relaxed).unwrap(), "\"relaxed\"");
        assert!(serde_json::from_str::<Wall>("\"open\"").is_err(), "the wire names walls, not paths");
    }

    #[test]
    fn resources_have_no_memory_cap_by_default() {
        let r = Resources::k2_default();
        assert_eq!(r.memory_mib, None, "L1: the user decides App memory");
        assert_eq!(r.node_heap_mib(), None, "no cap, no heap flag");
        assert_eq!((r.tasks, r.nofile, r.log_bytes), (512, 4096, 50 << 20));
        assert!(r.check().is_ok());
        let capped = Resources { memory_mib: Some(1024), ..r };
        assert_eq!(capped.node_heap_mib(), Some(512), "Node heap at 50% only when capped");
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.contains("\"memoryMib\":null"), "{line}");
        assert!(serde_json::from_str::<Resources>(r#"{"memoryMib":null,"tasks":512,"nofile":4096,"cpuWeight":null,"logBytes":1048576,"swap":1}"#).is_err());
    }

    #[test]
    fn the_db_door_forward() {
        let mut s = spec();
        s.db_port = Some(5432);
        assert!(check_start(&s).is_ok());
        assert!(serde_json::to_string(&s).unwrap().contains("\"dbPort\":5432"));
        // K2's Postgres band (0.46.3): a database on another major's
        // cluster (5433…5449) is reachable through the door too.
        s.db_port = Some(5433);
        assert!(check_start(&s).is_ok(), "the band the nft table admits");
        s.db_port = Some(5449);
        assert!(check_start(&s).is_ok());
        s.db_port = Some(5450);
        assert_eq!(check_start(&s), Err("db port"), "only the range the nft table admits");
        s.db_port = Some(5431);
        assert_eq!(check_start(&s), Err("db port"));
        // With the door, nothing else of the App's may sit on 5432 inside.
        let mut s = relaxed();
        s.db_port = Some(5432);
        s.extra_ports = vec![5432];
        assert_eq!(check_start(&s), Err("extra ports"));
        let mut s = spec();
        s.db_port = Some(5432);
        s.box_forwards = vec![BoxForward { app_port: IN_WALL_DB_PORT, host_port: 8470 }];
        assert_eq!(check_start(&s), Err("box forward"));
        assert!(!db_port_ok(22) && db_port_ok(5432));
    }

    fn bspec() -> BuildSpec {
        BuildSpec {
            app: "hearth".into(),
            build: "b-0123456789ab".into(),
            seed: "c".repeat(64),
            image: "localhost/k2-base/node24:ec857ea6bb60".into(),
            argv: vec!["npm".into(), "ci".into()],
            env: [("NODE_ENV".to_string(), "production".to_string())].into(),
            scripts: false,
            timeout_secs: BUILD_TIMEOUT_DEFAULT,
            resources: Resources::k2_default(),
        }
    }

    #[test]
    fn build_checks_and_wire() {
        assert!(check_build(&bspec()).is_ok());
        type Tweak = Box<dyn Fn(&mut BuildSpec)>;
        let bad: Vec<Tweak> = vec![
            Box::new(|s| s.app = "../x".into()),
            Box::new(|s| s.build = "d-0123456789ab".into()),
            Box::new(|s| s.build = "b-../../etc/xx".into()),
            Box::new(|s| s.seed = "x".into()),
            Box::new(|s| s.image = "docker.io/library/node:24".into()),
            Box::new(|s| s.argv = vec![]),
            Box::new(|s| s.argv = vec!["npm\nci".into()]),
            Box::new(|s| {
                s.env.insert("A-B".into(), "x".into());
            }),
            Box::new(|s| s.timeout_secs = 1),
            Box::new(|s| s.timeout_secs = 7200),
            Box::new(|s| s.resources.tasks = 0),
        ];
        for (i, f) in bad.iter().enumerate() {
            let mut s = bspec();
            f(&mut s);
            assert!(check_build(&s).is_err(), "case {i}");
        }
        let r = Request::Build(Box::new(bspec()));
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.starts_with("{\"verb\":\"build\""), "{line}");
        assert!(line.contains("\"timeoutSecs\":900") && line.contains("\"scripts\":false"), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
        let l = Request::Logs { app: "hearth".into(), deployment: "d-0123456789ab".into(), lines: 200 };
        let line = serde_json::to_string(&l).unwrap();
        assert!(line.starts_with("{\"verb\":\"logs\""), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), l);
        let env: BTreeMap<_, _> = no_scripts_env().into_iter().collect();
        assert_eq!(env["npm_config_ignore_scripts"], "true");
        assert_eq!(env["YARN_ENABLE_SCRIPTS"], "false");
        assert!(build_id_ok("b-0123456789ab") && !build_id_ok("b-0123456789AB"));
    }

    #[test]
    fn wire_round_trips_and_refuses_unknown_fields() {
        let r = Request::Start(Box::new(spec()));
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.starts_with("{\"verb\":\"start\""), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
        let r = Request::PassWrite { app: "jobs".into(), pass_hex: hex_encode(b"k2skn_abc") };
        let line = serde_json::to_string(&r).unwrap();
        assert!(line.starts_with("{\"verb\":\"pass-write\""), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
        let r = Request::SeedPut { hash: "a".repeat(64), len: 9 };
        assert_eq!(serde_json::to_string(&r).unwrap(), format!("{{\"verb\":\"seed-put\",\"hash\":\"{}\",\"len\":9}}", "a".repeat(64)));
        assert!(serde_json::from_str::<Request>(&format!("{{\"verb\":\"seed-has\",\"hash\":\"{}\",\"x\":1}}", "a".repeat(64))).is_err());
        assert!(serde_json::from_str::<Request>("{\"verb\":\"stop\",\"app\":\"a\",\"deployment\":\"d-0123456789ab\",\"graceSecs\":1}").is_err());
        assert!(serde_json::from_str::<Request>("{\"verb\":\"shell\",\"cmd\":\"id\"}").is_err());
        // exec carries its timeout; exec-tty its terminal size.
        let e = Request::Exec { app: "jobs".into(), deployment: "d-0123456789ab".into(), argv: vec!["ls".into()], timeout_secs: 60 };
        let line = serde_json::to_string(&e).unwrap();
        assert!(line.contains("\"timeoutSecs\":60"), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), e);
        assert!(serde_json::from_str::<Request>("{\"verb\":\"exec\",\"app\":\"a\",\"deployment\":\"d-0123456789ab\",\"argv\":[\"ls\"]}").is_err(), "no timeout: refused");
        let t = Request::ExecTty { app: "jobs".into(), deployment: "d-0123456789ab".into(), argv: vec!["/bin/sh".into()], cols: 120, rows: 40 };
        assert!(serde_json::to_string(&t).unwrap().starts_with("{\"verb\":\"exec-tty\""));
        let ps = Request::ProcessStage(Box::new(pspec()));
        let line = serde_json::to_string(&ps).unwrap();
        assert!(line.starts_with("{\"verb\":\"process-stage\""), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), ps);
        assert_eq!(hex_decode(&hex_encode(b"\x00\xffk2")).unwrap(), b"\x00\xffk2");
    }

    fn pspec() -> ProcessSpec {
        ProcessSpec {
            app: "gpu-job".into(),
            deployment: "d-0123456789ab".into(),
            seed: "b".repeat(64),
            port: 41_000,
            argv: vec!["python3".into(), "serve.py".into()],
            env: [("MODEL".to_string(), "small".to_string())].into(),
            memory_mib: 2048,
            secrets: vec![SecretItem { env: Some("HF_TOKEN".into()), file: None, value_hex: hex_encode(b"x") }],
            pass_hex: None,
        }
    }

    #[test]
    fn process_checks_and_paths() {
        assert!(check_process(&pspec()).is_ok());
        type Tweak = Box<dyn Fn(&mut ProcessSpec)>;
        let bad: Vec<Tweak> = vec![
            Box::new(|s| s.app = "../x".into()),
            Box::new(|s| s.deployment = "d-1".into()),
            Box::new(|s| s.seed = "x".into()),
            Box::new(|s| s.port = 38_650),
            Box::new(|s| s.argv = vec![]),
            Box::new(|s| s.argv = vec!["a\nb".into()]),
            Box::new(|s| s.memory_mib = 32),
            Box::new(|s| {
                s.env.insert("A-B".into(), "x".into());
            }),
            Box::new(|s| s.secrets[0].env = Some("1X".into())),
        ];
        for (i, f) in bad.iter().enumerate() {
            let mut s = pspec();
            f(&mut s);
            assert!(check_process(&s).is_err(), "case {i}");
        }
        let a = AppId::parse("gpu-job").unwrap();
        assert_eq!(process::spec(&a), "/var/lib/k2app/apps/gpu-job/process/spec.json");
        assert_eq!(process::code(&a), "/var/lib/k2app/apps/gpu-job/process/code");
        assert_eq!(process::state(&a), "/var/lib/k2appx/gpu-job");
        assert!(exec_argv_ok(&["ls".into(), "-la".into()]));
        assert!(!exec_argv_ok(&[]));
        assert!(!exec_argv_ok(&["".into()]));
        assert!(!exec_argv_ok(&["x\u{1b}]52;c;x\u{7}".into()]));
    }

    #[test]
    fn tty_frames_round_trip_and_cap() {
        let mut buf = tty::encode(tty::OUTPUT, b"hello");
        buf.extend(tty::encode(tty::EXIT, &7i32.to_be_bytes()));
        let mut r = &buf[..];
        assert_eq!(tty::read(&mut r).unwrap(), Some((tty::OUTPUT, b"hello".to_vec())));
        assert_eq!(tty::read(&mut r).unwrap(), Some((tty::EXIT, 7i32.to_be_bytes().to_vec())));
        assert_eq!(tty::read(&mut r).unwrap(), None);
        let mut big = vec![tty::INPUT];
        big.extend_from_slice(&((tty::MAX_FRAME as u32) + 1).to_be_bytes());
        assert!(tty::read(&mut &big[..]).is_err());
        assert!(tty::read(&mut &[tty::INPUT, 0, 0][..]).is_err(), "a short header is an error, not an end");
    }

    #[test]
    fn engine_markers_are_their_own() {
        let m = format_marker("0.46.1", 1, "x86_64");
        assert!(m.starts_with("K2_APP_ENGINE_BUILD v=0.46.1 proto=1 arch=x86_64;"));
        assert_eq!(find_markers(m.as_bytes()).unwrap().len(), 1);
        assert!(crate::version::find_markers(m.as_bytes()).unwrap().is_empty());
        assert!(crate::node_build::find_markers(m.as_bytes()).unwrap().is_empty());
        let helper = crate::version::format_marker("0.46.1", 6, "x86_64");
        assert!(find_markers(helper.as_bytes()).unwrap().is_empty());
        assert!(find_markers(format!("{}0.46.1 proto=1 arch=mips;", marker_prefix()).as_bytes()).is_err());
    }

    #[cfg(feature = "test-signer")]
    #[test]
    fn signed_engine_builds_verify_and_each_tamper_fails() {
        use crate::keys::TEST_KEY;
        use crate::testsign::{fake_elf, sign_with_test_key};
        let keys = [TEST_KEY];
        let elf = fake_elf("x86_64", &format_marker("0.46.1", 1, "x86_64"));
        let sig = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.1", 1));
        let v = verify_engine_build(&elf, &sig, &keys, "x86_64").unwrap();
        assert_eq!((v.version, v.protocol), (Semver(0, 46, 1), 1));
        assert!(verify_engine_build(&elf, &sig, crate::keys::RELEASE_KEYS, "x86_64").is_err());
        assert!(verify_engine_build(&elf, &sig, &keys, "aarch64").is_err());
        let mut bad = elf.clone();
        bad[70] ^= 1;
        assert!(verify_engine_build(&bad, &sig, &keys, "x86_64").is_err());
        // Domain separation: a helper or node comment over the same bytes.
        let s = sign_with_test_key(&elf, &crate::sig::trusted_comment("x86_64", "0.46.1", 1));
        assert_eq!(verify_engine_build(&elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
        let s = sign_with_test_key(&elf, &crate::node_build::trusted_comment("linux", "x86_64", "0.46.1", 1));
        assert_eq!(verify_engine_build(&elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
        let s = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.2", 1));
        assert_eq!(verify_engine_build(&elf, &s, &keys, "x86_64"), Err("build marker does not match the trusted comment"));
        // And the helper's verifier refuses an engine comment.
        let s = sign_with_test_key(&elf, &trusted_comment("x86_64", "0.46.1", 1));
        assert_eq!(crate::sig::verify_signed_build(&elf, &s, &keys, "x86_64"), Err("trusted comment names another file"));
    }

    #[test]
    fn canned_unit_and_tmpfiles() {
        let u = user_unit();
        assert!(u.contains("\nConditionUser=k2app\n"));
        assert!(u.contains("\nExecStart=/usr/local/libexec/k2-app-engine serve\n"));
        assert!(!u.contains("NoNewPrivileges"), "newuidmap needs its setuid bit");
        let t = tmpfiles_conf(&["k2", "k2std"]);
        assert!(t.contains("d /run/k2app 0750 k2app k2app -\n"));
        assert!(t.contains("a+ /run/k2app - - - - u:k2:x\n"));
        assert!(t.contains("a+ /run/k2app - - - - u:k2std:x\n"));
    }
}
