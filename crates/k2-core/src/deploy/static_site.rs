//! Static mode's rules (prd-tauri-core-0467-v1 TC-1, TC-2): what a static
//! App's manifest keys mean, with no IO beyond reading the Seed folder.
//! The daemon's file server (`k2-daemon` `static_files`) applies them; the
//! validator (`validate`) checks them.
//!
//! - `root`: a relative folder inside the Seed (no `..`, not absolute).
//! - `spa`: unknown extension-less paths answer `index.html` (default on
//!   when `<root>/index.html` exists).
//! - `notFound`: a file under `root` sent with a 404.
//! - `headers`: names from [`ALLOWED_HEADERS`] only; a CSP naming
//!   `frame-ancestors` is refused (framing is K2's).
//! - `immutable`: globs served `public, max-age=31536000, immutable`;
//!   empty = [`default_immutable`].
//! - Everything else is `no-cache` (revalidated by its ETag).

use std::path::{Path, PathBuf};

use super::manifest::Manifest;

/// Response headers a static App may set (TC-1).
pub const ALLOWED_HEADERS: &[&str] = &[
    "Content-Security-Policy",
    "Permissions-Policy",
    "Referrer-Policy",
    "Cross-Origin-Opener-Policy",
    "Cross-Origin-Embedder-Policy",
    "Cross-Origin-Resource-Policy",
    "Strict-Transport-Security",
];

/// Most `host.immutable` globs.
pub const MAX_IMMUTABLE: usize = 20;
/// Longest header value K2 passes on.
pub const MAX_HEADER_VALUE: usize = 4096;

pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
pub const CACHE_NO_CACHE: &str = "no-cache";

/// What one static Seed serves, resolved against its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// `root`, relative to the Seed (`.` by default), cleaned.
    pub root: PathBuf,
    pub spa: bool,
    /// `notFound`, relative to `root`, cleaned.
    pub not_found: Option<String>,
    /// Allowed headers as written (canonical names), in a stable order.
    pub headers: Vec<(String, String)>,
    pub immutable: Vec<String>,
}

impl Site {
    /// The site a static manifest describes, with `spa`'s default read
    /// from the Seed at `seed_dir`. Call it on a validated manifest; a key
    /// that fails its check here is dropped (validate refuses it first).
    pub fn from_manifest(m: &Manifest, seed_dir: &Path) -> Site {
        let root = m.host.root.as_deref().and_then(clean_rel).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let spa = m.host.spa.unwrap_or_else(|| seed_dir.join(&root).join("index.html").is_file());
        let not_found = m.host.not_found.as_deref().and_then(clean_rel);
        let headers = m
            .host
            .headers
            .iter()
            .filter_map(|(k, v)| canonical_header(k).filter(|_| check_header(k, v).is_ok()).map(|c| (c.to_string(), v.trim().to_string())))
            .collect();
        Site { root, spa, not_found, headers, immutable: m.host.immutable.clone() }
    }

    /// The `Cache-Control` for a file at `rel` (relative to `root`, `/`
    /// separated).
    pub fn cache_control(&self, rel: &str) -> &'static str {
        let immutable = if self.immutable.is_empty() {
            default_immutable(rel)
        } else {
            self.immutable.iter().any(|g| glob_match(g, rel))
        };
        if immutable && !no_cache_type(rel) {
            CACHE_IMMUTABLE
        } else {
            CACHE_NO_CACHE
        }
    }
}

/// HTML, JSON and web manifests are always revalidated (TC-2), even when a
/// glob names them.
fn no_cache_type(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    l.ends_with(".html") || l.ends_with(".htm") || l.ends_with(".json") || l.ends_with(".webmanifest")
}

/// The default immutable rule: a file under `assets/` whose name holds a
/// hash segment (8+ of `A-Za-z0-9_`, after the first segment, with a digit
/// or both cases), the shape Vite, Rollup and webpack write
/// (`assets/index-BX4f1a2c.js`, `assets/main.3f2a1b9c.css`).
pub fn default_immutable(rel: &str) -> bool {
    let Some(name) = rel.strip_prefix("assets/").map(|r| r.rsplit('/').next().unwrap_or(r)) else { return false };
    name.split(['-', '.'])
        .skip(1)
        .any(|seg| {
            seg.len() >= 8
                && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && (seg.bytes().any(|b| b.is_ascii_digit())
                    || (seg.bytes().any(|b| b.is_ascii_uppercase()) && seg.bytes().any(|b| b.is_ascii_lowercase())))
        })
}

/// A glob over `/`-separated paths: `*` (not `/`), `**` (anything, `/`
/// too; `**/` may match nothing), `?` (one character, not `/`).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                // `**/x` may match `x` at the top.
                if rest.first() == Some(&b'/') && go(&rest[1..], s) {
                    return true;
                }
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            Some(b'*') => {
                let rest = &p[1..];
                for i in 0..=s.len() {
                    if go(rest, &s[i..]) {
                        return true;
                    }
                    if s.get(i) == Some(&b'/') {
                        break;
                    }
                }
                false
            }
            Some(b'?') => s.first().is_some_and(|c| *c != b'/') && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    go(pattern.as_bytes(), path.as_bytes())
}

/// Is `g` a glob K2 reads (relative, no `..`, printable, 1–200 chars)?
pub fn glob_ok(g: &str) -> bool {
    (1..=200).contains(&g.len())
        && !g.starts_with('/')
        && !g.split('/').any(|s| s == "..")
        && g.bytes().all(|b| b.is_ascii_graphic())
        && !g.contains('\\')
}

/// The canonical spelling of an allowed header name (case-insensitive).
pub fn canonical_header(name: &str) -> Option<&'static str> {
    ALLOWED_HEADERS.iter().copied().find(|h| h.eq_ignore_ascii_case(name.trim()))
}

/// Check one `host.headers` entry: an allowed name, a value K2 can put on
/// one header line, and no `frame-ancestors` in a CSP.
pub fn check_header(name: &str, value: &str) -> Result<(), String> {
    let Some(canon) = canonical_header(name) else {
        return Err(format!("host.headers: \"{name}\" isn't one K2 lets an App set (allowed: {})", ALLOWED_HEADERS.join(", ")));
    };
    let v = value.trim();
    if v.is_empty() || v.len() > MAX_HEADER_VALUE || v.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(format!("host.headers.{canon}: one line of text, 1 to {MAX_HEADER_VALUE} characters"));
    }
    if canon == "Content-Security-Policy" && crate::frame_policy::csp_has_frame_ancestors(v) {
        return Err(
            "host.headers.Content-Security-Policy names frame-ancestors: framing is K2's (the App's frame policy, k2 publish frame <app>); \
             drop frame-ancestors from the CSP"
                .into(),
        );
    }
    Ok(())
}

/// A relative path inside the Seed, cleaned: `/` separators, no `.`
/// segments, never absolute, never `..`, no NUL or backslash. `.` and an
/// empty path are the folder itself (`Some(".")`).
pub fn clean_rel(raw: &str) -> Option<String> {
    let t = raw.trim();
    // A drive letter (`C:/x`) is absolute on Windows.
    if t.starts_with('/') || t.contains('\0') || t.contains('\\') || t.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in t.split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            s => out.push(s),
        }
    }
    Some(if out.is_empty() { ".".to_string() } else { out.join("/") })
}

/// A request path (before the query) as a path under `root`: percent-
/// decoded once, `/` separated, `None` for anything that could leave the
/// folder (`..`, an encoded `..` or `/`, NUL, a backslash, bad UTF-8).
/// `""` = the folder itself. A trailing `/` is kept.
pub fn request_rel(path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut dec = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let h = bytes.get(i + 1..i + 3)?;
            let v = u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?;
            // An encoded separator is never a separator (no `%2e%2e%2f`).
            if v == b'/' || v == b'\\' || v == 0 {
                return None;
            }
            dec.push(v);
            i += 3;
        } else {
            dec.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(dec).ok()?;
    if s.contains('\0') || s.contains('\\') {
        return None;
    }
    let trailing = s.ends_with('/') && s.len() > 1;
    let mut out: Vec<&str> = Vec::new();
    for seg in s.split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            x => out.push(x),
        }
    }
    let mut r = out.join("/");
    if trailing && !r.is_empty() {
        r.push('/');
    }
    Some(r)
}

/// Does the last segment of `rel` have a file extension?
pub fn has_extension(rel: &str) -> bool {
    let name = rel.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    name.rfind('.').is_some_and(|i| i > 0 && i + 1 < name.len())
}

/// The static-mode checks against the Seed folder (`validate`): `root` is a
/// folder in it, `notFound` a file under `root`, headers allowed, globs
/// readable, `start` empty. `(code, message, key)` per problem.
pub fn check_tree(m: &Manifest, seed_dir: &Path) -> Vec<(&'static str, String, &'static str)> {
    let mut out = Vec::new();
    let h = &m.host;
    if !h.start.is_empty() {
        out.push(("manifest_invalid", "host.start: a static App runs no server; leave start out (K2 serves the files)".to_string(), "host.start"));
    }
    let root = match h.root.as_deref() {
        None => Some(".".to_string()),
        Some(r) => match clean_rel(r) {
            Some(c) => Some(c),
            None => {
                out.push(("manifest_invalid", format!("host.root \"{r}\": a folder inside the Seed, like dist (no .., not absolute)"), "host.root"));
                None
            }
        },
    };
    let root_dir = root.as_ref().map(|r| seed_dir.join(r));
    if let (Some(r), Some(d)) = (&root, &root_dir) {
        if !d.is_dir() {
            out.push(("static_root_missing", format!("host.root \"{r}\" isn't a folder in this Seed (build first, then deploy the build output)"), "host.root"));
        } else if !inside(seed_dir, d) {
            out.push(("manifest_invalid", format!("host.root \"{r}\" leaves the Seed (a link out)"), "host.root"));
        }
    }
    if let Some(nf) = h.not_found.as_deref() {
        // Under a missing root the root's own problem is the one to fix.
        match (clean_rel(nf), root_dir.as_ref().filter(|d| d.is_dir())) {
            (None, _) => out.push(("manifest_invalid", format!("host.notFound \"{nf}\": a file under host.root, like 404.html"), "host.notFound")),
            (Some(c), Some(d)) => {
                let f = d.join(&c);
                if !f.is_file() || !inside(d, &f) {
                    out.push(("manifest_invalid", format!("host.notFound \"{nf}\" isn't a file under host.root"), "host.notFound"));
                }
            }
            (Some(_), None) => {}
        }
    }
    for (k, v) in &h.headers {
        if let Err(e) = check_header(k, v) {
            out.push(("manifest_invalid", e, "host.headers"));
        }
    }
    if h.immutable.len() > MAX_IMMUTABLE {
        out.push(("manifest_invalid", format!("host.immutable: at most {MAX_IMMUTABLE} globs"), "host.immutable"));
    }
    for g in &h.immutable {
        if !glob_ok(g) {
            out.push(("manifest_invalid", format!("host.immutable \"{g}\": a relative glob like assets/** (no .., no leading /)"), "host.immutable"));
        }
    }
    out
}

/// Is `p` (after links) under `dir` (after links)?
fn inside(dir: &Path, p: &Path) -> bool {
    match (dunce::canonicalize(dir), dunce::canonicalize(p)) {
        (Ok(d), Ok(x)) => x.starts_with(d),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_like_gitignore_paths() {
        assert!(glob_match("assets/**", "assets/a/b.js"));
        assert!(glob_match("assets/*.js", "assets/x.js"));
        assert!(!glob_match("assets/*.js", "assets/a/x.js"), "* never crosses /");
        assert!(glob_match("**/*.woff2", "fonts/a.woff2"));
        assert!(glob_match("**/*.woff2", "a.woff2"), "**/ may match nothing");
        assert!(glob_match("img/?.png", "img/a.png"));
        assert!(!glob_match("img/?.png", "img/ab.png"));
        assert!(!glob_match("assets/**", "other/assets/x.js"));
        assert!(glob_ok("assets/**") && !glob_ok("/assets/**") && !glob_ok("../x") && !glob_ok(""));
    }

    #[test]
    fn the_default_immutable_rule_is_hashed_assets_only() {
        assert!(default_immutable("assets/index-BX4f1a2c.js"));
        assert!(default_immutable("assets/main.3f2a1b9c.css"));
        assert!(default_immutable("assets/chunks/vendor-DfGhJkLm.js"), "mixed case is a hash");
        assert!(!default_immutable("assets/application.js"), "a word is not a hash");
        assert!(!default_immutable("assets/MyComponent.js"), "the first segment is the name");
        assert!(!default_immutable("index-BX4f1a2c.js"), "only under assets/");
        assert!(!default_immutable("assets/logo-abc.svg"), "short");
    }

    #[test]
    fn cache_control_follows_the_rule_and_never_pins_html() {
        let s = Site { root: ".".into(), spa: true, not_found: None, headers: vec![], immutable: vec![] };
        assert_eq!(s.cache_control("assets/index-BX4f1a2c.js"), CACHE_IMMUTABLE);
        assert_eq!(s.cache_control("index.html"), CACHE_NO_CACHE);
        assert_eq!(s.cache_control("manifest.webmanifest"), CACHE_NO_CACHE);
        assert_eq!(s.cache_control("logo.png"), CACHE_NO_CACHE);
        let g = Site { immutable: vec!["img/**".into(), "**/*.json".into()], ..s };
        assert_eq!(g.cache_control("img/a/logo.png"), CACHE_IMMUTABLE);
        assert_eq!(g.cache_control("assets/index-BX4f1a2c.js"), CACHE_NO_CACHE, "a glob list replaces the default");
        assert_eq!(g.cache_control("data/x.json"), CACHE_NO_CACHE, "JSON is always revalidated");
    }

    #[test]
    fn headers_are_allow_listed_and_framing_stays_k2s() {
        assert!(check_header("content-security-policy", "default-src 'self'").is_ok());
        assert_eq!(canonical_header("referrer-policy"), Some("Referrer-Policy"));
        let e = check_header("X-Frame-Options", "DENY").unwrap_err();
        assert!(e.contains("isn't one K2 lets an App set"), "{e}");
        let e = check_header("Content-Security-Policy", "default-src 'self'; frame-ancestors *").unwrap_err();
        assert!(e.contains("frame-ancestors"), "{e}");
        assert!(check_header("Referrer-Policy", "no-referrer\r\nSet-Cookie: x=1").is_err(), "one line only");
        assert!(check_header("Referrer-Policy", " ").is_err());
    }

    #[test]
    fn request_paths_never_leave_the_folder() {
        assert_eq!(request_rel("/").as_deref(), Some(""));
        assert_eq!(request_rel("/a/b.js").as_deref(), Some("a/b.js"));
        assert_eq!(request_rel("/docs/").as_deref(), Some("docs/"));
        assert_eq!(request_rel("/a//./b").as_deref(), Some("a/b"));
        assert_eq!(request_rel("/hello%20world.txt").as_deref(), Some("hello world.txt"));
        assert_eq!(request_rel("/../etc/passwd"), None);
        assert_eq!(request_rel("/a/../../x"), None);
        assert_eq!(request_rel("/%2e%2e/x"), None, "encoded .. is still ..");
        assert_eq!(request_rel("/a%2f..%2fx"), None, "an encoded / is refused");
        assert_eq!(request_rel("/a%00.js"), None, "NUL");
        assert_eq!(request_rel("/a\\..\\x"), None, "backslash");
        assert_eq!(request_rel("/%zz"), None, "bad escape");
        assert_eq!(request_rel("/%ff"), None, "bad UTF-8");
        assert_eq!(request_rel("relative"), None);
        assert!(has_extension("a/b.js") && !has_extension("a/b") && !has_extension("a.b/c") && !has_extension(".env"));
    }

    #[test]
    fn rel_paths_are_cleaned_and_jailed() {
        assert_eq!(clean_rel("dist").as_deref(), Some("dist"));
        assert_eq!(clean_rel("./dist/").as_deref(), Some("dist"));
        assert_eq!(clean_rel(".").as_deref(), Some("."));
        assert_eq!(clean_rel(""), Some(".".into()));
        assert_eq!(clean_rel("../x"), None);
        assert_eq!(clean_rel("/abs"), None);
        assert_eq!(clean_rel("C:/x"), None);
    }

    fn manifest(host: &str) -> Manifest {
        super::super::manifest::parse(&format!(
            r#"{{"schema":1,"kind":"app","id":"docs","name":"Docs","version":"1","host":{host}}}"#
        ))
        .expect("parse")
    }

    /// A temp folder removed on drop.
    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Self {
            let p = crate::test_env::unique_temp_path(tag);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_tree_check_reads_the_seed() {
        let d = Dir::new("static-site");
        std::fs::create_dir_all(d.path().join("dist/assets")).unwrap();
        std::fs::write(d.path().join("dist/index.html"), "<p>hi</p>").unwrap();
        std::fs::write(d.path().join("dist/404.html"), "gone").unwrap();
        let ok = manifest(r#"{"mode":"static","root":"dist","notFound":"404.html","headers":{"Referrer-Policy":"no-referrer"}}"#);
        assert!(check_tree(&ok, d.path()).is_empty(), "{:?}", check_tree(&ok, d.path()));
        let site = Site::from_manifest(&ok, d.path());
        assert!(site.spa, "index.html under root turns spa on by default");
        assert_eq!(site.root, PathBuf::from("dist"));
        assert_eq!(site.headers, vec![("Referrer-Policy".to_string(), "no-referrer".to_string())]);
        let no_index = manifest(r#"{"mode":"static"}"#);
        assert!(!Site::from_manifest(&no_index, d.path()).spa, "no index.html at the Seed root: spa off");

        let bad = manifest(
            r#"{"mode":"static","root":"build","notFound":"missing.html","start":["node"],"headers":{"X-Powered-By":"me"},"immutable":["../x"]}"#,
        );
        let codes: Vec<&str> = check_tree(&bad, d.path()).iter().map(|p| p.2).collect();
        assert_eq!(codes, vec!["host.start", "host.root", "host.headers", "host.immutable"], "notFound isn't checked under a missing root");
        let bad2 = manifest(r#"{"mode":"static","root":"dist","notFound":"nope.html"}"#);
        assert_eq!(check_tree(&bad2, d.path()).iter().map(|p| p.2).collect::<Vec<_>>(), vec!["host.notFound"]);
        assert_eq!(check_tree(&bad, d.path())[1].0, "static_root_missing");
        #[cfg(unix)]
        {
            let out = Dir::new("static-site-out");
            std::os::unix::fs::symlink(out.path(), d.path().join("out")).unwrap();
            let link = manifest(r#"{"mode":"static","root":"out"}"#);
            let p = check_tree(&link, d.path());
            assert_eq!(p.len(), 1, "{p:?}");
            assert!(p[0].1.contains("leaves the Seed"), "{p:?}");
        }
    }
}
