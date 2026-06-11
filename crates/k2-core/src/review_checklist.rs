//! Per-workspace review checklist (Phase 2 Unit 6).
//!
//! The checklist lives on disk at `<workspace>/.k2so/review-checklist.md`
//! as a markdown file with two H2 sections: "Verify Features" and
//! "Test Criteria". Each section is a list of `- [ ]` / `- [x]`
//! checkbox lines. Parsing and serialization are round-trip safe so
//! the user can hand-edit the file directly.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const CHECKLIST_FILENAME: &str = ".k2so/review-checklist.md";

fn checklist_path(workspace_path: &str) -> PathBuf {
    Path::new(workspace_path).join(CHECKLIST_FILENAME)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistItem {
    pub text: String,
    pub checked: bool,
    pub section: String,
}

pub fn parse_checklist(content: &str) -> Vec<ChecklistItem> {
    let mut items = Vec::new();
    let mut current_section = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            let header = trimmed[3..].trim().to_lowercase();
            if header.contains("verify") || header.contains("feature") {
                current_section = "verify".to_string();
            } else if header.contains("test") || header.contains("criteria") {
                current_section = "criteria".to_string();
            }
            continue;
        }
        if trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ") {
            items.push(ChecklistItem {
                text: trimmed[6..].to_string(),
                checked: true,
                section: current_section.clone(),
            });
        } else if trimmed.starts_with("- [ ] ") {
            items.push(ChecklistItem {
                text: trimmed[6..].to_string(),
                checked: false,
                section: current_section.clone(),
            });
        }
    }
    items
}

pub fn serialize_checklist(items: &[ChecklistItem], agent_name: &str, branch: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Review: {}\n", agent_name));
    out.push_str(&format!("Branch: {}\n\n", branch));
    let verify_items: Vec<&ChecklistItem> =
        items.iter().filter(|i| i.section == "verify").collect();
    if !verify_items.is_empty() {
        out.push_str("## Verify Features\n");
        for item in &verify_items {
            let mark = if item.checked { "x" } else { " " };
            out.push_str(&format!("- [{}] {}\n", mark, item.text));
        }
        out.push('\n');
    }
    let criteria_items: Vec<&ChecklistItem> =
        items.iter().filter(|i| i.section == "criteria").collect();
    if !criteria_items.is_empty() {
        out.push_str("## Test Criteria\n");
        for item in &criteria_items {
            let mark = if item.checked { "x" } else { " " };
            out.push_str(&format!("- [{}] {}\n", mark, item.text));
        }
        out.push('\n');
    }
    out
}

pub fn read(workspace_path: &str) -> Result<Vec<ChecklistItem>, String> {
    let path = checklist_path(workspace_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read checklist: {}", e))?;
    Ok(parse_checklist(&content))
}

pub fn write(
    workspace_path: &str,
    items: &[ChecklistItem],
    agent_name: &str,
    branch: &str,
) -> Result<(), String> {
    let path = checklist_path(workspace_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }
    let content = serialize_checklist(items, agent_name, branch);
    fs::write(&path, content).map_err(|e| format!("Failed to write checklist: {}", e))?;
    Ok(())
}

pub fn toggle(
    workspace_path: &str,
    index: usize,
    agent_name: &str,
    branch: &str,
) -> Result<Vec<ChecklistItem>, String> {
    let path = checklist_path(workspace_path);
    let content = if path.exists() {
        fs::read_to_string(&path).map_err(|e| format!("Failed to read checklist: {}", e))?
    } else {
        String::new()
    };
    let mut items = parse_checklist(&content);
    if index < items.len() {
        items[index].checked = !items[index].checked;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }
    let out = serialize_checklist(&items, agent_name, branch);
    fs::write(&path, out).map_err(|e| format!("Failed to write checklist: {}", e))?;
    Ok(items)
}

pub fn init(
    workspace_path: &str,
    items: &[ChecklistItem],
    agent_name: &str,
    branch: &str,
) -> Result<(), String> {
    let path = checklist_path(workspace_path);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }
    let content = serialize_checklist(items, agent_name, branch);
    fs::write(&path, content).map_err(|e| format!("Failed to write checklist: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Tests for the Phase 2 Unit 6 review_checklist module.
    //!
    //! All checklist state lives at `<workspace>/.k2so/review-checklist.md`,
    //! so each test gets a fresh tempdir as its synthetic workspace.
    //! No HOME mutation, no DB. Pure file I/O.
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path =
                std::env::temp_dir().join(format!("k2so-review_checklist-test-{pid}-{nanos}"));
            fs::create_dir_all(&path).expect("create tempdir");
            Self { path }
        }
        fn s(&self) -> String {
            self.path.to_string_lossy().to_string()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sample_items() -> Vec<ChecklistItem> {
        vec![
            ChecklistItem {
                text: "feature A works".into(),
                checked: false,
                section: "verify".into(),
            },
            ChecklistItem {
                text: "feature B works".into(),
                checked: true,
                section: "verify".into(),
            },
            ChecklistItem {
                text: "tests pass".into(),
                checked: false,
                section: "criteria".into(),
            },
        ]
    }

    #[test]
    fn read_missing_checklist_returns_empty_vec() {
        let tmp = TempDir::new();
        let items = read(&tmp.s()).expect("read missing");
        assert!(items.is_empty(), "missing file must yield []: {items:?}");
    }

    #[test]
    fn parse_handles_lowercase_uppercase_checked_markers() {
        let body = "## Verify Features\n- [x] alpha\n- [X] beta\n- [ ] gamma\n";
        let items = parse_checklist(body);
        assert_eq!(items.len(), 3);
        assert!(items[0].checked);
        assert!(items[1].checked);
        assert!(!items[2].checked);
        assert!(items.iter().all(|i| i.section == "verify"));
    }

    #[test]
    fn parse_assigns_correct_section_per_h2() {
        let body = "\
## Verify Features
- [ ] one
- [x] two

## Test Criteria
- [ ] three
";
        let items = parse_checklist(body);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].section, "verify");
        assert_eq!(items[1].section, "verify");
        assert_eq!(items[2].section, "criteria");
        assert_eq!(items[2].text, "three");
    }

    #[test]
    fn parse_ignores_non_checkbox_lines_and_blank_sections() {
        let body = "# Title\n\nSome paragraph.\n\n## Verify Features\n- [ ] only\n\nNot a checkbox\n";
        let items = parse_checklist(body);
        assert_eq!(items.len(), 1, "only the one checkbox line counts: {items:?}");
        assert_eq!(items[0].text, "only");
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = TempDir::new();
        let original = sample_items();
        write(&tmp.s(), &original, "qa-eng", "agent/qa/foo").expect("write");
        let parsed = read(&tmp.s()).expect("read");
        assert_eq!(parsed.len(), original.len(), "got: {parsed:?}");
        // The serializer regroups items by section (verify then
        // criteria) but parse_checklist returns them in file order,
        // which after serialize == verify-items first, then criteria.
        // Compare by (text, checked, section) tuples in order.
        for (a, b) in parsed.iter().zip(original.iter()) {
            assert_eq!(a.text, b.text);
            assert_eq!(a.checked, b.checked);
            assert_eq!(a.section, b.section);
        }
    }

    #[test]
    fn write_creates_dot_k2so_subdirectory() {
        let tmp = TempDir::new();
        // .k2so does NOT exist yet — write must create it.
        assert!(!tmp.path.join(".k2so").exists());
        write(&tmp.s(), &sample_items(), "qa", "branch-x").expect("write");
        assert!(tmp.path.join(".k2so/review-checklist.md").exists());
    }

    #[test]
    fn serialize_emits_section_headers_and_branch_metadata() {
        let body = serialize_checklist(&sample_items(), "qa-eng", "agent/qa/foo");
        assert!(body.starts_with("# Review: qa-eng"), "got: {body:?}");
        assert!(body.contains("Branch: agent/qa/foo"));
        assert!(body.contains("## Verify Features"));
        assert!(body.contains("## Test Criteria"));
        assert!(body.contains("- [x] feature B works"));
        assert!(body.contains("- [ ] feature A works"));
        assert!(body.contains("- [ ] tests pass"));
    }

    #[test]
    fn toggle_flips_target_item_and_persists() {
        let tmp = TempDir::new();
        write(&tmp.s(), &sample_items(), "qa", "branch").expect("seed");
        // sample_items is [verify-A unchecked, verify-B checked, criteria-tests unchecked].
        // After serialize+reparse the order is still verify-first, so index 0 = "feature A works".
        let updated = toggle(&tmp.s(), 0, "qa", "branch").expect("toggle");
        assert!(updated[0].checked, "index 0 should flip false -> true");
        // Re-read from disk to confirm persistence (toggle returns the in-memory state too).
        let from_disk = read(&tmp.s()).expect("read");
        assert!(from_disk[0].checked, "toggle must persist to disk");
    }

    #[test]
    fn toggle_out_of_bounds_index_is_a_noop_save() {
        let tmp = TempDir::new();
        write(&tmp.s(), &sample_items(), "qa", "branch").expect("seed");
        let result = toggle(&tmp.s(), 999, "qa", "branch").expect("toggle oob");
        assert_eq!(result.len(), 3, "items unchanged on out-of-bounds toggle");
        for (a, b) in result.iter().zip(sample_items().iter()) {
            assert_eq!(a.checked, b.checked);
        }
    }

    #[test]
    fn init_writes_default_when_missing() {
        let tmp = TempDir::new();
        init(&tmp.s(), &sample_items(), "qa", "branch").expect("init");
        let items = read(&tmp.s()).expect("read");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn init_is_idempotent_when_file_exists() {
        let tmp = TempDir::new();
        // First init writes sample.
        init(&tmp.s(), &sample_items(), "qa", "branch").expect("first init");
        // Second init with DIFFERENT items must be a no-op.
        let different = vec![ChecklistItem {
            text: "wholly different".into(),
            checked: true,
            section: "verify".into(),
        }];
        init(&tmp.s(), &different, "qa", "branch").expect("second init");
        let items = read(&tmp.s()).expect("read");
        assert_eq!(items.len(), 3, "first init's content must be preserved");
        assert!(items.iter().any(|i| i.text == "feature A works"));
    }
}
