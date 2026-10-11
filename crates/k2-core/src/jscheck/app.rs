//! An App's folder through the VALIDATE stage (`k2 app validate`): every
//! JS/TS file and every inline `<script>` in its HTML pages is parsed and
//! leak-linted ([`super::check`] with [`Surface::App`]).
//!
//! The walk never follows symlinks, skips dependency and build folders
//! (`node_modules`, `.next`, `dist`, …) and hidden folders, and is bounded
//! (files, bytes per file, depth), so a big repo answers quickly and a
//! hostile tree can't make the daemon read forever. What it skips is said
//! in `skipped`, never silently.

use std::fs;
use std::path::Path;

use super::{check, line_col_from, script_lang, Lang, Severity, Surface};
use crate::zen::bundle::inline_scripts;
use crate::zen::schema::Diagnostic;

/// Folders an App's own code never lives in.
pub const SKIP_DIRS: &[&str] = &[
    "node_modules", "bower_components", "vendor", "dist", "build", "out", "coverage", "target", ".next", ".nuxt",
    ".svelte-kit", ".turbo", ".vercel", ".output", ".cache", ".parcel-cache",
];
/// Code files read at most (the rest are counted in `skipped`).
pub const MAX_FILES: usize = 5000;
/// A file bigger than this is skipped (generated or bundled code).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 16;

/// What one folder check found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppReport {
    /// Files read (code files plus HTML pages).
    pub files: usize,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// One line per thing not read, and why.
    pub skipped: Vec<String>,
}

/// Check every code file under `root`. File names in findings are
/// relative to `root`, `/`-separated.
pub fn check_dir(root: &Path) -> AppReport {
    let mut r = AppReport::default();
    let mut too_many = 0usize;
    walk(root, root, 0, &mut r, &mut too_many);
    if too_many > 0 {
        r.skipped.push(format!("{too_many} more files not read: at most {MAX_FILES} files are checked"));
    }
    r.errors.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    r.warnings.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    r
}

fn walk(root: &Path, dir: &Path, depth: usize, r: &mut AppReport, too_many: &mut usize) {
    let Ok(rd) = fs::read_dir(dir) else {
        r.skipped.push(format!("{}: can't read this folder", rel(root, dir)));
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if depth >= MAX_DEPTH {
                r.skipped.push(format!("{}: deeper than {MAX_DEPTH} folders", rel(root, &path)));
                continue;
            }
            walk(root, &path, depth + 1, r, too_many);
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let html = lower.ends_with(".html") || lower.ends_with(".htm");
        let lang = Lang::for_file(&name);
        if lang.is_none() && !html {
            continue;
        }
        if r.files >= MAX_FILES {
            *too_many += 1;
            continue;
        }
        let label = rel(root, &path);
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        if size > MAX_FILE_BYTES {
            r.skipped.push(format!("{label}: {size} bytes, over the {MAX_FILE_BYTES}-byte limit (generated code?)"));
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            r.skipped.push(format!("{label}: not UTF-8 text"));
            continue;
        };
        let text = text.trim_start_matches('\u{feff}');
        r.files += 1;
        match lang {
            Some(lang) => add(r, &label, text, (1, 1), lang),
            None => {
                for s in inline_scripts(text) {
                    if s.has_src {
                        continue;
                    }
                    if let Some(lang) = script_lang(s.type_attr.as_deref()) {
                        add(r, &label, &s.body, s.base, lang);
                    }
                }
            }
        }
    }
}

fn add(r: &mut AppReport, label: &str, text: &str, base: (usize, usize), lang: Lang) {
    for f in check(text, lang, Surface::App) {
        let (line, col) = line_col_from(text, f.offset as usize, base);
        let d = Diagnostic { file: label.to_string(), line, col, message: f.message, suggestion: Some(f.suggestion) };
        match f.severity {
            Severity::Error => r.errors.push(d),
            Severity::Warning => r.warnings.push(d),
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    let s = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    if s.is_empty() { ".".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder under the OS temp dir, removed on drop.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!("k2-jscheck-{tag}-{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir_all(&d).unwrap_or_else(|e| panic!("mkdir {}: {e}", d.display()));
            Self(d)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap_or_else(|| panic!("parent of {rel}"))).unwrap_or_else(|e| panic!("mkdir {rel}: {e}"));
        fs::write(&p, text).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }

    #[test]
    fn a_folder_reports_parse_errors_leaks_and_inline_scripts_by_file() {
        let t = Scratch::new("app");
        let root = t.path();
        write(root, "server.js", "const app = require('express')()\nconst hits = []\napp.get('/', (req, res) => { hits.push(req.ip); res.end() })\n");
        write(root, "src/page.tsx", "export default function P() {\n  return <div>{</div>\n}\n");
        write(root, "public/index.html", "<!doctype html>\n<title>x</title>\n<script>\n  const a = ;\n</script>\n<script type=\"application/json\">{bad</script>\n<script src=\"app.js\">ignored(</script>\n");
        write(root, "lib/util.ts", "export const add = (a: number, b: number): number => a + b\n");
        write(root, "node_modules/x/index.js", "this is not js(\n");
        write(root, ".git/hooks/x.js", "nor this(\n");
        write(root, "dist/bundle.js", "nor this(\n");
        write(root, "README.md", "not code");
        let r = check_dir(root);
        assert_eq!(r.files, 4, "{r:#?}");
        let errs: Vec<(&str, usize)> = r.errors.iter().map(|d| (d.file.as_str(), d.line)).collect();
        assert_eq!(errs, vec![("public/index.html", 4), ("src/page.tsx", 2)], "{r:#?}");
        let warns: Vec<(&str, usize)> = r.warnings.iter().map(|d| (d.file.as_str(), d.line)).collect();
        assert_eq!(warns, vec![("server.js", 3)], "{r:#?}");
        assert!(r.warnings[0].suggestion.as_deref().is_some_and(|s| !s.is_empty()));
        assert_eq!(r.skipped, Vec::<String>::new());
    }

    #[test]
    fn symlinks_are_never_followed_and_big_files_are_skipped_out_loud() {
        let t = Scratch::new("root");
        let outside = Scratch::new("outside");
        write(outside.path(), "secret.js", "broken(\n");
        let root = t.path();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.join("linked")).unwrap_or_else(|e| panic!("symlink: {e}"));
        let big = "x\n".repeat((MAX_FILE_BYTES as usize) / 2 + 1);
        write(root, "huge.js", &big);
        let r = check_dir(root);
        assert_eq!(r.files, 0, "{r:#?}");
        assert!(r.errors.is_empty(), "{r:#?}");
        assert_eq!(r.skipped.len(), 1, "{r:#?}");
        assert!(r.skipped[0].starts_with("huge.js: "), "{r:#?}");
    }
}
