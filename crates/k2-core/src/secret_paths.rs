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
//!    - token files: `*.token`, `credentials.json`, `.credentials.json`;
//!    - other tools' login folders and files: `.aws/`, `.gnupg/`, `.kube/`,
//!      `.docker/config.json`, gcloud's credentials, `.codex/auth.json`,
//!      `.config/gh/hosts.yml`.
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
const SECRET_DIRS: &[&str] = &[".auth", ".ssh", ".aws", ".gnupg", ".kube"];
/// Secret by these last path segments, at any depth.
const SECRET_SUFFIXES: &[&[&str]] = &[
    &[".docker", "config.json"],
    &[".config", "gcloud", "credentials.db"],
    &[".config", "gcloud", "access_tokens.db"],
    &[".config", "gcloud", "application_default_credentials.json"],
    &[".codex", "auth.json"],
    &[".config", "gh", "hosts.yml"],
];

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
    if SECRET_SUFFIXES.iter().any(|tail| {
        parts.len() >= tail.len()
            && parts[parts.len() - tail.len()..].iter().zip(tail.iter()).all(|(p, t)| p == t)
    }) {
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

/// What [`scan_attachment`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentScan {
    pub hit: Option<SecretHit>,
    /// All of the file was read as text. False for a binary file: tokens were
    /// looked for in it, but a zip or a PDF can hold anything. This is a check
    /// for mistakes, not a guarantee, so an approver is told when it read little.
    pub read_as_text: bool,
}

/// [`classify`] for a file leaving in an email, over the bytes the caller will
/// send (a file swapped between a check and a read can't pass). Differences, as
/// a person pays for a false alarm here and a mail can't be recalled:
/// - it reads the whole file, however big, and a NUL doesn't end the search for
///   tokens;
/// - it knows other services' token shapes ([`outbound_token_present`]);
/// - `password:` counts only with something shaped like a value after it, so
///   "The password: field is required." passes.
pub fn scan_attachment(rel: &str, bytes: &[u8]) -> AttachmentScan {
    let read_as_text = !bytes.contains(&0u8);
    let found = |hit| AttachmentScan { hit: Some(hit), read_as_text };
    if is_secret_rel(rel) {
        return found(SecretHit::Path);
    }
    if final_component(rel).eq_ignore_ascii_case(".env.k2") {
        // Not UTF-8 = unreadable = a secret, as in [`env_k2_secret`].
        if std::str::from_utf8(bytes).map_or(true, env_k2_text_secret) {
            return found(SecretHit::Content);
        }
        return AttachmentScan { hit: None, read_as_text };
    }
    let text = String::from_utf8_lossy(bytes);
    if is_git_config(rel) && git_config_text_has_login(&text) {
        return found(SecretHit::Content);
    }
    if outbound_token_present(&text) || (read_as_text && value_assignment_present(&text)) {
        return found(SecretHit::Content);
    }
    AttachmentScan { hit: None, read_as_text }
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
    read_small_text(abs).is_some_and(|t| git_config_text_has_login(&t))
}

fn git_config_text_has_login(text: &str) -> bool {
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
    env_k2_text_secret(&text)
}

fn env_k2_text_secret(text: &str) -> bool {
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
    small_text(&std::fs::read(abs).ok()?)
}

/// `bytes` as text when they are small enough to scan and not binary.
fn small_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() as u64 > MAX_SCAN_BYTES || bytes[..bytes.len().min(8192)].contains(&0u8) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
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
    token_present(text)
        || assignment_present(text, "password")
        || assignment_present(text, "secret")
        || api_key_assignment_present(text)
}

/// A credential by its shape alone, for every user of this list (clone,
/// Thread attachments, downloads): nothing about the line around it matters.
fn token_present(text: &str) -> bool {
    jwt_present(text)
        || gh_token_present(text)
        || text.contains("service_role")
        || text.contains("PRIVATE KEY")
}

/// [`token_present`] plus other services' token shapes, for a file leaving in
/// an email only. They aren't in the shared list: a false hit here is a refusal
/// with a next step, while in a clone it silently drops a source file.
fn outbound_token_present(text: &str) -> bool {
    token_present(text)
        || aws_key_present(text)
        || ["sk-ant-", "sk-proj-", "sk_live_", "rk_live_", "xoxb-", "xoxp-", "xoxa-", "xoxs-"]
            .iter()
            .any(|p| prefixed_token_present(text, p))
        || ["k2skn_", "k2rs_"].iter().any(|p| k2_token_present(text, p))
}

/// `prefix` then at least 16 token characters, a digit among them: placeholder
/// words (`xoxb-your-bot-token-goes-here`) have none.
fn prefixed_token_present(text: &str, prefix: &str) -> bool {
    text.match_indices(prefix).any(|(i, _)| {
        let run = text[i + prefix.len()..].bytes().take_while(|b| is_b64url(*b)).take(64);
        let (mut n, mut digit) = (0, false);
        for b in run {
            n += 1;
            digit |= b.is_ascii_digit();
        }
        n >= 16 && digit
    })
}

/// A K2 token: `prefix` then exactly its 43 base62 characters. An identifier
/// like `k2rs_token_is_refused` has underscores.
fn k2_token_present(text: &str, prefix: &str) -> bool {
    text.match_indices(prefix).any(|(i, _)| {
        let n = text[i + prefix.len()..].bytes().take_while(|b| is_b64url(*b)).take(44);
        n.clone().count() == 43 && n.clone().all(|b| b.is_ascii_alphanumeric())
    })
}

/// `AKIA` or `ASIA` then 16 upper-case letters or digits, unless it's the kind
/// of key documentation prints (`AKIAIOSFODNN7EXAMPLE`).
fn aws_key_present(text: &str) -> bool {
    ["AKIA", "ASIA"].iter().any(|prefix| {
        text.match_indices(prefix).any(|(i, _)| {
            let key: Vec<u8> = text[i + prefix.len()..]
                .bytes()
                .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                .take(16)
                .collect();
            key.len() >= 16 && !key.windows(7).any(|w| w == b"EXAMPLE")
        })
    })
}

/// How far past a key word to look for its value. A file can repeat a key word
/// many times on one long line, and looking at the rest of the line each time
/// would take hours on a 25 MiB file, in the daemon.
const ASSIGNMENT_WINDOW: usize = 4096;

/// The first [`ASSIGNMENT_WINDOW`] bytes of `s`, ending on a character.
fn after_key(s: &str) -> &str {
    if s.len() <= ASSIGNMENT_WINDOW {
        return s;
    }
    let mut end = ASSIGNMENT_WINDOW;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// [`assignment_present`] for the key words, counting a line only when what
/// follows looks like a value: one unbroken token of at least 6 characters. A
/// quote may close the key first, as in JSON.
fn value_assignment_present(text: &str) -> bool {
    text.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        ["password", "secret", "api_key", "api-key", "apikey"].iter().any(|key| {
            lower.match_indices(key).any(|(i, _)| {
                let rest = after_key(&lower[i + key.len()..]).trim_start_matches(['"', '\'']).trim_start();
                let Some(value) = rest.strip_prefix([':', '=']) else { return false };
                let value = value.trim().trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
                value.len() >= 6 && !value.contains(char::is_whitespace)
            })
        })
    })
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
        let rest = after_key(&lower[after..]).trim_start();
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

    /// The check on a file leaving in an email, over the bytes in hand.
    #[test]
    fn scan_attachment_finds_what_a_mail_must_not_carry() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\n";
        let cases: &[(&str, &[u8], Option<SecretHit>)] = &[
            (".env", b"A=1\n", Some(SecretHit::Path)),
            ("sub/.ssh/known", b"x\n", Some(SecretHit::Path)),
            ("server.pem", b"x\n", Some(SecretHit::Path)),
            (".aws/credentials", b"x\n", Some(SecretHit::Path)),
            (".gnupg/secring.gpg", b"x\n", Some(SecretHit::Path)),
            (".kube/config", b"x\n", Some(SecretHit::Path)),
            (".docker/config.json", b"{}\n", Some(SecretHit::Path)),
            ("home/.config/gcloud/application_default_credentials.json", b"{}\n", Some(SecretHit::Path)),
            (".codex/auth.json", b"{}\n", Some(SecretHit::Path)),
            (".config/gh/hosts.yml", b"x\n", Some(SecretHit::Path)),
            ("config.json", b"{}\n", None),
            ("notes.txt", b"nothing to see\n", None),
            ("notes.txt", b"password = hunter2hunter2\n", Some(SecretHit::Content)),
            ("notes.txt", b"api_key: \"abcd1234\"\n", Some(SecretHit::Content)),
            ("notes.txt", pem.as_bytes(), Some(SecretHit::Content)),
            // Words about passwords, not one.
            ("help.txt", b"The password: field is required.\n", None),
            ("help.txt", b"secret: true\n", None),
            ("help.txt", b"Reset your password = see the wiki page.\n", None),
            (".env.k2", b"API_KEY=k2v://vault/x\n", None),
            (".env.k2", b"API_KEY=literal\n", Some(SecretHit::Content)),
            (".env.k2", b"\xff\xfe", Some(SecretHit::Content)),
            ("repo/.git/config", b"[remote]\n url = https://me:tok@host/x\n", Some(SecretHit::Content)),
            ("repo/.git/config", b"[remote]\n url = https://host/x\n", None),
            ("a.txt", b"key AKIAZ2GH7QK4LMNP8RST here\n", Some(SecretHit::Content)),
            ("a.txt", b"docs say AKIAIOSFODNN7EXAMPLE\n", None),
            ("a.txt", b"not AKIAshort\n", None),
            ("a.txt", b"sk-ant-api03-abcdefghijklmnop\n", Some(SecretHit::Content)),
            ("a.txt", b"sk-proj-abcd1234efgh5678\n", Some(SecretHit::Content)),
            ("a.txt", b"sk_live_abcd1234efgh5678\n", Some(SecretHit::Content)),
            ("a.txt", b"sk_live_your_key_goes_here\n", None),
            ("a.txt", b"xoxb-1234567890-abcdefgh\n", Some(SecretHit::Content)),
            ("a.txt", b"xoxb-your-bot-token-goes-here\n", None),
            ("a.txt", b"k2skn_0123456789abcdefghijABCDEFGHIJ0123456789a\n", Some(SecretHit::Content)),
            ("a.txt", b"k2rs_0123456789abcdefghijABCDEFGHIJ0123456789a\n", Some(SecretHit::Content)),
            ("a.txt", b"fn k2rs_token_is_refused_when_it_has_expired_long_ago_ok() {}\n", None),
            ("a.txt", b"k2rs_abcdefghijklmnop\n", None),
            ("a.txt", b"k2rs_0123456789abcdefghijABCDEFGHIJ0123456789abc\n", None),
            ("a.txt", b"sk-ant- is a prefix\n", None),
            // JSON, where a quote closes the key first.
            ("a.json", b"{ \"password\": \"hunter2hunter2\", \"x\": 1 }\n", Some(SecretHit::Content)),
            ("a.json", b"{ 'api_key' : 'abcd1234' }\n", Some(SecretHit::Content)),
            ("a.json", b"{ \"secret\": true }\n", None),
        ];
        for (rel, bytes, want) in cases {
            let scan = scan_attachment(rel, bytes);
            assert_eq!(scan.hit, *want, "{rel} {:?}", String::from_utf8_lossy(bytes));
            assert!(scan.read_as_text, "{rel}");
        }
        // A NUL doesn't hide a key, and past the old limits it's still read.
        let mut hidden = vec![0u8];
        hidden.extend_from_slice(pem.as_bytes());
        let scan = scan_attachment("a.bin", &hidden);
        assert_eq!(scan.hit, Some(SecretHit::Content));
        assert!(!scan.read_as_text);
        let mut big = vec![b'a'; MAX_SCAN_BYTES as usize + 9000];
        big.extend_from_slice(b"\n");
        big.extend_from_slice(pem.as_bytes());
        assert_eq!(scan_attachment("big.txt", &big).hit, Some(SecretHit::Content));
        // A binary file with nothing in it isn't refused, and says it wasn't read.
        let scan = scan_attachment("photo.bin", b"\0\x01\x02 password = hunter2hunter2\n");
        assert_eq!(scan.hit, None, "assignment words in a binary file are noise");
        assert!(!scan.read_as_text);
    }

    /// The list clone and Threads use takes the new folders, not the new token
    /// shapes: a false hit there drops a source file from a clone.
    #[test]
    fn the_shared_list_has_the_folders_but_not_the_outbound_token_shapes() {
        for rel in [".aws/credentials", "deploy/.aws/template.yaml", "k8s/.kube/README.md", ".gnupg/x", "x/.docker/config.json"] {
            assert!(is_secret_rel(rel), "{rel}");
        }
        for rel in ["docker/config.json", ".aws-sam/build.toml"] {
            assert!(!is_secret_rel(rel), "{rel}");
        }
        for text in [
            "sk-ant-api03-abcdefghijklmnop",
            "sk_live_abcd1234efgh5678",
            "xoxb-1234567890-abcdefgh",
            "key AKIAZ2GH7QK4LMNP8RST here",
            "k2rs_0123456789abcdefghijABCDEFGHIJ0123456789a",
        ] {
            assert!(!content_has_credential_str(text), "{text}");
        }
        assert!(content_has_credential_str("gho_abcdefghijklmnopqrstuvwxyz"));
        assert!(content_has_credential_str("password = x"), "the shared rule is unchanged");
    }

    /// A key word repeated all along one long line used to take hours, in the
    /// daemon. The file below took about 29 minutes before the window.
    #[test]
    fn a_pathological_file_scans_in_seconds() {
        let mut file = "secret:a b ".repeat((1 << 20) / 11).into_bytes();
        file.extend(std::iter::repeat(b' ').take(24 * 1024 * 1024));
        let started = std::time::Instant::now();
        let scan = scan_attachment("big.txt", &file);
        let took = started.elapsed();
        assert_eq!(scan.hit, None);
        assert!(took.as_secs() < 60, "took {took:?}");
        let shared = format!("secret:{}", " ".repeat(ASSIGNMENT_WINDOW + 1)).repeat(200);
        let started = std::time::Instant::now();
        assert!(!content_has_credential_str(&shared));
        assert!(started.elapsed().as_secs() < 60, "the shared rule is bounded too");
        // A value past the window isn't seen, one inside it is, and a window
        // never cuts a character.
        let far = format!("password:{}hunter2hunter2\n", " ".repeat(ASSIGNMENT_WINDOW));
        assert_eq!(scan_attachment("a.txt", far.as_bytes()).hit, None);
        let near = format!("password:{}hunter2hunter2\n", " ".repeat(100));
        assert_eq!(scan_attachment("a.txt", near.as_bytes()).hit, Some(SecretHit::Content));
        let wide = format!("password:{}", "é".repeat(ASSIGNMENT_WINDOW));
        assert_eq!(scan_attachment("a.txt", wide.as_bytes()).hit, Some(SecretHit::Content));
    }
}
