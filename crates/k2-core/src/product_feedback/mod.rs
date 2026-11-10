//! Send feedback to K2 (prd-k2-product-feedback-v1 rev 3, 0.46.11).
//!
//! People (the top-bar button) and agents (`k2 send-feedback`) send
//! feedback about K2 to Alakazam Labs. The daemon keeps it in a durable
//! outbox ([`store`], migration 0285), proves which box it is with a
//! 5-minute token from the K2 Connect relay ([`wire::fetch_token`]) and
//! posts it to the intake at [`DEFAULT_ENDPOINT`] ([`wire::post_submission`]).
//!
//! This is **not** `k2 feedback` / `k2 tickets` (an agent asking its own
//! human on the same box): no table, route, event or CLI verb here uses the
//! bare word "feedback".
//!
//! What leaves the box (the wire contract, PRD §4 / §7.1, public on
//! purpose so people can read what their box sends) is exactly
//! [`wire::submission_body`]: the category, the vault-scrubbed text, an
//! opt-in contact email, `senderKind`, `client`, the K2/OS versions, the
//! `clientId`, and only with consent the previewed [`diagnostics`]
//! sections. The sender's name, workspace and login stay on the box.
//!
//! Before the intake exists (or while it is down) rows stay `queued`; the
//! drain backs off and the settings answer says plainly that the service
//! isn't reachable yet. Nothing is lost.

pub mod diagnostics;
pub mod settings;
pub mod store;
pub mod wire;

/// The intake, compiled in (PF20).
pub const DEFAULT_ENDPOINT: &str = "https://feedback.akzm.k2.dev";
/// Tests and dev only: another intake base URL (the `K2_PUSH_GATEWAY_URL`
/// pattern).
pub const ENDPOINT_ENV: &str = "K2_PRODUCT_FEEDBACK_URL";
/// Where people write when their box can't send (PF7, Rosson 11-06).
pub const CONTACT_ADDRESS: &str = "hello@alakazamlabs.com";
/// The text a person or agent types, in bytes (PRD §4).
pub const MAX_TEXT_BYTES: usize = 20_000;
/// The intake's own ceiling (`octet_length(body) BETWEEN 1 AND 20480`): the
/// scrubbed text may grow a little (`[k2v:NAME]`), never past this.
pub const MAX_STORED_TEXT_BYTES: usize = 20_480;
/// All diagnostics sections together (§6a).
pub const MAX_DIAGNOSTICS_BYTES: usize = 256 * 1024;
/// One contact address.
pub const MAX_EMAIL_BYTES: usize = 254;
pub const CATEGORIES: &[&str] = &["bug", "idea", "praise", "other"];
pub const CLIENTS: &[&str] = &["desktop", "web", "cli"];
/// The feature key in `/boot-status` (clients hide the button on servers
/// without it).
pub const FEATURE: &str = "product-feedback-v1";

/// The intake base URL in use: [`ENDPOINT_ENV`] when set (non-blank), else
/// [`DEFAULT_ENDPOINT`]. No trailing slash.
pub fn endpoint() -> String {
    match std::env::var(ENDPOINT_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => DEFAULT_ENDPOINT.to_string(),
    }
}

/// The audience the relay token is minted for: the intake in use.
pub fn audience() -> String {
    endpoint()
}

/// A new clientId: `pf_` + 22 base62 (the intake's idempotency key).
pub fn new_client_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(25);
    out.push_str("pf_");
    // Rejection sampling keeps every symbol equally likely.
    let mut have = 0;
    while have < 22 {
        let mut buf = [0u8; 32];
        getrandom::getrandom(&mut buf).expect("the OS random source");
        for b in buf {
            if have == 22 {
                break;
            }
            if b < 248 {
                out.push(ALPHABET[(b % 62) as usize] as char);
                have += 1;
            }
        }
    }
    out
}

/// Whether `id` has the clientId shape.
pub fn is_client_id(id: &str) -> bool {
    id.len() == 25 && id.starts_with("pf_") && id[3..].bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A plain email shape check (one address, no spaces, a dot in the domain).
pub fn valid_email(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_EMAIL_BYTES || s.chars().any(|c| c.is_whitespace() || c == ',' || c == ';') {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else { return false };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

/// This computer's OS version: macOS `sw_vers -productVersion`, Linux
/// `/etc/os-release` (`NAME VERSION_ID`), else empty. Read once.
pub fn os_version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let v = if cfg!(target_os = "macos") {
            crate::bg_proc::command("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default()
        } else if cfg!(target_os = "linux") {
            std::fs::read_to_string("/etc/os-release").map(|s| os_release_label(&s)).unwrap_or_default()
        } else {
            String::new()
        };
        v.chars().filter(|c| !c.is_control()).take(64).collect()
    })
    .clone()
}

/// `NAME VERSION_ID` from an os-release file.
pub fn os_release_label(s: &str) -> String {
    let field = |k: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    [field("NAME"), field("VERSION_ID")].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// A client-sent short field (version, locale): printable, no spaces,
/// at most `max` chars; anything else is dropped.
pub fn short_field(s: &str, max: usize) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t.len() <= max && t.chars().all(|c| c.is_ascii_alphanumeric() || "._-+@".contains(c)))
        .then(|| t.to_string())
}

/// Milliseconds since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The text a person or agent typed, checked and scrubbed (PF1): 1 to
/// [`MAX_TEXT_BYTES`] bytes before scrubbing, vault values replaced by
/// `[k2v:NAME]` (the exact-value redactor the daemon installs at boot).
pub fn clean_text(raw: &str) -> Result<String, (&'static str, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(("text_required", "Write what you want to tell Alakazam Labs.".into()));
    }
    if raw.len() > MAX_TEXT_BYTES {
        return Err((
            "text_too_long",
            format!("Feedback is at most {MAX_TEXT_BYTES} bytes; this is {}.", raw.len()),
        ));
    }
    let scrubbed = crate::redaction::redact(trimmed);
    if scrubbed.len() > MAX_STORED_TEXT_BYTES {
        return Err((
            "text_too_long",
            format!("Feedback is at most {MAX_TEXT_BYTES} bytes once secrets are taken out."),
        ));
    }
    Ok(scrubbed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_have_the_wire_shape_and_differ() {
        let a = new_client_id();
        let b = new_client_id();
        assert!(is_client_id(&a), "{a}");
        assert!(is_client_id(&b), "{b}");
        assert_ne!(a, b);
        assert!(!is_client_id("pf_short"));
        assert!(!is_client_id("xx_0123456789012345678901"));
        assert!(!is_client_id("pf_01234567890123456789-1"));
    }

    #[test]
    fn email_shape() {
        assert!(valid_email("a@b.co"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("a b@c.de"));
        assert!(!valid_email("a@b.co,c@d.ef"));
        assert!(!valid_email("@b.co"));
        assert!(!valid_email(""));
    }

    #[test]
    fn text_limits() {
        assert_eq!(clean_text("   ").unwrap_err().0, "text_required");
        let long = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(clean_text(&long).unwrap_err().0, "text_too_long");
        assert_eq!(clean_text("  the tab froze  ").unwrap(), "the tab froze");
        assert_eq!(clean_text(&"y".repeat(MAX_TEXT_BYTES)).unwrap().len(), MAX_TEXT_BYTES);
    }

    #[test]
    fn os_release_and_short_fields() {
        assert_eq!(os_release_label("NAME=\"Arch Linux\"\nVERSION_ID=rolling\n"), "Arch Linux rolling");
        assert_eq!(os_release_label("NAME=Ubuntu\n"), "Ubuntu");
        assert_eq!(short_field(" 0.46.11 ", 32).as_deref(), Some("0.46.11"));
        assert_eq!(short_field("en_US", 35).as_deref(), Some("en_US"));
        assert_eq!(short_field("1 2", 32), None);
        assert_eq!(short_field(&"9".repeat(40), 32), None);
        assert_eq!(short_field("<script>", 32), None);
    }

    #[test]
    fn endpoint_default_is_the_akzm_intake() {
        let _env = crate::test_env::lock();
        let _g = crate::test_env::EnvVar::remove(ENDPOINT_ENV);
        assert_eq!(endpoint(), "https://feedback.akzm.k2.dev");
        let _h = crate::test_env::EnvVar::set(ENDPOINT_ENV, "http://127.0.0.1:9/ ");
        assert_eq!(endpoint(), "http://127.0.0.1:9");
    }
}
