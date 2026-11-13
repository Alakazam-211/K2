//! The live pack, on the kit's session: a deck opens as a live keyed doc, reads back byte for
//! byte while untouched, declines what it can't hold, and takes an outside save by key with its
//! id marks never lowered.
//!
//! Each row is a function returning `Err("<row>: ...")` when it fails, so a planted fault's
//! test can say which row caught it.

mod common;

use common::live::{app_save, content, fx};
use common::{deflated, limits};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::session::{DiskCheck, SessionError};
use k2_mpc_pptx::live::model::{self, LiveDeck};
use k2_mpc_pptx::live::{
    prop, shape_key, slide_key, DECK, SHAPES, SHAPE_PROPS, SLIDES, SLIDE_PROPS,
};
use k2_mpc_pptx::testgen::{self, DeckSpec, Shape, Slide};

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

fn root<'a>(c: &'a [(String, Neutral)], name: &str) -> &'a Neutral {
    &c.iter().find(|(r, _)| r == name).expect("the root").1
}

fn map_get(c: &[(String, Neutral)], r: &str, k: &str) -> Option<String> {
    match root(c, r) {
        Neutral::Map(m) => m.get(k).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn list_keys(c: &[(String, Neutral)], r: &str) -> Vec<String> {
    match root(c, r) {
        Neutral::List(l) => l.iter().map(|(k, _)| k.as_str().to_owned()).collect(),
        _ => Vec::new(),
    }
}

fn shape_field(c: &[(String, Neutral)], sid: u32, fid: u32, f: &str) -> Option<String> {
    map_get(c, SHAPE_PROPS, &prop(&shape_key(sid, fid), f))
}

/// The sample deck opens live; its keyed state is the pack's reading of the file, every
/// slide and top-level shape with its fields (a group one shape, its children not elements).
fn row_a_generated_deck_opens_live() -> Result<(), String> {
    const ROW: &str = "row_a_generated_deck_opens_live";
    let bytes = testgen::deck(&testgen::sample());
    let f = fx("opens", |_| {});
    f.write(&bytes);
    let s = f
        .try_open()
        .map_err(|e| format!("{ROW}: the deck didn't open live: {e}"))?;
    let c = f.person(&s);
    let got = content(&s, c);
    let want = model::read(&bytes, &limits())
        .map_err(|d| format!("{ROW}: the pack's reading: {d}"))?
        .to_content();
    need(got == want, ROW, || {
        "the live roots aren't the pack's reading of the file".into()
    })?;
    need(
        list_keys(&got, SLIDES) == ["s256", "s257", "s258", "s259"],
        ROW,
        || format!("slides {:?}", list_keys(&got, SLIDES)),
    )?;
    let shapes = list_keys(&got, SHAPES);
    need(
        shapes
            == [
                "s256-2", "s256-3", "s257-2", "s257-3", "s258-2", "s258-4", "s258-5", "s258-6",
                "s258-7", "s258-8", "s258-9", "s259-2", "s259-3",
            ],
        ROW,
        || format!("shapes {shapes:?}"),
    )?;
    let checks: &[(u32, u32, &str, Option<&str>)] = &[
        (256, 2, "ph", Some("ctrTitle:")),
        (256, 2, "geom", Some("")),
        (256, 2, "text", Some("Quarterly review")),
        (256, 2, "edit", Some("1")),
        (258, 4, "name", Some("TextBox 3")),
        (258, 4, "geom", Some("838200,1500000,3000000,500000,0,0,0")),
        (258, 4, "text", Some("Left box")),
        (258, 4, "refd", Some("1")),
        (258, 5, "refd", Some("1")),
        (258, 5, "text", None),
        (258, 7, "descr", Some("A small square")),
        (258, 7, "edit", Some("0")),
        (258, 8, "text", Some("Site\tCount\nNorth\t12")),
        (258, 8, "edit", Some("0")),
        (
            258,
            9,
            "geom",
            Some("7500000,3000000,2000000,1000000,0,0,0"),
        ),
        (258, 9, "edit", Some("0")),
        (258, 9, "slide", Some("258")),
        (258, 9, "fid", Some("9")),
        (259, 3, "ph", Some(":1")),
        (259, 3, "hidden", Some("0")),
    ];
    for (sid, fid, fld, want) in checks {
        let v = shape_field(&got, *sid, *fid, fld);
        need(v.as_deref() == *want, ROW, || {
            format!("@sld:{sid}/sp:{fid} {fld} = {v:?}, not {want:?}")
        })?;
    }
    let sf = |sid: u32, fld: &str| map_get(&got, SLIDE_PROPS, &prop(&slide_key(sid), fld));
    need(
        sf(257, "notes").as_deref() == Some("Mention the pilot sites.\nAsk for questions."),
        ROW,
        || format!("257's notes {:?}", sf(257, "notes")),
    )?;
    need(
        sf(257, "layout").as_deref() == Some("@layout:2147483650"),
        ROW,
        || format!("257's layout {:?}", sf(257, "layout")),
    )?;
    need(sf(259, "hidden").as_deref() == Some("1"), ROW, || {
        "259 isn't hidden".into()
    })?;
    need(sf(256, "notes").is_none(), ROW, || {
        "256 has no notes slide".into()
    })?;
    need(
        map_get(&got, DECK, "cx").as_deref() == Some("12192000"),
        ROW,
        || "the page width".into(),
    )?;
    need(
        map_get(&got, DECK, "layout/2147483650/name").as_deref() == Some("Title and Content"),
        ROW,
        || "the layout's name".into(),
    )
}

#[test]
fn a_generated_deck_opens_live() {
    row_a_generated_deck_opens_live().unwrap();
}

/// The read side: an untouched live deck reads back byte for byte, stored and deflated.
fn row_an_untouched_live_deck_reads_back_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_an_untouched_live_deck_reads_back_byte_for_byte";
    for (name, spec) in common::specs() {
        let parts = testgen::parts(&spec);
        for (layout, bytes) in [
            ("stored", testgen::stored_zip(&parts)),
            ("deflated", deflated(&parts)),
        ] {
            let f = fx("reads-back", |_| {});
            f.write(&bytes);
            let s = f
                .try_open()
                .map_err(|e| format!("{ROW}: {name}/{layout} didn't open live: {e}"))?;
            let c = f.person(&s);
            let back = s
                .read(c)
                .map_err(|e| format!("{ROW}: {name}/{layout}: read: {e}"))?;
            need(back.bytes == bytes, ROW, || {
                format!("{name}/{layout}: the bytes differ")
            })?;
        }
    }
    Ok(())
}

#[test]
fn an_untouched_live_deck_reads_back_byte_for_byte() {
    row_an_untouched_live_deck_reads_back_byte_for_byte().unwrap();
}

/// A slide's id mark is the highest id in its whole tree, a group's children included.
fn row_the_mark_covers_group_children() -> Result<(), String> {
    const ROW: &str = "row_the_mark_covers_group_children";
    let d =
        model::read(&testgen::deck(&testgen::sample()), &limits()).map_err(|e| e.to_string())?;
    let top = d.slides[2].shapes.iter().map(|s| s.fid).max().unwrap_or(0);
    need(top == 9, ROW, || format!("the top-level max is {top}"))?;
    need(d.fid_marks.get(&258) == Some(&11), ROW, || {
        format!(
            "slide 258's mark is {:?}, not 11 (its group holds 10 and 11)",
            d.fid_marks.get(&258)
        )
    })
}

#[test]
fn the_mark_covers_group_children() {
    row_the_mark_covers_group_children().unwrap();
}

#[test]
fn the_model_reads_back_from_its_roots() {
    let d = model::read(&testgen::deck(&testgen::sample()), &limits()).unwrap();
    assert_eq!(LiveDeck::from_content(&d.to_content()).unwrap(), d);
}

fn twin_ids() -> Vec<u8> {
    let mut s = Slide::titled(256, "Twins", "");
    s.shapes.push(Shape::text_box(4, (0, 0, 10, 10), "a"));
    s.shapes.push(Shape::text_box(4, (0, 20, 10, 10), "b"));
    testgen::deck(&DeckSpec::new(vec![s]))
}

fn long_text() -> Vec<u8> {
    let mut s = Slide::titled(256, "Long", "");
    s.shapes.push(Shape::text_box(
        4,
        (0, 0, 10, 10),
        &"a".repeat(model::TEXT_MAX + 1),
    ));
    testgen::deck(&DeckSpec::new(vec![s]))
}

/// Declines: a repeated `cNvPr` (`live_ids`), text over the field limit
/// (`live_text_too_large`), and over the kit's keyed cap (`live_too_large`, `MPC-E415`).
fn row_declines() -> Result<(), String> {
    const ROW: &str = "row_declines";
    let code = |bytes: &[u8], cap: Option<usize>| -> Result<&'static str, String> {
        let f = fx("decline", |c| {
            if let Some(n) = cap {
                c.keyed_max_entries = n;
            }
        });
        f.write(bytes);
        match f.try_open() {
            Ok(_) => Err("opened live".into()),
            Err(SessionError::Declined(d)) => Ok(d.code),
            Err(e) => Err(format!("{e:?}")),
        }
    };
    let got = code(&twin_ids(), None).map_err(|e| format!("{ROW}: twin ids: {e}"))?;
    need(got == "live_ids", ROW, || format!("twin ids: {got}"))?;
    let got = code(&long_text(), None).map_err(|e| format!("{ROW}: long text: {e}"))?;
    need(got == "live_text_too_large", ROW, || {
        format!("long text: {got}")
    })?;
    let sample = testgen::deck(&testgen::sample());
    let n = model::read(&sample, &limits())
        .map_err(|e| e.to_string())?
        .entries();
    let got = code(&sample, Some(n - 1)).map_err(|e| format!("{ROW}: over the cap: {e}"))?;
    need(got == "live_too_large", ROW, || {
        format!("over the cap: {got}")
    })?;
    let f = fx("at-cap", |c| c.keyed_max_entries = n);
    f.write(&sample);
    need(f.try_open().is_ok(), ROW, || {
        format!("a deck of exactly {n} entries didn't open")
    })
}

#[test]
fn declines() {
    row_declines().unwrap();
}

/// An outside save before any kit write applies by key: a new shape appears under its file
/// key, a removed group goes, and slide 258's id mark stays at 11 (never lowered).
fn row_outside_save_by_key_keeps_marks() -> Result<(), String> {
    const ROW: &str = "row_outside_save_by_key_keeps_marks";
    let spec = testgen::sample();
    let f = fx("outside", |_| {});
    f.write(&testgen::deck(&spec));
    let s = f.open();
    let c = f.person(&s);
    let mut saved = spec.clone();
    saved.slides[0]
        .shapes
        .push(Shape::text_box(5, (100, 100, 1000, 1000), "From the app"));
    saved.slides[2].shapes.retain(|x| x.id() != 9);
    app_save(&f.path(), &testgen::deck(&saved));
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(
        matches!(out, DiskCheck::Ingested { .. } | DiskCheck::Merged { .. }),
        ROW,
        || format!("the save wasn't taken in: {out:?}"),
    )?;
    let got = content(&s, c);
    let shapes = list_keys(&got, SHAPES);
    need(shapes.contains(&"s256-5".to_owned()), ROW, || {
        format!("no s256-5: {shapes:?}")
    })?;
    need(!shapes.contains(&"s258-9".to_owned()), ROW, || {
        "the group is still live".into()
    })?;
    need(
        shape_field(&got, 256, 5, "text").as_deref() == Some("From the app"),
        ROW,
        || "the new shape's text".into(),
    )?;
    need(
        map_get(&got, DECK, "mark/fid/258").as_deref() == Some("11"),
        ROW,
        || format!("258's mark {:?}", map_get(&got, DECK, "mark/fid/258")),
    )
}

#[test]
fn outside_save_by_key_keeps_marks() {
    row_outside_save_by_key_keeps_marks().unwrap();
}
