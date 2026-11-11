use super::*;
use crate::{Doc, Kind, Schema};

/// A Yrs doc's clock bound ([`crate::doc::Doc::max_clock`]).
const Y: u64 = u32::MAX as u64;

const S: Schema = Schema {
    name: "test",
    roots: &[("source", Kind::Text)],
};

struct TmpDir(std::path::PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "k2mb-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
    fn db(&self) -> std::path::PathBuf {
        self.0.join("docs.redb")
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws-1".into(),
        path: p.into(),
    }
}

fn meta(p: &str) -> DocMeta {
    DocMeta {
        key: key(p),
        pack: "test".into(),
        engine: "yrs-0.28".into(),
        engine_format: 2,
        codec: vec![1, 0],
        created_ms: 1_000,
        synced_hash: None,
        synced_seq: 0,
        declined: None,
    }
}

fn entry(author: &str, client: u64, update: Vec<u8>) -> LogEntry {
    LogEntry::yrs(author.into(), client, 5, "cli".into(), update, None)
}

/// The Yrs update of a Yrs entry.
fn yrs_bytes(e: &LogEntry) -> &[u8] {
    match &e.body {
        EntryBody::Yrs(u) => u,
        EntryBody::Native(r) => unreachable!("a Yrs entry, not {r:?}"),
    }
}

#[test]
fn create_find_rekey() {
    let t = TmpDir::new("rekey");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("notes.md")).unwrap();
    assert_eq!(s.find(&key("notes.md")).unwrap(), Some(id));
    assert_eq!(
        s.create(&meta("notes.md")),
        Err(StoreError::KeyTaken(key("notes.md")))
    );
    let id2 = s.create(&meta("other.md")).unwrap();
    assert_ne!(id, id2);
    assert_eq!(
        s.rekey(id, key("other.md")),
        Err(StoreError::KeyTaken(key("other.md")))
    );
    s.rekey(id, key("renamed.md")).unwrap();
    assert_eq!(s.find(&key("notes.md")).unwrap(), None);
    assert_eq!(s.find(&key("renamed.md")).unwrap(), Some(id));
    assert_eq!(s.meta(id).unwrap().key, key("renamed.md"));
    let mut m = s.meta(id).unwrap();
    m.synced_seq = 9;
    m.declined = Some("too_large".into());
    s.put_meta(id, &m).unwrap();
    assert_eq!(s.meta(id).unwrap(), m);
    m.key = key("sneaky.md");
    assert_eq!(s.put_meta(id, &m), Err(StoreError::KeyChange));
}

#[test]
fn append_is_ordered_and_durable_across_reopen() {
    let t = TmpDir::new("durable");
    let id = {
        let s = DocStore::open(t.db()).unwrap();
        let id = s.create(&meta("a.md")).unwrap();
        assert_eq!(
            s.append(id, &[entry("p1", 10, vec![1]), entry("p2", 11, vec![2])])
                .unwrap(),
            vec![1, 2]
        );
        assert_eq!(s.append(id, &[entry("p1", 10, vec![3])]).unwrap(), vec![3]);
        id
    }; // dropped: file closed
    let s = DocStore::open(t.db()).unwrap();
    let l = s.load(id).unwrap();
    assert_eq!(l.snapshot, None);
    assert_eq!(
        l.updates
            .iter()
            .map(|(q, e)| (*q, yrs_bytes(e).to_vec()))
            .collect::<Vec<_>>(),
        vec![(1, vec![1]), (2, vec![2]), (3, vec![3])]
    );
    assert_eq!(l.updates[1].1.author, "p2");
    assert_eq!(s.log_size(id).unwrap(), (3, 3));
    assert_eq!(
        s.append(999, &[entry("x", 1, vec![])]),
        Err(StoreError::NoDoc(999))
    );
}

#[test]
fn compaction_keeps_later_entries_and_never_reuses_positions() {
    let t = TmpDir::new("compact");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    s.append(
        id,
        &[
            entry("p", 10, vec![1]),
            entry("p", 10, vec![2]),
            entry("p", 10, vec![3]),
        ],
    )
    .unwrap();
    s.compact(id, 2, b"state@2").unwrap();
    let l = s.load(id).unwrap();
    assert_eq!(l.snapshot, Some((2, b"state@2".to_vec())));
    assert_eq!(
        l.updates.iter().map(|(q, _)| *q).collect::<Vec<_>>(),
        vec![3]
    );
    s.compact(id, 3, b"state@3").unwrap();
    assert!(s.load(id).unwrap().updates.is_empty());
    // the log is empty now; the next position must still follow the snapshot
    assert_eq!(s.append(id, &[entry("p", 10, vec![4])]).unwrap(), vec![4]);
}

#[test]
fn versions_and_the_named_guard() {
    let t = TmpDir::new("versions");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let v = |seq, reason| Version {
        seq,
        at_ms: 7,
        reason,
        name: None,
        authors: vec!["agent:cortana".into(), "person:sam".into()],
        content: vec![(
            "source".into(),
            Neutral::Text(format!("text at {seq} 🎉\n")),
        )],
        codec: None,
    };
    let a = s.put_version(id, &v(1, VersionReason::Import)).unwrap();
    let b = s
        .put_version(id, &v(5, VersionReason::BeforeAgent))
        .unwrap();
    assert_eq!((a, b), (1, 2));
    assert_eq!(s.version(id, b).unwrap(), v(5, VersionReason::BeforeAgent));
    s.name_version(id, a, "first draft").unwrap();
    assert_eq!(
        s.delete_version(id, a),
        Err(StoreError::NamedVersion(id, a))
    );
    s.delete_version(id, b).unwrap();
    let all = s.versions(id).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].1.name.as_deref(), Some("first draft"));
    assert_eq!(s.version(id, b), Err(StoreError::NoVersion(id, b)));
}

/// `client_kinds` (after v0.10.2, no format bump): created by the first
/// [`DocStore::set_client_kind`], which writes the owner row with it; only an agent's ID is
/// reusable; an owner row without a kind row (an older kit's, `set_client`), an own
/// replica's, and a kind byte this kit doesn't know are not; a store without the table
/// reads as empty; a delete erases the rows, a rewrite keeps them.
#[test]
fn client_kinds_mark_only_agent_ids_reusable() {
    use redb::TableHandle;
    let t = TmpDir::new("client-kinds");
    let s = DocStore::open(t.db()).unwrap();
    let (a, b) = (
        s.create(&meta("a.md")).unwrap(),
        s.create(&meta("b.md")).unwrap(),
    );
    let has = |s: &DocStore| {
        let r = s.begin_read().unwrap();
        let found = r
            .list_tables()
            .unwrap()
            .any(|t| t.name() == CLIENT_KINDS.name());
        found
    };
    let set = |v: &[u64]| {
        v.iter()
            .copied()
            .collect::<std::collections::HashSet<u64>>()
    };
    // an owner row alone (as an older kit writes it): no table, nothing reusable
    s.set_client(a, 10, "bot").unwrap();
    assert!(!has(&s), "set_client creates no kind table");
    assert!(s.reusable_clients(a).unwrap().is_empty());
    // kinds: the owner row comes with them
    s.set_client_kind(a, 11, "bot", false).unwrap();
    s.set_client_kind(a, 12, "bot", true).unwrap();
    s.set_client_kind(b, 13, "bot", false).unwrap();
    assert!(has(&s));
    assert_eq!(s.client_owner(a, 11).unwrap().as_deref(), Some("bot"));
    assert_eq!(s.client_owner(a, 12).unwrap().as_deref(), Some("bot"));
    assert_eq!(
        s.reusable_clients(a).unwrap(),
        set(&[11]),
        "only the agent's ID: not the owner-only row, not the own replica's"
    );
    // a byte this kit doesn't know: never reusable
    {
        let w = s.begin_write().unwrap();
        w.open_table(CLIENT_KINDS)
            .unwrap()
            .insert((a, 14), [9u8].as_slice())
            .unwrap();
        w.open_table(CLIENTS)
            .unwrap()
            .insert((a, 14), "bot")
            .unwrap();
        w.commit().unwrap();
    }
    assert_eq!(s.reusable_clients(a).unwrap(), set(&[11]));
    // a rewrite keeps the rows; a delete erases the doc's, not another doc's
    s.rewrite_file().unwrap();
    assert_eq!(s.reusable_clients(a).unwrap(), set(&[11]));
    assert!(s.tables_holding(a).unwrap().contains(&"client_kinds"));
    s.delete_doc(a, 1).unwrap();
    assert!(s.tables_holding(a).unwrap().is_empty());
    assert!(s.reusable_clients(a).unwrap().is_empty());
    assert_eq!(s.reusable_clients(b).unwrap(), set(&[13]));
}

/// One row of doc `id` in table `t`, whatever its key shape.
fn plant_row(w: &redb::WriteTransaction, t: DocRows, id: DocId) {
    let v: &[u8] = b"row";
    match t {
        DocRows::One(d) => {
            w.open_table(d).unwrap().insert(id, v).unwrap();
        }
        DocRows::Pair(d) => {
            w.open_table(d).unwrap().insert((id, 7u64), v).unwrap();
        }
        DocRows::Owner(d) => {
            w.open_table(d).unwrap().insert((id, 7u64), "o").unwrap();
        }
        DocRows::Slot(d) => {
            w.open_table(d).unwrap().insert((id, 1u8), v).unwrap();
        }
        DocRows::Clock(d) => {
            w.open_table(d)
                .unwrap()
                .insert((id, 7u64, 8u64), v)
                .unwrap();
        }
        DocRows::Elem(d) => {
            w.open_table(d)
                .unwrap()
                .insert((id, "r", "k", 0u8), v)
                .unwrap();
        }
        DocRows::Named(d) => {
            w.open_table(d).unwrap().insert((id, "n"), v).unwrap();
        }
    }
}

/// The orphan sweep is one pass over a list of tables (K2 Cores' ruling, 11-02): a table on the
/// list is swept with nothing else written for it. (a) A list with one table of every key
/// shape, as a later additive table would register: only the deleted doc's rows go, past the
/// doc in the middle of the key range. (b) The kit's own list through a real open: a row of a
/// doc nobody has in `docs` goes, a live doc's stays, and every entry is also in [`DOC_TABLES`],
/// so a delete and a rewrite reach it. `MPC_STRUCT_MUTANT=sweep-skips-table` plants
/// `BlobRefMutant::SweepSkipsTable`, caught here.
#[test]
fn the_orphan_sweep_covers_every_table_on_its_list() {
    use crate::differential::{with_blob_ref_mutant, BlobRefMutant};
    use redb::TableDefinition as D;
    use redb::TableHandle;
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    let run = || {
        const SHAPES: &[Additive] = &[
            Additive {
                rows: DocRows::One(D::new("t_one")),
                on_orphan: None,
            },
            Additive {
                rows: DocRows::Pair(D::new("t_pair")),
                on_orphan: None,
            },
            Additive {
                rows: DocRows::Owner(D::new("t_owner")),
                on_orphan: None,
            },
            Additive {
                rows: DocRows::Slot(D::new("t_slot")),
                on_orphan: None,
            },
            Additive {
                rows: DocRows::Clock(D::new("t_clock")),
                on_orphan: None,
            },
            Additive {
                rows: DocRows::Elem(D::new("t_elem")),
                on_orphan: None,
            },
            Additive {
                rows: DocRows::Named(D::new("t_named")),
                on_orphan: None,
            },
        ];
        let d = TmpDir::new("sweep-shapes");
        let s = DocStore::open(d.db()).unwrap();
        let (a, c) = (
            s.create(&meta("a.md")).unwrap(),
            s.create(&meta("c.md")).unwrap(),
        );
        // the deleted doc sits between the two live ones
        let gone = s.create(&meta("b.md")).unwrap();
        let w = s.begin_write().unwrap();
        w.open_table(DOCS).unwrap().remove(gone).unwrap();
        for t in SHAPES {
            for id in [a, gone, c] {
                plant_row(&w, t.rows, id);
            }
        }
        sweep_orphans(&w, SHAPES).unwrap();
        w.commit().unwrap();
        let r = s.begin_read().unwrap();
        for t in SHAPES {
            assert!(
                !t.rows.holds(&r, gone).unwrap(),
                "{} kept a deleted doc's row",
                t.rows.name()
            );
            assert!(
                t.rows.holds(&r, a).unwrap(),
                "{} lost a live doc's row",
                t.rows.name()
            );
            assert!(
                t.rows.holds(&r, c).unwrap(),
                "{} lost a live doc's row",
                t.rows.name()
            );
        }
        drop(r);
        drop(s);

        // the kit's own list, through an open
        for t in ADDITIVE_TABLES {
            assert!(
                DOC_TABLES.iter().any(|(x, _)| x.name() == t.rows.name()),
                "{} is swept but neither deleted nor rewritten",
                t.rows.name()
            );
        }
        let d = TmpDir::new("sweep-list");
        let s = DocStore::open(d.db()).unwrap();
        let live = s.create(&meta("live.md")).unwrap();
        let w = s.begin_write().unwrap();
        for t in ADDITIVE_TABLES {
            for id in [live, live + 100] {
                if t.rows.name() == blob_refs::DOC_BLOB_REFS.name() {
                    // a hook reads its rows: a real one, with no count taken
                    w.open_table(blob_refs::DOC_BLOB_REFS)
                        .unwrap()
                        .insert(
                            (id, "ab".repeat(32).as_str()),
                            blob_refs::enc_ref(0).as_slice(),
                        )
                        .unwrap();
                } else {
                    plant_row(&w, t.rows, id);
                }
            }
        }
        w.commit().unwrap();
        drop(s);
        let s = DocStore::open(d.db()).unwrap();
        let r = s.begin_read().unwrap();
        for t in ADDITIVE_TABLES {
            assert!(
                !t.rows.holds(&r, live + 100).unwrap(),
                "an open left a deleted doc's row in {}",
                t.rows.name()
            );
            assert!(
                t.rows.holds(&r, live).unwrap(),
                "an open took {}'s live row",
                t.rows.name()
            );
        }
    };
    match m.as_deref() {
        Ok("sweep-skips-table") => with_blob_ref_mutant(BlobRefMutant::SweepSkipsTable, run),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this test plants"),
        Err(_) => run(),
    }
}

/// An older kit's doc delete erases the tables it knows, so it leaves `client_kinds` rows,
/// which show which of a deleted doc's client IDs were own replicas'. Every open removes them;
/// another doc's rows stay. A later downgrade is why every open sweeps: the second round
/// comes after opens that already swept. A store without the table stays without it.
/// `MPC_STRUCT_MUTANT=no-orphan-sweep` plants `ClientMutant::NoOrphanSweep`, caught here.
#[test]
fn an_open_removes_the_client_kinds_of_docs_an_older_kit_deleted() {
    use crate::differential::{with_client_mutant, ClientMutant};
    use redb::TableHandle;
    let m = std::env::var("MPC_STRUCT_MUTANT");
    println!("MPC_STRUCT_MUTANT={}", m.as_deref().unwrap_or("(unset)"));
    let run = || {
        // what an older kit's delete does: every table it knows, so not this one
        let older_delete = |s: &DocStore, id: DocId, path: &str| {
            let w = s.begin_write().unwrap();
            for (t, how) in DOC_TABLES {
                if *how == Erase::Range && t.name() != CLIENT_KINDS.name() {
                    t.erase(&w, id).unwrap();
                }
            }
            w.open_table(PATHS)
                .unwrap()
                .remove(key(path).index().as_str())
                .unwrap();
            w.commit().unwrap();
        };
        let rows = |s: &DocStore| -> Vec<(u64, u64)> {
            let r = s.begin_read().unwrap();
            let t = r.open_table(CLIENT_KINDS).unwrap();
            let out = t.iter().unwrap().map(|kv| kv.unwrap().0.value()).collect();
            out
        };
        let d = TmpDir::new("kinds-orphans");
        let s = DocStore::open(d.db()).unwrap();
        let (a, b, c) = (
            s.create(&meta("a.md")).unwrap(),
            s.create(&meta("b.md")).unwrap(),
            s.create(&meta("c.md")).unwrap(),
        );
        for (id, plain, own) in [(a, 11, 12), (b, 13, 14), (c, 15, 16)] {
            s.set_client_kind(id, plain, "bot", false).unwrap();
            s.set_client_kind(id, own, "bot", true).unwrap();
        }
        let (ra, rb, rc) = (
            vec![(a, 11), (a, 12)],
            vec![(b, 13), (b, 14)],
            vec![(c, 15), (c, 16)],
        );
        // the doc in the middle of the key range: the walk goes past it to the next
        older_delete(&s, b, "b.md");
        let all = [ra.clone(), rb.clone(), rc.clone()].concat();
        assert_eq!(rows(&s), all, "the older delete leaves them");
        drop(s);
        let s = DocStore::open(d.db()).unwrap();
        let kept = [ra.clone(), rc.clone()].concat();
        assert_eq!(
            rows(&s),
            kept,
            "the open removes the deleted doc's, and only those"
        );
        assert_eq!(s.reusable_clients(a).unwrap().len(), 1);
        // a downgrade after opens that swept: the first doc, then the last
        older_delete(&s, a, "a.md");
        drop(s);
        let s = DocStore::open(d.db()).unwrap();
        assert_eq!(rows(&s), rc, "a later downgrade's are removed too");
        older_delete(&s, c, "c.md");
        drop(s);
        let s = DocStore::open(d.db()).unwrap();
        assert_eq!(rows(&s), vec![], "the last doc's too");
        // a store without the table: an open doesn't create it
        let d2 = TmpDir::new("kinds-orphans-none");
        drop(DocStore::open(d2.db()).unwrap());
        let s2 = DocStore::open(d2.db()).unwrap();
        let r = s2.begin_read().unwrap();
        assert!(!r
            .list_tables()
            .unwrap()
            .any(|t| t.name() == CLIENT_KINDS.name()));
    };
    match m.as_deref() {
        Ok("no-orphan-sweep") => with_client_mutant(ClientMutant::NoOrphanSweep, run),
        Ok(other) => panic!("MPC_STRUCT_MUTANT={other}: not a mutant this test plants"),
        Err(_) => run(),
    }
}

#[test]
fn client_attribution() {
    let t = TmpDir::new("clients");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    s.set_client(id, 4242, "agent:cortana@ws-1").unwrap();
    assert_eq!(
        s.client_owner(id, 4242).unwrap().as_deref(),
        Some("agent:cortana@ws-1")
    );
    assert_eq!(s.client_owner(id, 1).unwrap(), None);
}

#[test]
fn refuses_a_newer_store_format() {
    let t = TmpDir::new("newer");
    {
        let db = Database::create(t.db()).unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", STORE_FORMAT_READ + 1)
            .unwrap();
        w.commit().unwrap();
    }
    assert_eq!(
        DocStore::open(t.db()).err(),
        Some(StoreError::Newer {
            found: STORE_FORMAT_READ + 1
        })
    );
}

/// The DocMeta record, version 1, is frozen (v0.10.1): a doc the native engine writes keeps
/// this layout, its stamp in `engine` / `engine_format`, so this build can read the stamp of
/// a doc it can't load and refuse it (MPC-E520) instead of misreading it. These bytes are
/// the layout; the decode is strict (a trailing byte, another version, a cut record are
/// damage).
#[test]
fn doc_meta_v1_layout_is_frozen_with_a_native_stamp() {
    let m = DocMeta {
        key: DocKey {
            space: "ws".into(),
            path: "a.md".into(),
        },
        pack: "markdown".into(),
        engine: "mpce-0.1".into(),
        engine_format: 1,
        codec: vec![1, 2],
        created_ms: 5,
        synced_hash: Some(vec![0xaa, 0xbb]),
        synced_seq: 3,
        declined: None,
    };
    let hex = "0102000000777304000000612e6d64080000006d61726b646f776e080000006d\
               7063652d302e310100000002000000010205000000000000000102000000aabb\
               030000000000000000";
    let want: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(enc_meta(&m), want, "DocMeta v1 bytes");
    assert_eq!(dec_meta(&want).unwrap(), m);
    let mut trailing = want.clone();
    trailing.push(0);
    assert!(dec_meta(&trailing).is_err(), "a trailing byte is damage");
    let mut v2 = want.clone();
    v2[0] = 2;
    assert_eq!(dec_meta(&v2), Err(StoreError::Corrupt("doc meta version")));
    assert!(dec_meta(&want[..want.len() - 1]).is_err(), "a cut record");
}

#[test]
fn damaged_records_are_errors_not_panics() {
    assert!(dec_meta(&[]).is_err());
    assert!(dec_meta(&[1, 255, 255, 255, 255]).is_err());
    assert!(dec_entry(&[2]).is_err());
    assert!(dec_version(&[1, 0, 0]).is_err());
    let mut good = enc_version(&Version {
        seq: 1,
        at_ms: 1,
        reason: VersionReason::Idle,
        name: None,
        authors: vec![],
        content: vec![],
        codec: None,
    });
    good.push(0); // trailing byte
    assert!(dec_version(&good).is_err());
    // a huge declared string count can't make the reader allocate
    let mut w = W::default();
    w.u8(1).u64(1).u64(1).u8(2).opt_str(None).u32(u32::MAX);
    assert!(dec_version(&w.0).is_err());
    let mut rng = 1u64;
    for _ in 0..20_000 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let n = (rng >> 58) as usize;
        let b: Vec<u8> = (0..n).map(|i| (rng >> (i % 8 * 8)) as u8).collect();
        let _ = (
            dec_meta(&b),
            dec_entry(&b),
            dec_version(&b),
            dec_snapshot(&b),
        );
    }
}

/// A commit reads only the snapshot's position (v0.9.8): the in-place decoder takes and
/// refuses exactly what the copying one does, on well-formed, truncated, padded and random
/// records.
#[test]
fn snapshot_seq_in_place_matches_the_copying_decoder() {
    let good = enc_snapshot(42, b"state bytes");
    assert_eq!(dec_snapshot_ref(&good).unwrap(), (42, &b"state bytes"[..]));
    assert_eq!(dec_snapshot(&good).unwrap(), (42, b"state bytes".to_vec()));
    let mut cases = vec![good.clone(), good[..good.len() - 1].to_vec(), {
        let mut b = good.clone();
        b.push(0);
        b
    }];
    let mut bad_version = good.clone();
    bad_version[0] = 2;
    cases.push(bad_version);
    let mut rng = 7u64;
    for _ in 0..5_000 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let n = (rng >> 58) as usize;
        cases.push((0..n).map(|i| (rng >> (i % 8 * 8)) as u8).collect());
    }
    for b in cases {
        let copied = dec_snapshot(&b);
        let in_place = dec_snapshot_ref(&b).map(|(s, v)| (s, v.to_vec()));
        assert_eq!(format!("{copied:?}"), format!("{in_place:?}"), "{b:?}");
    }
}

#[test]
fn doc_round_trips_through_log_and_compaction() {
    let t = TmpDir::new("doc");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let mut doc = Doc::new(S).unwrap();
    let log = |doc: &mut Doc, who: &str, client: u64, text: &str| {
        let u = doc
            .replace_text_as(client, "source", text)
            .unwrap()
            .unwrap();
        s.set_client(id, client, who).unwrap();
        s.append(id, &[entry(who, client, u)]).unwrap()[0]
    };
    log(&mut doc, "import", 2, "# Notes\n\nfirst\n");
    log(&mut doc, "agent:a", 501, "# Notes\n\nfirst\nsecond 日本\n");
    let upto = log(
        &mut doc,
        "person:sam",
        502,
        "# Notes (Sam)\n\nfirst\nsecond 日本\n",
    );
    // rebuild from the log alone
    let rebuilt = Doc::from_store(S, &s.load(id).unwrap()).unwrap();
    assert_eq!(rebuilt.text("source").unwrap(), doc.text("source").unwrap());
    // compact, keep logging, rebuild from snapshot + tail
    s.compact(id, upto, &doc.encode_state().unwrap()).unwrap();
    log(
        &mut doc,
        "agent:a",
        501,
        "# Notes (Sam)\n\nfirst\nsecond 日本\nthird 🎉\n",
    );
    let rebuilt = Doc::from_store(S, &s.load(id).unwrap()).unwrap();
    assert_eq!(
        rebuilt.text("source").unwrap(),
        "# Notes (Sam)\n\nfirst\nsecond 日本\nthird 🎉\n"
    );
    assert_eq!(rebuilt.state_vector(), doc.state_vector());
    // an engine-neutral version of it
    let vid = s
        .put_version(
            id,
            &Version {
                seq: upto + 1,
                at_ms: 9,
                reason: VersionReason::Idle,
                name: None,
                authors: vec!["agent:a".into()],
                content: rebuilt.export_neutral().unwrap(),
                codec: None,
            },
        )
        .unwrap();
    assert_eq!(
        s.version(id, vid).unwrap().content,
        vec![(
            "source".to_owned(),
            Neutral::Text(doc.text("source").unwrap())
        )]
    );
}

#[test]
fn create_imported_is_one_unit() {
    let t = TmpDir::new("import");
    let s = DocStore::open(t.db()).unwrap();
    let mut d = Doc::new(S).unwrap();
    let u = d.replace_text_as(77, "source", "hello\n").unwrap().unwrap();
    let v = Version {
        seq: 999, // overwritten with the update's position
        at_ms: 3,
        reason: VersionReason::Import,
        name: None,
        authors: vec!["kit:import".into()],
        content: vec![("source".into(), Neutral::Text("hello\n".into()))],
        codec: None,
    };
    let (id, seq, vid) = s
        .create_imported(
            &meta("a.md"),
            77,
            "kit:import",
            Some(&entry("kit:import", 77, u)),
            &v,
        )
        .unwrap();
    assert_eq!(seq, Some(1));
    assert_eq!(s.find(&key("a.md")).unwrap(), Some(id));
    assert_eq!(
        s.client_owner(id, 77).unwrap().as_deref(),
        Some("kit:import")
    );
    assert_eq!(s.version(id, vid).unwrap().seq, 1);
    let l = s.load(id).unwrap();
    assert_eq!(
        Doc::from_store(S, &l).unwrap().text("source").unwrap(),
        "hello\n"
    );
    // the next append continues after the import
    assert_eq!(s.append(id, &[entry("x", 9, vec![0])]).unwrap(), vec![2]);
    // a taken key changes nothing
    assert_eq!(
        s.create_imported(&meta("a.md"), 78, "kit:import", None, &v),
        Err(StoreError::KeyTaken(key("a.md")))
    );
    // an empty import: no update, version at position 0
    let (id2, seq2, vid2) = s
        .create_imported(&meta("empty.md"), 79, "kit:import", None, &v)
        .unwrap();
    assert_eq!(seq2, None);
    assert_eq!(s.version(id2, vid2).unwrap().seq, 0);
    assert!(s.load(id2).unwrap().updates.is_empty());
}

fn enc_entry_v1(e: &LogEntry) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .str(&e.author)
        .u64(e.client)
        .u64(e.at_ms)
        .str(&e.via)
        .bytes(yrs_bytes(e));
    w.0
}

fn enc_entry_v2(e: &LogEntry) -> Vec<u8> {
    let mut w = W::default();
    w.u8(2)
        .str(&e.author)
        .u64(e.client)
        .u64(e.at_ms)
        .str(&e.via)
        .bytes(yrs_bytes(e))
        .opt_str(e.on_behalf_of.as_deref());
    w.0
}

#[test]
fn log_records_v3_v2_and_v1_all_decode() {
    let mut e = entry("agent-a", 9, vec![1, 2, 3]);
    // v1 (stored before amendment 1): decodes with no on_behalf_of
    let v1 = enc_entry_v1(&e);
    assert_eq!(v1[0], 1);
    assert_eq!(dec_entry(&v1).unwrap(), e);
    // v2 (format 2) still decodes
    e.on_behalf_of = Some("alice".into());
    assert_eq!(dec_entry(&enc_entry_v2(&e)).unwrap(), e);
    e.on_behalf_of = None;
    // v3 round trips, with and without on_behalf_of
    let v3 = enc_entry(&e);
    assert_eq!(v3[0], 3, "new records are version 3");
    assert_eq!(dec_entry(&v3).unwrap(), e);
    e.on_behalf_of = Some("alice".into());
    assert_eq!(dec_entry(&enc_entry(&e)).unwrap(), e);
    // a purged entry keeps its metadata and length, not its update
    let red = enc_entry_as(&e, Some(3));
    let (d, n) = dec_entry_full(&red).unwrap();
    assert_eq!(n, Some(3));
    assert_eq!(d.body, EntryBody::Yrs(Vec::new()));
    assert_eq!(
        (d.author.as_str(), d.client, d.at_ms),
        ("agent-a", 9, e.at_ms)
    );
    assert_eq!(d.on_behalf_of.as_deref(), Some("alice"));
    // damaged records are loud
    let mut bad = enc_entry(&e);
    bad[0] = 5;
    assert_eq!(
        dec_entry(&bad),
        Err(StoreError::Corrupt("log entry version"))
    );
    // version 4 is a native entry's: a Yrs body's tag under it is damage
    bad[0] = 4;
    assert_eq!(dec_entry(&bad), Err(StoreError::Corrupt("log entry")));
    let mut trailing = v1.clone();
    trailing.push(0);
    assert!(
        dec_entry(&trailing).is_err(),
        "a v1 record with extra bytes"
    );
    assert!(dec_entry(&enc_entry(&e)[..v3.len() - 1]).is_err());
}

#[test]
fn v1_log_records_load_from_a_store() {
    let d = TmpDir::new("v1-records");
    let s = DocStore::open(d.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let mut doc = Doc::new(S).unwrap();
    let u = doc.replace_text_as(5, "source", "old\n").unwrap().unwrap();
    // write a v1 record directly, as a format-1 store holds it
    let w = s.begin_write().unwrap();
    w.open_table(UPDATES)
        .unwrap()
        .insert((id, 1), enc_entry_v1(&entry("a", 5, u)).as_slice())
        .unwrap();
    w.commit().unwrap();
    let l = s.load(id).unwrap();
    assert_eq!(l.updates[0].1.on_behalf_of, None);
    assert_eq!(
        Doc::from_store(S, &l).unwrap().text("source").unwrap(),
        "old\n"
    );
    // and v2 records append after it
    assert_eq!(s.append(id, &[entry("b", 6, vec![0])]).unwrap(), vec![2]);
}

#[test]
fn older_formats_are_stamped_current_and_newer_refused() {
    let d = TmpDir::new("format");
    {
        let s = DocStore::open(d.db()).unwrap();
        let w = s.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", 1u64)
            .unwrap();
        w.commit().unwrap();
    }
    {
        let s = DocStore::open(d.db()).unwrap();
        let r = s.begin_read().unwrap();
        let f = r
            .open_table(META)
            .unwrap()
            .get("store_format")
            .unwrap()
            .unwrap()
            .value();
        // stamped the base format: format 6 comes only with a structured doc
        assert_eq!(f, STORE_FORMAT_BASE);
        assert_eq!(
            (STORE_FORMAT_BASE, STORE_FORMAT, STORE_FORMAT_READ),
            (5, 6, 7)
        );
        drop(r);
        let w = s.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", 6u64)
            .unwrap();
        w.commit().unwrap();
    }
    // a format-6 store opens as is (no downgrade to 5)
    assert_eq!(DocStore::open(d.db()).unwrap().format().unwrap(), 6);
    {
        let s = DocStore::open(d.db()).unwrap();
        let w = s.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", 7u64)
            .unwrap();
        w.commit().unwrap();
    }
    // format 7 (v0.10.1: forward-declared, the native engine's) opens read-only and keeps
    // its stamp; it isn't rewritten here
    {
        let s = DocStore::open(d.db()).unwrap();
        assert_eq!(s.format().unwrap(), 7);
        assert_eq!(s.rewrite_file(), Err(StoreError::RewriteNewer { found: 7 }));
        assert_eq!(
            s.check_rewritable(),
            Err(StoreError::RewriteNewer { found: 7 })
        );
        let w = s.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", 8u64)
            .unwrap();
        w.commit().unwrap();
    }
    let e = DocStore::open(d.db()).err().expect("refused");
    assert_eq!(e, StoreError::Newer { found: 8 });
    assert_eq!(e.to_string(), "store is format 8; this build reads up to 7");
}

/// A blob doc in `s` (space `ws-1`) with its import by `kit:import`.
fn blob_doc(s: &DocStore, p: &str) -> DocId {
    let k = s.blob_space_or_create("ws-1", [7; 32]).unwrap();
    let b = crate::blob::BlobId::from_slice(&[1; 32]).unwrap();
    s.blob_index(k.no, &b, 3, 1).unwrap();
    let mut m = meta(p);
    m.pack = BLOB_DOC_PACK.into();
    let v = BlobVersion::new(b, 3, 1, "kit:import", "import", "disk", BlobReason::Import);
    s.create_blob_doc(&m, &v, None).unwrap().0
}

/// Version `n`'s blob (indexed in space `ws-1`).
fn blob_v(s: &DocStore, n: u8, author: &str, obo: Option<&str>) -> BlobVersion {
    let k = s.blob_space("ws-1").unwrap().unwrap();
    let b = crate::blob::BlobId::from_slice(&[n; 32]).unwrap();
    s.blob_index(k.no, &b, 3, 1).unwrap();
    let mut v = BlobVersion::new(
        b,
        3,
        10 * u64::from(n),
        author,
        author,
        "cli",
        BlobReason::Write,
    );
    v.on_behalf_of = obo.map(str::to_owned);
    v
}

/// K-T0-9 in the store: a blob version an agent wrote on someone's behalf gets a `Pending`
/// approval in the same commit, keyed `(doc, version)` in `blob_approvals`, never in the text
/// `approvals` table; a person's own version gets none; a doc's delete erases them.
#[test]
fn on_behalf_blob_versions_get_a_pending_approval_in_the_same_commit() {
    let d = TmpDir::new("blob-approvals");
    let s = DocStore::open(d.db()).unwrap();
    let id = blob_doc(&s, "a.pdf");
    let vs = [
        blob_v(&s, 2, "aide", Some("alice")),
        blob_v(&s, 3, "alice", None),
    ];
    assert_eq!(
        s.commit_blob_versions(id, "ws-1", 1, &vs, None).unwrap(),
        [2, 3]
    );
    assert_eq!(s.blob_approval(id, 1).unwrap(), None, "the import has none");
    assert_eq!(
        s.blob_approval(id, 3).unwrap(),
        None,
        "a person's own version has none"
    );
    let p = s.blob_approval(id, 2).unwrap().unwrap();
    assert_eq!(p, Approval::pending("aide", "alice"));
    assert_eq!(s.blob_approvals(id).unwrap(), vec![(2, p.clone())]);
    assert!(
        s.approvals(id).unwrap().is_empty(),
        "nothing in the text table"
    );
    assert_eq!(s.approval(id, 2).unwrap(), None);
    let done = p.settled(ApprovalState::Declined, "alice", 77);
    s.set_blob_approval(id, 2, &done).unwrap();
    assert_eq!(s.blob_approval(id, 2).unwrap(), Some(done));
    assert_eq!(
        s.set_blob_approval(id, 3, &p),
        Err(StoreError::NoApproval(id, 3))
    );
    // a version that moved (LogMoved) records no approval either
    assert!(matches!(
        s.commit_blob_versions(id, "ws-1", 2, &[blob_v(&s, 4, "aide", Some("alice"))], None),
        Err(StoreError::LogMoved(..))
    ));
    assert_eq!(s.blob_approvals(id).unwrap().len(), 1);
    // the delete erases them (tables_holding panics on a table it doesn't classify)
    assert!(s.tables_holding(id).unwrap().contains(&"blob_approvals"));
    s.delete_doc(id, 100).unwrap();
    assert!(s.tables_holding(id).unwrap().is_empty());
    assert!(s.blob_approvals(id).unwrap().is_empty());
}

/// Format 3 → 4 in place (the store layer; `tests/tier0.rs` opens a real v0.4.0 store): the
/// table is created and every on-behalf blob version gets its `Pending` approval in the
/// open's transaction, once; settled records and own versions are left alone.
#[test]
fn a_format_3_store_is_upgraded_in_place_with_its_approvals() {
    let d = TmpDir::new("format-3-4");
    let id = {
        let s = DocStore::open(d.db()).unwrap();
        let id = blob_doc(&s, "a.pdf");
        let vs = [
            blob_v(&s, 2, "aide", Some("alice")),
            blob_v(&s, 3, "alice", None),
            blob_v(&s, 4, "bot", Some("sam")),
        ];
        s.commit_blob_versions(id, "ws-1", 1, &vs, None).unwrap();
        // what a format-3 store holds: no blob_approvals table, stamped 3
        let w = s.begin_write().unwrap();
        assert!(w.delete_table(blob::BLOB_APPROVALS).unwrap());
        w.open_table(META)
            .unwrap()
            .insert("store_format", 3u64)
            .unwrap();
        w.commit().unwrap();
        id
    };
    let s = DocStore::open(d.db()).unwrap();
    let r = s.begin_read().unwrap();
    let f = r
        .open_table(META)
        .unwrap()
        .get("store_format")
        .unwrap()
        .unwrap()
        .value();
    assert_eq!(f, STORE_FORMAT_BASE);
    drop(r);
    assert_eq!(
        s.blob_approvals(id).unwrap(),
        vec![
            (2, Approval::pending("aide", "alice")),
            (4, Approval::pending("bot", "sam"))
        ]
    );
    // settled, then opened again: the backfill ran once (a format-4 store isn't touched)
    let done = Approval::pending("aide", "alice").settled(ApprovalState::Approved, "alice", 9);
    s.set_blob_approval(id, 2, &done).unwrap();
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(s.blob_approval(id, 2).unwrap(), Some(done));
    // the rewrite copies the new table (it refuses tables it doesn't know)
    s.rewrite_file().unwrap();
    assert_eq!(s.blob_approvals(id).unwrap().len(), 2);
}

/// Format 4 → 5 in place (the store layer; `tests/history.rs` reopens a session on such a
/// store): the provenance table is created in the open's transaction, nothing is backfilled
/// (every existing element is its own root), version-1 revert records still decode (with
/// unknown roots) and version-2 ones round-trip; the rewrite copies the new table, and a
/// delete erases its rows.
#[test]
fn a_format_4_store_is_upgraded_in_place_with_provenance() {
    use super::history::{dec_record, enc_record, PROVENANCE};
    use super::{HunkRoots, NewChange, ProvRow, RecordHunk, RecordRoot, RevertRecord};
    let d = TmpDir::new("format-4-5");
    let rec = |roots: Option<HunkRoots>| RevertRecord {
        author: "a".into(),
        on_behalf_of: None,
        at_ms: 5,
        roots: vec![RecordRoot {
            root: "source".into(),
            empty_after: false,
            hunks: vec![RecordHunk {
                start: 0,
                end: 1,
                insert: "xy".into(),
                deleted: "q".into(),
                redacted: false,
                roots,
            }],
        }],
        elems: Vec::new(),
    };
    // version 1, as a format-4 build wrote it: decodes with unknown roots
    let v1: Vec<u8> = {
        let mut b = enc_record(&rec(None)).unwrap();
        b[0] = 1;
        let n = b.len();
        b.truncate(n - 1); // version 1 has no roots byte
        b
    };
    assert_eq!(dec_record(&v1).unwrap(), rec(None));
    let with = rec(Some(HunkRoots {
        inserted: vec![(2, 2, 7, 0)],
        deleted: vec![(1, 1, 3, 4)],
    }));
    assert_eq!(dec_record(&enc_record(&with).unwrap()).unwrap(), with);
    let id = {
        let s = DocStore::open(d.db()).unwrap();
        let id = s.create(&meta("p.md")).unwrap();
        // what a format-4 store holds: no provenance table, stamped 4
        let w = s.begin_write().unwrap();
        assert!(w.delete_table(PROVENANCE).unwrap());
        w.open_table(META)
            .unwrap()
            .insert("store_format", 4u64)
            .unwrap();
        w.commit().unwrap();
        id
    };
    let s = DocStore::open(d.db()).unwrap();
    {
        let r = s.begin_read().unwrap();
        let f = r.open_table(META).unwrap();
        assert_eq!(f.get("store_format").unwrap().unwrap().value(), 5);
        assert_eq!(r.open_table(PROVENANCE).unwrap().iter().unwrap().count(), 0);
    }
    assert!(s.provenance_rows(id, &[(7, 0, 9)], Y).unwrap()[0].is_empty());
    // a change with provenance rows: written with it, read back by clock range
    let entry = LogEntry {
        author: "a".into(),
        client: 7,
        at_ms: 5,
        via: "cli".into(),
        body: EntryBody::Yrs(vec![1]),
        on_behalf_of: None,
    };
    let change = NewChange {
        info: ChangeInfo::new(ChangeKind::Undo, "a", "cli", 5),
        record: with,
        span: None,
        provenance: vec![ProvRow {
            client: 7,
            start: 2,
            end: 5,
            root: (3, 10),
        }],
        authored: None,
        row_marks: Vec::new(),
        col_remap: false,
        note_rows: Vec::new(),
        blob_refs: Default::default(),
    };
    s.commit_changes(id, &[entry], &[change], &[]).unwrap();
    let got = s
        .provenance_rows(id, &[(7, 4, 6), (7, 5, 9), (8, 0, 9)], Y)
        .unwrap();
    assert_eq!(got[0].len(), 1);
    assert_eq!(got[0][0].root, (3, 10));
    assert!(got[1].is_empty() && got[2].is_empty());
    s.rewrite_file().unwrap();
    assert_eq!(s.provenance_rows(id, &[(7, 0, 3)], Y).unwrap()[0].len(), 1);
    assert!(s.tables_holding(id).unwrap().contains(&"provenance"));
    s.delete_doc(id, 100).unwrap();
    assert!(s.tables_holding(id).unwrap().is_empty());
}

#[test]
fn on_behalf_entries_get_a_pending_approval_in_the_same_append() {
    let d = TmpDir::new("approvals");
    let s = DocStore::open(d.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let mut doc = Doc::new(S).unwrap();
    let u1 = doc.replace_text_as(5, "source", "a\n").unwrap().unwrap();
    let u2 = doc.replace_text_as(6, "source", "a\nb\n").unwrap().unwrap();
    let mut obo = entry("helper", 6, u2);
    obo.on_behalf_of = Some("alice".into());
    let seqs = s.append(id, &[entry("alice", 5, u1), obo]).unwrap();
    assert_eq!(seqs, vec![1, 2]);
    assert_eq!(s.approval(id, 1).unwrap(), None, "own edits have none");
    let p = s.approval(id, 2).unwrap().unwrap();
    assert_eq!(p, Approval::pending("helper", "alice"));
    assert_eq!(
        (p.state, p.by.as_deref(), p.at_ms),
        (ApprovalState::Pending, None, None)
    );
    // compaction drops the log entry but not the approval
    s.compact(id, 2, &doc.encode_state().unwrap()).unwrap();
    assert!(s.load(id).unwrap().updates.is_empty());
    let done = p.settled(ApprovalState::Declined, "alice", 77);
    s.set_approval(id, 2, &done).unwrap();
    assert_eq!(s.approval(id, 2).unwrap(), Some(done.clone()));
    assert_eq!(s.approvals(id).unwrap(), vec![(2, done)]);
    // only existing records can be set
    assert_eq!(
        s.set_approval(id, 1, &p),
        Err(StoreError::NoApproval(id, 1))
    );
    // other docs see nothing
    let id2 = s.create(&meta("b.md")).unwrap();
    assert!(s.approvals(id2).unwrap().is_empty());
    // a damaged record is loud
    assert!(dec_approval(&[1, 9]).is_err());
    assert!(dec_approval(&[]).is_err());
}

#[test]
fn host_table_is_opaque_and_transactional() {
    let d = TmpDir::new("host-kv");
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(s.host_get("ns", "k").unwrap(), None);
    s.host_put("ns", "a/1", b"one").unwrap();
    s.host_put("ns", "a/2", b"two").unwrap();
    s.host_put("ns", "b/1", b"three").unwrap();
    s.host_put("ns2", "a/1", b"other ns").unwrap();
    s.host_put("n", "s", b"prefix of ns").unwrap();
    s.host_put("ns", "a/1", b"ONE").unwrap(); // overwrite
    assert_eq!(
        s.host_get("ns", "a/1").unwrap().as_deref(),
        Some(&b"ONE"[..])
    );
    assert_eq!(
        s.host_scan("ns", "a/").unwrap(),
        vec![
            ("a/1".to_owned(), b"ONE".to_vec()),
            ("a/2".to_owned(), b"two".to_vec())
        ]
    );
    assert_eq!(s.host_scan("ns", "").unwrap().len(), 3, "namespace only");
    assert!(s.host_scan("ns", "c").unwrap().is_empty());
    assert!(s.host_scan("nope", "").unwrap().is_empty());
    assert!(s.host_delete("ns", "a/2").unwrap());
    assert!(!s.host_delete("ns", "a/2").unwrap());
    s.host_batch(&[
        HostOp::Put {
            ns: "ns",
            key: "c",
            value: b"",
        },
        HostOp::Delete {
            ns: "ns",
            key: "b/1",
        },
    ])
    .unwrap();
    assert_eq!(s.host_get("ns", "c").unwrap(), Some(vec![]));
    assert_eq!(s.host_get("ns", "b/1").unwrap(), None);
    // it survives reopening
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(s.host_scan("ns", "").unwrap().len(), 2);
    assert_eq!(
        s.host_get("ns2", "a/1").unwrap().as_deref(),
        Some(&b"other ns"[..])
    );
}

/// v0.9.5 (rollback): a store holding a table this kit doesn't know (one a newer kit added
/// without a format bump) is refused for a rewrite with `StoreError::NewerData` (the
/// registry's `MPC-E502 store_newer`), its message naming the table and the kit to upgrade
/// to, and the store file is left exactly as it was. Both branches: an unknown table and an
/// unknown multimap table. Made through redb's normal API. (v0.9.7's `authored` is the
/// kit's own table now: `authored_hunks_side_table` rewrites a store holding it.)
#[test]
fn a_newer_kits_table_refuses_the_rewrite_and_changes_nothing() {
    for (name, multimap) in [("later_kit", false), ("later_kit_mm", true)] {
        let d = TmpDir::new(&format!("newer-table-{name}"));
        let s = DocStore::open(d.db()).unwrap();
        s.create(&meta("a.md")).unwrap();
        s.add_table_for_tests(name, multimap).unwrap();
        let before = std::fs::read(d.db()).unwrap();
        let want = StoreError::NewerData {
            table: name.to_owned(),
        };
        assert_eq!(s.check_rewritable(), Err(want.clone()), "{name}");
        assert_eq!(s.rewrite_file().err(), Some(want.clone()), "{name}");
        let kit = "a newer kit";
        assert_eq!(
            want.to_string(),
            format!(
                "This store holds data from a newer kit (table \"{name}\"). Upgrade K2 to a \
                 version with {kit} to purge or rewrite it; reading and editing still work."
            )
        );
        assert_eq!(
            std::fs::read(d.db()).unwrap(),
            before,
            "{name}: the store unchanged"
        );
        assert!(
            !rewrite::temp_path(&d.db()).exists(),
            "{name}: no temp file left"
        );
        // reading still works
        assert!(s.find(&key("a.md")).unwrap().is_some());
    }
}

/// Every table a fresh store has is in exactly one of the kit's lists (v0.9.7): the
/// per-doc tables a delete erases and a rewrite copies ([`DOC_TABLES`]) or the shared ones
/// ([`SHARED_TABLES`]); and every listed table exists, but `authored`, which only the first
/// widened change creates (lazily: a store without one stays purgeable by a kit <= v0.9.6).
/// A table added to `open` but to neither list fails here, not in a purge's rewrite.
#[test]
fn every_table_is_listed_once() {
    use redb::TableHandle;
    let d = TmpDir::new("tables");
    let s = DocStore::open(d.db()).unwrap();
    let tables = |s: &DocStore| -> Vec<String> {
        let r = s.begin_read().unwrap();
        let mut v: Vec<String> = r
            .list_tables()
            .unwrap()
            .map(|t| t.name().to_owned())
            .collect();
        v.sort();
        v
    };
    let found = tables(&s);
    let mut listed: Vec<String> = SHARED_TABLES
        .iter()
        .map(|n| (*n).to_owned())
        .chain(DOC_TABLES.iter().map(|(t, _)| t.name().to_owned()))
        .collect();
    listed.sort();
    let n = listed.len();
    listed.dedup();
    assert_eq!(listed.len(), n, "a table is listed twice");
    // `authored` comes with the first widened change; `elem_writes` with the first
    // structured record (format 6's upgrade, C1); `client_kinds` with the first join that
    // mints an agent's or an own replica's client ID; `doc_blob_refs` with the first name
    let lazy = [
        "authored".to_owned(),
        "elem_writes".to_owned(),
        "row_marks".to_owned(),
        "overwrite_notes".to_owned(),
        "client_kinds".to_owned(),
        "doc_blob_refs".to_owned(),
        "blob_struct".to_owned(),
        "blob_struct_keys".to_owned(),
        // the annotations' three, with a doc's first comment
        "annot_threads".to_owned(),
        "annot_comments".to_owned(),
        "annot_log".to_owned(),
        "annot_thread_log".to_owned(),
        "annot_changed".to_owned(),
    ];
    let eager: Vec<String> = listed
        .iter()
        .filter(|t| !lazy.contains(t))
        .cloned()
        .collect();
    assert_eq!(
        found, eager,
        "a fresh store: every listed table but the lazy ones"
    );
    assert!(lazy.iter().all(|t| listed.contains(t)));
    // nothing but a widened change creates it: not a delete, a window prune, a purge, a
    // rewrite or a reopen
    let id = s.create(&meta("a.md")).unwrap();
    s.prune_reverts(id, &[1], &[]).unwrap();
    let needles = Needles::new([&b"nothing-here"[..]], false);
    s.purge_doc(
        id,
        &DocPurge {
            head: 0,
            state: &[],
            changes: Pick::None,
            versions: Pick::None,
            needles: &needles,
            sync: None,
            key: [7; 32],
            erase: None,
            elems: None,
            ref_roots: &[],
            live_refs: Default::default(),
            now: 0,
        },
    )
    .unwrap();
    s.delete_doc(id, 1).unwrap();
    s.rewrite_file().unwrap();
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(tables(&s), eager);
}

/// A downgrade to a kit <= v0.9.6 and back (v0.9.7, no format bump): the older kit drops a
/// record (a delete, its window) and redacts one (a purge) without touching the authored
/// rows. The next open deletes the rows whose record is gone, blanks the inserts their
/// redacted record no longer holds, and rewrites the file: no byte of them is left.
#[test]
fn an_open_sweeps_what_an_older_kit_left_in_the_authored_table() {
    use super::history::{dec_record, enc_record, AUTHORED, REVERTS};
    use super::{NewChange, RecordHunk, RecordRoot, RevertRecord};
    use crate::text::Hunk;

    let d = TmpDir::new("authored-sweep");
    let s = DocStore::open(d.db()).unwrap();
    let a = s.create(&meta("a.md")).unwrap();
    let b = s.create(&meta("b.md")).unwrap();
    let hunk = |start: usize, end: usize, insert: &str| Hunk {
        start,
        end,
        insert: insert.into(),
    };
    let widened = |secret: &str| NewChange {
        info: ChangeInfo::new(ChangeKind::Edit, "ag", "cli", 5),
        record: RevertRecord {
            author: "ag".into(),
            on_behalf_of: None,
            at_ms: 5,
            roots: vec![RecordRoot {
                root: "source".into(),
                empty_after: false,
                hunks: vec![
                    RecordHunk {
                        start: 0,
                        end: 5,
                        insert: format!("wo{secret}"),
                        deleted: "wordX".into(),
                        redacted: false,
                        roots: None,
                    },
                    RecordHunk {
                        start: 6,
                        end: 10,
                        insert: "SEED".into(),
                        deleted: "seed".into(),
                        redacted: false,
                        roots: None,
                    },
                ],
            }],
            elems: Vec::new(),
        },
        span: None,
        provenance: Vec::new(),
        authored: Some(vec![(
            "source".into(),
            vec![hunk(2, 5, secret), hunk(6, 10, "SEED")],
        )]),
        row_marks: Vec::new(),
        col_remap: false,
        note_rows: Vec::new(),
        blob_refs: Default::default(),
    };
    let (sa, sb) = ("ALPHA-SECRET-QXJZ", "BRAVO-SECRET-VKWP");
    for (id, secret) in [(a, sa), (b, sb)] {
        s.commit_changes(id, &[entry("ag", 7, vec![1])], &[widened(secret)], &[])
            .unwrap();
        assert!(s.authored_hunks(id, 1).unwrap().is_some());
    }
    // the older kit: deletes doc `a` (every table it knows, so not `authored`) and purges
    // doc `b` by its secret (the record's hunk redacted, its rewrite refused: the
    // authored row and the freed pages keep the text)
    let w = s.begin_write().unwrap();
    for (t, how) in DOC_TABLES {
        if *how == Erase::Range && t.name() != "authored" {
            t.erase(&w, a).unwrap();
        }
    }
    w.open_table(PATHS)
        .unwrap()
        .remove(key("a.md").index().as_str())
        .unwrap();
    // a store from before v0.10.1 (no sweep marker: an open since sweeps once)
    w.open_table(META)
        .unwrap()
        .remove(AUTHORED_SWEPT_KEY)
        .unwrap();
    {
        let mut rt = w.open_table(REVERTS).unwrap();
        let mut rec = dec_record(rt.get((b, 1)).unwrap().unwrap().value()).unwrap();
        let h = &mut rec.roots[0].hunks[0];
        h.redacted = true;
        h.insert.clear();
        h.deleted.clear();
        rt.insert((b, 1), enc_record(&rec).unwrap().as_slice())
            .unwrap();
    }
    w.commit().unwrap();
    let rows = |s: &DocStore| -> Vec<(u64, u64)> {
        let r = s.begin_read().unwrap();
        let t = r.open_table(AUTHORED).unwrap();
        let v = redb::ReadableTable::iter(&t)
            .unwrap()
            .map(|kv| kv.unwrap().0.value())
            .collect();
        v
    };
    assert_eq!(rows(&s), vec![(a, 1), (b, 1)]);
    let needles = [sa.as_bytes(), sb.as_bytes()];
    assert!(
        !s.scan_for(&needles).unwrap().is_empty(),
        "the older kit left both"
    );
    drop(s);

    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(rows(&s), vec![(b, 1)], "a's row went with its record");
    assert_eq!(
        s.authored_hunks(b, 1).unwrap(),
        Some(vec![(
            "source".into(),
            vec![hunk(2, 5, ""), hunk(6, 10, "SEED")]
        )]),
        "the insert its redacted hunk held is blank; the other stays"
    );
    assert_eq!(
        s.scan_for(&needles).unwrap(),
        Vec::new(),
        "the byte scan is clean"
    );
    {
        let r = s.begin_read().unwrap();
        assert!(r
            .open_table(META)
            .unwrap()
            .get(REWRITE_PENDING)
            .unwrap()
            .is_none());
    }
    // nothing left to sweep: the next open changes nothing
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(rows(&s), vec![(b, 1)]);
}

/// The sweep marker's `meta` key, spelled out so these tests compile without the fix.
const AUTHORED_SWEPT_KEY: &str = "authored_swept";

/// For the once-per-store sweep (v0.10.1): a store at `format` made by a kit before the marker
/// (no `authored_swept`), holding one doc and a row a kit <= v0.9.6 left behind (its record
/// is gone). Returns the dir (keep it alive) and the doc.
fn store_with_leftover(tag: &str, format: u64) -> (TmpDir, DocId) {
    let d = TmpDir::new(tag);
    let s = DocStore::open(d.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let w = s.begin_write().unwrap();
    {
        let mut m = w.open_table(META).unwrap();
        m.remove(AUTHORED_SWEPT_KEY).unwrap();
        m.insert("store_format", format).unwrap();
    }
    w.commit().unwrap();
    plant_leftover(&s, id);
    (d, id)
}

/// A row in `authored` whose record is gone, as a kit <= v0.9.6 leaves one.
fn plant_leftover(s: &DocStore, id: DocId) {
    use super::history::{enc_authored, AUTHORED};
    let row = enc_authored(&[(
        "source".into(),
        vec![crate::text::Hunk {
            start: 0,
            end: 1,
            insert: "LEFT-BY-AN-OLDER-KIT".into(),
        }],
    )]);
    let w = s.begin_write().unwrap();
    w.open_table(AUTHORED)
        .unwrap()
        .insert((id, 1), row.as_slice())
        .unwrap();
    w.commit().unwrap();
}

fn authored_rows(s: &DocStore) -> Vec<(u64, u64)> {
    let r = s.begin_read().unwrap();
    let t = r.open_table(super::history::AUTHORED).unwrap();
    let v = redb::ReadableTable::iter(&t)
        .unwrap()
        .map(|kv| kv.unwrap().0.value())
        .collect();
    v
}

fn swept_marker(s: &DocStore) -> Option<u64> {
    let r = s.begin_read().unwrap();
    let v = r
        .open_table(META)
        .unwrap()
        .get(AUTHORED_SWEPT_KEY)
        .unwrap()
        .map(|v| v.value());
    v
}

/// v0.10.1: an open sweeps the authored table **once per store**, and marks the store in the
/// sweep's own transaction, whatever it found; later opens skip it (it reads every row:
/// O(the changes D1 widened), about half of a reopen at 100k versions). The trade-off,
/// accepted: a row an older kit leaves after the marker stays (no shipped release ran a kit
/// <= v0.9.6). At format 6, as at 5.
#[test]
fn an_open_sweeps_the_authored_table_once_and_marks_the_store() {
    let (d, id) = store_with_leftover("authored-sweep-once-6", 6);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(
        authored_rows(&s),
        vec![],
        "the first open sweeps the leftover"
    );
    assert_eq!(swept_marker(&s), Some(1), "and marks the store");
    plant_leftover(&s, id);
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(
        authored_rows(&s),
        vec![(id, 1)],
        "a marked store isn't swept again: the row stays"
    );
    assert_eq!(swept_marker(&s), Some(1));
    assert_eq!(s.format().unwrap(), 6);
}

/// v0.10.1: a text-only store stays at format 5 (no bump; a v0.9.x kit still opens it) and
/// is swept on its first open only. A new store is marked by the open that creates it.
#[test]
fn a_format_5_text_only_store_is_swept_on_its_first_open_only() {
    let fresh = TmpDir::new("authored-sweep-fresh");
    let s = DocStore::open(fresh.db()).unwrap();
    assert_eq!(swept_marker(&s), Some(1), "a new store is marked at once");
    drop(s);

    let (d, id) = store_with_leftover("authored-sweep-once-5", STORE_FORMAT_BASE);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(authored_rows(&s), vec![], "the first open sweeps");
    assert_eq!(swept_marker(&s), Some(1));
    plant_leftover(&s, id);
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    assert_eq!(authored_rows(&s), vec![(id, 1)], "the second doesn't");
    assert_eq!(s.format().unwrap(), STORE_FORMAT_BASE, "still format 5");
}

/// The authored hunks of a change D1 widened (v0.9.7): written in the change's own
/// transaction only when they differ from the record's, read back as written, and the
/// record's hunks otherwise; they leave the window with the record, a rewrite keeps them,
/// a delete erases them, and a purge blanks every insert its record lost.
#[test]
fn authored_hunks_side_table() {
    use super::history::{AUTHORED, REVERTS};
    use super::{NewChange, RecordHunk, RecordRoot, RevertRecord};
    use crate::text::Hunk;

    let d = TmpDir::new("authored");
    let s = DocStore::open(d.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let hunk = |start: usize, end: usize, insert: &str| Hunk {
        start,
        end,
        insert: insert.into(),
    };
    // `wordX` (someone else's) and `seed`; the change makes them `woRD-SECRET1` and `SEED`
    let record = |hunks: &[(u64, u64, &str, &str)]| RevertRecord {
        author: "ag".into(),
        on_behalf_of: None,
        at_ms: 5,
        roots: vec![RecordRoot {
            root: "source".into(),
            empty_after: false,
            hunks: hunks
                .iter()
                .map(|&(start, end, insert, deleted)| RecordHunk {
                    start,
                    end,
                    insert: insert.into(),
                    deleted: deleted.into(),
                    redacted: false,
                    roots: None,
                })
                .collect(),
        }],
        elems: Vec::new(),
    };
    let change = |rec: RevertRecord, authored: Option<RootHunks>| NewChange {
        info: ChangeInfo::new(ChangeKind::Edit, "ag", "cli", 5),
        record: rec,
        span: None,
        provenance: Vec::new(),
        authored,
        row_marks: Vec::new(),
        col_remap: false,
        note_rows: Vec::new(),
        blob_refs: Default::default(),
    };
    let widened = record(&[(0, 5, "woRD-SECRET1", "wordX"), (6, 10, "SEED", "seed")]);
    let authored: RootHunks = vec![(
        "source".into(),
        vec![hunk(2, 5, "RD-SECRET1"), hunk(6, 10, "SEED")],
    )];
    let plain = record(&[(0, 0, "new ", "")]);
    let plain_hunks: RootHunks = vec![("source".into(), vec![hunk(0, 0, "new ")])];
    let seqs = s
        .commit_changes(
            id,
            &[
                entry("ag", 7, vec![1]),
                entry("ag", 7, vec![2]),
                entry("ag", 7, vec![3]),
            ],
            &[
                change(widened.clone(), Some(authored.clone())),
                // a session that names the record's own hunks writes no row either
                change(plain.clone(), Some(plain_hunks.clone())),
                change(plain, None),
            ],
            &[],
        )
        .unwrap();
    assert_eq!(seqs, vec![1, 2, 3]);
    assert_eq!(s.authored_hunks(id, 1).unwrap(), Some(authored.clone()));
    assert_eq!(s.authored_hunks(id, 2).unwrap(), None);
    assert_eq!(s.authored_hunks(id, 3).unwrap(), None);
    assert_eq!(s.change_hunks(id, 1).unwrap(), Some(authored.clone()));
    assert_eq!(s.change_hunks(id, 2).unwrap(), Some(plain_hunks.clone()));
    assert_eq!(s.change_hunks(id, 3).unwrap(), Some(plain_hunks));
    assert_eq!(s.change_hunks(id, 9).unwrap(), None);
    // the encoding: canonical, strict
    let b = history::enc_authored(&authored);
    assert_eq!(history::dec_authored(&b).unwrap(), authored);
    let mut long = b.clone();
    long.push(0);
    assert!(history::dec_authored(&long).is_err());
    let mut v2 = b;
    v2[0] = 2;
    assert!(history::dec_authored(&v2).is_err());

    // a rewrite copies the row; leaving the window drops it with its record
    s.rewrite_file().unwrap();
    assert_eq!(s.authored_hunks(id, 1).unwrap(), Some(authored.clone()));
    let w2 = s.begin_write().unwrap();
    history::put_changes(
        &w2,
        id,
        &[4],
        &[change(widened.clone(), Some(authored.clone()))],
    )
    .unwrap();
    w2.commit().unwrap();
    s.prune_reverts(id, &[4], &[]).unwrap();
    assert_eq!(s.authored_hunks(id, 4).unwrap(), None);
    assert_eq!(s.change_hunks(id, 4).unwrap(), None);

    // a purge by needle: the record's first hunk is redacted, so the authored hunk inside it
    // is blanked; the second (not matched) keeps its insert and every position stays
    let needles = Needles::new([&b"SECRET1"[..], &b"woRD-SECRET1"[..]], false);
    let purge = |changes: Pick| DocPurge {
        head: 3,
        state: &[],
        changes,
        versions: Pick::None,
        needles: &needles,
        sync: None,
        key: [7; 32],
        erase: None,
        elems: None,
        ref_roots: &[],
        live_refs: Default::default(),
        now: 0,
    };
    s.purge_doc(id, &purge(Pick::None)).unwrap();
    let rec = s.revert_record(id, 1).unwrap().unwrap();
    assert!(rec.roots[0].hunks[0].redacted && !rec.roots[0].hunks[1].redacted);
    let blanked: RootHunks = vec![("source".into(), vec![hunk(2, 5, ""), hunk(6, 10, "SEED")])];
    assert_eq!(s.authored_hunks(id, 1).unwrap(), Some(blanked.clone()));
    assert_eq!(s.change_hunks(id, 1).unwrap(), Some(blanked));
    // taken whole: every insert goes
    s.purge_doc(id, &purge(Pick::Set([1].into()))).unwrap();
    assert_eq!(
        s.authored_hunks(id, 1).unwrap(),
        Some(vec![(
            "source".into(),
            vec![hunk(2, 5, ""), hunk(6, 10, "")]
        )])
    );
    // an authored row whose record is gone is deleted by a purge
    let w3 = s.begin_write().unwrap();
    w3.open_table(REVERTS).unwrap().remove((id, 1)).unwrap();
    w3.commit().unwrap();
    s.purge_doc(id, &purge(Pick::None)).unwrap();
    assert!(s
        .begin_read()
        .unwrap()
        .open_table(AUTHORED)
        .unwrap()
        .get((id, 1))
        .unwrap()
        .is_none());

    // a delete erases the table's rows (tables_holding lists every per-doc table)
    let w4 = s.begin_write().unwrap();
    history::put_changes(&w4, id, &[5], &[change(widened, Some(authored))]).unwrap();
    w4.commit().unwrap();
    assert!(s.tables_holding(id).unwrap().contains(&"authored"));
    s.delete_doc(id, 100).unwrap();
    assert!(s.tables_holding(id).unwrap().is_empty());
}

/// v0.9.8: a write-back's promotion from the bytes it just recorded leaves exactly the rows
/// the read-back promotion does (the synced state, the metadata, no write in flight), and
/// refuses the same way when nothing is in flight.
#[test]
fn promote_written_sync_equals_promote_pending_sync() {
    let t = TmpDir::new("promote-written");
    let s = DocStore::open(t.db()).unwrap();
    let a = s.create(&meta("a.md")).unwrap();
    let b = s.create(&meta("b.md")).unwrap();
    let sync = SyncState {
        hash: vec![7; 32],
        seq: 3,
        state: (0..5_000u32).map(|i| (i % 251) as u8).collect(),
    };
    let mut m = s.meta(a).unwrap();
    m.synced_hash = Some(sync.hash.clone());
    m.synced_seq = sync.seq;
    // a: the read-back way; b: from the recorded bytes
    s.set_pending_sync(a, &sync).unwrap();
    s.promote_pending_sync(a, &m).unwrap();
    let mut mb = s.meta(b).unwrap();
    mb.synced_hash = m.synced_hash.clone();
    mb.synced_seq = m.synced_seq;
    let p = s.set_pending_sync(b, &sync).unwrap();
    s.promote_written_sync(b, &mb, &p).unwrap();
    assert_eq!(s.sync_states(a).unwrap(), (Some(sync.clone()), None));
    assert_eq!(s.sync_states(b).unwrap(), s.sync_states(a).unwrap());
    assert_eq!(
        s.meta(b).unwrap().synced_hash,
        s.meta(a).unwrap().synced_hash
    );
    assert_eq!(s.meta(b).unwrap().synced_seq, s.meta(a).unwrap().synced_seq);
    // nothing in flight any more: both refuse
    assert!(matches!(
        s.promote_pending_sync(a, &m),
        Err(StoreError::Corrupt(_))
    ));
    assert!(matches!(
        s.promote_written_sync(b, &mb, &p),
        Err(StoreError::Corrupt(_))
    ));
}

/// One value-naming change, as a session makes it.
fn ref_change(names: &[crate::blob::BlobId], strict: bool) -> NewChange {
    let ops = names
        .iter()
        .enumerate()
        .map(|(i, b)| structured::ElemOpRecord {
            kind: structured::ElemOpKind::MapSet,
            key: format!("i{i}"),
            write: None,
            effect: structured::StoredEffect::Visible,
            left: Vec::new(),
            value_before: None,
            value_after: Some(crate::elem::Value::Str(b.to_ref())),
            redacted: false,
            prev: None,
            restores: None,
        })
        .collect();
    NewChange {
        info: ChangeInfo::new(ChangeKind::Edit, "p", "cli", 1_000),
        record: RevertRecord {
            author: "p".into(),
            on_behalf_of: None,
            at_ms: 1_000,
            roots: Vec::new(),
            elems: vec![structured::ElemRoot {
                root: "props".into(),
                list: false,
                ops,
            }],
        },
        span: None,
        provenance: Vec::new(),
        authored: None,
        row_marks: Vec::new(),
        col_remap: false,
        note_rows: Vec::new(),
        blob_refs: BlobRefScan {
            roots: &["props"],
            strict,
        },
    }
}

/// The space of [`key`], with one indexed blob per byte in `seeds`.
fn blobs_in(s: &DocStore, seeds: &[u8]) -> (u64, Vec<crate::blob::BlobId>) {
    let k = s.blob_space_or_create(&key("a.md").space, [7; 32]).unwrap();
    let ids = seeds
        .iter()
        .map(|b| {
            let id = crate::blob::BlobId::from_slice(&[*b; 32]).unwrap();
            s.blob_index(k.no, &id, 10, 1_000).unwrap();
            id
        })
        .collect();
    (k.no, ids)
}

/// Blob GC runs outside the session's lock, so a blob can go between the session's check and the
/// change's commit. The change's own transaction finds that, for a connection's edit: nothing of
/// it is stored, and the refusal is `MPC-E720`. Any other change is kept, with a row saying no
/// count was taken.
#[test]
fn a_connections_change_naming_a_blob_gone_since_its_check_stores_nothing() {
    let d = TmpDir::new("refs-gone");
    let s = DocStore::open(d.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let (no, ids) = blobs_in(&s, &[1]);
    s.blob_remove_unreferenced(&[(no, ids[0], 10)]).unwrap();
    let e = entry("p", 5, vec![1]);
    let refused = s.commit_changes(id, std::slice::from_ref(&e), &[ref_change(&ids, true)], &[]);
    assert_eq!(refused, Err(StoreError::BlobRefUnknown(ids[0].to_hex())));
    assert_eq!(s.log_size(id).unwrap().0, 0, "the log entry went with it");
    assert_eq!(s.blob_refs(id).unwrap(), Default::default());
    s.commit_changes(
        id,
        std::slice::from_ref(&e),
        &[ref_change(&ids, false)],
        &[],
    )
    .unwrap();
    let refs = s.blob_refs(id).unwrap();
    assert_eq!(refs.missing, 1);
    assert_eq!(refs.refs[0].state, crate::blob::BlobRefState::Missing);
}

/// An older kit's doc delete leaves the doc's rows and their counts, which an open gives back.
/// The blob keeps its time: an open has no host clock, and nothing names it. Another doc's row
/// and count stay, and a count an older delete left can't take a live doc's below its own.
#[test]
fn an_open_gives_back_the_references_of_a_doc_an_older_kit_deleted() {
    use redb::TableHandle;
    let d = TmpDir::new("refs-orphans");
    let s = DocStore::open(d.db()).unwrap();
    let (a, b) = (
        s.create(&meta("a.md")).unwrap(),
        s.create(&meta("b.md")).unwrap(),
    );
    let (no, ids) = blobs_in(&s, &[1, 2]);
    let e = entry("p", 5, vec![1]);
    s.commit_changes(a, std::slice::from_ref(&e), &[ref_change(&ids, true)], &[])
        .unwrap();
    s.commit_changes(
        b,
        std::slice::from_ref(&e),
        &[ref_change(&ids[..1], true)],
        &[],
    )
    .unwrap();
    assert_eq!(s.blob_row(no, &ids[0]).unwrap().unwrap().refs, 2);
    assert_eq!(s.blob_row(no, &ids[1]).unwrap().unwrap().refs, 1);
    // what an older kit's delete does: every table it knows, so not this one
    let w = s.begin_write().unwrap();
    for (t, how) in DOC_TABLES {
        if *how == Erase::Range && t.name() != CLIENT_KINDS.name() {
            t.erase(&w, a).unwrap();
        }
    }
    w.open_table(PATHS)
        .unwrap()
        .remove(key("a.md").index().as_str())
        .unwrap();
    w.commit().unwrap();
    assert_eq!(
        s.blob_row(no, &ids[1]).unwrap().unwrap().refs,
        1,
        "it leaves the count"
    );
    drop(s);
    let s = DocStore::open(d.db()).unwrap();
    let r0 = s.blob_row(no, &ids[0]).unwrap().unwrap();
    let r1 = s.blob_row(no, &ids[1]).unwrap().unwrap();
    assert_eq!(r0.refs, 1, "the other doc's count stays");
    assert_eq!(r1.refs, 0, "the open gave the count back");
    assert_eq!(r1.since_ms, 1_000, "the blob keeps its time");
    assert_eq!(
        s.blob_refs(b).unwrap().refs.len(),
        1,
        "the other doc's row stays"
    );
    let r = s.begin_read().unwrap();
    assert!(!blob_refs_holds(&r, a), "the deleted doc's rows went");
}

fn blob_refs_holds(r: &redb::ReadTransaction, id: DocId) -> bool {
    DocRows::Named(blob_refs::DOC_BLOB_REFS)
        .holds(r, id)
        .unwrap()
}

/// A client ID's owner row is written by `commit_changes` in the change's own transaction: a
/// commit that doesn't happen leaves neither, one that does leaves both. A row already there
/// stays as it is (a participant purge may have renamed it).
#[test]
fn an_edit_and_its_row_land_together() {
    let t = TmpDir::new("row-with-change");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    let change = || NewChange {
        info: ChangeInfo::new(ChangeKind::Edit, "ann", "cli", 5),
        record: RevertRecord {
            author: "ann".into(),
            on_behalf_of: None,
            at_ms: 5,
            roots: Vec::new(),
            elems: Vec::new(),
        },
        span: None,
        provenance: Vec::new(),
        authored: None,
        row_marks: Vec::new(),
        col_remap: false,
        note_rows: Vec::new(),
        blob_refs: Default::default(),
    };
    let e = entry("ann", 7, vec![1]);
    INTERRUPT_COMMIT.with(|i| i.set(Some(CommitInterrupt::Before)));
    let stopped = s.commit_changes(id, std::slice::from_ref(&e), &[change()], &[]);
    INTERRUPT_COMMIT.with(|i| i.set(None));
    assert!(stopped.is_err(), "the test's stop before the commit");
    assert_eq!(s.client_owner(id, 7).unwrap(), None, "no change, no row");
    assert!(s.load(id).unwrap().updates.is_empty(), "no change landed");

    s.commit_changes(id, std::slice::from_ref(&e), &[change()], &[])
        .unwrap();
    assert_eq!(s.load(id).unwrap().updates.len(), 1, "the change landed");
    assert_eq!(
        s.client_owner(id, 7).unwrap().as_deref(),
        Some("ann"),
        "the change's row landed with it"
    );

    s.set_client(id, 8, "pseudonym").unwrap();
    s.commit_changes(id, &[entry("ann", 8, vec![2])], &[change()], &[])
        .unwrap();
    assert_eq!(
        s.client_owner(id, 8).unwrap().as_deref(),
        Some("pseudonym"),
        "a row already there stays as it is"
    );
}

// ---- the log entry's body (C3, design §1.7): EntryBody, versions 3 and 4 ----

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// A native entry (version 4): its body is a position, never bytes.
fn native_entry(author: &str, client: u64, at: u64) -> LogEntry {
    LogEntry {
        body: EntryBody::Native(NativeRef { at }),
        ..entry(author, client, Vec::new())
    }
}

/// Every log row of doc `id`, as stored.
fn raw_rows(s: &DocStore, id: DocId) -> Vec<(Seq, Vec<u8>)> {
    let r = s.begin_read().unwrap();
    let t = r.open_table(UPDATES).unwrap();
    let mut rows = Vec::new();
    for kv in t.range((id, 0)..=(id, u64::MAX)).unwrap() {
        let (k, v) = kv.unwrap();
        rows.push((k.value().1, v.value().to_vec()));
    }
    rows
}

/// Version 3's bytes: a Yrs entry and a redacted one (author `agent-a`, client 9, at 5 ms, via
/// `cli`, on behalf of `alice`, update `[1, 2, 3]`).
const V3_YRS: &str = "03070000006167656e742d610900000000000000050000000000000003000000636c69\
                      00030000000102030105000000616c696365";
const V3_REDACTED: &str = "03070000006167656e742d610900000000000000050000000000000003000000636c69\
                           0103000000000000000105000000616c696365";
/// Version 4's bytes: the same metadata, body tag 2, position 0x0102030405060708.
const V4_NATIVE: &str = "04070000006167656e742d610900000000000000050000000000000003000000636c69\
                         0208070605040302010105000000616c696365";

/// A Yrs entry is written as version 3, byte for byte as before native entries existed (tag 0,
/// and tag 1 when a purge redacted it), and so is every row a Yrs doc's log holds: a store with
/// no native doc is unchanged. `MPC_ENTRY_PLANT=yrs-as-version-4` (Yrs entries written as
/// version 4) fails at the first golden.
#[test]
fn yrs_entries_stay_version_3_byte_for_byte() {
    plant::under(plant::EntryPlant::YrsAsVersion4, || {
        let mut e = entry("agent-a", 9, vec![1, 2, 3]);
        e.on_behalf_of = Some("alice".into());
        assert_eq!(
            enc_entry(&e),
            hex_bytes(V3_YRS),
            "a Yrs entry is version 3, tag 0"
        );
        assert_eq!(
            enc_entry_as(&e, Some(3)),
            hex_bytes(V3_REDACTED),
            "a redacted Yrs entry is version 3, tag 1"
        );
        // a Yrs doc's log, through the store: every row is the version-3 encoding
        let t = TmpDir::new("entry-v3");
        let s = DocStore::open(t.db()).unwrap();
        let id = s.create(&meta("a.md")).unwrap();
        let mut doc = Doc::new(S).unwrap();
        let mut logged = Vec::new();
        for (i, text) in ["one\n", "one\ntwo\n", "two\n"].into_iter().enumerate() {
            let u = doc.replace_text_as(5, "source", text).unwrap().unwrap();
            let mut e = entry("a", 5, u);
            e.on_behalf_of = (i == 1).then(|| "alice".into());
            logged.push(e);
        }
        s.append(id, &logged).unwrap();
        let rows = raw_rows(&s, id);
        assert_eq!(rows.len(), 3);
        for ((seq, b), e) in rows.iter().zip(&logged) {
            assert_eq!(b[0], 3, "row {seq} is version 3");
            let mut want = W::default();
            want.u8(3)
                .str(&e.author)
                .u64(e.client)
                .u64(e.at_ms)
                .str(&e.via)
                .u8(0)
                .bytes(yrs_bytes(e))
                .opt_str(e.on_behalf_of.as_deref());
            assert_eq!(b, &want.0, "row {seq} is version 3's layout, byte for byte");
        }
    });
}

/// Every entry an older kit wrote (versions 1, 2 and 3, a redacted one included) reads as a
/// Yrs body.
#[test]
fn old_entries_read_as_yrs() {
    let mut e = entry("agent-a", 9, vec![1, 2, 3]);
    let v1 = dec_entry(&enc_entry_v1(&e)).unwrap();
    assert_eq!(v1.body, EntryBody::Yrs(vec![1, 2, 3]), "version 1");
    e.on_behalf_of = Some("alice".into());
    let v2 = dec_entry(&enc_entry_v2(&e)).unwrap();
    assert_eq!(v2.body, EntryBody::Yrs(vec![1, 2, 3]), "version 2");
    let (v3, red) = dec_entry_full(&hex_bytes(V3_YRS)).unwrap();
    assert_eq!(
        (v3.body, red),
        (EntryBody::Yrs(vec![1, 2, 3]), None),
        "version 3"
    );
    let (v3r, red) = dec_entry_full(&hex_bytes(V3_REDACTED)).unwrap();
    assert_eq!(
        (v3r.body, red),
        (EntryBody::Yrs(Vec::new()), Some(3)),
        "version 3, redacted: an empty Yrs body and its length"
    );
    assert_eq!(v3r.on_behalf_of.as_deref(), Some("alice"));
}

/// A native entry is version 4 with body tag 2, and decodes to the same position; the store
/// keeps it as written. Version 4 with a Yrs tag, and version 3 with tag 2, are damage, as are
/// a trailing byte and a cut record.
#[test]
fn native_entry_round_trips_as_version_4() {
    let mut e = native_entry("agent-a", 9, 0x0102_0304_0506_0708);
    e.on_behalf_of = Some("alice".into());
    let b = enc_entry(&e);
    assert_eq!(b, hex_bytes(V4_NATIVE), "version 4's layout");
    assert_eq!(dec_entry_full(&b).unwrap(), (e.clone(), None));
    // through the store: the row is the encoding, and decodes to the entry
    let t = TmpDir::new("entry-v4");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    s.append(id, std::slice::from_ref(&e)).unwrap();
    let rows = raw_rows(&s, id);
    assert_eq!(rows, vec![(1, b.clone())]);
    assert_eq!(dec_entry(&rows[0].1).unwrap(), e);
    // the pairings no build writes
    for tag in [0u8, 1] {
        let mut w = W::default();
        w.u8(4)
            .str("a")
            .u64(9)
            .u64(5)
            .str("cli")
            .u8(tag)
            .u64(1)
            .opt_str(None);
        assert_eq!(
            dec_entry(&w.0),
            Err(StoreError::Corrupt("log entry")),
            "v4, tag {tag}"
        );
    }
    let mut w = W::default();
    w.u8(3)
        .str("a")
        .u64(9)
        .u64(5)
        .str("cli")
        .u8(2)
        .u64(1)
        .opt_str(None);
    assert_eq!(
        dec_entry(&w.0),
        Err(StoreError::Corrupt("log entry")),
        "v3, tag 2"
    );
    let mut trailing = b.clone();
    trailing.push(0);
    assert!(dec_entry(&trailing).is_err(), "a trailing byte");
    assert!(dec_entry(&b[..b.len() - 1]).is_err(), "a cut record");
}

/// A log whose entries aren't all of its doc's engine is refused at load, typed, and nothing is
/// returned: a native entry in a Yrs doc's log is `Corrupt("log entry engine")`, and so (at the
/// check, since no doc is stamped native in this build) is a Yrs entry where native ones are
/// due. A purge's log pass refuses the same log before changing anything.
/// `MPC_ENTRY_PLANT=load-skips-engine-check` fails at the first load assert.
#[test]
fn a_mixed_log_is_refused() {
    plant::under(plant::EntryPlant::LoadSkipsEngineCheck, || {
        let t = TmpDir::new("mixed-log");
        let s = DocStore::open(t.db()).unwrap();
        let id = s.create(&meta("a.md")).unwrap();
        let mut doc = Doc::new(S).unwrap();
        let u = doc.replace_text_as(5, "source", "one\n").unwrap().unwrap();
        s.append(id, &[entry("a", 5, u)]).unwrap();
        assert_eq!(s.load(id).unwrap().updates.len(), 1, "a Yrs log loads");
        s.append(id, &[native_entry("a", 5, 1)]).unwrap();
        assert_eq!(
            s.load(id),
            Err(StoreError::Corrupt(LOG_ENTRY_ENGINE)),
            "a native entry in a Yrs doc's log"
        );
        assert_eq!(s.log_size(id), Err(StoreError::Corrupt(LOG_ENTRY_ENGINE)));
    });
    // the reverse, at the check the load runs per entry
    assert_eq!(
        EntryEngine::Native.check(&EntryBody::Yrs(vec![1])),
        Err(StoreError::Corrupt(LOG_ENTRY_ENGINE))
    );
    assert_eq!(
        EntryEngine::Native.check(&EntryBody::Native(NativeRef { at: 1 })),
        Ok(())
    );
    assert_eq!(
        EntryEngine::Yrs.check(&EntryBody::Native(NativeRef { at: 1 })),
        Err(StoreError::Corrupt(LOG_ENTRY_ENGINE))
    );
    assert_eq!(EntryEngine::Yrs.check(&EntryBody::Yrs(vec![1])), Ok(()));
}

/// A doc another engine stamped is refused whole at load (`MPC-E520`), before any of its
/// entries is decoded: a row this build can't read doesn't turn the refusal into damage.
#[test]
fn a_doc_of_another_engine_is_refused_whole_at_load() {
    let t = TmpDir::new("foreign-load");
    let s = DocStore::open(t.db()).unwrap();
    let id = s.create(&meta("a.md")).unwrap();
    s.append(id, &[native_entry("a", 5, 1)]).unwrap();
    {
        let w = s.begin_write().unwrap();
        w.open_table(UPDATES)
            .unwrap()
            .insert((id, 2), [9u8, 9, 9].as_slice())
            .unwrap();
        w.commit().unwrap();
    }
    let foreign = DocMeta {
        engine: "mpce-0.1".into(),
        engine_format: 1,
        ..s.meta(id).unwrap()
    };
    s.put_meta(id, &foreign).unwrap();
    let e = s.load(id).unwrap_err();
    assert_eq!(
        e,
        StoreError::ForeignEngine {
            stored: "mpce-0.1".into(),
            stored_format: 1
        }
    );
    assert_eq!(crate::errors::Refusal::report(&e).id(), "MPC-E520");
}

/// A native entry never reaches the Yrs replay: [`Loaded::yrs_updates`] is all or nothing, so
/// [`Doc::from_store`] refuses a log holding one before it replays a single update, and its Yrs
/// twin replays every update. `MPC_ENTRY_PLANT=native-via-yrs-replay` (a native entry skipped
/// instead of refused) fails at the replay counter.
#[test]
fn a_native_doc_never_reaches_the_yrs_replay() {
    let mut doc = Doc::new(S).unwrap();
    let u1 = doc.replace_text_as(5, "source", "one\n").unwrap().unwrap();
    let u2 = doc
        .replace_text_as(5, "source", "one\ntwo\n")
        .unwrap()
        .unwrap();
    let yrs = Loaded {
        snapshot: None,
        updates: vec![(1, entry("a", 5, u1)), (2, entry("a", 5, u2))],
    };
    let mut mixed = yrs.clone();
    mixed.updates.push((3, native_entry("a", 5, 1)));
    plant::under(plant::EntryPlant::NativeViaYrsReplay, || {
        crate::doc::YRS_REPLAYS.with(|n| n.set(0));
        let r = Doc::from_store(S, &mixed);
        assert_eq!(
            crate::doc::YRS_REPLAYS.with(|n| n.get()),
            0,
            "nothing is replayed from a log holding a native entry"
        );
        assert!(
            matches!(r, Err(crate::doc::DocError::Malformed(_))),
            "{:?}",
            r.as_ref().err()
        );
        assert_eq!(
            mixed.yrs_updates(),
            Err(StoreError::Corrupt(LOG_ENTRY_ENGINE))
        );
    });
    // the twin: the same log without the native entry replays every update
    crate::doc::YRS_REPLAYS.with(|n| n.set(0));
    let d = Doc::from_store(S, &yrs).unwrap();
    assert_eq!(crate::doc::YRS_REPLAYS.with(|n| n.get()), 2);
    assert_eq!(d.text("source").unwrap(), "one\ntwo\n");
}
