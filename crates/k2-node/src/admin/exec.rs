//! The root executor's decision core (prd-k2-node-admin-v1 §7.3): every
//! check, in order, then the verb. No I/O beyond the config dir, the
//! state dir and the [`Platform`]; the socket lives in `server`.
//!
//! Order (verify before parse; any doubt refuses):
//! 1. (server) peer is the node user or root; message ≤ 1 MiB.
//! 2. `sig_c` over the raw body against root's pinned controller key.
//! 3. body parses strictly; this node, this controller; `iat`/`exp`
//!    alive (±5 min); nonce unseen.
//! 4. the executor classifies the verb from the node's own state; a
//!    privileged one needs `sig_o` by a pinned approval key.
//! 5. `remote_admin` allows it; args validate.
//! 6. audit `exec` + record the nonce **before** running; run; audit
//!    `result`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use k2_node_proto::admin::{
    self, code, AdminKeys, AdminRefusal, AdminResult, ApprovalKey, Class, Level, Opened, Runs, SignedCommand,
    CLOCK_SKEW_SECS, MAX_LIFETIME_SECS,
};
use k2_node_proto::crypto;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::config::{self, AdminConfig, AdminFile};
use super::store::{Audit, AuditEntry, NonceStore};

/// Where the executor reads and writes. Fixed by the root-owned service
/// unit's argv; nothing a socket peer sends can change them.
#[derive(Debug, Clone)]
pub struct ExecPaths {
    /// `/etc/k2-node` (admin.toml, policy.toml, control.toml, narrow.toml).
    pub config_dir: PathBuf,
    /// Root-only state (0700).
    pub state_dir: PathBuf,
    /// The node home (read: the node key; write: nothing but `run/`).
    pub home: PathBuf,
    /// The installed `k2-node` binary (self-update swaps it).
    pub binary: PathBuf,
}

impl ExecPaths {
    pub fn admin(&self) -> PathBuf {
        self.config_dir.join(config::ADMIN_FILE)
    }
    pub fn nonces(&self) -> PathBuf {
        self.state_dir.join("nonces.json")
    }
    pub fn audit(&self) -> PathBuf {
        self.state_dir.join("audit.log")
    }
    pub fn node_fp_pin(&self) -> PathBuf {
        self.state_dir.join("node_fp")
    }
    pub fn update_state(&self) -> PathBuf {
        self.state_dir.join("update.json")
    }
    pub fn layout(&self) -> crate::paths::Layout {
        crate::paths::Layout::new(&self.home, &self.config_dir)
    }
}

/// The OS glue (service manager). Swapped for a recorder in tests.
pub trait Platform: Send + Sync {
    fn now(&self) -> i64;
    fn os(&self) -> &'static str;
    fn arch(&self) -> &'static str;
    /// Restart the node service (`k2-node.service` / `system/dev.k2.node`).
    fn restart_node(&self) -> Result<(), String>;
    /// Restart a K2-owned unit by its short name (`k2-node`, `k2-node-packages`).
    fn restart_unit(&self, unit: &str) -> Result<(), String>;
    /// The running node service's main pid, read before a restart so the
    /// update mark names the process it is about (C3). `None` = not running
    /// or unknown.
    fn node_pid(&self) -> Option<u32>;
    /// Run a staged build's `version` as the node user before the swap (a
    /// build the node user can't exec never goes in). Returns its output.
    fn preflight(&self, staged: &Path, node_user: &str) -> Result<String, String>;
    /// Live K2 job units (`k2job-*`, Linux), counted by root.
    fn running_job_units(&self) -> Option<usize> {
        let pattern = format!("{}*", super::jobs::UNIT_PREFIX);
        let list = super::jobs::systemctl(&["list-units", "--plain", "--no-legend", "--state=active,activating,deactivating", &pattern]).ok()?;
        Some(list.lines().filter(|l| l.trim_start().starts_with(super::jobs::UNIT_PREFIX)).count())
    }
    /// `wall.enable`'s root half: the job user, the home ACL, jobs.toml.
    fn enable_job_wall(&self, paths: &ExecPaths, node_user: &str, job_user: &str) -> Result<super::jobs::WallSetup, AdminRefusal> {
        super::jobs::system_enable_job_wall(paths, node_user, job_user)
    }
}

/// How long a staged build's `version` may take in the preflight.
pub const PREFLIGHT_SECS: u64 = 10;

/// Run `<staged> version` with a clean env from `/`, as `as_user` (uid,
/// gid) when given, killed after `limit`. Setting the uid as root also
/// drops every supplementary group (std calls `setgroups(0)` before
/// `setuid` when no groups are named), so the build runs exactly as the
/// node user would run it after the swap.
pub fn preflight_exec(staged: &Path, as_user: Option<(u32, u32)>, limit: std::time::Duration) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new(staged);
    c.arg("version")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some((uid, gid)) = as_user {
        c.gid(gid).uid(uid);
    }
    // Nothing of root's crosses into the node user's process: every fd
    // above 2 (the admin listener, a job's pipes in flight) is close-on-exec
    // in the child (A-L1 / B-L3). std's own exec-error pipe already is.
    // SAFETY: only async-signal-safe calls (fcntl, syscall, getrlimit).
    unsafe {
        c.pre_exec(|| {
            cloexec_from(3);
            Ok(())
        });
    }
    let mut child = {
        let _g = super::fdpass::spawn_guard();
        c.spawn().map_err(|e| format!("exec {}: {e}", staged.display()))?
    };
    let t0 = std::time::Instant::now();
    let st = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if t0.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} version didn't answer within {} s", staged.display(), limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    if let Some(mut o) = child.stderr.take() {
        let _ = o.read_to_string(&mut err);
    }
    if !st.success() {
        return Err(format!("{} version: {st}: {}", staged.display(), err.trim()));
    }
    // Its first line only, printable, at most 200 chars: it goes into logs,
    // update.json and the controller (B-I1).
    let line: String = out.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).take(200).collect();
    Ok(line.trim().to_string())
}

/// Mark every fd from `lowfd` up close-on-exec (in a forked child, before
/// exec). Linux: `close_range(…, CLOSE_RANGE_CLOEXEC)` (5.11+), else a loop.
pub fn cloexec_from(lowfd: i32) {
    #[cfg(target_os = "linux")]
    {
        const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
        // SAFETY: a plain syscall on our own fd table.
        if unsafe { libc::syscall(libc::SYS_close_range, lowfd as libc::c_uint, libc::c_uint::MAX, CLOSE_RANGE_CLOEXEC) } == 0 {
            return;
        }
    }
    let mut rl = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: valid out-pointer.
    let max = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) } == 0 { rl.rlim_cur.min(65_536) as i32 } else { 4096 };
    for fd in lowfd..max {
        // SAFETY: fcntl on a possibly-closed fd just fails with EBADF.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
}

/// The node user's (uid, gid) when this process can switch to it (root),
/// else `None` (unit tests run unprivileged and exec as themselves).
pub fn preflight_user(node_user: &str) -> Result<Option<(u32, u32)>, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Ok(None);
    }
    super::server::user_ids(node_user).map(Some).ok_or_else(|| format!("the node user {node_user} is missing"))
}

/// A program's stdout (absolute path, clean env), or `None` on failure.
fn output_fixed(program: &str, args: &[&str]) -> Option<String> {
    let out = super::fdpass::spawn_output(
        std::process::Command::new(program)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir("/")
            .stdin(std::process::Stdio::null()),
    )
    .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `systemctl show -p MainPID --value` → pid (0 = not running).
pub fn parse_main_pid(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok().filter(|p| *p > 1)
}

/// `launchctl print system/<label>` → the `pid = N` line.
pub fn parse_launchctl_pid(s: &str) -> Option<u32> {
    s.lines().find_map(|l| l.trim().strip_prefix("pid = ").and_then(|p| p.trim().parse::<u32>().ok())).filter(|p| *p > 1)
}

/// systemd on Linux, launchd on macOS. Absolute program paths, clean env.
pub struct SystemPlatform;

pub(crate) fn run_fixed(program: &str, args: &[&str]) -> Result<(), String> {
    let out = super::fdpass::spawn_output(
        std::process::Command::new(program)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir("/")
            .stdin(std::process::Stdio::null()),
    )
    .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{program} {}: exit {:?}: {}", args.join(" "), out.status.code(), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// K2-owned units a remote `unit.restart` may name.
pub const K2_UNITS: &[&str] = &["k2-node", "k2-node-packages"];

impl Platform for SystemPlatform {
    fn now(&self) -> i64 {
        crate::util::now()
    }
    fn os(&self) -> &'static str {
        crate::sysinfo::os_label()
    }
    fn arch(&self) -> &'static str {
        std::env::consts::ARCH
    }
    fn restart_node(&self) -> Result<(), String> {
        self.restart_unit("k2-node")
    }
    fn restart_unit(&self, unit: &str) -> Result<(), String> {
        if !K2_UNITS.contains(&unit) {
            return Err(format!("{unit} is not a K2 unit"));
        }
        if cfg!(target_os = "macos") {
            match unit {
                "k2-node" => run_fixed("/bin/launchctl", &["kickstart", "-k", "system/dev.k2.node"]),
                _ => Err(format!("{unit} has no launchd job on macOS")),
            }
        } else {
            let u = match unit {
                "k2-node" => "k2-node.service",
                _ => "k2-node-packages.path",
            };
            run_fixed("/usr/bin/systemctl", &["restart", u]).or_else(|_| run_fixed("/bin/systemctl", &["restart", u]))
        }
    }
    fn node_pid(&self) -> Option<u32> {
        if cfg!(target_os = "macos") {
            output_fixed("/bin/launchctl", &["print", "system/dev.k2.node"]).as_deref().and_then(parse_launchctl_pid)
        } else {
            let args = ["show", "-p", "MainPID", "--value", "k2-node.service"];
            output_fixed("/usr/bin/systemctl", &args).or_else(|| output_fixed("/bin/systemctl", &args)).as_deref().and_then(parse_main_pid)
        }
    }
    fn preflight(&self, staged: &Path, node_user: &str) -> Result<String, String> {
        preflight_exec(staged, preflight_user(node_user)?, std::time::Duration::from_secs(PREFLIGHT_SECS))
    }
}

/// Test-keys builds only: a hook program replaces the service manager
/// (`<hook> restart-node` / `<hook> restart-unit <name>`), so the Linux
/// container end-to-end can run without systemd. A release build has no
/// such code path.
#[cfg(feature = "test-keys")]
pub struct HookPlatform(pub PathBuf);

#[cfg(feature = "test-keys")]
impl Platform for HookPlatform {
    fn now(&self) -> i64 {
        crate::util::now()
    }
    fn os(&self) -> &'static str {
        crate::sysinfo::os_label()
    }
    fn arch(&self) -> &'static str {
        std::env::consts::ARCH
    }
    fn restart_node(&self) -> Result<(), String> {
        run_fixed(&self.0.display().to_string(), &["restart-node"])
    }
    fn restart_unit(&self, unit: &str) -> Result<(), String> {
        run_fixed(&self.0.display().to_string(), &["restart-unit", unit])
    }
    fn node_pid(&self) -> Option<u32> {
        output_fixed(&self.0.display().to_string(), &["node-pid"]).as_deref().and_then(parse_main_pid)
    }
    fn preflight(&self, staged: &Path, node_user: &str) -> Result<String, String> {
        // The container end-to-end stages real test-keys builds: exec them.
        preflight_exec(staged, preflight_user(node_user)?, std::time::Duration::from_secs(PREFLIGHT_SECS))
    }
}

/// What the node may ask over the local socket. Header JSON; an `update`
/// is followed by one binary frame of `size` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LocalRequest {
    Command { cmd: SignedCommand },
    Update { cmd: SignedCommand, sig: String, size: u64 },
    Keys { keys: AdminKeys },
    Status,
    /// The Linux job wall (0.46.3, [`super::jobs`]). Unsigned: only the
    /// node user (or root) gets past the socket's peer check, and root
    /// fixes everything that makes the wall. An older executor answers
    /// `malformed`, and the node runs jobs without the wall.
    Job { job: super::jobs::JobOp },
    /// Node sharing housekeeping the node's ticker asks for (unsigned, node
    /// user only; it does only what `servers.toml` and time already
    /// decided: [`super::servers::prune`]).
    Servers { servers: ServersOp },
    /// `sudo k2-node servers owner …` at the machine (unsigned; the socket
    /// accepts it from root only, never the node user): start an owner
    /// change ([`super::owner`]), or `resume` one that stopped.
    OwnerChange {
        #[serde(default)]
        args: Option<k2_node_proto::share::OwnerArgs>,
        #[serde(default)]
        resume: bool,
        by: String,
    },
}

/// Internal `servers.*` ops (prd-compute-shared-queue-v1 §10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServersOp {
    /// Promote confirmed adds, drop expired pending lines, wipe due slots.
    Prune,
}

/// The executor's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalResponse {
    Result(AdminResult),
    Status(ExecStatus),
    Job(super::jobs::JobAnswer),
}

/// `status` (unsigned, read-only; feeds health §10.1 "executor").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecStatus {
    pub version: String,
    pub protocol: u32,
    pub remote_admin: String,
    pub controller_fp: Option<String>,
    pub approval_key_fps: Vec<String>,
    pub auto_update: String,
    pub node_fp: Option<String>,
    pub configured: bool,
    pub config_error: Option<String>,
    pub last_update: Option<Value>,
    /// Root's `wall-restart-pending` flag (0.46.4; absent from older executors).
    #[serde(default)]
    pub wall_restart_pending: bool,
}

/// A checked command, ready to run.
pub struct Checked {
    pub opened: Opened,
    pub class: Class,
    pub approval_key: Option<String>,
    pub cfg: AdminConfig,
    pub node_fp: String,
}

pub struct Executor {
    pub paths: ExecPaths,
    pub platform: Arc<dyn Platform>,
    /// Also trust the TEST minisign key (test-keys builds only).
    pub test_keys: bool,
    /// One root action at a time (the server is single-threaded; the
    /// update health gate runs in the background and holds this).
    pub busy: Arc<Mutex<()>>,
    /// After a healthy update (or a rollback) exit so the service manager
    /// starts the new binary. True in `admin serve`; false in tests.
    pub exit_after_update: bool,
    /// How long a shared node's update or restart waits for running jobs
    /// (V55; 30 min; tests shorten it).
    pub drain_max: std::time::Duration,
    /// A drain is under way (one at a time).
    pub draining: Arc<std::sync::atomic::AtomicBool>,
    /// Bumped by every update or rollback: a drain started under an older
    /// value never acts (a later `now` or rollback cancelled it, review A L7).
    pub drain_gen: Arc<std::sync::atomic::AtomicU64>,
    /// Set once a healthy update or a rollback wants the executor on the
    /// binary now on disk: `serve` stops accepting, lets the connections in
    /// flight finish (never `process::exit`, C5), and returns; the service
    /// manager starts the new binary on the next connection.
    pub reexec: Arc<std::sync::atomic::AtomicBool>,
    /// A settling drain (`wall.enable`) waits this long with control at
    /// `draining` after the count reaches 0, then counts again: one node
    /// status write (10 s) plus a control poll (2 s), so a job the node
    /// accepted just before it saw `draining` is in the count (round 4, 1).
    pub drain_settle: std::time::Duration,
}

type Refused = (String, String, AdminRefusal);

fn refused(o: Option<&Opened>, r: AdminRefusal) -> Refused {
    match o {
        Some(o) => (o.cmd.cmd_id.clone(), o.digest(), r),
        None => (String::new(), String::new(), r),
    }
}

impl Executor {
    pub fn new(paths: ExecPaths, platform: Arc<dyn Platform>) -> Self {
        Self {
            paths,
            platform,
            test_keys: cfg!(feature = "test-keys"),
            busy: Arc::new(Mutex::new(())),
            exit_after_update: false,
            drain_max: std::time::Duration::from_secs(super::update::DRAIN_MAX_SECS as u64),
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drain_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            reexec: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drain_settle: std::time::Duration::from_secs(super::update::DRAIN_SETTLE_SECS),
        }
    }

    /// Another handle on the same executor (shared locks and flags) for a
    /// background thread (the health gate, a drain).
    pub fn bg_handle(&self) -> Executor {
        Executor {
            paths: self.paths.clone(),
            platform: self.platform.clone(),
            test_keys: self.test_keys,
            busy: self.busy.clone(),
            exit_after_update: self.exit_after_update,
            drain_max: self.drain_max,
            draining: self.draining.clone(),
            drain_gen: self.drain_gen.clone(),
            reexec: self.reexec.clone(),
            drain_settle: self.drain_settle,
        }
    }

    pub fn audit(&self) -> Audit {
        Audit::new(&self.paths.audit())
    }

    pub fn now(&self) -> i64 {
        self.platform.now()
    }

    /// The node fingerprint, pinned in root state on first use (the node
    /// key file is the node user's; root never trusts a later change).
    pub fn node_fp(&self) -> Result<String, AdminRefusal> {
        let pin = self.paths.node_fp_pin();
        if let Ok(s) = std::fs::read_to_string(&pin) {
            let s = s.trim().to_string();
            if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(s);
            }
            return Err(AdminRefusal::new(code::NOT_CONFIGURED, "node fingerprint pin is unreadable"));
        }
        let pem = std::fs::read_to_string(self.paths.layout().key())
            .map_err(|_| AdminRefusal::new(code::NOT_CONFIGURED, "no node key yet (enroll the node first)"))?;
        let key = crypto::SigningKey::from_pkcs8_pem(&pem).map_err(|_| AdminRefusal::new(code::NOT_CONFIGURED, "node key unreadable"))?;
        let fp = key.fingerprint();
        crate::util::atomic_write(&pin, fp.as_bytes(), 0o600).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
        Ok(fp)
    }

    fn config(&self) -> Result<AdminConfig, AdminRefusal> {
        match config::read(&self.paths.admin()) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err(AdminRefusal::new(code::REMOTE_ADMIN_OFF, "remote admin is not set up on this node (sudo k2-node setup --remote-admin …)")),
            Err(e) => Err(AdminRefusal::new(code::NOT_CONFIGURED, format!("admin.toml: {e}"))),
        }
    }

    /// §7.3 steps 2–5. Never runs anything.
    pub fn check(&self, raw: &[u8]) -> Result<Checked, Refused> {
        let cfg = self.config().map_err(|r| refused(None, r))?;
        let ctl = cfg
            .controller
            .clone()
            .ok_or_else(|| refused(None, AdminRefusal::new(code::NOT_CONFIGURED, "no controller key pinned (sudo k2-node admin pin)")))?;
        let o = admin::open(raw, &ctl.spki_der).map_err(|r| refused(None, r))?;
        let node_fp = self.node_fp().map_err(|r| refused(Some(&o), r))?;
        let now = self.now();
        admin::check_target_and_time(&o.cmd, &node_fp, &ctl.fp, now).map_err(|r| refused(Some(&o), r))?;
        let nonces = NonceStore::open(&self.paths.nonces()).map_err(|e| refused(Some(&o), AdminRefusal::new(code::FAILED, e)))?;
        if nonces.seen(&o.cmd.nonce) {
            return Err(refused(Some(&o), AdminRefusal::new(code::REPLAYED, "this command already ran")));
        }
        let spec = admin::verb(&o.cmd.verb)
            .ok_or_else(|| refused(Some(&o), AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not on this node's list")))?;
        if spec.runs != Runs::Executor {
            return Err(refused(Some(&o), AdminRefusal::new(code::VERB_NOT_ALLOWED, "the node answers this verb itself")));
        }
        let computed = self.classify(&o.cmd.verb, &o.cmd.args).map_err(|r| refused(Some(&o), r))?;
        let class = computed.max(o.cmd.class);
        let approval_key = if class == Class::Privileged {
            Some(admin::verify_owner(&o, &cfg.approval_keys, now).map_err(|r| refused(Some(&o), r))?)
        } else {
            None
        };
        if cfg.level < spec.min_level {
            let hint = if spec.name == k2_node_proto::share::VERB_OWNER { "; or at the machine: sudo k2-node servers owner <server>" } else { "" };
            return Err(refused(
                Some(&o),
                AdminRefusal::new(code::REMOTE_ADMIN_OFF, format!("remote admin here is {}; {} needs {}{hint}", cfg.level.as_str(), spec.name, spec.min_level.as_str())),
            ));
        }
        Ok(Checked { opened: o, class, approval_key, cfg, node_fp })
    }

    /// The executor's own class for a verb (never the controller's label).
    pub fn classify(&self, verb: &str, args: &Value) -> Result<Class, AdminRefusal> {
        let spec = admin::verb(verb).ok_or_else(|| AdminRefusal::new(code::VERB_NOT_ALLOWED, "unknown verb"))?;
        match verb {
            "policy.set" => super::policy::classify_set(&self.paths.layout(), args),
            // The owner's own wall choice stands (A-L2 / B-L8).
            "wall.enable" if super::jobs::wall_needs_owner(&self.paths) => Ok(Class::Privileged),
            k2_node_proto::share::VERB_LIMITS => super::servers::classify_limits(&self.paths.config_dir, args, self.now()),
            _ => Ok(spec.base),
        }
    }

    /// Audit a refusal (best effort: a refusal is never upgraded to an
    /// allow because the audit failed) and build the result.
    pub fn refuse_result(&self, (cmd_id, digest, r): Refused) -> AdminResult {
        let seq = self
            .audit()
            .append(
                AuditEntry {
                    event: "refused".into(),
                    cmd_id: (!cmd_id.is_empty()).then(|| cmd_id.clone()),
                    digest: (!digest.is_empty()).then(|| digest.clone()),
                    code: Some(r.code.clone()),
                    ..Default::default()
                },
                self.now(),
            )
            .ok();
        let mut res = AdminResult::refused(&cmd_id, &digest, &r);
        res.audit_seq = seq;
        res
    }

    /// Steps 6: audit, nonce, run, audit.
    fn run_checked(&self, c: Checked, payload: Option<(&str, &[u8])>) -> AdminResult {
        let o = &c.opened;
        let digest = o.digest();
        let base = AuditEntry {
            cmd_id: Some(o.cmd.cmd_id.clone()),
            verb: Some(o.cmd.verb.clone()),
            class: Some(c.class.as_str().into()),
            digest: Some(digest.clone()),
            proposed_by: Some(o.cmd.proposed_by.clone()),
            approved_by: o.cmd.approved_by.clone(),
            approval_key: c.approval_key.clone(),
            ..Default::default()
        };
        let now = self.now();
        let audit = self.audit();
        let exec_seq = match audit.append(AuditEntry { event: "exec".into(), detail: json!({"args": audit_args(&o.cmd.verb, &o.cmd.args)}), ..base.clone() }, now) {
            Ok(s) => s,
            Err(e) => return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::FAILED, format!("audit write failed, not running: {e}"))),
        };
        let mut nonces = match NonceStore::open(&self.paths.nonces()) {
            Ok(n) => n,
            Err(e) => return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::FAILED, e)),
        };
        if nonces.seen(&o.cmd.nonce) {
            return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::REPLAYED, "this command already ran"));
        }
        if let Err(e) = nonces.insert(&o.cmd.nonce, o.cmd.exp, now) {
            return AdminResult::refused(&o.cmd.cmd_id, &digest, &AdminRefusal::new(code::FAILED, format!("nonce store: {e}")));
        }
        let out = self.dispatch(&c, payload);
        let (mut res, code_s) = match out {
            Ok(v) => (AdminResult::done(&o.cmd.cmd_id, &digest, v), None),
            Err(r) => {
                let mut x = AdminResult::refused(&o.cmd.cmd_id, &digest, &r);
                if r.code == code::FAILED {
                    x.state = "failed".into();
                }
                let c = r.code.clone();
                (x, Some(c))
            }
        };
        let seq = audit
            .append(
                AuditEntry { event: "result".into(), code: code_s, detail: json!({"state": res.state, "exec_seq": exec_seq}), ..base },
                self.now(),
            )
            .unwrap_or(exec_seq);
        res.audit_seq = Some(seq);
        res
    }

    /// One `command` request (operate or privileged).
    pub fn handle_command(&self, cmd: &SignedCommand) -> AdminResult {
        let raw = match cmd.to_json() {
            Ok(r) => r,
            Err(e) => return AdminResult::refused("", "", &AdminRefusal::new(code::MALFORMED, e)),
        };
        let Some(_g) = self.lock_briefly() else {
            return AdminResult::refused("", "", &AdminRefusal::new(code::BUSY, "an update health check is running; try again in a minute"));
        };
        match self.check(raw.as_bytes()) {
            Ok(c) => {
                if c.opened.cmd.verb == "node.update" {
                    return self.refuse_result(refused(Some(&c.opened), AdminRefusal::new(code::BAD_ARGS, "node.update comes with its build (op update)")));
                }
                self.run_checked(c, None)
            }
            Err(r) => self.refuse_result(r),
        }
    }

    /// The busy lock, waiting up to 5 s.
    fn lock_briefly(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        for _ in 0..50 {
            match self.busy.try_lock() {
                Ok(g) => return Some(g),
                Err(std::sync::TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        None
    }

    /// One `update` request: the signed `node.update` command plus the
    /// build bytes (already read into memory by the server).
    pub fn handle_update(&self, cmd: &SignedCommand, sig: &str, bytes: &[u8]) -> AdminResult {
        let raw = match cmd.to_json() {
            Ok(r) => r,
            Err(e) => return AdminResult::refused("", "", &AdminRefusal::new(code::MALFORMED, e)),
        };
        let Some(guard) = self.lock_briefly() else {
            return AdminResult::refused("", "", &AdminRefusal::new(code::BUSY, "another root action (or an update health check) is running"));
        };
        let r = match self.check(raw.as_bytes()) {
            Ok(c) if c.opened.cmd.verb == "node.update" => self.run_checked(c, Some((sig, bytes))),
            Ok(c) => self.refuse_result(refused(Some(&c.opened), AdminRefusal::new(code::BAD_ARGS, "op update carries only node.update"))),
            Err(r) => self.refuse_result(r),
        };
        drop(guard);
        r
    }

    /// The verbs (S1: level, control, unit; S3 policy; S5 update/rollback).
    fn dispatch(&self, c: &Checked, payload: Option<(&str, &[u8])>) -> Result<Value, AdminRefusal> {
        let args = &c.opened.cmd.args;
        let by = format!("controller:{}", c.opened.cmd.proposed_by);
        match c.opened.cmd.verb.as_str() {
            "admin.level.lower" => self.level_lower(c, args),
            "control.pause" | "control.drain" | "control.resume" => {
                no_args(args)?;
                let to = match c.opened.cmd.verb.as_str() {
                    "control.pause" => k2_node_proto::frames::Control::Paused,
                    "control.drain" => k2_node_proto::frames::Control::Draining,
                    _ => k2_node_proto::frames::Control::Active,
                };
                crate::config::write_control(&self.paths.layout().control(), to, &by, self.now()).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
                Ok(json!({"control": to.as_str()}))
            }
            "unit.restart" => {
                let unit = str_arg(args, "unit")?;
                only_keys(args, &["unit", "now"])?;
                if !K2_UNITS.contains(&unit.as_str()) {
                    return Err(AdminRefusal::new(code::BAD_ARGS, "unit must be a K2 unit (k2-node, k2-node-packages)"));
                }
                if unit == "k2-node" {
                    // A shared node waits for running jobs first (V55).
                    let now = super::update::now_arg(args)?;
                    let cmd_id = c.opened.cmd.cmd_id.clone();
                    return super::update::drain_then(self, now, "unit.restart", &c.opened.cmd.cmd_id, move |e| {
                        // Every K2-made node restart is marked (C3).
                        super::update::restart_marked(e, "unit.restart", &cmd_id, |e| e.platform.restart_unit("k2-node"))
                            .map_err(|x| AdminRefusal::new(code::FAILED, x))?;
                        Ok(json!({"restarted": "k2-node"}))
                    });
                }
                self.platform.restart_unit(&unit).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
                Ok(json!({"restarted": unit}))
            }
            "homes.private" => self.homes_private(args),
            "wall.enable" => self.wall_enable(args, &c.opened.cmd.cmd_id, c.class == Class::Privileged && c.approval_key.is_some()),
            "policy.set" => super::policy::apply_set(&self.paths.layout(), args),
            "policy.narrow" => super::policy::apply_narrow(&self.paths.config_dir, args),
            "node.update" => {
                let (sig, bytes) = payload.ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "node.update needs its build"))?;
                super::update::apply(self, c, sig, bytes)
            }
            "node.rollback" => {
                no_args_except(args, &["reason", "now"])?;
                let now = super::update::now_arg(args)?;
                super::update::drain_then(self, now, "node.rollback", &c.opened.cmd.cmd_id, |e| super::update::rollback(e, "manual"))
            }
            k2_node_proto::share::VERB_OWNER => {
                k2_node_proto::share::check_verb_args(k2_node_proto::share::VERB_OWNER, args).map_err(|m| AdminRefusal::new(code::BAD_ARGS, m))?;
                let a: k2_node_proto::share::OwnerArgs = serde_json::from_value(args.clone()).map_err(|e| AdminRefusal::new(code::BAD_ARGS, e.to_string()))?;
                let id = &c.opened.cmd.cmd_id;
                super::owner::start(self, &a, id, &format!("owner-approval:{id}"), true)
            }
            v if k2_node_proto::share::VERBS.contains(&v) => {
                let ctx = super::servers::Ctx::for_executor(self, &format!("owner-approval:{}", c.opened.cmd.cmd_id));
                super::servers::run(&ctx, v, args)
            }
            _ => Err(AdminRefusal::new(code::VERB_NOT_ALLOWED, "verb is not built on this node yet")),
        }
    }

    fn level_lower(&self, _c: &Checked, args: &Value) -> Result<Value, AdminRefusal> {
        only_keys(args, &["level"])?;
        let to = Level::parse(&str_arg(args, "level")?).ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "level must be off|policy|full"))?;
        let cfg = self.config()?;
        if to >= cfg.level {
            return Err(AdminRefusal::new(code::HUMAN_REQUIRED, "the level can only be lowered remotely; the machine's owner raises it there (sudo k2-node admin level …, or Settings → Compute in that machine's K2, also over K2 Connect)"));
        }
        let mut f: AdminFile = cfg.file.clone();
        f.remote_admin = to.as_str().into();
        config::write(&self.paths.admin(), &f).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
        Ok(json!({"remote_admin": to.as_str(), "was": cfg.level.as_str()}))
    }

    /// `admin_keys` (§7.4): first pin with the controller signature (trust
    /// on first use); rotation also needs a currently pinned key.
    pub fn handle_keys(&self, k: &AdminKeys) -> AdminResult {
        let Some(_g) = self.lock_briefly() else {
            return AdminResult::refused("admin_keys", "", &AdminRefusal::new(code::BUSY, "busy; try again"));
        };
        let r = (|| -> Result<Value, AdminRefusal> {
            let cfg = self.config()?;
            let ctl = cfg.controller.clone().ok_or_else(|| AdminRefusal::new(code::NOT_CONFIGURED, "no controller key pinned"))?;
            let part = k.signed_part().map_err(|e| AdminRefusal::new(code::MALFORMED, e))?;
            if !crypto::verify_domain(&ctl.spki_der, admin::DOMAIN_KEYS_CONTROLLER, &[&part], &k.sig_c) {
                return Err(AdminRefusal::new(code::BAD_CONTROLLER_SIGNATURE, "controller signature does not verify"));
            }
            let node_fp = self.node_fp()?;
            if !crypto::ct_eq(k.node_fp.as_bytes(), node_fp.as_bytes()) {
                return Err(AdminRefusal::new(code::WRONG_NODE, "keys are for another node"));
            }
            let now = self.now();
            if k.iat > now + CLOCK_SKEW_SECS || now > k.iat + MAX_LIFETIME_SECS + CLOCK_SKEW_SECS {
                return Err(AdminRefusal::new(code::EXPIRED, "key update expired"));
            }
            let mut nonces = NonceStore::open(&self.paths.nonces()).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
            if nonces.seen(&k.nonce) {
                return Err(AdminRefusal::new(code::REPLAYED, "this key update already ran"));
            }
            let mut new = Vec::new();
            for s in &k.keys {
                new.push(ApprovalKey::parse(s).ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "approval key is not ed25519:<b64>"))?);
            }
            if new.is_empty() || new.len() > 4 {
                return Err(AdminRefusal::new(code::BAD_ARGS, "1 to 4 approval keys"));
            }
            let mode = if cfg.approval_keys.is_empty() {
                "first_pin"
            } else {
                let sig = k
                    .sig_o
                    .as_ref()
                    .and_then(|s| crypto::unb64(s).ok())
                    .ok_or_else(|| AdminRefusal::new(code::APPROVAL_REQUIRED, "rotation needs a signature by a pinned approval key"))?;
                let msg = crypto::signed_bytes(admin::DOMAIN_KEYS_OWNER, &[&part]);
                if !cfg.approval_keys.iter().any(|a| a.verify(&msg, &sig)) {
                    return Err(AdminRefusal::new(code::BAD_APPROVAL_SIGNATURE, "rotation not signed by a pinned approval key"));
                }
                "rotate"
            };
            nonces.insert(&k.nonce, k.iat + MAX_LIFETIME_SECS, now).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
            let mut f = cfg.file.clone();
            f.approval_keys = new.iter().map(|a| a.encode()).collect();
            config::write(&self.paths.admin(), &f).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
            let fps: Vec<String> = new.iter().map(|a| a.fingerprint()).collect();
            let _ = self.audit().append(AuditEntry { event: "keys".into(), detail: json!({"mode": mode, "fps": fps}), ..Default::default() }, now);
            Ok(json!({"mode": mode, "approval_key_fps": fps}))
        })();
        match r {
            Ok(v) => AdminResult::done("admin_keys", "", v),
            Err(e) => self.refuse_result((String::from("admin_keys"), String::new(), e)),
        }
    }

    pub fn status(&self) -> ExecStatus {
        let (cfg, err) = match config::read(&self.paths.admin()) {
            Ok(c) => (c, None),
            Err(e) => (None, Some(e)),
        };
        let last_update = std::fs::read(self.paths.update_state()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        ExecStatus {
            version: crate::PRODUCT_VERSION.to_string(),
            protocol: k2_node_proto::PROTOCOL,
            remote_admin: cfg.as_ref().map(|c| c.level.as_str()).unwrap_or("off").to_string(),
            controller_fp: cfg.as_ref().and_then(|c| c.controller.as_ref().map(|x| x.fp.clone())),
            approval_key_fps: cfg.as_ref().map(|c| c.approval_keys.iter().map(|k| k.fingerprint()).collect()).unwrap_or_default(),
            auto_update: cfg.as_ref().map(|c| if c.auto_update { "follow" } else { "off" }).unwrap_or("follow").to_string(),
            node_fp: std::fs::read_to_string(self.paths.node_fp_pin()).ok().map(|s| s.trim().to_string()),
            configured: cfg.as_ref().is_some_and(|c| c.controller.is_some()),
            config_error: err,
            last_update,
            wall_restart_pending: self.paths.state_dir.join(super::jobs::WALL_RESTART_PENDING_FILE).exists(),
        }
    }

    pub fn handle(&self, req: &LocalRequest, payload: Option<&[u8]>) -> LocalResponse {
        match req {
            LocalRequest::Status => LocalResponse::Status(self.status()),
            LocalRequest::Command { cmd } => LocalResponse::Result(self.handle_command(cmd)),
            LocalRequest::Keys { keys } => LocalResponse::Result(self.handle_keys(keys)),
            LocalRequest::Update { cmd, sig, .. } => LocalResponse::Result(self.handle_update(cmd, sig, payload.unwrap_or(&[]))),
            // fds (a job's pipes) come only through `serve_conn`.
            LocalRequest::Job { job } => LocalResponse::Job(self.handle_job(job, Vec::new())),
            LocalRequest::Servers { servers } => LocalResponse::Result(self.handle_servers_op(*servers)),
            // Root only: `serve_conn` checks the peer before it gets here.
            LocalRequest::OwnerChange { args, resume, by } => LocalResponse::Result(self.handle_owner_local(args.as_ref(), *resume, by)),
        }
    }

    /// `sudo k2-node servers owner …` (root at the machine; audited
    /// `local`, then `owner_change` when it's done).
    pub fn handle_owner_local(&self, args: Option<&k2_node_proto::share::OwnerArgs>, resume: bool, by: &str) -> AdminResult {
        let name = k2_node_proto::share::VERB_OWNER;
        if resume {
            return match super::owner::resume(self, true) {
                None => AdminResult::done(name, "", json!({"resumed": false, "note": "no owner change is waiting"})),
                Some(Ok(v)) => AdminResult::done(name, "", json!({"resumed": true, "result": v})),
                Some(Err(r)) => self.refuse_result((name.to_string(), String::new(), r)),
            };
        }
        let Some(a) = args else {
            return self.refuse_result((name.to_string(), String::new(), AdminRefusal::new(code::BAD_ARGS, "name the server that becomes the owner")));
        };
        let Some(_g) = self.lock_briefly() else {
            return AdminResult::refused(name, "", &AdminRefusal::new(code::BUSY, "another root action is running; try again in a minute"));
        };
        let id = match crypto::random_id() {
            Ok(r) => format!("local-{}", &r[..12.min(r.len())]),
            Err(e) => return AdminResult::refused(name, "", &AdminRefusal::new(code::FAILED, e)),
        };
        let by = format!("local:{}", by.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).take(64).collect::<String>());
        let _ = self.audit().append(
            AuditEntry { event: "local".into(), cmd_id: Some(id.clone()), verb: Some(name.into()), proposed_by: Some(by.clone()), detail: json!({"args": a}), ..Default::default() },
            self.now(),
        );
        match super::owner::start(self, a, &id, &by, false) {
            Ok(v) => AdminResult::done(&id, "", v),
            Err(r) => self.refuse_result((id, String::new(), r)),
        }
    }

    /// An internal `servers.*` op from the node (unsigned: the socket's peer
    /// check let only the node user or root in, and the op only applies
    /// what the owner's file and the clock already say).
    pub fn handle_servers_op(&self, op: ServersOp) -> AdminResult {
        let name = match op {
            ServersOp::Prune => k2_node_proto::share::OP_SERVERS_PRUNE,
        };
        let Some(_g) = self.lock_briefly() else {
            return AdminResult::refused(name, "", &AdminRefusal::new(code::BUSY, "busy; try again"));
        };
        let ctx = super::servers::Ctx::for_executor(self, "node:prune");
        match super::servers::prune(&ctx) {
            Ok(v) => {
                let changed = v.get("changed").and_then(|c| c.as_array()).is_some_and(|c| !c.is_empty())
                    || v.get("wiped").and_then(|c| c.as_array()).is_some_and(|c| !c.is_empty());
                if changed {
                    let _ = self.audit().append(AuditEntry { event: name.into(), detail: v.clone(), ..Default::default() }, self.now());
                }
                AdminResult::done(name, "", v)
            }
            Err(r) => self.refuse_result((name.to_string(), String::new(), r)),
        }
    }
}

/// A command's args as the audit keeps them: the `servers.add` code
/// binding is a credential and never written down (V79).
pub fn audit_args(verb: &str, args: &Value) -> Value {
    let mut a = args.clone();
    if verb == k2_node_proto::share::VERB_ADD {
        if let Some(b) = a.get_mut("code_binding") {
            *b = Value::String("<redacted>".into());
        }
    }
    a
}

// ── arg helpers (strict: unknown keys refuse) ────────────────────────

pub fn only_keys(args: &Value, allowed: &[&str]) -> Result<(), AdminRefusal> {
    let m = args.as_object().ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, "args must be an object"))?;
    if let Some(k) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(AdminRefusal::new(code::BAD_ARGS, format!("unknown arg {k:?}")));
    }
    Ok(())
}

pub fn no_args(args: &Value) -> Result<(), AdminRefusal> {
    only_keys(args, &[])
}

pub fn no_args_except(args: &Value, allowed: &[&str]) -> Result<(), AdminRefusal> {
    only_keys(args, allowed)
}

pub fn str_arg(args: &Value, k: &str) -> Result<String, AdminRefusal> {
    args.get(k)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| AdminRefusal::new(code::BAD_ARGS, format!("{k} (string) is required")))
}

/// Create the state dir root-only (0700) if missing.
pub fn ensure_state_dir(p: &Path) -> Result<(), String> {
    crate::util::mkdir_mode(p, 0o700)
}

#[cfg(test)]
mod preflight_tests {
    use super::*;

    /// A-L1 / B-L3: the preflight child inherits no fd of root's above 2
    /// (here: one opened without close-on-exec, as systemd's listener is).
    #[test]
    fn the_preflight_child_inherits_no_fds_above_2() {
        let dir = crate::util::temp_dir("preflight-fds");
        let script = dir.join("k2-node");
        std::fs::write(&script, "#!/bin/sh\nprintf 'k2-node fds:'; ls /dev/fd | tr '\\n' ' '\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::ffi::CString::new(dir.join("held").display().to_string()).unwrap();
        // SAFETY: plain open(2) with a valid path; no O_CLOEXEC on purpose.
        let low = unsafe { libc::open(path.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600) };
        assert!(low > 2, "open: {}", std::io::Error::last_os_error());
        // Well above anything `ls` opens itself (F_DUPFD leaves it inheritable).
        let fd = unsafe { libc::fcntl(low, libc::F_DUPFD, 200) };
        unsafe { libc::close(low) };
        assert!(fd >= 200, "dup: {}", std::io::Error::last_os_error());
        let out = preflight_exec(&script, None, std::time::Duration::from_secs(10)).unwrap();
        unsafe { libc::close(fd) };
        let fds: Vec<i32> = out.trim_start_matches("k2-node fds:").split_whitespace().filter_map(|s| s.parse().ok()).collect();
        assert!(!fds.contains(&fd), "fd {fd} leaked into the child: {out}");
        assert!(fds.contains(&0) && fds.contains(&1), "the listing worked: {out}");
    }
}
