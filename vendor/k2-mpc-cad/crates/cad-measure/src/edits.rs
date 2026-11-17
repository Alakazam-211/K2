//! The edit verbs' referee files (`cad-measure edits OUT`) and their comparison with what a test
//! checker read (`cad-measure edit-referee DIR`).
//!
//! `edits` writes, for each style of the edit sampler and each call of each edit verb, the
//! input (`<name>.in.dxf`), the verb's output (`<name>.out.dxf`) and what the call meant
//! (`<name>.intent.txt`, one line per change: `move H dx dy`, `color H N`, `rgb H N`,
//! `ltype H NAME`, `layer H NAME`, `attrib H TAG VALUE`, `gone H`, `new H TYPE LAYER`,
//! `newp H KEY X Y`, `newx H TEXT`). The checker (`scripts/referee/ezdxf-edits.py`) writes
//! what it reads of each handle an intent names, in the input and in the output, and its audit
//! counts. `edit-referee` then holds the output's facts to the input's, changed as the intent
//! says and in no other way, and the output's audit to no new errors.

use std::collections::BTreeMap;
use std::path::Path;

use k2_mpc_dxf::edit;
use k2_mpc_dxf::testgen::{self, Eol, Ver};

/// One call: its verb, its arguments and what it means, by the labels of the sampler.
struct Call {
    name: &'static str,
    verb: &'static str,
    args: Vec<String>,
    intent: Vec<String>,
}

fn calls(b: &testgen::Built) -> Vec<Call> {
    let h = |l: &str| format!("{:X}", b.handle(l));
    let a = |l: &str| b.addr(l);
    let mut out = Vec::new();
    let moved = [
        "wall", "lamp", "lw", "poly", "room", "note", "ellipse", "dev", "dyn",
    ];
    let mut args: Vec<String> = moved.iter().map(|l| a(l)).collect();
    args.extend([
        "--by".into(),
        "1.5,-2.25".into(),
        "--unit".into(),
        "mm".into(),
    ]);
    out.push(Call {
        name: "move",
        verb: "entity move",
        args,
        intent: moved
            .iter()
            .map(|l| format!("move {} 1.5 -2.25", h(l)))
            .collect(),
    });
    out.push(Call {
        name: "set-color",
        verb: "entity set",
        args: vec![a("wall"), a("red"), "--color".into(), "3".into()],
        intent: vec![
            format!("color {} 3", h("wall")),
            format!("color {} 3", h("red")),
        ],
    });
    out.push(Call {
        name: "set-rgb",
        verb: "entity set",
        args: vec![a("lamp"), "--rgb".into(), "#00ff80".into()],
        intent: vec![format!("rgb {} {}", h("lamp"), 0x00ff80)],
    });
    out.push(Call {
        name: "set-ltype-layer",
        verb: "entity set",
        args: vec![
            a("arc"),
            a("room"),
            "--ltype".into(),
            "CONTINUOUS".into(),
            "--layer".into(),
            "ANNO".into(),
        ],
        intent: vec![
            format!("ltype {} CONTINUOUS", h("arc")),
            format!("layer {} ANNO", h("arc")),
            format!("ltype {} CONTINUOUS", h("room")),
            format!("layer {} ANNO", h("room")),
        ],
    });
    for (name, l, tag, value) in [
        ("attrib", "dev", "DEVICE", "CAM-07"),
        ("attrib-text", "dev", "LABEL", "Caf\u{e9} \u{2116}5"),
        ("attrib-dynamic", "dyn", "DEVICE", "CAM-08"),
    ] {
        out.push(Call {
            name,
            verb: "attrib set",
            args: vec![
                a(l),
                "--tag".into(),
                tag.into(),
                "--value".into(),
                value.into(),
            ],
            intent: vec![format!("attrib {} {tag} {value}", h(l))],
        });
    }
    // the handle an add creates, filled in from its plan
    let new = "NEW";
    out.push(Call {
        name: "add-line",
        verb: "entity add",
        args: [
            "line", "--from", "0,0", "--to", "3,4", "--unit", "mm", "--layer", "A-WALL",
        ]
        .map(String::from)
        .to_vec(),
        intent: vec![
            format!("new {new} LINE A-WALL"),
            format!("newp {new} start 0 0"),
            format!("newp {new} end 3 4"),
        ],
    });
    out.push(Call {
        name: "add-polyline",
        verb: "entity add",
        args: [
            "polyline",
            "--points",
            "0,0;5,0;5,5",
            "--closed",
            "--unit",
            "mm",
            "--layer",
            "E-DEV",
        ]
        .map(String::from)
        .to_vec(),
        intent: vec![
            format!("new {new} LWPOLYLINE E-DEV"),
            format!("newp {new} v0 0 0"),
            format!("newp {new} v1 5 0"),
            format!("newp {new} v2 5 5"),
        ],
    });
    out.push(Call {
        name: "add-text",
        verb: "entity add",
        args: [
            "text", "--at", "5,5", "--height", "2", "--value", "NEW 1", "--unit", "mm", "--layer",
            "ANNO",
        ]
        .map(String::from)
        .to_vec(),
        intent: vec![
            format!("new {new} TEXT ANNO"),
            format!("newp {new} insert 5 5"),
            format!("newx {new} NEW 1"),
        ],
    });
    out.push(Call {
        name: "delete",
        verb: "entity delete",
        args: vec![a("arc"), a("lamp"), a("dev")],
        intent: ["arc", "lamp", "dev"]
            .iter()
            .map(|l| format!("gone {}", h(l)))
            .collect(),
    });
    out
}

/// `cad-measure edits OUT`: the referee's inputs (module docs).
pub fn write(out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut files = 0;
    for v in [Ver::R2000, Ver::R2007, Ver::R2018] {
        for (eol, pad) in [(Eol::Lf, false), (Eol::CrLf, true)] {
            let mut s = testgen::edit_sampler(v);
            s.opts.eol = eol;
            s.opts.pad = pad;
            let b = s.build();
            for c in calls(&b) {
                let name = format!("{}-{eol:?}-{}", v.acadver(), c.name);
                let refs: Vec<&str> = c.args.iter().map(String::as_str).collect();
                let p = edit::plan(&b.bytes, c.verb, &refs)
                    .map_err(|e| format!("{name}: {} refused: {e}", c.verb))?;
                let put = |suffix: &str, bytes: &[u8]| {
                    let path = out.join(format!("{name}.{suffix}"));
                    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
                };
                put("in.dxf", &b.bytes)?;
                put("out.dxf", &p.output)?;
                let new = p
                    .created
                    .first()
                    .map(|k| k.trim_start_matches("@h:").to_owned())
                    .unwrap_or_default();
                let intent: Vec<String> = c
                    .intent
                    .iter()
                    .map(|l| l.replace(" NEW ", &format!(" {new} ")))
                    .collect();
                put("intent.txt", format!("{}\n", intent.join("\n")).as_bytes())?;
                files += 1;
            }
        }
    }
    println!("edits: {files} calls written");
    Ok(())
}

/// The checker's facts of one file: by handle, each fact line after its handle.
type Facts = BTreeMap<String, Vec<(String, Vec<String>)>>;

fn facts(text: &str) -> (Facts, Option<u64>) {
    let mut f: Facts = BTreeMap::new();
    let mut audit = None;
    for l in text.lines() {
        let w: Vec<&str> = l.splitn(3, ' ').collect();
        match w.as_slice() {
            ["AUDIT", n] => audit = n.parse().ok(),
            [kind, h, rest] => {
                // a text is one value; an attribute its tag, then its value
                let v: Vec<String> = match *kind {
                    "S" => vec![(*rest).to_owned()],
                    "A" => match rest.split_once(' ') {
                        Some((t, v)) => vec![t.to_owned(), v.to_owned()],
                        None => vec![(*rest).to_owned(), String::new()],
                    },
                    _ => rest.split(' ').map(str::to_owned).collect(),
                };
                f.entry(h.to_ascii_uppercase())
                    .or_default()
                    .push(((*kind).to_owned(), v));
            }
            [kind, h] => f
                .entry(h.to_ascii_uppercase())
                .or_default()
                .push(((*kind).to_owned(), Vec::new())),
            _ => {}
        }
    }
    (f, audit)
}

fn num(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

fn close(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (num(x), num(y)) {
            (Some(p), Some(q)) => (p - q).abs() <= 1e-9 * (1.0 + p.abs().max(q.abs())),
            _ => x == y,
        })
}

/// What the intent says the output's facts of a handle are, from the input's.
fn expected(input: &[(String, Vec<String>)], lines: &[Vec<&str>]) -> Vec<(String, Vec<String>)> {
    let mut f = input.to_vec();
    for w in lines {
        let rest = |k: usize| w[k..].join(" ");
        match w[0] {
            "move" => {
                let (dx, dy) = (num(w[2]).unwrap_or(0.0), num(w[3]).unwrap_or(0.0));
                for (kind, v) in &mut f {
                    if kind == "P" && v.len() >= 3 {
                        let x = num(&v[1]).unwrap_or(0.0) + dx;
                        let y = num(&v[2]).unwrap_or(0.0) + dy;
                        v[1] = format!("{x}");
                        v[2] = format!("{y}");
                    }
                }
            }
            "color" | "rgb" | "ltype" => {
                let (kind, value) = match w[0] {
                    "color" => ("C", w[2].to_owned()),
                    "rgb" => ("T", w[2].to_owned()),
                    _ => ("LT", rest(2)),
                };
                for (k, v) in &mut f {
                    if k == kind {
                        *v = vec![value.clone()];
                    }
                    if w[0] == "color" && k == "T" {
                        *v = vec!["none".into()];
                    }
                }
            }
            "layer" => {
                for (k, v) in &mut f {
                    if k == "E" && v.len() >= 2 {
                        v[1] = w[2].to_owned();
                    }
                }
            }
            "attrib" => {
                let value = rest(3);
                for (k, v) in &mut f {
                    if k == "A" && v.first().map(String::as_str) == Some(w[2]) {
                        *v = vec![w[2].to_owned(), value.clone()];
                    }
                }
            }
            "gone" => f = vec![("G".into(), Vec::new())],
            _ => {}
        }
    }
    f
}

/// `cad-measure edit-referee DIR`: each output held to its input and intent, as the checker
/// read both.
pub fn referee(dir: &Path) -> Result<(), String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".intent.txt")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    let read = |p: String| std::fs::read_to_string(dir.join(&p)).map_err(|e| format!("{p}: {e}"));
    let (mut compared, mut differ, mut handles) = (0u64, 0u64, 0u64);
    for name in &names {
        let intent = read(format!("{name}.intent.txt"))?;
        let ins = read(format!("{name}.in.dxf.ezdxf-edits.txt"));
        let outs = read(format!("{name}.out.dxf.ezdxf-edits.txt"));
        let (Ok(ins), Ok(outs)) = (ins, outs) else {
            println!("edit-referee {name}: no checker summary");
            differ += 1;
            continue;
        };
        if let Some(why) = outs.lines().find_map(|l| l.strip_prefix("X ")) {
            println!("edit-referee {name}: REFUSED by the checker: {why}");
            differ += 1;
            continue;
        }
        let (fi, ai) = facts(&ins);
        let (fo, ao) = facts(&outs);
        let mut by_h: BTreeMap<String, Vec<Vec<&str>>> = BTreeMap::new();
        for l in intent.lines().filter(|l| !l.is_empty()) {
            let w: Vec<&str> = l.split(' ').collect();
            by_h.entry(w[1].to_ascii_uppercase()).or_default().push(w);
        }
        let mut what = Vec::new();
        for (h, lines) in &by_h {
            handles += 1;
            let got = fo.get(h).cloned().unwrap_or_default();
            let want = if lines[0][0].starts_with("new") {
                let mut w = Vec::new();
                for l in lines {
                    match l[0] {
                        "new" => w.push(("E".to_owned(), vec![l[2].to_owned(), l[3].to_owned()])),
                        "newp" => w.push((
                            "P".to_owned(),
                            vec![l[2].to_owned(), l[3].to_owned(), l[4].to_owned()],
                        )),
                        "newx" => w.push(("S".to_owned(), vec![l[2..].join(" ")])),
                        _ => {}
                    }
                }
                // a new entity: each fact meant is among those read
                if !w
                    .iter()
                    .all(|(k, v)| got.iter().any(|(gk, gv)| gk == k && close(gv, v)))
                {
                    what.push(format!("@h:{h} (new): meant {w:?}, read {got:?}"));
                }
                continue;
            } else {
                expected(&fi.get(h).cloned().unwrap_or_default(), lines)
            };
            let mut a = got.clone();
            let mut b = want.clone();
            a.sort();
            b.sort();
            let same = a.len() == b.len()
                && a.iter()
                    .zip(&b)
                    .all(|((ka, va), (kb, vb))| ka == kb && close(va, vb));
            if !same {
                what.push(format!("@h:{h}: meant {b:?}, read {a:?}"));
            }
        }
        if ao.unwrap_or(u64::MAX) > ai.unwrap_or(0) {
            what.push(format!("audit errors {ao:?}, the input's {ai:?}"));
        }
        compared += 1;
        if what.is_empty() {
            println!("edit-referee {name}: agrees ({} handles)", by_h.len());
        } else {
            differ += 1;
            for w in what.iter().take(6) {
                println!("edit-referee {name}: DIFFERS: {w}");
            }
        }
    }
    println!("edit-referee: {compared} compared, {differ} differ, {handles} handles");
    if compared == 0 || differ > 0 {
        return Err(format!("{differ} of {} differ", names.len()));
    }
    Ok(())
}
