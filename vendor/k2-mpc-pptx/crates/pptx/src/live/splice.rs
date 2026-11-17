//! The live content written back into the file: [`encode_from`] diffs what
//! the carrier holds against what the live doc holds and splices only the difference into the
//! carrier's bytes, with the file verbs' own code (`crate::verbs`), so every other byte of the
//! file stays and every other part is its raw record.
//!
//! **Order:** slide adds (each after its predecessor in the live order), shape adds, field
//! changes (text, box, notes, hidden), slide moves, shape order, then deletes (shapes, then
//! slides).
//!
//! **One pass:** the carrier's package is opened once; its decode (what the live content is
//! diffed against) and every step run on that one open, each step on top of the ones before
//! it (`crate::verbs::Batch`), and the package is written once. A step never re-opens it.
//!
//! **The guard:** the written deck must read back as exactly the live content, slides, shapes
//! and their fields in order (less the facts the file derives: `linked`, `refd`). It runs once,
//! on the final bytes. Otherwise nothing is written and the kit gets a [`PackError`]; the
//! store still holds every change.
//!
//! **Counts** (`take_counts`, test-only: the `testgen` feature): the package opens, carrier
//! decodes and read-back guards of this thread's calls, so a row can hold the one pass.
//!
//! **The carrier** is handed in by the kit's carrier hook: the bytes the kit last read
//! from the file or wrote to it, kept in its store. [`encode_from`] decodes it itself, so the
//! content it diffs against is always the carrier's own.

use std::collections::BTreeMap;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::PackError;

use super::model::{read, read_open, LayoutFacts, LiveDeck, LiveShape, LiveSlide, Ph, TextEdit};
use crate::mutant;
use crate::verbs::{Batch, Edit, Place, Step};
use crate::{Limits, Pptx};

fn err(m: impl Into<String>) -> PackError {
    PackError(m.into())
}

/// What the splice did, counted per thread (test-only: kept with the `testgen` feature).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Package opens of the carrier or of a step's bytes (not the guard's).
    pub opens: u32,
    /// Decodes of the carrier into the live model.
    pub decodes: u32,
    /// Read-back guards on written bytes.
    pub guards: u32,
}

#[cfg(feature = "testgen")]
thread_local! {
    static COUNTS: std::cell::Cell<Counts> = const {
        std::cell::Cell::new(Counts {
            opens: 0,
            decodes: 0,
            guards: 0,
        })
    };
}

/// This thread's counts since the last call, which resets them (test-only).
#[cfg(feature = "testgen")]
pub fn take_counts() -> Counts {
    COUNTS.with(|c| c.replace(Counts::default()))
}

fn count(f: impl FnOnce(&mut Counts)) {
    #[cfg(feature = "testgen")]
    COUNTS.with(|c| {
        let mut n = c.get();
        f(&mut n);
        c.set(n);
    });
    #[cfg(not(feature = "testgen"))]
    let _ = f;
}

/// The slides as the guard compares them: the facts the file derives cleared.
fn guarded(slides: &[LiveSlide]) -> Vec<LiveSlide> {
    slides
        .iter()
        .map(|s| {
            let mut s = s.clone();
            s.linked = false;
            for x in &mut s.shapes {
                x.refd = false;
            }
            s
        })
        .collect()
}

/// The first place two decks' slides differ, for the guard's message.
fn first_difference(a: &[LiveSlide], b: &[LiveSlide]) -> String {
    let ids = |v: &[LiveSlide]| v.iter().map(|s| s.sid).collect::<Vec<_>>();
    if ids(a) != ids(b) {
        return format!("slides {:?}, live {:?}", ids(a), ids(b));
    }
    for (x, y) in a.iter().zip(b) {
        if x.shapes.len() != y.shapes.len()
            || x.shapes
                .iter()
                .map(|s| s.fid)
                .ne(y.shapes.iter().map(|s| s.fid))
        {
            return format!("@sld:{}: shapes differ in number or order", x.sid);
        }
        for (s, t) in x.shapes.iter().zip(&y.shapes) {
            if s != t {
                return format!("@sld:{}/sp:{}: its fields differ", x.sid, s.fid);
            }
        }
        if x != y {
            return format!("@sld:{}: its fields differ", x.sid);
        }
    }
    "no difference".into()
}

/// The shapes a new slide gets from its layout, as the file verb makes them.
fn placeholders(l: &LayoutFacts) -> Vec<LiveShape> {
    l.placeholders
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
        .collect()
}

/// `want` in order, by moves of `cur` (each element right after the one before it in `want`);
/// elements of `cur` not in `want` are ignored (they go later). Returns `(id, after)` per move.
fn moves(cur: &mut Vec<u32>, want: &[u32]) -> Vec<(u32, Option<u32>)> {
    let mut out = Vec::new();
    for (i, id) in want.iter().enumerate() {
        let prev = if i == 0 { None } else { Some(want[i - 1]) };
        // the element before `id` among the ones that stay
        let at = cur.iter().position(|x| x == id).expect("present");
        let before = cur[..at].iter().rev().find(|x| want.contains(x)).copied();
        if before != prev {
            cur.remove(at);
            let pos = match prev {
                Some(p) => cur.iter().position(|x| *x == p).map_or(0, |j| j + 1),
                None => 0,
            };
            cur.insert(pos, *id);
            out.push((*id, prev));
        }
    }
    out
}

/// The steps that make the file `from` into the live content `to`.
fn diff_steps(from: &LiveDeck, to: &LiveDeck) -> Result<Vec<Step>, PackError> {
    let mut steps = Vec::new();
    let mut order: Vec<u32> = from.slides.iter().map(|s| s.sid).collect();
    // the file's shapes per slide, as the steps leave them
    let mut file: BTreeMap<u32, Vec<LiveShape>> = from
        .slides
        .iter()
        .map(|s| (s.sid, s.shapes.clone()))
        .collect();
    let file_slide: BTreeMap<u32, &LiveSlide> = from.slides.iter().map(|s| (s.sid, s)).collect();
    // 1. slide adds
    for (i, s) in to.slides.iter().enumerate() {
        if file.contains_key(&s.sid) {
            continue;
        }
        let layout = s
            .layout
            .and_then(|id| from.layout(id))
            .ok_or_else(|| err(format!("@sld:{}: its layout isn't in the file", s.sid)))?;
        let place = if i == 0 {
            Place::First
        } else {
            Place::After(to.slides[i - 1].sid)
        };
        steps.push(Step::AddSlide {
            id: s.sid,
            layout: layout.id,
            place: Some(place),
        });
        let pos = match place {
            Place::First => 0,
            Place::After(p) => order
                .iter()
                .position(|x| *x == p)
                .map_or(order.len(), |j| j + 1),
        };
        order.insert(pos, s.sid);
        file.insert(s.sid, placeholders(layout));
    }
    // 2. shape adds
    for s in &to.slides {
        let have = file
            .get_mut(&s.sid)
            .expect("every live slide is in the file now");
        for x in &s.shapes {
            if have.iter().any(|h| h.fid == x.fid) {
                continue;
            }
            if x.kind != "textbox" {
                return Err(err(format!(
                    "@sld:{}/sp:{}: a live {} the splice can't make",
                    s.sid, x.fid, x.kind
                )));
            }
            let g = x.geom.ok_or_else(|| {
                err(format!(
                    "@sld:{}/sp:{}: a text box with no box",
                    s.sid, x.fid
                ))
            })?;
            steps.push(Step::AddTextBox {
                slide: s.sid,
                id: x.fid,
                at: (g.x, g.y, g.w, g.h),
                text: x.text.clone().unwrap_or_default(),
            });
            have.push(LiveShape {
                text: x.text.clone(),
                ..x.clone()
            });
        }
    }
    // 3. fields
    for s in &to.slides {
        let have = &file[&s.sid];
        for x in &s.shapes {
            let h = have.iter().find(|h| h.fid == x.fid).expect("added above");
            if h.text != x.text {
                let text = x.text.clone().ok_or_else(|| {
                    err(format!(
                        "@sld:{}/sp:{}: its text can't be removed",
                        s.sid, x.fid
                    ))
                })?;
                steps.push(Step::Edit(Edit::SetText {
                    slide: s.sid,
                    shape: x.fid,
                    text,
                }));
            }
            if h.geom != x.geom {
                let g = x.geom.ok_or_else(|| {
                    err(format!(
                        "@sld:{}/sp:{}: its box can't be removed",
                        s.sid, x.fid
                    ))
                })?;
                if let Some(o) = h.geom {
                    if (o.rot, o.fh, o.fv) != (g.rot, g.fh, g.fv) {
                        return Err(err(format!(
                            "@sld:{}/sp:{}: rotation and flips are read-only live",
                            s.sid, x.fid
                        )));
                    }
                }
                steps.push(Step::Edit(Edit::MoveShape {
                    slide: s.sid,
                    shape: x.fid,
                    x: Some(g.x),
                    y: Some(g.y),
                    w: Some(g.w),
                    h: Some(g.h),
                }));
            }
        }
        let (notes, hidden) = match file_slide.get(&s.sid) {
            Some(f) => (f.notes.clone(), f.hidden),
            None => (None, false),
        };
        if s.notes != notes {
            let text = s
                .notes
                .clone()
                .ok_or_else(|| err(format!("@sld:{}: its notes can't be removed", s.sid)))?;
            steps.push(Step::Edit(Edit::SetNotes { slide: s.sid, text }));
        }
        if s.hidden != hidden {
            steps.push(Step::SetHidden {
                slide: s.sid,
                hidden: s.hidden,
            });
        }
    }
    // 4. slide order
    let want: Vec<u32> = to.slides.iter().map(|s| s.sid).collect();
    for (id, after) in moves(&mut order, &want) {
        steps.push(Step::Edit(Edit::MoveSlide {
            slide: id,
            place: after.map_or(Place::First, Place::After),
        }));
    }
    // 5. shape order, within each slide
    for s in &to.slides {
        // the file's order after the adds (each added text box went in front of the others)
        let mut cur: Vec<u32> = file[&s.sid].iter().map(|h| h.fid).collect();
        let want: Vec<u32> = s.shapes.iter().map(|x| x.fid).collect();
        for (id, after) in moves(&mut cur, &want) {
            steps.push(Step::OrderShape {
                slide: s.sid,
                shape: id,
                after,
            });
        }
    }
    // 6. deletes: shapes, then slides
    for s in &to.slides {
        for h in &file[&s.sid] {
            if !s.shapes.iter().any(|x| x.fid == h.fid) {
                steps.push(Step::Edit(Edit::DeleteShape {
                    slide: s.sid,
                    shape: h.fid,
                }));
            }
        }
    }
    for s in &from.slides {
        if !to.slides.iter().any(|t| t.sid == s.sid) {
            steps.push(Step::DeleteSlide { slide: s.sid });
        }
    }
    Ok(steps)
}

/// The file for the live `content`, spliced into `carrier` (the kit's carrier: the bytes it
/// last read from the file or wrote to it), which this decodes itself. Content equal to what
/// the carrier reads as gives the carrier, byte for byte (so it is idempotent); the result
/// reads back as `content` or nothing is written (the guard). `codec` is ignored.
pub fn encode_from(
    carrier: &[u8],
    content: &[(String, Neutral)],
    _codec: &[u8],
    limits: &Limits,
) -> Result<Vec<u8>, PackError> {
    let bad = |d: crate::Decline| err(format!("the carrier doesn't read as a deck: {d}"));
    let p = Pptx::open(carrier, limits).map_err(bad)?;
    count(|c| c.opens += 1);
    let from = read_open(&p, limits).map_err(bad)?;
    count(|c| c.decodes += 1);
    let to = LiveDeck::from_content(content).map_err(err)?;
    let planted = mutant::active("EncodeReserialises");
    if guarded(from.body()) == guarded(to.body()) && !planted {
        return Ok(carrier.to_vec());
    }
    let steps = diff_steps(&from, &to)?;
    let refused =
        |step: &Step, e: crate::verbs::VerbError| err(format!("the splice refused {step:?}: {e}"));
    let mut bytes = if mutant::active("SpliceReopensPerStep") {
        // planted: a package open and a write per step
        let mut b = carrier.to_vec();
        for step in &steps {
            let out = {
                let q = Pptx::open(&b, limits).map_err(|d| err(d.to_string()))?;
                count(|c| c.opens += 1);
                let mut one = Batch::new(&q, limits);
                one.step(step).map_err(|e| refused(step, e))?;
                one.write().map_err(|e| refused(step, e))?
            };
            if let Some(out) = out {
                b = out;
            }
        }
        b
    } else {
        let mut batch = Batch::new(&p, limits);
        for step in &steps {
            batch.step(step).map_err(|e| refused(step, e))?;
        }
        batch
            .write()
            .map_err(|e| err(format!("the splice's write: {e}")))?
            .unwrap_or_else(|| carrier.to_vec())
    };
    if planted {
        // a part written whole instead of spliced: the presentation part, re-serialised
        let p = Pptx::open(&bytes, limits).map_err(|d| err(d.to_string()))?;
        let mut text = p.text(&p.main).map_err(|d| err(d.to_string()))?.to_owned();
        text.push('\n');
        let mut ch = BTreeMap::new();
        ch.insert(
            p.main.clone(),
            k2_mpc_opc::Change::Replace(text.into_bytes()),
        );
        bytes = p.opc.write(&ch).map_err(|d| err(d.to_string()))?;
    }
    let back = read(&bytes, limits)
        .map_err(|d| err(format!("the written deck doesn't read back: {d}")))?;
    count(|c| c.guards += 1);
    if guarded(back.body()) != guarded(to.body()) {
        return Err(err(format!(
            "the written deck doesn't read back as the live content ({}): nothing written",
            first_difference(&guarded(back.body()), &guarded(to.body()))
        )));
    }
    Ok(bytes)
}
