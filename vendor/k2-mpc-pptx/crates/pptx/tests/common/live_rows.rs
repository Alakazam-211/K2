//! The live verbs' rows, on the kit's session: F2 (one change event per `shape move`, within
//! 50 ms p99), each verb's ops and typed refusals, the agent's base, B1 (a new shape after a
//! group never takes a child's id), the differential oracle (live verbs written back equal the
//! file verbs on the same file), a watcher seeing every change, and bounded, paged reads.
//! Each row returns `Err("<row>: ...")`, so the planted faults can say which row caught them.

use std::sync::mpsc::Receiver;
use std::time::Instant;

use k2_mpc_base::elem::{Applied, MapOpKind, Value};
use k2_mpc_base::session::{ChangeEvent, ConnId, Session, SessionError};
use k2_mpc_base::tools::{ToolOutput, ToolRunError};
use k2_mpc_pptx::live::model::{self, LiveDeck, LiveSlide};
use k2_mpc_pptx::live::{prop, shape_key, SHAPES, SHAPE_PROPS};
use k2_mpc_pptx::testgen::{self, golden, DeckSpec, Shape, Slide};
use k2_mpc_pptx::verbs::{apply, Edit, LayoutSel, Place};

use super::limits;
use super::live::{args, fx, run, Fx};

fn row_err(row: &str, what: impl std::fmt::Display) -> String {
    format!("{row}: {what}")
}

/// An agent's verb with the version of a read it just made as its base.
pub fn agent_verb(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    let v = run(s, c, &["slide", "list", "--limit", "1"])?.version;
    let mut all = args(a);
    all.push("--base".into());
    all.push(v.to_string());
    s.run_tool(c, SHAPES, &all, b"")
}

fn out(o: &ToolOutput) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn drain(rx: &Receiver<ChangeEvent>) -> Vec<ChangeEvent> {
    rx.try_iter().collect()
}

/// The deck's live roots as the pack reads them back (through `read`, so the splice).
fn live_deck(s: &Session, c: ConnId) -> Result<LiveDeck, String> {
    LiveDeck::from_content(&s.read(c).map_err(|e| e.to_string())?.content)
}

/// The sample deck with one text box made of two runs in different formats (`@sld:258/sp:4`).
pub fn formatted_deck() -> Vec<u8> {
    let parts = testgen::parts(&testgen::sample());
    let name = "ppt/slides/slide3.xml";
    let old = parts
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
        .expect("slide 3");
    let run = "<a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t>Left box</a:t></a:r>";
    assert!(old.contains(run), "the fixture's run");
    let new = old.replacen(
        run,
        "<a:r><a:rPr lang=\"en-US\" b=\"1\" dirty=\"0\"/><a:t>Left</a:t></a:r><a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t> box</a:t></a:r>",
        1,
    );
    testgen::stored_zip(&testgen::with_part(&parts, name, new.into_bytes()))
}

fn open(tag: &str, bytes: &[u8]) -> (Fx, std::sync::Arc<Session>) {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    (f, s)
}

/// F2: an agent's `shape move` makes one change event, on `shape_props`, setting the shape's
/// `geom` (one field: x and y move together), and a watcher has it within 50 ms (p99 of 100).
pub fn row_f2_shape_move_one_event() -> Result<(), String> {
    const ROW: &str = "row_f2_shape_move_one_event";
    let (f, s) = open("f2", &testgen::deck(&testgen::sample()));
    let watcher = f.person(&s);
    let agent = f.agent(&s);
    let rx = s.subscribe(watcher).map_err(|e| row_err(ROW, e))?;
    drain(&rx);
    let o = agent_verb(
        &s,
        agent,
        &[
            "shape",
            "move",
            "@sld:258/sp:5",
            "--x",
            "6000000",
            "--y",
            "1600000",
        ],
    )
    .map_err(|e| row_err(ROW, e))?;
    if o.exit != 0 {
        return Err(row_err(ROW, format!("exit {}: {}", o.exit, out(&o))));
    }
    let ev = drain(&rx);
    if ev.len() != 1 {
        return Err(row_err(ROW, format!("{} change events, not one", ev.len())));
    }
    let want_key = prop(&shape_key(258, 5), "geom");
    let ok = match &ev[0].applied {
        Applied::Map { root, ops } => {
            root == SHAPE_PROPS
                && ops.len() == 1
                && ops[0].kind == MapOpKind::Set
                && ops[0].key == want_key
                && ops[0].after == Some(Value::str("6000000,1600000,2000000,1000000,0,0,0"))
        }
        _ => false,
    };
    if !ok {
        return Err(row_err(
            ROW,
            format!("the event isn't one geom set: {:?}", ev[0].applied),
        ));
    }
    let mut lat = Vec::new();
    for i in 0..100 {
        let v = run(&s, agent, &["slide", "list", "--limit", "1"])
            .map_err(|e| row_err(ROW, e))?
            .version;
        let x = (6_001_000 + i * 1000).to_string();
        let a = args(&[
            "shape",
            "move",
            "@sld:258/sp:5",
            "--x",
            &x,
            "--base",
            &v.to_string(),
        ]);
        let t = Instant::now();
        s.run_tool(agent, SHAPES, &a, b"")
            .map_err(|e| row_err(ROW, e))?;
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| row_err(ROW, format!("no event: {e}")))?;
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    lat.sort_by(f64::total_cmp);
    let p99 = lat[98];
    println!(
        "F2: shape move to the watcher's event, 100 moves: p50 {:.3} ms, p99 {:.3} ms",
        lat[49], p99
    );
    if p99 > 50.0 {
        return Err(row_err(ROW, format!("p99 {p99:.3} ms, over 50 ms")));
    }
    Ok(())
}

/// One verb call's expectation: its exit and a piece of its output.
fn expect(
    row: &str,
    s: &Session,
    c: ConnId,
    a: &[&str],
    exit: u8,
    has: &str,
) -> Result<ToolOutput, String> {
    let o = agent_verb(s, c, a).map_err(|e| row_err(row, format!("{a:?}: {e}")))?;
    if o.exit != exit || !out(&o).contains(has) {
        return Err(row_err(
            row,
            format!("{a:?}: exit {}, output {:?}", o.exit, out(&o)),
        ));
    }
    Ok(o)
}

/// Each verb's effect on the store and its refusals, then the file written back holds them.
pub fn row_each_verb() -> Result<(), String> {
    const ROW: &str = "row_each_verb";
    let bytes = formatted_deck();
    let (f, s) = open("verbs", &bytes);
    let a = f.agent(&s);
    let rx = s.subscribe(f.person(&s)).map_err(|e| row_err(ROW, e))?;
    let refusals: &[(&[&str], &str)] = &[
        (
            &["shape", "text", "@sld:258/sp:8", "--text", "x"],
            "refused: pptx_text_not_editable",
        ),
        (
            &["shape", "text", "@sld:258/sp:4", "--text", "x"],
            "refused: pptx_text_formatted",
        ),
        (
            &["shape", "move", "@sld:257/sp:3", "--x", "5"],
            "refused: pptx_needs_a_box",
        ),
        (
            &[
                "shape",
                "order",
                "@sld:258/sp:5",
                "--after",
                "@sld:257/sp:2",
            ],
            "refused: pptx_order_other_slide",
        ),
        (
            &["shape", "delete", "@sld:258/sp:4"],
            "refused: pptx_shape_referenced",
        ),
    ];
    for (call, has) in refusals {
        expect(ROW, &s, a, call, 2, has)?;
        if !drain(&rx).is_empty() {
            return Err(row_err(
                ROW,
                format!("{call:?}: a refused verb made a change"),
            ));
        }
    }
    expect(
        ROW,
        &s,
        a,
        &["shape", "text", "@sld:999/sp:2", "--text", "x"],
        4,
        "not found",
    )?;
    let calls: &[(&[&str], &str)] = &[
        (
            &[
                "shape",
                "text",
                "@sld:258/sp:4",
                "--text",
                "Plain now",
                "--drop-formatting",
            ],
            "@sld:258/sp:4",
        ),
        (
            &["shape", "text", "@sld:256/sp:2", "--text", "A new title"],
            "@sld:256/sp:2",
        ),
        (
            &[
                "shape",
                "move",
                "@sld:257/sp:3",
                "--x",
                "1",
                "--y",
                "2",
                "--w",
                "3",
                "--h",
                "4",
            ],
            "@sld:257/sp:3",
        ),
        (
            &[
                "shape",
                "add",
                "@sld:256",
                "--kind",
                "textbox",
                "--at",
                "100,200,3000,400",
                "--text",
                "Added",
            ],
            "@sld:256/sp:4",
        ),
        (
            &[
                "shape",
                "add",
                "@sld:258",
                "--kind",
                "textbox",
                "--at",
                "100,200,3000,400",
                "--text",
                "After a group",
            ],
            "@sld:258/sp:12",
        ),
        (
            &["shape", "order", "@sld:258/sp:4", "--front"],
            "@sld:258/sp:4",
        ),
        (
            &["shape", "order", "@sld:258/sp:9", "--back"],
            "@sld:258/sp:9",
        ),
        (&["shape", "delete", "@sld:258/sp:7"], "@sld:258/sp:7"),
        (
            &[
                "slide",
                "add",
                "--layout",
                "Title and Content",
                "--after",
                "@sld:257",
            ],
            "@sld:260",
        ),
        (&["slide", "move", "@sld:259", "--first"], "@sld:259"),
        (&["slide", "hide", "@sld:256"], "@sld:256"),
        (
            &["notes", "set", "@sld:256", "--text", "Say hello"],
            "@sld:256",
        ),
        (&["slide", "delete", "@sld:257"], "@sld:257"),
    ];
    for (call, has) in calls {
        expect(ROW, &s, a, call, 0, has)?;
        if drain(&rx).is_empty() {
            return Err(row_err(ROW, format!("{call:?}: no change event")));
        }
    }
    let d = live_deck(&s, a)?;
    let order: Vec<u32> = d.slides.iter().map(|x| x.sid).collect();
    if order != [259, 256, 260, 258] {
        return Err(row_err(ROW, format!("slide order {order:?}")));
    }
    let s258: Vec<u32> = d.slides[3].shapes.iter().map(|x| x.fid).collect();
    if s258 != [9, 2, 5, 6, 8, 12, 4] {
        return Err(row_err(ROW, format!("@sld:258's shapes {s258:?}")));
    }
    let added: Vec<u32> = d.slides[2].shapes.iter().map(|x| x.fid).collect();
    if added != [2, 3] {
        return Err(row_err(ROW, format!("the added slide's shapes {added:?}")));
    }
    // the file written back holds all of it
    let fl = s.flush(a).map_err(|e| row_err(ROW, e))?;
    if !fl.written {
        return Err(row_err(ROW, "the live deck wasn't written back"));
    }
    let back = model::read(&f.bytes(), &limits()).map_err(|e| row_err(ROW, e))?;
    if back.slides.iter().map(|x| x.sid).ne(order.iter().copied()) {
        return Err(row_err(ROW, "the file's slides aren't the live order"));
    }
    Ok(())
}

/// `slide add`'s placeholders take the ids the file verb gives them.
pub fn row_slide_add_ids_as_the_file_verb() -> Result<(), String> {
    const ROW: &str = "row_slide_add_ids_as_the_file_verb";
    let g = golden::deck(golden::Extra::None);
    let (f, s) = open("slide-add", &g);
    let a = f.agent(&s);
    expect(
        ROW,
        &s,
        a,
        &[
            "slide",
            "add",
            "--layout",
            "@layout:2147483650",
            "--after",
            "@sld:259",
        ],
        0,
        "@sld:260",
    )?;
    let live = live_deck(&s, a)?;
    let file = apply(
        &g,
        &Edit::AddSlide {
            layout: LayoutSel::Id(golden::TITLE_CONTENT),
            place: Some(Place::After(259)),
        },
        &limits(),
    )
    .map_err(|e| row_err(ROW, e))?;
    let want = model::read(&file.bytes, &limits()).map_err(|e| row_err(ROW, e))?;
    let shapes = |d: &LiveDeck| -> Vec<(u32, String)> {
        d.slides
            .iter()
            .find(|x| x.sid == 260)
            .map(|x| x.shapes.iter().map(|y| (y.fid, y.name.clone())).collect())
            .unwrap_or_default()
    };
    if shapes(&live) != shapes(&want) || shapes(&live).is_empty() {
        return Err(row_err(
            ROW,
            format!("live {:?}, file verb {:?}", shapes(&live), shapes(&want)),
        ));
    }
    Ok(())
}

/// B1: a shape added live on a slide whose group holds the highest ids takes one past them.
pub fn row_b1_new_shape_after_a_group() -> Result<(), String> {
    const ROW: &str = "row_b1_new_shape_after_a_group";
    let (f, s) = open("b1", &testgen::deck(&testgen::sample()));
    let a = f.agent(&s);
    let o = expect(
        ROW,
        &s,
        a,
        &[
            "shape",
            "add",
            "@sld:258",
            "--kind",
            "textbox",
            "--at",
            "0,0,10,10",
            "--text",
            "x",
        ],
        0,
        "@sld:258/sp:",
    )?;
    if out(&o).trim() != "@sld:258/sp:12" {
        return Err(row_err(
            ROW,
            format!(
                "the new shape is {} (the group holds 10 and 11)",
                out(&o).trim()
            ),
        ));
    }
    Ok(())
}

/// An agent's edit needs a base, and one on a stale base is refused by facet.
pub fn row_base_required_and_checked() -> Result<(), String> {
    const ROW: &str = "row_base_required_and_checked";
    let (f, s) = open("base", &testgen::deck(&testgen::sample()));
    let a = f.agent(&s);
    let p = f.person(&s);
    match run(&s, a, &["shape", "move", "@sld:258/sp:5", "--x", "1"]) {
        Err(ToolRunError::Session(SessionError::NeedsBase)) => {}
        other => {
            return Err(row_err(
                ROW,
                format!("no base: {:?}", other.map(|o| o.exit)),
            ))
        }
    }
    let v = run(&s, a, &["slide", "list"])
        .map_err(|e| row_err(ROW, e))?
        .version;
    run(&s, p, &["shape", "move", "@sld:258/sp:5", "--x", "2"]).map_err(|e| row_err(ROW, e))?;
    let stale = args(&[
        "shape",
        "move",
        "@sld:258/sp:5",
        "--y",
        "3",
        "--base",
        &v.to_string(),
    ]);
    match s.run_tool(a, SHAPES, &stale, b"") {
        Err(ToolRunError::Session(SessionError::FacetConflict(c)))
            if c.iter().any(|x| x.key == prop(&shape_key(258, 5), "geom")) => {}
        other => {
            return Err(row_err(
                ROW,
                format!("a stale base: {:?}", other.map(|o| o.exit)),
            ))
        }
    }
    // a stale base on another facet of the same shape passes
    let text = args(&[
        "shape",
        "text",
        "@sld:258/sp:5",
        "--text",
        "ok",
        "--base",
        &v.to_string(),
    ]);
    let o = s
        .run_tool(a, SHAPES, &text, b"")
        .map_err(|e| row_err(ROW, e))?;
    if o.exit != 0 {
        return Err(row_err(
            ROW,
            format!("another facet on the same base: exit {}", o.exit),
        ));
    }
    Ok(())
}

/// A watcher sees each verb's change, by key and by whom, through the golden turn.
pub fn row_watch_sees_each_change() -> Result<(), String> {
    const ROW: &str = "row_watch_sees_each_change";
    let (f, s) = open("watch", &golden::deck(golden::Extra::None));
    let a = f.agent(&s);
    let rx = s.subscribe(f.person(&s)).map_err(|e| row_err(ROW, e))?;
    let turn: &[&[&str]] = &[
        &["shape", "text", "@sld:256/sp:2", "--text", "Q4 review"],
        &[
            "shape",
            "move",
            "@sld:257/sp:7",
            "--x",
            "7500000",
            "--y",
            "4000000",
        ],
        &[
            "slide",
            "add",
            "--layout",
            "@layout:2147483650",
            "--after",
            "@sld:259",
        ],
        &["shape", "text", "@sld:260/sp:2", "--text", "Risks"],
        &["slide", "move", "@sld:259", "--after", "@sld:257"],
        &[
            "shape",
            "text",
            "@sld:260/sp:3",
            "--text",
            "Supplier delays\nHiring pace",
        ],
    ];
    let mut lines = Vec::new();
    for call in turn {
        expect(ROW, &s, a, call, 0, "@sld:")?;
        let ev = drain(&rx);
        if ev.is_empty() {
            return Err(row_err(ROW, format!("{call:?}: the watcher saw nothing")));
        }
        for e in &ev {
            // the kit's `via` is the participant's client, not the verb (a contract gap: Item 2
            // reads the verb from it): the row holds the agent and the change kind
            if e.pid != "atlas" {
                return Err(row_err(ROW, format!("{call:?}: by {:?}", e.pid)));
            }
            lines.push(k2_mpc_pptx::live::watch_line(e));
        }
    }
    for l in &lines {
        println!("watch: {l}");
    }
    let order: Vec<u32> = live_deck(&s, a)?.slides.iter().map(|x| x.sid).collect();
    if order != [256, 257, 259, 258, 260] {
        return Err(row_err(ROW, format!("the order after the turn {order:?}")));
    }
    Ok(())
}

// ---------- the differential oracle ----------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One verb, as the live call and the file verb's edit.
fn random_verb(r: &mut Rng, d: &LiveDeck, n: usize) -> Option<(Vec<String>, Edit)> {
    let slides: Vec<&LiveSlide> = d.slides.iter().collect();
    let s = slides[r.below(slides.len())];
    let plain: Vec<u32> = s
        .shapes
        .iter()
        .filter(|x| x.edit == model::TextEdit::Plain && !x.mc)
        .map(|x| x.fid)
        .collect();
    let boxed: Vec<u32> = s
        .shapes
        .iter()
        .filter(|x| x.geom.is_some() && !x.mc)
        .map(|x| x.fid)
        .collect();
    match r.below(6) {
        0 if !plain.is_empty() => {
            let fid = plain[r.below(plain.len())];
            let t = format!("text {n}\nline {}", r.below(100));
            Some((
                args(&[
                    "shape",
                    "text",
                    &format!("@sld:{}/sp:{fid}", s.sid),
                    "--text",
                    &t,
                ]),
                Edit::SetText {
                    slide: s.sid,
                    shape: fid,
                    text: t,
                },
            ))
        }
        1 if !boxed.is_empty() => {
            let fid = boxed[r.below(boxed.len())];
            let x = (r.below(9_000_000)) as i64;
            Some((
                args(&[
                    "shape",
                    "move",
                    &format!("@sld:{}/sp:{fid}", s.sid),
                    "--x",
                    &x.to_string(),
                ]),
                Edit::MoveShape {
                    slide: s.sid,
                    shape: fid,
                    x: Some(x),
                    y: None,
                    w: None,
                    h: None,
                },
            ))
        }
        2 => {
            let at = (r.below(1000) as i64, r.below(1000) as i64, 500, 300);
            let t = format!("box {n}");
            Some((
                args(&[
                    "shape",
                    "add",
                    &format!("@sld:{}", s.sid),
                    "--kind",
                    "textbox",
                    "--at",
                    &format!("{},{},{},{}", at.0, at.1, at.2, at.3),
                    "--text",
                    &t,
                ]),
                Edit::AddTextBox {
                    slide: s.sid,
                    at,
                    text: t,
                },
            ))
        }
        3 => {
            let after = slides[r.below(slides.len())].sid;
            Some((
                args(&[
                    "slide",
                    "add",
                    "--layout",
                    &format!("@layout:{}", testgen::LAYOUT_CONTENT),
                    "--after",
                    &format!("@sld:{after}"),
                ]),
                Edit::AddSlide {
                    layout: LayoutSel::Id(testgen::LAYOUT_CONTENT),
                    place: Some(Place::After(after)),
                },
            ))
        }
        4 if slides.len() > 1 => {
            let other = slides[r.below(slides.len())].sid;
            if other == s.sid {
                return None;
            }
            Some((
                args(&[
                    "slide",
                    "move",
                    &format!("@sld:{}", s.sid),
                    "--after",
                    &format!("@sld:{other}"),
                ]),
                Edit::MoveSlide {
                    slide: s.sid,
                    place: Place::After(other),
                },
            ))
        }
        5 => {
            let t = format!("notes {n}");
            Some((
                args(&["notes", "set", &format!("@sld:{}", s.sid), "--text", &t]),
                Edit::SetNotes {
                    slide: s.sid,
                    text: t,
                },
            ))
        }
        _ => None,
    }
}

fn guarded(d: &LiveDeck) -> Vec<LiveSlide> {
    d.slides
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

/// The differential oracle: for seeded random sequences of live verbs, the file
/// the live deck writes back reads as the file verbs applied to the same file in the same
/// order. Shape deletes come last in each sequence: the file verb reuses a deleted top shape's
/// id, the live store never does (B1's mark), so an add after a delete may differ by design.
pub fn row_differential_oracle() -> Result<(), String> {
    const ROW: &str = "row_differential_oracle";
    for seed in 1..=12u64 {
        let start = testgen::deck(&testgen::sample());
        let (f, s) = open("oracle", &start);
        let a = f.agent(&s);
        let mut file = start.clone();
        let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
        let mut n = 0;
        let mut tries = 0;
        while n < 12 && tries < 200 {
            tries += 1;
            let d = model::read(&file, &limits()).map_err(|e| row_err(ROW, e))?;
            let Some((call, edit)) = random_verb(&mut r, &d, n) else {
                continue;
            };
            let call: Vec<&str> = call.iter().map(String::as_str).collect();
            let o = agent_verb(&s, a, &call)
                .map_err(|e| row_err(ROW, format!("seed {seed}: {call:?}: {e}")))?;
            let fv = apply(&file, &edit, &limits());
            match (o.exit, fv) {
                (0, Ok(x)) => file = x.bytes,
                (e, Err(fe)) if e != 0 => {
                    let _ = fe;
                    continue;
                }
                (e, other) => {
                    return Err(row_err(
                        ROW,
                        format!(
                            "seed {seed}: {call:?}: live exit {e} ({}), file verb {:?}",
                            out(&o).trim(),
                            other.map(|_| "applied")
                        ),
                    ));
                }
            }
            n += 1;
        }
        // deletes last
        let d = model::read(&file, &limits()).map_err(|e| row_err(ROW, e))?;
        for sl in &d.slides {
            if let Some(x) = sl.shapes.iter().find(|x| !x.refd && x.kind == "textbox") {
                let addr = format!("@sld:{}/sp:{}", sl.sid, x.fid);
                expect(ROW, &s, a, &["shape", "delete", &addr], 0, &addr)?;
                file = apply(
                    &file,
                    &Edit::DeleteShape {
                        slide: sl.sid,
                        shape: x.fid,
                    },
                    &limits(),
                )
                .map_err(|e| row_err(ROW, e))?
                .bytes;
            }
        }
        let live = s
            .read(a)
            .map_err(|e| row_err(ROW, format!("seed {seed}: the live write: {e}")))?
            .bytes;
        let got = model::read(&live, &limits()).map_err(|e| row_err(ROW, e))?;
        let want = model::read(&file, &limits()).map_err(|e| row_err(ROW, e))?;
        if guarded(&got) != guarded(&want) {
            let ids = |d: &LiveDeck| -> Vec<(u32, Vec<u32>)> {
                d.slides
                    .iter()
                    .map(|s| (s.sid, s.shapes.iter().map(|x| x.fid).collect()))
                    .collect()
            };
            return Err(row_err(
                ROW,
                format!(
                    "seed {seed}: live {:?}, file verbs {:?}",
                    ids(&got),
                    ids(&want)
                ),
            ));
        }
    }
    Ok(())
}

// ---------- bounded reads ----------

/// A deck whose one slide holds `n` text boxes, each with `text`.
pub fn crowded(n: u32, text: &str) -> Vec<u8> {
    let mut s = Slide::titled(256, "Crowded", "");
    s.shapes = (0..n)
        .map(|i| Shape::text_box(10 + i, (i as i64 * 10, 0, 100, 100), text))
        .collect();
    testgen::deck(&DeckSpec::new(vec![s]))
}

/// `slide show` holds at most 64 shapes a page and cuts text at 1,024 bytes, with a cursor to
/// the rest; `shape show` cuts at 16,384 with `text_truncated`; `slide list` pages.
pub fn row_reads_bounded_and_paged() -> Result<(), String> {
    const ROW: &str = "row_reads_bounded_and_paged";
    let (f, s) = open("reads", &crowded(100, &"y".repeat(20_000)));
    let a = f.agent(&s);
    let o = run(&s, a, &["slide", "show", "@sld:256"]).map_err(|e| row_err(ROW, e))?;
    let page = out(&o);
    let shapes = page
        .lines()
        .filter(|l| l.starts_with("@sld:256/sp:"))
        .count();
    if shapes != 64 {
        return Err(row_err(
            ROW,
            format!("a page holds {shapes} shapes, not 64"),
        ));
    }
    if page.len() > 64 * (1024 + 200) + 400 {
        return Err(row_err(ROW, format!("a page is {} bytes", page.len())));
    }
    let next = page
        .lines()
        .find_map(|l| l.strip_prefix("next: slide show @sld:256 --after "))
        .ok_or_else(|| row_err(ROW, "no cursor"))?
        .to_owned();
    let o2 = run(&s, a, &["slide", "show", "@sld:256", "--after", &next])
        .map_err(|e| row_err(ROW, e))?;
    let rest = out(&o2)
        .lines()
        .filter(|l| l.starts_with("@sld:256/sp:"))
        .count();
    if rest != 36 || out(&o2).contains("next:") {
        return Err(row_err(ROW, format!("the second page holds {rest} shapes")));
    }
    let sh = out(&run(&s, a, &["shape", "show", "@sld:256/sp:10"]).map_err(|e| row_err(ROW, e))?);
    if !sh.contains("text_truncated\tyes") || sh.len() > 16_384 + 400 {
        return Err(row_err(
            ROW,
            format!(
                "shape show is {} bytes, cut: {}",
                sh.len(),
                sh.contains("text_truncated\tyes")
            ),
        ));
    }
    // slide list pages
    let many: Vec<Slide> = (0..150)
        .map(|i| Slide::titled(256 + i, &format!("S{i}"), ""))
        .collect();
    let (f2, s2) = open("list", &testgen::deck(&DeckSpec::new(many)));
    let b = f2.agent(&s2);
    let l = out(&run(&s2, b, &["slide", "list"]).map_err(|e| row_err(ROW, e))?);
    if l.lines().filter(|x| x.starts_with("@sld:")).count() != 100
        || !l.contains("next: slide list --after @sld:355")
    {
        return Err(row_err(ROW, "slide list's first page"));
    }
    let l2 =
        out(&run(&s2, b, &["slide", "list", "--after", "@sld:355"]).map_err(|e| row_err(ROW, e))?);
    if l2.lines().filter(|x| x.starts_with("@sld:")).count() != 50 || l2.contains("next:") {
        return Err(row_err(ROW, "slide list's second page"));
    }
    Ok(())
}

/// A live deck at its keyed cap refuses growth (the kit's `MPC-E822`), and text over the
/// field limit is refused before anything applies.
pub fn row_limits_refuse_growth_and_long_text() -> Result<(), String> {
    const ROW: &str = "row_limits_refuse_growth_and_long_text";
    let bytes = testgen::deck(&testgen::sample());
    let n = model::read(&bytes, &limits())
        .map_err(|e| row_err(ROW, e))?
        .entries();
    let f = fx("cap", |c| c.keyed_max_entries = n + 2);
    f.write(&bytes);
    let s = f.open();
    let a = f.agent(&s);
    match agent_verb(
        &s,
        a,
        &[
            "shape", "add", "@sld:256", "--kind", "textbox", "--at", "0,0,1,1", "--text", "x",
        ],
    ) {
        Err(ToolRunError::Session(e @ SessionError::AtLimit { .. })) => {
            let id = e.report().id().to_owned();
            if id != "MPC-E822" {
                return Err(row_err(ROW, format!("growth refused as {id}")));
            }
        }
        other => {
            return Err(row_err(
                ROW,
                format!("growth past the cap: {:?}", other.map(|o| o.exit)),
            ))
        }
    }
    let long = "z".repeat(model::TEXT_MAX + 1);
    match agent_verb(&s, a, &["shape", "text", "@sld:256/sp:2", "--text", &long]) {
        Err(ToolRunError::Tool(_)) => {}
        other => {
            return Err(row_err(
                ROW,
                format!("long text: {:?}", other.map(|o| o.exit)),
            ))
        }
    }
    Ok(())
}
