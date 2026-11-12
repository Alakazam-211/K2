//! The rule for the `fonts/manifest.tsv` paths that build.rs compiles in (`compiled_in` =
//! `k2-deckrender`): relative to the crate, under `fonts/`, no root and no `..`, and still under
//! `fonts/` once symlinks are resolved. So no row can compile in a file from elsewhere in the
//! tree (deckcraft's CC0 test fixtures, say). build.rs includes this file (`#[path]`) and the
//! lib's unit tests test it, so the test checks the code the build runs.

use std::path::{Component, Path, PathBuf};

/// The file build.rs compiles in for the manifest path `file` (relative to `crate_dir`), or why
/// it may not.
pub fn compiled_in_font_path(crate_dir: &Path, file: &str) -> Result<PathBuf, String> {
    let rel = Path::new(file);
    let mut parts = rel.components();
    match parts.next() {
        Some(Component::Normal(first)) if first == "fonts" => {}
        Some(Component::RootDir | Component::Prefix(_)) => {
            return Err(format!("{file}: an absolute path; a font k2-deckrender compiles in is a path under fonts/"));
        }
        _ => return Err(format!("{file}: not under fonts/; a font k2-deckrender compiles in is a path under fonts/")),
    }
    for part in parts {
        match part {
            Component::Normal(_) => {}
            Component::ParentDir => return Err(format!("{file}: a `..` component; a font k2-deckrender compiles in stays under fonts/")),
            _ => return Err(format!("{file}: only plain names after fonts/ (no `.`, root or prefix)")),
        }
    }
    let fonts_dir = crate_dir.join("fonts").canonicalize().map_err(|e| format!("{}: {e}", crate_dir.join("fonts").display()))?;
    let path = crate_dir.join(rel).canonicalize().map_err(|e| format!("{file}: {e}"))?;
    if !path.starts_with(&fonts_dir) {
        return Err(format!("{file}: resolves to {}, outside {} (a symlink?)", path.display(), fonts_dir.display()));
    }
    if !path.is_file() {
        return Err(format!("{file}: not a file"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::compiled_in_font_path;
    use std::path::{Path, PathBuf};

    /// `<tmp>/crate/{fonts/fam/a.ttf, src/lib.rs}` and `<tmp>/vendor/x.ttf`; removed on drop.
    struct Tree(PathBuf);
    impl Tree {
        fn new(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("k2-deckrender-font-path-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for (file, body) in [("crate/fonts/fam/a.ttf", "font"), ("crate/src/lib.rs", "code"), ("vendor/x.ttf", "fixture")] {
                let p = root.join(file);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            }
            Tree(root)
        }
        fn krate(&self) -> PathBuf {
            self.0.join("crate")
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn err(krate: &Path, file: &str) -> String {
        compiled_in_font_path(krate, file).expect_err(file)
    }

    #[test]
    fn a_plain_path_under_fonts_resolves() {
        let t = Tree::new("ok");
        let got = compiled_in_font_path(&t.krate(), "fonts/fam/a.ttf").unwrap();
        assert_eq!(got, t.krate().join("fonts/fam/a.ttf").canonicalize().unwrap());
    }

    #[test]
    fn parent_components_absolute_paths_and_other_folders_are_refused() {
        let t = Tree::new("refused");
        let k = t.krate();
        assert!(err(&k, "../vendor/x.ttf").contains("not under fonts/"));
        assert!(err(&k, "fonts/../../vendor/x.ttf").contains("`..`"));
        assert!(err(&k, "fonts/fam/../../src/lib.rs").contains("`..`"));
        let abs = k.join("fonts/fam/a.ttf").canonicalize().unwrap();
        assert!(err(&k, abs.to_str().unwrap()).contains("absolute"));
        assert!(err(&k, "src/lib.rs").contains("not under fonts/"));
        assert!(err(&k, "./fonts/fam/a.ttf").contains("not under fonts/"));
        assert!(compiled_in_font_path(&k, "fonts/fam/missing.ttf").is_err());
        assert!(err(&k, "fonts/fam").contains("not a file"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_fonts_is_refused() {
        let t = Tree::new("symlink");
        let k = t.krate();
        std::os::unix::fs::symlink(t.0.join("vendor/x.ttf"), k.join("fonts/fam/link.ttf")).unwrap();
        assert!(err(&k, "fonts/fam/link.ttf").contains("outside"));
    }
}
