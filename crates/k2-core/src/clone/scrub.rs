//! Secret classifier for the Clone-to bundle engine.
//!
//! The list itself is [`crate::secret_paths`] (0.46.1: one list shared by
//! clone, agent retire and Thread attachments). This module keeps the
//! clone's own reason codes on top of it:
//!
//! - a path pattern (`.env*`, `.auth/`, `.ssh/`, key files, login files)
//!   is [`SecretReason::PathPattern`];
//! - a content hit in a `.k2/` (or legacy `.k2so/`) file is
//!   [`SecretReason::K2soContent`] (benign config lives there too, so the
//!   folder is gated on content);
//! - any other content hit (including a `.env.k2` holding a literal
//!   secret, or a `.git/config` remote with a login) is
//!   [`SecretReason::Content`].
//!
//! `~/.claude/.credentials.json` (Claude Code's own user-level auth) is
//! NEVER migrated — but that file is user-level, not workspace state, so
//! it is excluded at the inventory layer (never even enumerated); the
//! classifier here operates only on workspace files.

use std::path::Path;

use crate::secret_paths::{self, SecretHit};

/// Why a file was classified as a secret. Carried for diagnostics /
/// future surfacing; the bundle only needs the boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretReason {
    /// Matched a secret PATH pattern (`.env*`, `.auth/`, key files…).
    PathPattern,
    /// A `.k2/` (or legacy `.k2so/`) file whose CONTENT tripped the
    /// credential scan.
    K2soContent,
    /// CONTENT tripped the credential scan (path not otherwise flagged).
    Content,
}

/// Classify a single file. `rel_path` is the workspace-relative path (so
/// directory tests are anchored to the tree, not to an absolute home
/// path); `abs_path` is read for the content scan.
///
/// Returns `Some(reason)` if the file is a secret, `None` otherwise.
pub fn classify_secret(rel_path: &str, abs_path: &Path) -> Option<SecretReason> {
    match secret_paths::classify(rel_path, abs_path)? {
        SecretHit::Path => Some(SecretReason::PathPattern),
        SecretHit::Content if path_under_agent_dot_dir(rel_path) && final_component(rel_path) != ".env.k2" => {
            Some(SecretReason::K2soContent)
        }
        SecretHit::Content => Some(SecretReason::Content),
    }
}

#[cfg(test)]
mod env_k2_tests {
    use super::*;

    #[test]
    fn env_k2_with_references_is_kept_and_a_literal_secret_is_not() {
        let dir = std::env::temp_dir().join(format!("k2-scrub-envk2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let refs = dir.join(".env.k2");
        std::fs::write(&refs, "STRIPE_KEY=k2v://ws/sales/STRIPE_KEY\nPORT=3000\n").expect("write");
        assert_eq!(classify_secret(".env.k2", &refs), None);
        std::fs::write(&refs, "STRIPE_KEY=k2v://ws/sales/STRIPE_KEY\nDB_PASSWORD=hunter2hunter2\n").expect("write");
        assert!(classify_secret(".env.k2", &refs).is_some());
        // Sensitive-looking names holding references still travel.
        std::fs::write(&refs, "OPENAI_API_KEY=k2v://server/OPENAI_KEY\nSTRIPE_SECRET_KEY=k2v://ws/sales/STRIPE_KEY\nDB_PASSWORD=k2v://ws/sales/DB\n").expect("write");
        assert_eq!(classify_secret(".env.k2", &refs), None);
        std::fs::write(&refs, "OPENAI_API_KEY=sk-live-literal-value-123456\n").expect("write");
        assert!(classify_secret(".env.k2", &refs).is_some(), "a literal key is still scrubbed");
        std::fs::write(&refs, "OPENAI_API_KEY=k2v://server/OPENAI_KEY\nNOTES=ghp_abcdefghijklmnopqrstuvwxyz0123456789\n").expect("write");
        assert!(classify_secret(".env.k2", &refs).is_some(), "a token under an innocent name is still scrubbed");
        let plain = dir.join(".env.local");
        std::fs::write(&plain, "PORT=3000\n").expect("write");
        assert_eq!(classify_secret(".env.local", &plain), Some(SecretReason::PathPattern));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_shared_list_reaches_the_clone() {
        let dir = std::env::temp_dir().join(format!("k2-scrub-shared-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".k2")).expect("dir");
        let pem = dir.join("server.pem");
        std::fs::write(&pem, "cert").expect("write");
        assert_eq!(classify_secret("server.pem", &pem), Some(SecretReason::PathPattern));
        let k2 = dir.join(".k2/config.toml");
        std::fs::write(&k2, "api_key = \"abc\"\n").expect("write");
        assert_eq!(classify_secret(".k2/config.toml", &k2), Some(SecretReason::K2soContent));
        let k2_doc = dir.join(".k2/notes.md");
        std::fs::write(&k2_doc, "# plan\n").expect("write");
        assert_eq!(classify_secret(".k2/notes.md", &k2_doc), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

fn final_component(rel_path: &str) -> &str {
    rel_path.rsplit(['/', '\\']).next().unwrap_or(rel_path)
}

/// Any path component equal to `.k2` (current agent dot-dir, 0.40.4
/// rename) or `.k2so` (legacy) → content-gated secret scope.
fn path_under_agent_dot_dir(rel_path: &str) -> bool {
    has_dir_component(rel_path, ".k2") || has_dir_component(rel_path, ".k2so")
}

/// Does `rel_path` contain `dir` as one of its directory components (i.e.
/// not as the final filename)? Splits on both separators so a Windows
/// dest path doesn't slip through.
fn has_dir_component(rel_path: &str, dir: &str) -> bool {
    let parts: Vec<&str> = rel_path.split(['/', '\\']).collect();
    // exclude the last element (the filename) from the "directory" test.
    if parts.len() < 2 {
        return false;
    }
    parts[..parts.len() - 1].iter().any(|&p| p == dir)
}
