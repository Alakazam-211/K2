//! `quote-lite`: a small CSV-shaped table pack for the line tests. Quoted fields (so a value
//! can hold LF, CRLF or a lone CR) and a record terminator of LF, CRLF or a lone CR, kept in
//! the doc's codec settings. Record terminators that disagree decline `mixed_line_endings`,
//! as a real delimited-file codec does. The first record is the header.
//!
//! It has no line map of its own: [`Pack::line_map`]'s default ([`Table::line_map`]) counts a
//! record as one line plus one per line break in its values, whatever ends the record.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::{Doc, Neutral};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, Host, Pack, PackContent, PackError, Participant,
    ParticipantKind, Rights, Session, SessionManager, Stay, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::table::{self, Field, Record, Shape, Table, ROWS};
use k2_mpc_base::tools::PackVerb;

pub struct QuoteLite;

pub const QUOTE: Schema = Schema {
    name: "quote-lite",
    roots: table::ROOTS,
};

const LF: u8 = 0;
const CRLF: u8 = 1;
const CR: u8 = 2;

fn declined(code: &'static str, message: &str) -> Declined {
    Declined {
        code,
        message: message.into(),
    }
}

/// The records, the record terminator, and whether the last record has one.
fn parse(s: &str) -> Result<(Vec<Record>, u8, bool), Declined> {
    let b = s.as_bytes();
    let mut records = Vec::new();
    let mut eol: Option<u8> = None;
    let mut fin = false;
    let mut i = 0;
    while i < b.len() {
        let mut rec: Record = Vec::new();
        let mut plain_empty;
        loop {
            let mut v = String::new();
            let quoted = b.get(i) == Some(&b'"');
            if quoted {
                i += 1;
                loop {
                    let close = s[i..]
                        .find('"')
                        .map(|k| i + k)
                        .ok_or_else(|| declined("malformed", "an unclosed quote"))?;
                    v.push_str(&s[i..close]);
                    i = close + 1;
                    if b.get(i) == Some(&b'"') {
                        v.push('"');
                        i += 1;
                    } else {
                        break;
                    }
                }
                if !matches!(b.get(i), None | Some(b',' | b'\r' | b'\n')) {
                    return Err(declined("malformed", "text after a closing quote"));
                }
            } else {
                let start = i;
                while !matches!(b.get(i), None | Some(b',' | b'\r' | b'\n')) {
                    i += 1;
                }
                v.push_str(&s[start..i]);
            }
            plain_empty = !quoted && v.is_empty();
            rec.push(Field::new(v.as_str()));
            if b.get(i) == Some(&b',') {
                i += 1;
                continue;
            }
            break;
        }
        if rec.len() == 1 && plain_empty {
            rec.clear(); // a blank line
        }
        records.push(rec);
        fin = false;
        let term = match (b.get(i), b.get(i + 1)) {
            (Some(b'\r'), Some(b'\n')) => Some((CRLF, 2)),
            (Some(b'\r'), _) => Some((CR, 1)),
            (Some(b'\n'), _) => Some((LF, 1)),
            _ => None,
        };
        if let Some((t, len)) = term {
            if *eol.get_or_insert(t) != t {
                return Err(declined(
                    "mixed_line_endings",
                    "the records' line endings differ",
                ));
            }
            i += len;
            fin = true;
        }
    }
    Ok((records, eol.unwrap_or(LF), fin))
}

fn field_text(v: &str) -> String {
    if v.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_owned()
    }
}

impl Pack for QuoteLite {
    fn schema(&self) -> Schema {
        QUOTE
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|_| declined("not_utf8", "not UTF-8"))?;
        let (recs, eol, fin) = parse(s)?;
        let (_, edits) = Table::import(&recs, true, Shape::detect(&recs))
            .map_err(|e| declined("malformed", &e.to_string()))?;
        let mut d = Doc::new(QUOTE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![eol, u8::from(fin)],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        let term = match codec.first() {
            Some(&CRLF) => "\r\n",
            Some(&CR) => "\r",
            _ => "\n",
        };
        let fin = codec.get(1) != Some(&0);
        let recs = t.render();
        let mut out = String::new();
        for (i, rec) in recs.iter().enumerate() {
            let f: Vec<String> = rec.iter().map(|f| field_text(&f.value)).collect();
            out.push_str(&f.join(","));
            if fin || i + 1 < recs.len() {
                out.push_str(term);
            }
        }
        Ok(out.into_bytes())
    }
    fn decode_block(&self, b: &[u8], _: &[u8]) -> Result<Vec<Record>, Declined> {
        let s = std::str::from_utf8(b).map_err(|_| declined("not_utf8", "not UTF-8"))?;
        Ok(parse(s)?.0)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        table::verbs::VERBS
    }
}

// ---------- a session on a quote-lite doc ----------

pub struct H;

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

pub struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Fx {
    _dir: TmpDir,
    pub s: Arc<Session>,
    pub p: ConnId,
}

pub fn fx(seed: &str) -> Fx {
    fx_pack(Arc::new(QuoteLite), seed)
}

pub fn fx_pack(pack: Arc<dyn Pack>, seed: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-grid-lines-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), Config::default());
    let s = mgr
        .open(
            pack,
            &DocKey {
                space: "ws".into(),
                path: "t.csv".into(),
            },
            Some(seed.as_bytes()),
        )
        .unwrap();
    let p = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx {
        _dir: TmpDir(dir),
        s,
        p,
    }
}

/// A doc opened from a real file, so an outside save can overwrite a live value. The file's path.
pub fn fx_file(seed: &str) -> (Fx, std::path::PathBuf) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-grid-lines-file-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let files = dir.join("files");
    std::fs::create_dir_all(&files).unwrap();
    let path = files.join("t.csv");
    std::fs::write(&path, seed).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "t.csv".into(),
    };
    let s = mgr
        .open_file(Arc::new(QuoteLite), &key, &path, &files)
        .unwrap();
    let p = s
        .join(
            Participant::new("p", "P", "#1", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    (
        Fx {
            _dir: TmpDir(dir),
            s,
            p,
        },
        path,
    )
}

impl Fx {
    /// The rendered file, as the session writes it.
    pub fn file(&self) -> String {
        String::from_utf8(self.s.read(self.p).unwrap().bytes).unwrap()
    }
    /// Another participant, an agent.
    pub fn agent(&self) -> ConnId {
        self.s
            .join(
                Participant::new("a", "A", "#2", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }
}

/// `argv` split at single spaces.
pub fn argv(a: &str) -> Vec<String> {
    a.split(' ').map(str::to_owned).collect()
}

/// `run_tool` on the rows root; an error before the verb ran is its exit and message.
pub fn run(f: &Fx, a: &str, stdin: &str) -> (u8, String) {
    match f.s.run_tool(f.p, ROWS, &argv(a), stdin.as_bytes()) {
        Ok(o) => (o.exit, String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => (e.exit(), e.to_string()),
    }
}
