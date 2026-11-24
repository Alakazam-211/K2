//! The shell CLI the binary carries: `cli/k2` from this checkout, packed
//! by `build.rs` (gzipped, its BLAKE3 and its version). A build of a
//! brand with no shell CLI ([`k2_client::brand::CLI_NO_BASH`]) carries
//! none: [`SCRIPT_GZ`] and [`SCRIPT_BLAKE3`] are `None`, and the version
//! is the one on `cli/src/prelude.sh`'s line, the line `cli/k2` is built
//! with.

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// The prefix of [`VERSION_MARKER`].
pub const MARKER_PREFIX: &str = "K5_CLI_VERSION=";

/// The CLI's version, read from [`VERSION_MARKER`] in a way the compiler
/// can't fold away, so the whole marker (and its NUL) stays in the binary's
/// bytes.
pub fn version() -> &'static str {
    let marker: &'static str = std::hint::black_box(VERSION_MARKER);
    marker
        .strip_prefix(MARKER_PREFIX)
        .and_then(|v| v.strip_suffix('\0'))
        .expect("build.rs writes the marker with its prefix and a NUL")
}

#[cfg(test)]
#[path = "version_line.rs"]
mod version_line;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_file(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    #[test]
    fn the_marker_is_the_script_s_version() {
        assert_eq!(VERSION_MARKER, format!("{MARKER_PREFIX}{SCRIPT_VERSION}\0"));
        assert_eq!(version(), SCRIPT_VERSION);
        // What the install code reads in the binary's bytes is this version.
        assert_eq!(k2_client::cli_install::BINARY_VERSION_MARKER, MARKER_PREFIX);
        assert_eq!(
            k2_client::cli_install::binary_versions(VERSION_MARKER.as_bytes()),
            vec![SCRIPT_VERSION.to_string()]
        );
    }

    /// A build that carries the shell CLI has its BLAKE3, in lowercase
    /// hex; a build with none has neither the copy nor a digest.
    #[test]
    fn the_copy_and_its_digest_are_there_only_with_the_shell_cli() {
        if k2_client::brand::CLI_NO_BASH {
            assert!(SCRIPT_GZ.is_none(), "a no-bash build carries a copy");
            assert_eq!(SCRIPT_BLAKE3, None);
        } else {
            assert!(SCRIPT_GZ.is_some_and(|gz| !gz.is_empty()));
            let blake = SCRIPT_BLAKE3.expect("a build with the shell CLI has its BLAKE3");
            assert_eq!(blake.len(), 64);
            assert!(
                blake
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{blake}"
            );
        }
    }

    /// The version line of `cli/src/prelude.sh`, the one a build with no
    /// shell CLI reads, is the version this build has: in a default build
    /// that is `cli/k2`'s (`cli/k2` is built from prelude.sh), so the two
    /// kinds of build always carry the same version.
    #[test]
    fn the_prelude_s_version_is_the_build_s() {
        let path = repo_file("cli/src/prelude.sh");
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert_eq!(
            version_line::cli_version(&bytes).as_deref(),
            Some(SCRIPT_VERSION),
            "{}",
            path.display()
        );
        let at = String::from_utf8_lossy(&bytes)
            .lines()
            .position(|l| l.starts_with("K2_CLI_VERSION=\""))
            .expect("prelude.sh has the version line");
        assert!(
            at < version_line::VERSION_LINES,
            "the version line is line {} of prelude.sh",
            at + 1
        );
    }
}
