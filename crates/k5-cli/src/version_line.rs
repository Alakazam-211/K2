//! The reader of the CLI's version line, `K2_CLI_VERSION="x.y.z"`, in
//! the first [`VERSION_LINES`] lines of a file: `cli/k2`, or
//! `cli/src/prelude.sh` (the line `scripts/build-cli.sh` copies into
//! `cli/k2` and the release bumps). `build.rs` reads the version with it,
//! and the tests read the other file with it to check the two agree.

/// How far into the file the version line may be (prelude.sh puts it
/// on line 12; k2-client's script check reads 40 lines too).
pub const VERSION_LINES: usize = 40;

/// The value of the `K2_CLI_VERSION="x.y.z"` line: digits, letters, dots,
/// `-` and `+` only, never empty.
pub fn cli_version(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(16 * 1024)]);
    let line = text
        .lines()
        .take(VERSION_LINES)
        .find_map(|l| l.strip_prefix("K2_CLI_VERSION=\""))?;
    let value = line.strip_suffix('"')?;
    let ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b));
    ok.then(|| value.to_string())
}
