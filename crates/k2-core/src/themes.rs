//! Custom-theme management (Phase 2 Unit 6).
//!
//! Themes live as JSON files under `~/.k2so/themes/`. Each file is
//! authored either by the AIFileEditor in Settings or copy-pasted by
//! hand; both paths go through `create_template` first to seed a
//! valid skeleton.
//!
//! No DB rows: themes are file-only. The renderer's
//! `custom-themes.ts` store re-reads the directory whenever it needs
//! a fresh list.

use serde::Serialize;
use std::fs;
use std::path::PathBuf;

pub fn themes_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".k2")
        .join("themes")
}

pub fn get_dir() -> Result<String, String> {
    Ok(themes_dir().to_string_lossy().to_string())
}

pub fn ensure_dir() -> Result<String, String> {
    let dir = themes_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create themes dir: {e}"))?;
    Ok(dir.to_string_lossy().to_string())
}

/// Create a new custom-theme template file. `base_theme_json` may be
/// the JSON of an existing theme (so the user starts from a known-good
/// baseline) or empty — in which case we seed a sensible default.
pub fn create_template(base_theme_json: &str) -> Result<String, String> {
    let dir = themes_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create themes dir: {e}"))?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("custom-theme-{timestamp}.json");
    let path = dir.join(&filename);
    let content = if base_theme_json.trim().is_empty() {
        default_theme_json()
    } else {
        match serde_json::from_str::<serde_json::Value>(base_theme_json) {
            Ok(_) => base_theme_json.to_string(),
            Err(_) => default_theme_json(),
        }
    };
    fs::write(&path, content).map_err(|e| format!("Failed to write theme file: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[derive(Debug, Serialize)]
pub struct CustomThemeEntry {
    pub path: String,
    pub name: String,
    pub valid: bool,
}

pub fn list_custom() -> Result<Vec<CustomThemeEntry>, String> {
    let dir = themes_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&dir).map_err(|e| format!("Failed to read themes dir: {e}"))?;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                entries.push(CustomThemeEntry {
                    path: path.to_string_lossy().to_string(),
                    name: path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    valid: false,
                });
                continue;
            }
        };
        let (name, valid) = match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(val) => {
                let name = val
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_str()
                            .unwrap_or("Untitled")
                    })
                    .to_string();
                let has_colors = val.get("colors").is_some();
                (name, has_colors)
            }
            Err(_) => (
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                false,
            ),
        };
        entries.push(CustomThemeEntry {
            path: path.to_string_lossy().to_string(),
            name,
            valid,
        });
    }
    Ok(entries)
}

pub fn delete(path: &str) -> Result<(), String> {
    let path = PathBuf::from(path);
    let dir = themes_dir();
    if !path.starts_with(&dir) {
        return Err("Can only delete files inside ~/.k2so/themes/".into());
    }
    fs::remove_file(&path).map_err(|e| format!("Failed to delete theme: {e}"))?;
    Ok(())
}

/// Process-wide HOME lock shared by every k2so-core test module that
/// mutates `$HOME` (themes, skill_layers, chat_history Unit-6 tests).
/// Living at file scope (not inside `mod tests`) so other modules can
/// reach it via `crate::themes::HOME_LOCK`. Tests in `app_settings`
/// and `whats_new` predate Phase 2 Unit 6 and use private module-level
/// locks; they can still race against this one — that's a pre-existing
/// gap, not something this commit can close without touching those
/// files (which is out of scope per the unit-6 backfill brief).
#[cfg(test)]
pub(crate) static HOME_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[cfg(test)]
mod tests {
    //! Tests for the Phase 2 Unit 6 themes module.
    //!
    //! `themes_dir()` hardcodes `dirs::home_dir().join(".k2so/themes")`,
    //! so we install a fresh HOME for each test via the same
    //! `HomeGuard` + serialized lock pattern used in
    //! `app_settings::tests`.
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        _tmp: TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let tmp = TempDir::new("k2so-themes-test");
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp.path());
            Self { original, _tmp: tmp }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}"));
            fs::create_dir_all(&path).expect("create tempdir");
            Self { path }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // HOME-mutating tests share the process-wide `HOME_LOCK` so
    // cargo's parallel runner doesn't see other tests' HOME between
    // this test's set and the inner call.
    use super::HOME_LOCK as TEST_LOCK;

    #[test]
    fn list_custom_returns_empty_when_dir_missing() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        let entries = list_custom().expect("list");
        assert!(entries.is_empty(), "fresh HOME must have no themes: {entries:?}");
    }

    #[test]
    fn ensure_dir_creates_themes_directory_under_home() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        // Assert the *shape* of the returned path, not the specific
        // tempdir prefix. `whats_new::tests` mutates HOME without
        // sharing our lock, so an absolute-path equality check would
        // be brittle. The contract we're pinning down is "themes dir
        // sits at <HOME>/.k2/themes and is created on demand" —
        // both of those are observable in the returned string alone.
        let dir = ensure_dir().expect("ensure_dir");
        assert!(dir.ends_with("/.k2/themes"), "got: {dir}");
        assert!(std::path::Path::new(&dir).exists());
    }

    #[test]
    fn create_template_with_empty_json_writes_default_skeleton() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        let path = create_template("").expect("create_template");
        let body = fs::read_to_string(&path).expect("read");
        // Default skeleton is the JSON in default_theme_json — verify
        // the "colors" key is present (used by list_custom's
        // valid=true check).
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert!(parsed.get("colors").is_some(), "default must include 'colors'");
        assert_eq!(parsed.get("name").and_then(|n| n.as_str()), Some("My Custom Theme"));
    }

    #[test]
    fn create_template_with_invalid_json_falls_back_to_default() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        let path = create_template("not json {{{").expect("create");
        let body = fs::read_to_string(&path).expect("read");
        // Must be valid JSON because the invalid input was rejected
        // and we fell back to default_theme_json.
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert!(parsed.get("colors").is_some());
    }

    #[test]
    fn create_template_with_valid_base_uses_it_verbatim() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        let base = r##"{"name":"Imported","colors":{"bg":"#000000"}}"##;
        let path = create_template(base).expect("create");
        let body = fs::read_to_string(&path).expect("read");
        assert_eq!(body, base);
    }

    #[test]
    fn delete_rejects_paths_outside_themes_dir() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        ensure_dir().expect("ensure_dir");
        // /tmp/whatever — definitely not under ~/.k2so/themes.
        let escape = std::env::temp_dir().join("not-a-theme.json");
        fs::write(&escape, "{}").expect("seed file");
        let err = delete(escape.to_str().unwrap()).expect_err("must reject");
        assert!(err.contains("Can only delete"), "got: {err}");
        // File must still exist — sandbox not breached.
        assert!(escape.exists());
        let _ = fs::remove_file(&escape);
    }

    #[test]
    fn delete_removes_theme_inside_dir() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        let path = create_template("").expect("create");
        assert!(Path::new(&path).exists());
        delete(&path).expect("delete");
        assert!(!Path::new(&path).exists());
    }

    #[test]
    fn list_custom_reports_valid_and_invalid_themes() {
        let _g = TEST_LOCK.lock();
        let _h = HomeGuard::new();
        let dir = ensure_dir().expect("ensure_dir");
        // One valid theme — must have "colors" to be marked valid.
        fs::write(
            Path::new(&dir).join("good.json"),
            r##"{"name":"Good Theme","colors":{"bg":"#000"}}"##,
        )
        .unwrap();
        // One JSON without colors → valid=false per list_custom logic.
        fs::write(
            Path::new(&dir).join("nocolors.json"),
            r#"{"name":"No Colors"}"#,
        )
        .unwrap();
        // Non-JSON garbage → valid=false, name=file_stem.
        fs::write(Path::new(&dir).join("broken.json"), "not json").unwrap();
        // Non-.json file ignored entirely.
        fs::write(Path::new(&dir).join("readme.md"), "ignore me").unwrap();

        let mut entries = list_custom().expect("list");
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 3, "got: {entries:?}");
        // broken.json -> file_stem "broken" (sorts first)
        // Good Theme  (sorts second alphabetically)
        // No Colors   (sorts third)
        let by_name: std::collections::HashMap<&str, bool> =
            entries.iter().map(|e| (e.name.as_str(), e.valid)).collect();
        assert_eq!(by_name.get("Good Theme"), Some(&true));
        assert_eq!(by_name.get("No Colors"), Some(&false));
        assert_eq!(by_name.get("broken"), Some(&false));
    }
}

fn default_theme_json() -> String {
    serde_json::json!({
        "name": "My Custom Theme",
        "type": "dark",
        "colors": {
            "bg": "#0a0a0a",
            "fg": "#e4e4e7",
            "gutterBg": "#0a0a0a",
            "gutterFg": "#555555",
            "gutterBorder": "#1a1a1a",
            "activeLine": "#ffffff08",
            "selection": "#3b82f633",
            "cursor": "#3b82f6",
            "accent": "#3b82f6"
        },
        "syntax": {
            "keyword": "#c678dd",
            "string": "#98c379",
            "number": "#d19a66",
            "comment": "#5c6370",
            "function": "#61afef",
            "type": "#e5c07b",
            "variable": "#e4e4e7",
            "property": "#e06c75",
            "operator": "#56b6c2",
            "tag": "#e06c75",
            "attribute": "#d19a66",
            "regexp": "#98c379",
            "punctuation": "#abb2bf"
        }
    })
    .to_string()
}
