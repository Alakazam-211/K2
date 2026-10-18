//! The one secret-file list (0.46.1, Thread attachments). Before this there
//! were two that disagreed: the clone scrub (`.env*`, `.auth/`, a content
//! scan) and agent retire (`.env*`, ssh keys, `*.pem|*.key|*.p12|*.pfx`).
//! Clone, retire and Thread attachments now all ask here.
//!
//! Two kinds of signal:
//!
//! 1. **Path patterns** ([`is_secret_rel`], no read): matched on the
//!    workspace-relative path, case-insensitive.
//!    - `.env`, `.env.*` (but not `.env.k2`, see below), `.envrc`;
//!    - anything under an `.auth/` or `.ssh/` folder;
//!    - ssh private keys `id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`;
//!    - key stores `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.keystore`, `*.jks`;
//!    - `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`;
//!    - token files: `*.token`, `credentials.json`, `.credentials.json`.
//! 2. **Content-gated paths and the content scan** ([`classify`], reads
//!    the file): `.env.k2` is a secret only when a sensitive key in it
//!    holds a literal value (it normally holds `k2v://` references);
//!    `.git/config` only when a remote URL carries a login
//!    (`https://user:token@…`); and every text file up to
//!    [`MAX_SCAN_BYTES`] goes through [`content_has_credential_str`]
//!    (JWTs, GitHub tokens, private keys, `password=` …).
//!
//! `.k2/` itself is not refused: PRDs, wiki notes and `WAKEUP.md` live
//! there. Its token-bearing files are caught by the patterns above and the
//! content scan.

use std::path::Path;

/// Files larger than this are not content-scanned (binary blobs or bulk).
/// 1 MiB is generous for config, source and env files.
pub const MAX_SCAN_BYTES: u64 = 1024 * 1024;

/// Why a file counts as a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretHit {
    /// Its path matched a secret pattern.
    Path,
    /// Its content tripped the credential scan (or a content-gated path
    /// holds a literal secret).
    Content,
}

impl SecretHit {
    pub fn as_str(self) -> &'static str {
        match self {
            SecretHit::Path => "path",
            SecretHit::Content => "content",
        }
    }
}

const KEY_NAMES: &[&str] = &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"];
const KEY_EXTS: &[&str] = &["pem", "key", "p12", "pfx", "keystore", "jks", "token"];
const LOGIN_FILES: &[&str] = &[
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    "credentials.json",
    ".credentials.json",
];
const SECRET_DIRS: &[&str] = &[".auth", ".ssh"];

/// The path-pattern half: true when `rel` (workspace-relative, `/` or `\`
/// separators) names a secret by its path alone. No disk access.
pub fn is_secret_rel(rel: &str) -> bool {
    let parts: Vec<String> = rel
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .map(|p| p.to_ascii_lowercase())
        .collect();
    let Some((name, dirs)) = parts.split_last() else {
        return false;
    };
    if dirs.iter().any(|d| SECRET_DIRS.contains(&d.as_str())) {
        return true;
    }
    let name = name.as_str();
    if name == ".env" || name == ".envrc" || (name.starts_with(".env.") && name != ".env.k2") {
        return true;
    }
    if KEY_NAMES.contains(&name) || LOGIN_FILES.contains(&name) {
        return true;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => KEY_EXTS.contains(&ext),
        _ => false,
    }
}

/// The whole check for one file: path patterns, the content-gated paths
/// (`.env.k2`, `.git/config`) and the content scan. `rel` is
/// workspace-relative; `abs` is read for the content checks.
pub fn classify(rel: &str, abs: &Path) -> Option<SecretHit> {
    if is_secret_rel(rel) {
        return Some(SecretHit::Path);
    }
    if final_component(rel).eq_ignore_ascii_case(".env.k2") {
        return env_k2_secret(abs).then_some(SecretHit::Content);
    }
    if is_git_config(rel) && git_config_has_login(abs) {
        return Some(SecretHit::Content);
    }
    content_has_credential(abs).then_some(SecretHit::Content)
}

fn final_component(rel: &str) -> &str {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel)
}

/// `.git/config` at any depth (a nested repo's too).
fn is_git_config(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    parts.len() >= 2 && parts[parts.len() - 1] == "config" && parts[parts.len() - 2] == ".git"
}

/// A `url = scheme://user[:pass]@host` line in a git config.
fn git_config_has_login(abs: &Path) -> bool {
    let Some(text) = read_small_text(abs) else { return false };
    text.lines().any(|l| {
        let l = l.trim();
        let Some((k, v)) = l.split_once('=') else { return false };
        if !k.trim().eq_ignore_ascii_case("url") {
            return false;
        }
        let v = v.trim();
        let Some((_, rest)) = v.split_once("://") else { return false };
        let authority = rest.split('/').next().unwrap_or("");
        authority.contains('@')
    })
}

/// Sensitive key names that must hold a `k2v://` reference in `.env.k2`
/// (mirrors the vault's sensitive-name lint).
fn sensitive_key(k: &str) -> bool {
    let k = k.to_ascii_uppercase();
    k.ends_with("_API_KEY")
        || k == "API_KEY"
        || k.ends_with("_TOKEN")
        || k.ends_with("_SECRET")
        || k.contains("PASSWORD")
        || k == "DATABASE_URL"
        || k.ends_with("_DSN")
}

/// `.env.k2` (K2 vault KV30): references (`k2v://…`) are safe; a literal
/// value under a sensitive name, or a credential anywhere among the
/// non-reference lines, makes it a secret. Unreadable = secret.
fn env_k2_secret(abs: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(abs) else { return true };
    let literal_sensitive = text.lines().any(|l| {
        let l = l.trim().trim_start_matches("export ");
        if l.starts_with('#') {
            return false;
        }
        let Some((k, v)) = l.split_once('=') else { return false };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        sensitive_key(k.trim()) && !v.is_empty() && !v.starts_with("k2v://")
    });
    if literal_sensitive {
        return true;
    }
    let literals: String = text
        .lines()
        .filter(|l| {
            let v = l.split_once('=').map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'')).unwrap_or("");
            !v.starts_with("k2v://")
        })
        .collect::<Vec<_>>()
        .join("\n");
    content_has_credential_str(&literals)
}

/// Up to [`MAX_SCAN_BYTES`] of a text file; `None` for a missing, large or
/// binary file.
fn read_small_text(abs: &Path) -> Option<String> {
    let meta = std::fs::metadata(abs).ok()?;
    if !meta.is_file() || meta.len() > MAX_SCAN_BYTES {
        return None;
    }
    let bytes = std::fs::read(abs).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0u8) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The content scan over one file (text, ≤ [`MAX_SCAN_BYTES`]). Binary,
/// oversized and unreadable files answer `false`.
pub fn content_has_credential(abs: &Path) -> bool {
    read_small_text(abs).is_some_and(|t| content_has_credential_str(&t))
}

/// The credential scan over a text body:
/// - `eyJ[A-Za-z0-9_-]{20,}`: JWTs;
/// - `gh[oprsu]_` + ≥20 alphanumerics: GitHub tokens;
/// - `service_role`: the Supabase service-role marker;
/// - `PRIVATE KEY`: PEM private-key blocks;
/// - `password`, `secret`, `api[_-]?key` followed by `:` or `=` and a
///   value.
pub fn content_has_credential_str(text: &str) -> bool {
    jwt_present(text)
        || gh_token_present(text)
        || text.contains("service_role")
        || text.contains("PRIVATE KEY")
        || assignment_present(text, "password")
        || assignment_present(text, "secret")
        || api_key_assignment_present(text)
}

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
        i = start;
        if i >= text.len() {
            break;
        }
    }
    false
}

fn gh_token_present(text: &str) -> bool {
    let bytes = text.as_bytes();
    for pfx in ["gho_", "ghp_", "ghr_", "ghs_", "ghu_"] {
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

fn assignment_present(text: &str, key: &str) -> bool {
    text.lines().any(|line| line_is_assignment(line, key))
}

fn api_key_assignment_present(text: &str) -> bool {
    text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        ["api_key", "api-key", "apikey"].iter().any(|v| line_is_assignment(&lower, v))
    })
}

/// `<key>` (case-insensitive) then optional whitespace, `:` or `=`, and a
/// non-empty value on the line.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_patterns_from_both_old_lists_and_the_new_ones() {
        for rel in [
            ".env",
            "app/.env.local",
            ".env.production.local",
            ".envrc",
            ".auth/session.json",
            "a/.auth/b/c.txt",
            ".ssh/known_hosts",
            "home/.ssh/config",
            "id_rsa",
            "keys/id_ed25519",
            "certs/server.pem",
            "certs/SERVER.KEY",
            "store.p12",
            "store.pfx",
            "android/release.keystore",
            "java/app.jks",
            ".netrc",
            "web/.npmrc",
            ".pypirc",
            ".git-credentials",
            ".k2/heartbeat.token",
            "gcloud/credentials.json",
            ".claude/.credentials.json",
            "WIN\\.env",
        ] {
            assert!(is_secret_rel(rel), "{rel} should be a secret path");
        }
    }

    #[test]
    fn ordinary_paths_and_dot_k2_docs_are_not_secret_paths() {
        for rel in [
            "README.md",
            "src/session.rs",
            ".env.k2",
            ".k2/prds/prd-x.md",
            ".k2/wiki/Home.md",
            ".k2/heartbeats/WAKEUP.md",
            "id_rsa.pub",
            "docs/keys.md",
            "env/notes.txt",
            ".git/config",
            ".pem",
            "",
            "/",
        ] {
            assert!(!is_secret_rel(rel), "{rel} should not be a secret path");
        }
        // `.env.example` is still `.env.*`: the old scrub rule.
        assert!(is_secret_rel(".env.example"));
    }

    fn tmp(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("k2-secret-paths-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("tmp");
        d
    }

    #[test]
    fn classify_runs_the_content_checks() {
        let d = tmp("classify");
        let plain = d.join("notes.md");
        std::fs::write(&plain, "# Notes\nnothing here\n").unwrap();
        assert_eq!(classify("notes.md", &plain), None);
        let leaked = d.join("cfg.yaml");
        std::fs::write(&leaked, "db:\n  password: hunter2hunter2\n").unwrap();
        assert_eq!(classify("cfg.yaml", &leaked), Some(SecretHit::Content));
        let env = d.join(".env");
        std::fs::write(&env, "PORT=1\n").unwrap();
        assert_eq!(classify(".env", &env), Some(SecretHit::Path));
        let envk2 = d.join(".env.k2");
        std::fs::write(&envk2, "STRIPE_KEY=k2v://ws/a/STRIPE_KEY\n").unwrap();
        assert_eq!(classify(".env.k2", &envk2), None);
        std::fs::write(&envk2, "STRIPE_SECRET=sk_live_literal\n").unwrap();
        assert_eq!(classify(".env.k2", &envk2), Some(SecretHit::Content));
        std::fs::create_dir_all(d.join(".git")).unwrap();
        let git = d.join(".git/config");
        std::fs::write(&git, "[remote \"origin\"]\n\turl = https://github.com/a/b.git\n").unwrap();
        assert_eq!(classify(".git/config", &git), None, "a plain remote is fine");
        std::fs::write(&git, "[remote \"origin\"]\n\turl = https://bob:tok3n@github.com/a/b.git\n").unwrap();
        assert_eq!(classify(".git/config", &git), Some(SecretHit::Content), "a remote with a login is not");
        let bin = d.join("blob.bin");
        std::fs::write(&bin, b"\0\0password=x").unwrap();
        assert_eq!(classify("blob.bin", &bin), None, "binary files are not scanned");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn credential_scan_rules() {
        assert!(content_has_credential_str("token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
        assert!(content_has_credential_str("ghp_abcdefghijklmnopqrstuvwxyz0123"));
        assert!(content_has_credential_str("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(content_has_credential_str("API_KEY = abc"));
        assert!(!content_has_credential_str("the password field is required"));
        assert!(!content_has_credential_str("eyJshort"));
    }
}
