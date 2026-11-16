//! The timeline's view schema for the kit's view index ([`VIEW`]), its addresses
//! ([`address_of`], [`key_of`]) and what it derives: each track's label and kind
//! ([`KIND_LABEL`], [`KIND`]), each clip's start timecode ([`derive`](fn@derive), [`TC`]) and
//! each clip's span on its `geom` page ([`page_derived`]).
//!
//! **Lanes and bars.** Tracks are the containers, in their stacking order (a later track
//! composites over an earlier one: the `timeline` map's `stacking` is `last_on_top`, in every
//! format); items the units, on their track, with their kind, name and whether they are
//! disabled. A track the file disables (`enabled` `0`: a hidden video track; `muted` `1`: a
//! muted audio track) is reported in the store and stays a lane of the view, never hidden: how
//! it looks is the host's call. The geometry is `geom`, two parts: X is the record start, W
//! the duration, in ticks (`native_unit` `tick`). A timeline has no page: the view picks its
//! zoom. Markers aren't units: they come with the keyed state.
//!
//! **Each track's label and kind.** Every container (a track) carries two derived values, so a
//! lane's header comes from the view alone:
//! - [`KIND_LABEL`] (`kind_label`): the track's label by the format's convention, its kind's
//!   letter and its place among the tracks of its kind, counted in stacking order from the
//!   bottom: the first video track is `V1`, the second audio track `A2`, any other kind `O1`
//!   ([`super::model::TrackLabels`], the rule the read labels the file's tracks with, so it is
//!   the label `track list` prints). Never the track's name: that is the container's title, and
//!   a `track rename` changes the title only.
//! - [`KIND`] (`kind`): `video`, `audio` or `other`, the tracks list's value (the store's own
//!   word for it, the kind `track list` prints).
//!
//! The kit carries them on each container of the view index (and a host's read of one), on the
//! keyed watch's snapshot containers (`derived`, `{"kind_label": "V1", "kind": "video"}`), and in
//! a change frame (`derived.containers.<track>.<name>`) for each track whose value the change
//! moved, and only those: a person's hand edit that moves a track up or down the stack moves the
//! labels of the tracks of its kind it passes; an outside save's new track comes with both. An
//! outside save pairs tracks by their name and kind ([`super::pair_tracks`]), so a track the save
//! adds below others is a new container and moves the labels of the kept tracks of its kind
//! above it: each keeps its key, and the frame carries its new label. A rename moves neither value, so its frame carries none. A client from before the values reads
//! the containers as it did (they are added only).
//!
//! **Time.** A participant's time position (the kit's playhead presence) counts in ticks:
//! `time_unit` `tick`, so a playhead's position is a record time in ticks as a decimal string, as
//! `geom` is. An agent's edit verb leaves its playhead at its edit point
//! ([`super::verbs`]); a person's is theirs, set by their client and never moved by a verb. A
//! client from before the field reads the schema as it did (it is added only).
//!
//! **Each clip's timecode.** Every unit (a clip, a transition, a nest) carries one derived value,
//! [`TC`] (`tc`): its record start's timecode at the timeline's rate, counted from the timeline's
//! start timecode (the `timeline` map's `rate` and `tc`), `HH:MM:SS:FF`: a clip one second into a
//! 24 fps timeline that starts at 01:00:00:00 reads `01:00:01:00`. Frames are labelled at the
//! rate's whole base (30 a second at `30000/1001`). A unit whose start isn't on the rate's frame
//! grid, or a timeline whose rate has no timecode or that has no rate or start timecode, has
//! none. The kit carries it on each unit of the view index and of the keyed watch's snapshot
//! units pages (`derived.tc`), and in a change frame (`derived.units.<unit>.tc`) for each unit
//! whose `tc` the change moved: a clip moved or trimmed at its start, or every unit when the
//! `timeline` map's rate, start timecode or drop-frame flag changes; a unit whose start didn't
//! move isn't in it. The rate and start timecode themselves come with the keyed state (the
//! snapshot's page of the `timeline` root, before the units pages). The span on the `geom` page
//! ([`page_derived`]) is labelled the same way. A client from before the value reads the units
//! as it did (it is added only).
//!
//! **Drop-frame.** When the `timeline` map's `drop_frame` is `1` and the rate has drop-frame
//! timecode (`30000/1001`, `60000/1001`), every label here is drop-frame, `HH:MM:SS;FF`: a start
//! at frame 107892 at `30000/1001` reads `01:00:00;00`, not the non-drop `00:59:56:12`. At any
//! other rate the flag changes nothing. Which files carry the flag:
//! - **FCPXML**: a sequence's `tcFormat="DF"` (the tier-0 model's `drop_frame`, which the
//!   structure index's timecodes use). FCPXML has no live pack yet.
//! - **OTIO** (the live pack's format): its schema has no drop-frame field (a time is a value
//!   and a rate). A timeline's `metadata` may hold a drop-frame (DF/NDF) entry
//!   ([`crate::otio::drop_frame_entry`]): exactly `DF` at `30000/1001` or `60000/1001` opens
//!   with `drop_frame` `1`; `NDF`, no entry, any other value, and `DF` at any other rate open
//!   with `0`, never declined. The flag is read-only (the write-back refuses a changed one), and
//!   the entry is never written.
//! - **PRPROJ**: none read. The reader reads a sequence's frame rate but no start timecode and
//!   no timecode setting, so its timelines are non-drop.
//!
//! **Addresses** are key-derived: `@trk:N`, `@item:N`, `@mk:N`, `@med:N` for `tN`, `iN`, `kN`,
//! `mN`. The blob tier names nodes by the model's keys (`@d-seq:1/d-trk:V1`).

use std::collections::{BTreeMap, HashMap, HashSet};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, KeyedRoot, KeyedState, Value};
use k2_mpc_base::grid::PageDerived;
use k2_mpc_base::view_index::{
    ContainerFields, Derived, FieldClass, GeomPart, Geometry, Src, Unit, UnitFields, ViewCarrier,
    ViewIndex, ViewSchema, ViewSource, ViewTouched,
};

use super::model::{
    f, key, key_num, prop, t, TrackLabels, ITEMS, ITEM_PROPS, MARKERS, MEDIA, TIMELINE, TRACKS,
};
use super::ticks;
use crate::time::{Rate, Timecode};

/// The derived value per unit: its record start's timecode ([`derive`](fn@derive)).
pub const TC: &str = "tc";

/// The derived values per unit, in their order.
pub const UNIT_DERIVED: &[&str] = &[TC];

/// The derived value per container: its track's label (`V1`, `A2`; [`TrackLabels`]).
pub const KIND_LABEL: &str = "kind_label";

/// The derived value per container: its track's kind (`video`, `audio`, `other`).
pub const KIND: &str = "kind";

/// The derived values per container, in their order.
pub const CONTAINER_DERIVED: &[&str] = &[KIND_LABEL, KIND];

/// The timeline's view schema (live tier).
pub const VIEW: ViewSchema = ViewSchema {
    containers: TRACKS,
    units: ITEMS,
    unit_container_field: f::TRACK,
    container_id_field: None,
    native_unit: "tick",
    container_fields: ContainerFields {
        title: Src::Field(f::NAME),
        // a disabled track is reported (`enabled`, `muted`), never hidden: the host decides how
        // it looks
        hidden: Src::None,
        layout: Src::None,
        ..ContainerFields::NONE
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        name: Src::Field(f::NAME),
        hidden: Src::Field(f::OFF),
        editable_text: Src::Values {
            field: f::EDIT,
            values: &[("1", "plain"), ("0", "never")],
        },
        ..UnitFields::NONE
    },
    geometry: Some(Geometry {
        field: f::GEOM,
        parts: &[GeomPart::X, GeomPart::W],
        rot_unit: "",
    }),
    field_class: &[
        (f::GEOM, FieldClass::Geometry),
        (f::SRC, FieldClass::Geometry),
        (f::NAME, FieldClass::Text),
        (f::OFF, FieldClass::Visibility),
        (f::MUTED, FieldClass::Visibility),
        (f::ENABLED, FieldClass::Visibility),
        (f::TRACK, FieldClass::Meta),
        (f::MEDIA, FieldClass::Meta),
        (f::TR, FieldClass::Meta),
        (f::RATE, FieldClass::Meta),
        (f::NID, FieldClass::Meta),
        (f::EDIT, FieldClass::Meta),
        (f::RETIMED, FieldClass::Meta),
        (f::LOCKED, FieldClass::Meta),
    ],
    derived: Derived {
        containers: &[],
        units: &["tc"],
    },
    unit_derived: UNIT_DERIVED,
    container_derived: CONTAINER_DERIVED,
    // a playhead's position: record ticks, as `geom`
    time_unit: Some("tick"),
    ..ViewSchema::NONE
};

const ADDR: &[(&str, char, &str)] = &[
    (TRACKS, 't', "@trk:"),
    (ITEMS, 'i', "@item:"),
    (MARKERS, 'k', "@mk:"),
    (MEDIA, 'm', "@med:"),
];

/// The address of element `key` of `root`: `@trk:N`, `@item:N`, `@mk:N`, `@med:N`; anything
/// else as it is.
pub fn address_of(root: &str, key: &Key) -> String {
    ADDR.iter()
        .find(|(r, _, _)| *r == root)
        .and_then(|(_, c, p)| key_num(key.as_str(), *c).map(|n| format!("{p}{n}")))
        .unwrap_or_else(|| key.as_str().to_owned())
}

/// [`address_of`]'s inverse: `None` when `address` can't name one of `root`'s elements.
pub fn key_of(root: &str, address: &str) -> Option<Key> {
    let (_, c, p) = ADDR.iter().find(|(r, _, _)| *r == root)?;
    let n = key_num(&format!("n{}", address.strip_prefix(p)?), 'n')?;
    Some(key(*c, n))
}

/// The timecode of record time `ticks` on a timeline starting at `tc` ticks, at `rate`; `None`
/// when the time isn't on the rate's frame grid or the rate has no timecode. Drop-frame
/// (`HH:MM:SS;FF`) when `drop` and the rate has drop-frame timecode (`30000/1001`,
/// `60000/1001`); at any other rate `drop` changes nothing.
pub fn timecode(ticks_at: i64, tc: i64, rate: Rate, drop: bool) -> Option<String> {
    let frame = ticks::frame(rate)?;
    let at = ticks_at.checked_add(tc)?;
    if at % frame != 0 {
        return None;
    }
    rate.timecode_base()?;
    // planted: the flag read but ignored when labelling
    let drop = drop && !crate::mutant::active("DropFrameIgnored");
    let drop = crate::model::drop_frame_at(Some(rate), drop);
    Timecode::from_frames(at / frame, rate, drop)
        .ok()
        .map(|t| t.to_string())
}

/// The root `name` of `state`, when it is a map.
fn map_of<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

/// The live elements of the list root `name` of `state`, in order.
fn list_of<'a>(state: &'a KeyedState, name: &str) -> &'a [(Key, Value)] {
    state
        .roots
        .iter()
        .find_map(|(r, root)| match root {
            KeyedRoot::List { live, .. } if r == name => Some(live.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

/// Every container's [`KIND_LABEL`] and [`KIND`], worked out again from the tracks list (its
/// order and each track's kind, its value): a track's label counts the tracks of its kind below
/// it, so one track's change can move others' labels.
fn derive_containers(state: &KeyedState, index: &mut ViewIndex) {
    let mut labels = TrackLabels::default();
    let by: HashMap<String, (String, &str)> = list_of(state, TRACKS)
        .iter()
        .map(|(k, v)| {
            let kind = v.as_str().unwrap_or("");
            (address_of(TRACKS, k), (labels.next(kind), kind))
        })
        .collect();
    for c in &mut index.containers {
        c.derived
            .retain(|(n, _)| !CONTAINER_DERIVED.contains(&n.as_str()));
        let Some((label, kind)) = by.get(&c.key) else {
            continue;
        };
        // planted: the label left out
        if !crate::mutant::active("KindLabelDropped") {
            c.set_derived(KIND_LABEL, label.clone());
        }
        // planted: an audio track's kind given as video
        let kind = match *kind {
            "audio" if crate::mutant::active("AudioKindAsVideo") => "video",
            k => k,
        };
        if !kind.is_empty() {
            c.set_derived(KIND, kind);
        }
    }
}

/// The timeline's clock: its rate, its start timecode in ticks, and whether its timecode is
/// drop-frame (`drop_frame` `1`), from the `timeline` map; `None` when it lacks a rate or a
/// start timecode (no flag is non-drop).
fn clock(state: &KeyedState) -> Option<(Rate, i64, bool)> {
    let tl = map_of(state, TIMELINE)?;
    let s = |k: &str| tl.get(k).and_then(|v| v.as_str());
    Some((
        s(t::RATE).and_then(ticks::parse_rate)?,
        s(t::TC).and_then(ticks::parse)?,
        s(t::DROP_FRAME) == Some("1"),
    ))
}

/// The [`TC`] of the unit at `address`: its record start (`geom`'s X, in `props`, the items'
/// field map) as a timecode at the clock's rate, from its start timecode, drop-frame when the
/// clock is ([`timecode`]).
fn unit_tc(
    props: &BTreeMap<String, Value>,
    (rate, tc, drop): (Rate, i64, bool),
    address: &str,
) -> Option<String> {
    // planted: counted from the timeline's zero, not its start timecode
    let tc = if crate::mutant::active("TcWithoutStartTimecode") {
        0
    } else {
        tc
    };
    // planted: counted at the rate's whole base (30/1 for 30000/1001)
    let rate = match rate.timecode_base() {
        Some(b) if crate::mutant::active("TcAtWholeRate") => Rate::whole(b).unwrap_or(rate),
        _ => rate,
    };
    let k = key_of(ITEMS, address)?;
    let (start, _) = props
        .get(&prop(&k, f::GEOM))
        .and_then(|v| v.as_str())
        .and_then(ticks::parse_pair)?;
    timecode(start, tc, rate, drop)
}

/// Unit `u`'s derived values, worked out again: its [`TC`], or none.
fn derive_unit(
    props: Option<&BTreeMap<String, Value>>,
    clock: Option<(Rate, i64, bool)>,
    u: &mut Unit,
) {
    u.derived.retain(|(n, _)| n != TC);
    if let (Some(props), Some(clock)) = (props, clock) {
        if let Some(t) = unit_tc(props, clock, &u.key) {
            u.set_derived(TC, t);
        }
    }
}

/// The planted fault `TcFromFileBytes`: the items' fields read from the file's bytes as the kit
/// last wrote or read them (the carrier, with its keys minted as a first open mints them), not
/// from the live store. `None` without the plant.
fn planted_file_props(carrier: Option<&ViewCarrier>) -> Option<BTreeMap<String, Value>> {
    if !crate::mutant::active("TcFromFileBytes") {
        return None;
    }
    let lt = super::model::read(carrier?.bytes(), &super::LiveLimits::default()).ok()?;
    let (keys, marks) = lt.minted();
    lt.to_content(&keys, marks, &lt.doc_id)
        .into_iter()
        .find_map(|(r, n)| match n {
            Neutral::Map(m) if r == ITEM_PROPS => Some(m),
            _ => None,
        })
}

/// What the pack derives on the kit's index, from the live store (the carrier holds nothing it
/// shows that the store doesn't): each container's [`KIND_LABEL`] and [`KIND`] (the tracks
/// list), and each unit's [`TC`] (the items' `geom` and the `timeline` map's rate, start
/// timecode and drop-frame flag). Nothing on the blob tier.
pub fn derive(source: ViewSource<'_>, index: &mut ViewIndex) {
    let ViewSource::Live(state, carrier) = source else {
        return;
    };
    derive_containers(state, index);
    let file = planted_file_props(carrier);
    let props = file.as_ref().or_else(|| map_of(state, ITEM_PROPS));
    let clock = clock(state);
    for u in &mut index.units {
        derive_unit(props, clock, u);
    }
}

/// [`derive`](fn@derive) for a change. Every container's [`KIND_LABEL`] and [`KIND`] derive
/// again when tracks (or units) came, went or moved, or the change touched a track (its kind may have
/// changed): a label counts the tracks below it, and a pass over the tracks is at most
/// [`super::model::LiveLimits::max_tracks`]. A unit's [`TC`] reads its own `geom` and the `timeline`
/// map, so the units the change touched derive again, and every unit does when the change wrote
/// the `timeline` map (its rate, start timecode or drop-frame flag may have moved: a clip's add
/// writes its key mark there too, which costs one pass over the units); the rest keep theirs.
pub fn derive_change(source: ViewSource<'_>, index: &mut ViewIndex, touched: &ViewTouched) -> bool {
    let ViewSource::Live(state, carrier) = source else {
        return false;
    };
    if touched.order || !touched.containers.is_empty() {
        derive_containers(state, index);
    }
    if crate::mutant::active("TcNotRederived") {
        // planted: kept, it says, with what the change touched left underived
        return true;
    }
    let file = planted_file_props(carrier);
    let props = file.as_ref().or_else(|| map_of(state, ITEM_PROPS));
    let clock = clock(state);
    // planted: a write of the `timeline` map (its rate or start timecode) derives nothing again
    let every = touched.roots.iter().any(|r| r == TIMELINE)
        && !crate::mutant::active("TcStartChangeIgnored");
    let again: HashSet<&str> = touched.units.iter().map(String::as_str).collect();
    for u in index
        .units
        .iter_mut()
        .filter(|u| every || again.contains(u.key.as_str()))
    {
        derive_unit(props, clock, u);
    }
    true
}

/// Each unit's start and end as timecodes, on the page of its `geom`
/// (`derived.tc.<address>.geom` = `HH:MM:SS:FF-HH:MM:SS:FF`, `;` before the frames when
/// drop-frame, as [`TC`]).
pub fn page_derived(state: &KeyedState, units: &[Key]) -> Vec<PageDerived> {
    let (Some(ip), Some((rate, tc, drop))) = (map_of(state, ITEM_PROPS), clock(state)) else {
        return Vec::new();
    };
    units
        .iter()
        .filter_map(|u| {
            let (start, dur) = ip
                .get(&prop(u, f::GEOM))
                .and_then(|v| v.as_str())
                .and_then(ticks::parse_pair)?;
            let a = timecode(start, tc, rate, drop)?;
            let b = timecode(start.checked_add(dur)?, tc, rate, drop)?;
            Some(PageDerived {
                unit: u.clone(),
                name: "tc",
                values: vec![(f::GEOM.to_owned(), format!("{a}-{b}"))],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_lead_back() {
        for (root, k, addr) in [
            (TRACKS, key('t', 3), "@trk:3"),
            (ITEMS, key('i', 41), "@item:41"),
            (MARKERS, key('k', 7), "@mk:7"),
            (MEDIA, key('m', 2), "@med:2"),
        ] {
            assert_eq!(address_of(root, &k), addr);
            assert_eq!(key_of(root, addr), Some(k));
        }
        for bad in ["@item:07", "@item:", "@item:-1", "item:1", "@trk:1"] {
            assert_eq!(key_of(ITEMS, bad), None, "{bad}");
        }
    }

    #[test]
    fn timecodes_count_from_the_start_timecode() {
        let r = Rate::whole(24).unwrap();
        let fr = ticks::frame(r).unwrap();
        assert_eq!(
            timecode(0, 3600 * 24 * fr, r, false).as_deref(),
            Some("01:00:00:00")
        );
        assert_eq!(
            timecode(25 * fr, 0, r, false).as_deref(),
            Some("00:00:01:01")
        );
        assert_eq!(timecode(fr / 2, 0, r, false), None);
        // no drop-frame at 24 fps: the flag changes nothing
        assert_eq!(
            timecode(0, 3600 * 24 * fr, r, true).as_deref(),
            Some("01:00:00:00")
        );
    }

    #[test]
    fn drop_frame_labels_at_ntsc_rates_only() {
        for (r, start) in [
            (Rate::ntsc(30).unwrap(), 107_892),
            (Rate::ntsc(60).unwrap(), 215_784),
        ] {
            let fr = ticks::frame(r).unwrap();
            assert_eq!(
                timecode(0, start * fr, r, true).as_deref(),
                Some("01:00:00;00"),
                "{r}"
            );
            assert_ne!(
                timecode(0, start * fr, r, false).as_deref(),
                Some("01:00:00:00"),
                "{r}"
            );
        }
        let r = Rate::ntsc(30).unwrap();
        let fr = ticks::frame(r).unwrap();
        assert_eq!(
            timecode(0, 107_892 * fr, r, false).as_deref(),
            Some("00:59:56:12")
        );
        let r = Rate::whole(30).unwrap();
        let fr = ticks::frame(r).unwrap();
        assert_eq!(
            timecode(0, 108_000 * fr, r, true).as_deref(),
            Some("01:00:00:00")
        );
    }
}
