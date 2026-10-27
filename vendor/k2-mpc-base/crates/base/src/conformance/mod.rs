//! Primitive conformance suite: K2's merge semantics, as tests every engine must pass.
//!
//! The primitives are defined here, not by what an engine offers. The tests drive
//! several replicas only through [`crate::Doc`], so any engine behind the seam gets the
//! same tests. Where today's engine can't meet a rule, the deviation is listed in
//! [`KNOWN_DEVIATIONS`] with its reason, and the test asserts the deviation **still
//! happens**: fixing it (or swapping the engine) makes the test fail until the entry is
//! removed. A new engine must have an empty list.
//!
//! Text is the first kind with a suite; each later kind lands with its own module.

#[cfg(test)]
mod text;

/// (rule id, why today's engine deviates). Empty for a conforming engine.
///
/// One genuine entry for yrs today (`text.yata_separating_case`). An earlier entry, `text.no_interleave_backward_multi`, was wrong:
/// its expectation (three contiguous runs) is unsatisfiable for any engine meeting the
/// strong list spec on that script, and Yrs, Yjs, Diamond Types and Loro all give the same
/// order (cross-checked 2026-10-27). The real Yrs-vs-FugueMax separating case is now
/// `text.yata_separating_case`.
pub const KNOWN_DEVIATIONS: &[(&str, &str)] = &[
    (
        "text.yata_separating_case",
        "yrs (YATA) orders the separating script [pfae], splitting one writer's run `fe`; \
     FugueMax gives [apfe]. Wire-format bound: fixed only by a FugueMax-class engine.",
    ),
    (
        "text.yata_paper_a19",
        "yrs (YATA) orders the Fugue paper's A.1.9 example `axb` (as the paper reports for \
         Yjs); FugueMax gives `xab`. Wire-format bound.",
    ),
];

#[cfg(test)]
pub(crate) mod harness {
    use crate::text::diff;
    use crate::{ClientId, Doc, Edit, Schema};

    /// A set of replicas of one doc, synced by state-vector exchange (the engine's own
    /// encoding; the suite never looks at the bytes).
    pub struct Net {
        pub schema: Schema,
        pub r: Vec<Doc>,
    }

    impl Net {
        pub fn new(schema: Schema, n: usize, seed_text: &str) -> Self {
            let mut first = Doc::new(schema).unwrap();
            if !seed_text.is_empty() {
                first
                    .replace_text_as(2, schema.roots[0].0, seed_text)
                    .unwrap();
            }
            let state = first.encode_state().unwrap();
            let r = (0..n).map(|_| Doc::load(schema, &state).unwrap()).collect();
            Net { schema, r }
        }

        pub fn root(&self) -> &'static str {
            self.schema.roots[0].0
        }

        pub fn text(&self, i: usize) -> String {
            self.r[i].text(self.root()).unwrap()
        }

        /// Replica `i` edits as `client`: insert `s` at byte offset `at`.
        pub fn insert(&mut self, i: usize, client: ClientId, at: usize, s: &str) {
            let root = self.root();
            let h = vec![diff::Hunk {
                start: at,
                end: at,
                insert: s.into(),
            }];
            self.r[i]
                .edit_as(
                    client,
                    &[Edit::Text {
                        root: root.into(),
                        hunks: h,
                    }],
                )
                .unwrap();
        }

        /// Replica `i` edits as `client`: delete bytes `start..end`.
        pub fn delete(&mut self, i: usize, client: ClientId, start: usize, end: usize) {
            let root = self.root();
            let h = vec![diff::Hunk {
                start,
                end,
                insert: String::new(),
            }];
            self.r[i]
                .edit_as(
                    client,
                    &[Edit::Text {
                        root: root.into(),
                        hunks: h,
                    }],
                )
                .unwrap();
        }

        /// Deliver everything `from` has to `to`.
        pub fn sync(&mut self, from: usize, to: usize) {
            let u = self.r[from]
                .encode_diff(&self.r[to].state_vector())
                .unwrap();
            self.r[to].apply_trusted(&u).unwrap();
        }

        /// Everyone gets everything; then assert convergence.
        pub fn sync_all(&mut self) -> String {
            for a in 0..self.r.len() {
                for b in 0..self.r.len() {
                    if a != b {
                        self.sync(a, b);
                    }
                }
            }
            let t = self.text(0);
            for i in 1..self.r.len() {
                assert_eq!(self.text(i), t, "replica {i} diverged");
            }
            t
        }
    }

    /// Is `rule` a listed deviation for today's engine?
    pub fn deviates(rule: &str) -> bool {
        super::KNOWN_DEVIATIONS.iter().any(|(r, _)| *r == rule)
    }
}
