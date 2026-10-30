//! [`Session::run_tool`]: a verb on a live doc, as one call.

use super::error::{exit, ToolRunError};
use super::fence::Fence;
use super::{EditOutput, Kind, Tool, ToolDoc, ToolError};
use crate::errors::{ids, ErrorReport};
use crate::session::Shown;
use crate::session::{ClaimPolicy, ConnId, EditResult, EditWithError, ForEdit, Notes, Session};
use crate::store::Seq;

/// What [`Session::run_tool`] produced. The host prints `stdout` as is, `stderr_summary`
/// on stderr, the fence around read output when it chooses to (`--fence`/`--no-fence`),
/// and exits with `exit`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOutput {
    /// The tool's output, byte-exact (for an edit verb: `patch`'s report, exact `replace`'s
    /// refusal report or `--all` count, else empty).
    pub stdout: Vec<u8>,
    /// One line for stderr: `notes.md v1842` after a read, `edited notes.md lines 12–18
    /// (+3 −1) · v1843` after an edit (the host adds who it was), `no change to notes.md ·
    /// v1842`, or `nothing applied to notes.md · v1842`.
    pub stderr_summary: String,
    pub exit: u8,
    /// The version read, or the version after the edit.
    pub version: Seq,
    /// The applied change (edit verbs that changed something).
    pub changed: Option<EditResult>,
    /// For read verbs: the fence to print on stderr around `stdout` (see [`Fence`]).
    pub fence: Option<Fence>,
    /// Heads-up notes on the lines this call touched (amendment 13): print
    /// `notes.to_string()` on **stderr** (one `note: ` line each), or serialise them for
    /// `--presence json`. Never part of `stdout`.
    pub notes: Notes,
    /// What `--for-edit` did (read verbs that asked for it). Boxed: it is rare and large.
    pub for_edit: Option<Box<ForEdit>>,
    /// When an edit verb's "no" is a refusal (exit 1: a patch that didn't apply, an exact
    /// `replace` that matched nothing or more than once, a pack verb that applied nothing,
    /// `patch --partial`'s rejects): its id and details, for `--json`. `stderr_summary` then
    /// ends with the id, `(MPC-E125)`. `None` for reads (grep's "no match" is an answer,
    /// not a refusal) and for edits that applied cleanly.
    pub refusal: Option<ErrorReport>,
}

/// The host's choices for one [`Session::run_tool_with`] call. `#[non_exhaustive]`: start
/// from [`ToolOptions::default`] (or `new`) and use the setters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOptions {
    /// What an edit verb does about others' claims (default [`ClaimPolicy::Ignore`]).
    pub policy: ClaimPolicy,
}

impl ToolOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The claim policy for an edit verb (`--respect-claims`: [`ClaimPolicy::Respect`]).
    pub fn policy(mut self, policy: ClaimPolicy) -> Self {
        self.policy = policy;
        self
    }
}

/// What an edit verb is checked against under the session lock: the host's claim policy and
/// the caller's `--if-version`.
#[derive(Clone, Copy)]
struct Guard {
    policy: ClaimPolicy,
    if_version: Option<Seq>,
}

enum Stop {
    Tool(ToolError),
    /// The tool computed nothing to apply (a failed all-or-nothing patch, `--dry-run`).
    NoApply,
}

impl Session {
    /// Run a tool verb (`argv[0]` is the verb, without the doc path) on `root` through
    /// `conn`. See `crate::tools` for the verbs and `docs/SESSIONS.md` ("Tools").
    ///
    /// - **Read verbs** need read. They read the live doc as [`Session::read`] does
    ///   (recording a read base, so `write --base` from the returned version works, and
    ///   "contributors since last read"), run the tool on it under the same lock, and set
    ///   the connection's viewing range for line-ranged reads (`head`, `tail`, `view`,
    ///   `sed -n 'a,bp'`). The output carries a [`Fence`].
    /// - **Edit verbs** need edit and run through [`Session::edit_with`]: the tool computes
    ///   on the live text and the result applies as one atomic change (or nothing does).
    ///   The caret moves to the end of the last hunk.
    /// - **The pack's own verbs** ([`crate::session::Pack::verbs`], e.g. Markdown's
    ///   `replace-section`): a verb no kit tool has is looked up there and runs as an edit
    ///   verb, the same way (amendment 6). A kit verb's name always means the kit's verb.
    ///
    /// A refused tool ([`ToolError`], exit 2) or session ([`crate::session::SessionError`])
    /// applies nothing. A tool's "no" (grep found nothing, a patch failed) is not an error:
    /// it is `exit: 1` in the output.
    pub fn run_tool(
        &self,
        conn: ConnId,
        root: &str,
        argv: &[String],
        stdin: &[u8],
    ) -> Result<ToolOutput, ToolRunError> {
        self.run_tool_with(conn, root, argv, stdin, &ToolOptions::default())
    }

    /// [`Session::run_tool`] with the host's options: the claim policy for edit verbs
    /// ([`ToolOptions::policy`]; under [`ClaimPolicy::Respect`] an agent's edit touching
    /// another participant's live claim exits 5 with nothing applied, and a person is never
    /// refused). Read verbs ignore the policy. `run_tool` is this with the defaults.
    ///
    /// An App viewer (K5) sees the others by handle in everything the output carries: the
    /// fence's contributors, a `--for-edit` claim or conflict (or the refusal it carries), and
    /// a refusal naming someone. Notes are lensed where they are made.
    pub fn run_tool_with(
        &self,
        conn: ConnId,
        root: &str,
        argv: &[String],
        stdin: &[u8],
        opts: &ToolOptions,
    ) -> Result<ToolOutput, ToolRunError> {
        prof!("tools.run_tool");
        let out = self.run_tool_inner(conn, root, argv, stdin, opts);
        self.tool_output_as_seen(conn, out)
    }

    fn run_tool_inner(
        &self,
        conn: ConnId,
        root: &str,
        argv: &[String],
        stdin: &[u8],
        opts: &ToolOptions,
    ) -> Result<ToolOutput, ToolRunError> {
        let verb = crate::parity::verb_form(argv);
        if let Some(v) = argv.first().and_then(|n| self.pack_verb(n)) {
            let (if_version, args) = super::args::take_if_version(&argv[1..])?;
            let edit = v.parse(&args)?;
            let name = self.key()?.path;
            let doc = name.clone();
            let guard = Guard {
                policy: opts.policy,
                if_version,
            };
            let mut out = self.run_edit(conn, root, &name, guard, &verb, move |live| {
                edit(&doc, live, stdin)
            })?;
            if out.exit != exit::OK && out.changed.is_none() && out.refusal.is_none() {
                let first = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .filter(|l| !l.trim().is_empty())
                    .map_or_else(|| format!("{verb}: nothing applied"), str::to_owned);
                let r = ErrorReport::new(&ids::PACK_VERB_NO, first).with("verb", verb.as_str());
                out.stderr_summary = format!("{} ({})", out.stderr_summary, r.id());
                out.refusal = Some(r);
            }
            return Ok(out);
        }
        let tool = Tool::parse(argv)?;
        let name = self.key()?.path;
        match tool.kind() {
            Kind::Read => {
                let (read, out, notes, for_edit) =
                    self.read_with(conn, root, &verb, tool.for_edit(), |r, text| {
                        let doc = ToolDoc {
                            name: &name,
                            text,
                            bytes: &r.bytes,
                        };
                        let out = tool.read(&doc, stdin);
                        let shown = out.as_ref().map_or(Shown::default(), |o| Shown {
                            view: o.view,
                            touched: o.touched,
                        });
                        (out, shown)
                    })?;
                let out = out?;
                let fence = Fence::new(
                    &name,
                    read.version,
                    read.contributors_since_last_read,
                    &out.stdout,
                );
                Ok(ToolOutput {
                    stdout: out.stdout,
                    stderr_summary: format!("{name} v{}", read.version),
                    exit: out.exit,
                    version: read.version,
                    changed: None,
                    fence: Some(fence),
                    notes,
                    for_edit: for_edit.map(Box::new),
                    refusal: None,
                })
            }
            Kind::Edit => {
                let guard = Guard {
                    policy: opts.policy,
                    if_version: tool.if_version(),
                };
                self.run_edit(conn, root, &name, guard, &verb, |live| {
                    tool.edit(&name, live, stdin)
                })
            }
        }
    }

    /// An edit verb (the kit's or a pack's): `f` computes on the live text inside
    /// `edit_with`, and its text applies as one change (`None`: nothing applies).
    fn run_edit(
        &self,
        conn: ConnId,
        root: &str,
        name: &str,
        guard: Guard,
        verb: &str,
        f: impl FnOnce(&str) -> Result<EditOutput, ToolError>,
    ) -> Result<ToolOutput, ToolRunError> {
        let mut report: Option<EditOutput> = None;
        let res = self.edit_with_impl(
            conn,
            root,
            guard.policy,
            guard.if_version,
            Some(verb),
            None,
            |live| {
                let mut o = f(live).map_err(Stop::Tool)?;
                let text = o.text.take();
                report = Some(o);
                text.ok_or(Stop::NoApply)
            },
        );
        let (changed, notes, applied) = match res {
            Ok((c, n)) => (c, n, true),
            Err(EditWithError::Compute(Stop::NoApply)) => (None, Notes::default(), false),
            Err(EditWithError::Compute(Stop::Tool(e))) => return Err(e.into()),
            Err(EditWithError::Session(e)) => return Err(e.into()),
        };
        let mut report = report.unwrap_or(EditOutput {
            text: None,
            stdout: Vec::new(),
            exit: exit::OK,
            refusal: None,
        });
        let refusal = report.refusal.take().filter(|_| report.exit != exit::OK);
        let (version, mut summary) = match &changed {
            Some(c) => (c.version, edited(name, c)),
            None => {
                let v = self.status()?.version;
                let what = if applied {
                    "no change to"
                } else {
                    "nothing applied to"
                };
                (v, format!("{what} {name} · v{v}"))
            }
        };
        if let Some(r) = &refusal {
            summary = format!("{summary} ({})", r.id());
        }
        Ok(ToolOutput {
            stdout: report.stdout,
            stderr_summary: summary,
            exit: report.exit,
            version,
            changed,
            fence: None,
            notes,
            for_edit: None,
            refusal,
        })
    }
}

fn edited(name: &str, c: &EditResult) -> String {
    let first = c.changed.iter().map(|r| r.first).min().unwrap_or(1);
    let last = c.changed.iter().map(|r| r.last).max().unwrap_or(first);
    let lines = if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    };
    format!(
        "edited {name} {lines} (+{} −{}) · v{}",
        c.lines_added, c.lines_removed, c.version
    )
}
