//! The `ios-node` lane: an iPhone Simulator on a leased Mac compute node
//! (prd-ios-on-mac-node-0467-v1 §3.5, review IR-5…IR-12).
//!
//! **The shipped iOS lane, over a remote host.** Each session gets an inner
//! [`IosLane`] whose runner ([`RemoteRunner`]) and helper factory
//! ([`link::RemoteHelper`]) reach the node over the session's encrypted
//! stream. No iOS logic is copied: simctl, the helper protocol, typing and
//! screenshots are the shipped code; the node polices every command
//! (`k2_node_proto::sim::policy`).
//!
//! This lane answers itself: `probe`, `profiles` (from the nodes' offers,
//! no session needed), `boot` (node pick, lease, stream), `stop`, `erase`,
//! `adopt` (none in v1: the row says the server restarted), `install_app`
//! (a no-follow pack, IR-3), `alive` (from the session's state, never the
//! node, IR-6) and `open-url` (no `file:`, nothing only this server can
//! reach, IN-A14).
//!
//! **Node paths (IR-5).** The inner lane's `DeviceRef.data_dir` is the
//! node's `<tmp>/<workspace_key>`; the server's own data folder never goes
//! to the node.
//!
//! The daemon plugs in through [`NodeDirectory`] (its compute registry:
//! offers, grants, the attach socket) and feeds the stream socket through
//! [`IosNodeLane::stream_hello`] / [`PendingStream::finish`] /
//! [`IosNodeLane::stream_closed`].

pub mod link;

#[cfg(any(test, feature = "test-util"))]
pub mod fake_node;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_node_proto::crypto::SigningKey;
use k2_node_proto::sim::ctl::{DiskOp, ReplyBody};
use k2_node_proto::sim::policy::SlotPaths;
use k2_node_proto::sim::stream::{ControllerHandshake, StreamChallenge, StreamHello, StreamProof};
use k2_node_proto::sim::{self as psim, SessionState, SimClose, SimErase, SimFacts, SimOpen, SimStateFrame};
use serde_json::json;

use self::link::{RawTx, StreamConn};
use super::ios::helper::HelperIo;
use super::ios::lane::{HelperFactory, IosLane, IosLaneConfig};
use super::ios::simctl::{Output, Runner, SimError, SimResult};
use super::lane::*;
use crate::brand::{CLI_NAME, PRODUCT_NAME};
use crate::cli_name;

// ── the daemon seam ──────────────────────────────────────────────────

/// One compute node as this server sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: String,
    /// `rig1-mac`.
    pub name: String,
    pub online: bool,
    /// The node's protocol (its hello).
    pub protocol: u32,
    pub features: Vec<String>,
    /// `Offer.labels["os"] == "macos"`.
    pub mac: bool,
    /// What the node offers this server for iPhones (`Offer.sim`).
    pub sim: Option<SimFacts>,
    /// Why the node takes nothing new right now, in words ("paused by its
    /// owner", "holding for Sew CI"); `None` = free.
    pub hold: Option<String>,
    /// `k2 compute node pinned-only <node> on`: automatic placement never
    /// picks it; a device that names it (`rig1-mac:<type>`) still boots there.
    pub pinned_only: bool,
}

impl NodeInfo {
    /// Speaks sim at all (protocol 11 and the feature).
    pub fn offers_sim(&self) -> bool {
        self.mac && self.protocol >= psim::PROTOCOL_SIM && self.features.iter().any(|f| f == psim::FEATURE_SIM_IOS) && self.sim.is_some()
    }
}

/// The compute registry, as this lane needs it. The daemon implements it;
/// tests use [`fake_node`].
pub trait NodeDirectory: Send + Sync {
    /// Every enrolled node (online or not).
    fn nodes(&self) -> Vec<NodeInfo>;
    /// This workspace (`projects.id`) has a live compute grant on the node.
    fn granted(&self, node_id: &str, workspace_id: &str) -> bool;
    /// The node is on this workspace's compute node list, or it has none
    /// (`k2 compute allow-nodes`). Fails closed on a read error.
    fn allowed(&self, node_id: &str, workspace_id: &str) -> bool;
    /// Send a sim frame on the node's attach socket; `false` = not connected.
    fn send(&self, node_id: &str, frame: k2_node_proto::frames::Frame) -> bool;
    /// The node's enrolled public key (SPKI DER), by node id.
    fn node_spki(&self, node_id: &str) -> Option<Vec<u8>>;
    /// The node id with this fingerprint.
    fn node_by_fp(&self, fp: &str) -> Option<String>;
    /// This server's compute key (the attach handshake's).
    fn controller_key(&self) -> Result<SigningKey, String>;
}

static LANE: OnceLock<Arc<IosNodeLane>> = OnceLock::new();

/// Install the daemon's directory and build the lane (once, at boot,
/// before [`super::register_builtin_lanes`]).
pub fn install(dir: Arc<dyn NodeDirectory>) -> Arc<IosNodeLane> {
    LANE.get_or_init(|| Arc::new(IosNodeLane::new(dir))).clone()
}

/// The lane, once installed (the daemon's socket and frame handlers).
pub fn lane() -> Option<Arc<IosNodeLane>> {
    LANE.get().cloned()
}

// ── sessions ─────────────────────────────────────────────────────────

/// How long boot waits for the node to admit the session and dial the stream.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Opening,
    Open,
    Reconnecting { since: Instant },
    Closed { code: String, message: String },
}

pub struct Session {
    pub id: String,
    pub node_id: String,
    pub node_name: String,
    pub workspace_id: String,
    pub workspace_key: String,
    paths: SlotPaths,
    state: Mutex<State>,
    cv: Condvar,
    conn: Mutex<Option<Arc<StreamConn>>>,
    inner: Mutex<Option<Arc<IosLane>>>,
    /// `ensure_dir` of the set already done on the current connection.
    set_ready: AtomicBool,
}

impl Session {
    fn set_state(&self, s: State) {
        *self.state.lock().unwrap() = s;
        self.cv.notify_all();
    }

    fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    fn conn(&self) -> SimResult<Arc<StreamConn>> {
        match self.conn.lock().unwrap().clone() {
            Some(c) if !c.is_closed() => Ok(c),
            _ => Err(SimError::new("node_reconnecting", format!("reconnecting to {}; try again in a moment", self.node_name))),
        }
    }

    fn data_dir(&self) -> PathBuf {
        PathBuf::from(self.paths.workspace_tmp(&self.workspace_key))
    }
}

/// The inner lane's runner: every call goes to the node over the stream.
struct RemoteRunner {
    session: Arc<Session>,
}

impl Runner for RemoteRunner {
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> SimResult<Output> {
        self.session.conn()?.run(program, args, timeout)
    }
    fn ensure_dir(&self, p: &Path) -> SimResult<()> {
        let s = p.to_string_lossy();
        let set = s == self.session.paths.set;
        if set && self.session.set_ready.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.session.conn()?.disk(DiskOp::EnsureDir, &s)?;
        if set {
            self.session.set_ready.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    fn is_dir(&self, p: &Path) -> bool {
        self.bool_op(DiskOp::IsDir, p)
    }
    fn is_file(&self, p: &Path) -> bool {
        self.bool_op(DiskOp::IsFile, p)
    }
    fn read(&self, p: &Path) -> SimResult<Vec<u8>> {
        match self.session.conn()?.disk(DiskOp::Read, &p.to_string_lossy())? {
            ReplyBody::Bytes { data } => k2_node_proto::crypto::unb64(&data).map_err(|e| SimError::new("tool", e)),
            _ => Err(SimError::new("tool", "unexpected reply to read")),
        }
    }
    fn remove_file(&self, p: &Path) -> SimResult<()> {
        self.session.conn()?.disk(DiskOp::RemoveFile, &p.to_string_lossy()).map(|_| ())
    }
    fn remove_dir_all(&self, p: &Path) -> SimResult<()> {
        self.session.conn()?.disk(DiskOp::RemoveDirAll, &p.to_string_lossy()).map(|_| ())
    }
}

impl RemoteRunner {
    fn bool_op(&self, op: DiskOp, p: &Path) -> bool {
        matches!(self.session.conn().and_then(|c| c.disk(op, &p.to_string_lossy())), Ok(ReplyBody::Bool { value: true }))
    }
}

// ── the lane ─────────────────────────────────────────────────────────

pub struct IosNodeLane {
    dir: Arc<dyn NodeDirectory>,
    /// workspace key → its session (one device per workspace).
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// workspace key → the node its device data lives on (the default pick).
    homes: Mutex<HashMap<String, String>>,
    open_timeout: Duration,
    grace: Duration,
}

/// The step shown on a booting row (IN-A13). The manager installs it.
pub type ProgressHook = fn(&str, &str);
static PROGRESS: OnceLock<ProgressHook> = OnceLock::new();

pub fn set_progress_hook(h: ProgressHook) {
    let _ = PROGRESS.set(h);
}

fn progress(workspace_id: &str, step: &str) {
    if let Some(h) = PROGRESS.get() {
        h(workspace_id, step);
    }
}

/// The teaching line when no Mac node offers iPhones to this server.
pub fn no_mac_node_line() -> String {
    concat!("No Mac can run an iPhone for this server yet. The server's owner adds one (a Mac mini is fine) as a compute node: on this server, `", cli_name!(), " compute node add <name>` prints an enroll code (setting up the Mac: `", cli_name!(), " study compute`, ADDING A MACHINE); then, on the Mac, the owner installs Xcode and an iOS runtime and runs `sudo k2-node setup --sim`.").into()
}

/// The profile id a person sees: `<node>:<device type id>`.
pub fn profile_id(node: &str, type_id: &str) -> String {
    format!("{node}:{type_id}")
}

/// `iPhone 17 (on rig1-mac)`.
pub fn profile_name(name: &str, node: &str) -> String {
    format!("{name} (on {node})")
}

fn outdated_line(n: &NodeInfo) -> String {
    let remote = n.sim.as_ref().is_some_and(|s| s.remote_update);
    if remote {
        format!("Update k2-node on {0}: `{CLI_NAME} compute node update {0}`", n.name)
    } else {
        // IR-13: remote admin is off on that node, so the update command
        // would fail; say what works.
        format!("Update k2-node on {0}. Remote updates are off there, so the Mac's owner either copies the new k2-node build to {0} and runs `sudo k2-node setup` on it, or turns remote updates on at {0} (`sudo k2-node admin level policy`, which sets remote admin to policy) so this server's owner can run `{CLI_NAME} compute node update {0}`.", n.name)
    }
}

impl IosNodeLane {
    pub fn new(dir: Arc<dyn NodeDirectory>) -> Self {
        Self { dir, sessions: Mutex::new(HashMap::new()), homes: Mutex::new(HashMap::new()), open_timeout: OPEN_TIMEOUT, grace: Duration::from_secs(psim::GRACE_SECS) }
    }

    /// Shorter waits (tests).
    pub fn with_timeouts(mut self, open: Duration, grace: Duration) -> Self {
        self.open_timeout = open;
        self.grace = grace;
        self
    }

    fn session(&self, workspace_key: &str) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(workspace_key).cloned()
    }

    fn by_id(&self, session: &str) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().values().find(|s| s.id == session).cloned()
    }

    fn drop_session(&self, s: &Session) {
        let mut m = self.sessions.lock().unwrap();
        if m.get(&s.workspace_key).is_some_and(|x| x.id == s.id) {
            m.remove(&s.workspace_key);
        }
        if let Some(c) = s.conn.lock().unwrap().take() {
            c.close("the session ended");
        }
        // The inner lane's runner holds the session: break the cycle.
        s.inner.lock().unwrap().take();
    }

    /// The live session for a device, or why there is none.
    fn live(&self, dev: &DeviceRef) -> Result<Arc<Session>, LaneError> {
        let s = self.session(&dev.workspace_key).ok_or_else(|| LaneError::not_running(concat!("the iPhone on the Mac node stopped. `", cli_name!(), " sim boot` starts it again (its apps and data are kept on the node).")))?;
        match s.state() {
            State::Open => Ok(s),
            State::Opening | State::Reconnecting { .. } => Err(LaneError::not_running(format!("reconnecting to {}; try again in a moment", s.node_name))),
            State::Closed { message, .. } => Err(LaneError::not_running(message)),
        }
    }

    /// The inner lane and the device as the node knows it (IR-5).
    fn inner(&self, dev: &DeviceRef) -> Result<(Arc<IosLane>, DeviceRef), LaneError> {
        let s = self.live(dev)?;
        let inner = s.inner.lock().unwrap().clone().ok_or_else(|| LaneError::not_running("the session isn't ready"))?;
        let d = DeviceRef { data_dir: s.data_dir(), ..dev.clone() };
        Ok((inner, d))
    }

    fn make_inner(&self, s: &Arc<Session>) -> Arc<IosLane> {
        let cfg = IosLaneConfig { set_path: PathBuf::from(&s.paths.set), helper_dir: PathBuf::from("/nonexistent/k2-ios-node") };
        let runner: Arc<dyn Runner> = Arc::new(RemoteRunner { session: s.clone() });
        let sess = s.clone();
        let factory: HelperFactory = Box::new(move |_set, udid, _xcode| {
            let conn = sess.conn()?;
            Ok(conn.start_helper(udid)? as Arc<dyn HelperIo>)
        });
        Arc::new(IosLane::with(cfg, runner, factory))
    }

    /// Pick the node for a boot, or the error that teaches what to do.
    fn pick_node(&self, req: &BootRequest) -> Result<(NodeInfo, String), LaneError> {
        let nodes = self.dir.nodes();
        let (want_node, want_type) = match req.profile_id.as_deref().and_then(|p| p.split_once(':')) {
            Some((n, t)) => (Some(n.to_string()), Some(t.to_string())),
            None => (None, req.profile_id.clone()),
        };
        // A pinned-only Mac takes an iPhone only when the device names it,
        // as a compute job must name it.
        let sim_nodes: Vec<&NodeInfo> = nodes
            .iter()
            .filter(|n| n.offers_sim())
            .filter(|n| want_node.as_ref().map_or(!n.pinned_only, |w| *w == n.name))
            .collect();
        if sim_nodes.is_empty() {
            if let Some(w) = &want_node {
                return Err(LaneError::node(NodeReason::LaneUnavailable, format!("{w} doesn't run iPhones for this server. `{CLI_NAME} sim list-devices --platform ios` lists the ones that do.")));
            }
            return Err(LaneError::node(NodeReason::LaneUnavailable, no_mac_node_line()));
        }
        let mut busy: Vec<String> = Vec::new();
        let mut not_ready: Vec<&NodeInfo> = Vec::new();
        let mut not_granted: Vec<&NodeInfo> = Vec::new();
        let mut not_allowed: Vec<&NodeInfo> = Vec::new();
        let mut outdated: Vec<&NodeInfo> = Vec::new();
        let mut ok: Vec<&NodeInfo> = Vec::new();
        for n in sim_nodes {
            let f = n.sim.as_ref().expect("offers_sim");
            if !n.online {
                busy.push(format!("{} is offline", n.name));
            } else if f.helper_protocol < u64::from(crate::sim::ios::helper::OLDEST_PROTOCOL) {
                // The node builds the shared helper source: too old a k2-node
                // builds one this server can't speak to (v1 asks for 3).
                outdated.push(n);
            } else if !f.ready {
                not_ready.push(n);
            } else if !self.dir.allowed(&n.id, &req.workspace_id) {
                // The workspace's node list binds everyone, the owner too.
                not_allowed.push(n);
            } else if !req.grant_exempt && !self.dir.granted(&n.id, &req.workspace_id) {
                // As with `k2 compute run`: the owner and admins may use any
                // active node without a grant; agents and others need one.
                not_granted.push(n);
            } else if let Some(h) = &n.hold {
                busy.push(format!("{} is {h}", n.name));
            } else if f.sessions_total as usize >= psim::MAX_SESSIONS_PER_NODE {
                busy.push(format!("{} is running another iPhone", n.name));
            } else if f.free_bytes < f.reserve_bytes.max(psim::RESERVE_MEM_BYTES) {
                busy.push(format!("{} is low on memory", n.name));
            } else {
                ok.push(n);
            }
        }
        if ok.is_empty() {
            if let Some(n) = outdated.first() {
                return Err(LaneError::node(NodeReason::Outdated, outdated_line(n)));
            }
            if let Some(n) = not_ready.first() {
                let needs: Vec<LaneNeed> = n.sim.as_ref().map(|f| f.needs.iter().map(|x| LaneNeed::OwnerStep { what: format!("On {}: {}", n.name, x.what), command: x.command.clone() }).collect()).unwrap_or_default();
                let first = needs.first().map(|x| match x {
                    LaneNeed::OwnerStep { what, command } => format!("{what}: `{command}`"),
                    _ => String::new(),
                });
                return Err(LaneError::NeedsSetup { message: first.unwrap_or_else(|| format!("{} isn't ready to run iPhones yet", n.name)), needs });
            }
            if !busy.is_empty() {
                return Err(LaneError::node(NodeReason::Busy, format!("{}. Try again later.", busy.join("; "))));
            }
            if let Some(n) = not_allowed.first() {
                return Err(LaneError::node(
                    NodeReason::NotAllowed,
                    format!(
                        "{0} can run iPhones, but it isn't on this workspace's compute node list (`{CLI_NAME} compute allow-nodes`), so nothing from it goes there. Only a person adds it: `{CLI_NAME} compute allow-nodes --add {0} --workspace <workspace>` (the owner or an Admin, from a terminal outside {PRODUCT_NAME}).",
                        n.name
                    ),
                ));
            }
            let n = not_granted.first().expect("some reason");
            return Err(LaneError::node(
                NodeReason::NotGranted,
                format!("{0} can run iPhones, but this workspace has no grant on it. The owner can run: `{CLI_NAME} compute grant {0} --workspace <workspace>`", n.name),
            ));
        }
        // Where this workspace's device already is (its data is there),
        // then the free node with the most memory.
        let home = self.homes.lock().unwrap().get(&req.workspace_key).cloned();
        ok.sort_by_key(|n| (Some(&n.id) != home.as_ref(), std::cmp::Reverse(n.sim.as_ref().map(|f| f.free_bytes).unwrap_or(0))));
        let n = ok[0].clone();
        let f = n.sim.as_ref().expect("offers_sim");
        let newest = f.runtimes.first().ok_or_else(|| LaneError::NeedsSetup { message: format!("On {}: install an iOS Simulator runtime: `xcodebuild -downloadPlatform iOS`", n.name), needs: vec![] })?;
        let t = match want_type {
            Some(t) => newest.device_types.iter().find(|d| d.identifier == t || d.name.eq_ignore_ascii_case(&t)).ok_or_else(|| {
                LaneError::invalid(format!("No iPhone {t:?} on {}. Try one of: {}", n.name, newest.device_types.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")))
            })?,
            None => newest.device_types.iter().find(|d| d.default).or(newest.device_types.first()).ok_or_else(|| LaneError::invalid(format!("{} lists no iPhones", n.name)))?,
        };
        Ok((n.clone(), t.identifier.clone()))
    }

    fn open_session(&self, n: &NodeInfo, req: &BootRequest) -> Result<Arc<Session>, LaneError> {
        let f = n.sim.as_ref().expect("offers_sim");
        let id = k2_node_proto::crypto::random_id().map_err(LaneError::tool)?;
        let s = Arc::new(Session {
            id: id.clone(),
            node_id: n.id.clone(),
            node_name: n.name.clone(),
            workspace_id: req.workspace_id.clone(),
            workspace_key: req.workspace_key.clone(),
            paths: f.paths.clone(),
            state: Mutex::new(State::Opening),
            cv: Condvar::new(),
            conn: Mutex::new(None),
            inner: Mutex::new(None),
            set_ready: AtomicBool::new(false),
        });
        *s.inner.lock().unwrap() = Some(self.make_inner(&s));
        if let Some(old) = self.sessions.lock().unwrap().insert(req.workspace_key.clone(), s.clone()) {
            if let Some(c) = old.conn.lock().unwrap().take() {
                c.close("replaced by a new session");
            }
        }
        let open = SimOpen { session: id, workspace_key: req.workspace_key.clone(), mem_bytes: f.reserve_bytes.max(psim::RESERVE_MEM_BYTES), lease_secs: psim::LEASE_MAX_SECS };
        if !self.dir.send(&n.id, k2_node_proto::frames::Frame::SimOpen(open)) {
            self.drop_session(&s);
            return Err(LaneError::node(NodeReason::Busy, format!("{} went offline. Try again later.", n.name)));
        }
        // Wait for the node to admit it and dial the stream.
        let deadline = Instant::now() + self.open_timeout;
        let mut st = s.state.lock().unwrap();
        loop {
            match &*st {
                State::Open => break,
                State::Closed { code, message } => {
                    let (code, message) = (code.clone(), message.clone());
                    drop(st);
                    self.drop_session(&s);
                    return Err(refusal_error(&n.name, &code, &message));
                }
                _ => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                drop(st);
                let _ = self.dir.send(&n.id, k2_node_proto::frames::Frame::SimClose(SimClose { session: s.id.clone(), reason: "open_timeout".into() }));
                self.drop_session(&s);
                return Err(LaneError::timeout(format!("{} didn't start the iPhone session within {}s", n.name, self.open_timeout.as_secs())));
            }
            st = s.cv.wait_timeout(st, left).unwrap().0;
        }
        drop(st);
        Ok(s)
    }

    fn close_session(&self, s: &Arc<Session>, reason: &str) {
        let _ = self.dir.send(&s.node_id, k2_node_proto::frames::Frame::SimClose(SimClose { session: s.id.clone(), reason: reason.into() }));
        s.set_state(State::Closed { code: psim::codes::CLOSED.into(), message: format!("stopped ({reason})") });
        self.drop_session(s);
    }

    // ── the daemon's hooks ───────────────────────────────────────────

    /// A `sim_state` from a node (its attach socket).
    pub fn on_state(&self, node_id: &str, f: &SimStateFrame) {
        let Some(s) = self.by_id(&f.session) else { return };
        if s.node_id != node_id {
            return; // another node can't speak for this session
        }
        match f.state {
            SessionState::Opening | SessionState::Open => {}
            SessionState::Reconnecting => {
                if s.state() == State::Open {
                    s.set_state(State::Reconnecting { since: Instant::now() });
                }
            }
            SessionState::Closed | SessionState::Refused => {
                let code = f.code.clone().unwrap_or_else(|| psim::codes::CLOSED.into());
                let message = closed_line(&s.node_name, &code, f.message.as_deref());
                s.set_state(State::Closed { code, message });
                if let Some(c) = s.conn.lock().unwrap().take() {
                    c.close("the node closed the session");
                }
            }
        }
    }

    /// A node's offer arrived: a session we hold that the node no longer
    /// lists is gone (a node reboot or k2-node restart, IR-7).
    pub fn on_offer(&self, node_id: &str, sim: Option<&SimFacts>) {
        let Some(f) = sim else { return };
        let ours: Vec<Arc<Session>> = self.sessions.lock().unwrap().values().filter(|s| s.node_id == node_id).cloned().collect();
        for s in ours {
            if matches!(s.state(), State::Open | State::Reconnecting { .. }) && !f.sessions.contains(&s.id) {
                s.set_state(State::Closed { code: psim::codes::NODE_RESTARTED.into(), message: closed_line(&s.node_name, psim::codes::NODE_RESTARTED, None) });
                if let Some(c) = s.conn.lock().unwrap().take() {
                    c.close("the node restarted");
                }
            }
        }
    }

    /// The sessions this server holds on a node: sent as `sim_sync` right
    /// after every attach (IR-7).
    pub fn held_sessions(&self, node_id: &str) -> Vec<String> {
        self.sessions.lock().unwrap().values().filter(|s| s.node_id == node_id && !matches!(s.state(), State::Closed { .. })).map(|s| s.id.clone()).collect()
    }

    /// The node dialed the stream: check the session is ours on exactly
    /// that node, then answer the challenge.
    pub fn stream_hello(&self, hello: &StreamHello) -> Result<(PendingStream, StreamChallenge), String> {
        let s = self.by_id(&hello.session).ok_or("no such sim session")?;
        let node = self.dir.node_by_fp(&hello.node_fp).ok_or("unknown node")?;
        if node != s.node_id {
            return Err("that session belongs to another node".into());
        }
        if matches!(s.state(), State::Closed { .. }) {
            return Err("that session has ended".into());
        }
        let spki = self.dir.node_spki(&node).ok_or("the node has no key")?;
        let key = self.dir.controller_key()?;
        let (hs, challenge) = ControllerHandshake::answer(&key, &spki, hello)?;
        Ok((PendingStream { hs, session: s }, challenge))
    }

    /// The stream socket closed (or failed): keep the device for the grace.
    pub fn stream_closed(&self, session: &str, conn_id: &str) {
        let Some(s) = self.by_id(session) else { return };
        let mut c = s.conn.lock().unwrap();
        if c.as_ref().is_some_and(|c| c.conn_id == conn_id) {
            if let Some(old) = c.take() {
                old.close("the stream closed");
            }
            drop(c);
            if s.state() == State::Open {
                s.set_state(State::Reconnecting { since: Instant::now() });
            }
        }
    }

    /// A session's state in words (tests, status).
    pub fn session_state(&self, workspace_key: &str) -> Option<&'static str> {
        self.session(workspace_key).map(|s| match s.state() {
            State::Opening => "opening",
            State::Open => "open",
            State::Reconnecting { .. } => "reconnecting",
            State::Closed { .. } => "closed",
        })
    }

    /// The current connection of a workspace's session (tests: the fake
    /// node's relay leg, replay).
    pub fn conn_of(&self, workspace_key: &str) -> Option<Arc<StreamConn>> {
        self.session(workspace_key).and_then(|s| s.conn.lock().unwrap().clone())
    }
}

/// The server half of a stream handshake, waiting for the node's proof.
pub struct PendingStream {
    hs: ControllerHandshake,
    session: Arc<Session>,
}

impl PendingStream {
    /// Verify the proof, then make this the session's connection.
    pub fn finish(self, proof: &StreamProof, raw: RawTx) -> Result<Arc<StreamConn>, String> {
        let keys = self.hs.finish(proof)?;
        let s = self.session;
        let conn = StreamConn::new(keys, raw, &s.node_name);
        {
            let mut c = s.conn.lock().unwrap();
            if let Some(old) = c.replace(conn.clone()) {
                old.close("a new connection replaced it");
            }
        }
        s.set_ready.store(false, Ordering::SeqCst);
        if matches!(s.state(), State::Closed { .. }) {
            conn.close("the session has ended");
            return Err("that session has ended".into());
        }
        s.set_state(State::Open);
        Ok(conn)
    }

    pub fn session_id(&self) -> &str {
        &self.session.id
    }
}

fn closed_line(node: &str, code: &str, message: Option<&str>) -> String {
    let why = match code {
        psim::codes::LEASE_LOST => format!("{node} lost the connection to this server, so the iPhone stopped"),
        psim::codes::LEASE_EXPIRED => "the iPhone's 8-hour lease ended".to_string(),
        psim::codes::MEM_CAP => format!("the iPhone used more memory than {node} allows"),
        psim::codes::SIMHOST_EXITED => format!("the Simulator service on {node} stopped"),
        psim::codes::NODE_RESTARTED => format!("{node} restarted"),
        psim::codes::NOT_HELD => "this server no longer held the session".to_string(),
        psim::codes::REPLACED => "a new session replaced it".to_string(),
        _ => format!("{node} ended the session{}", message.map(|m| format!(": {m}")).unwrap_or_default()),
    };
    format!("{why}. `{CLI_NAME} sim boot` starts it again (its apps and data are kept on {node}).")
}

fn refusal_error(node: &str, code: &str, message: &str) -> LaneError {
    match code {
        psim::codes::NODE_BUSY => LaneError::node(NodeReason::Busy, format!("{node} is {message}. Try again later.")),
        psim::codes::NOT_ENABLED => LaneError::node(NodeReason::LaneUnavailable, format!("{node} doesn't run iPhones for this server: {message}. Its owner turns it on with `sudo k2-node setup --sim`.")),
        psim::codes::NODE_OUTDATED => LaneError::node(NodeReason::Outdated, format!("Update k2-node on {node}: {message}")),
        _ => LaneError::not_running(closed_line(node, code, Some(message))),
    }
}

/// `open-url` on a node (IN-A14, IR-4): no `file:`, and nothing only this
/// server can reach.
pub fn url_guard(url: &str, node: &str) -> Result<(), LaneError> {
    if let Err(r) = psim::policy::check_open_url(url) {
        return Err(LaneError::invalid(r.message));
    }
    let lower = url.trim().to_ascii_lowercase();
    let Some((scheme, rest)) = lower.split_once("://") else { return Ok(()) };
    if scheme != "http" && scheme != "https" {
        return Ok(()); // an app's deep link
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = if let Some(h) = host_port.strip_prefix('[') { h.split(']').next().unwrap_or("") } else { host_port.split(':').next().unwrap_or("") };
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.is_empty()
        || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified(),
            IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
        });
    if local {
        return Err(LaneError::node(
            NodeReason::UrlUnreachable,
            format!("The iPhone runs on {node}, which can't reach this server's localhost or LAN. Open the App's public URL (the url line of `{CLI_NAME} app inspect <app-id>`). If that says local only, the App has no public URL yet: publish it first (`{CLI_NAME} publish --help`)."),
        ));
    }
    Ok(())
}

impl SimLane for IosNodeLane {
    fn kind(&self) -> LaneKind {
        LaneKind::IosNode
    }

    fn probe(&self) -> LaneProbe {
        let mut p = LaneProbe { lane: LaneKind::IosNode, usable: false, ready: false, needs: vec![], detail: None, versions: vec![], setup: None };
        let nodes = self.dir.nodes();
        let sim: Vec<&NodeInfo> = nodes.iter().filter(|n| n.offers_sim()).collect();
        if sim.is_empty() {
            p.detail = Some(no_mac_node_line());
            return p;
        }
        p.usable = true;
        let ready: Vec<&&NodeInfo> = sim.iter().filter(|n| n.sim.as_ref().is_some_and(|f| f.ready)).collect();
        p.ready = !ready.is_empty();
        if !p.ready {
            for n in &sim {
                for x in n.sim.as_ref().map(|f| f.needs.clone()).unwrap_or_default() {
                    p.needs.push(LaneNeed::OwnerStep { what: format!("On {}: {}", n.name, x.what), command: x.command });
                }
            }
        }
        for n in &sim {
            if let Some(x) = n.sim.as_ref().and_then(|f| f.xcode.clone()) {
                p.versions.push((format!("xcode on {}", n.name), x));
            }
        }
        let names: Vec<&str> = sim.iter().map(|n| n.name.as_str()).collect();
        p.detail = Some(if p.ready { format!("iPhones on {}", names.join(", ")) } else { format!("{} needs setup before it can run iPhones", names.join(", ")) });
        p
    }

    fn install(&self, _licence_accepted: &[String], _progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), LaneError> {
        // Nothing to install on this server: the node owner sets the Mac up.
        Ok(())
    }

    fn profiles(&self) -> Result<Vec<DeviceProfile>, LaneError> {
        let mut nodes: Vec<NodeInfo> = self.dir.nodes().into_iter().filter(|n| n.offers_sim() && n.sim.as_ref().is_some_and(|f| f.ready)).collect();
        if nodes.is_empty() {
            return Err(LaneError::node(NodeReason::LaneUnavailable, no_mac_node_line()));
        }
        nodes.sort_by_key(|n| std::cmp::Reverse(n.sim.as_ref().map(|f| f.free_bytes).unwrap_or(0)));
        let mut out = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            let Some(r) = n.sim.as_ref().and_then(|f| f.runtimes.first()) else { continue };
            for t in &r.device_types {
                out.push(DeviceProfile {
                    id: profile_id(&n.name, &t.identifier),
                    name: profile_name(&t.name, &n.name),
                    width_pt: t.width_pt,
                    height_pt: t.height_pt,
                    scale: t.scale_milli as f32 / 1000.0,
                    os: r.name.clone(),
                    default: i == 0 && t.default,
                });
            }
        }
        out.sort_by_key(|p| !p.default);
        Ok(out)
    }

    fn boot(&self, req: &BootRequest) -> Result<BootedDevice, LaneError> {
        progress(&req.workspace_id, "Finding a Mac node");
        let (node, type_id) = self.pick_node(req)?;
        progress(&req.workspace_id, &format!("Starting on {}", node.name));
        let s = self.open_session(&node, req)?;
        self.homes.lock().unwrap().insert(req.workspace_key.clone(), node.id.clone());
        let inner = s.inner.lock().unwrap().clone().expect("made at open");
        let type_name = node.sim.as_ref().and_then(|f| f.runtimes.first()).and_then(|r| r.device_types.iter().find(|d| d.identifier == type_id)).map(|d| d.name.clone()).unwrap_or_else(|| "iPhone".into());
        progress(&req.workspace_id, &format!("Booting {type_name}"));
        let ireq = BootRequest { profile_id: Some(type_id.clone()), data_dir: s.data_dir(), ..req.clone() };
        match inner.boot(&ireq) {
            Ok(mut b) => {
                b.profile.id = profile_id(&node.name, &b.profile.id);
                b.profile.name = profile_name(&b.profile.name, &node.name);
                b.pid = None;
                b.lane_state = json!({ "node": node.name, "nodeId": node.id, "session": s.id, "name": b.lane_state.get("name") });
                Ok(b)
            }
            Err(e) => {
                self.close_session(&s, "boot_failed");
                Err(e)
            }
        }
    }

    fn adopt(&self, _dev: &DeviceRef) -> Result<Option<BootedDevice>, LaneError> {
        // No adopt in v1 (IN-A9): the node's grace runs out, and `sim_sync`
        // on its next attach frees it at once (IR-7).
        Ok(None)
    }

    fn stop(&self, dev: &DeviceRef) -> Result<(), LaneError> {
        let Some(s) = self.session(&dev.workspace_key) else { return Ok(()) };
        if s.state() == State::Open {
            if let Some(inner) = s.inner.lock().unwrap().clone() {
                let d = DeviceRef { data_dir: s.data_dir(), ..dev.clone() };
                // The node also shuts the device down on sim_close.
                let _ = inner.stop(&d);
            }
        }
        self.close_session(&s, "stopped");
        Ok(())
    }

    fn erase(&self, workspace_key: &str, data_dir: &Path) -> Result<(), LaneError> {
        if let Some(s) = self.session(workspace_key) {
            self.close_session(&s, "erased");
        }
        if let Some(node) = self.homes.lock().unwrap().remove(workspace_key) {
            let _ = self.dir.send(&node, k2_node_proto::frames::Frame::SimErase(SimErase { workspace_key: workspace_key.to_string() }));
        }
        // The server's own (empty) folder for the row.
        match std::fs::remove_dir_all(data_dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LaneError::tool(format!("cannot remove {}: {e}", data_dir.display()))),
        }
    }

    fn install_app(&self, dev: &DeviceRef, artifact: &Path) -> Result<InstallOutcome, LaneError> {
        let plist = artifact.join("Info.plist");
        if artifact.extension().and_then(|e| e.to_str()) != Some("app") || !artifact.is_dir() || !plist.is_file() {
            return Err(LaneError::invalid(format!(
                "{} isn't an iOS .app bundle (a folder ending in .app with an Info.plist). Build for the Simulator, then pass the .app from the build folder.",
                artifact.display()
            )));
        }
        let (inner, d) = self.inner(dev)?;
        let s = self.live(dev)?;
        // IR-3: never follow a link; refuse one that leaves the bundle.
        let bytes = psim::pack::pack_app(artifact).map_err(|e| LaneError::invalid(format!("{} can't be uploaded: {e}", artifact.display())))?;
        let node_path = s.conn().map_err(super::ios::lane::lane_err)?.upload(&bytes).map_err(super::ios::lane::lane_err)?;
        inner.install_app(&d, Path::new(&node_path))
    }

    fn launch(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        let (inner, d) = self.inner(dev)?;
        inner.launch(&d, app_id)
    }

    fn terminate(&self, dev: &DeviceRef, app_id: &str) -> Result<(), LaneError> {
        let (inner, d) = self.inner(dev)?;
        inner.terminate(&d, app_id)
    }

    fn tree(&self, dev: &DeviceRef) -> Result<RawTree, LaneError> {
        let (inner, d) = self.inner(dev)?;
        inner.tree(&d)
    }

    fn act(&self, dev: &DeviceRef, action: &Action) -> Result<(), LaneError> {
        if let Action::OpenUrl { url } = action {
            let node = self.session(&dev.workspace_key).map(|s| s.node_name.clone()).unwrap_or_else(|| "the Mac node".into());
            url_guard(url, &node)?;
        }
        let (inner, d) = self.inner(dev)?;
        inner.act(&d, action)
    }

    fn screenshot(&self, dev: &DeviceRef) -> Result<Frame, LaneError> {
        let (inner, d) = self.inner(dev)?;
        inner.screenshot(&d)
    }

    fn frames(&self, dev: &DeviceRef, opts: FrameOpts) -> Result<Box<dyn FrameSource>, LaneError> {
        let (inner, d) = self.inner(dev)?;
        let opts = FrameOpts { max_fps: opts.max_fps.clamp(1, psim::FPS_MAX), ..opts };
        inner.frames(&d, opts)
    }

    fn logs(&self, dev: &DeviceRef, q: &LogQuery) -> Result<Vec<String>, LaneError> {
        let (inner, d) = self.inner(dev)?;
        inner.logs(&d, q)
    }

    /// From the session's state, never the node (IR-6): open, or
    /// reconnecting inside the grace.
    fn alive(&self, dev: &DeviceRef) -> bool {
        match self.session(&dev.workspace_key).map(|s| s.state()) {
            Some(State::Open) | Some(State::Opening) => true,
            Some(State::Reconnecting { since }) => since.elapsed() < self.grace,
            _ => false,
        }
    }
}
