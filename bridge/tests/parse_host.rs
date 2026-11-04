//! A host's command list, as neutral data (the board crate's `tests/data/host-cli-v1.json`), parses
//! through the bridge: each command's renames, aliases and positional order come in as [`Names`],
//! and every argument and flag the list gives a command parses. The bridge holds no host's names;
//! this test reads them from the data.

use k2_mpc_board_bridge::board::json::{self, Fields, Value};
use k2_mpc_board_bridge::board::verbs::{End, Verb};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::kit::errors::ErrorReport;
use k2_mpc_board_bridge::parse::{parse, Call, Names};
use k2_mpc_board_bridge::Why;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// What the host sets itself, by command and argument: the list says it in prose
/// (`pack.host_sets`), so the values are written here, and a test holds the two to the same keys.
const PRESETS: &[(&str, &str, &str)] = &[
    ("add", "at", "front"),
    ("frame", "kind", "frame"),
    ("frame", "at", "front"),
    ("delete", "cascade", "true"),
    ("vote", "delta", "1"),
];

fn data() -> Fields {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/board/tests/data/host-cli-v1.json");
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

fn member(f: &Fields, k: &str) -> Fields {
    match f.get(k) {
        Some(Value::Obj(o)) => o.clone(),
        _ => Fields::new(),
    }
}

fn texts(f: &Fields, k: &str) -> Vec<String> {
    match f.get(k) {
        Some(Value::Arr(a)) => a
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                other => panic!("{k} holds {other:?}, not text"),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// One command of the host's that runs a board verb.
struct Command {
    name: String,
    verb: String,
    args: Vec<String>,
    flags: Fields,
    renames: Fields,
    no_ops: BTreeSet<String>,
    sets: BTreeSet<String>,
}

fn commands(d: &Fields) -> Vec<Command> {
    let Some(Value::Arr(list)) = d.get("verbs") else {
        panic!("the host's list has no verbs");
    };
    let mut out = Vec::new();
    for v in list {
        let Value::Obj(entry) = v else {
            panic!("a verb that isn't an object: {v:?}")
        };
        let pack = member(entry, "pack");
        let Some(verb) = text(&pack, "verb") else {
            continue;
        };
        let no_ops = member(entry, "flag_verdicts")
            .iter()
            .filter(|(_, v)| matches!(v, Value::Obj(o) if text(o, "verdict") == Some("no_op")))
            .map(|(k, _)| k.clone())
            .collect();
        out.push(Command {
            name: text(entry, "name")
                .expect("check: a verb has a name")
                .to_string(),
            verb: verb.to_string(),
            args: texts(entry, "args"),
            flags: member(entry, "flags"),
            renames: member(&pack, "as"),
            no_ops,
            sets: member(&pack, "host_sets").keys().cloned().collect(),
        });
    }
    out
}

fn bare(a: &str) -> &str {
    a.trim_end_matches('?')
}

/// The host's names as data: a rename onto an argument the command also takes positionally,
/// from a word that isn't one of its own positional ones, fills that end's anchor (a connector
/// end named by object); every other rename is the argument itself.
fn names(d: &Fields) -> Names {
    let mut n = Names::default();
    let cmds = commands(d);
    for c in &cmds {
        if c.name != c.verb {
            n = n.verb(&c.name, &c.verb).expect("check: the command's verb");
        }
        let own: Vec<&str> = c.args.iter().map(|a| bare(a)).collect();
        for (word, to) in &c.renames {
            let Value::Str(to) = to else {
                panic!("{}: a rename to {to:?}", c.name)
            };
            n = if own.contains(&to.as_str()) && !own.contains(&word.as_str()) {
                n.anchor(&c.name, word, to)
                    .expect("check: an anchor rename")
            } else {
                n.arg(&c.name, word, to).expect("check: an argument rename")
            };
        }
        if !c.args.is_empty() {
            let order: Vec<&str> = c.args.iter().map(String::as_str).collect();
            n = n
                .positional(&c.name, &order)
                .expect("check: the positional order");
        }
        for (cmd, arg, value) in PRESETS {
            if *cmd == c.name {
                n = n.preset(&c.name, arg, value).expect("check: a preset");
            }
        }
        for flag in &c.no_ops {
            n = n
                .ignore(&c.name, flag)
                .expect("check: a flag that changes nothing");
        }
    }
    let mapped: BTreeSet<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
    for (alias, of) in member(d, "aliases") {
        let Value::Str(of) = of else { continue };
        if mapped.contains(of.as_str()) {
            n = n.alias(&alias, &of).expect("check: an alias");
        }
    }
    n
}

fn board() -> Board {
    let text = "{\"frames\":[],\"objects\":[\
{\"id\":\"s-1\",\"text\":\"one\",\"type\":\"sticky\"},\
{\"id\":\"s-2\",\"text\":\"two\",\"type\":\"sticky\"}],\
\"people\":{},\"settings\":{\"title\":\"T\"}}";
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the board")
}

/// A value for a positional argument of the host's, by its name.
fn positional_sample(arg: &str) -> &'static str {
    match arg {
        "id" | "from" => "s-1",
        "to" => "s-2",
        "text" => "one",
        "type" => "sticky",
        "name" => "kanban",
        "emoji" => "+1",
        "ops" => "[]",
        other => panic!("no sample for the host's argument {other}"),
    }
}

/// A flag's words for a command, by the kind the list gives.
fn flag_sample(c: &Command, flag: &str, kind: &str, rev: &str) -> Vec<String> {
    let w = |s: &str| s.to_string();
    match kind {
        "bool" if c.no_ops.contains(flag) => vec![format!("--{flag}")],
        "bool" => vec![format!("--{flag}"), w("true")],
        "number" => vec![format!("--{flag}"), w("1")],
        "rev" => vec![format!("--{flag}"), w(rev)],
        "text" => vec![format!("--{flag}"), w("top")],
        "settings" => vec![w("--title"), w("T2")],
        "fields" => match c.name.as_str() {
            "frame" => vec![w("--title"), w("F")],
            "connect" => vec![w("--label"), w("l")],
            _ => vec![w("--text"), w("hi")],
        },
        other => panic!(
            "{}: --{flag} takes {other}, no kind this test knows",
            c.name
        ),
    }
}

fn argv_of(c: &Command, rev: &str) -> Vec<String> {
    let mut out = vec![c.name.clone()];
    for a in &c.args {
        out.push(positional_sample(bare(a)).to_string());
    }
    for (flag, kind) in &c.flags {
        let Value::Str(kind) = kind else {
            panic!("{}: --{flag} has no kind", c.name)
        };
        out.extend(flag_sample(c, flag, kind, rev));
    }
    out
}

fn parsed(words: &[String], n: &Names) -> Result<Call, Why> {
    parse(words, n, &Limits::V1)
}

#[test]
fn the_presets_are_the_host_lists_own() {
    let d = data();
    let mut listed = BTreeSet::new();
    for c in commands(&d) {
        for arg in &c.sets {
            listed.insert((c.name.clone(), arg.clone()));
        }
    }
    let here: BTreeSet<(String, String)> = PRESETS
        .iter()
        .map(|(c, a, _)| (c.to_string(), a.to_string()))
        .collect();
    assert_eq!(
        here, listed,
        "a value here for each host_sets of the list's"
    );
}

#[test]
fn every_command_on_the_host_list_parses_with_every_argument_and_flag() {
    let d = data();
    let n = names(&d);
    let b = board();
    let rev = b.objects[0].rev().expect("check: a rev").to_string();
    let cmds = commands(&d);
    assert!(
        cmds.len() >= 10,
        "the list maps its commands: {}",
        cmds.len()
    );
    let mut problems = Vec::new();
    for c in &cmds {
        let words = argv_of(c, &rev);
        match parsed(&words, &n) {
            Ok(call) => {
                if call.name() != c.verb {
                    problems.push(format!("{words:?}: runs {}, not {}", call.name(), c.verb));
                }
                if let Err(e) = call.verb(&b) {
                    problems.push(format!("{words:?}: doesn't build: {e:?}"));
                }
            }
            Err(e) => problems.push(format!("{words:?}: {e:?}")),
        }
    }
    assert!(
        problems.is_empty(),
        "{} don't parse:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}

#[test]
fn every_alias_parses_as_the_command_it_names() {
    let d = data();
    let n = names(&d);
    let b = board();
    let rev = b.objects[0].rev().expect("check: a rev").to_string();
    let cmds: BTreeMap<String, Command> = commands(&d)
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();
    let mut seen = 0;
    for (alias, of) in member(&d, "aliases") {
        let Value::Str(of) = of else { continue };
        let Some(c) = cmds.get(&of) else { continue };
        let words = argv_of(c, &rev);
        let mut aliased = words.clone();
        aliased[0] = alias.clone();
        let want = parsed(&words, &n).unwrap_or_else(|e| panic!("{words:?}: {e:?}"));
        assert_eq!(parsed(&aliased, &n), Ok(want), "{alias} is {of}");
        seen += 1;
    }
    assert!(seen >= 3, "the list's aliases of mapped commands: {seen}");
}

#[test]
fn a_hosts_renames_and_positional_order_land_on_the_verbs_arguments() {
    let d = data();
    let n = names(&d);
    let b = board();
    let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let verb = |w: &[&str]| -> Verb {
        let call = parsed(&words(w), &n).unwrap_or_else(|e| panic!("{w:?}: {e:?}"));
        match call.verb(&b) {
            Ok(Some(v)) => v,
            other => panic!("{w:?} gives no verb: {other:?}"),
        }
    };
    let Verb::Comment { target, text, .. } = verb(&["comment", "s-1", "hi"]) else {
        panic!("comment isn't a comment");
    };
    assert_eq!(
        target.as_ref().map(Id::as_str),
        Some("s-1"),
        "the host's id is the target"
    );
    assert_eq!(text, "hi");
    let Verb::Connect { from, to, .. } = verb(&["connect", "s-1", "s-2", "--fromAnchor", "left"])
    else {
        panic!("connect isn't a connect");
    };
    assert_eq!(
        from,
        End::Object {
            id: Id::new("s-1").expect("an id"),
            anchor: Some("left".into())
        },
        "the anchor flag fills the from end's anchor"
    );
    assert_eq!(
        to,
        End::Object {
            id: Id::new("s-2").expect("an id"),
            anchor: None
        }
    );
    let Verb::Add { kind, at, .. } = verb(&["frame", "--title", "F"]) else {
        panic!("frame isn't an add");
    };
    assert_eq!(kind, ObjectType::Frame, "the host sets the type");
    assert_eq!(at, Position::Front, "the host sets where");
    let Verb::Vote { delta, .. } = verb(&["vote", "s-1"]) else {
        panic!("vote isn't a vote");
    };
    assert_eq!(delta, 1, "one vote each time");
    let call = parsed(&words(&["list", "--type", "sticky", "--full"]), &n)
        .expect("check: list with the host's flags");
    assert_eq!(
        call,
        Call::from_read(k2_mpc_board_bridge::parse::ReadVerb::List {
            kind: Some(ObjectType::Sticky)
        }),
        "--type is the kind, and --full changes nothing"
    );
}

/// The refusal, which must be the row `want`.
fn usage_refusal(r: Result<Call, Why>, what: &str, want: &str) -> ErrorReport {
    match r {
        Err(Why::Report(r)) => {
            assert_eq!(r.id(), want, "{what}: wanted {want}, got {}", r.id());
            r
        }
        other => panic!("{what}: wanted {want}, got no row: {other:?}"),
    }
}

#[test]
fn a_preset_can_be_given_as_it_is_and_not_otherwise() {
    let d = data();
    let n = names(&d);
    let w = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        parsed(&w(&["vote", "s-1", "--delta", "1"]), &n),
        parsed(&w(&["vote", "s-1"]), &n),
        "the preset value, given, is the same call"
    );
    let r = usage_refusal(
        parsed(&w(&["vote", "s-1", "--delta", "5"]), &n),
        "another delta",
        "MPC-E122",
    );
    assert_eq!(r.detail("arg").and_then(|v| v.as_str()), Some("delta"));
    usage_refusal(
        parsed(&w(&["frame", "--kind", "sticky"]), &n),
        "another type for frame",
        "MPC-E122",
    );
}

#[test]
fn names_that_do_not_hold_are_refused() {
    assert!(
        Names::default().verb("x", "frobnicate").is_err(),
        "no such verb"
    );
    assert!(
        Names::default().verb("read", "list").is_err(),
        "a board verb's own name"
    );
    assert!(
        Names::default().alias("x", "nothing").is_err(),
        "an alias of nothing"
    );
    assert!(
        Names::default().alias("read", "list").is_err(),
        "an alias that is a verb"
    );
    assert!(
        Names::default().arg("read", "x", "nope").is_err(),
        "no such argument"
    );
    assert!(
        Names::default().anchor("read", "x", "id").is_err(),
        "an id is no end"
    );
    assert!(
        Names::default().arg("update", "f", "set").is_err(),
        "the fields aren't one argument"
    );
    assert!(
        Names::default().positional("update", &["set"]).is_err(),
        "fields aren't positional"
    );
    assert!(
        Names::default().preset("move", "dx", "1").is_ok(),
        "a preset"
    );
    assert!(
        Names::default()
            .preset("move", "dx", "1")
            .and_then(|n| n.preset("move", "dx", "2"))
            .is_err(),
        "a preset twice"
    );
}
