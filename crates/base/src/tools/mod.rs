//! Text tools: the command-line verbs agents use on a live doc, as pure functions over
//! text, and [`crate::session::Session::run_tool`], which runs them on a session.
//!
//! Every text pack reuses these. They do no I/O and run no programs; regexes run on
//! linear-time engines with size limits; output is bounded ([`MAX_OUTPUT_BYTES`]).
//!
//! | Verb | Kind | Mirrors | Supported |
//! |---|---|---|---|
//! | `cat` | read | `cat` | `-n -b` |
//! | `head` | read | `head` | `-n [-]N`, `-c [-]N`, `-NUM` |
//! | `tail` | read | `tail` | `-n N`, `-n +N`, `-c [+]N`, `-NUM` (`-f` is the host's `watch`) |
//! | `wc` | read | `wc` | `-l -w -c -m` |
//! | `grep` | read | `grep` | `-E -F -G -e -i -v -w -x -c -o -q -l -L -H -h -n -m -A -B -C` |
//! | `sed` | read | `sed` | see `sed.rs` |
//! | `diff` | read | `diff -u` | `-u -U N --label` (the old text on stdin) |
//! | `view` | read | `sed -n 'a,bp'` | `--lines a,b` |
//! | `sed -i` | edit | `sed -i` | as `sed`, plus GNU `-i` and BSD `-i ''` |
//! | `patch` / `apply` | edit | `patch` | `-p N -R --dry-run -F N --partial` (diff on stdin) |
//! | `append` | edit | `cat >>` | `--ensure-newline` |
//! | `insert` | edit | `sed 'Ni\'` | `--line N`, `--after N`, `--at L:C` |
//! | `replace` | edit | `sed 'a,bc\'` | `--lines a,b`, `--range L:C-L:C` (stdin) |
//! | `replace` | edit | find and replace | `--old TEXT --new TEXT [--all]` (exact text; `-`: stdin) |
//! | `delete` | edit | `sed 'a,bd'` | `--lines a,b`, `--range L:C-L:C` |
//! | (every edit verb) | edit | | `--if-version V` (pack verbs too) |
//!
//! **argv** is the verb and its flags and operands, without the doc's path (the host
//! resolves it): `["grep", "-n", "TODO"]`. Flags follow GNU getopt (clusters, permutation,
//! unique long-option prefixes). Anything outside the supported subset is
//! [`ToolError::Unsupported`] with a hint; nothing is approximated.
//!
//! **Bytes and text.** Byte tools (`cat` without flags, `wc`, `head -c`, `tail -c`) see
//! the file's exact bytes (the pack's encoding: BOM and CRLF restored). Line tools and edit
//! verbs see the doc text (`\n` line breaks, no BOM); edits are encoded back by the pack.
//!
//! **Exact replace** (`replace --old TEXT --new TEXT [--all]`, `exact.rs`): `--old` is
//! matched byte for byte on the doc text (as above: `\n`, no BOM), and must match exactly
//! once unless `--all` (every non-overlapping match, left to right). No match, or more than
//! one, applies nothing and exits 1 with a report on stdout (the count, each match's lines,
//! near-miss hints). `-` reads that one of them from stdin. A host's `--old-file` /
//! `--new-file` are the host's: it reads the file and passes its text as `--old` / `--new`
//! (the kit does no I/O).
//!
//! **`--for-edit [--ttl MS]`** (amendment 13) on the read verbs `cat`, `head`, `tail`,
//! `view`, `grep` and `sed` (without `-i`): the caller also takes a short implicit claim on
//! the lines shown (`grep`: the span from the first to the last selected line;
//! [`crate::session::Config::for_edit_ms`] unless `--ttl`), ended by its next edit touching
//! them. Another participant's overlapping claim takes nothing and is reported; the read
//! succeeds either way ([`ToolOutput::for_edit`]). `--ttl` takes milliseconds, or a number
//! with `ms`, `s` or `m`.
//!
//! **`--if-version V`** (amendment 14) on every edit verb, the kit's and the pack's: apply
//! only if the doc is still at version `V` (as reads print it: `1842` or `v1842`). Checked
//! under the session lock with [`crate::session::Session::edit`]'s `if_version` check, after
//! admission and before every other refusal (the backlog, the verb's own "no", claims). Stale:
//! [`crate::session::SessionError::Stale`], exit 1, one line `doc changed since V (now W)`,
//! nothing applied. Read verbs refuse it (exit 2). A pack verb's `--if-version` is taken out
//! of its arguments by the kit before the pack parses them.
//!
//! **Heads-up notes** ([`ToolOutput::notes`]): every read and edit through `run_tool` says
//! who else is viewing, just edited, claimed, or (a person) has a caret on the lines it
//! touched. One line each, `note: ...`, for stderr only: `stdout` never changes.
//!
//! **Exit codes** are mapped in one place ([`exit`], [`ToolError::exit`],
//! [`ToolRunError::exit`], [`session_exit`]): 0 ok, 1 the tool's "no", 2 usage or
//! unsupported, 3 refused (rights), 4 not found. Usage errors exit 2 for every tool (GNU
//! sed and the coreutils use 1 there; the host's table wins).

mod args;
mod edit;
mod error;
mod exact;
mod fence;
mod grep;
mod pattern;
mod read;
mod run;
mod sed;
mod udiff;

pub use error::{exit, session_exit, ToolError, ToolRunError};
pub use fence::{random_nonce, Fence};
pub use run::{
    positional_note, PositionalMode, ToolOptions, ToolOutput, Warnings, POSITIONAL_DEFAULT,
};
#[cfg(test)]
pub(crate) use udiff::refusal as udiff_refusal_for_tests;
pub use udiff::unified;

use crate::store::Seq;

/// The most a tool may output (stdout, or an edit's new text), in bytes.
pub const MAX_OUTPUT_BYTES: usize = 32 << 20;

pub(crate) fn too_much_output() -> ToolError {
    ToolError::Limit(format!("output over {MAX_OUTPUT_BYTES} bytes"))
}

pub(crate) fn no_operands(tool: &str, operand: &str) -> ToolError {
    ToolError::unsupported(
        operand.to_owned(),
        format!("{tool} takes no file operands: the host passes the doc"),
    )
}

/// One line of a text, GNU style: a final `\n` ends the last line; it doesn't start a new
/// one, and `""` has no lines.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Line<'a> {
    /// Without its `\n`.
    pub text: &'a str,
    /// Ends with `\n` (only the last line may not).
    pub nl: bool,
    /// Byte offset of its start.
    pub start: usize,
}

pub(crate) fn lines(s: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for piece in s.split_inclusive('\n') {
        let nl = piece.ends_with('\n');
        out.push(Line {
            text: &piece[..piece.len() - usize::from(nl)],
            nl,
            start,
        });
        start += piece.len();
    }
    out
}

/// The doc a read verb runs on.
#[derive(Debug, Clone, Copy)]
pub struct ToolDoc<'a> {
    /// Display name (`grep -H`, `wc`, `patch` messages, the fence).
    pub name: &'a str,
    /// The root's text (`\n` line breaks, no BOM).
    pub text: &'a str,
    /// The file's bytes (the pack's encoding of the whole doc).
    pub bytes: &'a [u8],
}

/// What a read verb produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadOutput {
    pub stdout: Vec<u8>,
    pub exit: u8,
    /// Lines shown, 1-based inclusive, for the caller's viewing range (P4): `head -n`,
    /// `tail -n`, `view --lines`, `sed -n 'a,bp'`.
    pub view: Option<(usize, usize)>,
    /// The lines the call looked at (heads-up notes, the activity ring, `--for-edit`): the
    /// view, the whole doc for `cat`, the selected lines' span for `grep`. `None` for byte
    /// counts, `wc` and `diff`.
    pub touched: Option<(usize, usize)>,
    /// The doc line each stdout line came from, for `--ids` (`None`: the output isn't lines
    /// of the doc, e.g. `head -c`, `grep -c`).
    pub(crate) src: Option<SrcLines>,
}

/// The doc lines a read verb printed, one per stdout line ([`ReadOutput::src`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SrcLines {
    /// Lines `a..=b` (1-based) in order; empty when `a > b`.
    Range(usize, usize),
    /// One entry per stdout line; 0 for a line that is no doc line (grep's `--`).
    Each(Vec<usize>),
}

impl SrcLines {
    fn at(&self, k: usize) -> usize {
        match self {
            SrcLines::Range(a, _) => a + k,
            SrcLines::Each(v) => v.get(k).copied().unwrap_or(0),
        }
    }
}

impl ReadOutput {
    pub(crate) fn new(stdout: Vec<u8>, exit: u8) -> Self {
        ReadOutput {
            stdout,
            exit,
            view: None,
            touched: None,
            src: None,
        }
    }

    /// Set the viewing range, and the touched lines to it; the output is those lines.
    pub(crate) fn showing(mut self, view: Option<(usize, usize)>) -> Self {
        self.view = view;
        self.touched = view;
        self.src = Some(view.map_or(SrcLines::Range(1, 0), |(a, b)| SrcLines::Range(a, b)));
        self
    }

    /// `--ids` (CSV contract §3.1): prefix each stdout line with its row's key and a tab
    /// (`header` for the header's lines, `-` for a line that is no doc line). `text` is the
    /// rendered file the output came from: a stdout line is a `\n` line of it, which may hold
    /// several rows when the file has a lone CR, and then the prefix names each, in order,
    /// joined by commas (`header,r1,r2`). Output that isn't lines of the doc (`head -c`,
    /// `grep -c`/`-l`/`-q`) is a usage error.
    pub(crate) fn with_ids(
        mut self,
        map: &crate::table::LineMap,
        text: &str,
    ) -> Result<Self, ToolError> {
        if self.stdout.is_empty() {
            return Ok(self);
        }
        let Some(src) = &self.src else {
            return Err(ToolError::usage(
                "--ids needs lines of the doc: not with byte counts, -c, -l, -L or -q",
            ));
        };
        let bridge = crate::table::NlLines::of(text);
        let mut out = Vec::with_capacity(self.stdout.len() + self.stdout.len() / 4);
        for (k, line) in self.stdout.split_inclusive(|&b| b == b'\n').enumerate() {
            let n = src.at(k);
            #[cfg(any(test, feature = "differential"))]
            let (first, last) = if crate::differential::line_mutant()
                == Some(crate::differential::LineMutant::IdsCountNlLines)
            {
                (n, n)
            } else {
                bridge.span(n)
            };
            #[cfg(not(any(test, feature = "differential")))]
            let (first, last) = bridge.span(n);
            let mut owners: Vec<&crate::table::LineOwner> = Vec::new();
            for o in (first..=last).filter_map(|l| map.owner(l)) {
                if owners.last() != Some(&o) {
                    owners.push(o);
                }
            }
            if owners.is_empty() {
                out.push(b'-');
            }
            for (i, o) in owners.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                match o {
                    crate::table::LineOwner::Row(key) => {
                        out.extend_from_slice(key.to_string().as_bytes())
                    }
                    crate::table::LineOwner::Header => out.extend_from_slice(b"header"),
                    crate::table::LineOwner::Preamble => out.extend_from_slice(b"preamble"),
                }
            }
            out.push(b'\t');
            out.extend_from_slice(line);
        }
        self.stdout = out;
        Ok(self)
    }
}

/// `--for-edit [--ttl MS]` as a read verb parsed it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ForEditArg {
    on: bool,
    ttl_ms: Option<u64>,
}

/// The two options every read verb that can take `--for-edit` adds to its table.
pub(crate) const FOR_EDIT: args::Opt =
    args::opt("for-edit", None, Some("for-edit"), args::Arity::No);
pub(crate) const TTL: args::Opt = args::opt("ttl", None, Some("ttl"), args::Arity::Req);
/// `--ids` (CSV contract §3.1): on `cat`, `head`, `tail`, `grep` and `view`, each line is
/// prefixed with its row's key. Only a table has keys; elsewhere it is a usage error.
pub(crate) const IDS: args::Opt = args::opt("ids", None, Some("ids"), args::Arity::No);

/// Take `--for-edit [--ttl MS]` out of a structured read verb's arguments (the kit's, as
/// `--if-version` is for edits; arguments after `--` are left alone): `Some(ttl)` if given.
pub(crate) fn take_for_edit(
    args: &[String],
) -> Result<(Option<Option<u64>>, Vec<String>), ToolError> {
    let mut fe = ForEditArg::default();
    let mut rest = Vec::with_capacity(args.len());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            rest.push(a.clone());
            rest.extend(it.cloned());
            break;
        }
        if a == "--for-edit" {
            fe.on = true;
        } else if a == "--ttl" {
            let v = it
                .next()
                .ok_or_else(|| ToolError::usage("option '--ttl' requires an argument"))?;
            fe.ttl_ms = Some(parse_ttl(v)?);
        } else if let Some(v) = a.strip_prefix("--ttl=") {
            fe.ttl_ms = Some(parse_ttl(v)?);
        } else {
            rest.push(a.clone());
        }
    }
    Ok((fe.finish()?, rest))
}

impl ForEditArg {
    /// Take `it` if it is `--for-edit` or `--ttl`.
    pub(crate) fn take(&mut self, it: &args::Item) -> Result<bool, ToolError> {
        match it {
            args::Item::Opt("for-edit", _) => self.on = true,
            args::Item::Opt("ttl", Some(v)) => self.ttl_ms = Some(parse_ttl(v)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// `Some(ttl)` with `--for-edit`; `--ttl` alone is a usage error.
    pub(crate) fn finish(self) -> Result<Option<Option<u64>>, ToolError> {
        match (self.on, self.ttl_ms) {
            (true, t) => Ok(Some(t)),
            (false, None) => Ok(None),
            (false, Some(_)) => Err(ToolError::usage("--ttl goes with --for-edit")),
        }
    }
}

/// `--if-version V` (amendment 14), an option of every kit edit verb's table: apply only if
/// the doc is still at version `V`, checked under the session lock (pack verbs: the kit
/// takes it out of their arguments first, [`args::take_if_version`]).
pub(crate) const IF_VERSION: args::Opt =
    args::opt("if-version", None, Some("if-version"), args::Arity::Req);

/// `--if-version V` as an edit verb parsed it: given at most once.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IfVersionArg(Option<Seq>);

impl IfVersionArg {
    /// Take `it` if it is `--if-version`.
    pub(crate) fn take(&mut self, it: &args::Item) -> Result<bool, ToolError> {
        match it {
            args::Item::Opt("if-version", Some(v)) => {
                self.set(v)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn set(&mut self, v: &str) -> Result<(), ToolError> {
        if self.0.is_some() {
            return Err(ToolError::usage("--if-version given twice"));
        }
        self.0 = Some(parse_version(v)?);
        Ok(())
    }

    pub(crate) fn get(self) -> Option<Seq> {
        self.0
    }
}

/// `--if-version` on a read verb: refused, so agents learn where it applies.
pub(crate) fn if_version_on_read(verb: &str) -> ToolError {
    ToolError::usage(format!(
        "--if-version is for edit verbs; {verb} is a read (it prints the version)"
    ))
}

/// A version as reads print it: `1842` or `v1842`.
fn parse_version(v: &str) -> Result<Seq, ToolError> {
    let n = v.strip_prefix('v').unwrap_or(v);
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ToolError::usage(format!(
            "invalid --if-version: '{v}' (the version a read printed: 1842 or v1842)"
        )));
    }
    n.parse()
        .map_err(|_| ToolError::usage(format!("invalid --if-version: '{v}' (too large)")))
}

/// `30000`, `500ms`, `30s`, `2m`: milliseconds.
fn parse_ttl(v: &str) -> Result<u64, ToolError> {
    let bad = || {
        ToolError::usage(format!(
            "invalid --ttl: '{v}' (milliseconds, or Nms, Ns, Nm)"
        ))
    };
    let (num, mult) = if let Some(n) = v.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = v.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = v.strip_suffix('m') {
        (n, 60_000)
    } else {
        (v, 1)
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let n: u64 = num.parse().map_err(|_| bad())?;
    match n.checked_mul(mult) {
        Some(0) | None => Err(bad()),
        Some(ms) => Ok(ms),
    }
}

/// What an edit verb produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditOutput {
    /// The new text; `None` when nothing is to be applied (a failed all-or-nothing patch,
    /// `--dry-run`).
    pub text: Option<String>,
    /// Messages for stdout: `patch`'s report; exact `replace`'s refusal report (no match, or
    /// not unique) and its `--all` count; empty for the other verbs.
    pub stdout: Vec<u8>,
    pub exit: u8,
    /// The refusal a kit tool's "no" is (a failed patch, an exact `replace` that matched
    /// nothing or more than once): its id and details ([`ToolOutput::refusal`]).
    pub(crate) refusal: Option<crate::errors::ErrorReport>,
}

impl EditOutput {
    /// What an edit verb produced: the new text (`None`: apply nothing, e.g. a section not
    /// found), its report for stdout, and its exit code (0, or 1 for the tool's "no").
    /// For a pack's own verbs ([`PackVerb`]).
    pub fn new(text: Option<String>, stdout: Vec<u8>, exit: u8) -> Self {
        EditOutput {
            text,
            stdout,
            exit,
            refusal: None,
        }
    }
}

/// A pack verb's parsed call: `(doc name, live text, stdin) -> EditOutput`. A pure function
/// of its inputs, run under the session lock inside [`crate::session::Session::edit_with`]:
/// bounded, no I/O, never calling back into the kit.
pub type PackEdit = Box<dyn FnOnce(&str, &str, &[u8]) -> Result<EditOutput, ToolError>>;

/// What a pack's structured verb computed (A9): the edits to apply by key (one change; empty:
/// nothing applies), its stdout, and its exit code (0, 1 for the verb's "no", 4 for an
/// element it can't find...). A structured **read** verb returns no edits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct StructOutput {
    pub edits: Vec<crate::doc::Edit>,
    pub stdout: Vec<u8>,
    pub exit: u8,
    /// A read verb's rows or cells shown, by key (`row list`, `cell get`): with
    /// `--for-edit`, the kit takes its short claim on them under the read's lock, and the
    /// heads-up notes about those rows come with the read.
    pub shown: Option<crate::session::ClaimSpan>,
    /// The refusal a kit grid verb's "no" is, when it has its own id (`MPC-E806`, `MPC-E807`); the
    /// others are told apart by exit ([`ToolOutput::refusal`](super::ToolOutput)).
    pub(crate) refusal: Option<&'static crate::errors::ErrorEntry>,
}

impl StructOutput {
    pub fn new(edits: Vec<crate::doc::Edit>, stdout: Vec<u8>, exit: u8) -> Self {
        StructOutput {
            edits,
            stdout,
            exit,
            shown: None,
            refusal: None,
        }
    }

    /// Set [`StructOutput::shown`].
    pub fn showing(mut self, span: crate::session::ClaimSpan) -> Self {
        self.shown = Some(span);
        self
    }
}

/// What a structured verb computes on (A9): the doc as a [`crate::session::View`], its
/// stdin, and the pack's codec for the two things the kit never does itself: decoding a
/// block of records in the doc's dialect ([`VerbCtx::decode_block`]) and mapping rendered
/// lines to rows ([`VerbCtx::line_map`], `L42`).
pub struct VerbCtx<'a> {
    view: &'a crate::session::View,
    stdin: &'a [u8],
    pack: &'a dyn crate::session::Pack,
    codec: &'a [u8],
    hist: Option<&'a dyn VerbHistory>,
    /// The version positional addresses (`#N`, `LN`) count in, when it isn't the live one: an
    /// edit's `--base` (K-ADDR-3, R-1).
    at: Option<&'a crate::session::View>,
}

/// The doc's history as a structured verb may read it, under the call's lock (the session
/// gives it; [`VerbCtx::new`] alone has none).
pub(crate) trait VerbHistory {
    /// The doc's version the verb runs on.
    fn version(&self) -> Seq;
    /// The last write to one element facet, attributed; `None` without the history right
    /// or with no write recorded.
    fn last_write(
        &self,
        root: &str,
        key: &str,
        facet: crate::elem::Facet,
    ) -> Option<crate::session::ElemBlame>;
}

impl<'a> VerbCtx<'a> {
    pub fn new(
        view: &'a crate::session::View,
        stdin: &'a [u8],
        pack: &'a dyn crate::session::Pack,
        codec: &'a [u8],
    ) -> Self {
        VerbCtx {
            view,
            stdin,
            pack,
            codec,
            hist: None,
            at: None,
        }
    }
    /// Count positional addresses in `at` (an edit's `--base` version) instead of the live
    /// doc.
    pub(crate) fn with_positions_at(mut self, at: &'a crate::session::View) -> Self {
        self.at = Some(at);
        self
    }
    /// The doc as positional addresses (`#N`, `LN`) count in it (K-ADDR-3): an edit's `--base`
    /// version, else the live doc ([`VerbCtx::view`]). A key or a name always resolves live.
    pub fn positions_view(&self) -> &'a crate::session::View {
        self.at.unwrap_or(self.view)
    }
    /// The version positional addresses count in when it isn't the live doc (an edit's
    /// `--base`): `None` for reads and for edits without a `--base` or at the live version.
    pub(crate) fn positions_at(&self) -> Option<&'a crate::session::View> {
        self.at
    }
    /// What the file [`VerbCtx::positions_view`] renders says about its line counts: `LN`
    /// refuses unless they agree (`MPC-E176` on a lone CR, in a value or ending the records;
    /// `MPC-E177` when it can't be rendered). One encode and one scan of the file.
    pub(crate) fn positions_line_counts(&self) -> crate::table::LineCounts {
        crate::table::LineCounts::of(
            &self
                .pack
                .encode(&self.positions_view().content(), self.codec),
        )
    }
    /// [`VerbCtx::line_map`] of [`VerbCtx::positions_view`]: the rendered lines `LN` counts.
    pub fn positions_line_map(&self) -> Option<crate::table::LineMap> {
        self.pack.line_map(self.positions_view(), self.codec)
    }
    pub(crate) fn with_history(mut self, hist: &'a dyn VerbHistory) -> Self {
        self.hist = Some(hist);
        self
    }
    /// The doc's version the verb runs on (`None` outside a session call).
    pub fn version(&self) -> Option<Seq> {
        self.hist.map(|h| h.version())
    }
    /// The last write to one element facet (who, in which change), as `blame` reports it;
    /// `None` if the caller lacks the history right, outside a session call, or with no write
    /// recorded. E.g. who deleted a row: `(table::ROWS, key, Facet::Existence)`.
    pub fn last_write(
        &self,
        root: &str,
        key: &str,
        facet: crate::elem::Facet,
    ) -> Option<crate::session::ElemBlame> {
        self.hist.and_then(|h| h.last_write(root, key, facet))
    }
    /// `header on|off` on `t` as the pack allows it ([`crate::session::Pack::set_header`]);
    /// a refusal is exit 2 with the pack's reason, `t` unchanged.
    pub fn set_header(
        &self,
        t: &mut crate::table::Table,
        on: bool,
    ) -> Result<Vec<crate::doc::Edit>, ToolError> {
        self.set_header_declined(t, on)
            .map_err(|d| ToolError::usage(d.message))
    }
    /// [`VerbCtx::set_header`] with the pack's decline as it gave it (its code).
    pub(crate) fn set_header_declined(
        &self,
        t: &mut crate::table::Table,
        on: bool,
    ) -> Result<Vec<crate::doc::Edit>, crate::session::Declined> {
        self.pack.set_header(t, on, self.codec)
    }
    pub fn view(&self) -> &'a crate::session::View {
        self.view
    }
    /// The doc's codec settings, as the session held them for this call (the pack's,
    /// opaque to the kit).
    pub fn codec(&self) -> &'a [u8] {
        self.codec
    }
    pub fn stdin(&self) -> &'a [u8] {
        self.stdin
    }
    /// `bytes` as records in the doc's dialect ([`crate::session::Pack::decode_block`]); a
    /// decline is exit 2 with the pack's reason.
    pub fn decode_block(&self, bytes: &[u8]) -> Result<Vec<crate::table::Record>, ToolError> {
        self.pack
            .decode_block(bytes, self.codec)
            .map_err(|d| ToolError::usage(format!("stdin: {} ({})", d.message, d.code)))
    }
    /// Which row each rendered line belongs to ([`crate::session::Pack::line_map`]).
    pub fn line_map(&self) -> Option<crate::table::LineMap> {
        self.pack.line_map(self.view, self.codec)
    }
}

/// How a pack verb finds its positional address in a call's arguments
/// ([`PackVerb::positional`]).
pub type FindPositional = fn(&[String]) -> Option<String>;

/// A pack's structured verb, parsed (A9): `(the doc as a View, stdin, the pack's codec) ->
/// StructOutput`. A pure function of its inputs, run under the session lock (an edit verb
/// inside [`crate::session::Session::edit_ops_with`]): bounded, no I/O, never calling back
/// into the kit.
pub type StructEdit = Box<dyn FnOnce(&VerbCtx<'_>) -> Result<StructOutput, ToolError>>;

#[derive(Clone, Copy)]
enum VerbFn {
    /// A text edit verb (amendment 6).
    Text(fn(&[String]) -> Result<PackEdit, ToolError>),
    /// A structured edit verb (A9).
    StructEdit(fn(&[String]) -> Result<StructEdit, ToolError>),
    /// A structured read verb (A9).
    StructRead(fn(&[String]) -> Result<StructEdit, ToolError>),
}

/// A verb a pack adds beside the kit's tools: a **text edit verb** (amendment 6, e.g.
/// Markdown's `replace-section`, [`PackVerb::edit`]) or, for a structured doc (a
/// [`crate::table`] pack), a **structured verb** computing ops by key (A9, the CSV pack's
/// `row insert`, `cell get`: [`PackVerb::structured_edit`], [`PackVerb::structured_read`]).
/// Declared by [`crate::session::Pack::verbs`]; [`crate::session::Session::run_tool`] runs it
/// as it runs the kit's verbs: `parse` checks the arguments before the doc is touched (an
/// error exits 2 and applies nothing), then the parsed call computes on the live doc under
/// the session lock.
///
/// - A text edit verb computes on the live text inside `edit_with`, and the result applies
///   as one change (or, with `text: None`, nothing does). Needs read and edit.
/// - A structured edit verb computes on the doc as a [`crate::session::View`] inside
///   [`crate::session::Session::edit_ops_with`], and its edits apply as one change, by key,
///   with the same plumbing as the kit's edit verbs: `--if-version V` (doc-wide) and `--base
///   V` (per element facet, K-BR-12) are taken out of its arguments by the kit (an agent's
///   structured edit needs one of them), and the host's claim policy applies to the cells,
///   rows and columns it touches. Needs read and edit.
/// - A structured read verb computes on a View of the live doc; it returns no edits. Needs
///   read.
///
/// A structured verb's name may be two words (`row insert`): the kit matches `argv[0]
/// argv[1]` first, then `argv[0]`. The parity registry
/// ([`crate::parity::Registry::for_pack`]) lists every pack verb with the person actions it
/// serves; text verbs only for a text pack, structured verbs only for a table pack.
#[derive(Clone, Copy)]
pub struct PackVerb {
    name: &'static str,
    usage: &'static str,
    serves: &'static [crate::parity::PersonAction],
    exits: &'static [u8],
    run: VerbFn,
    /// Its positional address in a call's arguments, if any ([`PackVerb::positional`]).
    positional: Option<FindPositional>,
    /// The position units it declares, if it does ([`PackVerb::positions`]).
    positions: Option<&'static [crate::parity::positions::PositionForm]>,
}

impl PackVerb {
    /// A text edit verb. `name` is `argv[0]` (one word, not a kit verb); `usage` its flags,
    /// one line; `serves` the person actions it serves (at least one); `exits` every exit
    /// code it can end with (besides the session's refusals); `parse` gets the arguments
    /// after the verb.
    pub const fn edit(
        name: &'static str,
        usage: &'static str,
        serves: &'static [crate::parity::PersonAction],
        exits: &'static [u8],
        parse: fn(&[String]) -> Result<PackEdit, ToolError>,
    ) -> Self {
        PackVerb {
            name,
            usage,
            serves,
            exits,
            run: VerbFn::Text(parse),
            positional: None,
            positions: None,
        }
    }

    /// A structured edit verb (A9): as [`PackVerb::edit`], but `name` may be two words
    /// (`row insert`) and `parse` returns a [`StructEdit`] computing ops by key. `parse`
    /// gets the arguments after the verb's words, without `--if-version` and `--base`.
    pub const fn structured_edit(
        name: &'static str,
        usage: &'static str,
        serves: &'static [crate::parity::PersonAction],
        exits: &'static [u8],
        parse: fn(&[String]) -> Result<StructEdit, ToolError>,
    ) -> Self {
        PackVerb {
            name,
            usage,
            serves,
            exits,
            run: VerbFn::StructEdit(parse),
            positional: None,
            positions: None,
        }
    }

    /// A structured read verb (A9, `cell get`, `row list`): its [`StructEdit`] reads the
    /// View and returns no edits (edits are refused, exit 2). `--if-version` and `--base`
    /// are refused (exit 2), as for the kit's reads.
    pub const fn structured_read(
        name: &'static str,
        usage: &'static str,
        serves: &'static [crate::parity::PersonAction],
        exits: &'static [u8],
        parse: fn(&[String]) -> Result<StructEdit, ToolError>,
    ) -> Self {
        PackVerb {
            name,
            usage,
            serves,
            exits,
            run: VerbFn::StructRead(parse),
            positional: None,
            positions: None,
        }
    }

    /// **Declare its positional addresses** (K-ADDR-3, R-2): `find(args)` returns the
    /// positional address in a call's arguments (after the verb's words, without
    /// `--if-version` and `--base`), as the caller wrote it (`--nth 2`, `--row #3`), or
    /// `None`. An agent's text edit verb with one and no `--if-version` is then refused or
    /// warned about as the host chooses ([`PositionalMode`]); a structured edit verb's
    /// positional addresses count in its `--base` version ([`VerbCtx::positions_view`]).
    pub const fn positional(mut self, find: FindPositional) -> Self {
        self.positional = Some(find);
        self
    }

    /// **Declare its position units** (K-ADDR-4): each flag that takes a position, or the place
    /// it prints one, with its [`crate::parity::positions::Unit`]. `&[]` says it takes and
    /// prints none. A verb that doesn't call this still runs, and `capabilities` shows it as
    /// undeclared; [`crate::parity::positions::check`] is the test that refuses that.
    pub const fn positions(
        mut self,
        forms: &'static [crate::parity::positions::PositionForm],
    ) -> Self {
        self.positions = Some(forms);
        self
    }

    /// The position units it declares ([`PackVerb::positions`]); `None` if it declares nothing.
    pub fn declared_positions(&self) -> Option<&'static [crate::parity::positions::PositionForm]> {
        self.positions
    }

    /// The positional address in `args`, if the verb declares how to find one
    /// ([`PackVerb::positional`]).
    pub fn positional_in(&self, args: &[String]) -> Option<String> {
        self.positional.and_then(|f| f(args))
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
    pub fn usage(&self) -> &'static str {
        self.usage
    }
    pub fn serves(&self) -> &'static [crate::parity::PersonAction] {
        self.serves
    }
    pub fn exits(&self) -> &'static [u8] {
        self.exits
    }

    /// Read or edit (only [`PackVerb::structured_read`] verbs read).
    pub fn kind(&self) -> Kind {
        match self.run {
            VerbFn::StructRead(_) => Kind::Read,
            _ => Kind::Edit,
        }
    }

    /// A structured verb (A9): ops by key on a structured doc, not text.
    pub fn is_structured(&self) -> bool {
        !matches!(self.run, VerbFn::Text(_))
    }

    /// How many words of `argv` its name takes (1 or 2).
    pub fn words(&self) -> usize {
        self.name.split(' ').count()
    }

    /// Parse a text verb's arguments after the verb (`argv[1..]`). A structured verb: exit 2.
    pub fn parse(&self, args: &[String]) -> Result<PackEdit, ToolError> {
        match self.run {
            VerbFn::Text(f) => f(args),
            _ => Err(ToolError::usage(format!(
                "{} is a structured verb: it runs on a structured doc",
                self.name
            ))),
        }
    }

    /// Parse a structured verb's arguments after its words (without `--if-version` and
    /// `--base`). A text verb: exit 2.
    pub fn parse_structured(&self, args: &[String]) -> Result<StructEdit, ToolError> {
        match self.run {
            VerbFn::StructEdit(f) | VerbFn::StructRead(f) => f(args),
            VerbFn::Text(_) => Err(ToolError::usage(format!(
                "{} is a text verb: it runs on a text doc",
                self.name
            ))),
        }
    }
}

impl std::fmt::Debug for PackVerb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackVerb")
            .field("name", &self.name)
            .field("usage", &self.usage)
            .field("serves", &self.serves)
            .field("exits", &self.exits)
            .field("structured", &self.is_structured())
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

/// True when `name` (an `argv[0]`) is one of the kit's tools (or one it refuses with a
/// hint, `egrep`/`fgrep`). A pack verb by such a name is never routed.
pub fn is_kit_verb(name: &str) -> bool {
    matches!(name, "egrep" | "fgrep")
        || VERBS.iter().any(|(v, _)| v.split(' ').next() == Some(name))
}

/// Read verbs need read rights; edit verbs edit rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Read,
    Edit,
}

/// Every verb, with its kind (`sed` is a read verb, `sed -i` an edit verb).
pub const VERBS: &[(&str, Kind)] = &[
    ("cat", Kind::Read),
    ("head", Kind::Read),
    ("tail", Kind::Read),
    ("wc", Kind::Read),
    ("grep", Kind::Read),
    ("sed", Kind::Read),
    ("diff", Kind::Read),
    ("view", Kind::Read),
    ("sed -i", Kind::Edit),
    ("patch", Kind::Edit),
    ("apply", Kind::Edit),
    ("append", Kind::Edit),
    ("insert", Kind::Edit),
    ("replace", Kind::Edit),
    ("delete", Kind::Edit),
];

/// A parsed verb, ready to run (patterns and scripts compiled).
#[derive(Debug, Clone)]
pub struct Tool {
    verb: &'static str,
    inner: Inner,
    /// `--if-version V` (edit verbs only).
    if_version: Option<Seq>,
}

#[derive(Debug, Clone)]
enum Inner {
    Cat(read::Cat),
    Head(read::Head),
    Tail(read::Tail),
    Wc(read::Wc),
    Grep(Box<grep::Grep>),
    Sed(Box<sed::Sed>),
    Diff(udiff::DiffCmd),
    View(read::View),
    Patch(udiff::PatchCmd),
    Pos(edit::PosEdit),
    Exact(exact::Exact),
}

fn utf8<'a>(stdin: &'a [u8], what: &str) -> Result<&'a str, ToolError> {
    std::str::from_utf8(stdin).map_err(|_| ToolError::usage(format!("{what} is not UTF-8 text")))
}

impl Tool {
    /// Parse `argv` (`argv[0]` is the verb). Patterns and scripts are compiled here, so a
    /// bad one is refused before the doc is touched.
    pub fn parse(argv: &[String]) -> Result<Tool, ToolError> {
        let Some((verb, args)) = argv.split_first() else {
            return Err(ToolError::usage("no verb"));
        };
        // the positional verbs and `replace` hand back their --if-version; sed and patch keep it
        let mut edit_version = None;
        let mut pos = |verb: &'static str| -> Result<Inner, ToolError> {
            let (e, v) = edit::PosEdit::parse(verb, args)?;
            edit_version = v;
            Ok(Inner::Pos(e))
        };
        let (verb, inner): (&'static str, Inner) = match verb.as_str() {
            "cat" => ("cat", Inner::Cat(read::Cat::parse(args)?)),
            "head" => ("head", Inner::Head(read::Head::parse(args)?)),
            "tail" => ("tail", Inner::Tail(read::Tail::parse(args)?)),
            "wc" => ("wc", Inner::Wc(read::Wc::parse(args)?)),
            "grep" => ("grep", Inner::Grep(Box::new(grep::Grep::parse(args)?))),
            "sed" => ("sed", Inner::Sed(Box::new(sed::Sed::parse(args)?))),
            "diff" => ("diff", Inner::Diff(udiff::DiffCmd::parse(args)?)),
            "view" => ("view", Inner::View(read::View::parse(args)?)),
            "patch" => ("patch", Inner::Patch(udiff::PatchCmd::parse(args)?)),
            "apply" => ("apply", Inner::Patch(udiff::PatchCmd::parse(args)?)),
            "append" => ("append", pos("append")?),
            "insert" => ("insert", pos("insert")?),
            "delete" => ("delete", pos("delete")?),
            "replace" => (
                "replace",
                match edit::parse_replace(args)? {
                    (edit::Replace::Pos(e), v) => {
                        edit_version = v;
                        Inner::Pos(e)
                    }
                    (edit::Replace::Exact(x), v) => {
                        edit_version = v;
                        Inner::Exact(x)
                    }
                },
            ),
            "egrep" | "fgrep" => {
                return Err(ToolError::unsupported(
                    verb.clone(),
                    "use grep -E or grep -F",
                ))
            }
            v => {
                return Err(ToolError::unsupported(
                    v.to_owned(),
                    format!(
                        "verbs: {}",
                        VERBS
                            .iter()
                            .map(|(v, _)| *v)
                            .filter(|v| !v.contains(' '))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                ))
            }
        };
        let if_version = match &inner {
            Inner::Sed(s) => s.if_version,
            Inner::Patch(p) => p.if_version,
            _ => edit_version,
        };
        Ok(Tool {
            verb,
            inner,
            if_version,
        })
    }

    pub fn verb(&self) -> &'static str {
        self.verb
    }

    pub fn kind(&self) -> Kind {
        match &self.inner {
            Inner::Sed(s) if s.in_place => Kind::Edit,
            Inner::Patch(_) | Inner::Pos(_) | Inner::Exact(_) => Kind::Edit,
            _ => Kind::Read,
        }
    }

    /// `--for-edit`: `Some(ttl)` (the caller's `--ttl`, if any) on a read verb that asked
    /// for an implicit claim.
    pub fn for_edit(&self) -> Option<Option<u64>> {
        match &self.inner {
            Inner::Cat(c) => c.for_edit,
            Inner::Head(h) => h.for_edit,
            Inner::Tail(t) => t.for_edit,
            Inner::View(v) => v.for_edit,
            Inner::Grep(g) => g.for_edit,
            Inner::Sed(s) => s.for_edit,
            _ => None,
        }
    }

    /// `--ids`: a read verb asked for each line's row key (tables only).
    pub fn ids(&self) -> bool {
        match &self.inner {
            Inner::Cat(c) => c.ids,
            Inner::Head(h) => h.ids,
            Inner::Tail(t) => t.ids,
            Inner::View(v) => v.ids,
            Inner::Grep(g) => g.ids,
            _ => false,
        }
    }

    /// `--if-version V` (amendment 14): an edit verb that applies only if the doc is still at
    /// version `V`. Read verbs refuse the flag, so this is `None` for them.
    pub fn if_version(&self) -> Option<Seq> {
        self.if_version
    }

    /// The edit's positional address, as given (`--lines 3,4`, `--at 2:5`, a `sed -i` line
    /// address): it counts lines in some version (K-ADDR-3). `None` for reads, `append`,
    /// exact `replace`, `patch` (its context lines locate it) and `sed -i` scripts addressed
    /// by pattern or `$` only.
    pub fn positional(&self) -> Option<String> {
        match &self.inner {
            Inner::Pos(e) => e.positional(),
            Inner::Sed(s) if s.in_place => s.line_address().map(|n| format!("sed -i line {n}")),
            _ => None,
        }
    }

    /// What works instead when this edit verb's result on a table mixes line endings: its text
    /// ended lines in `\n` on a file whose records end otherwise. Each verb a refusal names here
    /// is tested to apply on such a file (`tests/grid_lines.rs`).
    pub(crate) fn mixed_endings_way_forward(&self) -> &'static str {
        use edit::PosEdit::{Append, InsertAfter, InsertAt, InsertLine};
        match &self.inner {
            Inner::Pos(InsertLine(_) | InsertAfter(_) | InsertAt(_)) => {
                "row insert works, or insert --at with text ending in the file's own line ending"
            }
            Inner::Pos(Append { .. }) => {
                "row insert --end works, or append with text ending in the file's own line ending"
            }
            _ => "cell set, row insert and row delete work on this file",
        }
    }

    /// `patch --dry-run`: an edit verb that never applies.
    pub fn is_dry_run(&self) -> bool {
        matches!(&self.inner, Inner::Patch(p) if p.dry_run)
    }

    /// Run a read verb. `stdin` is only read by `diff` (the old text).
    pub fn read(&self, doc: &ToolDoc<'_>, stdin: &[u8]) -> Result<ReadOutput, ToolError> {
        Ok(match &self.inner {
            Inner::Cat(c) => c.run(doc),
            Inner::Head(h) => h.run(doc),
            Inner::Tail(t) => t.run(doc),
            Inner::Wc(w) => w.run(doc),
            Inner::Grep(g) => g.run(doc)?,
            Inner::View(v) => v.run(doc),
            Inner::Sed(s) if !s.in_place => {
                let out = s.run(doc.text)?;
                ReadOutput::new(out.into_bytes(), 0).showing(s.view(lines(doc.text).len()))
            }
            Inner::Diff(d) => {
                let old = utf8(stdin, "the old text (stdin)")?;
                let (stdout, exit) = d.run(doc.name, old, doc.text);
                ReadOutput::new(stdout, exit)
            }
            _ => return Err(ToolError::usage(format!("{} is an edit verb", self.verb))),
        })
    }

    /// Run an edit verb on `text`: a pure function of the text, the arguments and stdin.
    pub fn edit(&self, name: &str, text: &str, stdin: &[u8]) -> Result<EditOutput, ToolError> {
        let done = |t: String| EditOutput {
            text: Some(t),
            stdout: Vec::new(),
            exit: exit::OK,
            refusal: None,
        };
        let out = match &self.inner {
            Inner::Sed(s) if s.in_place => done(s.run(text)?),
            Inner::Patch(p) => {
                let (t, msg, exit) = p.run(name, text, utf8(stdin, "the patch (stdin)")?)?;
                let refusal =
                    (exit != crate::tools::exit::OK).then(|| udiff::refusal(&msg, t.is_some()));
                EditOutput {
                    text: t,
                    stdout: msg.into_bytes(),
                    exit,
                    refusal,
                }
            }
            Inner::Pos(e) => done(e.apply(text, utf8(stdin, "stdin")?)?),
            Inner::Exact(x) => x.run(name, text, stdin)?,
            _ => return Err(ToolError::usage(format!("{} is a read verb", self.verb))),
        };
        if out
            .text
            .as_ref()
            .is_some_and(|t| t.len() > MAX_OUTPUT_BYTES)
        {
            return Err(too_much_output());
        }
        Ok(out)
    }
}
