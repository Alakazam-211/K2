//! `SHA256SUMS-linux-x86_64.txt` parsing and package verification.
//!
//! Every release carries one SUMS file for its Linux packages (deb, rpm,
//! AppImage and the Arch `.pkg.tar.zst`; `scripts/linux-sums.sh` keeps it
//! complete). The desktop updater installs a downloaded package as root
//! only after [`verify_package`] says its bytes match that package's line.
//!
//! Strict by design. A line is `<64 hex> <space><space|*><plain name>`; a
//! name is a bare file name (no `/`, no leading `.`, nothing outside
//! `[A-Za-z0-9._+-]`), so `dir/k2.deb` or `../k2.deb` in the file can never
//! vouch for `k2.deb`, and a caller cannot ask about a path. A malformed
//! line, a duplicate name, a missing line or a different hash is an error;
//! there is no "best effort" match.
//!
//! This checks integrity against the release's own SUMS file fetched over
//! HTTPS from the same release. It is not a signature: whoever can replace
//! release assets can replace both.

use sha2::{Digest, Sha256};

/// The release asset name the desktop updater reads.
pub const LINUX_SUMS_ASSET: &str = "SHA256SUMS-linux-x86_64.txt";

/// One parsed SUMS line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SumsEntry {
    /// Lowercase hex sha256.
    pub sha256: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SumsError {
    /// A non-empty line that is not `<hash>  <name>`.
    MalformedLine { line_no: usize, line: String },
    /// The same name appears twice (ambiguous: refuse).
    DuplicateName(String),
    /// The caller asked about something that is not a plain file name.
    BadName(String),
    /// No line for this name.
    Missing(String),
    /// The bytes hash to something else.
    Mismatch { name: String, expected: String, actual: String },
}

impl std::fmt::Display for SumsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SumsError::MalformedLine { line_no, line } => {
                write!(f, "{LINUX_SUMS_ASSET} line {line_no} is malformed: {line:?}")
            }
            SumsError::DuplicateName(n) => {
                write!(f, "{LINUX_SUMS_ASSET} lists {n} more than once")
            }
            SumsError::BadName(n) => write!(f, "not a plain package file name: {n:?}"),
            SumsError::Missing(n) => write!(f, "{LINUX_SUMS_ASSET} has no line for {n}"),
            SumsError::Mismatch { name, expected, actual } => write!(
                f,
                "{name} does not match {LINUX_SUMS_ASSET}: expected sha256 {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for SumsError {}

/// A bare release asset name: `[A-Za-z0-9_+][A-Za-z0-9._+-]*`, at most 255
/// bytes. Rejects `/`, `\`, `..`-style names, leading dots, spaces, NUL.
pub fn is_plain_file_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 255 {
        return false;
    }
    let first_ok = bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_' || bytes[0] == b'+';
    first_ok
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

fn parse_line(line: &str) -> Option<SumsEntry> {
    // 64 hex, then exactly one separator pair: "  " (text) or " *" (binary).
    if line.len() < 67 || !line.is_char_boundary(64) {
        return None;
    }
    let (hash, rest) = line.split_at(64);
    if !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let name = rest.strip_prefix("  ").or_else(|| rest.strip_prefix(" *"))?;
    if !is_plain_file_name(name) {
        return None;
    }
    Some(SumsEntry {
        sha256: hash.to_ascii_lowercase(),
        name: name.to_string(),
    })
}

/// Parse a whole SUMS file. Blank lines are allowed; a trailing `\r` is
/// stripped; anything else must be a valid line with a unique name.
pub fn parse_sums(text: &str) -> Result<Vec<SumsEntry>, SumsError> {
    let mut out: Vec<SumsEntry> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            continue;
        }
        let entry = parse_line(line).ok_or_else(|| SumsError::MalformedLine {
            line_no: i + 1,
            line: line.to_string(),
        })?;
        if out.iter().any(|e| e.name == entry.name) {
            return Err(SumsError::DuplicateName(entry.name));
        }
        out.push(entry);
    }
    Ok(out)
}

/// The expected lowercase sha256 for `name`.
pub fn expected_sha256(sums_text: &str, name: &str) -> Result<String, SumsError> {
    if !is_plain_file_name(name) {
        return Err(SumsError::BadName(name.to_string()));
    }
    parse_sums(sums_text)?
        .into_iter()
        .find(|e| e.name == name)
        .map(|e| e.sha256)
        .ok_or_else(|| SumsError::Missing(name.to_string()))
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `Ok(hash)` only when `bytes` hash to `name`'s line in `sums_text`.
pub fn verify_package(sums_text: &str, name: &str, bytes: &[u8]) -> Result<String, SumsError> {
    let expected = expected_sha256(sums_text, name)?;
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(SumsError::Mismatch {
            name: name.to_string(),
            expected,
            actual,
        });
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PKG: &[u8] = b"k2 arch package bytes";

    fn sums_with(name: &str, bytes: &[u8]) -> String {
        format!(
            "{}  K2_9.9.9_amd64.AppImage\n{}  {name}\n{}  K2-9.9.9-1.x86_64.rpm\n",
            "a".repeat(64),
            sha256_hex(bytes),
            "b".repeat(64),
        )
    }

    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn good_line_verifies() {
        let name = "k2-9.9.9-x86_64.pkg.tar.zst";
        let sums = sums_with(name, PKG);
        assert_eq!(verify_package(&sums, name, PKG).unwrap(), sha256_hex(PKG));
    }

    #[test]
    fn binary_marker_uppercase_hex_and_crlf_are_accepted() {
        let name = "K2_9.9.9_amd64.deb";
        let sums = format!("{} *{name}\r\n", sha256_hex(PKG).to_ascii_uppercase());
        assert_eq!(verify_package(&sums, name, PKG).unwrap(), sha256_hex(PKG));
    }

    #[test]
    fn missing_line_is_refused() {
        let sums = sums_with("K2_9.9.9_amd64.deb", PKG);
        let err = verify_package(&sums, "k2-9.9.9-x86_64.pkg.tar.zst", PKG).unwrap_err();
        assert_eq!(err, SumsError::Missing("k2-9.9.9-x86_64.pkg.tar.zst".into()));
        assert!(err.to_string().contains("has no line for k2-9.9.9-x86_64.pkg.tar.zst"));
    }

    #[test]
    fn wrong_hash_is_refused() {
        let name = "K2_9.9.9_amd64.deb";
        let sums = sums_with(name, b"other bytes");
        match verify_package(&sums, name, PKG).unwrap_err() {
            SumsError::Mismatch { name: n, expected, actual } => {
                assert_eq!(n, name);
                assert_eq!(expected, sha256_hex(b"other bytes"));
                assert_eq!(actual, sha256_hex(PKG));
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn empty_sums_file_is_missing() {
        assert_eq!(
            verify_package("", "K2_9.9.9_amd64.deb", PKG).unwrap_err(),
            SumsError::Missing("K2_9.9.9_amd64.deb".into())
        );
    }

    #[test]
    fn path_shaped_lines_never_vouch_for_a_bare_name() {
        let h = sha256_hex(PKG);
        for line in [
            format!("{h}  dir/K2_9.9.9_amd64.deb"),
            format!("{h}  ../K2_9.9.9_amd64.deb"),
            format!("{h}  ./K2_9.9.9_amd64.deb"),
            format!("{h}  /tmp/K2_9.9.9_amd64.deb"),
            format!("{h}  .K2_9.9.9_amd64.deb"),
            format!("{h}  K2 9.9.9.deb"),
            format!("{h} K2_9.9.9_amd64.deb"),
            format!("{h}   K2_9.9.9_amd64.deb"),
            format!("{h}0  K2_9.9.9_amd64.deb"),
            format!("{}  K2_9.9.9_amd64.deb", &h[..63]),
        ] {
            let err = verify_package(&line, "K2_9.9.9_amd64.deb", PKG).unwrap_err();
            assert!(
                matches!(err, SumsError::MalformedLine { line_no: 1, .. }),
                "{line:?} gave {err:?}"
            );
        }
    }

    #[test]
    fn caller_cannot_ask_about_a_path() {
        let sums = sums_with("K2_9.9.9_amd64.deb", PKG);
        for name in [
            "../K2_9.9.9_amd64.deb",
            "/tmp/K2_9.9.9_amd64.deb",
            "x/K2_9.9.9_amd64.deb",
            ".",
            "..",
            "",
            "K2_9.9.9_amd64.deb\0",
            "K2_9.9.9_amd64.deb\n",
        ] {
            assert_eq!(
                verify_package(&sums, name, PKG).unwrap_err(),
                SumsError::BadName(name.to_string()),
                "{name:?}"
            );
        }
    }

    #[test]
    fn duplicate_name_is_refused_even_if_one_matches() {
        let name = "K2_9.9.9_amd64.deb";
        let sums = format!("{}  {name}\n{}  {name}\n", sha256_hex(PKG), "c".repeat(64));
        assert_eq!(
            verify_package(&sums, name, PKG).unwrap_err(),
            SumsError::DuplicateName(name.into())
        );
    }

    #[test]
    fn one_malformed_line_poisons_the_file() {
        let name = "K2_9.9.9_amd64.deb";
        let sums = format!("{}  {name}\nnot a sums line\n", sha256_hex(PKG));
        assert!(matches!(
            verify_package(&sums, name, PKG).unwrap_err(),
            SumsError::MalformedLine { line_no: 2, .. }
        ));
    }

    #[test]
    fn multibyte_garbage_does_not_panic() {
        let line = format!("{}é  K2.deb", "a".repeat(63));
        assert!(parse_sums(&line).is_err());
    }
}
