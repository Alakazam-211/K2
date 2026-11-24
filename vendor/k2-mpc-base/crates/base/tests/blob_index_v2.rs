//! The structure index's row format 2: the view items (a node's box on the page, field kinds,
//! view text, editable text, fonts and render hash; the doc's fonts) carried from the pack's
//! reader to the blob tier's view index, a row of format 1 still read with them absent, the
//! rule that keeps them from an older kit (a pack hands them only from a format it bumped to,
//! naming the older one), and that rule played through in the tree. The file-type side is a
//! stand-in ([`index_pack`] plus made-up items); the kit knows no format.
//!
//! The N−1 check against a previous release is `tools/n1/n1.sh layout-index` / `node-index`,
//! with this file's ignored `n1_index_producer` and `n1_index_after`. Row format 3 (a
//! container's own record digest, `DraftNode::own`) has its own: `layout-own` / `node-own`,
//! with `n1_own_producer` and `n1_own_after`.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::{file, IxPack};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::under_env_index_mutant;
use k2_mpc_base::index::{
    Digest, IndexDraft, IndexJobError, IndexLimits, IndexRefused, IndexSpec, IndexStatus,
    IndexVerbs, NodeView, ViewGeom,
};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{Bbox, BlobView, EditableText, Flip, FontUse, ViewIndex, ViewSchema};

const VERBS: IndexVerbs =
    IndexVerbs::new("part list", "part show", "part diff", "part blame", None);

/// The stand-in pack: [`IxPack`]'s reader, plus made-up view items when `items` is set, under
/// `format` and (`older`) the older format it names; a page is a container.
#[derive(Clone)]
struct V2Pack {
    pack: &'static str,
    format: u32,
    older: Option<u32>,
    items: bool,
    /// A parent's record holds its children's, with its own record's digest (row format 3:
    /// [`IxPack::containers`]).
    own: bool,
    /// The spec declares own records ([`IndexSpec::with_own_records`]).
    declared: bool,
}

/// The pack after the bump: format 2, items, reading format 1.
const BUMPED: V2Pack = V2Pack {
    pack: "idx",
    format: 2,
    older: Some(1),
    items: true,
    own: false,
    declared: false,
};

/// The pack before it (and an older kit's): format 1, no items.
const BEFORE: V2Pack = V2Pack {
    pack: "idx",
    format: 1,
    older: None,
    items: false,
    own: false,
    declared: false,
};

fn fonts() -> Vec<FontUse> {
    vec![
        FontUse {
            family: "Aptos".into(),
            script: "latin".into(),
            from: "theme-minor".into(),
            embedded: false,
        },
        FontUse {
            family: "Consolas".into(),
            script: "latin".into(),
            from: "explicit".into(),
            embedded: true,
        },
    ]
}

/// What the stand-in reader adds for the view, node by node (in draft order): a page's render
/// digest, its fonts and field kinds; every other node a box, its editable text and field
/// kinds, and the first one a view text.
fn add_items(d: &mut IndexDraft) {
    d.fonts = fonts();
    for (i, n) in d.nodes.iter_mut().enumerate() {
        let mut v = NodeView::new();
        if n.kind == "page" {
            v.fonts = vec![0, 1];
            v.fields = vec!["slidenum".into()];
            n.render = Some(Digest::of(format!("draws {}", n.name).as_bytes()));
        } else {
            v.geom = Some(ViewGeom {
                bbox: Bbox {
                    x: 100 * i as i64,
                    y: 200,
                    w: 300,
                    h: 400,
                },
                rot: 60_000,
                flip: Flip { h: true, v: false },
                inherited: i == 1,
                approx: i == 3,
            });
            v.editable_text = Some(if i == 1 {
                EditableText::Formatted
            } else {
                EditableText::Never
            });
            if i == 1 {
                v.fields = vec!["datetime".into()];
                v.text = Some("view\ttext".into());
            }
        }
        n.view = Some(v);
    }
}

impl BlobPack for V2Pack {
    fn index_spec(&self) -> Option<IndexSpec> {
        let s = IndexSpec::new(self.pack, self.format, &VERBS);
        let s = match self.older {
            Some(o) => s.with_older_format(o),
            None => s,
        };
        Some(if self.declared {
            s.with_own_records()
        } else {
            s
        })
    }

    fn index(
        &self,
        src: &mut dyn Read,
        size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let base = IxPack {
            pack: self.pack,
            format: self.format,
            containers: self.own,
            ..IxPack::default()
        };
        let mut d = base.index(src, size, limits)?;
        if self.items {
            add_items(&mut d);
        }
        Ok(d)
    }

    fn view_schema(&self) -> Option<ViewSchema> {
        Some(ViewSchema {
            blob: Some(BlobView {
                container_kind: "page",
                ..BlobView::NONE
            }),
            ..ViewSchema::NONE
        })
    }
}

/// A page, a box with text, a group, and the group's child.
const TREE: &[&str] = &[
    "@pg:1|page|One|-|1|-|-",
    "@pg:1/it:1|box|Title|0|1|Hello|-",
    "@pg:1/it:2|group|G|0|1|-|-",
    "@pg:1/it:3|box|Kid|2|1|-|-",
];

/// [`TREE`] as a file of its own: a doc attribute `doc=<tag>` makes its bytes differ from any
/// other doc's (rows are content-keyed, so docs holding the same bytes share them).
fn doc_file(tag: &str) -> Vec<u8> {
    let attr = format!("#attr doc={tag}");
    let mut lines = vec![attr.as_str()];
    lines.extend_from_slice(TREE);
    file(&lines)
}

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: Option<PathBuf>,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        if let Some(d) = &self.dir {
            let _ = fs::remove_dir_all(d);
        }
    }
}

/// A store and blob store under `dir` (`keep`: left there when the fixture drops).
fn fx_at(dir: &Path, keep: bool) -> Fx {
    fs::create_dir_all(dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store, host.clone(), Config::default()).with_blob_store(blobs);
    Fx {
        dir: (!keep).then(|| dir.to_owned()),
        host,
        mgr,
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-ixv2-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fx_at(&dir, false)
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    /// `path` open under `pack`: imported from `bytes`, or (`None`) the stored doc.
    fn open(&self, pack: &V2Pack, path: &str, bytes: Option<&[u8]>) -> Arc<BlobSession> {
        let mut src = bytes;
        self.mgr
            .open_blob(
                Arc::new(pack.clone()),
                &key(path),
                src.as_mut().map(|b| b as &mut dyn Read),
            )
            .unwrap()
    }

    /// `path` closed and opened again under `pack` (a build with another pack).
    fn reopen(&self, pack: &V2Pack, path: &str) -> Arc<BlobSession> {
        self.mgr.close(&key(path), "another pack").unwrap();
        self.open(pack, path, None)
    }

    fn reader(&self, s: &BlobSession) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert("ada".into(), Rights::full());
        s.join(
            Participant::new("ada", "ada", "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }

    fn view(&self, s: &BlobSession) -> ViewIndex {
        let c = self.reader(s);
        s.view_index(c, None)
            .unwrap()
            .expect("the pack has a view index")
    }
}

/// Run the job for the current version to completion.
fn index(s: &BlobSession) -> IndexStatus {
    s.index_job(None)
        .unwrap()
        .expect("a job is due")
        .run()
        .expect("the job records")
}

fn unit<'a>(ix: &'a ViewIndex, k: &str) -> &'a k2_mpc_base::view_index::Unit {
    ix.units
        .iter()
        .find(|u| u.key == k)
        .unwrap_or_else(|| panic!("no unit {k}"))
}

/// The view index of a row of format 2 carries every item the reader gave, keyed where it's a
/// hash; a plant that drops one is caught here (`index-v2-field-dropped`).
#[test]
fn a_format_2_index_carries_its_view_items_to_the_blob_view() {
    under_env_index_mutant(|| {
        let f = fx("items");
        let s = f.open(&BUMPED, "a.idx", Some(&file(TREE)));
        assert_eq!(
            index(&s),
            IndexStatus::Ready {
                version: 1,
                nodes: 4
            }
        );
        let ix = f.view(&s);
        assert_eq!(ix.fonts, fonts(), "the doc's fonts");
        let c = &ix.containers[0];
        assert_eq!(
            (c.fonts.clone(), c.has_fields.clone()),
            (vec![0, 1], vec!["slidenum".to_owned()])
        );
        let h = c.render_hash.expect("the page's render hash");
        assert_ne!(
            h.as_bytes(),
            Digest::of(b"draws One").as_bytes(),
            "keyed, never the raw digest"
        );
        let t = unit(&ix, "@pg:1/it:1");
        assert_eq!(
            t.bbox,
            Some(Bbox {
                x: 100,
                y: 200,
                w: 300,
                h: 400
            }),
            "the box from the view items"
        );
        assert_eq!(
            (t.rot, t.flip, t.bbox_inherited, t.approx_box),
            (60_000, Flip { h: true, v: false }, true, false)
        );
        assert_eq!(t.editable_text, Some(EditableText::Formatted));
        assert_eq!(t.fields, vec!["datetime".to_owned()]);
        assert_eq!(
            t.text.as_ref().map(|x| x.summary.as_str()),
            Some("view\ttext"),
            "the view text over the node's"
        );
        let k = unit(&ix, "@pg:1/it:3");
        assert_eq!(
            (k.bbox.map(|b| b.x), k.approx_box, k.bbox_inherited),
            (Some(300), true, false)
        );
        assert_eq!(k.editable_text, Some(EditableText::Never));
    });
}

/// A row of format 1 (the pack before the bump wrote it) still reads under the bumped pack:
/// `Ready` at once, every view item absent and every other field as before; a job to index it
/// again is offered (optional), and after it the items are there and no job is.
#[test]
fn a_format_1_index_still_reads_with_the_items_absent() {
    under_env_index_mutant(|| {
        let f = fx("v1");
        let s = f.open(&BEFORE, "a.idx", Some(&file(TREE)));
        index(&s);
        let before = f.view(&s);
        let s = f.reopen(&BUMPED, "a.idx");
        assert_eq!(
            s.index_status(None).unwrap(),
            IndexStatus::Ready {
                version: 1,
                nodes: 4
            },
            "the older format's row reads"
        );
        let ix = f.view(&s);
        assert_eq!(
            ix, before,
            "the same view as before the bump: items absent, nothing else changed"
        );
        assert!(ix.fonts.is_empty() && ix.containers[0].render_hash.is_none());
        assert_eq!(unit(&ix, "@pg:1/it:1").bbox, None);
        assert_eq!(
            unit(&ix, "@pg:1/it:1")
                .text
                .as_ref()
                .map(|x| x.summary.as_str()),
            Some("Hello"),
            "the node's own text"
        );
        let job = s
            .index_job(None)
            .unwrap()
            .expect("indexing it again is offered");
        assert_eq!(
            job.run().unwrap(),
            IndexStatus::Ready {
                version: 1,
                nodes: 4
            }
        );
        let ix = f.view(&s);
        assert_eq!(ix.fonts, fonts());
        assert!(unit(&ix, "@pg:1/it:1").bbox.is_some());
        assert!(
            s.index_job(None).unwrap().is_none(),
            "this format's row is there"
        );
    });
}

/// View items from a pack whose spec names no older format are refused (`MPC-E714`), nothing
/// recorded: an older kit asking under the same format would meet a row of format 2.
#[test]
fn view_items_from_a_pack_that_names_no_older_format_are_refused() {
    under_env_index_mutant(|| {
        let f = fx("undeclared");
        let pack = V2Pack {
            older: None,
            ..BUMPED
        };
        let s = f.open(&pack, "a.idx", Some(&file(TREE)));
        let out = s.index_job(None).unwrap().expect("a job").run();
        assert!(
            matches!(out, Err(IndexJobError::BadResult(ref m)) if m.contains("older format")),
            "{out:?}"
        );
        assert!(matches!(
            s.index_status(None).unwrap(),
            IndexStatus::Pending { .. }
        ));
    });
}

/// The rule played through in the tree: a pack before the bump (as an older kit runs it) never
/// reads the bumped row; it finds the version pending, indexes it under its own format, and
/// its commit removes the newer row; the bumped pack then reads that older row, items absent,
/// until it indexes the version again.
#[test]
fn a_pack_before_the_bump_never_reads_a_format_2_row() {
    under_env_index_mutant(|| {
        let f = fx("older");
        let s = f.open(&BUMPED, "a.idx", Some(&file(TREE)));
        index(&s);
        assert_eq!(f.view(&s).fonts, fonts(), "format 2 written");
        let s = f.reopen(&BEFORE, "a.idx");
        assert_eq!(
            s.index_status(None).unwrap(),
            IndexStatus::Pending {
                version: 1,
                running: false
            },
            "the older format has no row: the bumped one is never read"
        );
        assert_eq!(
            index(&s),
            IndexStatus::Ready {
                version: 1,
                nodes: 4
            }
        );
        let s = f.reopen(&BUMPED, "a.idx");
        let ix = f.view(&s);
        assert!(
            ix.fonts.is_empty() && unit(&ix, "@pg:1/it:3").bbox.is_none(),
            "the older pack's commit took the format-2 row: read from its row, items absent"
        );
        assert!(
            s.index_job(None).unwrap().is_some(),
            "indexing it again is offered"
        );
    });
}

// ----- N−1 against a previous release (tools/n1/n1.sh `node-index`) -----

/// The N−1 dir (`K2MB_N1_OUT`).
fn n1_dir() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT: the N-1 dir"))
}

/// The store this build writes for the previous release (`<dir>/index`): `v2.idx` indexed by
/// the bumped pack (a row of format 2 only), `old.idx` by the pack before it (format 1, read
/// here with its items absent), `plain.idx` by a pack with no items (format 1 bytes, as the
/// previous release writes them).
#[test]
#[ignore = "N-1: run by tools/n1/n1.sh node-index"]
fn n1_index_producer() {
    let dir = n1_dir().join("index");
    let _ = fs::remove_dir_all(&dir);
    let f = fx_at(&dir, true);
    let v2 = f.open(&BUMPED, "v2.idx", Some(&doc_file("v2")));
    index(&v2);
    assert_eq!(f.view(&v2).fonts, fonts());
    let old = f.open(&BEFORE, "old.idx", Some(&doc_file("old")));
    index(&old);
    let old = f.reopen(&BUMPED, "old.idx");
    assert!(matches!(
        old.index_status(None).unwrap(),
        IndexStatus::Ready { nodes: 4, .. }
    ));
    let plain = V2Pack {
        pack: "plain",
        ..BEFORE
    };
    let p = f.open(&plain, "plain.idx", Some(&doc_file("plain")));
    index(&p);
    // what the previous release opens: v2.idx's only row is still the format-2 one (rows are
    // content-keyed, so each doc has its own bytes: a format-1 commit of the same bytes would
    // take it)
    let v2 = f.reopen(&BUMPED, "v2.idx");
    assert_eq!(
        f.view(&v2).fonts,
        fonts(),
        "v2.idx: its format-2 row is there"
    );
    assert!(
        v2.index_job(None).unwrap().is_none(),
        "v2.idx: read from its format-2 row, not an older one"
    );
    for k in ["v2.idx", "old.idx", "plain.idx"] {
        f.mgr.close(&key(k), "n1").unwrap();
    }
    println!("N-1 INDEX producer: v2.idx (format 2), old.idx and plain.idx (format 1) written");
}

/// After the previous release ran on this build's store (`<dir>/index`) and wrote its own
/// (`<dir>/prev`): every doc reads here, a format-1 row with its items absent and a job offered
/// that brings them; `plain.idx` as it was.
#[test]
#[ignore = "N-1: run by tools/n1/n1.sh node-index"]
fn n1_index_after() {
    for (store, docs) in [
        ("index", &["v2.idx", "old.idx"][..]),
        ("prev", &["a.idx"][..]),
    ] {
        let f = fx_at(&n1_dir().join(store), true);
        for d in docs {
            let s = f.open(&BUMPED, d, None);
            assert_eq!(
                s.index_status(None).unwrap(),
                IndexStatus::Ready {
                    version: 1,
                    nodes: 4
                },
                "{store}/{d}: a format-1 row reads"
            );
            let ix = f.view(&s);
            assert!(ix.fonts.is_empty(), "{store}/{d}: items absent");
            assert_eq!(
                unit(&ix, "@pg:1/it:1")
                    .text
                    .as_ref()
                    .map(|x| x.summary.as_str()),
                Some("Hello"),
                "{store}/{d}"
            );
            index(&s);
            let ix = f.view(&s);
            assert_eq!(ix.fonts, fonts(), "{store}/{d}: indexed again, items there");
            assert!(unit(&ix, "@pg:1/it:3").approx_box, "{store}/{d}");
        }
        let plain = V2Pack {
            pack: "plain",
            ..BEFORE
        };
        let p = if store == "index" {
            "plain.idx"
        } else {
            "p.idx"
        };
        let s = f.open(&plain, p, None);
        assert_eq!(
            s.index_status(None).unwrap(),
            IndexStatus::Ready {
                version: 1,
                nodes: 4
            },
            "{store}/{p}"
        );
        assert!(f.view(&s).fonts.is_empty());
    }
    println!("N-1 INDEX OK");
}

// ----- row format 3: a container's own record digest -----

/// A pack with own records, its format bumped as well: format 3, items and own digests
/// (declared), reading format 2.
const OWN3: V2Pack = V2Pack {
    pack: "deck",
    format: 3,
    older: Some(2),
    items: true,
    own: true,
    declared: true,
};

/// The pack before it (and a previous release's): format 2, items, no own digest.
const OWN2: V2Pack = V2Pack {
    pack: "deck",
    format: 2,
    older: Some(1),
    items: true,
    own: false,
    declared: false,
};

/// A pack that gives own digests (declared) without bumping its format for them: the kit's
/// offset format is what keeps its rows from an older kit asking for format 2.
const UNBUMPED: V2Pack = V2Pack {
    pack: "bad",
    format: 2,
    older: Some(1),
    items: true,
    own: true,
    declared: true,
};

/// [`UNBUMPED`]'s pack as an older kit's: the same format, no own digest.
const UNBUMPED_BEFORE: V2Pack = V2Pack {
    own: false,
    declared: false,
    ..UNBUMPED
};

/// Each node's own record hash (by key) as `s`'s current index reads it: `true` when there.
fn owns(s: &BlobSession) -> Vec<(String, bool)> {
    s.index_view(None)
        .unwrap()
        .nodes
        .iter()
        .map(|n| (n.key.as_str().to_owned(), n.own_hash.is_some()))
        .collect()
}

/// Whether the page (`@pg:1`) has a render hash in `s`'s current index (a view item: the
/// previous release's pack gives that one only).
fn renders(s: &BlobSession) -> bool {
    s.index_view(None)
        .unwrap()
        .nodes
        .iter()
        .any(|n| n.key.as_str() == "@pg:1" && n.render_hash.is_some())
}

/// [`TREE`]'s nodes with whether each has an own record hash: the page and the group (the
/// parents) when `own`.
fn owned(own: bool) -> Vec<(String, bool)> {
    ["@pg:1", "@pg:1/it:1", "@pg:1/it:2", "@pg:1/it:3"]
        .iter()
        .map(|k| (k.to_string(), own && (*k == "@pg:1" || *k == "@pg:1/it:2")))
        .collect()
}

/// Own digests are stored and read back (row format 3); the same version read by the pack
/// before them (format 2) is pending and indexed there, its commit taking the own-records row;
/// read again by the own-records pack, that row reads with the own hashes absent and a job is
/// offered that brings them back. Own digests from a spec that doesn't declare them are
/// refused (`MPC-E714`), nothing recorded.
#[test]
fn own_digests_are_stored_and_an_older_row_reads_without_them() {
    let f = fx("own");
    let s = f.open(&OWN3, "a.idx", Some(&doc_file("a")));
    index(&s);
    let stored = owns(&s);
    let s = f.reopen(&OWN2, "a.idx");
    let before_bump = (s.index_status(None).unwrap(), index(&s), owns(&s));
    let s = f.reopen(&OWN3, "a.idx");
    let older = (owns(&s), s.index_job(None).unwrap().is_some());
    index(&s);
    let again = owns(&s);
    let undeclared = V2Pack {
        declared: false,
        items: false,
        ..OWN3
    };
    let u = f.open(&undeclared, "u.idx", Some(&doc_file("u")));
    let out = u.index_job(None).unwrap().expect("a job").run();
    let refused = matches!(out, Err(IndexJobError::BadResult(ref m))
        if m.contains("own record digests") && m.contains("IndexSpec::with_own_records"));
    let none_recorded = u.index_status(None).unwrap();
    assert_eq!(
        (stored, before_bump, older, again, refused, none_recorded),
        (
            owned(true),
            (
                IndexStatus::Pending {
                    version: 1,
                    running: false
                },
                IndexStatus::Ready {
                    version: 1,
                    nodes: 4
                },
                owned(false)
            ),
            (owned(false), true),
            owned(true),
            true,
            IndexStatus::Pending {
                version: 1,
                running: false
            }
        ),
        "own digests: stored (format 3); the format before them indexes the version again; read \
         back with them absent and a job offered; undeclared, refused and nothing recorded \
         ({out:?})"
    );
}

/// A pack that declares own records without bumping its format: the kit stores its rows under
/// the offset format, so the same pack without own digests (an older kit's, asking for the
/// same format) finds the version pending, never a row of format 3; it indexes it, and the
/// own-records pack then reads that row (its own format, a fallback) with the own hashes
/// absent and a job offered that brings them back. Plant: `own-offset-skipped` (the pack's
/// format used: the older pack reads the format-3 row).
#[test]
fn own_records_live_out_of_an_older_kits_reach() {
    under_env_index_mutant(|| {
        let f = fx("own-offset");
        let s = f.open(&UNBUMPED, "b.idx", Some(&doc_file("b")));
        index(&s);
        let stored = owns(&s);
        let s = f.reopen(&UNBUMPED_BEFORE, "b.idx");
        let older_kit = s.index_status(None).unwrap();
        if matches!(older_kit, IndexStatus::Pending { .. }) {
            index(&s);
        }
        let s = f.reopen(&UNBUMPED, "b.idx");
        let back = (
            s.index_status(None).unwrap(),
            owns(&s),
            s.index_job(None).unwrap().is_some(),
        );
        assert_eq!(
            (stored, older_kit, back),
            (
                owned(true),
                IndexStatus::Pending {
                    version: 1,
                    running: false
                },
                (
                    IndexStatus::Ready {
                        version: 1,
                        nodes: 4
                    },
                    owned(false),
                    true
                )
            ),
            "own records out of an older kit's reach: stored under the offset format, the same \
             format without own digests finds the version pending, then this pack reads that \
             row with them absent"
        );
    });
}

/// The store this build writes for the previous release (`<dir>/own`): `a.idx` indexed by a
/// pack with own records and a bumped format (3), `bad.idx` by one that declared own records
/// without bumping (format 2, the one the previous release reads): both stored under the kit's
/// offset format. Under `MPC_INDEX_MUTANT=own-offset-skipped` (the plant run of `node-own`),
/// they are stored under the packs' own formats instead.
#[test]
#[ignore = "N-1: run by tools/n1/n1.sh node-own"]
fn n1_own_producer() {
    under_env_index_mutant(|| {
        let dir = n1_dir().join("own");
        let _ = fs::remove_dir_all(&dir);
        let f = fx_at(&dir, true);
        let a = f.open(&OWN3, "a.idx", Some(&doc_file("a")));
        index(&a);
        assert_eq!(owns(&a), owned(true), "a.idx: own hashes stored");
        let bad = f.open(&UNBUMPED, "bad.idx", Some(&doc_file("bad")));
        index(&bad);
        assert_eq!(owns(&bad), owned(true), "bad.idx: own hashes stored");
        for k in ["a.idx", "bad.idx"] {
            f.mgr.close(&key(k), "n1").unwrap();
        }
        println!(
            "N-1 OWN producer: a.idx (pack format 3) and bad.idx (pack format 2), own records \
             (planted: {:?})",
            k2_mpc_base::differential::index_mutant()
        );
    });
}

/// After the previous release ran on this build's store (`<dir>/own`) and wrote its own
/// (`<dir>/prev-own`): `a.idx` and `bad.idx` read from the rows the previous release recorded
/// (format 2 each: its commit took the offset rows), own hashes absent, and a job brings them
/// back; `p.idx`, the previous release's row of format 2, reads as it wrote it, its view items
/// there and no own hash, until indexed again.
#[test]
#[ignore = "N-1: run by tools/n1/n1.sh node-own"]
fn n1_own_after() {
    let f = fx_at(&n1_dir().join("own"), true);
    for (pack, doc) in [(&OWN3, "a.idx"), (&UNBUMPED, "bad.idx")] {
        let s = f.open(pack, doc, None);
        assert_eq!(
            (s.index_status(None).unwrap(), owns(&s), renders(&s)),
            (
                IndexStatus::Ready {
                    version: 1,
                    nodes: 4
                },
                owned(false),
                true
            ),
            "own/{doc}: the previous release's format-2 row reads, own hashes absent, its view \
             item there"
        );
        index(&s);
        assert_eq!(
            owns(&s),
            owned(true),
            "own/{doc}: indexed again, own hashes back"
        );
    }
    let p = fx_at(&n1_dir().join("prev-own"), true);
    let s = p.open(&OWN3, "p.idx", None);
    let ix = p.view(&s);
    assert_eq!(
        (
            s.index_status(None).unwrap(),
            owns(&s),
            renders(&s),
            unit(&ix, "@pg:1/it:1")
                .text
                .as_ref()
                .map(|x| x.summary.clone())
        ),
        (
            IndexStatus::Ready {
                version: 1,
                nodes: 4
            },
            owned(false),
            true,
            Some("Hello".to_owned())
        ),
        "prev-own/p.idx: the previous release's row of format 2 reads as written"
    );
    assert!(
        s.index_job(None).unwrap().is_some(),
        "prev-own/p.idx: a job is offered"
    );
    index(&s);
    assert_eq!(
        owns(&s),
        owned(true),
        "prev-own/p.idx: indexed again, own hashes there"
    );
    println!("N-1 OWN OK");
}
