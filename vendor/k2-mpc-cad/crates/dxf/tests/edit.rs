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
            assert!(
                first.len() >= 3 && first.starts_with(b" "),
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

/// honest_edits_land: every entity the rules allow takes each verb: a move, a colour, a delete
/// (a dynamic block's reference and an entity on a frozen layer included), and both references'
/// attributes. Plants: `DynblockAttribRefused`, `LockedReadAsFrozen`.
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
    println!("honest_edits_land: {} edits land", landed_n + 1);
}

/// refusals_named: each refusal comes with its own id, and nothing is planned. Plant:
/// `ProxyWritten`.
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
