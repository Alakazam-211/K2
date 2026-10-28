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
pub use run::{ToolOptions, ToolOutput};
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
        format!("doc {tool} takes no file operands: the host passes the doc"),
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
}

impl ReadOutput {
    pub(crate) fn new(stdout: Vec<u8>, exit: u8) -> Self {
        ReadOutput {
            stdout,
            exit,
            view: None,
            touched: None,
        }
    }

    /// Set the viewing range, and the touched lines to it.
    pub(crate) fn showing(mut self, view: Option<(usize, usize)>) -> Self {
        self.view = view;
        self.touched = view;
        self
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
}

impl EditOutput {
    /// What an edit verb produced: the new text (`None`: apply nothing, e.g. a section not
    /// found), its report for stdout, and its exit code (0, or 1 for the tool's "no").
    /// For a pack's own verbs ([`PackVerb`]).
    pub fn new(text: Option<String>, stdout: Vec<u8>, exit: u8) -> Self {
        EditOutput { text, stdout, exit }
    }
}

/// A pack verb's parsed call: `(doc name, live text, stdin) -> EditOutput`. A pure function
/// of its inputs, run under the session lock inside [`crate::session::Session::edit_with`]:
/// bounded, no I/O, never calling back into the kit.
pub type PackEdit = Box<dyn FnOnce(&str, &str, &[u8]) -> Result<EditOutput, ToolError>>;

/// An edit verb a pack adds beside the kit's tools (amendment 6), e.g. Markdown's
/// `replace-section`. Declared by [`crate::session::Pack::verbs`];
/// [`crate::session::Session::run_tool`] runs it exactly like a kit edit verb: `parse`
/// checks the arguments before the doc is touched (an error exits 2 and applies nothing),
/// then the returned [`PackEdit`] computes on the live text inside `edit_with`, and the
/// result applies as one change (or, with `text: None`, nothing does). Needs read and
/// edit. The parity registry ([`crate::parity::Registry::for_pack`]) lists it with the
/// person actions it serves.
#[derive(Clone, Copy)]
pub struct PackVerb {
    name: &'static str,
    usage: &'static str,
    serves: &'static [crate::parity::PersonAction],
    exits: &'static [u8],
    parse: fn(&[String]) -> Result<PackEdit, ToolError>,
}

impl PackVerb {
    /// An edit verb. `name` is `argv[0]` (one word, not a kit verb); `usage` its flags, one
    /// line; `serves` the person actions it serves (at least one); `exits` every exit code
    /// it can end with (besides the session's refusals); `parse` gets the arguments after
    /// the verb.
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
            parse,
        }
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
    /// Parse the arguments after the verb (`argv[1..]`).
    pub fn parse(&self, args: &[String]) -> Result<PackEdit, ToolError> {
        (self.parse)(args)
    }
}

impl std::fmt::Debug for PackVerb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackVerb")
            .field("name", &self.name)
            .field("usage", &self.usage)
            .field("serves", &self.serves)
            .field("exits", &self.exits)
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

    /// `--if-version V` (amendment 14): an edit verb that applies only if the doc is still at
    /// version `V`. Read verbs refuse the flag, so this is `None` for them.
    pub fn if_version(&self) -> Option<Seq> {
        self.if_version
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
        };
        let out = match &self.inner {
            Inner::Sed(s) if s.in_place => done(s.run(text)?),
            Inner::Patch(p) => {
                let (t, msg, exit) = p.run(name, text, utf8(stdin, "the patch (stdin)")?)?;
                EditOutput {
                    text: t,
                    stdout: msg.into_bytes(),
                    exit,
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
