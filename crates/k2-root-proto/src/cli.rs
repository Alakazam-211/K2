//! `helper cli-refresh` (protocol 9): which file at `/usr/local/bin/k2` is
//! the K2 CLI, and at which version. One parser for the helper (which
//! decides as root) and the daemon (which decides whether to ask); the
//! daemon's tests hold it equal to `k2_core::cli_stage::k2_cli_version_of`.

/// Bytes of the file read for the marker. The marker sits in the first
/// lines; nobody reads the whole ~3 MB CLI to find it.
pub const CLI_HEAD_BYTES: usize = 16 * 1024;
/// Larger than this is not the K2 CLI (it is ~3 MB): never replaced.
pub const CLI_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// The marker must be in the first this-many lines.
pub const CLI_MARKER_LINES: usize = 30;

/// The version string of a K2 CLI script: a `#!/bin/bash` first line and
/// a `K2_CLI_VERSION=` (or k2so-era `K2SO_CLI_VERSION=`) line in the first
/// [`CLI_MARKER_LINES`] of the first [`CLI_HEAD_BYTES`]. `None` for
/// anything else: a file that is not the K2 CLI is never replaced. The
/// value is returned as written (quotes trimmed); callers parse it with
/// [`crate::version::Semver::parse`], which refuses `-rc1`, leading zeros
/// and anything that is not plain `x.y.z`.
pub fn cli_version_of(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(CLI_HEAD_BYTES)]);
    // `lines()` drops a trailing `\r`, the way `k2_core::cli_stage` reads it.
    let mut lines = head.lines();
    if lines.next()? != "#!/bin/bash" {
        return None;
    }
    head.lines().take(CLI_MARKER_LINES).find_map(|l| {
        l.strip_prefix("K2_CLI_VERSION=")
            .or_else(|| l.strip_prefix("K2SO_CLI_VERSION="))
            .map(|v| v.trim().trim_matches('"').to_string())
    })
}

/// Shared fixtures: (file bytes, the version string both parsers return).
/// The daemon's test runs these through `k2_core`'s parser too.
pub const VERSION_CASES: &[(&str, Option<&str>)] = &[
    ("#!/bin/bash\nK2_CLI_VERSION=\"0.40.148\"\n", Some("0.40.148")),
    ("#!/bin/bash\nK2_CLI_VERSION=0.46.3\n", Some("0.46.3")),
    // A pre-release tag and leading zeros are read, then refused by Semver.
    ("#!/bin/bash\nK2_CLI_VERSION=\"0.46.3-rc1\"\n", Some("0.46.3-rc1")),
    ("#!/bin/bash\nK2_CLI_VERSION=\"0.046.03\"\n", Some("0.046.03")),
    // CRLF line ends: the `\r` is dropped by both.
    ("#!/bin/bash\r\nK2_CLI_VERSION=\"0.40.148\"\r\necho x\r\n", Some("0.40.148")),
    // The k2so-era marker reads as a (stale) K2 CLI.
    ("#!/bin/bash\nK2SO_CLI_VERSION=\"0.39.30\"\n", Some("0.39.30")),
    // Not the K2 CLI.
    ("#!/bin/sh\nK2_CLI_VERSION=\"0.1.0\"\n", None),
    ("#!/bin/bash\necho some other k2\n", None),
    ("\x7fELF", None),
    ("", None),
    (" #!/bin/bash\nK2_CLI_VERSION=\"0.1.0\"\n", None),
];

/// A marker after line [`CLI_MARKER_LINES`] is not read.
pub fn marker_after_line(n: usize) -> String {
    let mut s = String::from("#!/bin/bash\n");
    for i in 1..n {
        s.push_str(&format!("# line {}\n", i + 1));
    }
    s.push_str("K2_CLI_VERSION=\"0.40.148\"\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Semver;

    #[test]
    fn version_cases() {
        for (text, want) in VERSION_CASES {
            assert_eq!(cli_version_of(text.as_bytes()).as_deref(), *want, "{text:?}");
        }
        // Line 30 is read, line 31 is not.
        assert_eq!(cli_version_of(marker_after_line(29).as_bytes()).as_deref(), Some("0.40.148"));
        assert_eq!(cli_version_of(marker_after_line(30).as_bytes()), None);
        // Only plain x.y.z versions take part in the compare.
        for bad in ["0.46.3-rc1", "0.046.03", "0.46", "0.46.3.1", "v0.46.3", ""] {
            assert_eq!(Semver::parse(bad), None, "{bad}");
        }
        // A marker past the head is not read.
        let mut big = String::from("#!/bin/bash\n");
        big.push_str(&"#".repeat(CLI_HEAD_BYTES));
        big.push_str("\nK2_CLI_VERSION=\"0.1.0\"\n");
        assert_eq!(cli_version_of(big.as_bytes()), None);
    }
}
