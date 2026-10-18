//! Thread file links (Rosson 2026-10-18): which paths printed in a
//! Thread message lead to something inside the message's workspace.
//!
//! The renderer finds path-shaped text (`src/foo.ts:42`, an absolute
//! path, `~/…`) and asks the daemon, in one batch, which of them it may
//! turn into a link. The daemon decides: each candidate is resolved
//! against the workspace root, symlinks are followed (realpath), and only
//! a file or folder whose real path is under the workspace's real root is
//! linkable. Everything else (missing, outside, a symlink that escapes,
//! a `..` walk out, a socket or device) answers the same bare
//! "not linkable", so a caller learns nothing about paths outside the
//! workspace.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Most candidates one request may carry; the rest answer not linkable.
pub const MAX_CANDIDATES: usize = 200;
/// Longest candidate looked at (bytes); longer ones answer not linkable.
pub const MAX_CANDIDATE_LEN: usize = 4096;

/// One candidate's answer. `linkable: false` carries nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkAnswer {
    /// The candidate as sent.
    pub input: String,
    pub linkable: bool,
    /// `file` or `directory` (only when linkable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    /// The path relative to the workspace root, `/`-separated, `""` for
    /// the root itself (only when linkable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    /// The absolute path to open: the workspace root AS REGISTERED joined
    /// with `rel`, so it matches the paths the Files drawer and file tabs
    /// use (only when linkable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl LinkAnswer {
    fn refused(input: &str) -> Self {
        Self { input: input.to_string(), linkable: false, kind: None, rel: None, path: None }
    }
}

/// Resolve one candidate against `root` (the workspace path as
/// registered). `home` expands a leading `~/`; with no home a `~` path is
/// not linkable. A relative candidate is taken from the root.
pub fn resolve_link(root: &Path, home: Option<&Path>, input: &str) -> LinkAnswer {
    let raw = input.trim();
    if raw.is_empty() || raw.len() > MAX_CANDIDATE_LEN || raw.contains('\0') {
        return LinkAnswer::refused(input);
    }
    let joined: PathBuf = if raw == "~" || raw.starts_with("~/") {
        match home {
            Some(h) if h.is_absolute() => h.join(raw.trim_start_matches('~').trim_start_matches('/')),
            _ => return LinkAnswer::refused(input),
        }
    } else if raw.starts_with('~') {
        // `~user/…`: never expanded.
        return LinkAnswer::refused(input);
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let Ok(real_root) = root.canonicalize() else {
        return LinkAnswer::refused(input);
    };
    // realpath: follows every symlink and `..`; a missing path fails here.
    let Ok(real) = joined.canonicalize() else {
        return LinkAnswer::refused(input);
    };
    let Ok(rel) = real.strip_prefix(&real_root) else {
        return LinkAnswer::refused(input);
    };
    // Belt: a canonical path has no `..`, but never answer one that does.
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return LinkAnswer::refused(input);
    }
    let Ok(meta) = std::fs::metadata(&real) else {
        return LinkAnswer::refused(input);
    };
    let kind = if meta.is_file() {
        "file"
    } else if meta.is_dir() {
        "directory"
    } else {
        return LinkAnswer::refused(input);
    };
    let rel_str = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let open_path = if rel_str.is_empty() { root.to_path_buf() } else { root.join(&rel_str) };
    LinkAnswer {
        input: input.to_string(),
        linkable: true,
        kind: Some(kind),
        rel: Some(rel_str),
        path: Some(open_path.to_string_lossy().into_owned()),
    }
}

/// Resolve a batch. Past [`MAX_CANDIDATES`] every candidate answers not
/// linkable (the order and length of the answers always match the input).
pub fn resolve_links(root: &Path, home: Option<&Path>, inputs: &[String]) -> Vec<LinkAnswer> {
    inputs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i >= MAX_CANDIDATES {
                LinkAnswer::refused(p)
            } else {
                resolve_link(root, home, p)
            }
        })
        .collect()
}

/// The prefixes the renderer may use to TOKENIZE a path with spaces in
/// it (the workspace root, and its `~/` form when it sits under home).
/// Tokenizing only: every candidate still comes back here to be checked.
pub fn root_prefixes(root: &Path, home: Option<&Path>) -> Vec<String> {
    let mut out = vec![root.to_string_lossy().into_owned()];
    if let Some(h) = home {
        if let Ok(rest) = root.strip_prefix(h) {
            let rest = rest.to_string_lossy();
            if !rest.is_empty() {
                out.push(format!("~/{rest}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir().join(format!(
                "k2-file-links-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// `<tmp>/ws` (the workspace: src/a.ts, docs/) and `<tmp>/outside/secret.txt`.
    fn layout(label: &str) -> (Tmp, PathBuf, PathBuf) {
        let t = Tmp::new(label);
        let ws = t.0.join("ws");
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::create_dir_all(ws.join("docs")).unwrap();
        fs::write(ws.join("src/a.ts"), "x").unwrap();
        let outside = t.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "s").unwrap();
        (t, ws, outside)
    }

    fn assert_refused(a: &LinkAnswer) {
        assert!(!a.linkable, "expected not linkable: {a:?}");
        assert_eq!(a.kind, None, "a refusal names no kind: {a:?}");
        assert_eq!(a.rel, None, "a refusal names no rel: {a:?}");
        assert_eq!(a.path, None, "a refusal names no path: {a:?}");
    }

    #[test]
    fn relative_file_inside_is_linkable() {
        let (_t, ws, _) = layout("rel");
        let a = resolve_link(&ws, None, "src/a.ts");
        assert!(a.linkable);
        assert_eq!(a.kind, Some("file"));
        assert_eq!(a.rel.as_deref(), Some("src/a.ts"));
        assert_eq!(a.path.as_deref(), Some(ws.join("src/a.ts").to_str().unwrap()));
    }

    #[test]
    fn absolute_file_inside_is_linkable() {
        let (_t, ws, _) = layout("abs");
        let abs = ws.join("src/a.ts");
        let a = resolve_link(&ws, None, abs.to_str().unwrap());
        assert!(a.linkable);
        assert_eq!(a.rel.as_deref(), Some("src/a.ts"));
    }

    #[test]
    fn folder_inside_is_a_directory() {
        let (_t, ws, _) = layout("dir");
        let a = resolve_link(&ws, None, "docs/");
        assert!(a.linkable);
        assert_eq!(a.kind, Some("directory"));
        assert_eq!(a.rel.as_deref(), Some("docs"));
    }

    #[test]
    fn missing_is_refused() {
        let (_t, ws, _) = layout("missing");
        assert_refused(&resolve_link(&ws, None, "src/nope.ts"));
    }

    #[test]
    fn outside_absolute_is_refused_even_though_it_exists() {
        let (_t, ws, outside) = layout("outside");
        assert_refused(&resolve_link(&ws, None, outside.join("secret.txt").to_str().unwrap()));
    }

    #[test]
    fn dotdot_walk_out_is_refused() {
        let (_t, ws, _) = layout("dotdot");
        assert_refused(&resolve_link(&ws, None, "../outside/secret.txt"));
        assert_refused(&resolve_link(&ws, None, "src/../../outside/secret.txt"));
    }

    #[test]
    fn dotdot_that_stays_inside_is_linkable() {
        let (_t, ws, _) = layout("dotdot-in");
        let a = resolve_link(&ws, None, "docs/../src/a.ts");
        assert!(a.linkable);
        assert_eq!(a.rel.as_deref(), Some("src/a.ts"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_workspace_is_refused() {
        let (_t, ws, outside) = layout("symlink-out");
        std::os::unix::fs::symlink(outside.join("secret.txt"), ws.join("leak.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leakdir")).unwrap();
        assert_refused(&resolve_link(&ws, None, "leak.txt"));
        assert_refused(&resolve_link(&ws, None, "leakdir/secret.txt"));
        assert_refused(&resolve_link(&ws, None, "leakdir"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_the_workspace_is_linkable_at_its_target() {
        let (_t, ws, _) = layout("symlink-in");
        std::os::unix::fs::symlink(ws.join("src/a.ts"), ws.join("alias.ts")).unwrap();
        let a = resolve_link(&ws, None, "alias.ts");
        assert!(a.linkable);
        assert_eq!(a.rel.as_deref(), Some("src/a.ts"));
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_reached_through_a_symlinked_root_still_matches() {
        // The registered root is a symlink to the real folder (macOS
        // /var → /private/var is the everyday case).
        let (t, ws, _) = layout("root-link");
        let link_root = t.0.join("ws-link");
        std::os::unix::fs::symlink(&ws, &link_root).unwrap();
        let a = resolve_link(&link_root, None, ws.join("src/a.ts").to_str().unwrap());
        assert!(a.linkable);
        assert_eq!(a.rel.as_deref(), Some("src/a.ts"));
        // The path to open is under the root AS REGISTERED.
        assert_eq!(a.path.as_deref(), Some(link_root.join("src/a.ts").to_str().unwrap()));
    }

    #[test]
    fn tilde_expands_against_home() {
        let (t, ws, _) = layout("tilde");
        let a = resolve_link(&ws, Some(&t.0), "~/ws/src/a.ts");
        assert!(a.linkable);
        assert_eq!(a.rel.as_deref(), Some("src/a.ts"));
        assert_refused(&resolve_link(&ws, Some(&t.0), "~/outside/secret.txt"));
        assert_refused(&resolve_link(&ws, None, "~/ws/src/a.ts"));
        assert_refused(&resolve_link(&ws, Some(&t.0), "~root/ws/src/a.ts"));
    }

    #[test]
    fn junk_is_refused() {
        let (_t, ws, _) = layout("junk");
        assert_refused(&resolve_link(&ws, None, ""));
        assert_refused(&resolve_link(&ws, None, "   "));
        assert_refused(&resolve_link(&ws, None, "src/a.ts\0x"));
        assert_refused(&resolve_link(&ws, None, &"a/".repeat(3000)));
    }

    #[test]
    fn a_missing_root_refuses_everything() {
        let (t, _ws, _) = layout("noroot");
        assert_refused(&resolve_link(&t.0.join("gone"), None, "src/a.ts"));
    }

    #[test]
    fn batch_keeps_order_and_caps() {
        let (_t, ws, _) = layout("batch");
        let mut inputs = vec!["src/a.ts".to_string(), "nope".to_string()];
        let out = resolve_links(&ws, None, &inputs);
        assert_eq!(out.len(), 2);
        assert!(out[0].linkable);
        assert_refused(&out[1]);
        inputs = (0..MAX_CANDIDATES + 3).map(|_| "src/a.ts".to_string()).collect();
        let out = resolve_links(&ws, None, &inputs);
        assert_eq!(out.len(), MAX_CANDIDATES + 3);
        assert!(out[MAX_CANDIDATES - 1].linkable);
        assert_refused(&out[MAX_CANDIDATES]);
    }

    #[test]
    fn root_prefixes_include_the_tilde_form_under_home() {
        let home = Path::new("/Users/me");
        assert_eq!(
            root_prefixes(Path::new("/Users/me/Dev Projects/K2"), Some(home)),
            vec!["/Users/me/Dev Projects/K2".to_string(), "~/Dev Projects/K2".to_string()]
        );
        assert_eq!(root_prefixes(Path::new("/srv/ws"), Some(home)), vec!["/srv/ws".to_string()]);
    }
}
