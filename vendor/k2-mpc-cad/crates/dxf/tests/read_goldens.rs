//! The reads as they were: every existing read's text and typed data on our drawings, byte for
//! byte against files written at the commit before a change to the reads. A read that changes
//! its output fails here until its golden is written again, on purpose, in its own commit.
//!
//! `GOLDEN_WRITE=1` writes the files instead (in a job, at the commit whose output they pin).

use k2_mpc_dxf::testgen::{self, Ver};
use k2_mpc_dxf::verbs;

/// Each drawing, and each read with its arguments.
fn cases() -> Vec<(String, Vec<u8>, &'static str, Vec<&'static str>)> {
    let drawings = [
        ("sampler-2018", testgen::sampler(Ver::R2018).build().bytes),
        ("sampler-r12", testgen::sampler(Ver::R12).build().bytes),
        (
            "nested-2010",
            testgen::nested_blocks(Ver::R2010).build().bytes,
        ),
        (
            "h1-2018",
            testgen::h1("h1-r2018").expect("a drawing").build().bytes,
        ),
    ];
    let reads: [(&str, Vec<&str>); 9] = [
        ("drawing info", vec![]),
        ("layer list", vec![]),
        ("block list", vec!["--anon"]),
        ("entity find", vec!["--in", "all", "--limit", "500"]),
        ("entity find", vec!["--crossing", "-1000,-1000,1000,1000"]),
        ("text find", vec!["--contains", "o"]),
        ("header get", vec!["$ACADVER", "$INSUNITS", "$HANDSEED"]),
        ("check", vec!["--writeback"]),
        ("layer list", vec!["--unused"]),
    ];
    let mut out = Vec::new();
    for (name, bytes) in &drawings {
        for (k, (verb, args)) in reads.iter().enumerate() {
            out.push((format!("{name}-{k}"), bytes.clone(), *verb, args.clone()));
        }
    }
    out
}

fn render(bytes: &[u8], verb: &str, args: &[&str]) -> String {
    match verbs::run(bytes, verb, args) {
        Ok(o) => format!(
            "spec {}/{}\nnext {:?}\n--- stdout\n{}--- data\n{}\n",
            o.spec.0,
            o.spec.1,
            o.next,
            o.stdout,
            o.data.to_json_pretty()
        ),
        Err(e) => format!("refused {}\n", e.summary()),
    }
}

#[test]
pub fn reads_match_their_goldens() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let write = std::env::var("GOLDEN_WRITE").as_deref() == Ok("1");
    let mut compared = 0;
    for (name, bytes, verb, args) in cases() {
        let got = render(&bytes, verb, &args);
        let path = dir.join(format!("{name}.txt"));
        if write {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "no golden {}: write them at the commit before",
                path.display()
            )
        });
        assert!(
            got == want,
            "{verb} {args:?} on {name} differs from its golden"
        );
        compared += 1;
    }
    assert!(write || compared == 36, "only {compared} reads compared");
    println!("reads_match_their_goldens: {compared} reads byte for byte");
}
