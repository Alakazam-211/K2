//! The zone K2 boxes are named under, in one place.
//!
//! Every box that uses K2 Connect answers at `<label>.k2.dev`, and its
//! Apps and services at names under that. Code that builds such a name,
//! or asks whether a host is one of ours, goes through here, so a second
//! zone is a change to this file and not to every caller.
//! `scripts/garden/zone-gate.py` keeps new `k2.dev` literals out of the
//! rest of the Rust code.
//!
//! Nothing here reads settings or the network. Service addresses
//! (relay, cert, push, ...) still live with their own modules.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zone_and_its_suffix_agree() {
        assert_eq!(ZONE, "k2.dev");
        assert_eq!(ZONE_SUFFIX, ".k2.dev");
        assert_eq!(ZONE_SUFFIX, format!(".{ZONE}"));
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
