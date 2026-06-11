//! Secret classifier for the Clone-to bundle engine.
//!
//! Two independent signals mark a file as a secret:
//!
//! 1. **Path patterns** — `.env`, `.env.local`, `.env.*`, `.env.*.local`
//!    anywhere in the tree; anything under an `.auth/` directory; and
//!    token-bearing files under `.k2so/` (gated on content, since benign
//!    config like a focus-group name lives there too).
//! 2. **Content credential scan** — a small set of regex-free substring +
//!    pattern checks (JWTs, GH tokens, Supabase `service_role`, private
//!    keys, `password:`/`secret:`/`api_key:` assignments). A hit on a
//!    reasonably-sized text file marks it secret regardless of path.
//!
//! `~/.claude/.credentials.json` (Claude Code's own user-level auth) is
//! NEVER migrated — but that file is user-level, not workspace state, so
//! it is excluded at the inventory layer (never even enumerated); the
//! classifier here operates only on workspace files.

use std::path::Path;

/// Files larger than this are not content-scanned (they're either binary
/// blobs or bulk that the exclude list should already have dropped). 1 MiB
/// is generous for config/source/env files while bounding scan cost.
const MAX_SCAN_BYTES: u64 = 1024 * 1024;

/// Why a file was classified as a secret. Carried for diagnostics /
/// future surfacing; the bundle only needs the boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretReason {
    /// Matched a secret PATH pattern (`.env*`, `.auth/`).
    PathPattern,
    /// A `.k2so/` file whose CONTENT tripped the credential scan.
    K2soContent,
    /// CONTENT tripped the credential scan (path not otherwise flagged).
    Content,
}

/// Classify a single file. `rel_path` is the workspace-relative path (so
/// `.auth/` / `.k2so/` directory tests are anchored to the tree, not to
/// an absolute home path); `abs_path` is read for the content scan.
///
/// Returns `Some(reason)` if the file is a secret, `None` otherwise.
pub fn classify_secret(rel_path: &str, abs_path: &Path) -> Option<SecretReason> {
    // ── 1. PATH patterns (no read needed) ────────────────────────────
    if path_is_env_secret(rel_path) {
        return Some(SecretReason::PathPattern);
    }
    if path_under_auth_dir(rel_path) {
        return Some(SecretReason::PathPattern);
    }

    // ── 2. CONTENT scan ──────────────────────────────────────────────
    // `.k2so/` files are gated on content (benign config lives there);
    // every other file is also content-scanned as a credential safety net.
    let under_k2so = path_under_k2so_dir(rel_path);
    if content_has_credential(abs_path) {
        return Some(if under_k2so {
            SecretReason::K2soContent
        } else {
            SecretReason::Content
        });
    }

    None
}

/// `.env`, `.env.local`, `.env.*`, `.env.*.local` — matched on the FINAL
/// path component (the filename), anywhere in the tree.
fn path_is_env_secret(rel_path: &str) -> bool {
    let name = final_component(rel_path);
    // `.env` exactly, or any `.env.<suffix>` (covers `.env.local`,
    // `.env.production`, `.env.production.local`, …).
    name == ".env" || name.starts_with(".env.")
}

/// Any path component equal to `.auth` → the whole subtree is secret
/// (login/session state).
fn path_under_auth_dir(rel_path: &str) -> bool {
    has_dir_component(rel_path, ".auth")
}

/// Any path component equal to `.k2so` → content-gated secret scope.
fn path_under_k2so_dir(rel_path: &str) -> bool {
    has_dir_component(rel_path, ".k2so")
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

fn final_component(rel_path: &str) -> &str {
    rel_path.rsplit(['/', '\\']).next().unwrap_or(rel_path)
}

/// Read up to [`MAX_SCAN_BYTES`] of the file and look for any credential
/// signal. Binary / oversized / unreadable files yield `false` (the
/// exclude list is responsible for keeping bulk out; this scan is a
/// safety net over text).
fn content_has_credential(abs_path: &Path) -> bool {
    let meta = match std::fs::metadata(abs_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.is_file() || meta.len() > MAX_SCAN_BYTES {
        return false;
    }
    let bytes = match std::fs::read(abs_path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    // Skip obvious binaries (a NUL in the first 8 KiB is a strong signal).
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0u8) {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes);
    content_has_credential_str(&text)
}

/// The actual credential scan over a text body. Split out so tests can
/// drive it directly without touching the filesystem.
///
/// Rules (from the PRD / reporter):
/// - `eyJ[A-Za-z0-9_-]{20,}` — JWTs.
/// - `gh[oprstu]_[A-Za-z0-9]{20,}` — GitHub tokens (`gho_`/`ghp_`/`ghr_`/
///   `ghs_`/`ghu_`; the PRD's literal `gh[ports]_` is implemented as this
///   concrete prefix set + a length floor).
/// - `service_role` — Supabase service-role key marker.
/// - `PRIVATE KEY` — PEM private-key blocks.
/// - `password <sep>`, `secret <sep>`, `api[_-]?key <sep>` — assignments,
///   where `<sep>` is `:` or `=` (optional surrounding whitespace).
pub fn content_has_credential_str(text: &str) -> bool {
    jwt_present(text)
        || gh_token_present(text)
        || text.contains("service_role")
        || text.contains("PRIVATE KEY")
        || assignment_present(text, "password")
        || assignment_present(text, "secret")
        || api_key_assignment_present(text)
}

/// `eyJ` followed by ≥20 chars of `[A-Za-z0-9_-]`.
fn jwt_present(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(off) = text[i..].find("eyJ") {
        let start = i + off + 3;
        let mut n = 0;
        let mut j = start;
        while j < bytes.len() && is_b64url(bytes[j]) {
            n += 1;
            j += 1;
        }
        if n >= 20 {
            return true;
        }
        i = start; // advance past this "eyJ" to avoid re-matching it.
        if i >= text.len() {
            break;
        }
    }
    false
}

/// `gh[oprstu]_` followed by ≥20 alphanumerics.
fn gh_token_present(text: &str) -> bool {
    let bytes = text.as_bytes();
    let prefixes: &[&str] = &["gho_", "ghp_", "ghr_", "ghs_", "ghu_"];
    for pfx in prefixes {
        let mut i = 0;
        while let Some(off) = text[i..].find(pfx) {
            let start = i + off + pfx.len();
            let mut n = 0;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                n += 1;
                j += 1;
            }
            if n >= 20 {
                return true;
            }
            i = start;
            if i >= text.len() {
                break;
            }
        }
    }
    false
}

/// `<key>` (case-insensitive) followed by optional whitespace, a `:` or
/// `=`, then a non-empty value on the line.
fn assignment_present(text: &str, key: &str) -> bool {
    for line in text.lines() {
        if line_is_assignment(line, key) {
            return true;
        }
    }
    false
}

/// `api_key` / `api-key` / `apikey` assignment (case-insensitive).
fn api_key_assignment_present(text: &str) -> bool {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        for variant in ["api_key", "api-key", "apikey"] {
            if line_is_assignment(&lower, variant) {
                return true;
            }
        }
    }
    false
}

/// Does the line contain `<key>` (case-insensitive) immediately followed
/// (modulo whitespace) by `:` or `=` and a non-empty value?
fn line_is_assignment(line: &str, key: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(off) = lower[search_from..].find(&key) {
        let after = search_from + off + key.len();
        let rest = lower[after..].trim_start();
        if let Some(stripped) = rest.strip_prefix([':', '=']) {
            if !stripped.trim().is_empty() {
                return true;
            }
        }
        search_from = after;
        if search_from >= lower.len() {
            break;
        }
    }
    false
}

fn is_b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}
