//! The zone K2 boxes are named under, in one place.
//!
//! Every box that uses K2 Connect answers at `<label>.k2.dev`, and its
//! Apps and services at names under that. Code that builds such a name,
//! or asks whether a host is one of ours, goes through here, so a second
//! zone is a change to this file and not to every caller.
//! `scripts/garden/zone-gate.py` keeps new `k2.dev` literals out of the
//! rest of the Rust code.
//!
//! Nothing here reads settings or the network. The service addresses
//! below are the compiled-in defaults; the modules that use them keep
//! their own names for them and their own env overrides.

/// The zone itself.
pub const ZONE: &str = "k2.dev";

/// The zone as a suffix, with its dot: what a box's full name ends with.
pub const ZONE_SUFFIX: &str = ".k2.dev";

/// `<label>.k2.dev`. The label is used as given: no trimming, no case
/// change, and a label that already carries the zone gets it again.
pub fn box_host(label: impl std::fmt::Display) -> String {
    format!("{label}{ZONE_SUFFIX}")
}

/// Whether `host` is the zone or a name under it. Compares bytes as
/// given: a caller that wants any case lowers the host first.
pub fn in_zone(host: &str) -> bool {
    host == ZONE || host.ends_with(ZONE_SUFFIX)
}

// ── K2's own services ────────────────────────────────────────────────
// These are fixed hosts, not names under a box's label. They do not
// follow ZONE: if boxes ever answer under a second zone, these stay.

/// The default relay a tunnel dials.
pub const RELAY_HOST: &str = "relay.k2.dev";
/// The list of relays (K2 Web serves it).
pub const RELAY_LIST_URL: &str = "https://k2.dev/api/tunnel/relays";
/// K2 Connect's control plane.
pub const CONNECT_BASE: &str = "https://connect.k2.dev";
/// The certificate broker's endpoint.
pub const CERT_BROKER_URL: &str = "https://cert.k2.dev/cert";
/// The push gateway.
pub const PUSH_GATEWAY_URL: &str = "https://push.k2.dev";
/// The download host for App updates.
pub const DL_BASE: &str = "https://dl.k2.dev";
/// Where product feedback is sent.
pub const FEEDBACK_ENDPOINT: &str = "https://feedback.akzm.k2.dev";
/// The website's API base (`/api/dns/*` lives there).
pub const WEB_API_BASE: &str = "https://k2.dev";
/// Where a person upgrades their plan.
pub const DASHBOARD_URL: &str = "https://k2.dev/dashboard";
/// What an App's edge-terminated web host ends with (`<sub>.app.k2.dev`).
pub const WEB_APP_HOST_SUFFIX: &str = ".app.k2.dev";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zone_and_its_suffix_agree() {
        assert_eq!(ZONE, "k2.dev");
        assert_eq!(ZONE_SUFFIX, ".k2.dev");
        assert_eq!(ZONE_SUFFIX, format!(".{ZONE}"));
    }

    /// The compiled-in service addresses, spelled out: a change to one of
    /// them is a change every running daemon makes on its next update.
    #[test]
    fn the_service_addresses_are_what_they_were() {
        assert_eq!(RELAY_HOST, "relay.k2.dev");
        assert_eq!(RELAY_LIST_URL, "https://k2.dev/api/tunnel/relays");
        assert_eq!(CONNECT_BASE, "https://connect.k2.dev");
        assert_eq!(CERT_BROKER_URL, "https://cert.k2.dev/cert");
        assert_eq!(PUSH_GATEWAY_URL, "https://push.k2.dev");
        assert_eq!(DL_BASE, "https://dl.k2.dev");
        assert_eq!(FEEDBACK_ENDPOINT, "https://feedback.akzm.k2.dev");
        assert_eq!(WEB_API_BASE, "https://k2.dev");
        assert_eq!(DASHBOARD_URL, "https://k2.dev/dashboard");
        assert_eq!(WEB_APP_HOST_SUFFIX, ".app.k2.dev");
    }

    #[test]
    fn box_host_adds_the_zone_and_nothing_else() {
        assert_eq!(box_host("acme"), "acme.k2.dev");
        assert_eq!(box_host(String::from("app.acme")), "app.acme.k2.dev");
        assert_eq!(box_host(&String::from("Acme")), "Acme.k2.dev");
        assert_eq!(box_host("acme.k2.dev"), "acme.k2.dev.k2.dev");
        assert_eq!(box_host(""), ".k2.dev");
    }

    #[test]
    fn in_zone_is_the_zone_or_a_name_under_it() {
        assert!(in_zone("k2.dev"));
        assert!(in_zone("acme.k2.dev"));
        assert!(in_zone("app.acme.k2.dev"));
        assert!(!in_zone("notk2.dev"), "a name that only ends in the letters is not in the zone");
        assert!(!in_zone("k2.dev.example.com"));
        assert!(!in_zone("ACME.K2.DEV"), "bytes as given: the caller lowers first");
        assert!(!in_zone(""));
    }
}
