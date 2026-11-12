//! The splice's rows, at the pack level on bytes handed to `encode_from` as the kit's carrier
//! hook (K1) hands them (`tests/live_carrier.rs` has the rows through the kit): F5 (an
//! untouched deck writes back byte for byte), every kind of
//! live change written by splicing (the written deck reads back as the live content, and every
//! part the change doesn't touch is its raw record), and the guard refusing what the file
//! can't hold. Each row returns `Err("<row>: ...")`, so the planted faults can say which row
//! caught them. The kit's own byte-exact row for a carrier pack
//! (`test_support::carrier_judge`) runs over the same corpus (`row_carrier_judge`).

use std::collections::BTreeMap;
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::test_support::{carrier_judge, CarrierVerdict};
use k2_mpc_base::session::Pack;
use k2_mpc_pptx::live::model::{self, Geom, LiveDeck, LiveShape, LiveSlide, Ph, TextEdit};
use k2_mpc_pptx::live::splice::encode_from;
use k2_mpc_pptx::testgen::{self, golden};
use k2_mpc_pptx::PptxLive;

use super::{deflated, limits, specs};

/// Every zip entry's raw record (local header, name and data), by name.
pub fn records(b: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut i = 0usize;
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    while i + 30 <= b.len() && b[i..i + 4] == [0x50, 0x4B, 0x03, 0x04] {
        let comp = u32_at(i + 18);
        let n = u16_at(i + 26);
        let e = u16_at(i + 28);
        let end = i + 30 + n + e + comp;
        if end > b.len() {
            break;
        }
        let name = String::from_utf8_lossy(&b[i + 30..i + 30 + n]).into_owned();
        out.insert(name, b[i..end].to_vec());
        i = end;
    }
    out
}

fn deck(b: &[u8]) -> Result<LiveDeck, String> {
    model::read(b, &limits()).map_err(|d| d.to_string())
}

/// Splice `f`'s change of `b`'s live content into `b`.
pub fn spliced(b: &[u8], f: impl FnOnce(&mut LiveDeck)) -> Result<(Vec<u8>, LiveDeck), String> {
    let from = deck(b)?;
    let mut to = from.clone();
    f(&mut to);
    let out = encode_from(b, &to.to_content(), &[], &limits()).map_err(|e| e.0)?;
    Ok((out, to))
}

/// The byte-exact corpus: every fixture, stored (our writer) and deflated (the `zip` crate's),
/// and the golden sample deck, each named as a file (`<fixture>-<layout>.pptx`).
pub fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut decks: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, spec) in specs() {
        let parts = testgen::parts(&spec);
        decks.push((format!("{name}-stored.pptx"), testgen::stored_zip(&parts)));
        decks.push((format!("{name}-deflated.pptx"), deflated(&parts)));
    }
    decks.push(("golden.pptx".into(), golden::deck(golden::Extra::None)));
    decks
}

/// F5: an untouched deck writes back byte for byte, every file of the corpus.
pub fn row_f5_untouched_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_f5_untouched_byte_for_byte";
    for (name, b) in corpus() {
        let c = deck(&b)
            .map_err(|e| format!("{ROW}: {name}: {e}"))?
            .to_content();
        let out =
            encode_from(&b, &c, &[], &limits()).map_err(|e| format!("{ROW}: {name}: {}", e.0))?;
        if out != b {
            return Err(format!("{ROW}: {name}: the bytes differ"));
        }
    }
    Ok(())
}

/// The pack contract's byte-exact row, the kit's: `test_support::carrier_judge` over the whole
/// corpus. Per file it checks the hook (`encode_from` of the file's own content is the file),
/// the file opened live through the kit (it renders as itself, and a flush leaves it), and the
/// same after a restart. The pack keeps a carrier, so the verdict must be byte-exact for every
/// file; each failure is reported by file and leg.
pub fn row_carrier_judge() -> Result<(), String> {
    const ROW: &str = "row_carrier_judge";
    let decks = corpus();
    let files: Vec<(&str, &[u8])> = decks
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let pack: Arc<dyn Pack> = Arc::new(PptxLive::default());
    match carrier_judge(pack, &files) {
        Ok(CarrierVerdict::ByteExact { files: n }) if n == files.len() => Ok(()),
        Ok(other) => Err(format!(
            "{ROW}: {other:?}, not byte-exact over {} files",
            files.len()
        )),
        Err(fails) => Err(format!("{ROW}: {}", fails.join("; "))),
    }
}

fn slide(d: &mut LiveDeck, sid: u32) -> &mut LiveSlide {
    d.slides
        .iter_mut()
        .find(|s| s.sid == sid)
        .expect("the slide")
}

fn shape(d: &mut LiveDeck, sid: u32, fid: u32) -> &mut LiveShape {
    slide(d, sid)
        .shapes
        .iter_mut()
        .find(|s| s.fid == fid)
        .expect("the shape")
}

fn text_box(fid: u32, at: (i64, i64, i64, i64), text: &str) -> LiveShape {
    LiveShape {
        fid,
        kind: "textbox".into(),
        name: format!("TextBox {}", fid - 1),
        descr: None,
        geom: Some(Geom {
            x: at.0,
            y: at.1,
            w: at.2,
            h: at.3,
            ..Geom::default()
        }),
        text: Some(text.into()),
        ph: None,
        edit: TextEdit::Plain,
        hidden: false,
        refd: false,
        mc: false,
    }
}

/// A slide as `slide add` makes it from layout `layout`.
pub fn new_slide(d: &LiveDeck, sid: u32, layout: u64) -> LiveSlide {
    let l = d.layout(layout).expect("the layout");
    LiveSlide {
        sid,
        layout: Some(layout),
        hidden: false,
        notes: None,
        notes_fixed: false,
        linked: false,
        shapes: l
            .placeholders
            .iter()
            .enumerate()
            .map(|(i, p)| LiveShape {
                fid: 2 + i as u32,
                kind: "placeholder".into(),
                name: p.name.clone(),
                descr: None,
                geom: None,
                text: p.body.then(String::new),
                ph: Some(Ph {
                    ty: p.ty.clone(),
                    idx: p.idx.clone(),
                }),
                edit: TextEdit::Plain,
                hidden: false,
                refd: false,
                mc: false,
            })
            .collect(),
    }
}

type Change = (&'static str, fn(&mut LiveDeck), &'static [&'static str]);

/// Each kind of live change on the sample deck, with the parts it may touch (every other
/// part must be its raw record).
pub const CHANGES: &[Change] = &[
    (
        "shape text",
        |d| shape(d, 258, 4).text = Some("Changed\nTwice".into()),
        &["ppt/slides/slide3.xml"],
    ),
    (
        "shape move",
        |d| shape(d, 258, 5).geom.as_mut().expect("a box").x += 100,
        &["ppt/slides/slide3.xml"],
    ),
    (
        "a placeholder's first box",
        |d| {
            shape(d, 257, 3).geom = Some(Geom {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
                ..Geom::default()
            })
        },
        &["ppt/slides/slide2.xml"],
    ),
    (
        "notes set",
        |d| slide(d, 257).notes = Some("New notes".into()),
        &["ppt/notesSlides/notesSlide2.xml"],
    ),
    (
        "notes made",
        |d| slide(d, 256).notes = Some("Fresh".into()),
        &[
            "ppt/notesSlides/notesSlide1.xml",
            "ppt/notesSlides/_rels/notesSlide1.xml.rels",
            "ppt/slides/_rels/slide1.xml.rels",
            "[Content_Types].xml",
        ],
    ),
    (
        "slide hide",
        |d| slide(d, 256).hidden = true,
        &["ppt/slides/slide1.xml"],
    ),
    (
        "slide show",
        |d| slide(d, 259).hidden = false,
        &["ppt/slides/slide4.xml"],
    ),
    (
        "slide add",
        |d| {
            let s = new_slide(d, 260, testgen::LAYOUT_CONTENT);
            d.slides.insert(2, s);
        },
        &[
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "[Content_Types].xml",
            "ppt/slides/slide5.xml",
            "ppt/slides/_rels/slide5.xml.rels",
        ],
    ),
    (
        "shape add",
        |d| {
            slide(d, 256)
                .shapes
                .push(text_box(4, (100, 200, 3000, 400), "Added <live> & typed"))
        },
        &["ppt/slides/slide1.xml"],
    ),
    (
        "shape add after a group (B1)",
        |d| {
            slide(d, 258)
                .shapes
                .push(text_box(12, (100, 200, 3000, 400), "After the group"))
        },
        &["ppt/slides/slide3.xml"],
    ),
    (
        "shape order",
        |d| {
            let s = slide(d, 258);
            let i = s.shapes.iter().position(|x| x.fid == 4).expect("4");
            let four = s.shapes.remove(i);
            s.shapes.push(four);
            let j = s.shapes.iter().position(|x| x.fid == 7).expect("7");
            let seven = s.shapes.remove(j);
            s.shapes.insert(0, seven);
        },
        &["ppt/slides/slide3.xml"],
    ),
    (
        "shape delete",
        |d| slide(d, 258).shapes.retain(|x| x.fid != 7),
        &["ppt/slides/slide3.xml"],
    ),
    (
        "slide move",
        |d| {
            let s = d.slides.pop().expect("a slide");
            d.slides.insert(0, s);
        },
        &["ppt/presentation.xml"],
    ),
    (
        "slide delete",
        |d| d.slides.retain(|s| s.sid != 257),
        &[
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "[Content_Types].xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/_rels/slide2.xml.rels",
            "ppt/notesSlides/notesSlide2.xml",
            "ppt/notesSlides/_rels/notesSlide2.xml.rels",
        ],
    ),
];

/// Each kind of live change, spliced into the sample deck (stored and deflated): the written
/// deck reads back as the live content, and every other part is its raw record.
pub fn row_each_change_spliced() -> Result<(), String> {
    const ROW: &str = "row_each_change_spliced";
    let parts = testgen::parts(&testgen::sample());
    for (layout, b) in [
        ("stored", testgen::stored_zip(&parts)),
        ("deflated", deflated(&parts)),
    ] {
        let before = records(&b);
        for (what, f, touched) in CHANGES {
            let (out, to) = spliced(&b, f).map_err(|e| format!("{ROW}: {what} ({layout}): {e}"))?;
            let back = deck(&out).map_err(|e| format!("{ROW}: {what}: {e}"))?;
            if back
                .slides
                .iter()
                .map(|s| s.sid)
                .ne(to.slides.iter().map(|s| s.sid))
            {
                return Err(format!(
                    "{ROW}: {what} ({layout}): the slides read back in another order"
                ));
            }
            let after = records(&out);
            for (name, rec) in &before {
                if touched.contains(&name.as_str()) {
                    continue;
                }
                match after.get(name) {
                    Some(r) if r == rec => {}
                    Some(_) => {
                        return Err(format!(
                            "{ROW}: {what} ({layout}): {name} isn't its raw record"
                        ))
                    }
                    None => return Err(format!("{ROW}: {what} ({layout}): {name} went")),
                }
            }
        }
    }
    Ok(())
}

/// Every change at once, then the golden turn's changes on the golden deck.
pub fn row_all_changes_at_once() -> Result<(), String> {
    const ROW: &str = "row_all_changes_at_once";
    let b = testgen::deck(&testgen::sample());
    spliced(&b, |d| {
        for (_, f, _) in CHANGES
            .iter()
            .filter(|(w, ..)| *w != "slide delete" && *w != "slide move")
        {
            f(d);
        }
    })
    .map_err(|e| format!("{ROW}: sample: {e}"))?;
    let g = golden::deck(golden::Extra::None);
    spliced(&g, |d| {
        shape(d, 256, 2).text = Some("Q4 review".into());
        shape(d, 257, 7).geom.as_mut().expect("a box").x = 7_500_000;
        let mut s = new_slide(d, 260, golden::TITLE_CONTENT);
        s.shapes[0].text = Some("Risks".into());
        s.shapes[1].text = Some("Supplier delays\nHiring pace".into());
        d.slides.push(s);
        let i = d.slides.iter().position(|s| s.sid == 259).expect("259");
        let s = d.slides.remove(i);
        d.slides.insert(2, s);
    })
    .map_err(|e| format!("{ROW}: golden turn: {e}"))?;
    Ok(())
}

/// The guard: live content the splice can't write (a picture added live) is refused, with
/// nothing written.
pub fn row_guard_refuses_what_the_file_cant_hold() -> Result<(), String> {
    const ROW: &str = "row_guard_refuses_what_the_file_cant_hold";
    let b = testgen::deck(&testgen::sample());
    let got = spliced(&b, |d| {
        let mut x = text_box(20, (0, 0, 10, 10), "x");
        x.kind = "picture".into();
        slide(d, 256).shapes.push(x);
    });
    match got {
        Err(e) if e.contains("can't make") => Ok(()),
        Err(e) => Err(format!("{ROW}: refused for another reason: {e}")),
        Ok(_) => Err(format!("{ROW}: a picture added live was written")),
    }
}

/// The content a live doc's splice starts from is what its carrier decodes to: the pack
/// opts in to the kit's carrier, holds no bytes itself, and splices into whichever carrier it
/// is handed. The decoded content of one deck handed with another deck's bytes as the carrier
/// gives that content spliced into those bytes (the carrier's opaque parts), never the first
/// deck's file.
pub fn row_carrier_content_is_decoded() -> Result<(), String> {
    use k2_mpc_base::session::Pack;
    const ROW: &str = "row_carrier_content_is_decoded";
    let b = testgen::deck(&testgen::sample());
    let pack = k2_mpc_pptx::PptxLive::default();
    if !pack.wants_carrier() {
        return Err(format!(
            "{ROW}: the pack doesn't opt in to the kit's carrier"
        ));
    }
    let c = pack.decode(&b).map_err(|d| format!("{ROW}: {d}"))?;
    if pack.encode(&c.roots, &c.codec).is_ok() {
        return Err(format!(
            "{ROW}: encode built a deck from the doc alone, with no carrier"
        ));
    }
    let back = pack
        .encode_from(&b, &c.roots, &c.codec)
        .map_err(|e| format!("{ROW}: {}", e.0))?;
    if back != b {
        return Err(format!("{ROW}: untouched isn't the carrier byte for byte"));
    }
    // the same content, spliced into a carrier whose opaque part differs: that carrier's part
    let other = testgen::stored_zip(&testgen::with_part(
        &testgen::parts(&testgen::sample()),
        "docProps/app.xml",
        b"<?xml version=\"1.0\"?><Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>Another app</Application></Properties>".to_vec(),
    ));
    let want: Vec<(String, Neutral)> = deck(&other)?.to_content();
    if want != c.roots {
        return Err(format!(
            "{ROW}: control: the opaque change changed the content"
        ));
    }
    let out = pack
        .encode_from(&other, &c.roots, &c.codec)
        .map_err(|e| format!("{ROW}: {}", e.0))?;
    if out != other {
        return Err(format!(
            "{ROW}: the splice didn't start from the carrier it was handed"
        ));
    }
    Ok(())
}

/// One pass: an `encode_from` opens the carrier's package once, decodes it once, and reads the
/// written deck back once, however many shapes changed (1, 6 or 12 moved boxes on a crowded
/// slide, and every kind of change at once on the golden deck, a slide added and filled
/// among them); untouched, it opens and decodes once and writes nothing to guard.
pub fn row_one_pass_per_encode_from() -> Result<(), String> {
    use k2_mpc_pptx::live::splice::{take_counts, Counts};
    const ROW: &str = "row_one_pass_per_encode_from";
    let one = Counts {
        opens: 1,
        decodes: 1,
        guards: 1,
    };
    let mut s = testgen::Slide::titled(256, "Crowded", "");
    for i in 0..12u32 {
        s.shapes.push(testgen::Shape::text_box(
            4 + i,
            (i64::from(i) * 100_000, 0, 50_000, 50_000),
            &format!("box {i}"),
        ));
    }
    let b = testgen::deck(&testgen::DeckSpec::new(vec![s]));
    take_counts();
    spliced(&b, |_| {}).map_err(|e| format!("{ROW}: untouched: {e}"))?;
    let got = take_counts();
    let want = Counts { guards: 0, ..one };
    if got != want {
        return Err(format!("{ROW}: untouched: {got:?}, not {want:?}"));
    }
    for n in [1usize, 6, 12] {
        take_counts();
        spliced(&b, |d| {
            for x in d.slides[0].shapes.iter_mut().filter(|x| x.fid >= 4).take(n) {
                x.geom.as_mut().expect("a box").y += 1_000;
            }
        })
        .map_err(|e| format!("{ROW}: {n} moved: {e}"))?;
        let got = take_counts();
        if got != one {
            return Err(format!("{ROW}: {n} shapes moved: {got:?}, not {one:?}"));
        }
    }
    let g = golden::deck(golden::Extra::None);
    take_counts();
    spliced(&g, |d| {
        shape(d, 256, 2).text = Some("Q4 review".into());
        shape(d, 257, 7).geom.as_mut().expect("a box").x = 7_500_000;
        let mut s = new_slide(d, 260, golden::TITLE_CONTENT);
        s.shapes[0].text = Some("Risks".into());
        s.shapes[1].text = Some("Supplier delays\nHiring pace".into());
        d.slides.push(s);
        let i = d.slides.iter().position(|s| s.sid == 259).expect("259");
        let s = d.slides.remove(i);
        d.slides.insert(2, s);
    })
    .map_err(|e| format!("{ROW}: golden turn: {e}"))?;
    let got = take_counts();
    if got != one {
        return Err(format!("{ROW}: golden turn: {got:?}, not {one:?}"));
    }
    Ok(())
}
