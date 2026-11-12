//! N−1 of the structure index's row format 2 (tools/n1/n1.sh `layout-index`), built against the
//! previous release, which knows rows of format 1 only. Its pack is the file type before the
//! bump: `idx` format 1, no view items (as a previous release's pack is), and `plain` format 1.
//!
//! - **This build's store** (`$K2MB_N1_OUT/index`, from this repo's `n1_index_producer`):
//!   `plain.idx` and `old.idx` hold rows of format 1 and read at once; `v2.idx` holds a row of
//!   format 2 only, under the bumped pack's format, which this build never asks for: it reads
//!   the version as pending, indexes it under its own format, and its commit removes the newer
//!   row. It never decodes a row of format 2 (it would refuse one as damaged).
//! - **The other way:** it writes a store of its own (`$K2MB_N1_OUT/prev`: `a.idx` and `p.idx`,
//!   rows of format 1), which this repo's `n1_index_after` opens.
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::index::{
    Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexStatus, IndexVerbs,
};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, Host, Participant, Rights, SessionManager,
};
use k2_mpc_base::store::{DocKey, DocStore};

const VERBS: IndexVerbs = IndexVerbs::new("part list", "part show", "part diff", "part blame", None);

/// The file the producer imports: a page, a box with text, a group and its child
/// (`key|kind|name|parent|vis|text|content`).
const TREE: &str = "IDX\n@pg:1|page|One|-|1|-|-\n@pg:1/it:1|box|Title|0|1|Hello|-\n\
                    @pg:1/it:2|group|G|0|1|-|-\n@pg:1/it:3|box|Kid|2|1|-|-\n";

/// The pack before the bump: the same nodes as the producer's, no view items.
struct OldPack(&'static str);

impl BlobPack for OldPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new(self.0, 1, &VERBS))
    }
    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        _limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let mut b = String::new();
        src.read_to_string(&mut b).map_err(IndexRefused::Read)?;
        let mut lines = b.lines();
        if lines.next() != Some("IDX") {
            return Err(IndexRefused::Unsupported);
        }
        let mut d = IndexDraft::new("fmt-1");
        // `#attr` lines (each doc's own bytes) carry no node
        for l in lines.filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = l.split('|').collect();
            let mut n = DraftNode::new(f[1], f[2], Digest::of(l.as_bytes()));
            n.key = (f[0] != "-").then(|| f[0].to_owned());
            n.parent = (f[3] != "-").then(|| f[3].parse().unwrap());
            n.visible = f[4] == "1";
            n.text = (f[5] != "-").then(|| f[5].to_owned());
            d.nodes.push(n);
        }
        Ok(d)
    }
}

struct H {
    rights: Mutex<HashMap<String, Rights>>,
    now: AtomicU64,
}
impl Host for H {
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

fn manager(dir: &Path) -> SessionManager {
    std::fs::create_dir_all(dir).unwrap();
    let host = Arc::new(H {
        rights: Mutex::new(HashMap::new()),
        now: AtomicU64::new(1_000_000),
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
    SessionManager::new(store, host, Config::default()).with_blob_store(blobs)
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn open(m: &SessionManager, pack: &'static str, path: &str, bytes: Option<&[u8]>) -> Arc<BlobSession> {
    let mut src = bytes;
    m.open_blob(
        Arc::new(OldPack(pack)),
        &key(path),
        src.as_mut().map(|b| b as &mut dyn Read),
    )
    .unwrap()
}

/// The version's node keys, read back.
fn keys(s: &BlobSession) -> Vec<String> {
    s.index_view(None)
        .unwrap()
        .nodes
        .iter()
        .map(|n| n.key.as_str().to_owned())
        .collect()
}

fn ready(s: &BlobSession, what: &str) {
    assert_eq!(
        s.index_status(None).unwrap(),
        IndexStatus::Ready {
            version: 1,
            nodes: 4
        },
        "{what}"
    );
    assert_eq!(
        keys(s),
        ["@pg:1", "@pg:1/it:1", "@pg:1/it:2", "@pg:1/it:3"],
        "{what}"
    );
}

fn main() {
    let dir = PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    {
        let m = manager(&dir.join("index"));
        for (pack, doc) in [("plain", "plain.idx"), ("idx", "old.idx")] {
            let s = open(&m, pack, doc, None);
            ready(&s, &format!("{doc}: this build's format-1 row reads here"));
            m.close(&key(doc), "n1").unwrap();
        }
        let s = open(&m, "idx", "v2.idx", None);
        assert_eq!(
            s.index_status(None).unwrap(),
            IndexStatus::Pending {
                version: 1,
                running: false
            },
            "v2.idx: only a row of the bumped format, never read here: pending"
        );
        let st = s.index_job(None).unwrap().expect("a job").run().unwrap();
        assert_eq!(st, IndexStatus::Ready { version: 1, nodes: 4 });
        ready(&s, "v2.idx: indexed again under the older format");
        m.close(&key("v2.idx"), "n1").unwrap();
        println!("N-1 INDEX previous release: read plain.idx and old.idx, indexed v2.idx again");
    }
    {
        let m = manager(&dir.join("prev"));
        let tree = TREE.as_bytes();
        for (pack, doc) in [("idx", "a.idx"), ("plain", "p.idx")] {
            let s = open(&m, pack, doc, Some(tree));
            let st = s.index_job(None).unwrap().expect("a job").run().unwrap();
            assert_eq!(st, IndexStatus::Ready { version: 1, nodes: 4 }, "{doc}");
            m.close(&key(doc), "n1").unwrap();
        }
        println!("N-1 INDEX previous release: wrote prev/ (a.idx, p.idx)");
    }
}
