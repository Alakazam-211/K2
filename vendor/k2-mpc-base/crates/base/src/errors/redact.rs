//! No absolute path in what the kit hands out (K7, v0.9.8): [`redact`].

use std::path::Path;

use super::Value;

/// `text` with every absolute path made safe to show: a path inside `root` becomes relative
/// to it (`root` itself becomes `.`), any other absolute path `<outside>`.
///
/// `root` is matched as given and as the file system resolves it (`/tmp` and `/private/tmp`
/// on macOS), so a path the kit canonicalised is found too. Any other absolute path is a
/// token starting with `/` (or a drive, `C:\`) after a start of text, a space, a quote, an
/// opening bracket or `=`, up to the next space, quote, closing bracket, comma, semicolon
/// or `: ` (an OS error's separator). Relative paths and text without a leading `/` pass
/// unchanged.
pub fn redact(text: &str, root: &Path) -> String {
    let mut roots: Vec<String> = vec![trim(&root.to_string_lossy())];
    if let Ok(c) = std::fs::canonicalize(root) {
        let c = trim(&c.to_string_lossy());
        if !roots.contains(&c) {
            roots.push(c);
        }
    }
    // the longest first, so `/private/tmp/w` wins over `/tmp/w` inside it
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let mut t = text.to_owned();
    for r in roots.iter().filter(|r| !r.is_empty() && r.as_str() != "/") {
        t = strip_root(&t, r);
    }
    outside(&t)
}

fn trim(s: &str) -> String {
    let t = s.trim_end_matches(['/', '\\']);
    if t.is_empty() {
        s.to_owned()
    } else {
        t.to_owned()
    }
}

/// Every `root` followed by a separator or the end of a path: relative (`.` for the root).
fn strip_root(t: &str, root: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find(root) {
        out.push_str(&rest[..i]);
        let after = &rest[i + root.len()..];
        match after.chars().next() {
            Some('/') | Some('\\') => {
                rest = &after[1..];
            }
            None => {
                out.push('.');
                rest = after;
            }
            Some(c) if ends(c) || c == ':' => {
                out.push('.');
                rest = after;
            }
            // a longer name that only starts like the root (`/ws` in `/wsx`): not it
            Some(_) => {
                out.push_str(&rest[i..i + root.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What ends a path token.
fn ends(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | ']' | '}' | ',' | ';')
}

/// What may come right before a path token.
fn starts_after(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | '[' | '{' | '='),
    }
}

/// Every remaining absolute path: `<outside>`.
fn outside(t: &str) -> String {
    let chars: Vec<(usize, char)> = t.char_indices().collect();
    let mut out = String::with_capacity(t.len());
    let mut k = 0;
    while k < chars.len() {
        let (_, c) = chars[k];
        let prev = (k > 0).then(|| chars[k - 1].1);
        let drive = c.is_ascii_alphabetic()
            && chars.get(k + 1).is_some_and(|x| x.1 == ':')
            && chars.get(k + 2).is_some_and(|x| x.1 == '\\');
        let unix = c == '/' && chars.get(k + 1).is_some_and(|x| !ends(x.1) && x.1 != '/');
        if (unix || drive) && starts_after(prev) {
            // up to the token's end, or ": " (an OS error's separator)
            let mut j = k + 1;
            while j < chars.len() {
                let x = chars[j].1;
                if ends(x) || (x == ':' && chars.get(j + 1).map_or(true, |y| y.1 == ' ')) {
                    break;
                }
                j += 1;
            }
            out.push_str("<outside>");
            k = j;
            continue;
        }
        out.push(c);
        k += 1;
    }
    out
}

/// Every string in `v`, through [`redact`].
pub(super) fn value(v: &mut Value, root: &Path) {
    match v {
        Value::Str(s) => *s = redact(s, root),
        Value::List(l) => l.iter_mut().for_each(|x| value(x, root)),
        Value::Obj(o) => o.iter_mut().for_each(|(_, x)| value(x, root)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_relative_outside_hidden() {
        let root = Path::new("/w/space");
        assert_eq!(
            redact("/w/space/a/b.md: not found", root),
            "a/b.md: not found"
        );
        assert_eq!(
            redact("root /w/space is not a folder", root),
            "root . is not a folder"
        );
        assert_eq!(
            redact(
                "/w/space/a.md resolves to /etc/passwd, outside the root",
                root
            ),
            "a.md resolves to <outside>, outside the root"
        );
        assert_eq!(redact("(\"/home/x/y\")", root), "(\"<outside>\")");
        assert_eq!(
            redact("C:\\Users\\x\\f.md: denied", root),
            "<outside>: denied"
        );
        // not paths
        assert_eq!(
            redact("s/files/docs/g and a/b", root),
            "s/files/docs/g and a/b"
        );
        assert_eq!(redact("50/50 split", root), "50/50 split");
        // a sibling that only starts like the root
        assert_eq!(redact("/w/spacex/f", root), "<outside>");
    }
}
