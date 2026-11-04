//! The argument parser: a host's argv for a board file verb, to a call. A call is also made in code,
//! from a board [`Verb`] or a [`ReadVerb`], by a host that builds the verb itself; the parser
//! builds through the same path.
//!
//! **The data is the board crate's.** Verb names and argument names come from [`verbs::ARGS`],
//! the arguments whose value is JSON by design from [`verbs::JSON_ARGS`], and `--<field> <text>`
//! goes through [`verbs::flags`] (or [`verbs::settings_flags`] for `meta`), where the text `null`
//! removes a field. The bridge adds only the text forms of compound values (`README.md`). A host's
//! own spellings come in as [`Names`]: the bridge holds none.
//!
//! **The command line.** The verb comes first. A required argument is positional, in the order
//! [`verbs::ARGS`] gives (or the host's, from [`Names::positional`]), or `--<arg> <value>`, or
//! `--<arg>=<value>`; an optional one is `--<arg>` only. After `--`, every token is positional.
//! A flag always takes a value: `--cascade true`, never a bare `--cascade`, unless the host
//! named the flag one that changes nothing ([`Names::ignore`]). A verb that takes fields takes
//! any other `--<name> <text>` as a field.
//!
//! **Refusals.** An unknown verb or flag is the kit's MPC-E120 (`tool_unsupported`); a missing,
//! extra or bad argument is MPC-E122 (`tool_usage`); both exit 2. A field's value is the board
//! crate's to judge, so a bad one is that refusal's row at [`Phase::Input`], and `apply`'s ops
//! are judged at [`Phase::Ops`].

use crate::answer::Why;
use crate::bug::HostBug;
use crate::report::{no_object_report, refusal_report, verb_error_report};
use crate::rows::{row, Phase};
use crate::value::{read_value, value_out};
use k2_mpc_base::errors::{ErrorReport, Value as KValue};
use k2_mpc_board::json::{self, Fields, Value as BValue};
use k2_mpc_board::verbs::{
    self, AlignTo, Axis, Ctx, End, FieldEdits, Layer, ObjRev, ObjectView, Planned, Removed, Side,
    Verb, VerbError,
};
use k2_mpc_board::{Board, Id, Limits, Object, ObjectType, Position, Refusal, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

// ---------------------------------------------------------------------------------------------
// Forms: how each argument's value is written on a command line.

/// How an argument's value is written. The words of [`AlignTo`], [`Axis`] and [`Side`] are the
/// board crate's; the compound forms are the bridge's (`README.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    /// An object id.
    Id,
    /// Ids, comma-separated.
    Ids,
    /// `<id>@<rev>`, comma-separated.
    ObjRevs,
    /// A revision, `sha256-<hex>`.
    Rev,
    /// A blob's hash, `sha256-<hex>`.
    Hash,
    /// Text as it is.
    Text,
    /// `true` or `false`.
    Bool,
    /// A whole number.
    Int,
    /// A number, by the format's number rule.
    Num,
    /// `<x>,<y>`.
    Point,
    /// A type name.
    Type,
    /// `--<field> <text>` pairs.
    Fields,
    /// `front`, `back` or `after:<ids>`.
    Position,
    /// `front`, `back`, `forward`, `backward` or `after:<ids>`.
    Layer,
    /// `<id>`, `<id>:<anchor>` or `<x>,<y>`.
    End,
    /// An [`AlignTo`] word.
    Align,
    /// An [`Axis`] word.
    Axis,
    /// A [`Side`] word.
    Side,
    /// JSON, by design ([`verbs::JSON_ARGS`]).
    Json,
}

/// Every verb's arguments with their forms, in [`verbs::ARGS`]'s order. A test holds it to
/// [`verbs::ARGS`] and [`verbs::JSON_ARGS`].
/// One verb's arguments, each with its form.
type VerbForms = (&'static str, &'static [(&'static str, Form)]);

const FORMS: &[VerbForms] = {
    use Form::*;
    &[
        (
            "add",
            &[("kind", Type), ("fields", Fields), ("at", Position)],
        ),
        (
            "update",
            &[("id", Id), ("rev", Rev), ("set", Fields), ("unset", Fields)],
        ),
        ("delete", &[("id", Id), ("rev", Rev), ("cascade", Bool)]),
        ("put-back", &[("removed", Json)]),
        ("connect", &[("from", End), ("to", End), ("fields", Fields)]),
        ("react", &[("id", Id), ("emoji", Text), ("on", Bool)]),
        ("vote", &[("id", Id), ("delta", Int)]),
        (
            "comment",
            &[("target", Id), ("text", Text), ("point", Point)],
        ),
        ("reply", &[("id", Id), ("text", Text)]),
        ("resolve", &[("id", Id), ("rev", Rev), ("resolved", Bool)]),
        ("meta", &[("rev", Rev), ("set", Fields), ("unset", Fields)]),
        ("lock", &[("ids", ObjRevs), ("locked", Bool)]),
        ("check-add", &[("id", Id), ("text", Text)]),
        (
            "check-set",
            &[
                ("id", Id),
                ("rev", Rev),
                ("item", Text),
                ("text", Text),
                ("done", Bool),
            ],
        ),
        ("check-remove", &[("id", Id), ("rev", Rev), ("item", Text)]),
        (
            "table-cell",
            &[
                ("id", Id),
                ("rev", Rev),
                ("row", Text),
                ("column", Text),
                ("text", Text),
            ],
        ),
        ("table-add-row", &[("id", Id), ("after", Text)]),
        (
            "table-remove-row",
            &[("id", Id), ("rev", Rev), ("row", Text)],
        ),
        (
            "table-add-column",
            &[("id", Id), ("after", Text), ("width", Num)],
        ),
        (
            "table-remove-column",
            &[("id", Id), ("rev", Rev), ("column", Text)],
        ),
        ("move", &[("ids", ObjRevs), ("dx", Num), ("dy", Num)]),
        ("order", &[("ids", Ids), ("to", Layer)]),
        ("frame-order", &[("id", Id), ("to", Position)]),
        ("group", &[("ids", ObjRevs)]),
        ("ungroup", &[("ids", ObjRevs)]),
        ("align", &[("ids", ObjRevs), ("to", Align)]),
        ("distribute", &[("ids", ObjRevs), ("axis", Axis)]),
        ("tidy", &[("ids", ObjRevs)]),
        ("copy-style", &[("from", Id), ("to", ObjRevs)]),
        ("duplicate", &[("ids", Ids), ("dx", Num), ("dy", Num)]),
        ("paste", &[("objects", Json), ("x", Num), ("y", Num)]),
        ("connected-copy", &[("id", Id), ("side", Side)]),
        ("mind-child", &[("parent", Id), ("text", Text)]),
        ("template", &[("name", Text), ("x", Num), ("y", Num)]),
        ("clear-blob", &[("hash", Hash)]),
        ("list", &[("kind", Type)]),
        ("read", &[("id", Id)]),
        ("search", &[("text", Text)]),
        ("fields", &[("type", Text)]),
        ("templates", &[]),
        ("references", &[("id", Id)]),
        ("apply", &[("ops", Json)]),
        (
            "inverse",
            &[("before", Json), ("after", Json), ("group_id", Text)],
        ),
    ]
};

/// One argument of a verb: its name, whether it may be left out, and its form.
#[derive(Clone, Copy, Debug)]
struct Arg {
    name: &'static str,
    optional: bool,
    form: Form,
}

/// A verb's arguments, from [`verbs::ARGS`] with their forms; `None` for a verb it doesn't list.
fn args_of(verb: &str) -> Option<(&'static str, Vec<Arg>)> {
    let (name, listed) = verbs::ARGS.iter().find(|(n, _)| *n == verb)?;
    let (_, forms) = FORMS.iter().find(|(n, _)| n == name)?;
    let mut out = Vec::with_capacity(listed.len());
    for a in listed.iter() {
        let (bare, optional) = match a.strip_suffix('?') {
            Some(b) => (b, true),
            None => (*a, false),
        };
        let (_, form) = forms.iter().find(|(n, _)| *n == bare)?;
        out.push(Arg {
            name: bare,
            optional,
            form: *form,
        });
    }
    Some((*name, out))
}

fn takes_fields(args: &[Arg]) -> bool {
    args.iter().any(|a| a.form == Form::Fields)
}

// ---------------------------------------------------------------------------------------------
// Names: a host's spellings, as data.

/// Where a host's flag goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum To {
    /// An argument of the verb.
    Arg(&'static str),
    /// The anchor of an end the call names by object.
    Anchor(&'static str),
}

/// One command of a host's: the board verb it runs, and how its words map to the verb's.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HostVerb {
    verb: &'static str,
    positional: Option<Vec<(&'static str, bool)>>,
    flags: BTreeMap<String, To>,
    presets: BTreeMap<&'static str, String>,
    ignored: BTreeSet<String>,
}

impl HostVerb {
    fn of(verb: &'static str) -> HostVerb {
        HostVerb {
            verb,
            positional: None,
            flags: BTreeMap::new(),
            presets: BTreeMap::new(),
            ignored: BTreeSet::new(),
        }
    }
}

/// A host's own spellings for the board's verbs and arguments, as data: renames and aliases, the
/// order of its positional arguments, values it sets itself, and flags it takes that change
/// nothing on a file. The bridge holds none of its own: with `Names::default()`, the board's own
/// names are the only ones. The board's names always work too.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Names {
    verbs: BTreeMap<String, HostVerb>,
    aliases: BTreeMap<String, String>,
}

/// A host's [`Names`] that don't hold together: the host's bug, said in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadNames {
    what: String,
}

impl BadNames {
    fn new(what: impl Into<String>) -> BadNames {
        BadNames { what: what.into() }
    }

    /// What doesn't hold.
    pub fn what(&self) -> &str {
        &self.what
    }
}

impl fmt::Display for BadNames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the host's names don't hold: {}", self.what)
    }
}

impl std::error::Error for BadNames {}

impl Names {
    /// The host's command `host` runs the board verb `verb`.
    pub fn verb(mut self, host: &str, verb: &str) -> Result<Names, BadNames> {
        let Some((verb, _)) = args_of(verb) else {
            return Err(BadNames::new(format!(
                "{host}: the board has no verb {verb:?}"
            )));
        };
        if self.verbs.contains_key(host) || self.aliases.contains_key(host) {
            return Err(BadNames::new(format!("{host}: named twice")));
        }
        if host != verb && args_of(host).is_some() {
            return Err(BadNames::new(format!(
                "{host}: the board's own verb, so it can't run {verb}"
            )));
        }
        self.verbs.insert(host.to_string(), HostVerb::of(verb));
        Ok(self)
    }

    /// `alias` is another name for the host's command `of`, or for a board verb.
    pub fn alias(mut self, alias: &str, of: &str) -> Result<Names, BadNames> {
        if self.verbs.contains_key(alias)
            || self.aliases.contains_key(alias)
            || args_of(alias).is_some()
        {
            return Err(BadNames::new(format!(
                "{alias}: an alias, and named already"
            )));
        }
        if !self.verbs.contains_key(of) && args_of(of).is_none() {
            return Err(BadNames::new(format!(
                "{alias}: an alias of {of:?}, which runs no verb"
            )));
        }
        self.aliases.insert(alias.to_string(), of.to_string());
        Ok(self)
    }

    /// The host's command, made if it's a board verb the host keeps under its own name.
    fn command(&mut self, host: &str) -> Result<&mut HostVerb, BadNames> {
        if !self.verbs.contains_key(host) {
            let Some((verb, _)) = args_of(host) else {
                return Err(BadNames::new(format!(
                    "{host}: no command of the host's or verb"
                )));
            };
            self.verbs.insert(host.to_string(), HostVerb::of(verb));
        }
        self.verbs
            .get_mut(host)
            .ok_or_else(|| BadNames::new(format!("{host}: no command")))
    }

    fn arg_of(verb: &str, arg: &str) -> Result<Arg, BadNames> {
        let (_, args) = args_of(verb).ok_or_else(|| BadNames::new(format!("no verb {verb}")))?;
        args.into_iter()
            .find(|a| a.name == arg)
            .ok_or_else(|| BadNames::new(format!("{verb} has no argument {arg:?}")))
    }

    /// On the host's command `host`, the host's word `name` is the verb's argument `arg`: as a flag,
    /// `--<name>`, and in [`Names::positional`].
    pub fn arg(mut self, host: &str, name: &str, arg: &str) -> Result<Names, BadNames> {
        let cmd = self.command(host)?;
        let a = Names::arg_of(cmd.verb, arg)?;
        if a.form == Form::Fields {
            return Err(BadNames::new(format!(
                "{host}: --{name} can't stand for the fields"
            )));
        }
        if cmd
            .flags
            .insert(name.to_string(), To::Arg(a.name))
            .is_some()
        {
            return Err(BadNames::new(format!("{host}: --{name} named twice")));
        }
        Ok(self)
    }

    /// On the host's command `host`, the flag `--<name>` fills the anchor of the end `end`, which
    /// the call names by object.
    pub fn anchor(mut self, host: &str, name: &str, end: &str) -> Result<Names, BadNames> {
        let cmd = self.command(host)?;
        let a = Names::arg_of(cmd.verb, end)?;
        if a.form != Form::End {
            return Err(BadNames::new(format!(
                "{host}: {end} isn't an end, so it has no anchor"
            )));
        }
        if cmd
            .flags
            .insert(name.to_string(), To::Anchor(a.name))
            .is_some()
        {
            return Err(BadNames::new(format!("{host}: --{name} named twice")));
        }
        Ok(self)
    }

    /// The host's command `host` takes these positional arguments, in this order: the host's
    /// words or the verb's own names, each ending in `?` if it may be left out.
    pub fn positional(mut self, host: &str, names: &[&str]) -> Result<Names, BadNames> {
        let cmd = self.command(host)?;
        let mut order = Vec::with_capacity(names.len());
        for n in names {
            let (bare, optional) = match n.strip_suffix('?') {
                Some(b) => (b, true),
                None => (*n, false),
            };
            let arg = match cmd.flags.get(bare) {
                Some(To::Arg(a)) => *a,
                Some(To::Anchor(_)) => {
                    return Err(BadNames::new(format!(
                        "{host}: {bare} is an anchor, not positional"
                    )))
                }
                None => Names::arg_of(cmd.verb, bare)?.name,
            };
            if Names::arg_of(cmd.verb, arg)?.form == Form::Fields {
                return Err(BadNames::new(format!(
                    "{host}: the fields aren't positional"
                )));
            }
            if order.iter().any(|(a, _)| *a == arg) {
                return Err(BadNames::new(format!("{host}: {arg} is positional twice")));
            }
            order.push((arg, optional));
        }
        cmd.positional = Some(order);
        Ok(self)
    }

    /// The host's command `host` sets the verb's argument `arg` itself, to `text`. A call that
    /// gives it another value is refused.
    pub fn preset(mut self, host: &str, arg: &str, text: &str) -> Result<Names, BadNames> {
        let cmd = self.command(host)?;
        let a = Names::arg_of(cmd.verb, arg)?;
        if a.form == Form::Fields {
            return Err(BadNames::new(format!("{host}: the fields can't be preset")));
        }
        if cmd.presets.insert(a.name, text.to_string()).is_some() {
            return Err(BadNames::new(format!("{host}: {arg} preset twice")));
        }
        Ok(self)
    }

    /// The host's command `host` takes the flag `--<flag>`, bare, and it changes nothing on a file.
    pub fn ignore(mut self, host: &str, flag: &str) -> Result<Names, BadNames> {
        let cmd = self.command(host)?;
        if !cmd.ignored.insert(flag.to_string()) {
            return Err(BadNames::new(format!("{host}: --{flag} ignored twice")));
        }
        Ok(self)
    }

    /// The host's command for `word`: an alias's, the host's own, or the board verb's.
    fn find(&self, word: &str) -> Option<HostVerb> {
        let word = self.aliases.get(word).map_or(word, String::as_str);
        match self.verbs.get(word) {
            Some(cmd) => Some(cmd.clone()),
            None => args_of(word).map(|(verb, _)| HostVerb::of(verb)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Usage refusals: the kit's rows, every key their message names set.

/// The row's message with each `{key}` replaced by its detail, which is text.
fn fill(message: &str, details: &[(&'static str, KValue)]) -> Option<String> {
    let mut out = String::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}')?;
        let key = &after[..close];
        let (_, value) = details.iter().find(|(k, _)| *k == key)?;
        out.push_str(value.as_str()?);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn usage_report(id: &'static str, details: Vec<(&'static str, KValue)>) -> Why {
    let entry = match row(id) {
        Ok(e) => e,
        Err(b) => return Why::HostBug(b),
    };
    let Some(message) = fill(entry.message, &details) else {
        return Why::HostBug(HostBug::MissingFact {
            row: entry.id,
            key: entry.message.to_string(),
        });
    };
    let mut r = ErrorReport::new(entry, message);
    for (k, v) in details {
        r = r.with(k, v);
    }
    Why::Report(r)
}

fn t(s: &str) -> KValue {
    KValue::Str(s.to_string())
}

/// MPC-E120: a verb or a flag outside what the board's verbs take.
fn unsupported(verb: Option<&str>, flag: &str, hint: &str) -> Why {
    let mut d = vec![("flag", t(flag)), ("hint", t(hint))];
    if let Some(v) = verb {
        d.push(("verb", t(v)));
    }
    usage_report("MPC-E120", d)
}

/// MPC-E122: the command line is malformed. The message is the bridge's words, the verb and the
/// argument's name, never a value the caller gave: a value, when there is one, is in
/// `details.value`. A name from the command line itself goes in the message only if it's a plain
/// word.
fn usage_error(verb: &str, arg: Option<&str>, reason: &str) -> Why {
    let plain = |a: &str| {
        !a.is_empty()
            && a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    };
    let message = match arg {
        Some(a) if plain(a) => format!("{verb}: {reason}: {a}"),
        _ => format!("{verb}: {reason}"),
    };
    let mut d = vec![("reason", t(&message)), ("verb", t(verb))];
    if let Some(a) = arg {
        d.push(("arg", t(a)));
    }
    usage_report("MPC-E122", d)
}

/// MPC-E122 for a value that isn't valid: the value in `details.value`, and the board crate's
/// code in `details.code` when it judged it.
fn bad_value(verb: &str, arg: &str, value: &str, what: &str, code: Option<&str>) -> Why {
    let mut d = vec![
        ("reason", t(&format!("{verb}'s {arg} {what}"))),
        ("verb", t(verb)),
        ("arg", t(arg)),
        ("value", t(value)),
    ];
    if let Some(c) = code {
        d.push(("code", t(c)));
    }
    usage_report("MPC-E122", d)
}

/// A verb's refusal, at the caller's input.
fn at_input(e: &VerbError, verb: &str) -> Why {
    Why::from(verb_error_report(e, verb, Phase::Input))
}

/// A format refusal while reading the board for a read verb: the caller's call.
fn refused(r: &Refusal) -> Why {
    Why::from(refusal_report(r, Phase::Input))
}

// ---------------------------------------------------------------------------------------------
// The call.

/// Whether a call reads the file or changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    /// It reads.
    Read,
    /// It changes the file.
    Change,
}

/// A verb that reads the file, built in code: the read verbs, and `meta` with nothing to change.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadVerb {
    /// Every object back to front, or one type's.
    List {
        /// The type, or every type.
        kind: Option<ObjectType>,
    },
    /// One object.
    Read {
        /// The object.
        id: Id,
    },
    /// Objects whose text holds this.
    Search {
        /// The text.
        text: String,
    },
    /// The fields a type holds, or every type's.
    Fields {
        /// The type, or every type.
        type_name: Option<String>,
    },
    /// Every template.
    Templates,
    /// Every reference, or those from and to one object.
    References {
        /// The object, or every one.
        id: Option<Id>,
    },
    /// The settings, with the rev a change to them names.
    Meta,
}

impl ReadVerb {
    /// The verb's name, as the board crate's verbs give it.
    pub fn name(&self) -> &'static str {
        match self {
            ReadVerb::List { .. } => "list",
            ReadVerb::Read { .. } => "read",
            ReadVerb::Search { .. } => "search",
            ReadVerb::Fields { .. } => "fields",
            ReadVerb::Templates => "templates",
            ReadVerb::References { .. } => "references",
            ReadVerb::Meta => "meta",
        }
    }
}

/// What a call does.
#[derive(Clone, Debug, PartialEq)]
enum Body {
    /// A read verb.
    Read(ReadVerb),
    /// A board verb, whole.
    Verb(Verb),
    /// An `update` whose fields wait for the object's type, which only the board knows.
    Update {
        id: Id,
        rev: Sha256,
        pairs: Vec<(String, String)>,
        limits: Limits,
    },
    /// Raw ops, labelled: each `add`'s id is a label, and the new objects get fresh ids.
    Apply { ops: String, limits: Limits },
    /// The group that takes `after` back to `before`.
    Inverse {
        before: Box<Board>,
        after: Box<Board>,
        group_id: String,
    },
}

/// One call: a verb to read with, or to plan a change with.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    body: Body,
}

impl Call {
    /// A change call made from a board verb: the library form for a host that builds the [`Verb`]
    /// itself, with no command line. [`parse`] builds through the same path.
    pub fn from_verb(verb: Verb) -> Call {
        Call {
            body: Body::Verb(verb),
        }
    }

    /// A read call made from a read verb: the library form, as [`Call::from_verb`] is.
    pub fn from_read(read: ReadVerb) -> Call {
        Call {
            body: Body::Read(read),
        }
    }

    /// `apply`: raw ops as JSON, `{"ops": [...]}` or the bare list, labelled
    /// ([`verbs::labelled_from_json`]): each `add`'s id is a label for this call, and the new
    /// objects get fresh ids. A host relaying a canvas's own ops, with ids as given, calls
    /// [`verbs::group_from_json`] itself.
    pub fn apply(ops: impl Into<String>, limits: &Limits) -> Call {
        Call {
            body: Body::Apply {
                ops: ops.into(),
                limits: *limits,
            },
        }
    }

    /// `inverse`: the group that takes `after` back to `before` ([`verbs::inverse`]). Its ops name
    /// the revisions of `after`, so if someone has changed one since, applying it conflicts and
    /// changes nothing.
    pub fn inverse(before: Board, after: Board, group_id: impl Into<String>) -> Call {
        Call {
            body: Body::Inverse {
                before: Box::new(before),
                after: Box::new(after),
                group_id: group_id.into(),
            },
        }
    }

    /// The verb's name, as the board crate's verbs give it.
    pub fn name(&self) -> &'static str {
        match &self.body {
            Body::Read(r) => r.name(),
            Body::Verb(v) => v.name(),
            Body::Update { .. } => "update",
            Body::Apply { .. } => "apply",
            Body::Inverse { .. } => "inverse",
        }
    }

    /// Whether it reads or changes the file.
    pub fn kind(&self) -> CallKind {
        match &self.body {
            Body::Read(_) => CallKind::Read,
            Body::Verb(_) | Body::Update { .. } | Body::Apply { .. } | Body::Inverse { .. } => {
                CallKind::Change
            }
        }
    }

    /// The board verb the call plans, on `board`, which gives an `update`'s object its type:
    /// `None` for a read, `apply` and `inverse`, which plan no [`Verb`].
    pub fn verb(&self, board: &Board) -> Result<Option<Verb>, Why> {
        match &self.body {
            Body::Verb(v) => Ok(Some(v.clone())),
            Body::Update {
                id,
                rev,
                pairs,
                limits,
            } => Ok(Some(update_verb(board, id, *rev, pairs, limits)?)),
            Body::Read(_) | Body::Apply { .. } | Body::Inverse { .. } => Ok(None),
        }
    }

    /// A read verb's answer on `board`. A change call has none: asking is the host's bug.
    pub fn read(&self, board: &Board) -> Result<KValue, Why> {
        match &self.body {
            Body::Read(r) => read_answer(r, board),
            _ => Err(Why::HostBug(HostBug::UnknownError {
                what: format!("{} changes the file: plan it, don't read it", self.name()),
            })),
        }
    }

    /// A change verb's plan on `board`. A refusal is its row: at [`Phase::Ops`] for `apply`'s ops,
    /// at [`Phase::Input`] for everything else. A read call has no plan: asking is the host's bug.
    pub fn plan(&self, board: &Board, cx: &mut Ctx<'_>) -> Result<Planned, Why> {
        match &self.body {
            Body::Verb(v) => verbs::plan(board, cx, v).map_err(|e| at_input(&e, v.name())),
            Body::Update {
                id,
                rev,
                pairs,
                limits,
            } => {
                let v = update_verb(board, id, *rev, pairs, limits)?;
                verbs::plan(board, cx, &v).map_err(|e| at_input(&e, "update"))
            }
            Body::Apply { ops, limits } => {
                verbs::labelled_from_json(board, cx, ops.as_bytes(), limits)
                    .map_err(|e| Why::from(verb_error_report(&e, "apply", Phase::Ops)))
            }
            Body::Inverse {
                before,
                after,
                group_id,
            } => {
                let group = verbs::inverse(before, after, group_id).map_err(|r| refused(&r))?;
                Ok(Planned {
                    group,
                    ids: Vec::new(),
                    labels: BTreeMap::new(),
                    removed: Vec::new(),
                    removed_locked: Vec::new(),
                    orphans: Vec::new(),
                    skipped: Vec::new(),
                })
            }
            Body::Read(_) => Err(Why::HostBug(HostBug::UnknownError {
                what: format!("{} reads the file: read it, don't plan it", self.name()),
            })),
        }
    }
}

/// An `update` with its fields read by the type of the object on `board`. An object that isn't
/// there gets no fields, and planning it gives the board crate's own refusal for that.
fn update_verb(
    board: &Board,
    id: &Id,
    rev: Sha256,
    pairs: &[(String, String)],
    limits: &Limits,
) -> Result<Verb, Why> {
    let edits = match board.objects.iter().find(|o| o.id == *id) {
        Some(o) => verbs::flags(o.kind.as_str(), &pair_refs(pairs), limits)
            .map_err(|e| at_input(&e, "update"))?,
        None => FieldEdits::default(),
    };
    Ok(Verb::Update {
        id: id.clone(),
        rev,
        set: edits.set,
        unset: edits.unset,
    })
}

fn pair_refs(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect()
}

fn views(list: &[ObjectView]) -> Result<KValue, Why> {
    let mut out = Vec::with_capacity(list.len());
    for view in list {
        out.push(read_value(view)?);
    }
    Ok(KValue::List(out))
}

fn read_answer(r: &ReadVerb, board: &Board) -> Result<KValue, Why> {
    match r {
        ReadVerb::List { kind } => {
            views(&verbs::list(board, kind.as_ref()).map_err(|e| refused(&e))?)
        }
        ReadVerb::Read { id } => match verbs::read(board, id).map_err(|e| refused(&e))? {
            Some(view) => Ok(read_value(&view)?),
            None => Err(Why::from(no_object_report(id))),
        },
        ReadVerb::Search { text } => views(&verbs::search(board, text).map_err(|e| refused(&e))?),
        ReadVerb::Fields { type_name } => {
            let v = verbs::fields(type_name.as_deref()).map_err(|e| at_input(&e, "fields"))?;
            Ok(value_out(&v)?)
        }
        ReadVerb::Templates => {
            let list = verbs::templates()
                .iter()
                .map(|(name, title)| {
                    let mut f = Fields::new();
                    f.insert("name".into(), BValue::Str(name.to_string()));
                    f.insert("title".into(), BValue::Str(title.to_string()));
                    BValue::Obj(f)
                })
                .collect();
            Ok(value_out(&BValue::Arr(list))?)
        }
        ReadVerb::References { id } => {
            if let Some(id) = id {
                if !board.objects.iter().any(|o| o.id == *id) {
                    return Err(Why::from(no_object_report(id)));
                }
            }
            let picked = board
                .references()
                .iter()
                .filter(|r| id.as_ref().map_or(true, |i| r.from == *i || r.target == *i))
                .map(verbs::reference_value)
                .collect();
            Ok(value_out(&BValue::Arr(picked))?)
        }
        ReadVerb::Meta => {
            let view = verbs::meta(board).map_err(|e| refused(&e))?;
            Ok(value_out(&view.to_value())?)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing argv.

/// What a command line gave, before the values are read.
struct Given<'a> {
    verb: &'static str,
    limits: &'a Limits,
    values: BTreeMap<&'static str, String>,
    anchors: BTreeMap<&'static str, String>,
    fields: Vec<(String, String)>,
}

/// A host's argv, from the verb on, to a call: the board's verb names and argument names, and the
/// host's own from `names`. A usage refusal is the kit's MPC-E120 or MPC-E122; a field's value the
/// board crate refuses is its row at [`Phase::Input`].
pub fn parse(argv: &[String], names: &Names, limits: &Limits) -> Result<Call, Why> {
    let Some((word, rest)) = argv.split_first() else {
        return Err(usage_report(
            "MPC-E122",
            vec![("reason", t("no verb given"))],
        ));
    };
    let Some(cmd) = names.find(word) else {
        return Err(unsupported(
            None,
            word,
            "no such verb; capabilities lists every verb",
        ));
    };
    let verb = cmd.verb;
    let Some((_, args)) = args_of(verb) else {
        return Err(Why::HostBug(HostBug::UnknownError {
            what: format!("the bridge has no forms for {verb}"),
        }));
    };
    let fields_too = takes_fields(&args);
    let mut g = Given {
        verb,
        limits,
        values: BTreeMap::new(),
        anchors: BTreeMap::new(),
        fields: Vec::new(),
    };
    let mut positional: Vec<&String> = Vec::new();
    let mut tokens = rest.iter();
    while let Some(tok) = tokens.next() {
        if tok == "--" {
            positional.extend(tokens.by_ref());
            break;
        }
        let Some(flag) = tok.strip_prefix("--").filter(|f| !f.is_empty()) else {
            positional.push(tok);
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (flag, None),
        };
        if cmd.ignored.contains(name) {
            if inline.is_some() {
                return Err(usage_error(verb, Some(name), "this flag takes no value"));
            }
            continue;
        }
        let to = match cmd.flags.get(name) {
            Some(to) => Some(*to),
            None => args
                .iter()
                .find(|a| a.name == name && a.form != Form::Fields)
                .map(|a| To::Arg(a.name)),
        };
        if to.is_none() && !fields_too {
            return Err(unsupported(
                Some(verb),
                &format!("--{name}"),
                &flags_hint(&cmd, &args),
            ));
        }
        let value = match inline {
            Some(v) => v,
            None => match tokens.next() {
                Some(v) => v.clone(),
                None => return Err(usage_error(verb, Some(name), "a flag needs a value")),
            },
        };
        match to {
            Some(To::Arg(a)) => {
                if g.values.insert(a, value).is_some() {
                    return Err(usage_error(verb, Some(a), "an argument given twice"));
                }
            }
            Some(To::Anchor(a)) => {
                if g.anchors.insert(a, value).is_some() {
                    return Err(usage_error(verb, Some(a), "an anchor given twice"));
                }
            }
            None => g.fields.push((name.to_string(), value)),
        }
    }
    for (&arg, text) in &cmd.presets {
        match g.values.get(arg) {
            Some(v) if v != text => {
                return Err(usage_error(
                    verb,
                    Some(arg),
                    "this command sets this argument itself",
                ))
            }
            Some(_) => {}
            None => {
                g.values.insert(arg, text.clone());
            }
        }
    }
    let slots: Vec<&'static str> = match &cmd.positional {
        Some(order) => order.iter().map(|(a, _)| *a).collect(),
        None => args
            .iter()
            .filter(|a| !a.optional && a.form != Form::Fields)
            .map(|a| a.name)
            .collect(),
    };
    let free: Vec<&'static str> = slots
        .into_iter()
        .filter(|a| !g.values.contains_key(a))
        .collect();
    if positional.len() > free.len() {
        return Err(usage_error(
            verb,
            None,
            "more arguments than the verb takes",
        ));
    }
    for (slot, tok) in free.into_iter().zip(positional) {
        g.values.insert(slot, tok.clone());
    }
    if verb == "meta" && g.values.is_empty() && g.fields.is_empty() {
        return Ok(Call::from_read(ReadVerb::Meta));
    }
    for a in &args {
        if !a.optional && a.form != Form::Fields && !g.values.contains_key(a.name) {
            return Err(usage_error(
                verb,
                Some(a.name),
                "a required argument is missing",
            ));
        }
    }
    for end in g.anchors.keys() {
        if !g.values.contains_key(end) {
            return Err(usage_error(
                verb,
                Some(*end),
                "an anchor for an end that isn't given",
            ));
        }
    }
    build(&g)
}

/// The flags a verb takes, for MPC-E120's hint.
fn flags_hint(cmd: &HostVerb, args: &[Arg]) -> String {
    let mut names: Vec<String> = args
        .iter()
        .filter(|a| a.form != Form::Fields)
        .map(|a| format!("--{}", a.name))
        .collect();
    names.extend(cmd.flags.keys().map(|n| format!("--{n}")));
    names.extend(cmd.ignored.iter().map(|n| format!("--{n}")));
    if names.is_empty() {
        format!("{} takes no flags", cmd.verb)
    } else {
        format!("{} takes {}", cmd.verb, names.join(", "))
    }
}

impl Given<'_> {
    /// A value the checks in [`parse`] made sure of: one that isn't there is the bridge's bug.
    fn req(&self, arg: &str) -> Result<&str, Why> {
        self.values.get(arg).map(String::as_str).ok_or_else(|| {
            Why::HostBug(HostBug::UnknownError {
                what: format!("{} reached its build without {arg}", self.verb),
            })
        })
    }

    fn opt(&self, arg: &str) -> Option<&str> {
        self.values.get(arg).map(String::as_str)
    }

    fn bad(&self, arg: &str, value: &str, what: &str, r: Option<&Refusal>) -> Why {
        bad_value(self.verb, arg, value, what, r.map(|r| r.code.as_str()))
    }

    fn id(&self, arg: &str, text: &str) -> Result<Id, Why> {
        Id::new(text).map_err(|r| self.bad(arg, text, "isn't an id", Some(&r)))
    }

    fn ids(&self, arg: &str, text: &str) -> Result<Vec<Id>, Why> {
        text.split(',').map(|i| self.id(arg, i)).collect()
    }

    fn rev(&self, arg: &str, text: &str) -> Result<Sha256, Why> {
        Sha256::parse(text).map_err(|r| self.bad(arg, text, "isn't a sha256- revision", Some(&r)))
    }

    fn obj_revs(&self, arg: &str, text: &str) -> Result<Vec<ObjRev>, Why> {
        let mut out = Vec::new();
        for one in text.split(',') {
            let Some((id, rev)) = one.split_once('@') else {
                return Err(self.bad(arg, text, "isn't a list of <id>@<rev>", None));
            };
            out.push(ObjRev {
                id: self.id(arg, id)?,
                rev: self.rev(arg, rev)?,
            });
        }
        Ok(out)
    }

    fn num(&self, arg: &str, text: &str) -> Result<f64, Why> {
        match json::parse(text.as_bytes(), self.limits) {
            Ok(BValue::Num(n)) => Ok(n),
            Ok(_) => Err(self.bad(arg, text, "isn't a number", None)),
            Err(r) => Err(self.bad(arg, text, "isn't a number", Some(&r))),
        }
    }

    fn int(&self, arg: &str, text: &str) -> Result<i32, Why> {
        let n = self.num(arg, text)?;
        if n.fract() != 0.0 || n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {
            return Err(self.bad(arg, text, "isn't a whole number in range", None));
        }
        Ok(n as i32)
    }

    fn boolean(&self, arg: &str, text: &str) -> Result<bool, Why> {
        match text {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(self.bad(arg, text, "isn't true or false", None)),
        }
    }

    fn point(&self, arg: &str, text: &str) -> Result<(f64, f64), Why> {
        let Some((x, y)) = text.split_once(',') else {
            return Err(self.bad(arg, text, "isn't a point <x>,<y>", None));
        };
        Ok((self.num(arg, x)?, self.num(arg, y)?))
    }

    fn kind(&self, arg: &str, text: &str) -> Result<ObjectType, Why> {
        ObjectType::parse(text).map_err(|r| self.bad(arg, text, "isn't a type name", Some(&r)))
    }

    fn after(&self, arg: &str, text: &str) -> Result<Option<Vec<Id>>, Why> {
        match text.strip_prefix("after:") {
            Some(ids) => Ok(Some(self.ids(arg, ids)?)),
            None => Ok(None),
        }
    }

    fn position(&self, arg: &str, text: &str) -> Result<Position, Why> {
        match text {
            "front" => Ok(Position::Front),
            "back" => Ok(Position::Back),
            _ => match self.after(arg, text)? {
                Some(ids) => Ok(Position::After(ids)),
                None => Err(self.bad(arg, text, "isn't front, back or after:<ids>", None)),
            },
        }
    }

    fn layer(&self, arg: &str, text: &str) -> Result<Layer, Why> {
        match text {
            "front" => Ok(Layer::Front),
            "back" => Ok(Layer::Back),
            "forward" => Ok(Layer::Forward),
            "backward" => Ok(Layer::Backward),
            _ => match self.after(arg, text)? {
                Some(ids) => Ok(Layer::After(ids)),
                None => Err(self.bad(
                    arg,
                    text,
                    "isn't front, back, forward, backward or after:<ids>",
                    None,
                )),
            },
        }
    }

    fn end(&self, arg: &str) -> Result<End, Why> {
        let text = self.req(arg)?;
        let flag = self.anchors.get(arg).map(String::as_str);
        if text.contains(',') {
            if flag.is_some() {
                return Err(self.bad(arg, text, "is a point, which has no anchor", None));
            }
            let (x, y) = self.point(arg, text)?;
            return Ok(End::Point { x, y });
        }
        let (id, anchor) = match text.split_once(':') {
            Some((id, a)) => (id, Some(a)),
            None => (text, None),
        };
        let anchor = match (anchor, flag) {
            (Some(_), Some(_)) => {
                return Err(self.bad(arg, text, "has an anchor, and another is given", None))
            }
            (Some(a), None) | (None, Some(a)) => {
                if a.is_empty() {
                    return Err(self.bad(arg, text, "has an empty anchor", None));
                }
                Some(a.to_string())
            }
            (None, None) => None,
        };
        Ok(End::Object {
            id: self.id(arg, id)?,
            anchor,
        })
    }

    fn word<T: Copy>(
        &self,
        arg: &str,
        text: &str,
        all: &[T],
        name: impl Fn(T) -> &'static str,
    ) -> Result<T, Why> {
        match all.iter().copied().find(|w| name(*w) == text) {
            Some(w) => Ok(w),
            None => {
                let words: Vec<&str> = all.iter().map(|w| name(*w)).collect();
                Err(self.bad(
                    arg,
                    text,
                    &format!("isn't one of {}", words.join(", ")),
                    None,
                ))
            }
        }
    }

    fn json(&self, arg: &str, text: &str) -> Result<BValue, Why> {
        json::parse(text.as_bytes(), self.limits)
            .map_err(|r| self.bad(arg, "(JSON)", "isn't valid JSON", Some(&r)))
    }

    fn removed(&self, arg: &str) -> Result<Removed, Why> {
        let v = self.json(arg, self.req(arg)?)?;
        Removed::from_value(&v)
            .map_err(|r| self.bad(arg, "(JSON)", "isn't what a delete took", Some(&r)))
    }

    fn objects(&self, arg: &str) -> Result<Vec<Object>, Why> {
        let BValue::Arr(list) = self.json(arg, self.req(arg)?)? else {
            return Err(self.bad(arg, "(JSON)", "isn't a list of objects", None));
        };
        list.iter()
            .map(|v| {
                Object::from_value(v)
                    .map_err(|r| self.bad(arg, "(JSON)", "isn't a list of objects", Some(&r)))
            })
            .collect()
    }

    fn board(&self, arg: &str) -> Result<Board, Why> {
        Board::from_json(self.req(arg)?.as_bytes(), self.limits)
            .map_err(|r| self.bad(arg, "(JSON)", "isn't a board", Some(&r)))
    }

    /// The `--<field>` pairs for a new object of this type: a field can be set, not removed.
    fn new_fields(&self, type_name: &str) -> Result<Fields, Why> {
        let edits = verbs::flags(type_name, &pair_refs(&self.fields), self.limits)
            .map_err(|e| at_input(&e, self.verb))?;
        if let Some(name) = edits.unset.first() {
            return Err(usage_error(
                self.verb,
                Some(name.as_str()),
                "null removes a field, and a new object has none to remove",
            ));
        }
        Ok(edits.set)
    }

    fn get<T>(
        &self,
        arg: &str,
        read: impl Fn(&Self, &str, &str) -> Result<T, Why>,
    ) -> Result<T, Why> {
        read(self, arg, self.req(arg)?)
    }

    fn get_opt<T>(
        &self,
        arg: &str,
        read: impl Fn(&Self, &str, &str) -> Result<T, Why>,
    ) -> Result<Option<T>, Why> {
        self.opt(arg).map(|text| read(self, arg, text)).transpose()
    }
}

fn text(_: &Given<'_>, _: &str, t: &str) -> Result<String, Why> {
    Ok(t.to_string())
}

/// The one place a call is built from what a command line gave: every verb, by name.
fn build(g: &Given<'_>) -> Result<Call, Why> {
    let verb = g.verb;
    let align =
        |g: &Given<'_>, a: &str, t: &str| g.word(a, t, AlignTo::ALL, |w: AlignTo| w.as_str());
    let axis = |g: &Given<'_>, a: &str, t: &str| g.word(a, t, Axis::ALL, |w: Axis| w.as_str());
    let side = |g: &Given<'_>, a: &str, t: &str| g.word(a, t, Side::ALL, |w: Side| w.as_str());
    let v = match verb {
        "add" => {
            let kind = g.get("kind", Given::kind)?;
            Verb::Add {
                fields: g.new_fields(kind.as_str())?,
                at: g.get("at", Given::position)?,
                kind,
            }
        }
        "update" => {
            return Ok(Call {
                body: Body::Update {
                    id: g.get("id", Given::id)?,
                    rev: g.get("rev", Given::rev)?,
                    pairs: g.fields.clone(),
                    limits: *g.limits,
                },
            })
        }
        "delete" => Verb::Delete {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            cascade: g.get("cascade", Given::boolean)?,
        },
        "put-back" => Verb::PutBack {
            removed: g.removed("removed")?,
        },
        "connect" => Verb::Connect {
            from: g.end("from")?,
            to: g.end("to")?,
            fields: g.new_fields("connector")?,
        },
        "react" => Verb::React {
            id: g.get("id", Given::id)?,
            emoji: g.get("emoji", text)?,
            on: g.get_opt("on", Given::boolean)?,
        },
        "vote" => Verb::Vote {
            id: g.get("id", Given::id)?,
            delta: g.get("delta", Given::int)?,
        },
        "comment" => Verb::Comment {
            target: g.get_opt("target", Given::id)?,
            text: g.get("text", text)?,
            point: g.get_opt("point", Given::point)?,
        },
        "reply" => Verb::Reply {
            id: g.get("id", Given::id)?,
            text: g.get("text", text)?,
        },
        "resolve" => Verb::Resolve {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            resolved: g.get("resolved", Given::boolean)?,
        },
        "meta" => {
            let edits = verbs::settings_flags(&pair_refs(&g.fields), g.limits)
                .map_err(|e| at_input(&e, verb))?;
            if edits.set.is_empty() && edits.unset.is_empty() {
                return Err(usage_error(
                    verb,
                    None,
                    "a change to the settings names a field to change",
                ));
            }
            Verb::Meta {
                rev: g.get("rev", Given::rev)?,
                set: edits.set,
                unset: edits.unset,
            }
        }
        "lock" => Verb::Lock {
            ids: g.get("ids", Given::obj_revs)?,
            locked: g.get("locked", Given::boolean)?,
        },
        "check-add" => Verb::CheckAdd {
            id: g.get("id", Given::id)?,
            text: g.get("text", text)?,
        },
        "check-set" => Verb::CheckSet {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            item: g.get("item", text)?,
            text: g.get_opt("text", text)?,
            done: g.get_opt("done", Given::boolean)?,
        },
        "check-remove" => Verb::CheckRemove {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            item: g.get("item", text)?,
        },
        "table-cell" => Verb::TableCell {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            row: g.get("row", text)?,
            column: g.get("column", text)?,
            text: g.get("text", text)?,
        },
        "table-add-row" => Verb::TableAddRow {
            id: g.get("id", Given::id)?,
            after: g.get_opt("after", text)?,
        },
        "table-remove-row" => Verb::TableRemoveRow {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            row: g.get("row", text)?,
        },
        "table-add-column" => Verb::TableAddColumn {
            id: g.get("id", Given::id)?,
            after: g.get_opt("after", text)?,
            width: g.get_opt("width", Given::num)?,
        },
        "table-remove-column" => Verb::TableRemoveColumn {
            id: g.get("id", Given::id)?,
            rev: g.get("rev", Given::rev)?,
            column: g.get("column", text)?,
        },
        "move" => Verb::Move {
            ids: g.get("ids", Given::obj_revs)?,
            dx: g.get("dx", Given::num)?,
            dy: g.get("dy", Given::num)?,
        },
        "order" => Verb::Order {
            ids: g.get("ids", Given::ids)?,
            to: g.get("to", Given::layer)?,
        },
        "frame-order" => Verb::FrameOrder {
            id: g.get("id", Given::id)?,
            to: g.get("to", Given::position)?,
        },
        "group" => Verb::Group {
            ids: g.get("ids", Given::obj_revs)?,
        },
        "ungroup" => Verb::Ungroup {
            ids: g.get("ids", Given::obj_revs)?,
        },
        "align" => Verb::Align {
            ids: g.get("ids", Given::obj_revs)?,
            to: g.get("to", align)?,
        },
        "distribute" => Verb::Distribute {
            ids: g.get("ids", Given::obj_revs)?,
            axis: g.get("axis", axis)?,
        },
        "tidy" => Verb::Tidy {
            ids: g.get("ids", Given::obj_revs)?,
        },
        "copy-style" => Verb::CopyStyle {
            from: g.get("from", Given::id)?,
            to: g.get("to", Given::obj_revs)?,
        },
        "duplicate" => Verb::Duplicate {
            ids: g.get("ids", Given::ids)?,
            dx: g.get("dx", Given::num)?,
            dy: g.get("dy", Given::num)?,
        },
        "paste" => Verb::Paste {
            objects: g.objects("objects")?,
            x: g.get("x", Given::num)?,
            y: g.get("y", Given::num)?,
        },
        "connected-copy" => Verb::ConnectedCopy {
            id: g.get("id", Given::id)?,
            side: g.get("side", side)?,
        },
        "mind-child" => Verb::MindChild {
            parent: g.get("parent", Given::id)?,
            text: g.get("text", text)?,
        },
        "template" => Verb::Template {
            name: g.get("name", text)?,
            x: g.get("x", Given::num)?,
            y: g.get("y", Given::num)?,
        },
        "clear-blob" => Verb::ClearBlob {
            hash: g.get("hash", Given::rev)?,
        },
        "list" => {
            return Ok(Call::from_read(ReadVerb::List {
                kind: g.get_opt("kind", Given::kind)?,
            }))
        }
        "read" => {
            return Ok(Call::from_read(ReadVerb::Read {
                id: g.get("id", Given::id)?,
            }))
        }
        "search" => {
            return Ok(Call::from_read(ReadVerb::Search {
                text: g.get("text", text)?,
            }))
        }
        "fields" => {
            return Ok(Call::from_read(ReadVerb::Fields {
                type_name: g.get_opt("type", text)?,
            }))
        }
        "templates" => return Ok(Call::from_read(ReadVerb::Templates)),
        "references" => {
            return Ok(Call::from_read(ReadVerb::References {
                id: g.get_opt("id", Given::id)?,
            }))
        }
        "apply" => return Ok(Call::apply(g.req("ops")?, g.limits)),
        "inverse" => {
            return Ok(Call::inverse(
                g.board("before")?,
                g.board("after")?,
                g.get("group_id", text)?,
            ))
        }
        other => {
            return Err(Why::HostBug(HostBug::UnknownError {
                what: format!("the bridge builds no call for {other}"),
            }))
        }
    };
    Ok(Call::from_verb(v))
}

// ---------------------------------------------------------------------------------------------
// Usage lines, for capabilities.

/// How an argument is shown in a usage line.
fn placeholder(a: &Arg) -> String {
    let words = |all: Vec<&str>| all.join("|");
    let form = match a.form {
        Form::Id | Form::Text | Form::Type | Form::Fields => None,
        Form::Ids => Some("<id>,…".to_string()),
        Form::ObjRevs => Some("<id>@<rev>,…".to_string()),
        Form::Rev | Form::Hash => Some("sha256-<hex>".to_string()),
        Form::Bool => Some("true|false".to_string()),
        Form::Int => Some("<whole number>".to_string()),
        Form::Num => Some("<number>".to_string()),
        Form::Point => Some("<x>,<y>".to_string()),
        Form::Position => Some("front|back|after:<id>,…".to_string()),
        Form::Layer => Some("front|back|forward|backward|after:<id>,…".to_string()),
        Form::End => Some("<id>[:<anchor>]|<x>,<y>".to_string()),
        Form::Align => Some(words(AlignTo::ALL.iter().map(|w| w.as_str()).collect())),
        Form::Axis => Some(words(Axis::ALL.iter().map(|w| w.as_str()).collect())),
        Form::Side => Some(words(Side::ALL.iter().map(|w| w.as_str()).collect())),
        Form::Json => Some("JSON".to_string()),
    };
    match form {
        Some(f) => format!("<{}: {f}>", a.name),
        None => format!("<{}>", a.name),
    }
}

/// The verb's usage line, as `capabilities` prints it: the verb, its required arguments in order,
/// its optional ones as `[--<arg> <value>]`, and `[--<field> <text>]…` for a verb that takes
/// fields. `None` for a verb the board doesn't have. Every line's own example,
/// [`example`], parses.
pub fn usage(verb: &str) -> Option<String> {
    let (name, args) = args_of(verb)?;
    let mut out = vec![name.to_string()];
    for a in args
        .iter()
        .filter(|a| !a.optional && a.form != Form::Fields)
    {
        out.push(placeholder(a));
    }
    for a in args.iter().filter(|a| a.optional) {
        out.push(format!("[--{} {}]", a.name, placeholder(a)));
    }
    if takes_fields(&args) {
        let what = if name == "meta" { "setting" } else { "field" };
        out.push(format!("[--<{what}> <text>|null]…"));
    }
    if name == "apply" {
        out.push(
            "(the ops as {\"ops\": […]} or the bare list […]; each add's id a label)".to_string(),
        );
    }
    Some(out.join(" "))
}

/// A value of each form that parses, for [`example`].
fn sample(a: &Arg, verb: &str) -> String {
    let rev = Sha256::of(b"").to_string();
    match a.form {
        Form::Id => "s-1".to_string(),
        Form::Ids => "s-1,s-2".to_string(),
        Form::ObjRevs => format!("s-1@{rev},s-2@{rev}"),
        Form::Rev | Form::Hash => rev,
        Form::Text => match (verb, a.name) {
            ("template", "name") => "kanban".to_string(),
            ("fields", "type") => "sticky".to_string(),
            _ => "hi".to_string(),
        },
        Form::Bool => "true".to_string(),
        Form::Int => "1".to_string(),
        Form::Num => "10".to_string(),
        Form::Point => "0,0".to_string(),
        Form::Type => "sticky".to_string(),
        Form::Fields => String::new(),
        Form::Position => "front".to_string(),
        Form::Layer => "after:s-2".to_string(),
        Form::End => match a.name {
            "to" => "1.5,-2".to_string(),
            _ => "s-1:top".to_string(),
        },
        Form::Align => AlignTo::Left.as_str().to_string(),
        Form::Axis => Axis::Horizontal.as_str().to_string(),
        Form::Side => Side::Right.as_str().to_string(),
        Form::Json => match a.name {
            "ops" => "{\"ops\":[{\"op\":\"add\",\"obj\":{\"id\":\"a\",\"type\":\"sticky\"}}]}"
                .to_string(),
            "objects" => "[{\"id\":\"s-9\",\"type\":\"sticky\"}]".to_string(),
            "removed" => {
                let Ok(id) = Id::new("s-9") else {
                    return String::new();
                };
                let r = Removed {
                    object: Object {
                        id,
                        kind: ObjectType::Sticky,
                        fields: Fields::new(),
                    },
                    after: Vec::new(),
                    frame_after: Vec::new(),
                };
                text_of(json::to_canonical(&r.to_value()))
            }
            _ => text_of(Board::empty().to_canonical_json()),
        },
    }
}

/// JSON bytes the board crate wrote, as text; empty if it couldn't, which a test would catch.
fn text_of(bytes: Result<Vec<u8>, Refusal>) -> String {
    bytes
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// An example of the verb's command line that parses, in [`usage`]'s order: its required
/// arguments, then each optional one as a flag, then one field where the verb takes fields.
pub fn example(verb: &str) -> Option<Vec<String>> {
    let (name, args) = args_of(verb)?;
    let mut out = vec![name.to_string()];
    for a in args
        .iter()
        .filter(|a| !a.optional && a.form != Form::Fields)
    {
        out.push(sample(a, name));
    }
    for a in args.iter().filter(|a| a.optional) {
        out.push(format!("--{}", a.name));
        out.push(sample(a, name));
    }
    if takes_fields(&args) {
        let (field, value) = match name {
            "meta" => ("title", "T"),
            "connect" => ("label", "hi"),
            _ => ("text", "hi"),
        };
        out.push(format!("--{field}"));
        out.push(value.to_string());
    }
    Some(out)
}
