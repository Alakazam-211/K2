//! Phase 2.1 — workspace inbox primitive (`.k2so/inbox/`).
//!
//! Replaces the pre-Phase-2.1 `.k2so/work/{inbox,active,done}/` layout.
//! A workspace's inbox is its email-like channel: items arrive from
//! other workspaces (via `msg --inbox`) or are composed by the
//! workspace's own agent (via `inbox compose`). The agent organizes
//! triage by filing items into folders it creates on demand — there's
//! no system-imposed taxonomy.
//!
//! Standard folders (sentinel names that always work):
//!
//! - `<root>/`              → top-level new arrivals (untriaged)
//! - `<root>/active/`       → items the agent is currently working on
//! - `<root>/done/`         → items the agent has archived
//!
//! Custom folders (agent-created via `inbox move <id> <folder>`) live
//! at `<root>/<folder>/<id>.md`. The folder is created on first use.
//!
//! Every public function in this module is **pure with respect to the
//! `root` argument** — pass a sandboxed `.k2so/inbox/` for tests, the
//! real one in prod. No globals; no env-var resolution.
//!
//! ## Migration
//!
//! [`migrate_work_to_inbox`] is the one-shot helper that converts a
//! pre-Phase-2.1 workspace from `.k2so/work/{inbox,active,done}/` to
//! the new layout. Idempotent via a marker file. Daemon first-boot
//! invokes per-workspace; tests call directly.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::agent_identity::parse_frontmatter;
use crate::workspace::work_item::{atomic_write, safe_read_to_string};

/// One inbox item — markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub id: String,        // filename stem (no .md extension)
    pub filename: String,  // full filename including .md
    pub folder: String,    // "" for top-level, otherwise folder name
    pub title: String,
    pub priority: String,
    pub created: String,
    pub source: String,
    pub from: String,      // sender identity ("user", "cli", workspace name)
    pub body_preview: String,
}

/// Path to `<workspace>/.k2so/inbox/`. Doesn't create.
pub fn inbox_root(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(&workspace).join("inbox")
}

/// Path to a folder under the inbox root (e.g. "active" → `.k2so/inbox/active/`).
/// Empty string returns the inbox root itself.
pub fn folder_path(workspace: &Path, folder: &str) -> PathBuf {
    if folder.is_empty() {
        inbox_root(workspace)
    } else {
        inbox_root(workspace).join(folder)
    }
}

/// Marker file written by [`migrate_work_to_inbox`] after success.
fn migration_marker(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(&workspace).join(".work-to-inbox-migration-v1-done")
}

/// Slugify a title into a safe filename stem (lowercase, hyphenated,
/// alphanum + dash only).
pub fn slug_for_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_dash = true;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parse a `.md` file into an [`InboxItem`]. Returns `None` if the
/// file can't be read.
pub fn read_item(path: &Path, folder: &str) -> Option<InboxItem> {
    let content = safe_read_to_string(path).ok()?;
    let filename = path.file_name()?.to_string_lossy().to_string();
    let id = filename.strip_suffix(".md").unwrap_or(&filename).to_string();
    let fm = parse_frontmatter(&content);

    let body_preview = body_preview_for(&content);

    Some(InboxItem {
        id,
        filename,
        folder: folder.to_string(),
        title: fm.get("title").cloned().unwrap_or_default(),
        priority: fm.get("priority").cloned().unwrap_or_else(|| "normal".to_string()),
        created: fm.get("created").cloned().unwrap_or_default(),
        source: fm.get("source").cloned().unwrap_or_else(|| "manual".to_string()),
        from: fm
            .get("from")
            .or_else(|| fm.get("assigned_by"))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        body_preview,
    })
}

fn body_preview_for(content: &str) -> String {
    let body = if content.starts_with("---") {
        content[3..].find("---").map(|end| &content[3 + end + 3..]).unwrap_or("").trim()
    } else {
        content.trim()
    };
    let preview: String = body.chars().take(120).collect();
    if body.len() > 120 { format!("{}...", preview.trim()) } else { preview.trim().to_string() }
}

/// List items at the inbox root (top-level new arrivals).
pub fn list_root(workspace: &Path) -> Vec<InboxItem> { list_folder(workspace, "") }

/// List items in a folder. Empty folder string returns top-level.
/// Only direct children (non-recursive); ignores sub-directories.
pub fn list_folder(workspace: &Path, folder: &str) -> Vec<InboxItem> {
    let dir = folder_path(workspace, folder);
    if !dir.exists() {
        return Vec::new();
    }
    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map_or(false, |e| e == "md") {
                if let Some(item) = read_item(&path, folder) {
                    items.push(item);
                }
            }
        }
    }
    items
}

/// List folders the workspace has created under the inbox root.
/// Returns names only (e.g. "projects", "active", "done").
pub fn list_folders(workspace: &Path) -> Vec<String> {
    let root = inbox_root(workspace);
    if !root.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') {
                        out.push(name.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Read full text of one item by id (filename stem). Searches the
/// root first, then every folder. Returns the file contents on hit.
pub fn read_by_id(workspace: &Path, id: &str) -> Result<String, String> {
    let target = format!("{}.md", id);
    let root = inbox_root(workspace);
    let root_path = root.join(&target);
    if root_path.exists() {
        return safe_read_to_string(&root_path);
    }
    for folder in list_folders(workspace) {
        let p = root.join(&folder).join(&target);
        if p.exists() {
            return safe_read_to_string(&p);
        }
    }
    Err(format!("inbox item not found: {id}"))
}

/// Locate the on-disk path of an item by id. Returns `(folder, path)`
/// where folder is "" for root-level items.
pub fn locate_item(workspace: &Path, id: &str) -> Result<(String, PathBuf), String> {
    let target = format!("{}.md", id);
    let root = inbox_root(workspace);
    let root_path = root.join(&target);
    if root_path.exists() {
        return Ok((String::new(), root_path));
    }
    for folder in list_folders(workspace) {
        let p = root.join(&folder).join(&target);
        if p.exists() {
            return Ok((folder, p));
        }
    }
    Err(format!("inbox item not found: {id}"))
}

/// Compose a new item at the inbox root. Returns the created
/// [`InboxItem`]. Filename is derived from the title via [`slug_for_title`]
/// with a numeric suffix if a collision exists.
pub fn compose(
    workspace: &Path,
    title: &str,
    body: &str,
    priority: Option<&str>,
    source: Option<&str>,
    from: Option<&str>,
) -> Result<InboxItem, String> {
    if title.trim().is_empty() {
        return Err("title is required".to_string());
    }
    let root = inbox_root(workspace);
    fs::create_dir_all(&root).map_err(|e| format!("create inbox dir: {e}"))?;

    let stem = slug_for_title(title);
    let mut filename = format!("{}.md", stem);
    let mut suffix = 2;
    while root.join(&filename).exists() {
        filename = format!("{}-{}.md", stem, suffix);
        suffix += 1;
    }
    let priority = priority.unwrap_or("normal").to_string();
    let source = source.unwrap_or("manual").to_string();
    let from = from.unwrap_or("self").to_string();
    let created = simple_date_now();

    let content = format!(
        "---\ntitle: {title}\npriority: {priority}\ncreated: {created}\nsource: {source}\nfrom: {from}\n---\n\n{body}\n"
    );
    let path = root.join(&filename);
    atomic_write(&path, &content)?;

    let id = filename.strip_suffix(".md").unwrap_or(&filename).to_string();
    Ok(InboxItem {
        id,
        filename,
        folder: String::new(),
        title: title.to_string(),
        priority,
        created,
        source,
        from,
        body_preview: body_preview_for(&content),
    })
}

/// Move an item by id into the target folder. Creates the folder if
/// it doesn't exist. `to_folder` of "" moves back to the inbox root.
pub fn move_item(workspace: &Path, id: &str, to_folder: &str) -> Result<PathBuf, String> {
    let (current_folder, src) = locate_item(workspace, id)?;
    if current_folder == to_folder {
        return Ok(src);
    }
    let dst_dir = folder_path(workspace, to_folder);
    fs::create_dir_all(&dst_dir).map_err(|e| format!("create folder {to_folder}: {e}"))?;
    let dst = dst_dir.join(format!("{}.md", id));
    fs::rename(&src, &dst).map_err(|e| format!("move {id} → {to_folder}: {e}"))?;
    Ok(dst)
}

/// Move an item to the standard `done/` folder (archive convention).
pub fn archive(workspace: &Path, id: &str) -> Result<PathBuf, String> {
    move_item(workspace, id, "done")
}

/// Send an item to the macOS Recycle Bin. Recoverable from Trash.
///
/// SAFETY: routes through `scratch_safe_trash` so test scratch paths
/// under std::env::temp_dir() skip the trash crate (avoids macOS
/// Touch ID prompts). Production paths still go to recycle bin.
pub fn delete(workspace: &Path, id: &str) -> Result<(), String> {
    let (_, src) = locate_item(workspace, id)?;
    crate::safe_delete_scratch::scratch_safe_trash(&src)
}

/// Append a `respond` payload to the original item's file as a quoted
/// reply block. Returns the path of the file modified. Email-style
/// reply (no fan-out yet; sender-side cross-workspace reply lands in
/// follow-up work).
pub fn respond(workspace: &Path, id: &str, text: &str) -> Result<PathBuf, String> {
    let (_, path) = locate_item(workspace, id)?;
    let existing = safe_read_to_string(&path)?;
    let ts = simple_date_now();
    let appended = format!(
        "{existing}\n\n---\n## Reply ({ts})\n\n{text}\n"
    );
    atomic_write(&path, &appended)?;
    Ok(path)
}

/// Simple plain-text search across inbox + folders. Substring match
/// on filename + title + body content (case-insensitive).
pub fn search(workspace: &Path, query: &str) -> Vec<InboxItem> {
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = list_root(workspace)
        .into_iter()
        .filter(|item| item_matches(workspace, item, &needle))
        .collect::<Vec<_>>();
    for folder in list_folders(workspace) {
        let folder_items = list_folder(workspace, &folder);
        for item in folder_items {
            if item_matches(workspace, &item, &needle) {
                out.push(item);
            }
        }
    }
    out
}

fn item_matches(workspace: &Path, item: &InboxItem, needle_lower: &str) -> bool {
    if item.title.to_lowercase().contains(needle_lower) {
        return true;
    }
    if item.filename.to_lowercase().contains(needle_lower) {
        return true;
    }
    // Body check: read full file (size-bounded by safe_read_to_string).
    let folder = if item.folder.is_empty() { String::new() } else { item.folder.clone() };
    let path = folder_path(workspace, &folder).join(&item.filename);
    if let Ok(content) = safe_read_to_string(&path) {
        if content.to_lowercase().contains(needle_lower) {
            return true;
        }
    }
    false
}

fn simple_date_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ── Migration ──────────────────────────────────────────────────────────

/// Outcome of [`migrate_work_to_inbox`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub already_migrated: bool,
    pub moved_top_level: usize,
    pub moved_active: usize,
    pub moved_done: usize,
    pub trashed_work_root: bool,
    pub errors: Vec<String>,
}

/// One-shot migration: `.k2so/work/{inbox,active,done}/*.md` →
/// `.k2so/inbox/{,active,done}/*.md`. After all `.md` files are
/// moved, sends the (now-empty-or-not) `.k2so/work/` folder to the
/// macOS Recycle Bin via `safe_delete::trash`.
///
/// Idempotent: marker file `.k2so/.work-to-inbox-migration-v1-done`
/// short-circuits subsequent calls. Safe to invoke on every daemon
/// boot.
///
/// Atomic rename per-file (same filesystem; no copy + delete races).
/// Per-file failures are recorded in `errors` but don't abort the
/// rest — bias toward making as much progress as possible.
pub fn migrate_work_to_inbox(workspace: &Path) -> MigrationReport {
    let marker = migration_marker(workspace);
    if marker.exists() {
        return MigrationReport {
            already_migrated: true,
            moved_top_level: 0,
            moved_active: 0,
            moved_done: 0,
            trashed_work_root: false,
            errors: Vec::new(),
        };
    }
    let work_root = crate::workspace_dot_dir(&workspace).join("work");
    if !work_root.exists() {
        // Nothing to migrate — write marker so we don't keep checking.
        let _ = fs::create_dir_all(crate::workspace_dot_dir(&workspace));
        let _ = fs::write(&marker, "v1");
        return MigrationReport {
            already_migrated: true,
            moved_top_level: 0,
            moved_active: 0,
            moved_done: 0,
            trashed_work_root: false,
            errors: Vec::new(),
        };
    }

    let mut report = MigrationReport {
        already_migrated: false,
        moved_top_level: 0,
        moved_active: 0,
        moved_done: 0,
        trashed_work_root: false,
        errors: Vec::new(),
    };

    let new_root = inbox_root(workspace);
    if let Err(e) = fs::create_dir_all(&new_root) {
        report.errors.push(format!("create new inbox root: {e}"));
        return report;
    }

    // .k2so/work/inbox/* → .k2so/inbox/*
    report.moved_top_level = move_md_files(&work_root.join("inbox"), &new_root, &mut report.errors);
    // .k2so/work/active/* → .k2so/inbox/active/*
    report.moved_active = move_md_files(
        &work_root.join("active"),
        &new_root.join("active"),
        &mut report.errors,
    );
    // .k2so/work/done/* → .k2so/inbox/done/*
    report.moved_done = move_md_files(
        &work_root.join("done"),
        &new_root.join("done"),
        &mut report.errors,
    );

    // Send the entire .k2so/work/ to the macOS Recycle Bin (recoverable
    // for ~30 days). Per PRD A24.4: if there was unexpected user content
    // beyond the standard layout, the user can recover from Trash.
    //
    // SAFETY: routes through `scratch_safe_trash` so test scratch paths
    // under std::env::temp_dir() bypass the trash crate (avoids macOS
    // Touch ID prompts during cargo test + bash-CLI sandbox runs).
    // Production workspace paths still go through `safe_delete::trash`.
    match crate::safe_delete_scratch::scratch_safe_trash(&work_root) {
        Ok(()) => report.trashed_work_root = true,
        Err(e) => report.errors.push(format!("trash {}: {}", work_root.display(), e)),
    }

    // Marker so we don't re-scan on every boot. Write only if zero
    // hard errors; partial-success still gets a marker (we did our
    // best, and the user has Trash for recovery).
    if let Err(e) = fs::write(&marker, "v1") {
        report.errors.push(format!("write migration marker: {e}"));
    }
    report
}

fn move_md_files(src_dir: &Path, dst_dir: &Path, errors: &mut Vec<String>) -> usize {
    if !src_dir.exists() {
        return 0;
    }
    if let Err(e) = fs::create_dir_all(dst_dir) {
        errors.push(format!("create {}: {}", dst_dir.display(), e));
        return 0;
    }
    let mut moved = 0;
    let entries = match fs::read_dir(src_dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("read_dir {}: {}", src_dir.display(), e));
            return 0;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !path.extension().map_or(false, |e| e == "md") {
            continue;
        }
        let filename = match path.file_name() {
            Some(f) => f.to_owned(),
            None => continue,
        };
        let dst = dst_dir.join(&filename);
        if dst.exists() {
            // Don't clobber. Suffix with .legacy until free.
            let mut alt = dst.clone();
            let mut suffix = 2;
            while alt.exists() {
                let stem = filename.to_string_lossy();
                let s = stem.strip_suffix(".md").unwrap_or(&stem);
                alt = dst_dir.join(format!("{}-legacy-{}.md", s, suffix));
                suffix += 1;
            }
            match fs::rename(&path, &alt) {
                Ok(()) => moved += 1,
                Err(e) => errors.push(format!("rename {} → {}: {}", path.display(), alt.display(), e)),
            }
        } else {
            match fs::rename(&path, &dst) {
                Ok(()) => moved += 1,
                Err(e) => errors.push(format!("rename {} → {}: {}", path.display(), dst.display(), e)),
            }
        }
    }
    moved
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Lightweight scratch workspace. Cleaned up on Drop.
    struct ScratchWs {
        path: PathBuf,
    }
    impl ScratchWs {
        fn new() -> Self {
            let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join(format!("k2so-inbox-test-{}-{}-{}", pid, nanos, n));
            fs::create_dir_all(dir.join(".k2so")).unwrap();
            ScratchWs { path: dir }
        }
        fn path(&self) -> &Path { &self.path }
    }
    impl Drop for ScratchWs {
        fn drop(&mut self) {
            // Best-effort cleanup; ignore errors (other tests / CI may clean later).
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn make_ws() -> ScratchWs {
        ScratchWs::new()
    }

    #[test]
    fn slug_handles_punctuation_and_unicode() {
        assert_eq!(slug_for_title("Audit OAuth!"), "audit-oauth");
        assert_eq!(slug_for_title("multiple   spaces"), "multiple-spaces");
        assert_eq!(slug_for_title("!!!"), "untitled");
        assert_eq!(slug_for_title("Hello, world."), "hello-world");
    }

    #[test]
    fn compose_writes_file_with_frontmatter() {
        let ws = make_ws();
        let item = compose(ws.path(), "Audit auth", "Body text.", None, None, None).unwrap();
        assert_eq!(item.title, "Audit auth");
        assert_eq!(item.folder, "");
        assert_eq!(item.priority, "normal");

        let path = inbox_root(ws.path()).join(&item.filename);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("title: Audit auth"));
        assert!(content.contains("Body text."));
    }

    #[test]
    fn compose_renames_on_collision() {
        let ws = make_ws();
        let a = compose(ws.path(), "Same", "x", None, None, None).unwrap();
        let b = compose(ws.path(), "Same", "y", None, None, None).unwrap();
        assert_ne!(a.filename, b.filename);
        assert_eq!(a.filename, "same.md");
        assert_eq!(b.filename, "same-2.md");
    }

    #[test]
    fn list_root_shows_top_level_only() {
        let ws = make_ws();
        compose(ws.path(), "Top one", "", None, None, None).unwrap();
        compose(ws.path(), "Top two", "", None, None, None).unwrap();
        let items = list_root(ws.path());
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.folder.is_empty()));
    }

    #[test]
    fn move_creates_folder_on_first_use() {
        let ws = make_ws();
        let item = compose(ws.path(), "X", "", None, None, None).unwrap();
        move_item(ws.path(), &item.id, "projects").unwrap();
        let projects_dir = inbox_root(ws.path()).join("projects");
        assert!(projects_dir.exists());
        assert!(projects_dir.join(&item.filename).exists());

        let folders = list_folders(ws.path());
        assert!(folders.contains(&"projects".to_string()));

        let listed = list_folder(ws.path(), "projects");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].folder, "projects");
    }

    #[test]
    fn archive_moves_to_done() {
        let ws = make_ws();
        let item = compose(ws.path(), "Y", "", None, None, None).unwrap();
        archive(ws.path(), &item.id).unwrap();
        let done = inbox_root(ws.path()).join("done");
        assert!(done.exists());
        assert!(done.join(&item.filename).exists());
    }

    #[test]
    fn search_matches_title_filename_body() {
        let ws = make_ws();
        compose(ws.path(), "oauth migration", "details", None, None, None).unwrap();
        compose(ws.path(), "unrelated", "OAuth in body", None, None, None).unwrap();
        compose(ws.path(), "nope", "totally other", None, None, None).unwrap();
        let hits = search(ws.path(), "oauth");
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn respond_appends_quoted_reply() {
        let ws = make_ws();
        let item = compose(ws.path(), "Z", "original body", None, None, None).unwrap();
        respond(ws.path(), &item.id, "thanks").unwrap();
        let content = fs::read_to_string(inbox_root(ws.path()).join(&item.filename)).unwrap();
        assert!(content.contains("## Reply"));
        assert!(content.contains("thanks"));
    }

    #[test]
    fn migrate_idempotent_marker() {
        let ws = make_ws();
        // First run: no .k2so/work/ — writes marker, reports already_migrated.
        let r1 = migrate_work_to_inbox(ws.path());
        assert!(r1.already_migrated);
        // Second run hits the marker fast-path.
        let r2 = migrate_work_to_inbox(ws.path());
        assert!(r2.already_migrated);
    }

    #[test]
    fn migrate_moves_files_layout() {
        let ws = make_ws();
        let work = ws.path().join(".k2so").join("work");
        fs::create_dir_all(work.join("inbox")).unwrap();
        fs::create_dir_all(work.join("active")).unwrap();
        fs::create_dir_all(work.join("done")).unwrap();
        fs::write(work.join("inbox").join("a.md"), "---\ntitle: A\n---\n").unwrap();
        fs::write(work.join("inbox").join("b.md"), "---\ntitle: B\n---\n").unwrap();
        fs::write(work.join("active").join("c.md"), "---\ntitle: C\n---\n").unwrap();
        fs::write(work.join("done").join("d.md"), "---\ntitle: D\n---\n").unwrap();

        let report = migrate_work_to_inbox(ws.path());
        assert!(!report.already_migrated);
        assert_eq!(report.moved_top_level, 2);
        assert_eq!(report.moved_active, 1);
        assert_eq!(report.moved_done, 1);

        let new_root = inbox_root(ws.path());
        assert!(new_root.join("a.md").exists());
        assert!(new_root.join("b.md").exists());
        assert!(new_root.join("active").join("c.md").exists());
        assert!(new_root.join("done").join("d.md").exists());

        // Marker written.
        assert!(migration_marker(ws.path()).exists());

        // Second run no-ops.
        let report2 = migrate_work_to_inbox(ws.path());
        assert!(report2.already_migrated);
        assert_eq!(report2.moved_top_level, 0);
    }
}
