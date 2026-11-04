//! `explain` and `capabilities`, and `docs/ERRORS.md`: all generated from the registries
//! (the error registry, the verb registry, the person actions, [`Config`]).

use super::{entries, lookup, Area, ErrorEntry, ErrorReport, Status, Value, ENGINE_PREFIX, KIT};
use crate::parity::{PersonAction, Registry, Source, Tier, VerbKind, VerbSpec, KIT_VERBS};
use crate::session::Config;
use crate::tools::{exit, ToolError};

/// The verbs that need no doc: `(name, usage)`.
pub const META_VERBS: &[(&str, &str)] = &[
    (
        "explain",
        "explain [ID] [--json] (an error id: what happened, what to do next, the rule; no id: \
         every id)",
    ),
    (
        "capabilities",
        "capabilities [--json] (verbs, flags, parity rows, the error ids each verb can return, \
         limits, the kit version)",
    ),
];

/// The verbs that change the doc (the `@change` group): every kit edit verb that makes a
/// change, plus a pack's own verbs.
const CHANGE_VERBS: &[&str] = &[
    "insert",
    "replace",
    "delete",
    "sed -i",
    "patch",
    "apply",
    "append",
    "write",
    "undo",
    "redo",
    "undo --partial",
    "revert",
    "restore",
];

/// What [`run_meta`] produced: `stdout`, or a refusal (stdout empty) with its exit.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MetaOutput {
    pub stdout: Vec<u8>,
    pub exit: u8,
    pub refusal: Option<ErrorReport>,
}

impl MetaOutput {
    fn ok(s: String) -> Self {
        MetaOutput {
            stdout: s.into_bytes(),
            exit: exit::OK,
            refusal: None,
        }
    }
    fn refused(r: ErrorReport) -> Self {
        MetaOutput {
            stdout: Vec::new(),
            exit: r.exit(),
            refusal: Some(r),
        }
    }
}

/// Run a verb that needs no doc ([`META_VERBS`]): `explain [ID] [--json]` or
/// `capabilities [--json]`. `json`: the host's own `--json` was given (the verbs also take
/// it). `registries`: the verbs this host serves (e.g. `Registry::for_pack(&pack)` and
/// `Registry::for_tier(Tier::Blob)`; empty: the kit's, both tiers). `None` when `argv[0]`
/// isn't a meta verb.
pub fn run_meta(
    argv: &[String],
    json: bool,
    registries: &[&Registry],
    config: &Config,
) -> Option<MetaOutput> {
    let (verb, args) = argv.split_first()?;
    let verb = verb.as_str();
    if !META_VERBS.iter().any(|(v, _)| *v == verb) {
        return None;
    }
    let mut json = json;
    let mut operands = Vec::new();
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "--" => {}
            f if f.starts_with('-') && f.len() > 1 => {
                return Some(MetaOutput::refused(
                    ToolError::unsupported(f, format!("{verb} takes --json only")).report(),
                ))
            }
            o => operands.push(o),
        }
    }
    Some(match (verb, operands.as_slice()) {
        ("explain", []) => MetaOutput::ok(if json {
            let v = Value::List(
                entries()
                    .map(|e| entry_value(e, &Verbs::new(registries)))
                    .collect(),
            );
            v.to_json_pretty() + "\n"
        } else {
            index()
        }),
        ("explain", [id]) => match lookup(id) {
            Some(e) => MetaOutput::ok(if json {
                explain_json(e) + "\n"
            } else {
                explain(e)
            }),
            None => MetaOutput::refused(
                ErrorReport::new(&super::ids::UNKNOWN_ID, format!("no error id {id}"))
                    .with("id", *id),
            ),
        },
        ("explain", _) => MetaOutput::refused(
            ToolError::usage("explain takes one id (or none, to list them)").report(),
        ),
        ("capabilities", []) => {
            let v = capabilities(registries, config);
            MetaOutput::ok(if json {
                v.to_json_pretty() + "\n"
            } else {
                capabilities_text(&v)
            })
        }
        (_, _) => MetaOutput::refused(
            ToolError::unsupported(operands[0], format!("{verb} takes no operands")).report(),
        ),
    })
}

/// The verbs groups expand to: the host's registries, or the kit's (both tiers).
struct Verbs {
    all: Vec<VerbSpec>,
}

impl Verbs {
    fn new(registries: &[&Registry]) -> Self {
        let mut all: Vec<VerbSpec> = Vec::new();
        let kit = [
            Registry::for_tier(Tier::Text),
            Registry::for_tier(Tier::Blob),
        ];
        let regs: Vec<&Registry> = if registries.is_empty() {
            kit.iter().collect()
        } else {
            registries.to_vec()
        };
        for r in regs {
            for v in r.verbs() {
                // a file verb may share a kit verb's name: it is listed apart
                let file = v.source == Source::File;
                if !all
                    .iter()
                    .any(|a| a.name == v.name && (a.source == Source::File) == file)
                {
                    all.push(*v);
                }
            }
        }
        Verbs { all }
    }

    /// Whether `entry` can come from verb `v`. A row marked `@file` names file verbs
    /// ([`Source::File`]) and only those; any other row names kit and pack verbs, and its
    /// groups but `*` hold no file verb.
    fn returns(&self, entry: &ErrorEntry, v: &VerbSpec) -> bool {
        let file_row = entry.verbs.contains(&"@file");
        let file_verb = v.source == Source::File;
        entry.verbs.iter().any(|g| match *g {
            "*" => true,
            "@file" => false,
            "@change" => !file_verb && (v.source == Source::Pack || CHANGE_VERBS.contains(&v.name)),
            "@history" => !file_verb && v.kind == VerbKind::History,
            "@run_tool" => v.runs_through_run_tool(),
            "@pack" => v.source == Source::Pack,
            name => name == v.name && file_row == file_verb,
        })
    }
}

/// An entry's verbs as words: groups spelled out.
fn verbs_text(e: &ErrorEntry) -> String {
    if e.verbs.first() == Some(&"@file") {
        let names: Vec<&str> = e.verbs[1..]
            .iter()
            .map(|v| match *v {
                "(host)" => "a host call (no verb)",
                v => v,
            })
            .collect();
        return format!(
            "a file type's own verbs (its host's file registry): {}",
            names.join(", ")
        );
    }
    e.verbs
        .iter()
        .map(|g| match *g {
            "*" => "any verb on a doc".to_owned(),
            "@change" => format!("{} (and a pack's own verbs)", CHANGE_VERBS.join(", ")),
            "@history" => KIT_VERBS
                .iter()
                .filter(|v| v.kind == VerbKind::History)
                .map(|v| v.name)
                .collect::<Vec<_>>()
                .join(", "),
            "@run_tool" => format!(
                "{} (and a pack's own verbs)",
                crate::parity::RUN_TOOL_VERBS.join(", ")
            ),
            "@pack" => "a pack's own verbs".to_owned(),
            "(sync)" => "an editor's sync connection (no verb)".to_owned(),
            "(host)" => "a host call (no verb)".to_owned(),
            v => v.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn status_text(s: Status) -> String {
    match s {
        Status::Live => "live".to_owned(),
        Status::Reserved(by) => format!("reserved for {by}; nothing returns it yet"),
        Status::Retired(v) => format!("retired in {v}; never reused"),
    }
}

/// An entry's [`super::NextStep`] as `explain`'s `step:` line, `verb; args: a b; why: ...`
/// (no host syntax; `args` left out when there are none), or nothing when it has none.
fn step_line(e: &ErrorEntry) -> String {
    let Some(st) = e.step else {
        return String::new();
    };
    let mut o = format!("step:     {}", st.verb);
    if !st.args.is_empty() {
        o.push_str("; args: ");
        o.push_str(&st.args.join(" "));
    }
    o.push_str("; why: ");
    o.push_str(st.why);
    o.push('\n');
    o
}

/// An entry's [`super::NextStep`] as a `docs/ERRORS.md` line, the verb and each argument in
/// backticks, then why; or nothing when it has none.
fn step_markdown(e: &ErrorEntry) -> String {
    let Some(st) = e.step else {
        return String::new();
    };
    let run = if st.args.is_empty() {
        format!("`{}`", st.verb)
    } else {
        format!(
            "`{}` with {}",
            st.verb,
            st.args
                .iter()
                .map(|a| format!("`{a}`"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    format!("- **Next step:** {run}. {}\n", st.why)
}

/// `explain <id>`: the entry as text (a golden test per id holds it).
pub fn explain(e: &ErrorEntry) -> String {
    format!(
        "{id} {name} ({area})\n\
         exit:     {exit}\n\
         message:  {message}\n\
         next:     {next}\n\
         {step}\
         rule:     {rule}\n\
         verbs:    {verbs}\n\
         status:   {status}\n",
        id = e.id,
        name = e.name,
        area = e.area.title(),
        exit = e.exit,
        message = e.message,
        next = e.next,
        step = step_line(e),
        rule = e.rule,
        verbs = verbs_text(e),
        status = status_text(e.status),
    )
}

fn entry_value(e: &ErrorEntry, verbs: &Verbs) -> Value {
    let returned_by: Vec<&str> = verbs
        .all
        .iter()
        .filter(|v| verbs.returns(e, v))
        .map(|v| v.name)
        .collect();
    let mut o = vec![
        ("id", Value::from(e.id)),
        ("name", Value::from(e.name)),
        ("area", Value::from(e.area.title())),
        ("exit", Value::from(e.exit)),
        ("message", Value::from(e.message)),
        ("next", Value::from(e.next)),
    ];
    if let Some(st) = e.step {
        o.push(("next_step", st.to_value()));
    }
    o.extend([
        ("rule", Value::from(e.rule)),
        ("verbs", Value::list(e.verbs.iter().copied())),
        ("returned_by", Value::list(returned_by)),
        ("status", Value::from(e.status.word())),
        (
            "since_or_until",
            match e.status {
                Status::Live => Value::Null,
                Status::Reserved(by) => Value::from(by),
                Status::Retired(v) => Value::from(v),
            },
        ),
    ]);
    Value::obj(o)
}

/// `explain <id> --json`: the entry as one JSON object (pretty, stable key order).
pub fn explain_json(e: &ErrorEntry) -> String {
    entry_value(e, &Verbs::new(&[])).to_json_pretty()
}

/// `explain` with no id: every id, one per line, by area.
fn index() -> String {
    let mut s = String::from(
        "Error ids (explain <ID> for one; --json for machine-readable). Exit codes never \
         change; an id is never reused.\n",
    );
    for a in Area::ALL {
        let mine: Vec<&ErrorEntry> = KIT.iter().copied().filter(|e| e.area == *a).collect();
        if mine.is_empty() {
            continue;
        }
        s.push_str(&format!("\n{}xx {}\n", a.digit(), a.title()));
        for e in mine {
            let tag = match e.status {
                Status::Live => "",
                Status::Reserved(_) => " [reserved]",
                Status::Retired(_) => " [retired]",
            };
            s.push_str(&format!("  {}  exit {}  {}{}\n", e.id, e.exit, e.name, tag));
        }
    }
    s.push_str(&format!(
        "\nEngine ids ({ENGINE_PREFIX}nnn): {}\n",
        if super::ENGINE.is_empty() {
            "none yet (the engine allocates its ranges)".to_owned()
        } else {
            super::ENGINE
                .iter()
                .map(|e| e.id)
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    s
}

/// The exit codes' table (the kit's, [`crate::tools::exit`]).
pub(crate) const EXIT_CODES: &[(u8, &str, &str)] = &[
    (
        exit::OK,
        "ok",
        "success (diff, grep: no difference, a match)",
    ),
    (
        exit::NO,
        "no",
        "the tool's own no: grep found nothing, diff found differences, a conflict, a patch \
         that didn't apply; nothing applied unless the message says otherwise",
    ),
    (
        exit::USAGE,
        "usage_or_held",
        "bad usage or an unsupported flag (fix the command), or held for now (MPC-E130: don't \
         retry in a loop)",
    ),
    (
        exit::REFUSED,
        "refused",
        "the caller lacks a right; don't retry",
    ),
    (
        exit::NOT_FOUND,
        "not_found",
        "no such doc, version, change or claim; or live editing is off",
    ),
    (
        exit::LEASED,
        "leased",
        "someone else holds it for now (a lease or a claim); retry after the time left",
    ),
    (
        exit::STORE,
        "store_unavailable",
        "the store failed; nothing acknowledged",
    ),
    (
        exit::LINK_LOST,
        "link_lost",
        "the link dropped mid-verb (the host's)",
    ),
    (
        exit::TOO_OLD,
        "too_old",
        "too old (revert, an expired base); never retry the same request",
    ),
];

fn tier_word(t: Tier) -> &'static str {
    match t {
        Tier::Text => "text",
        Tier::Blob => "blob",
        Tier::Grid => "grid",
    }
}

fn kind_word(k: VerbKind) -> &'static str {
    match k {
        VerbKind::Read => "read",
        VerbKind::Edit => "edit",
        VerbKind::History => "history",
    }
}

/// `capabilities`: what this build can do, as data (the text form is rendered from it).
/// `registries` as for [`run_meta`].
pub fn capabilities(registries: &[&Registry], config: &Config) -> Value {
    let verbs = Verbs::new(registries);
    // a file type's own registry alone: its verbs are its host's, so the kit's doc-level rows,
    // parity, limits and positions don't describe them and are left out
    let file_only = !registries.is_empty() && verbs.all.iter().all(|v| v.source == Source::File);
    let live: Vec<&ErrorEntry> = KIT
        .iter()
        .copied()
        .filter(|e| e.status == Status::Live)
        .collect();
    let verb_rows = verbs.all.iter().map(|v| {
        Value::obj([
            ("name", Value::from(v.name)),
            ("kind", Value::from(kind_word(v.kind))),
            ("tiers", Value::list(v.tiers.iter().map(|t| tier_word(*t)))),
            (
                "needs",
                Value::list(v.needs.iter().map(|n| super::map::need_word(*n))),
            ),
            ("usage", Value::from(v.usage)),
            ("exits", Value::list(v.exits.iter().copied())),
            (
                "serves",
                Value::list(
                    v.serves
                        .iter()
                        .map(|a| format!("P{}", a.number()))
                        .chain(v.serves_named.iter().map(|n| (*n).to_owned())),
                ),
            ),
            (
                "source",
                Value::from(match v.source {
                    Source::Kit => "kit",
                    Source::Pack => "pack",
                    Source::File => "file",
                }),
            ),
            ("run_tool", Value::from(v.runs_through_run_tool())),
            ("alias_of", Value::from(v.alias_of())),
            (
                "errors",
                Value::list(
                    live.iter()
                        .filter(|e| !e.verbs.contains(&"*") && verbs.returns(e, v))
                        .map(|e| e.id),
                ),
            ),
        ])
    });
    let parity = PersonAction::ALL.iter().map(|a| {
        Value::obj([
            ("row", Value::from(format!("P{}", a.number()))),
            ("action", Value::from(a.describe())),
            (
                "tiers",
                Value::list(a.tiers().iter().map(|t| tier_word(*t))),
            ),
            (
                "verbs",
                Value::list(
                    verbs
                        .all
                        .iter()
                        .filter(|v| v.serves.contains(a))
                        .map(|v| v.name),
                ),
            ),
        ])
    });
    let mut limits: Vec<(String, Value)> = config
        .limits()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), Value::from(v)))
        .collect();
    limits.push((
        "tool_output_max_bytes".into(),
        Value::from(crate::tools::MAX_OUTPUT_BYTES),
    ));
    let mut out = vec![
        ("kit", Value::from(env!("CARGO_PKG_NAME"))),
        ("version", Value::from(env!("CARGO_PKG_VERSION"))),
        (
            "error_ids",
            Value::obj([
                ("kit", Value::from(super::KIT_PREFIX)),
                ("engine", Value::from(ENGINE_PREFIX)),
                ("explain", Value::from("explain <ID> [--json]")),
                (
                    "json_shape",
                    Value::list([
                        "id",
                        "exit",
                        "message",
                        "next",
                        "next_step",
                        "rule",
                        "details",
                    ]),
                ),
            ]),
        ),
        (
            "exit_codes",
            Value::List(
                EXIT_CODES
                    .iter()
                    .map(|(c, n, m)| {
                        Value::obj([
                            ("code", Value::from(*c)),
                            ("name", Value::from(*n)),
                            ("meaning", Value::from(*m)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("verbs", Value::List(verb_rows.collect())),
        (
            "errors_any_verb",
            Value::list(live.iter().filter(|e| e.verbs.contains(&"*")).map(|e| e.id)),
        ),
        (
            "meta_verbs",
            Value::List(
                META_VERBS
                    .iter()
                    .map(|(n, u)| {
                        Value::obj([("name", Value::from(*n)), ("usage", Value::from(*u))])
                    })
                    .collect(),
            ),
        ),
        ("parity", Value::List(parity.collect())),
        ("limits", Value::Obj(limits)),
        ("positions", positions(&verbs)),
        (
            "errors",
            Value::List(
                entries()
                    .map(|e| {
                        Value::obj([
                            ("id", Value::from(e.id)),
                            ("name", Value::from(e.name)),
                            ("exit", Value::from(e.exit)),
                            ("status", Value::from(e.status.word())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ];
    if file_only {
        out.retain(|(k, _)| !FILE_ONLY_LEFT_OUT.contains(k));
    }
    Value::obj(out)
}

/// The sections a file type's own registry alone leaves out of `capabilities` (the kit's
/// doc-level rows, parity, limits and positions): they describe the kit's verbs, not its.
const FILE_ONLY_LEFT_OUT: [&str; 4] = ["errors_any_verb", "parity", "limits", "positions"];

/// `capabilities`' `positions` (K-ADDR-4): each surface's unit, every unit, and the
/// position forms of each verb served, from [`crate::parity::positions`]'s one table.
fn positions(verbs: &Verbs) -> Value {
    use crate::parity::positions as p;
    let units = p::UNITS.iter().map(|u| {
        Value::obj([
            ("unit", Value::from(u.word())),
            ("base", u.base().map_or(Value::Null, Value::from)),
            ("meaning", Value::from(u.meaning())),
        ])
    });
    let rows = verbs.all.iter().filter_map(|v| {
        // a file verb's positions are its host's: the kit states none
        if v.source == Source::File {
            return None;
        }
        let forms = match p::stated(v) {
            p::Stated::NoRow => return None,
            p::Stated::Undeclared => {
                return Some(Value::obj([
                    ("verb", Value::from(v.name)),
                    ("undeclared", Value::from(true)),
                    ("forms", Value::List(Vec::new())),
                ]))
            }
            p::Stated::Forms(forms) => forms,
        };
        Some(Value::obj([
            ("verb", Value::from(v.name)),
            (
                "forms",
                Value::List(
                    forms
                        .iter()
                        .map(|f| {
                            Value::obj([
                                ("at", Value::from(f.at)),
                                ("unit", Value::from(f.unit.word())),
                                ("tier", f.tier.map_or(Value::Null, |t| tier_word(t).into())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]))
    });
    Value::obj([
        ("internal", Value::from(p::INTERNAL.word())),
        ("line_col", Value::from(p::LINE_COL)),
        ("wire", Value::from(p::WIRE.word())),
        ("yjs_wire", Value::from(p::YJS_WIRE.word())),
        ("doc", Value::from("docs/POSITIONS.md")),
        ("units", Value::List(units.collect())),
        ("verbs", Value::List(rows.collect())),
    ])
}

fn strs(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::List(l)) => l
            .iter()
            .map(|x| match x {
                Value::Str(s) => s.clone(),
                other => other.to_json(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The text form of [`capabilities`].
fn capabilities_text(c: &Value) -> String {
    let mut o = format!(
        "{} {}\nError ids: MPC-Ennn (the kit), MPCE-Ennn (the engine). `explain <ID>` says what \
         one means and what to do next; `--json` makes any output machine-readable.\n",
        s(c, "kit"),
        s(c, "version")
    );
    o.push_str("\nExit codes (never change):\n");
    if let Some(Value::List(l)) = c.get("exit_codes") {
        for e in l {
            o.push_str(&format!(
                "  {}  {:<17} {}\n",
                e.get("code").and_then(Value::as_u64).unwrap_or(0),
                s(e, "name"),
                s(e, "meaning")
            ));
        }
    }
    // a file type's own registry alone ([`FILE_ONLY_LEFT_OUT`]): every verb is the file's
    let file_only = match c.get("verbs") {
        Some(Value::List(l)) => !l.is_empty() && l.iter().all(|v| s(v, "source") == "file"),
        _ => false,
    };
    o.push_str(if file_only {
        "\nVerbs (the file type's own, as its host serves them):\n"
    } else {
        "\nVerbs (after the doc path):\n"
    });
    if let Some(Value::List(l)) = c.get("verbs") {
        for v in l {
            o.push_str(&format!(
                "  {}  [{}; {}; needs {}]\n    usage:  {}\n    exits:  {}\n    serves: {}\n    errors: {}\n",
                s(v, "name"),
                s(v, "kind"),
                // only a file verb works on no kit tier
                match strs(v.get("tiers")).join("+") {
                    t if t.is_empty() => "file".to_owned(),
                    t => t,
                },
                strs(v.get("needs")).join("+"),
                s(v, "usage"),
                strs(v.get("exits")).join(" "),
                strs(v.get("serves")).join(" "),
                {
                    let e = strs(v.get("errors"));
                    if e.is_empty() && file_only {
                        "(none listed)".to_owned()
                    } else if e.is_empty() {
                        "(only the ones any verb can return)".to_owned()
                    } else {
                        e.join(" ")
                    }
                },
            ));
        }
    }
    if c.get("errors_any_verb").is_some() {
        o.push_str(&format!(
            "  (any verb on a doc can also return: {})\n",
            strs(c.get("errors_any_verb")).join(" ")
        ));
    }
    o.push_str("\nVerbs with no doc:\n");
    if let Some(Value::List(l)) = c.get("meta_verbs") {
        for v in l {
            o.push_str(&format!("  {}\n", s(v, "usage")));
        }
    }
    if let Some(Value::List(l)) = c.get("parity") {
        o.push_str("\nParity (each thing a person can do, and the verbs that do it):\n");
        for p in l {
            let vs = strs(p.get("verbs"));
            o.push_str(&format!(
                "  {:<4} {} [{}]: {}\n",
                s(p, "row"),
                s(p, "action"),
                strs(p.get("tiers")).join("+"),
                if vs.is_empty() {
                    "(no verb yet)".to_owned()
                } else {
                    vs.join(", ")
                }
            ));
        }
    }
    if let Some(Value::Obj(m)) = c.get("limits") {
        o.push_str("\nLimits in effect:\n");
        for (k, v) in m {
            o.push_str(&format!("  {k} = {}\n", v.to_json()));
        }
    }
    if let Some(p) = c.get("positions") {
        o.push_str(&format!(
            "\nPositions ({}):\n  inside the kit: {}\n  line:col: {}\n  the hunk channel \
             (frames, carets): {}\n  sync updates with Yjs clients: {}\n  units:\n",
            s(p, "doc"),
            s(p, "internal"),
            s(p, "line_col"),
            s(p, "wire"),
            s(p, "yjs_wire")
        ));
        if let Some(Value::List(l)) = p.get("units") {
            for u in l {
                let base = match u.get("base").and_then(Value::as_u64) {
                    Some(b) => format!("{b}-based"),
                    None => "no base".to_owned(),
                };
                o.push_str(&format!(
                    "    {:<16} {base}: {}\n",
                    s(u, "unit"),
                    s(u, "meaning")
                ));
            }
        }
        o.push_str("  verbs:\n");
        if let Some(Value::List(l)) = p.get("verbs") {
            for v in l {
                let forms: Vec<String> = match v.get("forms") {
                    Some(Value::List(fs)) => fs
                        .iter()
                        .map(|f| match f.get("tier").and_then(Value::as_str) {
                            Some(t) => format!("{} {} ({t})", s(f, "at"), s(f, "unit")),
                            None => format!("{} {}", s(f, "at"), s(f, "unit")),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let stated = if v.get("undeclared").is_some() {
                    "undeclared (the pack states no position units)".to_owned()
                } else if forms.is_empty() {
                    "takes and prints no positions".to_owned()
                } else {
                    forms.join(", ")
                };
                o.push_str(&format!("    {}: {stated}\n", s(v, "verb")));
            }
        }
    }
    o.push_str("\nError ids: run explain for the list, explain <ID> for one.\n");
    o
}

/// An area's note in `docs/ERRORS.md`, a line under its heading, when its title doesn't say
/// all it holds.
fn area_note(a: Area) -> Option<&'static str> {
    match a {
        Area::Structured => Some(
            "It covers structured file types too, a board's own verbs included (MPC-E830 to \
             E859; E860 to E869 are kept for the board's live verbs).",
        ),
        _ => None,
    }
}

/// `docs/ERRORS.md`, generated (a test holds the committed file to this).
pub fn errors_markdown() -> String {
    let mut o = String::from(
        "# Errors\n\n\
         <!-- Generated from crates/base/src/errors/registry.rs by `errors_markdown()`; \
         don't edit by hand. Regenerate: MPC_BLESS=1 cargo test -p k2-mpc-base --test \
         error_ids. -->\n\n\
         Every refusal the kit returns has a stable id, `MPC-Ennn` (the engine's are \
         `MPCE-Ennn`). An id is never reused and never changes its exit code; a retired id \
         stays listed here. Each entry names the rule it enforces: requirement ids (`K-…`, \
         the requirements PRD), undo spec rules (`U…`, `docs/SPEC-UNDO.md`), conformance \
         rules (`hist.…`, `docs/CONFORMANCE.md`), amendments (`A13`: `docs/SESSIONS.md`).\n\n\
         A host prints a refusal as **plain text**, the error's own wording with the id \
         appended (`doc changed since 12 (now 14) (MPC-E110)`), or with `--json` as one \
         object:\n\n\
         ```json\n\
         {\"id\": \"MPC-E110\", \"exit\": 1, \"message\": \"doc changed since 12 (now 14)\",\n \
         \"next\": \"Someone changed the doc after …\",\n \
         \"next_step\": {\"verb\": \"cat\", \"args\": [], \"why\": \"Someone changed the doc …\"},\n \
         \"rule\": \"A14 (--if-version), K-T0-5\", \"details\": {\"expected\": 12, \"actual\": 14}}\n\
         ```\n\n\
         `details` is structured (numbers, values, lists, objects), never prose. `next_step` \
         is `next` as data: `verb` a kit verb, `args` its arguments, one token each (`{key}`: \
         the refusal's `details.key`), and `why` in prose; it has no host syntax, so a host \
         renders it in its own (K-PAR-10). It is left out when there is nothing to run (wait, \
         ask a person, report it, use ordinary file tools). `explain \
         <ID>` prints an entry; `capabilities` lists the ids each verb can return. Exit \
         codes:\n\n\
         | Exit | Name | Meaning |\n|---|---|---|\n",
    );
    for (c, n, m) in EXIT_CODES {
        o.push_str(&format!("| {c} | `{n}` | {m} |\n"));
    }
    o.push_str(
        "\n**Verb groups** in the tables: `*` any verb on a doc; `@change` every verb that \
         changes the doc; `@history` every history verb; `@run_tool` every verb `run_tool` \
         runs; `@pack` a pack's own verbs; `(sync)` an editor's sync connection; `(host)` a \
         host call with no verb.\n\n\
         **File verbs**: a row whose verbs start with `@file` is a refusal of a file type's own \
         verbs, which its host runs on the file and lists in `capabilities` from a file \
         registry (`Registry::for_file`); the names after `@file` are those verbs (`(host)`: a \
         host call on the file, no verb). The host maps \
         the file type's refusals to these rows; a format refusal's exact code is in \
         `details.code`.\n",
    );
    for a in Area::ALL {
        let mine: Vec<&ErrorEntry> = KIT.iter().copied().filter(|e| e.area == *a).collect();
        if mine.is_empty() {
            continue;
        }
        o.push_str(&format!(
            "\n## {}xx: {}\n{}\n| Id | Name | Exit | Rule | Verbs | Status |\n|---|---|---|---|---|---|\n",
            a.digit(),
            a.title(),
            area_note(*a).map_or(String::new(), |n| format!("{n}\n"))
        ));
        for e in &mine {
            o.push_str(&format!(
                "| `{}` | `{}` | {} | {} | {} | {} |\n",
                e.id,
                e.name,
                e.exit,
                e.rule,
                e.verbs
                    .iter()
                    .map(|v| format!("`{v}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                e.status.word()
            ));
        }
        for e in &mine {
            o.push_str(&format!(
                "\n### {} `{}`\n\n- **Message:** `{}`\n- **Next:** {}\n{}- **Rule:** {}\n- **Exit:** {}\n",
                e.id,
                e.name,
                e.message,
                e.next,
                step_markdown(e),
                e.rule,
                e.exit
            ));
            if e.status != Status::Live {
                o.push_str(&format!("- **Status:** {}\n", status_text(e.status)));
            }
            if let super::Reach::Indirect(why) = e.reach {
                if e.status == Status::Live {
                    o.push_str(&format!("- **Reached by:** {why}\n"));
                }
            }
        }
    }
    o.push_str(&format!(
        "\n## Engine ids ({ENGINE_PREFIX}nnn)\n\nReserved for the engine, which allocates its \
         ranges (decode, admission and security, schema, history, store) at G2-core. Its \
         codes cross the seam as these strings, never as enum ordinals; a code this build \
         doesn't list is reported as `MPC-E161` with `details.code`.\n\n{}\n",
        if super::ENGINE.is_empty() {
            "None yet.".to_owned()
        } else {
            super::ENGINE
                .iter()
                .map(|e| format!("- `{}` `{}`", e.id, e.name))
                .collect::<Vec<_>>()
                .join("\n")
        }
    ));
    o
}
