use super::*;
use crate::{Doc, Kind, Schema};

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
    LogEntry {
        author: author.into(),
        client,
        at_ms: 5,
        via: "cli".into(),
        update,
        on_behalf_of: None,
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
            .map(|(q, e)| (*q, e.update.clone()))
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
            .insert("store_format", STORE_FORMAT + 1)
            .unwrap();
        w.commit().unwrap();
    }
    assert_eq!(
        DocStore::open(t.db()).err(),
        Some(StoreError::Newer {
            found: STORE_FORMAT + 1
        })
    );
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
                content: rebuilt.export_neutral(),
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
        .bytes(&e.update);
    w.0
}

fn enc_entry_v2(e: &LogEntry) -> Vec<u8> {
    let mut w = W::default();
    w.u8(2)
        .str(&e.author)
        .u64(e.client)
        .u64(e.at_ms)
        .str(&e.via)
        .bytes(&e.update)
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
    e.on_behalf_of = Some("rosson".into());
    assert_eq!(dec_entry(&enc_entry_v2(&e)).unwrap(), e);
    e.on_behalf_of = None;
    // v3 round trips, with and without on_behalf_of
    let v3 = enc_entry(&e);
    assert_eq!(v3[0], 3, "new records are version 3");
    assert_eq!(dec_entry(&v3).unwrap(), e);
    e.on_behalf_of = Some("rosson".into());
    assert_eq!(dec_entry(&enc_entry(&e)).unwrap(), e);
    // a purged entry keeps its metadata and length, not its update
    let red = enc_entry_as(&e, Some(3));
    let (d, n) = dec_entry_full(&red).unwrap();
    assert_eq!(n, Some(3));
    assert!(d.update.is_empty());
    assert_eq!(
        (d.author.as_str(), d.client, d.at_ms),
        ("agent-a", 9, e.at_ms)
    );
    assert_eq!(d.on_behalf_of.as_deref(), Some("rosson"));
    // damaged records are loud
    let mut bad = enc_entry(&e);
    bad[0] = 4;
    assert_eq!(
        dec_entry(&bad),
        Err(StoreError::Corrupt("log entry version"))
    );
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
fn older_formats_are_stamped_3_and_newer_refused() {
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
        assert_eq!(f, STORE_FORMAT);
        assert_eq!(STORE_FORMAT, 3);
        drop(r);
        let w = s.begin_write().unwrap();
        w.open_table(META)
            .unwrap()
            .insert("store_format", 4u64)
            .unwrap();
        w.commit().unwrap();
    }
    assert!(matches!(
        DocStore::open(d.db()),
        Err(StoreError::Newer { found: 4 })
    ));
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
    obo.on_behalf_of = Some("rosson".into());
    let seqs = s.append(id, &[entry("rosson", 5, u1), obo]).unwrap();
    assert_eq!(seqs, vec![1, 2]);
    assert_eq!(s.approval(id, 1).unwrap(), None, "own edits have none");
    let p = s.approval(id, 2).unwrap().unwrap();
    assert_eq!(p, Approval::pending("helper", "rosson"));
    assert_eq!(
        (p.state, p.by.as_deref(), p.at_ms),
        (ApprovalState::Pending, None, None)
    );
    // compaction drops the log entry but not the approval
    s.compact(id, 2, &doc.encode_state().unwrap()).unwrap();
    assert!(s.load(id).unwrap().updates.is_empty());
    let done = p.settled(ApprovalState::Declined, "rosson", 77);
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
