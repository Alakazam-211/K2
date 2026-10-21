//! One node, many servers (prd-compute-shared-queue-v1, protocol 3).
//!
//! The SSOT for node sharing, used by the node (`k2-node`), the root
//! executor and the server (`k2-core` / `k2-daemon`):
//!
//! - **slots**: each connected server is a *tenant* with a slot 0–7 on one
//!   node; slot 0 is the owner server (the node's first enrolment);
//! - **classes** (`gate` > `interactive` > `batch`), **trust**, **network**;
//! - **shares**: one tenant's limits ([`ShareLimits`]), partial changes
//!   ([`ShareOverrides`]) and the narrow/widen rule both ends use;
//! - **`servers.toml`** ([`ServersFile`]): the node owner's decisions only
//!   (V45). The node never writes it; its own runtime state per server
//!   lives in node-owned pin files;
//! - the wire views: [`ShareView`] (in `Offer.share`), [`Quote`],
//!   [`QueuePos`] (in `StateUpdate.queue`), [`QueueGet`] / [`QueueView`];
//! - the `servers.*` admin verb arguments and the executor's internal
//!   `tenant.*` op arguments;
//! - the new reason codes ([`codes`]) and `blocked_by` values ([`blocked`]).
//!
//! Every field added to an existing frame is optional and absent when
//! empty, and the node sends sharing fields, features and frames only on
//! sessions whose server said protocol ≥ [`crate::frames::PROTOCOL_SHARED`]
//! in `Challenge.protocol`. Frames to a protocol-2 server stay identical to
//! 0.46.2's except the four version fields (V66; `k2-node/tests/golden_solo.rs`).
//!
//! No floats anywhere: CPU-hours travel as CPU-seconds.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Offer feature: this node takes more than one server (`servers.*` verbs).
pub const FEATURE_SERVERS: &str = "servers";
/// Offer feature: this node keeps a fair queue across its servers
/// ([`QueueGet`], `Cancel` reasons `withdraw` / `if_queued`, `Plan.class`…).
pub const FEATURE_QUEUE: &str = "queue";
/// Offer feature: this node walls tenants from each other (Linux: a user
/// per tenant; macOS 0.46.3: a per-tenant Seatbelt profile, `own` only).
pub const FEATURE_TENANT_WALL: &str = "tenant-wall";

/// A tenant's number on one node (0–7). Every node-side name a tenant can
/// touch carries its slot (V48).
pub type Slot = u8;
/// The owner server's slot: today's `controller.json`, ledger and paths.
pub const OWNER_SLOT: Slot = 0;
/// Slots are `0..MAX_SLOTS`.
pub const MAX_SLOTS: u8 = 8;
/// `servers.toml` `max_servers` when absent: 3 servers per node (the owner
/// plus two; Rosson 2026-10-21, proposed for the Mac minis, pending his
/// final answer). The node owner may set 1 (no sharing) up to [`MAX_SLOTS`].
pub const DEFAULT_MAX_SERVERS: u8 = 3;
/// A new server's disk budget on a Mac (the minis' 20 GB per extra server
/// until the disk floor counts the Sew VMs, Q13); elsewhere
/// [`DEFAULT_DISK_GB`].
pub const MAC_DISK_GB: u32 = 20;
pub const DEFAULT_DISK_GB: u32 = 40;
/// A pending add waits this long for its SAS (§4.1).
pub const PENDING_SECS: i64 = 600;
/// Quotes per tenant: its last four distinct `(cpu, mem)` asks (V53).
pub const MAX_QUOTES: usize = 4;

pub fn valid_slot(s: Slot) -> bool {
    s < MAX_SLOTS
}

/// Reason codes new in protocol 3 (refusals, wait reasons, states).
pub mod codes {
    /// `StateUpdate` reason while a job waits in the node queue.
    pub const NODE_QUEUED: &str = "node_queued";
    pub const FAIR_SHARE_WAIT: &str = "fair_share_wait";
    pub const TENANT_QUEUE_FULL: &str = "tenant_queue_full";
    pub const SHARE_PAUSED: &str = "share_paused";
    pub const SHARE_EXPIRED: &str = "share_expired";
    pub const SHARE_REVOKED: &str = "share_revoked";
    /// `cancelled/withdrawn`: the server took a queued job back (V46).
    pub const WITHDRAWN: &str = "withdrawn";
    /// `cancelled/superseded`: a queued row the server's `Welcome.resume`
    /// no longer lists (V47).
    pub const SUPERSEDED: &str = "superseded";
    pub const EXCEEDS_SHARE_CAPS: &str = "exceeds_share_caps";
    pub const GATE_NOT_ALLOWED: &str = "gate_not_allowed";
    pub const EXCLUSIVE_NOT_ALLOWED: &str = "exclusive_not_allowed";
    pub const TENANT_DISK_FULL: &str = "tenant_disk_full";
    pub const INSTALLING_TOOLS: &str = "installing_tools";
    pub const NOT_NODE_OWNER: &str = "not_node_owner";
    pub const SERVER_NOT_APPROVED: &str = "server_not_approved";
    pub const SERVER_TOO_OLD: &str = "server_too_old";
    pub const SERVER_ADD_PENDING: &str = "server_add_pending";
    pub const TENANT_NEEDS_JOB_WALL: &str = "tenant_needs_job_wall";
    pub const TEAM_NEEDS_TENANT_WALL: &str = "team_needs_tenant_wall";
    pub const TENANT_WALL_MISSING: &str = "tenant_wall_missing";
    /// `interrupted/node_update`: infra, without a node-health strike (V55).
    pub const NODE_UPDATE: &str = "node_update";
    /// `servers.add` to a node older than protocol 3.
    pub const NODE_TOO_OLD: &str = "node_too_old";
    /// A no-force cancel / withdraw found the job already started (V57).
    pub const JOB_STARTED: &str = "job_started";

    /// Every code above, for docs and tests.
    pub const ALL: &[&str] = &[
        NODE_QUEUED,
        FAIR_SHARE_WAIT,
        TENANT_QUEUE_FULL,
        SHARE_PAUSED,
        SHARE_EXPIRED,
        SHARE_REVOKED,
        WITHDRAWN,
        SUPERSEDED,
        EXCEEDS_SHARE_CAPS,
        GATE_NOT_ALLOWED,
        EXCLUSIVE_NOT_ALLOWED,
        TENANT_DISK_FULL,
        INSTALLING_TOOLS,
        NOT_NODE_OWNER,
        SERVER_NOT_APPROVED,
        SERVER_TOO_OLD,
        SERVER_ADD_PENDING,
        TENANT_NEEDS_JOB_WALL,
        TEAM_NEEDS_TENANT_WALL,
        TENANT_WALL_MISSING,
        NODE_UPDATE,
        NODE_TOO_OLD,
        JOB_STARTED,
    ];
}

/// `blocked_by` values in quotes, `QueuePos` and queue entries (§7). A
/// node-wide refusal code (`foreign_lock`, `disk_low`, `local_paused`…)
/// may also appear as is.
pub mod blocked {
    pub const FOREIGN_LOCK: &str = crate::refusal::FOREIGN_LOCK;
    pub const PAUSED: &str = "paused";
    pub const SHARE_PAUSED: &str = super::codes::SHARE_PAUSED;
    pub const DISK_LOW: &str = crate::refusal::DISK_LOW;
    pub const SERVER_OFFLINE: &str = "server_offline";
    pub const OVER_QUOTA: &str = "over_quota";
    pub const INSTALLING_TOOLS: &str = super::codes::INSTALLING_TOOLS;
    /// Bigger than the node's caps even when idle (the server's code).
    pub const EXCEEDS_NODE_CAPS: &str = "exceeds_node_caps";
}

// ── small enums ──────────────────────────────────────────────────────

/// Job class. Ordered by rank: `Batch < Interactive < Gate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobClass {
    Batch,
    #[default]
    Interactive,
    Gate,
}

impl JobClass {
    pub fn as_str(self) -> &'static str {
        match self {
            JobClass::Batch => "batch",
            JobClass::Interactive => "interactive",
            JobClass::Gate => "gate",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "batch" => Some(JobClass::Batch),
            "interactive" => Some(JobClass::Interactive),
            "gate" => Some(JobClass::Gate),
            _ => None,
        }
    }
    /// gate 2, interactive 1, batch 0 (the hub's `class_rank`).
    pub fn rank(self) -> u8 {
        self as u8
    }
}

/// What this session's server is on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Tenant,
}

impl Role {
    pub fn of(slot: Slot) -> Self {
        if slot == OWNER_SLOT {
            Role::Owner
        } else {
            Role::Tenant
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Tenant => "tenant",
        }
    }
}

/// Your own servers, or other people's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    Own,
    Team,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Own => "own",
            Trust::Team => "team",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "own" => Some(Trust::Own),
            "team" => Some(Trust::Team),
            _ => None,
        }
    }
}

/// A tenant's network. Ordered narrowest first: `None < NoLan < On`, so
/// the effective network is `min(policy, share)` (V65).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Network {
    None,
    NoLan,
    On,
}

impl Network {
    pub fn as_str(self) -> &'static str {
        match self {
            Network::None => "none",
            Network::NoLan => "no-lan",
            Network::On => "on",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Network::None),
            "no-lan" => Some(Network::NoLan),
            "on" => Some(Network::On),
            _ => None,
        }
    }
    /// The node policy's `network` string (`on` / `none`; anything else
    /// is treated as `on`, as today).
    pub fn of_policy(s: &str) -> Self {
        Network::parse(s).unwrap_or(Network::On)
    }
}

/// The node owner's decision about one server (`servers.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareState {
    Pending,
    Active,
    Paused,
    Revoked,
}

impl ShareState {
    pub fn as_str(self) -> &'static str {
        match self {
            ShareState::Pending => "pending",
            ShareState::Active => "active",
            ShareState::Paused => "paused",
            ShareState::Revoked => "revoked",
        }
    }
}

/// A server as listed (`servers.list`, `ShareView.state`, the server's
/// `compute_node_servers.state`): the decision plus what time and the
/// attach say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    Pending,
    Active,
    Paused,
    Expired,
    Revoked,
    /// Approved but its attach is refused (`server_too_old`).
    Refused,
}

impl ServerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ServerStatus::Pending => "pending",
            ServerStatus::Active => "active",
            ServerStatus::Paused => "paused",
            ServerStatus::Expired => "expired",
            ServerStatus::Revoked => "revoked",
            ServerStatus::Refused => "refused",
        }
    }
}

// ── shares ───────────────────────────────────────────────────────────

/// One tenant's settings on one node (§2 "Share", §7 defaults).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareLimits {
    /// Fair-share weight (1–100). Every server starts at 1, the owner too
    /// (Rosson 2026-10-21); the node owner changes it per server
    /// (`servers.limits`, a raise is a widening). `gate` jobs jump the line
    /// whatever the weights.
    pub weight: u32,
    /// Highest class this tenant may use (`gate` also needs [`Self::gate`]).
    pub top_class: JobClass,
    /// May send `gate` jobs (default: the owner server only).
    pub gate: bool,
    /// May send exclusive jobs.
    pub exclusive: bool,
    pub max_running: u32,
    pub max_queued: u32,
    /// Soft: over it, jobs sort after every tenant within quota (never fail).
    pub cpu_hours_day: u32,
    pub max_job_secs: u64,
    pub max_cpus: u32,
    pub max_mem_gb: u32,
    pub disk_gb: u32,
    pub network: Network,
}

impl ShareLimits {
    /// A new server's share (§7 "Defaults", Q5, Q7, Q13). `macos` picks
    /// `team`'s network (`no-lan` on Linux, `none` on macOS: Seatbelt can't
    /// filter by subnet) and the disk budget (20 GB on a Mac, 40 GB else).
    pub fn new_server(trust: Trust, macos: bool) -> Self {
        Self {
            weight: 1,
            top_class: JobClass::Interactive,
            gate: false,
            exclusive: false,
            max_running: 1,
            max_queued: 10,
            cpu_hours_day: 8,
            max_job_secs: 7200,
            max_cpus: 4,
            max_mem_gb: 8,
            disk_gb: if macos { MAC_DISK_GB } else { DEFAULT_DISK_GB },
            network: match (trust, macos) {
                (Trust::Own, _) => Network::On,
                (Trust::Team, false) => Network::NoLan,
                (Trust::Team, true) => Network::None,
            },
        }
    }

    /// The owner entry the first `servers.add` writes (§11.1): `gate`,
    /// weight 1 like every server (Rosson 2026-10-21). Its per-job and running limits come from the node's
    /// `policy.toml` (the caller passes them); the share adds no cap the
    /// owner didn't have before.
    pub fn owner(max_running: u32, max_cpus: u32, max_mem_gb: u32, disk_gb: u32, network: Network) -> Self {
        Self {
            weight: 1,
            top_class: JobClass::Gate,
            gate: true,
            exclusive: true,
            max_running,
            max_queued: 1000,
            cpu_hours_day: 24 * 1024,
            max_job_secs: 7 * 24 * 3600,
            max_cpus,
            max_mem_gb,
            disk_gb,
            network,
        }
    }

    /// May this tenant send a job of `class`?
    pub fn allows_class(&self, class: JobClass) -> bool {
        match class {
            JobClass::Gate => self.gate && self.top_class == JobClass::Gate,
            c => c <= self.top_class,
        }
    }

    /// Plain range checks (whole numbers only on the wire).
    pub fn validate(&self) -> Result<(), String> {
        let range = |name: &str, v: u64, lo: u64, hi: u64| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} is {lo}–{hi}, got {v}"))
            }
        };
        range("weight", self.weight.into(), 1, 100)?;
        range("max_running", self.max_running.into(), 1, 64)?;
        range("max_queued", self.max_queued.into(), 1, 1000)?;
        range("cpu_hours_day", self.cpu_hours_day.into(), 1, 24 * 1024)?;
        range("max_job_secs", self.max_job_secs, 60, 7 * 24 * 3600)?;
        range("max_cpus", self.max_cpus.into(), 1, 1024)?;
        range("max_mem_gb", self.max_mem_gb.into(), 1, 4096)?;
        range("disk_gb", self.disk_gb.into(), 1, 65536)?;
        if self.gate && self.top_class != JobClass::Gate {
            return Err("gate = true needs top_class = gate".into());
        }
        Ok(())
    }

    /// The fields where `self` gives more than `from` (empty = a pure
    /// narrowing). Narrowing applies at once; any widening needs the node
    /// owner's approval (§4.3, §9 `limits`). Both the executor and the
    /// server's request classifier call this.
    pub fn widens(&self, from: &ShareLimits) -> Vec<&'static str> {
        let mut w = Vec::new();
        let mut more = |name: &'static str, now: u64, before: u64| {
            if now > before {
                w.push(name);
            }
        };
        more("weight", self.weight.into(), from.weight.into());
        more("top_class", self.top_class.rank().into(), from.top_class.rank().into());
        more("gate", self.gate.into(), from.gate.into());
        more("exclusive", self.exclusive.into(), from.exclusive.into());
        more("max_running", self.max_running.into(), from.max_running.into());
        more("max_queued", self.max_queued.into(), from.max_queued.into());
        more("cpu_hours_day", self.cpu_hours_day.into(), from.cpu_hours_day.into());
        more("max_job_secs", self.max_job_secs, from.max_job_secs);
        more("max_cpus", self.max_cpus.into(), from.max_cpus.into());
        more("max_mem_gb", self.max_mem_gb.into(), from.max_mem_gb.into());
        more("disk_gb", self.disk_gb.into(), from.disk_gb.into());
        more("network", self.network as u64, from.network as u64);
        w
    }
}

/// A partial share (`servers.add` overrides, `servers.limits` changes).
/// Absent = keep.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_class: Option<JobClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_running: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_queued: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_hours_day: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_job_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cpus: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mem_gb: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
}

impl ShareOverrides {
    pub fn is_empty(&self) -> bool {
        *self == ShareOverrides::default()
    }

    /// Each given field within its range (the same ranges as
    /// [`ShareLimits::validate`]); the cross-field rules need the whole
    /// share, so the executor checks those after applying.
    pub fn check_ranges(&self) -> Result<(), String> {
        let range = |name: &str, v: Option<u64>, lo: u64, hi: u64| match v {
            Some(v) if !(lo..=hi).contains(&v) => Err(format!("{name} is {lo}–{hi}, got {v}")),
            _ => Ok(()),
        };
        range("weight", self.weight.map(u64::from), 1, 100)?;
        range("max_running", self.max_running.map(u64::from), 1, 64)?;
        range("max_queued", self.max_queued.map(u64::from), 1, 1000)?;
        range("cpu_hours_day", self.cpu_hours_day.map(u64::from), 1, 24 * 1024)?;
        range("max_job_secs", self.max_job_secs, 60, 7 * 24 * 3600)?;
        range("max_cpus", self.max_cpus.map(u64::from), 1, 1024)?;
        range("max_mem_gb", self.max_mem_gb.map(u64::from), 1, 4096)?;
        range("disk_gb", self.disk_gb.map(u64::from), 1, 65536)
    }

    /// `base` with these changes applied.
    pub fn apply(&self, base: &ShareLimits) -> ShareLimits {
        let mut s = base.clone();
        if let Some(v) = self.weight {
            s.weight = v;
        }
        if let Some(v) = self.top_class {
            s.top_class = v;
        }
        if let Some(v) = self.gate {
            s.gate = v;
        }
        if let Some(v) = self.exclusive {
            s.exclusive = v;
        }
        if let Some(v) = self.max_running {
            s.max_running = v;
        }
        if let Some(v) = self.max_queued {
            s.max_queued = v;
        }
        if let Some(v) = self.cpu_hours_day {
            s.cpu_hours_day = v;
        }
        if let Some(v) = self.max_job_secs {
            s.max_job_secs = v;
        }
        if let Some(v) = self.max_cpus {
            s.max_cpus = v;
        }
        if let Some(v) = self.max_mem_gb {
            s.max_mem_gb = v;
        }
        if let Some(v) = self.disk_gb {
            s.disk_gb = v;
        }
        if let Some(v) = self.network {
            s.network = v;
        }
        s
    }
}

// ── servers.toml (the node owner's decisions; V45) ───────────────────

pub const SERVERS_FILE: &str = "servers.toml";
pub const SERVERS_FILE_VERSION: u32 = 1;

/// `/etc/k2-node/servers.toml` (root:`k2nodectl`, 0664). Written by the
/// executor (approved `servers.*` verbs) or the machine owner
/// (`k2-node servers …`), **never by the node**. No file = solo mode with
/// the owner implied by `controller.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServersFile {
    pub version: u32,
    /// The owner server's full fingerprint; must equal `admin.toml`'s
    /// pinned controller, else a health error and no admin.
    pub owner: String,
    #[serde(default = "default_max_servers")]
    pub max_servers: u8,
    #[serde(default, rename = "server", skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerEntry>,
}

fn default_max_servers() -> u8 {
    DEFAULT_MAX_SERVERS
}

/// One `[[server]]` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEntry {
    /// The first 16 hex of the server's key fingerprint (from the code);
    /// the full fingerprint lands in the pin at enrol. The owner's line
    /// carries the full fingerprint.
    pub fp_prefix: String,
    pub label: String,
    pub state: ShareState,
    pub trust: Trust,
    pub slot: Slot,
    /// Linux tenant user's uid; a reused slot always gets a new one (V64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub weight: u32,
    pub top_class: JobClass,
    pub gate: bool,
    pub exclusive: bool,
    pub max_running: u32,
    pub max_queued: u32,
    pub cpu_hours_day: u32,
    pub max_job_secs: u64,
    pub max_cpus: u32,
    pub max_mem_gb: u32,
    pub disk_gb: u32,
    pub network: Network,
    /// Node clock; absent = no end date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// For `pending`: `added_at + PENDING_SECS`; ignored by time after that.
    #[serde(default)]
    pub pending_until: i64,
    pub added_at: i64,
    /// `owner-approval:<request id>` or `local:<user>`.
    pub added_by: String,
}

impl ServerEntry {
    pub fn limits(&self) -> ShareLimits {
        ShareLimits {
            weight: self.weight,
            top_class: self.top_class,
            gate: self.gate,
            exclusive: self.exclusive,
            max_running: self.max_running,
            max_queued: self.max_queued,
            cpu_hours_day: self.cpu_hours_day,
            max_job_secs: self.max_job_secs,
            max_cpus: self.max_cpus,
            max_mem_gb: self.max_mem_gb,
            disk_gb: self.disk_gb,
            network: self.network,
        }
    }

    pub fn set_limits(&mut self, l: &ShareLimits) {
        self.weight = l.weight;
        self.top_class = l.top_class;
        self.gate = l.gate;
        self.exclusive = l.exclusive;
        self.max_running = l.max_running;
        self.max_queued = l.max_queued;
        self.cpu_hours_day = l.cpu_hours_day;
        self.max_job_secs = l.max_job_secs;
        self.max_cpus = l.max_cpus;
        self.max_mem_gb = l.max_mem_gb;
        self.disk_gb = l.disk_gb;
        self.network = l.network;
    }

    /// The listed status at `now` (expiry and pending timeouts by time,
    /// with no file write, §4.3).
    pub fn status(&self, now: i64) -> ServerStatus {
        match self.state {
            ShareState::Revoked => ServerStatus::Revoked,
            _ if self.expires_at.is_some_and(|e| now >= e) => ServerStatus::Expired,
            ShareState::Pending if now >= self.pending_until => ServerStatus::Expired,
            ShareState::Pending => ServerStatus::Pending,
            ShareState::Paused => ServerStatus::Paused,
            ShareState::Active => ServerStatus::Active,
        }
    }

    /// May this server's jobs run at `now`, as far as the owner's decision
    /// goes? (Its pin must also be confirmed and not revoked.)
    pub fn usable(&self, now: i64) -> bool {
        self.status(now) == ServerStatus::Active
    }

    /// May the node enrol and attach to it (it takes no jobs while pending)?
    pub fn may_attach(&self, now: i64) -> bool {
        matches!(self.status(now), ServerStatus::Active | ServerStatus::Paused | ServerStatus::Pending)
    }

    /// Does a full server fingerprint match this line?
    pub fn matches_fp(&self, fp: &str) -> bool {
        !self.fp_prefix.is_empty() && fp.len() == 64 && fp.starts_with(&self.fp_prefix)
    }
}

/// A server label: 1–64 of `A-Z a-z 0-9 - _ .` (the hub's rule).
pub fn valid_label(label: &str) -> bool {
    (1..=64).contains(&label.len()) && label.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn lower_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A code's fingerprint prefix: exactly 16 lowercase hex.
pub fn valid_fp_prefix(s: &str) -> bool {
    s.len() == 16 && lower_hex(s)
}

impl ServersFile {
    /// The file the first `servers.add` writes: just the owner at slot 0.
    pub fn new(owner_fp: &str, owner_label: &str, owner: ShareLimits, max_servers: u8, now: i64, by: &str) -> Self {
        let mut e = ServerEntry {
            fp_prefix: owner_fp.to_string(),
            label: owner_label.to_string(),
            state: ShareState::Active,
            trust: Trust::Own,
            slot: OWNER_SLOT,
            uid: None,
            weight: 0,
            top_class: JobClass::Gate,
            gate: true,
            exclusive: true,
            max_running: 0,
            max_queued: 0,
            cpu_hours_day: 0,
            max_job_secs: 0,
            max_cpus: 0,
            max_mem_gb: 0,
            disk_gb: 0,
            network: Network::On,
            expires_at: None,
            pending_until: 0,
            added_at: now,
            added_by: by.to_string(),
        };
        e.set_limits(&owner);
        Self { version: SERVERS_FILE_VERSION, owner: owner_fp.to_string(), max_servers, servers: vec![e] }
    }

    pub fn entry(&self, slot: Slot) -> Option<&ServerEntry> {
        self.servers.iter().find(|e| e.slot == slot)
    }

    pub fn by_label(&self, label: &str) -> Option<&ServerEntry> {
        self.servers.iter().find(|e| e.label == label && e.state != ShareState::Revoked)
    }

    /// A `servers.*` verb's `server` argument: a label, or `slot:<n>`.
    pub fn find(&self, server: &str) -> Option<&ServerEntry> {
        match server.strip_prefix("slot:").and_then(|n| n.parse::<Slot>().ok()) {
            Some(n) => self.entry(n),
            None => self.by_label(server),
        }
    }

    /// The pending line still within its 10 minutes, if any (one at a time).
    pub fn live_pending(&self, now: i64) -> Option<&ServerEntry> {
        self.servers.iter().find(|e| e.status(now) == ServerStatus::Pending)
    }

    /// Servers that count toward `max_servers` and shared mode.
    pub fn active_count(&self, now: i64) -> usize {
        self.servers.iter().filter(|e| matches!(e.status(now), ServerStatus::Active | ServerStatus::Paused)).count()
    }

    /// Shared mode: two or more active servers (§2).
    pub fn shared(&self, now: i64) -> bool {
        self.active_count(now) >= 2
    }

    /// A free slot for a new server (never the owner's; a revoked line's
    /// slot is reused only after its wipe removed the line).
    pub fn free_slot(&self) -> Option<Slot> {
        (1..MAX_SLOTS).find(|s| self.entry(*s).is_none())
    }

    /// Structural checks. A broken file is never "fail open": the node
    /// keeps the last good copy and raises a health error (§14).
    pub fn validate(&self) -> Result<(), String> {
        if self.version != SERVERS_FILE_VERSION {
            return Err(format!("servers.toml version {} (this node reads {SERVERS_FILE_VERSION})", self.version));
        }
        if self.owner.len() != 64 || !lower_hex(&self.owner) {
            return Err("servers.toml owner is a full 64-hex fingerprint".into());
        }
        if !(1..=MAX_SLOTS).contains(&self.max_servers) {
            return Err(format!("max_servers is 1–{MAX_SLOTS}"));
        }
        let mut seen = [false; MAX_SLOTS as usize];
        for e in &self.servers {
            if !valid_slot(e.slot) {
                return Err(format!("{}: slot {} is out of range", e.label, e.slot));
            }
            if std::mem::replace(&mut seen[e.slot as usize], true) {
                return Err(format!("slot {} appears twice", e.slot));
            }
            if !valid_label(&e.label) {
                return Err(format!("server label {:?} isn't a plain name", e.label));
            }
            if e.slot == OWNER_SLOT {
                if e.fp_prefix != self.owner || e.trust != Trust::Own {
                    return Err("slot 0 is the owner: its fp_prefix is the owner fingerprint and its trust is own".into());
                }
            } else if !valid_fp_prefix(&e.fp_prefix) {
                return Err(format!("{}: fp_prefix is 16 lowercase hex", e.label));
            }
            e.limits().validate().map_err(|m| format!("{}: {m}", e.label))?;
        }
        if self.entry(OWNER_SLOT).is_none() {
            return Err("servers.toml has no owner line (slot 0)".into());
        }
        let pendings = self.servers.iter().filter(|e| e.state == ShareState::Pending).count();
        if pendings > 1 {
            return Err("more than one pending server".into());
        }
        Ok(())
    }
}

// ── wire views (protocol 3, server ≥ 3 only) ─────────────────────────

/// `Offer.share`: this session's tenant view (V53). Never another
/// tenant's details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareView {
    pub role: Role,
    pub slot: Slot,
    /// Two or more active servers.
    pub shared: bool,
    /// Active servers on this node (counts only).
    pub servers: u32,
    /// The owner server's label (for "shared by …"); absent for the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_label: Option<String>,
    pub state: ServerStatus,
    pub limits: ShareLimits,
    pub running: u32,
    pub queued: u32,
    /// CPU-seconds used today on this node (CPU-h × 3600; no floats).
    pub cpu_secs_today: u64,
    /// Seconds until the end date; absent = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<i64>,
    /// The node's clock minus this server's (`Welcome.server_time`, read at
    /// attach): positive = the node is ahead. Shown when over
    /// [`SKEW_SHOW_SECS`] (§9 `nodes`, §14). Order and ETAs never use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skew_secs: Option<i64>,
    /// Start-now and ETA for this tenant's last [`MAX_QUOTES`] distinct
    /// asks. Not part of the offer's change check: they ride the 30 s
    /// offer, so queue movement doesn't trigger an offer on every server.
    /// Keyed by the plan's own `limits.cpu_millis` / `limits.mem_bytes`,
    /// an absent limit as 0 (the server matches its plan the same way; the
    /// node quotes 0 as "an even share", like an unsized job).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quotes: Vec<Quote>,
}

/// Can a job of this size start now, and if not, when (§10.2)?
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub starts_now: bool,
    /// Rounded with [`round_eta`]; absent when it starts now or is blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_p90_secs: Option<u64>,
    /// A [`blocked`] value or node refusal code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
}

/// Clock skew worth showing (`k2 compute nodes`, owner health).
pub const SKEW_SHOW_SECS: i64 = 120;

/// ETAs other servers see: up to 1 min below 10 min, up to 5 min above.
/// A duration, never a clock time.
pub fn round_eta(secs: u64) -> u64 {
    let step = if secs < 600 { 60 } else { 300 };
    secs.div_ceil(step).max(1) * step
}

/// `StateUpdate.queue` while a job waits in the node queue (state
/// `queued`, reason [`codes::NODE_QUEUED`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePos {
    /// 1-based position in the node's whole queue.
    pub pos: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_p90_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
}

/// `queue_get` (server → node; only to nodes offering [`FEATURE_QUEUE`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueGet {
    /// Echoed in the [`QueueView`].
    pub request_id: String,
}

/// `queue_view` (node → server): the node's queue as this session's
/// tenant may see it (§7 "What each viewer sees").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueView {
    pub request_id: String,
    /// Node clock when taken.
    pub at: i64,
    /// Running jobs first, then the queue in start order.
    pub entries: Vec<QueueEntry>,
    /// Per-tenant totals: the owner server only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub totals: Vec<TenantTotals>,
}

/// One running or queued job. For other tenants' jobs only `mine = false`,
/// class, state, size and ETA are set (and `server_label` for the owner):
/// no job id, workspace, session, command or logs. The asking server's own
/// entries (`mine = true`) always carry `job_id` and `generation` (and
/// `workspace_id`, `session_id` when the plan had them), with exact ETAs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub pos: u32,
    pub mine: bool,
    pub class: JobClass,
    /// `running` or `queued`.
    pub state: crate::frames::JobState,
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    /// Running: time left; queued: time to start. Rounded for others' jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_p90_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
    /// Your own jobs only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Other tenants' label: the owner server only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantTotals {
    pub slot: Slot,
    pub label: String,
    pub running: u32,
    pub queued: u32,
    pub cpu_secs_today: u64,
}

/// The `servers.list` result (§9 `node servers`): every line of
/// `servers.toml` with the node's runtime facts. The owner server gets every
/// server; nobody else may run the verb (owner-only admin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServersList {
    /// `servers.toml` `max_servers` (or the default when there's no file).
    pub max_servers: u8,
    /// No `servers.toml`: solo, the owner implied by `controller.json`.
    pub solo: bool,
    pub servers: Vec<ServerListing>,
}

/// One server on the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerListing {
    pub slot: Slot,
    pub label: String,
    /// The decision and time combined; `refused` = approved but its attach
    /// is refused (see `refusal`).
    pub state: ServerStatus,
    pub trust: Trust,
    /// The full key fingerprint once enrolled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fp: Option<String>,
    /// The code's 16-hex prefix (the owner's: its full fingerprint).
    pub fp_prefix: String,
    pub limits: ShareLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<i64>,
    pub added_at: i64,
    pub added_by: String,
    pub running: u32,
    pub queued: u32,
    pub cpu_secs_today: u64,
    /// The SAS to read to the new server's owner, while `pending`
    /// (6 digits, no space).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sas: Option<String>,
    /// Why its attach is refused (`server_too_old`), when `refused`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// Its session is up now.
    pub online: bool,
}

// ── servers.* admin verbs (§10.2) ────────────────────────────────────

pub const VERB_LIST: &str = "servers.list";
pub const VERB_ADD: &str = "servers.add";
pub const VERB_LIMITS: &str = "servers.limits";
pub const VERB_PAUSE: &str = "servers.pause";
pub const VERB_RESUME: &str = "servers.resume";
pub const VERB_REVOKE: &str = "servers.revoke";
pub const VERBS: &[&str] = &[VERB_LIST, VERB_ADD, VERB_LIMITS, VERB_PAUSE, VERB_RESUME, VERB_REVOKE];

/// `servers.add`: approve a new server. Privileged (owner approval); runs
/// `tenant.prepare`. The enrol code never travels: only its binding
/// (`pairing::code_binding`), which is treated as a credential (redacted,
/// 10 min) (V79).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// The new server's base URL (`https://baden.k2.dev`).
    pub url: String,
    pub code_binding: String,
    /// The code's fingerprint prefix (16 hex).
    pub fp_prefix: String,
    pub trust: Trust,
    /// End date as a duration on the node's clock; absent = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_secs: Option<i64>,
    /// Changes to [`ShareLimits::new_server`].
    #[serde(default, skip_serializing_if = "ShareOverrides::is_empty")]
    pub limits: ShareOverrides,
}

/// `servers.limits`: change a share. Operate when [`ShareLimits::widens`]
/// is empty, Privileged otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsArgs {
    /// A label, or `slot:<n>`.
    pub server: String,
    pub set: ShareOverrides,
    /// A new end date from now; `Some(0)` clears it (a widening).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_secs: Option<i64>,
}

/// `servers.pause` / `servers.resume`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerArgs {
    pub server: String,
}

/// `servers.revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeArgs {
    pub server: String,
    /// Stop running jobs now (else they finish).
    #[serde(default)]
    pub now: bool,
    /// Wipe the slot at once (else after 24 h).
    #[serde(default)]
    pub wipe: bool,
}

/// Parse and check a `servers.*` verb's arguments (both ends; the node
/// checks again). `Err` is a `bad_args` message.
pub fn check_verb_args(verb: &str, args: &Value) -> Result<(), String> {
    fn parse<T: serde::de::DeserializeOwned>(verb: &str, args: &Value) -> Result<T, String> {
        serde_json::from_value(args.clone()).map_err(|e| format!("{verb}: {e}"))
    }
    let server_ok = |s: &str| -> Result<(), String> {
        let slot_ok = s.strip_prefix("slot:").is_some_and(|n| n.parse::<Slot>().is_ok_and(valid_slot));
        if slot_ok || valid_label(s) {
            Ok(())
        } else {
            Err(format!("{verb}: server is a label or slot:<n>"))
        }
    };
    match verb {
        VERB_LIST => {
            if args.as_object().is_some_and(|o| o.is_empty()) {
                Ok(())
            } else {
                Err(format!("{verb} takes no arguments"))
            }
        }
        VERB_ADD => {
            let a: AddArgs = parse(verb, args)?;
            let url_ok = a.url.starts_with("https://") || a.url.starts_with("http://");
            if !url_ok || a.url.len() > 512 || a.url.chars().any(char::is_whitespace) {
                return Err(format!("{verb}: url is an http(s) base URL"));
            }
            if a.code_binding.is_empty() || a.code_binding.len() > 128 {
                return Err(format!("{verb}: code_binding is required"));
            }
            if !valid_fp_prefix(&a.fp_prefix) {
                return Err(format!("{verb}: fp_prefix is the code's 16 hex"));
            }
            if a.for_secs.is_some_and(|s| !(3600..=366 * 24 * 3600).contains(&s)) {
                return Err(format!("{verb}: for is 1 h to 366 days"));
            }
            a.limits.apply(&ShareLimits::new_server(a.trust, false)).validate().map_err(|e| format!("{verb}: {e}"))
        }
        VERB_LIMITS => {
            let a: LimitsArgs = parse(verb, args)?;
            server_ok(&a.server)?;
            if a.set.is_empty() && a.for_secs.is_none() {
                return Err(format!("{verb}: change at least one limit"));
            }
            a.set.check_ranges().map_err(|e| format!("{verb}: {e}"))?;
            if a.for_secs.is_some_and(|s| s != 0 && !(3600..=366 * 24 * 3600).contains(&s)) {
                return Err(format!("{verb}: for is 1 h to 366 days, or 0 for none"));
            }
            Ok(())
        }
        VERB_PAUSE | VERB_RESUME => server_ok(&parse::<ServerArgs>(verb, args)?.server),
        VERB_REVOKE => server_ok(&parse::<RevokeArgs>(verb, args)?.server),
        _ => Err(format!("{verb} is not a servers verb")),
    }
}

// ── the executor's internal tenant ops (node user only; §10.2) ───────

pub const OP_TENANT_PREPARE: &str = "tenant.prepare";
pub const OP_TENANT_WIPE: &str = "tenant.wipe";
pub const OP_SERVERS_PRUNE: &str = "servers.prune";

/// `tenant.prepare`: the tenant's user (Linux, new uid each time, V64),
/// group, node-owned root (V54) and slice drop-in (V75).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantPrepare {
    pub slot: Slot,
    pub trust: Trust,
    pub weight: u32,
    pub network: Network,
}

/// `tenant.wipe`: the slot's root, journals, artifacts, ledger, user and
/// group (and on macOS the uid's `DARWIN_USER_DIR`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantWipe {
    pub slot: Slot,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn file() -> ServersFile {
        let mut f = ServersFile::new(&fp('a'), "rosson.k2.dev", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, 1000, "local:rosson");
        let mut b = f.servers[0].clone();
        b.slot = 1;
        b.label = "baden.k2.dev".into();
        b.fp_prefix = "0123456789abcdef".into();
        b.trust = Trust::Team;
        b.set_limits(&ShareLimits::new_server(Trust::Team, false));
        b.expires_at = Some(2000);
        f.servers.push(b);
        f
    }

    #[test]
    fn classes_rank_and_parse() {
        assert!(JobClass::Gate > JobClass::Interactive && JobClass::Interactive > JobClass::Batch);
        assert_eq!((JobClass::Batch.rank(), JobClass::Interactive.rank(), JobClass::Gate.rank()), (0, 1, 2));
        for c in [JobClass::Batch, JobClass::Interactive, JobClass::Gate] {
            assert_eq!(JobClass::parse(c.as_str()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), c.as_str());
        }
        assert_eq!(JobClass::parse("urgent"), None);
    }

    #[test]
    fn network_min_is_the_narrowest() {
        assert_eq!(Network::On.min(Network::NoLan), Network::NoLan);
        assert_eq!(Network::NoLan.min(Network::None), Network::None);
        assert_eq!(serde_json::to_value(Network::NoLan).unwrap(), "no-lan");
        assert_eq!(Network::of_policy("none"), Network::None);
        assert_eq!(Network::of_policy("anything"), Network::On);
    }

    #[test]
    fn defaults_follow_trust_and_os() {
        let own = ShareLimits::new_server(Trust::Own, true);
        assert_eq!((own.network, own.weight, own.max_running, own.max_queued, own.gate), (Network::On, 1, 1, 10, false));
        assert_eq!(ShareLimits::new_server(Trust::Team, false).network, Network::NoLan);
        assert_eq!(ShareLimits::new_server(Trust::Team, true).network, Network::None);
        let o = ShareLimits::owner(2, 8, 16, 100, Network::On);
        assert!(o.allows_class(JobClass::Gate) && o.weight == 1, "every server starts at weight 1, the owner too");
        assert_eq!((own.disk_gb, ShareLimits::new_server(Trust::Own, false).disk_gb), (MAC_DISK_GB, DEFAULT_DISK_GB));
        assert_eq!(DEFAULT_MAX_SERVERS, 3);
        let f: ServersFile = serde_json::from_value(serde_json::json!({"version": 1, "owner": "a".repeat(64)})).unwrap();
        assert_eq!(f.max_servers, 3, "a file without max_servers allows three servers");
        assert!(!own.allows_class(JobClass::Gate) && own.allows_class(JobClass::Interactive) && own.allows_class(JobClass::Batch));
        own.validate().unwrap();
        o.validate().unwrap();
    }

    #[test]
    fn widening_is_any_field_that_gives_more() {
        let base = ShareLimits::new_server(Trust::Team, false);
        let narrower = ShareOverrides { max_running: Some(1), cpu_hours_day: Some(4), network: Some(Network::None), ..Default::default() }.apply(&base);
        assert!(narrower.widens(&base).is_empty());
        let wider = ShareOverrides { max_running: Some(2), network: Some(Network::On), gate: Some(true), ..Default::default() }.apply(&base);
        assert_eq!(wider.widens(&base), vec!["gate", "max_running", "network"]);
        assert!(ShareOverrides::default().is_empty());
    }

    #[test]
    fn servers_file_round_trips_and_validates() {
        let f = file();
        f.validate().unwrap();
        let back: ServersFile = serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        let mut two_owners = f.clone();
        two_owners.servers[1].slot = 0;
        assert!(two_owners.validate().unwrap_err().contains("twice"));
        let mut bad_prefix = f.clone();
        bad_prefix.servers[1].fp_prefix = "XYZ".into();
        assert!(bad_prefix.validate().is_err());
        let mut no_owner = f.clone();
        no_owner.servers.remove(0);
        assert!(no_owner.validate().unwrap_err().contains("owner"));
        let mut team_owner = f.clone();
        team_owner.servers[0].trust = Trust::Team;
        assert!(team_owner.validate().is_err());
        assert!(serde_json::from_value::<ServersFile>(serde_json::json!({"version": 1, "owner": fp('a'), "surprise": 1})).is_err());
    }

    #[test]
    fn status_is_judged_by_time_without_writes() {
        let f = file();
        let b = f.entry(1).unwrap();
        assert_eq!(b.status(1999), ServerStatus::Active);
        assert!(b.usable(1999));
        assert_eq!(b.status(2000), ServerStatus::Expired);
        assert!(!b.usable(2000) && !b.may_attach(2000));
        let mut p = b.clone();
        p.state = ShareState::Pending;
        p.expires_at = None;
        p.pending_until = 1600;
        assert_eq!(p.status(1599), ServerStatus::Pending);
        assert!(p.may_attach(1599) && !p.usable(1599));
        assert_eq!(p.status(1600), ServerStatus::Expired);
        assert!(f.shared(1999) && !f.shared(2000));
        assert_eq!(f.free_slot(), Some(2));
        assert_eq!(f.find("baden.k2.dev").unwrap().slot, 1);
        assert_eq!(f.find("slot:0").unwrap().label, "rosson.k2.dev");
        assert!(b.matches_fp(&format!("0123456789abcdef{}", "0".repeat(48))));
        assert!(!b.matches_fp(&fp('a')));
    }

    #[test]
    fn eta_rounding_hides_detail() {
        assert_eq!(round_eta(0), 60);
        assert_eq!(round_eta(61), 120);
        assert_eq!(round_eta(599), 600);
        assert_eq!(round_eta(601), 900);
        assert_eq!(round_eta(3600), 3600);
    }

    #[test]
    fn verb_args_are_checked() {
        let add = serde_json::json!({
            "url": "https://baden.k2.dev",
            "code_binding": "b64binding",
            "fp_prefix": "0123456789abcdef",
            "trust": "team",
            "for_secs": 7 * 24 * 3600,
            "limits": {"max_running": 2}
        });
        check_verb_args(VERB_ADD, &add).unwrap();
        let mut bad = add.clone();
        bad["fp_prefix"] = "nothex".into();
        assert!(check_verb_args(VERB_ADD, &bad).is_err());
        let mut extra = add.clone();
        extra["limits"]["surprise"] = 1.into();
        assert!(check_verb_args(VERB_ADD, &extra).is_err());
        check_verb_args(VERB_LIMITS, &serde_json::json!({"server": "baden.k2.dev", "set": {"max_running": 2}})).unwrap();
        // The weight path (`k2 compute node servers limits <node> <server>
        // --weight N`): a raise widens (owner approval), a cut narrows.
        let w2 = serde_json::json!({"server": "slot:0", "set": {"weight": 2}});
        check_verb_args(VERB_LIMITS, &w2).unwrap();
        let a: LimitsArgs = serde_json::from_value(w2).unwrap();
        let base = ShareLimits::new_server(Trust::Own, false);
        assert_eq!(a.set.apply(&base).widens(&base), vec!["weight"]);
        assert!(ShareOverrides { weight: Some(1), ..Default::default() }.apply(&a.set.apply(&base)).widens(&a.set.apply(&base)).is_empty());
        assert!(check_verb_args(VERB_LIMITS, &serde_json::json!({"server": "b", "set": {"weight": 0}})).is_err(), "weight is 1–100");
        assert!(check_verb_args(VERB_LIMITS, &serde_json::json!({"server": "b", "set": {"weight": 101}})).is_err());
        assert!(check_verb_args(VERB_LIMITS, &serde_json::json!({"server": "baden.k2.dev", "set": {}})).is_err());
        check_verb_args(VERB_REVOKE, &serde_json::json!({"server": "slot:3", "now": true})).unwrap();
        assert!(check_verb_args(VERB_REVOKE, &serde_json::json!({"server": "slot:9"})).is_err());
        check_verb_args(VERB_LIST, &serde_json::json!({})).unwrap();
        assert!(check_verb_args("servers.owner", &serde_json::json!({})).is_err());
    }

    /// The pinned `servers.list` shape (L4 parses exactly this).
    #[test]
    fn servers_list_round_trips_with_absent_options_omitted() {
        let entry = ServerListing {
            slot: 1,
            label: "b.k2.dev".into(),
            state: ServerStatus::Pending,
            trust: Trust::Team,
            fp: None,
            fp_prefix: "0123456789abcdef".into(),
            limits: ShareLimits::new_server(Trust::Team, true),
            expires_at: Some(9),
            expires_in_secs: Some(5),
            added_at: 1,
            added_by: "local:rosson".into(),
            running: 0,
            queued: 2,
            cpu_secs_today: 30,
            sas: Some("482913".into()),
            refusal: None,
            online: false,
        };
        let l = ServersList { max_servers: DEFAULT_MAX_SERVERS, solo: false, servers: vec![entry] };
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["servers"][0]["state"], "pending");
        assert_eq!(v["servers"][0]["limits"]["network"], "none");
        assert_eq!(v["servers"][0]["sas"], "482913");
        assert!(v["servers"][0].get("fp").is_none() && v["servers"][0].get("refusal").is_none());
        assert_eq!(serde_json::from_value::<ServersList>(v).unwrap(), l);
    }

    #[test]
    fn every_code_is_unique_and_snake_case() {
        let mut seen = std::collections::BTreeSet::new();
        for c in codes::ALL {
            assert!(seen.insert(*c), "{c} twice");
            assert!(c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{c}");
        }
    }
}
