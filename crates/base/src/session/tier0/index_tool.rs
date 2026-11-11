//! [`BlobSession::run_tool`]: the structure index's verbs and claims by node, as one call
//! (slice A2, A5). A thin parser over the typed calls ([`BlobSession::index_list`],
//! [`BlobSession::index_nodes`], [`BlobSession::index_diff`], [`BlobSession::index_blame`],
//! [`BlobSession::claim_nodes`] and the rest), so a verb and a host's view see the same thing;
//! the verbs add the read budget, cursors and output ([`super::index_page`]).
//!
//! The verbs go by the pack's names ([`crate::index::IndexVerbs`]): `<noun> list`, `<noun>
//! show`, `<noun> diff`, `<noun> blame`, with `outline` and `node` the kit's names for the
//! first two (K-READ-1), and `claim --node`, `claim --renew`, `claim --release`. Every other
//! verb of a blob doc is its own call, not this one.

use super::index_page::{self as page, Hints, Shape};
use super::index_read::{check_addrs, nodes_of};
use super::*;
use crate::errors::{ids, ErrorReport, Value};
use crate::index::read::{self, not_found, CursorScope, Frame};
use crate::index::{IndexVerbs, ListQuery, NodeKey, NodeRead, NodesRead, READ_BUDGET_MIN};
use crate::session::awareness::{ClaimId, ClaimInfo};
use crate::tools::{exit, Fence, PositionalMode, ToolError, ToolOptions, ToolOutput, ToolRunError};

/// What an index verb is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    List,
    Show,
    Diff,
    Blame,
    Export,
    Claim,
    ClaimRenew,
    ClaimRelease,
}

impl Verb {
    /// The kit's name for it (what a cursor is bound to).
    fn word(self) -> &'static str {
        match self {
            Verb::List => "list",
            Verb::Show => "show",
            Verb::Diff => "diff",
            Verb::Blame => "blame",
            Verb::Export => "export",
            Verb::Claim => "claim",
            Verb::ClaimRenew => "claim --renew",
            Verb::ClaimRelease => "claim --release",
        }
    }
}

/// The flags of a claim call that take a value (the value is never read as a flag).
const CLAIM_VALUE_FLAGS: &[&str] = &["--note", "--ttl", "--id", "--version"];

/// The mode of a `claim` call: `--renew` or `--release` as a flag of the call (never inside
/// a flag's value: `--note --release` is a note), else a new claim.
fn claim_mode(args: &[String]) -> Verb {
    let mut it = args.iter();
    while let Some(t) = it.next() {
        match t.as_str() {
            "--" => break,
            "--renew" => return Verb::ClaimRenew,
            "--release" => return Verb::ClaimRelease,
            f if CLAIM_VALUE_FLAGS.contains(&f) => {
                it.next();
            }
            _ => {}
        }
    }
    Verb::Claim
}

/// The verb `argv` names and how many words it took, if it is one of these.
fn verb_of(verbs: Option<&IndexVerbs>, argv: &[String]) -> Option<(Verb, usize)> {
    let first = argv.first()?.as_str();
    match first {
        "outline" => return Some((Verb::List, 1)),
        "node" => return Some((Verb::Show, 1)),
        "claim" => return Some((claim_mode(&argv[1..]), 1)),
        _ => {}
    }
    let v = verbs?;
    let two = format!("{first} {}", argv.get(1)?);
    let found = [
        (v.list, Verb::List),
        (v.show, Verb::Show),
        (v.diff, Verb::Diff),
        (v.blame, Verb::Blame),
    ]
    .into_iter()
    .chain(v.export.map(|e| (e, Verb::Export)))
    .find(|(name, _)| *name == two)?;
    Some((found.1, 2))
}

/// The flags of one verb call.
#[derive(Default)]
struct Args {
    version: Option<VersionId>,
    depth: Option<u16>,
    kind: Option<String>,
    under: Option<String>,
    cursor: Option<String>,
    ttl: Option<u64>,
    note: Option<String>,
    id: Option<ClaimId>,
    tree: bool,
    no_truncate: bool,
    json: bool,
    text: bool,
    whole: bool,
    /// After `--node` (claims), or plain operands (show, diff).
    operands: Vec<String>,
}

fn num<T: std::str::FromStr>(flag: &str, v: &str) -> Result<T, ToolError> {
    v.parse()
        .map_err(|_| ToolError::usage(format!("{flag}: not a number: {v:?}")))
}

/// Parse `args` with the flags `allowed` (by name, without their values).
fn parse_args(args: &[String], allowed: &[&str], usage: &str) -> Result<Args, ToolError> {
    let mut a = Args::default();
    let mut it = args.iter();
    let mut flags_done = false;
    while let Some(tok) = it.next() {
        if flags_done || !tok.starts_with("--") || tok == "--" {
            if tok == "--" && !flags_done {
                flags_done = true;
                continue;
            }
            a.operands.push(tok.clone());
            continue;
        }
        let (name, inline) = match tok.split_once('=') {
            Some((n, v)) => (n, Some(v.to_owned())),
            None => (tok.as_str(), None),
        };
        if !allowed.contains(&name) {
            return Err(ToolError::unsupported(name, format!("usage: {usage}")));
        }
        let mut value = || -> Result<String, ToolError> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| ToolError::usage(format!("{name} needs a value"))),
            }
        };
        match name {
            "--version" => a.version = Some(num(name, &value()?)?),
            "--depth" => a.depth = Some(num(name, &value()?)?),
            "--kind" => a.kind = Some(value()?),
            "--under" => a.under = Some(value()?),
            "--cursor" => a.cursor = Some(value()?),
            "--ttl" => a.ttl = Some(num(name, &value()?)?),
            "--note" => a.note = Some(value()?),
            "--id" => {
                let v = value()?;
                a.id = Some(
                    ClaimId::parse(&v)
                        .ok_or_else(|| ToolError::usage(format!("--id: not a claim id: {v:?}")))?,
                );
            }
            // what follows are addresses
            "--node" => {}
            "--tree" | "--no-truncate" | "--json" | "--text" | "--whole" | "--renew"
            | "--release"
                if inline.is_some() =>
            {
                return Err(ToolError::usage(format!("{name} takes no value")));
            }
            "--tree" => a.tree = true,
            "--no-truncate" => a.no_truncate = true,
            "--json" => a.json = true,
            "--text" => a.text = true,
            "--whole" => a.whole = true,
            "--renew" | "--release" => {}
            _ => return Err(ToolError::unsupported(name, format!("usage: {usage}"))),
        }
    }
    Ok(a)
}

/// A structure read ready to page: its frames, its items whole, how they print, and what its
/// cursor is bound to.
struct Paged {
    shape: Shape,
    /// The first page's frame (a node read: the misses, capped), and its text lines.
    first: Frame,
    first_text: Vec<String>,
    /// Every later page's frame.
    rest: Frame,
    /// The whole answer's frame (every miss), for `--no-truncate`, and its text lines.
    whole: Frame,
    whole_text: Vec<String>,
    items: Vec<Value>,
    hints: Hints,
    verb: Verb,
    /// The arguments that pick the items, canonically.
    args: String,
    version: VersionId,
}

/// A page with no items: `[last page, a page with a cursor]`, the larger of its JSON and its
/// text.
fn env_of(frame: &Frame, header: &[String]) -> [usize; 2] {
    let text: usize = header.iter().map(|l| l.len() + 1).sum();
    let footer = "truncated: more with --cursor ".len() + read::CURSOR_LEN + 1;
    [
        read::frame_len(frame, false).max(text),
        read::frame_len(frame, true).max(text + footer),
    ]
}

/// The first page's misses: whole entries, in order, while they take at most a quarter of
/// the budget (`missing_total` counts them all; `MPC-E172` lists the first
/// [`read::MISSING_LISTED_MAX`]). JSON values and text lines.
fn missing_head(r: &NodesRead, budget: usize) -> (Vec<Value>, Vec<String>) {
    let cap = budget / 4;
    let (mut json, mut text, mut used) = (Vec::new(), Vec::new(), 0usize);
    let line = |m: &crate::index::MissingAddress| {
        let near: Vec<String> = m
            .candidates
            .iter()
            .map(|c| format!("{} {}", c.address, c.position.as_deref().unwrap_or("")))
            .collect();
        let more = m.candidates_total.saturating_sub(m.candidates.len());
        format!(
            "missing: {}{}{}",
            page::quoted(&m.address),
            if near.is_empty() {
                String::new()
            } else {
                format!("; near: {}", near.join(", "))
            },
            if more > 0 {
                format!(" (+{more} more)")
            } else {
                String::new()
            }
        )
    };
    #[cfg(any(test, feature = "differential"))]
    let every = crate::differential::index_mutant()
        == Some(crate::differential::IndexMutant::MissingUncapped);
    #[cfg(not(any(test, feature = "differential")))]
    let every = false;
    for m in &r.missing {
        let v = m.to_value();
        let l = line(m);
        let n = (v.to_json().len() + 1).max(l.len() + 1);
        if !every && used + n > cap {
            break;
        }
        used += n;
        json.push(v);
        text.push(l);
    }
    if json.len() < r.missing.len() {
        text.push(format!(
            "missing: {} more not listed",
            r.missing.len() - json.len()
        ));
    }
    (json, text)
}

impl BlobSession {
    /// **Run an index verb or a node claim** (`argv[0]` is the verb, without the doc path), as
    /// [`crate::session::Session::run_tool`] does for a text doc. The verbs (the pack's names,
    /// [`crate::index::IndexVerbs`]; `outline` and `node` too):
    ///
    /// - `<list> [--version V] [--depth N] [--kind K] [--under ADDR] [--tree] [--cursor C]
    ///   [--no-truncate] [--json]`: [`BlobSession::index_list`].
    /// - `<show> ADDR… [--version V] [--depth N] [--cursor C] [--no-truncate] [--json]`:
    ///   [`BlobSession::index_nodes`]. Exit 4 with `MPC-E172` when any address names
    ///   nothing: every address is still listed, `null` for the missing ones; the first page
    ///   and the refusal's `details.missing` say what exists. `<show> ADDR --text`: the one
    ///   node's text, whole, as it is; `<show> ADDR --whole`: the node and its ancestors in
    ///   full, as JSON (neither paged nor budgeted; `--text` exits 1 when there is none).
    /// - `<diff> V1 [V2] [--cursor C] [--no-truncate] [--json]` (V2: the current version):
    ///   [`BlobSession::index_diff`].
    /// - `<blame> [--version V] [--cursor C] [--no-truncate] [--json]`:
    ///   [`BlobSession::index_blame`].
    /// - `claim --node ADDR… [--version V] [--ttl MS] [--note TEXT]`,
    ///   `claim --renew [--id ID] [--ttl MS]`, `claim --release [--id ID]` (exit 1: none):
    ///   [`BlobSession::claim_nodes`], [`BlobSession::renew_claim`],
    ///   [`BlobSession::release_claim`]. An agent's `#N` names a version with `--version`
    ///   ([`ToolOptions::positional`], `MPC-E171`).
    ///
    /// **Pages** (K-READ-2): every page is at most [`ToolOptions::read_budget`] bytes, in text
    /// and in JSON: an item counts as the larger of its two forms, so a page is the same page
    /// in both. An item that couldn't fit a page alone is shortened until it does, saying what
    /// it left out and the read that returns it ([`super::index_page`]). Past the budget the
    /// page ends `truncated: true` with a cursor, bound to this doc, the version read, the
    /// verb and the arguments that pick the items: pass it back with `--cursor` and the same
    /// arguments for the next page. Used on another version (the doc moved on, for a read of
    /// the current one), doc or read: `MPC-E173 stale_cursor` (exit 1); not a cursor:
    /// `MPC-E174 bad_cursor` (exit 2). `--no-truncate`, when the whole answer is over the
    /// budget (more pages, or an item shortened): `MPC-E175 over_budget` (exit 2), nothing
    /// read. A node read's misses are on its first page only.
    ///
    /// Rights are the typed calls' and are asked on every call, every page included, before a
    /// cursor is looked at. Read output carries a [`Fence`]: names and text come from the file.
    /// The annotation verbs (`comment`, `reply`, ..., [`crate::parity::ANNOTATION_VERBS`]) are
    /// handed to [`BlobSession::run_annotation`], on any pack: one line a thread and a comment,
    /// or JSON with `--json`.
    ///
    /// A type with no index: `MPC-E711` (`outline`, `node`, `claim`); any other verb: a usage
    /// error naming its own call.
    pub fn run_tool(
        &self,
        conn: ConnId,
        argv: &[String],
        opts: &ToolOptions,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        // the annotation verbs: theirs, over the same calls (any pack)
        let form = crate::parity::verb_form(argv);
        if crate::parity::ANNOTATION_VERBS
            .iter()
            .any(|v| v.name == form)
        {
            let json = opts.json || argv.iter().any(|a| a == "--json");
            let argv: Vec<String> = argv.iter().filter(|a| *a != "--json").cloned().collect();
            let out = self.run_annotation(conn, &argv)?;
            let version = self.status()?.version;
            let stdout = if json {
                (crate::annot::render::json(&out).to_json() + "\n").into_bytes()
            } else {
                crate::annot::render::text(&out).into_bytes()
            };
            return Ok(tool_output(
                stdout,
                format!("{form} · v{version}"),
                exit::OK,
                version,
            ));
        }
        let spec = self.pack.index_spec();
        let Some((verb, words)) = verb_of(spec.as_ref().map(|s| s.verbs), argv) else {
            let name = argv.first().map(String::as_str).unwrap_or("");
            return Err(ToolError::usage(format!(
                "{name:?}: not a verb run_tool runs on a blob doc (its index verbs and node \
                 claims; every other verb is its own call)"
            ))
            .into());
        };
        let Some(spec) = spec else {
            return Err(SessionError::IndexUnsupported.into());
        };
        let name = |v: Verb| -> &'static str {
            match v {
                Verb::List => spec.verbs.list,
                Verb::Show => spec.verbs.show,
                Verb::Diff => spec.verbs.diff,
                Verb::Blame => spec.verbs.blame,
                Verb::Export => spec.verbs.export.unwrap_or("export"),
                other => other.word(),
            }
        };
        let args = &argv[words..];
        let json = opts.json;
        let budget = opts.read_budget.max(READ_BUDGET_MIN);
        let hints = |version: VersionId, from: VersionId, depth: u16| Hints {
            show: spec.verbs.show,
            list: spec.verbs.list,
            version,
            from,
            depth,
        };
        match verb {
            Verb::List => {
                let a = parse_args(
                    args,
                    &[
                        "--version",
                        "--depth",
                        "--kind",
                        "--under",
                        "--tree",
                        "--cursor",
                        "--no-truncate",
                        "--json",
                    ],
                    "[--version V] [--depth N] [--kind K] [--under ADDR] [--tree] [--cursor C] \
                     [--no-truncate] [--json]",
                )?;
                if let Some(o) = a.operands.first() {
                    return Err(
                        ToolError::usage(format!("{o}: {} takes no operands", name(verb))).into(),
                    );
                }
                let mut q = ListQuery::new();
                q.version = a.version;
                q.depth = a.depth;
                q.kind = a.kind.clone();
                q.under = a.under.clone();
                let o = self.index_list(conn, &q)?;
                let paged = Paged {
                    shape: Shape::Outline { tree: a.tree },
                    first: o.frame(),
                    first_text: Vec::new(),
                    rest: o.frame(),
                    whole: o.frame(),
                    whole_text: Vec::new(),
                    items: o.items(),
                    hints: hints(o.version, o.version, 0),
                    verb,
                    args: format!(
                        "v={:?};depth={:?};kind={:?};under={:?}",
                        a.version, a.depth, a.kind, a.under
                    ),
                    version: o.version,
                };
                self.page_out(paged, &a, json, budget, None)
            }
            Verb::Show => {
                let a = parse_args(
                    args,
                    &[
                        "--version",
                        "--depth",
                        "--cursor",
                        "--no-truncate",
                        "--json",
                        "--text",
                        "--whole",
                    ],
                    "ADDR... [--version V] [--depth N] [--cursor C] [--no-truncate] [--json] | \
                     ADDR --text | ADDR --whole",
                )?;
                check_addrs(&a.operands)?;
                let depth = a.depth.unwrap_or(0);
                if a.text || a.whole {
                    if a.text && a.whole
                        || a.operands.len() != 1
                        || a.cursor.is_some()
                        || a.depth.is_some()
                    {
                        return Err(ToolError::usage(
                            "--text and --whole read one node: one address, no --depth or \
                             --cursor, one of the two",
                        )
                        .into());
                    }
                    return self.show_one(conn, &a.operands[0], a.version, a.text);
                }
                let r = self.index_nodes(conn, &a.operands, a.version, depth)?;
                let addrs = &a.operands;
                let (head, head_text) = missing_head(&r, budget);
                let whole_text = missing_head_all(&r);
                let paged = Paged {
                    shape: Shape::Node,
                    first: r.frame_with(head),
                    first_text: head_text,
                    rest: r.frame_rest(),
                    whole: r.frame(),
                    whole_text,
                    items: r.items(addrs),
                    hints: hints(r.version, r.version, depth),
                    verb,
                    args: format!("v={:?};depth={depth};addrs={:?}", a.version, addrs),
                    version: r.version,
                };
                let missing = (!r.missing.is_empty()).then(|| Box::new(not_found(&r)));
                self.page_out(paged, &a, json, budget, missing)
            }
            Verb::Diff => {
                let a = parse_args(
                    args,
                    &["--cursor", "--no-truncate", "--json"],
                    "V1 [V2] [--cursor C] [--no-truncate] [--json]",
                )?;
                let (from, to) = match a.operands.as_slice() {
                    [f] => (num::<VersionId>("V1", f)?, self.current_version()?),
                    [f, t] => (num("V1", f)?, num("V2", t)?),
                    _ => {
                        return Err(ToolError::usage(format!(
                            "{} V1 [V2]: two versions (V2: the current one)",
                            name(verb)
                        ))
                        .into())
                    }
                };
                let d = self.index_diff(conn, from, to)?;
                let paged = Paged {
                    shape: Shape::Change,
                    first: d.frame(),
                    first_text: Vec::new(),
                    rest: d.frame(),
                    whole: d.frame(),
                    whole_text: Vec::new(),
                    items: d.items(),
                    hints: hints(to, from, 0),
                    verb,
                    args: format!("from={from};to={to}"),
                    version: to,
                };
                self.page_out(paged, &a, json, budget, None)
            }
            Verb::Blame => {
                let a = parse_args(
                    args,
                    &["--version", "--cursor", "--no-truncate", "--json"],
                    "[--version V] [--cursor C] [--no-truncate] [--json]",
                )?;
                if let Some(o) = a.operands.first() {
                    return Err(
                        ToolError::usage(format!("{o}: {} takes no operands", name(verb))).into(),
                    );
                }
                let b = self.index_blame(conn, a.version)?;
                let paged = Paged {
                    shape: Shape::Blame,
                    first: b.frame(),
                    first_text: Vec::new(),
                    rest: b.frame(),
                    whole: b.frame(),
                    whole_text: Vec::new(),
                    items: b.items(),
                    hints: hints(b.version, b.version, 0),
                    verb,
                    args: format!("v={:?}", a.version),
                    version: b.version,
                };
                self.page_out(paged, &a, json, budget, None)
            }
            Verb::Export => Err(ToolError::usage(format!(
                "{}: an export runs through the host's export registry with the node option, \
                 not run_tool",
                name(verb)
            ))
            .into()),
            Verb::Claim => {
                let a = parse_args(
                    args,
                    &["--node", "--version", "--ttl", "--note", "--json"],
                    "claim --node ADDR... [--version V] [--ttl MS] [--note TEXT]",
                )?;
                if !args
                    .iter()
                    .any(|t| t == "--node" || t.starts_with("--node="))
                {
                    return Err(ToolError::usage(
                        "claim on a blob doc names nodes: claim --node ADDR...",
                    )
                    .into());
                }
                let mut addrs = a.operands.clone();
                addrs.extend(
                    args.iter()
                        .filter_map(|t| t.strip_prefix("--node="))
                        .map(str::to_owned),
                );
                let warning = self.positional_claim(conn, &addrs, a.version, opts)?;
                check_addrs(&addrs)?;
                let keys = self.keys_of(conn, &addrs, a.version)?;
                let c = self.claim_nodes(conn, &keys, a.ttl, a.note.clone())?;
                let mut out = claim_out(&[c], json || a.json, "claimed", self.current_version()?);
                out.warnings.extend(warning);
                Ok(out)
            }
            Verb::ClaimRenew => {
                let a = parse_args(
                    args,
                    &["--renew", "--id", "--ttl", "--json"],
                    "claim --renew [--id ID] [--ttl MS]",
                )?;
                no_operands(&a)?;
                let c = self.renew_claim(conn, a.id, a.ttl)?;
                Ok(claim_out(
                    &[c],
                    json || a.json,
                    "renewed",
                    self.current_version()?,
                ))
            }
            Verb::ClaimRelease => {
                let a = parse_args(
                    args,
                    &["--release", "--id", "--json"],
                    "claim --release [--id ID]",
                )?;
                no_operands(&a)?;
                let ids = self.release_claim(conn, a.id)?;
                let (stdout, exit) = if json || a.json {
                    let v = Value::obj([(
                        "released",
                        Value::List(ids.iter().map(|i| Value::from(i.to_string())).collect()),
                    )]);
                    (v.to_json() + "\n", u8::from(ids.is_empty()))
                } else {
                    let s: String = ids.iter().map(|i| format!("released {i}\n")).collect();
                    (s, u8::from(ids.is_empty()))
                };
                Ok(tool_output(
                    stdout.into_bytes(),
                    if ids.is_empty() {
                        "no claim released".to_owned()
                    } else {
                        format!("released {} claim(s)", ids.len())
                    },
                    exit,
                    self.current_version()?,
                ))
            }
        }
    }

    /// An agent's claim by `#N` with no `--version` (K-ADDR-3): a warning, or `MPC-E171`.
    fn positional_claim(
        &self,
        conn: ConnId,
        addrs: &[String],
        version: Option<VersionId>,
        opts: &ToolOptions,
    ) -> std::result::Result<Vec<String>, ToolRunError> {
        if version.is_some() {
            return Ok(Vec::new());
        }
        let Some(pos) = addrs.iter().find(|a| {
            crate::addr::parse(a, crate::addr::Forms::none().native().nth())
                .is_ok_and(|p| p.is_positional())
        }) else {
            return Ok(Vec::new());
        };
        let agent = {
            let st = self.lock()?;
            let i = Self::member(&st, conn)?;
            st.members[i].p.kind == ParticipantKind::Agent
        };
        if !agent {
            return Ok(Vec::new());
        }
        match opts.positional {
            PositionalMode::Refuse => Err(ToolError::address(
                ErrorReport::new(
                    &ids::POSITIONAL_NEEDS_VERSION,
                    format!(
                        "{pos} is a position; an agent's claim by position names the version \
                         it read (--version V); nothing claimed"
                    ),
                )
                .with("verb", "claim")
                .with("address", pos.as_str()),
            )
            .into()),
            _ => Ok(vec![crate::tools::positional_note(pos)]),
        }
    }

    /// The keys `addrs` name in `version` (`None`: the current one); a missing address is
    /// `MPC-E172` (nothing is claimed), never a guess.
    fn keys_of(
        &self,
        conn: ConnId,
        addrs: &[String],
        version: Option<VersionId>,
    ) -> std::result::Result<Vec<NodeKey>, ToolRunError> {
        let view = {
            let (spec, t) = self.admit_index(conn, &[version], false, Action::Claim)?;
            self.load_view(&t[0], &spec)?
        };
        let r = nodes_of(&read::Walk::new(&view), addrs, 0)?;
        if !r.missing.is_empty() {
            return Err(SessionError::IndexRead(Box::new(not_found(&r))).into());
        }
        Ok(r.nodes.into_iter().flatten().map(|n| n.node.key).collect())
    }

    /// `<show> ADDR --text` (`text`) or `<show> ADDR --whole`: one node's text as it is, or
    /// the node and its ancestors in full as JSON. Neither paged nor budgeted: the reads a
    /// shortened item names.
    fn show_one(
        &self,
        conn: ConnId,
        addr: &str,
        version: Option<VersionId>,
        text: bool,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let addrs = [addr.to_owned()];
        let r = self.index_nodes(conn, &addrs, version, 0)?;
        let name = self.key()?.path;
        let Some(Some(n)) = r.nodes.first() else {
            let report = not_found(&r);
            let mut out = tool_output(
                Vec::new(),
                format!("{name} v{} ({})", r.version, report.id()),
                exit::NOT_FOUND,
                r.version,
            );
            out.refusal = Some(report);
            return Ok(out);
        };
        let (stdout, exit) = if text {
            match &n.node.text {
                Some(t) => (t.clone().into_bytes(), exit::OK),
                None => (Vec::new(), exit::NO),
            }
        } else {
            let v = NodeRead::entry_value(addr, Some(n), true);
            ((v.to_json() + "\n").into_bytes(), exit::OK)
        };
        let mut out = tool_output(stdout, format!("{name} v{}", r.version), exit, r.version);
        out.fence = Some(Fence::new(&name, r.version, Vec::new(), &out.stdout));
        Ok(out)
    }

    /// Page a read (K-READ-2) and print it.
    fn page_out(
        &self,
        p: Paged,
        a: &Args,
        json: bool,
        budget: usize,
        missing: Option<Box<ErrorReport>>,
    ) -> std::result::Result<ToolOutput, ToolRunError> {
        let json = json || a.json;
        let key = self.key()?;
        let space_key = self.space_key()?;
        let scope = CursorScope {
            space_key: &space_key,
            doc: &key,
            version: p.version,
            verb: p.verb.word(),
            args: &p.args,
        };
        let start = match &a.cursor {
            Some(c) => scope.take(c).map_err(SessionError::IndexRead)? as usize,
            None => 0,
        };
        if start > p.items.len() {
            return Err(SessionError::IndexRead(Box::new(ErrorReport::new(
                &ids::STALE_CURSOR,
                "the cursor is for another version, doc or call; nothing was read",
            )))
            .into());
        }
        // every item shortened to fit a page by itself, against the largest frame
        let env_first = env_of(&p.first, &p.first_text);
        let env_rest = env_of(&p.rest, &[]);
        let room = budget.saturating_sub(env_first[1].max(env_rest[1]));
        let mut shortened = false;
        let items: Vec<Value> = p
            .items
            .iter()
            .map(|v| {
                let (v, cut) = page::fit(p.shape, v.clone(), room, &p.hints);
                shortened |= cut;
                v
            })
            .collect();
        let sizes: Vec<usize> = items.iter().map(|v| page::size(p.shape, v)).collect();
        let env = if start == 0 { env_first } else { env_rest };
        let c = read::cut(env, &sizes, start, budget);
        // --no-truncate: everything or nothing, against the whole answer
        if a.no_truncate {
            let frame = if start == 0 { &p.whole } else { &p.rest };
            let json_len = frame.whole(p.items[start..].to_vec()).to_json().len();
            let head: &[String] = if start == 0 { &p.whole_text } else { &[] };
            let text_len: usize = head.iter().map(|l| l.len() + 1).sum::<usize>()
                + p.items[start..]
                    .iter()
                    .map(|v| page::text(p.shape, v).len())
                    .sum::<usize>();
            let whole = json_len.max(text_len);
            #[cfg(any(test, feature = "differential"))]
            let over = match crate::differential::index_mutant() {
                Some(crate::differential::IndexMutant::NoTruncatePages) => false,
                Some(crate::differential::IndexMutant::NoTruncateOnlyOnMore) => c.more,
                _ => whole > budget || shortened || c.more,
            };
            #[cfg(not(any(test, feature = "differential")))]
            let over = whole > budget || shortened || c.more;
            if over {
                return Err(SessionError::IndexRead(Box::new(
                    ErrorReport::new(
                        &ids::OVER_BUDGET,
                        format!(
                            "the result is {whole} bytes, over the {budget}-byte read budget; \
                             --no-truncate refuses a page"
                        ),
                    )
                    .with("bytes", whole)
                    .with("budget", budget),
                ))
                .into());
            }
        }
        let cursor = c.more.then(|| scope.make(c.end as u32));
        let frame = if start == 0 { &p.first } else { &p.rest };
        #[cfg(any(test, feature = "differential"))]
        let frame = if crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::MissingUncapped)
        {
            &p.first
        } else {
            frame
        };
        let stdout = if json {
            frame
                .page(items[start..c.end].to_vec(), cursor.as_deref())
                .to_json()
                + "\n"
        } else {
            let mut s = String::new();
            if start == 0 {
                for l in &p.first_text {
                    s.push_str(l);
                    s.push('\n');
                }
            }
            for v in &items[start..c.end] {
                s.push_str(&page::text(p.shape, v));
            }
            if let Some(cur) = &cursor {
                s.push_str(&format!("truncated: more with --cursor {cur}\n"));
            }
            s
        };
        if stdout.len() > crate::tools::MAX_OUTPUT_BYTES {
            return Err(ToolError::Limit(format!(
                "output over {} bytes",
                crate::tools::MAX_OUTPUT_BYTES
            ))
            .into());
        }
        let name = key.path.clone();
        let mut summary = format!("{name} v{}", p.version);
        if c.more {
            summary.push_str(&format!(
                " (items {}-{} of {}; more with --cursor)",
                start + 1,
                c.end,
                items.len()
            ));
        }
        let stdout = stdout.into_bytes();
        #[cfg(any(test, feature = "differential"))]
        let partial_ok = crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::PartialExitsZero);
        #[cfg(not(any(test, feature = "differential")))]
        let partial_ok = false;
        let (exit, refusal) = match missing {
            Some(r) if !partial_ok => {
                summary.push_str(&format!(" ({})", r.id()));
                (r.exit(), Some(*r))
            }
            _ => (exit::OK, None),
        };
        let mut out = tool_output(stdout, summary, exit, p.version);
        out.fence = Some(Fence::new(&name, p.version, Vec::new(), &out.stdout));
        out.refusal = refusal;
        Ok(out)
    }

    fn current_version(&self) -> Result<VersionId> {
        Ok(self.lock()?.vid)
    }

    /// The space key cursors are tagged under.
    fn space_key(&self) -> Result<[u8; 32]> {
        let st = self.lock()?;
        let store = Self::store(&st)?;
        let version = st.vid;
        store
            .blob_space(&st.key.space)?
            .map(|s| s.key)
            .ok_or(SessionError::NotIndexed {
                version,
                why: crate::index::NotIndexedWhy::Purged,
            })
    }
}

/// Every miss as text (what `--no-truncate` measures).
fn missing_head_all(r: &NodesRead) -> Vec<String> {
    missing_head(r, usize::MAX).1
}

fn no_operands(a: &Args) -> std::result::Result<(), ToolError> {
    match a.operands.first() {
        Some(o) => Err(ToolError::usage(format!("{o}: no operands here"))),
        None => Ok(()),
    }
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
    }
}

fn claim_value(c: &ClaimInfo) -> Value {
    let keys = match &c.span {
        crate::session::ClaimSpan::Nodes { keys } => {
            keys.iter().map(|k| Value::from(k.as_str())).collect()
        }
        _ => Vec::new(),
    };
    Value::obj([
        ("id", Value::from(c.id.to_string())),
        ("pid", Value::from(c.pid.as_str())),
        ("name", Value::from(c.name.as_str())),
        ("nodes", Value::List(keys)),
        ("expires_ms", Value::from(c.expires_ms)),
        ("remaining_ms", Value::from(c.remaining_ms)),
        ("note", Value::from(c.note.as_deref())),
    ])
}

fn claim_out(cs: &[ClaimInfo], json: bool, what: &str, version: VersionId) -> ToolOutput {
    let stdout = if json {
        Value::List(cs.iter().map(claim_value).collect()).to_json() + "\n"
    } else {
        cs.iter()
            .map(|c| format!("{what} {} ({}) for {} ms\n", c.span, c.id, c.remaining_ms))
            .collect()
    };
    let summary = cs
        .iter()
        .map(|c| format!("{what} {}", c.id))
        .collect::<Vec<_>>()
        .join(", ");
    tool_output(stdout.into_bytes(), summary, exit::OK, version)
}
