//! N−1 of the structure index's row format 3 (a container's own record digest; tools/n1/n1.sh
//! `layout-own`), built against the previous release, which knows rows of format 1 and 2 only.
//! Its packs are the file type before own records: `deck` format 2 (reading format 1), with a
//! view item (the page's render digest) and no own digest, as a previous release's pack is;
//! and `bad` the same, standing for a pack that declared own records without bumping its
//! format (this build's `bad` is format 2 too).
//!
//! - **This build's store** (`$K2MB_N1_OUT/own`, from this repo's `n1_own_producer`): `a.idx`
//!   and `bad.idx` each hold one row, under the kit's offset format for own records, which this
//!   build never asks for: it reads each version as pending (never `Corrupt`), indexes it under
//!   its own format, and its commit removes the offset row. Under the producer's planted fault
//!   (`own-offset-skipped`), `bad.idx`'s row is under format 2 and this side fails, naming what
//!   it read.
//! - **The other way:** it writes a store of its own (`$K2MB_N1_OUT/prev-own`: `p.idx`, a row
//!   of format 2), which this repo's `n1_own_after` opens.
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

/// The file this side imports: the producer's tree (`key|kind|name|parent|vis|text|content`),
/// its own bytes (`#attr doc=p`).
const TREE: &str = "IDX\n#attr doc=p\n@pg:1|page|One|-|1|-|-\n@pg:1/it:1|box|Title|0|1|Hello|-\n\
                    @pg:1/it:2|group|G|0|1|-|-\n@pg:1/it:3|box|Kid|2|1|-|-\n";

/// The pack before the bump: format 2 (reading format 1), the page's render digest its one
/// view item, no own digest.
struct OldPack(&'static str);

impl BlobPack for OldPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new(self.0, 2, &VERBS).with_older_format(1))
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
            if f[1] == "page" {
                n.render = Some(Digest::of(format!("draws {}", f[2]).as_bytes()));
            }
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

fn main() {
    let dir = PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"));
    {
        let m = manager(&dir.join("own"));
        for (pack, doc) in [("deck", "a.idx"), ("bad", "bad.idx")] {
            let s = open(&m, pack, doc, None);
            let st = s.index_status(None);
            assert!(
                matches!(
                    st,
                    Ok(IndexStatus::Pending {
                        version: 1,
                        running: false
                    })
                ),
                "{doc}: only a row under a format this build never asks for: pending, never \
                 Corrupt ({st:?})"
            );
            let st = s.index_job(None).unwrap().expect("a job").run().unwrap();
            assert_eq!(st, IndexStatus::Ready { version: 1, nodes: 4 }, "{doc}");
            assert_eq!(
                keys(&s),
                ["@pg:1", "@pg:1/it:1", "@pg:1/it:2", "@pg:1/it:3"],
                "{doc}: indexed again under its own format"
            );
            assert_eq!(s.status().unwrap().version, 1, "{doc}: the doc's status reads");
            m.close(&key(doc), "n1").unwrap();
        }
        println!("N-1 OWN previous release: a.idx and bad.idx pending, indexed again; no Corrupt");
    }
    {
        let m = manager(&dir.join("prev-own"));
        let s = open(&m, "deck", "p.idx", Some(TREE.as_bytes()));
        let st = s.index_job(None).unwrap().expect("a job").run().unwrap();
        assert_eq!(st, IndexStatus::Ready { version: 1, nodes: 4 }, "p.idx");
        m.close(&key("p.idx"), "n1").unwrap();
        println!("N-1 OWN previous release: wrote prev-own/ (p.idx, a row of format 2)");
    }
}
