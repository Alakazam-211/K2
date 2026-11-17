//! The read verbs: each a pure function of one version's bytes, with typed data whose shape
//! `data_spec` names. Counts are computed on each read (never stored on a node); every read
//! with coordinates states the drawing's units.

mod support;

use k2_mpc_base::errors::Value;
use k2_mpc_dxf::error::{BINARY, USAGE};
use k2_mpc_dxf::testgen::{self, Ent, Spec, Ver};
use support::*;

fn addrs(rows: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = rows
        .iter()
        .map(|r| string(r, &["address"]).to_owned())
        .collect();
    v.sort();
    v
}

#[test]
pub fn drawing_info_shape() {
    let b = testgen::sampler(Ver::R2000).build();
    let o = run(&b.bytes, "drawing info", &[]);
    assert_eq!(o.spec, ("cad.drawing", 1));
    let d = &o.data;
    assert_eq!(string(d, &["acadver"]), "AC1015");
    assert_eq!(string(d, &["eol"]), "lf");
    assert_eq!(number(d, &["counts", "entities"]), 12.0);
    assert_eq!(number(d, &["counts", "by_space", "model"]), 9.0);
    assert_eq!(number(d, &["counts", "by_space", "paper"]), 1.0);
    assert_eq!(number(d, &["counts", "by_space", "blocks"]), 2.0);
    assert_eq!(number(d, &["counts", "by_kind", "LINE"]), 2.0);
    assert_eq!(number(d, &["counts", "layers"]), 4.0);
    assert_eq!(number(d, &["counts", "blocks"]), 1.0);
    assert_eq!(number(d, &["handles", "duplicates"]), 0.0);
    assert_eq!(number(d, &["handles", "missing"]), 0.0);
    assert_eq!(string(d, &["handles", "mode"]), "on");
    assert_eq!(string(d, &["writer", "policy"]), "unproven");
    assert!(o.stdout.contains("AC1015"), "{}", o.stdout);
}

#[test]
pub fn layer_counts() {
    let b = testgen::nested_blocks(Ver::R2010).build();
    let o = run(&b.bytes, "layer list", &[]);
    assert_eq!(o.spec, ("cad.layer-list", 1));
    let rows = list(&o.data, &["layers"]);
    assert_eq!(rows.len(), 4, "layers 0, L1, L2, UNUSED");
    let l1 = row(rows, "name", "L1");
    assert_eq!(number(l1, &["ents"]), 6.0, "L1 in every space and block");
    assert_eq!(number(l1, &["ents_model"]), 2.0, "L1 in model space");
    let l2 = row(rows, "name", "L2");
    assert_eq!(number(l2, &["ents"]), 4.0);
    assert_eq!(number(l2, &["ents_model"]), 2.0);
    assert!(string(l1, &["address"]).starts_with("@h:"));
    let unused = run(&b.bytes, "layer list", &["--unused"]);
    let names: Vec<&str> = list(&unused.data, &["layers"])
        .iter()
        .map(|r| string(r, &["name"]))
        .collect();
    assert_eq!(names, ["0", "UNUSED"]);
}

#[test]
pub fn block_counts_nested() {
    let b = testgen::nested_blocks(Ver::R2010).build();
    let o = run(&b.bytes, "block list", &[]);
    assert_eq!(o.spec, ("cad.block-list", 1));
    let rows = list(&o.data, &["blocks"]);
    let get = |n: &str, p: &[&str]| number(row(rows, "name", n), p);
    assert_eq!(get("A", &["inserts", "direct", "model"]), 2.0);
    assert_eq!(get("A", &["inserts", "nested_model"]), 2.0);
    assert_eq!(get("B", &["inserts", "direct", "paper"]), 1.0);
    assert_eq!(get("B", &["inserts", "in_blocks"]), 2.0);
    assert_eq!(get("B", &["inserts", "nested_model"]), 4.0);
    assert_eq!(get("C", &["inserts", "direct", "model"]), 1.0);
    assert_eq!(get("C", &["inserts", "in_blocks"]), 1.0);
    assert_eq!(get("C", &["inserts", "nested_model"]), 5.0);
    assert_eq!(get("D", &["inserts", "nested_model"]), 0.0);
    assert_eq!(get("A", &["ents"]), 2.0);
    let unused = run(&b.bytes, "block list", &["--unused"]);
    let names: Vec<&str> = list(&unused.data, &["blocks"])
        .iter()
        .map(|r| string(r, &["name"]))
        .collect();
    assert_eq!(names, ["D"]);
}

#[test]
pub fn units_on_geometry_reads() {
    let b = testgen::sampler(Ver::R2013).build();
    for (verb, args) in [
        ("drawing info", vec![]),
        ("layer list", vec![]),
        ("block list", vec![]),
        ("entity find", vec!["--layer", "A-WALL"]),
        ("text find", vec!["--contains", "room"]),
    ] {
        let o = run(&b.bytes, verb, &args);
        assert_eq!(string(&o.data, &["units", "word"]), "mm", "{verb}");
        assert_eq!(number(&o.data, &["units", "insunits"]), 4.0, "{verb}");
    }
}

#[test]
pub fn extents_header_vs_computed() {
    let mut s = testgen::sampler(Ver::R2000);
    s.opts.extents = Some(([0.0, 0.0], [1.0, 1.0]));
    let o = run(&s.build().bytes, "drawing info", &[]);
    assert_eq!(
        at(&o.data, &["extents", "header_stale"]),
        &Value::Bool(true)
    );
    let max = list(&o.data, &["extents", "computed", "max"]);
    assert!(
        max[0].as_f64().unwrap() >= 35.0,
        "computed extents trust the header"
    );
    s.opts.extents = Some(([1e20, 1e20], [-1e20, -1e20]));
    let o = run(&s.build().bytes, "drawing info", &[]);
    assert_eq!(
        at(&o.data, &["extents", "header_sentinel"]),
        &Value::Bool(true)
    );
    s.opts.extents = None;
    let o = run(&s.build().bytes, "drawing info", &[]);
    assert_eq!(at(&o.data, &["extents", "header"]), &Value::Null);
}

#[test]
pub fn entity_find_window_and_crossing() {
    let b = testgen::sampler(Ver::R2000).build();
    let o = run(&b.bytes, "entity find", &["--window", "-1,-1,11,1"]);
    assert_eq!(o.spec, ("cad.entity-find", 1));
    let got = addrs(list(&o.data, &["rows"]));
    let mut want = vec![b.addr("wall"), b.addr("door1")];
    want.sort();
    assert_eq!(got, want, "window: wholly inside");
    let o = run(&b.bytes, "entity find", &["--crossing", "9,-1,11,1"]);
    let got = addrs(list(&o.data, &["rows"]));
    for l in ["wall", "door1", "lw"] {
        assert!(got.contains(&b.addr(l)), "crossing misses {l}");
    }
    assert!(
        !got.contains(&b.addr("poly")),
        "crossing takes the polyline"
    );
    let o = run(
        &b.bytes,
        "entity find",
        &["--layer", "a-wall", "--kind", "LINE"],
    );
    assert_eq!(
        addrs(list(&o.data, &["rows"])),
        [b.addr("wall")],
        "layer and kind"
    );
    let o = run(&b.bytes, "entity find", &["--kind", "line", "--in", "all"]);
    let got = addrs(list(&o.data, &["rows"]));
    assert!(
        got.contains(&b.addr("door-leaf")),
        "--in all misses block entities"
    );
    // paging: one row a page, every row once
    let all = run(&b.bytes, "entity find", &["--layer", "A-WALL"]);
    let total = list(&all.data, &["rows"]).len();
    let mut seen = Vec::new();
    let mut args: Vec<String> = vec![
        "--layer".into(),
        "A-WALL".into(),
        "--limit".into(),
        "1".into(),
    ];
    loop {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = run(&b.bytes, "entity find", &a);
        seen.extend(addrs(list(&o.data, &["rows"])));
        match o.next {
            Some(n) => args = n,
            None => break,
        }
        assert!(seen.len() <= total, "paging repeats rows");
    }
    assert_eq!(seen.len(), total, "paging lost rows");
}

#[test]
pub fn text_find() {
    let b = testgen::sampler(Ver::R2007).build();
    let o = run(&b.bytes, "text find", &["--contains", "room"]);
    assert_eq!(o.spec, ("cad.text-find", 1));
    assert_eq!(addrs(list(&o.data, &["rows"])), [b.addr("room")]);
    let o = run(&b.bytes, "text find", &["--contains", "D-01"]);
    assert_eq!(
        addrs(list(&o.data, &["rows"])),
        [b.addr("door1")],
        "attribute values"
    );
    let o = run(&b.bytes, "text find", &["--contains", "room", "--case"]);
    assert!(list(&o.data, &["rows"]).is_empty(), "--case folds");
    let o = run(&b.bytes, "text find", &["--contains", "two lines"]);
    assert_eq!(list(&o.data, &["rows"]).len(), 1, "multiline text");
    let o = run(&b.bytes, "text find", &["--contains", "title"]);
    assert_eq!(
        addrs(list(&o.data, &["rows"])),
        [b.addr("title")],
        "paper space"
    );
}

#[test]
pub fn header_get() {
    let b = testgen::sampler(Ver::R2000).build();
    let o = run(
        &b.bytes,
        "header get",
        &["$ACADVER", "$NOSUCH", "$insunits"],
    );
    assert_eq!(o.spec, ("dxf.header", 1));
    let vars = list(&o.data, &["vars"]);
    assert_eq!(vars.len(), 2);
    let v = row(vars, "name", "$ACADVER");
    assert_eq!(string(&list(v, &["pairs"])[0], &["value"]), "AC1015");
    assert_eq!(number(&list(v, &["pairs"])[0], &["code"]), 1.0);
    let missing: Vec<&str> = list(&o.data, &["missing"])
        .iter()
        .map(|m| m.as_str().unwrap_or(""))
        .collect();
    assert_eq!(missing, ["$NOSUCH"]);
}

/// The finding `code` of `f`, with severity `sev`.
///
/// # Panics
/// When there is none, or its severity differs.
fn finding<'a>(f: &'a [Value], data: &Value, code: &str, sev: &str) -> &'a Value {
    let x = f
        .iter()
        .find(|x| string(x, &["code"]) == code)
        .unwrap_or_else(|| panic!("no finding {code}: {}", data.to_json()));
    assert_eq!(string(x, &["severity"]), sev, "{code}'s severity");
    x
}

fn planted() -> Spec {
    let mut s = Spec::new(Ver::R2000)
        .layer("A", 1)
        .block(
            "BLK",
            [0.0, 0.0],
            vec![Ent::insert("NOPE", [0.0, 0.0]).label("nope")],
        )
        .model(Ent::insert("BLK", [0.0, 0.0]))
        .model(
            Ent::line([0.0, 0.0], [50.0, 50.0])
                .on("GHOST")
                .label("ghost"),
        )
        .model(Ent::line([0.0, 0.0], [1.0, 0.0]).on("A").handle(0x700))
        .model(Ent::line([0.0, 1.0], [1.0, 1.0]).on("A").handle(0x700))
        .model(
            Ent::raw(
                "LINE",
                &[
                    (10, "0.0"),
                    (20, "0.0"),
                    (30, "0.0"),
                    (11, "1.0"),
                    (21, "0.0"),
                    (31, "0.0"),
                    (1001, "NOAPP"),
                    (1000, "x"),
                ],
            )
            .on("A")
            .label("noapp"),
        );
    s.opts.handseed = Some(0x11);
    s.opts.extents = Some(([0.0, 0.0], [1.0, 1.0]));
    s.opts.eof = false;
    s
}

#[test]
pub fn check_findings() {
    let b = planted().build();
    let o = run(&b.bytes, "check", &[]);
    assert_eq!(o.spec, ("dxf.check", 1));
    let f = list(&o.data, &["findings"]);
    let has = |code: &str, sev: &str| finding(f, &o.data, code, sev).clone();
    assert_eq!(
        string(
            &has("dxf.check.insert_undefined_block", "error"),
            &["address"]
        ),
        b.addr("nope"),
        "an undefined block inserted inside a block"
    );
    assert_eq!(
        string(&has("dxf.check.layer_undefined", "warning"), &["address"]),
        b.addr("ghost")
    );
    has("dxf.check.duplicate_handle", "error");
    assert_eq!(
        string(&has("dxf.check.appid_missing", "error"), &["address"]),
        b.addr("noapp")
    );
    has("dxf.check.handseed_low", "warning");
    has("dxf.check.extents_stale", "info");
    has("dxf.check.eof_missing", "error");
    // a clean drawing has no error and no warning
    let clean = run(&testgen::sampler(Ver::R2018).build().bytes, "check", &[]);
    for x in list(&clean.data, &["findings"]) {
        assert_eq!(string(x, &["severity"]), "info", "{}", x.to_json());
    }
}

#[test]
pub fn check_writeback() {
    for (name, bytes) in testgen::byte_variants().into_iter().take(12) {
        let o = run(&bytes, "check", &["--writeback"]);
        assert_eq!(
            at(&o.data, &["writeback", "identical"]),
            &Value::Bool(true),
            "{name}"
        );
        assert!(
            number(&o.data, &["writeback", "editable", "yes"]) > 0.0,
            "{name}"
        );
    }
}

#[test]
pub fn verbs_refuse_usage() {
    let b = testgen::sampler(Ver::R2000).build();
    let e = refused(&b.bytes, "entity find", &[]);
    assert_eq!(e.code, USAGE, "no filter finds nothing, never everything");
    assert_eq!(e.code.exit, 2);
    let e = refused(&b.bytes, "layer list", &["--bogus"]);
    assert_eq!(e.code, USAGE);
    assert!(
        e.next.contains("--unused"),
        "the next step lists the flags: {}",
        e.next
    );
    let e = refused(&b.bytes, "nope verb", &[]);
    assert_eq!(e.code, USAGE);
    let e = refused(&b.bytes, "text find", &["--contains"]);
    assert_eq!(e.code, USAGE);
    let e = refused(b"AutoCAD Binary DXF\r\n\x1a\x00", "drawing info", &[]);
    assert_eq!(e.code, BINARY);
}

#[test]
pub fn code_pages_and_escapes() {
    // a text escape is read as its character
    let b = Spec::new(Ver::R2004)
        .model(Ent::text([0.0, 0.0], 1.0, "Caf\\U+00E9 menu").label("cafe"))
        .build();
    let o = run(&b.bytes, "text find", &["--contains", "Café"]);
    assert_eq!(
        addrs(list(&o.data, &["rows"])),
        [b.addr("cafe")],
        "the escape"
    );
    // code page 1252 bytes read as 1252
    let mut s = Spec::new(Ver::R2004).model(Ent::text([0.0, 0.0], 1.0, "ZZEURO").label("euro"));
    s.opts.codepage = "ANSI_1252".into();
    let mut bytes = s.build().bytes;
    let at = bytes
        .windows(6)
        .position(|w| w == b"ZZEURO")
        .expect("the marker");
    bytes.splice(at..at + 6, [0x80u8]).for_each(drop);
    let o = run(&bytes, "text find", &["--contains", "\u{20AC}"]);
    assert_eq!(
        list(&o.data, &["rows"]).len(),
        1,
        "the euro sign of code page 1252"
    );
    // another code page's bytes: flagged, never guessed
    let mut s = Spec::new(Ver::R2004).model(Ent::text([0.0, 0.0], 1.0, "ZZCYR"));
    s.opts.codepage = "ANSI_1251".into();
    let mut bytes = s.build().bytes;
    let at = bytes
        .windows(5)
        .position(|w| w == b"ZZCYR")
        .expect("the marker");
    bytes.splice(at..at + 5, [0xC0u8]).for_each(drop);
    let o = run(&bytes, "check", &[]);
    finding(
        list(&o.data, &["findings"]),
        &o.data,
        "dxf.check.code_page_unread",
        "warning",
    );
    let clean = run(&testgen::sampler(Ver::R2004).build().bytes, "check", &[]);
    assert!(
        !clean.data.to_json().contains("code_page_unread"),
        "a 1252 drawing is flagged"
    );
}
