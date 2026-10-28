//! The double BOM: `cat` prints a file's bytes, BOM included, and the doc text never holds
//! it. A `write` (`replace_text`) or `write --base` body read that way starts with U+FEFF;
//! when the doc's codec writes a BOM ([`Pack::writes_bom`]) that one U+FEFF is taken off, so
//! the file keeps exactly one. Without a BOM in the file, a leading U+FEFF is text and stays.
//! Checked for the kit's `TextFilePack` (and a pack delegating to it, as the Markdown pack
//! does) and for a pack with its own BOM codec (the default `writes_bom`). Public API only.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::schema::Kind;
use k2_mpc_base::session::{
    AuditEvent, ConnId, Declined, Host, Pack, PackContent, PackError, Participant, ParticipantKind,
    Rights, Session, SessionManager, Stay, TextFilePack, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::Schema;

const BOM: &str = "\u{feff}";
const TEXT: TextFilePack = TextFilePack::new("bomtext", 1 << 20);

/// Delegates everything to the kit's pack (as Markdown's does) and keeps the default
/// `writes_bom`.
struct Delegating;

impl Pack for Delegating {
    fn schema(&self) -> Schema {
        TEXT.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        TEXT.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        TEXT.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        TEXT.normalize_input(root, text)
    }
}

/// Its own codec: `[1]` = a BOM, `[0]` = none; one root named `body`. Default `writes_bom`.
struct OwnCodec;

impl Pack for OwnCodec {
    fn schema(&self) -> Schema {
        Schema {
            name: "bomown",
            roots: &[("body", Kind::Text)],
        }
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let (bom, rest) = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
            Some(r) => (1, r),
            None => (0, bytes),
        };
        let text = String::from_utf8(rest.to_vec()).map_err(|_| Declined {
            code: "not_utf8",
            message: "not UTF-8".into(),
        })?;
        Ok(PackContent {
            roots: vec![("body".into(), Neutral::Text(text))],
            codec: vec![bom],
        })
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let [(_, Neutral::Text(t))] = content else {
            return Err(PackError("one root".into()));
        };
        let mut out = if codec == [1] {
            b"\xEF\xBB\xBF".to_vec()
        } else {
            Vec::new()
        };
        out.extend_from_slice(t.as_bytes());
        Ok(out)
    }
}

struct TestHost(AtomicU64);

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::editor()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    _mgr: SessionManager,
    s: Arc<Session>,
    c: ConnId,
    root: &'static str,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(pack: Arc<dyn Pack>, root: &'static str, file: &[u8]) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-bom-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(
        store,
        Arc::new(TestHost(AtomicU64::new(1_000_000))),
        Default::default(),
    );
    let key = DocKey {
        space: "ws".into(),
        path: "notes.txt".into(),
    };
    let s = mgr.open(pack, &key, Some(file)).unwrap();
    let p = Participant::new("agent", "Agent", "#123456", ParticipantKind::Agent, "cli");
    let c = s.join(p, Stay::Held).unwrap().conn;
    Fx {
        dir,
        _mgr: mgr,
        s,
        c,
        root,
    }
}

impl Fx {
    /// What `cat` prints: the file's bytes.
    fn cat(&self) -> (String, u64) {
        let out = self
            .s
            .run_tool(self.c, self.root, &["cat".to_owned()], b"")
            .unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.version)
    }
    fn text(&self) -> String {
        self.s.read(self.c).unwrap().text_by_root.remove(0).1
    }
    fn bytes(&self) -> Vec<u8> {
        self.s.read(self.c).unwrap().bytes
    }
}

fn bom_count(b: &[u8]) -> usize {
    b.windows(3).filter(|w| *w == b"\xEF\xBB\xBF").count()
}

/// `cat`, change a line, `write` back (both `write --base V` and `--base live`): one BOM.
fn cat_edit_write_keeps_one_bom(pack: impl Fn() -> Arc<dyn Pack>, root: &'static str, file: &[u8]) {
    for base_live in [false, true] {
        let f = fx(pack(), root, file);
        let (shown, v) = f.cat();
        assert!(shown.starts_with(BOM), "cat prints the BOM");
        let body = shown.replace("two", "TWO");
        if base_live {
            f.s.replace_text(f.c, root, &body).unwrap();
        } else {
            let out = f.s.write_base(f.c, root, v, &body).unwrap();
            assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
        }
        assert!(!f.text().starts_with(BOM), "the doc text holds no BOM");
        assert!(f.text().contains("TWO"));
        let bytes = f.bytes();
        assert!(bytes.starts_with(b"\xEF\xBB\xBF"), "the file keeps its BOM");
        assert_eq!(bom_count(&bytes), 1, "exactly one BOM: {bytes:?}");
        // the body as the doc holds it (no BOM) also writes one BOM
        let (shown, v) = f.cat();
        let plain = shown.strip_prefix(BOM).unwrap().replace("one", "ONE");
        let out = f.s.write_base(f.c, root, v, &plain).unwrap();
        assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
        assert_eq!(bom_count(&f.bytes()), 1);
        // a write of the bytes `cat` printed, unchanged, is no change at all
        let (shown, v) = f.cat();
        let before = f.s.status().unwrap().version;
        let out = f.s.write_base(f.c, root, v, &shown).unwrap();
        match out {
            WriteOutcome::Applied(r) => assert!(r.changed.is_empty(), "{r:?}"),
            o => panic!("{o:?}"),
        }
        assert_eq!(f.s.status().unwrap().version, before);
    }
}

/// Without a BOM in the file, a leading U+FEFF in a body is text: it stays (one, not two).
fn no_bom_file_keeps_a_leading_feff(pack: impl Fn() -> Arc<dyn Pack>, root: &'static str) {
    let f = fx(pack(), root, b"one\ntwo\n");
    let (shown, v) = f.cat();
    assert!(!shown.starts_with(BOM));
    let out =
        f.s.write_base(f.c, root, v, &format!("{BOM}{shown}"))
            .unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
    assert_eq!(f.text(), format!("{BOM}one\ntwo\n"));
    // only one U+FEFF is ever taken off, and only from a BOM file: two stay two here
    f.s.replace_text(f.c, root, &format!("{BOM}{BOM}x\n"))
        .unwrap();
    assert_eq!(f.text(), format!("{BOM}{BOM}x\n"));
}

#[test]
fn textfile_pack_cat_then_write_keeps_one_bom() {
    cat_edit_write_keeps_one_bom(
        || Arc::new(TEXT),
        TextFilePack::ROOT,
        b"\xEF\xBB\xBFone\ntwo\nthree\n",
    );
    // CRLF too
    cat_edit_write_keeps_one_bom(
        || Arc::new(TEXT),
        TextFilePack::ROOT,
        b"\xEF\xBB\xBFone\r\ntwo\r\nthree\r\n",
    );
    no_bom_file_keeps_a_leading_feff(|| Arc::new(TEXT), TextFilePack::ROOT);
}

#[test]
fn a_pack_delegating_to_the_textfile_codec_needs_no_change() {
    cat_edit_write_keeps_one_bom(
        || Arc::new(Delegating),
        TextFilePack::ROOT,
        b"\xEF\xBB\xBFone\ntwo\nthree\n",
    );
    no_bom_file_keeps_a_leading_feff(|| Arc::new(Delegating), TextFilePack::ROOT);
}

#[test]
fn a_pack_with_its_own_bom_codec_gets_it_by_default() {
    cat_edit_write_keeps_one_bom(|| Arc::new(OwnCodec), "body", b"\xEF\xBB\xBFone\ntwo\n");
    no_bom_file_keeps_a_leading_feff(|| Arc::new(OwnCodec), "body");
}

/// `writes_bom` itself: the kit's pack reads its settings; the default encodes an empty root.
#[test]
fn writes_bom_answers() {
    let with = TEXT.decode(b"\xEF\xBB\xBFx\n").unwrap().codec;
    let without = TEXT.decode(b"x\n").unwrap().codec;
    assert!(TEXT.writes_bom(TextFilePack::ROOT, &with));
    assert!(!TEXT.writes_bom(TextFilePack::ROOT, &without));
    assert!(!TEXT.writes_bom("other", &with));
    assert!(
        !TEXT.writes_bom(TextFilePack::ROOT, &[9]),
        "bad settings: no"
    );
    assert!(Delegating.writes_bom(TextFilePack::ROOT, &with));
    assert!(!Delegating.writes_bom(TextFilePack::ROOT, &without));
    assert!(OwnCodec.writes_bom("body", &[1]));
    assert!(!OwnCodec.writes_bom("body", &[0]));
    assert!(!OwnCodec.writes_bom("source", &[1]));
}
