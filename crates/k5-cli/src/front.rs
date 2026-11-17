//! What the front does with a command line: answer it itself, or hand it
//! to the shell CLI.
//!
//! Today it answers only the version. Everything else falls through,
//! byte for byte ([`crate::fallthrough`]).

use std::ffi::OsString;

/// Where a command line goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `--version`, `-v` or `version`, with nothing after it.
    Version,
    /// Anything else: the shell CLI runs it.
    Fallthrough,
}

/// The route for the arguments after `argv[0]`.
pub fn route(args: &[OsString]) -> Route {
    match args {
        [one] if one == "--version" || one == "-v" || one == "version" => Route::Version,
        _ => Route::Fallthrough,
    }
}

/// What the version prints: the shell CLI's line, `k2 <version>`
/// (`cli/src/router.sh`, `echo "k2 $K2_CLI_VERSION"`). It says `k2` under
/// either name, as the shell does.
pub fn version_line(version: &str) -> String {
    format!("k2 {version}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_a_bare_version_is_answered_here() {
        for one in ["--version", "-v", "version"] {
            assert_eq!(route(&args(&[one])), Route::Version, "{one}");
        }
        let rows: &[&[&str]] = &[
            &[],
            &["help"],
            &["--help"],
            &["-V"],
            &["--Version"],
            &["versions"],
            &["--version="],
            &[" --version"],
            &["--version", "--json"],
            &["version", "x"],
            &["-v", ""],
            &["--json", "--version"],
            &["msg", "--version"],
            &[""],
        ];
        for row in rows {
            assert_eq!(route(&args(row)), Route::Fallthrough, "{row:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_argument_that_is_not_utf8_falls_through() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(route(&[OsString::from_vec(b"--version\xff".to_vec())]), Route::Fallthrough);
    }

    #[test]
    fn the_line_says_k2() {
        assert_eq!(version_line("0.46.14"), "k2 0.46.14\n");
    }
}
