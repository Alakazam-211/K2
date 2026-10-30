//! Exports, the kit's side (P0): `Session::file_at` / `file_now` (the file's bytes at a
//! version) and the `export` contract (the registry, the identity exporter, a test exporter
//! for the contract). Public API only.
//!
//! Grid docs (k2-mpc-base's C1) get their `file_at` round trip at the C1 ↔ main round.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobError, BlobStore};
use k2_mpc_base::errors::Refusal;
use k2_mpc_base::export::{
    AsIs, ExportCtl, ExportError, ExportLimits, ExportRegistry, ExportReport, ExportTarget,
    Exporter, Fidelity, Loss, NoResources, OptionKind, OptionSpec, Options, RegistryError, Sink,
    Source, SourceKind, VecSink, AS_IS,
};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, ConnId, FileCodec, Host, LogQuery, Need, Participant,
    ParticipantKind, Rights, Session, SessionError, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

const TEXT: TextFilePack = TextFilePack::new("text", 4 << 20);
const MARKDOWN: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct H {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::full())
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    host: Arc<H>,
    s: Arc<Session>,
    c: ConnId,
}

fn fx(tag: &str, pack: TextFilePack, path: &str, seed: &[u8]) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-export-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let s = mgr
        .open(
            Arc::new(pack),
            &DocKey {
                space: "ws".into(),
                path: path.into(),
            },
            Some(seed),
        )
        .unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx {
        _dir: TmpDir(dir),
        host,
        s,
        c,
    }
}

impl Fx {
    fn append(&self, line: &str) {
        let argv: Vec<String> = vec!["append".into()];
        let o = self
            .s
            .run_tool(self.c, ROOT, &argv, line.as_bytes())
            .unwrap();
        assert_eq!(o.exit, 0);
    }
    fn join(&self, pid: &str, r: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(pid.into(), r);
        self.s
            .join(
                Participant::new(pid, pid, "#2", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }
    /// Everything an export must leave alone (X-DOC-1, the parts the API shows).
    fn untouched(&self) -> String {
        format!(
            "{:?}\n{:?}\n{:?}",
            self.s.status().unwrap(),
            self.s.versions(self.c).unwrap(),
            self.s.log(self.c, &LogQuery::default()).unwrap()
        )
    }
}

/// `file_at` gives each version's file bytes as the write-back wrote them then (the same
/// codec: here a BOM and CRLF), with that version's doc number; the latest is exact, older
/// ones are rendered with today's codec (`FileCodec::Current`).
fn round_trip(pack: TextFilePack, path: &str, seed: &[u8]) {
    let f = fx(pack.limit().to_string().as_str(), pack, path, seed);
    let mut want = Vec::new();
    for (i, line) in ["two\n", "three\n", "four\n"].iter().enumerate() {
        f.append(line);
        let r = f.s.read(f.c).unwrap();
        let vid = f.s.name_version(f.c, &format!("v{i}"), None).unwrap();
        want.push((vid, r.bytes, r.version));
    }
    let last = want.last().unwrap().0;
    for (vid, bytes, version) in &want {
        let got = f.s.file_at(f.c, *vid).unwrap();
        assert_eq!(&got.bytes, bytes, "{path} v{vid}: the file's bytes then");
        assert_eq!(got.version, *version, "{path} v{vid}: the doc version");
        let codec = if *vid == last {
            FileCodec::Exact
        } else {
            FileCodec::Current
        };
        assert_eq!(got.codec, codec, "{path} v{vid}");
    }
    // the codec is applied: the bytes are the file's, not the text
    let first = &want[0].1;
    if seed.starts_with(b"\xEF\xBB\xBF") {
        assert!(first.starts_with(b"\xEF\xBB\xBF"), "BOM kept");
    }
    if seed.windows(2).any(|w| w == b"\r\n") {
        assert!(
            first.windows(2).filter(|w| *w == b"\r\n").count() >= 2,
            "CRLF kept"
        );
    }
    // after more edits the old versions still render as they were
    f.append("five\n");
    for (vid, bytes, _) in &want {
        assert_eq!(&f.s.file_at(f.c, *vid).unwrap().bytes, bytes);
    }
    let now = f.s.file_now(f.c).unwrap();
    let r = f.s.read(f.c).unwrap();
    assert_eq!(
        (now.bytes, now.version, now.codec),
        (r.bytes, r.version, FileCodec::Exact)
    );
}

#[test]
fn file_at_round_trips_a_text_doc() {
    round_trip(TEXT, "notes.txt", b"one\n");
}

#[test]
fn file_at_round_trips_a_markdown_doc_with_a_bom_and_crlf() {
    round_trip(MARKDOWN, "readme.md", b"\xEF\xBB\xBF# Title\r\none\r\n");
}

#[test]
fn file_at_an_unknown_version_is_a_clear_error() {
    let f = fx("unknown", TEXT, "a.txt", b"one\n");
    let e = f.s.file_at(f.c, 9_999).unwrap_err();
    assert!(matches!(e, SessionError::NoSuchChange(9_999)), "{e:?}");
    assert_eq!(e.to_string(), "no change or version 9999 in this doc");
    assert_eq!(e.report().exit(), 4, "not found: {}", e.report().plain());
}

/// `file_at` needs history (a version is history, like `show`); `file_now` needs read only.
#[test]
fn rights_file_at_needs_history_file_now_read() {
    let f = fx("rights", TEXT, "a.txt", b"one\n");
    let vid = f.s.name_version(f.c, "v", None).unwrap();
    let reader = f.join("reader", Rights::read_only());
    assert!(matches!(
        f.s.file_at(reader, vid),
        Err(SessionError::Forbidden {
            need: Need::History
        })
    ));
    assert_eq!(f.s.file_now(reader).unwrap().bytes, b"one\n");
    // read taken away: file_now is refused too
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("reader".into(), Rights::none());
    assert!(f.s.file_now(reader).is_err());
}

/// X-DOC-1 (what the API shows): `file_at`, `file_now` and an export through the registry
/// leave the doc as it was: status, versions, log.
#[test]
fn reading_and_exporting_never_change_the_doc() {
    let f = fx("untouched", MARKDOWN, "a.md", b"one\n");
    f.append("two\n");
    let vid = f.s.name_version(f.c, "v", None).unwrap();
    f.append("three\n");
    let before = f.untouched();
    let at = f.s.file_at(f.c, vid).unwrap();
    let _ = f.s.file_now(f.c).unwrap();
    let reg = ExportRegistry::with_as_is();
    let mut out = VecSink::default();
    let mut read = &at.bytes[..];
    let r = reg
        .export(
            &SourceKind::Pack("markdown".into()),
            Source::Bytes {
                read: &mut read,
                size: at.bytes.len() as u64,
                name: "a.md",
            },
            "as-is",
            &Options::new(),
            &mut out,
            &ctl(&AtomicBool::new(false), ExportLimits::default()),
        )
        .unwrap();
    // X-DOC-3: the as-is target passes the version's bytes through unchanged
    assert_eq!(out.bytes, at.bytes);
    assert_eq!(r.bytes, at.bytes.len() as u64);
    assert_eq!(f.untouched(), before, "the doc is as it was");
}

// ---------------------------------------------------------------- the contract

fn ctl<'a>(cancel: &'a AtomicBool, limits: ExportLimits) -> ExportCtl<'a> {
    static PROGRESS: fn(u64, u64) = |_, _| {};
    ExportCtl {
        cancel,
        progress: &PROGRESS,
        limits,
        resources: &NoResources,
    }
}

/// A test exporter: `upper` writes a text source in upper case (option `bang`: append "!",
/// option `size`: a number 1..=4 that does nothing), reporting one loss; `pages` writes
/// `pages` lines and reports them; `panic` panics; `stall` checks the cancel flag.
struct Upper;

const UPPER_OPTS: &[OptionSpec] = &[
    OptionSpec::new("bang", OptionKind::Bool, Some("false"), "append !"),
    OptionSpec::new("size", OptionKind::Int { min: 1, max: 4 }, None, "unused"),
];

impl Exporter for Upper {
    fn id(&self) -> &'static str {
        "test-upper@1"
    }
    fn targets(&self, kind: &SourceKind) -> Vec<ExportTarget> {
        match kind {
            SourceKind::Pack(p) if p == "markdown" => vec![
                ExportTarget::new("upper", "text/plain", "txt", Fidelity::Lossy, UPPER_OPTS),
                ExportTarget::new("panic", "text/plain", "txt", Fidelity::Lossy, &[]),
                ExportTarget::new("stall", "text/plain", "txt", Fidelity::Lossy, &[]),
                // a target the identity exporter also lists: the first registered wins
                AS_IS,
            ],
            _ => Vec::new(),
        }
    }
    fn export(
        &self,
        src: Source<'_>,
        target: &str,
        opts: &Options,
        out: &mut dyn Sink,
        ctl: &ExportCtl<'_>,
    ) -> Result<ExportReport, ExportError> {
        let text = match src {
            Source::Text { roots, .. } => roots.iter().map(|(_, t)| t.as_str()).collect(),
            Source::Bytes { read, .. } => {
                let mut s = String::new();
                read.read_to_string(&mut s)
                    .map_err(|e| ExportError::Decode(e.to_string()))?;
                s
            }
            _ => return Err(ExportError::Decode("unknown source".into())),
        };
        match target {
            "upper" => {
                let bang = opts.value(&UPPER_OPTS[0]) == Some("true");
                out.write_all(text.to_uppercase().as_bytes())?;
                if bang {
                    out.write_all(b"!")?;
                }
                let mut r = ExportReport::default();
                r.losses
                    .push(Loss::new("case-folded", "lower case is upper case"));
                Ok(r)
            }
            "panic" => panic!("planted"),
            "stall" => loop {
                ctl.check()?;
                std::thread::yield_now();
            },
            _ => Err(ExportError::Unsupported {
                kind: "markdown".into(),
                target: target.into(),
            }),
        }
    }
}

fn run(
    reg: &ExportRegistry,
    kind: &str,
    target: &str,
    opts: &[&str],
    cancel: &AtomicBool,
    limits: ExportLimits,
) -> (Result<ExportReport, ExportError>, Vec<u8>) {
    let roots = vec![(ROOT.to_owned(), "hello\n".to_owned())];
    let mut out = VecSink::default();
    let r = reg.export(
        &SourceKind::Pack(kind.into()),
        Source::Text {
            pack: kind,
            roots: &roots,
        },
        target,
        &Options::parse(opts.iter().copied()).unwrap(),
        &mut out,
        &ctl(cancel, limits),
    );
    (r, out.bytes)
}

#[test]
fn the_registry_lists_dedups_and_refuses_duplicates() {
    let mut reg = ExportRegistry::new();
    reg.register(Arc::new(Upper)).unwrap();
    reg.register(Arc::new(AsIs)).unwrap();
    assert_eq!(
        reg.register(Arc::new(Upper)),
        Err(RegistryError::DuplicateId("test-upper@1".into()))
    );
    let md = reg.targets_for(&SourceKind::Pack("markdown".into()));
    let ids: Vec<(&str, &str)> = md.iter().map(|(e, t)| (*e, t.id)).collect();
    assert_eq!(
        ids,
        vec![
            ("test-upper@1", "upper"),
            ("test-upper@1", "panic"),
            ("test-upper@1", "stall"),
            ("test-upper@1", "as-is"),
        ],
        "each target once, the first exporter's"
    );
    // a tier-0 PNG: only the identity exporter
    let png = reg.targets_for(&SourceKind::Format("png".into()));
    assert_eq!(png.len(), 1);
    assert_eq!(png[0].1, AS_IS);
    assert_eq!(png[0].1.fidelity, Fidelity::Lossless);
}

#[test]
fn an_export_runs_with_checked_options_and_reports() {
    let mut reg = ExportRegistry::with_as_is();
    reg.register(Arc::new(Upper)).unwrap();
    let no = AtomicBool::new(false);
    let lim = ExportLimits::default();
    let (r, out) = run(&reg, "markdown", "upper", &[], &no, lim);
    let r = r.unwrap();
    assert_eq!(out, b"HELLO\n");
    assert_eq!(r.bytes, 6, "the bytes that reached the sink");
    assert_eq!(
        r.losses,
        vec![Loss::new("case-folded", "lower case is upper case")]
    );
    let (_, out) = run(
        &reg,
        "markdown",
        "upper",
        &["bang=true", "size=4"],
        &no,
        lim,
    );
    assert_eq!(out, b"HELLO\n!");
    // options are checked before any work: nothing is written
    for (bad, why) in [
        (&["nope=1"][..], "unknown name"),
        (&["bang=yes"][..], "a Bool's value"),
        (&["size=5"][..], "an Int past max"),
        (&["size=x"][..], "not a number"),
    ] {
        let (r, out) = run(&reg, "markdown", "upper", bad, &no, lim);
        match r {
            Err(ExportError::Declined { code, .. }) => assert_eq!(code, "bad_option", "{why}"),
            other => panic!("{why}: {other:?}"),
        }
        assert!(out.is_empty(), "{why}: nothing written");
    }
    assert!(Options::parse(["novalue"]).is_err());
    assert!(Options::parse(["=x"]).is_err());
    // a target nobody writes for this source
    let (r, _) = run(&reg, "text", "upper", &[], &no, lim);
    let e = r.unwrap_err();
    assert!(matches!(e, ExportError::Unsupported { .. }), "{e:?}");
    assert_eq!(e.report().id(), "MPC-E001");
    assert_eq!(e.exit(), 2);
}

#[test]
fn cancel_limits_and_panics_are_refusals_with_nothing_placed() {
    let mut reg = ExportRegistry::with_as_is();
    reg.register(Arc::new(Upper)).unwrap();
    let no = AtomicBool::new(false);
    let lim = ExportLimits::default();
    // a cancel set before the call: nothing runs
    let yes = AtomicBool::new(true);
    let (r, out) = run(&reg, "markdown", "upper", &[], &yes, lim);
    assert!(matches!(r, Err(ExportError::Cancelled)));
    assert!(out.is_empty());
    // a cancel while it runs: the exporter's check stops it
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        f2.store(true, Ordering::SeqCst);
    });
    let (r, _) = run(&reg, "markdown", "stall", &[], &flag, lim);
    t.join().unwrap();
    assert!(matches!(r, Err(ExportError::Cancelled)), "{r:?}");
    // the output limit, at the sink
    let mut small = ExportLimits::default();
    small.max_output_bytes = 3;
    let (r, out) = run(&reg, "markdown", "upper", &[], &no, small);
    let e = r.unwrap_err();
    assert!(matches!(e, ExportError::Limit("output bytes")), "{e:?}");
    assert_eq!(e.report().id(), "MPC-E003");
    assert!(out.len() <= 3, "never past the limit");
    // the input limit, in the identity exporter
    let mut tiny = ExportLimits::default();
    tiny.max_input_bytes = 2;
    let (r, _) = run(&reg, "text", "as-is", &[], &no, tiny);
    assert!(matches!(r, Err(ExportError::Limit("input bytes"))), "{r:?}");
    // a panic is caught: Internal, with the exporter's id
    let (r, _) = run(&reg, "markdown", "panic", &[], &no, lim);
    match r {
        Err(ExportError::Internal { exporter, message }) => {
            assert_eq!(exporter, "test-upper@1");
            assert_eq!(message, "planted");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_identity_exporter_passes_bytes_and_one_root_through() {
    let reg = ExportRegistry::with_as_is();
    let no = AtomicBool::new(false);
    let lim = ExportLimits::default();
    // Text with one root: its text
    let (r, out) = run(&reg, "text", "as-is", &[], &no, lim);
    r.unwrap();
    assert_eq!(out, b"hello\n");
    // Text with two roots: declined, not guessed
    let roots = vec![
        ("a".to_owned(), "x".to_owned()),
        ("b".to_owned(), "y".to_owned()),
    ];
    let mut sink = VecSink::default();
    let e = reg
        .export(
            &SourceKind::Pack("two".into()),
            Source::Text {
                pack: "two",
                roots: &roots,
            },
            "as-is",
            &Options::new(),
            &mut sink,
            &ctl(&no, lim),
        )
        .unwrap_err();
    assert!(
        matches!(
            e,
            ExportError::Declined {
                code: "not_one_file",
                ..
            }
        ),
        "{e:?}"
    );
    // Bytes: unchanged, whatever they hold, with progress up to the size
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let seen = Mutex::new(Vec::new());
    let progress = |d: u64, t: u64| seen.lock().unwrap().push((d, t));
    let mut read = &bytes[..];
    let mut sink = VecSink::default();
    let c = ExportCtl {
        cancel: &no,
        progress: &progress,
        limits: lim,
        resources: &NoResources,
    };
    reg.export(
        &SourceKind::Format("bin".into()),
        Source::Bytes {
            read: &mut read,
            size: bytes.len() as u64,
            name: "x.bin",
        },
        "as-is",
        &Options::new(),
        &mut sink,
        &c,
    )
    .unwrap();
    assert_eq!(sink.bytes, bytes);
    let p = seen.lock().unwrap();
    assert_eq!(p.last(), Some(&(bytes.len() as u64, bytes.len() as u64)));
    assert!(p.windows(2).all(|w| w[0].0 < w[1].0), "progress only grows");
}

/// Every export error maps to its stable id and exit (X-ERR-1).
#[test]
fn export_errors_have_stable_ids() {
    for (e, id, exit) in [
        (
            ExportError::Unsupported {
                kind: "k".into(),
                target: "t".into(),
            },
            "MPC-E001",
            2,
        ),
        (
            ExportError::Declined {
                code: "bad_option",
                message: "m".into(),
            },
            "MPC-E002",
            2,
        ),
        (ExportError::Limit("pixels"), "MPC-E003", 2),
        (ExportError::Cancelled, "MPC-E004", 1),
        (ExportError::Decode("d".into()), "MPC-E005", 1),
        (ExportError::Io(std::io::Error::other("x")), "MPC-E006", 1),
        (
            ExportError::Internal {
                exporter: "x".into(),
                message: "y".into(),
            },
            "MPC-E007",
            1,
        ),
    ] {
        let r = e.report();
        assert_eq!((r.id(), r.exit(), e.exit()), (id, exit, exit), "{e:?}");
        assert_eq!(r.plain(), format!("{e} ({id})"));
    }
}

// ---------------------------------------------------------------- streaming copies

/// A tier-0 file's version copied to a writer: the bytes, verified; a live doc's too.
#[test]
fn file_at_to_copies_tier0_and_live_versions() {
    let dir = TmpDir(std::env::temp_dir().join(format!("k2mb-export-t0-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store, host.clone(), Config::default()).with_blob_store(blobs);
    let key = DocKey {
        space: "ws".into(),
        path: "pic.psb".into(),
    };
    let first: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let s = mgr
        .open_blob(Arc::new(AnyFile), &key, Some(&mut &first[..]))
        .unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let v1 = s.status().unwrap().version;
    let w = s.write_bytes(c, b"second", v1).unwrap();
    let mut out = Vec::new();
    let copy = s.file_at_to(c, v1, &mut out).unwrap();
    assert_eq!(
        (out.as_slice(), copy.bytes),
        (first.as_slice(), first.len() as u64)
    );
    assert_eq!((copy.version, copy.codec), (v1, FileCodec::Exact));
    let mut out = Vec::new();
    let copy = s.file_now_to(c, &mut out).unwrap();
    assert_eq!((out.as_slice(), copy.version), (&b"second"[..], w.version));
    // history for a past version, as read_version
    host.rights
        .lock()
        .unwrap()
        .insert("r".into(), Rights::read_only());
    let r = s
        .join(
            Participant::new("r", "R", "#2", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    assert!(matches!(
        s.file_at_to(r, v1, &mut Vec::new()),
        Err(SessionError::Forbidden {
            need: Need::History
        })
    ));
    assert!(s.file_now_to(r, &mut Vec::new()).is_ok());

    // a live doc: the same bytes as file_at
    let f = fx("live-to", MARKDOWN, "a.md", b"\xEF\xBB\xBFone\r\n");
    let vid = f.s.name_version(f.c, "v", None).unwrap();
    let mut out = Vec::new();
    let copy = f.s.file_at_to(f.c, vid, &mut out).unwrap();
    let at = f.s.file_at(f.c, vid).unwrap();
    assert_eq!(
        (out, copy.version, copy.bytes),
        (at.bytes.clone(), at.version, at.bytes.len() as u64)
    );
}

/// A blob whose bytes don't match its MAC: the copy fails at the end, after the bytes were
/// written; only `Ok` means the copy is good (the caller discards what it got).
#[test]
fn a_damaged_blob_fails_the_copy_at_its_end() {
    let dir = TmpDir(std::env::temp_dir().join(format!("k2mb-export-bad-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
    });
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.0.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr =
        SessionManager::new(store, host.clone(), Config::default()).with_blob_store(blobs.clone());
    let key = DocKey {
        space: "ws".into(),
        path: "big.tif".into(),
    };
    let bytes: Vec<u8> = (0..700_000u32).map(|i| (i % 253) as u8).collect();
    let s = mgr
        .open_blob(Arc::new(AnyFile), &key, Some(&mut &bytes[..]))
        .unwrap();
    let c = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    let v = s.status().unwrap().version;
    // flip the stored blob's last byte (same length: only the MAC catches it)
    let id = s.versions(c).unwrap()[0].1.blob;
    let file = walk(&dir.0.join("blobs"))
        .into_iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(id.to_hex().as_str()))
        .expect("the blob's file");
    let mut b = std::fs::read(&file).unwrap();
    let last = b.len() - 1;
    b[last] ^= 1;
    std::fs::write(&file, &b).unwrap();

    let mut out = Vec::new();
    let e = s.file_at_to(c, v, &mut out).unwrap_err();
    assert!(
        matches!(e, SessionError::Blob(BlobError::Damaged(_))),
        "{e:?}"
    );
    assert_eq!(e.report().exit(), 1);
    assert_eq!(
        out.len(),
        bytes.len(),
        "every byte reached the writer before the end's check failed: discard them"
    );
    assert!(
        s.file_now_to(c, &mut Vec::new()).is_err(),
        "the current file too"
    );
}

fn walk(d: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
