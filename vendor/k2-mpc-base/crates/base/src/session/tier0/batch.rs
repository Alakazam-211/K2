//! A tier-0 batch (`docs/SESSIONS.md`, "A batch of verbs"): an agent's calls of a file type's
//! verbs as one change. Each call plans on what the calls before it staged; the output is read
//! once with the file type's index and checked against every call's declared nodes; one
//! version, one `blob_verbs` row (`batch` and its calls), all or nothing.
//!
//! The stage is head plus one composed list of ranges over head's bytes, so every stage is
//! read the way a single verb's output is: head's verified bytes, the ranges replaced by the
//! kit's applier. Call k plans on stage k-1 (its bytes and its index), its ranges are composed
//! into the list, and the new stage is indexed and diffed with the one before it: a node the
//! call didn't declare refuses the batch at that call (`MPC-E726`).
//!
//! Like a single verb, the batch plans with no lease and off the session lock; it is admitted
//! under the lock first (rights, the edit rate, agent writes, the lease, each `merge: Lease`
//! call's own lease, the batch's guards), and checked again under the lock at its one commit
//! (the caller, the lease, head, each call's claims), where the automatic lease is taken for
//! that moment only.

use std::collections::BTreeSet;

use super::base::BaseSide;
use super::node_claims::reach_of;
use super::struct_index::Target;
use super::verbs::{
    check_plan, counts_of_diff, kit_flags, tool_output, units_of_diff, wants_dry_run, wants_json,
    with_notes, Applied, Decision, KitFlags, Ran,
};
use super::*;
use crate::errors::Value;
use crate::index::read::{self as iread, NodeChange};
use crate::index::{IndexView, NodeKey};
use crate::session::{ClaimPolicy, BATCH_CALLS_CEILING};
use crate::tools::result::BlobResult;
use crate::tools::{exit, ChangedUnit, ToolError, ToolOutput, ToolRunError};

/// Whether the verb path's fault `$m` ([`crate::differential::Tier0VerbMutant`]) is planted on
/// this thread (test-only; always false in a host's build).
macro_rules! planted {
    ($m:ident) => {
        plant_on!(
            crate::differential::tier0_verb_mutant()
                == Some(crate::differential::Tier0VerbMutant::$m)
        )
    };
}

/// One call of a batch: a file type's verb and its arguments, as `run_tool` takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BatchCall {
    pub argv: Vec<String>,
}

impl BatchCall {
    pub fn new(argv: Vec<String>) -> Self {
        BatchCall { argv }
    }
}

/// A call as admitted: its verb, and its arguments after the verb's words (the kit's flags
/// taken out; `$N` not yet resolved).
struct Parsed {
    verb: &'static BlobVerb,
    rest: Vec<String>,
    json: bool,
}

/// What a call did in the fold.
struct Done {
    verb: &'static BlobVerb,
    out: BlobVerbOut,
    /// The nodes its change to the stage before it changed.
    changed: BTreeSet<NodeKey>,
    /// The nodes that change reaches into, for the claim check.
    reach: BTreeSet<NodeKey>,
}

impl Done {
    /// Whether the call declared the node `addr` (`touched` or `created`).
    fn declares(&self, addr: &str) -> bool {
        self.out.touched.iter().any(|k| k == addr) || self.out.created.iter().any(|k| k == addr)
    }

    /// Whether the call declared or changed the node `addr`.
    fn holds(&self, addr: &str) -> bool {
        self.declares(addr) || self.changed.iter().any(|k| k.as_str() == addr)
    }
}

/// The batch's calls folded over a version.
struct Folded {
    /// The composed ranges over the version's bytes (none: no call changed anything).
    spans: Vec<Span>,
    /// The last stage's size.
    size: u64,
    /// The last stage's index (none: no call changed anything, the version's own).
    view: Option<IndexView>,
    /// The last stage's draft, to seed the stored index from.
    draft: Option<crate::index::IndexDraft>,
    calls: Vec<Done>,
    /// The planted fault [`crate::differential::Tier0VerbMutant::BatchCommitsEach`]: the
    /// version its last call committed.
    committed: Option<VersionId>,
}

/// Why a batch stopped: at which call (1-based; `None`, the batch as a whole), and the
/// outcome (a pack's refusal as its output, or an error). Boxed: it is the error side of
/// every step.
type Stopped = Box<(Option<usize>, std::result::Result<ToolOutput, ToolRunError>)>;

fn stopped(at: Option<usize>, out: std::result::Result<ToolOutput, ToolRunError>) -> Stopped {
    Box::new((at, out))
}

fn whole(e: impl Into<ToolRunError>) -> Stopped {
    stopped(None, Err(e.into()))
}

fn at_call(k: usize, e: impl Into<ToolRunError>) -> Stopped {
    stopped(Some(k), Err(e.into()))
}

fn call_usage(k: usize, m: impl std::fmt::Display) -> Stopped {
    at_call(k, ToolError::usage(format!("batch call {k}: {m}")))
}

/// The result's `batch` part: each call (its verb, word and the nodes it created), and the
/// call a refusal names.
#[derive(Default)]
struct Info {
    calls: Vec<(&'static str, &'static str, Vec<String>)>,
    failed_at: Option<usize>,
}

impl Info {
    fn to_value(&self) -> Value {
        Value::obj([
            (
                "calls",
                Value::List(
                    self.calls
                        .iter()
                        .map(|(verb, word, created)| {
                            Value::obj([
                                ("verb", Value::from(*verb)),
                                ("word", Value::from(*word)),
                                ("created", Value::list(created.iter().map(String::as_str))),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("failed_at", Value::from(self.failed_at.map(|k| k as u64))),
        ])
    }
}

/// An argument naming an earlier call's node: `$N` (its first created node) or `$N.k` (its
/// k-th), both 1-based; `None` for any other argument. `$$` starts a literal `$`.
fn reference(arg: &str) -> Option<std::result::Result<(usize, usize), ()>> {
    let rest = arg.strip_prefix('$')?;
    if !rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (n, k) = match rest.split_once('.') {
        Some((n, k)) => (n, Some(k)),
        None => (rest, None),
    };
    let num = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<usize>().ok())
            .flatten()
            .filter(|v| *v >= 1)
    };
    Some(match (num(n), k.map(num)) {
        (Some(n), None) => Ok((n, 1)),
        (Some(n), Some(Some(k))) => Ok((n, k)),
        _ => Err(()),
    })
}

/// A call's arguments with each `$N` / `$N.k` replaced by the node an earlier call created.
fn resolve(rest: &[String], done: &[Done]) -> std::result::Result<Vec<String>, String> {
    rest.iter()
        .map(|a| {
            if let Some(lit) = a.strip_prefix("$$") {
                return Ok(format!("${lit}"));
            }
            match reference(a) {
                None => Ok(a.clone()),
                Some(Err(())) => Err(format!("{a}: write $N or $N.k (1-based), or $$ for a $")),
                Some(Ok((n, k))) => {
                    let Some(d) = done.get(n - 1) else {
                        return Err(format!("{a}: names no earlier call"));
                    };
                    d.out.created.get(k - 1).cloned().ok_or_else(|| {
                        format!(
                            "{a}: call {n} created {} node(s), not a node {k}",
                            d.out.created.len()
                        )
                    })
                }
            }
        })
        .collect()
}

/// One piece of a stage, over head's bytes: a stretch of head (`[at, end)`), or new bytes.
enum Seg {
    Head(u64, u64),
    New(Vec<u8>),
}

/// The ranges `call` (over the stage `stage` gives of a version of `size` bytes) composed
/// into one list over the version's own bytes: the stage as pieces of head and new bytes, the
/// call's ranges applied to those pieces, and the pieces turned back into ranges. `None` when
/// a range of `call` lies past the stage (a plan [`check_plan`] refuses first).
fn compose(stage: &[Span], call: &[Span], size: u64) -> Option<Vec<Span>> {
    let mut segs: std::collections::VecDeque<Seg> = std::collections::VecDeque::new();
    let mut pos = 0u64;
    for s in stage {
        if s.at > pos {
            segs.push_back(Seg::Head(pos, s.at));
        }
        if !s.with.is_empty() {
            segs.push_back(Seg::New(s.with.clone()));
        }
        pos = s.at + s.len;
    }
    if size > pos {
        segs.push_back(Seg::Head(pos, size));
    }
    // move `n` bytes of the stage from the front of `segs` to `out`, or drop them
    fn take(
        segs: &mut std::collections::VecDeque<Seg>,
        mut n: u64,
        mut out: Option<&mut Vec<Seg>>,
    ) -> Option<()> {
        while n > 0 {
            let seg = segs.pop_front()?;
            let len = match &seg {
                Seg::Head(a, b) => b - a,
                Seg::New(b) => b.len() as u64,
            };
            if len <= n {
                n -= len;
                if let Some(o) = out.as_deref_mut() {
                    o.push(seg);
                }
                continue;
            }
            // split: the first n bytes go, the rest stays at the front
            let (first, rest) = match seg {
                Seg::Head(a, b) => (Seg::Head(a, a + n), Seg::Head(a + n, b)),
                Seg::New(mut b) => {
                    let rest = b.split_off(usize::try_from(n).ok()?);
                    (Seg::New(b), Seg::New(rest))
                }
            };
            if let Some(o) = out.as_deref_mut() {
                o.push(first);
            }
            segs.push_front(rest);
            n = 0;
        }
        Some(())
    }
    let mut out: Vec<Seg> = Vec::new();
    let mut at = 0u64;
    for s in call {
        take(&mut segs, s.at.checked_sub(at)?, Some(&mut out))?;
        if !s.with.is_empty() {
            out.push(Seg::New(s.with.clone()));
        }
        take(&mut segs, s.len, None)?;
        at = s.at + s.len;
    }
    out.extend(segs);
    // back to ranges over head: each gap between kept stretches of head, with the new bytes in
    // it, is one range
    let mut spans = Vec::new();
    let mut head_at = 0u64;
    let mut new: Vec<u8> = Vec::new();
    for seg in out {
        match seg {
            Seg::New(b) => new.extend(b),
            Seg::Head(a, b) => {
                if a < head_at {
                    return None;
                }
                if a > head_at || !new.is_empty() {
                    spans.push(Span {
                        at: head_at,
                        len: a - head_at,
                        with: std::mem::take(&mut new),
                    });
                }
                head_at = b;
            }
        }
    }
    if size > head_at || !new.is_empty() {
        spans.push(Span {
            at: head_at,
            len: size - head_at,
            with: new,
        });
    }
    Some(spans)
}

/// The planted fault [`crate::differential::Tier0VerbMutant::BatchComposeWrong`]: the call's
/// ranges taken as head's, merged by offset, a later range replacing an earlier one it
/// overlaps.
fn compose_by_offset(stage: &[Span], call: &[Span]) -> Vec<Span> {
    let mut out: Vec<Span> = stage
        .iter()
        .filter(|s| {
            !call
                .iter()
                .any(|c| c.at < s.at + s.len.max(1) && s.at < c.at + c.len.max(1))
        })
        .cloned()
        .collect();
    out.extend(call.iter().cloned());
    out.sort_by_key(|s| (s.at, s.len));
    out
}

/// The size of a version of `size` bytes with `spans` applied.
fn size_with(size: u64, spans: &[Span]) -> Option<u64> {
    let removed: u64 = spans.iter().map(|s| s.len).sum();
    let inserted: u64 = spans.iter().map(|s| s.with.len() as u64).sum();
    size.checked_sub(removed)?.checked_add(inserted)
}

impl BlobSession {
    /// **A batch of the pack's verbs** as one change (`docs/SESSIONS.md`, "A batch of
    /// verbs"). `args` are the batch's own flags (`--if-version V` or `--base V`,
    /// `--dry-run`, `--expect DIGEST`, `--json`); each call is an edit verb of
    /// [`BlobPack::verbs`] with its arguments (its own guards refused), and may name a node an
    /// earlier call created as `$N` (`$N.k`: its k-th; `$$` for a literal `$`). Each call
    /// plans on what the calls before it staged; one version, attributed to the caller, with
    /// the verb `batch` and its calls; all or nothing. At most
    /// [`Config::blob_batch_max_calls`] calls (`MPC-E728` past it, before any call runs).
    ///
    /// The result (with [`crate::tools::ToolOptions::result_json`] or `--json`) is the
    /// single verb's, with `changed.word` `batch`, plus `batch`: `{calls, failed_at}`, each
    /// call's `{verb, word, created}` (empty on a refusal) and the 1-based call a refusal
    /// names (null when it is the batch's as a whole).
    pub fn run_batch(
        &self,
        conn: ConnId,
        args: &[String],
        calls: &[BatchCall],
        opts: &crate::tools::ToolOptions,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let as_data = opts.result_json || wants_json(args);
        let mut info = Info::default();
        let out = match self.batch_in(conn, args, calls, opts, as_data, &mut info) {
            Ok(o) => Ok(o),
            Err(b) => {
                let (at, out) = *b;
                info.failed_at = at;
                info.calls.clear();
                match (at, out) {
                    (Some(k), Ok(mut o)) => {
                        o.stderr_summary = format!("batch call {k}: {}", o.stderr_summary);
                        Ok(o)
                    }
                    (_, out) => out,
                }
            }
        };
        let out = match out {
            Err(ToolRunError::Session(e)) => {
                let e = match self.for_viewer(conn, Err::<(), SessionError>(e), |_, v| v) {
                    Err(e) => e,
                    Ok(()) => unreachable!("an error stays one"),
                };
                Err(e.into())
            }
            other => other,
        };
        if !as_data {
            return out;
        }
        let call = crate::tools::result::Call {
            verb: "batch".to_owned(),
            kind: Some("edit"),
            args,
            pack: None,
            dry_run: wants_dry_run(args),
        };
        let mut o = crate::tools::with_result(&call, out, || {
            self.status().map(|s| s.version).unwrap_or_default()
        });
        if let Some(r) = o.result.as_deref_mut() {
            if let Some(Value::Obj(fields)) = r.value.as_mut() {
                fields.push(("batch".to_owned(), info.to_value()));
            }
            if let Some(v) = &r.value {
                o.stdout = format!("{}\n", v.to_json()).into_bytes();
            }
        }
        Ok(o)
    }

    fn batch_in(
        &self,
        conn: ConnId,
        args: &[String],
        calls: &[BatchCall],
        opts: &crate::tools::ToolOptions,
        as_data: bool,
        info: &mut Info,
    ) -> std::result::Result<ToolOutput, Stopped> {
        // a pack the registry refuses runs nothing
        crate::parity::Registry::for_blob_pack(&*self.pack)
            .map_err(|e| whole(ToolError::usage(format!("batch: {e}"))))?;
        let f = kit_flags(args).map_err(whole)?;
        if let Some(a) = f.rest.first() {
            return Err(whole(ToolError::usage(format!(
                "batch: {a}: a batch takes --if-version V or --base V, --dry-run, --expect \
                 DIGEST and --json; its calls are given one by one"
            ))));
        }
        let spec = self
            .pack
            .index_spec()
            .ok_or_else(|| whole(SessionError::IndexUnsupported))?;
        // the call cap, before anything runs
        let cap = self.config.blob_batch_max_calls;
        if cap == 0 || cap > BATCH_CALLS_CEILING {
            return Err(whole(ToolError::usage(format!(
                "batch: the setting blob_batch_max_calls is {cap}, outside 1 to \
                 {BATCH_CALLS_CEILING}: every batch is refused until the host sets it in range"
            ))));
        }
        if calls.is_empty() {
            return Err(whole(ToolError::usage("batch: no calls")));
        }
        if calls.len() as u64 > cap {
            return Err(whole(SessionError::VerbTooLarge {
                what: "the number of calls in a batch",
                size: calls.len() as u64,
                limit: cap,
            }));
        }
        let parsed = self.batch_parse(calls)?;
        let dry = f.dry_run;
        let now = self.now();
        // 1. admit, under the lock
        let (pid, agent, auto, path, mut t, mut cheap) = {
            let mut st = self.lock().map_err(whole)?;
            let st = &mut *st;
            let i = if dry {
                // a dry run is charged as a read: no edit token
                let i = self
                    .admit(st, conn, Need::Edit, Action::WriteBase, now)
                    .map_err(whole)?;
                if let Some(reason) = st.disk.as_ref().and_then(|d| d.declined.clone()) {
                    return Err(whole(SessionError::Declined(Declined {
                        code: "declined",
                        message: reason,
                    })));
                }
                i
            } else {
                self.admit_write(st, conn, Action::WriteBase, now)
                    .map_err(whole)?
            };
            self.check_agent_verb(st, i).map_err(whole)?;
            let pid = st.members[i].p.pid.clone();
            self.check_lease(st, &pid, now).map_err(whole)?;
            for (j, p) in parsed.iter().enumerate() {
                self.check_own_lease(st, &pid, p.verb)
                    .map_err(|e| at_call(j + 1, e))?;
            }
            Self::check_verb_guards(st, &f).map_err(whole)?;
            // the batch plans with no lease (the planted fault: the automatic lease held
            // across the whole run)
            let auto = !dry && planted!(BatchLeaseWholeRun) && st.lease.is_none();
            if auto {
                self.grant(st, i, self.config.lease_auto_ms.max(1), now, true);
            }
            (
                pid,
                st.members[i].p.kind == ParticipantKind::Agent,
                auto,
                st.key.path.clone(),
                self.target_of(st, None).map_err(whole)?,
                Self::cheap_set(st, Some(i)),
            )
        };
        let mut seed = None;
        let go = Go {
            conn,
            parsed: &parsed,
            f: &f,
            opts,
            as_data,
            spec: &spec,
            pid: &pid,
            agent,
            path: &path,
        };
        let out = self.batch_planned(&go, &mut t, &mut cheap, &mut seed, info);
        if auto {
            if let Ok(mut st) = self.lock() {
                if st.lease.as_ref().is_some_and(|l| l.auto && l.pid == pid) {
                    self.end_lease(&mut st, LeaseEnd::Released);
                }
            }
        }
        // a test's call in the gap before the version's index is recorded
        #[cfg(any(test, feature = "differential"))]
        if seed.is_some() {
            crate::differential::seed_gap_hook();
        }
        // the version's index, from the final stage the check already read, off the lock
        if let Some((version, draft)) = seed {
            self.seed_index(version, draft);
        }
        out
    }

    /// Each call's verb and arguments: an edit verb of the pack, none of the batch's own
    /// flags, every `$N` naming an earlier call.
    fn batch_parse(&self, calls: &[BatchCall]) -> std::result::Result<Vec<Parsed>, Stopped> {
        let mut out = Vec::with_capacity(calls.len());
        for (j, c) in calls.iter().enumerate() {
            let k = j + 1;
            let Some((verb, w)) = self.blob_verb_of(&c.argv) else {
                let named = c.argv.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
                return Err(call_usage(
                    k,
                    format!("{named:?} is not a verb of this file type"),
                ));
            };
            if verb.kind == BlobVerbKind::Read {
                return Err(call_usage(
                    k,
                    format!("{} is a read; a batch takes edit verbs only", verb.name),
                ));
            }
            let cf = kit_flags(&c.argv[w..]).map_err(|e| call_usage(k, e))?;
            let own = [
                (cf.if_version.is_some(), "--if-version"),
                (cf.base.is_some(), "--base"),
                (cf.dry_run, "--dry-run"),
                (cf.expect.is_some(), "--expect"),
            ]
            .into_iter()
            .find(|(on, _)| *on);
            if let Some((_, flag)) = own {
                return Err(call_usage(
                    k,
                    format!("{flag} is the batch's own flag: give it once, to the batch"),
                ));
            }
            for a in &cf.rest {
                if a.starts_with("$$") {
                    continue;
                }
                match reference(a) {
                    Some(Ok((n, _))) if n < k => {}
                    Some(Ok(_)) => {
                        return Err(call_usage(
                            k,
                            format!("{a}: names no earlier call (call {k} may name 1 to {j})"),
                        ))
                    }
                    Some(Err(())) => {
                        return Err(call_usage(
                            k,
                            format!("{a}: write $N or $N.k (1-based), or $$ for a $"),
                        ))
                    }
                    None => {}
                }
            }
            out.push(Parsed {
                verb,
                rest: cf.rest,
                json: cf.json,
            });
        }
        Ok(out)
    }

    /// Steps 2 to 5: fold, apply, check, then decide under the lock; again on a moved head.
    fn batch_planned(
        &self,
        go: &Go<'_>,
        t: &mut Target,
        cheap: &mut CheapSet,
        seed: &mut Option<(VersionId, crate::index::IndexDraft)>,
        info: &mut Info,
    ) -> std::result::Result<ToolOutput, Stopped> {
        let f = go.f;
        let dry = f.dry_run;
        let json = go.opts.json || f.json;
        let mut replanned = false;
        let mut replans = 0u32;
        loop {
            // 2. the fold over head, off the lock
            let before = self.load_view(t, go.spec).map_err(whole)?;
            let each = (!dry && planted!(BatchCommitsEach)).then_some(go);
            let mut folded = self.fold(t, &before, go, json, each)?;
            let notes = self.batch_notes(go.parsed, t.ver.size);
            let last_view = folded.view.as_ref().unwrap_or(&before);
            let diff = if folded.spans.is_empty() {
                Vec::new()
            } else {
                iread::diff(&before, last_view)
            };
            if let Some(v) = folded.committed {
                // the planted fault: each call already committed as it went
                let out = self.batch_output(
                    go,
                    t,
                    &folded,
                    &before,
                    &diff,
                    String::new(),
                    folded.size,
                    Some(v),
                    false,
                    notes,
                    info,
                );
                return Ok(out);
            }
            // `--base V` older than head: the whole batch at V too, and what changed since
            let base = match f.base.filter(|v| *v != t.version) {
                Some(v) => Some(self.batch_base(v, t, &before, go, json, &folded)?),
                None => None,
            };
            if folded.spans.is_empty() {
                if let Some((b, at_v)) = &base {
                    self.batch_check_base(b, t, go.spec, &before, &[], &folded, at_v)?;
                }
                return Ok(self.batch_no_change(t, go, &folded, notes, info));
            }
            // the composed plan, against head and the caps
            check_plan(&folded.spans, t.ver.size, &self.config).map_err(whole)?;
            // one check: what changed from head to the last stage is inside what the calls
            // declared (each stage was checked against its own call already)
            if !planted!(StrayUnchecked) {
                let strayed: Vec<String> = diff
                    .iter()
                    .map(|c| c.key.as_str())
                    .filter(|k| !folded.calls.iter().any(|d| d.declares(k)))
                    .map(str::to_owned)
                    .collect();
                if !strayed.is_empty() {
                    return Err(whole(SessionError::Strayed { keys: strayed }));
                }
            }
            let src = self
                .blobs
                .reader_of(&t.space, self.doc_id, &t.ver.blob)
                .map_err(|e| whole(SessionError::from(e)))?;
            let mut applied = Applied::new(src, folded.spans.clone(), true);
            // the doc's lock is only tried, as a write's: a busy doc gets a whole-file version
            let put = self.blobs.put_version(
                &t.space,
                Some(self.doc_id),
                &mut applied,
                LockWait::Try,
                cheap,
                &PackHints(&*self.pack),
            );
            let put = match put {
                Ok(p) => p,
                Err(e) => {
                    let e = applied.failed.take().unwrap_or(e);
                    return Err(whole(SessionError::from(e)));
                }
            };
            let digest = applied.digest_hex();
            let size_after = applied.written;
            let checked = match &f.expect {
                Some(x) if *x != digest => Err(whole(SessionError::NotAsPreviewed {
                    expected: x.clone(),
                    actual: digest.clone(),
                })),
                _ => Ok(()),
            };
            let checked = checked.and_then(|()| match &base {
                Some((b, at_v)) => {
                    self.batch_check_base(b, t, go.spec, &before, &diff, &folded, at_v)
                }
                None => Ok(()),
            });
            if let Err(e) = checked {
                self.blobs.finish(&t.space, put);
                return Err(e);
            }
            // 4 and 5. under the lock: the call as it stands now, then the commit
            let now = self.now();
            let mut guard = match self.lock() {
                Ok(g) => g,
                Err(e) => {
                    self.blobs.finish(&t.space, put);
                    return Err(whole(e));
                }
            };
            let st = &mut *guard;
            let i = match self.batch_decide(st, go, t, &folded, now) {
                Err((at, e)) => {
                    self.blobs.finish(&t.space, put);
                    return Err(stopped(at, Err(e.into())));
                }
                Ok(Decision::Moved) => {
                    let actual = st.vid;
                    self.blobs.finish(&t.space, put);
                    // `--if-version` names head; `--base` is checked per node again on the new
                    // head, as often as head moves (bounded); a plain batch plans once more
                    replans += 1;
                    let more = if f.base.is_some() {
                        replans <= BASE_REPLANS_MAX
                    } else {
                        !replanned
                    };
                    if !more || f.if_version.is_some() {
                        return Err(whole(SessionError::Stale {
                            expected: f.if_version.or(f.base).unwrap_or(t.version),
                            actual,
                        }));
                    }
                    // once more, on the new head: nothing was committed, so nothing applies
                    // twice
                    replanned = true;
                    *t = self.target_of(st, None).map_err(whole)?;
                    let i = Self::member(st, go.conn).map_err(whole)?;
                    *cheap = Self::cheap_set(st, Some(i));
                    continue;
                }
                Ok(Decision::Go(i)) => i,
            };
            let put_info = put.info();
            // the same bytes as head: nothing to record, dry run or not
            if put_info.id == st.cur.blob {
                self.blobs.finish(&t.space, put);
                return Ok(self.batch_no_change(t, go, &folded, notes, info));
            }
            if dry {
                self.blobs.finish(&t.space, put);
                return Ok(self.batch_output(
                    go, t, &folded, &before, &diff, digest, size_after, None, true, notes, info,
                ));
            }
            let p = &st.members[i].p;
            let mut v = BlobVersion::new(
                put_info.id,
                put_info.size,
                now,
                p.pid.clone(),
                p.name.clone(),
                p.via.clone(),
                BlobReason::Write,
            );
            v.on_behalf_of = p.on_behalf_of.clone();
            v.turn = go.opts.turn;
            v.verb = Some("batch".to_owned());
            v.calls = go.parsed.iter().map(|p| p.verb.name.to_owned()).collect();
            // the automatic lease, around the commit only
            let auto = st.lease.is_none();
            if auto {
                self.grant(st, i, self.config.lease_auto_ms.max(1), now, true);
            }
            let committed = self.commit_own(st, i, v, now, put.chunked());
            if auto && st.lease.as_ref().is_some_and(|l| l.auto && l.pid == go.pid) {
                self.end_lease(st, LeaseEnd::Released);
            }
            // the put stops waiting: referenced by the version, or given up
            self.blobs.finish(&t.space, put);
            let r = committed.map_err(whole)?;
            if r.changed {
                *seed = folded.draft.take().map(|d| (r.version, d));
            }
            return Ok(self.batch_output(
                go,
                t,
                &folded,
                &before,
                &diff,
                digest,
                size_after,
                Some(r.version),
                false,
                notes,
                info,
            ));
        }
    }

    /// **The fold**: each call run on the stage the calls before it left (`$N` resolved from
    /// their created nodes), its plan checked against that stage, composed into the ranges
    /// over `t`'s bytes, and the new stage indexed and diffed with the one before (a node the
    /// call didn't declare refuses at that call). `each`: the planted fault that commits each
    /// call as it goes.
    fn fold(
        &self,
        t: &Target,
        head: &IndexView,
        go: &Go<'_>,
        json: bool,
        each: Option<&Go<'_>>,
    ) -> std::result::Result<Folded, Stopped> {
        let c = &self.config;
        let mut spans: Vec<Span> = Vec::new();
        let mut size = t.ver.size;
        let mut view: Option<IndexView> = None;
        let mut draft = None;
        let mut calls: Vec<Done> = Vec::with_capacity(go.parsed.len());
        let mut committed = None;
        let on_head = planted!(BatchPlansOnHead);
        for (j, p) in go.parsed.iter().enumerate() {
            let k = j + 1;
            let args = resolve(&p.rest, &calls).map_err(|m| call_usage(k, m))?;
            let prev: &IndexView = view.as_ref().unwrap_or(head);
            // the planted fault: every call plans on head
            let (in_spans, in_size, in_view): (&[Span], u64, &IndexView) = if on_head {
                (&[], t.ver.size, head)
            } else {
                (&spans, size, prev)
            };
            let ran = self
                .run_verb_staged(t, in_spans, in_size, in_view, p.verb, &args, json || p.json)
                .map_err(|e| at_call(k, e))?;
            let out = match ran {
                Ran::Out(o) => o,
                Ran::Refused { entry, why, next } => {
                    return Err(stopped(
                        Some(k),
                        Ok(Self::pack_refused(
                            p.verb, entry, why, next, t.version, go.path,
                        )),
                    ))
                }
            };
            let plan = out.plan.clone().unwrap_or_default();
            if plan.is_empty() {
                calls.push(Done {
                    verb: p.verb,
                    out,
                    changed: BTreeSet::new(),
                    reach: BTreeSet::new(),
                });
                continue;
            }
            check_plan(&plan.spans, in_size, c).map_err(|e| at_call(k, e))?;
            let next = if planted!(BatchComposeWrong) {
                Some(compose_by_offset(&spans, &plan.spans))
            } else {
                compose(&spans, &plan.spans, t.ver.size)
            };
            let bad = |why: &str| {
                at_call(
                    k,
                    SessionError::BadPlan {
                        why: format!("the batch's ranges after call {k}: {why}"),
                    },
                )
            };
            let next = next.ok_or_else(|| bad("a range lies past the stage"))?;
            check_plan(&next, t.ver.size, c).map_err(|e| at_call(k, e))?;
            let next_size =
                size_with(t.ver.size, &next).ok_or_else(|| bad("they remove more than head"))?;
            // the new stage, read with the pack's own index, diffed with the one before
            let staged = BlobPlan {
                spans: next.clone(),
            };
            let (after, diff, d) = self
                .check_output(t, go.spec, prev, &staged, &out, next_size)
                .map_err(|e| at_call(k, e))?;
            let changed: BTreeSet<NodeKey> = diff.iter().map(|c| c.key.clone()).collect();
            let reach = if planted!(ReachIgnored) {
                changed.clone()
            } else {
                reach_of(
                    &*self.pack,
                    &[head, prev, &after],
                    &changed,
                    !planted!(ReachAtHoldTime),
                )
            };
            if let Some(g) = each {
                let v = self
                    .plant_commit_stage(g, t, &next, p.verb)
                    .map_err(|e| at_call(k, e))?;
                committed = Some(v);
            }
            spans = next;
            size = next_size;
            view = Some(after);
            draft = d;
            calls.push(Done {
                verb: p.verb,
                out,
                changed,
                reach,
            });
        }
        Ok(Folded {
            spans,
            size,
            view,
            draft,
            calls,
            committed,
        })
    }

    /// The planted fault [`crate::differential::Tier0VerbMutant::BatchCommitsEach`]: a stage
    /// committed as its own version, with its call's verb.
    fn plant_commit_stage(
        &self,
        go: &Go<'_>,
        t: &Target,
        spans: &[Span],
        verb: &BlobVerb,
    ) -> Result<VersionId> {
        let src = self.blobs.reader_of(&t.space, self.doc_id, &t.ver.blob)?;
        let mut applied = Applied::new(src, spans.to_vec(), false);
        let cheap = {
            let st = self.lock()?;
            let i = Self::member(&st, go.conn)?;
            Self::cheap_set(&st, Some(i))
        };
        let put = self.blobs.put_version(
            &t.space,
            Some(self.doc_id),
            &mut applied,
            LockWait::Try,
            &cheap,
            &PackHints(&*self.pack),
        )?;
        let now = self.now();
        let mut guard = self.lock()?;
        let st = &mut *guard;
        let i = Self::member(st, go.conn)?;
        let put_info = put.info();
        let p = &st.members[i].p;
        let mut v = BlobVersion::new(
            put_info.id,
            put_info.size,
            now,
            p.pid.clone(),
            p.name.clone(),
            p.via.clone(),
            BlobReason::Write,
        );
        v.verb = Some(verb.name.to_owned());
        let r = self.commit_own(st, i, v, now, put.chunked());
        self.blobs.finish(&t.space, put);
        Ok(r?.version)
    }

    /// `--base V` for a batch: the whole batch folded at V too (a pack's refusal there names
    /// its call), then the base side as a single verb's (the outside saves since V, what
    /// changed since). The planted fault
    /// [`crate::differential::Tier0VerbMutant::BatchBaseLastCallOnly`] keeps only the last
    /// call's nodes.
    fn batch_base(
        &self,
        v: VersionId,
        t: &Target,
        head: &IndexView,
        go: &Go<'_>,
        json: bool,
        at_head: &Folded,
    ) -> std::result::Result<(BaseSide, Folded), Stopped> {
        let vt = self.base_target(v, t).map_err(whole)?;
        let view_v = self.load_view(&vt, go.spec).map_err(whole)?;
        let at_v = self.fold(&vt, &view_v, go, json, None)?;
        let mut d_v = match &at_v.view {
            Some(last) if !at_v.spans.is_empty() => iread::diff(&view_v, last),
            _ => Vec::new(),
        };
        if planted!(BatchBaseLastCallOnly) {
            d_v.retain(|c| last_call_holds(at_head, &at_v, c.key.as_str()));
        }
        let touched: Vec<String> = at_v
            .calls
            .iter()
            .flat_map(|d| d.out.touched.iter().chain(&d.out.created))
            .cloned()
            .collect();
        let side = self
            .base_side_from(v, t, go.spec, head, view_v, d_v, touched)
            .map_err(whole)?;
        Ok((side, at_v))
    }

    /// The per-node `--base` check of a batch (the head side's change `d_h`): a conflict or a
    /// weak key names the first call holding one of its nodes.
    #[allow(clippy::too_many_arguments)]
    fn batch_check_base(
        &self,
        b: &BaseSide,
        t: &Target,
        spec: &IndexSpec,
        head: &IndexView,
        d_h: &[NodeChange],
        at_head: &Folded,
        at_v: &Folded,
    ) -> std::result::Result<(), Stopped> {
        let kept: Vec<NodeChange>;
        let d_h = if planted!(BatchBaseLastCallOnly) {
            kept = d_h
                .iter()
                .filter(|c| last_call_holds(at_head, at_v, c.key.as_str()))
                .cloned()
                .collect();
            &kept[..]
        } else {
            d_h
        };
        self.check_base(b, t, spec, head, d_h).map_err(|e| {
            let named: &[String] = match &e {
                SessionError::BaseMoved { addresses, .. } => addresses,
                SessionError::BaseWeakKey { keys, .. } => keys,
                _ => &[],
            };
            let at = (0..at_head.calls.len().max(at_v.calls.len()))
                .find(|&j| {
                    named.iter().any(|a| {
                        at_head.calls.get(j).is_some_and(|d| d.holds(a))
                            || at_v.calls.get(j).is_some_and(|d| d.holds(a))
                    })
                })
                .map(|j| j + 1);
            stopped(at, Err(e.into()))
        })
    }

    /// Under the lock, before the commit: the caller as they stand now (rights, the lease,
    /// each `merge: Lease` call's own lease), head (moved since the fold?), then each call's
    /// claims in order ([`ClaimPolicy::Respect`], an agent's batch), naming the call.
    fn batch_decide(
        &self,
        st: &mut BState,
        go: &Go<'_>,
        t: &Target,
        folded: &Folded,
        now: u64,
    ) -> std::result::Result<Decision, (Option<usize>, SessionError)> {
        let i = self
            .admit(st, go.conn, Need::Edit, Action::WriteBase, now)
            .map_err(|e| (None, e))?;
        self.check_lease(st, go.pid, now).map_err(|e| (None, e))?;
        for (j, p) in go.parsed.iter().enumerate() {
            self.check_own_lease(st, go.pid, p.verb)
                .map_err(|e| (Some(j + 1), e))?;
        }
        if st.vid != t.version && !planted!(CommitWithoutRecheck) {
            return Ok(Decision::Moved);
        }
        if go.agent && go.opts.policy == ClaimPolicy::Respect && !planted!(ClaimUnchecked) {
            let n = if planted!(BatchClaimsFirstCall) {
                1
            } else {
                folded.calls.len()
            };
            for (j, d) in folded.calls.iter().take(n).enumerate() {
                if let Some(e) = self.node_claim_against(st, go.pid, &d.reach, now) {
                    return Err((Some(j + 1), e));
                }
            }
        }
        Ok(Decision::Go(i))
    }

    /// The notes every call's verb carries, each once.
    fn batch_notes(&self, parsed: &[Parsed], size: u64) -> Vec<(Value, String)> {
        let mut out: Vec<(Value, String)> = Vec::new();
        for p in parsed {
            for n in self.verb_notes(p.verb, size) {
                if !out.iter().any(|(_, t)| *t == n.1) {
                    out.push(n);
                }
            }
        }
        out
    }

    fn batch_calls(folded: &Folded, info: &mut Info) {
        info.calls = folded
            .calls
            .iter()
            .map(|d| (d.verb.name, d.verb.word, d.out.created.clone()))
            .collect();
    }

    fn batch_stdout(folded: &Folded) -> Vec<u8> {
        folded
            .calls
            .iter()
            .flat_map(|d| d.out.stdout.iter().copied())
            .collect()
    }

    fn batch_names(go: &Go<'_>) -> String {
        go.parsed
            .iter()
            .map(|p| p.verb.name)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A batch that changes nothing: no version.
    fn batch_no_change(
        &self,
        t: &Target,
        go: &Go<'_>,
        folded: &Folded,
        notes: Vec<(Value, String)>,
        info: &mut Info,
    ) -> ToolOutput {
        Self::batch_calls(folded, info);
        let o = tool_output(
            Self::batch_stdout(folded),
            format!(
                "no change to {} (batch: {}) · v{}",
                go.path,
                Self::batch_names(go),
                t.version
            ),
            exit::OK,
            t.version,
        );
        let rd = crate::tools::ResultData {
            capture: crate::tools::Capture {
                base: Some(t.version),
                ..Default::default()
            },
            blob: Some(Box::default()),
            ..Default::default()
        };
        with_notes(o, rd, notes, go.as_data)
    }

    /// A batch's output (a dry run's, `after` none): the calls' stdout, and the change from
    /// head's index to the last stage's and the composed plan.
    #[allow(clippy::too_many_arguments)]
    fn batch_output(
        &self,
        go: &Go<'_>,
        t: &Target,
        folded: &Folded,
        before: &IndexView,
        diff: &[NodeChange],
        digest: String,
        size_after: u64,
        after: Option<VersionId>,
        dry: bool,
        notes: Vec<(Value, String)>,
        info: &mut Info,
    ) -> ToolOutput {
        Self::batch_calls(folded, info);
        let last = folded.view.as_ref().unwrap_or(before);
        let (units, more): (Vec<ChangedUnit>, usize) = units_of_diff(diff, before, last);
        let removed: u64 = folded.spans.iter().map(|s| s.len).sum();
        let inserted: u64 = folded.spans.iter().map(|s| s.with.len() as u64).sum();
        let changed = Value::obj([
            ("counts", counts_of_diff(diff, before, last)),
            (
                "units",
                Value::List(units.iter().map(ChangedUnit::to_value).collect()),
            ),
            ("units_more", Value::from(more)),
            ("lines", Value::Null),
            (
                "bytes",
                Value::obj([
                    ("size_before", Value::from(t.ver.size)),
                    ("size_after", Value::from(size_after)),
                    ("spans", Value::from(folded.spans.len())),
                    ("removed", Value::from(removed)),
                    ("inserted", Value::from(inserted)),
                    ("after_digest", Value::from(digest.as_str())),
                ]),
            ),
            (
                "word",
                if planted!(WordDropped) {
                    Value::Null
                } else {
                    Value::from("batch")
                },
            ),
        ]);
        let names = Self::batch_names(go);
        let summary = if dry {
            format!(
                "would edit {} (batch: {names}) · v{} (dry run, nothing applied)",
                go.path, t.version
            )
        } else {
            format!(
                "edited {} (batch: {names}) · v{}",
                go.path,
                after.unwrap_or(t.version)
            )
        };
        let viewing = folded
            .calls
            .iter()
            .rev()
            .find_map(|d| d.out.viewing.as_deref());
        let o = tool_output(
            Self::batch_stdout(folded),
            summary,
            exit::OK,
            after.unwrap_or(t.version),
        );
        let rd = crate::tools::ResultData {
            capture: crate::tools::Capture {
                base: Some(t.version),
                units,
                more,
            },
            blob: Some(Box::new(BlobResult {
                after,
                changed: Some(changed),
                view: self.view_of_change(viewing, diff, before, last),
            })),
            ..Default::default()
        };
        with_notes(o, rd, notes, go.as_data)
    }
}

/// Whether the last call (at head or at V) holds the node `key`: the planted fault
/// [`crate::differential::Tier0VerbMutant::BatchBaseLastCallOnly`].
fn last_call_holds(at_head: &Folded, at_v: &Folded, key: &str) -> bool {
    at_head.calls.last().is_some_and(|d| d.holds(key))
        || at_v.calls.last().is_some_and(|d| d.holds(key))
}

/// What every step of one batch needs.
struct Go<'a> {
    conn: ConnId,
    parsed: &'a [Parsed],
    f: &'a KitFlags,
    opts: &'a crate::tools::ToolOptions,
    as_data: bool,
    spec: &'a IndexSpec,
    pid: &'a str,
    agent: bool,
    path: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(src: &[u8], spans: &[Span]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for s in spans {
            out.extend_from_slice(&src[at..s.at as usize]);
            out.extend_from_slice(&s.with);
            at = (s.at + s.len) as usize;
        }
        out.extend_from_slice(&src[at..]);
        out
    }

    fn sp(at: u64, len: u64, with: &str) -> Span {
        Span {
            at,
            len,
            with: with.as_bytes().to_vec(),
        }
    }

    /// Composing equals applying one plan after the other, over many small plans.
    #[test]
    fn compose_equals_applying_in_turn() {
        let head = b"abcdefghij".to_vec();
        let firsts = [
            vec![],
            vec![sp(0, 0, "XY")],
            vec![sp(2, 3, "")],
            vec![sp(1, 1, "LONGER"), sp(5, 2, "z")],
            vec![sp(10, 0, "END")],
            vec![sp(0, 10, "q")],
        ];
        let mut checked = 0;
        for first in &firsts {
            let stage = apply(&head, first);
            let n = stage.len() as u64;
            for at in 0..=n {
                for len in 0..=(n - at).min(3) {
                    for with in ["", "1", "234"] {
                        let second = vec![sp(at, len, with)];
                        let composed = compose(first, &second, head.len() as u64)
                            .expect("a range inside the stage composes");
                        assert_eq!(
                            apply(&head, &composed),
                            apply(&stage, &second),
                            "compose: {first:?} then {second:?}"
                        );
                        check_plan(&composed, head.len() as u64, &Config::default())
                            .expect("the composed ranges are in order and apart");
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 100, "compose: {checked} cases ran");
    }

    #[test]
    fn references_parse() {
        assert_eq!(reference("$1"), Some(Ok((1, 1))));
        assert_eq!(reference("$2.3"), Some(Ok((2, 3))));
        assert_eq!(reference("$0"), Some(Err(())));
        assert_eq!(reference("$1."), Some(Err(())));
        assert_eq!(reference("$1x"), Some(Err(())));
        assert_eq!(reference("$x"), None);
        assert_eq!(reference("plain"), None);
    }
}
