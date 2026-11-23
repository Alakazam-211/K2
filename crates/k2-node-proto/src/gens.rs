//! Cache generations, setup snapshots, idle cores and job stats: the
//! contracts of prd-compute-blacksmith-class-v1 (slice B0; §4.1, §4.4.4,
//! §4.5.1, §4.7, §6). Formats only: no node or server uses them yet.
//!
//! - **Plan fields need features, not a protocol bump** (vs-live BK-19).
//!   `Plan.cache_commit`, `Plan.cache_ref`, `Plan.setup`,
//!   `Plan.cpus_burst` and the new result fields (`Outputs.stats`,
//!   `Outputs.cache`, `Outputs.setup`) and `HostFacts.cache_fs` are absent
//!   when unset, so every older plan, digest and result is unchanged. A
//!   server writes a plan field only for a node offering its feature
//!   ([`plan_features`] names which); an older node would compute another
//!   digest and refuse the assign.
//! - **Protocol 10** ([`crate::frames::PROTOCOL_GENS`]) is for the new
//!   frames only: the box frames ([`crate::boxes`]) and the node's
//!   per-lineage status ([`LineageStatus`]). A node offers [`FEATURES`]
//!   only to a server that said protocol ≥ 10.
//! - **Receipt v3** (BK-5): v2 plus `kind` and `cache` ([`crate::ci::CacheFacts`]),
//!   `frames::Receipt::check_version`; v1 and v2 stay byte-identical.
//!
//! The words: a **warm set** is what a job reuses (a cargo target dir plus
//! `CARGO_HOME` and the sccache dir, or one `--cache` folder). A
//! **lineage** is a chain of read-only **generations** of one warm set. A
//! job never writes a generation: it gets a **fork** (a private clone of
//! the head), which the node commits as the lineage's next generation or
//! discards when the job ends.

use serde::{Deserialize, Serialize};

// ── features (protocol 10) ───────────────────────────────────────────

/// Offer feature: the node keeps cache generations (`Plan.cache_commit`,
/// `Plan.cache_ref`, `Outputs.cache`, `HostFacts.cache_fs`, the
/// `lineage_status` frame). Offered only when the node owner turned
/// `cache-gens` on and the node has a job wall (§4.1.3, BK-2).
pub const FEATURE_CACHE_GENS: &str = "cache-gens";
/// Offer feature: `Plan.setup` and `Outputs.setup` (§4.7).
pub const FEATURE_SETUP_SNAPSHOTS: &str = "setup-snapshots";
/// Offer feature: `Plan.cpus_burst` (§4.4.4, BK-13).
pub const FEATURE_CPU_BURST: &str = "cpu-burst";
/// Offer feature: the box frames ([`crate::boxes`], §4.6).
pub const FEATURE_BOX: &str = "box";
/// Offer feature: `Outputs.stats` (§4.5.1).
pub const FEATURE_JOB_STATS: &str = "job-stats";
/// Offer feature: the node emits receipt v3 when `Plan.receipt_v = 3`
/// (§4.1.7, BK-5).
pub const FEATURE_RECEIPT_V3: &str = "receipt-v3";

/// Every protocol-10 feature. A node offers one only to a server that said
/// protocol ≥ [`crate::frames::PROTOCOL_GENS`]; a server sends a field or
/// frame only to a node offering its feature.
pub const FEATURES: &[&str] = &[
    FEATURE_CACHE_GENS,
    FEATURE_SETUP_SNAPSHOTS,
    FEATURE_CPU_BURST,
    FEATURE_BOX,
    FEATURE_JOB_STATS,
    FEATURE_RECEIPT_V3,
    crate::provenance::FEATURE_PROVENANCE_V1,
];

/// The features a plan's protocol-10 fields need (sorted, unique). A
/// server checks them against the node's offer before it assigns, the same
/// rule as `Plan.gpu` and `Plan.receipt_v`.
pub fn plan_features(p: &crate::frames::Plan) -> Vec<&'static str> {
    let mut out = Vec::new();
    if p.cache_commit.is_some() || p.cache_ref.is_some() {
        out.push(FEATURE_CACHE_GENS);
    }
    if p.cpus_burst.is_some() {
        out.push(FEATURE_CPU_BURST);
    }
    if p.receipt_v == Some(crate::frames::RECEIPT_V3) {
        out.push(FEATURE_RECEIPT_V3);
    }
    if p.provenance.is_some() {
        out.push(crate::provenance::FEATURE_PROVENANCE_V1);
    }
    if !p.setup.is_empty() {
        out.push(FEATURE_SETUP_SNAPSHOTS);
    }
    out.sort_unstable();
    out
}

/// The first of `plan_features(p)` the node doesn't offer, if any.
pub fn missing_feature(p: &crate::frames::Plan, offered: &[String]) -> Option<&'static str> {
    plan_features(p).into_iter().find(|f| !offered.iter().any(|o| o == f))
}

// ── refusal and reason codes (§7) ────────────────────────────────────

pub mod codes {
    /// An agent asked to widen the trusted branches (`k2 ci cache trust
    /// --add/--set`): a person's step, exit 3 (§4.1.5).
    pub const CACHE_TRUST_NEEDS_PERSON: &str = "cache_trust_needs_person";
    /// `k2-node admin cache-gens on` on a node with no job wall (BK-2).
    pub const CACHE_GENS_NEEDS_WALL: &str = "cache_gens_needs_wall";
    /// The signer can't prove a release build's cache chain (§4.2.3).
    pub const WARM_CACHE_UNPROVEN: &str = "warm_cache_unproven";
    /// `--mem auto` with no history and no fallback, exit 2 (§4.4.3).
    pub const MEM_AUTO_NO_HISTORY: &str = "mem_auto_no_history";
    /// A check names a `[setup.*]` the file doesn't have (§4.7).
    pub const SETUP_NOT_FOUND: &str = "setup_not_found";
    /// A `[[quarantine]]` entry is malformed: a bad `until`, an unknown
    /// check (§4.3.4).
    pub const CI_QUARANTINE_INVALID: &str = "ci_quarantine_invalid";
}

// ── lineages ─────────────────────────────────────────────────────────

/// What the node does with a job's fork when it ends (`Plan.cache_commit`,
/// §4.1.4). Only the daemon's CI engine sets `trusted`; the node still
/// checks every rule it can (end state, no `--dirty`/`--with`/secret,
/// ancestry) and records what it did in the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCommit {
    /// A CI check from a verified trusted branch: commit to `trusted` when
    /// it passed and is not older than the head.
    Trusted,
    /// Any other job: commit to `scratch/<cache_ref>` on any end but cancel.
    Scratch,
    /// A release build: commit to `release/<cache_ref>` only with both
    /// opt-ins (BK-7).
    Release,
    /// Fork if there is something to fork, never commit (a secret job
    /// under `job_uid`, BK-14).
    None,
}

impl CacheCommit {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheCommit::Trusted => "trusted",
            CacheCommit::Scratch => "scratch",
            CacheCommit::Release => "release",
            CacheCommit::None => "none",
        }
    }
}

/// The trusted lineage's name.
pub const LINEAGE_TRUSTED: &str = "trusted";
/// Prefixes of the other lineages: `scratch/<ref>`, `release/<build>`,
/// `setup/<key>`.
pub const LINEAGE_SCRATCH: &str = "scratch/";
pub const LINEAGE_RELEASE: &str = "release/";
pub const LINEAGE_SETUP: &str = "setup/";
/// A `--dirty` job's scratch ref: `wt:<worktree>`.
pub const WORKTREE_REF_PREFIX: &str = "wt:";
/// Longest `cache_ref`.
pub const CACHE_REF_MAX: usize = 200;

/// A lineage's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageKind {
    Trusted,
    Scratch,
    Release,
    Setup,
}

/// A scratch ref: a branch name (`main`, `feat/x`) or `wt:<worktree>`.
/// 1–[`CACHE_REF_MAX`] of `A-Z a-z 0-9 . _ / -` (plus the one `:` of
/// `wt:`), no `..`, no `//`, not starting or ending with `/`, not starting
/// with `-` or `.`. The node turns it into a folder name with
/// [`ref_folder`], never the raw text.
pub fn valid_ref(r: &str) -> bool {
    let body = r.strip_prefix(WORKTREE_REF_PREFIX).unwrap_or(r);
    let b = body.as_bytes();
    !b.is_empty()
        && r.len() <= CACHE_REF_MAX
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-'))
        && !body.contains("..")
        && !body.contains("//")
        && !body.starts_with('/')
        && !body.ends_with('/')
        && !body.starts_with('-')
        && !body.starts_with('.')
        && (!r.starts_with(WORKTREE_REF_PREFIX) || !body.contains('/'))
}

/// A ref's folder name on the node: `r-` + the first 32 hex of its
/// SHA-256 (no name the submitter picks reaches the disk).
pub fn ref_folder(r: &str) -> String {
    format!("r-{}", &crate::crypto::sha256_hex(r.as_bytes())[..32])
}

/// Parse a lineage name: `trusted`, `scratch/<ref>`, `release/<build>`,
/// `setup/<64 hex>`.
pub fn parse_lineage(s: &str) -> Result<(LineageKind, &str), String> {
    if s == LINEAGE_TRUSTED {
        return Ok((LineageKind::Trusted, ""));
    }
    if let Some(r) = s.strip_prefix(LINEAGE_SCRATCH) {
        if valid_ref(r) {
            return Ok((LineageKind::Scratch, r));
        }
    } else if let Some(b) = s.strip_prefix(LINEAGE_RELEASE) {
        if crate::ci::is_name(b) {
            return Ok((LineageKind::Release, b));
        }
    } else if let Some(k) = s.strip_prefix(LINEAGE_SETUP) {
        if crate::ci::is_fp(k) {
            return Ok((LineageKind::Setup, k));
        }
    }
    Err(format!("lineage {s:?}: not trusted, scratch/<ref>, release/<build> or setup/<64 hex>"))
}

/// The plan's cache fields agree: `cache_ref` is a ref for `scratch`, a
/// build name for `release`, the verified branch (a plain branch name, no
/// `wt:`) or nothing for `trusted`, absent for `none`.
pub fn check_plan_cache(commit: Option<CacheCommit>, cache_ref: Option<&str>) -> Result<(), String> {
    match (commit, cache_ref) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err("plan: cache_ref without cache_commit".into()),
        (Some(CacheCommit::Trusted | CacheCommit::None), None) => Ok(()),
        (Some(CacheCommit::Trusted), Some(b)) if valid_ref(b) && !b.starts_with(WORKTREE_REF_PREFIX) => Ok(()),
        (Some(c @ (CacheCommit::Trusted | CacheCommit::None)), Some(_)) => {
            Err(format!("plan: cache_commit {} takes no cache_ref (trusted: only a branch name)", c.as_str()))
        }
        (Some(CacheCommit::Scratch), Some(r)) if valid_ref(r) => Ok(()),
        (Some(CacheCommit::Release), Some(b)) if crate::ci::is_name(b) => Ok(()),
        (Some(c), r) => Err(format!("plan: cache_commit {} needs a valid cache_ref (got {r:?})", c.as_str())),
    }
}

/// Lineage limits (§4.1.3, §4.1.6).
/// Scratch lineages the node reports per project, newest first (BK-17).
pub const MAX_SCRATCH_LINEAGES: usize = 16;
/// A scratch lineage idle this long is evicted.
pub const SCRATCH_IDLE_SECS: i64 = 3 * 86_400;
/// Generations a lineage keeps: the head and the one before it (forks may
/// still be cloning from it).
pub const KEEP_GENERATIONS: usize = 2;
/// Placement: a trusted head within this many commits of the job's commit
/// counts as warm (§4.1.6).
pub const WARM_ANCESTRY_COMMITS: u32 = 50;
/// A node whose clone method is [`CloneMethod::Copy`] caps every lineage
/// at this size (§4.1.3).
pub const COPY_LINEAGE_MAX_BYTES: u64 = 20 << 30;
/// The `warmth` a forked job reports (`SrcRan.warmth`), beside today's
/// `warm`, `cold` and `private`.
pub const WARMTH_GEN: &str = "gen";

// ── the node's filesystem (§4.1.3) ───────────────────────────────────

/// How the node clones a generation into a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneMethod {
    /// btrfs: each generation is a subvolume, a fork is a snapshot (O(1)).
    Subvolume,
    /// XFS with reflink, or btrfs without subvolumes: `FICLONE` per file.
    Reflink,
    /// macOS APFS: `clonefile(2)` on the directory.
    Clonefile,
    /// Anything else: `cp -a` (slow; lineages capped at
    /// [`COPY_LINEAGE_MAX_BYTES`]).
    Copy,
}

/// `HostFacts.cache_fs`: what the node's probe of its cache root found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheFs {
    /// The filesystem's name (`btrfs`, `xfs`, `apfs`, `ext4`).
    pub fs: String,
    pub clone: CloneMethod,
    /// The node root sits on a `k2-node setup --cache-volume` image (BK-4).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub volume: bool,
}

// ── the node's per-lineage status (protocol 10 frame) ────────────────

/// One lineage of one warm set on the node, as the server may see it
/// (its own tenant's only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageInfo {
    /// The warm set's key: the project key (target dir sets) or the
    /// `--cache` workspace key.
    pub key: String,
    /// The warm set's name: `target` for a project's cargo set, else the
    /// `--cache` name.
    pub name: String,
    /// `trusted`, `scratch/<ref>`, `release/<build>` or `setup/<key>`.
    pub lineage: String,
    pub head_seq: u64,
    /// The head generation's commit (40 hex).
    pub head_commit: String,
    pub bytes: u64,
    pub last_used: i64,
    /// Forks running from this lineage now.
    #[serde(default)]
    pub forks: u32,
}

/// `lineage_status` (node → controller, protocol 10, feature
/// [`FEATURE_CACHE_GENS`]): every lineage of this server's tenant, sent
/// when a session starts and whenever a head moves. Scratch lineages are
/// capped at [`MAX_SCRATCH_LINEAGES`] per project, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageStatus {
    pub at: i64,
    pub lineages: Vec<LineageInfo>,
    /// More lineages than the frame carries.
    #[serde(default)]
    pub truncated: bool,
}

/// Most lineages in one [`LineageStatus`].
pub const MAX_STATUS_LINEAGES: usize = 512;

impl LineageStatus {
    pub fn check(&self) -> Result<(), String> {
        if self.lineages.len() > MAX_STATUS_LINEAGES {
            return Err(format!("lineage_status: over {MAX_STATUS_LINEAGES} lineages"));
        }
        for l in &self.lineages {
            if !crate::frames::valid_project_key(&l.key) {
                return Err("lineage_status: a key is not a project key".into());
            }
            if l.name != "target" && !crate::cache::valid_name(&l.name) {
                return Err(format!("lineage_status: {:?} is not a warm set name", l.name));
            }
            parse_lineage(&l.lineage)?;
            crate::ci::check_commit("lineage_status.head_commit", &l.head_commit)?;
            if l.head_seq == 0 {
                return Err("lineage_status: head_seq starts at 1".into());
            }
        }
        Ok(())
    }
}

// ── results ──────────────────────────────────────────────────────────

/// `Outputs.cache`: which generation the job forked and what the node did
/// with the fork (§4.1.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheResult {
    /// The lineage forked from; absent when the job built cold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// The forked generation's commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// How long the fork took.
    pub fork_ms: u64,
    /// The fork became the next generation of `commit_lineage`.
    pub committed: bool,
    /// Where it committed (`trusted`, `scratch/main`), when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_lineage: Option<String>,
    /// Why it did or didn't commit, one short line (`older than the head`,
    /// `exit 101`, `secret job`).
    pub why: String,
}

/// One `[setup.*]` step on the plan (§4.7.2): the node forks the snapshot
/// of `key` into `paths`, or runs `argv` first on a miss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    /// The `[setup.<name>]` name.
    pub name: String,
    /// [`setup_key`], 64 hex.
    pub key: String,
    pub argv: Vec<String>,
    /// Repo-relative paths, or `$CARGO_HOME/…`.
    pub paths: Vec<String>,
}

/// Most setup steps on one plan.
pub const MAX_SETUP_STEPS: usize = 8;
/// A second job with the same missing key waits for the first this long
/// (single flight, §4.7.2).
pub const SETUP_WAIT_SECS: u64 = 600;

impl SetupStep {
    pub fn check(&self) -> Result<(), String> {
        check_setup_name(&self.name)?;
        if !crate::ci::is_fp(&self.key) {
            return Err(format!("setup.{}: key is not 64 hex", self.name));
        }
        if self.argv.is_empty() {
            return Err(format!("setup.{}: argv is empty", self.name));
        }
        if self.paths.is_empty() {
            return Err(format!("setup.{}: no paths to keep", self.name));
        }
        for p in &self.paths {
            check_setup_path(&self.name, p)?;
        }
        Ok(())
    }
}

/// A setup name: 1–63 of `a-z 0-9 . _ -`, starting with a letter or digit.
pub fn check_setup_name(name: &str) -> Result<(), String> {
    crate::ci::check_name("setup", name)
}

/// A setup path: repo-relative (no `..`, not absolute) or under
/// `$CARGO_HOME/`.
pub fn check_setup_path(setup: &str, p: &str) -> Result<(), String> {
    let rel = p.strip_prefix("$CARGO_HOME/").unwrap_or(p);
    let ok = !rel.is_empty()
        && rel.len() <= 256
        && !rel.starts_with('/')
        && !rel.contains('$')
        && !rel.contains('\\')
        && rel.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    if ok {
        Ok(())
    } else {
        Err(format!("setup.{setup}.paths: {p:?} is not repo-relative or $CARGO_HOME/…"))
    }
}

/// The snapshot key (§4.7.2): SHA-256 (64 hex) of the canonical JSON of
/// `{argv, inputs: {path: sha256 of its contents}, os, arch, distro, paths,
/// tools: {tool: resolved version}}`.
pub fn setup_key(
    argv: &[String],
    tools: &std::collections::BTreeMap<String, String>,
    os: &str,
    arch: &str,
    distro: &str,
    inputs: &std::collections::BTreeMap<String, String>,
    paths: &[String],
) -> Result<String, String> {
    let v = serde_json::json!({
        "argv": argv,
        "tools": tools,
        "os": os,
        "arch": arch,
        "distro": distro,
        "inputs": inputs,
        "paths": paths,
    });
    Ok(crate::crypto::sha256_hex(&crate::canonical::to_vec(&v)?))
}

/// `Outputs.setup[]`: one step's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupResult {
    pub name: String,
    pub key: String,
    /// The snapshot was there (no step ran).
    pub hit: bool,
    pub ms: u64,
}

// ── job stats (§4.5.1) ───────────────────────────────────────────────

/// One sample of a job's series. Integers only: CPU in thousandths of a
/// core over the interval, bytes, load ×1000.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatPoint {
    /// Seconds since the job started.
    pub t: u32,
    pub cpu_milli: u32,
    pub mem_bytes: u64,
    /// Cumulative IO bytes (`io.stat`; absent on macOS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_write: Option<u64>,
    pub load1_milli: u32,
}

/// `Outputs.stats`: the job's series, downsampled to at most
/// [`STATS_MAX_POINTS`] points and [`STATS_MAX_BYTES`] of JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStats {
    /// The spacing of `points` after downsampling (5 s or more).
    pub every_secs: u32,
    pub points: Vec<StatPoint>,
    /// `cpu.stat` `nr_throttled` and `throttled_usec` (Linux).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nr_throttled: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throttled_usec: Option<u64>,
    /// `memory.events` `oom_kill` (Linux).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_events: Option<u64>,
}

/// The node samples every second and keeps a point every this many.
pub const STATS_EVERY_SECS: u32 = 5;
pub const STATS_MAX_POINTS: usize = 720;
pub const STATS_MAX_BYTES: usize = 64 * 1024;

impl JobStats {
    /// Halve the series (every other point, the spacing doubles) until it
    /// fits both caps.
    pub fn downsampled(mut self) -> Self {
        let fits = |s: &JobStats| s.points.len() <= STATS_MAX_POINTS && serde_json::to_vec(s).map(|v| v.len()).unwrap_or(usize::MAX) <= STATS_MAX_BYTES;
        while !fits(&self) && !self.points.is_empty() {
            self.points = self.points.iter().step_by(2).cloned().collect();
            self.every_secs = self.every_secs.max(1).saturating_mul(2);
        }
        self
    }
}

// ── idle cores (§4.4.4) ──────────────────────────────────────────────

/// The env var a burst job reads its ceiling from (`K2_JOB_CPUS` stays the
/// guarantee).
pub const ENV_JOB_CPUS_MAX: &str = "K2_JOB_CPUS_MAX";

/// `Plan.cpus_burst` (thousandths of a core, like `JobLimits.cpu_millis`)
/// is above the guarantee.
pub fn check_cpus_burst(cpu_millis: Option<u64>, burst_millis: Option<u64>) -> Result<(), String> {
    match (cpu_millis, burst_millis) {
        (_, None) => Ok(()),
        (None, Some(_)) => Err("plan: cpus_burst without a cpus guarantee".into()),
        (Some(c), Some(b)) if b > c => Ok(()),
        (Some(_), Some(_)) => Err("plan: cpus_burst must be above cpus".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_and_lineages() {
        for ok in ["main", "feat/x-1", "release/0.46", "wt:agent-a80d", "a.b_c"] {
            assert!(valid_ref(ok), "{ok}");
        }
        for bad in ["", "-x", ".x", "a..b", "a//b", "/a", "a/", "wt:a/b", "wt:", "a b", "a:b", &"x".repeat(201)] {
            assert!(!valid_ref(bad), "{bad:?}");
        }
        assert_eq!(parse_lineage("trusted").unwrap().0, LineageKind::Trusted);
        assert_eq!(parse_lineage("scratch/feat/x").unwrap(), (LineageKind::Scratch, "feat/x"));
        assert_eq!(parse_lineage("scratch/wt:a").unwrap(), (LineageKind::Scratch, "wt:a"));
        assert_eq!(parse_lineage("release/linux-x86_64").unwrap(), (LineageKind::Release, "linux-x86_64"));
        assert_eq!(parse_lineage(&format!("setup/{}", "a".repeat(64))).unwrap().0, LineageKind::Setup);
        for bad in ["", "trusted/x", "scratch/", "scratch/..", "release/Linux", "setup/abc", "other"] {
            assert!(parse_lineage(bad).is_err(), "{bad:?}");
        }
        assert_eq!(ref_folder("main").len(), 34);
        assert_ne!(ref_folder("main"), ref_folder("Main"));
    }

    #[test]
    fn plan_cache_fields_agree() {
        check_plan_cache(None, None).unwrap();
        check_plan_cache(Some(CacheCommit::Trusted), None).unwrap();
        check_plan_cache(Some(CacheCommit::None), None).unwrap();
        check_plan_cache(Some(CacheCommit::Scratch), Some("wt:x")).unwrap();
        check_plan_cache(Some(CacheCommit::Release), Some("linux-x86_64")).unwrap();
        assert!(check_plan_cache(None, Some("main")).is_err());
        check_plan_cache(Some(CacheCommit::Trusted), Some("main")).unwrap();
        assert!(check_plan_cache(Some(CacheCommit::Trusted), Some("wt:x")).is_err(), "trusted names a branch, never a worktree");
        assert!(check_plan_cache(Some(CacheCommit::None), Some("main")).is_err());
        assert!(check_plan_cache(Some(CacheCommit::Scratch), None).is_err());
        assert!(check_plan_cache(Some(CacheCommit::Release), Some("Linux")).is_err());
        assert_eq!(serde_json::to_string(&CacheCommit::None).unwrap(), "\"none\"");
    }

    #[test]
    fn setup_steps_and_keys() {
        let s = SetupStep {
            name: "node".into(),
            key: "b".repeat(64),
            argv: vec!["bun".into(), "install".into()],
            paths: vec!["node_modules".into(), "$CARGO_HOME/registry".into()],
        };
        s.check().unwrap();
        for bad in ["../x", "/abs", "a/../b", "$HOME/x", "", "a//b"] {
            assert!(SetupStep { paths: vec![bad.into()], ..s.clone() }.check().is_err(), "{bad:?}");
        }
        assert!(SetupStep { name: "Node".into(), ..s.clone() }.check().is_err());
        let tools = [("bun".to_string(), "1.2.0".to_string())].into_iter().collect();
        let inputs = [("bun.lock".to_string(), "c".repeat(64))].into_iter().collect();
        let k = setup_key(&s.argv, &tools, "linux", "x86_64", "ubuntu", &inputs, &s.paths).unwrap();
        assert_eq!(k.len(), 64);
        let inputs2 = [("bun.lock".to_string(), "d".repeat(64))].into_iter().collect();
        assert_ne!(k, setup_key(&s.argv, &tools, "linux", "x86_64", "ubuntu", &inputs2, &s.paths).unwrap(), "a lockfile change is a new key");
    }

    #[test]
    fn stats_downsample_to_the_caps() {
        let pts: Vec<StatPoint> =
            (0..5000).map(|i| StatPoint { t: i * 5, cpu_milli: 4000, mem_bytes: 1 << 30, io_read: Some(1 << 20), io_write: Some(1 << 20), load1_milli: 3000 }).collect();
        let s = JobStats { every_secs: 5, points: pts, ..Default::default() }.downsampled();
        assert!(s.points.len() <= STATS_MAX_POINTS);
        assert!(serde_json::to_vec(&s).unwrap().len() <= STATS_MAX_BYTES);
        assert!(s.every_secs > 5);
        let small = JobStats { every_secs: 5, points: vec![StatPoint::default(); 3], ..Default::default() };
        assert_eq!(small.clone().downsampled(), small);
    }

    #[test]
    fn burst_is_above_the_guarantee() {
        check_cpus_burst(Some(4000), Some(8000)).unwrap();
        check_cpus_burst(None, None).unwrap();
        assert!(check_cpus_burst(None, Some(8000)).is_err());
        assert!(check_cpus_burst(Some(4000), Some(4000)).is_err());
    }

    #[test]
    fn lineage_status_checks() {
        let l = LineageInfo {
            key: "0123456789abcdef".into(),
            name: "target".into(),
            lineage: "scratch/main".into(),
            head_seq: 3,
            head_commit: "a".repeat(40),
            bytes: 1,
            last_used: 1,
            forks: 0,
        };
        LineageStatus { at: 1, lineages: vec![l.clone()], truncated: false }.check().unwrap();
        assert!(LineageStatus { at: 1, lineages: vec![LineageInfo { head_seq: 0, ..l.clone() }], truncated: false }.check().is_err());
        assert!(LineageStatus { at: 1, lineages: vec![LineageInfo { key: "../x".into(), ..l.clone() }], truncated: false }.check().is_err());
        assert!(LineageStatus { at: 1, lineages: vec![LineageInfo { lineage: "scratch/..".into(), ..l }], truncated: false }.check().is_err());
    }
}
