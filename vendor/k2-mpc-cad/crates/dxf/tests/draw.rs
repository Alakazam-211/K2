//! The drawing-model read (`draw header`, `draw page`): known answers on our own generated
//! drawings, kind by kind, each row naming the value it checks. Every resolved value is checked
//! against the rule that gives it (layer, block, own value), never against the read's own
//! output.

mod support;

use k2_mpc_base::errors::Value;
use k2_mpc_cad_model::draw::{PAGE_BYTES, WHYS};
use k2_mpc_dxf::testgen::{self, Built, Ver};
use support::*;

fn sampler() -> Built {
    testgen::draw_sampler(Ver::R2018).build()
}

fn header(bytes: &[u8]) -> Value {
    run(bytes, "draw header", &["--version", "7"]).data
}

/// Every page of `space`, in order, chained by each page's `next`.
fn pages(bytes: &[u8], space: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut args: Vec<String> = vec![
        "--version".into(),
        "7".into(),
        "--space".into(),
        space.into(),
    ];
    loop {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = run(bytes, "draw page", &a);
        out.push(o.data);
        match o.next {
            Some(n) => args = n,
            None => break,
        }
        assert!(out.len() < 10_000, "pages never end");
    }
    out
}

/// Every entity row of `space`, its pages joined.
fn rows(bytes: &[u8], space: &str) -> Vec<Value> {
    pages(bytes, space)
        .iter()
        .flat_map(|p| list(p, &["entities"]).to_vec())
        .collect()
}

fn ent<'v>(rows: &'v [Value], b: &Built, label: &str) -> &'v Value {
    row(rows, "h", &b.addr(label))
}

fn rgb(v: &Value) -> Vec<u64> {
    match v {
        Value::List(l) => l.iter().map(|x| x.as_u64().unwrap_or(999)).collect(),
        other => panic!("not a colour: {}", other.to_json()),
    }
}

/// `got` is `want` as the drawing-model read writes numbers: to 9 significant digits.
fn close(got: f64, want: f64, what: &str) {
    assert!(
        (got - want).abs() <= 1e-8 * (1.0 + want.abs()),
        "{what}: {got}, expected {want}"
    );
}

fn xy(v: &Value) -> [f64; 2] {
    match v {
        Value::List(l) if l.len() == 2 => [l[0].as_f64().expect("x"), l[1].as_f64().expect("y")],
        other => panic!("not a point: {}", other.to_json()),
    }
}

fn block_addr(h: &Value, name: &str) -> String {
    string(row(list(h, &["blocks"]), "name", name), &["handle"]).to_owned()
}

#[test]
pub fn draw_header_tables() {
    let b = sampler();
    let h = header(&b.bytes);
    assert_eq!(string(&h, &["schema"]), "cad.drawing.header/1");
    assert_eq!(number(&h, &["version"]), 7.0);
    // layer: true colour, lineweight and transparency of its own record
    let wall = row(list(&h, &["layers"]), "name", "A-WALL");
    assert_eq!(rgb(at(wall, &["rgb"])), [0x33, 0x66, 0x99], "A-WALL rgb");
    close(number(wall, &["lw_mm"]), 0.35, "A-WALL lineweight");
    close(number(wall, &["alpha"]), 127.0 / 255.0, "A-WALL alpha");
    assert_eq!(string(wall, &["ltype"]), "DASHED");
    let off = row(list(&h, &["layers"]), "name", "OFF");
    assert_eq!(at(off, &["on"]), &Value::Bool(false), "OFF is off");
    let frozen = row(list(&h, &["layers"]), "name", "FROZEN");
    assert_eq!(
        at(frozen, &["frozen"]),
        &Value::Bool(true),
        "FROZEN is frozen"
    );
    let anno = row(list(&h, &["layers"]), "name", "ANNO");
    assert_eq!(
        at(anno, &["fg"]),
        &Value::Bool(true),
        "colour 7 is the foreground"
    );
    // linetypes and text styles
    let dashed = row(list(&h, &["linetypes"]), "name", "DASHED");
    let dashes: Vec<f64> = list(dashed, &["dashes"])
        .iter()
        .map(|x| x.as_f64().expect("a dash"))
        .collect();
    assert_eq!(dashes, [0.5, -0.25]);
    close(number(dashed, &["length"]), 0.75, "DASHED length");
    let sans = row(list(&h, &["text_styles"]), "name", "SANS");
    assert_eq!(string(sans, &["family"]), "Arial");
    assert_eq!(at(sans, &["shx"]), &Value::Bool(false));
    let romans = row(list(&h, &["text_styles"]), "name", "ROMANS");
    assert_eq!(at(romans, &["shx"]), &Value::Bool(true), "a .shx font");
    // settings, blocks, layouts and the closed set of reasons
    close(number(&h, &["settings", "ltscale"]), 2.0, "$LTSCALE");
    close(number(&h, &["settings", "pdmode"]), 3.0, "$PDMODE");
    let door = row(list(&h, &["blocks"]), "name", "DOOR");
    close(number(door, &["entities"]), 4.0, "DOOR's entities");
    assert_eq!(string(door, &["digest"]).len(), 16, "a digest");
    let model = row(list(&h, &["layouts"]), "kind", "model");
    assert_eq!(string(model, &["space"]), "model");
    let paper = row(list(&h, &["layouts"]), "kind", "paper");
    close(number(paper, &["paper", "w_mm"]), 420.0, "paper width");
    close(number(paper, &["paper", "h_mm"]), 297.0, "paper height");
    assert_eq!(string(paper, &["name"]), "Layout1", "the layout's own name");
    let vp = &list(paper, &["viewports"])[0];
    assert_eq!(xy(at(vp, &["centre"])), [210.0, 148.5]);
    close(number(vp, &["scale"]), 2.0, "viewport scale");
    let whys: Vec<&str> = list(&h, &["whys"])
        .iter()
        .map(|w| w.as_str().unwrap_or(""))
        .collect();
    assert_eq!(whys, WHYS);
    println!(
        "draw_header_tables: {} layers, {} linetypes, {} styles, {} blocks, {} layouts",
        list(&h, &["layers"]).len(),
        list(&h, &["linetypes"]).len(),
        list(&h, &["text_styles"]).len(),
        list(&h, &["blocks"]).len(),
        list(&h, &["layouts"]).len()
    );
}

#[test]
pub fn draw_style_resolved() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let st = |l: &str| at(ent(&r, &b, l), &["style"]).clone();
    // by layer: the layer's true colour, linetype, lineweight and transparency
    let wall = st("wall");
    assert_eq!(
        rgb(at(&wall, &["rgb"])),
        [0x33, 0x66, 0x99],
        "wall by layer"
    );
    assert_eq!(
        string(&wall, &["ltype"]),
        "DASHED",
        "wall's linetype by layer"
    );
    close(
        number(&wall, &["lw_mm"]),
        0.35,
        "wall's lineweight by layer",
    );
    close(
        number(&wall, &["alpha"]),
        127.0 / 255.0,
        "wall's alpha by layer",
    );
    close(number(&wall, &["lt_scale"]), 2.0, "1 x $LTSCALE 2");
    // own values
    let own = st("own-aci");
    assert_eq!(rgb(at(&own, &["rgb"])), [0, 0, 255], "colour 5");
    close(number(&own, &["lw_mm"]), 0.5, "own lineweight");
    assert_eq!(
        rgb(at(&st("own-rgb"), &["rgb"])),
        [255, 128, 0],
        "true colour wins"
    );
    // by block in model space: the foreground, continuous, the default lineweight
    let bb = st("byblock-model");
    assert_eq!(rgb(at(&bb, &["rgb"])), [255, 255, 255]);
    assert_eq!(at(&bb, &["fg"]), &Value::Bool(true));
    assert_eq!(string(&bb, &["ltype"]), "CONTINUOUS");
    close(number(&bb, &["lw_mm"]), 0.25, "$LWDEFAULT");
    let a = st("alpha");
    close(number(&a, &["alpha"]), 76.0 / 255.0, "own transparency");
    close(number(&a, &["lt_scale"]), 1.0, "0.5 x $LTSCALE 2");
    assert_eq!(string(&st("dot"), &["ltype"]), "DOT");
    // visibility
    for (l, want) in [
        ("wall", true),
        ("invisible", false),
        ("off", false),
        ("frozen", false),
    ] {
        assert_eq!(
            at(ent(&r, &b, l), &["visible"]),
            &Value::Bool(want),
            "{l} visible"
        );
    }
    println!("draw_style_resolved: 9 entities' styles and 4 visibilities");
}

#[test]
pub fn draw_block_scope_insert() {
    let b = sampler();
    let h = header(&b.bytes);
    let door = block_addr(&h, "DOOR");
    let r = rows(&b.bytes, &door);
    for l in ["door-leaf0", "door-swing"] {
        let st = at(ent(&r, &b, l), &["style"]);
        for k in ["rgb", "ltype", "lw_mm", "alpha"] {
            assert_eq!(
                at(st, &[k]),
                &Value::Str("insert".into()),
                "{l}'s {k} is its reference's"
            );
        }
    }
    assert_eq!(
        at(ent(&r, &b, "door-leaf0"), &["visible"]),
        &Value::Str("insert".into()),
        "a layer-0 entity follows its reference's layer"
    );
    let jamb = at(ent(&r, &b, "door-jamb"), &["style"]);
    assert_eq!(
        rgb(at(jamb, &["rgb"])),
        [0x33, 0x66, 0x99],
        "a block entity on its own layer"
    );
    // never in model space
    let m = rows(&b.bytes, "model");
    let text = Value::List(m.clone()).to_json();
    assert!(!text.contains("\"insert\""), "\"insert\" in model space");
    println!(
        "draw_block_scope_insert: 2 entities x 4 values by reference; {} model rows clean",
        m.len()
    );
}

#[test]
pub fn draw_order_sortents() {
    let b = sampler();
    let h = header(&b.bytes);
    let model = row(list(&h, &["layouts"]), "kind", "model");
    assert_eq!(string(model, &["draw_order"]), "sortents");
    let r = rows(&b.bytes, "model");
    let z = |l: &str| number(ent(&r, &b, l), &["z"]) as usize;
    assert_eq!(z("wall"), r.len() - 1, "wall's sort handle puts it last");
    assert_eq!(z("own-aci"), r.len() - 2, "own-aci sorts by its own handle");
    for (k, e) in r.iter().enumerate() {
        assert_eq!(number(e, &["z"]) as usize, k, "z is the row's place");
    }
    println!("draw_order_sortents: {} rows in draw order", r.len());
}

#[test]
pub fn draw_pages_bounded() {
    let bytes = testgen::heavy(Ver::R2018, 120, 2_000).build().bytes;
    let ps = pages(&bytes, "model");
    assert!(ps.len() >= 2, "{} pages", ps.len());
    let mut z = 0usize;
    for p in &ps {
        let n = p.to_json().len();
        assert!(
            n <= PAGE_BYTES || at(p, &["oversize"]) == &Value::Bool(true),
            "a page of {n} bytes"
        );
        for e in list(p, &["entities"]) {
            assert_eq!(number(e, &["z"]) as usize, z, "pages chain");
            z += 1;
        }
    }
    assert_eq!(z, 120, "every entity once");
    println!("draw_pages_bounded: {} pages, {z} entities", ps.len());
}

#[test]
pub fn draw_stale_cursor_refused() {
    let bytes = testgen::heavy(Ver::R2018, 120, 2_000).build().bytes;
    let first = run(&bytes, "draw page", &["--version", "7", "--space", "model"]);
    let next = first.next.expect("a second page");
    let cursor = next.last().expect("a cursor").clone();
    let e = refused(
        &bytes,
        "draw page",
        &["--version", "8", "--space", "model", "--cursor", &cursor],
    );
    assert_eq!(e.code.name, "stale_cursor");
    println!("draw_stale_cursor_refused: {} refused", e.code.id);
}

#[test]
pub fn draw_basic_geometry() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let g = |l: &str| at(ent(&r, &b, l), &["geom"]).clone();
    let wall = g("wall");
    assert_eq!(xy(at(&wall, &["p1"])), [0.0, 0.0]);
    assert_eq!(xy(at(&wall, &["p2"])), [10.0, 0.0]);
    let c = g("circle");
    assert_eq!(xy(at(&c, &["centre"])), [20.0, 20.0]);
    close(number(&c, &["r"]), 2.0, "radius");
    let arc = g("arc");
    close(number(&arc, &["a0"]), 0.0, "arc start");
    close(
        number(&arc, &["a1"]),
        std::f64::consts::FRAC_PI_2,
        "arc end",
    );
    // mirrored frame: centre x negated, angles reflected, still counter-clockwise
    let m = g("arc-mirrored");
    assert_eq!(xy(at(&m, &["centre"])), [-30.0, 0.0]);
    close(
        number(&m, &["a0"]),
        std::f64::consts::FRAC_PI_2,
        "mirrored start",
    );
    close(number(&m, &["a1"]), std::f64::consts::PI, "mirrored end");
    let t = g("arc-tilted");
    assert_eq!(string(&t, &["ocs"]), "tilted");
    assert!(list(&t, &["path"]).len() > 2, "a tilted arc is a path");
    let e = g("ellipse-mirrored");
    close(
        number(&e, &["p0"]),
        -std::f64::consts::FRAC_PI_2,
        "mirrored ellipse start",
    );
    close(number(&e, &["p1"]), 0.0, "mirrored ellipse end");
    let x = g("xline");
    let d = xy(at(&x, &["dir"]));
    close(d[0], 0.6, "xline dir x");
    close(d[1], 0.8, "xline dir y");
    close(number(&g("point"), &["pdmode"]), 3.0, "point display");
    println!("draw_basic_geometry: 8 entities' values");
}

#[test]
pub fn draw_polylines() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let lw = at(ent(&r, &b, "lw"), &["geom"]);
    let v = list(lw, &["verts"]);
    assert_eq!(v.len(), 3);
    close(number(&v[0], &["w0"]), 0.1, "start width");
    close(number(&v[0], &["w1"]), 0.2, "end width");
    close(number(&v[1], &["bulge"]), 1.0, "the second vertex's bulge");
    assert_eq!(at(lw, &["closed"]), &Value::Bool(true));
    let poly = list(at(ent(&r, &b, "poly"), &["geom"]), &["verts"]);
    close(number(&poly[1], &["bulge"]), 0.5, "a vertex's bulge");
    // a spline-fit polyline: its frame vertices are not drawn
    let fit = list(at(ent(&r, &b, "fit-poly"), &["geom"]), &["verts"]);
    let pts: Vec<[f64; 2]> = fit.iter().map(|v| xy(at(v, &["p"]))).collect();
    assert_eq!(pts, [[1.0, 0.0], [2.0, 1.0]], "fit vertices only");
    println!(
        "draw_polylines: 3 polylines, {} vertices",
        v.len() + poly.len() + fit.len()
    );
}

#[test]
pub fn draw_inserts_are_references() {
    let b = sampler();
    let h = header(&b.bytes);
    let r = rows(&b.bytes, "model");
    for e in &r {
        assert_eq!(string(e, &["owner"]), "model", "a row of another space");
    }
    let d = at(ent(&r, &b, "door1"), &["geom"]);
    assert_eq!(string(d, &["block"]), block_addr(&h, "DOOR"));
    assert!(
        at(d, &["entities"]) == &Value::Null,
        "an insert carries no copy of its block"
    );
    let m: Vec<f64> = list(d, &["m"])
        .iter()
        .map(|x| x.as_f64().unwrap_or(f64::NAN))
        .collect();
    assert_eq!(m, [1.0, 0.0, 0.0, 1.0, 10.0, 0.0]);
    let a = &list(d, &["attribs"])[0];
    assert_eq!(string(a, &["tag"]), "NO");
    assert_eq!(string(a, &["value"]), "D-01");
    // base point (1, 1), scale 2, a quarter turn, at (100, 0)
    let n = at(ent(&r, &b, "nest1"), &["geom"]);
    let m: Vec<f64> = list(n, &["m"])
        .iter()
        .map(|x| x.as_f64().unwrap_or(f64::NAN))
        .collect();
    let want = [0.0, 2.0, -2.0, 0.0, 102.0, -2.0];
    for (got, w) in m.iter().zip(want) {
        close(*got, w, "nest1's matrix");
    }
    let arr = at(ent(&r, &b, "array"), &["geom", "array"]);
    assert_eq!((number(arr, &["cols"]), number(arr, &["rows"])), (2.0, 3.0));
    // nested: the block holds a reference, not the inner block's entities
    let nest = rows(&b.bytes, &block_addr(&h, "NEST"));
    assert_eq!(nest.len(), 1);
    assert_eq!(string(&nest[0], &["kind"]), "INSERT");
    assert_eq!(string(&nest[0], &["geom", "block"]), block_addr(&h, "DOOR"));
    println!(
        "draw_inserts_are_references: 3 references, {} model rows",
        r.len()
    );
}

#[test]
pub fn draw_text_placed() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let g = |l: &str| at(ent(&r, &b, l), &["geom"]).clone();
    let left = g("text-left");
    assert_eq!(xy(at(&left, &["anchor"])), [1.0, 1.0]);
    assert_eq!(string(&left, &["align", "h"]), "left");
    assert_eq!(string(&left, &["align", "v"]), "baseline");
    let c = g("text-centre");
    assert_eq!(xy(at(&c, &["anchor"])), [20.0, 10.0], "the second point");
    assert_eq!(string(&c, &["align", "h"]), "center");
    assert_eq!(string(&c, &["align", "v"]), "middle");
    let fit = g("text-fit");
    assert_eq!(string(&fit, &["align", "h"]), "fit");
    assert_eq!(xy(at(&fit, &["from"])), [0.0, 20.0]);
    assert_eq!(xy(at(&fit, &["to"])), [10.0, 30.0]);
    close(
        number(&fit, &["rotation"]),
        std::f64::consts::FRAC_PI_4,
        "fit direction",
    );
    let codes = g("text-codes");
    assert_eq!(string(&codes, &["plain"]), "\u{2300}50 UNDER 90\u{b0}");
    let runs = list(&codes, &["runs"]);
    assert_eq!(runs.len(), 3);
    assert_eq!(at(&runs[1], &["underline"]), &Value::Bool(true));
    println!("draw_text_placed: 4 texts");
}

#[test]
pub fn draw_mtext_runs() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let m = at(ent(&r, &b, "mtext"), &["geom"]);
    assert_eq!(string(m, &["attach", "h"]), "center");
    assert_eq!(string(m, &["attach", "v"]), "middle");
    close(number(m, &["ref_width"]), 40.0, "reference width");
    let paras = list(m, &["paragraphs"]);
    let texts: Vec<Vec<&str>> = paras
        .iter()
        .map(|p| {
            list(p, &["runs"])
                .iter()
                .map(|r| string(r, &["text"]))
                .collect()
        })
        .collect();
    assert_eq!(
        texts,
        vec![vec!["Bold"], vec!["Line ", "big ", "1/2", "\u{a0}", "red"]]
    );
    let p1 = list(&paras[1], &["runs"]);
    assert_eq!(
        at(&list(&paras[0], &["runs"])[0], &["bold"]),
        &Value::Bool(true)
    );
    close(number(&p1[1], &["height"]), 5.0, "2x of 2.5");
    assert_eq!(string(&p1[2], &["stack", "upper"]), "1");
    assert_eq!(string(&p1[2], &["stack", "lower"]), "2");
    assert_eq!(rgb(at(&p1[4], &["rgb"])), [255, 0, 0]);
    assert_eq!(number(m, &["unknown_codes"]), 0.0);
    println!(
        "draw_mtext_runs: {} paragraphs, {} runs",
        paras.len(),
        p1.len() + 1
    );
}

/// The point of a (rational) B-spline at `u`, by de Boor: our own, for the check only.
fn de_boor(k: &[f64], c: &[[f64; 2]], w: &[f64], p: usize, u: f64) -> [f64; 2] {
    let n = c.len();
    let mut s = p;
    while s + 1 < n && k[s + 1] <= u {
        s += 1;
    }
    let mut d: Vec<[f64; 3]> = (0..=p)
        .map(|j| {
            let i = s - p + j;
            let wi = w.get(i).copied().unwrap_or(1.0);
            [c[i][0] * wi, c[i][1] * wi, wi]
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = s - p + j;
            let den = k[i + p + 1 - r] - k[i];
            let a = if den > 0.0 { (u - k[i]) / den } else { 0.0 };
            let prev = d[j - 1];
            for (c, p) in d[j].iter_mut().zip(prev) {
                *c = (1.0 - a) * p + a * *c;
            }
        }
    }
    [d[p][0] / d[p][2], d[p][1] / d[p][2]]
}

fn to_path(p: [f64; 2], path: &[[f64; 2]]) -> f64 {
    path.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 == 0.0 {
                0.0
            } else {
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
            };
            (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
pub fn draw_splines_within_tolerance() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let mut samples = 0u64;
    for l in ["spline", "spline-rational"] {
        let g = at(ent(&r, &b, l), &["geom"]);
        let tol = number(g, &["tol"]);
        let path: Vec<[f64; 2]> = list(g, &["path"]).iter().map(xy).collect();
        let knots: Vec<f64> = list(g, &["knots"])
            .iter()
            .map(|x| x.as_f64().expect("knot"))
            .collect();
        let ctrl: Vec<[f64; 2]> = list(g, &["ctrl"]).iter().map(xy).collect();
        let w: Vec<f64> = list(g, &["weights"])
            .iter()
            .map(|x| x.as_f64().expect("weight"))
            .collect();
        let p = number(g, &["degree"]) as usize;
        let (lo, hi) = (knots[p], knots[ctrl.len()]);
        let mut worst = 0.0f64;
        for k in 0..=4000 {
            let u = lo + (hi - lo) * k as f64 / 4000.0;
            let q = de_boor(&knots, &ctrl, &w, p, u.min(hi - 1e-12 * (hi - lo)));
            worst = worst.max(to_path(q, &path));
            samples += 1;
        }
        assert!(
            worst <= tol * (1.0 + 1e-9),
            "{l}: the curve is {worst} from its path, past its tolerance {tol}"
        );
        assert_eq!(number(g, &["tol_achieved"]), tol, "{l}: within the cap");
    }
    // the rational quarter circle: every path point on the circle
    let q = at(ent(&r, &b, "spline-rational"), &["geom"]);
    for pt in list(q, &["path"]) {
        let p = xy(pt);
        close(p[0].hypot(p[1]), 10.0, "a point of the quarter circle");
    }
    let fit = at(ent(&r, &b, "spline-fit"), &["geom"]);
    assert_eq!(at(fit, &["interpolated"]), &Value::Bool(false));
    assert_eq!(list(fit, &["path"]).len(), 3);
    let zero = ent(&r, &b, "spline-zero-weight");
    assert_eq!(string(zero, &["why"]), "spline_weights");
    println!("draw_splines_within_tolerance: {samples} curve points checked against 2 paths");
}

fn in_square(p: [f64; 2], b: [f64; 4]) -> bool {
    p[0] >= b[0] - 1e-9 && p[0] <= b[2] + 1e-9 && p[1] >= b[1] - 1e-9 && p[1] <= b[3] + 1e-9
}

#[test]
pub fn draw_hatch_pattern_clipped() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let s = at(ent(&r, &b, "hatch-solid"), &["geom"]);
    assert_eq!(string(s, &["fill"]), "solid");
    assert_eq!(list(&list(s, &["loops"])[0], &["poly", "verts"]).len(), 4);
    let h = at(ent(&r, &b, "hatch-pattern"), &["geom"]);
    assert_eq!(string(h, &["fill"]), "pattern");
    let lines = list(h, &["pattern", "lines"]);
    assert!(!lines.is_empty(), "pattern lines");
    let mut length = 0.0;
    for l in lines {
        let v: Vec<f64> = match l {
            Value::List(x) => x.iter().map(|n| n.as_f64().expect("a number")).collect(),
            _ => panic!("not a segment"),
        };
        let mid = [(v[0] + v[2]) / 2.0, (v[1] + v[3]) / 2.0];
        assert!(
            in_square(mid, [20.0, -30.0, 30.0, -20.0]),
            "outside the boundary: {v:?}"
        );
        assert!(
            !in_square(mid, [23.0, -27.0, 27.0, -23.0]),
            "inside the island: {v:?}"
        );
        length += (v[2] - v[0]).hypot(v[3] - v[1]);
    }
    println!(
        "draw_hatch_pattern_clipped: {} segments, length {length}",
        lines.len()
    );
}

#[test]
pub fn draw_hatch_dense() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let d = at(ent(&r, &b, "hatch-dense"), &["geom", "pattern"]);
    assert_eq!(
        at(d, &["dense"]),
        &Value::Bool(true),
        "dense before any line"
    );
    assert_eq!(at(d, &["lines"]), &Value::Null);
    assert_eq!(string(d, &["units"]), "drawing");
    assert_eq!(
        list(d, &["defs"]).len(),
        1,
        "its definition lines still given"
    );
    println!("draw_hatch_dense: dense, 1 definition line");
}

#[test]
pub fn draw_other_kinds() {
    let b = sampler();
    let h = header(&b.bytes);
    let r = rows(&b.bytes, "model");
    let g = |l: &str| at(ent(&r, &b, l), &["geom"]).clone();
    let q: Vec<[f64; 2]> = list(&g("solid"), &["quad"]).iter().map(xy).collect();
    assert_eq!(
        q,
        [[0.0, -10.0], [4.0, -10.0], [4.0, -6.0], [0.0, -6.0]],
        "a solid's corners in drawing order"
    );
    close(number(&g("face"), &["hidden_edges"]), 2.0, "hidden edges");
    let dim = g("dim");
    assert_eq!(string(&dim, &["block"]), block_addr(&h, "*D1"));
    close(number(&dim, &["measured"]), 100.0, "measurement");
    assert_eq!(string(&dim, &["text", "shown"]), "100");
    assert_eq!(list(&g("leader"), &["path"]).len(), 3);
    // boxes the model doesn't give: computed from what is drawn, containing it
    let lb: Vec<f64> = list(ent(&r, &b, "leader"), &["bbox"])
        .iter()
        .map(|x| x.as_f64().expect("a number"))
        .collect();
    assert_eq!(lb, [0.0, 80.0, 10.0, 85.0], "the leader's box");
    let hb: Vec<f64> = list(ent(&r, &b, "hatch-pattern"), &["bbox"])
        .iter()
        .map(|x| x.as_f64().expect("a number"))
        .collect();
    assert!(
        hb[0] <= 20.0
            && hb[1] <= -30.0
            && hb[2] >= 30.0
            && hb[3] >= -20.0
            && hb[0] > 19.99
            && hb[3] < -19.99,
        "the hatch's box {hb:?}"
    );
    assert_eq!(string(ent(&r, &b, "hatch-pattern"), &["bbox_q"]), "approx");
    // a leader's arrow: its kind and its size from its dimension style, its own overrides first
    let plain = g("leader");
    assert_eq!(string(&plain, &["arrow", "kind"]), "closed_filled");
    close(
        number(&plain, &["arrow", "size"]),
        0.18,
        "the style's arrow size",
    );
    let styled = g("leader-styled");
    close(
        number(&styled, &["arrow", "size"]),
        5.0,
        "2.5 x scale 2, the leader's own",
    );
    // an image: its corners, and its definition's file
    let img = g("image");
    let c: Vec<[f64; 2]> = list(&img, &["corners"]).iter().map(xy).collect();
    assert_eq!(c[0], [100.0, 100.0]);
    close(c[2][0], 110.0, "image width");
    close(c[2][1], 105.0, "image height");
    assert_eq!(string(&img, &["image", "path"]), "images/site-plan.png");
    let clip: Vec<[f64; 2]> = list(&img, &["clip"]).iter().map(xy).collect();
    assert_eq!(
        clip.len(),
        4,
        "a rectangle clip: closed, its first point not repeated"
    );
    assert_eq!(at(&img, &["closed"]), &Value::Bool(true));
    let mask: Vec<[f64; 2]> = list(&g("wipeout"), &["mask"]).iter().map(xy).collect();
    assert_eq!(mask.len(), 4, "a closed ring, its first point not repeated");
    for p in &mask {
        assert!(in_square(*p, [60.0, 60.0, 70.0, 70.0]), "mask point {p:?}");
    }
    println!("draw_other_kinds: 7 kinds");
}

#[test]
pub fn draw_whys_closed_set() {
    let b = sampler();
    let h = header(&b.bytes);
    let mut seen = 0u64;
    let mut spaces: Vec<String> = list(&h, &["layouts"])
        .iter()
        .map(|l| string(l, &["space"]).to_owned())
        .collect();
    spaces.extend(
        list(&h, &["blocks"])
            .iter()
            .map(|x| string(x, &["handle"]).to_owned()),
    );
    for sp in &spaces {
        for e in rows(&b.bytes, sp) {
            if at(&e, &["drawn"]) == &Value::Bool(false) {
                let w = string(&e, &["why"]);
                assert!(
                    WHYS.contains(&w),
                    "{} gives why {w:?}, outside the set",
                    string(&e, &["h"])
                );
                seen += 1;
            }
        }
    }
    let m = rows(&b.bytes, "model");
    assert_eq!(string(ent(&m, &b, "unknown"), &["why"]), "not_modelled");
    assert_eq!(string(ent(&m, &b, "solid3d"), &["why"]), "not_modelled");
    assert_eq!(
        at(ent(&m, &b, "mleader"), &["drawn"]),
        &Value::Bool(true),
        "a multileader is drawn, from the DXF reader's typed multileader"
    );
    let door = rows(&b.bytes, &block_addr(&h, "DOOR"));
    assert_eq!(string(ent(&door, &b, "door-attdef"), &["why"]), "attdef");
    assert!(seen >= 5, "only {seen} rows not drawn");
    println!(
        "draw_whys_closed_set: {seen} rows not drawn, each with a known why, over {} spaces",
        spaces.len()
    );
}

/// The fixture K2 Design's viewer starts from: the header and pages of the sampler, byte for
/// byte against files written at the commit that changes them (`GOLDEN_WRITE=1`).
#[test]
pub fn draw_fixture_matches_golden() {
    let b = sampler();
    let h = header(&b.bytes);
    let door = block_addr(&h, "DOOR");
    let cases = [
        ("draw-sampler-2018-header", h.to_json_pretty()),
        (
            "draw-sampler-2018-page-model",
            pages(&b.bytes, "model")[0].to_json_pretty(),
        ),
        (
            "draw-sampler-2018-page-door",
            pages(&b.bytes, &door)[0].to_json_pretty(),
        ),
        (
            "draw-sampler-2018-page-nest",
            pages(&b.bytes, &block_addr(&h, "NEST"))[0].to_json_pretty(),
        ),
        (
            "draw-sampler-2018-page-dimension",
            pages(&b.bytes, &block_addr(&h, "*D1"))[0].to_json_pretty(),
        ),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let write = std::env::var("GOLDEN_WRITE").as_deref() == Ok("1");
    let mut compared = 0;
    for (name, got) in &cases {
        let path = dir.join(format!("{name}.json"));
        let got = format!("{got}\n");
        if write {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("no golden {}: write them at this commit", path.display()));
        assert!(got == want, "{name} differs from its golden");
        compared += 1;
    }
    assert!(write || compared == 5, "only {compared} compared");
    println!("draw_fixture_matches_golden: {compared} files byte for byte");
}

#[test]
pub fn draw_reads_every_version() {
    let mut total = 0usize;
    for v in Ver::ALL {
        let b = testgen::sampler(v).build();
        let h = header(&b.bytes);
        for l in list(&h, &["layouts"]) {
            let sp = string(l, &["space"]).to_owned();
            let r = rows(&b.bytes, &sp);
            assert_eq!(
                r.len() as f64,
                number(l, &["entities"]),
                "{}: every entity of {sp} on its pages",
                v.acadver()
            );
            total += r.len();
        }
    }
    assert!(total > 0);
    println!(
        "draw_reads_every_version: {} versions, {total} rows",
        Ver::ALL.len()
    );
}

#[test]
pub fn draw_mtext_box_attachment() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    for k in 1..=9usize {
        let e = ent(&r, &b, &format!("mtext-attach-{k}"));
        let bb: Vec<f64> = list(e, &["bbox"])
            .iter()
            .map(|x| x.as_f64().expect("a number"))
            .collect();
        let x = 300.0 + 60.0 * k as f64;
        let (col, row) = ((k - 1) % 3, (k - 1) / 3);
        let gx = [bb[0], (bb[0] + bb[2]) / 2.0, bb[2]][col];
        let gy = [bb[3], (bb[1] + bb[3]) / 2.0, bb[1]][row];
        close(
            gx,
            x,
            &format!("attachment {k}: the box's x at the attachment point"),
        );
        close(
            gy,
            100.0,
            &format!("attachment {k}: the box's y at the attachment point"),
        );
        close(bb[2] - bb[0], 40.0, "the reference width");
    }
    println!("draw_mtext_box_attachment: 9 attachments");
}

/// A dimension's text, as a drawing program writes it, sits clear of its dimension line: the
/// text's box (middle-centre attachment above the line) doesn't reach the line.
#[test]
pub fn draw_dimension_text_clear_of_line() {
    let b = sampler();
    let h = header(&b.bytes);
    let r = rows(&b.bytes, &block_addr(&h, "*D1"));
    let line = r
        .iter()
        .find(|e| string(e, &["kind"]) == "LINE")
        .expect("the dimension line");
    let ly = list(line, &["bbox"])[3].as_f64().expect("the line's y");
    let text = r
        .iter()
        .find(|e| string(e, &["kind"]) == "MTEXT")
        .expect("the dimension text");
    let tb: Vec<f64> = list(text, &["bbox"])
        .iter()
        .map(|x| x.as_f64().expect("a number"))
        .collect();
    assert!(
        tb[1] > ly,
        "the dimension text's box {tb:?} reaches its line at y {ly}"
    );
    println!(
        "draw_dimension_text_clear_of_line: text box from y {} above the line at {ly}",
        tb[1]
    );
}

/// A multiline text's box grows with its runs' own heights and with wrapping at its reference
/// width: the sampler's second paragraph has 5-high runs over 40 wide, so the text takes three
/// lines (2.5 high, then two lines of 5 at a pitch of 5/3).
#[test]
pub fn draw_mtext_box_wraps_and_grows() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let e = ent(&r, &b, "mtext");
    let bb: Vec<f64> = list(e, &["bbox"])
        .iter()
        .map(|x| x.as_f64().expect("a number"))
        .collect();
    let height = bb[3] - bb[1];
    let want = 2.5 + 2.0 * 5.0 * 5.0 / 3.0;
    close(height, want, "three lines: 2.5, then two of 5");
    close((bb[1] + bb[3]) / 2.0, 60.0, "middle attachment");
    assert_eq!(string(e, &["bbox_q"]), "approx");
    println!("draw_mtext_box_wraps_and_grows: height {height} for three lines");
}

/// Text boxes are estimates (an advance of 0.6 of the height a character; real fonts vary):
/// every single-line and multiline text's box says `approx`, never `exact`.
#[test]
pub fn draw_text_boxes_are_approx() {
    let b = sampler();
    let r = rows(&b.bytes, "model");
    let mut n = 0;
    for e in r
        .iter()
        .filter(|e| matches!(string(e, &["kind"]), "TEXT" | "MTEXT" | "ATTDEF"))
    {
        assert_eq!(
            string(e, &["bbox_q"]),
            "approx",
            "{}: a text box is an estimate",
            string(e, &["h"])
        );
        n += 1;
    }
    assert!(n >= 10, "only {n} texts");
    println!("draw_text_boxes_are_approx: {n} texts");
}

/// A 2018 multi-line attribute definition's embedded multiline text (after `101`) leaves the
/// attribute's own fields alone: its width factor and generation flags are the plain twin's.
#[test]
pub fn draw_attdef_embedded_object() {
    let b = testgen::attdef_embedded_sampler().build();
    let r = rows(&b.bytes, "model");
    let plain = at(ent(&r, &b, "plain"), &["geom"]).clone();
    let ml = at(ent(&r, &b, "multiline"), &["geom"]).clone();
    assert_ne!(plain, Value::Null, "the constant definition is drawn");
    assert_eq!(
        ml, plain,
        "the attribute's own width factor and generation, not its embedded text's"
    );
    println!("draw_attdef_embedded_object: width and generation the plain twin's");
}

/// A 2018 multiline text's embedded object (after `101`) is read in its own meaning: its `11` is
/// its insertion point, not the text's direction, so the text isn't turned; its columns reach
/// the read. Its box is the plain twin's, 40 to the right.
#[test]
pub fn draw_mtext_embedded_object() {
    let b = testgen::mtext_embedded_sampler().build();
    let r = rows(&b.bytes, "model");
    let plain = at(ent(&r, &b, "plain"), &["geom"]).clone();
    let emb = at(ent(&r, &b, "embedded"), &["geom"]).clone();
    close(
        number(&emb, &["rotation"]),
        0.0,
        "the embedded object's insertion point isn't a direction",
    );
    close(
        number(&plain, &["rotation"]),
        0.0,
        "the plain text isn't turned",
    );
    assert_eq!(number(&emb, &["columns", "kind"]), 1.0, "static columns");
    assert_eq!(number(&emb, &["columns", "count"]), 2.0);
    close(number(&emb, &["columns", "width"]), 30.0, "column width");
    close(number(&emb, &["columns", "gutter"]), 2.5, "column gutter");
    assert_eq!(at(&plain, &["columns"]), &Value::Null);
    let bb = |l: &str| -> Vec<f64> {
        list(ent(&r, &b, l), &["bbox"])
            .iter()
            .map(|x| x.as_f64().expect("a number"))
            .collect()
    };
    let (p, e) = (bb("plain"), bb("embedded"));
    for k in 0..4 {
        close(
            e[k],
            p[k] + if k % 2 == 0 { 40.0 } else { 0.0 },
            "the twin's box, 40 to the right",
        );
    }
    println!("draw_mtext_embedded_object: rotation 0, 2 columns, the twin's box");
}

/// Without a draw-order table a space draws in the file's order, not handle order: the
/// references drawing writes a text with a high handle before a leader with a lower one.
#[test]
pub fn draw_order_file_without_table() {
    let b = testgen::references_sampler().build();
    let h = header(&b.bytes);
    let model = row(list(&h, &["layouts"]), "kind", "model");
    assert_eq!(string(model, &["draw_order"]), "file");
    let r = rows(&b.bytes, "model");
    let hs: Vec<&str> = r.iter().map(|e| string(e, &["h"])).collect();
    let (t, l) = (
        hs.iter().position(|x| *x == "@h:500").expect("the text"),
        hs.iter()
            .position(|x| {
                r.iter()
                    .any(|e| string(e, &["h"]) == *x && string(e, &["kind"]) == "LEADER")
            })
            .expect("the leader"),
    );
    assert!(
        t < l,
        "file order: the text (written first, handle 500) before the leader"
    );
    println!(
        "draw_order_file_without_table: {} rows, the text at {t} before the leader at {l}",
        r.len()
    );
}

/// The significant digits of a number as JSON writes it (`-0.00123` has 3).
fn significant(json: &str) -> usize {
    let d: String = json.chars().filter(char::is_ascii_digit).collect();
    d.trim_start_matches('0').trim_end_matches('0').len()
}

/// Every number with a fraction in `v`, as JSON writes it.
fn floats(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Float(_) => out.push(v.to_json()),
        Value::List(items) => items.iter().for_each(|x| floats(x, out)),
        Value::Obj(kv) => kv.iter().for_each(|(_, x)| floats(x, out)),
        _ => {}
    }
}

/// draw_floats_platform_identical: a number of the drawing-model read is written rounded to 9
/// significant digits (then the fewest that read back as that), so a value whose last bits a
/// platform's maths library computes otherwise is written the same: the value and its
/// neighbours one unit in the last place apart render alike; and every number of the
/// sampler's header and pages has at most 9 significant digits. Plant: `DrawFloatsUnrounded`.
#[test]
pub fn draw_floats_platform_identical() {
    use k2_mpc_cad_model::draw::out_float;
    let json = |x: f64| Value::float(out_float(x)).unwrap().to_json();
    let mut pinned = 0;
    for (x, want) in [
        (30f64.to_radians().sin(), "0.5"),
        (0.1 + 0.2, "0.3"),
        (2f64.sqrt() * 1000.0, "1414.21356"),
        (-std::f64::consts::PI, "-3.14159265"),
        (1e-7 / 3.0, "0.0000000333333333"),
    ] {
        for y in [
            f64::from_bits(x.to_bits() - 1),
            x,
            f64::from_bits(x.to_bits() + 1),
        ] {
            assert_eq!(
                json(y),
                want,
                "draw_floats_platform_identical: {y:?} (one unit from {x:?})"
            );
        }
        pinned += 1;
    }
    let b = sampler();
    let h = header(&b.bytes);
    let mut all = Vec::new();
    floats(&h, &mut all);
    let mut spaces: Vec<String> = list(&h, &["layouts"])
        .iter()
        .map(|l| string(l, &["space"]).to_owned())
        .collect();
    spaces.extend(["DOOR", "NEST", "*D1"].map(|n| block_addr(&h, n)));
    for sp in &spaces {
        for p in pages(&b.bytes, sp) {
            floats(&p, &mut all);
        }
    }
    let long: Vec<&String> = all.iter().filter(|s| significant(s) > 9).collect();
    assert!(
        long.is_empty(),
        "draw_floats_platform_identical: numbers over 9 significant digits: {:?}",
        &long[..long.len().min(5)]
    );
    assert!(
        all.len() > 100,
        "draw_floats_platform_identical: {} numbers",
        all.len()
    );
    println!(
        "draw_floats_platform_identical: {pinned} values pinned, {} numbers of 9 digits or fewer",
        all.len()
    );
}

/// draw_noise_snapped_to_zero: a number of the drawing-model read whose magnitude is below
/// 1e-12 of the drawing's scale (its computed extents' larger span) is written 0, so a value
/// that is zero but for a platform's last-bit noise (a box edge at -2.22e-16) is the same on
/// every platform: every number of the sampler's header and pages is 0 or above that size, and
/// the sampler's noisy box edge renders 0. Plant: `DrawNoiseKept`.
#[test]
pub fn draw_noise_snapped_to_zero() {
    let b = sampler();
    let h = header(&b.bytes);
    let c: Vec<f64> = list(&h, &["extents", "computed"])
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let eps = 1e-12 * (c[2] - c[0]).max(c[3] - c[1]);
    let mut all = Vec::new();
    floats(&h, &mut all);
    let mut spaces: Vec<String> = list(&h, &["layouts"])
        .iter()
        .map(|l| string(l, &["space"]).to_owned())
        .collect();
    spaces.extend(["DOOR", "NEST", "*D1"].map(|n| block_addr(&h, n)));
    for sp in &spaces {
        for p in pages(&b.bytes, sp) {
            floats(&p, &mut all);
        }
    }
    let noise: Vec<&String> = all
        .iter()
        .filter(|s| {
            let x: f64 = s.parse().unwrap_or(1.0);
            x != 0.0 && x.abs() < eps
        })
        .collect();
    assert!(
        noise.is_empty(),
        "draw_noise_snapped_to_zero: numbers below {eps:e} not written 0: {:?}",
        &noise[..noise.len().min(5)]
    );
    assert!(
        all.len() > 100,
        "draw_noise_snapped_to_zero: {} numbers",
        all.len()
    );
    println!(
        "draw_noise_snapped_to_zero: {} numbers, none below {eps:e} but 0",
        all.len()
    );
}

/// draw_sampler_as_complete_as_a_drawing: the sampler carries what a real drawing does, so a
/// drawing saved from it in another format compares fact for fact: its stated extents (the
/// computed ones), a multileader's arrowhead size, and a layout's own sheet viewport (id 1),
/// which the read leaves out, beside the viewport onto model space it lists.
#[test]
pub fn draw_sampler_as_complete_as_a_drawing() {
    let b = sampler();
    let h = header(&b.bytes);
    let ext = at(&h, &["extents"]);
    assert_eq!(
        at(ext, &["stated"]).to_json(),
        at(ext, &["computed"]).to_json(),
        "draw_sampler_as_complete_as_a_drawing: the stated extents are the computed ones"
    );
    let rows = rows(&b.bytes, "model");
    let m = ent(&rows, &b, "mleader");
    assert!(
        at(m, &["geom", "arrow", "size"]).as_f64().is_some(),
        "draw_sampler_as_complete_as_a_drawing: the multileader's arrowhead size: {}",
        at(m, &["geom", "arrow"]).to_json()
    );
    let paper = row(list(&h, &["layouts"]), "kind", "paper");
    let vps: Vec<String> = list(paper, &["viewports"])
        .iter()
        .map(|v| string(v, &["handle"]).to_owned())
        .collect();
    assert_eq!(
        vps,
        vec![b.addr("viewport")],
        "draw_sampler_as_complete_as_a_drawing: the sheet's own viewport left out, the other listed"
    );
    assert_eq!(
        number(paper, &["entities"]),
        3.0,
        "draw_sampler_as_complete_as_a_drawing: the layout holds its sheet viewport, the viewport \
         and the title"
    );
    println!("draw_sampler_as_complete_as_a_drawing: extents stated, arrow sized, sheet viewport");
}

/// The refusal an `edit` word stands for.
fn edit_code(word: &str) -> &'static str {
    match word {
        "version" => "DXF-E040",
        "no-handle" => "DXF-E041",
        "xref" => "DXF-E042",
        "proxy" => "DXF-E043",
        "dynamic" => "DXF-E044",
        "locked-layer" => "DXF-E045",
        "in-block" => "DXF-E046",
        "multiline-attribute" => "DXF-E049",
        _ => "DXF-E021",
    }
}

/// draw_edit_matches_the_verbs: every row of every space (model, the layouts, every block) has
/// `edit`: null exactly when `entity move` takes the entity, else a word of the closed set whose
/// refusal is the one `entity move` gives. Plant: `InBlockDrawnEditable`.
#[test]
pub fn draw_edit_matches_the_verbs() {
    use k2_mpc_cad_model::draw::EDIT_WORDS;
    let b = sampler();
    let h = header(&b.bytes);
    let unit = string(&h, &["units", "word"]).to_owned();
    let unit = if matches!(unit.as_str(), "unitless" | "unknown") {
        "drawing".to_owned()
    } else {
        unit
    };
    let mut spaces: Vec<String> = list(&h, &["layouts"])
        .iter()
        .map(|l| string(l, &["space"]).to_owned())
        .collect();
    spaces.extend(
        list(&h, &["blocks"])
            .iter()
            .map(|x| string(x, &["handle"]).to_owned()),
    );
    let (mut takes, mut refuses) = (0, 0);
    for sp in &spaces {
        for r in rows(&b.bytes, sp) {
            let addr = string(&r, &["h"]).to_owned();
            let word = at(&r, &["edit"]).as_str().map(str::to_owned);
            let got = match k2_mpc_dxf::edit::plan(
                &b.bytes,
                "entity move",
                &[&addr, "--by", "1,0", "--unit", &unit],
            ) {
                Ok(_) => None,
                Err(e) => Some(k2_mpc_dxf::pack::pack_id(e.code.id)),
            };
            match (&word, &got) {
                (None, None) => takes += 1,
                (Some(w), Some(code)) => {
                    assert!(
                        EDIT_WORDS.contains(&w.as_str()),
                        "draw_edit_matches_the_verbs: {addr}: {w} isn't one of the words"
                    );
                    assert_eq!(
                        code.as_str(),
                        edit_code(w),
                        "draw_edit_matches_the_verbs: {addr} ({}): edit {w}",
                        string(&r, &["kind"])
                    );
                    refuses += 1;
                }
                _ => panic!(
                    "draw_edit_matches_the_verbs: {addr} ({}): edit {word:?}, entity move {got:?}",
                    string(&r, &["kind"])
                ),
            }
        }
    }
    assert!(
        takes > 10 && refuses > 5,
        "draw_edit_matches_the_verbs: {takes} taken, {refuses} refused"
    );
    println!("draw_edit_matches_the_verbs: {takes} rows the verbs take, {refuses} they refuse, each named");
}
