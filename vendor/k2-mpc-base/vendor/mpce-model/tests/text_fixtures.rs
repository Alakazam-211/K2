//! Fixtures for the text reference model, with K2's harness semantics: replica Rk edits as
//! client ids[k]; `ins(c, at, ch)` inserts at offset `at` of c's visible text; `sync(f, t)`
//! delivers every event f has that t lacks; a script ends with a full sync over every
//! ordered pair (Ra, Rb), a != b, in replica order. A seed text is authored by client 2.
//! Expected values: oracles/scripts/fixed.txt (Diamond Types 1.0 / Loro) and the Fugue
//! paper; the same fixtures as spec/quint/text_fixtures.qnt.

use mpce_model::check::{def4, def4_with, strong_list_spec, Def4Violation};
use mpce_model::text::{Doc, Id};
use mpce_model::tree::SiblingRule;
use std::collections::BTreeMap;

struct Net {
    ids: Vec<u64>,
    reps: BTreeMap<u64, Doc>,
}

impl Net {
    fn new(ids: &[u64], seed: &str, rule: SiblingRule) -> Net {
        let mut s = Doc::with_rule(2, rule);
        for (i, c) in seed.chars().enumerate() {
            s.insert(i, c).unwrap();
        }
        let mut reps = BTreeMap::new();
        for &c in ids {
            let mut d = Doc::with_rule(c, rule);
            d.sync_from(&s).unwrap();
            reps.insert(c, d);
        }
        Net { ids: ids.to_vec(), reps }
    }
    fn ins(&mut self, c: u64, at: usize, ch: char) {
        self.reps.get_mut(&c).unwrap().insert(at, ch).unwrap();
        self.check_all();
    }
    fn del(&mut self, c: u64, at: usize) {
        self.reps.get_mut(&c).unwrap().delete(at).unwrap();
        self.check_all();
    }
    fn sync(&mut self, f: u64, t: u64) {
        let from = self.reps[&f].clone();
        self.reps.get_mut(&t).unwrap().sync_from(&from).unwrap();
        self.check_all();
    }
    fn full_sync(&mut self) {
        for a in self.ids.clone() {
            for b in self.ids.clone() {
                if a != b {
                    self.sync(a, b);
                }
            }
        }
    }
    fn text(&self, c: u64) -> String {
        self.reps[&c].text()
    }
    /// every replica state satisfies Def. 4, and all of them together the strong list spec
    fn check_all(&self) {
        let mut origins = BTreeMap::new();
        for d in self.reps.values() {
            origins.extend(d.origins());
            assert_eq!(def4(&d.list_with_tombstones(), &d.origins()), Ok(()), "Def. 4 on R{:?}", d.agent());
        }
        let lists: Vec<Vec<Id>> = self.reps.values().map(|d| d.list_with_tombstones()).collect();
        assert_eq!(strong_list_spec(&lists, &origins), Ok(()));
    }
    fn converged(&self) -> String {
        let t = self.text(self.ids[0]);
        for &c in &self.ids {
            assert_eq!(self.text(c), t, "R{c} diverged");
        }
        t
    }
}

fn fm(ids: &[u64], seed: &str) -> Net {
    Net::new(ids, seed, SiblingRule::FugueMax)
}

#[test]
fn yata_separating_case() {
    // R0, R1, R2 = 102, 103, 101; Yrs/Yjs give [pfae]
    let mut n = fm(&[102, 103, 101], "[]");
    n.ins(103, 1, 'e');
    n.sync(103, 101);
    n.ins(102, 1, 'a');
    n.ins(101, 1, 'p');
    n.sync(101, 103);
    n.ins(103, 2, 'f');
    n.full_sync();
    assert_eq!(n.converged(), "[apfe]");
}

#[test]
fn yata_paper_a19() {
    // Fugue paper Appendix A.1.9; YATA gives axb
    let mut n = fm(&[101, 102, 103], "");
    n.ins(103, 0, 'b');
    n.sync(103, 101);
    n.ins(101, 0, 'a');
    n.ins(102, 0, 'x');
    n.sync(102, 101);
    assert_eq!(n.text(101), "xab");
    n.full_sync();
    assert_eq!(n.converged(), "xab");
}

/// The kit's three-writer script (text.rs:148) with writer ids `ids` (R0, R1, R2).
fn three_writer(ids: [u64; 3]) -> String {
    let mut n = fm(&ids, "[]");
    let runs = ["cba", "fed", "zyx"];
    for k in 0..3 {
        for (r, run) in runs.iter().enumerate() {
            n.ins(ids[r], 1, run.chars().nth(k).unwrap());
        }
        if k == 0 {
            n.sync(ids[0], ids[1]);
        }
        if k == 1 {
            n.sync(ids[2], ids[0]);
        }
    }
    n.full_sync();
    n.converged()
}

#[test]
fn three_writer_all_permutations() {
    // fixed.txt: Yrs, Yjs, DT and Loro agree on all six
    assert_eq!(three_writer([101, 102, 103]), "[abdecfxyz]");
    assert_eq!(three_writer([101, 103, 102]), "[abdecxyzf]");
    assert_eq!(three_writer([102, 101, 103]), "[defabcxyz]");
    assert_eq!(three_writer([102, 103, 101]), "[xayzbdecf]");
    assert_eq!(three_writer([103, 101, 102]), "[defxayzbc]");
    assert_eq!(three_writer([103, 102, 101]), "[xayzdefbc]");
}

#[test]
fn paper_fig6() {
    // "the final list order must be AXBC"; allowed only through Lemma 5's exception
    let mut n = fm(&[101, 102, 103], "");
    n.ins(101, 0, 'A');
    n.ins(102, 0, 'B');
    n.ins(103, 0, 'C');
    n.sync(103, 101);
    assert_eq!(n.text(101), "AC");
    n.ins(101, 1, 'X');
    assert_eq!(n.text(101), "AXC");
    n.full_sync();
    assert_eq!(n.converged(), "AXBC");
    let d = &n.reps[&101];
    assert_eq!(def4(&d.list_with_tombstones(), &d.origins()), Ok(()));
    assert!(matches!(def4_with(&d.list_with_tombstones(), &d.origins(), false), Err(Def4Violation::Backward { .. })));
}

#[test]
fn paper_fig7() {
    // "the only list order that satisfies maximal non-interleaving in this example is AXYBC"
    let mut n = fm(&[101, 102, 103], "");
    n.ins(101, 0, 'A');
    n.ins(102, 0, 'B');
    n.ins(103, 0, 'C');
    n.sync(101, 102);
    assert_eq!(n.text(102), "AB");
    n.sync(103, 101);
    assert_eq!(n.text(101), "AC");
    n.ins(101, 1, 'X');
    n.ins(102, 1, 'Y');
    n.full_sync();
    assert_eq!(n.converged(), "AXYBC");
}

/// fixed.txt accept-clean-dtonly (R0, R1, R2 = 101, 102, 103).
fn dtonly(rule: SiblingRule) -> Net {
    let mut n = Net::new(&[101, 102, 103], "[]", rule);
    if rule == SiblingRule::Fugue {
        // plain Fugue violates Def. 4 here, so the same script runs without per-step checks
        for (c, at, ch) in [(103, 1, 'p'), (102, 1, 'e')] {
            n.reps.get_mut(&c).unwrap().insert(at, ch).unwrap();
        }
        let r102 = n.reps[&102].clone();
        n.reps.get_mut(&103).unwrap().sync_from(&r102).unwrap();
        let r103 = n.reps[&103].clone();
        n.reps.get_mut(&101).unwrap().sync_from(&r103).unwrap();
        n.reps.get_mut(&102).unwrap().insert(2, 'f').unwrap();
        n.reps.get_mut(&101).unwrap().insert(2, 'a').unwrap();
        for a in [101, 102, 103] {
            for b in [101, 102, 103] {
                if a != b {
                    let from = n.reps[&a].clone();
                    n.reps.get_mut(&b).unwrap().sync_from(&from).unwrap();
                }
            }
        }
    } else {
        n.ins(103, 1, 'p');
        n.ins(102, 1, 'e');
        n.sync(102, 103);
        n.sync(103, 101);
        n.ins(102, 2, 'f');
        n.ins(101, 2, 'a');
        n.full_sync();
    }
    n
}

#[test]
fn fugue_vs_fuguemax_dtonly() {
    // DT gives [efap] (FugueMax); Loro and Yrs [eafp] (plain Fugue's order here)
    let max = dtonly(SiblingRule::FugueMax);
    assert_eq!(max.converged(), "[efap]");
    let fugue = dtonly(SiblingRule::Fugue);
    assert_eq!(fugue.converged(), "[eafp]");
    // the checker accepts FugueMax's order and rejects Fugue's on (2), not (1) or (3)
    let d = &fugue.reps[&101];
    assert!(matches!(def4(&d.list_with_tombstones(), &d.origins()), Err(Def4Violation::Backward { .. })));
}

#[test]
fn min_any1_and_accept_clean2() {
    // fixed.txt min-any1 (R0, R1, R2 = 103, 101, 102): dt/loro [pea]
    let mut n = fm(&[103, 101, 102], "[]");
    n.ins(103, 1, 'a');
    n.ins(102, 1, 'p');
    n.sync(103, 101);
    n.ins(101, 1, 'e');
    n.full_sync();
    assert_eq!(n.converged(), "[pea]");
    // fixed.txt accept-clean2 (R0, R1, R2 = 103, 102, 104): dt/loro [aeqp]
    let mut n = fm(&[103, 102, 104], "[]");
    n.ins(104, 1, 'p');
    n.ins(103, 1, 'a');
    n.sync(104, 102);
    n.ins(102, 1, 'e');
    n.sync(102, 104);
    n.ins(104, 2, 'q');
    n.full_sync();
    assert_eq!(n.converged(), "[aeqp]");
}

#[test]
fn deletes_and_redelivery() {
    let mut n = fm(&[101, 102], "[]");
    n.ins(101, 1, 'a');
    n.ins(101, 2, 'b');
    n.sync(101, 102);
    n.del(102, 1);
    assert_eq!(n.text(102), "[b]");
    n.ins(101, 2, 'c');
    assert_eq!(n.text(101), "[acb]");
    n.full_sync();
    assert_eq!(n.converged(), "[cb]");
    // re-delivering everything changes nothing
    let before: Vec<Vec<Id>> = n.reps.values().map(|d| d.list_with_tombstones()).collect();
    for c in [101u64, 102] {
        let log = n.reps[&c].log().to_vec();
        for e in &log {
            assert_eq!(n.reps.get_mut(&c).unwrap().deliver(e), Ok(false));
        }
    }
    let after: Vec<Vec<Id>> = n.reps.values().map(|d| d.list_with_tombstones()).collect();
    assert_eq!(before, after);
    assert_eq!(n.converged(), "[cb]");
}

#[test]
fn causal_delivery_is_enforced() {
    let mut a = Doc::new(101);
    let e1 = a.insert(0, 'x').unwrap();
    let e2 = a.insert(1, 'y').unwrap();
    let mut b = Doc::new(102);
    assert_eq!(b.deliver(&e2), Err(mpce_model::text::DocError::MissingDeps));
    assert_eq!(b.deliver(&e1), Ok(true));
    assert_eq!(b.deliver(&e2), Ok(true));
    assert_eq!(b.text(), "xy");
}

#[test]
fn checker_rejects_yata_orders() {
    // the separating case's state {[, ], e, p, a}: YATA orders it [pae], FugueMax [ape]
    let mut n = fm(&[102, 103, 101], "[]");
    n.ins(103, 1, 'e');
    n.sync(103, 101);
    n.ins(102, 1, 'a');
    n.ins(101, 1, 'p');
    n.sync(102, 101);
    let d = &n.reps[&101];
    assert_eq!(d.text(), "[ape]");
    let l = d.list_with_tombstones();
    let (open, close, a, p, e) = (l[0], l[4], l[1], l[2], l[3]);
    let yata = vec![open, p, a, e, close];
    assert!(matches!(def4(&yata, &d.origins()), Err(Def4Violation::Backward { .. })));
}
