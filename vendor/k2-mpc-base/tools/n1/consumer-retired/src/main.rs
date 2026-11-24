//! N−1 of the person-writer row's retired bit (tools/n1/n1.sh `layout-retired`), built against
//! the previous release, which reads a person row's state byte as 0 (free) or 1 (leased) and any
//! other value as unknown: a row it never hands out.
//!
//! - `read <dir>`: `<dir>` holds two stores this repo's `n1_retired_producer` wrote, each with
//!   `keep.md`, whose person `ada` released a writer on device `laptop` (its id in
//!   `<store>.writer`):
//!   - `free.redb` (the control): the writer free; here `ada` on `laptop` takes it again;
//!   - `retired.redb`: the writer retired (this build's rotation marked its row); here it opens
//!     with no refusal, the row isn't listed, `ada` on `laptop` takes another writer, and an edit
//!     lands. Under the producer's planted fault (`retired-bit-dropped`) the row reads free here
//!     and this side fails at its never-let-again assert.
//! - `produce <dir>`: the other way: it writes `<dir>/prev.redb` (`keep.md`, `ada`'s writer on
//!   `laptop` released, a second on `phone` left parked), its free writer's id in `prev.writer`
//!   and every person row it reads back in `prev.rows`, field for field, for this repo's
//!   `n1_retired_prev_rows`.
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::session::{
    AuditEvent, Config, Host, JoinOptions, Participant, ParticipantKind, Rights, SessionManager,
    Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::ClientId;

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
/// This side's edit (this repo's `n1_retired_after` looks for it).
const PREV_LINE: &str = "edited by the previous release";

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "keep.md".into(),
    }
}

fn mgr(s: &Arc<DocStore>) -> SessionManager {
    let host = Arc::new(H {
        now: AtomicU64::new(1_900_000_000_000),
    });
    let mut c = Config::default();
    c.reuse_person_writers = true;
    SessionManager::new(s.clone(), host, c)
}

fn ada() -> Participant {
    Participant::new("ada", "Ada", "#123456", ParticipantKind::Person, "tab")
}

fn device(name: &str) -> JoinOptions {
    JoinOptions::new().device(name.as_bytes())
}

fn writer_of(dir: &Path, name: &str) -> ClientId {
    fs::read_to_string(dir.join(format!("{name}.writer")))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// `ada` joins `name`'s `keep.md` on `laptop`, appends this side's line and releases. Returns
/// the writer it took and the person rows this release lists for the doc.
fn join_and_edit(dir: &Path, name: &str) -> (ClientId, Vec<ClientId>) {
    let p = dir.join(format!("{name}.redb"));
    let s = Arc::new(
        DocStore::open(&p).unwrap_or_else(|e| panic!("row: {name}.redb opens here: {e}")),
    );
    let id = s.find(&key()).unwrap().expect("keep.md is stored");
    let listed: Vec<ClientId> = s.person_writers(id).unwrap().iter().map(|r| r.0).collect();
    let m = mgr(&s);
    let sess = m.open(Arc::new(MD), &key(), None).unwrap();
    let j = sess
        .join_with(ada(), Stay::Held, &device("laptop"))
        .unwrap_or_else(|e| panic!("row: {name}.redb: ada joins here: {e}"));
    let text = sess.read(j.conn).unwrap().text_by_root[0].1.clone();
    sess.replace_text(j.conn, TextFilePack::ROOT, &format!("{text}{PREV_LINE}\n"))
        .unwrap_or_else(|e| panic!("row: {name}.redb: the edit lands here: {e}"));
    assert!(sess.release(j.conn).unwrap());
    drop(sess);
    m.release().unwrap();
    (j.client, listed)
}

fn read(dir: &Path) {
    // the control: a free writer of this build's is let again here
    let free = writer_of(dir, "free");
    let (took, _) = join_and_edit(dir, "free");
    assert_eq!(took, free, "control: free.redb: ada's free writer is let again here");
    println!("N1-RETIRED free.redb (control): the previous release let ada's free writer {free} again");

    // the retired writer: never let again
    let retired = writer_of(dir, "retired");
    let (took, listed) = join_and_edit(dir, "retired");
    println!(
        "N1-RETIRED retired.redb: the previous release opened it (no refusal), listed person \
         rows {listed:?}, ada on laptop took {took}, this build's retired writer {retired}"
    );
    assert_ne!(
        took, retired,
        "row: retired.redb: the previous release never lets this build's retired writer again"
    );
    assert!(
        !listed.contains(&retired),
        "row: retired.redb: the retired row reads as unknown here (not listed)"
    );
    println!(
        "N1-RETIRED retired.redb ok: the retired writer {retired} not listed and never let again \
         (ada took {took}); the edit landed"
    );
}

fn produce(dir: &Path) {
    let p = dir.join("prev.redb");
    let _ = fs::remove_file(&p);
    let s = Arc::new(DocStore::open(&p).unwrap());
    let m = mgr(&s);
    let sess = m
        .open(Arc::new(MD), &key(), Some(b"# Previous\n\nmade by the previous release\n"))
        .unwrap();
    let laptop = sess.join_with(ada(), Stay::Held, &device("laptop")).unwrap();
    sess.replace_text(laptop.conn, TextFilePack::ROOT, "# Previous\n\nby ada on laptop\n")
        .unwrap();
    assert!(sess.release(laptop.conn).unwrap());
    let phone = sess.join_with(ada(), Stay::Held, &device("phone")).unwrap();
    sess.replace_text(phone.conn, TextFilePack::ROOT, "# Previous\n\nby ada on her phone\n")
        .unwrap();
    // an unclean end: parked
    assert!(sess.leave(phone.conn).unwrap());
    drop(sess);
    m.release().unwrap();
    drop(m);
    let id = s.find(&key()).unwrap().unwrap();
    let rows = s.person_writers(id).unwrap();
    assert_eq!(rows.len(), 2, "control: two person rows");
    let mut out = String::new();
    for (c, owner, r) in &rows {
        assert_eq!(owner, "ada");
        let dev: String = r.device.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!(
            "{c} {} {dev} {} {} {} {}\n",
            u8::from(r.leased),
            r.token,
            r.ended_token,
            r.ended_seq,
            r.ended_ms
        ));
    }
    drop(s);
    fs::write(dir.join("prev.rows"), &out).unwrap();
    fs::write(dir.join("prev.writer"), laptop.client.to_string()).unwrap();
    println!(
        "N1-RETIRED prev produced prev.redb: ada's free writer {} on laptop, a parked one {} on \
         her phone; {} rows listed",
        laptop.client,
        phone.client,
        rows.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: read|produce <dir>");
        std::process::exit(2);
    };
    let dir = Path::new(dir);
    match mode.as_str() {
        "read" => read(dir),
        "produce" => produce(dir),
        _ => {
            eprintln!("usage: read|produce <dir>");
            std::process::exit(2);
        }
    }
}
