//! Tagged-key minting through the public API (`k2_mpc_base::mint`), with its planted fault.
//!
//! `MPC_STRUCT_MUTANT` plants a fault for the whole suite (`k2_mpc_base::differential`, the
//! `differential` feature, on for these tests):
//! - `mint-ignore-retired` is `MintMutant::IgnoreRetired`, caught by
//!   `drop_then_mint_never_reuses`;
//! - `any-writer-tag` is `MintMutant::AnyWriterTag`, caught by
//!   `an_insert_with_another_writers_tag_is_refused`;
//! - `single-lww-mark` is `MintMutant::SingleLwwMark`, caught by
//!   `concurrent_drops_keep_the_highest_mark`;
//! - `client-id-zero` is `MintMutant::ClientIdZero`, caught by
//!   `client_ids_draw_again_past_zero_one_and_the_import_id`;
//! - `shift-not-resample` is `MintMutant::ShiftNotResample`, caught by
//!   `a_draw_of_zero_then_seven_gives_seven`;
//! - `reuse-held-client` is `MintMutant::ReuseHeldClient`, caught by
//!   `join_and_leave_churn_never_shares_a_client_id`.
//!
//! Unset, the tests run as is. Any other value is refused.

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, KeyedRoot, ListOp, MapOp};
use k2_mpc_base::mint::{marks_for_drops, next_for, retired_mark, TaggedKey, WriterTag};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, Host, Pack, PackContent, PackError,
    Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::{Doc, Edit, Key, Kind, Schema, Value};

fn under_struct_mutant<R>(f: impl FnOnce() -> R) -> R {
    use k2_mpc_base::differential::{with_mint_mutant, MintMutant};
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    match m.as_deref() {
        Ok("mint-ignore-retired") => with_mint_mutant(MintMutant::IgnoreRetired, f),
        Ok("any-writer-tag") => with_mint_mutant(MintMutant::AnyWriterTag, f),
        Ok("single-lww-mark") => with_mint_mutant(MintMutant::SingleLwwMark, f),
        Ok("client-id-zero") => with_mint_mutant(MintMutant::ClientIdZero, f),
        Ok("shift-not-resample") => with_mint_mutant(MintMutant::ShiftNotResample, f),
        Ok("reuse-held-client") => with_mint_mutant(MintMutant::ReuseHeldClient, f),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this suite plants"),
        Err(_) => f(),
    }
}

/// One list of items as a writer sees it: every key it knows (live or deleted) and the retired
/// high-water marks a drop wrote, per (prefix, tag) (`mint/<prefix>.<tag>`).
#[derive(Default)]
struct Items {
    known: BTreeSet<Key>,
    retired: std::collections::BTreeMap<(String, WriterTag), u64>,
}

impl Items {
    fn mint(&mut self, prefix: &str, tag: WriterTag) -> Key {
        let r = self
            .retired
            .get(&(prefix.to_owned(), tag))
            .copied()
            .unwrap_or(0);
        let n = next_for(&self.known, r, prefix, tag).expect("a next number");
        let k = TaggedKey::new(prefix, n, tag)
            .unwrap()
            .to_key()
            .expect("a key that fits");
        assert!(self.known.insert(k.clone()), "{k} minted while known");
        k
    }

    /// Drop forgets the key; the dropper raises the retired mark first.
    fn drop_key(&mut self, k: &Key) {
        let t = TaggedKey::parse(k).expect("a tagged key");
        let mark = self
            .retired
            .entry((t.prefix().to_owned(), t.tag()))
            .or_insert(0);
        *mark = (*mark).max(t.n());
        assert!(self.known.remove(k));
    }
}

#[test]
fn drop_then_mint_never_reuses() {
    under_struct_mutant(|| {
        let me = WriterTag::of(4242).unwrap();
        let mut items = Items::default();
        let mut ever = HashSet::new();
        let first: Vec<Key> = (0..3).map(|_| items.mint("i", me)).collect();
        ever.extend(first.iter().cloned());
        // Drop every key: nothing in the list names them any more.
        for k in &first {
            items.drop_key(k);
        }
        assert!(items.known.is_empty());
        let k = items.mint("i", me);
        assert!(
            ever.insert(k.clone()),
            "{k} was minted again after a drop (the retired mark was ignored)"
        );
        assert_eq!(TaggedKey::parse(&k).unwrap().n(), 4);
    });
}

#[test]
fn writers_mint_apart_and_parse_back() {
    under_struct_mutant(|| {
        let tags: Vec<WriterTag> = [2u64, 36, 99_999, (1 << 53) - 2]
            .into_iter()
            .map(|c| WriterTag::of(c).unwrap())
            .collect();
        let mut items = Items::default();
        let mut ever = HashSet::new();
        for round in 0..20 {
            for &t in &tags {
                let k = items.mint(if round % 2 == 0 { "i" } else { "lnk" }, t);
                let p = TaggedKey::parse(&k).unwrap();
                assert_eq!(p.tag(), t);
                assert_eq!(p.to_key().unwrap(), k);
                assert!(ever.insert(k));
            }
        }
        assert_eq!(ever.len(), 20 * tags.len());
    });
}

// ---- the writer-tag rule in a session (`Pack::tagged_keys`)

/// A keyed pack: a MovableList `items` with tagged keys (prefix `i`) and a MovableList `links`
/// without. The file is `<root> <key>` per line.
struct ItemsPack {
    /// Keep retired marks in a Map root `marks` (`Pack::mint_marks`).
    marks: bool,
}

impl Pack for ItemsPack {
    fn schema(&self) -> Schema {
        Schema {
            name: "items",
            roots: &[
                ("items", Kind::MovableList),
                ("links", Kind::MovableList),
                ("marks", Kind::Map),
            ],
        }
    }
    fn mint_marks(&self) -> Option<&'static str> {
        self.marks.then_some("marks")
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut items, mut links) = (Vec::new(), Vec::new());
        for l in std::str::from_utf8(b).unwrap().lines() {
            let (root, k) = l.split_once(' ').unwrap();
            let e = (Key::new(k).unwrap(), Value::Null);
            if root == "items" {
                items.push(e);
            } else {
                links.push(e);
            }
        }
        Ok(PackContent {
            roots: vec![
                ("items".into(), Neutral::List(items)),
                ("links".into(), Neutral::List(links)),
                ("marks".into(), Neutral::Map(Default::default())),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut out = String::new();
        for (root, n) in c {
            if let Neutral::List(es) = n {
                for (k, _) in es {
                    out.push_str(&format!("{root} {k}\n"));
                }
            }
        }
        Ok(out.into_bytes())
    }
    fn tagged_keys(&self) -> &'static [(&'static str, &'static [&'static str])] {
        &[("items", &["i"])]
    }
}

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct Fx {
    _mgr: SessionManager,
    s: Arc<Session>,
    dir: std::path::PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(name: &str, file: &[u8]) -> Fx {
    fx_with(name, file, false)
}

fn fx_with(name: &str, file: &[u8], marks: bool) -> Fx {
    let dir = std::env::temp_dir().join(format!("k2mpc-mint-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "a.items".into(),
    };
    let s = mgr
        .open(Arc::new(ItemsPack { marks }), &key, Some(file))
        .unwrap();
    Fx { _mgr: mgr, s, dir }
}

fn person(s: &Session, pid: &str) -> ConnId {
    let p = Participant::new(pid, pid, "#000000", ParticipantKind::Person, "app");
    s.join(p, Stay::UntilIdle).unwrap().conn
}

fn insert(root: &str, key: &Key) -> Vec<Edit> {
    vec![Edit::List {
        root: root.into(),
        ops: vec![ListOp::Insert {
            key: key.clone(),
            at: Anchor::End,
            value: Value::Null,
        }],
    }]
}

fn live(s: &Session, root: &str) -> Vec<String> {
    let st = s.keyed_state_for_tests().unwrap();
    match st.roots.iter().find(|(r, _)| r == root) {
        Some((_, KeyedRoot::List { live, .. })) => {
            live.iter().map(|(k, _)| k.to_string()).collect()
        }
        other => panic!("{root}: {other:?}"),
    }
}

#[test]
fn minted_keys_insert_and_count_on() {
    under_struct_mutant(|| {
        let f = fx("count", b"items i7\nlinks l1\n");
        let a = person(&f.s, "ana");
        let t = f.s.writer_tag(a).unwrap();
        let k1 = f.s.mint_key(a, "items", "i").unwrap();
        assert_eq!(
            k1.as_str(),
            format!("i1.{t}"),
            "the import's untagged i7 isn't ours"
        );
        f.s.edit_ops(a, insert("items", &k1), &EditOptions::new())
            .unwrap();
        let k2 = f.s.mint_key(a, "items", "i").unwrap();
        assert_eq!(k2.as_str(), format!("i2.{t}"));
        // a deleted key still counts
        f.s.edit_ops(a, insert("items", &k2), &EditOptions::new())
            .unwrap();
        f.s.edit_ops(
            a,
            vec![Edit::List {
                root: "items".into(),
                ops: vec![ListOp::Delete { key: k2.clone() }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        assert_eq!(
            f.s.mint_key(a, "items", "i").unwrap().as_str(),
            format!("i3.{t}")
        );
        // an undeclared root or prefix is a bad edit
        assert!(matches!(
            f.s.mint_key(a, "links", "l"),
            Err(SessionError::BadEdit(_))
        ));
        assert!(matches!(
            f.s.mint_key(a, "items", "x"),
            Err(SessionError::BadEdit(_))
        ));
        // a root without tagged keys takes any key
        f.s.edit_ops(
            a,
            insert("links", &Key::new("l2").unwrap()),
            &EditOptions::new(),
        )
        .unwrap();
        assert_eq!(live(&f.s, "items"), ["i7".to_owned(), k1.to_string()]);
    });
}

#[test]
fn an_insert_with_another_writers_tag_is_refused() {
    under_struct_mutant(|| {
        let f = fx("other", b"");
        let a = person(&f.s, "ana");
        let b = person(&f.s, "bo");
        let ta = f.s.writer_tag(a).unwrap();
        let tb = f.s.writer_tag(b).unwrap();
        assert_ne!(ta, tb, "one writer per client ID");
        // B inserts a key carrying A's tag: refused, nothing applied
        let theirs = TaggedKey::new("i", 1, ta).unwrap().to_key().unwrap();
        let v = f.s.status().unwrap().version;
        let r =
            f.s.edit_ops(b, insert("items", &theirs), &EditOptions::new());
        assert_eq!(
            r.map(|_| ()),
            Err(SessionError::KeyNotOwn {
                root: "items".into(),
                key: theirs.to_string(),
                tag: tb.to_string(),
            }),
            "an insert with another writer's tag must be refused (E820)"
        );
        assert_eq!(f.s.status().unwrap().version, v);
        assert!(live(&f.s, "items").is_empty());
        // A inserts the same key: its own
        f.s.edit_ops(a, insert("items", &theirs), &EditOptions::new())
            .unwrap();
        assert_eq!(live(&f.s, "items"), [theirs.to_string()]);
    });
}

#[test]
fn untagged_and_undeclared_prefixes_are_refused() {
    under_struct_mutant(|| {
        let f = fx("forms", b"items i9\n");
        let a = person(&f.s, "ana");
        let t = f.s.writer_tag(a).unwrap();
        for k in [
            "i1".to_owned(),
            format!("x1.{t}"),
            format!("i01.{t}"),
            format!("i1.{t}.x"),
        ] {
            let r = f.s.edit_ops(
                a,
                insert("items", &Key::new(k.as_str()).unwrap()),
                &EditOptions::new(),
            );
            assert!(
                matches!(r, Err(SessionError::KeyNotOwn { .. })),
                "{k}: {r:?}"
            );
        }
        // a key the list already knows is the engine's refusal (a reused key), not the tag's
        let r = f.s.edit_ops(
            a,
            insert("items", &Key::new("i9").unwrap()),
            &EditOptions::new(),
        );
        assert!(
            !matches!(r, Err(SessionError::KeyNotOwn { .. })) && r.is_err(),
            "{r:?}"
        );
    });
}

// ---- retired marks: one key per dropper, read as the max

const MARKED: Schema = Schema {
    name: "marked",
    roots: &[("items", Kind::MovableList), ("marks", Kind::Map)],
};

fn list_ops(ops: Vec<ListOp>) -> Edit {
    Edit::List {
        root: "items".into(),
        ops,
    }
}

/// `client` deletes then drops `keys` on `d`, writing its own marks in the drop's change; the
/// updates it made.
fn drop_on(d: &mut Doc, client: u64, keys: &[Key]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let del = keys
        .iter()
        .map(|k| ListOp::Delete { key: k.clone() })
        .collect();
    out.extend(d.edit_as(client, &[list_ops(del)]).unwrap());
    let held = d.map("marks").unwrap();
    let marks = marks_for_drops(
        "items",
        keys,
        WriterTag::of(client).unwrap(),
        held.iter().map(|(k, v)| (k.as_str(), v)),
    );
    let drop = keys
        .iter()
        .map(|k| ListOp::Drop { key: k.clone() })
        .collect();
    let edits = [
        list_ops(drop),
        Edit::Map {
            root: "marks".into(),
            ops: marks,
        },
    ];
    out.extend(d.edit_as(client, &edits).unwrap());
    out
}

fn apply_all(d: &mut Doc, ups: &[Vec<u8>], author: u64) {
    for u in ups {
        d.apply_client_update(u, author).unwrap();
    }
}

/// The next number `w` mints on `d`: from every place the list knows and the marks.
fn next_on(d: &Doc, w: WriterTag) -> u64 {
    let st = d.keyed_state().unwrap();
    let mut places: Vec<Key> = Vec::new();
    let mut marks = std::collections::BTreeMap::new();
    for (root, r) in &st.roots {
        match r {
            KeyedRoot::List { live, hidden, .. } if root == "items" => {
                places.extend(live.iter().map(|(k, _)| k.clone()));
                places.extend(hidden.keys().cloned());
            }
            KeyedRoot::Map(m) if root == "marks" => marks = m.clone(),
            _ => {}
        }
    }
    let retired = retired_mark(marks.iter().map(|(k, v)| (k.as_str(), v)), "items", "i", w);
    next_for(&places, retired, "i", w).unwrap()
}

/// Two replicas drop keys of one writer's tag concurrently (n up to 7 on one, up to 3 on the
/// other), merged both ways and in both orders: the mark is the max, so a mint on any replica
/// gives n >= 8 and never a dropped key. Each dropper role is played by each client in turn,
/// so a shared last-writer-wins mark loses the 7 in one of them whichever client ID wins.
#[test]
fn concurrent_drops_keep_the_highest_mark() {
    under_struct_mutant(|| {
        let writer = 9000u64;
        let w = WriterTag::of(writer).unwrap();
        let key = |n: u64| TaggedKey::new("i", n, w).unwrap().to_key().unwrap();
        let minted: Vec<Key> = (1..=7).map(key).collect();
        let mut base = Doc::new(MARKED).unwrap();
        let ins: Vec<ListOp> = minted
            .iter()
            .map(|k| ListOp::Insert {
                key: k.clone(),
                at: Anchor::End,
                value: Value::Null,
            })
            .collect();
        base.edit_as(writer, &[list_ops(ins)]).unwrap();
        let state = base.encode_state().unwrap();
        for (hi_client, lo_client) in [(101u64, 202u64), (202, 101)] {
            let mut hi = Doc::load(MARKED, &state).unwrap();
            let mut lo = Doc::load(MARKED, &state).unwrap();
            let hi_ups = drop_on(&mut hi, hi_client, &minted[3..]); // n 4..=7
            let lo_ups = drop_on(&mut lo, lo_client, &minted[..3]); // n 1..=3
            let mut ab = Doc::load(MARKED, &state).unwrap();
            apply_all(&mut ab, &hi_ups, hi_client);
            apply_all(&mut ab, &lo_ups, lo_client);
            let mut ba = Doc::load(MARKED, &state).unwrap();
            apply_all(&mut ba, &lo_ups, lo_client);
            apply_all(&mut ba, &hi_ups, hi_client);
            apply_all(&mut hi, &lo_ups, lo_client);
            apply_all(&mut lo, &hi_ups, hi_client);
            for (name, d) in [("hi", &hi), ("lo", &lo), ("ab", &ab), ("ba", &ba)] {
                assert_eq!(
                    d.map("marks").unwrap(),
                    hi.map("marks").unwrap(),
                    "{name}: the marks converge"
                );
                let n = next_on(d, w);
                assert!(
                    n >= 8,
                    "{name} (high dropper {hi_client}): the next mint is n = {n}, a dropped key \
                     ({}): the lower mark won",
                    key(n)
                );
            }
        }
    });
}

/// The session path: `mint_key` reads the pack's marks root, the max over every dropper's
/// mark for the connection's prefix and tag.
#[test]
fn mint_key_reads_the_max_of_every_droppers_mark() {
    under_struct_mutant(|| {
        let f = fx_with("marks", b"", true);
        let a = person(&f.s, "ana");
        let t = f.s.writer_tag(a).unwrap();
        let other = WriterTag::of(4242).unwrap();
        let set = |k: String, n: &str| MapOp::Set {
            key: k,
            value: Value::str(n),
        };
        f.s.edit_ops(
            a,
            vec![Edit::Map {
                root: "marks".into(),
                ops: vec![
                    set(format!("mint/items/i.{t}/{t}"), "4"),
                    set(format!("mint/items/i.{t}/{other}"), "9"),
                    set(format!("mint/items/i.{other}/{t}"), "50"),
                    set(format!("mint/links/i.{t}/{t}"), "70"),
                ],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        assert_eq!(
            f.s.mint_key(a, "items", "i").unwrap().as_str(),
            format!("i10.{t}")
        );
        // without a marks root declared, no mark is read
        let g = fx_with("nomarks", b"", false);
        let b = person(&g.s, "bo");
        let tb = g.s.writer_tag(b).unwrap();
        g.s.edit_ops(
            b,
            vec![Edit::Map {
                root: "marks".into(),
                ops: vec![set(format!("mint/items/i.{tb}/{tb}"), "4")],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        assert_eq!(
            g.s.mint_key(b, "items", "i").unwrap().as_str(),
            format!("i1.{tb}")
        );
    });
}

// ---- client IDs: one helper, drawn again past 0, 1 and the import agent's; never shared

fn join_as(s: &Session, pid: &str, kind: ParticipantKind) -> (ConnId, u64) {
    let p = Participant::new(pid, pid, "#000000", kind, "app");
    let j = s.join(p, Stay::UntilIdle).unwrap();
    (j.conn, j.client)
}

/// Raw draws of 0, 1 (the session's own) and 2^53 - 1 (the native engine's import agent) are
/// drawn again, never handed out and never shifted.
#[test]
fn client_ids_draw_again_past_zero_one_and_the_import_id() {
    use k2_mpc_base::differential::with_client_draws;
    use k2_mpc_base::doc::{IMPORT_CLIENT, MAX_CLIENT_ID, MAX_KIT_CLIENT};
    assert_eq!(IMPORT_CLIENT, MAX_CLIENT_ID);
    assert_eq!(MAX_KIT_CLIENT, (1 << 53) - 2);
    under_struct_mutant(|| {
        let f = fx("draws", b"");
        let join = |pid: &str| {
            let p = Participant::new(pid, pid, "#000000", ParticipantKind::Person, "app");
            f.s.join(p, Stay::UntilIdle).map(|j| j.client)
        };
        let got = with_client_draws([0, 1, IMPORT_CLIENT, 77], || join("ana"));
        assert_eq!(
            got,
            Ok(77),
            "0, 1 and the import agent's ID are drawn again"
        );
        let got = with_client_draws([IMPORT_CLIENT, 0, MAX_KIT_CLIENT], || join("bo"));
        assert_eq!(
            got,
            Ok(MAX_KIT_CLIENT),
            "the highest the kit mints is 2^53 - 2"
        );
        // with no forced draw, a long run of random ones stays in range
        for i in 0..40 {
            let c = join_as(&f.s, &format!("p{i}"), ParticipantKind::Person).1;
            assert!((2..=MAX_KIT_CLIENT).contains(&c), "{c}");
        }
    });
}

/// Two live connections never hold one client ID: a drawn ID a present connection holds is
/// drawn again, and so is one this doc has used before; the agent-reuse path
/// (`Config::reuse_agent_clients`) reuses an agent's earlier ID only while no connection holds
/// it.
#[test]
fn live_connections_never_share_a_client_id() {
    use k2_mpc_base::differential::with_client_draws;
    under_struct_mutant(|| {
        let f = fx("share", b"");
        let s = &f.s;
        let (_, a1) = with_client_draws([500], || join_as(s, "ana", ParticipantKind::Person));
        assert_eq!(a1, 500);
        // ana's second connection (not shared): 500 is held, drawn again
        let (_, a2) = with_client_draws([500, 501], || join_as(s, "ana", ParticipantKind::Person));
        assert_eq!(a2, 501);
        // an agent leaves and comes back: its earlier ID again
        let (b1c, b1) = with_client_draws([600], || join_as(s, "bot", ParticipantKind::Agent));
        assert_eq!(b1, 600);
        assert!(s.leave(b1c).unwrap());
        let (_, b2) = with_client_draws([], || join_as(s, "bot", ParticipantKind::Agent));
        assert_eq!(b2, 600, "the agent-reuse path");
        // a second live connection of that agent never shares 600: reuse skips a held ID, and a
        // fresh draw of it (the doc has used it) is drawn again
        let (_, b3) = with_client_draws([600, 601], || join_as(s, "bot", ParticipantKind::Agent));
        assert_eq!(b3, 601);
        // nor does anyone else get an ID this doc has used, live or not
        let (_, c1) = with_client_draws([500, 600, 601, 602], || {
            join_as(s, "cy", ParticipantKind::Person)
        });
        assert_eq!(c1, 602);
    });
}

/// A draw of 0 then 7 gives 7, never 2: a draw outside the range is drawn again, not moved
/// into it (a shift maps several draws onto one ID).
#[test]
fn a_draw_of_zero_then_seven_gives_seven() {
    use k2_mpc_base::differential::with_client_draws;
    under_struct_mutant(|| {
        let f = fx("seven", b"");
        let join = |pid: &str| {
            let p = Participant::new(pid, pid, "#000000", ParticipantKind::Person, "app");
            f.s.join(p, Stay::UntilIdle).map(|j| j.client)
        };
        assert_eq!(with_client_draws([0, 7], || join("ana")), Ok(7));
        assert_eq!(with_client_draws([1, 8], || join("bo")), Ok(8));
    });
}

/// A doc refuses a writer of 0, 1 (the session's own) and 2^53 - 1 (the native engine's import
/// agent), from a host or a client; 2 and 2^53 - 2 write.
#[test]
fn check_client_refuses_zero_one_and_the_import_agent() {
    use k2_mpc_base::doc::{DocError, IMPORT_CLIENT, MAX_KIT_CLIENT};
    let edit = |c: u64| {
        let mut d = Doc::new(MARKED).unwrap();
        let ins = ListOp::Insert {
            key: Key::new("a").unwrap(),
            at: Anchor::End,
            value: Value::Null,
        };
        d.edit_as(c, &[list_ops(vec![ins])]).map(|_| ())
    };
    for c in [0, 1, IMPORT_CLIENT, IMPORT_CLIENT + 1] {
        assert_eq!(edit(c), Err(DocError::BadClient(c)), "{c}");
    }
    for c in [2, MAX_KIT_CLIENT] {
        assert_eq!(edit(c), Ok(()), "{c}");
    }
}

/// Joins and leaves at random across people, agents (with ID reuse on, the default) and
/// agents with their own replica: no client ID is ever held by two present connections at
/// once, and an own replica's ID is never handed out again. `MPC_JOIN_CHURN` sets the number
/// of steps (10,000 by default; the gate runs 100,000).
#[test]
fn join_and_leave_churn_never_shares_a_client_id() {
    under_struct_mutant(|| {
        let steps: u64 = std::env::var("MPC_JOIN_CHURN")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);
        println!("MPC_JOIN_CHURN={steps}");
        let f = fx("churn", b"");
        let mut rng = 0x6368_7572_6e5f_3031u64;
        let mut next = move || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let mut present: Vec<(ConnId, u64)> = Vec::new();
        let mut replica_ids: HashSet<u64> = HashSet::new();
        for step in 0..steps {
            if present.is_empty() || next() % 2 == 0 {
                let pid = format!("p{}", next() % 6);
                let p = Participant::new(&pid, &pid, "#000000", ParticipantKind::Agent, "cli");
                let p = match next() % 4 {
                    0 => Participant::new(&pid, &pid, "#000000", ParticipantKind::Person, "app"),
                    1 => p.own_replica(),
                    _ => p,
                };
                let own = p.own_replica;
                let j = f.s.join(p, Stay::UntilIdle).unwrap();
                let (conn, c) = (j.conn, j.client);
                assert!(
                    present.iter().all(|(_, x)| *x != c),
                    "step {step}: client {c} is already held by a present connection"
                );
                assert!(
                    !replica_ids.contains(&c),
                    "step {step}: client {c} was an own replica's"
                );
                if own {
                    replica_ids.insert(c);
                }
                present.push((conn, c));
            } else {
                let i = (next() % present.len() as u64) as usize;
                let (conn, _) = present.swap_remove(i);
                assert!(f.s.leave(conn).unwrap());
            }
        }
    });
}

/// After the doc is unloaded and loaded again (the store reopened), a fresh client ID never
/// equals one the store has an owner for: forced draws of every earlier ID are drawn again.
#[test]
fn a_fresh_id_after_a_reload_is_never_an_owned_one() {
    use k2_mpc_base::differential::with_client_draws;
    under_struct_mutant(|| {
        let dir = std::env::temp_dir().join(format!("k2mpc-mint-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = DocKey {
            space: "ws".into(),
            path: "a.items".into(),
        };
        let open = |bytes: Option<&[u8]>| {
            let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
            let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), Config::default());
            let s = mgr
                .open(Arc::new(ItemsPack { marks: false }), &key, bytes)
                .unwrap();
            (mgr, s)
        };
        let person = |s: &Session, pid: &str| {
            let p = Participant::new(pid, pid, "#000000", ParticipantKind::Person, "app");
            s.join(p, Stay::UntilIdle).unwrap()
        };
        let mut owned = Vec::new();
        {
            let (_mgr, s) = open(Some(b""));
            for (i, d) in [101u64, 102, 103].into_iter().enumerate() {
                let j = with_client_draws([d], || person(&s, &format!("p{i}")));
                // a client ID's owner row lands with its first change
                let key = s.mint_key(j.conn, "items", "i").unwrap();
                s.edit_ops(j.conn, insert("items", &key), &EditOptions::new())
                    .unwrap();
                owned.push(j.client);
            }
        }
        assert_eq!(owned, [101, 102, 103]);
        let (_mgr, s) = open(None);
        let got = with_client_draws(owned.iter().copied().chain([104]), || {
            person(&s, "q").client
        });
        assert_eq!(
            got, 104,
            "an ID the store has an owner for is drawn again after a reload"
        );
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

// ---- four replicas mint, drop and sync at random: no key is ever minted twice

#[derive(Debug, Clone)]
enum Step {
    /// Replica `r` mints a key of its own and inserts it.
    Mint(usize),
    /// Replica `r` deletes and drops one of its live items (any writer's), writing its mark.
    Drop(usize, usize),
    /// Everything replica `from` has that `to` lacks.
    Sync(usize, usize),
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    prop_oneof![
        3 => (0..4usize).prop_map(Step::Mint),
        2 => (0..4usize, any::<usize>()).prop_map(|(r, k)| Step::Drop(r, k)),
        3 => (0..4usize, 0..4usize).prop_map(|(a, b)| Step::Sync(a, b)),
    ]
}

fn live_items(d: &Doc) -> Vec<Key> {
    d.list("items")
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// Run `steps` on four replicas, then sync all, and mint once more everywhere; fails on a key
/// minted twice.
fn run_replicas(steps: &[Step]) {
    use k2_mpc_base::conformance::replica::sync;
    let clients = [101u64, 202, 303, 404];
    let mut docs: Vec<Doc> = (0..4).map(|_| Doc::new(MARKED).unwrap()).collect();
    let mut ever: HashSet<Key> = HashSet::new();
    let mut mint = |docs: &mut Vec<Doc>, r: usize| {
        let w = WriterTag::of(clients[r]).unwrap();
        let n = next_on(&docs[r], w);
        let k = TaggedKey::new("i", n, w).unwrap().to_key().unwrap();
        assert!(ever.insert(k.clone()), "{k} minted twice (replica {r})");
        let ins = ListOp::Insert {
            key: k,
            at: Anchor::End,
            value: Value::Null,
        };
        docs[r].edit_as(clients[r], &[list_ops(vec![ins])]).unwrap();
    };
    for s in steps {
        match *s {
            Step::Mint(r) => mint(&mut docs, r),
            Step::Drop(r, pick) => {
                let live = live_items(&docs[r]);
                if !live.is_empty() {
                    let k = live[pick % live.len()].clone();
                    drop_on(&mut docs[r], clients[r], &[k]);
                }
            }
            Step::Sync(a, b) if a != b => {
                let (from, to) = if a < b {
                    let (x, y) = docs.split_at_mut(b);
                    (&x[a], &mut y[0])
                } else {
                    let (x, y) = docs.split_at_mut(a);
                    (&y[0], &mut x[b])
                };
                sync(from, to).unwrap();
            }
            Step::Sync(..) => {}
        }
    }
    for _ in 0..2 {
        for a in 0..4 {
            for b in 0..4 {
                if a != b {
                    let (from, to) = if a < b {
                        let (x, y) = docs.split_at_mut(b);
                        (&x[a], &mut y[0])
                    } else {
                        let (x, y) = docs.split_at_mut(a);
                        (&y[0], &mut x[b])
                    };
                    sync(from, to).unwrap();
                }
            }
        }
    }
    for d in &docs[1..] {
        assert_eq!(
            d.keyed_state().unwrap(),
            docs[0].keyed_state().unwrap(),
            "converged"
        );
    }
    for r in 0..4 {
        mint(&mut docs, r);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(64))]
    /// Four replicas mint, drop and sync in any order: no key is ever minted twice, drops
    /// included, and the replicas converge.
    #[test]
    fn four_replicas_never_mint_a_key_twice(steps in proptest::collection::vec(step(), 1..60)) {
        under_struct_mutant(|| run_replicas(&steps));
    }
}

/// A fixed schedule the proptest also covers, kept as a named case: replica 0's keys are
/// dropped by replica 1, and replica 0 learns of the drop before it mints again.
#[test]
fn a_writer_never_remints_a_key_another_replica_dropped() {
    use Step::*;
    under_struct_mutant(|| {
        run_replicas(&[
            Mint(0),
            Mint(0),
            Mint(0),
            Sync(0, 1),
            Drop(1, 2),
            Drop(1, 1),
            Sync(1, 0),
            Mint(0),
        ])
    });
}
