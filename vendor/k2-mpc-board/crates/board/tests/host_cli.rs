//! A host's whiteboard commands, matched to the pack's verbs, so one command set works on a file and
//! on the host's own boards. `tests/data/host-cli-v1.json` holds the host's list as data: names,
//! args, flags and refusal meanings, with no host code. Every difference says why beside it, and
//! each test names every entry that doesn't hold.

use k2_mpc_board::json::{self, Fields, Value};
use k2_mpc_board::schema;
use k2_mpc_board::verbs::{self, AlignTo, Axis, End, Layer, ObjRev, Removed, Side, Verb, VerbCode};
use k2_mpc_board::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

const VERB_VERDICTS: &[&str] = &["same", "rename", "ruling", "not_file"];
const FLAG_VERDICTS: &[&str] = &["no_op", "ruling", "not_file"];
const REFUSAL_VERDICTS: &[&str] = &["same", "ruling", "not_file", "host"];
const VALUE_KINDS: &[&str] = &["bool", "text", "number", "rev", "fields", "settings"];
const CONFLICTS: &[&str] = &[
    "conflict:changed",
    "conflict:deleted",
    "conflict:exists",
    "conflict:refused",
];

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn data() -> Fields {
    let path = manifest().join("tests/data/host-cli-v1.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match json::parse(&bytes, &Limits::V1) {
        Ok(Value::Obj(f)) => f,
        other => panic!("the host's list isn't a JSON object: {other:?}"),
    }
}

fn text<'v>(f: &'v Fields, k: &str) -> Option<&'v str> {
    match f.get(k) {
        Some(Value::Str(s)) => Some(s),
        _ => None,
    }
}

fn says_why(f: &Fields) -> bool {
    text(f, "why").is_some_and(|w| !w.trim().is_empty())
}

fn member<'v>(f: &'v Fields, k: &str) -> Option<&'v Fields> {
    match f.get(k) {
        Some(Value::Obj(o)) => Some(o),
        _ => None,
    }
}

fn objects<'v>(f: &'v Fields, k: &str) -> Vec<&'v Fields> {
    match f.get(k) {
        None => Vec::new(),
        Some(Value::Arr(a)) => a
            .iter()
            .map(|v| match v {
                Value::Obj(o) => o,
                other => panic!("{k} holds {other:?}, not an object"),
            })
            .collect(),
        Some(other) => panic!("{k} is {other:?}, not a list"),
    }
}

fn texts<'v>(f: &'v Fields, k: &str) -> Vec<&'v str> {
    match f.get(k) {
        None => Vec::new(),
        Some(Value::Arr(a)) => a
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.as_str(),
                other => panic!("{k} holds {other:?}, not text"),
            })
            .collect(),
        Some(other) => panic!("{k} is {other:?}, not a list"),
    }
}

/// One verb of the host's list: its entry, and what it says about the pack.
struct HostVerb<'d> {
    name: &'d str,
    entry: &'d Fields,
    pack: Fields,
}

impl HostVerb<'_> {
    fn verb(&self) -> Option<&str> {
        text(&self.pack, "verb")
    }
}

fn host_verbs(d: &Fields) -> Vec<HostVerb<'_>> {
    objects(d, "verbs")
        .into_iter()
        .map(|entry| HostVerb {
            name: text(entry, "name").unwrap_or("(no name)"),
            entry,
            pack: member(entry, "pack").cloned().unwrap_or_default(),
        })
        .collect()
}

fn pack_verbs() -> BTreeSet<&'static str> {
    verbs::WRITE_VERBS
        .iter()
        .chain(verbs::OTHER_VERBS)
        .copied()
        .collect()
}

/// A verb's arguments from [`verbs::ARGS`], each with whether it may be left out.
fn args_of(verb: &str) -> Option<Vec<(&'static str, bool)>> {
    let (_, args) = verbs::ARGS.iter().find(|(n, _)| *n == verb)?;
    Some(
        args.iter()
            .map(|a| match a.strip_suffix('?') {
                Some(name) => (name, true),
                None => (*a, false),
            })
            .collect(),
    )
}

fn resolves(code: &str) -> bool {
    VerbCode::ALL.iter().any(|c| c.as_str() == code)
        || RefusalCode::ALL.iter().any(|c| c.as_str() == code)
        || CONFLICTS.contains(&code)
}

fn pack_codes(f: &Fields) -> Option<Vec<String>> {
    match f.get("pack") {
        None => None,
        Some(Value::Str(s)) => Some(vec![s.clone()]),
        Some(Value::Arr(a)) => Some(
            a.iter()
                .map(|v| match v {
                    Value::Str(s) => s.clone(),
                    other => format!("{other:?}"),
                })
                .collect(),
        ),
        Some(other) => Some(vec![format!("{other:?}")]),
    }
}

fn report(what: &str, problems: &[String]) -> String {
    format!("{} {what}:\n  {}", problems.len(), problems.join("\n  "))
}

#[test]
fn every_host_verb_has_a_known_verdict_and_says_why() {
    let d = data();
    let mut problems = Vec::new();
    let mut names = BTreeSet::new();
    for hv in host_verbs(&d) {
        if !names.insert(hv.name) {
            problems.push(format!("{}: listed twice", hv.name));
        }
        let verdict = text(&hv.pack, "verdict").unwrap_or("(none)");
        if !VERB_VERDICTS.contains(&verdict) {
            problems.push(format!("{}: no verdict {verdict:?}", hv.name));
            continue;
        }
        if verdict != "same" && !says_why(&hv.pack) {
            problems.push(format!("{}: {verdict}, and it doesn't say why", hv.name));
        }
        let verb = hv.verb();
        let ok = match verdict {
            "same" => verb == Some(hv.name),
            "rename" => verb.is_some_and(|v| v != hv.name),
            "not_file" => verb.is_none(),
            _ => true,
        };
        if !ok {
            problems.push(format!(
                "{}: {verdict}, with the pack's verb {verb:?}",
                hv.name
            ));
        }
    }
    let used: BTreeSet<&str> = VERB_VERDICTS
        .iter()
        .chain(FLAG_VERDICTS)
        .chain(REFUSAL_VERDICTS)
        .copied()
        .collect();
    let described: BTreeSet<&str> = member(&d, "verdicts")
        .map(|v| v.keys().map(String::as_str).collect())
        .unwrap_or_default();
    if used != described {
        problems.push(format!(
            "verdicts: the list describes {described:?}, the test knows {used:?}"
        ));
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("verdicts don't hold", &problems)
    );
}

#[test]
fn every_verb_the_host_maps_is_a_pack_verb() {
    let d = data();
    let known = pack_verbs();
    let mut problems = Vec::new();
    for hv in host_verbs(&d) {
        let Some(verb) = hv.verb() else { continue };
        if !known.contains(verb) {
            problems.push(format!("{}: the pack has no verb {verb:?}", hv.name));
        }
        if args_of(verb).is_none() {
            problems.push(format!("{}: verbs::ARGS doesn't list {verb:?}", hv.name));
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("host verbs have no pack verb", &problems)
    );
}

#[test]
fn every_host_arg_and_flag_is_an_argument_of_its_verb() {
    let d = data();
    let mut problems = Vec::new();
    for hv in host_verbs(&d) {
        let (name, entry) = (hv.name, hv.entry);
        let Some(verb) = hv.verb() else { continue };
        let Some(args) = args_of(verb) else { continue };
        let names: Vec<&str> = args.iter().map(|(n, _)| *n).collect();
        let flags = member(entry, "flags").cloned().unwrap_or_default();
        let verdicts = member(entry, "flag_verdicts").cloned().unwrap_or_default();
        let renamed = member(&hv.pack, "as").cloned().unwrap_or_default();
        let host_sets = member(&hv.pack, "host_sets").cloned().unwrap_or_default();
        let mut host_names: Vec<String> = texts(entry, "args")
            .iter()
            .map(|a| a.trim_end_matches('?').to_string())
            .collect();
        for (flag, kind) in &flags {
            if !matches!(kind, Value::Str(k) if VALUE_KINDS.contains(&k.as_str())) {
                problems.push(format!("{name}: --{flag} takes {kind:?}, no kind of value"));
            }
            host_names.push(flag.clone());
        }
        let mut given: BTreeSet<String> = BTreeSet::new();
        for n in &host_names {
            if n == "<field>" {
                let takes: Vec<&str> = ["fields", "set", "unset"]
                    .into_iter()
                    .filter(|a| names.contains(a))
                    .collect();
                if takes.is_empty() {
                    problems.push(format!(
                        "{name}: --<field>, and the pack's {verb} takes none"
                    ));
                }
                given.extend(takes.iter().map(|a| a.to_string()));
                continue;
            }
            if let Some(Value::Obj(v)) = verdicts.get(n) {
                let verdict = text(v, "verdict").unwrap_or("");
                if !FLAG_VERDICTS.contains(&verdict) || !says_why(v) {
                    problems.push(format!(
                        "{name}: --{n} has the verdict {verdict:?} with no why"
                    ));
                }
                if names.contains(&n.as_str()) {
                    problems.push(format!("{name}: --{n} has a verdict, but {verb} takes it"));
                }
                continue;
            }
            let to = match renamed.get(n) {
                Some(Value::Str(to)) => to.as_str(),
                _ => n.as_str(),
            };
            if names.contains(&to) {
                given.insert(to.to_string());
            } else {
                problems.push(format!(
                    "{name}: the host's {n} is no argument of the pack's {verb} {names:?}"
                ));
            }
        }
        for (arg, why) in &host_sets {
            if !matches!(why, Value::Str(w) if !w.trim().is_empty()) {
                problems.push(format!(
                    "{name}: the host sets {arg} and doesn't say to what"
                ));
            }
            match names.iter().find(|a| **a == arg.as_str()) {
                Some(a) => {
                    given.insert(a.to_string());
                }
                None => problems.push(format!(
                    "{name}: the host sets {arg}, which {verb} doesn't take"
                )),
            }
        }
        for k in verdicts.keys() {
            if !host_names.contains(k) {
                problems.push(format!(
                    "{name}: a verdict for --{k}, which the host doesn't have"
                ));
            }
        }
        for k in renamed.keys() {
            if !host_names.contains(k) {
                problems.push(format!("{name}: renames {k}, which the host doesn't have"));
            }
        }
        for (arg, optional) in &args {
            if !optional && !given.contains(*arg) {
                problems.push(format!(
                    "{name}: the pack's {verb} needs {arg}, and the host doesn't give it"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("args and flags don't resolve", &problems)
    );
}

fn is_object_field(name: &str) -> bool {
    schema::COMMON.iter().any(|f| f.name == name)
        || schema::TYPES
            .iter()
            .any(|(_, fields)| fields.iter().any(|f| f.name == name))
}

#[test]
fn every_field_with_a_verdict_is_one_the_format_lacks() {
    let d = data();
    let mut problems = Vec::new();
    for hv in host_verbs(&d) {
        let name = hv.name;
        let flags = member(hv.entry, "flags").cloned().unwrap_or_default();
        let of = text(&flags, "<field>");
        let Some(verdicts) = member(hv.entry, "field_verdicts") else {
            continue;
        };
        if of.is_none() {
            problems.push(format!("{name}: field verdicts, and no --<field>"));
        }
        for (field, v) in verdicts {
            let Value::Obj(v) = v else {
                problems.push(format!("{name}: --{field}'s verdict isn't an object"));
                continue;
            };
            let verdict = text(v, "verdict").unwrap_or("");
            if !FLAG_VERDICTS.contains(&verdict) || !says_why(v) {
                problems.push(format!(
                    "{name}: --{field} has the verdict {verdict:?} with no why"
                ));
            }
            let known = match of {
                Some("settings") => schema::SETTINGS.iter().any(|f| f.name == field),
                _ => is_object_field(field),
            };
            if known {
                problems.push(format!(
                    "{name}: --{field} has a verdict, but the format holds it"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("field verdicts don't hold", &problems)
    );
}

/// A refusal entry's verdict, why and codes, wherever it sits.
fn refusal_problems(at: &str, f: &Fields, verdicts: &[&str], problems: &mut Vec<String>) {
    let verdict = text(f, "verdict").unwrap_or("(none)");
    if !verdicts.contains(&verdict) {
        problems.push(format!("{at}: no verdict {verdict:?}"));
        return;
    }
    if verdict != "same" && !says_why(f) {
        problems.push(format!("{at}: {verdict}, and it doesn't say why"));
    }
    let codes = pack_codes(f);
    match (verdict, &codes) {
        ("same", Some(c)) if c.len() == 1 && Some(c[0].as_str()) == text(f, "code") => {}
        ("same", _) => problems.push(format!("{at}: same, with the pack's {codes:?}")),
        ("ruling", None) => problems.push(format!("{at}: a ruling that names no pack code")),
        ("not_file" | "host", Some(c)) => {
            problems.push(format!("{at}: {verdict}, yet it names {c:?}"))
        }
        _ => {}
    }
    for code in codes.unwrap_or_default() {
        if !resolves(&code) {
            problems.push(format!("{at}: the pack has no code {code:?}"));
        }
    }
}

#[test]
fn every_refusal_means_a_pack_code_or_says_whose_it_is() {
    let d = data();
    let mut problems = Vec::new();
    let mut table = BTreeSet::new();
    for r in objects(&d, "refusals") {
        let code = text(r, "code").unwrap_or("(no code)");
        if !table.insert(code) {
            problems.push(format!("{code}: listed twice"));
        }
        refusal_problems(code, r, REFUSAL_VERDICTS, &mut problems);
    }
    for hv in host_verbs(&d) {
        let own = texts(hv.entry, "refusals");
        for code in &own {
            if !table.contains(code) {
                problems.push(format!(
                    "{}: refuses {code}, which isn't in the table",
                    hv.name
                ));
            }
        }
        for (code, v) in member(hv.entry, "refusal_as").cloned().unwrap_or_default() {
            let at = format!("{}: {code}", hv.name);
            if !own.contains(&code.as_str()) {
                problems.push(format!(
                    "{at}: read another way, but not one of its refusals"
                ));
            }
            let Value::Obj(mut v) = v else {
                problems.push(format!("{at}: not an object"));
                continue;
            };
            v.insert("code".into(), Value::Str(code.clone()));
            refusal_problems(&at, &v, &["ruling", "not_file", "host"], &mut problems);
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("refusals don't resolve", &problems)
    );
}

#[test]
fn every_pack_verb_is_on_the_host_list_or_says_why() {
    let d = data();
    let known = pack_verbs();
    let mapped: BTreeSet<String> = host_verbs(&d)
        .iter()
        .filter_map(|hv| hv.verb().map(str::to_string))
        .collect();
    let mut problems = Vec::new();
    let mut only = BTreeSet::new();
    for p in objects(&d, "pack_only") {
        let verb = text(p, "verb").unwrap_or("(no verb)");
        if !only.insert(verb) {
            problems.push(format!("{verb}: pack_only twice"));
        }
        if !known.contains(verb) {
            problems.push(format!("{verb}: pack_only names no pack verb"));
        }
        if mapped.contains(verb) {
            problems.push(format!("{verb}: pack_only, and a host verb maps to it"));
        }
        if !says_why(p) {
            problems.push(format!("{verb}: pack_only, and it doesn't say why"));
        }
    }
    for verb in &known {
        if !mapped.contains(*verb) && !only.contains(verb) {
            problems.push(format!("{verb}: a pack verb on neither list"));
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("pack verbs aren't accounted for", &problems)
    );
}

#[test]
fn every_alias_names_a_host_verb() {
    let d = data();
    let names: BTreeSet<&str> = host_verbs(&d).iter().map(|hv| hv.name).collect();
    let mut problems = Vec::new();
    for (alias, to) in member(&d, "aliases").cloned().unwrap_or_default() {
        if names.contains(alias.as_str()) {
            problems.push(format!("{alias}: an alias, and a verb"));
        }
        if !matches!(&to, Value::Str(t) if names.contains(t.as_str())) {
            problems.push(format!(
                "{alias}: an alias of {to:?}, which isn't on the list"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("aliases don't resolve", &problems)
    );
}

#[test]
fn every_change_of_form_names_a_test_that_is_there() {
    let d = data();
    let mut problems = Vec::new();
    for hv in host_verbs(&d) {
        for form in objects(hv.entry, "forms") {
            let what = text(form, "what").unwrap_or("");
            if what.trim().is_empty() {
                problems.push(format!("{}: a form that doesn't say what", hv.name));
            }
            let test = text(form, "test").unwrap_or("");
            let Some((path, name)) = test.split_once("::") else {
                problems.push(format!("{}: {test:?} isn't path::test", hv.name));
                continue;
            };
            let source = std::fs::read_to_string(manifest().join(path)).unwrap_or_default();
            if !source.contains(&format!("#[test]\nfn {name}() {{")) {
                problems.push(format!(
                    "{}: no test {name} in {path}, for: {what}",
                    hv.name
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("forms have no test", &problems)
    );
}

/// A verb's field names, from its variant, with `?` on an `Option`. The match has no wildcard and
/// no `..`, so a variant or a field this doesn't name doesn't build.
macro_rules! variant_args {
    ($v:expr; $($variant:ident { $($req:ident),* $(; $($opt:ident),+)? }),+ $(,)?) => {
        match $v {
            $(Verb::$variant { $($req,)* $($($opt,)+)? } => {
                $(let _ = $req;)*
                $($(let _: &Option<_> = $opt;)+)?
                #[allow(unused_mut)]
                let mut names: Vec<String> = vec![$(stringify!($req).to_string()),*];
                $($(names.push(concat!(stringify!($opt), "?").to_string());)+)?
                names
            })+
        }
    };
}

fn one_of_each() -> Vec<Verb> {
    let i = Id::new("s-1").expect("an id");
    let r = Sha256::of(b"x");
    let ids = || vec![i.clone()];
    let named = || {
        vec![ObjRev {
            id: i.clone(),
            rev: r,
        }]
    };
    let t = || "t".to_string();
    let object = Object {
        id: i.clone(),
        kind: ObjectType::Sticky,
        fields: Fields::new(),
    };
    vec![
        Verb::Add {
            kind: ObjectType::Sticky,
            fields: Fields::new(),
            at: Position::Front,
        },
        Verb::Update {
            id: i.clone(),
            rev: r,
            set: Fields::new(),
            unset: Vec::new(),
        },
        Verb::Delete {
            id: i.clone(),
            rev: r,
            cascade: true,
        },
        Verb::PutBack {
            removed: Removed {
                object: object.clone(),
                after: Vec::new(),
                frame_after: Vec::new(),
            },
        },
        Verb::Connect {
            from: End::Point { x: 0.0, y: 0.0 },
            to: End::Point { x: 1.0, y: 1.0 },
            fields: Fields::new(),
        },
        Verb::React {
            id: i.clone(),
            emoji: "y".into(),
            on: Some(true),
        },
        Verb::Vote {
            id: i.clone(),
            delta: 1,
        },
        Verb::Comment {
            target: None,
            text: t(),
            point: None,
        },
        Verb::Reply {
            id: i.clone(),
            text: t(),
        },
        Verb::Resolve {
            id: i.clone(),
            rev: r,
            resolved: true,
        },
        Verb::Meta {
            rev: r,
            set: Fields::new(),
            unset: Vec::new(),
        },
        Verb::Lock {
            ids: named(),
            locked: true,
        },
        Verb::CheckAdd {
            id: i.clone(),
            text: t(),
        },
        Verb::CheckSet {
            id: i.clone(),
            rev: r,
            item: t(),
            text: None,
            done: None,
        },
        Verb::CheckRemove {
            id: i.clone(),
            rev: r,
            item: t(),
        },
        Verb::TableCell {
            id: i.clone(),
            rev: r,
            row: t(),
            column: t(),
            text: t(),
        },
        Verb::TableAddRow {
            id: i.clone(),
            after: None,
        },
        Verb::TableRemoveRow {
            id: i.clone(),
            rev: r,
            row: t(),
        },
        Verb::TableAddColumn {
            id: i.clone(),
            after: None,
            width: None,
        },
        Verb::TableRemoveColumn {
            id: i.clone(),
            rev: r,
            column: t(),
        },
        Verb::Move {
            ids: named(),
            dx: 1.0,
            dy: 1.0,
        },
        Verb::Order {
            ids: ids(),
            to: Layer::Front,
        },
        Verb::FrameOrder {
            id: i.clone(),
            to: Position::Front,
        },
        Verb::Group { ids: named() },
        Verb::Ungroup { ids: named() },
        Verb::Align {
            ids: named(),
            to: AlignTo::Left,
        },
        Verb::Distribute {
            ids: named(),
            axis: Axis::Horizontal,
        },
        Verb::Tidy { ids: named() },
        Verb::CopyStyle {
            from: i.clone(),
            to: named(),
        },
        Verb::Duplicate {
            ids: ids(),
            dx: 1.0,
            dy: 1.0,
        },
        Verb::Paste {
            objects: vec![object],
            x: 0.0,
            y: 0.0,
        },
        Verb::ConnectedCopy {
            id: i.clone(),
            side: Side::Top,
        },
        Verb::MindChild {
            parent: i.clone(),
            text: t(),
        },
        Verb::Template {
            name: "kanban".into(),
            x: 0.0,
            y: 0.0,
        },
        Verb::ClearBlob { hash: r },
    ]
}

#[test]
fn the_args_table_matches_the_verb_variants() {
    let samples = one_of_each();
    let sampled: BTreeSet<&str> = samples.iter().map(Verb::name).collect();
    let write: BTreeSet<&str> = verbs::WRITE_VERBS.iter().copied().collect();
    assert_eq!(sampled, write, "one sample of every verb plan takes");
    let mut problems = Vec::new();
    for v in &samples {
        let mut want = variant_args!(v;
            Add { kind, fields, at },
            Update { id, rev, set, unset },
            Delete { id, rev, cascade },
            PutBack { removed },
            Connect { from, to, fields },
            React { id, emoji; on },
            Vote { id, delta },
            Comment { text; target, point },
            Reply { id, text },
            Resolve { id, rev, resolved },
            Meta { rev, set, unset },
            Lock { ids, locked },
            CheckAdd { id, text },
            CheckSet { id, rev, item; text, done },
            CheckRemove { id, rev, item },
            TableCell { id, rev, row, column, text },
            TableAddRow { id; after },
            TableRemoveRow { id, rev, row },
            TableAddColumn { id; after, width },
            TableRemoveColumn { id, rev, column },
            Move { ids, dx, dy },
            Order { ids, to },
            FrameOrder { id, to },
            Group { ids },
            Ungroup { ids },
            Align { ids, to },
            Distribute { ids, axis },
            Tidy { ids },
            CopyStyle { from, to },
            Duplicate { ids, dx, dy },
            Paste { objects, x, y },
            ConnectedCopy { id, side },
            MindChild { parent, text },
            Template { name, x, y },
            ClearBlob { hash },
        );
        want.sort();
        let Some((_, have)) = verbs::ARGS.iter().find(|(n, _)| *n == v.name()) else {
            problems.push(format!("{}: not in verbs::ARGS", v.name()));
            continue;
        };
        let mut have: Vec<String> = have.iter().map(|a| a.to_string()).collect();
        have.sort();
        if have != want {
            problems.push(format!(
                "{}: ARGS has {have:?}, its variant {want:?}",
                v.name()
            ));
        }
    }
    let keys: Vec<&str> = verbs::ARGS.iter().map(|(n, _)| *n).collect();
    let unique: BTreeSet<&str> = keys.iter().copied().collect();
    if unique.len() != keys.len() {
        problems.push("verbs::ARGS names a verb twice".to_string());
    }
    if unique != pack_verbs() {
        problems.push(format!(
            "verbs::ARGS lists {unique:?}, the verbs are {:?}",
            pack_verbs()
        ));
    }
    assert!(
        problems.is_empty(),
        "{}",
        report("ARGS entries don't match", &problems)
    );
}
