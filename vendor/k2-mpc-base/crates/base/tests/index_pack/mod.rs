//! A small pack for the structure-index tests: `IDX` files, one line per node, so a test
//! writes a file's tree by hand. It stands in for a real file type's reader (the kit knows
//! none).
//!
//! ```text
//! IDX
//! #source fmt-2
//! #attr seed=7
//! key|kind|name|parent|vis|text|content|note[|tag]
//! ```
//!
//! `key`, `text`, `content`, `note` and `tag` are `-` for none (`tag` may be left out);
//! `parent` is a line index or `-`;
//! `vis` is 0 or 1. Any other shape is `Unreadable`.
//!
//! **Its verbs** (the tier-0 verb path, [`VERB_SET`]; none unless a test gives them): edits of
//! one line by its key, an append, a read, and verbs that misbehave on purpose for the kit's
//! refusal rows.
#![allow(dead_code)]

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};

use k2_mpc_base::anchor::{judge_across_save_by_default, AnchorJudgment};
use k2_mpc_base::errors::{ErrorEntry, Value};
use k2_mpc_base::index::{
    cut_text, Digest, DraftNode, Fingerprint, IndexDraft, IndexLimits, IndexNode, IndexRefused,
    IndexSpec, IndexVerbs, INDEX_TEXT_MAX,
};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::{
    AgentWrites, BlobMerge, BlobPack, BlobPlan, BlobVerb, BlobVerbCtx, BlobVerbOut,
    BlobVerbRefused, HoldReach, SaveFacts, SavePairing, VerbInput, VerbMerge,
};
use k2_mpc_base::view_index::{BlobView, ViewSchema};

pub const VERBS: IndexVerbs =
    IndexVerbs::new("part list", "part show", "part diff", "part blame", None);

/// The test pack. `stop_after`: read only that many bytes (the pack stops early).
#[derive(Clone)]
pub struct IxPack {
    pub pack: &'static str,
    pub format: u32,
    /// What [`BlobPack::agent_writes`] says when set (else `refuse_agents` decides).
    pub agent_writes: Option<AgentWrites>,
    pub refuse_agents: bool,
    pub stop_after: Option<u64>,
    /// Put an unkeyed root node before the file's own (a reader whose output changed).
    pub extra_root: bool,
    /// Its verbs ([`VERB_SET`]); none by default.
    pub verbs: &'static [BlobVerb],
    /// [`BlobPack::can_merge`] (its merge itself keeps both: it never merges).
    pub merge: bool,
    /// A view schema whose containers are `layer` nodes.
    pub view: bool,
    /// What [`BlobPack::save_pairing`] says of every outside save (`None`: the default).
    pub pairing: Option<SavePairing>,
    /// [`BlobPack::judge_across_save`] doubts a node whose content changed across an outside
    /// save, renamed or not (else the default).
    pub content_doubt: bool,
    /// [`BlobPack::hold_reach`] per node kind (a kind not listed: the default).
    pub reach: &'static [(&'static str, HoldReach)],
    /// [`BlobPack::merge3_bytes`] merges by line key ([`line_merge`]) instead of keeping both.
    pub line_merge: bool,
    /// With `line_merge`: the merge also sets the last line's note to `strayed` (a merge that
    /// changes a node neither side changed).
    pub merge_strays: bool,
}

impl Default for IxPack {
    fn default() -> Self {
        Self {
            pack: "idx",
            format: 1,
            agent_writes: None,
            refuse_agents: false,
            stop_after: None,
            extra_root: false,
            verbs: &[],
            merge: false,
            view: false,
            pairing: None,
            content_doubt: false,
            reach: &[],
            line_merge: false,
            merge_strays: false,
        }
    }
}

impl BlobPack for IxPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new(self.pack, self.format, &VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        INDEX_RUNS.with(|n| n.set(n.get() + 1));
        let mut bytes = Vec::new();
        let read = match self.stop_after {
            Some(n) => src.take(n).read_to_end(&mut bytes),
            None => src.read_to_end(&mut bytes),
        };
        read.map_err(IndexRefused::Read)?;
        let mut draft = parse(&bytes, limits)?;
        if self.extra_root {
            for n in &mut draft.nodes {
                n.parent = n.parent.map(|p| p + 1);
            }
            draft
                .nodes
                .insert(0, DraftNode::new("extra", "Extra", Digest::of(b"extra")));
        }
        Ok(draft)
    }

    fn agent_writes(&self) -> AgentWrites {
        if let Some(w) = self.agent_writes {
            return w;
        }
        if self.refuse_agents {
            AgentWrites::Refused
        } else {
            AgentWrites::Allowed
        }
    }

    fn can_merge(&self) -> bool {
        self.merge
    }

    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        if !self.line_merge {
            return BlobMerge::Conflict;
        }
        match line_merge(base, ours, theirs) {
            Some(mut out) if self.merge_strays => {
                if let Some(last) = out.iter().rposition(|l| l.starts_with("@n:")) {
                    let mut f: Vec<String> = out[last].split('|').map(str::to_owned).collect();
                    while f.len() < 8 {
                        f.push("-".to_owned());
                    }
                    f[7] = "strayed".to_owned();
                    out[last] = f.join("|");
                }
                BlobMerge::Merged(join_lines(&out))
            }
            Some(out) => BlobMerge::Merged(join_lines(&out)),
            None => BlobMerge::Conflict,
        }
    }

    fn verbs(&self) -> &'static [BlobVerb] {
        self.verbs
    }

    fn hold_reach(&self, kind: &str) -> HoldReach {
        self.reach
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(HoldReach::Node, |(_, r)| *r)
    }

    fn save_pairing(&self, _facts: &SaveFacts<'_>) -> SavePairing {
        self.pairing.unwrap_or(SavePairing::Unknown)
    }

    fn judge_across_save(
        &self,
        was: &Fingerprint,
        hint: Option<&str>,
        now: &IndexNode,
    ) -> AnchorJudgment {
        if self.content_doubt && was.content != now.content_hash {
            return AnchorJudgment::IdentityDoubt;
        }
        match self.judge_anchor(was, hint, now) {
            AnchorJudgment::Same | AnchorJudgment::HintOff => {
                judge_across_save_by_default(was, now)
            }
            other => other,
        }
    }

    fn view_schema(&self) -> Option<ViewSchema> {
        self.view.then_some(ViewSchema {
            blob: Some(BlobView {
                container_kind: "layer",
                ..BlobView::NONE
            }),
            ..ViewSchema::NONE
        })
    }

    fn errors(&self) -> &'static [ErrorEntry] {
        if self.verbs.is_empty() {
            &[]
        } else {
            IDX_ERRORS
        }
    }
}

/// A three-way merge by line: keyed lines (`@n:...`) pair by key, every other line must be
/// the same on all three sides. A line one side changed takes that side's; both changed it
/// differently: `None` (a conflict). Lines keep `ours`' order; a line only `theirs` added goes
/// at the end; a line one side removed and the other left is removed.
pub fn line_merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<String>> {
    let lines = |b: &[u8]| -> Option<Vec<String>> {
        Some(
            std::str::from_utf8(b)
                .ok()?
                .lines()
                .map(str::to_owned)
                .collect(),
        )
    };
    let (b, o, t) = (lines(base)?, lines(ours)?, lines(theirs)?);
    let key = |l: &str| {
        l.starts_with("@n:")
            .then(|| l.split('|').next().unwrap_or("").to_owned())
    };
    let find = |v: &[String], k: &str| v.iter().find(|l| key(l).as_deref() == Some(k)).cloned();
    let plain = |v: &[String]| {
        v.iter()
            .filter(|l| key(l).is_none())
            .cloned()
            .collect::<Vec<_>>()
    };
    if plain(&b) != plain(&o) || plain(&b) != plain(&t) {
        return None;
    }
    let pick =
        |b: Option<String>, o: Option<String>, t: Option<String>| -> Result<Option<String>, ()> {
            if o == b {
                Ok(t)
            } else if t == b || t == o {
                Ok(o)
            } else {
                Err(())
            }
        };
    let mut out = Vec::new();
    for l in &o {
        match key(l) {
            None => out.push(l.clone()),
            Some(k) => {
                if let Some(x) = pick(find(&b, &k), Some(l.clone()), find(&t, &k)).ok()? {
                    out.push(x);
                }
            }
        }
    }
    for l in &t {
        if let Some(k) = key(l) {
            if find(&o, &k).is_none() {
                if let Some(x) = pick(find(&b, &k), None, Some(l.clone())).ok()? {
                    out.push(x);
                }
            }
        }
    }
    Some(out)
}

fn join_lines(lines: &[String]) -> Vec<u8> {
    let mut s = lines.join("\n");
    s.push('\n');
    s.into_bytes()
}

// ---------- the verbs (the tier-0 verb path) ----------

/// The pack's own refusal: a part it won't change.
pub static IDX_ERRORS: &[ErrorEntry] = &[ErrorEntry::pack(
    "IDX-E001",
    "part_locked",
    2,
    "that part is locked",
    "Pick another part.",
)];

const EDIT: &[PersonAction] = &[PersonAction::Rewrite];
const READ: &[PersonAction] = &[PersonAction::ReadAll];

/// Every verb: `part set` (one field of a line), `part guard` (the same, under the lease
/// only), `part add` (a line at the end, read whole), `part peek` (a read), `part bad`
/// (misbehaves as its first argument says).
pub static VERB_SET: &[BlobVerb] = &[
    BlobVerb::edit(
        "part set",
        "KEY name|text|note VALUE",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_set,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part guard",
        "KEY name|text|note VALUE (under your own lease)",
        EDIT,
        VerbMerge::Lease,
        VerbInput::Stream,
        part_set,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part add",
        "KEY KIND NAME (a line at the end)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Whole,
        part_add,
    )
    .with_word("added"),
    BlobVerb::read("part peek", "KEY", READ, VerbInput::Stream, part_peek),
    BlobVerb::edit(
        "part both",
        "KEY KEY2 VALUE (the note of two lines)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_both,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part bad",
        "unsorted|overlap|outside|many|stray|garble|refuse KEY [KEY2]",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_bad,
    )
    .with_word("changed"),
];

thread_local! {
    /// How many times this pack's index ran on this thread ([`index_runs`]).
    static INDEX_RUNS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times the pack's index ran on this thread (an index job's run, a verb's output
/// check, a whole-file write's check), for the rows that count them.
pub fn index_runs() -> u64 {
    INDEX_RUNS.with(|n| n.get())
}

thread_local! {
    /// Run by `part set` before it reads (a test moves head from it); `None`: nothing.
    static HOOK: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
}

/// How many times `part set` and `part guard` planned (any thread).
pub static PLANS: AtomicU64 = AtomicU64::new(0);

/// Set `part set`'s hook on this thread (`None` clears it).
pub fn set_hook(f: Option<Box<dyn FnMut()>>) {
    HOOK.with(|h| *h.borrow_mut() = f);
}

/// One line of the file: where it starts, its bytes without the newline, and its fields.
struct Line {
    at: u64,
    text: String,
}

/// Every line, with its offset (read forward, never held whole: a streaming verb).
fn lines(src: &mut dyn Read) -> Result<Vec<Line>, BlobVerbRefused> {
    let mut r = BufReader::new(src);
    let mut out = Vec::new();
    let mut at = 0u64;
    loop {
        let mut buf = Vec::new();
        let n = r
            .read_until(b'\n', &mut buf)
            .map_err(|e| BlobVerbRefused::usage(format!("reading: {e}")))?;
        if n == 0 {
            return Ok(out);
        }
        let text = String::from_utf8_lossy(&buf)
            .trim_end_matches('\n')
            .to_owned();
        out.push(Line { at, text });
        at += n as u64;
    }
}

fn line_of<'a>(ls: &'a [Line], key: &str) -> Result<&'a Line, BlobVerbRefused> {
    ls.iter()
        .find(|l| !l.text.starts_with('#') && l.text.split('|').next() == Some(key))
        .ok_or_else(|| BlobVerbRefused::usage(format!("{key}: no such part")))
}

fn arg<'a>(args: &'a [String], i: usize, what: &str) -> Result<&'a str, BlobVerbRefused> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| BlobVerbRefused::usage(format!("missing {what}")))
}

/// `KEY FIELD VALUE`: the line with that field set.
fn part_set(
    _ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    HOOK.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f();
        }
    });
    PLANS.fetch_add(1, Ordering::SeqCst);
    let (key, field, value) = (
        arg(args, 0, "KEY")?,
        arg(args, 1, "FIELD")?,
        arg(args, 2, "VALUE")?,
    );
    let mut value = value.to_owned();
    // the planted fault: the text field reads hidden state
    if field == "text"
        && k2_mpc_base::differential::tier0_verb_mutant()
            == Some(k2_mpc_base::differential::Tier0VerbMutant::PackImpure)
    {
        static N: AtomicU64 = AtomicU64::new(0);
        value.push_str(&N.fetch_add(1, Ordering::SeqCst).to_string());
    }
    let at = field_at(field)?;
    let ls = lines(src)?;
    let l = line_of(&ls, key)?;
    let new = with_field(&l.text, at, &value);
    let plan = BlobPlan::new().span(l.at, l.text.len() as u64, new.into_bytes());
    Ok(BlobVerbOut::edit(plan, format!("set {key} {field}\n"))
        .touching(key)
        .with_data(Value::obj([
            ("key", Value::from(key)),
            ("field", Value::from(field)),
        ])))
}

/// A settable field's column: `name`, `text`, `content`, `note`, `tag`.
fn field_at(field: &str) -> Result<usize, BlobVerbRefused> {
    Ok(match field {
        "name" => 2,
        "text" => 5,
        "content" => 6,
        "note" => 7,
        "tag" => 8,
        other => {
            return Err(BlobVerbRefused::usage(format!(
                "{other}: name, text, content, note or tag"
            )))
        }
    })
}

/// A line with column `at` set to `value` (missing columns `-`).
fn with_field(line: &str, at: usize, value: &str) -> String {
    let mut f: Vec<String> = line.split('|').map(str::to_owned).collect();
    while f.len() < 8.max(at + 1) {
        f.push("-".to_owned());
    }
    f[at] = value.to_owned();
    f.join("|")
}

/// The verbs for per-node `--base` ([`VERB_SET`]'s `part set` and `part add`, and two more).
pub static BASE_SET: &[BlobVerb] = &[
    BlobVerb::edit(
        "part set",
        "KEY name|text|content|note|tag VALUE",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_set,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part add",
        "KEY KIND NAME (a line at the end)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Whole,
        part_add,
    )
    .with_word("added"),
    BlobVerb::edit(
        "part named",
        "NAME FIELD VALUE (every line with that name: a selector)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_named,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part new",
        "KIND NAME (a line at the end, its key one above the highest @n:)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Whole,
        part_new,
    )
    .with_word("added"),
];

/// The verbs for a tier-0 batch ([`BASE_SET`]'s four, `part guard` (under the caller's own
/// lease), `part bad` (misbehaves as asked) and `part peek` (a read)).
pub static BATCH_SET: &[BlobVerb] = &[
    BlobVerb::edit(
        "part set",
        "KEY name|text|content|note|tag VALUE",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_set,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part add",
        "KEY KIND NAME (a line at the end)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Whole,
        part_add,
    )
    .with_word("added"),
    BlobVerb::edit(
        "part new",
        "KIND NAME (a line at the end, its key one above the highest @n:)",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Whole,
        part_new,
    )
    .with_word("added"),
    BlobVerb::edit(
        "part guard",
        "KEY name|text|note VALUE (under your own lease)",
        EDIT,
        VerbMerge::Lease,
        VerbInput::Stream,
        part_set,
    )
    .with_word("set"),
    BlobVerb::edit(
        "part bad",
        "unsorted|overlap|outside|many|stray|garble|refuse KEY [KEY2]",
        EDIT,
        VerbMerge::Pack,
        VerbInput::Stream,
        part_bad,
    )
    .with_word("changed"),
    BlobVerb::read("part peek", "KEY", READ, VerbInput::Stream, part_peek),
];

/// `NAME FIELD VALUE`: the field set on every line with that name (keyed or not), each node
/// touched by its index key.
fn part_named(
    ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let (name, field, value) = (
        arg(args, 0, "NAME")?,
        arg(args, 1, "FIELD")?,
        arg(args, 2, "VALUE")?,
    );
    let at = field_at(field)?;
    let ls = lines(src)?;
    let mut plan = BlobPlan::new();
    for l in ls.iter().skip(1).filter(|l| !l.text.starts_with('#')) {
        if l.text.split('|').nth(2) == Some(name) {
            plan = plan.span(
                l.at,
                l.text.len() as u64,
                with_field(&l.text, at, value).into_bytes(),
            );
        }
    }
    let mut out = BlobVerbOut::edit(plan, format!("set {name} {field}\n"));
    if let Some(view) = ctx.index() {
        for n in view.nodes.iter().filter(|n| n.name == name) {
            out = out.touching(n.key.as_str());
        }
    }
    Ok(out)
}

/// `KIND NAME`: a line at the end, keyed one above the highest `@n:N` (as a file that mints
/// its own IDs from a counter would).
fn part_new(
    _ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let (kind, name) = (arg(args, 0, "KIND")?, arg(args, 1, "NAME")?);
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .map_err(|e| BlobVerbRefused::usage(format!("reading: {e}")))?;
    let text = String::from_utf8_lossy(&bytes);
    let top = text
        .lines()
        .filter_map(|l| {
            l.split('|')
                .next()?
                .strip_prefix("@n:")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    let key = format!("@n:{}", top + 1);
    let mut line = String::new();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        line.push('\n');
    }
    line.push_str(&format!("{key}|{kind}|{name}|-|1|-|-|-\n"));
    let plan = BlobPlan::new().span(bytes.len() as u64, 0, line.into_bytes());
    Ok(BlobVerbOut::edit(plan, format!("added {key}\n")).creating(key))
}

/// `KEY KEY2 VALUE`: both lines' note set (two ranges, in file order).
fn part_both(
    _ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let (a, b, value) = (
        arg(args, 0, "KEY")?,
        arg(args, 1, "KEY2")?,
        arg(args, 2, "VALUE")?,
    );
    let ls = lines(src)?;
    let mut spans = Vec::new();
    for key in [a, b] {
        let l = line_of(&ls, key)?;
        let mut f: Vec<String> = l.text.split('|').map(str::to_owned).collect();
        while f.len() < 8 {
            f.push("-".to_owned());
        }
        f[7] = value.to_owned();
        spans.push((l.at, l.text.len() as u64, f.join("|")));
    }
    spans.sort_by_key(|s| s.0);
    let plan = spans
        .into_iter()
        .fold(BlobPlan::new(), |p, (at, len, with)| p.span(at, len, with));
    Ok(BlobVerbOut::edit(plan, format!("set {a} {b}\n"))
        .touching(a)
        .touching(b))
}

/// `KEY KIND NAME`: a line at the end (read whole).
fn part_add(
    ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let (key, kind, name) = (
        arg(args, 0, "KEY")?,
        arg(args, 1, "KIND")?,
        arg(args, 2, "NAME")?,
    );
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .map_err(|e| BlobVerbRefused::usage(format!("reading: {e}")))?;
    let mut line = String::new();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        line.push('\n');
    }
    line.push_str(&format!("{key}|{kind}|{name}|-|1|-|-|-\n"));
    let plan = BlobPlan::new().span(ctx.size(), 0, line.into_bytes());
    Ok(BlobVerbOut::edit(plan, format!("added {key}\n")).creating(key))
}

/// `KEY`: the line's name (a read).
fn part_peek(
    ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let key = arg(args, 0, "KEY")?;
    let ls = lines(src)?;
    let l = line_of(&ls, key)?;
    let name = l.text.split('|').nth(2).unwrap_or("").to_owned();
    let mut data = vec![
        ("key", Value::from(key)),
        ("name", Value::from(name.as_str())),
    ];
    if let Some(m) = ctx.merge() {
        data.push((
            "merge",
            Value::obj([
                ("max_bytes", Value::from(m.max_bytes)),
                ("size", Value::from(m.size)),
                ("fits", Value::from(m.fits)),
            ]),
        ));
    }
    Ok(BlobVerbOut::read(format!("{key} {name}\n")).with_data(Value::obj(data)))
}

/// `MODE KEY [KEY2]`: a verb that misbehaves.
fn part_bad(
    ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let mode = arg(args, 0, "MODE")?;
    let ls = lines(src)?;
    let whole = |l: &Line, with: &str| (l.at, l.text.len() as u64, with.as_bytes().to_vec());
    let plan = |spans: Vec<(u64, u64, Vec<u8>)>| {
        spans
            .into_iter()
            .fold(BlobPlan::new(), |p, (at, len, with)| p.span(at, len, with))
    };
    Ok(match mode {
        // the second line's range before the first's
        "unsorted" => {
            let (a, b) = (
                line_of(&ls, arg(args, 1, "KEY")?)?,
                line_of(&ls, arg(args, 2, "KEY2")?)?,
            );
            let (lo, hi) = if a.at < b.at { (a, b) } else { (b, a) };
            BlobVerbOut::edit(
                plan(vec![whole(hi, &hi.text), whole(lo, &lo.text)]),
                "unsorted\n",
            )
            .touching(arg(args, 1, "KEY")?)
            .touching(arg(args, 2, "KEY2")?)
        }
        "overlap" => {
            let l = line_of(&ls, arg(args, 1, "KEY")?)?;
            BlobVerbOut::edit(
                plan(vec![
                    (l.at, 4, b"@n:9".to_vec()),
                    (l.at + 2, 2, b":9".to_vec()),
                ]),
                "overlap\n",
            )
            .touching(arg(args, 1, "KEY")?)
        }
        "outside" => BlobVerbOut::edit(plan(vec![(ctx.size(), 5, b"x".to_vec())]), "outside\n"),
        "many" => {
            let n = ctx.max_spans() + 1;
            BlobVerbOut::edit(plan((0..n).map(|_| (0, 0, Vec::new())).collect()), "many\n")
        }
        // edits KEY2's line, declares only KEY
        "stray" => {
            let l = line_of(&ls, arg(args, 2, "KEY2")?)?;
            let mut f: Vec<&str> = l.text.split('|').collect();
            if f.len() > 2 {
                f[2] = "strayed";
            }
            let new = f.join("|");
            BlobVerbOut::edit(plan(vec![whole(l, &new)]), "stray\n").touching(arg(args, 1, "KEY")?)
        }
        // the header the index needs, broken
        "garble" => BlobVerbOut::edit(plan(vec![(0, 3, b"XDI".to_vec())]), "garble\n"),
        "refuse" => {
            return Err(BlobVerbRefused::pack(
                &IDX_ERRORS[0],
                format!("{} is locked", arg(args, 1, "KEY")?),
                "Pick another part.",
            ))
        }
        other => return Err(BlobVerbRefused::usage(format!("{other}: no such mode"))),
    })
}

fn parse(bytes: &[u8], limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let bad = |why: &str| IndexRefused::Unreadable {
        why: why.to_owned(),
    };
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not text"))?;
    let mut lines = text.lines();
    if lines.next() != Some("IDX") {
        return Err(bad("no IDX header"));
    }
    let mut draft = IndexDraft::new("fmt-1");
    for line in lines {
        if let Some(s) = line.strip_prefix("#source ") {
            draft.source = s.to_owned();
            continue;
        }
        if let Some(a) = line.strip_prefix("#attr ") {
            let (k, v) = a.split_once('=').ok_or_else(|| bad("attr"))?;
            draft.attrs.insert(k.to_owned(), v.to_owned());
            continue;
        }
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 7 {
            return Err(bad("short line"));
        }
        let opt = |s: &str| (s != "-").then(|| s.to_owned());
        let mut n = DraftNode::new(f[1], f[2], Digest::of(line.as_bytes()));
        n.key = opt(f[0]);
        n.parent = if f[3] == "-" {
            None
        } else {
            Some(f[3].parse().map_err(|_| bad("parent"))?)
        };
        n.visible = f[4] == "1";
        if let Some(t) = opt(f[5]) {
            let (kept, cut) = cut_text(&t, limits.max_text_bytes.min(INDEX_TEXT_MAX));
            if cut {
                n.flags.insert("text_truncated".to_owned());
            }
            n.text = Some(kept.to_owned());
        }
        n.content = opt(f[6]).map(|c| Digest::of(c.as_bytes()));
        if let Some(note) = f.get(7).copied().and_then(opt) {
            n.attrs.insert("note".to_owned(), note.to_owned());
            n.weak.insert("placed".to_owned(), note.to_owned());
        }
        if let Some(tag) = f.get(8).copied().and_then(opt) {
            n.attrs.insert("tag".to_owned(), tag.to_owned());
        }
        draft.nodes.push(n);
    }
    Ok(draft)
}

/// An `IDX` file from node lines.
pub fn file(lines: &[&str]) -> Vec<u8> {
    let mut s = String::from("IDX\n");
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s.into_bytes()
}
