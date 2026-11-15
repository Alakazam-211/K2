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

/// Every zone whose names are ours: what [`in_zone`], [`under_zone`] and
/// [`label_of`] ask about. A zone goes on this list before anything is
/// served under it, so that a name in it is never taken for someone
/// else's domain: `k5.dev` is here for that reason, and no box answers
/// under it yet.
///
/// Who should ask these: a check that REFUSES our own names (as a custom
/// domain, a mail domain, a name to issue a certificate for here). Those
/// fail closed when the list grows. Code that ACCEPTS a host because it
/// is ours (the Origin check, routing by host, a peer's address) uses
/// [`ZONE_SUFFIX`], the primary zone, until serving a second zone is
/// built on purpose: a longer list must never widen what is let in.
pub const ZONES: &[&str] = &["k2.dev", "k5.dev"];

/// The zone a box's own name is built under: what [`box_host`] uses.
/// Building names under another zone is a separate, deliberate change.
pub const PRIMARY_ZONE: &str = "k2.dev";

/// The primary zone, under its older name.
pub const ZONE: &str = PRIMARY_ZONE;

/// The primary zone as a suffix, with its dot.
pub const ZONE_SUFFIX: &str = ".k2.dev";

/// `<label>.k2.dev`: the label under the primary zone. The label is used
/// as given: no trimming, no case change, and a label that already
/// carries the zone gets it again.
pub fn box_host(label: impl std::fmt::Display) -> String {
    format!("{label}{ZONE_SUFFIX}")
}

/// What is left of `host` when the dot and name of one of our zones is
/// taken off its end: `acme` for `acme.k2.dev`, `app.acme` for
/// `app.acme.k2.dev`. `None` when the host is not under a zone of ours
/// (the bare zone itself is not under it). Compares bytes as given: a
/// caller that wants any case lowers the host first.
pub fn label_of(host: &str) -> Option<&str> {
    ZONES.iter().find_map(|zone| host.strip_suffix(zone).and_then(|rest| rest.strip_suffix('.')))
}

/// Whether `host` is a name under one of our zones (not the bare zone).
pub fn under_zone(host: &str) -> bool {
    label_of(host).is_some()
}

/// Whether `host` is one of our zones or a name under one. Compares
/// bytes as given: a caller that wants any case lowers the host first.
pub fn in_zone(host: &str) -> bool {
    ZONES.contains(&host) || under_zone(host)
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

    /// Two zones are ours, and names are still built under the first.
    /// Adding a zone to ZONES changes what counts as ours and nothing
    /// about how a box's name is built.
    #[test]
    fn the_zones_are_k2_dev_and_k5_dev_and_names_are_built_under_k2_dev() {
        assert_eq!(ZONES, &["k2.dev", "k5.dev"]);
        assert_eq!(PRIMARY_ZONE, "k2.dev");
        assert_eq!(ZONES[0], PRIMARY_ZONE);
        assert_eq!(ZONE, PRIMARY_ZONE);
        assert_eq!(ZONE_SUFFIX, ".k2.dev");
        assert_eq!(box_host("acme"), "acme.k2.dev");
    }

    /// A k5.dev name is ours for the checks that refuse our own names.
    #[test]
    fn k5_dev_names_are_in_a_zone_of_ours() {
        assert!(in_zone("k5.dev"));
        assert!(in_zone("acme.k5.dev"));
        assert!(in_zone("app.acme.k5.dev"));
        assert!(under_zone("acme.k5.dev"));
        assert!(!under_zone("k5.dev"), "the bare zone is not under itself");
        assert_eq!(label_of("acme.k5.dev"), Some("acme"));
        assert_eq!(label_of("app.acme.k5.dev"), Some("app.acme"));
        assert_eq!(label_of("k5.dev"), None);
        assert!(!in_zone("notk5.dev"), "a name that only ends in the letters is not in the zone");
        assert!(!in_zone("k5.dev.example.com"));
        assert!(!in_zone("ACME.K5.DEV"), "bytes as given: the caller lowers first");
        assert!(!in_zone("acme.k5.dev."), "a trailing dot is the caller's to trim");
    }

    /// For a name that does not end in k5.dev, label_of is
    /// `strip_suffix(".k2.dev")`, case for case.
    #[test]
    fn label_of_takes_the_zone_off_the_end() {
        for host in [
            "acme.k2.dev", "app.acme.k2.dev", "k2.dev", ".k2.dev", "notk2.dev", "k2.dev.example.com",
            "ACME.K2.DEV", "acme.k2.dev.k2.dev", "acme.k2.dev.", "", "acme", "10.0.0.5:38471.k2.dev",
        ] {
            assert_eq!(label_of(host), host.strip_suffix(".k2.dev"), "{host}");
            assert_eq!(under_zone(host), host.ends_with(".k2.dev"), "{host}");
            assert_eq!(in_zone(host), host == "k2.dev" || host.ends_with(".k2.dev"), "{host}");
        }
        assert_eq!(label_of("acme.k2.dev"), Some("acme"));
        assert_eq!(label_of("app.acme.k2.dev"), Some("app.acme"));
        assert_eq!(label_of("k2.dev"), None);
        assert_eq!(label_of("notk2.dev"), None);
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
