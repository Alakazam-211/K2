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
//! **Never live docs** (MI12), refused before any open:
//! - anything that isn't `.md` / `.markdown` (`.mdx` is JSX), `not_markdown`;
//! - secret files by path or content (`secret_paths::classify`), `secret`;
//! - `.k2/vault/**`, `protected`;
//! - files K2 generates and rewrites (AGENTS.md, its own skills, rosters,
//!   the harness mirrors), `generated`. An edit there would be overwritten
//!   by the next recompose.

use std::path::{Path, PathBuf};

use k2_multiplayer_base::store::DocKey;

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

/// The deny list for an existing, jailed file (`rel` workspace-relative,
/// `link` the path before symlinks were resolved, `abs` after).
pub fn deny(rel: &str, link: &Path, abs: &Path) -> Result<(), Refusal> {
    if !k2_multiplayer_markdown::is_markdown_path(rel) {
        return Err(Refusal::declined("not_markdown", format!("{rel} is not a Markdown file (.md, .markdown)")));
    }
    if is_vault(rel) {
        return Err(Refusal::declined("protected", format!("{rel} is in the vault folder; it is never a live doc")));
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
fn workspace_holding(abs: &Path) -> Result<(String, PathBuf), Refusal> {
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
    Ok(Target { project_id, root, rel, abs })
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
    fn deny_list_refuses_each_kind_and_admits_plain_markdown() {
        let d = tmp();
        let w = |rel: &str, body: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(&p, body).expect("write");
            p
        };
        let ok = w("notes/a.md", "# hi\n");
        assert_eq!(deny("notes/a.md", &ok, &ok), Ok(()));
        let txt = w("a.txt", "x");
        assert_eq!(deny("a.txt", &txt, &txt).expect_err("txt").code, "not_markdown");
        let mdx = w("a.mdx", "x");
        assert_eq!(deny("a.mdx", &mdx, &mdx).expect_err("mdx").code, "not_markdown");
        let v = w(".k2/vault/k.md", "x");
        assert_eq!(deny(".k2/vault/k.md", &v, &v).expect_err("vault").code, "protected");
        let g = w(".k2/AGENTS.md", "x");
        assert_eq!(deny(".k2/AGENTS.md", &g, &g).expect_err("gen").code, "generated");
        let s = w(".ssh/notes.md", "x");
        assert_eq!(deny(".ssh/notes.md", &s, &s).expect_err("secret").code, "secret");
        std::fs::remove_dir_all(&d).expect("cleanup");
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
