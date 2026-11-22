//! How long the requesting server keeps what compute jobs leave on its
//! disk (0.46.17): `~/.k2/compute/artifacts` and `~/.k2/compute/blobs`.
//!
//! What lives there:
//! - `artifacts/<job>-g<gen>/<sha256>`: files `k2 compute fetch` (or a
//!   CI step, or a secret job's fetch-then-delete) pulled from a node,
//!   one folder per job attempt, plus `.<sha256>.part` for a pull that
//!   stopped. A copy: the node keeps its own for 7 days (except a secret
//!   job's on a node another server owns, which the node drops once this
//!   server holds it).
//! - `blobs/<job>.dirty.tar`: a `--dirty` job's uncommitted changes, kept
//!   from submit until the node takes them, and again for `k2 compute
//!   retry`. One per job: blobs are not shared between jobs.
//! - `blobs/<job>-g<gen>.bundle`: a git bundle made for one transfer and
//!   deleted right after it (one left behind is a failed transfer's).
//!
//! The rule ([`plan`]): every file belongs to a job (its "unit"). A unit
//! goes when its job ended more than the max age ago ([`Why::Age`]), then
//! the oldest ended units go until the store is under the size cap
//! ([`Why::Cap`]). Never a pinned job's ([`set_pin`]), never a job that
//! hasn't ended (its blob may be sent again), never one the daemon says
//! is in use (a fetch streaming from it). A job whose copy here is the
//! only one (a secret job's files, fetched and then deleted on the node:
//! [`JobFact::only_copy`]) goes by age only, never for the cap
//! ([`Keep::OnlyCopy`]). Blobs go only with their unit,
//! so a blob stays while any kept job refers to it (mark and sweep over
//! the job each file names). A file whose job this server doesn't know
//! goes once it's [`ORPHAN_GRACE_SECS`] old (a submit writes its blob a
//! moment before the job row), as does a left-over bundle.
//!
//! The policy and the pins live in `compute_admin_settings` (keys
//! [`POLICY_KEY`] and `artifact_keep:<job id>`), like the workspace node
//! lists: no migration. Deleting, and knowing what's in use, is the
//! daemon's (`k2-daemon/src/compute_retention.rs`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// The policy's settings key.
pub const POLICY_KEY: &str = "artifact_retention";
/// A pin's settings key prefix (`artifact_keep:<job id>`).
pub const PIN_PREFIX: &str = "artifact_keep:";
/// Default max age: 7 days after the job ended, as long as the node keeps
/// its own copy ([`super::proto::results::KEEP_SECS`]) and an ended job
/// keeps its `--env` values ([`super::store::ENV_KEEP_AFTER_END_SECS`]):
/// past that a retry can't run anyway, and nothing can be fetched again.
pub const DEFAULT_MAX_AGE_SECS: i64 = 7 * 86_400;
/// Default size cap: 20 GB, or [`DEFAULT_CAP_DISK_PERCENT`] of the disk if
/// that is smaller. 20 GB holds about four days at the heaviest use seen
/// (about 5 GB a day); what the cap pushes out early is only this
/// server's copy (the node keeps its own for 7 days, so a fetch pulls it
/// again). The percentage keeps a small disk from giving a fifth of
/// itself to a cache: 12.8 GB on a 128 GB disk.
pub const DEFAULT_CAP_BYTES: u64 = 20 << 30;
pub const DEFAULT_CAP_DISK_PERCENT: u64 = 10;
/// The smallest cap a person may set.
pub const MIN_CAP_BYTES: u64 = 1 << 30;
/// A file whose job isn't known, or a bundle left by a transfer, goes
/// once it is this old.
pub const ORPHAN_GRACE_SECS: i64 = 3600;
/// The folder (under `~/.k2/compute`) a prune moves what it removes into
/// before deleting it, so a removal is one rename.
pub const TRASH_DIR: &str = ".prune-trash";

/// The retention policy: `None` = the default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_bytes: Option<u64>,
}

impl Policy {
    pub fn max_age(&self) -> i64 {
        self.max_age_secs.unwrap_or(DEFAULT_MAX_AGE_SECS)
    }

    /// The cap on a disk of `disk_total` bytes (0 = unknown).
    pub fn cap(&self, disk_total: u64) -> u64 {
        self.cap_bytes.unwrap_or_else(|| default_cap(disk_total))
    }

    /// Where the cap comes from: `set` or `auto`.
    pub fn cap_from(&self) -> &'static str {
        if self.cap_bytes.is_some() {
            "set"
        } else {
            "auto"
        }
    }

    /// One line for people: `keep 7 d after a job ends · cap 20.0 GB (auto)`.
    pub fn line(&self, disk_total: u64) -> String {
        let cap = match self.cap_bytes {
            Some(c) => format!("cap {}", human_bytes(c)),
            None => format!("cap {} (auto: 20 GB or {DEFAULT_CAP_DISK_PERCENT}% of the disk, the smaller)", human_bytes(self.cap(disk_total))),
        };
        format!("keep {} after a job ends · {cap}", human_secs(self.max_age()))
    }
}

/// 20 GB, or 10% of a known disk if smaller.
pub fn default_cap(disk_total: u64) -> u64 {
    if disk_total == 0 {
        return DEFAULT_CAP_BYTES;
    }
    DEFAULT_CAP_BYTES.min((disk_total / 100 * DEFAULT_CAP_DISK_PERCENT).max(MIN_CAP_BYTES))
}

/// The stored policy (a missing or unreadable row is the default).
pub fn load(conn: &Connection) -> Policy {
    conn.query_row("SELECT value FROM compute_admin_settings WHERE key = ?1", params![POLICY_KEY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// Store `p` (the default: the row goes).
pub fn save(conn: &Connection, p: &Policy, by: &str, at: i64) -> Result<(), String> {
    if let Some(c) = p.cap_bytes {
        if c < MIN_CAP_BYTES {
            return Err(format!("the cap is at least {}", human_bytes(MIN_CAP_BYTES)));
        }
    }
    if p.max_age_secs.is_some_and(|a| a < 3600) {
        return Err(format!(
            "the max age is at least 1h ({cli} compute prune --older-than removes younger files once)",
            cli = crate::brand::CLI_NAME
        ));
    }
    if *p == Policy::default() {
        conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![POLICY_KEY]).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let v = serde_json::to_string(p).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![POLICY_KEY, v, by, at],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

// ── pins ─────────────────────────────────────────────────────────────

/// Who pinned a job, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub job: String,
    pub by: String,
    pub at: i64,
}

fn pin_key(job_id: &str) -> String {
    format!("{PIN_PREFIX}{job_id}")
}

/// Pin (`on`) or unpin a job. Returns whether that changed anything.
pub fn set_pin(conn: &Connection, job_id: &str, on: bool, by: &str, at: i64) -> Result<bool, String> {
    let job_id = job_id.trim();
    if job_id.is_empty() {
        return Err("name a job".into());
    }
    let n = if on {
        conn.execute(
            "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, '1', ?2, ?3) ON CONFLICT(key) DO NOTHING",
            params![pin_key(job_id), by, at],
        )
    } else {
        conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![pin_key(job_id)])
    }
    .map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// Every pin, by job id. `Err` when the table can't be read: then the
/// caller removes nothing (a pin it can't see must still hold).
pub fn pins(conn: &Connection) -> Result<BTreeMap<String, Pin>, String> {
    // `substr`, not LIKE: `_` in the prefix is a LIKE wildcard.
    let mut st = conn
        .prepare("SELECT key, updated_by, updated_at FROM compute_admin_settings WHERE substr(key, 1, ?1) = ?2")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![PIN_PREFIX.len() as i64, PIN_PREFIX], |r| {
            let key: String = r.get(0)?;
            Ok(Pin { job: key[PIN_PREFIX.len()..].to_string(), by: r.get(1)?, at: r.get(2)? })
        })
        .map_err(|e| e.to_string())?;
    rows.map(|p| p.map(|p| (p.job.clone(), p)).map_err(|e| e.to_string())).collect()
}

// ── what's on disk ───────────────────────────────────────────────────

/// What a file or folder in the store is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// `artifacts/<job>-g<gen>/`.
    Artifacts,
    /// `blobs/<job>.dirty.tar`.
    Dirty,
    /// `blobs/<job>-g<gen>.bundle`.
    Bundle,
    /// Anything else in either folder.
    Other,
}

/// One top-level entry of `artifacts/` or `blobs/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
    /// The job it belongs to (`None` for [`Kind::Other`]).
    pub job: Option<String>,
    pub generation: Option<i64>,
    /// Bytes of every file in it (not following links).
    pub bytes: u64,
    /// Files in it.
    pub files: u64,
    /// Newest change in it (unix seconds).
    pub mtime: i64,
}

impl Entry {
    pub fn is_blob(&self) -> bool {
        matches!(self.kind, Kind::Dirty | Kind::Bundle) || (self.kind == Kind::Other && self.path.parent().and_then(Path::file_name).is_some_and(|n| n == "blobs"))
    }
}

fn mtime_of(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Bytes, files and newest mtime under `p` (links counted, never followed).
pub fn measure(p: &Path) -> (u64, u64, i64) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return (0, 0, 0) };
    if !m.is_dir() {
        return (m.len(), 1, mtime_of(&m));
    }
    let (mut bytes, mut files, mut newest) = (0u64, 0u64, mtime_of(&m));
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            newest = newest.max(mtime_of(&m));
            if m.is_dir() {
                stack.push(e.path());
            } else {
                bytes += m.len();
                files += 1;
            }
        }
    }
    (bytes, files, newest)
}

/// `<job>-g<gen>` → (job, gen).
fn job_gen(name: &str) -> Option<(String, i64)> {
    let (job, gen) = name.rsplit_once("-g")?;
    let gen: i64 = gen.parse().ok()?;
    valid_job_id(job).then(|| (job.to_string(), gen))
}

/// Job ids are 32 hex (a `plan-…` name is a `--plan` check's blob).
fn valid_job_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|c| c.is_ascii_alphanumeric())
}

fn classify(path: &Path, in_blobs: bool) -> (Kind, Option<String>, Option<i64>) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !in_blobs {
        return match job_gen(&name).filter(|_| path.is_dir()) {
            Some((j, g)) => (Kind::Artifacts, Some(j), Some(g)),
            None => (Kind::Other, None, None),
        };
    }
    if let Some(job) = name.strip_suffix(".dirty.tar").filter(|j| valid_job_id(j)) {
        return (Kind::Dirty, Some(job.to_string()), None);
    }
    if let Some((j, g)) = name.strip_suffix(".bundle").and_then(job_gen) {
        return (Kind::Bundle, Some(j), Some(g));
    }
    (Kind::Other, None, None)
}

/// Every top-level entry of `<compute_dir>/artifacts` and `/blobs`.
pub fn scan(compute_dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    for (sub, in_blobs) in [("artifacts", false), ("blobs", true)] {
        let Ok(rd) = std::fs::read_dir(compute_dir.join(sub)) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let (kind, job, generation) = classify(&path, in_blobs);
            let (bytes, files, mtime) = measure(&path);
            out.push(Entry { path, kind, job, generation, bytes, files, mtime });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

// ── the jobs they belong to ──────────────────────────────────────────

/// What the store needs to know about a job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobFact {
    /// It ended (`done`, `failed`, …): nothing will send its blob again
    /// unless someone retries it.
    pub ended: bool,
    pub ended_at: Option<i64>,
    /// A shard's parent (a pin on the parent keeps its shards).
    pub parent: Option<String>,
    /// Who it belongs to, for the listing.
    pub workspace_id: String,
    /// This server holds the only copy of its files: the node deleted its
    /// own after this server fetched them (a secret job on a node another
    /// server owns, `secret_artifacts` event with `deleted: true`).
    pub only_copy: bool,
}

/// The `compute_events` kind the secret fetch-then-delete records.
pub const ONLY_COPY_EVENT: &str = "secret_artifacts";

/// Facts for `ids` (a job not in the table is left out).
pub fn job_facts(conn: &Connection, ids: &BTreeSet<String>) -> HashMap<String, JobFact> {
    let mut out = HashMap::new();
    // One pass over a rare event kind (compute_events has no job index).
    let only: BTreeSet<String> = conn
        .prepare("SELECT DISTINCT job_id FROM compute_events WHERE kind = ?1 AND job_id IS NOT NULL AND json_extract(detail_json, '$.deleted') = 1")
        .and_then(|mut st| st.query_map(params![ONLY_COPY_EVENT], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<BTreeSet<_>>>())
        .unwrap_or_default();
    let Ok(mut st) = conn.prepare(
        "SELECT state, ended_at, json_extract(plan_json, '$.k2ctl.shard.parent'), workspace_id FROM compute_jobs WHERE id = ?1",
    ) else {
        return out;
    };
    for id in ids {
        let row = st
            .query_row(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?)))
            .optional()
            .ok()
            .flatten();
        if let Some((state, ended_at, parent, workspace_id)) = row {
            let ended = super::proto::frames::JobState::parse(&state).is_some_and(|s| s.is_terminal());
            out.insert(id.clone(), JobFact { ended, ended_at, parent, workspace_id, only_copy: only.contains(id) });
        }
    }
    out
}

// ── the plan ─────────────────────────────────────────────────────────

/// Why a unit goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Why {
    /// Its job ended more than the max age ago.
    Age,
    /// The store was over the cap and it was among the oldest.
    Cap,
    /// Its job isn't known here (and the file is past the grace).
    Orphan,
    /// A transfer's bundle that was never cleaned up.
    Leftover,
}

impl Why {
    pub fn word(self) -> &'static str {
        match self {
            Why::Age => "age",
            Why::Cap => "cap",
            Why::Orphan => "orphan",
            Why::Leftover => "leftover",
        }
    }
}

/// Why a unit stays although the rule would let it go, or because it
/// must.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Keep {
    Pinned,
    /// Its job hasn't ended.
    Active,
    /// The daemon says a fetch or a job is using it.
    InUse,
    /// Inside the max age and under the cap.
    Young,
    /// Inside the max age, and this server's copy is the only one: the
    /// cap never takes it (it goes by age).
    OnlyCopy,
}

/// All of one job's entries (or one entry no job owns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub job: Option<String>,
    pub entries: Vec<Entry>,
    /// When its job ended (else its newest file's mtime).
    pub at: i64,
}

impl Unit {
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }
    pub fn files(&self) -> u64 {
        self.entries.iter().map(|e| e.files).sum()
    }
}

/// What [`plan`] decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub remove: Vec<(Unit, Why)>,
    pub keep: Vec<(Unit, Keep)>,
    pub bytes_before: u64,
    pub cap: u64,
    pub max_age: i64,
}

impl Plan {
    pub fn bytes_after(&self) -> u64 {
        self.keep.iter().map(|(u, _)| u.bytes()).sum()
    }
    /// Still over the cap after every removal it may make (pinned, running
    /// or in-use files, or only copies, alone fill it).
    pub fn over_cap(&self) -> bool {
        self.bytes_after() > self.cap
    }
    /// Bytes kept because they are the only copy.
    pub fn only_copy_bytes(&self) -> u64 {
        self.keep.iter().filter(|(_, k)| *k == Keep::OnlyCopy).map(|(u, _)| u.bytes()).sum()
    }
}

/// What [`plan`] works from.
pub struct Input<'a> {
    pub entries: Vec<Entry>,
    pub facts: &'a HashMap<String, JobFact>,
    pub pins: &'a BTreeSet<String>,
    /// The daemon: is anything reading or writing this entry now?
    pub in_use: &'a dyn Fn(&Entry) -> bool,
    pub now: i64,
    pub max_age: i64,
    pub cap: u64,
}

/// Decide what goes (see the module docs).
pub fn plan(input: Input<'_>) -> Plan {
    let bytes_before = input.entries.iter().map(|e| e.bytes).sum();
    let mut by_job: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut loose = Vec::new();
    for e in input.entries {
        match e.job.clone() {
            Some(j) => by_job.entry(j).or_default().push(e),
            None => loose.push(e),
        }
    }
    let mut out = Plan { bytes_before, cap: input.cap, max_age: input.max_age, ..Plan::default() };
    let mut young: Vec<Unit> = Vec::new();
    let mut only: Vec<Unit> = Vec::new();
    let grace_passed = |u: &Unit| input.now - u.entries.iter().map(|e| e.mtime).max().unwrap_or(0) > ORPHAN_GRACE_SECS;
    for e in loose {
        let u = Unit { job: None, at: e.mtime, entries: vec![e] };
        if (input.in_use)(&u.entries[0]) {
            out.keep.push((u, Keep::InUse));
        } else if grace_passed(&u) {
            out.remove.push((u, Why::Orphan));
        } else {
            young.push(u);
        }
    }
    for (job, mut entries) in by_job {
        let fact = input.facts.get(&job);
        let newest = entries.iter().map(|e| e.mtime).max().unwrap_or(0);
        let pinned = input.pins.contains(&job) || fact.and_then(|f| f.parent.as_ref()).is_some_and(|p| input.pins.contains(p));
        if pinned {
            out.keep.push((Unit { job: Some(job), entries, at: newest }, Keep::Pinned));
            continue;
        }
        let Some(fact) = fact else {
            let u = Unit { job: Some(job), entries, at: newest };
            if u.entries.iter().any(|e| (input.in_use)(e)) {
                out.keep.push((u, Keep::InUse));
            } else if grace_passed(&u) {
                out.remove.push((u, Why::Orphan));
            } else {
                young.push(u);
            }
            continue;
        };
        if !fact.ended {
            // A bundle is made for one transfer and deleted after it: one
            // past the grace is a failed transfer's, never sent again.
            let (stale, live): (Vec<Entry>, Vec<Entry>) =
                std::mem::take(&mut entries).into_iter().partition(|e| e.kind == Kind::Bundle && input.now - e.mtime > ORPHAN_GRACE_SECS && !(input.in_use)(e));
            if !stale.is_empty() {
                let at = stale.iter().map(|e| e.mtime).max().unwrap_or(0);
                out.remove.push((Unit { job: Some(job.clone()), entries: stale, at }, Why::Leftover));
            }
            if !live.is_empty() {
                out.keep.push((Unit { job: Some(job), entries: live, at: newest }, Keep::Active));
            }
            continue;
        }
        let u = Unit { job: Some(job), at: fact.ended_at.unwrap_or(newest), entries };
        if u.entries.iter().any(|e| (input.in_use)(e)) {
            out.keep.push((u, Keep::InUse));
        } else if input.now - u.at > input.max_age {
            out.remove.push((u, Why::Age));
        } else if fact.only_copy {
            only.push(u);
        } else {
            young.push(u);
        }
    }
    // The cap: oldest first, among what the rule may remove.
    young.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.job.cmp(&b.job)));
    let mut total: u64 = out.keep.iter().map(|(u, _)| u.bytes()).sum::<u64>()
        + young.iter().map(Unit::bytes).sum::<u64>()
        + only.iter().map(Unit::bytes).sum::<u64>();
    out.keep.extend(only.into_iter().map(|u| (u, Keep::OnlyCopy)));
    for u in young {
        if total > input.cap {
            total -= u.bytes();
            out.remove.push((u, Why::Cap));
        } else {
            out.keep.push((u, Keep::Young));
        }
    }
    out
}

// ── words ────────────────────────────────────────────────────────────

/// `3d`, `72h`, `90m`, `1d12h`, `300s`, `300` → seconds.
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let bad = || format!("bad duration '{s}' (use 3d, 72h, 90m or 1d12h)");
    if s.is_empty() {
        return Err(bad());
    }
    if let Ok(n) = s.parse::<i64>() {
        return if n >= 0 { Ok(n) } else { Err(bad()) };
    }
    let (mut total, mut num) = (0i64, String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let unit = match c {
            'd' => 86_400,
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(bad()),
        };
        let n: i64 = num.parse().map_err(|_| bad())?;
        total = total.checked_add(n.checked_mul(unit).ok_or_else(bad)?).ok_or_else(bad)?;
        num.clear();
    }
    if !num.is_empty() {
        return Err(bad());
    }
    Ok(total)
}

/// `20G`, `512M`, `1T`, bytes → bytes.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let bad = || format!("bad size '{s}' (use 20G, 512M or 1T)");
    let (digits, mult) = match s.chars().last() {
        Some('T' | 't') => (&s[..s.len() - 1], 1u64 << 40),
        Some('G' | 'g') => (&s[..s.len() - 1], 1 << 30),
        Some('M' | 'm') => (&s[..s.len() - 1], 1 << 20),
        _ => (s, 1),
    };
    let n: u64 = digits.parse().map_err(|_| bad())?;
    n.checked_mul(mult).ok_or_else(bad)
}

/// `20.0 GB`, `512 MB`, `0 B`.
pub fn human_bytes(b: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / GB)
    } else if b >= 1 << 20 {
        format!("{} MB", b >> 20)
    } else if b >= 1 << 10 {
        format!("{} KB", b >> 10)
    } else {
        format!("{b} B")
    }
}

/// `7 d`, `36 h`, `45 min`.
pub fn human_secs(s: i64) -> String {
    if s >= 86_400 && s % 86_400 == 0 {
        format!("{} d", s / 86_400)
    } else if s >= 3600 {
        format!("{} h", s / 3600)
    } else if s >= 60 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;
