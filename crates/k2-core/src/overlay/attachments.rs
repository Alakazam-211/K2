//! File objects on Thread messages (0.46.1, Rosson 2026-10-18; eval
//! `.k2/prds/eval-thread-file-objects-v1.md`, slices S1 + S2). Feature key
//! `thread-attachments-v1`.
//!
//! An attachment is a **pointer, never a copy and never a grant**: the doc
//! keeps the file's workspace-relative path plus a snapshot (name, size,
//! type, modified time, sha256), and every view re-checks the file
//! (`ok` / `changed` / `gone`). Only the daemon writes the field, after
//! its own checks at send time:
//!
//! 1. the path resolves (realpath, symlinks followed) to a regular file
//!    whose real path is inside the Thread's workspace
//!    ([`crate::file_links::resolve_link`], the same resolver the Thread's
//!    inline file links use);
//! 2. it is not a secret ([`crate::secret_paths::classify`]: the shared
//!    path list plus the content scan); refused with no override;
//! 3. name, size, type and modified time are snapshotted, and the sha256
//!    when the file is at most [`HASH_MAX_BYTES`].
//!
//! Every refusal refuses the whole post. Folders are not attachments yet
//! (`kind` is a string so `folder` can come later without a migration).
//! The doc lives in the overlay redb store as JSON: no SQL migration.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::secret_paths::{self, SecretHit};

/// The only kind in v1.
pub const KIND_FILE: &str = "file";
/// Most attachments one message carries.
pub const MAX_PER_MESSAGE: usize = 10;
/// Longest path accepted (characters), the same as the R3 folder limit.
pub const MAX_PATH_CHARS: usize = 1024;
/// Files above this are not hashed (`sha256: null`).
pub const HASH_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Live states (computed at view time, never stored).
pub const STATE_OK: &str = "ok";
pub const STATE_CHANGED: &str = "changed";
pub const STATE_GONE: &str = "gone";
/// A guest whose file rights don't cover the path: `{kind, state}` only.
pub const STATE_HIDDEN: &str = "hidden";

/// One attachment as stored on the doc.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadAttachment {
    /// `file` (v1).
    pub kind: String,
    /// Workspace-relative, `/` separators, canonical (symlinks resolved).
    pub path: String,
    /// The file name at send time.
    pub name: String,
    /// Bytes at send time.
    pub size: u64,
    /// A content type from the name.
    pub mime: String,
    /// Milliseconds since the epoch, at send time.
    pub modified_at: i64,
    /// Hex sha256 at send time; `None` above [`HASH_MAX_BYTES`].
    pub sha256: Option<String>,
    /// 1-based line to open at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Last line of a range (≥ `line`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
}

/// One attachment as a sender asks for it (`attachments` on
/// `POST /cli/thread/post`): a path (absolute, or relative to the
/// workspace root) and an optional line or range.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentRequest {
    /// `relPath` is the same field under the catalog's name (the App SDK
    /// and Garden widgets never send a field called `path`, UWA6).
    #[serde(alias = "relPath")]
    pub path: String,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub line_end: Option<u32>,
}

/// Why a post's attachments were refused (the whole post is refused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachError {
    /// Malformed, too many, too long, a bad line.
    Usage(String),
    /// The real path is outside the Thread's workspace (a symlink target
    /// included). `input` is the path as sent.
    Outside { input: String },
    /// Missing, a folder, a socket.
    NotAFile { input: String },
    /// A secret by path or content. `rel` is the workspace-relative path.
    Secret { rel: String, hit: SecretHit },
}

impl AttachError {
    /// The wire error code.
    pub fn code(&self) -> &'static str {
        match self {
            AttachError::Usage(_) => "usage",
            AttachError::Outside { .. } => "attachment_outside_workspace",
            AttachError::NotAFile { .. } => "attachment_not_a_file",
            AttachError::Secret { .. } => "attachment_secret",
        }
    }

    /// The wire hint.
    pub fn hint(&self) -> String {
        match self {
            AttachError::Usage(m) => m.clone(),
            AttachError::Outside { input } => format!(
                "{input} is outside this Thread's workspace; copy it into the workspace (e.g. out/) and attach that"
            ),
            AttachError::NotAFile { input } => format!("{input} is not a file in this workspace (missing, or a folder)"),
            AttachError::Secret { rel, .. } => format!(
                "{rel} looks like a secret and can't ride a Thread message; secrets travel through the vault (k2 vault), not the Thread"
            ),
        }
    }

    /// For the audit line: `outside`, `not_a_file`, `secret_path`,
    /// `secret_content`, `usage`.
    pub fn reason(&self) -> &'static str {
        match self {
            AttachError::Usage(_) => "usage",
            AttachError::Outside { .. } => "outside",
            AttachError::NotAFile { .. } => "not_a_file",
            AttachError::Secret { hit: SecretHit::Path, .. } => "secret_path",
            AttachError::Secret { hit: SecretHit::Content, .. } => "secret_content",
        }
    }
}

/// Parse the `attachments` JSON (an array of [`AttachmentRequest`]).
pub fn parse_requests(raw: &str) -> Result<Vec<AttachmentRequest>, AttachError> {
    let reqs: Vec<AttachmentRequest> = serde_json::from_str(raw)
        .map_err(|e| AttachError::Usage(format!("attachments must be [{{path, line?, lineEnd?}}]: {e}")))?;
    if reqs.len() > MAX_PER_MESSAGE {
        return Err(AttachError::Usage(format!("at most {MAX_PER_MESSAGE} files per message")));
    }
    for r in &reqs {
        let p = r.path.trim();
        if p.is_empty() {
            return Err(AttachError::Usage("an attachment needs a path".into()));
        }
        if p.chars().count() > MAX_PATH_CHARS {
            return Err(AttachError::Usage(format!("an attachment path is at most {MAX_PATH_CHARS} characters")));
        }
        // 0.46.6 review: a path with a control character (a newline) would
        // land verbatim in the agent's `Attached:` line.
        if p.chars().any(char::is_control) {
            return Err(AttachError::Usage("an attachment path has a control character".into()));
        }
        match (r.line, r.line_end) {
            (Some(0), _) => return Err(AttachError::Usage("line numbers start at 1".into())),
            (None, Some(_)) => return Err(AttachError::Usage("lineEnd needs line".into())),
            (Some(a), Some(b)) if b < a => return Err(AttachError::Usage("lineEnd must be at least line".into())),
            _ => {}
        }
    }
    Ok(reqs)
}

/// Check and snapshot every requested file against the workspace `root`
/// (as registered). All or nothing.
pub fn snapshot_all(
    root: &Path,
    home: Option<&Path>,
    reqs: &[AttachmentRequest],
) -> Result<Vec<ThreadAttachment>, AttachError> {
    reqs.iter().map(|r| snapshot_one(root, home, r)).collect()
}

fn joined_path(root: &Path, home: Option<&Path>, raw: &str) -> Option<PathBuf> {
    if raw == "~" || raw.starts_with("~/") {
        let h = home.filter(|h| h.is_absolute())?;
        return Some(h.join(raw.trim_start_matches('~').trim_start_matches('/')));
    }
    if raw.starts_with('~') {
        return None;
    }
    let p = Path::new(raw);
    Some(if p.is_absolute() { p.to_path_buf() } else { root.join(p) })
}

fn snapshot_one(root: &Path, home: Option<&Path>, req: &AttachmentRequest) -> Result<ThreadAttachment, AttachError> {
    let input = req.path.trim().to_string();
    if input.contains('\0') {
        return Err(AttachError::Usage("an attachment path has a NUL byte".into()));
    }
    let not_a_file = || AttachError::NotAFile { input: input.clone() };
    let joined = joined_path(root, home, &input).ok_or_else(not_a_file)?;
    // Missing (or a dangling link) first: there is nothing to attach.
    let meta = std::fs::metadata(&joined).map_err(|_| not_a_file())?;
    // The shared resolver decides containment (realpath under the real
    // root); it answers a bare "not linkable" for anything outside.
    let link = crate::file_links::resolve_link(root, home, &input);
    if !link.linkable {
        return Err(AttachError::Outside { input });
    }
    if link.kind != Some("file") || !meta.is_file() {
        return Err(not_a_file());
    }
    let rel = link.rel.unwrap_or_default();
    if rel.is_empty() {
        return Err(not_a_file());
    }
    let real = root.join(&rel);
    if let Some(hit) = secret_paths::classify(&rel, &real) {
        return Err(AttachError::Secret { rel, hit });
    }
    let meta = std::fs::metadata(&real).map_err(|_| not_a_file())?;
    let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
    let sha256 = if meta.len() <= HASH_MAX_BYTES { sha256_file(&real) } else { None };
    Ok(ThreadAttachment {
        kind: KIND_FILE.to_string(),
        mime: mime_for_name(&name).to_string(),
        name,
        path: rel,
        size: meta.len(),
        modified_at: modified_ms(&meta),
        sha256,
        line: req.line,
        line_end: req.line_end,
    })
}

/// Milliseconds since the epoch (0 when the disk keeps none).
pub fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A streaming hex sha256 of a file; `None` when it can't be read.
pub fn sha256_file(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Text-ish extensions the fs table calls a blob: shown as `text/plain`
/// so a phone knows it can preview them.
const TEXT_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "rb", "go", "java", "kt", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "sh", "bash", "zsh", "fish", "toml", "yaml", "yml", "ini", "cfg", "conf", "css",
    "scss", "less", "sql", "lua", "php", "pl", "r", "dart", "vue", "svelte", "tex", "rst", "adoc", "diff",
    "patch", "gradle", "proto", "graphql", "lock", "tsv",
];

/// The content type of an attachment: the fs table's, or `text/plain` for
/// source and config files it doesn't list.
pub fn mime_for_name(name: &str) -> &'static str {
    let m = crate::fs_commands::mime_for_name(name);
    if m != "application/octet-stream" {
        return m;
    }
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if TEXT_EXTS.contains(&ext.as_str()) || matches!(name, "Makefile" | "Dockerfile" | "LICENSE" | "README") {
        "text/plain"
    } else {
        m
    }
}

/// The file an attachment points at, re-jailed now: `root` + `rel` with
/// no `..` or absolute part, its real path still inside the real root,
/// still a regular file, and not a secret by path. `None` = gone.
pub fn resolve_now(root: &Path, att: &ThreadAttachment) -> Option<PathBuf> {
    let rel = Path::new(&att.path);
    if att.path.is_empty() || rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    if secret_paths::is_secret_rel(&att.path) {
        return None;
    }
    let real_root = dunce::canonicalize(&root).ok()?;
    let real = dunce::canonicalize(&root.join(rel)).ok()?;
    if !real.starts_with(&real_root) {
        return None;
    }
    // 0.46.6 review: the file it resolves to NOW is checked by path too, so
    // an attachment swapped for a link to `.env` or a key reads as gone.
    let now_rel = real.strip_prefix(&real_root).ok()?.to_string_lossy().replace('\\', "/");
    if secret_paths::is_secret_rel(&now_rel) {
        return None;
    }
    std::fs::metadata(&real).ok().filter(|m| m.is_file())?;
    Some(real)
}

/// The live state of one attachment against the file as it is now, from a
/// stat (`rehash`: a size match with another mtime re-hashes, so a `touch`
/// that left the bytes alone stays `ok`).
pub fn live_state(root: &Path, att: &ThreadAttachment, rehash: bool) -> &'static str {
    let Some(real) = resolve_now(root, att) else { return STATE_GONE };
    let Ok(meta) = std::fs::metadata(&real) else { return STATE_GONE };
    if meta.len() == att.size && modified_ms(&meta) == att.modified_at {
        return STATE_OK;
    }
    if rehash && meta.len() == att.size {
        if let Some(want) = att.sha256.as_deref() {
            if sha256_file(&real).as_deref() == Some(want) {
                return STATE_OK;
            }
        }
    }
    STATE_CHANGED
}

/// An attachment as a viewer sees it: the stored fields plus `state`.
pub fn view_json(att: &ThreadAttachment, state: &str) -> serde_json::Value {
    let mut v = serde_json::to_value(att).unwrap_or(serde_json::Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("state".into(), serde_json::json!(state));
    }
    v
}

/// What a viewer without the rights sees: no name, no path, no size.
pub fn hidden_json() -> serde_json::Value {
    serde_json::json!({ "kind": KIND_FILE, "state": STATE_HIDDEN })
}

/// `[file]` / `[N files]`: what a push preview and a list preview show for
/// a post's attachments. Never a name.
pub fn placeholder(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("[file]".to_string()),
        n => Some(format!("[{n} files]")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn layout(label: &str) -> (Tmp, PathBuf, PathBuf) {
        let t = std::env::temp_dir().join(format!("k2-attach-{label}-{}", uuid::Uuid::new_v4()));
        let ws = t.join("ws");
        fs::create_dir_all(ws.join("docs")).unwrap();
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::write(ws.join("docs/report.pdf"), b"%PDF-1.7 report").unwrap();
        fs::write(ws.join("src/session.rs"), "fn main() {}\n".repeat(50)).unwrap();
        let outside = t.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("other.txt"), b"other").unwrap();
        (Tmp(t), ws, outside)
    }

    fn req(path: &str) -> AttachmentRequest {
        AttachmentRequest { path: path.to_string(), line: None, line_end: None }
    }

    #[test]
    fn snapshot_records_the_file() {
        let (_t, ws, _) = layout("snap");
        let got = snapshot_all(&ws, None, &[req("docs/report.pdf")]).expect("ok");
        assert_eq!(got.len(), 1);
        let a = &got[0];
        assert_eq!(a.kind, "file");
        assert_eq!(a.path, "docs/report.pdf");
        assert_eq!(a.name, "report.pdf");
        assert_eq!(a.size, 15);
        assert_eq!(a.mime, "application/pdf");
        assert!(a.modified_at > 0);
        assert_eq!(a.sha256.as_deref(), Some(sha256_file(&ws.join("docs/report.pdf")).unwrap().as_str()));
        assert_eq!(a.sha256.as_ref().unwrap().len(), 64);
        // Absolute inside works and stores the relative path; a line rides.
        let abs = ws.join("src/session.rs");
        let got = snapshot_all(
            &ws,
            None,
            &[AttachmentRequest { path: abs.to_string_lossy().into_owned(), line: Some(12), line_end: Some(40) }],
        )
        .expect("ok");
        assert_eq!(got[0].path, "src/session.rs");
        assert_eq!(got[0].mime, "text/plain");
        assert_eq!((got[0].line, got[0].line_end), (Some(12), Some(40)));
    }

    #[test]
    fn refusals() {
        let (_t, ws, outside) = layout("refuse");
        let out = outside.join("other.txt").to_string_lossy().into_owned();
        assert_eq!(snapshot_all(&ws, None, &[req(&out)]).unwrap_err().code(), "attachment_outside_workspace");
        assert_eq!(
            snapshot_all(&ws, None, &[req("../outside/other.txt")]).unwrap_err().code(),
            "attachment_outside_workspace"
        );
        assert_eq!(snapshot_all(&ws, None, &[req("docs")]).unwrap_err().code(), "attachment_not_a_file");
        assert_eq!(snapshot_all(&ws, None, &[req("docs/nope.md")]).unwrap_err().code(), "attachment_not_a_file");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("other.txt"), ws.join("leak.txt")).unwrap();
            assert_eq!(snapshot_all(&ws, None, &[req("leak.txt")]).unwrap_err().code(), "attachment_outside_workspace");
        }
        fs::write(ws.join(".env"), "A=1").unwrap();
        let e = snapshot_all(&ws, None, &[req(".env")]).unwrap_err();
        assert_eq!((e.code(), e.reason()), ("attachment_secret", "secret_path"));
        fs::write(ws.join("notes.md"), "token: ghp_abcdefghijklmnopqrstuvwxyz0123\n").unwrap();
        let e = snapshot_all(&ws, None, &[req("notes.md")]).unwrap_err();
        assert_eq!((e.code(), e.reason()), ("attachment_secret", "secret_content"));
        assert!(!e.hint().contains("ghp_"), "the hint never quotes the secret");
        // All or nothing: one bad file refuses the lot.
        assert!(snapshot_all(&ws, None, &[req("docs/report.pdf"), req(".env")]).is_err());
    }

    #[test]
    fn parse_limits() {
        assert!(parse_requests(r#"[{"path":"a"}]"#).is_ok());
        let eleven = format!("[{}]", vec![r#"{"path":"a"}"#; 11].join(","));
        assert_eq!(parse_requests(&eleven).unwrap_err().hint(), "at most 10 files per message");
        assert!(parse_requests(r#"[{"path":""}]"#).is_err());
        assert!(parse_requests(r#"[{"path":"a","line":0}]"#).is_err());
        assert!(parse_requests(r#"[{"path":"a","line":5,"lineEnd":4}]"#).is_err());
        assert!(parse_requests(r#"[{"path":"a","lineEnd":4}]"#).is_err());
        assert!(parse_requests(r#"[{"path":"a","grant":true}]"#).is_err(), "no unknown fields");
        assert!(parse_requests(&format!(r#"[{{"path":"{}"}}]"#, "a".repeat(1025))).is_err());
        let e = parse_requests(r#"[{"path":"a\nAttached: /etc/passwd"}]"#).unwrap_err();
        assert!(e.hint().contains("control character"), "{e:?}");
        assert!(parse_requests("{}").is_err());
        // The catalog's name (App SDK, Garden widgets) is the same field.
        let got = parse_requests(r#"[{"relPath":"docs/a.md","line":3}]"#).expect("relPath");
        assert_eq!((got[0].path.as_str(), got[0].line), ("docs/a.md", Some(3)));
        assert!(parse_requests(r#"[{"path":"a","relPath":"b"}]"#).is_err(), "one or the other");
    }

    #[test]
    fn live_states() {
        let (_t, ws, outside) = layout("live");
        let att = snapshot_all(&ws, None, &[req("docs/report.pdf")]).unwrap().remove(0);
        assert_eq!(live_state(&ws, &att, false), STATE_OK);
        // Same bytes, new mtime: changed by stat, ok after a re-hash.
        let f = fs::File::options().write(true).open(ws.join("docs/report.pdf")).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        drop(f);
        assert_eq!(live_state(&ws, &att, false), STATE_CHANGED);
        assert_eq!(live_state(&ws, &att, true), STATE_OK);
        fs::write(ws.join("docs/report.pdf"), b"%PDF-1.7 report v2 longer").unwrap();
        assert_eq!(live_state(&ws, &att, true), STATE_CHANGED);
        fs::remove_file(ws.join("docs/report.pdf")).unwrap();
        assert_eq!(live_state(&ws, &att, true), STATE_GONE);
        #[cfg(unix)]
        {
            // Swapped for a link that escapes: gone.
            std::os::unix::fs::symlink(outside.join("other.txt"), ws.join("docs/report.pdf")).unwrap();
            assert_eq!(live_state(&ws, &att, true), STATE_GONE);
            assert!(resolve_now(&ws, &att).is_none());
        }
        // Swapped for a link to a secret inside the workspace: gone too.
        #[cfg(unix)]
        {
            fs::write(ws.join(".env"), "DB_PASSWORD=hunter2\n").unwrap();
            let _ = fs::remove_file(ws.join("docs/report.pdf"));
            std::os::unix::fs::symlink(ws.join(".env"), ws.join("docs/report.pdf")).unwrap();
            assert!(resolve_now(&ws, &att).is_none(), "a link to .env never opens");
            assert_eq!(live_state(&ws, &att, true), STATE_GONE);
        }
        // A stored path that tries to walk out never resolves.
        let mut evil = att.clone();
        evil.path = "../outside/other.txt".into();
        assert!(resolve_now(&ws, &evil).is_none());
        evil.path = outside.join("other.txt").to_string_lossy().into_owned();
        assert!(resolve_now(&ws, &evil).is_none());
    }

    #[test]
    fn old_docs_parse_and_docs_without_files_omit_the_field() {
        let old = r#"{"id":"a","kind":"text","from":"sales","created_at":1,"body":"hi","via":"thread"}"#;
        let doc: crate::overlay::OverlayDoc = serde_json::from_str(old).expect("old doc parses");
        assert!(doc.attachments.is_empty());
        let back = serde_json::to_string(&doc).unwrap();
        assert!(!back.contains("attachments"), "{back}");
        let (_t, ws, _) = layout("doc");
        let mut doc = doc;
        doc.attachments = snapshot_all(&ws, None, &[req("docs/report.pdf")]).unwrap();
        let v: serde_json::Value = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["attachments"][0]["path"], "docs/report.pdf");
        assert!(v["attachments"][0].get("state").is_none(), "state is never stored");
        assert!(v["attachments"][0].get("line").is_none(), "no line, no key");
        let round: crate::overlay::OverlayDoc = serde_json::from_value(v).unwrap();
        assert_eq!(round, doc);
    }

    #[test]
    fn placeholders_never_name_a_file() {
        assert_eq!(placeholder(0), None);
        assert_eq!(placeholder(1).as_deref(), Some("[file]"));
        assert_eq!(placeholder(3).as_deref(), Some("[3 files]"));
        assert_eq!(hidden_json(), serde_json::json!({ "kind": "file", "state": "hidden" }));
    }
}
