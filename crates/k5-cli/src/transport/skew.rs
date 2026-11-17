//! Version skew: the binary's own version against the daemon's.
//!
//! A daemon on this machine names its version on every `/cli/*` answer
//! ([`super::wire::SERVER_VERSION_HEADER`], slice 10a). When an answer the
//! binary already got names another version than its own, it prints one
//! line on stderr, once per process, and carries on: the command on PATH
//! is older or newer than the server it talks to. It never refuses on the
//! version alone, and it makes no request of its own for it.
//!
//! No header (an older daemon, a LAN or tunnel answer) is no line. A value
//! that is not a version (empty, longer than [`MAX_LEN`], or a byte that
//! is not a letter, a digit, `.`, `-` or `+`) is taken as no header, so a
//! stranger's bytes never reach the terminal.

use std::sync::atomic::{AtomicBool, Ordering};

/// The longest value read as a version.
pub const MAX_LEN: usize = 64;

/// The value when it is shaped like a version.
pub fn clean(value: &str) -> Option<&str> {
    let ok = !value.is_empty()
        && value.len() <= MAX_LEN
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
    ok.then_some(value)
}

/// The line, K2's ruled text. It names the command `k5`, as the ruling
/// does: `k5 update` is the fix whichever name ran this.
pub fn line(ours: &str, theirs: &str) -> String {
    let k5 = crate::called_as::CalledAs::K5Name.name();
    format!("{k5} {ours} is talking to a {theirs} server; the command on this PATH is out of date: run {k5} update")
}

/// The server's version when an answer names one that differs from
/// `ours`.
pub fn differs<'a>(ours: &str, theirs: Option<&'a str>) -> Option<&'a str> {
    theirs.and_then(clean).filter(|t| *t != ours)
}

/// Set once the line was printed (or a caller printed its own answer to
/// the same question, [`mark_shown`]).
static SHOWN: AtomicBool = AtomicBool::new(false);

/// After an answer: the line on stderr, the first time this process sees
/// a server version other than `ours`.
pub fn note(ours: &str, theirs: Option<&str>) {
    note_into(&SHOWN, ours, theirs, crate::out::stderr_line);
}

/// [`note`] with its flag and its printer given: true when it printed.
pub fn note_into(shown: &AtomicBool, ours: &str, theirs: Option<&str>, print: impl FnOnce(&str)) -> bool {
    let Some(theirs) = differs(ours, theirs) else {
        return false;
    };
    if shown.swap(true, Ordering::SeqCst) {
        return false;
    }
    print(&line(ours, theirs));
    true
}

/// No line from [`note`] in this process from now on: `k5 --version
/// --check` prints its own answer.
pub fn mark_shown() {
    SHOWN.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_the_ruled_text() {
        assert_eq!(
            line("0.46.16", "0.46.17"),
            "k5 0.46.16 is talking to a 0.46.17 server; the command on this PATH is out of date: run k5 update"
        );
    }

    #[test]
    fn only_a_version_shaped_value_counts() {
        for ok in ["0.46.16", "1.0.0-rc.1+build.7", "x"] {
            assert_eq!(clean(ok), Some(ok), "{ok}");
        }
        let long = "1".repeat(MAX_LEN + 1);
        for bad in ["", "0.46 .16", "0.46.16\u{1b}[2J", "0.46.16;", "\u{e9}", long.as_str()] {
            assert_eq!(clean(bad), None, "{bad:?}");
        }
        assert_eq!(clean(&"1".repeat(MAX_LEN)).map(str::len), Some(MAX_LEN));
    }

    #[test]
    fn once_per_flag_and_never_when_equal_or_absent() {
        let shown = AtomicBool::new(false);
        let mut lines = Vec::new();
        for theirs in [None, Some("0.46.16"), Some(""), Some("bad value")] {
            assert!(!note_into(&shown, "0.46.16", theirs, |l| lines.push(l.to_string())), "{theirs:?}");
        }
        assert!(!shown.load(Ordering::SeqCst), "nothing differed, the flag is still down");
        assert!(note_into(&shown, "0.46.16", Some("0.46.17"), |l| lines.push(l.to_string())));
        for theirs in [Some("0.46.17"), Some("0.47.0"), None] {
            assert!(!note_into(&shown, "0.46.16", theirs, |l| lines.push(l.to_string())), "{theirs:?}");
        }
        assert_eq!(lines, [line("0.46.16", "0.46.17")]);
    }

    #[test]
    fn a_shown_flag_prints_nothing() {
        let shown = AtomicBool::new(true);
        assert!(!note_into(&shown, "0.46.16", Some("0.46.17"), |l| panic!("printed {l}")));
    }
}
