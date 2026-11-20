//! The block's `normalize_color(raw)`: `#rrggbb`, `#rgb`, `rgb(r,g,b)`
//! (or `rgba(…, a)`), or `r,g,b`, as its regular expressions take them.

use super::super::py_strip;

/// Python's `\s` in a `str` pattern (`str.isspace`).
fn space(c: char) -> bool {
    py_strip(&c.to_string()).is_empty()
}

/// `[0-9]{1,3}` at `i`: the number and where it ends. A fourth digit
/// can't be followed by what the patterns want next, so it fails.
fn number(t: &[char], i: usize) -> Option<(u32, usize)> {
    let mut j = i;
    while j < t.len() && t[j].is_ascii_digit() {
        j += 1;
    }
    if j == i || j - i > 3 {
        return None;
    }
    let n = t[i..j].iter().collect::<String>().parse().ok()?;
    Some((n, j))
}

fn skip_space(t: &[char], mut i: usize) -> usize {
    while i < t.len() && space(t[i]) {
        i += 1;
    }
    i
}

/// `\s*[, ]\s*` from `i` (the first form), or `\s*,\s*` (the second):
/// where the next number starts.
fn separator(t: &[char], i: usize, space_too: bool) -> Option<usize> {
    let mut j = i;
    while j < t.len() && (space(t[j]) || t[j] == ',') {
        j += 1;
    }
    let run = &t[i..j];
    let commas = run.iter().filter(|c| **c == ',').count();
    let ok = match commas {
        0 => space_too && run.contains(&' '),
        1 => true,
        _ => false,
    };
    ok.then_some(j)
}

/// Three numbers with separators from `i`: the numbers and where they end.
fn three(t: &[char], i: usize, space_too: bool) -> Option<([u32; 3], usize)> {
    let (a, i) = number(t, i)?;
    let i = separator(t, i, space_too)?;
    let (b, i) = number(t, i)?;
    let i = separator(t, i, space_too)?;
    let (c, i) = number(t, i)?;
    Some(([a, b, c], i))
}

/// `rgba?\(\s*N\s*[, ]\s*N\s*[, ]\s*N(?:\s*[,/]\s*[0-9.]+\s*)?\)`, any case.
fn rgb_call(t: &[char]) -> Option<[u32; 3]> {
    let word: String = t.iter().take(5).collect::<String>().to_ascii_lowercase();
    let i = if word.starts_with("rgba(") {
        5
    } else if word.starts_with("rgb(") {
        4
    } else {
        return None;
    };
    let i = skip_space(t, i);
    let (n, mut i) = three(t, i, true)?;
    if t.get(i) != Some(&')') {
        i = skip_space(t, i);
        if !matches!(t.get(i), Some(',') | Some('/')) {
            return None;
        }
        i = skip_space(t, i + 1);
        let start = i;
        while i < t.len() && (t[i].is_ascii_digit() || t[i] == '.') {
            i += 1;
        }
        if i == start {
            return None;
        }
        i = skip_space(t, i);
        if t.get(i) != Some(&')') {
            return None;
        }
    }
    (i + 1 == t.len()).then_some(n)
}

/// `N\s*,\s*N\s*,\s*N`.
fn triple(t: &[char]) -> Option<[u32; 3]> {
    let (n, i) = three(t, 0, false)?;
    (i == t.len()).then_some(n)
}

/// `normalize_color(raw)`.
pub fn normalize(raw: &str) -> Option<String> {
    let st = py_strip(raw);
    if st.is_empty() {
        return None;
    }
    let hexpart = st.strip_prefix('#').unwrap_or(st).to_lowercase();
    let hex = |n: usize| {
        hexpart.chars().count() == n && hexpart.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    };
    if hex(6) {
        return Some(format!("#{hexpart}"));
    }
    if hex(3) {
        return Some(format!(
            "#{}",
            hexpart.chars().flat_map(|c| [c, c]).collect::<String>()
        ));
    }
    let t: Vec<char> = st.chars().collect();
    let n = rgb_call(&t).or_else(|| triple(&t))?;
    if n.iter().any(|x| *x > 255) {
        return None;
    }
    Some(format!("#{:02x}{:02x}{:02x}", n[0], n[1], n[2]))
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn the_forms_the_patterns_take() {
        for (raw, want) in [
            ("#ABCDEF", Some("#abcdef")),
            (" abc ", Some("#aabbcc")),
            ("RGB(1, 2,3)", Some("#010203")),
            ("rgb(1 2 3)", Some("#010203")),
            ("rgba( 255 ,0,16 / 0.5 )", Some("#ff0010")),
            ("rgb(1,2,3 )", None),
            ("rgb(1,,2,3)", None),
            ("rgb(1\t2\t3)", None),
            ("10, 20 ,30", Some("#0a141e")),
            ("10 20 30", None),
            ("1,2,256", None),
            ("1,2,3333", None),
            ("#abcd", None),
            ("", None),
        ] {
            assert_eq!(normalize(raw).as_deref(), want, "{raw:?}");
        }
    }
}
