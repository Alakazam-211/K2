//! The command families the binary answers itself, one module each. Each
//! does what the shell CLI's command does, byte for byte: the same
//! requests, the same output and the same exit code.
//!
//! - [`whoami`]: `k5 whoami [--json]`.
//! - [`done`]: `k5 done [--blocked "reason"] [--agent <name>]`.
//! - [`respond`]: `k5 respond [--final] <message>`.
//!
//! A family may hand a command line back to the shell CLI ([`done::run`]
//! and [`respond::run`] return `None`) when it is a form only the shell
//! answers. It decides before it prints or sends anything.

pub mod done;
pub mod respond;
pub mod whoami;

/// The port as curl takes it from the shell's URL: digits only, and a
/// port number.
pub fn curl_port(text: &str) -> Option<u16> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

/// What `printf '%s' "$out"` prints of a body the shell read into `out`
/// with `$(…)` and a sentinel: every byte but the NULs.
pub fn without_nuls(body: &[u8]) -> Vec<u8> {
    body.iter().copied().filter(|b| *b != 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_as_curl_reads_it() {
        assert_eq!(curl_port("4100"), Some(4100));
        for bad in ["", "41o0", " 4100", "4100 ", "-1", "65536", "+1"] {
            assert_eq!(curl_port(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn nuls_go_and_nothing_else() {
        assert_eq!(without_nuls(b"a\0b\n\n"), b"ab\n\n");
        assert_eq!(without_nuls(b""), b"");
    }
}
