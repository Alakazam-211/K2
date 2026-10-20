//! The Linux job wall, root side (0.46.3, N3; `eval-compute-shape-A` S1).
//!
//! k2-node runs under `NoNewPrivileges` and must never own the jobs' user,
//! so the root executor starts each job for it: one fixed-template verb
//! family on the local socket, asked by the node user only (the peer
//! check), never by the controller. The node chooses the job's command,
//! env and limits (it can run any job anyway); root chooses everything
//! that makes the wall: the user (`job_user` in root-owned
//! `/etc/k2-node/jobs.toml`), the unit's sandbox properties, which paths
//! are writable (only under the node home's `jobs/` and `cache/`, checked
//! after resolving symlinks) and which are gone (the node's `state/` and
//! `run/`, `/etc/k2-node`, the executor's state).
//!
//! A job is `systemd-run --pipe --wait` of `k2-node job-exec <root>/exec.json`
//! as `k2job:<node group>` with the node's stdout/stderr pipes (passed
//! here over the socket with `SCM_RIGHTS`). `RemainAfterExit=yes` keeps
//! the unit loaded after the command ends, so the node reads its result
//! (`status`: exit code, signal, `oom-kill`) and then `release`s it.
//!
//! `jobs.toml` is a separate file (not `admin.toml`, which is strict and
//! read by older executors after a rollback): an older k2-node simply
//! never reads it, and without it there is no wall (jobs run as the node
//! user, as before).

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use k2_node_proto::admin::{code, AdminRefusal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::exec::Executor;

/// Root-owned, 0644, in the config dir. Written by `k2-node setup`.
pub const JOBS_FILE: &str = "jobs.toml";
/// The default job user (Linux).
pub const DEFAULT_JOB_USER: &str = "k2job";
/// Every job unit's name starts with this.
pub const UNIT_PREFIX: &str = "k2job-";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobsFile {
    /// The user jobs run as. Never root, never the node user.
    pub job_user: String,
}

/// `Ok(None)` when missing (no wall set up).
pub fn read_jobs_file(config_dir: &Path) -> Result<Option<JobsFile>, String> {
    let p = config_dir.join(JOBS_FILE);
    match std::fs::read_to_string(&p) {
        Ok(s) => toml::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

/// Check a would-be job user against the node user.
pub fn check_job_user(job_user: &str, node_user: &str) -> Result<(u32, u32), String> {
    if job_user.is_empty() || !job_user.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        return Err(format!("{job_user:?} is not a plain user name"));
    }
    let (uid, gid) = super::server::user_ids(job_user).ok_or_else(|| format!("{job_user} is not a user on this machine"))?;
    if uid == 0 {
        return Err("jobs never run as root".into());
    }
    if job_user == node_user || super::server::user_ids(node_user).is_some_and(|(n, _)| n == uid) {
        return Err(format!("{job_user} is the node user; jobs need their own user"));
    }
    Ok((uid, gid))
}

/// `sudo k2-node admin job-user <name>` (setup): validate and write.
pub fn write_jobs_file(config_dir: &Path, job_user: &str, node_user: &str) -> Result<String, String> {
    check_job_user(job_user, node_user)?;
    let body = toml::to_string(&JobsFile { job_user: job_user.to_string() }).map_err(|e| e.to_string())?;
    let text = format!(
        "# K2 compute node: jobs run as this user in a sandboxed systemd unit (0.46.3).\n\
         # Written by k2-node setup; remove the file to go back to jobs as the node user.\n{body}"
    );
    crate::util::atomic_write(&config_dir.join(JOBS_FILE), text.as_bytes(), 0o644)?;
    Ok(format!("jobs run as {job_user}"))
}

// ── the wire (node ↔ executor) ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum JobOp {
    /// Is the wall set up? (job user + systemd).
    Ready,
    /// Start a job. The request is followed by one `SCM_RIGHTS` message
    /// carrying exactly two fds: the job's stdout and stderr.
    Start(JobStart),
    Status { unit: String },
    /// SIGTERM everything in the unit, SIGKILL after its stop timeout
    /// (returns at once).
    Stop { unit: String },
    /// Stop (waits), forget the unit.
    Release { unit: String },
    /// Give a job tree back to the node user (chown, never following a
    /// symlink) so it can delete or reuse it.
    Reclaim { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStart {
    /// `<job_id>-g<generation>`; the unit is `k2job-<key>.service`.
    pub key: String,
    /// The job's dir: `<home>/jobs/<key>` (holds `exec.json`).
    pub root: PathBuf,
    /// More writable dirs: under the job root or `<home>/cache/`.
    pub rw: Vec<PathBuf>,
    pub network_none: bool,
    pub mem_bytes: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub stop_grace_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum JobAnswer {
    Ready { job_user: String },
    Started { unit: String },
    Status(UnitStatus),
    Done { detail: Value },
    Refused(AdminRefusal),
}

/// What `systemctl show` says about a job unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitStatus {
    pub loaded: bool,
    pub active_state: String,
    pub sub_state: String,
    /// `success` | `exit-code` | `signal` | `oom-kill` | `timeout` | …
    pub result: String,
    /// `ExecMainCode`: 1 exited, 2 killed, 3 dumped (0 = not yet).
    pub main_code: i32,
    /// The exit status or the signal number.
    pub main_status: i32,
    /// `oom_kill` from the unit's `memory.events`.
    pub oom_kills: u64,
}

impl UnitStatus {
    /// The command is over (the unit may still hold leftovers until
    /// `stop`). Asked only about a unit that started (`start` waits for it
    /// to load), so one systemd no longer knows ended and was collected:
    /// a stopped unit is unloaded at once, `RemainAfterExit` or not.
    pub fn ended(&self) -> bool {
        !self.loaded || self.active_state == "failed" || self.active_state == "inactive" || (self.active_state == "active" && self.sub_state == "exited")
    }

    /// (exit code, signal) of the main process, when it ended.
    pub fn exit(&self) -> Option<(Option<i32>, Option<i32>)> {
        match self.main_code {
            1 => Some((Some(self.main_status), None)),
            2 | 3 => Some((None, Some(self.main_status))),
            _ => None,
        }
    }

    pub fn oom(&self) -> bool {
        self.result == "oom-kill" || self.oom_kills > 0
    }
}

/// `systemctl show` key=value lines → status.
pub fn parse_show(text: &str) -> UnitStatus {
    let mut s = UnitStatus::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "LoadState" => s.loaded = v == "loaded",
            "ActiveState" => s.active_state = v.to_string(),
            "SubState" => s.sub_state = v.to_string(),
            "Result" => s.result = v.to_string(),
            "ExecMainCode" => s.main_code = v.parse().unwrap_or(0),
            "ExecMainStatus" => s.main_status = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    s
}

/// cgroup v2 `memory.events` → `oom_kill` count.
pub fn parse_oom_kills(text: &str) -> u64 {
    text.lines().find_map(|l| l.strip_prefix("oom_kill ")).and_then(|n| n.trim().parse().ok()).unwrap_or(0)
}

/// Oldest systemd whose unit sandbox has every property the template uses
/// (`ProtectClock`, 245: Ubuntu 20.04, Debian 11, current Arch/Fedora).
pub const MIN_SYSTEMD: u32 = 245;

/// `systemctl --version` → `255` from "systemd 255 (255.4-1ubuntu8)".
pub fn systemd_version(text: &str) -> Option<u32> {
    text.lines().next()?.split_whitespace().nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn valid_key(k: &str) -> bool {
    (1..=160).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn unit_name(key: &str) -> Option<String> {
    valid_key(key).then(|| format!("{UNIT_PREFIX}{key}.service"))
}

pub fn valid_unit(u: &str) -> bool {
    u.strip_prefix(UNIT_PREFIX).and_then(|r| r.strip_suffix(".service")).is_some_and(valid_key)
}

/// Everything root decides about one job unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPlan {
    pub unit: String,
    pub job_uid: u32,
    pub group_gid: u32,
    pub root: PathBuf,
    pub rw: Vec<PathBuf>,
    /// InaccessiblePaths (missing ones are skipped: `-` prefix).
    pub gone: Vec<PathBuf>,
    pub network_none: bool,
    pub mem_bytes: Option<u64>,
    pub cpu_millis: Option<u64>,
    pub stop_grace_secs: u64,
    /// The installed `k2-node` (its `job-exec` shim).
    pub binary: PathBuf,
}

/// The fixed template (argv[0] is `systemd-run`).
pub fn systemd_run_argv(p: &UnitPlan) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "systemd-run".into(),
        format!("--unit={}", p.unit),
        format!("--description=K2 compute job {}", p.unit.trim_end_matches(".service")),
        "--quiet".into(),
        "--pipe".into(),
        "--wait".into(),
        "--slice=k2-jobs.slice".into(),
        format!("--uid={}", p.job_uid),
        format!("--gid={}", p.group_gid),
        format!("--working-directory={}", p.root.display()),
    ];
    let mut prop = |s: String| {
        a.push("-p".into());
        a.push(s);
    };
    for s in [
        "RemainAfterExit=yes",
        "KillMode=control-group",
        "OOMPolicy=stop",
        "UMask=0002",
        "NoNewPrivileges=yes",
        "ProtectSystem=strict",
        "ProtectHome=yes",
        "PrivateTmp=yes",
        "PrivateDevices=yes",
        "ProtectKernelTunables=yes",
        "ProtectKernelModules=yes",
        "ProtectKernelLogs=yes",
        "ProtectControlGroups=yes",
        "ProtectClock=yes",
        "RestrictSUIDSGID=yes",
        "RestrictRealtime=yes",
        "LockPersonality=yes",
        "TasksMax=8192",
        "LimitNOFILE=65536",
    ] {
        prop(s.to_string());
    }
    prop(format!("TimeoutStopSec={}", p.stop_grace_secs.clamp(1, 600)));
    let mut rw = vec![p.root.display().to_string()];
    rw.extend(p.rw.iter().map(|x| x.display().to_string()));
    prop(format!("ReadWritePaths={}", rw.join(" ")));
    let gone: Vec<String> = p.gone.iter().map(|x| format!("-{}", x.display())).collect();
    if !gone.is_empty() {
        prop(format!("InaccessiblePaths={}", gone.join(" ")));
    }
    if let Some(m) = p.mem_bytes.filter(|m| *m > 0) {
        prop(format!("MemoryMax={m}"));
        prop("MemorySwapMax=0".into());
    }
    if let Some(c) = p.cpu_millis.filter(|c| *c > 0) {
        // thousandths of a core → percent of one core (≥ 1%).
        prop(format!("CPUQuota={}%", (c / 10).max(1)));
    }
    if p.network_none {
        prop("PrivateNetwork=yes".into());
        prop("IPAddressDeny=any".into());
        // The resolver's local sockets would still answer from outside
        // the namespace (DNS as a side channel).
        prop("InaccessiblePaths=-/run/systemd/resolve -/run/nscd -/var/run/nscd".into());
    }
    a.push("--".into());
    a.push(p.binary.display().to_string());
    a.push("job-exec".into());
    a.push(p.root.join(crate::wall::EXEC_SPEC).display().to_string());
    a
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

fn canon(p: &Path) -> Result<PathBuf, AdminRefusal> {
    std::fs::canonicalize(p).map_err(|e| refuse(code::BAD_ARGS, format!("{}: {e}", p.display())))
}

/// `child` strictly inside `parent` (both canonical).
fn inside(child: &Path, parent: &Path) -> bool {
    child != parent && child.starts_with(parent)
}

/// The job root and writable dirs, resolved and confined to the node
/// home's `jobs/<one dir>` and `cache/…`.
pub fn check_paths(home: &Path, root: &Path, rw: &[PathBuf]) -> Result<(PathBuf, Vec<PathBuf>), AdminRefusal> {
    let jobs = canon(&home.join("jobs"))?;
    let cache = canon(&home.join("cache"))?;
    let r = canon(root)?;
    if r.parent() != Some(jobs.as_path()) {
        return Err(refuse(code::BAD_ARGS, "a job root is one folder under the node home's jobs/"));
    }
    let mut out = Vec::new();
    for p in rw {
        let c = canon(p)?;
        if !(inside(&c, &r) || c == r || inside(&c, &cache)) {
            return Err(refuse(code::BAD_ARGS, format!("{} is outside the job's own folders", p.display())));
        }
        out.push(c);
    }
    Ok((r, out))
}

/// A path `reclaim` may take back: a job root or anything under one, or
/// anything under `cache/` (not `cache/` itself).
pub fn check_reclaim(home: &Path, path: &Path) -> Result<PathBuf, AdminRefusal> {
    let jobs = canon(&home.join("jobs"))?;
    let cache = canon(&home.join("cache"))?;
    let c = canon(path)?;
    if inside(&c, &jobs) || inside(&c, &cache) {
        Ok(c)
    } else {
        Err(refuse(code::BAD_ARGS, "only job folders and caches under the node home"))
    }
}

fn find_program(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].iter().map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    let prog = find_program("systemctl").ok_or("systemctl not found")?;
    let out = std::process::Command::new(&prog)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir("/")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("systemctl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Read one unit's status (root).
pub fn unit_status(unit: &str) -> Result<UnitStatus, String> {
    let text = systemctl(&["show", "--property=LoadState,ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,ControlGroup", unit])?;
    let mut s = parse_show(&text);
    if let Some(cg) = text.lines().find_map(|l| l.strip_prefix("ControlGroup=")).filter(|c| c.starts_with('/')) {
        if let Ok(ev) = std::fs::read_to_string(Path::new("/sys/fs/cgroup").join(cg.trim_start_matches('/')).join("memory.events")) {
            s.oom_kills = parse_oom_kills(&ev);
        }
    }
    Ok(s)
}

impl Executor {
    fn node_user(&self) -> String {
        super::config::read(&self.paths.admin())
            .ok()
            .flatten()
            .map(|c| c.node_user)
            .unwrap_or_else(|| super::config::default_node_user().to_string())
    }

    /// The job user (jobs.toml) and its uid, or why there's no wall.
    pub fn job_user(&self) -> Result<(String, u32), AdminRefusal> {
        if !cfg!(target_os = "linux") {
            return Err(refuse(code::NOT_CONFIGURED, "the job user wall is Linux only (macOS uses the Seatbelt profile)"));
        }
        let f = read_jobs_file(&self.paths.config_dir)
            .map_err(|e| refuse(code::NOT_CONFIGURED, e))?
            .ok_or_else(|| refuse(code::NOT_CONFIGURED, "no job user set up (sudo k2-node setup creates k2job)"))?;
        let (uid, _) = check_job_user(&f.job_user, &self.node_user()).map_err(|e| refuse(code::NOT_CONFIGURED, e))?;
        let home = self.paths.home.display().to_string();
        if home.starts_with("/home/") || home.starts_with("/root/") || home.starts_with("/run/user/") {
            return Err(refuse(code::NOT_CONFIGURED, "the node home is under /home or /root, which the job unit hides"));
        }
        if find_program("systemd-run").is_none() {
            return Err(refuse(code::NOT_CONFIGURED, "systemd-run not found"));
        }
        let v = systemctl(&["--version"]).ok().and_then(|t| systemd_version(&t)).unwrap_or(0);
        if v < MIN_SYSTEMD {
            return Err(refuse(code::NOT_CONFIGURED, format!("systemd {v} is older than {MIN_SYSTEMD} (the job unit's sandbox needs it)")));
        }
        Ok((f.job_user, uid))
    }

    /// The job ops (the server checked the peer: root or the node user).
    pub fn handle_job(&self, op: &JobOp, fds: Vec<OwnedFd>) -> JobAnswer {
        let r = match op {
            JobOp::Ready => self.job_user().map(|(u, _)| JobAnswer::Ready { job_user: u }),
            JobOp::Start(s) => self.start_job(s, fds).map(|unit| JobAnswer::Started { unit }),
            JobOp::Status { unit } => self.with_unit(unit, |u| unit_status(u).map(JobAnswer::Status)),
            JobOp::Stop { unit } => self.with_unit(unit, |u| systemctl(&["stop", "--no-block", u]).map(|_| JobAnswer::Done { detail: json!({"stopping": u}) })),
            JobOp::Release { unit } => self.with_unit(unit, |u| {
                let stopped = systemctl(&["stop", u]);
                let _ = systemctl(&["reset-failed", u]);
                Ok(JobAnswer::Done { detail: json!({"released": u, "stop": stopped.err()}) })
            }),
            JobOp::Reclaim { path } => self.reclaim_path(path),
        };
        r.unwrap_or_else(JobAnswer::Refused)
    }

    fn with_unit(&self, unit: &str, f: impl FnOnce(&str) -> Result<JobAnswer, String>) -> Result<JobAnswer, AdminRefusal> {
        if !valid_unit(unit) {
            return Err(refuse(code::BAD_ARGS, "not a K2 job unit"));
        }
        self.job_user()?;
        f(unit).map_err(|e| refuse(code::FAILED, e))
    }

    fn reclaim_path(&self, path: &Path) -> Result<JobAnswer, AdminRefusal> {
        self.job_user()?;
        let p = check_reclaim(&self.paths.home, path)?;
        let (uid, gid) = super::server::user_ids(&self.node_user()).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        let s = crate::wall::reclaim(&p, uid, gid).map_err(|e| refuse(code::FAILED, e))?;
        Ok(JobAnswer::Done { detail: json!({"reclaimed": p, "changed": s.changed, "skipped": s.skipped}) })
    }

    fn start_job(&self, s: &JobStart, fds: Vec<OwnedFd>) -> Result<String, AdminRefusal> {
        let (_, job_uid) = self.job_user()?;
        let unit = unit_name(&s.key).ok_or_else(|| refuse(code::BAD_ARGS, "job key is not a plain name"))?;
        let (root, rw) = check_paths(&self.paths.home, &s.root, &s.rw)?;
        let (_, group_gid) = super::server::user_ids(&self.node_user()).ok_or_else(|| refuse(code::NOT_CONFIGURED, "the node user is missing"))?;
        let [out, err]: [OwnedFd; 2] = fds.try_into().map_err(|_| refuse(code::BAD_ARGS, "start needs exactly the job's stdout and stderr"))?;
        let home = &self.paths.home;
        let plan = UnitPlan {
            unit: unit.clone(),
            job_uid,
            group_gid,
            root,
            rw,
            gone: vec![home.join("state"), home.join("run"), self.paths.config_dir.clone(), self.paths.state_dir.clone()],
            network_none: s.network_none,
            mem_bytes: s.mem_bytes,
            cpu_millis: s.cpu_millis,
            stop_grace_secs: s.stop_grace_secs,
            binary: self.paths.binary.clone(),
        };
        // A leftover unit of the same name (a node restart) goes first.
        if unit_status(&unit).is_ok_and(|st| st.loaded) {
            let _ = systemctl(&["stop", &unit]);
            let _ = systemctl(&["reset-failed", &unit]);
        }
        let argv = systemd_run_argv(&plan);
        let prog = find_program("systemd-run").ok_or_else(|| refuse(code::NOT_CONFIGURED, "systemd-run not found"))?;
        let mut child = std::process::Command::new(prog)
            .args(&argv[1..])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| refuse(code::FAILED, format!("systemd-run: {e}")))?;
        // `--wait` keeps systemd-run until the unit is released; started
        // = the unit is loaded. A refused start ends systemd-run early.
        let t0 = Instant::now();
        let started = loop {
            if let Ok(Some(st)) = child.try_wait() {
                if !unit_status(&unit).is_ok_and(|s| s.loaded) {
                    break Err(refuse(code::FAILED, format!("systemd-run ended before the job started ({st}); its message is in the job's log")));
                }
                break Ok(());
            }
            if unit_status(&unit).is_ok_and(|s| s.loaded) {
                break Ok(());
            }
            if t0.elapsed() > Duration::from_secs(15) {
                let _ = child.kill();
                break Err(refuse(code::FAILED, "the job unit didn't appear within 15 s"));
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        // Reap systemd-run whenever it ends (no zombies in the executor).
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let _ = self.audit().append(
            super::store::AuditEntry { event: "job".into(), detail: json!({"unit": unit, "network_none": s.network_none, "ok": started.is_ok()}), ..Default::default() },
            self.now(),
        );
        started.map(|_| unit)
    }

    /// `wall.enable` (owner-approved, Linux): what `sudo k2-node setup`
    /// does for the wall, for a node updated remotely: the `k2job` system
    /// user (no login, no home, its own group), a traverse-only ACL on the
    /// node home, and `jobs.toml`. k2-node picks it up when it restarts
    /// (propose `unit.restart unit=k2-node` after it, in the same plan).
    pub fn wall_enable(&self, args: &Value) -> Result<Value, AdminRefusal> {
        super::exec::no_args(args)?;
        if !cfg!(target_os = "linux") {
            return Err(refuse(code::BAD_ARGS, "macOS walls jobs with Seatbelt on its own; nothing to enable"));
        }
        let fail = |e: String| refuse(code::FAILED, e);
        let user = DEFAULT_JOB_USER;
        let mut created = false;
        if super::server::user_ids(user).is_none() {
            let nologin = ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("/bin/false");
            let useradd = find_program("useradd").ok_or_else(|| fail("useradd not found".into()))?;
            super::exec::run_fixed(
                &useradd.display().to_string(),
                &["--system", "--user-group", "--home-dir", "/nonexistent", "--no-create-home", "--shell", nologin, "--comment", "K2 compute jobs", user],
            )
            .map_err(fail)?;
            created = true;
        }
        let setfacl = find_program("setfacl").ok_or_else(|| refuse(code::NOT_CONFIGURED, "setfacl is missing: install the acl package, then try again"))?;
        let home = self.paths.home.display().to_string();
        super::exec::run_fixed(&setfacl.display().to_string(), &["-m", &format!("u:{user}:x"), &home]).map_err(fail)?;
        let msg = write_jobs_file(&self.paths.config_dir, user, &self.node_user()).map_err(fail)?;
        Ok(json!({"job_user": user, "created": created, "home_acl": format!("u:{user}:x {home}"), "jobs": msg, "restart_needed": "k2-node"}))
    }

    /// `homes.private` (owner-approved): chmod people's homes to owner-only.
    pub fn homes_private(&self, args: &Value) -> Result<Value, AdminRefusal> {
        super::exec::only_keys(args, &["homes"])?;
        let root = crate::wall::homes_root();
        let targets: Vec<PathBuf> = match args.get("homes") {
            None => crate::wall::open_homes_in(root).into_iter().map(|(p, _)| p).collect(),
            Some(v) => {
                let list = v.as_array().ok_or_else(|| refuse(code::BAD_ARGS, "homes is a list of paths"))?;
                let mut out = Vec::new();
                for h in list {
                    out.push(PathBuf::from(h.as_str().ok_or_else(|| refuse(code::BAD_ARGS, "homes is a list of paths"))?));
                }
                out
            }
        };
        let mut done = Vec::new();
        let mut errors = Vec::new();
        for h in &targets {
            match crate::wall::make_private(root, h) {
                Ok((was, now)) => done.push(json!({"home": h, "was": format!("{was:o}"), "now": format!("{now:o}")})),
                Err(e) => errors.push(json!({"home": h, "error": e})),
            }
        }
        if !errors.is_empty() && done.is_empty() {
            return Err(refuse(code::FAILED, format!("no home changed: {}", Value::Array(errors))));
        }
        Ok(json!({"made_private": done, "errors": errors}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> UnitPlan {
        UnitPlan {
            unit: "k2job-j1-g1.service".into(),
            job_uid: 990,
            group_gid: 991,
            root: PathBuf::from("/var/lib/k2node/jobs/j1-g1"),
            rw: vec![PathBuf::from("/var/lib/k2node/cache/pk/target-1")],
            gone: vec![PathBuf::from("/var/lib/k2node/state"), PathBuf::from("/etc/k2-node")],
            network_none: false,
            mem_bytes: Some(2 << 30),
            cpu_millis: Some(4000),
            stop_grace_secs: 10,
            binary: PathBuf::from("/usr/local/libexec/k2-node"),
        }
    }

    #[test]
    fn template_walls_the_job() {
        let a = systemd_run_argv(&plan());
        let has = |s: &str| a.iter().any(|x| x == s);
        assert_eq!(a[0], "systemd-run");
        for s in [
            "--unit=k2job-j1-g1.service",
            "--uid=990",
            "--gid=991",
            "--pipe",
            "--wait",
            "RemainAfterExit=yes",
            "ProtectHome=yes",
            "ProtectSystem=strict",
            "NoNewPrivileges=yes",
            "PrivateTmp=yes",
            "UMask=0002",
            "ProtectClock=yes",
            "MemoryMax=2147483648",
            "MemorySwapMax=0",
            "CPUQuota=400%",
            "TimeoutStopSec=10",
            "ReadWritePaths=/var/lib/k2node/jobs/j1-g1 /var/lib/k2node/cache/pk/target-1",
            "InaccessiblePaths=-/var/lib/k2node/state -/etc/k2-node",
        ] {
            assert!(has(s), "{s} missing from {a:?}");
        }
        assert!(!has("PrivateNetwork=yes"), "network = full");
        let tail = &a[a.len() - 4..];
        assert_eq!(tail, ["--", "/usr/local/libexec/k2-node", "job-exec", "/var/lib/k2node/jobs/j1-g1/exec/exec.json"], "root's binary runs root's shim; argv never on the command line");
        let mut p = plan();
        p.network_none = true;
        p.mem_bytes = None;
        p.cpu_millis = Some(5);
        let a = systemd_run_argv(&p);
        assert!(a.iter().any(|x| x == "PrivateNetwork=yes") && a.iter().any(|x| x == "IPAddressDeny=any"));
        assert!(a.iter().any(|x| x.contains("-/run/systemd/resolve")), "no DNS side channel");
        assert!(!a.iter().any(|x| x.starts_with("MemoryMax")));
        assert!(a.iter().any(|x| x == "CPUQuota=1%"), "never below 1%");
    }

    #[test]
    fn show_parsing_and_endings() {
        let running = parse_show("LoadState=loaded\nActiveState=active\nSubState=running\nResult=success\nExecMainCode=0\nExecMainStatus=0\n");
        assert!(running.loaded && !running.ended() && running.exit().is_none());
        let done = parse_show("LoadState=loaded\nActiveState=active\nSubState=exited\nResult=success\nExecMainCode=1\nExecMainStatus=0\n");
        assert!(done.ended());
        assert_eq!(done.exit(), Some((Some(0), None)));
        let failed = parse_show("LoadState=loaded\nActiveState=failed\nSubState=failed\nResult=exit-code\nExecMainCode=1\nExecMainStatus=3\n");
        assert_eq!(failed.exit(), Some((Some(3), None)));
        let oom = parse_show("LoadState=loaded\nActiveState=failed\nSubState=failed\nResult=oom-kill\nExecMainCode=2\nExecMainStatus=9\n");
        assert!(oom.ended() && oom.oom());
        assert_eq!(oom.exit(), Some((None, Some(9))));
        // A started unit systemd no longer knows was stopped and collected
        // (found on Ubuntu 24.04, systemd 255): ended, with no exit to tell
        // and never a made-up "success" exit code.
        let gone = parse_show("LoadState=not-found\nActiveState=inactive\nSubState=dead\nResult=success\nExecMainCode=0\nExecMainStatus=0\n");
        assert!(gone.ended());
        assert_eq!(gone.exit(), None);
        assert!(!gone.oom());
        assert_eq!(parse_oom_kills("low 0\nhigh 0\nmax 12\noom 1\noom_kill 2\noom_group_kill 0\n"), 2);
        assert_eq!(parse_oom_kills(""), 0);
        assert_eq!(systemd_version("systemd 255 (255.4-1ubuntu8.18)\n+PAM +AUDIT"), Some(255));
        assert_eq!(systemd_version("systemd 257.2-3-arch (257.2-3-arch)"), Some(257));
        assert_eq!(systemd_version("nonsense"), None);
    }

    #[test]
    fn unit_names_are_plain() {
        assert_eq!(unit_name("abc-g1").as_deref(), Some("k2job-abc-g1.service"));
        for bad in ["", "a/b", "a b", "../x", "a.service", "a;b"] {
            assert!(unit_name(bad).is_none(), "{bad}");
        }
        assert!(valid_unit("k2job-abc-g1.service"));
        for bad in ["sshd.service", "k2job-.service", "k2job-a/b.service", "k2-node.service", "k2job-a.socket"] {
            assert!(!valid_unit(bad), "{bad}");
        }
    }

    #[test]
    fn paths_are_confined_after_resolving_links() {
        let home = crate::util::temp_dir("jobpaths");
        for d in ["jobs/j1-g1/src", "cache/pk/target-1", "state", "etc"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        let (r, rw) = check_paths(&home, &home.join("jobs/j1-g1"), &[home.join("jobs/j1-g1/src"), home.join("cache/pk/target-1")]).unwrap();
        assert_eq!(r, home.join("jobs/j1-g1"));
        assert_eq!(rw.len(), 2);
        // A symlink in a node-writable dir can't widen the job.
        std::os::unix::fs::symlink(home.join("state"), home.join("cache/pk/sneaky")).unwrap();
        std::os::unix::fs::symlink(home.join("etc"), home.join("jobs/j1-g1/out")).unwrap();
        for bad in [home.join("cache/pk/sneaky"), home.join("jobs/j1-g1/out"), home.join("state"), home.join("cache"), PathBuf::from("/etc")] {
            assert!(check_paths(&home, &home.join("jobs/j1-g1"), std::slice::from_ref(&bad)).is_err(), "{}", bad.display());
        }
        assert!(check_paths(&home, &home.join("jobs"), &[]).is_err(), "jobs/ itself isn't a job root");
        assert!(check_paths(&home, &home.join("jobs/j1-g1/src"), &[]).is_err(), "one level under jobs/");
        assert!(check_paths(&home, &home.join("state"), &[]).is_err());
        assert!(check_reclaim(&home, &home.join("jobs/j1-g1")).is_ok());
        assert!(check_reclaim(&home, &home.join("cache/pk/target-1")).is_ok());
        for bad in [home.join("jobs"), home.join("cache"), home.join("state"), home.join("cache/pk/sneaky")] {
            assert!(check_reclaim(&home, &bad).is_err(), "{}", bad.display());
        }
    }

    #[test]
    fn job_user_is_never_root_or_the_node_user() {
        assert!(check_job_user("root", "k2node").is_err());
        assert!(check_job_user("no-such-user-k2x", "k2node").is_err());
        assert!(check_job_user("a b", "k2node").is_err());
        let me = crate::node::current_user();
        assert!(check_job_user(&me, &me).is_err(), "the node user can't be the job user");
    }

    #[test]
    fn jobs_file_round_trip_and_strict() {
        let d = crate::util::temp_dir("jobsfile");
        assert_eq!(read_jobs_file(&d).unwrap(), None);
        std::fs::write(d.join(JOBS_FILE), "job_user = \"k2job\"\n").unwrap();
        assert_eq!(read_jobs_file(&d).unwrap(), Some(JobsFile { job_user: "k2job".into() }));
        std::fs::write(d.join(JOBS_FILE), "job_user = \"k2job\"\nextra = 1\n").unwrap();
        assert!(read_jobs_file(&d).is_err(), "fail closed");
    }

    #[test]
    fn ops_round_trip_on_the_wire() {
        let ops = [
            JobOp::Ready,
            JobOp::Start(JobStart { key: "j-g1".into(), root: "/r".into(), rw: vec![], network_none: true, mem_bytes: Some(1), cpu_millis: None, stop_grace_secs: 10 }),
            JobOp::Status { unit: "k2job-j-g1.service".into() },
            JobOp::Reclaim { path: "/r".into() },
        ];
        for op in ops {
            let s = serde_json::to_string(&op).unwrap();
            assert_eq!(serde_json::from_str::<JobOp>(&s).unwrap(), op);
        }
    }
}
