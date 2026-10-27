//! Per-workspace job caches (protocol 6, 0.46.6): `k2 compute run --cache
//! NAME`.
//!
//! A job asks for a named cache; the node mounts one folder for it at
//! [`ENV_JOB_CACHE`] that outlives the job, so fuzz corpora, cargo target
//! dirs and the like survive between runs on the same node.
//!
//! - **Keyed by (server, workspace, NAME).** The node keeps a server's
//!   caches in that server's own root (its slot), under [`ws_key`] of the
//!   server's controller fingerprint and the workspace id, then the name.
//!   No name the submitter picks reaches another workspace's or server's
//!   folder.
//! - **The node owns the folder's life**: it counts against the server's
//!   disk share and the node's disk floor, the node evicts idle caches
//!   (least recently used first) when they outgrow their cap or the disk
//!   runs low, and a cache last written by another job user (its uid
//!   changed: a re-setup, an owner change) starts empty instead of
//!   failing half-way.
//! - **Never with secret env**: a secret job keeps nothing between runs
//!   (secret-env SE: no warm slot, no shared cache). Both ends refuse it.
//! - **Version skew**: the server sends `Plan.cache` and the `cache_get`
//!   frame only to nodes offering [`FEATURE_JOB_CACHE`]; a node offers it
//!   only to servers that said protocol ≥ 6. A job with `--cache` is never
//!   placed on an older node (the server refuses it, naming the update).

use serde::{Deserialize, Serialize};

use crate::crypto;

/// Offer feature (protocol 6): this node takes `Plan.cache` and answers
/// `cache_get`.
pub const FEATURE_JOB_CACHE: &str = "job-cache";

/// The env var a job finds its cache folder in (absent without `--cache`).
/// A `K2_*` name, so no `--env` can set or override it.
pub const ENV_JOB_CACHE: &str = "K2_JOB_CACHE";

/// Longest cache name.
pub const NAME_MAX: usize = 64;

/// Reason codes (server refusals and node failures).
pub mod codes {
    /// The name isn't 1–64 of a-z 0-9 . _ - starting with a letter or digit.
    pub const CACHE_INVALID: &str = "cache_invalid";
    /// `--cache` with `--secret`: a secret job keeps nothing between runs.
    pub const CACHE_WITH_SECRET: &str = "cache_with_secret";
    /// The node's k2-node predates job caches (no `job-cache` feature).
    pub const NODE_TOO_OLD_FOR_CACHE: &str = "node_too_old_for_cache";
    /// The node couldn't make or prepare the cache folder.
    pub const CACHE_FAILED: &str = "cache_failed";
}

/// A cache name: 1–[`NAME_MAX`] of `a-z 0-9 . _ -`, starting with a
/// letter or digit (never `.`/`..`, never a hidden name, no `/`). Lower
/// case only, so two names never meet on a case-insensitive disk (macOS).
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=NAME_MAX).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// The folder one server's workspace keeps its caches in (16 hex): a hash
/// of the server's controller fingerprint and its workspace id, so a
/// workspace id never names a folder by itself and two servers' workspaces
/// never share one. Both ends compute it: the node to place a cache, the
/// server to tell which of its workspaces a listed cache belongs to.
pub fn ws_key(controller_fp: &str, workspace_id: &str) -> String {
    let h = crypto::sha256_hex(&crypto::signed_bytes("k2-compute-job-cache-v1", &[controller_fp.as_bytes(), workspace_id.as_bytes()]));
    h[..16].to_string()
}

/// A [`ws_key`]: exactly 16 lower-case hex digits.
pub fn valid_ws_key(k: &str) -> bool {
    k.len() == 16 && k.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// `cache_get` (server → node; only to nodes offering
/// [`FEATURE_JOB_CACHE`]): list this server's caches on the node, or clear
/// some first. Only ever this server's own (the node answers from the
/// asking session's slot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheGet {
    /// Echoed in the [`CacheView`].
    pub request_id: String,
    /// One workspace's caches only (`None` = every workspace of this
    /// server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Clear before answering: the caches that match (`workspace_id`,
    /// `name`) and that no running job holds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear: bool,
    /// One cache name only (`None` = every name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `cache_view` (node → server): the asking server's caches on the node
/// after the op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheView {
    pub request_id: String,
    /// Node clock when taken.
    pub at: i64,
    /// What's left (a busy one a clear skipped stays, `busy: true`).
    pub caches: Vec<CacheEntry>,
    /// What a clear removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<CacheEntry>,
    /// A refusal code when the ask itself was bad (e.g. [`codes::CACHE_INVALID`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// One cache folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// [`ws_key`] of the workspace (the server maps it back to its own).
    pub ws_key: String,
    pub name: String,
    /// Bytes on disk (allocated blocks, no link followed).
    pub bytes: u64,
    /// Node clock of the last job's end that used it (0 = never ended).
    pub used_at: i64,
    /// A running job holds it now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub busy: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_lower_case_folder_names() {
        for ok in ["fuzz", "fuzz-corpus", "target.v2", "a", "0day", "cargo_target", &"a".repeat(NAME_MAX)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", ".hidden", "-x", "_x", "a/b", "../x", "Fuzz", "a b", "a\0", "ü", &"a".repeat(NAME_MAX + 1)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn ws_keys_are_stable_hex_and_separate_servers_and_workspaces() {
        let k = ws_key("fp-a", "ws-1");
        assert!(valid_ws_key(&k), "{k}");
        assert_eq!(k, ws_key("fp-a", "ws-1"));
        assert_ne!(k, ws_key("fp-b", "ws-1"), "another server's workspace with the same id");
        assert_ne!(k, ws_key("fp-a", "ws-2"));
        // The parts are length-framed: no (fp, ws) pair collides by shifting bytes.
        assert_ne!(ws_key("fp-a", "ws-1"), ws_key("fp-aw", "s-1"));
        assert!(!valid_ws_key("ABCDEF0123456789"));
        assert!(!valid_ws_key("abc"));
    }

    #[test]
    fn a_plain_list_ask_is_just_the_request_id() {
        let g = CacheGet { request_id: "r".into(), workspace_id: None, clear: false, name: None };
        assert_eq!(crate::canonical::to_string(&g).unwrap(), r#"{"request_id":"r"}"#);
    }
}
