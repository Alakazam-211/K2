//! The edit verbs, plan by plan: what each writes and nothing else, in the file's style, its
//! numbers exact, its new handles safe, its refusals typed; and the self-check refusing what
//! doesn't read back. Each row builds its drawing with the generator and reads the output with
//! the pack's own reader; the referee (ezdxf, a test checker) reads the same outputs in its own
//! job (`cad-measure edits`).

use k2_mpc_dxf::edit::check::{self_check, Intent};
use k2_mpc_dxf::edit::{self, Planned};
use k2_mpc_dxf::testgen::{self, splice_attrib, splice_object, splice_value, Ent, Eol, Ver};
use k2_mpc_dxf::{convert, structure, token, CadError, Limits};

fn plan(bytes: &[u8], verb: &str, args: &[&str]) -> Result<Planned, CadError> {
    edit::plan(bytes, verb, args)
}

/// The plan, or the row's failure naming the refusal.
fn landed(row: &str, bytes: &[u8], verb: &str, args: &[&str]) -> Planned {
    match plan(bytes, verb, args) {
        Ok(p) => p,
        Err(e) => panic!("{row}: {verb} {args:?} was refused: {e}"),
    }
}

fn refused_id(bytes: &[u8], verb: &str, args: &[&str]) -> String {
    match plan(bytes, verb, args) {
        Ok(p) => format!("landed ({} spans)", p.spans.len()),
        Err(e) => k2_mpc_dxf::pack::pack_id(e.code.id),
    }
}

/// A count of the plan's data (`plan.<k>`).
fn plan_count(p: &Planned, k: &str) -> usize {
    p.data
        .get("plan")
        .and_then(|v| v.get(k))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize
}

/// Each pair of `bytes`: (code, value bytes, value range, start).
fn pairs(bytes: &[u8]) -> Vec<(i32, Vec<u8>, std::ops::Range<usize>, usize)> {
    let t = token::tokenize(bytes, &Limits::default()).expect("the output reads");
    t.pairs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.code, t.value(i).to_vec(), p.value_range(), p.start()))
        .collect()
}

/// The pairs of the entity with handle `h` (its followers included), as (code, value).
fn entity_pairs(bytes: &[u8], h: u64) -> Vec<(i32, String)> {
    let l = Limits::default();
    let s = structure::walk(token::tokenize(bytes, &l).unwrap(), &l).unwrap();
    let e = s
        .entities
        .iter()
        .find(|e| {
            (e.head.first + 1..e.head.end)
                .find(|&i| s.tok.pairs[i].code == 5)
                .and_then(|i| k2_mpc_dxf::handle::parse(s.tok.value(i)))
                == Some(h)
        })
        .unwrap_or_else(|| panic!("no entity @h:{h:X}"));
    let end = e.followers.last().map_or(e.head.end, |f| f.end);
    (e.head.first..end)
        .map(|i| (s.tok.pairs[i].code, s.tok.text(i).into_owned()))
        .collect()
}

fn values(p: &[(i32, String)], code: i32) -> Vec<String> {
    p.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, v)| v.clone())
        .collect()
}

/// The edit sampler in each style the format allows.
fn variants() -> Vec<(String, testgen::Built)> {
    let mut out = Vec::new();
    for v in [
        Ver::R2000,
        Ver::R2004,
        Ver::R2007,
        Ver::R2010,
        Ver::R2013,
        Ver::R2018,
    ] {
        for (eol, pad) in [(Eol::Lf, false), (Eol::CrLf, true), (Eol::Mixed, false)] {
            let mut s = testgen::edit_sampler(v);
            s.opts.eol = eol;
            s.opts.pad = pad;
            s.opts.comments = pad;
            s.opts.bom = eol == Eol::CrLf;
            s.opts.lower_handles = eol == Eol::Mixed;
            out.push((format!("{}-{eol:?}-pad{pad}", v.acadver()), s.build()));
        }
    }
    out
}

/// One call of each verb on a built sampler.
fn calls(b: &testgen::Built) -> Vec<(&'static str, Vec<String>)> {
    let a = |l: &str| b.addr(l);
    vec![
        (
            "entity move",
            vec![
                a("wall"),
                a("dev"),
                a("lw"),
                "--by".into(),
                "1.5,-2".into(),
                "--unit".into(),
                "mm".into(),
            ],
        ),
        (
            "entity set",
            vec![
                a("wall"),
                a("lamp"),
                "--color".into(),
                "3".into(),
                "--ltype".into(),
                "CONTINUOUS".into(),
            ],
        ),
        (
            "attrib set",
            vec![
                a("dev"),
                "--tag".into(),
                "DEVICE".into(),
                "--value".into(),
                "CAM-07".into(),
            ],
        ),
        (
            "entity add",
            vec![
                "line".into(),
                "--from".into(),
                "0,0".into(),
                "--to".into(),
                "3,4".into(),
                "--unit".into(),
                "mm".into(),
                "--layer".into(),
                "A-WALL".into(),
            ],
        ),
        ("entity delete", vec![a("arc"), a("lamp")]),
        (
            "entity rotate",
            vec![
                a("wall"),
                a("dev"),
                a("room"),
                a("arc"),
                "--by".into(),
                "90".into(),
                "--about".into(),
                "1,1".into(),
                "--unit".into(),
                "mm".into(),
            ],
        ),
        (
            "entity scale",
            vec![
                a("lamp"),
                a("dev"),
                a("lw"),
                "--factor".into(),
                "1.5".into(),
                "--about".into(),
                "0,0".into(),
                "--unit".into(),
                "mm".into(),
            ],
        ),
        (
            "block insert",
            vec![
                "--block".into(),
                "TAGGED".into(),
                "--at".into(),
                "100,50".into(),
                "--rotate".into(),
                "90".into(),
                "--scale".into(),
                "2".into(),
                "--attr".into(),
                "DEVICE=CAM-50".into(),
                "--unit".into(),
                "mm".into(),
                "--layer".into(),
                "E-DEV".into(),
            ],
        ),
        (
            "text set",
            vec![a("room"), "--value".into(), "ROOM 202".into()],
        ),
    ]
}

/// untouched_units_exact: every verb on every style of the sampler plans only value lines,
/// whole-pair inserts at a pair's start and whole-pair removals; the output reads, with the
/// pair count the plan says. Plant: `SpanCoversCodeLine`.
#[test]
pub fn untouched_units_exact() {
    let mut planned = 0;
    for (name, b) in variants() {
        let before = pairs(&b.bytes);
        let values: Vec<std::ops::Range<usize>> = before.iter().map(|p| p.2.clone()).collect();
        let starts: std::collections::BTreeSet<usize> = before.iter().map(|p| p.3).collect();
        for (verb, args) in calls(&b) {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let p = landed("untouched_units_exact", &b.bytes, verb, &refs);
            for sp in &p.spans {
                let ok = if sp.lo == sp.hi {
                    starts.contains(&sp.lo) || sp.lo == b.bytes.len()
                } else if sp.with.is_empty() {
                    starts.contains(&sp.lo)
                        && (starts.iter().any(|s| *s == sp.hi) || sp.hi == b.bytes.len())
                } else {
                    values.iter().any(|r| r.start == sp.lo && r.end == sp.hi)
                };
                assert!(
                    ok,
                    "untouched_units_exact: {name}: {verb}: a range at {}..{} is neither a value \
                     line, a whole-pair insert nor a whole-pair removal",
                    sp.lo, sp.hi
                );
            }
            let after = pairs(&p.output);
            let ins = plan_count(&p, "pairs_inserted");
            let rem = plan_count(&p, "pairs_removed");
            assert_eq!(
                after.len() + rem,
                before.len() + ins,
                "untouched_units_exact: {name}: {verb}: the pair count"
            );
            planned += 1;
        }
    }
    println!("untouched_units_exact: {planned} plans over 18 styles");
}

/// style_kept: in a CRLF file with padded codes, every pair a verb writes ends its lines with
/// CRLF and pads its code. Plant: `EditStyleLeaks`.
#[test]
pub fn style_kept() {
    let mut s = testgen::edit_sampler(Ver::R2018);
    s.opts.eol = Eol::CrLf;
    s.opts.pad = true;
    let b = s.build();
    let mut written = 0;
    for (verb, args) in calls(&b) {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let p = landed("style_kept", &b.bytes, verb, &refs);
        for sp in p.spans.iter().filter(|sp| sp.lo == sp.hi) {
            let text = &sp.with;
            for (k, c) in text.iter().enumerate() {
                if *c == b'\n' {
                    assert!(
                        k > 0 && text[k - 1] == b'\r',
                        "style_kept: {verb}: a written line ends with LF alone"
                    );
                }
            }
            let first = text.split(|c| *c == b'\r').next().unwrap_or_default();
            let code = String::from_utf8_lossy(first.trim_ascii());
            assert!(
                first == format!("{code:>3}").as_bytes(),
                "style_kept: {verb}: a written code isn't padded as the file's are"
            );
            written += 1;
        }
    }
    assert!(written >= 2, "style_kept: inserts were written ({written})");
    println!("style_kept: {written} inserts in the file's style");
}

/// move_decimal_exact: a move adds in decimal on the file's own tokens: 1.1 + 0.1 is 1.2, 10.0
/// + 0.1 is 10.1, a coordinate moved by 0 keeps its token. Plant: `FloatDriftOnMove`.
#[test]
pub fn move_decimal_exact() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let p = landed(
        "move_decimal_exact",
        &b.bytes,
        "entity move",
        &[&b.addr("wall"), "--by", "0.1,0", "--unit", "mm"],
    );
    let e = entity_pairs(&p.output, b.handle("wall"));
    assert_eq!(
        (values(&e, 10), values(&e, 11)),
        (vec!["1.2".to_owned()], vec!["10.1".to_owned()]),
        "move_decimal_exact: the x values"
    );
    assert_eq!(
        (values(&e, 20), values(&e, 21)),
        (vec!["0.0".to_owned()], vec!["0.0".to_owned()]),
        "move_decimal_exact: y moved by 0 keeps its token"
    );
    println!("move_decimal_exact: 1.1 + 0.1 = 1.2, 10.0 + 0.1 = 10.1");
}

/// point_table: a move moves each kind's points and nothing else: an ellipse's major axis (a
/// vector) and a circle's radius stay, every vertex of a polyline moves, a block reference's
/// attributes move with it. Plants: `VectorMovedAsPoint`, `AttribsLeftBehind`.
#[test]
pub fn point_table() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let targets = ["ellipse", "lamp", "lw", "poly", "dev", "room", "note"];
    let mut args: Vec<String> = targets.iter().map(|l| b.addr(l)).collect();
    args.extend(["--by".into(), "10,5".into(), "--unit".into(), "mm".into()]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let p = landed("point_table", &b.bytes, "entity move", &refs);
    let el = entity_pairs(&p.output, b.handle("ellipse"));
    assert_eq!(
        (
            values(&el, 10),
            values(&el, 20),
            values(&el, 11),
            values(&el, 21)
        ),
        (
            vec!["15.0".to_owned()],
            vec!["10.0".to_owned()],
            vec!["2.0".to_owned()],
            vec!["1.0".to_owned()]
        ),
        "point_table: an ellipse's centre moves, its major axis doesn't"
    );
    let c = entity_pairs(&p.output, b.handle("lamp"));
    assert_eq!(
        (values(&c, 10), values(&c, 40)),
        (vec!["30.0".to_owned()], vec!["2.0".to_owned()]),
        "point_table: a circle's centre moves, its radius doesn't"
    );
    let lw = entity_pairs(&p.output, b.handle("lw"));
    assert_eq!(
        values(&lw, 10),
        vec!["10.0", "20.0", "20.0"],
        "point_table: every vertex moves"
    );
    let poly = entity_pairs(&p.output, b.handle("poly"));
    assert_eq!(
        values(&poly, 10),
        vec!["0.0", "10.0", "14.0", "14.0"],
        "point_table: a polyline's own 10 holds its elevation and stays; its vertices move"
    );
    let dev = entity_pairs(&p.output, b.handle("dev"));
    assert_eq!(
        values(&dev, 10),
        vec!["40.0", "40.0", "40.0"],
        "point_table: a block reference and its two attributes move together"
    );
    let note = entity_pairs(&p.output, b.handle("note"));
    assert_eq!(
        values(&note, 10),
        vec!["11.0"],
        "point_table: a multiline text's insertion point moves"
    );
    println!("point_table: 7 kinds moved by 10,5");
}

/// attrib_value_written: an attribute's value by tag, written as the file writes text: in a
/// 2000 file (code page 1252) `é` is its byte and a character the page lacks is `\U+XXXX`; in a
/// 2018 file, UTF-8. Either reads back the same. Plant: `CodepageIgnored`.
#[test]
pub fn attrib_value_written() {
    let value = "Caf\u{e9} \u{2116}5";
    for (v, want) in [
        (Ver::R2000, b"Caf\xe9 \\U+21165".to_vec()),
        (Ver::R2018, value.as_bytes().to_vec()),
    ] {
        let b = testgen::edit_sampler(v).build();
        let p = landed(
            "attrib_value_written",
            &b.bytes,
            "attrib set",
            &[&b.addr("dev"), "--tag", "LABEL", "--value", value],
        );
        let got: Vec<Vec<u8>> = p.spans.iter().map(|s| s.with.clone()).collect();
        assert_eq!(
            got,
            vec![want],
            "attrib_value_written: {}: the value line as written",
            v.acadver()
        );
        let read = k2_mpc_dxf::verbs::run(&p.output, "block show", &[&b.addr("dev"), "--attribs"])
            .expect("block show reads");
        assert!(
            read.data.to_json().contains("Caf\u{e9} \u{2116}5"),
            "attrib_value_written: {}: it reads back: {}",
            v.acadver(),
            read.data.to_json()
        );
    }
    println!("attrib_value_written: 2 versions");
}

/// set_props: a colour number replaces the true colour (`420` removed); a linetype goes in its
/// place in the common group (after the layer, before the colour). Plants: `TrueColourLeft`,
/// `PairOrderIgnored`.
#[test]
pub fn set_props() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let p = landed(
        "set_props",
        &b.bytes,
        "entity set",
        &[&b.addr("red"), "--color", "3"],
    );
    let e = entity_pairs(&p.output, b.handle("red"));
    assert_eq!(
        (values(&e, 62), values(&e, 420)),
        (vec!["3".to_owned()], Vec::<String>::new()),
        "set_props: a colour number replaces the true colour"
    );
    let p = landed(
        "set_props",
        &b.bytes,
        "entity set",
        &[&b.addr("red"), "--ltype", "CONTINUOUS"],
    );
    let codes: Vec<i32> = entity_pairs(&p.output, b.handle("red"))
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| [8, 6, 62, 420].contains(c))
        .collect();
    assert_eq!(
        codes,
        vec![8, 6, 62, 420],
        "set_props: the linetype goes after the layer, before the colour"
    );
    println!("set_props: 2 changes");
}

/// adds_mint_above_hmax: a new entity's handle is above every handle owned or pointed at (a
/// dangling pointer included), and `$HANDSEED` is raised past it. Plants: `HmaxOwnedOnly`,
/// `HandseedLeft`.
#[test]
pub fn adds_mint_above_hmax() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    // an object pointing at a handle above every one owned
    let bytes = splice_object(
        &b.bytes,
        &[
            (0, "XRECORD"),
            (5, "E00"),
            (330, "C"),
            (100, "AcDbXrecord"),
            (340, "FF0"),
        ],
    );
    let p = plan(
        &bytes,
        "entity add",
        &[
            "text", "--at", "1,2", "--height", "2.5", "--value", "NEW", "--unit", "mm",
        ],
    );
    assert!(
        p.is_ok(),
        "adds_mint_above_hmax: the add lands: {:?}",
        p.as_ref().err()
    );
    let p = p.unwrap();
    assert_eq!(
        p.created,
        vec!["@h:FF1".to_owned()],
        "adds_mint_above_hmax: the handle is above the dangling pointer"
    );
    let seed = k2_mpc_dxf::verbs::run(&p.output, "header get", &["$HANDSEED"]).unwrap();
    assert!(
        seed.stdout.contains("\t5\tFF2"),
        "adds_mint_above_hmax: $HANDSEED raised past it: {}",
        seed.stdout
    );
    println!("adds_mint_above_hmax: @h:FF1, $HANDSEED FF2");
}

/// delete_cleans_known_refs: a delete takes the entity's followers (attributes, the end marker)
/// and its row of the draw-order table with it; another object pointing at it refuses the
/// delete (`DXF-E027`). Plants: `FollowersLeft`, `SortentsLeft`.
#[test]
pub fn delete_cleans_known_refs() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let p = plan(
        &b.bytes,
        "entity delete",
        &[&b.addr("dev"), &b.addr("lamp")],
    );
    assert!(
        p.is_ok(),
        "delete_cleans_known_refs: the delete lands: {:?}",
        p.as_ref().err()
    );
    let p = p.unwrap();
    let after = pairs(&p.output);
    let has = |code: i32, v: &str| after.iter().any(|x| x.0 == code && x.1 == v.as_bytes());
    assert!(
        !has(331, "A00"),
        "delete_cleans_known_refs: the draw-order row of the lamp is gone"
    );
    let attribs = after
        .iter()
        .filter(|x| x.0 == 0 && x.1 == b"ATTRIB")
        .count();
    let before = pairs(&b.bytes)
        .iter()
        .filter(|x| x.0 == 0 && x.1 == b"ATTRIB")
        .count();
    assert_eq!(
        attribs,
        before - 2,
        "delete_cleans_known_refs: the reference's two attributes went with it"
    );
    let pointed = splice_object(
        &b.bytes,
        &[
            (0, "XRECORD"),
            (5, "E00"),
            (330, "C"),
            (100, "AcDbXrecord"),
            (340, &format!("{:X}", b.handle("wall"))),
        ],
    );
    assert_eq!(
        refused_id(&pointed, "entity delete", &[&b.addr("wall")]),
        "DXF-E027",
        "delete_cleans_known_refs: a pointer the pack can't mend refuses"
    );
    println!("delete_cleans_known_refs: 2 deleted, 1 refused");
}

/// self_check_catches: the self-check refuses an output where a unit outside the plan changed
/// (b), where the pair count is off (a), and where a pointer still names what was removed (e).
/// Plant: `SelfCheckSkipped`.
#[test]
pub fn self_check_catches() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let wall = b.handle("wall");
    let lamp = b.handle("lamp");
    let mv = landed(
        "self_check_catches",
        &b.bytes,
        "entity move",
        &[&b.addr("wall"), "--by", "1,0", "--unit", "mm"],
    );
    let intent = Intent {
        changed: [wall].into(),
        hmax: 0,
        ..Intent::default()
    };
    let mut caught = Vec::new();
    // (b): the lamp's radius changed too
    let stray = splice_value(&mv.output, lamp, 40, "9.0");
    caught.push(("b", self_check(&b.bytes, &stray, &intent, 0, 0)));
    // (a): a pair the plan didn't insert
    let extra = testgen::splice_insert(&mv.output, wall, 8, 999, "a comment");
    caught.push(("a", self_check(&b.bytes, &extra, &intent, 0, 0)));
    // (e): the lamp cut, its draw-order row left
    let cut = testgen::splice_delete(&b.bytes, lamp);
    let removed = Intent {
        removed: [lamp].into(),
        ..Intent::default()
    };
    let pairs_cut = pairs(&b.bytes).len() - pairs(&cut).len();
    caught.push(("e", self_check(&b.bytes, &cut, &removed, 0, pairs_cut)));
    for (check, r) in &caught {
        match r {
            Err(e) => assert!(
                e.code.id == "DXF-E090" && e.why.contains(&format!("({check})")),
                "self_check_catches: ({check}) refused as itself: {e}"
            ),
            Ok(()) => panic!("self_check_catches: ({check}) wasn't caught"),
        }
    }
    assert!(
        self_check(&b.bytes, &mv.output, &intent, 0, 0).is_ok(),
        "self_check_catches: the honest move passes"
    );
    println!("self_check_catches: 3 caught, the honest move passes");
}

/// honest_edits_land: every entity the rules allow takes each verb: a move, a colour, a delete,
/// a turn and a scale (a dynamic block's reference and an entity on a frozen layer included),
/// both references' attributes, the texts' values, and a reference of each block a reference can
/// name (one whose annotative mark is off included). Plants: `DynblockAttribRefused`,
/// `LockedReadAsFrozen`, `AnnotativeReadLoose`.
#[test]
pub fn honest_edits_land() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let honest = [
        "wall", "red", "lamp", "arc", "lw", "poly", "room", "note", "ellipse", "dev", "dyn",
        "twice", "frozen", "title",
    ];
    let mut landed_n = 0;
    for l in honest {
        let a = b.addr(l);
        for (verb, args) in [
            (
                "entity move",
                vec![a.as_str(), "--by", "1,1", "--unit", "mm"],
            ),
            ("entity set", vec![a.as_str(), "--color", "2"]),
            ("entity delete", vec![a.as_str()]),
            (
                "entity rotate",
                vec![a.as_str(), "--by", "30", "--about", "0,0", "--unit", "mm"],
            ),
            (
                "entity scale",
                vec![
                    a.as_str(),
                    "--factor",
                    "2",
                    "--about",
                    "0,0",
                    "--unit",
                    "mm",
                ],
            ),
        ] {
            landed("honest_edits_land", &b.bytes, verb, &args);
            landed_n += 1;
        }
    }
    for l in ["dev", "dyn"] {
        landed(
            "honest_edits_land",
            &b.bytes,
            "attrib set",
            &[&b.addr(l), "--tag", "DEVICE", "--value", "CAM-99"],
        );
        landed_n += 1;
    }
    landed(
        "honest_edits_land",
        &b.bytes,
        "attrib set",
        &[
            &b.addr("twice"),
            "--tag",
            "DEVICE",
            "--nth",
            "2",
            "--value",
            "C",
        ],
    );
    for kind in [
        vec!["line", "--from", "0,0", "--to", "1,1"],
        vec![
            "polyline",
            "--points",
            "0,0;1,0;1,1",
            "--closed",
            "--bulges",
            "0,0.5,0",
        ],
        vec![
            "text", "--at", "5,5", "--height", "1", "--value", "X", "--align", "center",
        ],
    ] {
        let mut a = kind.clone();
        a.extend(["--unit", "mm", "--layer", "FROZEN"]);
        landed("honest_edits_land", &b.bytes, "entity add", &a);
        landed_n += 1;
    }
    for l in ["room", "title"] {
        landed(
            "honest_edits_land",
            &b.bytes,
            "text set",
            &[&b.addr(l), "--value", "NEW"],
        );
        landed_n += 1;
    }
    for block in ["TAGGED", "PLAINMARK", "DEV"] {
        landed(
            "honest_edits_land",
            &b.bytes,
            "block insert",
            &[
                "--block", block, "--at", "1,1", "--unit", "mm", "--layer", "FROZEN",
            ],
        );
        landed_n += 1;
    }
    println!("honest_edits_land: {} edits land", landed_n + 1);
}

/// refusals_named: each refusal comes with its own id, and nothing is planned; the verbs that
/// turn, scale, insert and set a text too (a block that is missing, an external reference, a layout, anonymous, annotative or
/// dynamic; an attribute a reference can't set; a text set on what isn't a single-line text).
/// Plants: `ProxyWritten`, `AnnotativeIgnored`.
#[test]
pub fn refusals_named() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let mv = |l: &str| {
        vec![
            b.addr(l),
            "--by".into(),
            "1,0".into(),
            "--unit".into(),
            "mm".into(),
        ]
    };
    let field = {
        let bytes = splice_attrib(
            &b.bytes,
            b.handle("dev"),
            "DEVICE",
            &[(102, "{ACAD_XDICTIONARY"), (360, "E00"), (102, "}")],
        );
        let attrib = {
            let l = Limits::default();
            let s = structure::walk(token::tokenize(&bytes, &l).unwrap(), &l).unwrap();
            let d = convert::doc(&s);
            let i = d
                .entities
                .iter()
                .position(|e| e.handle == Some(b.handle("dev")))
                .unwrap();
            d.entities[i].extra.as_ref().unwrap().attribs[0]
                .handle
                .unwrap()
        };
        splice_object(
            &bytes,
            &[
                (0, "DICTIONARY"),
                (5, "E00"),
                (330, &format!("{attrib:X}")),
                (100, "AcDbDictionary"),
                (3, "ACAD_FIELD"),
                (360, "E01"),
            ],
        )
    };
    let multiline = splice_attrib(
        &b.bytes,
        b.handle("dev"),
        "LABEL",
        &[(101, "Embedded Object")],
    );
    let r12 = testgen::sampler(Ver::R12).build();
    let no_handle = testgen::Spec::new(Ver::R2018)
        .model(Ent::line([0.0, 0.0], [1.0, 0.0]).label("a"))
        .model(Ent::line([0.0, 1.0], [1.0, 1.0]).no_handle())
        .build();
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<String>>();
    let ins = |block: &str, more: &[&str]| {
        let mut v = s(&["--block", block, "--at", "0,0", "--unit", "mm"]);
        v.extend(s(more));
        v
    };
    let cases: Vec<(&str, &[u8], &str, Vec<String>)> = vec![
        ("DXF-E043", &b.bytes, "entity move", mv("proxy")),
        ("DXF-E045", &b.bytes, "entity move", mv("locked")),
        ("DXF-E042", &b.bytes, "entity move", mv("xrefline")),
        ("DXF-E044", &b.bytes, "entity move", mv("in-dyn")),
        ("DXF-E044", &b.bytes, "entity move", mv("in-dyn-def")),
        ("DXF-E046", &b.bytes, "entity move", mv("in-block")),
        (
            "DXF-E047",
            &b.bytes,
            "attrib set",
            s(&[&b.addr("dev"), "--tag", "NOPE", "--value", "x"]),
        ),
        (
            "DXF-E011",
            &b.bytes,
            "attrib set",
            s(&[&b.addr("twice"), "--tag", "DEVICE", "--value", "x"]),
        ),
        (
            "DXF-E048",
            &field,
            "attrib set",
            s(&[&b.addr("dev"), "--tag", "DEVICE", "--value", "x"]),
        ),
        (
            "DXF-E049",
            &multiline,
            "attrib set",
            s(&[&b.addr("dev"), "--tag", "DEVICE", "--value", "x"]),
        ),
        (
            "DXF-E040",
            &r12.bytes,
            "entity move",
            s(&["@h:1", "--by", "1,0", "--unit", "mm"]),
        ),
        (
            "DXF-E041",
            &no_handle.bytes,
            "entity move",
            s(&[&no_handle.addr("a"), "--by", "1,0", "--unit", "mm"]),
        ),
        (
            "DXF-E022",
            &b.bytes,
            "entity move",
            s(&[&b.addr("wall"), "--by", "1,0", "--unit", "in"]),
        ),
        (
            "DXF-E020",
            &b.bytes,
            "entity set",
            s(&[&b.addr("wall"), "--layer", "NO-SUCH"]),
        ),
        (
            "DXF-E045",
            &b.bytes,
            "entity set",
            s(&[&b.addr("wall"), "--layer", "LOCKED"]),
        ),
        (
            "DXF-E053",
            &b.bytes,
            "entity set",
            s(&[&b.addr("wall"), "--ltype", "NO-SUCH"]),
        ),
        (
            "DXF-E054",
            &b.bytes,
            "attrib set",
            s(&[&b.addr("dev"), "--tag", "DEVICE", "--value", "two\nlines"]),
        ),
        ("DXF-E010", &b.bytes, "entity delete", s(&["@h:FFFFF"])),
        (
            "DXF-E043",
            &b.bytes,
            "entity rotate",
            s(&[
                &b.addr("proxy"),
                "--by",
                "90",
                "--about",
                "0,0",
                "--unit",
                "mm",
            ]),
        ),
        (
            "DXF-E045",
            &b.bytes,
            "entity scale",
            s(&[
                &b.addr("locked"),
                "--factor",
                "2",
                "--about",
                "0,0",
                "--unit",
                "mm",
            ]),
        ),
        (
            "DXF-E022",
            &b.bytes,
            "entity rotate",
            s(&[
                &b.addr("wall"),
                "--by",
                "90",
                "--about",
                "0,0",
                "--unit",
                "in",
            ]),
        ),
        (
            "DXF-E021",
            &b.bytes,
            "text set",
            s(&[&b.addr("note"), "--value", "x"]),
        ),
        (
            "DXF-E021",
            &b.bytes,
            "text set",
            s(&[&b.addr("dev"), "--value", "x"]),
        ),
        (
            "DXF-E054",
            &b.bytes,
            "text set",
            s(&[&b.addr("room"), "--value", "a\nb\u{7}"]),
        ),
        ("DXF-E051", &b.bytes, "block insert", ins("NO-SUCH", &[])),
        ("DXF-E051", &b.bytes, "block insert", ins("BACK", &[])),
        (
            "DXF-E051",
            &b.bytes,
            "block insert",
            ins("*Model_Space", &[]),
        ),
        ("DXF-E051", &b.bytes, "block insert", ins("*U7", &[])),
        ("DXF-E051", &b.bytes, "block insert", ins("ANNOBLK", &[])),
        ("DXF-E043", &b.bytes, "block insert", ins("PROXYBLK", &[])),
        ("DXF-E044", &b.bytes, "block insert", ins("DYN", &[])),
        (
            "DXF-E047",
            &b.bytes,
            "block insert",
            ins("TAGGED", &["--attr", "NOPE=1"]),
        ),
        (
            "DXF-E047",
            &b.bytes,
            "block insert",
            ins("TAGGED", &["--attr", "MAKER=X"]),
        ),
        (
            "DXF-E045",
            &b.bytes,
            "block insert",
            ins("TAGGED", &["--layer", "LOCKED"]),
        ),
    ];
    let mut n = 0;
    for (want, bytes, verb, args) in &cases {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            refused_id(bytes, verb, &refs),
            *want,
            "refusals_named: {verb} {args:?}"
        );
        n += 1;
    }
    println!("refusals_named: {n} refusals, each by its own id");
}

/// The model of `bytes`, read as the index reads it.
fn doc_of(bytes: &[u8]) -> k2_mpc_dxf::Doc {
    let l = Limits::default();
    convert::doc(&structure::walk(token::tokenize(bytes, &l).unwrap(), &l).unwrap())
}

fn num(v: &str) -> f64 {
    v.trim().parse().unwrap_or(f64::NAN)
}

/// rotate_turns_points_and_angles: a quarter turn about a point is exact on the file's tokens
/// (a line's ends, an ellipse's major axis turned as a vector); the angles of an arc, a text and
/// a block reference with its attributes gain the turn (written in where the entity had none,
/// put in [0, 360)); a turn of 30 degrees reads back within a unit in the last place. Plants:
/// `QuarterTurnInFloat`, `AngleNotTurned`.
#[test]
pub fn rotate_turns_points_and_angles() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let targets = ["wall", "ellipse", "arc", "room", "dev"];
    let mut args: Vec<String> = targets.iter().map(|l| b.addr(l)).collect();
    args.extend(
        ["--by", "90", "--about", "1,1", "--unit", "mm"]
            .iter()
            .map(|x| (*x).to_owned()),
    );
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let p = landed(
        "rotate_turns_points_and_angles",
        &b.bytes,
        "entity rotate",
        &refs,
    );
    let wall = entity_pairs(&p.output, b.handle("wall"));
    assert_eq!(
        (
            values(&wall, 10),
            values(&wall, 20),
            values(&wall, 11),
            values(&wall, 21)
        ),
        (
            vec!["2.0".to_owned()],
            vec!["1.1".to_owned()],
            vec!["2.0".to_owned()],
            vec!["10.0".to_owned()]
        ),
        "rotate_turns_points_and_angles: a quarter turn about 1,1 is exact"
    );
    let el = entity_pairs(&p.output, b.handle("ellipse"));
    assert_eq!(
        (values(&el, 11), values(&el, 21)),
        (vec!["-1.0".to_owned()], vec!["2.0".to_owned()]),
        "rotate_turns_points_and_angles: the major axis turns as a vector"
    );
    let arc = entity_pairs(&p.output, b.handle("arc"));
    let ang =
        |v: &[(i32, String)], c: i32| values(v, c).iter().map(|x| num(x)).collect::<Vec<f64>>();
    assert_eq!(
        (ang(&arc, 50), ang(&arc, 51)),
        (vec![90.0], vec![270.0]),
        "rotate_turns_points_and_angles: an arc's ends turn"
    );
    let room = entity_pairs(&p.output, b.handle("room"));
    assert_eq!(
        ang(&room, 50),
        vec![90.0],
        "rotate_turns_points_and_angles: a text gains its rotation"
    );
    let dev = entity_pairs(&p.output, b.handle("dev"));
    assert_eq!(
        ang(&dev, 50),
        vec![90.0, 90.0, 90.0],
        "rotate_turns_points_and_angles: a block reference and its two attributes turn"
    );
    // any other angle, in f64
    let p = landed(
        "rotate_turns_points_and_angles",
        &b.bytes,
        "entity rotate",
        &[
            &b.addr("lamp"),
            "--by",
            "30",
            "--about",
            "0,0",
            "--unit",
            "mm",
        ],
    );
    let lamp = entity_pairs(&p.output, b.handle("lamp"));
    let (sin, cos) = 30f64.to_radians().sin_cos();
    let want = [20.0 * cos - 20.0 * sin, 20.0 * sin + 20.0 * cos];
    let got = [num(&values(&lamp, 10)[0]), num(&values(&lamp, 20)[0])];
    assert!(
        (got[0] - want[0]).abs() < 1e-12 && (got[1] - want[1]).abs() < 1e-12,
        "rotate_turns_points_and_angles: 30 degrees: {got:?}, expected {want:?}"
    );
    println!("rotate_turns_points_and_angles: 5 kinds a quarter turn, 1 by 30");
}

/// scale_sizes_and_points: a scale about a point moves points from it by the factor and scales
/// sizes (a circle's radius, a text's height, a block reference's scales written in, its
/// attributes' heights), exactly in decimal (3.0 by 0.1 is 0.3). Plants: `ScaleAboutOrigin`,
/// `SizesLeft`, `ScaleInFloat`.
#[test]
pub fn scale_sizes_and_points() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let p = landed(
        "scale_sizes_and_points",
        &b.bytes,
        "entity scale",
        &[
            &b.addr("lamp"),
            &b.addr("room"),
            &b.addr("dev"),
            "--factor",
            "2",
            "--about",
            "10,10",
            "--unit",
            "mm",
        ],
    );
    let lamp = entity_pairs(&p.output, b.handle("lamp"));
    assert_eq!(
        (values(&lamp, 10), values(&lamp, 20), values(&lamp, 40)),
        (
            vec!["30.0".to_owned()],
            vec!["30.0".to_owned()],
            vec!["4.0".to_owned()]
        ),
        "scale_sizes_and_points: a circle's centre from 10,10 and its radius"
    );
    let room = entity_pairs(&p.output, b.handle("room"));
    assert_eq!(
        (values(&room, 10), values(&room, 40)),
        (vec!["-8.0".to_owned()], vec!["5.0".to_owned()]),
        "scale_sizes_and_points: a text's point and height"
    );
    let dev = entity_pairs(&p.output, b.handle("dev"));
    assert_eq!(
        (values(&dev, 41), values(&dev, 42), values(&dev, 43)),
        (
            vec!["2".to_owned()],
            vec!["2".to_owned()],
            vec!["2".to_owned()]
        ),
        "scale_sizes_and_points: a block reference's scales written in"
    );
    let p = landed(
        "scale_sizes_and_points",
        &b.bytes,
        "entity scale",
        &[
            &b.addr("poly"),
            "--factor",
            "0.1",
            "--about",
            "0,0",
            "--unit",
            "mm",
        ],
    );
    let poly = entity_pairs(&p.output, b.handle("poly"));
    assert_eq!(
        values(&poly, 20),
        vec!["0.0", "0.0", "0.0", "0.3"],
        "scale_sizes_and_points: 3.0 by 0.1 is 0.3, in decimal"
    );
    println!("scale_sizes_and_points: 4 kinds by 2 about 10,10, a polyline by 0.1");
}

/// block_insert_places_attribs: a reference of a block with attribute definitions is written
/// with an attribute per settable definition (a constant one has none), each placed as the
/// reference places its block (`Doc::place`), its height scaled and its rotation turned, its
/// value the call's or the default, on the reference's layer when the definition is on `0`; the
/// block record's list of references gains it; `$HANDSEED` is past every new handle. Plants:
/// `AttribsUnplaced`, `ConstantAttdefWritten`, `BlkrefsLeft`.
#[test]
pub fn block_insert_places_attribs() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let p = landed(
        "block_insert_places_attribs",
        &b.bytes,
        "block insert",
        &[
            "--block",
            "TAGGED",
            "--at",
            "100,50",
            "--rotate",
            "90",
            "--scale",
            "2",
            "--attr",
            "DEVICE=CAM-50",
            "--unit",
            "mm",
            "--layer",
            "E-DEV",
        ],
    );
    assert_eq!(
        p.created.len(),
        1,
        "block_insert_places_attribs: one reference"
    );
    let h = k2_mpc_dxf::handle::from_addr(&p.created[0]).unwrap();
    let d = doc_of(&p.output);
    let i = d.entities.iter().position(|e| e.handle == Some(h)).unwrap();
    let e = &d.entities[i];
    let atts = &e.extra.as_ref().expect("its attributes").attribs;
    let got: Vec<(&str, &str)> = atts.iter().map(|a| (&*a.tag, &*a.value)).collect();
    assert_eq!(
        got,
        vec![("DEVICE", "CAM-50"), ("LABEL", "L")],
        "block_insert_places_attribs: one attribute per settable definition"
    );
    // each where Doc::place puts its definition's point
    for (a, def) in atts.iter().zip([[1.0, 1.5], [1.0, 0.5]]) {
        let want = d.place(i, def).unwrap();
        let at = a.at.unwrap();
        assert!(
            (at[0] - want[0]).abs() < 1e-9 && (at[1] - want[1]).abs() < 1e-9,
            "block_insert_places_attribs: {} at {at:?}, placed {want:?}",
            a.tag
        );
    }
    let pairs = entity_pairs(&p.output, h);
    assert_eq!(
        (values(&pairs, 40), values(&pairs, 50), values(&pairs, 8)),
        (
            vec!["0.50".to_owned(), "0.50".to_owned()],
            vec!["90".to_owned(), "90".to_owned(), "90".to_owned()],
            vec![
                "E-DEV".to_owned(),
                "E-DEV".to_owned(),
                "ANNO".to_owned(),
                "E-DEV".to_owned()
            ]
        ),
        "block_insert_places_attribs: heights, rotations, layers (the reference, its \
         attributes, its end)"
    );
    let all = pairs_of(&p.output);
    let key = format!("{h:X}");
    let listed = all
        .windows(2)
        .any(|w| w[0].0 == 331 && w[0].1 == key && w[1].0 == 102 && w[1].1 == "}");
    assert!(
        listed,
        "block_insert_places_attribs: the block record lists the new reference"
    );
    let seed = k2_mpc_dxf::verbs::run(&p.output, "header get", &["$HANDSEED"]).unwrap();
    assert!(
        seed.stdout.contains(&format!("\t5\t{:X}", h + 4)),
        "block_insert_places_attribs: $HANDSEED past the reference, 2 attributes and its end: {}",
        seed.stdout
    );
    println!("block_insert_places_attribs: @h:{key}, 2 attributes placed");
}

fn pairs_of(bytes: &[u8]) -> Vec<(i32, String)> {
    let t = token::tokenize(bytes, &Limits::default()).unwrap();
    (0..t.pairs.len())
        .map(|i| (t.pairs[i].code, t.text(i).trim().to_owned()))
        .collect()
}

/// text_set_written: a single-line text's value, written as the file writes text (a 2000 file's
/// code page, `\U+` for what it lacks; UTF-8 from 2007), reads back as given.
#[test]
pub fn text_set_written() {
    let value = "Caf\u{e9} \u{2116}5";
    for (v, want) in [
        (Ver::R2000, b"Caf\xe9 \\U+21165".to_vec()),
        (Ver::R2018, value.as_bytes().to_vec()),
    ] {
        let b = testgen::edit_sampler(v).build();
        let p = landed(
            "text_set_written",
            &b.bytes,
            "text set",
            &[&b.addr("room"), "--value", value],
        );
        let got: Vec<Vec<u8>> = p.spans.iter().map(|s| s.with.clone()).collect();
        assert_eq!(got, vec![want], "text_set_written: {}", v.acadver());
        let d = doc_of(&p.output);
        let room = d
            .entities
            .iter()
            .find(|e| e.handle == Some(b.handle("room")))
            .unwrap();
        assert_eq!(
            room.text.as_deref(),
            Some(value),
            "text_set_written: {} reads back",
            v.acadver()
        );
    }
    println!("text_set_written: 2 versions");
}

/// rotate_noise_snapped: a turn by an angle that isn't a quarter is computed in `f64`; a
/// coordinate that comes out zero but for the sine's and cosine's last bits (a text at 1,1 turned
/// 45 degrees about the origin: x is 1.1e-16) is written 0.0, so the bytes are the same on every
/// platform. Plant: `RotateNoiseKept`.
#[test]
pub fn rotate_noise_snapped() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let p = landed(
        "rotate_noise_snapped",
        &b.bytes,
        "entity rotate",
        &[
            &b.addr("room"),
            "--by",
            "45",
            "--about",
            "0,0",
            "--unit",
            "mm",
        ],
    );
    let room = entity_pairs(&p.output, b.handle("room"));
    assert_eq!(
        values(&room, 10),
        vec!["0.0".to_owned()],
        "rotate_noise_snapped: x at the noise of 45 degrees is 0.0"
    );
    let y = num(&values(&room, 20)[0]);
    assert!(
        (y - std::f64::consts::SQRT_2).abs() < 1e-12,
        "rotate_noise_snapped: y is sqrt 2: {y}"
    );
    println!("rotate_noise_snapped: x 0.0, y {y}");
}

/// `bytes` with `$CLAYER` set to `layer` (our own splice of its value line).
fn with_clayer(bytes: &[u8], layer: &str) -> Vec<u8> {
    let t = token::tokenize(bytes, &Limits::default()).unwrap();
    let i = (0..t.pairs.len())
        .find(|&i| t.pairs[i].code == 9 && t.text(i).trim() == "$CLAYER")
        .expect("the drawing has $CLAYER");
    let r = t.pairs[i + 1].value_range();
    [&bytes[..r.start], layer.as_bytes(), &bytes[r.end..]].concat()
}

fn draw_header(bytes: &[u8]) -> k2_mpc_base::errors::Value {
    k2_mpc_dxf::verbs::run(bytes, "draw header", &["--version", "1"])
        .expect("draw header reads")
        .data
}

/// header_blocks_match_block_insert: the draw header's blocks say what `block insert` does with
/// each: for every named block of the drawing's own, `dynamic: true` exactly when block insert
/// refuses it as dynamic (`DXF-E044`) and `annotative: true` exactly when it refuses it as
/// annotative (`DXF-E051`, annotative); and a block's `attdefs` are its attribute definitions in
/// the file's order (tag, default, constant, prompt). Plant: `HeaderDynamicMissed`.
#[test]
pub fn header_blocks_match_block_insert() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let h = draw_header(&b.bytes);
    let blocks = match h.get("blocks") {
        Some(k2_mpc_base::errors::Value::List(l)) => l.clone(),
        _ => panic!("header_blocks_match_block_insert: no blocks"),
    };
    let mut checked = 0;
    for blk in &blocks {
        let name = blk.get("name").and_then(|v| v.as_str()).unwrap().to_owned();
        let flag = |k: &str| match blk.get(k) {
            Some(k2_mpc_base::errors::Value::Bool(x)) => *x,
            other => panic!("header_blocks_match_block_insert: {name}: {k} is {other:?}"),
        };
        let (dynamic, annotative) = (flag("dynamic"), flag("annotative"));
        if flag("anon") || flag("xref") {
            continue;
        }
        let r = plan(
            &b.bytes,
            "block insert",
            &["--block", &name, "--at", "0,0", "--unit", "mm"],
        );
        let (code, why) = match &r {
            Ok(_) => (String::new(), String::new()),
            Err(e) => (k2_mpc_dxf::pack::pack_id(e.code.id), e.why.clone()),
        };
        assert_eq!(
            (dynamic, annotative),
            (
                code == "DXF-E044",
                code == "DXF-E051" && why.contains("annotative")
            ),
            "header_blocks_match_block_insert: {name}: block insert gives {code} {why}"
        );
        checked += 1;
    }
    let tagged = blocks
        .iter()
        .find(|x| x.get("name").and_then(|v| v.as_str()) == Some("TAGGED"))
        .unwrap();
    let defs: Vec<(String, String, bool, String)> = match tagged.get("attdefs") {
        Some(k2_mpc_base::errors::Value::List(l)) => l
            .iter()
            .map(|d| {
                let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
                let c = matches!(
                    d.get("constant"),
                    Some(k2_mpc_base::errors::Value::Bool(true))
                );
                (s("tag"), s("default"), c, s("prompt"))
            })
            .collect(),
        other => panic!("header_blocks_match_block_insert: TAGGED's attdefs: {other:?}"),
    };
    let want = |t: &str, d: &str, c: bool| (t.to_owned(), d.to_owned(), c, t.to_owned());
    assert_eq!(
        defs,
        vec![
            want("DEVICE", "CAM-00", false),
            want("LABEL", "L", false),
            want("MAKER", "ACME", true)
        ],
        "header_blocks_match_block_insert: TAGGED's attribute definitions"
    );
    assert!(
        checked >= 5,
        "header_blocks_match_block_insert: {checked} blocks"
    );
    println!(
        "header_blocks_match_block_insert: {checked} blocks, the flags as block insert judges"
    );
}

/// entity_add_lands_on_current_layer: the draw header's `current_layer` is the drawing's
/// `$CLAYER` by the layers list's address, and an add or an insert that names no layer lands on
/// it. Plant: `CurrentLayerAsZero`.
#[test]
pub fn entity_add_lands_on_current_layer() {
    let b = testgen::edit_sampler(Ver::R2018).build();
    let bytes = with_clayer(&b.bytes, "E-DEV");
    let h = draw_header(&bytes);
    let layers = match h.get("layers") {
        Some(k2_mpc_base::errors::Value::List(l)) => l.clone(),
        _ => panic!("no layers"),
    };
    let edev = layers
        .iter()
        .find(|l| l.get("name").and_then(|v| v.as_str()) == Some("E-DEV"))
        .and_then(|l| l.get("handle"))
        .and_then(|v| v.as_str())
        .unwrap()
        .to_owned();
    assert_eq!(
        h.get("current_layer").and_then(|v| v.as_str()),
        Some(edev.as_str()),
        "entity_add_lands_on_current_layer: the header's current layer"
    );
    let add = landed(
        "entity_add_lands_on_current_layer",
        &bytes,
        "entity add",
        &["line", "--from", "0,0", "--to", "1,1", "--unit", "mm"],
    );
    let ins = landed(
        "entity_add_lands_on_current_layer",
        &bytes,
        "block insert",
        &["--block", "TAGGED", "--at", "0,0", "--unit", "mm"],
    );
    for (what, p) in [("entity add", &add), ("block insert", &ins)] {
        let h = k2_mpc_dxf::handle::from_addr(&p.created[0]).unwrap();
        let d = doc_of(&p.output);
        let e = d.entities.iter().find(|e| e.handle == Some(h)).unwrap();
        assert_eq!(
            &*e.layer, "E-DEV",
            "entity_add_lands_on_current_layer: {what} with no --layer"
        );
    }
    println!("entity_add_lands_on_current_layer: {edev}, both land on it");
}
