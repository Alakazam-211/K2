//! The merge matrix: every pair of one agent edit and one person edit on the sample deck
//! (13 x 13), checked against the oracle in `common::matrix`: both land, or the person's lands
//! and the agent's is held at the predicted address, or the kit keeps both. Every merged
//! result opens, indexes, and reads back with each edit as its side made it.

mod common;

use common::limits;
use common::matrix::{expect, run, shows, Expect, Who, OPS};
use k2_mpc_base::index::IndexLimits;
use k2_mpc_base::session::BlobMerge;
use k2_mpc_pptx::index::draft;
use k2_mpc_pptx::merge::{merge3, Outcome};
use k2_mpc_pptx::{Pptx, PptxPack, Since};

#[test]
fn every_pair_of_edits_merges_as_the_rules_say() {
    let mut counts = [0usize; 3];
    let mut failures = Vec::new();
    for &o in OPS {
        for &t in OPS {
            let want = expect(o, t);
            let (_, _, out) = run(o, t);
            let got = match (&want, &out) {
                (Expect::Whole, Outcome::Conflict(_)) => {
                    counts[2] += 1;
                    Ok(())
                }
                (Expect::Both, Outcome::Merged(m)) => {
                    counts[0] += 1;
                    if !m.conflicts.is_empty() {
                        Err(format!(
                            "held {:?}",
                            m.conflicts.iter().map(|c| &c.address).collect::<Vec<_>>()
                        ))
                    } else if !shows(&m.bytes, o, Who::Agent) {
                        Err("the agent's edit is missing".into())
                    } else if !shows(&m.bytes, t, Who::Person) {
                        Err("the person's edit is missing".into())
                    } else {
                        Ok(())
                    }
                }
                (Expect::Held(addr), Outcome::Merged(m)) => {
                    counts[1] += 1;
                    let held: Vec<&str> = m.conflicts.iter().map(|c| c.address.as_str()).collect();
                    if held != [addr.as_str()] {
                        Err(format!("held {held:?}, not [{addr}]"))
                    } else if !shows(&m.bytes, t, Who::Person) {
                        Err("the person's edit isn't in the file".into())
                    } else if shows(&m.bytes, o, Who::Agent) {
                        Err("the agent's edit is in the file".into())
                    } else {
                        Ok(())
                    }
                }
                (w, Outcome::Merged(_)) => Err(format!("merged, expected {w:?}")),
                (w, Outcome::Conflict(why)) => Err(format!("conflict ({why}), expected {w:?}")),
                (_, Outcome::Unsupported(d)) => Err(format!("unsupported: {d}")),
            };
            if let Outcome::Merged(m) = &out {
                let r = Pptx::open(&m.bytes, &limits())
                    .map_err(|d| d.to_string())
                    .and_then(|p| {
                        draft(
                            &p,
                            &IndexLimits::new(100_000, 64, 64 << 20, 64 << 10),
                            &limits(),
                        )
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                    });
                if let Err(e) = r {
                    failures.push(format!("{o:?} x {t:?}: the result doesn't read: {e}"));
                }
            }
            if let Err(e) = got {
                failures.push(format!("{o:?} x {t:?}: {e}"));
            }
        }
    }
    println!(
        "merge matrix: {} pairs: {} both land, {} held, {} whole-file conflicts",
        OPS.len() * OPS.len(),
        counts[0],
        counts[1],
        counts[2]
    );
    assert!(
        failures.is_empty(),
        "{} pairs failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(counts.iter().all(|c| *c > 0));
}

/// Over the whole matrix: through the pack (told the save's base), a pair that
/// merges with nothing held is the same file; a held pair is a whole-file conflict, holding
/// nothing in memory, until the kit's durable held conflicts land.
#[test]
fn through_the_pack_a_held_pair_is_a_whole_file_conflict() {
    let base = common::matrix::base();
    let mut held = 0;
    let mut failures = Vec::new();
    for &o in OPS {
        for &t in OPS {
            let (ob, tb, out) = run(o, t);
            let pack = PptxPack::default();
            let got = pack.merge3_since(&base, &ob, &tb, Since::NoKitWrite);
            let want = match out {
                Outcome::Merged(m) if m.conflicts.is_empty() => BlobMerge::Merged(m.bytes),
                Outcome::Merged(_) => {
                    held += 1;
                    BlobMerge::Conflict
                }
                Outcome::Conflict(_) => BlobMerge::Conflict,
                Outcome::Unsupported(_) => BlobMerge::Unsupported,
            };
            if got != want || !pack.conflicts.list().is_empty() {
                failures.push(format!("{o:?} x {t:?}"));
            }
        }
    }
    println!("through the pack: {held} held pairs, each a whole-file conflict");
    assert!(held > 0);
    assert!(
        failures.is_empty(),
        "{} pairs not as expected:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_merge_is_a_pure_function() {
    for &o in OPS.iter().step_by(3) {
        for &t in OPS.iter().step_by(2) {
            let (ob, tb, first) = run(o, t);
            let again = merge3(&common::matrix::base(), &ob, &tb, &limits());
            assert_eq!(first, again, "{o:?} x {t:?}");
        }
    }
}
