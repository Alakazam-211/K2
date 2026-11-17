//! The shell CLI the binary carries: `cli/k2` from this checkout, packed
//! by `build.rs` (gzipped, its sha256 and its version).

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// The prefix of [`VERSION_MARKER`].
pub const MARKER_PREFIX: &str = "K5_CLI_VERSION=";

/// The CLI's version, read from [`VERSION_MARKER`] in a way the compiler
/// can't fold away, so the whole marker stays in the binary's bytes.
pub fn version() -> &'static str {
    let marker: &'static str = std::hint::black_box(VERSION_MARKER);
    marker.strip_prefix(MARKER_PREFIX).expect("build.rs writes the marker with its prefix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_the_script_s_version() {
        assert_eq!(VERSION_MARKER, format!("{MARKER_PREFIX}{SCRIPT_VERSION}"));
        assert_eq!(version(), SCRIPT_VERSION);
        assert_eq!(SCRIPT_SHA256.len(), 64);
        assert!(SCRIPT_SHA256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{SCRIPT_SHA256}");
    }
}
