//! Writes every file this pack writes, for the external checkers (k2-mpc-ooxml's
//! `checkers/run.sh`, `docs/checkers.md`): the generated fixtures (stored and deflated), each
//! verb's output, and every merged result of the merge matrix, with `expect-pptx.tsv`: each
//! file's slide and picture counts as this pack reads them, which the package reader must
//! find too. Only with `PPTX_EMIT_DIR` set; otherwise it checks that each file opens and says
//! how many there are. Names are the row's; contents are synthetic.

mod common;

use std::path::PathBuf;

use common::limits;
use common::matrix::{run, OPS};
use k2_mpc_pptx::merge::Outcome;
use k2_mpc_pptx::testgen::{self, LAYOUT_CONTENT};
use k2_mpc_pptx::verbs::{apply, Edit, LayoutSel, Place};
use k2_mpc_pptx::Pptx;

fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (name, spec) in common::specs() {
        let parts = testgen::parts(&spec);
        out.push((
            format!("fixture-{name}-stored"),
            testgen::stored_zip(&parts),
        ));
        out.push((format!("fixture-{name}-deflated"), common::deflated(&parts)));
    }
    let b = testgen::deck(&testgen::sample());
    let edits = [
        (
            "slide-add",
            Edit::AddSlide {
                layout: LayoutSel::Id(LAYOUT_CONTENT),
                place: Some(Place::After(256)),
            },
        ),
        (
            "slide-move",
            Edit::MoveSlide {
                slide: 259,
                place: Place::First,
            },
        ),
        (
            "shape-add",
            Edit::AddTextBox {
                slide: 257,
                at: (100, 100, 2000000, 500000),
                text: "a\nb".into(),
            },
        ),
        (
            "shape-text",
            Edit::SetText {
                slide: 258,
                shape: 5,
                text: "now text".into(),
            },
        ),
        (
            "shape-move",
            Edit::MoveShape {
                slide: 257,
                shape: 2,
                x: Some(1),
                y: Some(2),
                w: Some(3),
                h: Some(4),
            },
        ),
        (
            "shape-delete",
            Edit::DeleteShape {
                slide: 258,
                shape: 8,
            },
        ),
        (
            "notes-edit",
            Edit::SetNotes {
                slide: 257,
                text: "edited".into(),
            },
        ),
        (
            "notes-new",
            Edit::SetNotes {
                slide: 256,
                text: "new".into(),
            },
        ),
    ];
    for (name, e) in edits {
        out.push((
            format!("verb-{name}"),
            apply(&b, &e, &limits()).expect("applies").bytes,
        ));
    }
    for (i, &o) in OPS.iter().enumerate() {
        for (j, &t) in OPS.iter().enumerate() {
            if let (_, _, Outcome::Merged(m)) = run(o, t) {
                out.push((format!("merge-{i:02}-{j:02}"), m.bytes));
            }
        }
    }
    out
}

#[test]
fn every_written_file_is_emitted_for_the_checkers() {
    let files = files();
    for (name, bytes) in &files {
        Pptx::open(bytes, &limits()).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    if let Some(dir) = std::env::var_os("PPTX_EMIT_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("the folder");
        let mut expect = String::new();
        for (name, bytes) in &files {
            std::fs::write(dir.join(format!("{name}.pptx")), bytes).expect("write");
            let p = Pptx::open(bytes, &limits()).expect("opens");
            let pictures = p
                .opc
                .parts
                .keys()
                .filter(|k| p.opc.types.of(k).is_some_and(|t| t.starts_with("image/")))
                .count();
            expect.push_str(&format!(
                "{name}.pptx\tslides={}\tpictures={pictures}\n",
                p.deck.slides.len()
            ));
        }
        std::fs::write(dir.join("expect-pptx.tsv"), expect).expect("write");
        println!("emitted: {} files to the checkers' folder", files.len());
    } else {
        println!(
            "emitted: {} files (none written: PPTX_EMIT_DIR unset)",
            files.len()
        );
    }
}
