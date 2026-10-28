//! The lane-blind sim-session manager: every `/cli/sim/*` verb, the box
//! limits, the control hold, snapshots, the idle reaper, adopt-on-boot and
//! the audit rows. Blocking; the daemon calls it from `spawn_blocking`.
//!
//! The daemon resolves *who* (owner, login, agent passport) and *which
//! workspace* and checks the agent switch before it calls in here; this
//! module trusts its [`Ws`] and [`Actor`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use super::api::*;
use super::encode;
use super::hold::{AgentBlocked, ControlHold, HoldEvent, HoldView, AGENT_WAIT};
use super::lane::*;
use super::limits;
use super::store::{self, SimRow};
use super::stream::SimState;
use super::tree::Snapshot;

// ── inputs ───────────────────────────────────────────────────────────

/// The workspace a call targets (resolved by the daemon).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ws {
    /// `projects.id`.
    pub id: String,
    pub handle: String,
    /// The workspace folder (screenshots go under `.k2/sim/shots/`).
    pub path: PathBuf,
}

/// Who acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The owner token, a Connect login, or a K2 shell tab: `name` is the
    /// audit label (`owner`, `login:ana`, `shell:web`).
    Person { name: String },
    /// An agent passport on its own workspace (the switch is on).
    Agent { handle: String },
}

impl Actor {
    pub fn audit(&self) -> String {
        match self {
            Actor::Person { name } => name.clone(),
            Actor::Agent { handle } => format!("agent:{handle}"),
        }
    }
    pub fn is_agent(&self) -> bool {
        matches!(self, Actor::Agent { .. })
    }
    fn hold_name(&self) -> String {
        match self {
            Actor::Person { name } => name.strip_prefix("login:").unwrap_or(name).to_string(),
            Actor::Agent { handle } => handle.clone(),
        }
    }
}

// ── errors ───────────────────────────────────────────────────────────

/// A refusal, with the HTTP status, a stable code and words to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    /// Exit code the CLI should use (2 held, 3 needs a person).
    pub exit: u8,
    pub needs: Vec<LaneNeed>,
}

impl SimError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> SimError {
        SimError { status, code, message: message.into(), exit: 1, needs: vec![] }
    }
    pub fn exit(mut self, e: u8) -> SimError {
        self.exit = e;
        self
    }
    pub fn body(&self) -> Value {
        let mut v = json!({
            "error": { "code": self.code, "hint": self.message },
            "code": self.code,
            "message": self.message,
            "exit": self.exit,
        });
        if !self.needs.is_empty() {
            v["needs"] = serde_json::to_value(&self.needs).unwrap_or(Value::Null);
        }
        v
    }
    fn no_device() -> SimError {
        SimError::new(409, "sim_no_device", "this workspace has no running device. Boot one with `k2 sim boot`.")
    }
}

impl From<LaneError> for SimError {
    fn from(e: LaneError) -> SimError {
        let exit = match e {
            LaneError::NeedsSetup { .. } => 3,
            _ => 1,
        };
        let needs = match &e {
            LaneError::NeedsSetup { needs, .. } => needs.clone(),
            _ => vec![],
        };
        SimError { status: e.http_status(), code: e.code(), message: e.message().to_string(), exit, needs }
    }
}

impl From<rusqlite::Error> for SimError {
    fn from(e: rusqlite::Error) -> SimError {
        SimError::new(500, "sim_db", format!("database: {e}"))
    }
}

pub type R<T> = Result<T, SimError>;

// ── events ───────────────────────────────────────────────────────────

/// `(kind, payload)` → the daemon's session-events bus. Kinds:
/// `sim_state`, `open_mobile`, `sim_hold`.
pub type EventHook = fn(&str, Value);
static EVENT_HOOK: OnceLock<EventHook> = OnceLock::new();

pub fn set_event_hook(h: EventHook) {
    let _ = EVENT_HOOK.set(h);
}

fn emit(kind: &str, payload: Value) {
    if let Some(h) = EVENT_HOOK.get() {
        h(kind, payload);
    }
}

// ── the manager ──────────────────────────────────────────────────────

/// In-memory state per sim session (never persisted).
#[derive(Default)]
struct Live {
    hold: ControlHold,
    snapshot: Option<Snapshot>,
    snap_seq: u64,
    viewers: u32,
    orientation: Option<Orientation>,
}

/// A lane's first-use install in progress (or its last result).
#[derive(Debug, Clone, Default)]
struct SetupState {
    running: bool,
    done: u64,
    total: u64,
    error: Option<String>,
}

pub struct Manager {
    /// Last disk measurement and per-workspace-key device bytes.
    disk: Mutex<Option<(SimDisk, HashMap<String, u64>)>>,
    setup: Mutex<HashMap<LaneKind, SetupState>>,
    live: Mutex<HashMap<i64, Live>>,
    /// One operation at a time per workspace (boot / stop / erase / act).
    op_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    probes: Mutex<HashMap<LaneKind, LaneProbe>>,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

pub fn manager() -> &'static Manager {
    MANAGER.get_or_init(Manager::new)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Settle time after an action before the diff read.
/// A `platform` that is no lane K2 knows (400).
pub const SIM_LANE_UNKNOWN: &str = "sim_lane_unknown";
/// A known lane this server doesn't have (409): an iPhone on a box with no
/// Mac. The Mobile tab never offers one (it lists the lanes `status` reports).
pub const SIM_LANE_UNAVAILABLE: &str = "sim_lane_unavailable";
/// What an iPhone needs; the Mobile tab shows the same line.
pub const IPHONE_NEEDS_A_MAC: &str = "iPhone needs a Mac (this computer or a Mac compute node)";

const SETTLE: Duration = Duration::from_millis(350);
const DIFF_MAX: usize = 8;
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(240);
pub const LOG_DEFAULT_LINES: usize = 200;
pub const LOG_MAX_LINES: usize = 2000;
pub const LOG_MAX_BYTES: usize = 256 * 1024;
pub const WAIT_DEFAULT: Duration = Duration::from_secs(10);
pub const WAIT_MAX: Duration = Duration::from_secs(120);

impl Manager {
    pub fn new() -> Manager {
        Manager { disk: Mutex::new(None), setup: Mutex::new(HashMap::new()), live: Mutex::new(HashMap::new()), op_locks: Mutex::new(HashMap::new()), probes: Mutex::new(HashMap::new()) }
    }

    fn op_lock(&self, project_id: &str) -> Arc<Mutex<()>> {
        self.op_locks.lock().entry(project_id.to_string()).or_default().clone()
    }

    fn with_live<T>(&self, id: i64, f: impl FnOnce(&mut Live) -> T) -> T {
        let mut m = self.live.lock();
        f(m.entry(id).or_default())
    }

    // ── lanes ────────────────────────────────────────────────────────

    /// Probe every registered lane and cache the answers (at boot, and on
    /// `status?refresh=1`).
    pub fn refresh_probes(&self) -> Vec<LaneProbe> {
        let mut out = Vec::new();
        for l in super::lanes() {
            out.push(l.probe());
        }
        let mut c = self.probes.lock();
        c.clear();
        for p in &out {
            c.insert(p.lane, p.clone());
        }
        out
    }

    pub fn probes(&self) -> Vec<LaneProbe> {
        let have = !self.probes.lock().is_empty();
        if !have {
            return self.refresh_probes();
        }
        let mut v: Vec<LaneProbe> = self.probes.lock().values().cloned().collect();
        v.sort_by_key(|p| p.lane);
        v
    }

    /// The boot-status key's condition (M-A13).
    pub fn any_lane_usable(&self) -> bool {
        self.probes().iter().any(|p| p.usable)
    }

    fn pick_lane(&self, platform: Option<&str>) -> R<Arc<dyn SimLane>> {
        let probes = self.probes();
        let usable = |k: LaneKind| probes.iter().any(|p| p.lane == k && p.usable) && super::lane(k).is_some();
        let order: Vec<LaneKind> = match platform.map(|p| p.trim().to_ascii_lowercase()) {
            Some(p) if p == "android" => vec![LaneKind::Android],
            Some(p) if p == "ios" => vec![LaneKind::Ios, LaneKind::IosNode],
            // A lane's own name (the devices answer names it): that lane.
            Some(p) if p == "ios-node" => vec![LaneKind::IosNode],
            Some(p) if p.is_empty() => default_order(),
            None => default_order(),
            Some(other) => {
                return Err(SimError::new(
                    400,
                    SIM_LANE_UNKNOWN,
                    format!("unknown platform '{other}': use android or ios (k2 sim status lists the lanes this server has)"),
                ));
            }
        };
        for k in &order {
            if usable(*k) {
                return super::lane(*k).ok_or_else(|| SimError::new(503, "sim_unavailable", "lane vanished"));
            }
        }
        // An iPhone asked for on a server with no iOS lane at all (a Linux
        // box, no Mac compute node): say what it needs, never a bare 400.
        let ios_only = order.iter().all(|k| k.platform() == "ios");
        if ios_only && order.iter().all(|k| super::lane(*k).is_none()) {
            return Err(SimError::new(409, SIM_LANE_UNAVAILABLE, IPHONE_NEEDS_A_MAC));
        }
        let why: Vec<String> = order
            .iter()
            .map(|k| match probes.iter().find(|p| p.lane == *k) {
                Some(p) => format!("{}: {}", k, p.detail.clone().unwrap_or_else(|| "not usable".into())),
                None => format!("{k}: not available on this server"),
            })
            .collect();
        Err(SimError::new(503, "sim_unavailable", format!("no device lane can run here ({})", why.join("; "))))
    }

    // ── views ────────────────────────────────────────────────────────

    fn view(&self, row: &SimRow, ws: &Ws) -> SimView {
        let (hold, viewers, app_id) = self.with_live(row.id, |l| {
            (l.hold.view(Instant::now()), l.viewers, l.snapshot.as_ref().and_then(|s| s.app_id.clone()))
        });
        SimView {
            sim: row.wire_id(),
            workspace: ws.id.clone(),
            workspace_handle: ws.handle.clone(),
            lane: row.lane,
            state: row.state,
            profile: row.profile.clone(),
            app_id,
            booted_at: row.booted_at,
            last_used_at: row.last_used_at,
            hold,
            viewers,
            reason: row.reason.clone(),
        }
    }

    fn emit_state(&self, row: &SimRow, ws: &Ws) {
        emit(
            "sim_state",
            json!({ "workspace": ws.id, "workspaceHandle": ws.handle, "workspacePath": ws.path.display().to_string(), "sim": row.wire_id(), "state": row.state, "reason": row.reason }),
        );
    }

    fn row(&self, ws: &Ws) -> R<Option<SimRow>> {
        let db = crate::db::shared();
        let conn = db.lock();
        Ok(store::by_project(&conn, &ws.id)?)
    }

    fn running_row(&self, ws: &Ws) -> R<SimRow> {
        match self.row(ws)? {
            Some(r) if r.state == SimState::Running => Ok(r),
            _ => Err(SimError::no_device()),
        }
    }

    fn dev(row: &SimRow) -> DeviceRef {
        let key = super::workspace_key(&row.project_id);
        DeviceRef {
            workspace_id: row.project_id.clone(),
            data_dir: super::device_dir(row.lane, &key),
            workspace_key: key,
            device_id: row.device_id.clone().unwrap_or_default(),
            lane_state: row.lane_state.clone(),
        }
    }

    fn lane_of(row: &SimRow) -> R<Arc<dyn SimLane>> {
        super::lane(row.lane).ok_or_else(|| SimError::new(503, "sim_unavailable", format!("the {} lane is not available on this server", row.lane)))
    }

    fn touch(&self, row: &SimRow) {
        let db = crate::db::shared();
        let conn = db.lock();
        let _ = store::touch(&conn, row.id, now());
    }

    pub fn status(&self, ws: &Ws, refresh: bool) -> R<StatusResp> {
        let lanes = self.probes_with_setup(refresh);
        let db = crate::db::shared();
        let conn = db.lock();
        let row = store::by_project(&conn, &ws.id)?;
        let running = store::count_occupying(&conn, None)?;
        let agents_allowed = store::agents_can_use_sim(&conn, &ws.id);
        drop(conn);
        let disk = self.disk.lock().clone().map(|(mut d, per_key)| {
            let key = super::workspace_key(&ws.id);
            d.this_device = per_key.get(&key).copied().unwrap_or(0);
            d
        });
        Ok(StatusResp {
            lanes,
            sim: row.as_ref().map(|r| self.view(r, ws)),
            running,
            limits: limits::current(),
            agents_allowed,
            can_control: false,
            disk,
        })
    }

    pub fn devices(&self, platform: Option<&str>) -> R<DevicesResp> {
        let lane = self.pick_lane(platform)?;
        Ok(DevicesResp { lane: lane.kind(), devices: lane.profiles()? })
    }

    // ── boot / stop / erase ──────────────────────────────────────────

    pub fn boot(&self, ws: &Ws, actor: &Actor, req: &BootReq) -> R<SimView> {
        let lock = self.op_lock(&ws.id);
        let _g = lock.lock();
        if let Some(r) = self.row(ws)? {
            if matches!(r.state, SimState::Running | SimState::Booting) {
                if let Some(p) = req.platform.as_deref().and_then(LaneKind::parse) {
                    if p.platform() != r.lane.platform() {
                        return Err(SimError::new(409, "sim_busy", format!("this workspace already runs a {} device; `k2 sim stop` it first", r.lane.platform())));
                    }
                }
                return Ok(self.view(&r, ws));
            }
        }
        let lane = self.pick_lane(req.platform.as_deref())?;
        let probe = lane.probe();
        if !probe.ready {
            let mut e = SimError::new(
                409,
                "sim_needs_setup",
                format!(
                    "the {} lane needs a one-time setup by the server's owner before devices can boot{}. The owner sees a card (Settings → K2 Server → Mobile devices).",
                    lane.kind(),
                    probe.detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
                ),
            )
            .exit(3);
            e.needs = probe.needs.clone();
            return Err(e);
        }
        // Box limits and disk.
        let lim = limits::current();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let running = store::count_occupying(&conn, Some(&ws.id))?;
            if let Err(r) = limits::check_boot(&lim, running, crate::disk_guard::box_mem().1) {
                return Err(SimError::new(409, "sim_limit", r.message()));
            }
        }
        let key = super::workspace_key(&ws.id);
        let data_dir = super::device_dir(lane.kind(), &key);
        let need = if data_dir.exists() { limits::BOOT_DISK_BYTES } else { limits::BOOT_DISK_BYTES * 2 };
        let probe_path = super::sim_home();
        let _ = std::fs::create_dir_all(&probe_path);
        if let crate::disk_guard::Verdict::Refuse(f) = crate::disk_guard::check(&[probe_path], need) {
            return Err(SimError::new(409, "disk_low", f.sentence("boot a device")));
        }
        std::fs::create_dir_all(&data_dir).map_err(|e| SimError::new(500, "sim_io", format!("create {}: {e}", data_dir.display())))?;
        let row = {
            let db = crate::db::shared();
            let conn = db.lock();
            store::begin_boot(&conn, &ws.id, lane.kind(), now(), &actor.audit())?
        };
        self.emit_state(&row, ws);
        let breq = BootRequest {
            workspace_id: ws.id.clone(),
            workspace_key: key,
            profile_id: req.device.clone(),
            data_dir,
            boot_timeout: BOOT_TIMEOUT,
        };
        let t0 = Instant::now();
        let res = lane.boot(&breq);
        let db = crate::db::shared();
        match res {
            Ok(b) => {
                let row = {
                    let conn = db.lock();
                    store::set_running(&conn, row.id, &b.device_id, &b.profile, &b.lane_state, b.pid, now())?;
                    store::audit(
                        &conn,
                        now(),
                        &actor.audit(),
                        Some(&ws.id),
                        Some(lane.kind()),
                        "boot",
                        json!({ "profile": b.profile.id, "ms": t0.elapsed().as_millis() as u64 }),
                    );
                    store::by_id(&conn, row.id)?.ok_or_else(|| SimError::new(500, "sim_db", "row vanished"))?
                };
                self.with_live(row.id, |l| {
                    *l = Live::default();
                });
                self.emit_state(&row, ws);
                if !req.no_tab {
                    emit(
                        "open_mobile",
                        json!({ "workspace": ws.id, "workspaceHandle": ws.handle, "workspacePath": ws.path.display().to_string(), "sim": row.wire_id() }),
                    );
                }
                Ok(self.view(&row, ws))
            }
            Err(e) => {
                let conn = db.lock();
                let msg = crate::redaction::redact(e.message());
                store::set_state(&conn, row.id, SimState::Failed, Some(&msg), now())?;
                store::audit(&conn, now(), &actor.audit(), Some(&ws.id), Some(lane.kind()), "boot_failed", json!({ "code": e.code() }));
                if let Some(r) = store::by_id(&conn, row.id)? {
                    drop(conn);
                    self.emit_state(&r, ws);
                }
                Err(e.into())
            }
        }
    }

    /// Stop the device (data kept). `reason` lands on the row (`idle`,
    /// `stopped by <who>`).
    pub fn stop(&self, ws: &Ws, actor: &Actor, reason: &str) -> R<SimView> {
        let lock = self.op_lock(&ws.id);
        let _g = lock.lock();
        let Some(row) = self.row(ws)? else { return Err(SimError::no_device()) };
        if !row.state.occupies_slot() {
            return Ok(self.view(&row, ws));
        }
        self.stop_row(&row, ws, actor, reason, "stop")
    }

    fn stop_row(&self, row: &SimRow, ws: &Ws, actor: &Actor, reason: &str, action: &str) -> R<SimView> {
        let db = crate::db::shared();
        {
            let conn = db.lock();
            store::set_state(&conn, row.id, SimState::Stopping, Some(reason), now())?;
        }
        let lane = Self::lane_of(row)?;
        let res = lane.stop(&Self::dev(row));
        let conn = db.lock();
        match res {
            Ok(()) => {
                store::set_state(&conn, row.id, SimState::Stopped, Some(reason), now())?;
                store::audit(&conn, now(), &actor.audit(), Some(&ws.id), Some(row.lane), action, json!({ "reason": reason }));
            }
            Err(e) => {
                let msg = crate::redaction::redact(e.message());
                store::set_state(&conn, row.id, SimState::Failed, Some(&msg), now())?;
                store::audit(&conn, now(), &actor.audit(), Some(&ws.id), Some(row.lane), "stop_failed", json!({ "code": e.code() }));
                drop(conn);
                if let Some(r) = self.row(ws)? {
                    self.emit_state(&r, ws);
                }
                return Err(e.into());
            }
        }
        let r = store::by_id(&conn, row.id)?.ok_or_else(|| SimError::new(500, "sim_db", "row vanished"))?;
        drop(conn);
        self.with_live(row.id, |l| {
            l.snapshot = None;
            l.hold = ControlHold::new();
        });
        self.emit_state(&r, ws);
        Ok(self.view(&r, ws))
    }

    /// Stop (if running) and delete the workspace's device data.
    pub fn erase(&self, ws: &Ws, actor: &Actor) -> R<Value> {
        let lock = self.op_lock(&ws.id);
        let _g = lock.lock();
        let Some(row) = self.row(ws)? else {
            return Ok(json!({ "ok": true, "erased": false, "line": "nothing to erase" }));
        };
        if row.state.occupies_slot() {
            self.stop_row(&row, ws, actor, "erased", "stop")?;
        }
        let lane = Self::lane_of(&row)?;
        let key = super::workspace_key(&ws.id);
        let dir = super::device_dir(row.lane, &key);
        let freed = dir_size(&dir);
        lane.erase(&key, &dir)?;
        let db = crate::db::shared();
        let conn = db.lock();
        store::delete(&conn, row.id)?;
        store::audit(&conn, now(), &actor.audit(), Some(&ws.id), Some(row.lane), "erase", json!({ "freedBytes": freed }));
        drop(conn);
        self.live.lock().remove(&row.id);
        emit("sim_state", json!({ "workspace": ws.id, "workspaceHandle": ws.handle, "sim": row.wire_id(), "state": "erased" }));
        Ok(json!({ "ok": true, "erased": true, "freedBytes": freed, "line": format!("erased {} device data ({} MB)", row.lane, freed / (1024 * 1024)) }))
    }

    // ── apps ─────────────────────────────────────────────────────────

    pub fn install_app(&self, ws: &Ws, actor: &Actor, path: &str) -> R<InstallResp> {
        let row = self.running_row(ws)?;
        let artifact = inside_workspace(&ws.path, path)?;
        let lock = self.op_lock(&ws.id);
        let _g = lock.lock();
        let t0 = Instant::now();
        let out = Self::lane_of(&row)?.install_app(&Self::dev(&row), &artifact)?;
        self.touch(&row);
        let db = crate::db::shared();
        let conn = db.lock();
        store::audit(&conn, now(), &actor.audit(), Some(&ws.id), Some(row.lane), "install", json!({ "app": out.app_id, "version": out.version }));
        let line = format!(
            "ok install {}{} · {}ms",
            out.app_id,
            out.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default(),
            t0.elapsed().as_millis()
        );
        Ok(InstallResp { outcome: out, line })
    }

    pub fn launch(&self, ws: &Ws, actor: &Actor, app: &str, stop: bool) -> R<ActResp> {
        let row = self.running_row(ws)?;
        valid_app_id(app)?;
        if actor.is_agent() {
            self.agent_turn(&row)?;
        }
        let t0 = Instant::now();
        let lane = Self::lane_of(&row)?;
        if stop {
            lane.terminate(&Self::dev(&row), app)?;
        } else {
            lane.launch(&Self::dev(&row), app)?;
        }
        self.touch(&row);
        let verb = if stop { "terminate" } else { "launch" };
        let ms = t0.elapsed().as_millis() as u64;
        Ok(ActResp { ok: true, line: format!("ok {verb} {app} · {ms}ms"), diff: vec![], snapshot: None, took_ms: ms })
    }

    // ── tree ─────────────────────────────────────────────────────────

    fn read_tree(&self, row: &SimRow) -> R<Snapshot> {
        let raw = Self::lane_of(row)?.tree(&Self::dev(row))?;
        let snap = self.with_live(row.id, |l| {
            l.snap_seq += 1;
            let s = Snapshot::build(l.snap_seq, &raw);
            l.snapshot = Some(s.clone());
            s
        });
        Ok(snap)
    }

    fn header(row: &SimRow, snap: &Snapshot) -> String {
        format!(
            "sim {} · {} · {}{}",
            row.wire_id(),
            row.lane,
            row.profile.as_ref().map(|p| p.name.as_str()).unwrap_or("device"),
            snap.app_id.as_deref().map(|a| format!(" · {a}")).unwrap_or_default()
        )
    }

    pub fn tree(&self, ws: &Ws, actor: &Actor, all: bool, find: Option<&str>, boxes: bool) -> R<TreeResp> {
        let row = self.running_row(ws)?;
        let snap = self.read_tree(&row)?;
        if actor.is_agent() {
            self.with_live(row.id, |l| l.hold.agent_read_tree());
            self.touch(&row);
        }
        let text = snap.render(&Self::header(&row, &snap), all, find, boxes);
        Ok(TreeResp { snapshot: snap, text })
    }

    /// The latest snapshot (picking in the tab; no device read).
    pub fn last_snapshot(&self, ws: &Ws) -> R<Option<Snapshot>> {
        let row = self.running_row(ws)?;
        Ok(self.with_live(row.id, |l| l.snapshot.clone()))
    }

    // ── act ──────────────────────────────────────────────────────────

    /// The agent waits up to [`AGENT_WAIT`] for a person's hold to end.
    fn agent_turn(&self, row: &SimRow) -> R<()> {
        let deadline = Instant::now() + AGENT_WAIT;
        loop {
            let now_i = Instant::now();
            let r = self.with_live(row.id, |l| l.hold.agent_check(now_i));
            match r {
                Ok(()) => return Ok(()),
                Err(AgentBlocked::Held { retry_after: Some(d), .. }) if now_i + d <= deadline => {
                    std::thread::sleep(d + Duration::from_millis(10));
                }
                Err(b @ AgentBlocked::Held { .. }) => return Err(SimError::new(409, "sim_held", b.message()).exit(2)),
                Err(b @ AgentBlocked::ReadTreeFirst) => return Err(SimError::new(409, "sim_read_tree", b.message()).exit(2)),
            }
        }
    }

    fn resolve_point(&self, row: &SimRow, id: &str) -> R<(Point, Option<super::tree::Rect>)> {
        let snap = self.with_live(row.id, |l| l.snapshot.clone());
        let Some(snap) = snap else {
            return Err(SimError::new(409, "sim_stale_element", format!("no snapshot yet for {id}. Run `k2 sim tree` first.")));
        };
        let el = snap.resolve(id).map_err(|e| SimError::new(409, "sim_stale_element", e.to_string()))?;
        let p = snap.tap_point(&el.id).map_err(|e| SimError::new(409, "sim_stale_element", e.to_string()))?;
        Ok((p, el.frame))
    }

    fn screen_size(&self, row: &SimRow) -> (f32, f32) {
        let s = self.with_live(row.id, |l| l.snapshot.as_ref().map(|s| (s.width_pt, s.height_pt)));
        s.or_else(|| row.profile.as_ref().map(|p| (p.width_pt as f32, p.height_pt as f32))).unwrap_or((400.0, 800.0))
    }

    /// Turn an [`ActSpec`] into lane actions. `secret` carries the vault
    /// value for `type --secret` (the daemon resolved it).
    fn plan(&self, row: &SimRow, spec: &ActSpec, secret: Option<SimText>) -> R<(Vec<Action>, String)> {
        Ok(match spec {
            ActSpec::Tap { element, x, y, hold_ms } => {
                let (at, label) = match (element, x, y) {
                    (Some(id), _, _) => (self.resolve_point(row, id)?.0, id.clone()),
                    (None, Some(x), Some(y)) => (Point { x: *x, y: *y }, format!("{},{}", x.round(), y.round())),
                    _ => return Err(SimError::new(400, "sim_invalid", "tap needs an element id or --at x,y")),
                };
                let (w, h) = self.screen_size(row);
                if at.x < 0.0 || at.y < 0.0 || at.x > w || at.y > h {
                    return Err(SimError::new(400, "sim_invalid", format!("{label} is off the {}x{} pt screen", w.round(), h.round())));
                }
                match hold_ms {
                    Some(ms) if *ms > 0 => (vec![Action::LongPress { at, hold: Duration::from_millis((*ms).min(10_000) as u64) }], format!("long-press {label}")),
                    _ => (vec![Action::Tap { at }], format!("tap {label}")),
                }
            }
            ActSpec::Type { into, text, secret: name } => {
                let t = match (text, name, secret) {
                    (_, Some(_), Some(s)) => s,
                    (_, Some(n), None) => return Err(SimError::new(400, "sim_invalid", format!("secret '{n}' was not resolved"))),
                    (Some(t), None, _) => SimText::plain(t.clone()),
                    (None, None, _) => return Err(SimError::new(400, "sim_invalid", "type needs text or --secret <name>")),
                };
                if t.len_chars() > 4096 {
                    return Err(SimError::new(400, "sim_invalid", "type is limited to 4096 characters"));
                }
                let shown = if t.secret { format!("secret ({} chars)", t.len_chars()) } else { format!("{} chars", t.len_chars()) };
                let mut acts = Vec::new();
                let mut label = format!("type {shown}");
                if let Some(id) = into {
                    acts.push(Action::Tap { at: self.resolve_point(row, id)?.0 });
                    label = format!("type {shown} into {id}");
                }
                acts.push(Action::Type { text: t });
                (acts, label)
            }
            ActSpec::Swipe { direction, from } => {
                let rect = match from {
                    Some(id) => self.resolve_point(row, id)?.1,
                    None => None,
                };
                let (w, h) = self.screen_size(row);
                let r = rect.unwrap_or(super::tree::Rect { x: 0.0, y: 0.0, w, h });
                let c = r.center();
                let (dx, dy) = (r.w * 0.3, r.h * 0.3);
                // "swipe up" = the finger moves up (content scrolls down).
                let (a, b) = match direction {
                    Direction::Up => (Point { x: c.x, y: c.y + dy }, Point { x: c.x, y: c.y - dy }),
                    Direction::Down => (Point { x: c.x, y: c.y - dy }, Point { x: c.x, y: c.y + dy }),
                    Direction::Left => (Point { x: c.x + dx, y: c.y }, Point { x: c.x - dx, y: c.y }),
                    Direction::Right => (Point { x: c.x - dx, y: c.y }, Point { x: c.x + dx, y: c.y }),
                };
                let d = format!("{direction:?}").to_lowercase();
                (
                    vec![Action::Swipe { from: a, to: b, duration: Duration::from_millis(300) }],
                    match from {
                        Some(id) => format!("swipe {d} from {id}"),
                        None => format!("swipe {d}"),
                    },
                )
            }
            ActSpec::Key { key } => (vec![Action::Key { key: *key }], format!("key {}", serde_plain(key))),
            ActSpec::Button { button } => (vec![Action::Button { button: *button }], format!("button {}", serde_plain(button))),
            ActSpec::Rotate { turn, orientation } => {
                let cur = self.with_live(row.id, |l| l.orientation.unwrap_or(Orientation::Portrait));
                let to = match (orientation, turn) {
                    (Some(o), _) => *o,
                    (None, Some(Direction::Right)) => turn_right(cur),
                    (None, Some(Direction::Left)) => turn_right(turn_right(turn_right(cur))),
                    _ => return Err(SimError::new(400, "sim_invalid", "rotate left|right (or an orientation)")),
                };
                (vec![Action::Rotate { orientation: to }], format!("rotate {}", serde_plain(&to)))
            }
            ActSpec::OpenUrl { url } => {
                if url.len() > 2048 || url.chars().any(char::is_control) || !url.contains(':') {
                    return Err(SimError::new(400, "sim_invalid", "open-url needs a URL or deep link (scheme:…), at most 2048 characters"));
                }
                (vec![Action::OpenUrl { url: url.clone() }], "open-url".to_string())
            }
            ActSpec::Location { lat, lon } => {
                if !(-90.0..=90.0).contains(lat) || !(-180.0..=180.0).contains(lon) {
                    return Err(SimError::new(400, "sim_invalid", "location needs lat -90..90 and lon -180..180"));
                }
                (vec![Action::Location { lat: *lat, lon: *lon }], format!("location {lat:.4},{lon:.4}"))
            }
            ActSpec::Appearance { appearance } => {
                (vec![Action::Appearance { appearance: *appearance }], format!("appearance {}", serde_plain(appearance)))
            }
        })
    }

    pub fn act(&self, ws: &Ws, actor: &Actor, req: &ActReq, secret: Option<SimText>) -> R<ActResp> {
        let row = self.running_row(ws)?;
        let t0 = Instant::now();
        if actor.is_agent() {
            self.agent_turn(&row)?;
        } else {
            self.with_live(row.id, |l| l.hold.person_input(&actor.hold_name(), Instant::now()));
        }
        let (actions, label) = self.plan(&row, &req.action, secret)?;
        let before = self.with_live(row.id, |l| l.snapshot.clone());
        let lane = Self::lane_of(&row)?;
        let dev = Self::dev(&row);
        for a in &actions {
            lane.act(&dev, a)?;
            if let Action::Rotate { orientation } = a {
                self.with_live(row.id, |l| l.orientation = Some(*orientation));
            }
        }
        self.touch(&row);
        let act_ms = t0.elapsed().as_millis() as u64;
        let (diff, snap_id) = if req.no_diff {
            (vec![], None)
        } else {
            std::thread::sleep(SETTLE);
            let after = self.read_tree(&row)?;
            if actor.is_agent() {
                self.with_live(row.id, |l| l.hold.agent_read_tree());
            }
            let d = match &before {
                Some(b) => after.diff(b, DIFF_MAX),
                None => vec![],
            };
            (d, Some(after.id.clone()))
        };
        let mut line = format!("ok {label} · {act_ms}ms");
        if let Some(s) = &snap_id {
            if diff.is_empty() {
                line.push_str(&format!(" · no change ({s})"));
            } else {
                line.push_str(&format!(" · {} ({s})", diff.join(" · ")));
            }
        }
        Ok(ActResp { ok: true, line, diff, snapshot: snap_id, took_ms: t0.elapsed().as_millis() as u64 })
    }

    // ── screenshot / logs / wait ─────────────────────────────────────

    pub fn screenshot(&self, ws: &Ws, actor: &Actor, region: Option<&str>) -> R<ScreenshotResp> {
        let row = self.running_row(ws)?;
        let rect = match region {
            Some(id) => Some(self.resolve_point(&row, id)?.1.ok_or_else(|| SimError::new(409, "sim_stale_element", format!("{id} has no position")))?),
            None => None,
        };
        let f = Self::lane_of(&row)?.screenshot(&Self::dev(&row))?;
        let (pw, ph) = encode::points_size(&f);
        let mut img = encode::scale_to(encode::to_rgb(&f).map_err(|e| SimError::new(502, "sim_tool_failed", e))?, pw, ph);
        if let Some(r) = rect {
            img = encode::crop_points(&img, r).map_err(|e| SimError::new(400, "sim_invalid", e))?;
        }
        let png = encode::png(&img).map_err(|e| SimError::new(500, "sim_io", e))?;
        let dir = ws.path.join(".k2").join("sim").join("shots");
        std::fs::create_dir_all(&dir).map_err(|e| SimError::new(500, "sim_io", format!("create {}: {e}", dir.display())))?;
        let name = format!("{}-{}{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S%3f"), row.wire_id(), region.map(|r| format!("-{r}")).unwrap_or_default());
        let path = dir.join(sanitize_file(&name));
        crate::fs_atomic::atomic_write(&path, &png).map_err(|e| SimError::new(500, "sim_io", format!("write {}: {e}", path.display())))?;
        if actor.is_agent() {
            self.touch(&row);
        }
        Ok(ScreenshotResp { path: path.display().to_string(), width: img.width(), height: img.height() })
    }

    pub fn logs(&self, ws: &Ws, app: Option<&str>, since: Option<Duration>, grep: Option<&str>, max_lines: Option<usize>) -> R<LogsResp> {
        let row = self.running_row(ws)?;
        if let Some(a) = app {
            valid_app_id(a)?;
        }
        let max_lines = max_lines.unwrap_or(LOG_DEFAULT_LINES).clamp(1, LOG_MAX_LINES);
        let q = LogQuery {
            app_id: app.map(str::to_string),
            since,
            grep: grep.map(|g| g.chars().take(200).collect()),
            max_lines: max_lines + 1,
            max_bytes: LOG_MAX_BYTES,
        };
        let raw = Self::lane_of(&row)?.logs(&Self::dev(&row), &q)?;
        let mut truncated = raw.len() > max_lines;
        let start = raw.len().saturating_sub(max_lines);
        let mut bytes = 0usize;
        let mut lines = Vec::new();
        for l in &raw[start..] {
            let l: String = l.chars().take(2000).collect();
            let red = crate::redaction::redact(&l);
            bytes += red.len();
            if bytes > LOG_MAX_BYTES {
                truncated = true;
                break;
            }
            lines.push(red);
        }
        Ok(LogsResp { lines, truncated })
    }

    pub fn wait(&self, ws: &Ws, actor: &Actor, req: &WaitReq) -> R<WaitResp> {
        let t0 = Instant::now();
        let timeout = req.timeout_ms.map(Duration::from_millis).unwrap_or(WAIT_DEFAULT).min(WAIT_MAX);
        let needle = req.for_text.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty());
        loop {
            let row = self.running_row(ws)?;
            match &needle {
                Some(n) => {
                    let snap = self.read_tree(&row)?;
                    if actor.is_agent() {
                        self.with_live(row.id, |l| l.hold.agent_read_tree());
                    }
                    let hit = snap.elements.iter().find(|e| {
                        [&e.label, &e.value, &e.identifier].iter().filter_map(|v| v.as_deref()).any(|v| v.to_lowercase().contains(n.as_str()))
                    });
                    if let Some(e) = hit {
                        let ms = t0.elapsed().as_millis() as u64;
                        return Ok(WaitResp {
                            ok: true,
                            line: format!("ok found {} {} · {ms}ms ({})", e.id, e.describe(), snap.id),
                            element: Some(e.id.clone()),
                            snapshot: Some(snap.id.clone()),
                            took_ms: ms,
                        });
                    }
                }
                None => {
                    let free = self.with_live(row.id, |l| matches!(l.hold.view(Instant::now()), HoldView::Free));
                    if free {
                        let ms = t0.elapsed().as_millis() as u64;
                        return Ok(WaitResp { ok: true, line: format!("ok device free · {ms}ms"), element: None, snapshot: None, took_ms: ms });
                    }
                }
            }
            if t0.elapsed() >= timeout {
                let what = match &req.for_text {
                    Some(t) => format!("\"{t}\" did not appear"),
                    None => "a person still holds the device".to_string(),
                };
                return Err(SimError::new(408, "sim_wait_timeout", format!("{what} within {}s", timeout.as_secs())).exit(1));
            }
            std::thread::sleep(Duration::from_millis(if needle.is_some() { 300 } else { 200 }));
        }
    }

    // ── people: hold, viewers, input from the stream ─────────────────

    pub fn hold(&self, ws: &Ws, actor: &Actor, action: HoldAction) -> R<SimView> {
        if actor.is_agent() {
            return Err(SimError::new(403, "forbidden", "take over and hand back are for people at the Mobile tab"));
        }
        let row = self.running_row(ws)?;
        let who = actor.hold_name();
        let now_i = Instant::now();
        let changed = self.with_live(row.id, |l| match action {
            HoldAction::TakeOver => {
                l.hold.take_over(&who, now_i);
                true
            }
            HoldAction::HandBack => l.hold.hand_back(&who, now_i),
        });
        if changed {
            let db = crate::db::shared();
            let conn = db.lock();
            let a = if action == HoldAction::TakeOver { "take_over" } else { "hand_back" };
            store::audit(&conn, now(), &actor.audit(), Some(&ws.id), Some(row.lane), a, json!({}));
        }
        self.touch(&row);
        let v = self.view(&row, ws);
        // After a hand-back the agent's next input is refused once with
        // "run `k2 sim tree` first" (hold.rs), which is how it is told.
        emit("sim_hold", json!({ "workspace": ws.id, "workspaceHandle": ws.handle, "sim": row.wire_id(), "hold": v.hold }));
        Ok(v)
    }

    /// A person's input from the stream (S3): hold, touch the idle clock,
    /// send to the lane.
    pub fn person_action(&self, ws: &Ws, who: &str, action: &Action) -> R<()> {
        let row = self.running_row(ws)?;
        let (was, now_v) = self.with_live(row.id, |l| {
            let was = l.hold.view(Instant::now());
            l.hold.person_input(who, Instant::now());
            (was, l.hold.view(Instant::now()))
        });
        Self::lane_of(&row)?.act(&Self::dev(&row), action)?;
        if let Action::Rotate { orientation } = action {
            self.with_live(row.id, |l| l.orientation = Some(*orientation));
        }
        self.touch(&row);
        if matches!(was, HoldView::Free) {
            emit("sim_hold", json!({ "workspace": ws.id, "sim": row.wire_id(), "hold": now_v }));
        }
        Ok(())
    }

    pub fn viewer_joined(&self, sim_id: i64) {
        self.with_live(sim_id, |l| l.viewers += 1);
    }

    pub fn viewer_left(&self, sim_id: i64) {
        self.with_live(sim_id, |l| l.viewers = l.viewers.saturating_sub(1));
    }

    pub fn hold_view(&self, sim_id: i64) -> HoldView {
        self.with_live(sim_id, |l| l.hold.view(Instant::now()))
    }

    /// The running row for a workspace (the stream arm).
    pub fn running(&self, ws: &Ws) -> R<SimRow> {
        self.running_row(ws)
    }

    pub fn frames(&self, row: &SimRow, opts: FrameOpts) -> R<Box<dyn FrameSource>> {
        Ok(Self::lane_of(row)?.frames(&Self::dev(row), opts)?)
    }

    // ── owner setup: licence + first-use install ─────────────────────

    pub fn licence(&self, lane: LaneKind, id: &str) -> R<LicenceText> {
        let l = super::lane(lane).ok_or_else(|| SimError::new(404, "sim_unavailable", format!("no {lane} lane here")))?;
        Ok(l.licence(id)?)
    }

    /// The box owner's setup (the card, Settings, or the owner token):
    /// record the accepted licence (bound to its text's sha1), then run the
    /// lane's first-use install in the background. `by` must already be
    /// checked to be the owner (owner token or an Owner login); agents
    /// never reach this (M-A23).
    pub fn start_setup(&self, lane_kind: LaneKind, accept: Option<(&str, &str)>, by: &str) -> R<()> {
        let lane = super::lane(lane_kind).ok_or_else(|| SimError::new(404, "sim_unavailable", format!("no {lane_kind} lane here")))?;
        let probe = lane.probe();
        if !probe.usable {
            return Err(SimError::new(503, "sim_unavailable", probe.detail.unwrap_or_else(|| format!("the {lane_kind} lane can't run here"))));
        }
        for need in &probe.needs {
            if let LaneNeed::Licence { id, .. } = need {
                let text = lane.licence(id)?;
                match accept {
                    Some((aid, sha)) if aid == id && sha == text.sha1 => {
                        super::licence::record(super::licence::Accepted {
                            lane: lane_kind,
                            id: id.clone(),
                            sha1: text.sha1.clone(),
                            by: by.to_string(),
                            at: now(),
                        })
                        .map_err(|e| SimError::new(500, "sim_io", e))?;
                        let db = crate::db::shared();
                        let conn = db.lock();
                        store::audit(&conn, now(), by, None, Some(lane_kind), "licence_accept", json!({ "licence": id, "sha1": text.sha1 }));
                    }
                    Some((aid, sha)) if aid == id => {
                        return Err(SimError::new(
                            409,
                            "sim_licence_changed",
                            format!("the {id} text changed since the card was shown (now {}, card {sha}). Read it again and accept the new text.", text.sha1),
                        ));
                    }
                    _ => {
                        let mut e = SimError::new(
                            409,
                            "sim_needs_setup",
                            format!("the server's owner must accept \"{}\" first (licence {id}, sha1 {})", text.title, text.sha1),
                        )
                        .exit(3);
                        e.needs = probe.needs.clone();
                        return Err(e);
                    }
                }
            }
        }
        {
            let mut s = self.setup.lock();
            if s.get(&lane_kind).is_some_and(|x| x.running) {
                return Ok(());
            }
            s.insert(lane_kind, SetupState { running: true, ..Default::default() });
        }
        let accepted = super::licence::accepted_ids(lane_kind);
        let by = by.to_string();
        std::thread::Builder::new()
            .name(format!("k2-sim-setup-{lane_kind}"))
            .spawn(move || {
                let m = manager();
                let progress = |done: u64, total: u64| {
                    let mut s = m.setup.lock();
                    if let Some(st) = s.get_mut(&lane_kind) {
                        st.done = done;
                        st.total = total;
                    }
                };
                let t0 = Instant::now();
                let res = lane.install(&accepted, &progress);
                let err = res.as_ref().err().map(|e| crate::redaction::redact(e.message()));
                {
                    let mut s = m.setup.lock();
                    if let Some(st) = s.get_mut(&lane_kind) {
                        st.running = false;
                        st.error = err.clone();
                    }
                }
                let db = crate::db::shared();
                let conn = db.lock();
                store::audit(
                    &conn,
                    now(),
                    &by,
                    None,
                    Some(lane_kind),
                    if err.is_none() { "setup" } else { "setup_failed" },
                    json!({ "secs": t0.elapsed().as_secs(), "error": err }),
                );
                drop(conn);
                m.refresh_probes();
                emit("sim_state", json!({ "lane": lane_kind, "state": if err.is_none() { "setup-done" } else { "setup-failed" } }));
            })
            .map_err(|e| SimError::new(500, "sim_io", format!("spawn setup: {e}")))?;
        Ok(())
    }

    /// Probes with any running setup's progress folded into `detail`.
    pub fn probes_with_setup(&self, refresh: bool) -> Vec<LaneProbe> {
        let mut v = if refresh { self.refresh_probes() } else { self.probes() };
        let s = self.setup.lock();
        for p in &mut v {
            if let Some(st) = s.get(&p.lane) {
                if st.running {
                    p.ready = false;
                    p.detail = Some(if st.total > 0 {
                        format!("installing: {}% of {:.1} GB", st.done * 100 / st.total.max(1), st.total as f64 / 1e9)
                    } else {
                        "installing…".to_string()
                    });
                } else if let Some(e) = &st.error {
                    p.detail = Some(format!("the last setup failed: {e}"));
                }
            }
        }
        v
    }

    /// Measure `~/.k2/sim` (tools vs device data, per lane). Walks the
    /// tree, so the daemon runs it on a slow timer (every 10 min) and after
    /// setup / erase, never per request.
    pub fn measure_disk(&self) -> SimDisk {
        let mut d = SimDisk { measured_at: now(), ..Default::default() };
        let mut per_key = HashMap::new();
        for l in super::lanes() {
            let root = super::sim_home().join(l.kind().as_str());
            let devices = root.join("devices");
            let total = dir_size(&root);
            let mut dev_total = 0u64;
            if let Ok(rd) = std::fs::read_dir(&devices) {
                for e in rd.flatten() {
                    let n = dir_size(&e.path());
                    dev_total += n;
                    per_key.insert(e.file_name().to_string_lossy().to_string(), n);
                }
            }
            d.tools.push((l.kind(), total.saturating_sub(dev_total)));
            d.devices.push((l.kind(), dev_total));
        }
        *self.disk.lock() = Some((d.clone(), per_key));
        d
    }

    // ── background: adopt, reaper ────────────────────────────────────

    /// At daemon start: re-attach recorded devices that still run; mark the
    /// rest stopped.
    pub fn adopt_all(&self) -> Vec<String> {
        let rows = {
            let db = crate::db::shared();
            let conn = db.lock();
            store::all(&conn).unwrap_or_default()
        };
        let mut out = Vec::new();
        for row in rows.into_iter().filter(|r| r.state.occupies_slot()) {
            let Some(lane) = super::lane(row.lane) else {
                self.mark(&row, SimState::Stopped, "this server no longer runs that lane", "adopt_lost");
                continue;
            };
            match lane.adopt(&Self::dev(&row)) {
                Ok(Some(b)) => {
                    let db = crate::db::shared();
                    let conn = db.lock();
                    let _ = store::set_lane_state(&conn, row.id, &b.lane_state, b.pid);
                    if row.state != SimState::Running {
                        let _ = store::set_running(&conn, row.id, &b.device_id, &b.profile, &b.lane_state, b.pid, now());
                    }
                    out.push(format!("{} adopted", row.wire_id()));
                }
                Ok(None) => {
                    self.mark(&row, SimState::Stopped, "the device stopped while K2 restarted", "adopt_lost");
                    out.push(format!("{} gone", row.wire_id()));
                }
                Err(e) => {
                    self.mark(&row, SimState::Failed, &crate::redaction::redact(e.message()), "adopt_lost");
                    out.push(format!("{} failed: {}", row.wire_id(), e.code()));
                }
            }
        }
        out
    }

    fn mark(&self, row: &SimRow, state: SimState, reason: &str, action: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        let _ = store::set_state(&conn, row.id, state, Some(reason), now());
        store::audit(&conn, now(), "k2", Some(&row.project_id), Some(row.lane), action, json!({ "reason": reason }));
    }

    /// One reaper pass (every ~30 s): dead devices, idle shutdown,
    /// take-over expiry, unused data. `ws_of` resolves a project id to its
    /// workspace (None = the workspace is gone).
    pub fn sweep(&self, ws_of: &dyn Fn(&str) -> Option<Ws>) -> Vec<String> {
        let lim = limits::current();
        let rows = {
            let db = crate::db::shared();
            let conn = db.lock();
            store::all(&conn).unwrap_or_default()
        };
        let mut out = Vec::new();
        let t = now();
        let system = Actor::Person { name: "k2".into() };
        for row in rows {
            let ws = ws_of(&row.project_id);
            if row.state == SimState::Running {
                let Some(lane) = super::lane(row.lane) else { continue };
                if let Some(HoldEvent::TakeoverExpired { .. }) = self.with_live(row.id, |l| l.hold.tick(Instant::now())) {
                    if let Some(ws) = &ws {
                        emit("sim_hold", json!({ "workspace": ws.id, "sim": row.wire_id(), "hold": HoldView::Free }));
                    }
                }
                if !lane.alive(&Self::dev(&row)) {
                    self.mark(&row, SimState::Stopped, "the device exited", "exited");
                    if let Some(ws) = &ws {
                        if let Ok(Some(r)) = self.row(ws) {
                            self.emit_state(&r, ws);
                        }
                    }
                    out.push(format!("{} exited", row.wire_id()));
                    continue;
                }
                let viewers = self.with_live(row.id, |l| l.viewers);
                if viewers == 0 && t - row.last_used_at >= lim.idle_shutdown_secs as i64 {
                    if let Some(ws) = &ws {
                        let lock = self.op_lock(&ws.id);
                        let _g = lock.lock();
                        match self.stop_row(&row, ws, &system, "idle", "idle_stop") {
                            Ok(_) => out.push(format!("{} idle stop", row.wire_id())),
                            Err(e) => out.push(format!("{} idle stop failed: {}", row.wire_id(), e.code)),
                        }
                    }
                }
            } else if !row.state.occupies_slot() {
                let last = row.last_run_at.unwrap_or(row.created_at);
                if t - last >= lim.unused_data_days as i64 * 86_400 {
                    let key = super::workspace_key(&row.project_id);
                    let dir = super::device_dir(row.lane, &key);
                    if let Some(lane) = super::lane(row.lane) {
                        match lane.erase(&key, &dir) {
                            Ok(()) => {
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = store::delete(&conn, row.id);
                                store::audit(&conn, t, "k2", Some(&row.project_id), Some(row.lane), "data_expired", json!({ "days": lim.unused_data_days }));
                                out.push(format!("{} data expired", row.wire_id()));
                            }
                            Err(e) => out.push(format!("{} data expiry failed: {}", row.wire_id(), e.code())),
                        }
                    }
                }
            }
        }
        // Device folders whose workspace (and row) is gone.
        for lane in super::lanes() {
            let devices = super::sim_home().join(lane.kind().as_str()).join("devices");
            let Ok(rd) = std::fs::read_dir(&devices) else { continue };
            let keys: std::collections::HashSet<String> = {
                let db = crate::db::shared();
                let conn = db.lock();
                store::all(&conn).unwrap_or_default().iter().map(|r| super::workspace_key(&r.project_id)).collect()
            };
            for e in rd.flatten() {
                let key = e.file_name().to_string_lossy().to_string();
                if !keys.contains(&key) && e.path().is_dir() {
                    match lane.erase(&key, &e.path()) {
                        Ok(()) => out.push(format!("{} orphan {key} erased", lane.kind())),
                        Err(err) => out.push(format!("{} orphan {key}: {}", lane.kind(), err.code())),
                    }
                }
            }
        }
        out
    }
}

impl Default for Manager {
    fn default() -> Self {
        Manager::new()
    }
}

fn default_order() -> Vec<LaneKind> {
    if cfg!(target_os = "macos") {
        vec![LaneKind::Ios, LaneKind::Android, LaneKind::IosNode]
    } else {
        vec![LaneKind::Android, LaneKind::IosNode, LaneKind::Ios]
    }
}

fn turn_right(o: Orientation) -> Orientation {
    match o {
        Orientation::Portrait => Orientation::LandscapeRight,
        Orientation::LandscapeRight => Orientation::PortraitUpsideDown,
        Orientation::PortraitUpsideDown => Orientation::LandscapeLeft,
        Orientation::LandscapeLeft => Orientation::Portrait,
    }
}

/// `serde` name of a unit enum value (`home`, `landscape-left`).
fn serde_plain<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Package names and bundle ids: letters, digits, `.`, `_`, `-`.
pub fn valid_app_id(a: &str) -> R<()> {
    if a.is_empty() || a.len() > 255 || !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') || a.starts_with('-') {
        return Err(SimError::new(400, "sim_invalid", format!("'{a}' is not a package name or bundle id")));
    }
    Ok(())
}

/// Resolve `p` (absolute, or relative to the workspace) and require it to
/// be a file inside the workspace (symlinks resolved).
pub fn inside_workspace(ws: &Path, p: &str) -> R<PathBuf> {
    let candidate = if Path::new(p).is_absolute() { PathBuf::from(p) } else { ws.join(p) };
    let real = candidate
        .canonicalize()
        .map_err(|e| SimError::new(400, "sim_invalid", format!("{}: {e}", candidate.display())))?;
    let root = ws.canonicalize().map_err(|e| SimError::new(400, "sim_invalid", format!("{}: {e}", ws.display())))?;
    if !real.starts_with(&root) {
        return Err(SimError::new(403, "forbidden", format!("{} is outside this workspace", real.display())));
    }
    if !real.is_file() && !real.is_dir() {
        return Err(SimError::new(400, "sim_invalid", format!("{} is not a file", real.display())));
    }
    Ok(real)
}

fn sanitize_file(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Total bytes under `dir` (best effort; symlinks not followed).
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.path().symlink_metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                total += m.len();
            }
        }
    }
    total
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
