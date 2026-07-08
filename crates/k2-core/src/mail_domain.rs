//! Mail-domain normalization — the ONE helper used at EVERY boundary
//! (prd-email-server-v1 pre-mortem #14).
//!
//! `Ünïcode.example` vs punycode, trailing dots, mixed case: stored
//! inconsistently they mean DKIM signed for one form and DMARC checked
//! for another. So every boundary (CLI in, API in, UI in) funnels
//! through [`normalize_mail_domain`], only the normalized form
//! (lowercase punycode A-label, no trailing dot) is ever stored, and
//! display-decoding back to Unicode happens at the edge.

/// Longest total domain length (RFC 1035: 253 octets in presentation
/// form, i.e. without the trailing dot).
const MAX_DOMAIN_LEN: usize = 253;

/// Longest single label (RFC 1035: 63 octets).
const MAX_LABEL_LEN: usize = 63;

/// Normalize a user-supplied mail domain to its canonical stored form:
///
/// 1. trim surrounding whitespace,
/// 2. strip ONE trailing dot (FQDN spelling `acme.dev.`),
/// 3. IDN → punycode A-labels + lowercase (UTS-46 via the `idna`
///    crate — `Ünïcode.example` → `xn--nicode-2ya.example`),
/// 4. validate every label: non-empty, ≤ 63 chars, `[a-z0-9-]` only,
///    no leading/trailing hyphen; total length ≤ 253.
///
/// Errors are one-line and name the offending input (comprehension-gate
/// style); they never echo anything but the domain itself.
pub fn normalize_mail_domain(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("domain must not be empty".to_string());
    }
    // FQDN spelling: exactly one trailing dot is fine; the A-label
    // conversion below rejects the empty labels a double dot leaves.
    let no_dot = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if no_dot.is_empty() {
        return Err("domain must not be empty".to_string());
    }

    // UTS-46: lowercases, maps, and punycodes non-ASCII labels. This
    // rejects most garbage (empty labels, disallowed characters) but
    // is deliberately backed up by the explicit label check below —
    // non-strict UTS-46 lets a few things through (e.g. underscores)
    // that a MAIL domain must never contain.
    let ascii = idna::domain_to_ascii(no_dot)
        .map_err(|_| format!("invalid domain '{trimmed}': not a valid internationalized domain name"))?;

    if ascii.is_empty() {
        return Err(format!("invalid domain '{trimmed}': empty after normalization"));
    }
    if ascii.len() > MAX_DOMAIN_LEN {
        return Err(format!(
            "invalid domain '{trimmed}': longer than {MAX_DOMAIN_LEN} characters"
        ));
    }
    for label in ascii.split('.') {
        if label.is_empty() {
            return Err(format!("invalid domain '{trimmed}': empty label"));
        }
        if label.len() > MAX_LABEL_LEN {
            return Err(format!(
                "invalid domain '{trimmed}': label '{label}' longer than {MAX_LABEL_LEN} characters"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "invalid domain '{trimmed}': label '{label}' starts or ends with a hyphen"
            ));
        }
        if !label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(format!(
                "invalid domain '{trimmed}': label '{label}' contains characters outside a-z, 0-9, '-'"
            ));
        }
    }
    Ok(ascii)
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_passes_through() {
        assert_eq!(normalize_mail_domain("acme.dev"), Ok("acme.dev".to_string()));
        assert_eq!(
            normalize_mail_domain("mail.acme-corp.co.uk"),
            Ok("mail.acme-corp.co.uk".to_string())
        );
        // Single-label domains are allowed (loopback test boxes).
        assert_eq!(normalize_mail_domain("localhost"), Ok("localhost".to_string()));
    }

    #[test]
    fn uppercase_lowercases() {
        assert_eq!(normalize_mail_domain("ACME.Dev"), Ok("acme.dev".to_string()));
    }

    #[test]
    fn trailing_dot_stripped() {
        assert_eq!(normalize_mail_domain("acme.dev."), Ok("acme.dev".to_string()));
        // FQDN + case + whitespace all at once.
        assert_eq!(normalize_mail_domain("  ACME.dev.  "), Ok("acme.dev".to_string()));
    }

    #[test]
    fn idn_becomes_punycode_a_label() {
        // The pre-mortem's own fixture: Ünïcode.example.
        assert_eq!(
            normalize_mail_domain("Ünïcode.example"),
            Ok("xn--ncode-cta3g.example".to_string())
        );
        // Round stability: normalizing the A-label form is a no-op.
        assert_eq!(
            normalize_mail_domain("xn--ncode-cta3g.example"),
            Ok("xn--ncode-cta3g.example".to_string())
        );
        // A second script for good measure (München).
        assert_eq!(
            normalize_mail_domain("münchen.de"),
            Ok("xn--mnchen-3ya.de".to_string())
        );
    }

    #[test]
    fn empties_and_invalid_labels_rejected() {
        for bad in [
            "",
            "   ",
            ".",
            "..",
            "acme..dev",       // empty label
            ".acme.dev",       // leading empty label
            "-acme.dev",       // leading hyphen
            "acme-.dev",       // trailing hyphen
            "ac me.dev",       // whitespace inside
            "acme_corp.dev",   // underscore (host label, not mail domain)
            "acme.dev/mail",   // path junk
            "user@acme.dev",   // pasted an address, not a domain
        ] {
            assert!(
                normalize_mail_domain(bad).is_err(),
                "'{bad}' must be rejected"
            );
        }
        // A 64-char label and a >253-char domain are both rejected.
        let long_label = format!("{}.dev", "a".repeat(64));
        assert!(normalize_mail_domain(&long_label).is_err(), "64-char label");
        let long_domain = std::iter::repeat("abcdefgh")
            .take(32)
            .collect::<Vec<_>>()
            .join(".");
        assert!(long_domain.len() > MAX_DOMAIN_LEN);
        assert!(normalize_mail_domain(&long_domain).is_err(), ">253-char domain");
    }
}
