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

    let filename = allocate_md_filename(&root, title);
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

// ── File package delivery (`k2 msg --inbox-silent|wake <path>`) ────────

/// Outcome of [`deliver_file`] — durable package under `.k2/inbox/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveredPackage {
    pub id: String,
    pub title: String,
    pub filename: String,
    pub folder: String,
    /// Absolute path of the listable cover / body `.md`.
    pub cover_path: String,
    /// Absolute paths of any sidecar binaries under `<id>.files/`.
    pub sidecar_paths: Vec<String>,
    pub body_preview: String,
    pub source: String,
    pub from: String,
}

/// Deliver a local file into the target workspace's inbox as a durable
/// package. Markdown sources become normalized `.md` items; other files
/// become a cover note + sidecar under `<id>.files/<safe_name>`.
///
/// Title resolution (first hit wins):
/// 1. `title_override`
/// 2. YAML frontmatter `title:` (when source is `.md`)
/// 3. First `# heading` in the `.md` body
/// 4. Filename stem
///
/// `source` defaults to `"msg-inbox"`. `from` defaults to `"self"`.
pub fn deliver_file(
    workspace: &Path,
    source_path: &Path,
    title_override: Option<&str>,
    from: Option<&str>,
    source: Option<&str>,
) -> Result<DeliveredPackage, String> {
    if !source_path.is_file() {
        return Err(format!(
            "source path is not a readable file: {}",
            source_path.display()
        ));
    }

    let source_tag = source.unwrap_or("msg-inbox").to_string();
    let from_tag = from.unwrap_or("self").to_string();
    let is_md = source_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("md"))
        .unwrap_or(false);

    let root = inbox_root(workspace);
    fs::create_dir_all(&root).map_err(|e| format!("create inbox dir: {e}"))?;

    if is_md {
        deliver_markdown_file(
            &root,
            source_path,
            title_override,
            &from_tag,
            &source_tag,
        )
    } else {
        deliver_binary_file(
            &root,
            source_path,
            title_override,
            &from_tag,
            &source_tag,
        )
    }
}

/// Build the short live-wake pointer payload (without `[from …]` framing —
/// `deliver_live` / `format_message` adds that).
pub fn wake_pointer_text(id: &str, title: &str) -> String {
    wake_pointer_text_n(id, title, None)
}

/// Wake pointer with optional multi-file note (`N files` when `file_count > 1`).
pub fn wake_pointer_text_n(id: &str, title: &str, file_count: Option<usize>) -> String {
    match file_count {
        Some(n) if n > 1 => {
            format!("[inbox:{id}] {title}\nOpen: k2 inbox read {id}\n({n} files)")
        }
        _ => format!("[inbox:{id}] {title}\nOpen: k2 inbox read {id}"),
    }
}

/// Deliver one or more local files into the target workspace's inbox as a
/// single tray package (one id, one cover `.md`, all files as sidecars under
/// `<id>.files/`).
///
/// Rules:
/// - 0 paths → error
/// - 1 path → delegates to [`deliver_file`] (preserves single-file shape)
/// - N paths → one multi-file package
///
/// Title (multi): override, else if exactly one source is `.md` use that
/// file's title resolution, else `"N files: name1, name2, …"` (truncated).
pub fn deliver_files(
    workspace: &Path,
    source_paths: &[PathBuf],
    title_override: Option<&str>,
    from: Option<&str>,
    source: Option<&str>,
) -> Result<DeliveredPackage, String> {
    if source_paths.is_empty() {
        return Err("no source paths — pass at least one file".to_string());
    }
    if source_paths.len() == 1 {
        return deliver_file(
            workspace,
            &source_paths[0],
            title_override,
            from,
            source,
        );
    }

    // Validate every path is a readable file before allocating a package id.
    for p in source_paths {
        if !p.is_file() {
            return Err(format!(
                "source path is not a readable file: {}",
                p.display()
            ));
        }
    }

    let source_tag = source.unwrap_or("msg-inbox").to_string();
    let from_tag = from.unwrap_or("self").to_string();
    let root = inbox_root(workspace);
    fs::create_dir_all(&root).map_err(|e| format!("create inbox dir: {e}"))?;

    let title = resolve_multi_title(title_override, source_paths);
    let filename = allocate_md_filename(&root, &title);
    let id = filename.strip_suffix(".md").unwrap_or(&filename).to_string();
    let files_dir = root.join(format!("{id}.files"));
    fs::create_dir_all(&files_dir).map_err(|e| format!("create sidecar dir: {e}"))?;

    let mut sidecar_paths: Vec<String> = Vec::with_capacity(source_paths.len());
    let mut body_lines: Vec<String> = Vec::with_capacity(source_paths.len() + 4);
    body_lines.push(format!(
        "Multi-file package delivered via `k2 msg --inbox-*` ({n} files).\n",
        n = source_paths.len()
    ));
    body_lines.push(String::new());

    // Track used sidecar names so collisions get a numeric suffix.
    let mut used_names: Vec<String> = Vec::new();

    for source_path in source_paths {
        let original_name = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment");
        let safe_name = unique_sidecar_name(original_name, &used_names);
        used_names.push(safe_name.clone());

        let sidecar = files_dir.join(&safe_name);
        fs::copy(source_path, &sidecar)
            .map_err(|e| format!("copy sidecar {}: {e}", source_path.display()))?;
        let file_size = fs::metadata(&sidecar).map(|m| m.len()).unwrap_or(0);
        let size_human = human_bytes(file_size);
        let rel_sidecar = format!("{id}.files/{safe_name}");
        body_lines.push(format!(
            "- **`{safe_name}`** ({size_human}) — `.k2/inbox/{rel_sidecar}`"
        ));
        sidecar_paths.push(sidecar.display().to_string());
    }

    body_lines.push(String::new());
    body_lines.push(format!(
        "Open this cover note with `k2 inbox read {id}`. Sidecars live under `.k2/inbox/{id}.files/`.\n"
    ));

    let created = simple_date_now();
    let body = body_lines.join("\n");
    let written = format!(
        "---\ntitle: {title}\npriority: normal\ncreated: {created}\nsource: {source_tag}\nfrom: {from_tag}\n---\n\n{body}"
    );
    let cover_path = root.join(&filename);
    atomic_write(&cover_path, &written)?;

    Ok(DeliveredPackage {
        id,
        title,
        filename,
        folder: String::new(),
        cover_path: cover_path.display().to_string(),
        sidecar_paths,
        body_preview: body_preview_for(&written),
        source: source_tag,
        from: from_tag,
    })
}

/// Unpack a flat (or `files/`-prefixed) `.tar.gz` into a multi-file tray
/// package. Used for remote multi-upload efficiency: CLI packs basenames,
/// stages the archive, daemon unpacks into one package.
///
/// Entries with directory components other than a single leading `files/`
/// are rejected (path-traversal defense). Empty archives error.
pub fn unpack_tray_bundle(
    workspace: &Path,
    bundle_tar_gz_path: &Path,
    title_override: Option<&str>,
    from: Option<&str>,
    source: Option<&str>,
) -> Result<DeliveredPackage, String> {
    if !bundle_tar_gz_path.is_file() {
        return Err(format!(
            "bundle path is not a readable file: {}",
            bundle_tar_gz_path.display()
        ));
    }

    let tmp_root = std::env::temp_dir().join(format!(
        "k2-inbox-bundle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp_root).map_err(|e| format!("create unpack dir: {e}"))?;

    let extract_result = (|| -> Result<Vec<PathBuf>, String> {
        let file = fs::File::open(bundle_tar_gz_path)
            .map_err(|e| format!("open bundle {}: {e}", bundle_tar_gz_path.display()))?;
        let dec = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(dec);
        let mut extracted: Vec<PathBuf> = Vec::new();

        for entry in ar.entries().map_err(|e| format!("read bundle: {e}"))? {
            let mut entry = entry.map_err(|e| format!("read bundle entry: {e}"))?;
            let header = entry.header().clone();
            // Skip directories / non-files.
            if !header.entry_type().is_file() {
                continue;
            }
            let entry_path = entry
                .path()
                .map_err(|e| format!("entry path: {e}"))?
                .to_path_buf();
            let basename = safe_tar_entry_basename(&entry_path)?;
            let dest = tmp_root.join(&basename);
            // If two entries sanitize to the same name, suffix.
            let dest = if dest.exists() {
                let mut n = 2;
                loop {
                    let candidate = tmp_root.join(format!("{basename}.{n}"));
                    if !candidate.exists() {
                        break candidate;
                    }
                    n += 1;
                }
            } else {
                dest
            };
            {
                let mut out = fs::File::create(&dest)
                    .map_err(|e| format!("create extracted file {}: {e}", dest.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| format!("extract {}: {e}", dest.display()))?;
            }
            extracted.push(dest);
        }
        if extracted.is_empty() {
            return Err("bundle contains no files".to_string());
        }
        Ok(extracted)
    })();

    let extracted = match extract_result {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp_root);
            return Err(e);
        }
    };

    let result = deliver_files(workspace, &extracted, title_override, from, source);
    let _ = fs::remove_dir_all(&tmp_root);
    result
}

/// Accept flat basenames or a single `files/<name>` prefix; reject `..` /
/// multi-component paths.
fn safe_tar_entry_basename(entry_path: &Path) -> Result<String, String> {
    let mut components: Vec<&str> = entry_path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    // Drop a single leading "files" directory if present.
    if components.len() == 2 && components[0].eq_ignore_ascii_case("files") {
        components.remove(0);
    }
    if components.len() != 1 {
        return Err(format!(
            "bundle entry has nested path (only flat or files/ prefix allowed): {}",
            entry_path.display()
        ));
    }
    let name = components[0];
    if name == ".." || name == "." || name.is_empty() {
        return Err(format!(
            "bundle entry has unsafe name: {}",
            entry_path.display()
        ));
    }
    Ok(sanitize_sidecar_filename(name))
}

/// Multi-file title: override → sole-md title → `"N files: a, b, …"` truncated.
fn resolve_multi_title(title_override: Option<&str>, source_paths: &[PathBuf]) -> String {
    if let Some(t) = title_override {
        let t = t.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }

    let md_paths: Vec<&PathBuf> = source_paths
        .iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
        })
        .collect();
    if md_paths.len() == 1 {
        let content = safe_read_to_string(md_paths[0]).ok();
        return resolve_title(None, content.as_deref(), md_paths[0]);
    }

    let names: Vec<String> = source_paths
        .iter()
        .map(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        })
        .collect();
    let n = names.len();
    let joined = names.join(", ");
    let raw = format!("{n} files: {joined}");
    // Keep titles readable in lists / wake lines (~80 chars).
    const MAX: usize = 80;
    if raw.chars().count() <= MAX {
        raw
    } else {
        let mut out = String::new();
        for c in raw.chars().take(MAX.saturating_sub(1)) {
            out.push(c);
        }
        out.push('…');
        out
    }
}

/// Sanitize + ensure uniqueness among already-used sidecar names.
fn unique_sidecar_name(original: &str, used: &[String]) -> String {
    let base = sanitize_sidecar_filename(original);
    if !used.iter().any(|u| u == &base) {
        return base;
    }
    // Split stem/ext for nicer suffixes: report.pdf → report-2.pdf
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() && !e.contains('/') => {
            (s.to_string(), Some(e.to_string()))
        }
        _ => (base.clone(), None),
    };
    let mut n = 2;
    loop {
        let candidate = match &ext {
            Some(e) => format!("{stem}-{n}.{e}"),
            None => format!("{stem}-{n}"),
        };
        if !used.iter().any(|u| u == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn deliver_markdown_file(
    root: &Path,
    source_path: &Path,
    title_override: Option<&str>,
    from: &str,
    source: &str,
) -> Result<DeliveredPackage, String> {
    let content = safe_read_to_string(source_path)?;
    let title = resolve_title(title_override, Some(&content), source_path);
    let body = extract_md_body(&content);
    let filename = allocate_md_filename(root, &title);
    let created = simple_date_now();
    let priority = parse_frontmatter(&content)
        .get("priority")
        .cloned()
        .unwrap_or_else(|| "normal".to_string());

    let written = format!(
        "---\ntitle: {title}\npriority: {priority}\ncreated: {created}\nsource: {source}\nfrom: {from}\n---\n\n{body}\n"
    );
    let path = root.join(&filename);
    atomic_write(&path, &written)?;

    let id = filename.strip_suffix(".md").unwrap_or(&filename).to_string();
    Ok(DeliveredPackage {
        id,
        title,
        filename,
        folder: String::new(),
        cover_path: path.display().to_string(),
        sidecar_paths: Vec::new(),
        body_preview: body_preview_for(&written),
        source: source.to_string(),
        from: from.to_string(),
    })
}

fn deliver_binary_file(
    root: &Path,
    source_path: &Path,
    title_override: Option<&str>,
    from: &str,
    source: &str,
) -> Result<DeliveredPackage, String> {
    let title = resolve_title(title_override, None, source_path);
    let original_name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment");
    let safe_name = sanitize_sidecar_filename(original_name);
    let file_size = fs::metadata(source_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let size_human = human_bytes(file_size);

    let filename = allocate_md_filename(root, &title);
    let id = filename.strip_suffix(".md").unwrap_or(&filename).to_string();
    let files_dir = root.join(format!("{id}.files"));
    fs::create_dir_all(&files_dir).map_err(|e| format!("create sidecar dir: {e}"))?;
    let sidecar = files_dir.join(&safe_name);

    // Copy bytes (not rename — source stays with the caller).
    fs::copy(source_path, &sidecar)
        .map_err(|e| format!("copy sidecar {}: {e}", source_path.display()))?;

    let rel_sidecar = format!("{id}.files/{safe_name}");
    let created = simple_date_now();
    let body = format!(
        "Attachment package delivered via `k2 msg --inbox-*`.\n\n\
         - **File:** `{safe_name}` ({size_human})\n\
         - **Sidecar:** `.k2/inbox/{rel_sidecar}` (or `.k2so/inbox/` on legacy workspaces)\n\n\
         Open this cover note with `k2 inbox read {id}`. Open the sidecar path on disk for the file bytes.\n"
    );
    let written = format!(
        "---\ntitle: {title}\npriority: normal\ncreated: {created}\nsource: {source}\nfrom: {from}\n---\n\n{body}"
    );
    let cover_path = root.join(&filename);
    atomic_write(&cover_path, &written)?;

    Ok(DeliveredPackage {
        id,
        title,
        filename,
        folder: String::new(),
        cover_path: cover_path.display().to_string(),
        sidecar_paths: vec![sidecar.display().to_string()],
        body_preview: body_preview_for(&written),
        source: source.to_string(),
        from: from.to_string(),
    })
}

/// Allocate a free `<slug>.md` (or `<slug>-N.md`) under `root`.
fn allocate_md_filename(root: &Path, title: &str) -> String {
    let stem = slug_for_title(title);
    let mut filename = format!("{stem}.md");
    let mut suffix = 2;
    while root.join(&filename).exists() {
        filename = format!("{stem}-{suffix}.md");
        suffix += 1;
    }
    filename
}

/// Title resolution order for file packages (see PRD §4.3).
fn resolve_title(
    title_override: Option<&str>,
    md_content: Option<&str>,
    source_path: &Path,
) -> String {
    if let Some(t) = title_override {
        let t = t.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(content) = md_content {
        let fm = parse_frontmatter(content);
        if let Some(t) = fm.get("title") {
            let t = t.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        if let Some(h) = first_md_heading(content) {
            return h;
        }
    }
    source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "untitled".to_string())
}

/// Body after the closing `---` of YAML frontmatter, or the full file.
fn extract_md_body(content: &str) -> String {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return content[3 + end + 3..].trim().to_string();
        }
    }
    content.trim().to_string()
}

/// First ATX heading (`# Title`) in the markdown body, if any.
fn first_md_heading(content: &str) -> Option<String> {
    let body = extract_md_body(content);
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            // Require at least one # followed by space-ish heading text.
            let rest = rest.trim_start_matches('#').trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Sanitize a sidecar filename: strip path components, reject `..`, keep
/// alphanum / dash / underscore / dot; collapse everything else to `_`.
pub fn sanitize_sidecar_filename(name: &str) -> String {
    // Drop any directory components (defense in depth).
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment");
    if base == ".." || base == "." || base.is_empty() {
        return "attachment".to_string();
    }
    let mut out = String::with_capacity(base.len());
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('.').trim_matches('_');
    if trimmed.is_empty() || trimmed == ".." {
        "attachment".to_string()
    } else {
        trimmed.to_string()
    }
}

fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
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

    #[test]
    fn deliver_file_md_normalizes_frontmatter() {
        let ws = make_ws();
        let src = ws.path().join("brief.md");
        fs::write(
            &src,
            "---\ntitle: Ship It\npriority: high\nfrom: someone\n---\n\nDo the thing.\n",
        )
        .unwrap();

        let pkg = deliver_file(ws.path(), &src, None, Some("alice"), None).unwrap();
        assert_eq!(pkg.title, "Ship It");
        assert_eq!(pkg.source, "msg-inbox");
        assert_eq!(pkg.from, "alice");
        assert!(pkg.sidecar_paths.is_empty());
        assert_eq!(pkg.filename, "ship-it.md");

        let content = fs::read_to_string(inbox_root(ws.path()).join(&pkg.filename)).unwrap();
        assert!(content.contains("title: Ship It"));
        assert!(content.contains("source: msg-inbox"));
        assert!(content.contains("from: alice"));
        assert!(content.contains("priority: high"));
        assert!(content.contains("Do the thing."));
        // Original from: is overwritten by deliver's from arg.
        assert!(!content.contains("from: someone"));
    }

    #[test]
    fn deliver_file_md_title_override_and_heading_fallback() {
        let ws = make_ws();
        let src = ws.path().join("notes.md");
        fs::write(&src, "# Heading Title\n\nBody only.\n").unwrap();

        let pkg = deliver_file(ws.path(), &src, None, None, None).unwrap();
        assert_eq!(pkg.title, "Heading Title");

        let pkg2 = deliver_file(
            ws.path(),
            &src,
            Some("Explicit"),
            Some("bob"),
            Some("msg-inbox"),
        )
        .unwrap();
        assert_eq!(pkg2.title, "Explicit");
        assert_eq!(pkg2.from, "bob");
    }

    #[test]
    fn deliver_file_binary_writes_cover_and_sidecar() {
        let ws = make_ws();
        let src = ws.path().join("report.pdf");
        fs::write(&src, b"%PDF-1.4 fake").unwrap();

        let pkg = deliver_file(ws.path(), &src, Some("Q2 Report"), Some("cli"), None).unwrap();
        assert_eq!(pkg.title, "Q2 Report");
        assert_eq!(pkg.sidecar_paths.len(), 1);
        assert!(pkg.sidecar_paths[0].ends_with("report.pdf"));
        assert!(Path::new(&pkg.sidecar_paths[0]).exists());
        assert!(Path::new(&pkg.cover_path).exists());

        let files_dir = inbox_root(ws.path()).join(format!("{}.files", pkg.id));
        assert!(files_dir.join("report.pdf").exists());
        let cover = fs::read_to_string(&pkg.cover_path).unwrap();
        assert!(cover.contains("source: msg-inbox"));
        assert!(cover.contains("report.pdf"));
        assert!(cover.contains(&format!("k2 inbox read {}", pkg.id)));
    }

    #[test]
    fn sanitize_sidecar_rejects_traversal() {
        assert_eq!(sanitize_sidecar_filename("../etc/passwd"), "passwd");
        assert_eq!(sanitize_sidecar_filename(".."), "attachment");
        assert_eq!(sanitize_sidecar_filename("my file (1).pdf"), "my_file__1_.pdf");
    }

    #[test]
    fn wake_pointer_text_format() {
        let t = wake_pointer_text("ship-it", "Ship It");
        assert_eq!(t, "[inbox:ship-it] Ship It\nOpen: k2 inbox read ship-it");
    }

    #[test]
    fn deliver_file_missing_source_errors() {
        let ws = make_ws();
        let err = deliver_file(ws.path(), &ws.path().join("nope.md"), None, None, None)
            .unwrap_err();
        assert!(err.contains("not a readable file"));
    }

    #[test]
    fn deliver_files_empty_errors() {
        let ws = make_ws();
        let err = deliver_files(ws.path(), &[], None, None, None).unwrap_err();
        assert!(err.contains("no source paths"));
    }

    #[test]
    fn deliver_files_one_path_matches_deliver_file() {
        let ws = make_ws();
        let src = ws.path().join("solo.pdf");
        fs::write(&src, b"%PDF solo").unwrap();

        let single = deliver_file(ws.path(), &src, Some("Solo"), Some("cli"), None).unwrap();
        // Second call would allocate a different slug suffix if same title —
        // compare structural shape rather than id.
        let multi = deliver_files(
            ws.path(),
            &[src.clone()],
            Some("Solo2"),
            Some("cli"),
            None,
        )
        .unwrap();
        assert_eq!(multi.sidecar_paths.len(), single.sidecar_paths.len());
        assert_eq!(multi.from, "cli");
        assert_eq!(multi.source, "msg-inbox");
        assert!(multi.sidecar_paths[0].ends_with("solo.pdf"));
        assert!(Path::new(&multi.cover_path).exists());
    }

    #[test]
    fn deliver_files_two_files_cover_and_sidecars() {
        let ws = make_ws();
        let a = ws.path().join("a.md");
        let b = ws.path().join("b.pdf");
        fs::write(&a, "---\ntitle: Alpha Note\n---\n\nHello.\n").unwrap();
        fs::write(&b, b"%PDF-bytes").unwrap();

        let pkg = deliver_files(
            ws.path(),
            &[a, b],
            None,
            Some("alice"),
            None,
        )
        .unwrap();
        // Exactly one .md among two → use that md's title.
        assert_eq!(pkg.title, "Alpha Note");
        assert_eq!(pkg.sidecar_paths.len(), 2);
        assert_eq!(pkg.from, "alice");
        assert_eq!(pkg.source, "msg-inbox");

        let files_dir = inbox_root(ws.path()).join(format!("{}.files", pkg.id));
        assert!(files_dir.join("a.md").exists());
        assert!(files_dir.join("b.pdf").exists());
        assert!(Path::new(&pkg.cover_path).exists());

        let cover = fs::read_to_string(&pkg.cover_path).unwrap();
        assert!(cover.contains("title: Alpha Note"));
        assert!(cover.contains("source: msg-inbox"));
        assert!(cover.contains("from: alice"));
        assert!(cover.contains("priority: normal"));
        assert!(cover.contains("a.md"));
        assert!(cover.contains("b.pdf"));
        assert!(cover.contains(&format!("k2 inbox read {}", pkg.id)));
        assert!(cover.contains("2 files"));
    }

    #[test]
    fn deliver_files_title_override_and_n_files_default() {
        let ws = make_ws();
        let a = ws.path().join("x.txt");
        let b = ws.path().join("y.csv");
        fs::write(&a, "x").unwrap();
        fs::write(&b, "y").unwrap();

        let pkg = deliver_files(ws.path(), &[a.clone(), b.clone()], None, None, None).unwrap();
        assert!(pkg.title.starts_with("2 files:"));
        assert!(pkg.title.contains("x.txt"));
        assert!(pkg.title.contains("y.csv"));

        let pkg2 = deliver_files(
            ws.path(),
            &[a, b],
            Some("Batch Drop"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(pkg2.title, "Batch Drop");
    }

    #[test]
    fn deliver_files_missing_source_errors() {
        let ws = make_ws();
        let good = ws.path().join("ok.txt");
        fs::write(&good, "ok").unwrap();
        let bad = ws.path().join("missing.bin");
        let err = deliver_files(ws.path(), &[good, bad], None, None, None).unwrap_err();
        assert!(err.contains("not a readable file"));
    }

    #[test]
    fn unpack_tray_bundle_flat_tar_gz() {
        let ws = make_ws();
        // Build a minimal flat tar.gz of two files.
        let stage = ws.path().join("stage");
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("one.txt"), b"one").unwrap();
        fs::write(stage.join("two.bin"), b"two").unwrap();
        let bundle = ws.path().join("pack.tar.gz");
        {
            let f = fs::File::create(&bundle).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            for name in ["one.txt", "two.bin"] {
                let path = stage.join(name);
                let mut file = fs::File::open(&path).unwrap();
                let meta = file.metadata().unwrap();
                let mut header = tar::Header::new_gnu();
                header.set_metadata(&meta);
                header.set_size(meta.len());
                header.set_cksum();
                tar.append_data(&mut header, Path::new(name), &mut file)
                    .unwrap();
            }
            let enc = tar.into_inner().unwrap();
            enc.finish().unwrap();
        }

        let pkg = unpack_tray_bundle(
            ws.path(),
            &bundle,
            Some("From Bundle"),
            Some("remote"),
            None,
        )
        .unwrap();
        assert_eq!(pkg.title, "From Bundle");
        assert_eq!(pkg.sidecar_paths.len(), 2);
        assert_eq!(pkg.from, "remote");
        let files_dir = inbox_root(ws.path()).join(format!("{}.files", pkg.id));
        assert!(files_dir.join("one.txt").exists());
        assert!(files_dir.join("two.bin").exists());
    }

    #[test]
    fn wake_pointer_text_n_mentions_file_count() {
        let t = wake_pointer_text_n("pack", "Stuff", Some(3));
        assert!(t.contains("(3 files)"));
        assert!(t.contains("[inbox:pack] Stuff"));
        let single = wake_pointer_text_n("pack", "Stuff", Some(1));
        assert!(!single.contains("files)"));
    }
}
