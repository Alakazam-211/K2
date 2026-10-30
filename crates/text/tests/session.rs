//! The pack as a host uses it: real files in a temp folder, `SessionManager::open_file`
//! with the pack the file name picks, agents editing through the kit's verbs, write-back,
//! outside edits and refusals. Fidelity is checked on the file on disk.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, DiskCheck, DiskState, Host, Pack, Participant, ParticipantKind,
    Rights, Session, SessionError, SessionManager, Stay, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_text::{Family, TextPack, ROOT};

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
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
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mt-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c);
    Fx {
        dir,
        ws,
        host,
        store,
        mgr,
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.ws.join(name)
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let p = self.path(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(name)).unwrap()
    }
    fn try_open(&self, name: &str) -> Result<Arc<Session>, SessionError> {
        let pack: Arc<dyn Pack> = Arc::new(TextPack::for_path(name).expect("a text name"));
        self.mgr
            .open_file(pack, &key(name), &self.path(name), &self.ws)
    }
    fn open(&self, name: &str) -> Arc<Session> {
        self.try_open(name)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }
    fn join(&self, s: &Session, pid: &str) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full());
        s.join(
            Participant::new(pid, pid, "#123456", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
}

fn verb(s: &Session, c: ConnId, argv: &[&str], stdin: &[u8]) -> k2_mpc_base::tools::ToolOutput {
    let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
    s.run_tool(c, ROOT, &argv, stdin)
        .unwrap_or_else(|e| panic!("{argv:?}: {e}"))
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out)
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut v = Vec::new();
    walk(&root, &root, &mut v);
    v.sort();
    assert!(v.len() >= 53, "corpus went missing");
    v
}

fn a_word(s: &str) -> Option<String> {
    s.split(|c: char| !c.is_ascii_alphabetic())
        .find(|w| w.len() >= 3 && w.bytes().any(|b| b.is_ascii_lowercase()))
        .map(str::to_owned)
}

fn mtime(p: &Path) -> std::time::SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

// ---------- tests ----------

/// Every accepted corpus file, live through a session: no-op verbs, a full read-and-write
/// back, and a one-word edit; the file on disk is checked after each write-back.
#[test]
fn every_corpus_file_is_byte_exact_through_a_session() {
    let f = fx("corpus");
    let mut n = 0;
    for (rel, orig) in corpus()
        .into_iter()
        .filter(|(r, _)| !r.starts_with("refused/"))
    {
        f.write(&rel, &orig);
        let before = mtime(&f.path(&rel));
        let s = f.open(&rel);
        let c = f.join(&s, "agent");
        let v0 = s.status().unwrap().version;
        // `cat` (a byte tool) gives the file's exact bytes
        assert_eq!(verb(&s, c, &["cat"], b"").stdout, orig, "{rel}: cat");
        let text = s.read(c).unwrap().text_by_root.remove(0).1;
        // no-op edits: no change, no version, nothing written
        if let Some(w) = a_word(&text) {
            let out = verb(&s, c, &["replace", "--old", &w, "--new", &w, "--all"], b"");
            assert_eq!(
                (out.exit, out.changed.is_none()),
                (0, true),
                "{rel}: no-op replace"
            );
            let out = verb(&s, c, &["sed", "-i", &format!("s/{w}/{w}/")], b"");
            assert_eq!(out.exit, 0, "{rel}: no-op sed -i");
        }
        // an agent rewrites the file from the bytes it read (BOM removed: edit verbs never
        // see it; the line endings are normalised in by the pack)
        let read = s.read(c).unwrap();
        let body = String::from_utf8(read.bytes.clone()).unwrap();
        let body = body.strip_prefix('\u{FEFF}').unwrap_or(&body);
        match s.write_base(c, ROOT, read.version, body).unwrap() {
            WriteOutcome::Applied(_) => {}
            other => panic!("{rel}: write --base: {other:?}"),
        }
        s.replace_text(c, ROOT, &text).unwrap();
        assert_eq!(
            s.status().unwrap().version,
            v0,
            "{rel}: a no-op made a version"
        );
        let out = s.flush(c).unwrap();
        assert!(!out.written, "{rel}: an untouched file was written");
        assert_eq!(f.bytes(&rel), orig, "{rel}");
        assert_eq!(mtime(&f.path(&rel)), before, "{rel}: mtime changed");
        // a one-word edit changes exactly that word on disk
        if let Some(w) = a_word(&text) {
            let up = w.to_uppercase();
            let out = verb(&s, c, &["replace", "--old", &w, "--new", &up, "--all"], b"");
            assert_eq!(
                out.exit,
                0,
                "{rel}: {}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(s.flush(c).unwrap().written, "{rel}");
            let expected = String::from_utf8(orig.clone())
                .unwrap()
                .replace(&w, &up)
                .into_bytes();
            assert_eq!(f.bytes(&rel), expected, "{rel}: the edit touched more");
            n += 1;
        }
        s.leave(c).unwrap();
    }
    assert!(n >= 43, "only {n} files were edited");
}

#[test]
fn edits_land_in_the_files_own_line_ending_style() {
    let f = fx("styles");
    for (name, orig, want) in [
        ("mac.txt", &b"one\rtwo\r"[..], &b"one\rinserted\rtwo\r"[..]),
        (
            "win.ini",
            b"\xEF\xBB\xBFa=1\r\nb=2\r\n",
            b"\xEF\xBB\xBFa=1\r\ninserted\r\nb=2\r\n",
        ),
        ("unix.yml", b"a: 1\nb: 2\n", b"a: 1\ninserted\nb: 2\n"),
    ] {
        f.write(name, orig);
        let s = f.open(name);
        let c = f.join(&s, "agent");
        // the body's own line endings don't matter: a CRLF body into a CR file is still CR
        let out = verb(&s, c, &["insert", "--line", "2"], b"inserted\r\n");
        assert_eq!(out.exit, 0, "{name}");
        assert!(s.flush(c).unwrap().written);
        assert_eq!(f.bytes(name), want, "{name}");
    }
    // a file without line breaks takes its type's style: CRLF for .bat, LF for .sh
    for (name, want) in [
        ("run.bat", &b"@echo off\r\nexit\r\n"[..]),
        ("run.sh", b"@echo off\nexit\n"),
    ] {
        f.write(name, b"@echo off");
        let s = f.open(name);
        let c = f.join(&s, "agent");
        s.replace_text(c, ROOT, "@echo off\nexit\n").unwrap();
        assert!(s.flush(c).unwrap().written);
        assert_eq!(f.bytes(name), want, "{name}");
    }
}

#[test]
fn an_outside_edit_merges_and_keeps_the_files_style() {
    let f = fx("outside");
    let name = "query.sql";
    f.write(name, b"SELECT 1;\r\nSELECT 2;\r\n");
    let s = f.open(name);
    let c = f.join(&s, "agent");
    verb(
        &s,
        c,
        &["replace", "--old", "SELECT 2", "--new", "SELECT 22"],
        b"",
    );
    // another program appends a line before the write-back
    f.write(name, b"SELECT 1;\r\nSELECT 2;\r\nSELECT 3;\r\n");
    assert!(matches!(
        s.file_changed().unwrap(),
        DiskCheck::Ingested { .. }
    ));
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes(name), b"SELECT 1;\r\nSELECT 22;\r\nSELECT 3;\r\n");
    // the same text in other bytes (CRLF -> LF): adopted, not an edit
    f.write(name, b"SELECT 1;\nSELECT 22;\nSELECT 3;\n");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    verb(&s, c, &["append"], b"SELECT 4;\n");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(
        f.bytes(name),
        b"SELECT 1;\nSELECT 22;\nSELECT 3;\nSELECT 4;\n"
    );
}

#[test]
fn a_file_that_turns_mixed_or_binary_is_declined_and_never_written() {
    let f = fx("declined");
    let name = "notes.txt";
    f.write(name, b"a\r\nb\r\n");
    let s = f.open(name);
    let c = f.join(&s, "agent");
    for (bad, code) in [
        (&b"a\r\nb\nc\r\n"[..], "mixed_line_endings"),
        (b"a\rb\r\n", "mixed_line_endings"),
        (b"\xFF\xFEa\0", "not_utf8"),
        (b"caf\xE9\r\n", "not_utf8"),
        (b"a\0b", "binary"),
    ] {
        f.write(name, bad);
        match s.file_changed().unwrap() {
            DiskCheck::Declined(d) => assert_eq!(d.code, code, "{}", d.message),
            other => panic!("{code}: {other:?}"),
        }
        assert!(matches!(
            s.status().unwrap().disk,
            Some(DiskState::Declined(_))
        ));
        assert!(matches!(
            s.replace_text(c, ROOT, "x\n"),
            Err(SessionError::Declined(_))
        ));
        assert!(!s.flush(c).unwrap().written);
        assert_eq!(f.bytes(name), bad, "never overwritten while declined");
    }
    assert!(f.host.events.lock().unwrap().iter().any(|e| matches!(
        e,
        AuditEvent::FileDeclined {
            code: "mixed_line_endings",
            ..
        }
    )));
    // fixed (one style again, the synced text in CR bytes): live again, the CR format is
    // adopted without an edit, and new lines are written in it
    f.write(name, b"a\rb\r");
    assert_eq!(s.file_changed().unwrap(), DiskCheck::Unchanged);
    assert_eq!(s.status().unwrap().declined, None);
    verb(&s, c, &["append"], b"c\n");
    assert!(s.flush(c).unwrap().written);
    assert_eq!(f.bytes(name), b"a\rb\rc\r");
}

#[test]
fn refused_files_do_not_open_and_are_not_touched() {
    let f = fx("refused");
    let mut n = 0;
    for (rel, orig) in corpus()
        .into_iter()
        .filter(|(r, _)| r.starts_with("refused/"))
    {
        let code = rel.rsplit('.').nth(1).unwrap().to_owned();
        f.write(&rel, &orig);
        let before = mtime(&f.path(&rel));
        match f.try_open(&rel) {
            Err(SessionError::Declined(d)) => assert_eq!(d.code, code, "{rel}"),
            Err(e) => panic!("{rel}: {e}"),
            Ok(_) => panic!("{rel}: opened"),
        }
        assert_eq!(f.store.find(&key(&rel)).unwrap(), None, "{rel}: stored");
        assert_eq!(f.bytes(&rel), orig);
        assert_eq!(mtime(&f.path(&rel)), before);
        n += 1;
    }
    assert!(n >= 6);
    // too large for its type: refused before anything is stored
    f.write("big.log", &vec![b'x'; (1 << 20) + 1]);
    match f.try_open("big.log") {
        Err(SessionError::Declined(d)) => assert_eq!(d.code, "too_large"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn a_rename_to_another_text_type_keeps_the_doc() {
    let f = fx("rename");
    f.write("notes.txt", b"k = 1\n");
    let s = f.open("notes.txt");
    let c = f.join(&s, "agent");
    verb(&s, c, &["append"], b"v = 2\n");
    assert!(s.flush(c).unwrap().written);
    let id = f.store.find(&key("notes.txt")).unwrap().expect("stored");
    fs::rename(f.path("notes.txt"), f.path("notes.toml")).unwrap();
    f.mgr.rekey(&key("notes.txt"), key("notes.toml")).unwrap();
    s.file_moved(&f.path("notes.toml")).unwrap();
    s.leave(c).unwrap();
    drop(s);
    f.mgr.release().unwrap();
    // reopened under the new name with the TOML type: same schema, same doc and history
    let mgr = SessionManager::new(f.store.clone(), f.host.clone(), f.mgr.config().clone());
    let pack: Arc<dyn Pack> = Arc::new(TextPack::for_path("notes.toml").unwrap());
    let s = mgr
        .open_file(pack, &key("notes.toml"), &f.path("notes.toml"), &f.ws)
        .unwrap();
    let c = f.join(&s, "agent");
    assert_eq!(verb(&s, c, &["cat"], b"").stdout, b"k = 1\nv = 2\n");
    assert_eq!(
        f.store.find(&key("notes.toml")).unwrap(),
        Some(id),
        "the same doc"
    );
    assert_eq!(f.store.find(&key("notes.txt")).unwrap(), None);
}

// ---------- csv-as-text: tables edited as text ----------

/// An Excel "CSV UTF-8" export: a BOM and CRLF rows.
const LEDGER: &[u8] = b"\xEF\xBB\xBFid,item,qty,owner\r\n\
1,bolts,10,ana\r\n\
2,nuts,20,ben\r\n\
3,washers,30,cy\r\n\
4,screws,40,dee\r\n\
5,rivets,50,eve\r\n";

/// What an agent sends to `write --base`: the bytes it read, without the BOM.
fn body_of(bytes: &[u8]) -> String {
    let s = String::from_utf8(bytes.to_vec()).unwrap();
    s.strip_prefix('\u{FEFF}').unwrap_or(&s).to_owned()
}

fn edit(body: &str, old: &str, new: &str) -> String {
    assert_eq!(body.matches(old).count(), 1, "{old:?}");
    body.replace(old, new)
}

/// The demo claim. Two agents read the same version of a CSV, each changes a different row,
/// and both send `write --base V` at the same moment (two threads). Both writes are applied:
/// the second is three-way merged onto the first, never last-writer-wins. The file on disk
/// has both edits, its BOM and its CRLF rows, and nothing else changed.
#[test]
fn two_agents_editing_different_rows_concurrently_both_land() {
    let f = fx("csv-rows");
    let name = "ledger.csv";
    f.write(name, LEDGER);
    assert_eq!(
        TextPack::for_path(name).unwrap().family(),
        Family::CsvAsText
    );
    let s = f.open(name);
    let (a, b) = (f.join(&s, "agent-a"), f.join(&s, "agent-b"));
    let (ra, rb) = (s.read(a).unwrap(), s.read(b).unwrap());
    assert_eq!(ra.version, rb.version, "both start from one version");
    assert_eq!(ra.bytes, LEDGER, "read gives the file's bytes");
    let ours_a = edit(&body_of(&ra.bytes), "2,nuts,20,ben", "2,nuts,25,ben");
    let ours_b = edit(
        &body_of(&rb.bytes),
        "5,rivets,50,eve",
        "5,rivets,50,eve-marie",
    );
    let go = Barrier::new(2);
    let (oa, ob) = std::thread::scope(|t| {
        let ta = t.spawn(|| {
            go.wait();
            s.write_base(a, ROOT, ra.version, &ours_a).unwrap()
        });
        let tb = t.spawn(|| {
            go.wait();
            s.write_base(b, ROOT, rb.version, &ours_b).unwrap()
        });
        (ta.join().unwrap(), tb.join().unwrap())
    });
    assert!(matches!(oa, WriteOutcome::Applied(_)), "agent a: {oa:?}");
    assert!(matches!(ob, WriteOutcome::Applied(_)), "agent b: {ob:?}");
    assert!(s.flush(a).unwrap().written);
    let want = String::from_utf8(LEDGER.to_vec())
        .unwrap()
        .replace("2,nuts,20,ben", "2,nuts,25,ben")
        .replace("5,rivets,50,eve", "5,rivets,50,eve-marie");
    assert_eq!(
        f.bytes(name),
        want.as_bytes(),
        "both edits, BOM and CRLF kept"
    );
}

/// Different rows merge, adjacent rows and two appended rows too, in every line-ending
/// style. Since kit v0.9.7.1 `write --base` merges by whole lines, and in CSV-as-text a row
/// is one line: two agents editing different cells of the same row conflict, and the second
/// re-reads and retries (below).
#[test]
fn concurrent_row_and_cell_edits_merge_in_every_style() {
    let f = fx("csv-cells");
    let base = "id\tname\tqty\n1\tana\t10\n2\tben\t20\n3\tcy\t30\n";
    // (what a changes, what b changes, the merged text; appends land a's first)
    type Change<'a> = (&'a str, &'a str);
    let cases: &[(&str, Change, Change, &str)] = &[
        (
            "adjacent rows",
            ("1\tana\t10", "1\tana\t11"),
            ("2\tben\t20", "2\tben\t22"),
            "id\tname\tqty\n1\tana\t11\n2\tben\t22\n3\tcy\t30\n",
        ),
        (
            "cells in different rows",
            ("1\tana\t", "1\tanabel\t"),
            ("\t30\n", "\t33\n"),
            "id\tname\tqty\n1\tanabel\t10\n2\tben\t20\n3\tcy\t33\n",
        ),
        (
            "two appended rows",
            ("3\tcy\t30\n", "3\tcy\t30\n4\tdee\t40\n"),
            ("3\tcy\t30\n", "3\tcy\t30\n5\teve\t50\n"),
            "id\tname\tqty\n1\tana\t10\n2\tben\t20\n3\tcy\t30\n4\tdee\t40\n5\teve\t50\n",
        ),
    ];
    for (i, (what, ea, eb, merged)) in cases.iter().enumerate() {
        for (eol, style) in [("\n", "lf"), ("\r\n", "crlf"), ("\r", "cr")] {
            let name = format!("t{i}-{style}.tsv");
            let in_style = |t: &str| t.replace('\n', eol);
            f.write(&name, in_style(base).as_bytes());
            let s = f.open(&name);
            let (a, b) = (f.join(&s, "agent-a"), f.join(&s, "agent-b"));
            let v = s.read(a).unwrap().version;
            for (c, (old, new)) in [(a, ea), (b, eb)] {
                let out = s
                    .write_base(c, ROOT, v, &in_style(&edit(base, old, new)))
                    .unwrap();
                assert!(
                    matches!(out, WriteOutcome::Applied(_)),
                    "{what} ({eol:?}): {out:?}"
                );
            }
            assert!(s.flush(a).unwrap().written);
            assert_eq!(
                f.bytes(&name),
                in_style(merged).as_bytes(),
                "{what} ({eol:?})"
            );
        }
    }
}

/// Two cells of one row (kit v0.9.7.1): the row is one line, so the second agent's
/// `write --base` from the same base conflicts and applies nothing; it re-reads the current
/// version, makes its change again, and that write lands, with both cells changed. In every
/// line-ending style.
#[test]
fn two_cells_of_one_row_conflict_and_the_retry_lands() {
    let f = fx("csv-row");
    let base = "id\tname\tqty\n1\tana\t10\n2\tben\t20\n3\tcy\t30\n";
    let merged = "id\tname\tqty\n1\tana\t10\n2\tbenedict\t21\n3\tcy\t30\n";
    for (eol, style) in [("\n", "lf"), ("\r\n", "crlf"), ("\r", "cr")] {
        let name = format!("row-{style}.tsv");
        let in_style = |t: &str| t.replace('\n', eol);
        f.write(&name, in_style(base).as_bytes());
        let s = f.open(&name);
        let (a, b) = (f.join(&s, "agent-a"), f.join(&s, "agent-b"));
        let v = s.read(a).unwrap().version;
        let out = s
            .write_base(
                a,
                ROOT,
                v,
                &in_style(&edit(base, "2\tben\t", "2\tbenedict\t")),
            )
            .unwrap();
        assert!(
            matches!(out, WriteOutcome::Applied(_)),
            "a ({eol:?}): {out:?}"
        );
        // b, from the same base, changes another cell of the same row: refused
        let b_text = in_style(&edit(base, "\t20\n", "\t21\n"));
        match s.write_base(b, ROOT, v, &b_text).unwrap() {
            WriteOutcome::Conflicts(c) => {
                assert_eq!(c.len(), 1, "{c:?}");
                assert_eq!(c[0].lines, (3, 3), "{c:?}");
            }
            other => panic!("two cells of one row merged ({eol:?}): {other:?}"),
        }
        // b re-reads the current version and writes its change again: it lands
        let now = s.read(b).unwrap();
        let current = body_of(&now.bytes);
        let retry = edit(
            &current,
            &in_style("benedict\t20\n"),
            &in_style("benedict\t21\n"),
        );
        let out = s.write_base(b, ROOT, now.version, &retry).unwrap();
        assert!(
            matches!(out, WriteOutcome::Applied(_)),
            "b's retry ({eol:?}): {out:?}"
        );
        assert!(s.flush(a).unwrap().written);
        assert_eq!(f.bytes(&name), in_style(merged).as_bytes(), "{eol:?}");
    }
}

/// What text mode does not do: a row has no identity. Moving a row (a cut and a paste, or
/// a sort) is a delete and an insert, so an edit another agent made to that row from the
/// same base does not follow it: it conflicts, nothing of it is applied, and the agent
/// re-reads and writes again. A grid pack (rows and columns with identity) is the fix.
#[test]
fn a_moved_row_is_text_and_a_concurrent_edit_to_it_conflicts() {
    let f = fx("csv-move");
    let name = "ledger.csv";
    f.write(name, LEDGER);
    let s = f.open(name);
    let (a, b) = (f.join(&s, "agent-a"), f.join(&s, "agent-b"));
    let r = s.read(a).unwrap();
    let body = body_of(&r.bytes);
    // a moves row 5 to the top; b, from the same base, edits row 5
    let moved =
        edit(&body, "5,rivets,50,eve\r\n", "").replacen("1,bolts", "5,rivets,50,eve\r\n1,bolts", 1);
    let edited = edit(&body, "5,rivets,50,eve", "5,rivets,55,eve");
    assert!(matches!(
        s.write_base(a, ROOT, r.version, &moved).unwrap(),
        WriteOutcome::Applied(_)
    ));
    match s.write_base(b, ROOT, r.version, &edited).unwrap() {
        WriteOutcome::Conflicts(c) => assert!(!c.is_empty()),
        other => panic!("an edit to a moved row merged: {other:?}"),
    }
    assert!(s.flush(a).unwrap().written);
    let want = String::from_utf8(LEDGER.to_vec()).unwrap();
    let want = want.replace("5,rivets,50,eve\r\n", "").replacen(
        "1,bolts",
        "5,rivets,50,eve\r\n1,bolts",
        1,
    );
    assert_eq!(f.bytes(name), want.as_bytes(), "only the move landed");
    // b re-reads and writes again: now it lands on the moved row
    let r = s.read(b).unwrap();
    let again = edit(&body_of(&r.bytes), "5,rivets,50,eve", "5,rivets,55,eve");
    assert!(matches!(
        s.write_base(b, ROOT, r.version, &again).unwrap(),
        WriteOutcome::Applied(_)
    ));
    assert!(s.flush(b).unwrap().written);
    assert!(f
        .bytes(name)
        .starts_with(b"\xEF\xBB\xBFid,item,qty,owner\r\n5,rivets,55,eve\r\n"));
}

/// What text mode does not do, continued: a column has no identity either. Swapping two
/// columns rewrites every row, so a cell edit made from the same base in any rewritten row
/// conflicts; the merge can't carry it across to the column's new place.
#[test]
fn a_column_move_rewrites_every_row_and_a_concurrent_cell_edit_conflicts() {
    let f = fx("csv-cols");
    let name = "ledger.csv";
    f.write(name, LEDGER);
    let s = f.open(name);
    let (a, b) = (f.join(&s, "agent-a"), f.join(&s, "agent-b"));
    let r = s.read(a).unwrap();
    let body = body_of(&r.bytes);
    // a swaps the qty and owner columns
    let swapped: String = body
        .split_inclusive("\r\n")
        .map(|row| {
            let cells: Vec<&str> = row.trim_end_matches("\r\n").split(',').collect();
            format!("{},{},{},{}\r\n", cells[0], cells[1], cells[3], cells[2])
        })
        .collect();
    assert!(matches!(
        s.write_base(a, ROOT, r.version, &swapped).unwrap(),
        WriteOutcome::Applied(_)
    ));
    // b, from the same base, edits a qty cell, then an owner cell, in another row each
    for (old, new) in [
        ("3,washers,30,cy", "3,washers,35,cy"),
        ("4,screws,40,dee", "4,screws,40,dora"),
    ] {
        match s
            .write_base(b, ROOT, r.version, &edit(&body, old, new))
            .unwrap()
        {
            WriteOutcome::Conflicts(c) => assert!(!c.is_empty()),
            other => panic!("a cell edit merged across a column move ({new}): {other:?}"),
        }
    }
    assert!(s.flush(a).unwrap().written);
    let mut want = b"\xEF\xBB\xBF".to_vec();
    want.extend_from_slice(swapped.as_bytes());
    assert_eq!(f.bytes(name), want, "only the swap landed");
}
