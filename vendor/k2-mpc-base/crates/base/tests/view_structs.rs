//! How a pack builds the view index's public structs, in the only pattern the kit documents:
//! `..X::default()` for the index's parts (`Unit`, `Container`, `ViewIndex`, `FontUse`,
//! `Layout`, `Bbox`, `Flip`, `UnitText`, `Placeholder`) and `..X::NONE` for the schema's
//! (`ViewSchema`, `UnitFields`, `ContainerFields`, `Derived`, `BlobView`, `GridSlot`).
//!
//! A pack that writes a full literal stops compiling when the kit adds a field (a pack's derive
//! hook building its group children's units did, when the kit added a unit's derived values).
//! Every struct a pack builds is built here the documented way, with every field a pack of
//! today sets: a field added to one of them must keep this file compiling, and a field whose
//! addition would need a pack to change shows here first.
//!
//! Where a pack sets every field a struct has today, clippy calls the `..X::default()` needless
//! (`clippy::needless_update`): it isn't, it is what keeps the next field from breaking the pack,
//! so the lint is allowed here, as a pack allows it.
#![allow(clippy::needless_update)]

use k2_mpc_base::grid::GridSlot;
use k2_mpc_base::view_index::{
    Bbox, BlobView, Container, ContainerFields, Derived, EditableText, Flip, FontUse, Layout,
    Placeholder, Src, Unit, UnitFields, UnitText, ViewIndex, ViewSchema,
};

/// A group child's unit as a deck's derive hook builds it.
fn pack_unit() -> Unit {
    Unit {
        key: "@sld:256/sp:7".into(),
        container: "@sld:256".into(),
        parent: Some("@sld:256/sp:4".into()),
        live: false,
        kind: "shape".into(),
        name: "Rectangle 7".into(),
        name_is_generic: Some(true),
        alt: None,
        z: 1,
        bbox: Some(Bbox {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
            ..Bbox::default()
        }),
        bbox_inherited: false,
        rot: 0,
        flip: Flip {
            h: true,
            ..Flip::default()
        },
        approx_box: false,
        text: Some(UnitText {
            summary: "Hello".into(),
            truncated: false,
            ..UnitText::default()
        }),
        fields: vec!["slidenum".into()],
        hidden: false,
        placeholder: Some(Placeholder {
            ty: "body".into(),
            idx: Some("1".into()),
            ..Placeholder::default()
        }),
        editable_text: Some(EditableText::Plain),
        ..Unit::default()
    }
}

#[test]
fn every_view_struct_a_pack_builds_builds_the_documented_way() {
    let u = pack_unit();
    let c = Container {
        key: "@sld:256".into(),
        position: 1,
        title: Some("Intro".into()),
        hidden: false,
        layout: Some("@layout:1".into()),
        render_hash: None,
        fonts: vec![0],
        has_fields: vec!["slidenum".into()],
        unit_count: 1,
        ..Container::default()
    };
    let ix = ViewIndex {
        fonts: vec![FontUse {
            family: "Calibri".into(),
            script: "latin".into(),
            from: "theme-minor".into(),
            embedded: false,
            ..FontUse::default()
        }],
        layouts: vec![Layout {
            key: "@layout:1".into(),
            name: "Title Slide".into(),
            master: Some("@master:1".into()),
            ..Layout::default()
        }],
        containers: vec![c],
        units: vec![u.clone()],
        ..ViewIndex::default()
    };
    let schema = ViewSchema {
        containers: "slides",
        units: "shapes",
        unit_container_field: "slide",
        container_fields: ContainerFields {
            title: Src::Derived("title"),
            ..ContainerFields::NONE
        },
        unit_fields: UnitFields {
            kind: Src::ListValue,
            ..UnitFields::NONE
        },
        derived: Derived {
            units: &["text"],
            ..Derived::NONE
        },
        blob: Some(BlobView {
            container_kind: "slide",
            ..BlobView::NONE
        }),
        grid: Some(GridSlot {
            rows: "shapes",
            ..GridSlot::NONE
        }),
        ..ViewSchema::NONE
    };
    assert_eq!(ix.unit("@sld:256/sp:7"), Some(&u));
    assert_eq!(schema.units, "shapes");
    // what a pack leaves out is the empty value
    assert!(u.derived.is_empty());
    assert_eq!(ix.containers[0].unit_count, 1);
}
