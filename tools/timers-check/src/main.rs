//! The timers check (tools/README.md, "Timers check"): the kit's production timers in a
//! host's build of the kit (no features, so none of the test-only hooks). Exits non-zero, with
//! the reason, when they are missing, on by default, or don't count a session's calls; prints
//! a `RUNREC` line with what a scope costs on this machine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::Edit;
use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::timers::{self, Path};

struct H(AtomicU64);

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn fail(why: &str) -> ! {
    eprintln!("timers-check: FAIL: {why}");
    std::process::exit(1)
}

fn main() {
    if timers::HOOKS {
        fail("the kit was built with its test-only hooks (differential or conformance)");
    }
    if timers::enabled() {
        fail("the timers are on before anyone switched them on");
    }
    let dir: PathBuf =
        std::env::temp_dir().join(format!("k2mb-timers-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).expect("temp folder");
    let file = dir.join("ws/a.md");
    std::fs::write(&file, "one\ntwo\n").expect("file");
    let host = Arc::new(H(AtomicU64::new(1_000_000)));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
    let mgr = SessionManager::new(store, host.clone(), Config::default());
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 4 << 20)),
            &key,
            &file,
            &dir.join("ws"),
        )
        .expect("open_file");
    let edit = |conn, n: u32| {
        host.0.fetch_add(1_000, Ordering::SeqCst);
        s.edit(
            conn,
            vec![Edit::Text {
                root: TextFilePack::ROOT.to_owned(),
                hunks: vec![Hunk {
                    start: 0,
                    end: 0,
                    insert: format!("line {n}\n"),
                }],
            }],
            None,
        )
        .expect("edit");
    };
    // off: nothing counts
    let conn = s
        .join(
            Participant::new("pat", "Pat", "#1", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .expect("join")
        .conn;
    edit(conn, 0);
    let off = timers::snapshot();
    if let Some(p) = off.paths.iter().find(|p| p.count != 0) {
        fail(&format!("{} counted {} while off", p.name, p.count));
    }
    // on: an edit and a join count
    timers::set_enabled(true);
    for n in 1..=10 {
        edit(conn, n);
    }
    s.join(
        Participant::new("sam", "Sam", "#2", ParticipantKind::Person, "cli"),
        Stay::Held,
    )
    .expect("join");
    timers::set_enabled(false);
    let snap = mgr.timers();
    if snap.paths.len() != Path::ALL.len() {
        fail("the snapshot doesn't list every path");
    }
    // (path, calls wanted, exactly): an edit's engine doc may be edited again (a replica)
    for (path, want, exact) in [
        (Path::EditAs, 10, false),
        (Path::Commit, 10, true),
        (Path::StoreTxnCommit, 10, true),
        (Path::Join, 1, true),
    ] {
        let got = timers::read(path).0;
        if got < want || (exact && got != want) {
            fail(&format!("{} counted {got}, wanted {want}", path.name()));
        }
    }
    let c = timers::calibrate(10_000_000);
    let commit = snap.get(Path::StoreTxnCommit).expect("listed");
    eprintln!(
        "RUNREC {{\"run\":\"timers_check\",\"tool\":\"timers_check\",\"status\":\"COMPLETE\",\
         \"debug\":{},\"hooks\":{},\"paths\":{},\"off_ns\":{:.3},\"on_ns\":{:.2},\"rounds\":{},\
         \"store_txn_commit_p50_ns\":{}}}",
        cfg!(debug_assertions),
        timers::HOOKS,
        Path::ALL.len(),
        c.off_ns,
        c.on_ns,
        c.rounds,
        commit.p50_ns
    );
    mgr.release().expect("release");
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
    println!(
        "timers-check: ok ({} paths, off by default, counted once switched on)",
        Path::ALL.len()
    );
}
