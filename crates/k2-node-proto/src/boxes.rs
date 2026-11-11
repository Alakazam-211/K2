//! Warm boxes for agents (protocol 10, prd-compute-blacksmith-class-v1
//! §4.6; slice B0 contracts, built in X1/X2). Formats only: no node or
//! server sends these frames yet.
//!
//! A **box** is a held job: a node, a job dir with a worktree, a fork of
//! the warm set ([`crate::gens`]) and a memory reservation, kept between
//! commands. It is not a long-running process: each `box run` is a normal
//! job execution in the box's dir, with its own sub-id `<box>-c<n>`
//! ([`cmd_job_id`]; job ids allow only `[A-Za-z0-9_-]`, BK-12), log, exit,
//! resources and receipt v3 (`kind: box`, which the signer refuses).
//!
//! **Version skew.** The server sends a box frame only to a node offering
//! [`crate::gens::FEATURE_BOX`], which a node offers only to a server that
//! said protocol ≥ [`crate::frames::PROTOCOL_GENS`]. On an older node a
//! `box up` is refused with [`codes::BOX_NEEDS_NODE_UPDATE`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::frames::{Assign, Blob};

/// Limits (§4.6.2, Q8).
pub const IDLE_DEFAULT_SECS: u64 = 30 * 60;
pub const MAX_DEFAULT_SECS: u64 = 8 * 3600;
/// Node policy `box_max_hours` default; a box's `--max` is capped by it.
pub const NODE_MAX_HOURS_DEFAULT: u32 = 8;
pub const MAX_PER_SESSION: usize = 2;
pub const MAX_PER_WORKSPACE: usize = 4;
/// `--hold-on-failure` longest hold.
pub const HOLD_MAX_SECS: u64 = 8 * 3600;

pub mod codes {
    /// Too many boxes for this session or workspace (exit 3, lists them).
    pub const BOX_LIMIT: &str = "box_limit";
    /// `box up --secret` (exit 2): a box never carries secrets in v1.
    pub const BOX_NO_SECRETS: &str = "box_no_secrets";
    /// The placed node's k2-node predates boxes (exit 3).
    pub const BOX_NEEDS_NODE_UPDATE: &str = "box_needs_node_update";
    /// `--hold-on-failure` with `--secret` (exit 2).
    pub const HOLD_NEEDS_NO_SECRETS: &str = "hold_needs_no_secrets";
    /// The box ended (idle, max, node lost, `box down`); `box run` says so.
    pub const BOX_ENDED: &str = "box_ended";
    /// The box's node went offline: a box never moves.
    pub const NODE_LOST: &str = "node_lost";
}

/// The sub-id of a box's `n`th command (1-based): `<box>-c<n>`.
pub fn cmd_job_id(box_id: &str, n: u32) -> Result<String, String> {
    let id = format!("{box_id}-c{n}");
    if n == 0 || !crate::frames::valid_job_id(box_id) || !crate::frames::valid_job_id(&id) {
        return Err(format!("box {box_id:?} command {n}: not a valid job id"));
    }
    Ok(id)
}

/// `box_up` (controller → node): hold a job dir for a box. The assign's
/// plan is the box's base (needs, limits, source, `cache_commit: scratch`)
/// with an empty `argv`; `plan.job_id` is the box id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxUp {
    pub assign: Assign,
    /// End after this long with no command.
    pub idle_secs: u64,
    /// End this long after `up`, whatever happens.
    pub max_secs: u64,
    /// The failed job this box holds (`--hold-on-failure`, X2); its dir,
    /// fork and reservation are kept instead of a new checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_job: Option<String>,
}

/// What a `box run` brings: the same payload as `--dirty` (HEAD + `git diff
/// --binary` + untracked, never `.k2/`) and its digest. Same digest as the
/// last run: no blob. HEAD moved: the node fetches it (bundle transfer
/// under the command's sub-id) and resets the worktree before applying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxSync {
    /// Full 40-hex commit (the worktree's HEAD).
    pub commit: String,
    /// `sha256:<hex>` of the dirty payload (empty payload has a digest too).
    pub digest: String,
    /// The payload, when its digest differs from the box's last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<Blob>,
}

/// `box_run` (controller → node): run one command in the box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxRun {
    pub box_id: String,
    pub generation: u32,
    /// 1-based; the command's job id is [`cmd_job_id`].
    pub n: u32,
    pub sync: BoxSync,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub max_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_secs: Option<u64>,
}

/// `box_down` (controller → node): end the box; its fork commits to
/// `scratch/wt:<worktree>` (or is discarded, per the base plan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxDown {
    pub box_id: String,
    pub generation: u32,
    /// `down`, `idle`, `max`, `disk_guard`, `node_lost`, …
    pub reason: String,
}

/// A box's phase on the node (the ledger state `held`, BK-12, is
/// [`BoxPhase::Held`] and [`BoxPhase::Running`]: the strip, the janitor,
/// the disk guard and the tenant-share passes skip both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxPhase {
    /// Checking out and forking.
    Preparing,
    /// Idle, waiting for a command.
    Held,
    /// A command is running.
    Running,
    Ended,
}

/// `box_state` (node → controller): a box's phase changed, or its idle end
/// is near (the idle notice: `reason = "idle_soon"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxState {
    pub box_id: String,
    pub generation: u32,
    pub phase: BoxPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: i64,
    /// When it ends if nothing runs.
    pub idle_until: i64,
    pub max_until: i64,
    /// Commands run so far.
    pub runs: u32,
    /// The last sync digest the box applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_digest: Option<String>,
}

impl BoxUp {
    pub fn check(&self) -> Result<(), String> {
        let p = &self.assign.plan;
        if !crate::frames::valid_job_id(&p.job_id) {
            return Err("box_up: the box id is not a job id".into());
        }
        if !p.argv.is_empty() {
            return Err("box_up: a box's base plan has no argv (each box_run brings one)".into());
        }
        if !p.secret_env.is_empty() {
            return Err(format!("{}: a box never carries secrets", codes::BOX_NO_SECRETS));
        }
        if self.idle_secs == 0 || self.max_secs == 0 || self.idle_secs > self.max_secs {
            return Err("box_up: idle and max are > 0, idle ≤ max".into());
        }
        if self.max_secs > u64::from(NODE_MAX_HOURS_DEFAULT.max(1)) * 3600 * 3 {
            return Err("box_up: max over 24 h".into());
        }
        if let Some(j) = &self.from_job {
            if !crate::frames::valid_job_id(j) {
                return Err("box_up: from_job is not a job id".into());
            }
        }
        Ok(())
    }
}

impl BoxRun {
    pub fn check(&self) -> Result<(), String> {
        cmd_job_id(&self.box_id, self.n)?;
        if self.argv.is_empty() {
            return Err("box_run: argv is empty".into());
        }
        crate::ci::check_commit("box_run.sync.commit", &self.sync.commit)?;
        crate::ci::check_digest("box_run.sync.digest", &self.sync.digest)?;
        crate::env::check_env_pairs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).map_err(|(c, m)| format!("box_run: {c}: {m}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_ids() {
        let b = "0123456789abcdef0123456789abcdef";
        assert_eq!(cmd_job_id(b, 1).unwrap(), format!("{b}-c1"));
        assert!(crate::frames::valid_job_id(&cmd_job_id(b, u32::MAX).unwrap()));
        assert!(cmd_job_id(b, 0).is_err());
        assert!(cmd_job_id("a/b", 1).is_err());
        assert!(cmd_job_id(&"x".repeat(62), 10).is_err(), "over 64 bytes");
    }

    #[test]
    fn frames_round_trip_as_frames() {
        let s = BoxState {
            box_id: "b1".into(),
            generation: 1,
            phase: BoxPhase::Held,
            reason: Some("idle_soon".into()),
            at: 1,
            idle_until: 2,
            max_until: 3,
            runs: 0,
            sync_digest: None,
        };
        let f = crate::frames::Frame::BoxState(s);
        let text = serde_json::to_string(&f).unwrap();
        assert!(text.starts_with(r#"{"t":"box_state""#), "{text}");
        assert_eq!(serde_json::from_str::<crate::frames::Frame>(&text).unwrap(), f);
        assert_eq!(f.direction(), crate::frames::Direction::NodeToController);
        let d = crate::frames::Frame::BoxDown(BoxDown { box_id: "b1".into(), generation: 1, reason: "down".into() });
        assert_eq!(d.direction(), crate::frames::Direction::ControllerToNode);
        assert_eq!(d.kind(), "box_down");
    }
}
