//! Which brand a binary was built for, and what it says when it meets
//! another brand's server on the same machine.
//!
//! **The mark.** Every binary that asks for its brand ([`marked_brand_id`])
//! or keeps it (`k2_brand::mark::keep`) carries [`BRAND_MARK`]: the bytes
//! `K5_BRAND_MARK=<brand id>` and a NUL, beside the version mark. A release
//! reads the marks without running anything (`scripts/brand/check-brand-mark.py`)
//! and refuses a bundle that holds two brands, or a binary of another
//! brand: one cargo call that missed `K2_BRAND_DIR` is the likeliest way
//! a mismatch is made.
//!
//! **The session's brand.** The daemon sets [`SESSION_BRAND_ENV`] and
//! [`SESSION_CLI_ENV`] in every terminal it starts (the brand's id and its
//! command). A command line of another brand run in that terminal says so
//! before it does anything, even for help or study, which never ask the
//! daemon: those are the calls that teach the name.
//!
//! **The headers.** A daemon's `/cli/*` answer to a request from this
//! machine carries [`SERVER_BRAND_HEADER`] and [`SERVER_CLI_HEADER`] beside
//! its version; never an answer that leaves the machine. Nothing between
//! servers reads either: brands talk to each other, and the wire stays the
//! same in every brand.
//!
//! Either way the command line prints [`mismatch_line`] on stderr, once,
//! and carries on. It never refuses (K2's ruling 2): the request is fine,
//! and a refusal could lock an owner out of a half-updated machine. A value
//! that is not a name's shape ([`is_brand_word`]) is no line: raw bytes
//! from an environment or a header are never printed.
//!
//! The variable and header names are internal spellings: the same bytes
//! in every brand's build.

use super::CLI_NAME;

/// The brand's id, set by the daemon in every terminal it starts.
pub const SESSION_BRAND_ENV: &str = "K2_SESSION_BRAND";
/// The brand's command, set beside [`SESSION_BRAND_ENV`].
pub const SESSION_CLI_ENV: &str = "K2_SESSION_CLI";
/// The daemon's brand id on a `/cli/*` answer to this machine.
pub const SERVER_BRAND_HEADER: &str = "X-K5-Server-Brand";
/// The daemon's brand's command, beside [`SERVER_BRAND_HEADER`].
pub const SERVER_CLI_HEADER: &str = "X-K5-Server-Cli";

/// The mark and its reader live in the leaf crate, so a helper that does
/// not link this crate carries the same bytes (`k2_brand::mark`).
pub use k2_brand::mark::{marked_brand_id, BRAND_MARK, BRAND_MARK_PREFIX};

/// A brand id or a command name: `^[a-z][a-z0-9]{1,11}$`, the name
/// policy's shape. Anything else is never printed.
pub fn is_brand_word(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=12).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The other brand's command when `brand` and `cli` (from the session's
/// environment or an answer's headers) name a brand that is not this
/// build's; `None` when they name this one, or are missing, or either is
/// not a name's shape.
pub fn mismatch<'a>(brand: Option<&str>, cli: Option<&'a str>) -> Option<&'a str> {
    let (brand, cli) = (brand?, cli?);
    (is_brand_word(brand) && is_brand_word(cli) && brand != marked_brand_id()).then_some(cli)
}

/// The line for a server of another brand whose command is `theirs`.
pub fn mismatch_line(theirs: &str) -> String {
    format!(
        "note: this {ours} command is another brand's than the server on this machine; that server's help, hints and skills name {theirs}. Use {theirs} here.",
        ours = CLI_NAME
    )
}

/// The pairs the daemon sets in a terminal it starts.
pub fn session_env() -> [(String, String); 2] {
    [
        (SESSION_BRAND_ENV.to_string(), marked_brand_id().to_string()),
        (SESSION_CLI_ENV.to_string(), CLI_NAME.to_string()),
    ]
}

/// The header lines a daemon adds to a `/cli/*` answer to this machine.
pub fn server_header_lines() -> String {
    format!(
        "{SERVER_BRAND_HEADER}: {}\r\n{SERVER_CLI_HEADER}: {CLI_NAME}\r\n",
        marked_brand_id()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::BRAND_ID;

    #[test]
    fn the_mark_is_the_leaf_crate_s() {
        assert_eq!(marked_brand_id(), BRAND_ID);
        assert!(BRAND_MARK.starts_with(BRAND_MARK_PREFIX.as_bytes()));
    }

    #[test]
    fn a_name_s_shape() {
        for ok in ["k2", "xy", "q7", "abcdefghijkl", "a1"] {
            assert!(is_brand_word(ok), "{ok}");
        }
        for bad in [
            "",
            "x",
            "X2",
            "2x",
            "x-y",
            "x y",
            "abcdefghijklm",
            "xy\n",
            "k\u{e9}",
        ] {
            assert!(!is_brand_word(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_another_brand_with_both_names_well_formed_is_a_mismatch() {
        let other = if BRAND_ID == "zz" { "zy" } else { "zz" };
        assert_eq!(mismatch(Some(other), Some("zq")), Some("zq"));
        assert_eq!(mismatch(Some(BRAND_ID), Some("zq")), None, "this brand");
        assert_eq!(mismatch(None, Some("zq")), None);
        assert_eq!(mismatch(Some(other), None), None);
        assert_eq!(
            mismatch(Some(other), Some("Z Q")),
            None,
            "a command that is not a name"
        );
        assert_eq!(
            mismatch(Some("Z\u{1b}[1m"), Some("zq")),
            None,
            "an id that is not a name"
        );
    }

    #[test]
    fn the_line_names_both_commands_and_the_env_and_headers_carry_both_names() {
        assert_eq!(
            mismatch_line("zq"),
            format!("note: this {CLI_NAME} command is another brand's than the server on this machine; that server's help, hints and skills name zq. Use zq here.")
        );
        let env = session_env();
        assert_eq!(
            env[0],
            ("K2_SESSION_BRAND".to_string(), BRAND_ID.to_string())
        );
        assert_eq!(env[1], ("K2_SESSION_CLI".to_string(), CLI_NAME.to_string()));
        assert_eq!(
            server_header_lines(),
            format!("X-K5-Server-Brand: {BRAND_ID}\r\nX-K5-Server-Cli: {CLI_NAME}\r\n")
        );
    }
}
