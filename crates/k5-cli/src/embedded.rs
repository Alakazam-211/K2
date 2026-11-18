//! The shell CLI the binary carries: `cli/k2` from this checkout, packed
//! by `build.rs` (gzipped, its BLAKE3 and its version).

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
mod tests {
    use super::*;

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
        assert_eq!(SCRIPT_BLAKE3.len(), 64);
        assert!(
            SCRIPT_BLAKE3
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{SCRIPT_BLAKE3}"
        );
    }
}
