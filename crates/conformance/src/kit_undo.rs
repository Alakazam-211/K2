//! [`Undo`] on the kit: its undo is a session's ([`k2_mpc_base::Session::undo`], the undo
//! spec), so the adapter opens a real session through the public API
//! (`kit_session`), on a two-root pack, with a store in a temporary directory.

use std::borrow::Cow;
use std::sync::Arc;

use k2_mpc_base::session::{
    ConflictRegion, Declined, Pack, PackContent, PackError, UndoMode, UndoOutcome, UndoRequest,
};
use k2_mpc_base::{Edit, Kind, Neutral, Schema};

use crate::caps::{Applied, ElemBlame, PartialUndo, Region, Undo, UndoRefusal};
use crate::kit_session::KitDoc;
use crate::yrs::YrsText;
use crate::{Hunk, Refused};

const ROOTS: [&str; 2] = ["r0", "r1"];
/// Separates the two roots in the pack's file bytes.
const SEP: char = '\u{1e}';

/// A pack with two text roots (the file is the two texts joined by `SEP`).
struct TwoRoots;

impl Pack for TwoRoots {
    fn schema(&self) -> Schema {
        Schema {
            name: "conformance-two-roots",
            roots: &[("r0", Kind::Text), ("r1", Kind::Text)],
        }
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let declined = |m: &str| Declined {
            code: "not_two_roots",
            message: m.into(),
        };
        let s = std::str::from_utf8(bytes).map_err(|_| declined("not UTF-8"))?;
        let (a, b) = s.split_once(SEP).ok_or_else(|| declined("no separator"))?;
        Ok(PackContent {
            roots: vec![
                (ROOTS[0].into(), Neutral::Text(a.into())),
                (ROOTS[1].into(), Neutral::Text(b.into())),
            ],
            codec: Vec::new(),
        })
    }

    fn encode(&self, content: &[(String, Neutral)], _codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let text = |name: &str| {
            content
                .iter()
                .find(|(r, _)| r == name)
                .map(|(_, Neutral::Text(t))| t.clone())
                .ok_or_else(|| PackError(format!("no root {name}")))
        };
        Ok(format!("{}{SEP}{}", text(ROOTS[0])?, text(ROOTS[1])?).into_bytes())
    }

    fn normalize_input<'a>(&self, _root: &str, text: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(text)
    }
}

/// The kit's undo on a live session.
pub struct KitUndo {
    doc: KitDoc,
}

fn region(c: &ConflictRegion) -> Region {
    Region {
        root: ROOTS
            .iter()
            .position(|r| *r == c.root)
            .expect("a known root"),
        range: c.range.clone(),
        ours: c.ours.clone(),
        theirs: c.theirs.clone(),
    }
}

fn regions(cs: &[ConflictRegion]) -> Vec<Region> {
    cs.iter().map(region).collect()
}

impl KitUndo {
    fn run(
        &mut self,
        agent: u64,
        req: &UndoRequest,
        redo: bool,
    ) -> Result<UndoOutcome, UndoRefusal> {
        let c = self.doc.conn(agent);
        let _ = self.doc.changes();
        if redo {
            self.doc.session.redo_with(c, req)
        } else {
            self.doc.session.undo_with(c, req)
        }
        .map_err(|e| UndoRefusal::Other(e.to_string()))
    }

    fn refusal(out: UndoOutcome) -> UndoRefusal {
        match out {
            UndoOutcome::Conflicts { step, conflicts } => UndoRefusal::Conflicts {
                step,
                regions: regions(&conflicts),
            },
            UndoOutcome::Blocked { conflicts } => UndoRefusal::Blocked(regions(&conflicts)),
            UndoOutcome::Nothing | UndoOutcome::HistoryLost | UndoOutcome::ClearedByPurge => {
                UndoRefusal::Nothing
            }
            other => UndoRefusal::Other(format!("{other:?}")),
        }
    }

    fn all(&mut self, agent: u64, steps: usize, redo: bool) -> Result<Applied, UndoRefusal> {
        match self.run(agent, &UndoRequest::steps(steps), redo)? {
            UndoOutcome::Applied { steps, .. } => Ok(Applied {
                steps,
                changes: self.doc.changes(),
            }),
            other => Err(Self::refusal(other)),
        }
    }
}

impl Undo for KitUndo {
    type Engine = YrsText;

    fn open(seeds: [&str; 2]) -> Self {
        let bytes = format!("{}{SEP}{}", seeds[0], seeds[1]);
        KitUndo {
            doc: KitDoc::open(Arc::new(TwoRoots), "undo.txt", bytes.as_bytes(), &ROOTS),
        }
    }

    fn edit(&mut self, agent: u64, edits: &[(usize, Vec<Hunk>)]) -> Result<(), Refused> {
        let c = self.doc.conn(agent);
        let edits = edits
            .iter()
            .map(|(r, hunks)| Edit::Text {
                root: ROOTS[*r].into(),
                hunks: hunks.clone(),
            })
            .collect();
        self.doc
            .session
            .edit(c, edits, None)
            .map(|_| ())
            .map_err(|e| Refused(e.to_string()))
    }

    fn undo(&mut self, agent: u64, steps: usize) -> Result<Applied, UndoRefusal> {
        self.all(agent, steps, false)
    }

    fn undo_partial(&mut self, agent: u64, steps: usize) -> Result<PartialUndo, UndoRefusal> {
        let req = UndoRequest::steps(steps).mode(UndoMode::Partial);
        match self.run(agent, &req, false)? {
            UndoOutcome::Applied {
                steps,
                skipped,
                blocked,
                ..
            } => Ok(PartialUndo {
                applied: Applied {
                    steps,
                    changes: self.doc.changes(),
                },
                skipped: regions(&skipped),
                blocked: blocked.as_deref().map(regions),
            }),
            other => Err(Self::refusal(other)),
        }
    }

    fn drop_top(&mut self, agent: u64) -> Result<(), UndoRefusal> {
        let req = UndoRequest::default().mode(UndoMode::Drop);
        match self.run(agent, &req, false)? {
            UndoOutcome::Dropped { .. } => Ok(()),
            other => Err(Self::refusal(other)),
        }
    }

    fn redo(&mut self, agent: u64, steps: usize) -> Result<Applied, UndoRefusal> {
        self.all(agent, steps, true)
    }

    fn revert(&mut self, agent: u64, change: u64) -> Result<Applied, UndoRefusal> {
        let c = self.doc.conn(agent);
        let _ = self.doc.changes();
        match self.doc.session.revert(c, change, false) {
            Ok(UndoOutcome::Applied { steps, .. }) => Ok(Applied {
                steps,
                changes: self.doc.changes(),
            }),
            Ok(other) => Err(Self::refusal(other)),
            Err(e) => Err(UndoRefusal::Other(e.to_string())),
        }
    }

    /// The session's per-character blame (feature `conformance`): the element is the
    /// engine's (client, clock); its agent is the owner of the client.
    fn blame(&self, root: usize) -> Vec<ElemBlame> {
        self.doc
            .session
            .char_blame(ROOTS[root])
            .expect("char blame")
            .into_iter()
            .map(|b| ElemBlame {
                ch: b.ch,
                agent: b
                    .pid
                    .as_deref()
                    .and_then(|p| self.doc.agent_of(p))
                    .unwrap_or(crate::SEED_AGENT),
                id: (b.client, b.clock),
                change: b.change,
            })
            .collect()
    }

    fn texts(&self) -> [String; 2] {
        let mut t = self.doc.texts();
        let b = t.pop().expect("root 1");
        let a = t.pop().expect("root 0");
        [a, b]
    }
}
