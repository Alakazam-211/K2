//! The release of freed memory after a tick that wrote a doc back (v0.24,
//! [`Config::release_after_write_ms`]). A Yrs doc makes no cut, so this is the only point its
//! write-backs' and outside saves' freed memory goes back to the OS.
//!
//! Plant (`MPC_BETA_PLANT`): `write-release-skipped` (a writing tick releases nothing), caught
//! at `a_write_back_tick_releases_once_per_interval`'s first release assert and at
//! `the_release_after_a_write_is_off_at_zero_and_the_kits_own_by_default`'s kit's-own assert.
//! The host's release skipped (`host-release-skipped`, the kit's own running instead) also fails
//! the interval row, at its first release assert: a host's release is the one a writing tick runs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::store::{DocKey, DocStore};

struct H {
    now: AtomicU64,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

thread_local! {
    /// The host releases this thread ran (the tick runs on the test's thread).
    static RELEASES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn counting() -> MemoryRelease {
    RELEASES.with(|c| c.set(c.get() + 1));
    MemoryRelease::ByHost {
        returned: Some(true),
    }
}

fn releases() -> u64 {
    RELEASES.with(|c| c.get())
}

const BY_HOST: MemoryRelease = MemoryRelease::ByHost {
    returned: Some(true),
};

struct Fx {
    dir: PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
    s: Arc<Session>,
    c: ConnId,
    n: u64,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, c: Config) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-release-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    let file = dir.join("ws").join("a.md");
    std::fs::write(&file, "start\n").unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, host.clone(), c);
    let k = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(Arc::new(MD), &k, &file, &dir.join("ws"))
        .unwrap();
    let c = s
        .join(
            Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx {
        dir,
        host,
        mgr,
        s,
        c,
        n: 0,
    }
}

impl Fx {
    /// An edit, then a tick past the write delay: it writes the doc back.
    fn edit_and_tick(&mut self, ms: u64) -> TickReport {
        self.n += 1;
        let t = self.s.read(self.c).unwrap().text_by_root[0].1.clone();
        self.s
            .replace_text(self.c, TextFilePack::ROOT, &format!("{t}line {}\n", self.n))
            .unwrap();
        self.tick(ms)
    }
    fn tick(&self, ms: u64) -> TickReport {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
        let r = self.mgr.tick().unwrap();
        assert!(r.errors.is_empty(), "control: {:?}", r.errors);
        r
    }
}

fn with(every: u64, host: bool) -> Config {
    Config {
        release_after_write_ms: every,
        release: host.then(|| HostRelease::new(counting)),
        idle_leave_ms: u64::MAX / 4,
        ..Config::default()
    }
}

/// A tick that wrote a doc back releases (the host's release when it gave one), once per
/// interval at most: the next writing tick inside the interval doesn't, a tick that wrote
/// nothing doesn't, and the first writing tick past it does again.
#[test]
fn a_write_back_tick_releases_once_per_interval() {
    let mut f = fx("interval", with(10_000, true));
    let quiet = Config::default().write_quiet_ms + 1;
    let r0 = releases();
    let r = f.edit_and_tick(quiet);
    assert_eq!(r.written.len(), 1, "control: the tick wrote the doc back");
    assert_eq!(
        r.memory_released_after_write,
        Some(BY_HOST),
        "row: a writing tick runs the host's release"
    );
    assert_eq!(releases() - r0, 1, "row: once");
    // a tick that writes nothing
    let r = f.tick(quiet);
    assert!(r.written.is_empty(), "control: nothing to write");
    assert_eq!(
        r.memory_released_after_write, None,
        "row: no write, no release"
    );
    // a writing tick inside the interval
    let r = f.edit_and_tick(quiet);
    assert_eq!(r.written.len(), 1, "control: written");
    assert_eq!(
        r.memory_released_after_write, None,
        "row: inside the interval, no release"
    );
    assert_eq!(releases() - r0, 1, "row: still once");
    // past the interval
    let r = f.edit_and_tick(10_000);
    assert_eq!(r.written.len(), 1, "control: written");
    assert_eq!(
        r.memory_released_after_write,
        Some(BY_HOST),
        "row: past the interval, the release again"
    );
    assert_eq!(releases() - r0, 2, "row: twice in all");
}

/// `release_after_write_ms = 0`: never; and with no host release, the kit's own runs.
#[test]
fn the_release_after_a_write_is_off_at_zero_and_the_kits_own_by_default() {
    let quiet = Config::default().write_quiet_ms + 1;
    let mut f = fx("off", with(0, true));
    let r0 = releases();
    for _ in 0..3 {
        let r = f.edit_and_tick(quiet + 30_000);
        assert_eq!(r.written.len(), 1, "control: written");
        assert_eq!(r.memory_released_after_write, None, "row: 0 is never");
    }
    assert_eq!(releases(), r0, "row: the host's release never ran");
    let mut f = fx("default", Config::default());
    let r = f.edit_and_tick(quiet);
    assert_eq!(r.written.len(), 1, "control: written");
    let own = r.memory_released_after_write;
    if cfg!(all(target_os = "linux", target_env = "gnu")) {
        assert!(
            matches!(own, Some(MemoryRelease::Trimmed { .. })),
            "row: by default the kit's own release (glibc's trim): {own:?}"
        );
    } else {
        assert_eq!(
            own,
            Some(MemoryRelease::NotOnThisPlatform),
            "row: by default the kit's own (none on this platform)"
        );
    }
    assert_eq!(releases(), r0, "row: no host release was given");
}
