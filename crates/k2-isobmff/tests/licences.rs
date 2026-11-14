//! The shipped-tree check for `k2-isobmff`, run with every `cargo test`:
//! for each target K2 builds, the crate has no dependencies of any kind
//! (normal, build or dev), so its whole shipped tree is itself, MIT, from
//! a path, never git. `deny.toml` (cargo deny) checks the same with the
//! advisories. `cargo metadata` is read with a small JSON reader below, so
//! even this test adds no crate.

use std::collections::BTreeMap;
use std::process::Command;

const ALLOWED: &[&str] = &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0"];
const TARGETS: &[&str] = &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc"];

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(BTreeMap<String, J>),
}

static NULL: J = J::Null;

impl J {
    fn get(&self, k: &str) -> &J {
        match self {
            J::Obj(m) => m.get(k).unwrap_or(&NULL),
            _ => &NULL,
        }
    }
    fn arr(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => &[],
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at {}", c as char, self.i))
        }
    }
    fn value(&mut self) -> Result<J, String> {
        self.ws();
        match self.s.get(self.i).copied() {
            Some(b'{') => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(J::Obj(m));
                }
                loop {
                    self.ws();
                    let J::Str(k) = self.value()? else { return Err("object key".into()) };
                    self.eat(b':')?;
                    m.insert(k, self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(J::Obj(m));
                        }
                        _ => return Err(format!("object at {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                loop {
                    v.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(J::Arr(v));
                        }
                        _ => return Err(format!("array at {}", self.i)),
                    }
                }
            }
            Some(b'"') => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    let c = *self.s.get(self.i).ok_or("unterminated string")?;
                    self.i += 1;
                    match c {
                        b'"' => return Ok(J::Str(out)),
                        b'\\' => {
                            let e = *self.s.get(self.i).ok_or("escape")?;
                            self.i += 1;
                            match e {
                                b'n' => out.push('\n'),
                                b't' => out.push('\t'),
                                b'r' => out.push('\r'),
                                b'b' => out.push('\u{8}'),
                                b'f' => out.push('\u{c}'),
                                b'u' => {
                                    let h = std::str::from_utf8(self.s.get(self.i..self.i + 4).ok_or("\\u")?).map_err(|e| e.to_string())?;
                                    let cp = u32::from_str_radix(h, 16).map_err(|e| e.to_string())?;
                                    self.i += 4;
                                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                                }
                                other => out.push(other as char),
                            }
                        }
                        _ => {
                            // Copy a UTF-8 run as is.
                            let start = self.i - 1;
                            let mut end = self.i;
                            while end < self.s.len() && self.s[end] != b'"' && self.s[end] != b'\\' {
                                end += 1;
                            }
                            out.push_str(std::str::from_utf8(&self.s[start..end]).map_err(|e| e.to_string())?);
                            self.i = end;
                        }
                    }
                }
            }
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(J::Bool(true))
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(J::Bool(false))
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(J::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                Ok(J::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
            _ => Err(format!("unexpected input at {}", self.i)),
        }
    }
}

fn parse_json(s: &[u8]) -> J {
    let mut p = P { s, i: 0 };
    let v = p.value().expect("cargo metadata is JSON");
    p.ws();
    assert_eq!(p.i, s.len(), "trailing bytes after the JSON value");
    v
}

/// SPDX: `a OR b`, `a AND b`, parentheses, `X WITH exception` (allowed when
/// X is), and the legacy `a/b`.
fn spdx_allowed(expr: &str) -> bool {
    fn atom(t: &[&str], i: &mut usize) -> bool {
        if t.get(*i) == Some(&"(") {
            *i += 1;
            let v = any(t, i);
            *i += 1; // ")"
            return v;
        }
        let ok = t.get(*i).is_some_and(|id| ALLOWED.contains(id));
        *i += 1;
        if t.get(*i) == Some(&"WITH") {
            *i += 2;
        }
        ok
    }
    fn all(t: &[&str], i: &mut usize) -> bool {
        let mut v = atom(t, i);
        while t.get(*i) == Some(&"AND") {
            *i += 1;
            v &= atom(t, i);
        }
        v
    }
    fn any(t: &[&str], i: &mut usize) -> bool {
        let mut v = all(t, i);
        while t.get(*i) == Some(&"OR") {
            *i += 1;
            v |= all(t, i);
        }
        v
    }
    let spaced = expr.replace('(', " ( ").replace(')', " ) ").replace('/', " OR ");
    let t: Vec<&str> = spaced.split_whitespace().collect();
    let mut i = 0;
    let v = any(&t, &mut i);
    v && i == t.len()
}

fn metadata(target: &str) -> J {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked", "--filter-platform", target, "--manifest-path", manifest])
        .output()
        .expect("run cargo metadata");
    assert!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    parse_json(&out.stdout)
}

#[test]
fn json_reader_and_spdx() {
    let j = parse_json(br#"{"a": [1, -2.5e3, true, null, "x\"y\u00e9"], "b": {}, "c": []}"#);
    assert_eq!(j.get("a").arr().len(), 5);
    assert_eq!(j.get("a").arr()[4].str(), Some("x\"y\u{e9}"));
    assert_eq!(j.get("missing"), &J::Null);
    assert!(spdx_allowed("MIT"));
    assert!(spdx_allowed("MIT OR Apache-2.0"));
    assert!(spdx_allowed("MIT/Apache-2.0"));
    assert!(spdx_allowed("(MIT OR Apache-2.0) AND Unicode-3.0"));
    assert!(spdx_allowed("Apache-2.0 WITH LLVM-exception"));
    assert!(!spdx_allowed("MPL-2.0"));
    assert!(!spdx_allowed("MIT AND MPL-2.0"));
    assert!(!spdx_allowed("GPL-2.0-or-later"));
    assert!(!spdx_allowed("LGPL-2.1-or-later OR GPL-2.0"));
}

#[test]
fn the_shipped_tree_is_this_crate_alone() {
    for target in TARGETS {
        let md = metadata(target);
        let packages = md.get("packages").arr();
        let root = packages.iter().find(|p| p.get("name").str() == Some("k2-isobmff")).expect("k2-isobmff in the workspace");
        let id = root.get("id").str().expect("id");
        assert_eq!(root.get("license").str(), Some("MIT"));
        assert!(spdx_allowed("MIT"));
        assert_eq!(root.get("source"), &J::Null, "a path package, not from a registry or git");
        let deps = root.get("dependencies").arr();
        assert!(deps.is_empty(), "k2-isobmff must have no dependencies (normal, build or dev): {deps:?} ({target})");
        let node = md.get("resolve").get("nodes").arr().iter().find(|n| n.get("id").str() == Some(id)).expect("resolve node");
        assert!(node.get("deps").arr().is_empty(), "nothing resolved under k2-isobmff ({target})");
        eprintln!("[licence] k2-isobmff {}: MIT (path), no dependencies ({target})", root.get("version").str().unwrap_or("?"));
    }
}
