//! Another brand's server on this machine: one line on stderr, once per
//! process, and the command goes on (K2's rulings 2 and 4).
//!
//! Two ways to know: the session's environment, which the daemon sets in
//! every terminal it starts ([`session`], read before any route, so help
//! and study warn too, with no request), and the brand headers of a
//! `/cli/*` answer from this machine ([`answer`], read as each answer
//! comes in, for a command line run outside a session). The rules, the
//! names and the line are `k2_client::brand::session`'s: a value that is
//! not a name's shape is never printed and gives no line.

use k2_client::brand::session::{self as brand, SESSION_BRAND_ENV, SESSION_CLI_ENV};
use std::sync::atomic::{AtomicBool, Ordering};

static SHOWN: AtomicBool = AtomicBool::new(false);

/// The line for `brand` and `cli` when they name another brand, unless
/// this process has shown it already.
fn due(brand: Option<&str>, cli: Option<&str>) -> Option<String> {
    let theirs = brand::mismatch(brand, cli)?;
    (!SHOWN.swap(true, Ordering::SeqCst)).then(|| brand::mismatch_line(theirs))
}

fn show(brand: Option<&str>, cli: Option<&str>) {
    if let Some(line) = due(brand, cli) {
        crate::out::stderr_line(&line);
    }
}

/// The session's brand, from the environment (a value that is not UTF-8
/// is no value).
pub fn session() {
    let brand = std::env::var(SESSION_BRAND_ENV).ok();
    let cli = std::env::var(SESSION_CLI_ENV).ok();
    show(brand.as_deref(), cli.as_deref());
}

/// An answer's brand headers, from a daemon on this machine.
pub fn answer(brand: Option<&str>, cli: Option<&str>) {
    show(brand, cli);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line per process, whichever way the other brand was seen; this
    /// brand, a missing value or one that is not a name is no line.
    #[test]
    fn the_line_comes_once_and_only_for_another_brand() {
        let ours = k2_client::brand::BRAND_ID;
        let other = if ours == "zz" { "zy" } else { "zz" };
        assert_eq!(due(Some(ours), Some("zq")), None);
        assert_eq!(due(Some(other), None), None);
        assert_eq!(due(Some(other), Some("Z Q")), None);
        assert_eq!(due(None, Some("zq")), None);
        assert_eq!(
            due(Some(other), Some("zq")),
            Some(brand::mismatch_line("zq"))
        );
        assert_eq!(due(Some(other), Some("zq")), None, "the second time");
        assert_eq!(
            due(Some("zr"), Some("zr")),
            None,
            "another brand still: once"
        );
    }
}
