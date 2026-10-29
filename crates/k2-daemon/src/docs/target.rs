//! Which file a docs call names, and whether it may be a live doc
//! (Markdown multiplayer v1, K2; vs-live MI8, MI12).
//!
//! Every caller is normalised to `(project id, workspace-relative path)`,
//! the kit's [`DocKey`]:
//! - a relative `path` resolves under `workspace` (a handle, name, id or
//!   root path; an agent's own workspace when it names none);
//! - an absolute `path` resolves to the registered workspace whose root
//!   holds it (the longest root wins).
//!
//! Then the jail: the file must exist, and after symlinks it must still be
//! inside the workspace root (`fs_routes::jail_rel_path`). The kit re-checks
//! nothing about paths (it never touches disk in this slice).
//!
//! **Which tier, by file name** (the text pack's HOST-INTEGRATION §1): the
//! more specific pack first, `.md` / `.markdown` → the Markdown pack;
//! then any text, by the text pack's table (`TextPack::for_path`: code,
//! config, markup, diagram sources, `.txt`, unknown names, and `.csv` /
//! `.tsv` as `csv-as-text` until a grid pack exists) → the text pack (tier
//! 1, live editing); everything else (images, media, office files,
//! archives, binaries) → a tier-0 doc (the kit's blob docs:
//! presence, a lease, versions and restore), up to
//! [`super::blob_max_bytes`] (`too_large` past it). `.mdx` is JSX, so it is
//! tier 0. A text file a pack refuses at open (`too_large`, `not_utf8`,
//! `binary`, `mixed_line_endings`) falls back to tier 0 with the reason
//! shown (`routes`), never to a plain editor. A path keeps its kind for
//! life (`mv` can't change Markdown, text or any-file).
//!
//! **Never live docs** (MI12), refused before any open, on every tier:
//! - secret files by path or content (`secret_paths::classify`), `secret`;
//! - `.k2/vault/**`, `protected`;
//! - files K2 generates and rewrites (AGENTS.md, its own skills, rosters,
//!   the harness mirrors), `generated`. An edit there would be overwritten
//!   by the next recompose.

use std::path::{Path, PathBuf};

use k2_mpc_base::store::DocKey;

/// How much of the kit a file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Markdown (tier 1): live co-editing, text tools, undo.
    Markdown,
    /// Any other text (tier 1, the text pack): live co-editing, text tools,
    /// undo; `family` picks the editor mode.
    Text(k2_mpc_text::Family),
    /// Any other file (tier 0): presence, a lease, versions and restore.
    Blob,
}

impl Tier {
    /// The tier a workspace-relative path gets (by name only).
    pub fn of(rel: &str) -> Tier {
        if k2_mpc_markdown::is_markdown_path(rel) {
            Tier::Markdown
        } else if let Some(pack) = k2_mpc_text::TextPack::for_path(rel) {
            Tier::Text(pack.family())
        } else {
            Tier::Blob
        }
    }

    /// On the wire: `1` (live text: Markdown or the text pack) or `0` (any
    /// file).
    pub fn number(self) -> u8 {
        match self {
            Tier::Markdown | Tier::Text(_) => 1,
            Tier::Blob => 0,
        }
    }

    /// The pack's name on the wire: `markdown`, `text`, or `blob`.
    pub fn pack(self) -> &'static str {
        match self {
            Tier::Markdown => "markdown",
            Tier::Text(_) => "text",
            Tier::Blob => "blob",
        }
    }

    /// The editor family (`markdown`, the text pack's `text` / `code` /
    /// `config` / `markup` / `diagram` / `csv-as-text`, or `none`).
    pub fn family(self) -> &'static str {
        match self {
            Tier::Markdown => "markdown",
            Tier::Text(f) => f.name(),
            Tier::Blob => "none",
        }
    }

    /// The same kind of doc (`mv` keeps it): Markdown, text (any family:
    /// one schema), or any-file.
    pub fn same_kind(self, other: Tier) -> bool {
        matches!((self, other), (Tier::Markdown, Tier::Markdown) | (Tier::Text(_), Tier::Text(_)) | (Tier::Blob, Tier::Blob))
    }
}

/// A resolved, jailed doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub project_id: String,
    /// Canonical workspace root.
    pub root: PathBuf,
    /// Workspace-relative path, `/`-separated (the doc key's path).
    pub rel: String,
    /// Canonical absolute file path.
    pub abs: PathBuf,
    pub tier: Tier,
}

impl Target {
    pub fn key(&self) -> DocKey {
        DocKey { space: self.project_id.clone(), path: self.rel.clone() }
    }
}

/// Why a path can't be a doc. `code` goes on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Refusal { status: "400 Bad Request", code, message: message.into() }
    }
    fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Refusal { status: "404 Not Found", code, message: message.into() }
    }
    fn declined(code: &'static str, message: impl Into<String>) -> Self {
        Refusal { status: "422 Unprocessable Entity", code, message: message.into() }
    }
}

/// Files K2 writes itself (vs-live §3.2). Workspace-relative, lowercase
/// compared. `*` matches one path segment.
const GENERATED: &[&str] = &[
    ".k2/agents.md",
    ".k2/skills/k2-cli/skill.md",
    ".k2/skills/k2-canonical-agents/skill.md",
    ".k2/skills/k2-zen/skill.md",
    ".k2/skills/k2-permissions/skill.md",
    ".k2/skills/k2so/skill.md",
    ".k2/context/catalog/*-roster.md",
    ".k2/context/catalog/apps-maintained.md",
    ".claude/skills/*/skill.md",
    ".opencode/agent/*.md",
    ".pi/skills/*/skill.md",
];

/// Root-level harness mirrors K2 keeps as symlinks to `.k2/` files
/// (`workspace/harness.rs`). Generated only when they are symlinks; a
/// person's own `CLAUDE.md` file is theirs.
const MIRRORS: &[&str] = &["claude.md", "gemini.md", "agent.md", "agents.md"];

fn segment_match(pattern: &str, seg: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == seg,
        Some((pre, post)) => seg.len() >= pre.len() + post.len() && seg.starts_with(pre) && seg.ends_with(post),
    }
}

/// True for a path K2 generates (lowercase compare; `*` = one segment).
pub fn is_generated(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let segs: Vec<&str> = lower.split('/').collect();
    GENERATED.iter().any(|g| {
        let gs: Vec<&str> = g.split('/').collect();
        gs.len() == segs.len() && gs.iter().zip(&segs).all(|(p, s)| segment_match(p, s))
    })
}

/// True under `.k2/vault/`.
pub fn is_vault(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower == ".k2/vault" || lower.starts_with(".k2/vault/")
}

/// True inside any `.git` folder: git's database, its config and its hooks
/// (a written hook would run as the workspace's user).
pub fn is_git_internal(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let segs: Vec<&str> = lower.split('/').collect();
    segs.len() > 1 && segs[..segs.len() - 1].contains(&".git")
}

/// `.k2/` folders whose other files may be tier-0 docs (wiki attachments,
/// PRD images). Everything else under `.k2/` is K2's configuration
/// (settings, heartbeats, hooks, rosters): never written through a doc.
const K2_DOC_DIRS: &[&str] = &[".k2/wiki/", ".k2/prds/", ".k2/docs/"];

/// True for a path under `.k2/` outside [`K2_DOC_DIRS`].
pub fn is_k2_config(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.starts_with(".k2/") && !K2_DOC_DIRS.iter().any(|d| lower.starts_with(d))
}

/// `n` bytes for a person: `512 B`, `3.0 KiB`, `40.0 MiB`.
pub fn human_bytes(n: u64) -> String {
    match n {
        n if n < 1 << 10 => format!("{n} B"),
        n if n < 1 << 20 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{:.1} MiB", n as f64 / 1_048_576.0),
    }
}

/// A file over the any-file cap is declined (exit 4), saying both sizes:
/// it can't be a tier-0 doc, nor the tier-0 fallback of a text doc whose
/// pack (with its own, smaller cap) refuses it.
pub fn size_cap(rel: &str, len: u64, cap: u64) -> Result<(), Refusal> {
    if len > cap {
        return Err(Refusal::declined(
            "too_large",
            format!(
                "{rel} is {}; files over {} can't be live docs (any-file docs keep every version)",
                human_bytes(len),
                human_bytes(cap)
            ),
        ));
    }
    Ok(())
}

/// The deny list for an existing, jailed file (`rel` workspace-relative,
/// `link` the path before symlinks were resolved, `abs` after). Applies to
/// every tier.
pub fn deny(rel: &str, link: &Path, abs: &Path) -> Result<(), Refusal> {
    if is_vault(rel) {
        return Err(Refusal::declined("protected", format!("{rel} is in the vault folder; it is never a live doc")));
    }
    if is_git_internal(rel) {
        return Err(Refusal::declined("protected", format!("{rel} is inside git's own folder; it is never a live doc")));
    }
    if Tier::of(rel) != Tier::Markdown && is_k2_config(rel) {
        return Err(Refusal::declined(
            "protected",
            format!("{rel} is K2's own configuration; only Markdown and files under .k2/wiki, .k2/prds or .k2/docs are live docs there"),
        ));
    }
    let link_name = link.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_root_mirror = MIRRORS.contains(&link_name.as_str())
        && std::fs::symlink_metadata(link).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if is_generated(rel) || is_root_mirror {
        return Err(Refusal::declined(
            "generated",
            format!("K2 writes {rel} itself, so it can't be a live doc; edit its source instead"),
        ));
    }
    if let Some(hit) = k2_core::secret_paths::classify(rel, abs) {
        return Err(Refusal::declined(
            "secret",
            format!("{rel} looks like it holds a secret ({}); it is never a live doc", hit.as_str()),
        ));
    }
    Ok(())
}

/// A registered workspace: `(project id, root path)`.
fn workspace_by_token(token: &str) -> Result<(String, String), Refusal> {
    let path = crate::workspace_msg::resolve_workspace_allowing_handle(token.trim())
        .ok_or_else(|| Refusal::not_found("workspace_not_found", format!("no workspace '{}' on this server", token.trim())))?;
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    }
    .ok_or_else(|| Refusal::not_found("workspace_not_found", format!("no workspace '{}' on this server", token.trim())))?;
    Ok((id, path))
}

fn workspace_by_id(id: &str) -> Result<(String, String), Refusal> {
    let path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, id).ok().map(|p| p.path)
    }
    .ok_or_else(|| Refusal::not_found("workspace_not_found", "your workspace is no longer registered"))?;
    Ok((id.to_string(), path))
}

/// The registered workspace whose root holds the canonical `abs` (longest
/// root wins: a workspace nested inside another owns its own files).
pub(super) fn workspace_holding(abs: &Path) -> Result<(String, PathBuf), Refusal> {
    let projects = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::list(&conn).unwrap_or_default()
    };
    projects
        .into_iter()
        .filter_map(|p| Path::new(&p.path).canonicalize().ok().map(|root| (p.id, root)))
        .filter(|(_, root)| abs.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .ok_or_else(|| Refusal::not_found("workspace_not_found", "that file is not inside a workspace on this server"))
}

fn rel_string(root: &Path, abs: &Path) -> Result<String, Refusal> {
    let rel = abs
        .strip_prefix(root)
        .map_err(|_| Refusal::bad("path_escapes", "path escapes the workspace"))?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    if parts.is_empty() {
        return Err(Refusal::bad("not_a_file", "name a file, not the workspace root"));
    }
    Ok(parts.join("/"))
}

/// Where `mv` puts a doc: `to` relative to the doc's workspace (or absolute
/// inside it). The new name must not exist yet, its folder must, it stays
/// in the jail, it keeps the doc's kind (Markdown stays Markdown, text stays
/// text, any other file stays any-file), and it passes the deny list by path (not the
/// vault, not generated, not a secret's name). Returns `(rel, abs)`.
pub fn resolve_new(t: &Target, to: &str) -> Result<(String, PathBuf), Refusal> {
    let to = to.trim();
    if to.is_empty() || to.contains('\0') {
        return Err(Refusal::bad("bad_request", "to is required: the new name"));
    }
    let rel_in = if Path::new(to).is_absolute() {
        let parent = Path::new(to)
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .ok_or_else(|| Refusal::not_found("folder_not_found", format!("no folder for {to}")))?;
        let name = Path::new(to).file_name().ok_or_else(|| Refusal::bad("bad_request", "to names no file"))?;
        let abs = parent.join(name);
        rel_string(&t.root, &abs).map_err(|_| Refusal::bad("other_workspace", "mv stays inside the doc's workspace"))?
    } else {
        to.to_string()
    };
    let root = t.root.to_string_lossy().to_string();
    let abs = crate::fs_routes::jail_rel_path(&root, &rel_in, true).map_err(|e| Refusal::bad("path_escapes", e))?;
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(Refusal { status: "409 Conflict", code: "exists", message: format!("{rel_in} already exists") });
    }
    if !abs.parent().is_some_and(Path::is_dir) {
        return Err(Refusal::not_found("folder_not_found", format!("no folder for {rel_in}")));
    }
    let parent = abs.parent().and_then(|p| p.canonicalize().ok()).ok_or_else(|| Refusal::bad("path_escapes", "bad folder"))?;
    let abs = parent.join(abs.file_name().unwrap_or_default());
    let rel = rel_string(&t.root, &abs)?;
    if !Tier::of(&rel).same_kind(t.tier) {
        return Err(Refusal::bad(
            "tier_change",
            match t.tier {
                Tier::Markdown => format!("{} is a Markdown doc; mv keeps it one (name it .md or .markdown)", t.rel),
                Tier::Text(_) => format!("{} is a text doc; mv keeps it text ({rel} isn't: Markdown, or not text)", t.rel),
                Tier::Blob => format!("{} is an any-file doc; mv keeps it one ({rel} would be a text doc)", t.rel),
            },
        ));
    }
    if is_vault(&rel) {
        return Err(Refusal::declined("protected", format!("{rel} is in the vault folder")));
    }
    if is_generated(&rel) {
        return Err(Refusal::declined("generated", format!("K2 writes {rel} itself")));
    }
    if is_git_internal(&rel) || (t.tier != Tier::Markdown && is_k2_config(&rel)) {
        return Err(Refusal::declined("protected", format!("{rel} is protected (git's folder or K2's configuration)")));
    }
    if k2_core::secret_paths::is_secret_rel(&rel) {
        return Err(Refusal::declined("secret", format!("{rel} is a secret's name; it is never a live doc")));
    }
    Ok((rel, abs))
}

/// A workspace by handle, name, id or root: `(project id, root path)`.
pub fn workspace_root(token: &str) -> Result<(String, String), Refusal> {
    workspace_by_token(token)
}

/// The workspace-relative path a share names (MI25): `""` or `.` is the
/// whole workspace (a folder share). With `must_exist`, the path must be an
/// existing folder, or a file that could be a live doc (the deny list
/// applies), jailed like any doc; a folder inside `.k2/vault` is refused.
/// Without it (unshare), the path is only normalised. Returns
/// `(rel, is_folder)`.
pub fn share_rel(root: &str, raw: &str, must_exist: bool) -> Result<(String, bool), Refusal> {
    let trimmed = raw.trim().trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok((String::new(), true));
    }
    if Path::new(trimmed).is_absolute() || trimmed.split('/').any(|s| s == "..") || trimmed.contains('\0') {
        return Err(Refusal::bad("bad_request", "path must be relative to the workspace, without '..'"));
    }
    if !must_exist {
        let rel = trimmed.split('/').filter(|s| !s.is_empty() && *s != ".").collect::<Vec<_>>().join("/");
        return Ok((rel, false));
    }
    let abs = crate::fs_routes::jail_rel_path(root, trimmed, false).map_err(|e| {
        if e.starts_with("Invalid path") {
            Refusal::not_found("file_not_found", format!("no file or folder {trimmed} in that workspace"))
        } else {
            Refusal::bad("path_escapes", e)
        }
    })?;
    let root_canon = Path::new(root)
        .canonicalize()
        .map_err(|e| Refusal::not_found("workspace_not_found", format!("workspace root unavailable: {e}")))?;
    if abs == root_canon {
        return Ok((String::new(), true));
    }
    let rel = rel_string(&root_canon, &abs)?;
    if abs.is_dir() {
        if is_vault(&rel) {
            return Err(Refusal::declined("protected", "the vault folder is never shared"));
        }
        return Ok((rel, true));
    }
    deny(&rel, &root_canon.join(trimmed), &abs)?;
    Ok((rel, false))
}

/// Resolve `path` (relative to `workspace`, or absolute) to a jailed,
/// allowed doc. `default_workspace` is the caller's own project id (agents),
/// used when `workspace` is empty.
pub fn resolve(workspace: Option<&str>, path: &str, default_workspace: Option<&str>) -> Result<Target, Refusal> {
    let path = path.trim();
    if path.is_empty() {
        return Err(Refusal::bad("bad_request", "path is required"));
    }
    if path.contains('\0') {
        return Err(Refusal::bad("bad_request", "path contains a NUL byte"));
    }
    let (project_id, root, link) = if Path::new(path).is_absolute() {
        let link = PathBuf::from(path);
        let abs = link
            .canonicalize()
            .map_err(|_| Refusal::not_found("file_not_found", format!("no file at {path}")))?;
        let (id, root) = workspace_holding(&abs)?;
        if let Some(ws) = workspace.map(str::trim).filter(|w| !w.is_empty()) {
            let (named, _) = workspace_by_token(ws)?;
            if named != id {
                return Err(Refusal::bad("workspace_mismatch", format!("{path} is not in workspace '{ws}'")));
            }
        }
        (id, root, link)
    } else {
        let (id, root_str) = match workspace.map(str::trim).filter(|w| !w.is_empty()) {
            Some(ws) => workspace_by_token(ws)?,
            None => match default_workspace {
                Some(own) => workspace_by_id(own)?,
                None => return Err(Refusal::bad("workspace_required", "name the workspace (workspace=<name>) or give an absolute path")),
            },
        };
        // The jail: relative only, no `..`, and after symlinks still inside
        // the root (the checks below repeat the last one for both forms).
        crate::fs_routes::jail_rel_path(&root_str, path, false).map_err(|e| {
            if e.starts_with("Invalid path") {
                Refusal::not_found("file_not_found", format!("no file {path} in that workspace"))
            } else {
                Refusal::bad("path_escapes", e)
            }
        })?;
        let root = Path::new(&root_str)
            .canonicalize()
            .map_err(|e| Refusal::not_found("workspace_not_found", format!("workspace root unavailable: {e}")))?;
        let link = root.join(path);
        (id, root, link)
    };
    let abs = link
        .canonicalize()
        .map_err(|_| Refusal::not_found("file_not_found", format!("no file {path}")))?;
    if !abs.starts_with(&root) {
        return Err(Refusal::bad("path_escapes", "path escapes the workspace"));
    }
    let meta = std::fs::metadata(&abs).map_err(|_| Refusal::not_found("file_not_found", format!("no file {path}")))?;
    if !meta.is_file() {
        return Err(Refusal::bad("not_a_file", format!("{path} is not a file")));
    }
    let rel = rel_string(&root, &abs)?;
    deny(&rel, &link, &abs)?;
    let tier = Tier::of(&rel);
    size_cap(&rel, meta.len(), super::blob_max_bytes())?;
    Ok(Target { project_id, root, rel, abs, tier })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_paths_match_by_segment() {
        for p in [
            ".k2/AGENTS.md",
            ".k2/skills/k2-cli/SKILL.md",
            ".k2/context/catalog/team-roster.md",
            ".claude/skills/anything/SKILL.md",
            ".opencode/agent/x.md",
        ] {
            assert!(is_generated(p), "{p}");
        }
        for p in [
            ".k2/wiki/Home.md",
            ".k2/skills/my-own/SKILL.md",
            ".k2/context/catalog/notes.md",
            "AGENTS.md",
            ".claude/skills/a/b/SKILL.md",
        ] {
            assert!(!is_generated(p), "{p}");
        }
    }

    #[test]
    fn vault_is_protected() {
        assert!(is_vault(".k2/vault"));
        assert!(is_vault(".k2/VAULT/x.md"));
        assert!(!is_vault(".k2/vaulted.md"));
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-docs-target-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("mkdir");
        d.canonicalize().expect("canon")
    }

    #[test]
    fn deny_list_refuses_each_kind_and_admits_markdown_and_any_other_file() {
        let d = tmp();
        let w = |rel: &str, body: &[u8]| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(&p, body).expect("write");
            p
        };
        let ok = w("notes/a.md", b"# hi\n");
        assert_eq!(deny("notes/a.md", &ok, &ok), Ok(()));
        let txt = w("a.txt", b"x");
        assert_eq!(deny("a.txt", &txt, &txt), Ok(()));
        let png = w("img/logo.png", &[0x89, b'P', b'N', b'G', 0, 0xff]);
        assert_eq!(deny("img/logo.png", &png, &png), Ok(()));
        let v = w(".k2/vault/k.md", b"x");
        assert_eq!(deny(".k2/vault/k.md", &v, &v).expect_err("vault").code, "protected");
        let vb = w(".k2/vault/blob.bin", &[0, 1, 2]);
        assert_eq!(deny(".k2/vault/blob.bin", &vb, &vb).expect_err("vault blob").code, "protected");
        let g = w(".k2/AGENTS.md", b"x");
        assert_eq!(deny(".k2/AGENTS.md", &g, &g).expect_err("gen").code, "generated");
        let s = w(".ssh/notes.md", b"x");
        assert_eq!(deny(".ssh/notes.md", &s, &s).expect_err("secret").code, "secret");
        let key = w("keys/deploy.pem", b"x");
        assert_eq!(deny("keys/deploy.pem", &key, &key).expect_err("secret by name").code, "secret");
        let env = w(".env", b"API_KEY=sk-live-0123456789abcdef0123456789abcdef\n");
        assert_eq!(deny(".env", &env, &env).expect_err("dotenv").code, "secret");
        let hook = w(".git/hooks/pre-commit", b"#!/bin/sh\n");
        assert_eq!(deny(".git/hooks/pre-commit", &hook, &hook).expect_err("git hook").code, "protected");
        let cfg = w(".k2/heartbeats.json", b"{}");
        assert_eq!(deny(".k2/heartbeats.json", &cfg, &cfg).expect_err("k2 config").code, "protected");
        let att = w(".k2/wiki/assets/map.png", &[1, 2, 3]);
        assert_eq!(deny(".k2/wiki/assets/map.png", &att, &att), Ok(()));
        let prd = w(".k2/PROJECT.md", b"# p\n");
        assert_eq!(deny(".k2/PROJECT.md", &prd, &prd), Ok(()), "Markdown under .k2/ stays a doc");
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn markdown_first_then_text_by_family_then_any_file() {
        for p in ["a.md", "notes/b.markdown", "C.MD"] {
            assert_eq!(Tier::of(p), Tier::Markdown, "{p}");
        }
        use k2_mpc_text::Family as F;
        for (p, f) in [
            ("a.txt", F::Text),
            ("server.log", F::Text),
            ("README", F::Text),
            ("main.rs", F::Code),
            ("Makefile", F::Code),
            ("app.tsx", F::Code),
            ("config.yaml", F::Config),
            ("Cargo.toml", F::Config),
            ("page.html", F::Markup),
            ("flow.mmd", F::Diagram),
            ("data.csv", F::CsvAsText),
            ("data.tsv", F::CsvAsText),
        ] {
            assert_eq!(Tier::of(p), Tier::Text(f), "{p}");
            assert_eq!(Tier::of(p).number(), 1, "{p}");
        }
        for p in ["logo.png", "report.pdf", "deck.pptx", "song.mp3", "a.zip", "font.woff2"] {
            assert_eq!(Tier::of(p), Tier::Blob, "{p}");
        }
        assert_eq!(Tier::Blob.number(), 0);
        assert_eq!(Tier::of("data.csv").family(), "csv-as-text");
        assert!(Tier::of("a.txt").same_kind(Tier::of("b.toml")), "text stays text across families");
        assert!(!Tier::of("a.txt").same_kind(Tier::Markdown));
        assert!(!Tier::of("a.txt").same_kind(Tier::Blob));
    }

    #[test]
    fn a_tier_0_file_over_the_cap_is_declined_with_both_sizes() {
        let r = size_cap("big.bin", 40 << 20, 32 << 20).expect_err("over the cap");
        assert_eq!((r.status, r.code), ("422 Unprocessable Entity", "too_large"));
        assert!(r.message.contains("40.0 MiB") && r.message.contains("32.0 MiB"), "{}", r.message);
        assert_eq!(size_cap("big.bin", 32 << 20, 32 << 20), Ok(()), "the cap itself is allowed");
        assert!(size_cap("big.md", 40 << 20, 32 << 20).is_err(), "too big for its tier-0 fallback too");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 << 10), "3.0 KiB");
    }

    #[cfg(unix)]
    #[test]
    fn a_root_mirror_symlink_is_generated_but_a_real_claude_md_is_not() {
        let d = tmp();
        std::fs::create_dir_all(d.join(".k2")).expect("mkdir");
        std::fs::write(d.join(".k2/PROJECT.md"), "p").expect("write");
        std::os::unix::fs::symlink(d.join(".k2/PROJECT.md"), d.join("CLAUDE.md")).expect("symlink");
        let abs = d.join("CLAUDE.md").canonicalize().expect("canon");
        assert_eq!(deny(".k2/PROJECT.md", &d.join("CLAUDE.md"), &abs).expect_err("mirror").code, "generated");
        let own = d.join("GEMINI.md");
        std::fs::write(&own, "mine").expect("write");
        assert_eq!(deny("GEMINI.md", &own, &own), Ok(()));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }
}
