//! Inventory pass — resolve PROJECT + SLUG and enumerate workspace, Claude
//! memory/sessions, and multi-provider sessions into a structured
//! [`CloneInventory`].

use super::scrub::classify_secret;
use super::{CloneInventory, CloneOptions, DestinationClass, InventoryEntry};
use crate::chat_history::claude_project_hash;
use std::path::{Path, PathBuf};

/// Bulk directories never worth migrating — build outputs, dependency
/// caches, OS junk. Extends the spirit of `projects_ops`/`file_index`'s
/// skip-lists. Applied INSIDE nested git repos too (their `node_modules`,
/// etc.). NOTE: `.git` is intentionally NOT here — a nested repo's `.git`
/// rides along as a direct file copy so it arrives a functioning clone.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "out",
    ".next",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    // login/session state — pruned from the general walks, but enumerated
    // by the dedicated secrets pass so `carry_secrets` decides its fate.
    ".auth",
    "screenshots",
    "artifacts",
    "coverage",
    "__pycache__",
];

/// Bulk file globs / names dropped everywhere.
fn is_bulk_file(name: &str) -> bool {
    name == ".DS_Store"
        || name == "Thumbs.db"
        || name == "desktop.ini"
        || name.ends_with(".log")
}

/// Resolve PROJECT + SLUG, enumerate the workspace tree (excludes +
/// secret scrub), collect the memory dir, and collect the live session
/// `.jsonl`(s). See module + struct docs for the rules.
///
/// `project_path` is resolved to an absolute, canonical-ish path; SLUG is
/// computed from that resolved path so the on-disk `~/.claude/projects/
/// <slug>/` matches what Claude Code wrote.
pub fn inventory(project_path: &str, opts: CloneOptions) -> Result<CloneInventory, String> {
    let project = resolve_project_path(project_path)?;
    let project_str = project.to_string_lossy().to_string();
    let slug = claude_project_hash(&project_str);

    let mut entries: Vec<InventoryEntry> = Vec::new();
    let mut scrubbed: Vec<String> = Vec::new();

    // ── 1. Workspace tree ────────────────────────────────────────────
    collect_workspace(&project, &opts, &mut entries, &mut scrubbed)?;

    // ── 2 + 3. Memory + sessions (shared slug dir) ───────────────────
    let home = resolve_home(&opts)?;
    let projects_dir = home.join(".claude").join("projects");
    let slug_dir = projects_dir.join(&slug);

    collect_memory(&slug_dir, &mut entries);
    collect_sessions(&projects_dir, &slug, &opts, &mut entries)?;

    // ── 4. Multi-provider sessions (C1–C3) ───────────────────────────
    // Gemini / Pi / Codex / Grok / Cursor / Hermes under providers/.
    // Never enumerates credentials / auth / whole Hermes state.db.
    super::providers::collect_provider_sessions(&home, &project_str, &opts, &mut entries);

    scrubbed.sort();
    scrubbed.dedup();

    Ok(CloneInventory {
        project_path: project_str,
        slug,
        entries,
        scrubbed_secrets: scrubbed,
    })
}

/// Canonicalize the project path. Falls back to an absolutized
/// (non-canonical) form if the path doesn't yet exist on disk, so the
/// caller still gets a stable SLUG.
fn resolve_project_path(project_path: &str) -> Result<PathBuf, String> {
    let p = Path::new(project_path);
    if let Ok(canon) = std::fs::canonicalize(p) {
        return Ok(canon);
    }
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("cannot resolve cwd for relative project path: {e}"))?;
        Ok(cwd.join(p))
    }
}

/// Home dir: explicit override (tests) else `dirs::home_dir()`.
fn resolve_home(opts: &CloneOptions) -> Result<PathBuf, String> {
    if let Some(h) = &opts.home_override {
        return Ok(h.clone());
    }
    dirs::home_dir().ok_or_else(|| "cannot resolve home directory".to_string())
}

/// True only if `path` resolves (following symlinks) to a regular file the
/// bundler can `File::open` + stream into the tar. The walkers run with
/// `follow_links(false)`, so a symlink is reported with its OWN type — a
/// symlink that targets a directory is NOT a dir by `entry.file_type()` and
/// slips past the is_dir skips. `append_file` would then follow it into the
/// directory and crash with EISDIR. Resolving here excludes real dirs,
/// symlinked dirs, and broken/unreadable links; symlinks to real files still
/// resolve to `is_file()` and copy their bytes.
fn is_appendable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// The agent dot-dirs that must ALWAYS travel with a clone, gitignore or
/// not — a workspace's `.gitignore` very commonly lists `/.k2/`, which
/// used to silently drop the entire agent state from the bundle.
/// `.k2so` is the pre-0.40.4 legacy name; both live in the wild.
const AGENT_DOT_DIRS: &[&str] = &[".k2", ".k2so", ".claude"];

/// Walk the PROJECT tree in three passes:
///
/// 1. The main walk with the `ignore` crate (honors `.gitignore` /
///    `.ignore` / `.k2ignore` / `.k2soignore`), applying the bulk
///    skip-list (also inside nested git repos).
/// 2. A force-include walk over the agent dot-dirs (`.k2/`, `.k2so/`,
///    `.claude/`) with NO ignore rules — agent state travels even when
///    the workspace `.gitignore` lists it.
/// 3. A gitignore-proof secrets enumeration (`.env*` files + `.auth/`
///    subtrees) so `carry_secrets` truthfully decides their fate instead
///    of `.gitignore` silently dropping them first.
///
/// Every surviving file is secret-classified (unless `carry_secrets`);
/// passes dedupe on the workspace-relative path so a file reachable by
/// more than one pass is only processed once.
fn collect_workspace(
    project: &Path,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
    scrubbed: &mut Vec<String>,
) -> Result<(), String> {
    if !project.is_dir() {
        return Err(format!(
            "project path is not a directory: {}",
            project.display()
        ));
    }

    // Dedupe set across the three passes: every workspace-relative path
    // that has been PROCESSED (whether bundled or scrubbed).
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // ── Pass 1: ignore-honoring main walk ────────────────────────────
    let walker = ignore::WalkBuilder::new(project)
        .hidden(false) // we WANT .k2/.claude/.git etc.
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .follow_links(false)
        .require_git(false)
        // `.k2ignore` (current) + `.k2soignore` (legacy) as custom ignore
        // file names (same syntax as .gitignore); `ignore` reads `.ignore`
        // by default but not these.
        .add_custom_ignore_filename(".k2ignore")
        .add_custom_ignore_filename(".k2soignore")
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            // Only directory components are bulk-skipped wholesale.
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                let name = entry.file_name().to_string_lossy();
                return !SKIP_DIRS.iter().any(|&s| s.eq_ignore_ascii_case(&name));
            }
            true
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            continue; // entries are files; dirs are recreated implicitly.
        }
        process_workspace_file(project, entry.path(), opts, &mut seen, entries, scrubbed);
    }

    // ── Pass 2: force-include the agent dot-dirs ──────────────────────
    // The main walk honors `.gitignore`, which very often lists `/.k2/`
    // (and sometimes `.claude/`) — that used to silently drop the whole
    // agent dir from the bundle. Re-walk each dot-dir with NO ignore
    // rules; `seen` dedupes anything the main walk already took. The
    // secret content-scan still applies (a credential-bearing file inside
    // `.k2/` is scrubbed unless carrying).
    for dot in AGENT_DOT_DIRS {
        let dir = project.join(dot);
        if !dir.is_dir() {
            continue;
        }
        let walker = ignore::WalkBuilder::new(&dir)
            .hidden(false)
            .standard_filters(false) // agent state travels; no ignore rules.
            .follow_links(false)
            .filter_entry(|entry| {
                if entry.depth() == 0 {
                    return true;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    let name = entry.file_name().to_string_lossy();
                    return !SKIP_DIRS.iter().any(|&s| s.eq_ignore_ascii_case(&name));
                }
                true
            })
            .build();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.depth() == 0 {
                continue;
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            process_workspace_file(project, entry.path(), opts, &mut seen, entries, scrubbed);
        }
    }

    // ── Pass 3: gitignore-proof secrets enumeration ───────────────────
    collect_secret_candidates(project, opts, &mut seen, entries, scrubbed);

    Ok(())
}

/// Shared per-file processing for every `collect_workspace` pass: symlink
/// guard, bulk-file drop, relativize, DEDUPE (first pass to reach a rel
/// path wins), secret classification, push.
fn process_workspace_file(
    project: &Path,
    path: &Path,
    opts: &CloneOptions,
    seen: &mut std::collections::HashSet<String>,
    entries: &mut Vec<InventoryEntry>,
    scrubbed: &mut Vec<String>,
) {
    // A symlink reports its OWN type under follow_links(false), so a
    // symlink pointing at a directory (e.g.
    // `.k2so/external/agent-skills/.opencode/skills`) escapes the walkers'
    // is_dir skips. append_file's `File::open` would then follow it into
    // the directory and crash with EISDIR ("Is a directory"). Resolve the
    // target and skip anything that isn't a real file — symlinked dirs,
    // broken/unreadable links. Symlinks to real files still resolve to a
    // file here and copy their bytes.
    if !is_appendable_file(path) {
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if is_bulk_file(&name) {
        return;
    }
    let rel = match path.strip_prefix(project) {
        Ok(r) => r.to_string_lossy().to_string(),
        Err(_) => return,
    };
    // Dedupe across passes: a file the main walk already took (or already
    // scrubbed) must not be re-processed by the force-include/secret walks.
    if !seen.insert(rel.clone()) {
        return;
    }

    // Secret classification (unless carrying secrets).
    if !opts.carry_secrets {
        if let Some(_reason) = classify_secret(&rel, path) {
            scrubbed.push(rel);
            return;
        }
    }

    entries.push(InventoryEntry {
        abs_path: path.to_path_buf(),
        rel_path: rel,
        class: DestinationClass::Workspace,
    });
}

/// Enumerate the secret-shaped files the main walk may have missed —
/// `.env` / `.env.*` files at any depth plus everything under an `.auth/`
/// directory — with NO ignore rules, so a workspace `.gitignore` listing
/// `.env*` can't hide them from the `carry_secrets` decision. The
/// existing semantics then apply via [`process_workspace_file`]:
/// `carry_secrets = false` → scrubbed + listed in `scrubbed_secrets`;
/// `carry_secrets = true` → included in the bundle.
///
/// The bulk skip-list still prunes descent (an `.env` inside
/// `node_modules/` is not workspace state) — except `.auth` itself, which
/// is this pass's target.
fn collect_secret_candidates(
    project: &Path,
    opts: &CloneOptions,
    seen: &mut std::collections::HashSet<String>,
    entries: &mut Vec<InventoryEntry>,
    scrubbed: &mut Vec<String>,
) {
    let walker = ignore::WalkBuilder::new(project)
        .hidden(false)
        .standard_filters(false) // no gitignore: secrets are decided by opts.
        .follow_links(false)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                let name = entry.file_name().to_string_lossy();
                // Descend into `.auth` (the target of this pass) but keep
                // the rest of the bulk skip-list pruning the walk.
                return name == ".auth"
                    || !SKIP_DIRS.iter().any(|&s| s.eq_ignore_ascii_case(&name));
            }
            true
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy();
        let is_env = name == ".env" || name.starts_with(".env.");
        let under_auth = path
            .strip_prefix(project)
            .map(|rel| {
                let mut comps: Vec<_> = rel.components().collect();
                comps.pop(); // exclude the filename; `.auth` is a DIR test.
                comps.iter().any(|c| c.as_os_str() == ".auth")
            })
            .unwrap_or(false);
        if !is_env && !under_auth {
            continue; // this pass only targets secret-shaped paths.
        }
        process_workspace_file(project, path, opts, seen, entries, scrubbed);
    }
}

/// Collect the ENTIRE `<slug>/memory/` directory (`MEMORY.md` +
/// every `*.md`, recursively). Missing dir → no memory entries.
fn collect_memory(slug_dir: &Path, entries: &mut Vec<InventoryEntry>) {
    let memory_dir = slug_dir.join("memory");
    if !memory_dir.is_dir() {
        return;
    }
    let walker = ignore::WalkBuilder::new(&memory_dir)
        .hidden(false)
        .standard_filters(false) // memory is curated; copy it whole.
        .follow_links(false)
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        // Same symlink-to-dir guard as collect_workspace: a symlink reports
        // its own type under follow_links(false), so skip anything that
        // doesn't resolve to a real file (else append_file hits EISDIR).
        if !is_appendable_file(path) {
            continue;
        }
        // rel is relative to the memory dir → re-rooted under `memory/`
        // in the bundle.
        if let Ok(rel) = path.strip_prefix(&memory_dir) {
            entries.push(InventoryEntry {
                abs_path: path.to_path_buf(),
                rel_path: rel.to_string_lossy().to_string(),
                class: DestinationClass::Memory,
            });
        }
    }
}

/// Collect session `.jsonl`(s). Default: the newest-mtime live session in
/// the slug dir. With `include_all_history`: every `*.jsonl` in the slug
/// dir AND in each `<slug>-<branch>/` worktree variant dir.
///
/// `rel_path` for a session is relative to `projects_dir` (i.e. it
/// includes the `<slug>/` or `<slug>-<branch>/` prefix) so the remote can
/// place it under its recomputed slug while preserving the branch suffix.
fn collect_sessions(
    projects_dir: &Path,
    slug: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) -> Result<(), String> {
    if !projects_dir.is_dir() {
        return Ok(());
    }

    // Candidate dirs: the slug dir + any `<slug>-<branch>/` variant.
    let mut session_dirs: Vec<PathBuf> = Vec::new();
    let slug_dir = projects_dir.join(slug);
    if slug_dir.is_dir() {
        session_dirs.push(slug_dir);
    }
    if opts.include_all_history {
        if let Ok(rd) = std::fs::read_dir(projects_dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{slug}-")) && ent.path().is_dir() {
                    session_dirs.push(ent.path());
                }
            }
        }
    }

    // (abs_path, rel_to_projects_dir, mtime)
    let mut found: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    for dir in &session_dirs {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let mtime = ent
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let rel = match path.strip_prefix(projects_dir) {
                Ok(r) => r.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            found.push((path, rel, mtime));
        }
    }

    if found.is_empty() {
        return Ok(());
    }

    if opts.include_all_history {
        for (abs, rel, _) in found {
            entries.push(InventoryEntry {
                abs_path: abs,
                rel_path: rel,
                class: DestinationClass::Session,
            });
        }
    } else {
        // Live = newest mtime across the slug dir.
        found.sort_by(|a, b| b.2.cmp(&a.2));
        let (abs, rel, _) = found.into_iter().next().unwrap();
        entries.push(InventoryEntry {
            abs_path: abs,
            rel_path: rel,
            class: DestinationClass::Session,
        });
    }

    Ok(())
}
