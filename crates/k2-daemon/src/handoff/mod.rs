//! Front-door hand-off — no-downtime daemon upgrades (runtime stage H,
//! `prd-k2-app-runtime-v1` §7.6, vs-live AR3/K37).
//!
//! ## Pattern (why this one)
//!
//! Three known ways to restart a server without dropping connections:
//!
//! - **systemd socket activation** (`.socket` units, `LISTEN_FDS`): the
//!   listening sockets outlive the process, so a restart refuses nothing —
//!   but systemd stops the old process before it starts the new one, so
//!   every *established* connection (a TLS-terminated App splice, a
//!   WebSocket) still dies. It needs root-owned `.socket` units, and does
//!   nothing on launchd or a supervisor-less box.
//! - **Pingora** (`SCM_RIGHTS` over an upgrade socket): an operator starts
//!   the new binary with `--upgrade`, signals the old one, which sends its
//!   listeners and drains. Keeps established connections, but needs an
//!   outside actor to start the successor in the right order.
//! - **shellflip** (Cloudflare): the old process starts the successor
//!   itself, passes the listeners over an inherited socketpair
//!   (`SCM_RIGHTS`), waits for it to come up, tells systemd the new main
//!   pid, and drains.
//!
//! K2 uses the **shellflip** shape: it keeps established splices alive
//! (the old daemon drains them), the daemon can drive it from its own
//! self-update path with no operator, and it works the same under a
//! system unit, a user unit (Arch / `k2 daemon install`) and no supervisor
//! at all. Implemented in-tree (~libc `sendmsg`/`recvmsg`) instead of the
//! crate so it fits the daemon's boot order (the singleton lock and the
//! listeners are handed over before the DB opens).
//!
//! ## Sequence
//!
//! ```text
//!  old daemon (O)                              successor (N)
//!  spawn N (setsid, fd 3 = socketpair end) ──▶ boots to just before the
//!                                              singleton lock
//!                                    ◀── HELLO {pid, version}
//!  quiesce: accept loops stop (sockets stay
//!  open, the kernel queues new connections),
//!  drain flag on (no PTY/frpc/service spawn),
//!  in-process agent PTYs killed, k2-ptyd
//!  agents detached (they keep running;
//!  N adopts them at B1), frpc detached,
//!  DB query_only, Thread store (redb,
//!  exclusive file lock) closed
//!  TAKE {roles} + SCM_RIGHTS [lock, listeners] ──▶ adopts the singleton
//!                                              lock + the listeners
//!                                    ◀── ACK
//!  sd_notify MAINPID=N (systemd), verify
//!  main runtime shut down (every loop stops)
//!  pump live splices on the splice runtime     serves new connections
//!  until none left or the deadline → exit      adopts frpc + published
//!                                              services and ptyd agents,
//!                                              resumes in-process agents
//! ```
//!
//! A failure before TAKE leaves O serving as if nothing happened (N is
//! killed). A failure after quiesce makes O exit for a plain supervisor
//! restart.
//!
//! ## Supervisors
//!
//! - **systemd**: needs `NotifyAccess=main` on the unit (the root helper's
//!   `daemon handoff-dropin`, or the provisioning templates). Without it
//!   the `MAINPID=` message is dropped and O's exit would restart the unit
//!   under N, so [`capable`] refuses. `NotifyAccess=all` is never used: any
//!   process in the unit (an agent) could then claim the main pid and pass
//!   the root helper's daemon-caller check.
//! - **launchd** (macOS): there is no way to move a job's pid; O's exit
//!   would relaunch the job against N's singleton lock. Refused; macOS
//!   keeps the plain restart (launchd already leaves frpc, published
//!   services and skin helpers running across it).
//! - **none** (dev, tests): works as is.

#[cfg(unix)]
mod fdpass;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use k2_core::log_debug;

/// The env var naming the inherited socketpair fd in the successor.
pub const ENV_FD: &str = "K2_HANDOFF_FD";
/// The fd number the socketpair end lands on in the successor.
#[cfg(unix)]
const CHILD_FD: i32 = 3;
/// Default drain deadline for the old daemon (PRD: 10 min).
pub const DEFAULT_DRAIN_SECS: u64 = 600;
/// Override (tests, ops): seconds.
pub const DRAIN_ENV: &str = "K2_HANDOFF_DRAIN_SECS";

/// The listening sockets a hand-off carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// The main HTTP listener (`daemon.port`).
    Main,
    /// The tunnel-ingress listener (frpc's cleartext target / the E2E
    /// listener's decrypted splice target).
    TunnelIngress,
    /// The E2E TLS listener frpc dials (`tunnel-https.port`).
    TunnelTls,
    /// front443's box :443 socket (0.46.1). systemd passes it at a unit
    /// start (`LISTEN_FDS`); a hand-off passes it on from daemon to daemon.
    /// Only sent to a successor that names it in its HELLO (an older
    /// binary can't parse it; systemd keeps its own copy of the socket, so
    /// :443 stays bound and queues until a daemon that knows it starts).
    Front443,
    /// The App TLS door (0.46.6, vs-live CD-A7): loopback `127.0.0.1:38474`,
    /// where front443 routes custom App names. Only sent to a successor
    /// that names it in its HELLO.
    AppDoor,
}

impl Role {
    /// The roles every hand-off-capable daemon understands (0.46.0).
    pub const LEGACY: [Role; 3] = [Role::Main, Role::TunnelIngress, Role::TunnelTls];
    /// Every role this build understands.
    pub const ALL: [Role; 5] = [Role::Main, Role::TunnelIngress, Role::TunnelTls, Role::Front443, Role::AppDoor];
}

/// The roles to send a successor that announced `accepts` (`None`: a
/// pre-0.46.1 successor, which understands only [`Role::LEGACY`]).
pub fn roles_for_successor(held: &[Role], accepts: Option<&[Role]>) -> Vec<Role> {
    let ok = |r: &Role| match accepts {
        Some(a) => a.contains(r),
        None => Role::LEGACY.contains(r),
    };
    held.iter().copied().filter(ok).collect()
}

// ── Successor side: what the predecessor handed over ────────────────────

fn inherited() -> &'static Mutex<HashMap<Role, std::net::TcpListener>> {
    static M: OnceLock<Mutex<HashMap<Role, std::net::TcpListener>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Take the listener handed over for `role` (once). `None` on a normal
/// boot, or once taken.
pub fn take_inherited(role: Role) -> Option<std::net::TcpListener> {
    inherited()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&role)
}

/// Close every handed-over listener nobody took (E2E turned off since the
/// predecessor bound it): a socket left open with no accept loop would let
/// frpc connect and then hang.
pub fn drop_untaken() {
    let left: Vec<Role> = {
        let mut m = inherited().lock().unwrap_or_else(|p| p.into_inner());
        let roles = m.keys().copied().collect();
        m.clear();
        roles
    };
    if !left.is_empty() {
        log_debug!("[daemon/handoff] closed handed-over listener(s) this daemon does not serve: {left:?}");
    }
}

/// A listener for `role` is waiting to be taken.
pub fn has_inherited(role: Role) -> bool {
    inherited()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains_key(&role)
}

// ── App listeners (prd-app-runtime-update-continuity-v1 R19, R36, R37) ──

/// A per-App listener a hand-off carries (0.46.4): the App's front port
/// (Meet, Caddy names, local users) and its egress proxy. The gateway link
/// closes at quiesce and re-opens on adopt, as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    Front,
    Egress,
}

/// One App listener in a TAKE's follow-up message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppSock {
    pub app: String,
    pub kind: AppKind,
}

/// App fds per follow-up message (the receiver reads at most this many).
pub const APP_FDS_PER_MSG: usize = 8;

type AppKey = (String, AppKind);

fn live_apps() -> &'static Mutex<HashMap<AppKey, Weak<tokio::net::TcpListener>>> {
    static M: OnceLock<Mutex<HashMap<AppKey, Weak<tokio::net::TcpListener>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record an App's live listener (a re-bind replaces it).
pub fn register_app(app: &str, kind: AppKind, l: &Arc<tokio::net::TcpListener>) {
    live_apps().lock().unwrap_or_else(|p| p.into_inner()).insert((app.to_string(), kind), Arc::downgrade(l));
}

fn inherited_apps() -> &'static Mutex<HashMap<AppKey, std::net::TcpListener>> {
    static M: OnceLock<Mutex<HashMap<AppKey, std::net::TcpListener>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// R19: take the listener the predecessor handed over for an App (once).
/// `None` on a normal boot, from a predecessor that didn't carry it, or once
/// taken.
pub fn take_inherited_app(app: &str, kind: AppKind) -> Option<std::net::TcpListener> {
    inherited_apps().lock().unwrap_or_else(|p| p.into_inner()).remove(&(app.to_string(), kind))
}

/// Tests: as if a predecessor had handed this listener over.
#[cfg(test)]
pub(crate) fn inherit_app_for_test(app: &str, kind: AppKind, l: std::net::TcpListener) {
    inherited_apps().lock().unwrap_or_else(|p| p.into_inner()).insert((app.to_string(), kind), l);
}

static PREDECESSOR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The pid of the daemon this one took over from (its parent at hand-off
/// time), when it booted from a hand-off.
pub fn predecessor_pid() -> Option<u32> {
    match PREDECESSOR.load(Ordering::SeqCst) {
        0 | 1 => None,
        p => Some(p),
    }
}

/// R36: the predecessor still runs (and may hold App ports while it drains).
pub fn predecessor_alive() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 only checks the pid.
        predecessor_pid().is_some_and(|p| unsafe { libc::kill(p as i32, 0) } == 0)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

static WAS_HANDED_OFF: AtomicBool = AtomicBool::new(false);

/// This daemon booted from a hand-off (it received the predecessor's
/// sockets).
pub fn booted_from_handoff() -> bool {
    WAS_HANDED_OFF.load(Ordering::SeqCst)
}

// ── Predecessor side: the live listeners and the quiesce signal ─────────

fn live() -> &'static Mutex<HashMap<Role, Weak<tokio::net::TcpListener>>> {
    static M: OnceLock<Mutex<HashMap<Role, Weak<tokio::net::TcpListener>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record the live listener for `role` (a re-bind replaces it). Weak: a
/// listener whose accept loop died must close, not linger in the registry
/// accepting connections nobody serves.
pub fn register(role: Role, l: &Arc<tokio::net::TcpListener>) {
    live()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(role, Arc::downgrade(l));
}

fn quiesce_tx() -> &'static tokio::sync::watch::Sender<bool> {
    static TX: OnceLock<tokio::sync::watch::Sender<bool>> = OnceLock::new();
    TX.get_or_init(|| tokio::sync::watch::channel(false).0)
}

/// Resolves once the daemon stops accepting (hand-off). Every accept
/// loop selects on it next to `accept()`, and stops WITHOUT closing the
/// socket it holds an `Arc` of (the hand-off code holds its own).
pub async fn quiesced() {
    let mut rx = quiesce_tx().subscribe();
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

pub fn is_quiesced() -> bool {
    *quiesce_tx().borrow()
}

// ── The splice runtime and the drain count ──────────────────────────────

/// A runtime of its own for the connections that must outlive a hand-off
/// (E2E TLS splices: published services, skin gateway, App links). The
/// main runtime is shut down when the daemon hands off — every loop and
/// every daemon-served connection stops — while this one keeps pumping
/// the splices it owns until they close.
pub fn splice_handle() -> tokio::runtime::Handle {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("k2-splice")
            .enable_all()
            .build()
            .expect("build the splice runtime")
    })
    .handle()
    .clone()
}

static LIVE_SPLICES: AtomicUsize = AtomicUsize::new(0);

/// Held by a splice for its whole life; the old daemon drains until none
/// are left.
pub struct SpliceGuard(());

impl Drop for SpliceGuard {
    fn drop(&mut self) {
        LIVE_SPLICES.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn splice_guard() -> SpliceGuard {
    LIVE_SPLICES.fetch_add(1, Ordering::SeqCst);
    SpliceGuard(())
}

pub fn live_splices() -> usize {
    LIVE_SPLICES.load(Ordering::SeqCst)
}

static LIVE_APP_CONNS: AtomicUsize = AtomicUsize::new(0);

/// R37 (update continuity, 0.46.4): held by a connection on an App's front
/// port or egress proxy (their own runtimes outlive the main one); the old
/// daemon drains these too before it exits.
pub struct AppConnGuard(());

impl Drop for AppConnGuard {
    fn drop(&mut self) {
        LIVE_APP_CONNS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn app_conn_guard() -> AppConnGuard {
    LIVE_APP_CONNS.fetch_add(1, Ordering::SeqCst);
    AppConnGuard(())
}

pub fn live_app_conns() -> usize {
    LIVE_APP_CONNS.load(Ordering::SeqCst)
}

// ── Supervisor glue: NOTIFY_SOCKET and the boot executable ──────────────

static NOTIFY_SOCKET: OnceLock<Option<std::ffi::OsString>> = OnceLock::new();
static BOOT_EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
static HANDOFF_FD: OnceLock<Option<std::ffi::OsString>> = OnceLock::new();

/// Call once at process start, before any thread or child exists: keep
/// systemd's `NOTIFY_SOCKET` for the hand-off and REMOVE it from the
/// environment, so no agent, terminal or published service inherits a
/// path to the unit's notify socket. Also records the executable path
/// (after a self-update swap, `/proc/self/exe` names a deleted inode; the
/// successor must exec the path).
pub fn capture_process_context() {
    let ns = std::env::var_os("NOTIFY_SOCKET");
    if ns.is_some() {
        std::env::remove_var("NOTIFY_SOCKET");
    }
    // front443: systemd's passed :443 socket (LISTEN_FDS), taken while
    // still single-threaded; the variables leave the environment too.
    crate::front443::systemd_fds::capture();
    let _ = NOTIFY_SOCKET.set(ns);
    let _ = BOOT_EXE.set(std::env::current_exe().ok());
    // A successor's socketpair fd: read (and drop from the env, so no child
    // inherits the name) while this process is still single-threaded.
    let fd = std::env::var_os(ENV_FD);
    if fd.is_some() {
        std::env::remove_var(ENV_FD);
    }
    let _ = HANDOFF_FD.set(fd);
}

/// The real uid, for systemd's `/run/user/<uid>` (systemd is Linux-only;
/// elsewhere `under_systemd()` is false before this is reached).
fn systemd_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getuid has no preconditions.
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// systemd's notify socket for this unit: the one the process started
/// with, or — when the hand-off drop-in was installed after this process
/// started (no `NOTIFY_SOCKET` in its env then) and the unit now reports
/// `NotifyAccess=main` — the manager's fixed socket.
fn notify_socket() -> Option<std::ffi::OsString> {
    if let Some(s) = NOTIFY_SOCKET.get().cloned().flatten() {
        return Some(s);
    }
    if !under_systemd() {
        return None;
    }
    match systemctl_show("NotifyAccess").as_deref() {
        Some("main") | Some("exec") | Some("all") => {}
        _ => return None,
    }
    let (_user, _unit, scope) = crate::root_helper::own_unit();
    let path = if scope == "user" {
        let rt = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", systemd_uid())));
        rt.join("systemd").join("notify")
    } else {
        PathBuf::from("/run/systemd/notify")
    };
    path.exists().then(|| path.into_os_string())
}

/// The path this daemon was started from.
pub fn boot_exe() -> Option<PathBuf> {
    BOOT_EXE.get().cloned().flatten()
}

fn under_systemd() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

fn under_launchd() -> bool {
    cfg!(target_os = "macos")
        && std::env::var("XPC_SERVICE_NAME")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
}

/// `systemctl [--user] show -p <prop> --value <unit>`.
fn systemctl_show(prop: &str) -> Option<String> {
    let (_user, unit, scope) = crate::root_helper::own_unit();
    let mut cmd = std::process::Command::new("systemctl");
    if scope == "user" {
        cmd.arg("--user");
    }
    cmd.args(["show", "-p", prop, "--value", &unit]);
    let out = cmd.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The marker of the unit line that lets systemd stop a hand-off successor
/// gracefully: `ExecStop=/bin/sh -c '…kill -TERM "$MAINPID"; wait…' k2-wait-mainpid`.
pub const WAIT_MAINPID_MARKER: &str = "k2-wait-mainpid";

/// The `ExecStop=` line every K2 daemon unit carries (and what the marker
/// stands for). A hand-off successor is not systemd's child, so systemd
/// can't wait for it: a stop or restart sends SIGTERM and SIGKILL at once
/// (journal: "Supervising process … which is not our child", then "Killing
/// … with signal SIGKILL"; systemd 255, MAINPIDFD makes no difference). The
/// successor never runs its shutdown — no k2-ptyd stop-vs-restart decision,
/// so a stop leaves agents running, no saved screens, frpc not released.
/// systemd DOES wait for its own ExecStop process: this one SIGTERMs the
/// main process and waits until it is gone, for a child or not.
pub const EXEC_STOP_WAIT_MAINPID: &str =
    "ExecStop=/bin/sh -c 'kill -TERM \"$$MAINPID\" 2>/dev/null; while kill -0 \"$$MAINPID\" 2>/dev/null; do sleep 0.2; done' k2-wait-mainpid";

/// Pure part of [`unit_waits_for_main`]: the unit's `ExecStop` property
/// carries the marker.
pub fn exec_stop_waits_for_main(exec_stop_prop: &str) -> bool {
    exec_stop_prop.contains(WAIT_MAINPID_MARKER)
}

/// The unit's ExecStop waits for the main process (see
/// [`EXEC_STOP_WAIT_MAINPID`]). Re-read on every call (unit files change).
fn unit_waits_for_main() -> bool {
    systemctl_show("ExecStop").is_some_and(|v| exec_stop_waits_for_main(&v))
}

/// Under systemd, a hand-off needs a unit that waits for its successor at
/// stop ([`EXEC_STOP_WAIT_MAINPID`]); without it a hand-off request
/// (`systemctl reload` → SIGUSR2) is a plain graceful restart instead: this
/// daemon exits as for any restart (k2-ptyd sessions are detached and kept,
/// §7.6 — a reload job is not a stop), `Restart=always` (every shipped
/// unit) starts the binary at the unit's path again, and it adopts the
/// agents. Only frpc/Caddy/App listeners restart; ptyd still holds the
/// main listener, so agents' calls queue (S5).
pub fn plain_restart_instead() -> bool {
    under_systemd() && !unit_waits_for_main()
}

/// Can this daemon hand off right now? `Err` says why not (and what fixes
/// it). The caller then falls back to a plain restart.
pub fn capable() -> Result<(), String> {
    if !cfg!(unix) {
        return Err("hand-off needs a Unix daemon".into());
    }
    if IN_PROGRESS.load(Ordering::SeqCst) {
        return Err("a hand-off is already in progress".into());
    }
    if k2_core::handoff_state::draining() {
        return Err("this daemon already handed off".into());
    }
    if under_launchd() {
        return Err(
            "launchd cannot move a job to a new process — macOS keeps the plain restart \
             (frpc, published services and skin helpers already survive it there)"
                .into(),
        );
    }
    if plain_restart_instead() {
        return Err(format!(
            "the systemd unit has no ExecStop that waits for the main process ({WAIT_MAINPID_MARKER}), so it \
             couldn't stop a hand-off successor gracefully — a plain restart instead (agents keep running in k2-ptyd)"
        ));
    }
    if under_systemd() && notify_socket().is_none() {
        return Err(
            "the systemd unit has no NotifyAccess=main, so the new daemon cannot become the \
             unit's main process — install the hand-off drop-in (`POST \
             /cli/daemon/handoff/setup`; root helper `daemon handoff-dropin install`)"
                .into(),
        );
    }
    if boot_exe().is_none() {
        return Err("the daemon's executable path is unknown".into());
    }
    Ok(())
}

static IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static HANDED_OFF: AtomicBool = AtomicBool::new(false);

/// This daemon completed a hand-off and now only drains.
pub fn handed_off() -> bool {
    HANDED_OFF.load(Ordering::SeqCst)
}

// ── Wire protocol ───────────────────────────────────────────────────────

// Windows has no hand-off yet; the message shape is still tested there.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum Msg {
    Hello {
        pid: u32,
        version: String,
        /// The roles this successor can take (0.46.1+). Absent from an
        /// older successor: it gets [`Role::LEGACY`] only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roles: Option<Vec<Role>>,
        /// 0.46.4 (R19): this successor takes per-App listeners after the
        /// ACK. Absent from older successors: they get none (the
        /// predecessor closes its copies at quiesce, R37).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        app_listeners: bool,
    },
    /// Followed in the same `sendmsg` by the fds: the singleton lock
    /// first, then one listener per role, in `roles` order.
    Take {
        roles: Vec<Role>,
        /// front443's live route table, when `Front443` is among `roles`:
        /// the successor routes from its first connection (it boots the
        /// front before its DB is open). Ignored by older successors.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        front_routes: Option<crate::front443::routes::RouteTable>,
        /// 0.46.4 (R19): App listeners follow the ACK (`Apps` messages of
        /// at most [`APP_FDS_PER_MSG`] fds, then `AppsDone`). Absent from an
        /// older predecessor: the successor waits for nothing.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        apps_follow: bool,
    },
    Ack,
    /// R19: these App listeners, one fd each, in order.
    Apps { apps: Vec<AppSock> },
    AppsDone,
}

/// R19, predecessor: send every App listener in messages of at most
/// [`APP_FDS_PER_MSG`] fds, then `AppsDone`. Returns how many went.
#[cfg(unix)]
fn send_app_listeners(sock: &std::os::unix::net::UnixStream, held: &[(AppSock, Arc<tokio::net::TcpListener>)]) -> Result<usize, String> {
    use std::os::fd::AsRawFd;
    for chunk in held.chunks(APP_FDS_PER_MSG) {
        let apps: Vec<AppSock> = chunk.iter().map(|(s, _)| s.clone()).collect();
        let fds: Vec<i32> = chunk.iter().map(|(_, l)| l.as_raw_fd()).collect();
        let body = serde_json::to_vec(&Msg::Apps { apps }).map_err(|e| e.to_string())?;
        fdpass::send_msg(sock, &body, &fds).map_err(|e| format!("send Apps: {e}"))?;
    }
    fdpass::send_msg(sock, &serde_json::to_vec(&Msg::AppsDone).expect("apps done"), &[]).map_err(|e| format!("send AppsDone: {e}"))?;
    Ok(held.len())
}

/// R19, successor: read `Apps` messages until `AppsDone` and keep each
/// listener for [`take_inherited_app`]. Returns how many arrived.
#[cfg(unix)]
fn receive_app_listeners(sock: &std::os::unix::net::UnixStream) -> Result<usize, String> {
    let mut n = 0usize;
    loop {
        let (body, fds) = fdpass::recv_msg(sock, APP_FDS_PER_MSG).map_err(|e| format!("receive Apps: {e}"))?;
        match serde_json::from_slice::<Msg>(&body) {
            Ok(Msg::Apps { apps }) => {
                if apps.len() != fds.len() {
                    return Err(format!("Apps named {} listeners and carried {} fds", apps.len(), fds.len()));
                }
                let mut m = inherited_apps().lock().unwrap_or_else(|p| p.into_inner());
                for (s, fd) in apps.into_iter().zip(fds) {
                    m.insert((s.app, s.kind), std::net::TcpListener::from(fd));
                    n += 1;
                }
            }
            Ok(Msg::AppsDone) => return Ok(n),
            Ok(other) => return Err(format!("expected Apps, got {other:?}")),
            Err(e) => return Err(format!("unreadable Apps: {e}")),
        }
    }
}

/// How long the predecessor waits for the successor's HELLO (it boots to
/// just before the singleton lock first: login-shell PATH capture,
/// migrations of the home dir).
#[cfg(unix)]
const HELLO_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(unix)]
const ACK_TIMEOUT: Duration = Duration::from_secs(30);

// ── Successor: receive ──────────────────────────────────────────────────

/// Called at boot right before the singleton lock. When this process was
/// started by a hand-off (`K2_HANDOFF_FD`), say HELLO, receive the lock
/// and the listeners, store the listeners for [`take_inherited`], ACK, and
/// return the lock file (already locked: same open file description as the
/// predecessor's). `Ok(None)` on a normal boot.
pub fn successor_receive() -> Result<Option<std::fs::File>, String> {
    let Some(raw) = HANDOFF_FD.get().cloned().flatten() else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        successor_receive_unix(&raw.to_string_lossy()).map(Some)
    }
    #[cfg(not(unix))]
    {
        let _ = raw;
        Err("hand-off is Unix-only".into())
    }
}

#[cfg(unix)]
fn successor_receive_unix(fd_str: &str) -> Result<std::fs::File, String> {
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream;
    let fd: i32 = fd_str
        .trim()
        .parse()
        .map_err(|_| format!("{ENV_FD}={fd_str:?} is not an fd"))?;
    // SAFETY: the predecessor put the socketpair end on this fd for us.
    let sock = unsafe { UnixStream::from_raw_fd(fd) };
    fdpass::set_cloexec(fd);
    sock.set_read_timeout(Some(ACK_TIMEOUT * 2))
        .map_err(|e| format!("hand-off socket timeout: {e}"))?;
    fdpass::send_msg(
        &sock,
        &serde_json::to_vec(&Msg::Hello {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            roles: Some(Role::ALL.to_vec()),
            app_listeners: true,
        })
        .expect("serialize hello"),
        &[],
    )
    .map_err(|e| format!("send HELLO: {e}"))?;
    let (body, mut fds) = fdpass::recv_msg(&sock, 8).map_err(|e| format!("receive TAKE: {e}"))?;
    let (roles, apps_follow) = match serde_json::from_slice::<Msg>(&body) {
        Ok(Msg::Take { roles, front_routes, apps_follow }) => {
            if let Some(t) = front_routes {
                crate::front443::routes::set_handed(t);
            }
            (roles, apps_follow)
        }
        Ok(other) => return Err(format!("expected TAKE, got {other:?}")),
        Err(e) => return Err(format!("unreadable TAKE: {e}")),
    };
    if fds.len() != roles.len() + 1 {
        return Err(format!(
            "TAKE carried {} fds for {} roles + the lock",
            fds.len(),
            roles.len()
        ));
    }
    let lock = fds.remove(0);
    let lock = std::fs::File::from(lock);
    {
        let mut m = inherited().lock().unwrap_or_else(|p| p.into_inner());
        for (role, fd) in roles.iter().zip(fds) {
            let l = std::net::TcpListener::from(fd);
            log_debug!(
                "[daemon/handoff] inherited {role:?} listener {:?}",
                l.local_addr().ok()
            );
            m.insert(*role, l);
        }
    }
    WAS_HANDED_OFF.store(true, Ordering::SeqCst);
    // R36: the predecessor is our parent until it exits (it spawned us).
    // SAFETY: getppid has no preconditions.
    PREDECESSOR.store(unsafe { libc::getppid() } as u32, Ordering::SeqCst);
    fdpass::send_msg(&sock, &serde_json::to_vec(&Msg::Ack).expect("ack"), &[])
        .map_err(|e| format!("send ACK: {e}"))?;
    log_debug!("[daemon/handoff] took over from the previous daemon ({} listeners)", roles.len());
    // R19: the App listeners follow. Never fatal: without them each App
    // keeps its stored port by retrying the bind (R36).
    if apps_follow {
        match receive_app_listeners(&sock) {
            Ok(n) => log_debug!("[daemon/handoff] inherited {n} App listener(s)"),
            Err(e) => log_debug!("[daemon/handoff] WARN: App listeners: {e} (each App re-binds its stored port)"),
        }
    }
    Ok(lock)
}

// ── Predecessor: run ────────────────────────────────────────────────────

/// Where a failed hand-off left this daemon.
#[derive(Debug)]
pub enum Failed {
    /// Nothing changed; this daemon keeps serving.
    Untouched(String),
    /// The daemon had already quiesced; it must exit for a plain restart.
    Quiesced(String),
}

/// Start a hand-off to `exe` (the binary now at the boot path, after a
/// self-update swap, or the same binary). Returns at once; the work runs
/// on its own task. On success the daemon's shutdown is triggered in
/// drain mode; on a failure after quiesce, a plain shutdown (the
/// supervisor restarts it).
pub fn start(exe: PathBuf, shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>) -> Result<(), String> {
    capable()?;
    if IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Err("a hand-off is already in progress".into());
    }
    tokio::spawn(async move {
        let res = tokio::task::spawn_blocking(move || run_blocking(&exe)).await;
        let res = match res {
            Ok(r) => r,
            Err(e) => Err(Failed::Untouched(format!("hand-off task: {e}"))),
        };
        match res {
            Ok(pid) => {
                log_debug!(
                    "[daemon/handoff] successor pid {pid} owns the front door; draining {} live \
                     splice(s)",
                    live_splices()
                );
                if let Some(tx) = shutdown_tx {
                    let _ = tx.send(());
                }
            }
            Err(Failed::Untouched(e)) => {
                log_debug!("[daemon/handoff] FAILED before taking anything over — still serving: {e}");
                IN_PROGRESS.store(false, Ordering::SeqCst);
            }
            Err(Failed::Quiesced(e)) => {
                log_debug!(
                    "[daemon/handoff] FAILED after quiescing — exiting for a plain supervisor \
                     restart: {e}"
                );
                if let Some(tx) = shutdown_tx {
                    let _ = tx.send(());
                }
            }
        }
    });
    Ok(())
}

#[cfg(not(unix))]
fn run_blocking(_exe: &std::path::Path) -> Result<u32, Failed> {
    Err(Failed::Untouched("hand-off is Unix-only".into()))
}

#[cfg(unix)]
fn run_blocking(exe: &std::path::Path) -> Result<u32, Failed> {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;

    let (ours, theirs) =
        UnixStream::pair().map_err(|e| Failed::Untouched(format!("socketpair: {e}")))?;
    let theirs_fd = theirs.as_raw_fd();
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1))
        .env(ENV_FD, CHILD_FD.to_string())
        .stdin(std::process::Stdio::null());
    if let Some(ns) = notify_socket() {
        // Only the successor gets it back (children of this daemon never
        // had it): it becomes the unit's main process and may hand off
        // again.
        cmd.env("NOTIFY_SOCKET", ns);
    }
    // SAFETY: dup2/setsid/fcntl are async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            // Its own session: the root helper's caller check wants the
            // unit's main process to be a session leader whose sudo calls
            // share its session.
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::dup2(theirs_fd, CHILD_FD) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(CHILD_FD, libc::F_GETFD);
            if flags == -1 || libc::fcntl(CHILD_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| Failed::Untouched(format!("spawn successor {}: {e}", exe.display())))?;
    drop(theirs);
    let pid = child.id();
    log_debug!("[daemon/handoff] started successor pid {pid} ({})", exe.display());
    let abort = |child: &mut std::process::Child, why: String| -> Failed {
        let _ = child.kill();
        let _ = child.wait();
        Failed::Untouched(why)
    };

    // HELLO
    if let Err(e) = ours.set_read_timeout(Some(HELLO_TIMEOUT)) {
        return Err(abort(&mut child, format!("socket timeout: {e}")));
    }
    let hello = fdpass::recv_msg(&ours, 0).map_err(|e| format!("waiting for HELLO: {e}"));
    let (version, accepts, takes_apps) = match hello.and_then(|(b, _)| {
        serde_json::from_slice::<Msg>(&b).map_err(|e| format!("unreadable HELLO: {e}"))
    }) {
        Ok(Msg::Hello { pid: p, version, roles, app_listeners }) if p == pid => (version, roles, app_listeners),
        Ok(other) => return Err(abort(&mut child, format!("expected HELLO from {pid}, got {other:?}"))),
        Err(e) => return Err(abort(&mut child, e)),
    };
    log_debug!("[daemon/handoff] successor {pid} (k2-daemon {version}) is ready for the sockets");

    // Collect the live listeners (strong refs from here: the accept loops
    // drop theirs when they quiesce).
    let held: Vec<(Role, Arc<tokio::net::TcpListener>)> = {
        let m = live().lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<_> = m
            .iter()
            .filter_map(|(r, w)| w.upgrade().map(|l| (*r, l)))
            .collect();
        v.sort_by_key(|(r, _)| *r as u8);
        // An older successor can't parse a role it doesn't know: send it
        // only what it announced (or the 0.46.0 set).
        let send = roles_for_successor(&v.iter().map(|(r, _)| *r).collect::<Vec<_>>(), accepts.as_deref());
        v.retain(|(r, _)| send.contains(r));
        v
    };
    if !held.iter().any(|(r, _)| *r == Role::Main) {
        return Err(abort(&mut child, "the main listener is not registered".into()));
    }
    let Some(lock_fd) = crate::daemon_main::singleton_lock_fd() else {
        return Err(abort(&mut child, "this daemon holds no singleton lock".into()));
    };
    // R19: the App listeners, strong refs before quiesce (their accept
    // loops drop theirs when they stop, R37). An older successor gets none:
    // the sockets close at quiesce and it binds them fresh.
    let app_held: Vec<(AppSock, Arc<tokio::net::TcpListener>)> = if takes_apps {
        let m = live_apps().lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<_> = m.iter().filter_map(|((app, kind), w)| w.upgrade().map(|l| (AppSock { app: app.clone(), kind: *kind }, l))).collect();
        v.sort_by(|a, b| (a.0.app.as_str(), a.0.kind as u8).cmp(&(b.0.app.as_str(), b.0.kind as u8)));
        v
    } else {
        Vec::new()
    };

    // QUIESCE — from here a failure means exit + plain restart.
    quiesce();

    let roles: Vec<Role> = held.iter().map(|(r, _)| *r).collect();
    let mut fds = vec![lock_fd];
    fds.extend(held.iter().map(|(_, l)| l.as_raw_fd()));
    // A successor without the App door (an older build) can't parse an
    // `App` route kind: it gets the table without them (CD-A7).
    let front_routes = roles.contains(&Role::Front443).then(|| {
        let t = (*crate::front443::routes::current()).clone();
        if !roles_for_successor(&[Role::AppDoor], accepts.as_deref()).is_empty() {
            t
        } else {
            t.without_kind(crate::front443::routes::Kind::App)
        }
    });
    let take = serde_json::to_vec(&Msg::Take { roles: roles.clone(), front_routes, apps_follow: takes_apps }).expect("serialize take");
    if let Err(e) = fdpass::send_msg(&ours, &take, &fds) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failed::Quiesced(format!("send TAKE: {e}")));
    }
    let _ = ours.set_read_timeout(Some(ACK_TIMEOUT));
    match fdpass::recv_msg(&ours, 0)
        .map_err(|e| e.to_string())
        .and_then(|(b, _)| serde_json::from_slice::<Msg>(&b).map_err(|e| e.to_string()))
    {
        Ok(Msg::Ack) => {}
        Ok(other) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failed::Quiesced(format!("expected ACK, got {other:?}")));
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failed::Quiesced(format!("waiting for ACK: {e}")));
        }
    }
    // R19: the App listeners after the ACK (never fatal: the successor
    // re-binds the stored ports when they don't arrive, R36).
    if takes_apps {
        match send_app_listeners(&ours, &app_held) {
            Ok(n) => log_debug!("[daemon/handoff] handed over {n} App listener(s)"),
            Err(e) => log_debug!("[daemon/handoff] WARN: App listeners: {e}"),
        }
    }
    drop(app_held);
    // Our copies of the sockets and of the lock go; the successor's keep
    // them open (and the lock held only by it).
    drop(held);
    crate::daemon_main::release_singleton_lock();
    HANDED_OFF.store(true, Ordering::SeqCst);
    // Never wait on the successor: it outlives us (the drain exits this
    // process; the kernel reparents N).
    std::mem::forget(child);
    log_debug!("[daemon/handoff] successor {pid} acknowledged {roles:?}");

    if let Some(ns) = notify_socket() {
        match fdpass::sd_notify(&ns, &format!("MAINPID={pid}")) {
            Ok(()) => {
                std::thread::sleep(Duration::from_millis(200));
                match systemctl_show("MainPID") {
                    Some(p) if p == pid.to_string() => {
                        log_debug!("[daemon/handoff] systemd now tracks the successor (MainPID={p})")
                    }
                    other => log_debug!(
                        "[daemon/handoff] WARN: systemd MainPID is {other:?}, not {pid} — this \
                         process stays resident as the unit's main process"
                    ),
                }
            }
            Err(e) => log_debug!("[daemon/handoff] WARN: sd_notify MAINPID={pid}: {e}"),
        }
    }
    Ok(pid)
}

/// Stop accepting, stop starting things, release what the successor
/// takes over. Idempotent.
#[cfg(unix)]
fn quiesce() {
    let _ = quiesce_tx().send(true);
    k2_core::handoff_state::set_draining();
    // Agent PTYs. k2-ptyd sessions (PRD prd-k2-ptyd-v1 P30) are DETACHED:
    // the agent keeps running in ptyd and the successor adopts it (B1). An
    // in-process session dies with this daemon anyway, so it is killed and
    // the successor resumes it, as before. Kills run in parallel (each has
    // a 100 ms SIGHUP grace).
    let sessions = crate::v2_session_map::snapshot();
    let detached = sessions.iter().filter(|(_, s)| crate::daemon_stop::keep_for_next_daemon(s)).count();
    let joins: Vec<_> = sessions
        .into_iter()
        .filter(|(_, s)| !s.is_detached())
        .map(|(_, s)| std::thread::spawn(move || s.kill()))
        .collect();
    let n = joins.len();
    for j in joins {
        let _ = j.join();
    }
    if detached > 0 {
        log_debug!("[daemon/handoff] quiesce: {detached} ptyd session(s) detached for the successor, {n} in-process killed");
    }
    // frpc: the successor adopts it.
    let frpc = k2_core::tunnel::connector::detach_for_handoff();
    // The DB: the successor owns it now.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = conn.execute_batch("PRAGMA query_only = ON;") {
            log_debug!("[daemon/handoff] WARN: DB query_only: {e}");
        }
    }
    // The Thread store: redb holds an exclusive file lock for as long as
    // it is open, so close it before TAKE — the successor's first Thread
    // call (its card sweep runs right after it takes over) must find the
    // file free, not wait for this process to exit.
    let threads = if k2_core::overlay::store::release_for_handoff() {
        "Thread store closed"
    } else {
        "Thread store still closing (the successor retries)"
    };
    log_debug!(
        "[daemon/handoff] quiesced: accept loops stopped, {n} agent PTY(s) killed, frpc {} left \
         for the successor, DB read-only, {threads}",
        frpc.map(|p| format!("pid {p}")).unwrap_or_else(|| "(none)".into())
    );
}

/// After the main runtime is gone: pump the live splices until none are
/// left or the deadline passes, then exit. Never returns.
pub fn drain_then_exit() -> ! {
    let secs = std::env::var(DRAIN_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DRAIN_SECS);
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    let live = || live_splices() + live_app_conns();
    let start = live();
    let mut last = start;
    while live() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        let now = live();
        if now != last {
            last = now;
        }
    }
    let left = live();
    log_debug!(
        "[daemon/handoff] drain done: {start} splice(s) at hand-off, {left} cut at the {secs}s \
         deadline — exiting"
    );
    std::process::exit(0);
}

#[cfg(test)]
mod tests;
