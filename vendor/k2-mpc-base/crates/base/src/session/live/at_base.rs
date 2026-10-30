//! The hunk channel's two calls (kit v0.9.7, K2's Live tab): [`Session::events_since`] (K3)
//! and [`Session::edit_at_base`] (K2, amendment E-LIVE r2). A child of `live`, so it works
//! on the session's own state.
//!
//! **The window** (rule 1): the changes after a base version, read from the store (each
//! change's summary and its revert record: the hunks it applied, canonical, with the text
//! they inserted and deleted; and, for a change D1 widened (v0.9.7), the hunks its author
//! wrote, which are what the channel broadcast and what the window reports, clarification
//! r2.1). Every path that changes a text root records one (edits, agent
//! writes, `write --base`, undo, redo, revert, restore, the import, outside edits), so the
//! window has no gaps. It is bounded by [`Config::live_window_events`] and
//! [`Config::live_window_bytes`]; past them, a change out of the revert window, or one whose
//! text a purge took, the base is `MPC-E112 base_expired` and the client reloads.

use super::*;
use crate::errors::{ids, ErrorReport, Value};
use crate::session::rebase::{rebase_ours, LiveConflict};

/// Returned by [`Session::edit_at_base`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditAtBase {
    /// The change as [`Session::edit`] reports it (`version` unchanged when nothing applied:
    /// the edit was already in the text, e.g. a deletion someone else made first).
    pub result: EditResult,
    /// The hunks as applied: the edit rebased onto the doc's text, canonical, in the
    /// coordinates of the text before the change (the change's own [`ChangeEvent`] carries
    /// them too). Empty when nothing applied.
    pub hunks: Vec<Hunk>,
}

/// One change of the window, as its events (one per text root it touched: the authored
/// hunks, as broadcast) and, per event, the record's hunks with the text each deleted (to
/// rebuild the base's text; D1's widened hunks give the same text as the authored ones).
struct Windowed {
    event: ChangeEvent,
    elems: Vec<Hunk>,
    deleted: Vec<String>,
}

fn rep(e: SessionError) -> ErrorReport {
    e.report()
}

fn expired(base: Seq, why: &str) -> ErrorReport {
    ErrorReport::new(
        &ids::BASE_EXPIRED,
        format!("base version {base} is unknown or no longer cached; nothing applied"),
    )
    .with("base", base)
    .with("why", why)
}

/// `text` before `events` (applied in order, each in its own pre-change coordinates), from
/// the text they deleted.
fn unapply(text: &str, events: &[(&[Hunk], &[String])]) -> String {
    let mut t = text.to_owned();
    for (hunks, deleted) in events.iter().rev() {
        let mut delta: isize = 0;
        let inverse: Vec<Hunk> = hunks
            .iter()
            .zip(deleted.iter())
            .map(|(h, d)| {
                let start = (h.start as isize + delta) as usize;
                delta += h.insert.len() as isize - (h.end - h.start) as isize;
                Hunk {
                    start,
                    end: start + h.insert.len(),
                    insert: d.clone(),
                }
            })
            .collect();
        t = diff::apply(&t, &inverse);
    }
    t
}

/// Planted (`window-holds-expanded-hunks`): each change's hunks widened to whole words of
/// the text before it (a deletion plus a re-insertion of the rest of each word it touched),
/// the same change in effect.
#[cfg(any(test, feature = "differential"))]
fn expanded(base: &str, since: &[Vec<Hunk>]) -> Vec<Vec<Hunk>> {
    let word = |c: char| c.is_alphanumeric();
    let mut t = base.to_owned();
    let mut out = Vec::with_capacity(since.len());
    for hs in since {
        let wide: Vec<Hunk> = hs
            .iter()
            .map(|h| {
                let mut s = h.start;
                while let Some(c) = t[..s].chars().next_back().filter(|&c| word(c)) {
                    s -= c.len_utf8();
                }
                let mut e = h.end;
                while let Some(c) = t[e..].chars().next().filter(|&c| word(c)) {
                    e += c.len_utf8();
                }
                Hunk {
                    start: s,
                    end: e,
                    insert: format!("{}{}{}", &t[s..h.start], h.insert, &t[h.end..e]),
                }
            })
            .collect();
        let wide = diff::coalesce(wide);
        t = diff::apply(&t, hs);
        out.push(wide);
    }
    out
}

impl Session {
    /// The window after `base` (rule 1): its changes' events in order, or `MPC-E112`.
    fn window(&self, st: &State, base: Seq) -> std::result::Result<Vec<Windowed>, ErrorReport> {
        if base > st.version {
            return Err(expired(base, "a version this doc hasn't reached"));
        }
        if st.version - base > self.config.live_window_events {
            return Err(expired(
                base,
                "more changes since than the live window holds",
            ));
        }
        let store = self.store(st).map_err(rep)?;
        let rows = store
            .window_rows(self.doc_id, base, st.version)
            .map_err(|e| SessionError::from(e).report())?;
        let mut out = Vec::new();
        let mut bytes: u64 = 0;
        for row in rows {
            let (Some(info), Some(rec)) = (row.info, row.record) else {
                return Err(expired(base, "a change since is out of the revert window"));
            };
            let client = match row.client {
                Some(c) => c,
                // compacted: the author's client in this doc, as attribution has it
                None => store
                    .client_owned_by(self.doc_id, &info.author)
                    .map_err(|e| SessionError::from(e).report())?
                    .unwrap_or(0),
            };
            let mut authored = row.authored;
            for r in rec.roots {
                if r.hunks.iter().any(|h| h.redacted) {
                    return Err(expired(base, "a purge took the text of a change since"));
                }
                let mut elems = Vec::with_capacity(r.hunks.len());
                let mut deleted = Vec::with_capacity(r.hunks.len());
                for h in r.hunks {
                    elems.push(Hunk {
                        start: h.start as usize,
                        end: h.end as usize,
                        insert: h.insert,
                    });
                    deleted.push(h.deleted);
                }
                // D1 (v0.9.7): the hunks the author wrote, as the channel broadcast them
                let hunks = authored
                    .as_mut()
                    .and_then(|a| a.iter_mut().find(|(root, _)| *root == r.root))
                    .map(|(_, h)| std::mem::take(h))
                    .unwrap_or_else(|| elems.clone());
                bytes += hunks.iter().map(|h| h.insert.len() as u64).sum::<u64>();
                bytes += 64;
                if bytes > self.config.live_window_bytes {
                    return Err(expired(base, "more text since than the live window holds"));
                }
                out.push(Windowed {
                    event: ChangeEvent {
                        version: row.seq,
                        pid: info.author.clone(),
                        client,
                        via: info.via.clone(),
                        root: r.root,
                        hunks,
                        at_ms: info.at_ms,
                        on_behalf_of: info.on_behalf_of.clone(),
                    },
                    elems,
                    deleted,
                });
            }
        }
        Ok(out)
    }

    /// **The changes after `version`** (K3): every [`ChangeEvent`] the doc broadcast since,
    /// in order, one per text root a change touched (hunks in the coordinates of the root's
    /// text before that change). Needs read. A client that missed events (a reconnect)
    /// replays these onto its text at `version`; `version` = the doc's version gives none.
    ///
    /// Bounded by [`Config::live_window_events`] changes and [`Config::live_window_bytes`]:
    /// past them, or when a change since is out of the revert window or was purged,
    /// `MPC-E112 base_expired`: reload (read). Read from the store, so it works after a
    /// restart.
    pub fn events_since(
        &self,
        conn: ConnId,
        version: Seq,
    ) -> std::result::Result<Vec<ChangeEvent>, ErrorReport> {
        let now = self.now();
        let mut st = self.lock().map_err(rep)?;
        let i = self
            .admit(&mut st, conn, Need::Read, Action::Read, now)
            .map_err(rep)?;
        let events = self.window(&st, version)?.into_iter().map(|w| w.event);
        // an App viewer sees handles, never pids (K5)
        Ok(match Self::app_viewer(&st, i) {
            Some(v) => {
                let mut l = st.handles.lens(&v);
                events.map(|e| l.change(e)).collect()
            }
            None => events.collect(),
        })
    }

    /// A refusal as member `i` may see it: an App viewer's names others by handle (K5).
    fn viewer_rep(&self, st: &mut State, i: usize, e: SessionError) -> ErrorReport {
        match Self::app_viewer(st, i) {
            Some(v) => st.handles.lens(&v).error(e).report(),
            None => e.report(),
        }
    }

    /// **A live edit made at `base`** (K2, amendment E-LIVE r2): `hunks` (on text root
    /// `root`, in UTF-8 byte offsets of its text **at version `base`**) are carried through
    /// every change applied since, in server order, and applied as one change by `conn`
    /// (attributed, versioned, broadcast, undoable as an edit: its authored ranges are the
    /// hunks as applied). Typing near someone else, or at the same point, or deleting text
    /// they deleted too, never bounces:
    ///
    /// - two inserts at one point: the change applied first stays left (ours goes after);
    /// - deletion vs deletion: no conflict (what's already gone is a no-op);
    /// - others' inserted text is never deleted: a deletion over it is split around it, and
    ///   a replacement's text goes at the left end of its range.
    ///
    /// The client (`client/`, K1) has **one change in flight** (rule 0), and `base` is the
    /// latest version it has seen, its own acknowledged changes included. Refusals (nothing
    /// applied, P4):
    /// - `MPC-E112 base_expired`: `base` is past the live window (as
    ///   [`Session::events_since`]); reload;
    /// - `MPC-E151 bad_hunk`: the hunks are out of order or range, or off a character
    ///   boundary, at `base` (rule 7), checked first;
    /// - `MPC-E111 write_conflict` with the region (`details.conflicts`): only when ours
    ///   replaces text a change since deleted or replaced, or inserts strictly inside a range
    ///   a change since deleted (rule 4 (b));
    /// - as for every edit: rights, the rate (`MPC-E901`), the approvals backlog, `opts`'
    ///   version guard (`MPC-E110`) and claim policy.
    pub fn edit_at_base(
        &self,
        conn: ConnId,
        root: &str,
        base: Seq,
        hunks: Vec<Hunk>,
        opts: &EditOptions,
    ) -> std::result::Result<EditAtBase, ErrorReport> {
        let now = self.now();
        let mut st = self.lock().map_err(rep)?;
        let st = &mut *st;
        let i = self.admit_edit(st, conn, Action::Edit, now).map_err(rep)?;
        Self::check_version(st, opts.if_version).map_err(|e| self.viewer_rep(st, i, e))?;
        self.check_backlog(st, i)
            .map_err(|e| self.viewer_rep(st, i, e))?;
        let live = self.text_of(st, root).map_err(rep)?.1.to_owned();
        // rule 1: the window; rule 2: the changes since, on this root, in server order
        let window = self.window(st, base)?;
        let on_root: Vec<&Windowed> = window.iter().filter(|w| w.event.root == root).collect();
        // rule 7: the hunks, checked against the base's text
        let base_text = if on_root.is_empty() {
            live.clone()
        } else {
            let evs: Vec<(&[Hunk], &[String])> = on_root
                .iter()
                .map(|w| (w.elems.as_slice(), w.deleted.as_slice()))
                .collect();
            unapply(&live, &evs)
        };
        diff::validate(&base_text, &hunks)
            .map_err(|e| SessionError::Doc(DocError::from(e)).report())?;
        let ours = diff::coalesce(hunks);
        // rules 3–5, over the changes since as broadcast: the authored hunks (clarification
        // r2.1, F1), never widened to tokens
        #[allow(unused_mut)]
        let mut since: Vec<Vec<Hunk>> = on_root.iter().map(|w| w.event.hunks.clone()).collect();
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::live_mutant()
            == Some(crate::differential::LiveMutant::WindowHoldsExpandedHunks)
        {
            since = expanded(&base_text, &since);
        }
        let rebased = match rebase_ours(&ours, &since) {
            Ok(r) => r,
            Err(c) => return Err(self.live_conflict(st, i, &base_text, base, &c, &on_root)),
        };
        debug_assert!(
            diff::validate(&live, &rebased).is_ok(),
            "a rebase fits the text"
        );
        if rebased.is_empty() {
            return Ok(EditAtBase {
                result: EditResult {
                    version: st.version,
                    ..EditResult::default()
                },
                hunks: Vec::new(),
            });
        }
        // rule 6: one change by `conn`
        self.check_claims(st, i, root, &rebased, opts.policy, now)
            .map_err(|e| self.viewer_rep(st, i, e))?;
        let edits = self
            .normalize(vec![Edit::Text {
                root: root.to_owned(),
                hunks: rebased,
            }])
            .map_err(rep)?;
        let applied = match &edits[0] {
            Edit::Text { hunks, .. } => diff::coalesce(hunks.clone()),
        };
        st.group_hint = opts.group;
        let v = st.version;
        let result = self
            .commit_edit(st, i, edits, ChangeKind::Edit, None, now, Vec::new())
            .map_err(|e| self.viewer_rep(st, i, e))?;
        Ok(EditAtBase {
            hunks: if result.version == v {
                Vec::new()
            } else {
                applied
            },
            result,
        })
    }

    /// `MPC-E111` for a live edit (rule 4 (b)): the region, in the text just before the
    /// change it met (the base's text, at the first change since), and that change.
    #[allow(clippy::too_many_arguments)]
    fn live_conflict(
        &self,
        st: &mut State,
        i: usize,
        base_text: &str,
        base: Seq,
        c: &LiveConflict,
        on_root: &[&Windowed],
    ) -> ErrorReport {
        let mut before = base_text.to_owned();
        for w in on_root.iter().take(c.step) {
            before = diff::apply(&before, &w.event.hunks);
        }
        let lines = (
            position::line_of(&before, c.start),
            position::line_of(&before, c.end),
        );
        let theirs = on_root.get(c.step).map(|w| &w.event);
        // its author as `i` may see it: an App viewer by handle (K5)
        let author = theirs.map(|e| match Self::app_viewer(st, i) {
            Some(v) => st.handles.lens(&v).pid(&e.pid),
            None => e.pid.clone(),
        });
        let region = Value::obj([
            ("root", Value::from(theirs.map_or("", |e| e.root.as_str()))),
            ("lines", Value::from(lines)),
            ("kind", Value::from("overlap")),
            ("why", Value::from(c.why)),
            ("start", Value::from(c.start as u64)),
            ("end", Value::from(c.end as u64)),
            (
                "at_version",
                Value::from(theirs.map_or(base, |e| e.version - 1)),
            ),
            (
                "base",
                Value::from(before.get(c.start..c.end).unwrap_or_default()),
            ),
            ("ours", Value::from(c.insert.as_str())),
            (
                "theirs",
                Value::from(theirs.map_or_else(String::new, |e| {
                    format!(
                        "{} in change {}",
                        author.as_deref().unwrap_or_default(),
                        e.version
                    )
                })),
            ),
        ]);
        ErrorReport::new(
            &ids::WRITE_CONFLICT,
            format!(
                "your write overlaps changes made since version {base}: 1 region(s) at {}; \
                 nothing applied",
                awareness::pair_text(lines)
            ),
        )
        .with("base", base)
        .with("conflicts", Value::List(vec![region]))
    }
}
