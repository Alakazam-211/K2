//! The kit's view index of fixed BLEND samples, pinned whole as golden snapshots
//! (`tests/kit_view_golden/`, the kit's own pretty JSON, which an editor's mocks can read as it
//! is). Any difference from a golden fails, naming each path; a change made on purpose is
//! re-blessed (`kit_view_golden`, at its top).
//!
//! - `outliner`: the outliner scene the live rows use (`live_support::scene`), its whole index
//!   on the live tier.
//! - `rig`: a placed scene with `Arm` parented to `Base` through a parent inverse that turns and
//!   moves it (not the identity), and a collection `Crowd` of [`CROWD`] objects read in pages:
//!   exactly two at the budget [`page_budget`] picks. The golden holds the whole index and each
//!   page's frame with its units' addresses; the pages' units are checked to be the index's
//!   (the scene is `live_support::rig`, which the live rows use too).

mod kit_view_golden;
mod live_support;

use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::placement::Step;
use k2_mpc_base::view_index::{Unit, UnitsPage, ViewIndex, VIEW_PAGE_MIN};
use live_support::{fx, rig, scene, ARM_INVERSE, CROWD};

/// The live index of `bytes`, as an agent reads it.
fn index_of(tag: &str, bytes: &[u8]) -> ViewIndex {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    let a = f.agent(&s);
    s.view_index(a)
        .expect("the view index reads")
        .expect("the pack has a view index")
}

/// The page budget for `units` of `container`: the kit's page of exactly their first half (with
/// the longest `next` a page can have, as the kit counts a page's frame), or the kit's least,
/// whichever is more, and room for a few more bytes, since later units' addresses can be a digit
/// longer. Over the whole container's units, so two pages hold them.
fn page_budget(container: &str, units: &[Unit]) -> usize {
    let half = units.len().div_ceil(2);
    let first = [Some(units.len()), None]
        .into_iter()
        .map(|next| {
            UnitsPage {
                container: container.to_owned(),
                from: 0,
                units: units[..half].to_vec(),
                next,
                total: units.len(),
            }
            .to_value()
            .to_json()
            .len()
        })
        .max()
        .unwrap_or(0);
    first.max(VIEW_PAGE_MIN) + 256
}

fn golden(name: &str, got: &Json) {
    if let Err(e) = kit_view_golden::check(name, got) {
        panic!("{e}");
    }
}

#[test]
fn the_outliner_view_is_the_golden() {
    let ix = index_of("golden-outliner", &scene());
    golden("outliner", &ix.to_value());
}

#[test]
fn the_rig_view_and_its_pages_are_the_golden() {
    let ix = index_of("golden-rig", &rig());
    // the sample keeps what it is for: Arm is placed on Base, its parent inverse first
    let unit = |name: &str| {
        ix.units
            .iter()
            .find(|u| u.name == name)
            .unwrap_or_else(|| panic!("no unit {name}"))
    };
    let (base, arm) = (unit("Base"), unit("Arm"));
    let p = arm.placement.as_ref().expect("Arm is placed");
    assert_eq!(
        p.relative_to.as_deref(),
        Some(base.key.as_str()),
        "Arm is placed on Base"
    );
    assert!(
        matches!(p.steps.first(), Some(Step::Matrix(m)) if *m == ARM_INVERSE.map(f64::from)),
        "Arm's first step is its parent inverse: {:?}",
        p.steps
    );
    // Crowd, read in pages
    let crowd = ix
        .containers
        .iter()
        .find(|c| c.title.as_deref() == Some("Crowd"))
        .expect("a Crowd container")
        .key
        .clone();
    let all = ix.units_of(&crowd).expect("Crowd's units");
    assert_eq!(all.len(), CROWD, "Crowd's units");
    let budget = page_budget(&crowd, all);
    let mut pages: Vec<UnitsPage> = Vec::new();
    let mut from = Some(0);
    while let Some(at) = from {
        assert!(
            pages.len() < 2,
            "Crowd spans more than two pages at {budget} bytes"
        );
        let page = ix.units_page(&crowd, at, budget).expect("a page");
        from = page.next;
        pages.push(page);
    }
    assert_eq!(pages.len(), 2, "Crowd spans two pages at {budget} bytes");
    let paged: Vec<Unit> = pages.iter().flat_map(|p| p.units.clone()).collect();
    assert_eq!(paged, all, "the pages' units are the index's, in order");
    let frames = pages.iter().map(|p| {
        Json::obj([
            ("container", Json::from(&p.container)),
            ("from", Json::from(p.from)),
            ("next", Json::from(p.next)),
            ("total", Json::from(p.total)),
            ("units", Json::list(p.units.iter().map(|u| u.key.as_str()))),
        ])
    });
    golden(
        "rig",
        &Json::obj([
            ("index", ix.to_value()),
            (
                "crowd_pages",
                Json::obj([
                    ("budget", Json::from(budget)),
                    ("pages", Json::List(frames.collect())),
                ]),
            ),
        ]),
    );
}
