//! The kit's verb registry for a live deck: `Registry::for_pack` takes every verb in
//! `live::verbs::VERBS` as a pack verb of the keyed tier, and `capabilities` lists each one
//! there, as a host shows them.

use k2_mpc_base::errors::{capabilities, Value};
use k2_mpc_base::parity::{Registry, Source, Tier};
use k2_mpc_base::session::Config;
use k2_mpc_pptx::live::verbs::VERBS;
use k2_mpc_pptx::{Limits, PptxLive};

#[test]
fn the_kit_registry_takes_and_lists_every_deck_verb() {
    let pack = PptxLive::new(Limits::default());
    assert_eq!(Tier::of_pack(&pack), Tier::Keyed, "a deck's docs are keyed");
    let r = Registry::for_pack(&pack);
    assert!(r.is_ok(), "the deck's verbs are refused: {:?}", r.err());
    let r = r.unwrap();
    assert!(!VERBS.is_empty(), "the deck declares no verbs");
    for v in VERBS {
        let spec = r
            .get(v.name())
            .unwrap_or_else(|| panic!("{} is not in the registry", v.name()));
        assert_eq!(spec.source, Source::Pack, "{}", v.name());
        assert_eq!(spec.tiers, [Tier::Keyed].as_slice(), "{}", v.name());
    }
    let caps = capabilities(&[&r], &Config::default());
    let rows = match caps.get("verbs") {
        Some(Value::List(l)) => l,
        other => panic!("capabilities has no verbs list: {other:?}"),
    };
    let mut listed = 0;
    for v in VERBS {
        let row = rows
            .iter()
            .find(|row| row.get("name").and_then(|n| n.as_str()) == Some(v.name()))
            .unwrap_or_else(|| panic!("{} is not in capabilities", v.name()));
        assert_eq!(
            row.get("tiers").map(|t| t.to_json()),
            Some("[\"keyed\"]".to_owned()),
            "{}'s tiers in capabilities",
            v.name()
        );
        listed += 1;
    }
    assert_eq!(listed, VERBS.len(), "every deck verb listed");
}
