//! What an agent gets back from a keyed pack's verbs, as data: the verb registry lists a keyed
//! pack's structured verbs (tier `keyed`), a pack's refusals reach the host by id (the pack's
//! registered entries, or the text form's stop-gap), and a call's result as data
//! (`mpc.result/1`: versions, changed units by address and field, the error, conflicts by
//! field). Public API only. The results' goldens are `tests/golden/results/` (`MPC_BLESS=1`
//! writes them). Each plant of `MPC_ERGO_MUTANT` (`k2_mpc_base::differential::ErgoMutant`) fails
//! the rows named at it, at their own assertions.
//!
//! The test pack, `deck`: a MovableList `shapes` and a Map `fields` keyed
//! `<shape>\x1f<field>` (a composite map, [`Pack::composite`]); a shape's address is `@s:<key>`.
//! The file is one line per shape: its key, then `\t<field>=<value>` per field.

#[path = "grid_pack/mod.rs"]
mod grid_pack;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::errors::{capabilities, run_meta, ErrorEntry, Value as Json};
use k2_mpc_base::parity::{PersonAction, Registry, Source};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{
    PackEdit, PackVerb, StructEdit, StructOutput, ToolError, ToolOptions, ToolOutput, RESULT_SCHEMA,
};

// ---------- the test pack ----------

const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";
const SEP: char = '\u{1f}';

const DECK: Schema = Schema {
    name: "deck",
    roots: &[(SHAPES, Kind::MovableList), (FIELDS, Kind::Map)],
};

struct Deck;

fn bad(message: String) -> Declined {
    Declined {
        code: "malformed",
        message,
    }
}

impl Pack for Deck {
    fn schema(&self) -> Schema {
        DECK
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        let mut list = Vec::new();
        let mut fields = BTreeMap::new();
        for line in s.lines() {
            let mut parts = line.split('\t');
            let key = Key::new(parts.next().unwrap_or("")).map_err(|e| bad(e.to_string()))?;
            for p in parts {
                let (f, v) = p
                    .split_once('=')
                    .ok_or_else(|| bad(format!("no = in {p:?}")))?;
                fields.insert(format!("{}{SEP}{f}", key.as_str()), Value::str(v));
            }
            list.push((key, Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                (SHAPES.into(), Neutral::List(list)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut shapes: &[(Key, Value)] = &[];
        let empty = BTreeMap::new();
        let mut fields = &empty;
        for (r, n) in c {
            match n {
                Neutral::List(l) if r == SHAPES => shapes = l,
                Neutral::Map(m) if r == FIELDS => fields = m,
                _ => return Err(PackError(format!("unexpected root {r}"))),
            }
        }
        let mut out = String::new();
        for (k, _) in shapes {
            out.push_str(k.as_str());
            let prefix = format!("{}{SEP}", k.as_str());
            for (fk, v) in fields.range(prefix.clone()..) {
                let Some(f) = fk.strip_prefix(&prefix) else {
                    break;
                };
                out.push('\t');
                out.push_str(f);
                out.push('=');
                out.push_str(v.as_str().unwrap_or(""));
            }
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(FIELDS, SHAPES)]
    }
    fn address_of(&self, _root: &str, key: &Key) -> String {
        format!("@s:{}", key.as_str())
    }
    fn key_of(&self, _root: &str, address: &str) -> Option<Key> {
        Key::new(address.strip_prefix("@s:")?).ok()
    }
    fn errors(&self) -> &'static [ErrorEntry] {
        ERRORS
    }
}

/// The deck's own refusals.
static ERRORS: &[ErrorEntry] = &[
    ErrorEntry::pack(
        "DECK-E001",
        "shape_locked",
        2,
        "shape {shape} is locked",
        "Someone locked the shape in the app. Nothing was applied. Work on another shape.",
    )
    .step("shape get", &["{shape}"], "Read the shape; it is locked."),
    ErrorEntry::pack(
        "DECK-E002",
        "shape_frozen",
        2,
        "shape {shape} is frozen",
        "The shape is frozen. Nothing was applied.",
    ),
];

fn usage(m: &str) -> ToolError {
    ToolError::usage(m.to_owned())
}

/// Whether the view holds shape `shape`.
fn known(ctx: &k2_mpc_base::tools::VerbCtx<'_>, shape: &str) -> bool {
    ctx.view()
        .list(SHAPES)
        .unwrap_or(&[])
        .iter()
        .any(|(k, _)| k.as_str() == shape)
}

/// `shape get <shape>`: one shape's fields, `<field>=<value>` per line (exit 4: no such shape).
fn shape_get(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape get <shape>"));
    };
    let shape = shape.clone();
    Ok(Box::new(move |ctx| {
        if !known(ctx, &shape) {
            return Ok(StructOutput::new(
                Vec::new(),
                format!("not found: @s:{shape}\n").into_bytes(),
                4,
            ));
        }
        let prefix = format!("{shape}{SEP}");
        let empty = BTreeMap::new();
        let mut out = String::new();
        for (k, v) in ctx
            .view()
            .map(FIELDS)
            .unwrap_or(&empty)
            .range(prefix.clone()..)
        {
            let Some(f) = k.strip_prefix(&prefix) else {
                break;
            };
            out.push_str(&format!("{f}={}\n", v.as_str().unwrap_or("")));
        }
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0))
    }))
}

/// `shape fields <shape>`: one shape's fields as typed data, `{address, fields}`, its shape named
/// `deck.shape-fields/1`; the same `<field>=<value>` lines as text (exit 4: no such shape).
fn shape_fields(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape fields <shape>"));
    };
    let shape = shape.clone();
    Ok(Box::new(move |ctx| {
        if !known(ctx, &shape) {
            return Ok(StructOutput::new(
                Vec::new(),
                format!("not found: @s:{shape}\n").into_bytes(),
                4,
            ));
        }
        let prefix = format!("{shape}{SEP}");
        let empty = BTreeMap::new();
        let mut out = String::new();
        let mut fields = Vec::new();
        for (k, v) in ctx
            .view()
            .map(FIELDS)
            .unwrap_or(&empty)
            .range(prefix.clone()..)
        {
            let Some(f) = k.strip_prefix(&prefix) else {
                break;
            };
            let v = v.as_str().unwrap_or("");
            out.push_str(&format!("{f}={v}\n"));
            fields.push((f.to_owned(), Json::from(v)));
        }
        let data = Json::obj([
            ("address", Json::from(format!("@s:{shape}"))),
            ("fields", Json::Obj(fields)),
        ]);
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0)
            .data(data)
            .data_spec("deck.shape-fields", 1))
    }))
}

/// `shape set <shape> <field> <value>`: one field (exit 4: no such shape).
fn shape_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, field, value] = args else {
        return Err(usage("shape set <shape> <field> <value>"));
    };
    let (shape, field, value) = (shape.clone(), field.clone(), value.clone());
    Ok(Box::new(move |ctx| {
        if !known(ctx, &shape) {
            return Ok(StructOutput::new(
                Vec::new(),
                format!("not found: @s:{shape}\n").into_bytes(),
                4,
            ));
        }
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: format!("{shape}{SEP}{field}"),
                    value: Value::str(&value),
                }],
            }],
            format!("set @s:{shape} {field}\n").into_bytes(),
            0,
        ))
    }))
}

/// `shape add <shape>`: one element, at the end.
fn shape_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape add <shape>"));
    };
    let key = Key::new(shape).map_err(|e| usage(&e.to_string()))?;
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            vec![Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Insert {
                    key: key.clone(),
                    at: Anchor::End,
                    value: Value::Null,
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape old <shape>`: always refused, in the text form packs print today (a code no entry of
/// the pack's registers), exit 2.
fn shape_old(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape old <shape>"));
    };
    let shape = shape.clone();
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            Vec::new(),
            format!(
                "refused: deck_legacy: @s:{shape} is from an old deck\nnext: shape get @s:{shape}\n"
            )
            .into_bytes(),
            2,
        ))
    }))
}

/// `shape lock <shape>`: always refused, by the pack's registered id (`DECK-E001`).
fn shape_lock(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape lock <shape>"));
    };
    let shape = shape.clone();
    Ok(Box::new(move |_| {
        Ok(StructOutput::refused(
            &ERRORS[0],
            format!("@s:{shape} is locked"),
            format!("shape get @s:{shape}"),
        )
        .detail("shape", format!("@s:{shape}")))
    }))
}

/// `shape frozen <shape>`: always refused, in the text form, with a code the pack registers
/// (`shape_frozen`, `DECK-E002`), exit 2.
fn shape_frozen(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape] = args else {
        return Err(usage("shape frozen <shape>"));
    };
    let shape = shape.clone();
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            Vec::new(),
            format!("refused: shape_frozen: @s:{shape} is frozen\nnext: shape get @s:{shape}\n")
                .into_bytes(),
            2,
        ))
    }))
}

const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "shape get",
        "shape get <shape>",
        &[PersonAction::ReadAll],
        &[0, 4],
        shape_get,
    )
    .positions(&[]),
    PackVerb::structured_read(
        "shape fields",
        "shape fields <shape>",
        &[PersonAction::ReadAll],
        &[0, 4],
        shape_fields,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape set",
        "shape set <shape> <field> <value>",
        &[PersonAction::TypeOver],
        &[0, 2, 4],
        shape_set,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape add",
        "shape add <shape>",
        &[PersonAction::TypeOver],
        &[0, 2],
        shape_add,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape old",
        "shape old <shape>",
        &[PersonAction::TypeOver],
        &[2],
        shape_old,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape lock",
        "shape lock <shape>",
        &[PersonAction::TypeOver],
        &[2],
        shape_lock,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape frozen",
        "shape frozen <shape>",
        &[PersonAction::TypeOver],
        &[2],
        shape_frozen,
    )
    .positions(&[]),
];

// ---------- harness ----------

struct TestHost;

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        // 2026-10-27 14:30:00 UTC
        1_793_111_400_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    s: Arc<Session>,
    _mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const SEED: &str = "s1\tcolor=blue\tw=10\ns2\tcolor=red\n";

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-agent-results-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(TestHost), Config::default());
    let s = mgr
        .open(
            Arc::new(Deck),
            &DocKey {
                space: "ws".into(),
                path: "talk.deck".into(),
            },
            Some(SEED.as_bytes()),
        )
        .unwrap();
    Fx { dir, s, _mgr: mgr }
}

fn agent(s: &Session, pid: &str) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn argv(a: &str) -> Vec<String> {
    a.split(' ').map(str::to_owned).collect()
}

fn version(f: &Fx) -> u64 {
    f.s.status().unwrap().version
}

/// A detail of a report, as a string.
fn detail<'a>(r: &'a k2_mpc_base::errors::ErrorReport, key: &str) -> Option<&'a str> {
    r.detail(key).and_then(|v| v.as_str())
}

// ---------- the keyed tier: the registry takes a keyed pack's structured verbs ----------

/// `Registry::for_pack` on a keyed pack (not a table) lists every structured verb it declares,
/// as a pack verb of the keyed tier; `capabilities` names that tier `keyed`.
#[test]
fn registry_takes_keyed_structured_verbs() {
    let r = Registry::for_pack(&Deck);
    assert!(
        r.is_ok(),
        "a keyed pack's structured verbs are refused: {:?}",
        r.err()
    );
    let r = r.unwrap();
    for v in VERBS {
        let spec = r
            .get(v.name())
            .unwrap_or_else(|| panic!("{} is not listed", v.name()));
        assert_eq!(spec.source, Source::Pack, "{}", v.name());
    }
    let caps = capabilities(&[&r], &Config::default());
    let verbs = match caps.get("verbs") {
        Some(k2_mpc_base::errors::Value::List(l)) => l.clone(),
        other => panic!("no verbs list: {other:?}"),
    };
    let row = verbs
        .iter()
        .find(|v| v.get("name").and_then(|n| n.as_str()) == Some("shape set"))
        .expect("shape set in capabilities");
    assert_eq!(
        row.get("tiers").map(|t| t.to_json()),
        Some("[\"keyed\"]".to_owned()),
        "a keyed pack's verb's tier"
    );
    // the kit's verbs on a doc's presence and records are there; reads of a text root, the
    // text tools and the grid's own are not
    for n in ["open", "who", "log", "status"] {
        assert!(r.get(n).is_some(), "{n} on a keyed doc");
    }
    for n in [
        "cat",
        "sed",
        "insert",
        "patch",
        "rows drop-deleted",
        "lease",
    ] {
        assert!(r.get(n).is_none(), "{n} on a keyed doc");
    }
}

/// A keyed pack takes structured verbs only: a text verb on one is refused, by name.
#[test]
fn registry_refuses_a_text_verb_on_a_keyed_pack() {
    fn text_noop(_: &[String]) -> Result<PackEdit, ToolError> {
        Err(ToolError::usage("never"))
    }
    struct WithText;
    impl Pack for WithText {
        fn schema(&self) -> Schema {
            DECK
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            Deck.decode(b)
        }
        fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
            Deck.encode(c, codec)
        }
        fn verbs(&self) -> &'static [PackVerb] {
            const V: &[PackVerb] = &[PackVerb::edit(
                "shout",
                "shout",
                &[PersonAction::Format],
                &[0],
                text_noop,
            )];
            V
        }
    }
    let e = Registry::for_pack(&WithText).unwrap_err();
    assert!(e.to_string().contains("a text verb on a keyed pack"), "{e}");
}

/// The keyed doc runs the verbs the registry lists: an agent's `shape set` applies.
#[test]
fn a_listed_keyed_verb_runs() {
    let f = fx("runs");
    let a = agent(&f.s, "bot");
    let v = f.s.status().unwrap().version;
    let o =
        f.s.run_tool(
            a,
            FIELDS,
            &argv(&format!("shape set s1 color red --if-version {v}")),
            b"",
        )
        .expect("the verb runs");
    assert_eq!(o.exit, 0, "{}", String::from_utf8_lossy(&o.stdout));
    let o =
        f.s.run_tool(a, FIELDS, &argv("shape get s1"), b"")
            .expect("the verb runs");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "color=red\nw=10\n");
    // `cat` reads a text root, which a keyed doc has none of: refused, so the keyed tier
    // doesn't list it
    let e = f.s.run_tool(a, FIELDS, &argv("cat"), b"").err();
    assert!(e.is_some(), "cat on a keyed doc");
}

// ---------- A: a pack's refusals as data ----------

/// A verb's refusal by the pack's registered id reaches the host as a report with that id, its
/// message and details, and the text and exit stay what the pack prints.
#[test]
fn pack_refusal_has_report() {
    let f = fx("refusal");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o =
        f.s.run_tool(
            a,
            FIELDS,
            &argv(&format!("shape lock s1 --if-version {v}")),
            b"",
        )
        .expect("the verb runs");
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "refused: shape_locked: @s:s1 is locked\nnext: shape get @s:s1\n",
        "the text is the pack's, byte for byte"
    );
    assert_eq!(o.exit, 2);
    let r = o.refusal.as_ref().expect("a pack's refusal is a report");
    assert_eq!(r.id(), "DECK-E001", "the pack's registered id");
    assert!(
        o.stderr_summary.ends_with("(DECK-E001)"),
        "{}",
        o.stderr_summary
    );
    assert_eq!(r.exit(), o.exit);
    assert_eq!(r.message(), "@s:s1 is locked");
    assert_eq!(detail(r, "code"), Some("shape_locked"));
    assert_eq!(detail(r, "next"), Some("shape get @s:s1"));
    assert_eq!(detail(r, "verb"), Some("shape lock"));
    assert_eq!(detail(r, "shape"), Some("@s:s1"), "the pack's own detail");
    assert_eq!(
        r.next_step().map(|s| s.verb),
        Some("shape get"),
        "the entry's next step"
    );
    assert_eq!(version(&f), v, "nothing applied");
}

/// The stop-gap: a verb that prints `refused: <code>: <why>` itself, exit 2, is reported under
/// the pack's entry named `<code>`, or as `MPC-E823` with the code and its `next` line as
/// details when the pack registers none of that name. The text and the exit are unchanged.
#[test]
fn pack_refusal_stopgap_parses() {
    let f = fx("stopgap");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o =
        f.s.run_tool(
            a,
            FIELDS,
            &argv(&format!("shape old s1 --if-version {v}")),
            b"",
        )
        .expect("the verb runs");
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "refused: deck_legacy: @s:s1 is from an old deck\nnext: shape get @s:s1\n"
    );
    assert_eq!(o.exit, 2);
    let r = o.refusal.as_ref().expect("a pack's refusal is a report");
    assert_eq!(
        r.id(),
        "MPC-E823",
        "an unregistered code: the kit's stop-gap id"
    );
    assert_eq!(r.exit(), o.exit);
    assert_eq!(r.message(), "@s:s1 is from an old deck");
    assert_eq!(detail(r, "code"), Some("deck_legacy"));
    assert_eq!(detail(r, "next"), Some("shape get @s:s1"));
    assert_eq!(detail(r, "verb"), Some("shape old"));
    assert!(
        o.stderr_summary.ends_with("(MPC-E823)"),
        "{}",
        o.stderr_summary
    );
    // a code the pack registers: its entry, though the verb prints the text itself
    let o =
        f.s.run_tool(
            a,
            FIELDS,
            &argv(&format!("shape frozen s1 --if-version {v}")),
            b"",
        )
        .expect("the verb runs");
    let r = o.refusal.as_ref().expect("a pack's refusal is a report");
    assert_eq!(r.id(), "DECK-E002", "the code's registered entry");
    assert_eq!(detail(r, "next"), Some("shape get @s:s1"));
    assert_eq!(version(&f), v, "nothing applied");
}

/// A pack's entries are in its registry, `explain` and `capabilities` know them, and the
/// registry refuses entries that would clash or lie.
#[test]
fn pack_errors_are_registered() {
    let r = Registry::for_pack(&Deck).expect("the deck's registry");
    let ids: Vec<&str> = r.errors().iter().map(|e| e.id).collect();
    assert_eq!(ids, ["DECK-E001", "DECK-E002"], "the pack's entries");
    let cfg = Config::default();
    let out = run_meta(&argv("explain DECK-E001"), false, &[&r], &cfg).expect("a meta verb");
    assert_eq!(out.exit, 0, "{:?}", out.refusal);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("DECK-E001 shape_locked"), "{text}");
    let caps = capabilities(&[&r], &cfg).to_json();
    assert!(
        caps.contains("\"DECK-E001\""),
        "capabilities lists the pack's ids"
    );
    // what the registry refuses
    struct With(&'static [ErrorEntry]);
    impl Pack for With {
        fn schema(&self) -> Schema {
            DECK
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            Deck.decode(b)
        }
        fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
            Deck.encode(c, codec)
        }
        fn verbs(&self) -> &'static [PackVerb] {
            VERBS
        }
        fn errors(&self) -> &'static [ErrorEntry] {
            self.0
        }
    }
    const fn e(id: &'static str, name: &'static str, exit: u8) -> ErrorEntry {
        ErrorEntry::pack(id, name, exit, "m", "n")
    }
    let refused = |errors: &'static [ErrorEntry], why: &str| {
        let err = Registry::for_pack(&With(errors)).expect_err(why);
        assert!(err.to_string().contains(why), "{err}");
    };
    let leak = |v: Vec<ErrorEntry>| -> &'static [ErrorEntry] { Box::leak(v.into_boxed_slice()) };
    refused(leak(vec![e("MPC-E001", "x", 2)]), "DECK-E");
    refused(leak(vec![e("TIMELINE-E001", "x", 2)]), "DECK-E");
    refused(leak(vec![e("DECK-E1", "x", 2)]), "DECK-E");
    refused(
        leak(vec![e("DECK-E001", "x", 2), e("DECK-E001", "y", 2)]),
        "twice",
    );
    refused(
        leak(vec![e("DECK-E001", "x", 2), e("DECK-E002", "x", 2)]),
        "twice",
    );
    refused(leak(vec![e("DECK-E001", "x", 0)]), "exit");
    refused(leak(vec![e("DECK-E001", "x", 9)]), "exit");
    refused(leak(vec![e("DECK-E001", "Not Snake", 2)]), "snake_case");
    assert!(Registry::for_pack(&With(leak(vec![e("DECK-E001", "x_1", 2)]))).is_ok());
}

// ---------- the call's result as data (mpc.result/1) ----------

/// `argv` through `run_tool_with` with the result as data.
fn run_json(f: &Fx, c: ConnId, a: &str) -> ToolOutput {
    f.s.run_tool_with(
        c,
        FIELDS,
        &argv(a),
        b"",
        &ToolOptions::new().result_json(true),
    )
    .expect("the call returns its result")
}

/// The call's result, and stdout is its JSON.
fn result_of(o: &ToolOutput) -> &Json {
    let r = o.result().expect("the call's result as data");
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        format!("{}\n", r.to_json()),
        "stdout is the result's JSON"
    );
    r
}

/// The value at `path` (object keys) in `v`, as compact JSON; `<absent>` when a key is missing.
fn at(v: &Json, path: &str) -> String {
    let mut cur = v;
    for k in path.split('.') {
        match cur.get(k) {
            Some(x) => cur = x,
            None => return "<absent>".to_owned(),
        }
    }
    cur.to_json()
}

/// The result checked against its golden file (`MPC_BLESS=1` writes it).
fn golden(name: &str, v: &Json) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/results")
        .join(format!("{name}.json"));
    let got = format!("{}\n", v.to_json_pretty());
    if std::env::var_os("MPC_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert_eq!(
        got, want,
        "{name}: the result differs from its golden (MPC_BLESS=1 regenerates it)"
    );
}

/// An edit's result: versions before and after, counts, the changed units by address and field
/// with their values before and after, the verb's own text as data, and no error.
#[test]
fn result_of_an_edit() {
    let f = fx("edit");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o = run_json(&f, a, &format!("shape set s1 color red --if-version {v}"));
    assert_eq!(o.exit, 0);
    let r = result_of(&o);
    assert_eq!(at(r, "schema"), format!("\"{RESULT_SCHEMA}\""));
    assert_eq!(RESULT_SCHEMA, "mpc.result/1");
    assert_eq!(at(r, "verb"), "\"shape set\"");
    assert_eq!(at(r, "kind"), "\"edit\"");
    assert_eq!(at(r, "ok"), "true");
    assert_eq!(at(r, "exit"), "0");
    assert_eq!(
        at(r, "version.base"),
        v.to_string(),
        "version.base: the version computed on"
    );
    assert_eq!(at(r, "version.after"), version(&f).to_string());
    assert_ne!(version(&f), v);
    assert_eq!(at(r, "version.if_version"), v.to_string());
    assert_eq!(
        at(r, "changed.counts"),
        r#"[{"root":"fields","inserted":0,"deleted":0,"moved":0,"restored":0,"set":1,"dropped":0}]"#
    );
    assert_eq!(
        at(r, "changed.units"),
        r#"[{"address":"@s:s1","root":"fields","key":"s1","field":"color","op":"set","before":"blue","after":"red"}]"#,
        "the changed units, by address"
    );
    assert_eq!(at(r, "changed.units_more"), "0");
    assert_eq!(at(r, "data"), r#"{"text":"set @s:s1 color\n"}"#);
    assert_eq!(at(r, "data_schema"), "\"text\"");
    assert_eq!(at(r, "data_spec"), "null", "text names no shape");
    assert_eq!(at(r, "error"), "null");
    assert_eq!(at(r, "conflicts"), "[]");
    assert_eq!(at(r, "notes"), "[]");
    assert_eq!(at(r, "warnings"), "[]");
    golden("edit", r);
}

/// Every unit an edit changed, by address and field: a field of a composite map splits into its
/// element's address and the field; an element of a list is its address.
#[test]
fn changed_units_by_address() {
    let f = fx("units");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o = run_json(&f, a, &format!("shape add s3 --if-version {v}"));
    assert_eq!(
        at(result_of(&o), "changed.units"),
        r#"[{"address":"@s:s3","root":"shapes","key":"s3","field":null,"op":"insert","before":null,"after":null}]"#,
        "the changed units, by address"
    );
    let v = version(&f);
    let o = run_json(&f, a, &format!("shape set s2 w 3 --if-version {v}"));
    assert_eq!(
        at(result_of(&o), "changed.units"),
        r#"[{"address":"@s:s2","root":"fields","key":"s2","field":"w","op":"set","before":null,"after":"3"}]"#,
        "the changed units, by address"
    );
}

/// Without the result as data, the text is what it was: the verb's stdout, no result.
#[test]
fn text_unchanged_with_json_off() {
    let f = fx("text");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o =
        f.s.run_tool(
            a,
            FIELDS,
            &argv(&format!("shape set s1 color red --if-version {v}")),
            b"",
        )
        .expect("the verb runs");
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "set @s:s1 color\n",
        "text stays the default"
    );
    assert!(o.result().is_none(), "text stays the default");
}

/// `version.base` is the version the verb computed on, with or without a guard; `after` is the
/// version it made.
#[test]
fn result_version_base() {
    let f = fx("base");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o = run_json(&f, a, &format!("shape set s1 color red --base {v}"));
    let r = result_of(&o);
    assert_eq!(
        at(r, "version.base"),
        v.to_string(),
        "version.base: the version computed on"
    );
    assert_eq!(at(r, "version.base_given"), v.to_string());
    assert_eq!(at(r, "version.if_version"), "null");
    assert_eq!(at(r, "version.after"), version(&f).to_string());
}

/// A pack verb's own `--json` asks for the result: the kit takes it out of the verb's arguments.
#[test]
fn json_flag_on_a_pack_verb() {
    let f = fx("flag");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o = f.s.run_tool(
        a,
        FIELDS,
        &argv(&format!("shape set s1 color red --json --if-version {v}")),
        b"",
    );
    assert!(o.is_ok(), "--json on a pack verb: {:?}", o.err());
    let o = o.unwrap();
    assert_eq!(at(result_of(&o), "verb"), "\"shape set\"");
    let o =
        f.s.run_tool(a, FIELDS, &argv("shape get s1 --json"), b"")
            .expect("a read with --json");
    assert_eq!(at(result_of(&o), "data"), r#"{"text":"color=red\nw=10\n"}"#);
}

/// A pack's refusal in the result: the error object as `explain` has it (id, next step, details),
/// the exit the text form's, nothing after.
#[test]
fn result_of_a_refusal() {
    let f = fx("refused");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o = run_json(&f, a, &format!("shape lock s1 --if-version {v}"));
    assert_eq!(o.exit, 2);
    let r = result_of(&o);
    assert_eq!(
        at(r, "version.base"),
        v.to_string(),
        "version.base: the version computed on"
    );
    assert_eq!(at(r, "ok"), "false");
    assert_eq!(at(r, "exit"), "2");
    assert_eq!(
        at(r, "error.id"),
        "\"DECK-E001\"",
        "a pack's refusal is a report"
    );
    assert_eq!(at(r, "error.next_step.verb"), "\"shape get\"");
    assert_eq!(at(r, "error.details.code"), "\"shape_locked\"");
    assert_eq!(at(r, "version.after"), "null");
    assert_eq!(at(r, "changed"), "null");
    assert_eq!(
        at(r, "data.text"),
        "\"refused: shape_locked: @s:s1 is locked\\nnext: shape get @s:s1\\n\""
    );
    golden("refusal", r);
}

/// A session's refusal (a stale `--if-version`) is the same call's error as the result's
/// `error`, with its exit, instead of an `Err`.
#[test]
fn result_of_a_session_refusal() {
    let f = fx("stale");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    run_json(&f, a, &format!("shape set s1 color red --if-version {v}"));
    let call = format!("shape set s1 color green --if-version {v}");
    let e =
        f.s.run_tool(a, FIELDS, &argv(&call), b"")
            .expect_err("stale");
    let o = run_json(&f, a, &call);
    let r = result_of(&o);
    assert_eq!(o.exit, e.exit());
    assert_eq!(at(r, "exit"), e.exit().to_string());
    assert_eq!(at(r, "error.id"), format!("\"{}\"", e.report().id()));
    assert_eq!(at(r, "error.id"), "\"MPC-E110\"");
    assert_eq!(o.refusal.as_ref().map(|x| x.id()), Some("MPC-E110"));
    assert_eq!(at(r, "version.after"), "null");
}

/// A `--base` edit over someone's later write to the same field: the conflict by address and
/// field, who and which change.
#[test]
fn result_conflicts_by_field() {
    let f = fx("conflict");
    let (a, b) = (agent(&f.s, "bot"), agent(&f.s, "other"));
    let v0 = version(&f);
    run_json(
        &f,
        b,
        &format!("shape set s1 color green --if-version {v0}"),
    );
    let v1 = version(&f);
    let o = run_json(&f, a, &format!("shape set s1 color red --base {v0}"));
    let r = result_of(&o);
    assert_eq!(at(r, "ok"), "false");
    assert_eq!(id_of(r), "MPC-E803", "a facet conflict");
    let c = match r.get("conflicts") {
        Some(Json::List(l)) if !l.is_empty() => l[0].clone(),
        other => panic!("conflicts by field: {other:?}"),
    };
    assert_eq!(at(&c, "address"), "\"@s:s1\"", "conflicts by field");
    assert_eq!(at(&c, "field"), "\"color\"");
    assert_eq!(at(&c, "root"), "\"fields\"");
    assert_eq!(at(&c, "by"), "\"other\"");
    assert_ne!(at(&c, "change"), "<absent>");
    assert_eq!(version(&f), v1, "nothing of the refused call applied");
}

/// The error id of a result.
fn id_of(r: &Json) -> String {
    r.get("error")
        .and_then(|e| e.get("id"))
        .and_then(|i| i.as_str())
        .unwrap_or("<none>")
        .to_owned()
}

/// A read's result: the version read, no change, the verb's text as data.
#[test]
fn result_of_a_read() {
    let f = fx("read");
    let a = agent(&f.s, "bot");
    let v = version(&f);
    let o = run_json(&f, a, "shape get s1");
    let r = result_of(&o);
    assert_eq!(at(r, "kind"), "\"read\"");
    assert_eq!(at(r, "version.read"), v.to_string());
    assert_eq!(at(r, "changed"), "null");
    assert_eq!(at(r, "data"), r#"{"text":"color=blue\nw=10\n"}"#);
    assert_eq!(at(r, "data_schema"), "\"text\"");
    assert_eq!(at(r, "data_spec"), "null", "text names no shape");
    golden("read", r);
    // not found: exit 4 and the kit's id
    let o = run_json(&f, a, "shape get s9");
    let r = result_of(&o);
    assert_eq!(o.exit, 4);
    assert_eq!(at(r, "error.id"), "\"MPC-E805\"");
    golden("not_found", r);
}

/// A read's typed data: `data_schema` says `typed`, and `data_spec` names the data's shape and
/// its version; stdout's text form is unchanged without the result.
#[test]
fn result_of_a_typed_read() {
    let f = fx("typed");
    let a = agent(&f.s, "bot");
    let o = run_json(&f, a, "shape fields s1");
    assert_eq!(o.exit, 0);
    let r = result_of(&o);
    assert_eq!(
        at(r, "data"),
        r#"{"address":"@s:s1","fields":{"color":"blue","w":"10"}}"#,
        "the verb's typed data"
    );
    assert_eq!(at(r, "data_schema"), "\"typed\"", "typed data says so");
    assert_eq!(
        at(r, "data_spec"),
        "\"deck.shape-fields/1\"",
        "the typed data names its shape"
    );
    golden("typed_read", r);
    // the text form, without the result, is the verb's own lines
    let o =
        f.s.run_tool(a, FIELDS, &argv("shape fields s1"), b"")
            .unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "color=blue\nw=10\n");
    assert!(o.result().is_none());
}

/// A verb that prints JSON today (a grid's `cell get --json`) keeps its form: `--json` is its
/// own. The result is the host's to ask for, and holds that output as its data.
#[test]
fn a_verb_that_prints_json_keeps_its_form() {
    let dir = std::env::temp_dir().join(format!("k2mb-agent-results-grid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(TestHost), Config::default());
    let s = mgr
        .open(
            Arc::new(grid_pack::GridLite),
            &DocKey {
                space: "ws".into(),
                path: "t.csv".into(),
            },
            Some(b"id,city\nk1,Oslo\n"),
        )
        .unwrap();
    let a = agent(&s, "bot");
    let call = argv("cell get --row r1 --col c2 --json");
    let o = s.run_tool(a, k2_mpc_base::table::ROWS, &call, b"").unwrap();
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        text.starts_with("{\"value\":\"Oslo\""),
        "its own JSON: {text}"
    );
    assert!(o.result().is_none(), "its --json is its own");
    let o = s
        .run_tool_with(
            a,
            k2_mpc_base::table::ROWS,
            &call,
            b"",
            &ToolOptions::new().result_json(true),
        )
        .unwrap();
    let r = result_of(&o);
    assert_eq!(at(r, "data.text"), Json::from(text.as_str()).to_json());
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `capabilities` names the result's schema.
#[test]
fn capabilities_names_the_result_schema() {
    let r = Registry::for_pack(&Deck).expect("the deck's registry");
    let caps = capabilities(&[&r], &Config::default());
    assert_eq!(at(&caps, "result_schema.name"), "\"mpc.result\"");
    assert_eq!(at(&caps, "result_schema.version"), "1");
}
