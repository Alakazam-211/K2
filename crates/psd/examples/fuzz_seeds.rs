//! Writes the fuzz seed corpus (generated files only, nothing from the outside) under
//! `<out dir>/<target>/corpus/`: `cargo run -p k2-mpc-psd --example fuzz_seeds -- <out dir>`. The
//! fuzz targets that read it are on the repository's main branch.

use std::path::{Path, PathBuf};

use photocraft_psd::descriptor::{Descriptor, UnicodeString, Value, VersionedDescriptor};
use photocraft_psd::{testgen, GroupSpec, LayerSpec, PixelData, PsdBuilder, TaggedBlock};

const TARGETS: &[&str] = &["records_walk", "records_diff", "records_limits"];
/// `index_draft` reads a read size and two caps first: a 64-byte step, the most nodes, depth 64.
const INDEX_PREFIX: [u8; 3] = [63, 255, 64];

fn write(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).expect("create seed directory");
    std::fs::write(dir.join(name), bytes).expect("write seed");
}

/// Nested groups with layer IDs, a text layer, a smart object and a fill, from the builder.
fn layered_with_blocks() -> Vec<u8> {
    let text = {
        let mut d = 1u16.to_be_bytes().to_vec();
        d.extend([0u8; 48]);
        d.extend(50u16.to_be_bytes());
        d.extend(
            VersionedDescriptor::new(
                Descriptor::new("TxLr").with("Txt ", Value::Text(UnicodeString::new("Seed"))),
            )
            .to_bytes(),
        );
        d
    };
    let placed = {
        let mut d = b"soLD".to_vec();
        d.extend(4u32.to_be_bytes());
        d.extend(
            VersionedDescriptor::new(
                Descriptor::new("null")
                    .with("Idnt", Value::Text(UnicodeString::new("seed-id")))
                    .with("Trnf", Value::List(vec![Value::Double(0.0); 8])),
            )
            .to_bytes(),
        );
        d
    };
    let px = |n: usize| PixelData::Rgba8(vec![7; n * 4]);
    let with = |mut l: LayerSpec, id: u32, extra: Vec<TaggedBlock>| {
        l.extra_blocks
            .push(TaggedBlock::new(*b"lyid", id.to_be_bytes().to_vec()));
        l.extra_blocks.extend(extra);
        l
    };
    let mut b = PsdBuilder::new(4, 3);
    b.push_layer(with(LayerSpec::new("base", 0, 0, 2, 2, px(4)), 1, vec![]));
    b.begin_group(GroupSpec::new("outer"));
    b.push_layer(with(
        LayerSpec::new("title", 0, 0, 1, 1, px(1)),
        2,
        vec![TaggedBlock::new(*b"TySh", text)],
    ));
    b.begin_group(GroupSpec::new("inner"));
    b.push_layer(with(
        LayerSpec::new("logo", 1, 1, 2, 2, px(4)),
        3,
        vec![TaggedBlock::new(*b"SoLd", placed)],
    ));
    b.push_layer(with(
        LayerSpec::new("fill", 0, 0, 4, 3, px(12)),
        3,
        vec![TaggedBlock::new(*b"SoCo", vec![0; 4])],
    ));
    b.end_group().expect("the group closes");
    b.end_group().expect("the group closes");
    b.composite(px(12));
    b.to_bytes().expect("the builder's file serializes")
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: fuzz_seeds <out dir>"),
    );
    let mut n = 0usize;
    let index_dir = out.join("index_draft").join("corpus");
    for case in testgen::all_cases() {
        let bytes = case.file.to_bytes().expect("generated files serialize");
        if bytes.len() > 1 << 16 {
            continue;
        }
        for t in TARGETS {
            write(&out.join(t).join("corpus"), &format!("gen-{n:04}"), &bytes);
        }
        let mut seed = INDEX_PREFIX.to_vec();
        seed.extend_from_slice(&bytes);
        write(&index_dir, &format!("gen-{n:04}"), &seed);
        n += 1;
    }
    let mut seed = INDEX_PREFIX.to_vec();
    seed.extend(layered_with_blocks());
    write(&index_dir, "layered-blocks", &seed);
    println!(
        "seeds: {n} files written for {} targets, and {} for index_draft",
        TARGETS.len(),
        n + 1
    );
}
