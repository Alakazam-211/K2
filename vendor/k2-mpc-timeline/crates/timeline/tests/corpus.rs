//! The public corpus (`tests/corpus/manifest.tsv`, fetched by `scripts/fetch-corpus.sh` into the
//! folder `TIMELINE_PUBLIC_CORPUS` names; it fails when that is unset, and scripts/public-corpus.sh
//! runs it): every file reads, keeps the fuzz properties, indexes into a well-formed draft, and
//! holds what an independent count says it holds (`tests/corpus/expected.tsv`, from
//! `scripts/corpus-expected.py`, a Python test checker that shares no code with the readers).

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_mpc_base::index::{IndexLimits, NodeKey, INDEX_TEXT_MAX};
use k2_mpc_timeline::model::{ItemKind, Timeline};
use k2_mpc_timeline::{fuzzing, index_document, Document, ReadError, ReadLimits};

type Reader = fn(&[u8], &ReadLimits) -> Result<Document, ReadError>;

const MANIFEST: &str = include_str!("corpus/manifest.tsv");
const EXPECTED: &str = include_str!("corpus/expected.tsv");

fn counts(name: &str, t: &Timeline) -> Vec<u64> {
    let n = |k: &[ItemKind]| -> u64 {
        t.tracks
            .iter()
            .flat_map(|tr| &tr.items)
            .filter(|i| k.contains(&i.kind))
            .count() as u64
    };
    if name.ends_with(".otio") {
        let markers: u64 = t
            .tracks
            .iter()
            .flat_map(|tr| &tr.items)
            .filter(|i| matches!(i.kind, ItemKind::Clip | ItemKind::Generator))
            .map(|i| i.markers.len() as u64)
            .sum();
        vec![
            t.tracks.len() as u64,
            n(&[ItemKind::Clip, ItemKind::Generator]),
            n(&[ItemKind::Transition]),
            markers,
        ]
    } else if name.ends_with(".fcpxml") {
        let primary = t.tracks.iter().find(|tr| tr.name == "lane 0");
        vec![primary.map_or(0, |tr| tr.items.len() as u64)]
    } else {
        let clips = t
            .tracks
            .iter()
            .flat_map(|tr| &tr.items)
            .filter(|i| i.kind != ItemKind::Transition)
            .count() as u64;
        vec![
            t.tracks.len() as u64,
            clips,
            n(&[ItemKind::Transition]),
            t.markers.len() as u64,
        ]
    }
}

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn the_public_corpus_reads_and_matches_an_independent_count() {
    let dir = std::env::var_os("TIMELINE_PUBLIC_CORPUS")
        .map(PathBuf::from)
        .expect("TIMELINE_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    let mut expected: BTreeMap<(String, usize), Vec<u64>> = BTreeMap::new();
    for line in EXPECTED
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split('\t').collect();
        assert!(f[1] != "-", "the checker couldn't read {}", f[0]);
        let nums = f[2..].iter().map(|x| x.parse().unwrap()).collect();
        expected.insert((f[0].to_owned(), f[1].parse().unwrap()), nums);
    }
    let il = IndexLimits::new(1_000_000, 64, 1 << 30, INDEX_TEXT_MAX);
    let (mut files, mut timelines, mut items, mut nodes) = (0, 0, 0u64, 0usize);
    // every file is checked; the failures are reported together at the end
    let mut failures: Vec<String> = Vec::new();
    for line in MANIFEST
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let name = line.split('\t').next().unwrap();
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (read, props): (Reader, fn(&[u8])) = if name.ends_with(".otio") {
            (k2_mpc_timeline::otio::read_otio, fuzzing::otio)
        } else if name.ends_with(".fcpxml") {
            (k2_mpc_timeline::fcpxml::read_fcpxml, fuzzing::fcpxml)
        } else {
            (k2_mpc_timeline::prproj::read_prproj, fuzzing::prproj)
        };
        let doc: Document = match read(&bytes, &ReadLimits::default()) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{name}: refused: {e}"));
                continue;
            }
        };
        props(&bytes);
        let draft = match index_document(&doc, &il) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{name}: index refused: {e}"));
                continue;
            }
        };
        for (i, n) in draft.nodes.iter().enumerate() {
            assert!(
                n.parent.is_none_or(|p| (p as usize) < i),
                "{name}: node {i}"
            );
            if let Some(k) = &n.key {
                assert!(NodeKey::parse(k).is_some(), "{name}: key {k}");
            }
        }
        for (n, t) in doc.timelines.iter().enumerate() {
            let got = counts(name, t);
            match expected.get(&(name.to_owned(), n + 1)) {
                Some(want) if *want == got => {}
                want => failures.push(format!(
                    "{name}: timeline {}: counted {got:?}, the checker {want:?}",
                    n + 1
                )),
            }
            items += t.tracks.iter().map(|tr| tr.items.len() as u64).sum::<u64>();
        }
        let rows = expected.keys().filter(|(f, _)| f == name).count();
        if doc.timelines.len() != rows {
            failures.push(format!(
                "{name}: {} timelines, the checker {rows}",
                doc.timelines.len()
            ));
        }
        files += 1;
        timelines += doc.timelines.len();
        nodes += draft.nodes.len();
    }
    assert!(
        failures.is_empty(),
        "public corpus: {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "public corpus: {files} files, {timelines} timelines, {items} items, {nodes} index nodes"
    );
}
