//! The public corpus (`tests/corpus/manifest.tsv`, fetched by `scripts/fetch-corpus.sh` into the
//! folder `FAB_PUBLIC_CORPUS` names; it fails when that is unset): every file is read to the model
//! or declined with a typed reason, never a panic. A drill file is read strictly as XNC first,
//! else as a common export (reported not conforming, with the constructs it relied on). A file
//! that reads keeps the fuzz properties and indexes into a well-formed draft. The run prints each
//! file's outcome and the counts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_mpc_fab::{drill, fuzzing, photoplot, Limits};

const MANIFEST: &str = include_str!("corpus/manifest.tsv");

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn the_public_corpus_reads_or_declines_with_a_typed_reason() {
    let dir = std::env::var_os("FAB_PUBLIC_CORPUS")
        .map(PathBuf::from)
        .expect("FAB_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    let l = Limits::default();
    let mut reasons: BTreeMap<String, u64> = BTreeMap::new();
    let mut conformance: BTreeMap<String, u64> = BTreeMap::new();
    let (mut files, mut read, mut declined) = (0u64, 0u64, 0u64);
    let mut failures: Vec<String> = Vec::new();
    for line in MANIFEST
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split('\t').collect();
        let (name, kind) = (f[0], f[1]);
        let path = dir.join(name.replace('/', "_"));
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{name}: not fetched ({e})"));
                continue;
            }
        };
        files += 1;
        let outcome = std::panic::catch_unwind(|| match kind {
            "photoplot" => {
                let r = photoplot::read(&bytes, &l).map(|m| {
                    let d = k2_mpc_fab::index_photoplot(
                        &m,
                        &k2_mpc_base::index::IndexLimits::new(
                            1_000_000,
                            64,
                            1 << 30,
                            k2_mpc_base::index::INDEX_TEXT_MAX,
                        ),
                    );
                    (
                        m.warnings.conformance().name().to_owned(),
                        format!(
                            "{} objects, {} apertures, {} warnings, {} nodes",
                            m.objects.len(),
                            m.apertures.len(),
                            m.warnings.total(),
                            d.map_or(0, |d| d.nodes.len())
                        ),
                    )
                });
                fuzzing::photoplot(&bytes);
                r
            }
            _ => {
                // strict XNC first; else the common-export reading, reported not conforming
                let summary = |m: &drill::Drill| {
                    format!(
                        "{} tools, {} hits, {} routs",
                        m.tools.len(),
                        m.hits.len(),
                        m.routs.len()
                    )
                };
                let r = match drill::read(&bytes, &l) {
                    Ok(m) => Ok(("strict current".to_owned(), summary(&m))),
                    Err(strict) => match drill::common::read(&bytes, &l) {
                        Ok(c) => {
                            let relied: Vec<&str> = c.relied.keys().map(|k| k.name()).collect();
                            Ok((
                                format!("common {}", c.conformance().name()),
                                format!(
                                    "{}; relied on {}; strict refused {}",
                                    summary(&c.drill),
                                    relied.join(","),
                                    strict.summary()
                                ),
                            ))
                        }
                        Err(e) => {
                            println!("corpus {name}: strict refused {strict}");
                            Err(e)
                        }
                    },
                };
                fuzzing::drill(&bytes);
                fuzzing::drill_common(&bytes);
                r
            }
        });
        match outcome {
            Ok(Ok((conf, what))) => {
                read += 1;
                *conformance.entry(format!("{kind} {conf}")).or_insert(0) += 1;
                println!("corpus {name}: read ({conf}; {what})");
            }
            Ok(Err(e)) => {
                declined += 1;
                *reasons
                    .entry(format!("{kind} {}", e.summary()))
                    .or_insert(0) += 1;
                println!("corpus {name}: declined {e}");
            }
            Err(_) => failures.push(format!("{name}: panicked")),
        }
    }
    for (k, v) in &conformance {
        println!("corpus read as: {k}: {v}");
    }
    for (k, v) in &reasons {
        println!("corpus declined: {k}: {v}");
    }
    println!(
        "public corpus: {files} files, {read} read, {declined} declined, {} failures",
        failures.len()
    );
    assert!(failures.is_empty(), "corpus failures: {failures:?}");
    assert!(files > 0, "no corpus files");
}
