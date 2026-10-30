//! Blame on a live text doc (K-HIST-11, K-HIST-18, P22): [`Session::blame`]. A child of
//! `live`, so it works on the session's own state.
//!
//! **Grain.** Text is blamed per character: every live character is an element with an ID
//! (the client and clock that inserted it, [`crate::doc::Doc::blame_runs`]) and a
//! **provenance root** (undo spec U23: itself, or for text an undo, redo or revert put back,
//! the element it restores), both read off the session's provenance map (built from the
//! engine and the store's provenance rows on first need, O(doc), then kept exact). The
//! store maps a client to its participant (the client table) and a clock to its change
//! (`change_clocks`, written with each change). A character **blames to its root's insert**
//! (U18a): the change that first inserted the element it restores, and that change's
//! author; text put back by an undo, redo or revert also names the **restoring** change and
//! its author. A line is then given its **most recent root author**: see [`BlameLine`].

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use super::*;

/// One line of [`Blame`]: who last wrote it, by which change, when.
///
/// **The rule.** A line's characters are its text **and its terminating `\n`** (if any), so
/// splitting a line attributes the first half to whoever typed the `\n`, as `git blame`
/// would. Each character carries the change that inserted its **provenance root** (undo
/// spec v1.8, U18a, U23): for text an undo, redo or revert put back, the change that first
/// inserted the element it restores; for every other character, the change that inserted
/// it. The line goes to the character with the **greatest root change id** (its log
/// position: change ids increase in the daemon's apply order, so this is the true "most
/// recent", immune to host clock skew; `at_ms` is shown, never ranked). Characters whose
/// change the store can't name (text inserted before v0.3.0 recorded clock spans) rank
/// below every named one; among only those, the line's first character decides. When that
/// character was put back by an undo, redo or revert, [`BlameLine::restored_by`] and
/// [`BlameLine::restored_in`] name the restoring change and its author (`restore`, a new
/// edit, is not a restoration: its text is the restorer's).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlameLine {
    /// 1-based, as editors number lines.
    pub line: usize,
    /// The line's text, without its `\n`.
    pub text: String,
    /// The pid that wrote it: a participant, `kit:import` (the file's first content),
    /// `kit:external` (an edit made outside the kit), a pseudonym (an erased participant,
    /// K-PURGE-7), or `kit:unknown` when the store has no owner for the client (never a
    /// guess).
    pub author: String,
    /// The change (its log position, the stable change id). `None` for text from before
    /// v0.3.0 (no clock span recorded): the author still comes from the client table.
    pub change: Option<Seq>,
    pub kind: Option<ChangeKind>,
    /// The surface it came through (`tab`, `cli`, `disk`...).
    pub via: Option<String>,
    /// When the change was made (host clock, ms).
    pub at_ms: Option<u64>,
    /// The person an assistant's change was made for (K-ID-10).
    pub on_behalf_of: Option<String>,
    /// The approval state of an on-behalf change.
    pub approval: Option<ApprovalState>,
    /// A purge reached the change behind this line (its revert record was redacted,
    /// K-PURGE-3): its recorded content is gone, its metadata (id, time, author or the
    /// author's pseudonym) stays. The line's text is the live text, which a purge never
    /// changes (edit it out, or purge with `and_remove`). Not known once the change's record
    /// has left the revert window.
    pub erased: bool,
    /// The pid whose undo, redo or revert put the line's character back (U18a), when it was
    /// put back; every field above is then the root's (the change that first inserted it).
    pub restored_by: Option<String>,
    /// The restoring change (an undo, redo or revert), when there is one and the store
    /// recorded it.
    pub restored_in: Option<Seq>,
}

/// Returned by [`Session::blame`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Blame {
    pub root: String,
    /// The doc's version the blame describes.
    pub version: Seq,
    /// The lines asked for, in order. A text ending in `\n` has no empty last line here
    /// (`git blame` counts lines the same way).
    pub lines: Vec<BlameLine>,
}

fn kind_name(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Edit => "edit",
        ChangeKind::Undo => "undo",
        ChangeKind::Redo => "redo",
        ChangeKind::Restore => "restore",
        ChangeKind::Revert => "revert",
        ChangeKind::External => "external",
        ChangeKind::Import => "import",
    }
}

fn approval_name(a: ApprovalState) -> &'static str {
    match a {
        ApprovalState::Pending => "pending",
        ApprovalState::Approved => "approved",
        ApprovalState::Declined => "declined",
    }
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) of a host-clock time in ms.
fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (y, m, d) = super::super::tier0::civil_from_days((secs / 86_400) as i64);
    let tod = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    )
}

impl Blame {
    /// The `git blame`-like listing a host prints: per line, the change id, the author's
    /// label, the date (UTC), the line number and the text:
    ///
    /// ```text
    /// 3 (Sam                       2023-11-14 22:13:20 1) # Plan
    /// 5 (Aide for Rosson (pending) 2023-11-14 22:13:25 2) Intro.
    /// ```
    ///
    /// `label` turns a pid into a display name (the host's lookup; `None` prints the pid).
    /// An unknown change prints `-`; an erased line gets `[erased]` after its date. A line
    /// put back by an undo, redo or revert names its root's author, then `(restored by B in
    /// C)` with the restorer and the restoring change. The listing holds doc text: a host
    /// fences it for agents as it fences `show`.
    pub fn render(&self, label: impl Fn(&str) -> Option<String>) -> String {
        let name = |pid: &str| label(pid).unwrap_or_else(|| pid.to_owned());
        let rows: Vec<(String, String, String, &BlameLine)> = self
            .lines
            .iter()
            .map(|l| {
                let id = l.change.map_or_else(|| "-".to_owned(), |c| c.to_string());
                let mut who = name(&l.author);
                if let Some(p) = &l.on_behalf_of {
                    who.push_str(" for ");
                    who.push_str(&name(p));
                }
                if let Some(a @ (ApprovalState::Pending | ApprovalState::Declined)) = l.approval {
                    let _ = write!(who, " ({})", approval_name(a));
                }
                if let Some(r) = &l.restored_by {
                    let _ = write!(who, " (restored by {}", name(r));
                    if let Some(c) = l.restored_in {
                        let _ = write!(who, " in {c}");
                    }
                    who.push(')');
                }
                let mut date = l.at_ms.map_or_else(|| "-".to_owned(), utc);
                if l.erased {
                    date.push_str(" [erased]");
                }
                (id, who, date, l)
            })
            .collect();
        let w = |f: fn(&(String, String, String, &BlameLine)) -> usize| {
            rows.iter().map(f).max().unwrap_or(0)
        };
        let (wi, ww, wd) = (
            w(|r| r.0.chars().count()),
            w(|r| r.1.chars().count()),
            w(|r| r.2.chars().count()),
        );
        let wl = self.lines.last().map_or(1, |l| l.line.to_string().len());
        let mut out = String::new();
        for (id, who, date, l) in rows {
            let _ = writeln!(
                out,
                "{id:>wi$} ({who:<ww$} {date:<wd$} {:>wl$}) {}",
                l.line, l.text
            );
        }
        out
    }

    /// A machine-readable listing in the shape of `git blame --line-porcelain`: per line a
    /// header `<change> <line> <line> 1` (change `0` when unknown), then `author`,
    /// `author-time` (seconds) and `author-time-ms`, `kind`, `via`, `on-behalf-of`,
    /// `approval`, `erased`, `restored-by <pid>` and `restored-in <change>` when they apply,
    /// then the text after a tab. The header's change and the author are the root's (U18a).
    pub fn porcelain(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            let _ = writeln!(out, "{} {} {} 1", l.change.unwrap_or(0), l.line, l.line);
            let _ = writeln!(out, "author {}", l.author);
            if let Some(t) = l.at_ms {
                let _ = writeln!(out, "author-time {}", t / 1000);
                let _ = writeln!(out, "author-time-ms {t}");
            }
            if let Some(k) = l.kind {
                let _ = writeln!(out, "kind {}", kind_name(k));
            }
            if let Some(v) = &l.via {
                let _ = writeln!(out, "via {v}");
            }
            if let Some(p) = &l.on_behalf_of {
                let _ = writeln!(out, "on-behalf-of {p}");
            }
            if let Some(a) = l.approval {
                let _ = writeln!(out, "approval {}", approval_name(a));
            }
            if l.erased {
                out.push_str("erased\n");
            }
            if let Some(r) = &l.restored_by {
                let _ = writeln!(out, "restored-by {r}");
            }
            if let Some(c) = l.restored_in {
                let _ = writeln!(out, "restored-in {c}");
            }
            let _ = writeln!(out, "\t{}", l.text);
        }
        out
    }
}

/// One live character and the element that holds it ([`Session::char_blame`], feature
/// `conformance`).
#[cfg(feature = "conformance")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharBlame {
    pub ch: char,
    pub client: ClientId,
    /// The element's clock (UTF-16 code units).
    pub clock: u32,
    /// The pid that owns the client, if the store knows it.
    pub pid: Option<String>,
    /// The change that inserted it, if the store recorded one.
    pub change: Option<Seq>,
    /// Its provenance root's client and clock (undo spec U23): itself, unless an undo, redo
    /// or revert put it back.
    pub root_client: ClientId,
    pub root_clock: u32,
    /// The pid that owns the root's client, if the store knows it.
    pub root_pid: Option<String>,
    /// The change that inserted the root, if the store recorded one.
    pub root_change: Option<Seq>,
}

/// A live character's element and root, with the changes that inserted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resolved {
    id: ElemId,
    /// `None`: its own root.
    root: Option<ElemId>,
    id_seq: Option<Seq>,
    root_seq: Option<Seq>,
}

/// The change covering `clock` among `spans` (clock order), moving the cursor `k` forward.
fn seek(spans: &[crate::store::SpanOf], k: &mut usize, clock: u32) -> Option<Seq> {
    while *k < spans.len() && spans[*k].1 <= clock {
        *k += 1;
    }
    spans
        .get(*k)
        .filter(|s| s.0 <= clock && clock < s.1)
        .map(|s| s.2)
}

/// Walk the characters of `text[lo..]` the provenance runs `runs` cover (from `lo` on),
/// calling `f(byte, char, resolved)` for each, with one store read for every change lookup.
fn walk(
    store: &DocStore,
    doc_id: DocId,
    text: &str,
    runs: &[super::super::prov::Run],
    lo: usize,
    mut f: impl FnMut(usize, char, Resolved),
) -> Result<()> {
    let mut q = Vec::with_capacity(runs.len() * 2);
    for r in runs {
        let root = r.root.unwrap_or(r.id);
        q.push((root.client, root.clock, root.clock + r.units));
        if r.root.is_some() {
            q.push((r.id.client, r.id.clock, r.id.clock + r.units));
        }
    }
    let spans = store.clock_spans(doc_id, &q)?;
    let mut spans = spans.iter();
    let mut at = lo;
    for r in runs {
        let rs = spans.next().map_or(&[][..], Vec::as_slice);
        let is = if r.root.is_some() {
            Some(spans.next().map_or(&[][..], Vec::as_slice))
        } else {
            None
        };
        let (mut a, mut b, mut k) = (0, 0, 0u32);
        for ch in text[at..at + r.bytes as usize].chars() {
            let id = r.id.plus(k);
            let root = r.root.map(|x| x.plus(k));
            let root_seq = seek(rs, &mut a, root.unwrap_or(id).clock);
            let id_seq = match is {
                Some(v) => seek(v, &mut b, id.clock),
                None => root_seq,
            };
            f(
                at,
                ch,
                Resolved {
                    id,
                    root,
                    id_seq,
                    root_seq,
                },
            );
            k += ch.len_utf16() as u32;
            at += ch.len_utf8();
        }
    }
    Ok(())
}

/// A span of live text, `start..end` bytes, whose root was inserted by `client` in change
/// `seq` (`None`: no clock span recorded for it), put back by `restorer` (its client and
/// change) when it is a restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    start: usize,
    end: usize,
    client: ClientId,
    seq: Option<Seq>,
    restorer: Option<(ClientId, Option<Seq>)>,
}

/// The pieces of `text[lo..hi]`, by root.
fn pieces(
    store: &DocStore,
    doc_id: DocId,
    text: &str,
    runs: &[super::super::prov::Run],
    lo: usize,
) -> Result<Vec<Piece>> {
    let mut out: Vec<Piece> = Vec::new();
    walk(store, doc_id, text, runs, lo, |at, ch, x| {
        let p = Piece {
            start: at,
            end: at + ch.len_utf8(),
            client: x.root.unwrap_or(x.id).client,
            seq: x.root_seq,
            restorer: x.root.map(|_| (x.id.client, x.id_seq)),
        };
        match out.last_mut() {
            Some(l)
                if l.end == p.start
                    && (l.client, l.seq, l.restorer) == (p.client, p.seq, p.restorer) =>
            {
                l.end = p.end
            }
            _ => out.push(p),
        }
    })?;
    Ok(out)
}

impl Session {
    /// Test-only (feature `conformance`, off by default): every live character of text root
    /// `root`, in order, with the element that holds it: `(char, (client, clock), owner pid,
    /// change)`, and its provenance root `(client, clock)` with the root's owner and change
    /// (U23: itself, unless an undo, redo or revert put it back). Clocks count UTF-16 code
    /// units; a change is the log position that inserted it, where the store recorded one.
    /// For per-element rules (an undo deletes only elements whose root its change inserted).
    #[cfg(feature = "conformance")]
    pub fn char_blame(&self, root: &str) -> Result<Vec<CharBlame>> {
        let mut st = self.lock()?;
        let st = &mut *st;
        self.text_of(st, root)?;
        self.prov(st)?;
        let st = &*st;
        let (_, text) = self.text_of(st, root)?;
        let runs = st
            .prov
            .as_ref()
            .expect("built")
            .query(root, text, 0, text.len());
        let store = self.store(st)?.clone();
        let mut owners: HashMap<ClientId, Option<String>> = HashMap::new();
        let mut owner = |c: ClientId| -> Result<Option<String>> {
            Ok(match owners.entry(c) {
                std::collections::hash_map::Entry::Occupied(e) => e.get().clone(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(store.client_owner(self.doc_id, c)?).clone()
                }
            })
        };
        let mut chars = Vec::with_capacity(text.len());
        walk(&store, self.doc_id, text, &runs, 0, |_, ch, x| {
            chars.push((ch, x))
        })?;
        let mut out = Vec::with_capacity(chars.len());
        for (ch, x) in chars {
            let root = x.root.unwrap_or(x.id);
            out.push(CharBlame {
                ch,
                client: x.id.client,
                clock: x.id.clock,
                pid: owner(x.id.client)?,
                change: x.id_seq,
                root_client: root.client,
                root_clock: root.clock,
                root_pid: owner(root.client)?,
                root_change: x.root_seq,
            });
        }
        Ok(out)
    }

    /// **Blame** `root` (K-HIST-11, P22): each line's most recent author, by the rule on
    /// [`BlameLine`], with the change, kind, surface, time, `on_behalf_of` and approval state
    /// (K-HIST-18), and `erased` where a purge reached the change. `lines` is `blame -L a,b`:
    /// 1-based, inclusive, of this root (`first` must exist; `last` past the end is
    /// clamped); `None` blames every line. Needs read and **history** (K-PERM-9). Reads only:
    /// nothing in the doc, the store or presence changes.
    ///
    /// Cost: the provenance map's range (built once per load, O(doc): the engine walks a
    /// throwaway copy of the doc), plus one store read for the range's runs; keep it off the
    /// edit path (a host computes it on demand: the blame gutter, the verb). Works the same
    /// after a reload: roots are in the store (provenance rows, store format 5).
    pub fn blame(&self, conn: ConnId, root: &str, lines: Option<LineRange>) -> Result<Blame> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Blame, now)?;
        self.admit(&mut st, conn, Need::History, Action::Blame, now)?;
        let st = &mut *st;
        self.text_of(st, root)?;
        self.prov(st)?;
        let st = &*st;
        let (_, text) = self.text_of(st, root)?;
        // git's lines: each with its `\n`; no empty line after a final `\n`
        let count = text.split_inclusive('\n').count();
        let (first, last) = match &lines {
            None => (1, count),
            Some(r) => {
                if r.root != root {
                    return Err(SessionError::BadPosition(format!(
                        "lines of root {:?}, blaming {root:?}",
                        r.root
                    )));
                }
                if r.first == 0 || r.first > r.last || r.first > count {
                    return Err(SessionError::BadPosition(format!(
                        "lines {}..{} of a {count}-line text",
                        r.first, r.last
                    )));
                }
                (r.first, r.last.min(count))
            }
        };
        let mut out = Blame {
            root: root.to_owned(),
            version: st.version,
            lines: Vec::new(),
        };
        if count == 0 {
            return Ok(out);
        }
        let spans: Vec<(usize, usize)> = text
            .split_inclusive('\n')
            .scan(0usize, |at, l| {
                let s = *at;
                *at += l.len();
                Some((s, *at))
            })
            .skip(first - 1)
            .take(last - first + 1)
            .collect();
        let (lo, hi) = (spans[0].0, spans[spans.len() - 1].1);
        let runs = st.prov.as_ref().expect("built").query(root, text, lo, hi);
        let store = self.store(st)?.clone();
        let pieces = pieces(&store, self.doc_id, text, &runs, lo)?;
        // what the store knows about the changes and clients involved
        let seqs: BTreeSet<Seq> = pieces
            .iter()
            .flat_map(|p| [p.seq, p.restorer.and_then(|r| r.1)])
            .flatten()
            .collect();
        let infos = store.change_infos(self.doc_id, &seqs)?;
        let purged: HashSet<Seq> = store.purged_changes(self.doc_id, &seqs)?;
        let approvals: HashMap<Seq, ApprovalState> = store
            .approvals(self.doc_id)?
            .into_iter()
            .filter(|(s, _)| seqs.contains(s))
            .map(|(s, a)| (s, a.state))
            .collect();
        let mut owners: HashMap<ClientId, Option<String>> = HashMap::new();
        for c in pieces
            .iter()
            .flat_map(|p| [Some(p.client), p.restorer.map(|r| r.0)])
            .flatten()
        {
            if let std::collections::hash_map::Entry::Vacant(e) = owners.entry(c) {
                e.insert(store.client_owner(self.doc_id, c)?);
            }
        }
        // change ids increase in apply order: the greatest is the most recent
        let rank = |p: &Piece| match p.seq {
            Some(s) => (true, s),
            None => (false, 0),
        };
        let mut k = 0;
        for (n, (s, e)) in spans.into_iter().enumerate() {
            while k < pieces.len() && pieces[k].end <= s {
                k += 1;
            }
            let mut best: Option<&Piece> = None;
            for p in pieces[k..].iter().take_while(|p| p.start < e) {
                if best.map_or(true, |b| rank(p) > rank(b)) {
                    best = Some(p);
                }
            }
            let best = best.ok_or_else(|| {
                SessionError::Doc(DocError::Apply("blame: a line with no characters".into()))
            })?;
            let info = best.seq.and_then(|s| infos.get(&s));
            let author = owners
                .get(&best.client)
                .cloned()
                .flatten()
                .or_else(|| info.map(|i| i.author.clone()))
                .unwrap_or_else(|| UNKNOWN_PID.to_owned());
            out.lines.push(BlameLine {
                line: first + n,
                text: text[s..e]
                    .strip_suffix('\n')
                    .unwrap_or(&text[s..e])
                    .to_owned(),
                author,
                change: best.seq,
                kind: info.map(|i| i.kind),
                via: info.map(|i| i.via.clone()),
                at_ms: info.map(|i| i.at_ms),
                on_behalf_of: info.and_then(|i| i.on_behalf_of.clone()),
                approval: best.seq.and_then(|s| approvals.get(&s).copied()),
                erased: best.seq.is_some_and(|s| purged.contains(&s)),
                restored_by: best.restorer.map(|(c, seq)| {
                    owners
                        .get(&c)
                        .cloned()
                        .flatten()
                        .or_else(|| seq.and_then(|s| infos.get(&s)).map(|i| i.author.clone()))
                        .unwrap_or_else(|| UNKNOWN_PID.to_owned())
                }),
                restored_in: best.restorer.and_then(|r| r.1),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionManager, TextFilePack};

    struct H;
    impl Host for H {
        fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
            Rights::full()
        }
        fn now_ms(&self) -> u64 {
            1_000
        }
        fn on_event(&self, _: &AuditEvent) {}
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_700_000_000_000), "2023-11-14 22:13:20");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00");
        assert_eq!(utc(4_107_542_399_999), "2100-02-28 23:59:59");
    }

    /// Blame runs the engine on a throwaway replica: the live doc's state vector, encoded
    /// state, text and version are the same after. And a client the store has no owner for
    /// (a damaged log, a bug) blames as `kit:unknown`, with no change: never a guess.
    #[test]
    fn live_doc_untouched_and_unknown_clients() {
        let dir = std::env::temp_dir().join(format!("k2mb-blame-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(store, Arc::new(H), Config::default());
        let key = DocKey {
            space: "ws".into(),
            path: "u.md".into(),
        };
        const ROOT: &str = TextFilePack::ROOT;
        let s = mgr
            .open(
                Arc::new(TextFilePack::new("md", 1 << 20)),
                &key,
                Some(b"one\n"),
            )
            .unwrap();
        let p = Participant::new("alice", "A", "#111111", ParticipantKind::Person, "tab");
        let c = s.join(p, Stay::Held).unwrap().conn;
        s.replace_text(c, ROOT, "one\ntwo\n").unwrap();
        let snap = |s: &Session| {
            let st = s.lock().unwrap();
            (
                st.doc.state_vector(),
                st.doc.encode_state().unwrap(),
                st.texts.clone(),
                st.version,
            )
        };
        let before = snap(&s);
        let b = s.blame(c, ROOT, None).unwrap();
        assert_eq!(snap(&s), before);
        assert_eq!(b.lines[1].author, "alice");
        // text from a client with no owner in the store
        let ghost = {
            let st = s.lock().unwrap();
            let mut d = Doc::load(s.schema, &st.doc.encode_state().unwrap()).unwrap();
            d.replace_text_as(4242, ROOT, "ghost\none\ntwo\n")
                .unwrap()
                .unwrap()
        };
        s.apply_raw_for_tests(&ghost).unwrap();
        {
            let mut st = s.lock().unwrap();
            st.texts = texts_of(&st.doc, &s.schema).unwrap();
        }
        let b = s.blame(c, ROOT, None).unwrap();
        let l = &b.lines[0];
        assert_eq!(l.text, "ghost");
        assert_eq!(
            (l.author.as_str(), l.change, l.kind, l.at_ms),
            (UNKNOWN_PID, None, None, None)
        );
        assert_eq!(b.lines[1].author, IMPORT_PID);
        assert_eq!(b.lines[2].author, "alice");
        assert!(
            b.render(|_| None).starts_with("- (kit:unknown - "),
            "{}",
            b.render(|_| None)
        );
        drop(s);
        drop(mgr);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
