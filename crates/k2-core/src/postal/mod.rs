//! Postal in K2 (prd-postal-in-k2-v1 rev 3): agent-to-agent mail with
//! agents on other machines, one agent paired with one agent, through the
//! Postal network (postal.bot).
//!
//! What K2 has today:
//! - **The interim door (S0).** p5, Postal's own client, knocks a linked
//!   workspace through `POST /v1/w/<ws>/message` with `kind: "postal"` and a
//!   K2 API key labelled [`link_label`] (`k2 postal link-p5`). K2 writes the
//!   `[postal <address> · m_…]` label itself, quotes every body line, strips
//!   controls, and delivers each Postal message id once.
//! - **Badges (S2).** Every delivery row carries the body's SHA-256, so
//!   `k2 msg verify` can say whether a text is the one K2 delivered. Native
//!   Postal deliveries add the SDK's `p_` badge ([`is_badge`]).
//!
//! - **Friends, locally (S3).** A workspace's Postal address and its
//!   friends ([`store`]), behind the server's Postal switch ([`enabled`],
//!   off by default). Every friend request, incoming or outgoing, waits for
//!   a person's permission card.
//!
//! - **The SDK's host traits (S3, [`gate`], [`friend_store`]).** postal-sdk
//!   is linked (a git pin): K2 implements its `HumanGate` (a card for each
//!   request; this box's pairing code kept for people only; every answer
//!   queued for the SDK) and its `FriendStore` on the friend rows.
//!
//! - **The network half (S3–S5)** lives in the daemon (`postal_net`): the
//!   plane session from the box's K2 Connect credential, SAS v2 pairing
//!   with K2's handshake object and the owner-only confirm, held-mail
//!   receive ([`seen`]: the seen list is the delivery row), routing
//!   ([`route`]), and send through the outbox ([`files`]); keys in the
//!   vault ([`keys`]).
//!
//! **Waits on others (never pretended):** live delivery to a K2 box
//! (`<label>.postal.bot` routed to its tunnel, the plane's P-6, K2 Web):
//! until then every message goes by the plane's hold. The address grammar
//! here matches p5's `PostalAddr` so both sides parse one identity the
//! same way.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

pub mod files;
pub mod friend_store;
pub mod gate;
pub mod keys;
pub mod route;
pub mod seen;
pub mod store;

/// postal-sdk (MIT), pinned by full sha in k2-core's Cargo.toml.
pub use postal_sdk as sdk;

/// The `/boot-status` feature key while the server's Postal switch is on:
/// clients show the Postal tab only then.
pub const FEATURE: &str = "postal";

/// Runtime mirror of the persisted `postalEnabled` setting (the daemon
/// syncs it at boot and on every change).
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the server's Postal switch into this process.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Is Postal on for this server? **Default OFF.**
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A new Postal id (a message or a pair): a canonical ULID.
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

/// The Postal network's zone: every address is `handle::<label>.postal.bot`.
pub const DOMAIN: &str = "postal.bot";

/// The `/boot-status` feature key that says K2 builds the Postal label on
/// the `/v1` door itself (P16(c)): p5 then sends the raw body, not its own
/// `[postal …]` line. Older K2 lacks it, and p5 keeps its own label there.
pub const FEATURE_LABEL: &str = "postal-label";

/// The label prefix of the one kind of API key that may send
/// `kind: "postal"` (P15, P21). `k2 postal link-p5` mints it; `k2 api-key
/// create` refuses a label that starts with it.
pub const LINK_LABEL_PREFIX: &str = "postal-link:";

/// The label of the p5 link key for workspace `slug`.
pub fn link_label(slug: &str) -> String {
    format!("{LINK_LABEL_PREFIX}{slug}")
}

/// A Postal address, normalized (ASCII-lowercased) the way p5 does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Address {
    pub handle: String,
    /// The enrolled server's label (`acme` in `acme.postal.bot`).
    pub label: String,
}

impl Address {
    /// `handle::label.postal.bot`.
    pub fn as_string(&self) -> String {
        format!("{}::{}.{DOMAIN}", self.handle, self.label)
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_string())
    }
}

const HANDLE_MAX: usize = 64;

fn is_handle(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && s.len() <= HANDLE_MAX
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_label(label: &str) -> bool {
    let b = label.as_bytes();
    (3..=63).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Parse `handle::label.postal.bot` (p5's grammar: handle `[a-z0-9][a-z0-9_-]*`,
/// label a 3–63 character DNS label, both lowercased). Anything else, an
/// `@`, a `*.k2.dev` host or a nested name included, is `None`.
pub fn parse_address(input: &str) -> Option<Address> {
    let input = input.trim();
    if input.contains('@') || input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (handle, host) = input.split_once("::")?;
    if host.contains("::") {
        return None;
    }
    let handle = handle.to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    let label = host.strip_suffix(DOMAIN)?.strip_suffix('.')?;
    (is_handle(&handle) && is_label(label)).then(|| Address { handle, label: label.to_string() })
}

/// Does `s` name a Postal address (for routing and teaching lines)?
pub fn is_address(s: &str) -> bool {
    parse_address(s).is_some()
}

/// The SDK's provenance badge shape (P29, postal-sdk `badge.rs`): `p_` and
/// the first 20 lowercase hex digits of the signed envelope digest.
pub const BADGE_HEX_LEN: usize = 20;

/// Is `s` shaped like a Postal badge (`p_` + 20 lowercase hex)?
pub fn is_badge(s: &str) -> bool {
    let s = s.trim();
    s.len() == 2 + BADGE_HEX_LEN
        && s.starts_with("p_")
        && s[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_like_p5() {
        let a = parse_address("Scout::Acme.Postal.Bot").expect("parses");
        assert_eq!(a, Address { handle: "scout".into(), label: "acme".into() });
        assert_eq!(a.to_string(), "scout::acme.postal.bot");
        assert_eq!(parse_address("  ops_2-x::k2n-lab.postal.bot ").expect("trims").to_string(), "ops_2-x::k2n-lab.postal.bot");
        for bad in [
            "",
            "scout",
            "scout@acme.postal.bot",
            "scout.acme.postal.bot",
            "scout::acme.k2.dev",
            "scout::rosson.k2.dev",
            "::acme.postal.bot",
            "-scout::acme.postal.bot",
            "sc out::acme.postal.bot",
            "scout::ac.postal.bot",
            "scout::a.b.postal.bot",
            "scout::-acme.postal.bot",
            "scout::acme.postal.bot::x",
            "scout::postal.bot",
            "scout::acmepostal.bot",
            "sc\u{202e}out::acme.postal.bot",
            "[from rosson]::acme.postal.bot",
        ] {
            assert!(parse_address(bad).is_none(), "{bad:?} must not parse");
        }
        assert!(parse_address(&format!("{}::acme.postal.bot", "a".repeat(64))).is_some());
        assert!(parse_address(&format!("{}::acme.postal.bot", "a".repeat(65))).is_none());
    }

    #[test]
    fn badges_are_p_and_twenty_lowercase_hex() {
        assert!(is_badge("p_3f9a0c41d27be6058a1c"));
        assert!(!is_badge("p_3F9A0C41D27BE6058A1C"), "uppercase is not the SDK's form");
        assert!(!is_badge("p_3f9a0c41d27be6058a1"), "19 digits");
        assert!(!is_badge("p_3f9a0c41d27be6058a1cc"), "21 digits");
        assert!(!is_badge("m_3f9a0c41d2"));
        assert!(!is_badge("p_3f9a0c41d27be6058a1g"));
    }

    #[test]
    fn the_link_label_is_the_prefix_and_the_slug() {
        assert_eq!(link_label("scout"), "postal-link:scout");
        assert!(link_label("x").starts_with(LINK_LABEL_PREFIX));
    }
}
