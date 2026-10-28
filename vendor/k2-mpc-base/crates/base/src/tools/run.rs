//! [`Session::run_tool`]: a verb on a live doc, as one call.

use super::error::{exit, ToolRunError};
use super::fence::Fence;
use super::{EditOutput, Kind, Tool, ToolDoc, ToolError};
use crate::session::{ConnId, EditResult, EditWithError, Session};
use crate::store::Seq;

/// What [`Session::run_tool`] produced. The host prints `stdout` as is, `stderr_summary`
/// on stderr, the fence around read output when it chooses to (`--fence`/`--no-fence`),
/// and exits with `exit`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOutput {
    /// The tool's output, byte-exact (for an edit verb: `patch`'s report, else empty).
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
        if let Some(v) = argv.first().and_then(|n| self.pack_verb(n)) {
            let edit = v.parse(&argv[1..])?;
            let name = self.key()?.path;
            let doc = name.clone();
            return self.run_edit(conn, root, &name, move |live| edit(&doc, live, stdin));
        }
        let tool = Tool::parse(argv)?;
        let name = self.key()?.path;
        match tool.kind() {
            Kind::Read => {
                let (read, out) = self.read_with(conn, root, |r, text| {
                    let doc = ToolDoc {
                        name: &name,
                        text,
                        bytes: &r.bytes,
                    };
                    let out = tool.read(&doc, stdin);
                    let view = out.as_ref().ok().and_then(|o| o.view);
                    (out, view)
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
                })
            }
            Kind::Edit => self.run_edit(conn, root, &name, |live| tool.edit(&name, live, stdin)),
        }
    }

    /// An edit verb (the kit's or a pack's): `f` computes on the live text inside
    /// `edit_with`, and its text applies as one change (`None`: nothing applies).
    fn run_edit(
        &self,
        conn: ConnId,
        root: &str,
        name: &str,
        f: impl FnOnce(&str) -> Result<EditOutput, ToolError>,
    ) -> Result<ToolOutput, ToolRunError> {
        let mut report: Option<EditOutput> = None;
        let res = self.edit_with(conn, root, |live| {
            let mut o = f(live).map_err(Stop::Tool)?;
            let text = o.text.take();
            report = Some(o);
            text.ok_or(Stop::NoApply)
        });
        let (changed, applied) = match res {
            Ok(c) => (c, true),
            Err(EditWithError::Compute(Stop::NoApply)) => (None, false),
            Err(EditWithError::Compute(Stop::Tool(e))) => return Err(e.into()),
            Err(EditWithError::Session(e)) => return Err(e.into()),
        };
        let report = report.unwrap_or(EditOutput {
            text: None,
            stdout: Vec::new(),
            exit: exit::OK,
        });
        let (version, summary) = match &changed {
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
        Ok(ToolOutput {
            stdout: report.stdout,
            stderr_summary: summary,
            exit: report.exit,
            version,
            changed,
            fence: None,
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
