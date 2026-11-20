//! Where a line of the shell's Python is, as `python3 -c` numbers it, so a
//! traceback the binary prints names the line the shell's would. Found at
//! compile time in the shell's own source: a marker that is not there is
//! a build error, so a change to the shell's Python can't leave a stale
//! number behind. These go with the shell (R12).

/// `cli/src/compute-lib.sh`.
pub const LIB: &str = include_str!("../../../../../cli/src/compute-lib.sh");
/// `cli/src/compute-jobs.sh`.
pub const JOBS: &str = include_str!("../../../../../cli/src/compute-jobs.sh");
/// `cli/src/compute-command.sh`.
pub const COMMAND: &str = include_str!("../../../../../cli/src/compute-command.sh");

const fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + n.len() <= h.len() {
        let mut k = 0;
        while k < n.len() && h[i + k] == n[k] {
            k += 1;
        }
        if k == n.len() {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The line of the first `marker` in the first `python3 -c '…'` after
/// `after` in `src`: line 1 is the text right after `-c '`.
pub const fn py_line(src: &str, after: &str, marker: &str) -> u32 {
    let h = src.as_bytes();
    let Some(a) = find(h, after.as_bytes(), 0) else {
        panic!("no such function in the shell's source");
    };
    let open = b"python3 -c '";
    let Some(p) = find(h, open, a) else {
        panic!("no python3 -c ' after it");
    };
    let start = p + open.len();
    let Some(end) = find(h, b"'", start) else {
        panic!("the shell's Python has no end");
    };
    let Some(m) = find(h, marker.as_bytes(), start) else {
        panic!("a marker is not in the shell's Python");
    };
    if m + marker.len() > end {
        panic!("a marker is past the end of that Python");
    }
    let mut line = 1;
    let mut i = start;
    while i < m {
        if h[i] == b'\n' {
            line += 1;
        }
        i += 1;
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_count_from_after_the_quote() {
        let src = "f() {\n  x | python3 -c '\nimport json\nv = 1\nprint(v)'\n}";
        assert_eq!(py_line(src, "f() {", "import json"), 2);
        assert_eq!(py_line(src, "f() {", "print(v)"), 4);
    }
}
