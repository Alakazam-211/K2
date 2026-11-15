//! The deck's view schema for the kit's view index, its addresses and what the pack derives on
//! top of the kit's index.
//!
//! - [`VIEW`]: slides are the containers and shapes the units; a shape's `slide` field holds
//!   its slide's `sid`; the page is the deck map's `cx` × `cy` in EMU; `geom` is the one geometry
//!   field (`x,y,w,h,rot,fh,fv`); `edit` gives `editable_text`. On the blob tier, the structure
//!   index's `slide` nodes are the containers, every node under one but `notes` a unit
//!   ([`crate::index`]).
//! - [`address_of`] and [`key_of`]: a slide `s<sldId>` is `@sld:<sldId>`, a shape
//!   `s<sldId>-<cNvPr>` is `@sld:<sldId>/sp:<cNvPr>`, the structure index's own keys, so the
//!   two tiers name an element alike.
//! - [`derive`]: a slide's title (its first title placeholder's text, as the index names a
//!   slide), the layouts (the deck map's layout facts), each shape's `name_is_generic`.
//!
//! What only the file's bytes hold (fonts, render hashes, field kinds, boxes inherited from a
//! layout, a group's children) the live tier reads from the carrier the kit hands the hook
//! ([`super::view_derive`], with the pack's own view's code, [`crate::view`]); on the blob tier
//! the index reader works it out with the same code, and the version's structure index carries
//! it (its view items: [`crate::index`]).

use std::collections::BTreeSet;

use k2_mpc_base::elem::{Key, KeyedRoot};
use k2_mpc_base::view_index::{
    BlobView, ContainerFields, Derived, FieldClass, GeomPart, Geometry, Layout, NodeSrc, PageSpec,
    Src, UnitFields, ViewIndex, ViewSchema, ViewSource, VIEW_TEXT_MAX,
};

use super::model::{f, parse_key, shape_key, slide_key, DECK, SHAPES, SLIDES};
use crate::view::name_is_generic;

/// The `edit` field's values as words.
pub const EDIT_WORDS: &[(&str, &str)] = &[("1", "plain"), ("fmt", "formatted"), ("0", "never")];

/// The geometry field's parts, in order.
pub const GEOM_PARTS: &[GeomPart] = &[
    GeomPart::X,
    GeomPart::Y,
    GeomPart::W,
    GeomPart::H,
    GeomPart::Rot,
    GeomPart::FlipH,
    GeomPart::FlipV,
];

/// The deck's view schema, both tiers.
pub const VIEW: ViewSchema = ViewSchema {
    containers: SLIDES,
    units: SHAPES,
    unit_container_field: f::SLIDE,
    container_id_field: Some(f::SID),
    native_unit: "emu",
    page: Some(PageSpec {
        root: DECK,
        w: "cx",
        h: "cy",
    }),
    container_fields: ContainerFields {
        title: Src::Derived("slide_title"),
        hidden: Src::Field(f::HIDDEN),
        layout: Src::Field(f::LAYOUT),
        ..ContainerFields::NONE
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        name: Src::Field(f::NAME),
        alt: Src::Field(f::DESCR),
        text: Src::Field(f::TEXT),
        hidden: Src::Field(f::HIDDEN),
        editable_text: Src::Values {
            field: f::EDIT,
            values: EDIT_WORDS,
        },
        placeholder: Src::Field(f::PH),
    },
    geometry: Some(Geometry {
        field: f::GEOM,
        parts: GEOM_PARTS,
        rot_unit: "60000th-degree",
    }),
    field_class: &[
        (f::GEOM, FieldClass::Geometry),
        (f::TEXT, FieldClass::Text),
        (f::NOTES, FieldClass::Text),
        (f::HIDDEN, FieldClass::Visibility),
        (f::SLIDE, FieldClass::Meta),
        (f::FID, FieldClass::Meta),
        (f::NAME, FieldClass::Meta),
        (f::DESCR, FieldClass::Meta),
        (f::PH, FieldClass::Meta),
        (f::EDIT, FieldClass::Meta),
        (f::REFD, FieldClass::Meta),
        (f::MC, FieldClass::Meta),
        (f::SID, FieldClass::Meta),
        (f::LAYOUT, FieldClass::Meta),
        (f::NO_NOTES, FieldClass::Meta),
        (f::LINKED, FieldClass::Meta),
    ],
    derived: Derived {
        containers: &["title"],
        units: &["name_is_generic"],
    },
    blob: Some(BlobView {
        container_kind: "slide",
        skip_kinds: &["notes"],
        page: Some(("width", "height")),
        layout_kind: Some("layout"),
        container_title: NodeSrc::Name,
        container_layout: NodeSrc::Attr("layout"),
        unit_alt: NodeSrc::Attr("descr"),
        unit_rot: NodeSrc::Attr("rot"),
        placeholder_type: NodeSrc::Attr("placeholder"),
        placeholder_idx: NodeSrc::Attr("idx"),
        ..BlobView::NONE
    }),
    ..ViewSchema::NONE
};

/// A file id as a key or address writes it: digits, no sign, no leading zero.
fn id(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}

/// The address of element `key` of `root`: `@sld:<sldId>` for a slide, `@sld:<sldId>/sp:<cNvPr>`
/// for a shape. A key that isn't a slide's or a shape's (never in a deck) is its own address.
pub fn address_of(root: &str, key: &Key) -> String {
    match (root, parse_key(key.as_str())) {
        (SLIDES, Some((sid, None))) => format!("@sld:{sid}"),
        (SHAPES, Some((sid, Some(fid)))) => format!("@sld:{sid}/sp:{fid}"),
        _ => key.as_str().to_owned(),
    }
}

/// The key of the element of `root` that `address` names, or `None` ([`address_of`]'s inverse).
pub fn key_of(root: &str, address: &str) -> Option<Key> {
    let rest = address.strip_prefix("@sld:")?;
    match (root, rest.split_once("/sp:")) {
        (SLIDES, None) => Some(slide_key(id(rest)?)),
        (SHAPES, Some((s, f))) => Some(shape_key(id(s)?, id(f)?)),
        _ => None,
    }
}

/// The view fields the pack derives on the kit's index: on the live tier, each slide's title
/// (the text of its first title or centred-title placeholder, back to front, cut to
/// [`VIEW_TEXT_MAX`]; empty with none, as the structure index names a slide) and the layouts
/// (the deck map's facts, by id: `@layout:<id>`, its own name, its master); on both tiers,
/// each shape's `name_is_generic` ([`name_is_generic`]) and a placeholder with no type written
/// as `obj` (`ST_PlaceholderType`'s default, as the structure index gives it).
pub fn derive(source: ViewSource<'_>, index: &mut ViewIndex) {
    if let ViewSource::Live(state, _) = source {
        let deck = state.roots.iter().find_map(|(r, root)| match root {
            KeyedRoot::Map(m) if r == DECK => Some(m),
            _ => None,
        });
        let get = |k: String| -> Option<String> {
            deck.and_then(|m| m.get(&k))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let ids: BTreeSet<u64> = deck
            .into_iter()
            .flat_map(|m| m.keys())
            .filter_map(|k| {
                let (id, _) = k.strip_prefix("layout/")?.split_once('/')?;
                id.parse().ok()
            })
            .collect();
        for id in ids {
            index.layouts.push(Layout {
                key: format!("@layout:{id}"),
                name: get(format!("layout/{id}/name")).unwrap_or_default(),
                master: get(format!("layout/{id}/master")).filter(|m| !m.is_empty()),
            });
        }
        for c in &mut index.containers {
            let title = index
                .units
                .iter()
                .filter(|u| u.container == c.key)
                .find(|u| {
                    u.placeholder
                        .as_ref()
                        .is_some_and(|p| p.ty == "title" || p.ty == "ctrTitle")
                })
                .and_then(|u| u.text.as_ref())
                .map(|t| t.summary.clone())
                .unwrap_or_default();
            let (kept, _) = k2_mpc_base::index::cut_text(&title, VIEW_TEXT_MAX as u32);
            c.title = Some(kept.to_owned());
        }
    }
    for u in &mut index.units {
        u.name_is_generic = Some(name_is_generic(&u.name));
        if let Some(p) = &mut u.placeholder {
            if p.ty.is_empty() {
                p.ty = "obj".to_owned();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_the_file_ids_and_lead_back() {
        for (root, key, addr) in [
            (SLIDES, slide_key(256), "@sld:256"),
            (SHAPES, shape_key(256, 7), "@sld:256/sp:7"),
        ] {
            assert_eq!(address_of(root, &key), addr);
            assert_eq!(key_of(root, addr), Some(key));
        }
        assert_eq!(key_of(SHAPES, "@sld:256"), None, "a slide isn't a shape");
        assert_eq!(
            key_of(SLIDES, "@sld:256/sp:7"),
            None,
            "a shape isn't a slide"
        );
        for bad in [
            "@sld:0256",
            "@sld:",
            "@sld:256/sp:",
            "sld:256",
            "@sld:256/sp:07",
            "@sld:-1",
        ] {
            assert_eq!(key_of(SLIDES, bad), None, "{bad}");
            assert_eq!(key_of(SHAPES, bad), None, "{bad}");
        }
    }
}
