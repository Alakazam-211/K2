//! Applies fixed-size edits to a BLEND file with the writer: test tooling for the headless round
//! trip (`tests/roundtrip.rs`).
//!
//!     cargo run --example blend_patch -- IN OUT [KEY FIELD VALUE]...
//!
//! FIELD is `loc`, `rot` or `scale` (VALUE `x,y,z`), `hide_viewport`, `hide_select` or
//! `hide_render` (VALUE `true` or `false`), or `name` (VALUE the new name).

use k2_mpc_blend::live::LiveLimits;
use k2_mpc_blend::{write, Change, Edit};

fn floats(v: &str) -> [f32; 3] {
    let p: Vec<f32> = v.split(',').map(|x| x.parse().expect("a number")).collect();
    [p[0], p[1], p[2]]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (input, output, rest) = (&args[0], &args[1], &args[2..]);
    assert!(
        rest.len().is_multiple_of(3),
        "edits come as KEY FIELD VALUE"
    );
    let edits: Vec<Edit> = rest
        .chunks(3)
        .map(|e| {
            let b = |v: &str| Some(v == "true");
            let change = match e[1].as_str() {
                "loc" => Change::Location(floats(&e[2])),
                "rot" => Change::Rotation(floats(&e[2])),
                "scale" => Change::Scale(floats(&e[2])),
                "hide_viewport" => Change::Visibility {
                    viewport: b(&e[2]),
                    select: None,
                    render: None,
                },
                "hide_select" => Change::Visibility {
                    viewport: None,
                    select: b(&e[2]),
                    render: None,
                },
                "hide_render" => Change::Visibility {
                    viewport: None,
                    select: None,
                    render: b(&e[2]),
                },
                "name" => Change::Name(e[2].clone()),
                other => panic!("no field {other}"),
            };
            Edit::new(e[0].clone(), change)
        })
        .collect();
    let bytes = std::fs::read(input).expect("the input file");
    let w = write(&bytes, &edits, &LiveLimits::default().read).expect("the writer writes");
    std::fs::write(output, &w.bytes).expect("the output file");
    println!(
        "patched {} field(s), {:?}: {input} -> {output}",
        w.patched.len(),
        w.compression
    );
}
