//! The tier-0 verb path (`docs/SESSIONS.md`, "Verbs on a blob doc"): a file type's own verb,
//! run by the kit on a blob doc. The verb is a pure function from a version's bytes and its
//! arguments to a **plan**, the byte ranges to replace ([`BlobPlan`]); it never writes. The kit:
//!
//! 1. admits the call under the session lock (rights, the edit rate, a declined file, agent
//!    writes, the lease, a verb that needs the caller's own lease, `--if-version`), takes the
//!    write's own lease for its length, and notes head;
//! 2. off the lock, runs the verb on head, checks the plan, and applies it over a verified
//!    reader of head, streaming into the blob store (uncommitted): every byte outside the
//!    ranges is the kit's own copy;
//! 3. off the lock, reads the output with the pack's own index and diffs it with head's: a node
//!    the plan didn't declare is refused, as is an output the index refuses;
//! 4. under the lock again: claims (`ClaimPolicy::Respect`), then, when head moved meanwhile,
//!    one more plan on the new head, else the commit, with the verb's record (`blob_verbs`).
//!
//! A dry run (`--dry-run`) is the same call up to the commit, and stops there: no version, no
//! lease, no rate charge, nothing written (the contract in `docs/SESSIONS.md`, "Dry run: one
//! contract for every tier"). Nothing tells the pack which it is.

use std::collections::BTreeSet;

use super::struct_index::Target;
use super::*;
use crate::errors::{ErrorEntry, ErrorReport, Value};
use crate::index::read::{self as iread, NodeChange, NodeChangeKind};
use crate::index::{DigestStream, IndexNode, IndexView};
use crate::parity::PersonAction;
use crate::session::ClaimPolicy;
use crate::tools::result::BlobResult;
use crate::tools::{exit, ChangedUnit, ToolError, ToolOutput, ToolRunError, UNITS_MAX};

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

/// What a blob verb is ([`BlobVerb::kind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobVerbKind {
    /// It reads: it may return no plan.
    Read,
    /// It edits: its plan makes one version.
    Edit,
}

/// How an edit verb's changes live beside a person's app saving the same file
/// ([`BlobVerb::merge`]; `docs/SESSIONS.md`, "Verbs on a blob doc").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbMerge {
    /// The pack's merge covers the units the verb changes ([`BlobPack::can_merge`]): an outside
    /// save after the verb merges.
    Pack,
    /// The verb runs only while the caller holds the lease themselves (`MPC-E718` without it):
    /// an outside save meanwhile keeps both copies.
    Lease,
}

/// How a verb reads its version ([`BlobVerb::input`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbInput {
    /// As a verified stream, never held whole: any size.
    Stream,
    /// Whole, in memory: refused over [`Config::blob_verb_max_bytes`] (`MPC-E728`) before it
    /// runs.
    Whole,
}

/// A verb's function: from the context, the version's bytes and the arguments after the verb's
/// words (the kit's flags taken out), to its output or a refusal. **Pure**: the same version
/// and arguments give the same output; no state, no clock, no I/O.
pub type BlobVerbFn = fn(
    &BlobVerbCtx<'_>,
    &mut dyn Read,
    &[String],
) -> std::result::Result<BlobVerbOut, BlobVerbRefused>;

/// **A file type's own verb on a blob doc** ([`BlobPack::verbs`]). Build one with
/// [`BlobVerb::edit`] or [`BlobVerb::read`].
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct BlobVerb {
    /// One or two lowercase words (`part set`, `slide add`).
    pub name: &'static str,
    pub kind: BlobVerbKind,
    /// Its arguments, one line.
    pub usage: &'static str,
    /// Every exit it can end with by itself (the kit's refusals come on top).
    pub exits: &'static [u8],
    /// The person actions it serves.
    pub serves: &'static [PersonAction],
    /// An edit verb's merge; `None` for a read verb.
    pub merge: Option<VerbMerge>,
    pub input: VerbInput,
    pub run: BlobVerbFn,
    /// An edit verb's past-tense word for results (`moved`, `renamed`; [`BlobVerb::with_word`]):
    /// lowercase, never empty (the registry refuses an edit verb without one). Listed in
    /// `capabilities` and in the verb's result (`changed.word`). Empty for a read verb.
    pub word: &'static str,
}

impl BlobVerb {
    /// An edit verb: its plan makes one version.
    pub const fn edit(
        name: &'static str,
        usage: &'static str,
        serves: &'static [PersonAction],
        merge: VerbMerge,
        input: VerbInput,
        run: BlobVerbFn,
    ) -> Self {
        BlobVerb {
            name,
            kind: BlobVerbKind::Edit,
            usage,
            exits: &[exit::OK],
            serves,
            merge: Some(merge),
            input,
            run,
            word: "",
        }
    }

    /// A read verb: it returns no plan.
    pub const fn read(
        name: &'static str,
        usage: &'static str,
        serves: &'static [PersonAction],
        input: VerbInput,
        run: BlobVerbFn,
    ) -> Self {
        BlobVerb {
            name,
            kind: BlobVerbKind::Read,
            usage,
            exits: &[exit::OK],
            serves,
            merge: None,
            input,
            run,
            word: "",
        }
    }

    /// The exits it can end with by itself ([`BlobVerb::exits`]; default 0 only).
    pub const fn with_exits(mut self, exits: &'static [u8]) -> Self {
        self.exits = exits;
        self
    }

    /// An edit verb's past-tense word for results ([`BlobVerb::word`]).
    pub const fn with_word(mut self, word: &'static str) -> Self {
        self.word = word;
        self
    }
}

impl std::fmt::Debug for BlobVerb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobVerb")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("usage", &self.usage)
            .field("exits", &self.exits)
            .field("merge", &self.merge)
            .field("input", &self.input)
            .field("word", &self.word)
            .finish_non_exhaustive()
    }
}

/// The merge bound against a version ([`BlobStatus::merge`], [`BlobVerbCtx::merge`]): an
/// outside save is merged only when each version is at most `max_bytes`
/// ([`Config::blob_merge_max_bytes`]); over it, a save while someone edits keeps both copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MergeBound {
    pub max_bytes: u64,
    /// The version's size.
    pub size: u64,
    /// `size <= max_bytes`.
    pub fits: bool,
}

/// What a verb computes on: the version, its index, and the kit's bounds. It carries no
/// dry-run flag: a verb computes the same thing either way.
pub struct BlobVerbCtx<'a> {
    version: VersionId,
    size: u64,
    index: Option<&'a IndexView>,
    json: bool,
    merge: Option<MergeBound>,
    max_spans: u64,
    max_insert_bytes: u64,
}

impl<'a> BlobVerbCtx<'a> {
    /// The version the verb reads.
    pub fn version(&self) -> VersionId {
        self.version
    }

    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Its structure index (the pack's own, as stored): what a verb resolves addresses
    /// against. `None` only for a read verb of a pack with no index.
    pub fn index(&self) -> Option<&'a IndexView> {
        self.index
    }

    /// The node `addr` names in [`BlobVerbCtx::index`]: by its key, else by its native ID.
    pub fn node(&self, addr: &str) -> Option<&'a IndexNode> {
        let view = self.index?;
        view.nodes
            .iter()
            .find(|n| n.key.as_str() == addr)
            .or_else(|| {
                view.nodes
                    .iter()
                    .find(|n| n.native.as_ref().is_some_and(|k| k.as_str() == addr))
            })
    }

    /// The call asked for JSON (`--json`).
    pub fn json(&self) -> bool {
        self.json
    }

    /// The merge bound against this version, for a pack that can merge (a pack's info verb
    /// shows it); `None` otherwise.
    pub fn merge(&self) -> Option<MergeBound> {
        self.merge
    }

    /// The most ranges a plan may hold ([`Config::blob_verb_max_spans`]).
    pub fn max_spans(&self) -> u64 {
        self.max_spans
    }

    /// The most new bytes a plan may hold ([`Config::blob_verb_max_insert_bytes`]).
    pub fn max_insert_bytes(&self) -> u64 {
        self.max_insert_bytes
    }
}

/// One range of a plan: the `len` bytes at offset `at` of the version read are replaced by
/// `with` (`len` 0: an insert at `at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub at: u64,
    pub len: u64,
    pub with: Vec<u8>,
}

/// **An edit verb's plan**: its ranges, in order of `at`, none overlapping (an insert at the
/// offset where a replaced range starts goes before it). The output is the version read with
/// each range replaced, and every other byte copied by the kit. No range: no change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlobPlan {
    pub spans: Vec<Span>,
}

impl BlobPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a range: replace `len` bytes at `at` with `with`.
    pub fn span(mut self, at: u64, len: u64, with: impl Into<Vec<u8>>) -> Self {
        self.spans.push(Span {
            at,
            len,
            with: with.into(),
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// What a verb returns ([`BlobVerbFn`]).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BlobVerbOut {
    /// An edit verb's plan; `None` (or empty) for a read and for an edit that changes nothing.
    pub plan: Option<BlobPlan>,
    /// Its output for a person (a read's answer, an edit's one line).
    pub stdout: Vec<u8>,
    /// Its payload as data (the result's `data`); `None`: stdout as text.
    pub data: Option<Value>,
    /// The nodes it changes, by key: the kit refuses an output whose index diff names any
    /// other (`MPC-E726`).
    pub touched: Vec<String>,
    /// The nodes it adds, by key.
    pub created: Vec<String>,
    /// The address its result is about, when the pack knows better than the kit.
    pub viewing: Option<String>,
}

impl BlobVerbOut {
    /// A read's output.
    pub fn read(stdout: impl Into<Vec<u8>>) -> Self {
        BlobVerbOut {
            stdout: stdout.into(),
            ..Self::default()
        }
    }

    /// An edit's output: its plan and stdout.
    pub fn edit(plan: BlobPlan, stdout: impl Into<Vec<u8>>) -> Self {
        BlobVerbOut {
            plan: Some(plan),
            stdout: stdout.into(),
            ..Self::default()
        }
    }

    /// Declare a node it changes, by key.
    pub fn touching(mut self, key: impl Into<String>) -> Self {
        self.touched.push(key.into());
        self
    }

    /// Declare a node it adds, by key.
    pub fn creating(mut self, key: impl Into<String>) -> Self {
        self.created.push(key.into());
        self
    }

    /// Its payload as data.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The address its result is about.
    pub fn viewing(mut self, addr: impl Into<String>) -> Self {
        self.viewing = Some(addr.into());
        self
    }
}

/// A verb's refusal ([`BlobVerbFn`]): nothing is written.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BlobVerbRefused {
    /// Its arguments don't parse: exit 2.
    Usage(String),
    /// A refusal by one of the pack's own ids ([`BlobPack::errors`]): its exit, `why` as the
    /// message, and `next` what to do.
    Pack {
        entry: &'static ErrorEntry,
        why: String,
        next: String,
    },
}

impl BlobVerbRefused {
    pub fn usage(msg: impl Into<String>) -> Self {
        Self::Usage(msg.into())
    }

    pub fn pack(
        entry: &'static ErrorEntry,
        why: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self::Pack {
            entry,
            why: why.into(),
            next: next.into(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// the kit's flags
// ---------------------------------------------------------------------------------------

/// The kit's own flags, taken out of a verb's arguments before the pack sees them.
#[derive(Debug, Default)]
struct KitFlags {
    if_version: Option<VersionId>,
    base: Option<VersionId>,
    dry_run: bool,
    expect: Option<String>,
    json: bool,
    /// The arguments the pack sees.
    rest: Vec<String>,
}

fn kit_flags(args: &[String]) -> std::result::Result<KitFlags, ToolError> {
    let (if_version, base, args) = crate::tools::take_guards(args)?;
    let mut f = KitFlags {
        if_version,
        base,
        ..KitFlags::default()
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--" => {
                f.rest.push(a.clone());
                f.rest.extend(it.cloned());
                break;
            }
            "--dry-run" => f.dry_run = true,
            "--json" => f.json = true,
            "--expect" => {
                let v = it
                    .next()
                    .ok_or_else(|| ToolError::usage("option '--expect' requires an argument"))?;
                f.expect = Some(v.clone());
            }
            s => match s.strip_prefix("--expect=") {
                Some(v) => f.expect = Some(v.to_owned()),
                None => f.rest.push(a.clone()),
            },
        }
    }
    Ok(f)
}

/// Whether a blob verb's arguments ask for a dry run (`--dry-run` before any `--`).
pub(super) fn wants_dry_run(args: &[String]) -> bool {
    kit_flags(args).is_ok_and(|f| f.dry_run)
}

/// Whether a blob verb's arguments ask for its result as data (`--json` before any `--`).
pub(super) fn wants_json(args: &[String]) -> bool {
    kit_flags(args).is_ok_and(|f| f.json)
}

// ---------------------------------------------------------------------------------------
// the applier
// ---------------------------------------------------------------------------------------

/// How much of the version the applier reads at a time.
const CHUNK: usize = 64 << 10;

/// The version read with a plan's ranges replaced: every other byte copied from a verified
/// reader of it, in order. Optionally hashed as it goes (the output's digest).
struct Applied {
    src: BlobReader,
    spans: Vec<Span>,
    next: usize,
    /// Bytes of the version consumed.
    pos: u64,
    buf: Vec<u8>,
    at: usize,
    digest: Option<DigestStream>,
    written: u64,
    done: bool,
    /// The version's own read failure (a damaged blob), kept for its id.
    failed: Option<BlobError>,
    off_by_one: bool,
}

impl Applied {
    fn new(src: BlobReader, spans: Vec<Span>, digest: bool) -> Self {
        Applied {
            src,
            spans,
            next: 0,
            pos: 0,
            buf: Vec::new(),
            at: 0,
            digest: digest.then(crate::index::Digest::stream),
            written: 0,
            done: false,
            failed: None,
            off_by_one: planted!(ApplierOffByOne),
        }
    }

    fn src_read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.src.read(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                let b = self.src.error(e);
                let msg = b.to_string();
                self.failed = Some(b);
                Err(io::Error::other(msg))
            }
        }
    }

    /// The next piece of output in `buf` (`done` at the version's end).
    fn fill(&mut self) -> io::Result<()> {
        self.buf.clear();
        self.at = 0;
        loop {
            let Some((at, len)) = self.spans.get(self.next).map(|s| (s.at, s.len)) else {
                let mut chunk = vec![0u8; CHUNK];
                let n = self.src_read(&mut chunk)?;
                if n == 0 {
                    self.done = true;
                } else {
                    self.pos += n as u64;
                    chunk.truncate(n);
                    self.buf = chunk;
                }
                return Ok(());
            };
            if self.pos < at {
                let want = usize::try_from((at - self.pos).min(CHUNK as u64)).unwrap_or(CHUNK);
                let mut chunk = vec![0u8; want];
                let n = self.src_read(&mut chunk)?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the version ended before a range of the plan",
                    ));
                }
                self.pos += n as u64;
                chunk.truncate(n);
                if self.off_by_one && self.pos == at && n >= 2 {
                    let b = chunk[n - 2];
                    chunk.insert(n - 1, b);
                }
                self.buf = chunk;
                return Ok(());
            }
            // at the range (past its start only for a plan taken unchecked): skip what it
            // replaces, then give its bytes
            let mut skip = at.saturating_add(len).saturating_sub(self.pos);
            let mut sink = vec![0u8; CHUNK];
            while skip > 0 {
                let k = usize::try_from(skip.min(CHUNK as u64)).unwrap_or(CHUNK);
                let n = self.src_read(&mut sink[..k])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "a range of the plan ends past the version's end",
                    ));
                }
                self.pos += n as u64;
                skip -= n as u64;
            }
            self.buf = self.spans[self.next].with.clone();
            self.next += 1;
            if !self.buf.is_empty() {
                return Ok(());
            }
        }
    }

    /// The output's digest, `sha256:<hex>`.
    fn digest_hex(&mut self) -> String {
        let d = self
            .digest
            .take()
            .map(DigestStream::finish)
            .unwrap_or_else(|| crate::index::Digest::of(b""));
        let mut s = String::with_capacity(7 + 64);
        s.push_str("sha256:");
        for b in d.as_bytes() {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Read the rest, to the version's verified end.
    fn drain(&mut self) -> std::result::Result<(), BlobError> {
        let mut sink = vec![0u8; CHUNK];
        loop {
            match self.read(&mut sink) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(self.failure(e)),
            }
        }
    }

    /// What a failed read was: the version's own (a damaged blob), else the plan's.
    fn failure(&mut self, e: io::Error) -> BlobError {
        self.failed
            .take()
            .unwrap_or_else(|| BlobError::Io(format!("applying the plan: {e}")))
    }
}

impl Read for Applied {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.at >= self.buf.len() {
            if self.done {
                return Ok(0);
            }
            self.fill()?;
        }
        let n = (self.buf.len() - self.at).min(out.len());
        out[..n].copy_from_slice(&self.buf[self.at..self.at + n]);
        if let Some(d) = self.digest.as_mut() {
            d.update(&out[..n]);
        }
        self.at += n;
        self.written += n as u64;
        Ok(n)
    }
}

/// Read a verified stream to its end: only a clean end proves the bytes.
fn drain_verified(reader: &mut BlobReader) -> Result<()> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(reader.error(e).into()),
        }
    }
}

/// A plan the kit takes: ranges in order and apart, inside the version, within the caps.
pub(super) fn check_plan(spans: &[Span], size: u64, c: &Config) -> Result<()> {
    let bad = |why: String| Err(SessionError::BadPlan { why });
    if spans.len() as u64 > c.blob_verb_max_spans {
        return bad(format!(
            "{} ranges, over the limit of {}",
            spans.len(),
            c.blob_verb_max_spans
        ));
    }
    let inserted: u64 = spans.iter().map(|s| s.with.len() as u64).sum();
    if inserted > c.blob_verb_max_insert_bytes {
        return bad(format!(
            "{inserted} new bytes, over the limit of {}",
            c.blob_verb_max_insert_bytes
        ));
    }
    let mut end_before = 0u64;
    for (k, s) in spans.iter().enumerate() {
        let Some(end) = s.at.checked_add(s.len).filter(|e| *e <= size) else {
            return bad(format!(
                "range {} ends past the version's end ({size} bytes)",
                k + 1
            ));
        };
        if k > 0 && s.at < end_before {
            return bad(format!(
                "range {} starts before the one before it ends (out of order or overlapping)",
                k + 1
            ));
        }
        end_before = end;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// the result
// ---------------------------------------------------------------------------------------

/// The changed units of an index diff, by node (the result's `changed.units`): an added node
/// is an `insert`, a removed one a `delete`, a moved one a `move`, a renamed one or one with
/// other content or text a `set`, and each changed property a `set` of that field (`before`
/// and `after` its values for `attrs.<name>`). At most [`UNITS_MAX`], and how many more.
fn units_of_diff(
    diff: &[NodeChange],
    before: &IndexView,
    after: &IndexView,
) -> (Vec<ChangedUnit>, usize) {
    let node = |v: &IndexView, k: &str| v.nodes.iter().find(|n| n.key.as_str() == k).cloned();
    let mut units = Vec::new();
    let mut more = 0usize;
    let mut push = |key: &str, op: &'static str, field: Option<String>, b, a| {
        if units.len() < UNITS_MAX {
            units.push(ChangedUnit {
                address: Some(key.to_owned()),
                root: "index".to_owned(),
                key: key.to_owned(),
                field,
                op,
                before: b,
                after: a,
            });
        } else {
            more += 1;
        }
    };
    for c in diff {
        let k = c.key.as_str();
        if c.changes.contains(&NodeChangeKind::Added) {
            push(k, "insert", None, None, None);
            continue;
        }
        if c.changes.contains(&NodeChangeKind::Removed) {
            push(k, "delete", None, None, None);
            continue;
        }
        if c.changes.contains(&NodeChangeKind::Moved) {
            push(k, "move", None, None, None);
        }
        if c.changes.iter().any(|x| {
            matches!(
                x,
                NodeChangeKind::Renamed | NodeChangeKind::Content | NodeChangeKind::Text
            )
        }) {
            push(k, "set", None, None, None);
        }
        let (was, now) = (node(before, k), node(after, k));
        for p in &c.props {
            let (b, a) = match p.strip_prefix("attrs.") {
                Some(name) => (
                    was.as_ref().and_then(|n| n.attrs.get(name).cloned()),
                    now.as_ref().and_then(|n| n.attrs.get(name).cloned()),
                ),
                None => (None, None),
            };
            push(k, "set", Some(p.clone()), b, a);
        }
    }
    (units, more)
}

/// Per node kind: how many nodes the diff adds, removes and changes.
fn counts_of_diff(diff: &[NodeChange], before: &IndexView, after: &IndexView) -> Value {
    let kind_of = |k: &str| {
        after
            .nodes
            .iter()
            .chain(&before.nodes)
            .find(|n| n.key.as_str() == k)
            .map_or_else(String::new, |n| n.kind.clone())
    };
    let mut by: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();
    for c in diff {
        let e = by.entry(kind_of(c.key.as_str())).or_default();
        if c.changes.contains(&NodeChangeKind::Added) {
            e.0 += 1;
        } else if c.changes.contains(&NodeChangeKind::Removed) {
            e.1 += 1;
        } else {
            e.2 += 1;
        }
    }
    Value::List(
        by.into_iter()
            .map(|(kind, (a, r, c))| {
                Value::obj([
                    ("kind", Value::from(kind)),
                    ("added", Value::from(a)),
                    ("removed", Value::from(r)),
                    ("changed", Value::from(c)),
                ])
            })
            .collect(),
    )
}

/// What one edit computed, for its result.
struct Edited<'a> {
    verb: &'a BlobVerb,
    path: &'a str,
    base: VersionId,
    size_before: u64,
    out: BlobVerbOut,
    plan: &'a BlobPlan,
    before: &'a IndexView,
    after: &'a IndexView,
    diff: &'a [NodeChange],
    digest: String,
    size_after: u64,
}

fn tool_output(stdout: Vec<u8>, summary: String, exit: u8, version: VersionId) -> ToolOutput {
    ToolOutput {
        stdout,
        stderr_summary: summary,
        exit,
        version,
        changed: None,
        fence: None,
        notes: Default::default(),
        for_edit: None,
        refusal: None,
        warnings: Default::default(),
        result: None,
    }
}

/// A call's notes as data and, for a text result, as `note: ` lines.
fn with_notes(
    mut o: ToolOutput,
    rd: crate::tools::ResultData,
    notes: Vec<(Value, String)>,
    as_data: bool,
) -> ToolOutput {
    let mut rd = rd;
    if !as_data {
        o.warnings
            .extend(notes.iter().map(|(_, t)| format!("note: {t}")));
    }
    rd.notes = notes.into_iter().map(|(v, _)| v).collect();
    o.result = Some(Box::new(rd));
    o
}

// ---------------------------------------------------------------------------------------
// the run
// ---------------------------------------------------------------------------------------

/// What running a verb on a version gave.
pub(super) enum Ran {
    Out(BlobVerbOut),
    Refused {
        entry: &'static ErrorEntry,
        why: String,
        next: String,
    },
}

/// Under the lock again, before the commit: the call as it stands now.
enum Decision {
    /// Head moved since the plan.
    Moved,
    /// Commit (or, a dry run, stop), as the connection at this index.
    Go(usize),
}

impl BlobSession {
    /// The pack's verb `argv` names, and how many words its name takes.
    pub(super) fn blob_verb_of(&self, argv: &[String]) -> Option<(&'static BlobVerb, usize)> {
        let verbs = self.pack.verbs();
        let first = argv.first()?;
        if let Some(second) = argv.get(1) {
            let two = format!("{first} {second}");
            if let Some(v) = verbs.iter().find(|v| v.name == two) {
                return Some((v, 2));
            }
        }
        verbs
            .iter()
            .find(|v| v.name == first.as_str())
            .map(|v| (v, 1))
    }

    /// The merge bound against a version of `size` bytes ([`MergeBound`]); `None` for a pack
    /// that can't merge.
    pub(super) fn merge_bound(&self, size: u64) -> Option<MergeBound> {
        self.pack.can_merge().then(|| {
            let max = self.config.blob_merge_max_bytes;
            MergeBound {
                max_bytes: max,
                size,
                fits: size <= max,
            }
        })
    }

    /// The notes every result of `verb` on a version of `size` bytes carries: `lease_only` on
    /// a `merge: Lease` edit, `merge_over_bound` over the merge bound.
    fn verb_notes(&self, verb: &BlobVerb, size: u64) -> Vec<(Value, String)> {
        let mut out = Vec::new();
        if verb.kind == BlobVerbKind::Edit && verb.merge == Some(VerbMerge::Lease) {
            let text = format!(
                "{} runs only under your own lease: a save by a person's app meanwhile keeps \
                 both copies; release the lease when done",
                verb.name
            );
            out.push((
                Value::obj([
                    ("kind", Value::from("lease_only")),
                    ("text", Value::from(text.as_str())),
                ]),
                text,
            ));
        }
        if let Some(m) = self.merge_bound(size).filter(|m| !m.fits) {
            if !planted!(MergeNoteMissing) {
                let text = "over the merge bound: if a person's app saves this file while you \
                            edit it, both are kept as copies"
                    .to_owned();
                out.push((
                    Value::obj([
                        ("kind", Value::from("merge_over_bound")),
                        ("size", Value::from(m.size)),
                        ("max_bytes", Value::from(m.max_bytes)),
                        ("text", Value::from(text.as_str())),
                    ]),
                    text,
                ));
            }
        }
        out
    }

    /// **Run the pack's verb** `verb` with `args` (after its words), as
    /// [`BlobSession::run_tool`] routes it (`docs/SESSIONS.md`, "Verbs on a blob doc").
    /// `as_data`: the call's result is data (its notes then go there, not to stderr).
    pub(super) fn run_blob_verb(
        &self,
        conn: ConnId,
        verb: &BlobVerb,
        args: &[String],
        opts: &crate::tools::ToolOptions,
        as_data: bool,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let out = self.run_blob_verb_in(conn, verb, args, opts, as_data);
        match out {
            Err(ToolRunError::Session(e)) => {
                let e = match self.for_viewer(conn, Err::<(), SessionError>(e), |_, v| v) {
                    Err(e) => e,
                    Ok(()) => unreachable!("an error stays one"),
                };
                Err(e.into())
            }
            other => other,
        }
    }

    fn run_blob_verb_in(
        &self,
        conn: ConnId,
        verb: &BlobVerb,
        args: &[String],
        opts: &crate::tools::ToolOptions,
        as_data: bool,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        // a pack the registry refuses runs nothing
        crate::parity::Registry::for_blob_pack(&*self.pack)
            .map_err(|e| ToolError::usage(format!("{}: {e}", verb.name)))?;
        let f = kit_flags(args)?;
        match verb.kind {
            BlobVerbKind::Read => {
                if f.if_version.is_some() || f.base.is_some() {
                    return Err(crate::tools::if_version_on_read(verb.name).into());
                }
                if f.dry_run || f.expect.is_some() {
                    return Err(ToolError::usage(format!(
                        "--dry-run and --expect are for edit verbs; {} is a read (it changes \
                         nothing)",
                        verb.name
                    ))
                    .into());
                }
                self.run_blob_read(conn, verb, &f, opts, as_data)
            }
            BlobVerbKind::Edit => self.run_blob_edit(conn, verb, &f, opts, as_data),
        }
    }

    /// Run `verb` on version `t` (its index `view`), reading the version as the verb says.
    pub(super) fn run_verb_on(
        &self,
        t: &Target,
        view: Option<&IndexView>,
        verb: &BlobVerb,
        args: &[String],
        json: bool,
    ) -> std::result::Result<Ran, ToolRunError> {
        let c = &self.config;
        let ctx = BlobVerbCtx {
            version: t.version,
            size: t.ver.size,
            index: view,
            json,
            merge: self.merge_bound(t.ver.size),
            max_spans: c.blob_verb_max_spans,
            max_insert_bytes: c.blob_verb_max_insert_bytes,
        };
        let mut reader = self
            .blobs
            .reader_of(&t.space, self.doc_id, &t.ver.blob)
            .map_err(SessionError::from)?;
        let out = match verb.input {
            VerbInput::Whole => {
                if t.ver.size > c.blob_verb_max_bytes {
                    return Err(SessionError::VerbTooLarge {
                        what: "the version a whole-file verb reads",
                        size: t.ver.size,
                        limit: c.blob_verb_max_bytes,
                    }
                    .into());
                }
                let mut bytes = Vec::new();
                if let Err(e) = reader.read_to_end(&mut bytes) {
                    return Err(SessionError::from(reader.error(e)).into());
                }
                (verb.run)(&ctx, &mut &bytes[..], args)
            }
            VerbInput::Stream => {
                let out = (verb.run)(&ctx, &mut reader, args);
                drain_verified(&mut reader)?;
                out
            }
        };
        match out {
            Ok(o) => Ok(Ran::Out(o)),
            Err(BlobVerbRefused::Usage(m)) => Err(ToolError::usage(m).into()),
            Err(BlobVerbRefused::Pack { entry, why, next }) => {
                Ok(Ran::Refused { entry, why, next })
            }
        }
    }

    /// A pack's refusal by its id, as its output: nothing applied.
    fn pack_refused(
        verb: &BlobVerb,
        entry: &'static ErrorEntry,
        why: String,
        next: String,
        version: VersionId,
        path: &str,
    ) -> ToolOutput {
        let report = ErrorReport::new(entry, why.clone())
            .with("code", entry.name)
            .with("next", next.as_str())
            .with("verb", verb.name);
        let mut o = tool_output(
            format!("refused: {}: {why}\nnext: {next}\n", entry.name).into_bytes(),
            format!("nothing applied to {path} · v{version} ({})", entry.id),
            entry.exit,
            version,
        );
        o.refusal = Some(report);
        o
    }

    fn run_blob_read(
        &self,
        conn: ConnId,
        verb: &BlobVerb,
        f: &KitFlags,
        opts: &crate::tools::ToolOptions,
        as_data: bool,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let now = self.now();
        let (t, path) = {
            let mut st = self.lock()?;
            self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
            (self.target_of(&st, None)?, st.key.path.clone())
        };
        let view = match self.pack.index_spec() {
            Some(spec) => Some(self.load_view(&t, &spec)?),
            None => None,
        };
        let out = match self.run_verb_on(&t, view.as_ref(), verb, &f.rest, opts.json || f.json)? {
            Ran::Refused { entry, why, next } => {
                return Ok(Self::pack_refused(verb, entry, why, next, t.version, &path))
            }
            Ran::Out(o) => o,
        };
        if out.plan.as_ref().is_some_and(|p| !p.is_empty()) {
            return Err(SessionError::BadPlan {
                why: format!(
                    "{}, a read verb, returned byte ranges to replace",
                    verb.name
                ),
            }
            .into());
        }
        let o = tool_output(
            out.stdout,
            format!("{path} v{} ({})", t.version, verb.name),
            exit::OK,
            t.version,
        );
        let rd = crate::tools::ResultData {
            data: out.data,
            ..Default::default()
        };
        Ok(with_notes(
            o,
            rd,
            self.verb_notes(verb, t.ver.size),
            as_data,
        ))
    }

    /// `--if-version V`: head must be `V`. `--base V`: `V` no newer than head (an older one is
    /// checked per node, [`BlobSession::check_base`]).
    fn check_verb_guards(st: &BState, f: &KitFlags) -> Result<()> {
        let stale = f
            .if_version
            .filter(|v| *v != st.vid)
            .or(f.base.filter(|v| *v > st.vid));
        match stale {
            Some(v) => Err(SessionError::Stale {
                expected: v,
                actual: st.vid,
            }),
            None => Ok(()),
        }
    }

    /// A `merge: Lease` verb needs the caller's own lease, never one a write took for itself.
    fn check_own_lease(&self, st: &BState, pid: &str, verb: &BlobVerb) -> Result<()> {
        if verb.merge != Some(VerbMerge::Lease) || planted!(LeaseOnlyIgnored) {
            return Ok(());
        }
        match &st.lease {
            Some(l) if l.pid == pid && !l.auto => Ok(()),
            _ => Err(SessionError::NeedsLease {
                verb: verb.name.to_owned(),
                ttl_ms: self.config.lease_default_ms,
            }),
        }
    }

    fn run_blob_edit(
        &self,
        conn: ConnId,
        verb: &BlobVerb,
        f: &KitFlags,
        opts: &crate::tools::ToolOptions,
        as_data: bool,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let dry = f.dry_run;
        let spec = self
            .pack
            .index_spec()
            .ok_or(SessionError::IndexUnsupported)?;
        let now = self.now();
        // 1. admit, under the lock
        let (pid, agent, auto, path, mut t, mut cheap) = {
            let mut st = self.lock()?;
            let st = &mut *st;
            let i = if dry && !planted!(ChargesRate) {
                // a dry run is charged as a read: no edit token
                let i = self.admit(st, conn, Need::Edit, Action::WriteBase, now)?;
                if let Some(reason) = st.disk.as_ref().and_then(|d| d.declined.clone()) {
                    return Err(SessionError::Declined(Declined {
                        code: "declined",
                        message: reason,
                    })
                    .into());
                }
                i
            } else {
                self.admit_write(st, conn, Action::WriteBase, now)?
            };
            self.check_agent_verb(st, i)?;
            let pid = st.members[i].p.pid.clone();
            self.check_lease(st, &pid, now)?;
            self.check_own_lease(st, &pid, verb)?;
            if !(dry && planted!(SkipsGuards)) {
                Self::check_verb_guards(st, f)?;
            }
            let auto = (!dry || planted!(TakesLease)) && st.lease.is_none();
            if auto {
                self.grant(st, i, self.config.lease_auto_ms.max(1), now, true);
            }
            (
                pid,
                st.members[i].p.kind == ParticipantKind::Agent,
                auto,
                st.key.path.clone(),
                self.target_of(st, None)?,
                Self::cheap_set(st, Some(i)),
            )
        };
        let mut seed = None;
        let out = self.edit_planned(
            conn, verb, f, opts, as_data, &spec, &pid, agent, &path, &mut t, &mut cheap, &mut seed,
        );
        // the write's own lease ends with it (a dry run takes none)
        if auto && !(dry && planted!(TakesLease)) {
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
        // the version's index, from the output the check already read, off the lock
        if let Some((version, draft)) = seed {
            self.seed_index(version, draft);
        }
        out
    }

    /// Steps 2 to 5: plan, apply, check, then decide under the lock; once more on a moved
    /// head.
    #[allow(clippy::too_many_arguments)]
    fn edit_planned(
        &self,
        conn: ConnId,
        verb: &BlobVerb,
        f: &KitFlags,
        opts: &crate::tools::ToolOptions,
        as_data: bool,
        spec: &IndexSpec,
        pid: &str,
        agent: bool,
        path: &str,
        t: &mut Target,
        cheap: &mut CheapSet,
        seed: &mut Option<(VersionId, crate::index::IndexDraft)>,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let dry = f.dry_run;
        let json = opts.json || f.json;
        let mut replanned = false;
        loop {
            // 2. plan, off the lock
            let before = match self.load_view(t, spec) {
                // the planted fault: head's index not recorded yet, the version before it read
                Err(SessionError::IndexPending { .. })
                    if planted!(SeedGapLoses)
                        && plant_on!(crate::differential::in_seed_gap())
                        && t.version > 1 =>
                {
                    let prev = self.index_target(Some(t.version - 1))?;
                    let view = self.load_view(&prev, spec)?;
                    t.ver = prev.ver;
                    view
                }
                r => r?,
            };
            // `--base V` older than head: the call at V, and what changed since (per node)
            let base = match f.base.filter(|v| *v != t.version) {
                Some(v) => match self.base_side(v, t, spec, &before, verb, &f.rest, json)? {
                    super::base::BaseRun::Side(b) => Some(b),
                    super::base::BaseRun::Refused { entry, why, next } => {
                        return Ok(Self::pack_refused(verb, entry, why, next, t.version, path))
                    }
                },
                None => None,
            };
            let out = match self.run_verb_on(t, Some(&before), verb, &f.rest, json)? {
                Ran::Refused { entry, why, next } => {
                    return Ok(Self::pack_refused(verb, entry, why, next, t.version, path))
                }
                Ran::Out(o) => o,
            };
            let notes = self.verb_notes(verb, t.ver.size);
            let plan = out.plan.clone().unwrap_or_default();
            if plan.is_empty() {
                if let Some(b) = &base {
                    self.check_base(b, t, spec, &before, &[])?;
                }
                return Ok(self.no_change(verb, t, path, out, notes, as_data));
            }
            if !planted!(PlanUnchecked) {
                check_plan(&plan.spans, t.ver.size, &self.config)?;
            }
            let src = self
                .blobs
                .reader_of(&t.space, self.doc_id, &t.ver.blob)
                .map_err(SessionError::from)?;
            let mut applied = Applied::new(src, plan.spans.clone(), true);
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
                    return Err(SessionError::from(e).into());
                }
            };
            let digest = applied.digest_hex();
            let size_after = applied.written;
            // 3. check the output with the pack's own index; from here the put is finished
            // on every path
            let checked = self
                .check_output(t, spec, &before, &plan, &out, size_after)
                .and_then(|(after, diff, draft)| match &f.expect {
                    Some(x) if *x != digest => Err(SessionError::NotAsPreviewed {
                        expected: x.clone(),
                        actual: digest.clone(),
                    }),
                    _ => Ok((after, diff, draft)),
                });
            // the per-node `--base` check, with the change at head
            let checked = checked.and_then(|x| match &base {
                Some(b) => self.check_base(b, t, spec, &before, &x.1).map(|()| x),
                None => Ok(x),
            });
            let (after, diff, draft) = match checked {
                Ok(x) => x,
                Err(e) => {
                    self.blobs.finish(&t.space, put);
                    return Err(e.into());
                }
            };
            // the nodes the change reaches into for the claim check (each node's hold reach,
            // read at the version the verb runs on and in its output), off the lock
            let touched: BTreeSet<crate::index::NodeKey> =
                diff.iter().map(|c| c.key.clone()).collect();
            let reach = if planted!(ReachIgnored) {
                touched
            } else {
                super::node_claims::reach_of(
                    &*self.pack,
                    &[&before, &after],
                    &touched,
                    !planted!(ReachAtHoldTime),
                )
            };
            // 4 and 5. under the lock: the call as it stands now, then the commit
            let now = self.now();
            let mut guard = match self.lock() {
                Ok(g) => g,
                Err(e) => {
                    self.blobs.finish(&t.space, put);
                    return Err(e.into());
                }
            };
            let st = &mut *guard;
            let decided = self.decide(st, conn, pid, agent, verb, t, &reach, opts, now);
            let i = match decided {
                Err(e) => {
                    self.blobs.finish(&t.space, put);
                    return Err(e.into());
                }
                Ok(Decision::Moved) => {
                    let actual = st.vid;
                    self.blobs.finish(&t.space, put);
                    // `--if-version` names head; `--base` is checked per node again on the new
                    // head
                    if replanned || f.if_version.is_some() {
                        return Err(SessionError::Stale {
                            expected: f.if_version.or(f.base).unwrap_or(t.version),
                            actual,
                        }
                        .into());
                    }
                    // once more, on the new head: nothing was committed, so nothing applies
                    // twice
                    replanned = true;
                    *t = self.target_of(st, None)?;
                    let i = Self::member(st, conn)?;
                    *cheap = Self::cheap_set(st, Some(i));
                    continue;
                }
                Ok(Decision::Go(i)) => i,
            };
            let info = put.info();
            let edited = Edited {
                verb,
                path,
                base: t.version,
                size_before: t.ver.size,
                out,
                plan: &plan,
                before: &before,
                after: &after,
                diff: &diff,
                digest,
                size_after,
            };
            // the same bytes as head: nothing to record, dry run or not
            if info.id == st.cur.blob {
                self.blobs.finish(&t.space, put);
                return Ok(self.no_change(verb, t, path, edited.out, notes, as_data));
            }
            if dry && !planted!(Commits) {
                if planted!(MovesPresence) {
                    st.members[i].last_edit_ms = Some(now);
                }
                if planted!(StoresIndex) {
                    self.want_index(st, t.version + 1, info.id, info.size);
                }
                if planted!(WritesBack) {
                    self.plant_write_back(st, t, &plan);
                }
                if planted!(LeavesPut) {
                    drop(put);
                } else {
                    self.blobs.finish(&t.space, put);
                }
                return Ok(self.edited_output(edited, None, true, notes, as_data));
            }
            let p = &st.members[i].p;
            let mut v = BlobVersion::new(
                info.id,
                info.size,
                now,
                p.pid.clone(),
                p.name.clone(),
                p.via.clone(),
                BlobReason::Write,
            );
            v.on_behalf_of = p.on_behalf_of.clone();
            v.turn = opts.turn;
            v.verb = Some(verb.name.to_owned());
            let committed = self.commit_own(st, i, v, now, put.chunked());
            // the put stops waiting: referenced by the version, or given up
            self.blobs.finish(&t.space, put);
            let r = committed?;
            let skip = plant_on!(
                crate::differential::tier0_verb_mutant()
                    == Some(crate::differential::Tier0VerbMutant::SeedSkipped)
            );
            if r.changed && !skip {
                *seed = draft.map(|d| (r.version, d));
            }
            return Ok(self.edited_output(edited, Some(r.version), dry, notes, as_data));
        }
    }

    /// Under the lock, before the commit: the caller as they stand now (rights, the lease, the
    /// caller's own lease), head (moved since the plan?), then claims
    /// ([`ClaimPolicy::Respect`]: an agent's change reaching a node another participant
    /// claims, `reach` from [`super::node_claims::reach_of`]).
    #[allow(clippy::too_many_arguments)]
    fn decide(
        &self,
        st: &mut BState,
        conn: ConnId,
        pid: &str,
        agent: bool,
        verb: &BlobVerb,
        t: &Target,
        reach: &BTreeSet<crate::index::NodeKey>,
        opts: &crate::tools::ToolOptions,
        now: u64,
    ) -> Result<Decision> {
        let i = self.admit(st, conn, Need::Edit, Action::WriteBase, now)?;
        self.check_lease(st, pid, now)?;
        self.check_own_lease(st, pid, verb)?;
        if st.vid != t.version && !planted!(CommitWithoutRecheck) {
            return Ok(Decision::Moved);
        }
        if agent && opts.policy == ClaimPolicy::Respect && !planted!(ClaimUnchecked) {
            if let Some(e) = self.node_claim_against(st, pid, reach, now) {
                return Err(e);
            }
        }
        Ok(Decision::Go(i))
    }

    /// Step 3: the output read with the pack's own index (a second application of the plan,
    /// read to the version's verified end), diffed with head's; a node the plan didn't declare
    /// refuses. Returns the output's index and the diff.
    pub(super) fn check_output(
        &self,
        t: &Target,
        spec: &IndexSpec,
        before: &IndexView,
        plan: &BlobPlan,
        out: &BlobVerbOut,
        size_after: u64,
    ) -> Result<(IndexView, Vec<NodeChange>, Option<crate::index::IndexDraft>)> {
        let src = self.blobs.reader_of(&t.space, self.doc_id, &t.ver.blob)?;
        let mut applied = Applied::new(src, plan.spans.clone(), false);
        let draft = self.pack.index(
            &mut applied,
            size_after,
            &super::struct_index::limits_of(&self.config),
        );
        applied.drain()?;
        let draft = match draft {
            Ok(d) => d,
            // the planted fault: an output the index refuses is taken, its diff empty
            Err(_) if planted!(OutputIndexSkipped) => {
                return Ok((before.clone(), Vec::new(), None))
            }
            Err(e) => return Err(SessionError::OutputUnreadable { why: e.to_string() }),
        };
        let after = self.output_view(t, spec, &draft, t.version + 1)?;
        let diff = iread::diff(before, &after);
        if !planted!(StrayUnchecked) {
            let declared: BTreeSet<&str> = out
                .touched
                .iter()
                .chain(&out.created)
                .map(String::as_str)
                .collect();
            let strayed: Vec<String> = diff
                .iter()
                .map(|c| c.key.as_str())
                .filter(|k| !declared.contains(k))
                .map(str::to_owned)
                .collect();
            if !strayed.is_empty() {
                return Err(SessionError::Strayed { keys: strayed });
            }
        }
        Ok((after, diff, Some(draft)))
    }

    /// Record the index of the version a verb made from the draft its check read
    /// ([`super::struct_index::IndexJob::seed`]), as the host's job would, so the next verb
    /// needn't wait for that job. Nothing to do when a job is already out for these bytes, the
    /// row exists, or the version is past the index's limits; a refusal there is the job's to
    /// make again (the version is committed either way).
    fn seed_index(&self, version: VersionId, draft: crate::index::IndexDraft) {
        if let Ok(Some(job)) = self.index_job(Some(version)) {
            let _ = job.seed(draft);
        }
    }

    /// The planted fault [`crate::differential::Tier0VerbMutant::WritesBack`]: a dry run puts
    /// its output in the file.
    fn plant_write_back(&self, st: &BState, t: &Target, plan: &BlobPlan) {
        let Some(path) = st.disk.as_ref().map(|d| d.path.clone()) else {
            return;
        };
        let Ok(src) = self.blobs.reader_of(&t.space, self.doc_id, &t.ver.blob) else {
            return;
        };
        let mut bytes = Vec::new();
        if Applied::new(src, plan.spans.clone(), false)
            .read_to_end(&mut bytes)
            .is_ok()
        {
            let _ = fs::write(path, bytes);
        }
    }

    /// **Where the change is** (the result's `view`, `docs/SESSIONS.md`, "Verbs on a blob doc"):
    /// the changed index nodes in the diff's order as its units (live unless removed), their
    /// containers ([`crate::view_index::BlobView::container_kind`], live in the output; the
    /// last touched last), and the primary address: the verb's own (`viewing`, when the address
    /// grammar reads it), else the container of the **last** changed node, else that node. Cells,
    /// lines and time are null on this tier. `None` when nothing changed. A dry run computes the
    /// same, and nothing is set from it.
    fn view_of_change(
        &self,
        viewing: Option<&str>,
        diff: &[NodeChange],
        before: &IndexView,
        after: &IndexView,
    ) -> Option<Value> {
        if diff.is_empty() {
            return None;
        }
        let kind = self
            .pack
            .view_schema()
            .and_then(|s| s.blob)
            .map(|b| b.container_kind)
            .filter(|k| !k.is_empty());
        let container_in = |v: &IndexView, key: &str| -> Option<String> {
            let kind = kind?;
            let mut cur = v.nodes.iter().find(|n| n.key.as_str() == key)?;
            loop {
                if cur.kind == kind {
                    return Some(cur.key.as_str().to_owned());
                }
                let p = cur.parent.as_ref()?;
                cur = v.nodes.iter().find(|n| &n.key == p)?;
            }
        };
        let live = |k: &str| after.nodes.iter().any(|n| n.key.as_str() == k);
        // a node's container, live in the output (a removed node's as it was before)
        let container_of = |k: &str| {
            if live(k) {
                container_in(after, k)
            } else {
                container_in(before, k).filter(|c| live(c))
            }
        };
        let mut units = Vec::new();
        let mut more = 0usize;
        let mut containers: Vec<String> = Vec::new();
        for c in diff {
            let k = c.key.as_str();
            if units.len() < crate::session::ELEM_SELECTION_MAX {
                units.push(crate::session::ViewUnit {
                    address: k.to_owned(),
                    live: !c.changes.contains(&NodeChangeKind::Removed),
                });
            } else {
                more += 1;
            }
            if let Some(cn) = container_of(k) {
                containers.retain(|x| *x != cn);
                containers.push(cn);
            }
        }
        // the planted fault: the first changed node's place, not the last's
        let pick = if planted!(ViewFirstNode) {
            diff.first()
        } else {
            diff.last()
        };
        let node = pick.map(|c| c.key.as_str());
        let address = viewing
            .and_then(crate::session::view_address_ok)
            .or_else(|| node.and_then(container_of))
            .or_else(|| node.map(str::to_owned));
        let view = crate::session::ViewAt {
            address,
            containers,
            units,
            units_more: more,
            cells: None,
            lines: Vec::new(),
            time: None,
        };
        let mut v = view.to_value();
        if let Value::Obj(fields) = &mut v {
            for (k, x) in fields.iter_mut() {
                if k == "lines" {
                    *x = Value::Null;
                }
            }
        }
        Some(v)
    }

    /// An edit that changes nothing (an empty plan, or the same bytes): no version.
    fn no_change(
        &self,
        verb: &BlobVerb,
        t: &Target,
        path: &str,
        out: BlobVerbOut,
        notes: Vec<(Value, String)>,
        as_data: bool,
    ) -> ToolOutput {
        let o = tool_output(
            out.stdout,
            format!("no change to {path} ({}) · v{}", verb.name, t.version),
            exit::OK,
            t.version,
        );
        let rd = crate::tools::ResultData {
            capture: crate::tools::Capture {
                base: Some(t.version),
                ..Default::default()
            },
            data: out.data,
            blob: Some(Box::default()),
            ..Default::default()
        };
        with_notes(o, rd, notes, as_data)
    }

    /// An edit's output (a dry run's, `after` none): the verb's stdout and data, and the
    /// change from the index diff and the plan.
    fn edited_output(
        &self,
        e: Edited<'_>,
        after: Option<VersionId>,
        dry: bool,
        notes: Vec<(Value, String)>,
        as_data: bool,
    ) -> ToolOutput {
        let (units, more) = if planted!(UnitsFromPlan) {
            let units = e
                .out
                .touched
                .iter()
                .chain(&e.out.created)
                .map(|k| ChangedUnit {
                    address: Some(k.clone()),
                    root: "index".to_owned(),
                    key: k.clone(),
                    field: None,
                    op: "set",
                    before: None,
                    after: None,
                })
                .collect();
            (units, 0)
        } else {
            units_of_diff(e.diff, e.before, e.after)
        };
        let removed: u64 = e.plan.spans.iter().map(|s| s.len).sum();
        let inserted: u64 = e.plan.spans.iter().map(|s| s.with.len() as u64).sum();
        let changed = Value::obj([
            ("counts", counts_of_diff(e.diff, e.before, e.after)),
            (
                "units",
                Value::List(units.iter().map(ChangedUnit::to_value).collect()),
            ),
            ("units_more", Value::from(more)),
            ("lines", Value::Null),
            (
                "bytes",
                Value::obj([
                    ("size_before", Value::from(e.size_before)),
                    ("size_after", Value::from(e.size_after)),
                    ("spans", Value::from(e.plan.spans.len())),
                    ("removed", Value::from(removed)),
                    ("inserted", Value::from(inserted)),
                    ("after_digest", Value::from(e.digest.as_str())),
                ]),
            ),
            // the verb's past-tense word, kit data (the planted fault: left out)
            (
                "word",
                if planted!(WordDropped) {
                    Value::Null
                } else {
                    Value::from(e.verb.word)
                },
            ),
        ]);
        let summary = if dry {
            format!(
                "would edit {} ({}) · v{} (dry run, nothing applied)",
                e.path, e.verb.name, e.base
            )
        } else {
            format!(
                "edited {} ({}) · v{}",
                e.path,
                e.verb.name,
                after.unwrap_or(e.base)
            )
        };
        let o = tool_output(e.out.stdout, summary, exit::OK, after.unwrap_or(e.base));
        let rd = crate::tools::ResultData {
            capture: crate::tools::Capture {
                base: Some(e.base),
                units,
                more,
            },
            data: e.out.data,
            blob: Some(Box::new(BlobResult {
                after,
                changed: Some(changed),
                view: self.view_of_change(e.out.viewing.as_deref(), e.diff, e.before, e.after),
            })),
            ..Default::default()
        };
        with_notes(o, rd, notes, as_data)
    }
}
