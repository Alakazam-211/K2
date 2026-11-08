//! GPU jobs, root side (0.46.11, prd-compute-gpu-jobs-v1 S1, GH#79).
//!
//! **The owner's switch is at the machine.** `sudo k2-node admin gpu on
//! [--device <pci>|all] [--server owner|<label>|<fp16>]...` writes a new
//! root-owned `/etc/k2-node/gpu.toml` ([`GPU_FILE`]). It is its own file,
//! not `admin.toml`, for the reason `jobs.toml` is: an older executor never
//! reads it, so a rollback means "no GPU jobs", never a broken
//! `admin.toml` (vs-live A2). No file, a broken file or another `version`
//! all mean off.
//!
//! **Root decides the device.** A job's start (`JobStart.gpu`) only says
//! "one GPU, vendor X". The executor:
//! 1. checks `gpu.toml` names this slot's server (the owner's server is
//!    always allowed once a device is on, Q8; another server by the
//!    fingerprint in its `servers.toml` line, read at every start, so a
//!    reused slot inherits nothing, A3);
//! 2. leases a free device of that vendor ([`LEASE_FILE`]): one job per
//!    GPU, and a lease frees only once the last unit that held it is gone
//!    or ended with an empty cgroup (a background compositor holds it, A11);
//! 3. adds that render node, and only it, to the unit ([`gpu_props`]), with
//!    a supplementary group only when the node isn't world read-write (and
//!    never group 0, the node's group or a tenant's), and `K2_GPU_*` env
//!    set by root ([`gpu_env`]).
//!
//! **How the device gets in** ([`DEVICE_MODE`], one line to switch once
//! the S1-0 probe on a real node says which works):
//! - [`DeviceMode::BindPaths`] (primary): the unit keeps `PrivateDevices=yes`;
//!   `BindPaths=` puts the one render node in its private `/dev`, and
//!   `DeviceAllow=` opens only its minor in the device cgroup.
//! - [`DeviceMode::DevicePolicy`] (fallback): `PrivateDevices=no`,
//!   `DevicePolicy=closed` with an explicit allow list (the pseudo devices,
//!   the TTYs, the leased render node), `SystemCallFilter=~@raw-io`, and
//!   every other DRM node and `/dev/kfd` made inaccessible.
//!
//! In both, the device cgroup is what enforces access; the private `/dev`
//! only hides names. v1 never gives a job a card (KMS) node, `/dev/kfd`
//! (ROCm compute: a later, separate switch) or an NVIDIA device (refused
//! until one passes on real hardware, Q6).
//!
//! **The self-test.** Before `Ready` says a device is usable, root runs a
//! throwaway unit with exactly those properties, as the owner's job user,
//! that must open the render node and must not open any other DRM node or
//! `/dev/kfd` ([`GPU_SELFTEST_SCRIPT`]). It runs in the background (the
//! node's next probe learns the result), and at `gpu on`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use k2_node_proto::admin::{code, AdminRefusal};
use k2_node_proto::gpu::{codes as gcodes, GpuAsk, GpuVendor};
use k2_node_proto::share::{Slot, OWNER_SLOT};
use serde::{Deserialize, Serialize};

use super::exec::Executor;
use super::jobs;
use crate::gpu::SysGpu;

/// Root-owned, 0644, in the config dir. Written only by `sudo k2-node
/// admin gpu on|off` (never by the node, never remotely).
pub const GPU_FILE: &str = "gpu.toml";
/// Root's leases, in the executor's state dir.
pub const LEASE_FILE: &str = "gpu-lease.json";
/// The `gpu.toml` version this build reads; any other is off.
pub const GPU_FILE_VERSION: u32 = 1;
/// The owner's server in `servers` (always allowed once a device is on).
pub const OWNER: &str = "owner";

/// How the leased device gets into the unit (module docs). **The S1-0
/// probe decides this line**: `BindPaths` when its "primary" check passes,
/// else `DevicePolicy`.
pub const DEVICE_MODE: DeviceMode = DeviceMode::BindPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceMode {
    /// `PrivateDevices=yes` + `BindPaths=` + `DeviceAllow=`.
    BindPaths,
    /// `PrivateDevices=no` + `DevicePolicy=closed` + an allow list +
    /// `InaccessiblePaths=` for every other DRM node.
    DevicePolicy,
}

/// What a desktop owner must know (Q3): printed by `gpu on`.
pub const DESKTOP_WARNING: &str = "Note: GPU jobs share this GPU with anything else using it, your desktop included. \
A GPU hang or reset caused by a job can end the desktop session (a hard GPU fault can freeze the machine). \
On an APU, a job's GPU buffers come from system RAM and aren't counted in its --mem.";

// ── gpu.toml ────────────────────────────────────────────────────────

/// `gpu.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuFile {
    pub version: u32,
    #[serde(default, rename = "device", skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<GpuDevice>,
}

/// One `[[device]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuDevice {
    /// PCI address (`0000:c4:00.0`); root finds its render node in sysfs.
    pub pci: String,
    /// `owner`, or a server's first 16 hex of its key fingerprint (its
    /// `servers.toml` line), resolved to a slot at every start.
    #[serde(default)]
    pub servers: Vec<String>,
}

fn valid_server(s: &str) -> bool {
    s == OWNER || (s.len() == 16 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
}

impl GpuFile {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != GPU_FILE_VERSION {
            return Err(format!("version {} (this k2-node reads {GPU_FILE_VERSION})", self.version));
        }
        let mut seen = std::collections::BTreeSet::new();
        for d in &self.devices {
            if !crate::gpu::valid_pci(&d.pci) {
                return Err(format!("device {:?} is not a PCI address (0000:c4:00.0)", d.pci));
            }
            if !seen.insert(d.pci.as_str()) {
                return Err(format!("device {} is listed twice", d.pci));
            }
            if let Some(s) = d.servers.iter().find(|s| !valid_server(s)) {
                return Err(format!("device {}: server {s:?} is `owner` or 16 lowercase hex of a server's key", d.pci));
            }
        }
        Ok(())
    }

    pub fn device(&self, pci: &str) -> Option<&GpuDevice> {
        self.devices.iter().find(|d| d.pci == pci)
    }
}

impl GpuDevice {
    /// May slot `slot`'s server (whose key starts with `fp16`) use it? The
    /// owner's server always may (Q8); another only when named.
    pub fn allows(&self, slot: Slot, fp16: Option<&str>) -> bool {
        slot == OWNER_SLOT || fp16.is_some_and(|f| self.servers.iter().any(|s| s == f))
    }
}

/// `Ok(None)` when missing (GPU jobs off). A broken file is an error, and
/// every caller treats it as off.
pub fn read_gpu_file(config_dir: &Path) -> Result<Option<GpuFile>, String> {
    let p = config_dir.join(GPU_FILE);
    let s = match jobs::read_root_trusted(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", p.display())),
    };
    let f: GpuFile = toml::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
    f.validate().map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Some(f))
}

/// Write `gpu.toml` (root, 0644); no device left removes the file.
pub fn write_gpu_file(config_dir: &Path, f: &GpuFile) -> Result<(), String> {
    let p = config_dir.join(GPU_FILE);
    if f.devices.is_empty() {
        return match std::fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove {}: {e}", p.display())),
        };
    }
    f.validate()?;
    let body = toml::to_string(f).map_err(|e| e.to_string())?;
    let text = format!(
        "# K2 compute node: GPU jobs (0.46.11). Written by sudo k2-node admin gpu on|off (never by the node, never remotely).\n\
         # Each [[device]] is one GPU; servers = who may run GPU jobs on it (\"owner\", or a server's key prefix).\n{body}"
    );
    crate::util::atomic_write(&p, text.as_bytes(), 0o644)
}

/// The first 16 hex of a server's key, from its `servers.toml` line.
pub fn fp16(fp_prefix: &str) -> Option<String> {
    let f = fp_prefix.get(..16)?.to_ascii_lowercase();
    valid_server(&f).then_some(f)
}

// ── the unit ────────────────────────────────────────────────────────

/// The leased device, as root decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuUnit {
    pub pci: String,
    pub vendor: String,
    /// `/dev/dri/renderD128`.
    pub render: PathBuf,
    /// A supplementary group for the unit (the node isn't world rw).
    pub group: Option<u32>,
    pub mode: DeviceMode,
    /// [`DeviceMode::DevicePolicy`]: every other DRM node and `/dev/kfd`.
    pub hide: Vec<PathBuf>,
}

/// The fallback's allow list beside the render node (what
/// `PrivateDevices=yes` gives a unit, spelled out).
pub const FALLBACK_ALLOW: &[&str] = &[
    "/dev/null rw",
    "/dev/zero rw",
    "/dev/full rw",
    "/dev/random rw",
    "/dev/urandom rw",
    "/dev/tty rw",
    "/dev/ptmx rw",
    "char-pts rw",
];

/// The unit properties for `g` (after the template's own).
pub fn gpu_props(g: &GpuUnit) -> Vec<String> {
    let r = g.render.display().to_string();
    let mut p = Vec::new();
    match g.mode {
        DeviceMode::BindPaths => {
            // The one node into the private /dev, and only its minor in
            // the device cgroup (PrivateDevices implies DevicePolicy=closed).
            p.push(format!("BindPaths={r}"));
            p.push(format!("DeviceAllow={r} rw"));
        }
        DeviceMode::DevicePolicy => {
            p.push("DevicePolicy=closed".into());
            for a in FALLBACK_ALLOW {
                p.push(format!("DeviceAllow={a}"));
            }
            p.push(format!("DeviceAllow={r} rw"));
            p.push("SystemCallFilter=~@raw-io".into());
            if !g.hide.is_empty() {
                let h: Vec<String> = g.hide.iter().map(|x| format!("-{}", x.display())).collect();
                p.push(format!("InaccessiblePaths={}", h.join(" ")));
            }
        }
    }
    if let Some(gid) = g.group {
        p.push(format!("SupplementaryGroups={gid}"));
    }
    p
}

/// `K2_GPU_*` for the job (root's lease, never the node's word).
pub fn gpu_env(g: &GpuUnit) -> Vec<(String, String)> {
    use k2_node_proto::gpu::{ENV_PCI, ENV_RENDER, ENV_VENDOR};
    vec![(ENV_VENDOR.into(), g.vendor.clone()), (ENV_PCI.into(), g.pci.clone()), (ENV_RENDER.into(), g.render.display().to_string())]
}

/// What `PrivateDevices=` the template says under `mode`.
pub fn private_devices(mode: Option<DeviceMode>) -> &'static str {
    match mode {
        Some(DeviceMode::DevicePolicy) => "PrivateDevices=no",
        _ => "PrivateDevices=yes",
    }
}

/// May the job's unit open `render` (a char device)? `Ok(None)`: it is
/// world read-write (Arch's default); `Ok(Some(gid))`: the unit needs that
/// group (Ubuntu's `render`, 0660). Never group 0, the node's group or a
/// tenant's group (`k2t-*`), and never a node only its owner may use.
pub fn device_access(mode: u32, gid: u32, node_gid: u32, tenant_gids: &[u32]) -> Result<Option<u32>, String> {
    if mode & 0o006 == 0o006 {
        return Ok(None);
    }
    if mode & 0o060 != 0o060 {
        return Err(format!("the render node's mode {:o} gives its group no read-write; a job can't open it", mode & 0o777));
    }
    if gid == 0 || gid == node_gid || tenant_gids.contains(&gid) {
        return Err(format!("the render node's group ({gid}) is root's, the node's or a tenant's; K2 never gives a job that group"));
    }
    Ok(Some(gid))
}

/// `(mode, gid)` of a char device, or why not one.
fn char_device(p: &Path) -> Result<(u32, u32), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let m = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !m.file_type().is_char_device() {
        return Err(format!("{} is not a character device", p.display()));
    }
    Ok((m.mode(), m.gid()))
}

// ── leases ──────────────────────────────────────────────────────────

/// One device's last holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub unit: String,
    pub slot: Slot,
    pub at: i64,
}

/// [`LEASE_FILE`]: by PCI address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseFile {
    #[serde(default)]
    pub leases: BTreeMap<String, Lease>,
}

/// Does a unit still hold its device? (Tests fake it.)
pub trait LeaseProbe {
    /// The unit is loaded and its command runs, or it ended with
    /// processes left in its cgroup. Unknown = holds (fail closed).
    fn holds(&self, unit: &str) -> bool;
}

/// The machine, as root sees it.
pub struct SystemLeaseProbe;

impl LeaseProbe for SystemLeaseProbe {
    fn holds(&self, unit: &str) -> bool {
        let Ok(s) = jobs::unit_status(unit) else { return true };
        if !s.loaded {
            return false;
        }
        if !s.ended() {
            return true;
        }
        let Some(cg) = s.control_group else { return false };
        match std::fs::read_to_string(Path::new("/sys/fs/cgroup").join(cg.trim_start_matches('/')).join("cgroup.procs")) {
            Ok(t) => !t.trim().is_empty(),
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }
}

pub fn read_leases(path: &Path) -> Result<LeaseFile, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LeaseFile::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

pub fn write_leases(path: &Path, f: &LeaseFile) -> Result<(), String> {
    let b = serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?;
    crate::util::atomic_write(path, &b, 0o600)
}

/// Which device a start of slot `slot` (key prefix `fp16`) gets for `ask`:
/// the first device `file` allows it, present in `found`, of the asked
/// vendor, not NVIDIA, that `passed` its self-test and no live unit holds.
/// `Err((code, why))`: [`gcodes::GPU_UNAVAILABLE`] when none could ever
/// serve it here, [`gcodes::GPU_BUSY`] when each is held.
#[allow(clippy::too_many_arguments)]
pub fn pick<'a>(
    file: &GpuFile,
    found: &'a [SysGpu],
    passed: &dyn Fn(&str) -> bool,
    slot: Slot,
    fp16: Option<&str>,
    ask: &GpuAsk,
    leases: &LeaseFile,
    probe: &dyn LeaseProbe,
) -> Result<&'a SysGpu, (&'static str, String)> {
    let mut usable: Vec<&SysGpu> = Vec::new();
    for d in file.devices.iter().filter(|d| d.allows(slot, fp16)) {
        let Some(g) = found.iter().find(|g| g.pci == d.pci) else { continue };
        if g.vendor == "nvidia" || !ask.vendor.matches(g.vendor) || !passed(&g.pci) {
            continue;
        }
        usable.push(g);
    }
    if usable.is_empty() {
        let v = if ask.vendor == GpuVendor::Any { String::new() } else { format!(" {}", ask.vendor.as_str()) };
        return Err((gcodes::GPU_UNAVAILABLE, format!("no{v} GPU here runs jobs for this server (the machine's owner: sudo k2-node admin gpu on)")));
    }
    for g in &usable {
        match leases.leases.get(&g.pci) {
            Some(l) if probe.holds(&l.unit) => continue,
            _ => return Ok(*g),
        }
    }
    Err((gcodes::GPU_BUSY, format!("every GPU this server may use here is held by another job ({})", usable.iter().map(|g| g.pci.as_str()).collect::<Vec<_>>().join(", "))))
}

// ── Ready ───────────────────────────────────────────────────────────

/// One usable device, as `Ready` tells the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyGpu {
    pub pci: String,
    pub vendor: String,
    pub render: String,
    /// `owner` and the key prefixes of the other servers that may use it.
    pub servers: Vec<String>,
}

impl ReadyGpu {
    /// May the server in `slot` (key prefix `fp16`) use it?
    pub fn allows(&self, slot: Slot, fp16: Option<&str>) -> bool {
        slot == OWNER_SLOT || fp16.is_some_and(|f| self.servers.iter().any(|s| s == f))
    }
}

/// The devices `file` turns on that are here, not NVIDIA, and `passed`.
pub fn ready_devices(file: &GpuFile, found: &[SysGpu], passed: &dyn Fn(&str) -> bool) -> Vec<ReadyGpu> {
    file.devices
        .iter()
        .filter_map(|d| {
            let g = found.iter().find(|g| g.pci == d.pci)?;
            (g.vendor != "nvidia" && passed(&g.pci)).then(|| {
                let mut servers = vec![OWNER.to_string()];
                servers.extend(d.servers.iter().filter(|s| *s != OWNER).cloned());
                ReadyGpu { pci: g.pci.clone(), vendor: g.vendor.to_string(), render: g.render.clone(), servers }
            })
        })
        .collect()
}

// ── the self-test ───────────────────────────────────────────────────

/// Inside a throwaway unit with the GPU properties: the leased node opens,
/// no other DRM node and no `/dev/kfd` does. Missing `head` fails it.
pub const GPU_SELFTEST_SCRIPT: &str = "R=\"$K2_GPU_RENDER\"; test -c \"$R\" && head -c0 <\"$R\" || { echo \"cannot open $R\"; exit 1; }; \
for d in /dev/dri/* /dev/kfd; do [ \"$d\" = \"$R\" ] && continue; [ -c \"$d\" ] || continue; \
if head -c0 <\"$d\" 2>/dev/null; then echo \"opened $d\"; exit 1; fi; done; echo k2-gpu-ok";

/// Self-test results by PCI address (passes are kept; a failure is tried
/// again after [`SELFTEST_RETRY`]).
#[derive(Debug, Clone, Default)]
struct SelfTests {
    passed: std::collections::BTreeSet<String>,
    failed: BTreeMap<String, (Instant, String)>,
    running: bool,
}

const SELFTEST_RETRY: Duration = Duration::from_secs(600);

fn selftests() -> &'static std::sync::Mutex<SelfTests> {
    static S: std::sync::OnceLock<std::sync::Mutex<SelfTests>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// One start's lease at a time (pick + record).
fn lease_lock() -> &'static std::sync::Mutex<()> {
    static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

impl Executor {
    fn lease_path(&self) -> PathBuf {
        self.paths.state_dir.join(LEASE_FILE)
    }

    /// The node's group and every tenant group (`k2t-*`) present.
    fn wall_gids(&self) -> Result<(u32, Vec<u32>), AdminRefusal> {
        let (_, node_gid) = super::server::user_ids(&self.node_user()).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        let tenants = (1..k2_node_proto::share::MAX_SLOTS).filter_map(|s| jobs::group_id(&jobs::tenant_group_name(s))).collect();
        Ok((node_gid, tenants))
    }

    /// The unit's view of device `g` under [`DEVICE_MODE`].
    pub(crate) fn gpu_unit(&self, g: &SysGpu) -> Result<GpuUnit, AdminRefusal> {
        let render = g.render_path();
        let (mode, gid) = char_device(&render).map_err(|e| refuse(gcodes::GPU_UNAVAILABLE, e))?;
        let (node_gid, tenant_gids) = self.wall_gids()?;
        let group = device_access(mode, gid, node_gid, &tenant_gids).map_err(|e| refuse(gcodes::GPU_UNAVAILABLE, e))?;
        let hide = if DEVICE_MODE == DeviceMode::DevicePolicy {
            let mut h: Vec<PathBuf> =
                crate::gpu::drm_nodes(Path::new(crate::gpu::SYS)).into_iter().filter(|n| *n != g.render).map(|n| Path::new("/dev/dri").join(n)).collect();
            h.push(PathBuf::from("/dev/dri/by-path"));
            h.push(PathBuf::from("/dev/kfd"));
            h
        } else {
            Vec::new()
        };
        Ok(GpuUnit { pci: g.pci.clone(), vendor: g.vendor.to_string(), render, group, mode: DEVICE_MODE, hide })
    }

    /// Has `pci` passed its self-test in this executor?
    fn selftest_passed(&self, pci: &str) -> bool {
        selftests().lock().unwrap_or_else(|e| e.into_inner()).passed.contains(pci)
    }

    /// What `Ready` says about GPUs: the usable devices, or none (no file,
    /// a broken one, no job wall, not Linux). Devices not tested yet are
    /// tested in the background; the node's next probe learns the result.
    pub fn gpu_ready(&self) -> Vec<ReadyGpu> {
        if !cfg!(target_os = "linux") || self.job_user().is_err() {
            return Vec::new();
        }
        let Ok(Some(file)) = read_gpu_file(&self.paths.config_dir) else { return Vec::new() };
        let found = crate::gpu::discover(Path::new(crate::gpu::SYS));
        self.kick_selftests(&file, &found);
        ready_devices(&file, &found, &|p| self.selftest_passed(p))
    }

    /// Start the self-test of every device on and not tested yet (one
    /// background run at a time).
    fn kick_selftests(&self, file: &GpuFile, found: &[SysGpu]) {
        let todo: Vec<SysGpu> = {
            let mut st = selftests().lock().unwrap_or_else(|e| e.into_inner());
            if st.running {
                return;
            }
            let todo: Vec<SysGpu> = file
                .devices
                .iter()
                .filter_map(|d| found.iter().find(|g| g.pci == d.pci))
                .filter(|g| g.vendor != "nvidia" && !st.passed.contains(&g.pci))
                .filter(|g| st.failed.get(&g.pci).is_none_or(|(at, _)| at.elapsed() >= SELFTEST_RETRY))
                .cloned()
                .collect();
            if todo.is_empty() {
                return;
            }
            st.running = true;
            todo
        };
        let paths = self.paths.clone();
        let platform = self.platform.clone();
        std::thread::spawn(move || {
            let exec = Executor::new(paths, platform);
            for g in &todo {
                let r = exec.gpu_selftest(g);
                if let Err(e) = &r {
                    crate::nlog!("GPU self-test {}: {e}", g.pci);
                }
            }
            selftests().lock().unwrap_or_else(|e| e.into_inner()).running = false;
        });
    }

    /// The self-test for `g` (root): a throwaway unit with the GPU
    /// properties, as the owner's job user. Recorded either way.
    pub fn gpu_selftest(&self, g: &SysGpu) -> Result<(), String> {
        let r = self.gpu_selftest_run(g);
        let mut st = selftests().lock().unwrap_or_else(|e| e.into_inner());
        match &r {
            Ok(()) => {
                st.passed.insert(g.pci.clone());
                st.failed.remove(&g.pci);
            }
            Err(e) => {
                st.passed.remove(&g.pci);
                st.failed.insert(g.pci.clone(), (Instant::now(), e.clone()));
            }
        }
        r
    }

    fn gpu_selftest_run(&self, g: &SysGpu) -> Result<(), String> {
        if g.vendor == "nvidia" {
            return Err("NVIDIA GPUs aren't supported for jobs yet".into());
        }
        let ids = self.job_ids(OWNER_SLOT).map_err(|e| e.message)?;
        let unit_gpu = self.gpu_unit(g).map_err(|e| e.message)?;
        let prog = ["/usr/bin/systemd-run", "/bin/systemd-run"].iter().map(PathBuf::from).find(|p| p.is_file()).ok_or("systemd-run not found")?;
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unit = format!("{}gpu-selftest-{}-{}.service", jobs::UNIT_PREFIX, std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let mut a: Vec<String> = vec![
            format!("--unit={unit}"),
            "--quiet".into(),
            "--wait".into(),
            "--collect".into(),
            "--pipe".into(),
            format!("--uid={}", ids.uid),
            format!("--gid={}", ids.gid),
        ];
        for (k, v) in gpu_env(&unit_gpu) {
            a.push(format!("--setenv={k}={v}"));
        }
        let mut props: Vec<String> = vec![
            "NoNewPrivileges=yes".into(),
            "ProtectSystem=strict".into(),
            "PrivateTmp=yes".into(),
            private_devices(Some(unit_gpu.mode)).into(),
            "ProtectKernelTunables=yes".into(),
            "RuntimeMaxSec=20".into(),
        ];
        props.extend(gpu_props(&unit_gpu));
        for p in props {
            a.push("-p".into());
            a.push(p);
        }
        a.extend(["--".into(), "/bin/sh".into(), "-c".into(), GPU_SELFTEST_SCRIPT.into()]);
        let mut child = std::process::Command::new(prog)
            .args(&a)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("systemd-run: {e}"))?;
        let t0 = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if t0.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                let _ = jobs::systemctl(&["stop", &unit]);
                return Err("the GPU self-test unit didn't finish within 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if status.success() && stdout.trim() == "k2-gpu-ok" {
            return Ok(());
        }
        Err(format!("{status}: {} {}", stdout.trim(), String::from_utf8_lossy(&out.stderr).trim()))
    }

    /// A start's GPU (`JobStart.gpu`): the device root picks for slot
    /// `slot`, leased to `unit` before the unit exists. `release_gpu_lease`
    /// undoes it when the start fails.
    pub(crate) fn gpu_for_start(&self, slot: Slot, ask: &GpuAsk, unit: &str) -> Result<GpuUnit, AdminRefusal> {
        ask.check().map_err(|e| refuse(code::BAD_ARGS, e))?;
        if !cfg!(target_os = "linux") {
            return Err(refuse(gcodes::GPU_UNAVAILABLE, "GPU jobs are Linux only"));
        }
        let file = read_gpu_file(&self.paths.config_dir)
            .map_err(|e| refuse(gcodes::GPU_UNAVAILABLE, e))?
            .ok_or_else(|| refuse(gcodes::GPU_UNAVAILABLE, "GPU jobs are off on this machine (its owner: sudo k2-node admin gpu on)"))?;
        // Root reads the slot's server from servers.toml at every start (a
        // reused slot is another fingerprint, so it inherits nothing).
        let fp = if slot == OWNER_SLOT { None } else { fp16(&jobs::tenant_entry(&self.paths.config_dir, slot)?.fp_prefix) };
        let found = crate::gpu::discover(Path::new(crate::gpu::SYS));
        let _g = lease_lock().lock().unwrap_or_else(|e| e.into_inner());
        let path = self.lease_path();
        // An unreadable lease file holds every device (fail closed).
        let mut leases = read_leases(&path).map_err(|e| refuse(gcodes::GPU_BUSY, e))?;
        let g = pick(&file, &found, &|p| self.selftest_passed(p), slot, fp.as_deref(), ask, &leases, &SystemLeaseProbe).map_err(|(c, m)| refuse(c, m))?;
        let unit_gpu = self.gpu_unit(g)?;
        leases.leases.insert(g.pci.clone(), Lease { unit: unit.to_string(), slot, at: self.now() });
        write_leases(&path, &leases).map_err(|e| refuse(code::FAILED, format!("GPU lease: {e}")))?;
        Ok(unit_gpu)
    }

    /// A start that failed gives its lease back (only its own).
    pub(crate) fn release_gpu_lease(&self, pci: &str, unit: &str) {
        let _g = lease_lock().lock().unwrap_or_else(|e| e.into_inner());
        let path = self.lease_path();
        if let Ok(mut f) = read_leases(&path) {
            if f.leases.get(pci).is_some_and(|l| l.unit == unit) {
                f.leases.remove(pci);
                let _ = write_leases(&path, &f);
            }
        }
    }
}

// ── `sudo k2-node admin gpu on|off|status` ──────────────────────────

/// `--server` values → what `gpu.toml` stores: `owner`, or a server's key
/// prefix from `servers.toml` (by label, `slot:<n>` or the prefix itself).
pub fn resolve_servers(config_dir: &Path, asked: &[String]) -> Result<Vec<String>, String> {
    let file = jobs::read_servers(config_dir)?;
    let mut out = vec![OWNER.to_string()];
    for s in asked {
        let v = if s == OWNER {
            OWNER.to_string()
        } else {
            let f = file.as_ref().ok_or_else(|| format!("--server {s}: this node serves only its owner (no servers.toml); use --server owner"))?;
            let e = f
                .find(s)
                .or_else(|| f.servers.iter().find(|e| fp16(&e.fp_prefix).as_deref() == Some(s.as_str())))
                .ok_or_else(|| format!("--server {s}: no such server here (k2-node servers list shows labels)"))?;
            if e.slot == OWNER_SLOT {
                OWNER.to_string()
            } else {
                fp16(&e.fp_prefix).ok_or_else(|| format!("--server {s}: its key prefix in servers.toml isn't hex"))?
            }
        };
        if !out.contains(&v) {
            out.push(v);
        }
    }
    Ok(out)
}

fn audit_local(state_dir: &Path, event: &str, detail: serde_json::Value) {
    let _ = super::store::Audit::new(&state_dir.join("audit.log"))
        .append(super::store::AuditEntry { event: event.into(), detail, ..Default::default() }, crate::util::now());
}

/// `gpu on`: which devices (`None` = every supported GPU found) for which
/// servers. Writes `gpu.toml` (keeping other devices), then runs the
/// self-test of each device turned on when `exec` is given (root).
pub fn cli_on(config_dir: &Path, state_dir: &Path, sys: &Path, device: Option<&str>, servers: &[String], exec: Option<&Executor>) -> Result<String, String> {
    let found = crate::gpu::discover(sys);
    if found.is_empty() {
        return Err("no GPU with a render node on this machine (/sys/class/drm/renderD*)".into());
    }
    let mut notes = Vec::new();
    let picked: Vec<&SysGpu> = match device {
        None | Some("all") => found.iter().collect(),
        Some(p) => vec![found.iter().find(|g| g.pci == p).ok_or_else(|| {
            format!("--device {p}: no such GPU here (found: {})", found.iter().map(|g| format!("{} {}", g.pci, g.vendor)).collect::<Vec<_>>().join(", "))
        })?],
    };
    let mut on: Vec<&SysGpu> = Vec::new();
    for g in picked {
        if g.vendor == "nvidia" {
            if device.is_some_and(|d| d != "all") {
                return Err(format!("{}: NVIDIA GPUs aren't supported for jobs yet (not tested on real hardware)", g.pci));
            }
            notes.push(format!("{} (nvidia): skipped, NVIDIA GPUs aren't supported for jobs yet", g.pci));
            continue;
        }
        on.push(g);
    }
    if on.is_empty() {
        return Err(format!("no supported GPU to turn on. {}", notes.join("; ")));
    }
    let srv = resolve_servers(config_dir, servers)?;
    let mut f = read_gpu_file(config_dir).ok().flatten().unwrap_or(GpuFile { version: GPU_FILE_VERSION, devices: Vec::new() });
    for g in &on {
        match f.devices.iter_mut().find(|d| d.pci == g.pci) {
            Some(d) => {
                for s in &srv {
                    if !d.servers.contains(s) {
                        d.servers.push(s.clone());
                    }
                }
            }
            None => f.devices.push(GpuDevice { pci: g.pci.clone(), servers: srv.clone() }),
        }
    }
    write_gpu_file(config_dir, &f)?;
    audit_local(state_dir, "gpu_on", serde_json::json!({"devices": on.iter().map(|g| &g.pci).collect::<Vec<_>>(), "servers": srv, "by": "local"}));
    let mut out = Vec::new();
    for g in &on {
        let servers = &f.device(&g.pci).map(|d| d.servers.clone()).unwrap_or_default();
        out.push(format!("GPU jobs on: {} {} ({}) for {}", g.pci, g.vendor, g.render, servers.join(", ")));
        if let Some(x) = exec {
            match x.gpu_selftest(g) {
                Ok(()) => out.push("  self-test: ok (a job unit opens only this render node)".into()),
                Err(e) => out.push(format!("  self-test FAILED: {e}\n  GPU jobs stay unavailable on it until this passes (k2-node admin gpu status)")),
            }
        }
    }
    out.extend(notes);
    out.push(format!(
        "Each server named also needs its own owner's grant: k2 compute grant <this node> --workspace <ws> --gpu. \
         Its K2 sees GPU jobs at its next probe (within a minute), or restart k2-node now.\n\
         Not given to jobs: display (card) nodes, ROCm compute (/dev/kfd). One job per GPU at a time.\n{DESKTOP_WARNING}"
    ));
    Ok(out.join("\n"))
}

/// `gpu off [--device <pci>] [--server <s>]`: a whole device (or all), or
/// one server from a device. The next start sees it (no restart).
pub fn cli_off(config_dir: &Path, state_dir: &Path, device: Option<&str>, server: Option<&str>) -> Result<String, String> {
    let Some(mut f) = read_gpu_file(config_dir)? else { return Ok("GPU jobs are already off (no gpu.toml)".into()) };
    let msg = match (device, server) {
        (_, Some(s)) => {
            let v = resolve_servers(config_dir, &[s.to_string()])?.pop().unwrap_or_default();
            if v == OWNER {
                return Err("the owner's server always may use a GPU that is on; turn the device off instead (gpu off --device <pci>)".into());
            }
            for d in f.devices.iter_mut().filter(|d| device.is_none_or(|p| p == "all" || d.pci == p)) {
                d.servers.retain(|x| *x != v);
            }
            format!("server {s} may no longer run GPU jobs here")
        }
        (None | Some("all"), None) => {
            f.devices.clear();
            "GPU jobs off (gpu.toml removed)".to_string()
        }
        (Some(p), None) => {
            let before = f.devices.len();
            f.devices.retain(|d| d.pci != p);
            if f.devices.len() == before {
                return Err(format!("--device {p} isn't on (k2-node admin gpu status)"));
            }
            format!("GPU jobs off on {p}")
        }
    };
    write_gpu_file(config_dir, &f)?;
    audit_local(state_dir, "gpu_off", serde_json::json!({"device": device, "server": server, "by": "local"}));
    Ok(format!("{msg}. A GPU job already running keeps its device until it ends; new ones are refused (gpu_unavailable)."))
}

/// `gpu status`: every GPU, whether jobs may use it and by whom, its lease.
pub fn cli_status(config_dir: &Path, sys: &Path, state_dir: &Path, json: bool) -> Result<String, String> {
    let found = crate::gpu::discover(sys);
    let file = read_gpu_file(config_dir);
    let leases = read_leases(&state_dir.join(LEASE_FILE)).unwrap_or_default();
    let rows: Vec<serde_json::Value> = found
        .iter()
        .map(|g| {
            let d = file.as_ref().ok().and_then(|f| f.as_ref()).and_then(|f| f.device(&g.pci));
            serde_json::json!({
                "pci": g.pci, "vendor": g.vendor, "render": g.render, "driver": g.driver,
                "on": d.is_some(), "servers": d.map(|d| d.servers.clone()).unwrap_or_default(),
                "supported": g.vendor != "nvidia",
                "lease": leases.leases.get(&g.pci).map(|l| serde_json::json!({"unit": l.unit, "slot": l.slot, "at": l.at})),
            })
        })
        .collect();
    let err = file.as_ref().err().cloned();
    if json {
        return Ok(serde_json::json!({"gpus": rows, "file_error": err, "device_mode": DEVICE_MODE}).to_string());
    }
    let mut out = Vec::new();
    if let Some(e) = err {
        out.push(format!("gpu.toml is unreadable, so GPU jobs are off: {e}"));
    }
    if rows.is_empty() {
        out.push("no GPU with a render node on this machine".into());
    }
    for r in &rows {
        let on = if r["on"] == true { format!("on for {}", r["servers"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()) } else { "off".into() };
        let lease = r["lease"]["unit"].as_str().map(|u| format!(", last lease {u}")).unwrap_or_default();
        out.push(format!("{} {} {} ({}): {on}{}{lease}", r["pci"].as_str().unwrap_or(""), r["vendor"].as_str().unwrap_or(""), r["render"].as_str().unwrap_or(""), r["driver"].as_str().unwrap_or(""), if r["supported"] == false { " (not supported for jobs yet)" } else { "" }));
    }
    out.push(format!("device mode: {:?} (the executor's self-test runs at its first GPU check; see the node log)", DEVICE_MODE));
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amd(pci: &str, render: &str) -> SysGpu {
        SysGpu {
            pci: pci.into(),
            vendor: "amd",
            vendor_id: "1002".into(),
            device_id: "1586".into(),
            driver: "amdgpu".into(),
            render: render.into(),
            cards: vec!["card1".into()],
            vram_bytes: None,
            vis_vram_bytes: None,
            gtt_bytes: None,
        }
    }

    struct Live(Vec<&'static str>);
    impl LeaseProbe for Live {
        fn holds(&self, unit: &str) -> bool {
            self.0.contains(&unit)
        }
    }

    const FP_A: &str = "aaaaaaaaaaaaaaaa";
    const FP_B: &str = "bbbbbbbbbbbbbbbb";

    fn file(servers: &[&str]) -> GpuFile {
        GpuFile { version: 1, devices: vec![GpuDevice { pci: "0000:c4:00.0".into(), servers: servers.iter().map(|s| s.to_string()).collect() }] }
    }

    #[test]
    fn gpu_toml_missing_or_broken_is_off() {
        let d = crate::util::temp_dir("gpu-toml");
        assert_eq!(read_gpu_file(&d).unwrap(), None, "no file: off");
        std::fs::write(d.join(GPU_FILE), "version = 1\n[[device]]\npci = \"0000:c4:00.0\"\nservers = [\"owner\"]\n").unwrap();
        assert_eq!(read_gpu_file(&d).unwrap(), Some(file(&["owner"])));
        for broken in [
            "version = 2\n",
            "version = 1\n[[device]]\npci = \"../../etc\"\n",
            "version = 1\n[[device]]\npci = \"0000:c4:00.0\"\nservers = [\"root\"]\n",
            "version = 1\n[[device]]\npci = \"0000:c4:00.0\"\n[[device]]\npci = \"0000:c4:00.0\"\n",
            "not toml",
        ] {
            std::fs::write(d.join(GPU_FILE), broken).unwrap();
            assert!(read_gpu_file(&d).is_err(), "{broken:?} must be refused (off)");
        }
    }

    #[test]
    fn unknown_keys_refuse() {
        let d = crate::util::temp_dir("gpu-toml-keys");
        std::fs::write(d.join(GPU_FILE), "version = 1\n[[device]]\npci = \"0000:c4:00.0\"\ncompute = true\n").unwrap();
        assert!(read_gpu_file(&d).is_err(), "kfd (`compute`) is a later switch: an unknown key is off, never a guess");
        std::fs::write(d.join(GPU_FILE), "version = 1\nshare = true\n").unwrap();
        assert!(read_gpu_file(&d).is_err());
    }

    #[test]
    fn write_then_off_removes_the_file() {
        let d = crate::util::temp_dir("gpu-toml-write");
        write_gpu_file(&d, &file(&["owner", FP_A])).unwrap();
        let text = std::fs::read_to_string(d.join(GPU_FILE)).unwrap();
        assert!(text.contains("pci = \"0000:c4:00.0\"") && text.contains(FP_A), "{text}");
        assert_eq!(read_gpu_file(&d).unwrap(), Some(file(&["owner", FP_A])));
        assert!(cli_off(&d, &d, None, None).unwrap().contains("GPU jobs off"));
        assert!(!d.join(GPU_FILE).exists());
    }

    #[test]
    fn only_named_servers_may_use_a_device_and_the_owner_always() {
        let f = file(&["owner", FP_A]);
        let dev = &f.devices[0];
        assert!(dev.allows(OWNER_SLOT, None));
        assert!(dev.allows(1, Some(FP_A)));
        assert!(!dev.allows(1, Some(FP_B)), "another server's key: no");
        assert!(!dev.allows(1, None), "a slot whose server can't be read: no");
    }

    /// V64/A3: a slot reused by another server (a new key) inherits nothing.
    #[test]
    fn a_reused_slot_inherits_no_gpu() {
        let f = file(&["owner", FP_A]);
        let found = vec![amd("0000:c4:00.0", "renderD128")];
        let leases = LeaseFile::default();
        let ask = GpuAsk::one(GpuVendor::Any);
        assert!(pick(&f, &found, &|_| true, 1, Some(FP_A), &ask, &leases, &Live(vec![])).is_ok());
        let e = pick(&f, &found, &|_| true, 1, Some(FP_B), &ask, &leases, &Live(vec![])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_UNAVAILABLE);
    }

    #[test]
    fn start_refuses_gpu_for_a_server_not_in_gpu_toml() {
        let f = file(&["owner"]);
        let found = vec![amd("0000:c4:00.0", "renderD128")];
        let e = pick(&f, &found, &|_| true, 2, Some(FP_B), &GpuAsk::one(GpuVendor::Any), &LeaseFile::default(), &Live(vec![])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_UNAVAILABLE);
        // Nor a device that isn't here, of another vendor, NVIDIA, or one
        // whose self-test hasn't passed.
        let e = pick(&f, &[], &|_| true, OWNER_SLOT, None, &GpuAsk::one(GpuVendor::Any), &LeaseFile::default(), &Live(vec![])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_UNAVAILABLE);
        let e = pick(&f, &found, &|_| true, OWNER_SLOT, None, &GpuAsk::one(GpuVendor::Intel), &LeaseFile::default(), &Live(vec![])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_UNAVAILABLE);
        let mut nv = amd("0000:c4:00.0", "renderD128");
        nv.vendor = "nvidia";
        let e = pick(&f, &[nv], &|_| true, OWNER_SLOT, None, &GpuAsk::one(GpuVendor::Any), &LeaseFile::default(), &Live(vec![])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_UNAVAILABLE, "NVIDIA is refused until tested on real hardware");
        let e = pick(&f, &found, &|_| false, OWNER_SLOT, None, &GpuAsk::one(GpuVendor::Any), &LeaseFile::default(), &Live(vec![])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_UNAVAILABLE, "no self-test pass: no GPU");
    }

    /// A11: a lease frees only once its unit is gone or ended with an empty
    /// cgroup (the probe's word), and two devices serve two jobs.
    #[test]
    fn lease_is_exclusive_until_the_unit_and_its_cgroup_are_gone() {
        let f = GpuFile {
            version: 1,
            devices: vec![
                GpuDevice { pci: "0000:c4:00.0".into(), servers: vec!["owner".into()] },
                GpuDevice { pci: "0000:c5:00.0".into(), servers: vec!["owner".into()] },
            ],
        };
        let found = vec![amd("0000:c4:00.0", "renderD128"), amd("0000:c5:00.0", "renderD129")];
        let ask = GpuAsk::one(GpuVendor::Amd);
        let mut leases = LeaseFile::default();
        leases.leases.insert("0000:c4:00.0".into(), Lease { unit: "k2job-a.service".into(), slot: 0, at: 1 });
        let g = pick(&f, &found, &|_| true, OWNER_SLOT, None, &ask, &leases, &Live(vec!["k2job-a.service"])).unwrap();
        assert_eq!(g.pci, "0000:c5:00.0", "the held device is skipped");
        leases.leases.insert("0000:c5:00.0".into(), Lease { unit: "k2job-b.service".into(), slot: 0, at: 2 });
        let e = pick(&f, &found, &|_| true, OWNER_SLOT, None, &ask, &leases, &Live(vec!["k2job-a.service", "k2job-b.service"])).unwrap_err();
        assert_eq!(e.0, gcodes::GPU_BUSY);
        // Unit a ended and its cgroup emptied: its device is free again.
        let g = pick(&f, &found, &|_| true, OWNER_SLOT, None, &ask, &leases, &Live(vec!["k2job-b.service"])).unwrap();
        assert_eq!(g.pci, "0000:c4:00.0");
    }

    #[test]
    fn ready_says_gpu_only_when_on_and_self_test_passed() {
        let found = vec![amd("0000:c4:00.0", "renderD128")];
        let f = file(&["owner", FP_A]);
        assert!(ready_devices(&f, &found, &|_| false).is_empty(), "not tested (or failed): nothing");
        let r = ready_devices(&f, &found, &|_| true);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].render.as_str(), r[0].servers.clone()), ("renderD128", vec!["owner".to_string(), FP_A.to_string()]));
        assert!(r[0].allows(1, Some(FP_A)) && !r[0].allows(2, Some(FP_B)));
        assert!(ready_devices(&GpuFile { version: 1, devices: vec![] }, &found, &|_| true).is_empty(), "off: nothing");
        assert!(ready_devices(&f, &[], &|_| true).is_empty(), "a device that isn't here: nothing");
    }

    #[test]
    fn supplementary_group_never_root_or_node_group() {
        // Arch: 0666, no group needed.
        assert_eq!(device_access(0o20666, 989, 900, &[61001]), Ok(None));
        // Ubuntu: 0660 render.
        assert_eq!(device_access(0o20660, 989, 900, &[61001]), Ok(Some(989)));
        assert!(device_access(0o20660, 0, 900, &[]).is_err(), "never group 0");
        assert!(device_access(0o20660, 900, 900, &[]).is_err(), "never the node's group");
        assert!(device_access(0o20660, 61001, 900, &[61001]).is_err(), "never a tenant's group");
        assert!(device_access(0o20600, 989, 900, &[]).is_err(), "a node only its owner may open");
    }

    /// The unit's GPU properties: the primary binds and allows exactly the
    /// leased render node; the fallback spells out the allow list and hides
    /// every other DRM node and /dev/kfd. Neither ever names a card node or
    /// /dev/kfd as allowed.
    #[test]
    fn gpu_props_allow_only_the_leased_render_node() {
        let mut g = GpuUnit {
            pci: "0000:c4:00.0".into(),
            vendor: "amd".into(),
            render: PathBuf::from("/dev/dri/renderD128"),
            group: None,
            mode: DeviceMode::BindPaths,
            hide: vec![],
        };
        assert_eq!(gpu_props(&g), vec!["BindPaths=/dev/dri/renderD128".to_string(), "DeviceAllow=/dev/dri/renderD128 rw".to_string()]);
        g.group = Some(989);
        assert_eq!(gpu_props(&g).last().unwrap(), "SupplementaryGroups=989");
        g.group = None;
        g.mode = DeviceMode::DevicePolicy;
        g.hide = vec![PathBuf::from("/dev/dri/card1"), PathBuf::from("/dev/dri/renderD129"), PathBuf::from("/dev/kfd")];
        let p = gpu_props(&g);
        assert_eq!(p[0], "DevicePolicy=closed");
        assert!(p.contains(&"DeviceAllow=/dev/dri/renderD128 rw".to_string()));
        assert!(p.contains(&"SystemCallFilter=~@raw-io".to_string()));
        assert!(p.contains(&"InaccessiblePaths=-/dev/dri/card1 -/dev/dri/renderD129 -/dev/kfd".to_string()));
        let allows: Vec<&String> = p.iter().filter(|x| x.starts_with("DeviceAllow=")).collect();
        assert!(allows.iter().all(|a| !a.contains("card") && !a.contains("kfd") && !a.contains("renderD129")), "{allows:?}");
        assert_eq!(private_devices(Some(DeviceMode::DevicePolicy)), "PrivateDevices=no");
        assert_eq!(private_devices(None), "PrivateDevices=yes");
        assert_eq!(
            gpu_env(&g),
            vec![
                ("K2_GPU_VENDOR".to_string(), "amd".to_string()),
                ("K2_GPU_PCI".to_string(), "0000:c4:00.0".to_string()),
                ("K2_GPU_RENDER".to_string(), "/dev/dri/renderD128".to_string())
            ]
        );
    }

    #[test]
    fn gpu_on_writes_the_file_and_warns_about_the_desktop() {
        let d = crate::util::temp_dir("gpu-on");
        let sys = d.join("sys");
        crate::gpu::tests::fixture(&sys, &[("renderD128", "card1", "0000:c4:00.0", "0x1002", "0x1586", "amdgpu", None, None, None)]);
        let cfg = d.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let out = cli_on(&cfg, &d, &sys, None, &[], None).unwrap();
        assert!(out.contains("GPU jobs on: 0000:c4:00.0 amd (renderD128) for owner"), "{out}");
        assert!(out.contains("can end the desktop session"), "{out}");
        assert!(out.contains("--gpu"), "names the grant step: {out}");
        assert_eq!(read_gpu_file(&cfg).unwrap(), Some(file(&["owner"])));
        // A server that isn't on this node is refused (no servers.toml).
        assert!(cli_on(&cfg, &d, &sys, None, &["acme.k2.dev".into()], None).is_err());
        // No GPU at all: refused.
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(cli_on(&cfg, &d, &empty, None, &[], None).is_err());
        let st = cli_status(&cfg, &sys, &d, false).unwrap();
        assert!(st.contains("0000:c4:00.0 amd renderD128 (amdgpu): on for owner"), "{st}");
    }

    #[test]
    fn gpu_on_refuses_nvidia() {
        let d = crate::util::temp_dir("gpu-on-nv");
        let sys = d.join("sys");
        crate::gpu::tests::fixture(&sys, &[("renderD128", "card0", "0000:01:00.0", "0x10de", "0x2684", "nvidia", None, None, None)]);
        let cfg = d.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let e = cli_on(&cfg, &d, &sys, Some("0000:01:00.0"), &[], None).unwrap_err();
        assert!(e.contains("NVIDIA GPUs aren't supported"), "{e}");
        assert!(cli_on(&cfg, &d, &sys, None, &[], None).is_err(), "only NVIDIA: nothing to turn on");
        assert_eq!(read_gpu_file(&cfg).unwrap(), None);
    }
}
