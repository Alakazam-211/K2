//! Work item types + safe filesystem readers shared across the
//! scheduler, skill composer, and daemon/Tauri HTTP handlers.
//!
//! A `WorkItem` is the parsed markdown file shape used throughout the
//! agent system — inbox/active/done folders all contain `.md` files
//! with YAML frontmatter (title, priority, type, source, etc.) and a
//! short body preview.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::workspace::agent_identity::parse_frontmatter;

/// Maximum file size for reading work items and agent profiles (1 MiB).
/// Prevents memory exhaustion from malicious or corrupted files.
pub const MAX_FILE_SIZE: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub filename: String,
    pub title: String,
    pub priority: String,
    pub assigned_by: String,
    pub created: String,
    pub item_type: String,
    pub folder: String,
    pub body_preview: String,
    /// Work source: "feature", "issue", "crash", "security", "audit", "manual"
    pub source: String,
}

/// Parse raw markdown content into a [`WorkItem`]. Pure — no I/O, no
/// Tauri deps, directly unit-testable.
pub fn parse_work_item_content(content: &str, filename: &str, folder: &str) -> WorkItem {
    let fm = parse_frontmatter(content);

    // Extract body preview (first ~120 chars after frontmatter).
    let body_preview = if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let body = content[3 + end + 3..].trim();
            let preview: String = body.chars().take(120).collect();
            if body.len() > 120 {
                format!("{}...", preview.trim())
            } else {
                preview.trim().to_string()
            }
        } else {
            String::new()
        }
    } else {
        let preview: String = content.chars().take(120).collect();
        if content.len() > 120 {
            format!("{}...", preview.trim())
        } else {
            preview.trim().to_string()
        }
    };

    WorkItem {
        filename: filename.to_string(),
        title: fm.get("title").cloned().unwrap_or_default(),
        priority: fm.get("priority").cloned().unwrap_or("normal".to_string()),
        assigned_by: fm.get("assigned_by").cloned().unwrap_or("unknown".to_string()),
        created: fm.get("created").cloned().unwrap_or_default(),
        item_type: fm.get("type").cloned().unwrap_or("task".to_string()),
        folder: folder.to_string(),
        body_preview,
        source: fm.get("source").cloned().unwrap_or("manual".to_string()),
    }
}

/// Read a `.md` file from disk + parse into a [`WorkItem`]. `None` if
/// the file can't be read within [`MAX_FILE_SIZE`] or has no filename.
pub fn read_work_item(path: &Path, folder: &str) -> Option<WorkItem> {
    let content = safe_read_to_string(path).ok()?;
    let filename = path.file_name()?.to_string_lossy().to_string();
    Some(parse_work_item_content(&content, &filename, folder))
}

/// Read a file with size-limit check to prevent OOM from large or
/// malicious files. Returns a `String` error describing the failure —
/// suitable for bubbling back through `#[tauri::command]` handlers.
pub fn safe_read_to_string(path: &Path) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "File too large ({} bytes, max {}): {}",
            metadata.len(),
            MAX_FILE_SIZE,
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))
}

/// Thin wrapper around [`crate::fs_atomic::atomic_write_str`]
/// preserving the `Result<(), String>` signature existing callers
/// propagate with `?` and `.map_err(|e| e.to_string())`.
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    crate::fs_atomic::atomic_write_str(path, content)
        .map_err(|e| format!("atomic write failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_work_item_reads_frontmatter() {
        let md = "---\ntitle: Fix auth\npriority: high\ntype: bug\nsource: crash\n---\n\nBody text here.";
        let item = parse_work_item_content(md, "fix-auth.md", "inbox");
        assert_eq!(item.title, "Fix auth");
        assert_eq!(item.priority, "high");
        assert_eq!(item.item_type, "bug");
        assert_eq!(item.source, "crash");
        assert_eq!(item.folder, "inbox");
        assert!(item.body_preview.contains("Body text"));
    }

    #[test]
    fn parse_work_item_fills_defaults_on_missing_fields() {
        let md = "---\ntitle: Minimal\n---\nJust a title.";
        let item = parse_work_item_content(md, "min.md", "active");
        assert_eq!(item.title, "Minimal");
        assert_eq!(item.priority, "normal");
        assert_eq!(item.item_type, "task");
        assert_eq!(item.source, "manual");
        assert_eq!(item.assigned_by, "unknown");
    }

    #[test]
    fn body_preview_truncates_long_bodies() {
        let body = "x".repeat(200);
        let md = format!("---\ntitle: Long\n---\n{}", body);
        let item = parse_work_item_content(&md, "long.md", "inbox");
        assert!(item.body_preview.ends_with("..."));
        assert!(item.body_preview.len() <= 200);
    }
}
