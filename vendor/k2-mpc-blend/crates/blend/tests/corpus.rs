//! The public corpus (`tests/corpus/manifest.tsv`, fetched by `scripts/fetch-corpus.sh` into the
//! folder `BLEND_PUBLIC_CORPUS` names; it fails when that is unset): CC0 demo files from 2.73 to
//! 5.01, in both header forms and all three ways a file is stored. Every file reads, streamed from
//! disk; holds what an independent reader says it holds (`tests/corpus/expected.tsv`, from
//! `scripts/corpus-expected.py`, a Python test checker that shares no code with the pack); indexes
//! into a well-formed draft, the same twice; keeps the fuzz properties; and, cut short anywhere, is
//! refused.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use k2_mpc_base::index::{IndexLimits, NodeKey, INDEX_TEXT_MAX};
use k2_mpc_blend::{
    fuzzing, index_blend, read_blend, read_scene, BlendFile, Detail, HeaderForm, ReadError,
    ReadLimits, SceneModel,
};

const MANIFEST: &str = include_str!("corpus/manifest.tsv");
const EXPECTED: &str = include_str!("corpus/expected.tsv");

fn names() -> Vec<&'static str> {
    MANIFEST
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split('\t').next().unwrap())
        .collect()
}

/// The rows the checker prints, from the pack's model.
fn rows(file: &BlendFile, m: &SceneModel) -> Vec<String> {
    let h = &file.header;
    let mut out = vec![
        format!("version\t{}", h.version_text()),
        format!(
            "form\t{}",
            if h.form == HeaderForm::Classic {
                "classic"
            } else {
                "large"
            }
        ),
        format!("compression\t{}", file.compression.as_str()),
        format!("pointer_size\t{}", h.pointer_size),
    ];
    let key = |i: usize| m.ids[i].key.clone().unwrap_or_else(|| "-".into());
    let keys = |v: &[usize]| v.iter().map(|&i| key(i)).collect::<Vec<_>>().join(" ");
    for (i, id) in m.ids.iter().enumerate() {
        if id.ui {
            continue;
        }
        out.push(format!("id\t{}", key(i)));
        match &id.detail {
            Detail::Object { data, parent } if id.struct_name == "Object" => {
                let loc = id
                    .fields
                    .iter()
                    .find(|(n, _)| n == "loc")
                    .map(|(_, v)| {
                        v.split(',')
                            .map(|x| format!("{:08x}", x.parse::<f32>().unwrap().to_bits()))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                out.push(format!(
                    "object\t{}\tdata={}\tparent={}\tloc={loc}",
                    key(i),
                    data.map_or("-".into(), key),
                    parent.map_or("-".into(), key),
                ));
            }
            Detail::Scene {
                objects,
                collections,
                ..
            }
            | Detail::Collection {
                objects,
                collections,
            } => out.push(format!(
                "holds\t{}\tobjects={}\tcollections={}",
                key(i),
                keys(objects),
                keys(collections)
            )),
            _ => {}
        }
    }
    out
}

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn every_public_file_reads_as_the_checker_says() {
    let dir = std::env::var_os("BLEND_PUBLIC_CORPUS")
        .map(PathBuf::from)
        .expect("BLEND_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    let mut expected: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for l in EXPECTED.lines() {
        let (file, row) = l.split_once('\t').expect("a file, then the row");
        expected.entry(file).or_default().push(row.to_owned());
    }
    let limits = IndexLimits::new(200_000, 64, 256 << 20, INDEX_TEXT_MAX);
    for name in names() {
        let path = dir.join(name);
        let open = || BufReader::new(File::open(&path).unwrap_or_else(|e| panic!("{name}: {e}")));
        let file = read_blend(&mut open(), &ReadLimits::default())
            .unwrap_or_else(|e| panic!("check: {name} reads: {e}"));
        let model = read_scene(&file).unwrap_or_else(|e| panic!("check: {name} models: {e}"));
        let mut got = rows(&file, &model);
        let mut want = expected
            .remove(name)
            .unwrap_or_else(|| panic!("no rows for {name}"));
        got.sort();
        want.sort();
        assert_eq!(got, want, "{name}: the pack and the checker agree");
        assert_eq!(
            model.unresolved, 0,
            "{name}: every pointer followed named one block"
        );
        // the index: well formed, the same twice
        let draft = index_blend(&mut open(), &limits)
            .unwrap_or_else(|e| panic!("check: {name} indexes: {e}"));
        assert_eq!(
            draft,
            index_blend(&mut open(), &limits).unwrap(),
            "{name}: deterministic"
        );
        assert_eq!(
            draft.nodes.len(),
            model.ids.iter().filter(|i| !i.ui).count()
        );
        for (i, n) in draft.nodes.iter().enumerate() {
            assert!(
                n.parent.is_none_or(|p| (p as usize) < i),
                "{name}: parents first"
            );
            assert!(n.key.as_deref().is_none_or(|k| NodeKey::parse(k).is_some()));
        }
        let stable = model.ids.iter().filter(|i| i.content_stable).count();
        eprintln!(
            "{name}: version {} {:?} {} blocks={} data_blocks={} nodes={} stable_digests={stable} shared_addresses={}",
            file.header.version_text(),
            file.header.form,
            file.compression.as_str(),
            file.blocks.len(),
            model.ids.len(),
            draft.nodes.len(),
            file.shared_addresses()
        );
        // the fuzz properties, and refusals of the file cut short
        let bytes = std::fs::read(&path).unwrap();
        if bytes.len() <= 2 << 20 {
            fuzzing::properties(&bytes);
        }
        for cut in [
            7,
            13,
            100,
            bytes.len() / 3,
            bytes.len() / 2,
            bytes.len() - 1,
        ] {
            match read_blend(&mut &bytes[..cut], &ReadLimits::default()) {
                Err(ReadError::Truncated { .. } | ReadError::Malformed { .. }) => {}
                Err(ReadError::NotBlend) if cut < 12 => {}
                other => panic!("{name} cut at {cut}: {:?}", other.map(|_| "read")),
            }
        }
    }
    assert!(
        expected.is_empty(),
        "rows for files not in the manifest: {:?}",
        expected.keys()
    );
}
