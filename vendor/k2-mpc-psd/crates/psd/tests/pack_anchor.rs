//! The pack's half of an anchor's state, as spec rows: each PSD-specific judgement (kind
//! families, the identity rule within one, `placed_id` as a weak ID only, candidates by family)
//! on layers indexed by the kit from a hand-built file, then every node of the public corpus
//! (fetched by reference, `PSD_PUBLIC_CORPUS`; prints counts only), then through a session's
//! comment threads, where the kit calls the pack's hooks itself.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::anchor::{
    resolve, Anchor, AnchorDoubt, AnchorRules, AnchorState, CandidateWhy, KitRules,
};
use k2_mpc_base::annot::{NewAnchor, NewComment};
use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{IndexNode, IndexStatus, IndexView, NodeKey, NodeRect};
use k2_mpc_base::session::{
    AuditEvent, BlobSession, Config, ConnId, Host, Participant, ParticipantKind, Rights,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_psd::anchor::same_family;
use k2_mpc_psd::PsdPack;
use photocraft_psd::descriptor::{Descriptor, UnicodeString, Value, VersionedDescriptor};
use support::*;

// ----- a kit session, to get real index nodes (keys, keyed hashes, fingerprints) -----

struct TestHost {
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
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2psd-anchor-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 1_000_000),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx { dir, host, mgr }
}

impl Fx {
    fn open(&self, path: &str, bytes: &[u8]) -> Arc<BlobSession> {
        let key = DocKey {
            space: "ws".into(),
            path: path.into(),
        };
        self.mgr
            .open_blob(Arc::new(PsdPack), &key, Some(&mut &bytes[..]))
            .unwrap()
    }
    fn join(&self, s: &BlobSession, pid: &str) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::editor());
        s.join(
            Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
}

fn indexed(s: &BlobSession) -> IndexView {
    let st = s
        .index_job(None)
        .unwrap()
        .expect("a job is due")
        .run()
        .expect("the job records");
    assert!(matches!(st, IndexStatus::Ready { .. }), "{st:?}");
    s.index_view(None).unwrap()
}

/// The kit's index of `bytes`, as a session records it.
fn view_of(bytes: &[u8]) -> IndexView {
    let f = fx("view");
    let s = f.open("art.psd", bytes);
    indexed(&s)
}

// ----- the hand-built file -----

fn tysh(text: &str) -> Vec<u8> {
    let mut d = 1u16.to_be_bytes().to_vec();
    d.extend([0u8; 48]);
    d.extend(50u16.to_be_bytes());
    d.extend(
        VersionedDescriptor::new(
            Descriptor::new("TxLr").with("Txt ", Value::Text(UnicodeString::new(text))),
        )
        .to_bytes(),
    );
    d
}

fn sold(id: &str) -> Vec<u8> {
    let mut d = b"soLD".to_vec();
    d.extend(4u32.to_be_bytes());
    d.extend(
        VersionedDescriptor::new(
            Descriptor::new("null")
                .with("Idnt", Value::Text(UnicodeString::new(id)))
                .with("Trnf", Value::List(vec![Value::Double(0.0); 8])),
        )
        .to_bytes(),
    );
    d
}

const PIXELS_IRRELEVANT: u8 = 8 | 16;

/// One of each kind, two smart objects sharing a placed item, and a group with a child.
fn layers() -> Vec<Lay> {
    let mut shape = Lay::new("Box")
        .id(6)
        .at([0, 0, 4, 4])
        .block(b"SoCo", &[0; 4])
        .block(b"vmsk", &[0; 8]);
    shape.flags = PIXELS_IRRELEVANT;
    vec![
        Lay::new("Photo").id(1).at([0, 0, 10, 10]).pixels(&[1; 100]),
        Lay::new("Title")
            .id(2)
            .at([20, 0, 30, 40])
            .block(b"TySh", &tysh("Title"))
            .pixels(&[2; 400]),
        Lay::new("Logo")
            .id(3)
            .at([0, 20, 8, 28])
            .block(b"SoLd", &sold("uuid-1"))
            .pixels(&[3; 64]),
        Lay::new("Logo copy")
            .id(4)
            .at([0, 40, 8, 48])
            .block(b"SoLd", &sold("uuid-1"))
            .pixels(&[3; 64]),
        Lay::new("Solid")
            .id(5)
            .at([0, 0, 2, 2])
            .block(b"SoCo", &[0; 4])
            .pixels(&[4; 4]),
        shape,
        Lay::end(),
        Lay::new("Child").id(9).pixels(&[5]),
        Lay::folder("G").id(7),
        Lay::end(),
        Lay::folder("Board").id(8).block(b"artb", &[0; 4]),
    ]
}

struct Spec {
    nodes: Vec<IndexNode>,
}

impl Spec {
    fn new() -> Self {
        let v = view_of(&psd_file(false, &layers(), &[], &[]));
        Spec { nodes: v.nodes }
    }
    fn at(&self, name: &str) -> usize {
        self.nodes
            .iter()
            .position(|n| n.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
    }
    fn anchor(&self, name: &str) -> Anchor {
        Anchor::on_node(&self.nodes[self.at(name)], 1, None)
    }
    /// The nodes, with `name`'s changed by `edit`.
    fn with(&self, name: &str, edit: impl FnOnce(&mut IndexNode)) -> Vec<IndexNode> {
        let mut nodes = self.nodes.clone();
        let i = self.at(name);
        edit(&mut nodes[i]);
        nodes
    }
    /// `name`'s anchor resolved against the nodes with `edit` applied, by the pack's rules.
    fn state(&self, name: &str, edit: impl FnOnce(&mut IndexNode)) -> AnchorState {
        resolve(&self.anchor(name), 2, &self.with(name, edit), &PsdPack).state
    }
    fn content_of(&self, name: &str) -> Option<k2_mpc_base::index::NodeHash> {
        self.nodes[self.at(name)].content_hash
    }
}

fn shift(n: &mut IndexNode, dx: i64) {
    let b = n.bounds.expect("check: the node has bounds");
    n.bounds = Some(NodeRect {
        left: b.left + dx,
        right: b.right + dx,
        ..b
    });
}

/// Far: ten widths and heights away, past any diagonal.
fn far(n: &mut IndexNode) {
    let b = n.bounds.expect("check: the node has bounds");
    let d = 10 * ((b.right - b.left) + (b.bottom - b.top)).max(1);
    shift(n, d);
}

/// The anchored layer deleted, and a different one in its place: a new key, no layer ID, another
/// name, no weak ID.
fn gone(n: &mut IndexNode) {
    n.key = NodeKey::parse("@k:gone.1").expect("check: a key");
    n.native = None;
    n.name = "Gone".into();
    n.fingerprint.weak.clear();
}

const KIND_CHANGED: AnchorState = AnchorState::Suspect(AnchorDoubt::KindChanged);
const IDENTITY_DOUBT: AnchorState = AnchorState::Suspect(AnchorDoubt::IdentityDoubt);
const CANDIDATE_ONLY: AnchorState = AnchorState::Suspect(AnchorDoubt::CandidateOnly);

#[test]
fn the_file_reads_to_one_node_of_each_kind() {
    let s = Spec::new();
    for (name, kind) in [
        ("Photo", "raster"),
        ("Title", "text"),
        ("Logo", "smart"),
        ("Logo copy", "smart"),
        ("Solid", "fill"),
        ("Box", "shape"),
        ("G", "group"),
        ("Board", "artboard"),
    ] {
        assert_eq!(s.nodes[s.at(name)].kind, kind, "{name}");
        assert_eq!(
            s.state(name, |_| {}),
            AnchorState::Attached,
            "{name} as anchored"
        );
    }
    assert_ne!(s.content_of("Photo"), s.content_of("Solid"));
    assert_eq!(
        s.nodes[s.at("Logo")].fingerprint.weak["placed_id"],
        s.nodes[s.at("Logo copy")].fingerprint.weak["placed_id"],
        "two layers share a placed item"
    );
}

#[test]
fn a_change_between_pixel_kinds_is_a_kind_change() {
    let s = Spec::new();
    for (name, to) in [
        ("Photo", "smart"),
        ("Title", "raster"),
        ("Logo", "raster"),
        ("Photo", "adjustment"),
        ("Solid", "raster"),
        ("G", "raster"),
        ("Board", "fill"),
    ] {
        assert_eq!(
            s.state(name, |n| n.kind = to.into()),
            KIND_CHANGED,
            "{name} to {to}"
        );
    }
}

#[test]
fn a_change_within_a_family_is_the_same_layer() {
    let s = Spec::new();
    for (name, to) in [
        ("G", "artboard"),
        ("Board", "group"),
        ("Solid", "shape"),
        ("Box", "fill"),
    ] {
        assert_eq!(
            s.state(name, |n| n.kind = to.into()),
            AnchorState::Attached,
            "{name} to {to}"
        );
        // the kit's default alone would call it a kind change: this is the pack's rule
        let nodes = s.with(name, |n| n.kind = to.into());
        assert_eq!(
            resolve(&s.anchor(name), 2, &nodes, &KitRules).state,
            KIND_CHANGED,
            "{name} to {to}, by the kit's default"
        );
    }
}

#[test]
fn a_plain_rename_move_or_pixel_edit_is_the_same_layer() {
    let s = Spec::new();
    let other = s.content_of("Solid");
    for name in ["Photo", "Title", "Logo", "Box"] {
        assert_eq!(
            s.state(name, |n| n.name = "Renamed".into()),
            AnchorState::Attached,
            "{name} renamed"
        );
        assert_eq!(
            s.state(name, far),
            AnchorState::Attached,
            "{name} moved far"
        );
        assert_eq!(
            s.state(name, |n| n.content_hash = other),
            AnchorState::Attached,
            "{name} with new pixels"
        );
        assert_eq!(
            s.state(name, |n| {
                n.name = "Renamed".into();
                far(n);
            }),
            AnchorState::Attached,
            "{name} renamed and moved far, its pixels the same"
        );
    }
}

#[test]
fn renamed_moved_and_repainted_at_once_is_in_doubt() {
    let s = Spec::new();
    let other = s.content_of("Solid");
    let all = |n: &mut IndexNode| {
        n.name = "Something else".into();
        far(n);
        n.content_hash = other;
    };
    for name in ["Photo", "Title", "Logo"] {
        assert_eq!(s.state(name, all), IDENTITY_DOUBT, "{name}");
    }
    // within a family too: a fill that became a shape and changed everything
    let other = s.content_of("Photo");
    assert_eq!(
        s.state("Solid", |n| {
            n.kind = "shape".into();
            n.name = "Something else".into();
            far(n);
            n.content_hash = other;
        }),
        IDENTITY_DOUBT
    );
    // a move within its own diagonal is no jump
    assert_eq!(
        s.state("Photo", |n| {
            n.name = "Something else".into();
            shift(n, 3);
            n.content_hash = other;
        }),
        AnchorState::Attached
    );
}

#[test]
fn a_shared_placed_id_vouches_for_nothing() {
    let s = Spec::new();
    let other = s.content_of("Photo");
    // the key's layer changed everything: the placed item it still shares doesn't vouch for it
    assert_eq!(
        s.state("Logo", |n| {
            n.name = "Something else".into();
            far(n);
            n.content_hash = other;
        }),
        IDENTITY_DOUBT
    );
    // the key is gone: the other layer of the placed item is a candidate, never taken
    let nodes = s.with("Logo", gone);
    let r = resolve(&s.anchor("Logo"), 2, &nodes, &PsdPack);
    assert_eq!(r.state, CANDIDATE_ONLY);
    assert_eq!(r.node, None, "a weak ID never identifies the layer");
    assert_eq!(r.candidates.len(), 1, "{:?}", r.candidates);
    assert_eq!(r.candidates[0].key, s.nodes[s.at("Logo copy")].key);
    assert_eq!(r.candidates[0].why, CandidateWhy::FingerprintMatch);
}

#[test]
fn candidates_come_from_the_same_family_only() {
    let s = Spec::new();
    // a placed ID on a layer of another kind is no candidate
    let mut nodes = s.with("Logo", gone);
    let copy = nodes.iter().position(|n| n.name == "Logo copy").unwrap();
    nodes[copy].kind = "raster".into();
    let photo = nodes.iter().position(|n| n.name == "Photo").unwrap();
    nodes[photo]
        .fingerprint
        .weak
        .insert("placed_id".into(), "uuid-1".into());
    let r = resolve(&s.anchor("Logo"), 2, &nodes, &PsdPack);
    assert_eq!(r.state, AnchorState::Orphaned, "{:?}", r.candidates);

    // a group's name on an artboard: a candidate across the family
    let mut nodes = s.with("G", gone);
    let board = nodes.iter().position(|n| n.name == "Board").unwrap();
    nodes[board].name = "G".into();
    let r = resolve(&s.anchor("G"), 2, &nodes, &PsdPack);
    assert_eq!(r.state, CANDIDATE_ONLY);
    assert_eq!(r.candidates.len(), 1);
    assert_eq!(r.candidates[0].why, CandidateWhy::SameKindAndName);
    assert_eq!(r.candidates[0].key, nodes[board].key);

    // the text's name on a raster layer: no candidate
    let mut nodes = s.with("Title", gone);
    let photo = nodes.iter().position(|n| n.name == "Photo").unwrap();
    nodes[photo].name = "Title".into();
    let r = resolve(&s.anchor("Title"), 2, &nodes, &PsdPack);
    assert_eq!(r.state, AnchorState::Orphaned, "{:?}", r.candidates);

    // an empty name matches nothing
    let mut nodes = s.with("Photo", |n| {
        n.name = String::new();
        n.fingerprint.name = String::new();
    });
    let anchor = Anchor::on_node(&nodes[s.at("Photo")], 1, None);
    gone(&mut nodes[s.at("Photo")]);
    let child = nodes.iter().position(|n| n.name == "Child").unwrap();
    nodes[child].name = String::new();
    let r = resolve(&anchor, 2, &nodes, &PsdPack);
    assert_eq!(r.state, AnchorState::Orphaned, "{:?}", r.candidates);
}

#[test]
fn a_hint_is_never_judged_off() {
    let s = Spec::new();
    let i = s.at("Photo");
    let a = Anchor::on_node(&s.nodes[i], 1, Some("0.42,0.13".into()));
    let r = resolve(&a, 2, &s.with("Photo", far), &PsdPack);
    assert_eq!(r.state, AnchorState::Attached);
}

#[test]
fn the_session_calls_the_packs_hooks() {
    let f = fx("thread");
    let one = psd_file(
        false,
        &[
            Lay::new("Photo").id(1).at([0, 0, 4, 4]).pixels(&[1; 16]),
            Lay::end(),
            Lay::new("In").id(3),
            Lay::folder("G").id(2),
        ],
        &[],
        &[],
    );
    let s = f.open("a.psd", &one);
    let c = f.join(&s, "alice");
    indexed(&s);
    let photo = s
        .comment(
            c,
            NewComment::new(NewAnchor::node("@lyid:1", None), "this one"),
        )
        .unwrap()
        .id;
    let group = s
        .comment(
            c,
            NewComment::new(NewAnchor::node("@lyid:2", None), "and this"),
        )
        .unwrap()
        .id;

    // the photo becomes a smart object, the group an artboard: same IDs
    let two = psd_file(
        false,
        &[
            Lay::new("Photo")
                .id(1)
                .at([0, 0, 4, 4])
                .block(b"SoLd", &sold("u"))
                .pixels(&[1; 16]),
            Lay::end(),
            Lay::new("In").id(3),
            Lay::folder("G").id(2).block(b"artb", &[0; 4]),
        ],
        &[],
        &[],
    );
    s.write_bytes(c, &two, s.status().unwrap().version).unwrap();
    indexed(&s);
    let state = |t| {
        s.thread(c, t, None)
            .unwrap()
            .resolution
            .expect("resolved against the new index")
            .state
    };
    assert_eq!(state(photo), KIND_CHANGED, "raster to smart");
    assert_eq!(state(group), AnchorState::Attached, "group to artboard");
}

// ----- the public corpus -----

/// What the pack's rules must say for one corpus node and one change, judged here from the
/// kinds and fields alone.
#[derive(Default)]
struct Tally {
    rows: u32,
    by: HashMap<&'static str, u32>,
}

impl Tally {
    fn row(
        &mut self,
        what: &'static str,
        got: AnchorState,
        want: AnchorState,
        file: &str,
        i: usize,
    ) {
        assert_eq!(got, want, "{file} node {i}: {what}");
        self.rows += 1;
        *self.by.entry(what).or_default() += 1;
    }
}

fn base_state(n: &IndexNode) -> AnchorState {
    if n.visible_in_tree {
        AnchorState::Attached
    } else {
        AnchorState::Hidden
    }
}

fn other_family(kind: &str) -> &'static str {
    if same_family(kind, "adjustment") {
        "raster"
    } else {
        "adjustment"
    }
}

fn partner(kind: &str) -> Option<&'static str> {
    match kind {
        "group" => Some("artboard"),
        "artboard" => Some("group"),
        "fill" => Some("shape"),
        "shape" => Some("fill"),
        _ => None,
    }
}

fn corpus_rows(file: &str, v: &IndexView, t: &mut Tally) {
    let nodes = &v.nodes;
    for (i, n) in nodes.iter().enumerate() {
        if !matches!(n.key_class, k2_mpc_base::index::KeyClass::Native) {
            continue; // minted keys are the kit's to carry; the pack judges native layers here
        }
        let a = Anchor::on_node(n, 1, None);
        let state = |edit: &dyn Fn(&mut IndexNode)| {
            let mut changed = nodes.clone();
            edit(&mut changed[i]);
            resolve(&a, 2, &changed, &PsdPack).state
        };
        let base = base_state(n);
        t.row("as anchored", state(&|_| {}), base, file, i);
        t.row(
            "renamed",
            state(&|m| m.name.push_str(" renamed")),
            base,
            file,
            i,
        );
        t.row(
            "new pixels",
            state(&|m| m.content_hash = None),
            base,
            file,
            i,
        );
        let to = other_family(&n.kind);
        t.row(
            "another family",
            state(&|m| m.kind = to.into()),
            KIND_CHANGED,
            file,
            i,
        );
        if let Some(p) = partner(&n.kind) {
            t.row(
                "its family partner",
                state(&|m| m.kind = p.into()),
                base,
                file,
                i,
            );
        }
        if n.bounds.is_some() {
            t.row("moved far", state(&far), base, file, i);
            t.row(
                "renamed, moved far and repainted",
                state(&|m| {
                    m.name.push_str(" renamed");
                    far(m);
                    m.content_hash = None;
                }),
                IDENTITY_DOUBT,
                file,
                i,
            );
            if n.content_hash.is_some() {
                t.row(
                    "renamed and moved far, the same pixels",
                    state(&|m| {
                        m.name.push_str(" renamed");
                        far(m);
                    }),
                    base,
                    file,
                    i,
                );
            }
        }
        // the key gone: candidates by family, placed ID or name, never one of another family,
        // never taken
        let mut changed = nodes.clone();
        gone(&mut changed[i]);
        let r = resolve(&a, 2, &changed, &PsdPack);
        let want: Vec<&NodeKey> = changed
            .iter()
            .filter(|m| {
                same_family(&n.kind, &m.kind)
                    && (n
                        .fingerprint
                        .weak
                        .iter()
                        .any(|(k, val)| m.fingerprint.weak.get(k) == Some(val))
                        || (!n.name.is_empty() && m.name == n.name))
            })
            .map(|m| &m.key)
            .take(8)
            .collect();
        let got: Vec<&NodeKey> = r.candidates.iter().map(|c| &c.key).collect();
        assert_eq!(got, want, "{file} node {i}: candidates");
        assert_eq!(r.node, None, "{file} node {i}: a candidate is never taken");
        let want_state = if want.is_empty() {
            AnchorState::Orphaned
        } else {
            CANDIDATE_ONLY
        };
        t.row("its key gone", r.state, want_state, file, i);
    }
}

#[test]
fn every_public_corpus_node_follows_the_packs_rules() {
    let Some(dir) = std::env::var_os("PSD_PUBLIC_CORPUS") else {
        eprintln!("public corpus: skipped (PSD_PUBLIC_CORPUS is not set)");
        return;
    };
    let dir = Path::new(&dir);
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/manifest.tsv"
    ))
    .expect("check: the manifest is committed");
    let mut t = Tally::default();
    let mut files = 0;
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let name = line
            .split('\t')
            .next()
            .expect("check: a manifest row has fields");
        if !name.ends_with(".psd") && !name.ends_with(".psb") {
            continue;
        }
        let bytes = std::fs::read(dir.join(name))
            .unwrap_or_else(|_| panic!("{name}: listed in the manifest, missing from the corpus"));
        let v = view_of(&bytes);
        corpus_rows(name, &v, &mut t);
        files += 1;
    }
    assert_eq!(files, 18, "every PSD and PSB of the manifest is checked");
    let mut by: Vec<_> = t.by.iter().collect();
    by.sort();
    eprintln!("anchor rows: {files} files, {} rows, {by:?}", t.rows);
    assert!(t.rows > 1000, "{} rows", t.rows);
}

/// The rules object the session uses is the pack's (a sanity check of the trait impl).
#[test]
fn the_pack_is_its_own_anchor_rules() {
    let s = Spec::new();
    let i = s.at("G");
    let mut now = s.nodes[i].clone();
    now.kind = "artboard".into();
    let fp = &s.nodes[i].fingerprint;
    assert_eq!(
        AnchorRules::judge(&PsdPack, fp, None, &now),
        k2_mpc_base::session::BlobPack::judge_anchor(&PsdPack, fp, None, &now)
    );
}
