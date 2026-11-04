//! The board verbs on a `.k5board`, through the pack's bridge
//! (`k2-mpc-board-bridge` v0.1.2): its parser makes the call from the command
//! line, and every refusal is one of the kit's rows (`MPC-E120`, `MPC-E122`,
//! `MPC-E830` to `MPC-E859`), whose exit the CLI gives as it is.
//!
//! **The names are the pack's** (`verbs::ARGS`): the parser reads them from that
//! table, so a name can't drift. What K2 adds is data, in this file:
//!
//! - [`names`]: K2's own flags, `--fromAnchor` and `--toAnchor` on `connect`.
//! - [`DEFAULTS`]: the values K2's command line has always filled in for an
//!   argument the pack requires (`add --at front`, `delete --cascade true`,
//!   `vote --delta 1`).
//! - [`k2_argv`]: K2's spellings from before the bridge, which the bridge's
//!   names can't say: `move <id>... --rev <rev>,<rev>` (one rev per id),
//!   `order <id>... --to …`, and `comment <id> <words>…`.
//!
//! `old_command_lines_mean_what_they_did` holds every command line K2's help
//! has shown to the call it made before the bridge, and
//! `every_pack_verb_runs_through_the_bridge` fails when the pack gains a verb
//! the bridge doesn't run.

use std::sync::OnceLock;

use k2_mpc_board::{verbs, Limits};
use k2_mpc_board_bridge::kit::errors::{lookup, ErrorReport, Value as KValue};
use k2_mpc_board_bridge::parse::{self, Call, Names};
use k2_mpc_board_bridge::{HostBug, Why};

/// The verbs `k2 board` answers on K2's own routes, with K2's output: `list`,
/// `read` and `search` (`/read`), `fields` (`/fields`) and `apply` (`/save`:
/// your ids kept, groups, held edits). Through `/verb` the bridge answers them
/// too, by the pack's rules; the CLI sends every other verb there.
pub const K2_ROUTES: &[&str] = &["list", "read", "search", "fields", "apply"];

/// Values K2's command line fills in when an argument the pack requires isn't
/// given, by flag or by place: `(verb, argument, value)`.
pub const DEFAULTS: &[(&str, &str, &str)] = &[("add", "at", "front"), ("delete", "cascade", "true"), ("vote", "delta", "1")];

/// Arguments that hold `--<field> <text>` pairs, never a value of their own.
const FIELDS_ARGS: &[&str] = &["fields", "set", "unset"];

/// K2's names for the bridge's parser: the board's own, plus `connect`'s
/// `--fromAnchor` and `--toAnchor`.
pub fn names() -> &'static Names {
    static N: OnceLock<Names> = OnceLock::new();
    N.get_or_init(|| {
        Names::default()
            .anchor("connect", "fromAnchor", "from")
            .and_then(|n| n.anchor("connect", "toAnchor", "to"))
            .expect("K2's names for the board bridge hold")
    })
}

/// A command line, from the verb on, as the bridge's call. K2's flags (the
/// file, `--json`, `--dry-run`) are already gone.
pub fn call_of(argv: &[String]) -> Result<Call, Why> {
    let argv = k2_argv(argv)?;
    parse::parse(&argv, names(), &Limits::V1)
}

/// One word of a command line, read the bridge's way: a flag always takes the
/// next word as its value, and after `--` every word is positional.
#[derive(Debug, Clone, PartialEq)]
enum Word {
    Flag(String, Option<String>),
    Pos(String),
}

fn words(rest: &[String]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut it = rest.iter();
    while let Some(tok) = it.next() {
        if tok == "--" {
            out.extend(it.by_ref().map(|t| Word::Pos(t.clone())));
            break;
        }
        match tok.strip_prefix("--").filter(|f| !f.is_empty()) {
            Some(flag) => match flag.split_once('=') {
                Some((n, v)) => out.push(Word::Flag(n.to_string(), Some(v.to_string()))),
                None => out.push(Word::Flag(flag.to_string(), it.next().cloned())),
            },
            None => out.push(Word::Pos(tok.clone())),
        }
    }
    out
}

/// MPC-E122 in the bridge's shape: the verb, the argument, and the reason in
/// the bridge's words; a value the caller gave is never in the message.
fn usage(verb: &str, arg: &str, reason: &str) -> Why {
    let Some(entry) = lookup("MPC-E122") else {
        return Why::HostBug(HostBug::NoRow { id: "MPC-E122" });
    };
    let message = format!("{verb}: {reason}: {arg}");
    Why::Report(
        ErrorReport::new(entry, message.clone())
            .with("reason", KValue::Str(message))
            .with("verb", KValue::Str(verb.to_string()))
            .with("arg", KValue::Str(arg.to_string())),
    )
}

/// The pack's required arguments a word in place fills, in order.
fn slots(verb: &str) -> Vec<&'static str> {
    verbs::ARGS
        .iter()
        .find(|(n, _)| *n == verb)
        .map(|(_, args)| args.iter().copied().filter(|a| !a.ends_with('?') && !FIELDS_ARGS.contains(a)).collect())
        .unwrap_or_default()
}

/// K2's spellings to the bridge's: see the module's notes. Everything else
/// passes as it came, for the bridge to read or refuse.
pub fn k2_argv(argv: &[String]) -> Result<Vec<String>, Why> {
    let Some((verb, rest)) = argv.split_first() else {
        return Ok(argv.to_vec());
    };
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let mut pos: Vec<String> = Vec::new();
    for w in words(rest) {
        match w {
            Word::Flag(n, v) => flags.push((n, v)),
            Word::Pos(p) => pos.push(p),
        }
    }
    let has = |flags: &[(String, Option<String>)], n: &str| flags.iter().any(|(f, _)| f == n);
    match verb.as_str() {
        // `move <id>... --rev <rev>[,<rev>...]`: one rev per id, in order.
        "move" if has(&flags, "rev") && !has(&flags, "ids") => {
            let revs: Vec<String> = flags
                .iter()
                .find(|(f, _)| f == "rev")
                .and_then(|(_, v)| v.clone())
                .map(|v| v.split(',').map(|r| r.trim().to_string()).collect())
                .unwrap_or_default();
            if pos.is_empty() {
                return Err(usage("move", "ids", "name the objects to move"));
            }
            if revs.len() != pos.len() || revs.iter().any(String::is_empty) {
                return Err(usage("move", "rev", "one rev for each id, in the same order"));
            }
            let ids: Vec<String> = pos.iter().zip(&revs).map(|(i, r)| format!("{i}@{r}")).collect();
            flags.retain(|(f, _)| f != "rev");
            flags.push(("ids".to_string(), Some(ids.join(","))));
            pos.clear();
        }
        // `order <id>... --to …`: the ids, in one list.
        "order" if pos.len() > 1 && has(&flags, "to") && !has(&flags, "ids") => {
            let ids = pos.join(",");
            pos = vec![ids];
        }
        // `comment <id> <words>…`, or `comment --point x,y <words>…`.
        "comment" if !has(&flags, "text") && !pos.is_empty() => {
            if !has(&flags, "point") && !has(&flags, "target") && pos.len() > 1 {
                let target = pos.remove(0);
                flags.push(("target".to_string(), Some(target)));
            }
            if has(&flags, "point") || has(&flags, "target") {
                flags.push(("text".to_string(), Some(pos.join(" "))));
                pos.clear();
            }
        }
        // `comment <id> --text <words>`: the word in place is the target.
        "comment" if has(&flags, "text") && !has(&flags, "target") && !has(&flags, "point") && pos.len() == 1 => {
            flags.push(("target".to_string(), pos.pop()));
        }
        _ => {}
    }
    // K2's defaults, for a required argument neither a flag nor a word in place gives.
    let free: Vec<&str> = slots(verb).into_iter().filter(|a| !has(&flags, a)).collect();
    for (v, arg, value) in DEFAULTS {
        if v == verb && free.iter().skip(pos.len()).any(|a| a == arg) {
            flags.push((arg.to_string(), Some(value.to_string())));
        }
    }
    let mut out = vec![verb.clone()];
    for (n, v) in flags {
        out.push(format!("--{n}"));
        out.extend(v);
    }
    if !pos.is_empty() {
        out.push("--".to_string());
        out.extend(pos);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_mpc_board::verbs::{End, Layer, ObjRev, Verb};
    use k2_mpc_board::{json as fj, Board, Id, ObjectType, Position, Sha256};
    use k2_mpc_board_bridge::parse::CallKind;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    fn board() -> Board {
        Board::from_json(br#"{"objects":[{"id":"s-1","type":"sticky"},{"id":"s-2","type":"sticky"}],"frames":[],"people":{},"settings":{}}"#, &Limits::V1).expect("board")
    }

    fn verb(words: &[&str]) -> Verb {
        let call = call_of(&argv(words)).unwrap_or_else(|e| panic!("{words:?}: {e:?}"));
        call.verb(&board()).unwrap_or_else(|e| panic!("{words:?}: {e:?}")).unwrap_or_else(|| panic!("{words:?}: no verb"))
    }

    fn id(s: &str) -> Id {
        Id::new(s).expect("id")
    }

    fn refused(words: &[&str]) -> (&'static str, u8) {
        match call_of(&argv(words)) {
            Ok(c) => panic!("{words:?} parsed: {c:?}"),
            Err(Why::Report(r)) => (r.id(), r.exit()),
            Err(other) => panic!("{words:?}: {other:?}"),
        }
    }

    /// Every command line K2's help showed before the bridge still makes the
    /// call it made then, by the names agents use: the guarantee the old
    /// hand-wiring's drift test gave.
    #[test]
    fn old_command_lines_mean_what_they_did() {
        let rev = Sha256::of(b"r").to_string();
        let r = Sha256::parse(&rev).expect("rev");
        let mut fields = fj::Fields::new();
        fields.insert("text".into(), fj::Value::Str("Ship it".into()));
        fields.insert("x".into(), fj::Value::Num(1.25));
        assert_eq!(verb(&["add", "sticky", "--text", "Ship it", "--x", "1.25"]), Verb::Add { kind: ObjectType::parse("sticky").expect("t"), fields: fields.clone(), at: Position::Front });
        assert_eq!(verb(&["add", "sticky", "--text", "Ship it", "--x", "1.25", "--at", "back"]), Verb::Add { kind: ObjectType::parse("sticky").expect("t"), fields, at: Position::Back });
        let mut set = fj::Fields::new();
        set.insert("text".into(), fj::Value::Str("new".into()));
        assert_eq!(verb(&["update", "s-1", "--rev", &rev, "--text", "new"]), Verb::Update { id: id("s-1"), rev: r, set: set.clone(), unset: vec![] });
        assert_eq!(verb(&["delete", "s-1", "--rev", &rev]), Verb::Delete { id: id("s-1"), rev: r, cascade: true });
        assert_eq!(verb(&["delete", "s-1", "--rev", &rev, "--cascade", "false"]), Verb::Delete { id: id("s-1"), rev: r, cascade: false });
        let mut label = fj::Fields::new();
        label.insert("label".into(), fj::Value::Str("next".into()));
        assert_eq!(
            verb(&["connect", "s-1", "s-2", "--toAnchor", "left", "--label", "next"]),
            Verb::Connect { from: End::Object { id: id("s-1"), anchor: None }, to: End::Object { id: id("s-2"), anchor: Some("left".into()) }, fields: label }
        );
        assert_eq!(
            verb(&["connect", "s-1", "1.5,-2", "--fromAnchor", "top"]),
            Verb::Connect { from: End::Object { id: id("s-1"), anchor: Some("top".into()) }, to: End::Point { x: 1.5, y: -2.0 }, fields: fj::Fields::new() }
        );
        assert_eq!(verb(&["react", "s-1", "👍"]), Verb::React { id: id("s-1"), emoji: "👍".into(), on: None });
        assert_eq!(verb(&["react", "s-1", "👍", "--on", "false"]), Verb::React { id: id("s-1"), emoji: "👍".into(), on: Some(false) });
        assert_eq!(verb(&["vote", "s-1"]), Verb::Vote { id: id("s-1"), delta: 1 });
        assert_eq!(verb(&["vote", "s-1", "--delta", "-1"]), Verb::Vote { id: id("s-1"), delta: -1 });
        assert_eq!(verb(&["comment", "s-1", "why", "this?"]), Verb::Comment { target: Some(id("s-1")), text: "why this?".into(), point: None });
        assert_eq!(verb(&["comment", "s-1", "why this?"]), Verb::Comment { target: Some(id("s-1")), text: "why this?".into(), point: None });
        assert_eq!(verb(&["comment", "--point", "5,5", "a", "note"]), Verb::Comment { target: None, text: "a note".into(), point: Some((5.0, 5.0)) });
        assert_eq!(verb(&["comment", "s-1", "--text", "why"]), Verb::Comment { target: Some(id("s-1")), text: "why".into(), point: None });
        let mut title = fj::Fields::new();
        title.insert("title".into(), fj::Value::Str("Q4 plan".into()));
        assert_eq!(verb(&["meta", "--rev", &rev, "--title", "Q4 plan"]), Verb::Meta { rev: r, set: title, unset: vec![] });
        assert_eq!(
            verb(&["move", "s-1", "s-2", "--rev", &format!("{rev},{rev}"), "--dx", "40", "--dy", "-0"]),
            Verb::Move { ids: vec![ObjRev { id: id("s-1"), rev: r }, ObjRev { id: id("s-2"), rev: r }], dx: 40.0, dy: -0.0 }
        );
        assert_eq!(verb(&["order", "s-1", "s-2", "--to", "back"]), Verb::Order { ids: vec![id("s-1"), id("s-2")], to: Layer::Back });
        assert_eq!(verb(&["order", "s-1", "--to", "after:s-2"]), Verb::Order { ids: vec![id("s-1")], to: Layer::After(vec![id("s-2")]) });
        // The bridge's own forms work too.
        assert_eq!(verb(&["move", &format!("s-1@{rev}"), "--dx", "1", "--dy", "2"]), Verb::Move { ids: vec![ObjRev { id: id("s-1"), rev: r }], dx: 1.0, dy: 2.0 });
        assert_eq!(verb(&["add", "sticky", "back"]), Verb::Add { kind: ObjectType::parse("sticky").expect("t"), fields: fj::Fields::new(), at: Position::Back });
        assert_eq!(verb(&["connect", "s-1:top", "s-2"]), Verb::Connect { from: End::Object { id: id("s-1"), anchor: Some("top".into()) }, to: End::Object { id: id("s-2"), anchor: None }, fields: fj::Fields::new() });
    }

    #[test]
    fn k2_spellings_refuse_as_the_kits_rows() {
        let rev = Sha256::of(b"r").to_string();
        // one rev per id
        assert_eq!(refused(&["move", "s-1", "--rev", &format!("{rev},{rev}"), "--dx", "1", "--dy", "1"]), ("MPC-E122", 2));
        assert_eq!(refused(&["move", "--rev", &rev, "--dx", "1", "--dy", "1"]), ("MPC-E122", 2));
        // the bridge's own: an unknown verb, an unknown flag, a missing argument, a bad value
        assert_eq!(refused(&["frobnicate"]), ("MPC-E120", 2));
        assert_eq!(refused(&["delete", "s-1", "--rev", &rev, "--colour", "red"]), ("MPC-E120", 2));
        assert_eq!(refused(&["update", "s-1", "--text", "x"]), ("MPC-E122", 2));
        assert_eq!(refused(&["update", "s-1", "--rev", "nope", "--text", "x"]), ("MPC-E122", 2));
        assert_eq!(refused(&["vote", "s-1", "--delta", "1.5"]), ("MPC-E122", 2));
        assert_eq!(refused(&["order", "s-1", "--to", "sideways"]), ("MPC-E122", 2));
        assert_eq!(refused(&["add", "hologram"]), ("MPC-E851", 2), "a type the format lacks: its own row");
        assert_eq!(refused(&[]), ("MPC-E122", 2));
    }

    /// The exits K2 agreed with K2 Cores (11-04), as the kit's rows give them.
    /// A row that moves fails here: the CLI gives each row's exit as it is.
    #[test]
    fn the_kits_rows_give_the_agreed_exits() {
        let agreed: &[(u8, &[&str])] = &[
            (1, &["MPC-E830", "MPC-E831", "MPC-E832", "MPC-E833", "MPC-E834", "MPC-E856", "MPC-E857", "MPC-E858", "MPC-E859"]),
            (2, &["MPC-E120", "MPC-E122", "MPC-E847", "MPC-E848", "MPC-E849", "MPC-E850", "MPC-E851", "MPC-E852", "MPC-E853", "MPC-E854", "MPC-E855"]),
            (4, &["MPC-E838", "MPC-E839", "MPC-E840", "MPC-E841", "MPC-E842", "MPC-E843", "MPC-E844", "MPC-E845", "MPC-E846"]),
            (5, &["MPC-E835", "MPC-E836", "MPC-E837"]),
        ];
        let names = [("MPC-E830", "conflict"), ("MPC-E831", "exists"), ("MPC-E844", "newer_format"), ("MPC-E835", "locked"), ("MPC-E836", "voting_off"), ("MPC-E837", "no_votes_left")];
        for (exit, ids) in agreed {
            for id in *ids {
                let e = lookup(id).unwrap_or_else(|| panic!("the kit has no row {id}"));
                assert_eq!(e.exit, *exit, "{id} ({})", e.name);
            }
        }
        for (id, name) in names {
            assert_eq!(lookup(id).expect("row").name, name);
        }
        let listed: usize = agreed.iter().map(|(_, ids)| ids.len()).sum();
        assert_eq!(listed, 30 + 2, "every board row (E830 to E859) and the two usage rows");
    }

    /// Every verb the pack lists parses through K2's names (by its own example
    /// line), and is a read or a change as the bridge's registry says.
    #[test]
    fn every_pack_verb_runs_through_the_bridge() {
        for (name, _) in verbs::ARGS {
            let ex = parse::example(name).unwrap_or_else(|| panic!("the bridge has no example for {name}"));
            let call = call_of(&ex).unwrap_or_else(|e| panic!("{name}: {ex:?}: {e:?}"));
            let edit = k2_mpc_board_bridge::registry::kind_of(name) == k2_mpc_board_bridge::kit::parity::VerbKind::Edit;
            assert!(edit || call.kind() == CallKind::Read, "{name}");
        }
        for v in K2_ROUTES {
            assert!(verbs::ARGS.iter().any(|(n, _)| n == v), "{v} is the pack's verb");
        }
        for (v, arg, _) in DEFAULTS {
            assert!(slots(v).contains(arg), "{v} {arg}: a default only for an argument the pack requires");
        }
        names();
    }

    #[test]
    fn k2_argv_rewrites_only_k2s_spellings() {
        assert_eq!(k2_argv(&argv(&["templates"])).expect("argv"), argv(&["templates"]));
        assert_eq!(k2_argv(&argv(&["meta"])).expect("argv"), argv(&["meta"]), "meta with nothing reads the settings");
        assert_eq!(k2_argv(&argv(&["vote", "s-1"])).expect("argv"), argv(&["vote", "--delta", "1", "--", "s-1"]));
        assert_eq!(k2_argv(&argv(&["vote", "s-1", "--delta=-1"])).expect("argv"), argv(&["vote", "--delta", "-1", "--", "s-1"]));
        assert_eq!(k2_argv(&argv(&["add", "sticky", "back"])).expect("argv"), argv(&["add", "--", "sticky", "back"]), "a word in place fills at: no default");
        assert_eq!(k2_argv(&argv(&["tidy", "--", "--x"])).expect("argv"), argv(&["tidy", "--", "--x"]));
    }
}
