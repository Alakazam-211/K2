//! Applies fixed-size edits to a BLEND file with the writer: test tooling for the headless round
//! trip (`tests/roundtrip.rs`).
//!
//!     cargo run --example blend_patch -- IN OUT [KEY FIELD VALUE]...
//!
//! FIELD is `loc`, `rot` or `scale` (VALUE `x,y,z`), `hide_viewport`, `hide_select` or
//! `hide_render` (VALUE `true` or `false`), or `name` (VALUE the new name).
//!
//! A refusal writes no file: it prints `refused: <code>: <why>` and `next: <step>` (the file
//! verbs' registered entry, `blend_driven` for a channel an action or a driver animates) and
//! exits with the verbs' exit (2). A write's warnings (`blend_constrained`) go to stderr, one
//! `note: ` line each.

use k2_mpc_blend::live::LiveLimits;
use k2_mpc_blend::verbs::VerbError;
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
    let w = match write(&bytes, &edits, &LiveLimits::default().read) {
        Ok(w) => w,
        Err(e) => {
            let e = VerbError::from(e);
            println!("refused: {}: {e}", e.code().unwrap_or("-"));
            if let Some(step) = e.entry().and_then(|x| x.step) {
                println!("next: {}: {}", step.verb, step.why);
            }
            std::process::exit(i32::from(e.exit()));
        }
    };
    std::fs::write(output, &w.bytes).expect("the output file");
    for warning in &w.warnings {
        eprintln!("{}", warning.note());
    }
    println!(
        "patched {} field(s), {:?}: {input} -> {output}",
        w.patched.len(),
        w.compression
    );
}
