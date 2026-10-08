//! Source walk (prd-app-frame-ancestors-v1 §5 test 4, FA6): every raw
//! `"HTTP/1.1 …"` response the daemon, the App gateway helper or the
//! legacy companion writes carries the framing headers, or is on the
//! named exempt list below. A new response writer without them fails the
//! build's tests.
//!
//! "Carries the headers" means the response statement mentions one of
//! [`FRAME_MARKERS`]: `k2_core::frame_policy::…` (the daemon's
//! `CONNECT_HEADER_LINES`), the helper's per-App `frame_lines`, or a
//! literal `X-Frame-Options` (the companion's fixed `DENY`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Any of these inside the response statement counts as "framed".
const FRAME_MARKERS: &[&str] = &["frame_policy::", "frame_lines", "X-Frame-Options"];

/// Writers that are not documents a browser could frame. Keyed by file
/// (relative to the crate `src/`) and enclosing function name.
const EXEMPT: &[(&str, &str, &str)] = &[
    (
        "k2-daemon/src/routes/http.rs",
        "send_cors_preflight",
        "204 CORS preflight, no body",
    ),
    (
        "k2-daemon/src/wiki_routes.rs",
        "write_site_options",
        "204 CORS preflight, no body",
    ),
    (
        "k2-daemon/src/cell_server.rs",
        "write_response",
        "UDS / vsock cell API, never reached by a browser",
    ),
    (
        "k2-daemon/src/domains/acme.rs",
        "spawn",
        "Http01Guard::spawn: ACME HTTP-01 token for the CA, text/plain",
    ),
];

/// Drop `#[cfg(test)]` + `mod x {` blocks (rustfmt shape: the block
/// closes with a line equal to the attribute's indent + `}`), keeping
/// line numbers stable. Same rule as `route_policy`'s walk.
fn strip_test_modules(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let next_is_mod = lines.get(i + 1).is_some_and(|n| {
            let t = n.trim_start();
            let t = t
                .strip_prefix("pub(crate) ")
                .or_else(|| t.strip_prefix("pub "))
                .unwrap_or(t);
            t.starts_with("mod ") && t.trim_end().ends_with('{')
        });
        if l.trim() == "#[cfg(test)]" && next_is_mod {
            let indent = &l[..l.len() - l.trim_start().len()];
            let close = format!("{indent}}}");
            let mut j = i + 2;
            while j < lines.len() && lines[j] != close {
                j += 1;
            }
            for _ in i..=j.min(lines.len() - 1) {
                out.push(String::new());
            }
            i = j + 1;
            continue;
        }
        out.push(l.to_string());
        i += 1;
    }
    out
}

/// One raw response statement: where it starts, its enclosing fn, and
/// whether it carries a frame marker.
#[derive(Debug)]
struct Writer {
    line: usize,
    func: String,
    framed: bool,
}

/// Every `"HTTP/1.1 ` / `b"HTTP/1.1 ` string literal in code position.
/// The statement runs from that line to the first line ending in `;`
/// (at most 25 lines). The enclosing fn is the nearest `fn name` above.
fn writers(src: &str) -> Vec<Writer> {
    let lines = strip_test_modules(src);
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        let Some(pos) = line.find("\"HTTP/1.1 ") else {
            continue;
        };
        if let Some(c) = line.find("//") {
            if c < pos {
                continue;
            }
        }
        // Response literals only: a status code follows.
        let after = &line[pos + "\"HTTP/1.1 ".len()..];
        if !(after.starts_with('{') || after.as_bytes().first().is_some_and(u8::is_ascii_digit)) {
            continue;
        }
        let mut stmt = String::new();
        for l in lines.iter().skip(n).take(25) {
            stmt.push_str(l);
            stmt.push('\n');
            if l.trim_end().ends_with(';') {
                break;
            }
        }
        let func = lines[..=n]
            .iter()
            .rev()
            .find_map(|l| {
                let t = l.trim_start();
                let idx = t.find("fn ")?;
                let before = &t[..idx];
                let is_decl = before.is_empty()
                    || before
                        .split_whitespace()
                        .all(|w| matches!(w, "pub" | "pub(crate)" | "async" | "unsafe" | "const"));
                if !is_decl {
                    return None;
                }
                let name: String = t[idx + 3..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .unwrap_or_default();
        out.push(Writer {
            line: n + 1,
            func,
            framed: FRAME_MARKERS.iter().any(|m| stmt.contains(m)),
        });
    }
    out
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let entry = entry.expect("dir entry");
        let p = entry.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Files that are only compiled for tests: `#[cfg(test)] mod x;` in a
/// parent names them.
fn test_only_files(files: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for f in files {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        let lines: Vec<&str> = src.lines().collect();
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let parent = f.parent().expect("parent");
        let mod_dir = if matches!(stem, "mod" | "lib" | "main") {
            parent.to_path_buf()
        } else {
            parent.join(stem)
        };
        for (i, l) in lines.iter().enumerate() {
            if l.trim() != "#[cfg(test)]" {
                continue;
            }
            let Some(next) = lines.get(i + 1) else { continue };
            let t = next.trim();
            let t = t
                .strip_prefix("pub(crate) ")
                .or_else(|| t.strip_prefix("pub "))
                .unwrap_or(t);
            let Some(name) = t.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) else {
                continue;
            };
            out.insert(mod_dir.join(format!("{name}.rs")));
            out.insert(mod_dir.join(name).join("mod.rs"));
        }
    }
    out
}

/// `k2-daemon/src/**` plus `k2-core/src/companion/**` (FA6), minus
/// test-only files.
fn walked_files() -> Vec<(String, PathBuf)> {
    let daemon = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = daemon.parent().expect("crates dir");
    let mut files = Vec::new();
    rs_files(&daemon.join("src"), &mut files);
    rs_files(&crates.join("k2-core").join("src").join("companion"), &mut files);
    files.sort();
    let test_only = test_only_files(&files);
    files
        .into_iter()
        .filter(|f| !test_only.contains(f))
        .map(|f| {
            let rel = f
                .strip_prefix(crates)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            (rel, f)
        })
        .collect()
}

#[test]
fn every_raw_http_response_carries_the_frame_headers() {
    let files = walked_files();
    assert!(files.len() > 150, "walk found too few files: {}", files.len());
    let mut seen = 0usize;
    let mut missing = Vec::new();
    let mut exempt_hit: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (rel, path) in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        for w in writers(&src) {
            seen += 1;
            if w.framed {
                continue;
            }
            if let Some((f, func, _)) = EXEMPT.iter().find(|(f, func, _)| f == rel && *func == w.func) {
                exempt_hit.insert((f, func));
                continue;
            }
            missing.push(format!("{rel}:{} (fn {})", w.line, w.func));
        }
    }
    assert!(seen >= 25, "walk saw only {seen} response literals — extractor broken?");
    assert!(
        missing.is_empty(),
        "{} raw HTTP response(s) without the frame headers. Add \
         k2_core::frame_policy::CONNECT_HEADER_LINES (daemon) or the helper's \
         frame_lines, or list the writer in routes/frame_walk.rs EXEMPT with a reason:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
    // A stale exemption hides nothing but must not linger.
    let stale: Vec<_> = EXEMPT
        .iter()
        .filter(|(f, func, _)| !exempt_hit.contains(&(*f, *func)))
        .map(|(f, func, why)| format!("{f} fn {func} ({why})"))
        .collect();
    assert!(stale.is_empty(), "EXEMPT rows that match no writer:\n  {}", stale.join("\n  "));
}

#[test]
fn walk_extractor_sees_writers_skips_tests_and_comments() {
    let src = r#"
async fn good(s: &mut S) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{frame}\r\n",
        n,
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    );
}
pub(crate) async fn bad(s: &mut S) {
    let resp = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"
    );
}
fn bytes() {
    s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
}
fn not_a_response() {
    assert!(greeting_ok("HTTP/1.1 400 Bad Request").is_err()); // has a code
    // "HTTP/1.1 500 x" in a comment
    let note = "HTTP/1.1 keep-alive";
}
#[cfg(test)]
mod tests {
    fn stub() {
        let r = "HTTP/1.1 200 OK\r\n\r\n";
    }
}
"#;
    let ws = writers(src);
    let got: Vec<(String, bool)> = ws.iter().map(|w| (w.func.clone(), w.framed)).collect();
    assert_eq!(
        got,
        vec![
            ("good".to_string(), true),
            ("bad".to_string(), false),
            ("bytes".to_string(), false),
            ("not_a_response".to_string(), false),
        ],
        "{ws:?}"
    );
}
