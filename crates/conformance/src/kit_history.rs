//! [`Versions`], [`HistoryPurge`] and [`Erasure`] on the kit: its history is a session's
//! over its store (C7a, K-HIST, K-PURGE), so the adapter opens a real session through the
//! public API (`kit_session`) on the kit's plain-text pack, with a store in a
//! temporary directory. Nothing here is the adapter's own semantics:
//!
//! - a version is a **named version** of the live doc (`Session::name_version`); `show` and
//!   `restore` are `Session::show` and `Session::restore` (a new change by the restorer,
//!   through the kit's edit pipeline; its applied hunks are the change event's);
//! - a purge is `Session::purge` with a text scope and `and_remove` (`purge --text
//!   --and-remove`): the live copies are deleted as the owner's change, then every copy the
//!   store holds is redacted and the store file rewritten;
//! - the byte scan is the kit's own (`DocStore::scan_for`: the store file, whole or by any
//!   8-byte window), plus the live text;
//! - erasure is `Session::purge` with a participant scope; line authors are
//!   `Session::blame`, change counts `Session::log`.

use std::sync::Arc;

use k2_mpc_base::session::{LogItem, LogQuery, PurgeRequest, PurgeScope, TextFilePack};
use k2_mpc_base::store::{scan_bytes, VersionId};
use k2_mpc_base::Edit;

use crate::caps::{Change, Erasure, HistoryPurge, Versions};
use crate::kit_session::KitDoc;
use crate::yrs::YrsText;
use crate::{Hunk, Refused};

const ROOT: &str = TextFilePack::ROOT;
const ROOTS: [&str; 1] = [ROOT];

/// One kit doc's live session.
pub struct KitHistory {
    doc: KitDoc,
    named: u64,
}

fn refused(e: impl std::fmt::Display) -> Refused {
    Refused(e.to_string())
}

impl Versions for KitHistory {
    type Engine = YrsText;
    /// The store's version id.
    type VersionId = VersionId;

    fn open(seed: &str) -> Self {
        let pack = Arc::new(TextFilePack::new("conformance-text", 4 << 20));
        KitHistory {
            doc: KitDoc::open(pack, "history.txt", seed.as_bytes(), &ROOTS),
            named: 0,
        }
    }

    fn edit(&mut self, agent: u64, hunks: &[Hunk]) -> Result<(), Refused> {
        let c = self.doc.conn(agent);
        let edit = Edit::Text {
            root: ROOT.into(),
            hunks: hunks.to_vec(),
        };
        self.doc
            .session
            .edit(c, vec![edit], None)
            .map(|_| ())
            .map_err(refused)
    }

    fn text(&self) -> String {
        self.doc.texts().remove(0)
    }

    fn version(&mut self, agent: u64) -> VersionId {
        let c = self.doc.conn(agent);
        self.named += 1;
        let name = format!("conformance {}", self.named);
        self.doc
            .session
            .name_version(c, &name, None)
            .expect("name a version")
    }

    fn show(&self, v: &VersionId) -> Result<String, Refused> {
        let roots = self
            .doc
            .session
            .show(self.doc.watcher, *v)
            .map_err(refused)?;
        roots
            .into_iter()
            .find(|(r, _)| r == ROOT)
            .map(|(_, t)| t)
            .ok_or_else(|| Refused("no text root in the version".into()))
    }

    fn restore_applied(
        &mut self,
        agent: u64,
        v: &VersionId,
    ) -> Result<Option<(Change, Vec<Hunk>)>, Refused> {
        let c = self.doc.conn(agent);
        let _ = self.doc.applied();
        self.doc.session.restore(c, *v).map_err(refused)?;
        let mut changes = self.doc.applied();
        match changes.len() {
            0 => Ok(None),
            1 => {
                let (change, mut roots) = changes.pop().expect("one");
                match roots.len() {
                    1 => Ok(Some((change, roots.pop().expect("one").1))),
                    n => Err(Refused(format!("a restore touched {n} roots of one"))),
                }
            }
            n => Err(Refused(format!("a restore made {n} changes: {changes:?}"))),
        }
    }
}

impl HistoryPurge for KitHistory {
    fn purge_text(&mut self, owner: u64, needle: &str) -> Result<(), Refused> {
        let c = self.doc.conn(owner);
        let req = PurgeRequest::new(PurgeScope::Text(needle.into())).and_remove();
        self.doc.session.purge(c, req).map(|_| ()).map_err(refused)
    }

    fn holds(&self, needle: &str) -> bool {
        let n = [needle.as_bytes()];
        !self
            .doc
            .store
            .scan_for(&n)
            .expect("scan the store")
            .is_empty()
            || !scan_bytes(self.text().as_bytes(), &n).is_empty()
    }
}

impl Erasure for KitHistory {
    fn label(&self, agent: u64) -> String {
        self.doc.pid(agent)
    }

    fn erase(&mut self, owner: u64, agent: u64, pseudonym: &str) -> Result<(), Refused> {
        let c = self.doc.conn(owner);
        let pid = self.doc.pid(agent);
        let req = PurgeRequest::new(PurgeScope::Participant {
            pid,
            pseudonym: pseudonym.into(),
        });
        self.doc.session.purge(c, req).map_err(refused)?;
        // the session removed their connections
        self.doc.forget(agent);
        Ok(())
    }

    fn line_authors(&self) -> Vec<(String, String)> {
        self.doc
            .session
            .blame(self.doc.watcher, ROOT, None)
            .expect("blame")
            .lines
            .into_iter()
            .map(|l| (l.text, l.author))
            .collect()
    }

    fn changes_by(&self, label: &str) -> usize {
        self.doc
            .session
            .log(self.doc.watcher, &LogQuery::default().by(label))
            .expect("log")
            .iter()
            .filter(|i| matches!(i, LogItem::Change { .. }))
            .count()
    }
}
