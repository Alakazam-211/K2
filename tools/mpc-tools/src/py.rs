//! Python's text habits that the ported tools kept, so their outputs stay the same.

/// Python's `str.isspace()` (and `\s` in its regular expressions): Unicode whitespace, plus
/// the four separators U+001C to U+001F, which Unicode does not count as whitespace.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `str.strip()` with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Python's `str.split()` with no argument: runs of whitespace separate, none are empty.
pub fn split(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|w| !w.is_empty()).collect()
}

/// What Python's text-mode read gives: `\r\n` and a lone `\r` both become `\n`.
pub fn universal_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Bytes decoded as Python's `errors="replace"` does, then read in text mode.
pub fn read_text_lossy(bytes: &[u8]) -> String {
    universal_newlines(&String::from_utf8_lossy(bytes))
}

/// Python's `str.splitlines()`: lines end at `\n`, `\r\n`, `\r`, `\v`, `\f`, U+001C to
/// U+001E, U+0085, U+2028 and U+2029, and no empty line follows a last ending.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let ends = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if ends {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = it.peek().copied() {
                    it.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Python's `str.rstrip()` with no argument.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_as_python_counts_them() {
        for c in [
            ' ', '\t', '\n', '\u{b}', '\u{c}', '\r', '\u{1c}', '\u{1f}', '\u{85}', '\u{a0}',
            '\u{3000}',
        ] {
            assert!(is_space(c), "{c:?} is whitespace to Python");
        }
        for c in ['a', '_', '\u{200b}', '\u{feff}', '\u{1b}'] {
            assert!(!is_space(c), "{c:?} is not whitespace to Python");
        }
    }

    #[test]
    fn newlines_and_splits() {
        assert_eq!(universal_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(split("  a \u{a0}b\tc  "), vec!["a", "b", "c"]);
        assert_eq!(strip("\u{1c} x \n"), "x");
        assert_eq!(
            splitlines("a\nb\r\nc\rd\u{2028}e\n"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert!(splitlines("").is_empty());
    }
}
