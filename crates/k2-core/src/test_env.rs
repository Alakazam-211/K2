//! The ONE lock for process-environment mutation in tests (quiet-gate
//! PRD §5.3, 0.45.1).
//!
//! `$HOME`, `$PATH`, `$SHELL` and the `K2_*` / `K2SO_*` knobs are process
//! globals. Cargo runs a test binary's tests on many threads, so two tests
//! that change the same variable under DIFFERENT locks (or none) stomp each
//! other: one test's store lands under another test's (then deleted) temp
//! home. Before 0.45.1 k2-core had four separate HOME/PATH locks and
//! k2-daemon three more, which is exactly how the `skin::`, `sql::`,
//! `federation_routes::` and `subscription_usage::` flakes happened.
//!
//! Rules (also in `docs/testing.md`):
//!
//! - Every test that sets or removes an env var takes [`lock`] first, or
//!   uses a guard here that takes it ([`EnvVar`], [`TempHome`]).
//! - Every test that READS `$HOME` (directly, or through `paths::home_dir()`
//!   / a store under `~/.k2`) and needs it stable holds [`lock`] (a
//!   [`TempHome`] does) for the whole time it depends on that value.
//! - Never call `std::env::set_var` / `remove_var` for `HOME`, `PATH` or
//!   `SHELL` outside this module: a source-walk ratchet test fails on it.
//! - k2-daemon re-uses this exact lock (through `test-util`), so both
//!   crates' code in one test binary serialize on the same mutex.
//!
//! The lock is RE-ENTRANT on the same thread, so a test holding it may
//! call a helper that takes it again. It does not poison: a panicking
//! test releases it and the next test runs normally. Guards restore the
//! previous value on drop (LIFO), including on panic.
//!
//! Async tests: the guard is `!Send`, which is fine for `#[tokio::test]`
//! (the test body runs on the test thread). Code inside spawned tasks that
//! reads `$HOME` sees the test's value only while the guard is alive.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};

static ENV_LOCK: ReentrantMutex<()> = parking_lot::const_reentrant_mutex(());

thread_local! {
    /// How deep this thread holds [`ENV_LOCK`] (it is re-entrant): the
    /// process switches are reset on the outermost take and restored on the
    /// outermost release only.
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The process-wide feature switches the settings routes mirror into this
/// process (`sync_permission_cards`, `sync_garden_access`,
/// `federation::set_enabled`, …). They are atomics, not env vars, but just
/// as global: a test that called `/cli/settings/update` left Garden access
/// (default true) on for every later test, and a test that expects
/// 0.46.3's retired path failed whenever it ran after one (nightly soak
/// 2026-10-25, shuffled seeds 1–10). A holder of [`lock`] starts from the
/// values a fresh test process has, and on release the values it found
/// are put back, so no test sees another's switches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Switches {
    garden_access: bool,
    permission_cards: bool,
    federation: bool,
    compute: bool,
    compute_placement_v1: bool,
    apps: bool,
    apps_process_engine: bool,
    apps_auto_prepare: bool,
    apps_engine_heal: bool,
    api: bool,
    airgap: bool,
    listen_lan: bool,
}

impl Switches {
    /// What a fresh process holds (each static's initializer).
    const PRISTINE: Switches = Switches {
        garden_access: false,
        permission_cards: false,
        federation: false,
        compute: false,
        compute_placement_v1: false,
        apps: false,
        apps_process_engine: false,
        apps_auto_prepare: crate::deploy::AUTO_PREPARE_DEFAULT,
        apps_engine_heal: true,
        api: false,
        airgap: false,
        listen_lan: false,
    };

    fn read() -> Switches {
        Switches {
            garden_access: crate::zen::garden_access::enabled(),
            permission_cards: crate::permission::gate::setting_raw(),
            federation: crate::federation::setting_raw(),
            compute: crate::compute::setting_raw(),
            compute_placement_v1: crate::compute::placement_policy() == crate::compute::PlacementPolicy::V1,
            apps: crate::deploy::setting_raw(),
            apps_process_engine: crate::deploy::process_engine_setting_raw(),
            apps_auto_prepare: crate::deploy::auto_prepare_setting_raw(),
            apps_engine_heal: crate::deploy::engine_heal_setting_raw(),
            api: crate::app_settings::api_setting_raw(),
            airgap: crate::airgap::setting_raw(),
            listen_lan: crate::listen::setting_raw(),
        }
    }

    fn write(self) {
        crate::zen::garden_access::set_switch(self.garden_access);
        crate::permission::gate::set_server_switch(self.permission_cards);
        crate::federation::set_enabled(self.federation);
        crate::compute::set_enabled(self.compute);
        crate::compute::set_placement_policy(if self.compute_placement_v1 { crate::compute::PlacementPolicy::V1 } else { crate::compute::PlacementPolicy::V2 });
        crate::deploy::set_enabled(self.apps);
        crate::deploy::set_process_engine(self.apps_process_engine);
        crate::deploy::set_auto_prepare(self.apps_auto_prepare);
        crate::deploy::set_engine_heal(self.apps_engine_heal);
        crate::app_settings::set_api_enabled(self.api);
        crate::airgap::set_setting_enabled(self.airgap);
        crate::listen::set_setting_lan(self.listen_lan);
    }
}

/// Guard for the crate-wide test env lock. The outermost guard on a
/// thread also owns the process switches (see [`Switches`]).
#[must_use = "bind the guard for the test's duration"]
pub struct EnvLock {
    /// The switches found when the outermost guard was taken (restored on
    /// its drop); `None` for a nested guard.
    found: Option<Switches>,
    _guard: ReentrantMutexGuard<'static, ()>,
}

impl Drop for EnvLock {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        if let Some(found) = self.found.take() {
            // Still held: the mutex guard field drops after this body.
            found.write();
        }
    }
}

/// Take the crate-wide (and, through `test-util`, cross-crate) env lock.
/// The outermost take on a thread resets the process switches to a fresh
/// process's values; its release puts back what it found.
pub fn lock() -> EnvLock {
    let guard = ENV_LOCK.lock();
    // Before any holder can change TMPDIR: the first take in the process is
    // the first moment a change could happen, and every later one is under
    // this lock (see [`stable_temp_dir`]).
    TEMP_ROOT.get_or_init(std::env::temp_dir);
    let outermost = DEPTH.with(|d| {
        let n = d.get();
        d.set(n + 1);
        n == 0
    });
    let found = outermost.then(|| {
        let found = Switches::read();
        Switches::PRISTINE.write();
        found
    });
    EnvLock { found, _guard: guard }
}

/// A module test lock taken AFTER the env lock: `_inner` is the module's
/// guard, `_env` the env lock (fields drop in order: module first).
///
/// Use [`serial`] for every test-only module lock (a singleton DB row, a
/// job map, a mock slot) whose tests may also change env. Because the env
/// lock is always taken first, "module lock, then `TempHome`" in one test
/// and "`EnvVar`, then module lock" in another can never deadlock (the env
/// lock is re-entrant on the holder's thread).
#[must_use = "bind the guard for the test's duration"]
pub struct EnvSerial<G> {
    _inner: G,
    _env: EnvLock,
}

/// [`EnvSerial`] over a `std` mutex guard.
pub type SerialGuard = EnvSerial<std::sync::MutexGuard<'static, ()>>;

/// Take the env lock, then `module_lock` (poison recovered: a panicking
/// test must not fail the next one).
pub fn serial(module_lock: &'static std::sync::Mutex<()>) -> SerialGuard {
    let env = lock();
    let inner = module_lock.lock().unwrap_or_else(|p| p.into_inner());
    EnvSerial { _inner: inner, _env: env }
}

/// Take the env lock, then build the module guard(s) with `f` (for a module
/// lock that comes with its own `TempHome`, a tuple, …).
pub fn serial_with<G>(f: impl FnOnce() -> G) -> EnvSerial<G> {
    let env = lock();
    EnvSerial { _inner: f(), _env: env }
}

/// A zero-sized handle whose `.lock()` takes [`lock`]. Old per-module lock
/// statics (`themes::HOME_LOCK`, …) are declared as this type, so any code
/// still written against them (including branches merged later) serializes
/// on the ONE env lock instead of a private one.
pub struct SharedEnvLock;

impl SharedEnvLock {
    /// Same as [`lock`].
    pub fn lock(&self) -> EnvLock {
        lock()
    }
}

/// Set or remove one env var for the life of the guard, then restore the
/// previous value. Holds [`lock`] the whole time.
#[must_use = "the variable is restored when the guard drops"]
pub struct EnvVar {
    key: OsString,
    prev: Option<OsString>,
    _lock: EnvLock,
}

impl EnvVar {
    /// Set `key` to `value` until the guard drops.
    pub fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.as_ref().to_os_string();
        let prev = std::env::var_os(&key);
        std::env::set_var(&key, value);
        Self { key, prev, _lock: lock }
    }

    /// Remove `key` until the guard drops.
    pub fn remove(key: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.as_ref().to_os_string();
        let prev = std::env::var_os(&key);
        std::env::remove_var(&key);
        Self { key, prev, _lock: lock }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
    }
}

/// The temp dir this test process started with (see [`stable_temp_dir`]).
static TEMP_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// This test process's own temp dir, whatever `TMPDIR` says right now.
///
/// `std::env::temp_dir()` reads `TMPDIR` on every call, and a few tests point
/// it at a short folder of their own under [`lock`] and delete that folder
/// afterwards. A test that reads `temp_dir()` without the lock in that window
/// builds its files inside another test's folder, and they vanish mid-test
/// (`k2_core` soak 10-30, seed 5: `db::home_switch` got SQLite `CannotOpen`;
/// `k2_daemon` seed 6: `mail::secrets` lost its store). Take scratch paths
/// from here instead. Lock-free after the process's first [`lock`], so it is
/// safe under any module lock.
pub fn stable_temp_dir() -> PathBuf {
    if let Some(root) = TEMP_ROOT.get() {
        return root.clone();
    }
    let _env = lock();
    TEMP_ROOT.get().expect("lock() set it").clone()
}

/// A fresh, unique temp dir path (not created). Uses a uuid, never the pid,
/// a `ThreadId` or a timestamp: those repeat across the several test
/// binaries of one `cargo test` run and across runs.
pub fn unique_temp_path(tag: &str) -> PathBuf {
    stable_temp_dir().join(format!("k2-{tag}-{}", uuid::Uuid::new_v4().simple()))
}

/// A short temp path for a test's HOME (or a socket): `/tmp/<name>` on
/// Unix, short so a per-cell socket under it fits SUN_LEN. Windows: the
/// temp folder, because "/tmp" has no drive there, so K2 skips it as HOME
/// and uses USERPROFILE: the real home, not the test's.
pub fn short_tmp(name: impl AsRef<str>) -> PathBuf {
    #[cfg(windows)]
    {
        std::env::temp_dir().join(name.as_ref())
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(format!("/tmp/{}", name.as_ref()))
    }
}

/// `first` ahead of the path list `rest` (PATH, K2_TEST_AGENT_SHIM_DIR),
/// joined with this OS's separator: `first:rest` on Unix, `first;rest` on
/// Windows (where ':' is part of a drive path). An empty `rest` is `first`.
pub fn path_list_prepend(first: impl AsRef<Path>, rest: impl AsRef<std::ffi::OsStr>) -> String {
    let rest = rest.as_ref();
    if rest.is_empty() {
        return first.as_ref().display().to_string();
    }
    let mut parts = vec![first.as_ref().to_path_buf()];
    parts.extend(std::env::split_paths(rest));
    std::env::join_paths(parts).expect("a path in the list holds the separator").to_string_lossy().into_owned()
}

/// A cheap long-lived program for a test PTY, one that just waits for
/// input: `unix` (`cat` or `/bin/cat`) on Unix. Windows has no `cat`;
/// System32's `cmd.exe` stays open the same way and echoes what is typed,
/// and the temp-HOME spawn belt allows it (System32, like `/bin`).
pub fn pty_idle_program(unix: &str) -> String {
    #[cfg(windows)]
    {
        let _ = unix;
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{root}\System32\cmd.exe")
    }
    #[cfg(not(windows))]
    {
        unix.to_string()
    }
}

/// The bash a test runs `cli/k2` with: `bash` on Unix; on Windows, Git for
/// Windows' bash (Windows looks a program up through the child's PATH, which
/// these tests set, so a bare name isn't enough). `K2_TEST_BASH` overrides.
/// No Git Bash on Windows is a loud failure.
pub fn bash_program() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(b) = std::env::var_os("K2_TEST_BASH").filter(|v| !v.is_empty()) {
            return PathBuf::from(b);
        }
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|r| PathBuf::from(r).join("Git").join("bin").join("bash.exe"))
            .find(|p| p.is_file())
            .expect("the CLI tests need Git for Windows (Git Bash), or K2_TEST_BASH")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("bash")
    }
}

/// The PATH a test gives `cli/k2` under a cleared environment: `unix` on
/// Unix; on Windows, System32 and the folder of the real Python on this
/// process's PATH (the CLI's `python3`). Git Bash adds its own tools.
pub fn cli_path(unix: &str) -> String {
    #[cfg(windows)]
    {
        let _ = unix;
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let mut parts = vec![format!(r"{sys}\System32"), sys];
        let python = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .find(|d| d.join("python.exe").is_file() && !d.to_string_lossy().contains("WindowsApps"));
        if let Some(d) = python {
            parts.insert(0, d.display().to_string());
        }
        parts.join(";")
    }
    #[cfg(not(windows))]
    {
        unix.to_string()
    }
}

/// The variables a cleared environment must keep for a Windows program
/// (Python can't start without SystemRoot); empty on Unix.
pub fn cli_base_env() -> Vec<(String, String)> {
    #[cfg(windows)]
    {
        ["SystemRoot", "windir", "ComSpec", "PATHEXT", "TEMP", "TMP"]
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Remove a test's temp dir, or panic naming what couldn't go. Windows
/// refuses to delete a file something still holds open (Unix doesn't
/// mind), so a bare `remove_dir_all` failure says only "in use"; this
/// lists which files, so the holder can be found.
pub fn remove_test_dir(dir: &Path) {
    // The process keeps one skin.db connection for its life; a test home
    // that held skin.db can't be removed on Windows while it is open.
    crate::skin::close_cached_connection();
    let Err(e) = std::fs::remove_dir_all(dir) else { return };
    let mut stuck = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p);
            } else if let Err(err) = std::fs::remove_file(&p) {
                stuck.push(format!("{} ({err})", p.display()));
            }
        }
    }
    stuck.sort();
    stuck.truncate(12);
    panic!("remove {}: {e}; still held: {stuck:#?}", dir.display());
}

/// RAII temp `$HOME`: holds [`lock`], points `$HOME` at a fresh temp dir
/// (with `.k2/` pre-created so writers that stage a tmp file and rename it
/// into `~/.k2/…` find the parent), checks the test cannot reach the
/// production daemon, and on drop restores the previous `$HOME` and removes
/// the dir.
#[must_use = "HOME is restored when the guard drops"]
pub struct TempHome {
    prev: Option<OsString>,
    home: PathBuf,
    /// `K2_RUNTIME_DIR` → `<home>/run`: the private runtime dir (rendered
    /// frpc.toml) stays inside this temp home, never the real
    /// `$TMPDIR/k2-runtime` / `/run/user/<uid>/k2` (2026-10-14 incident).
    _runtime: EnvVar,
    /// `K2_TEST_TRASH_DIR` → `<home>/.Trash`: `safe_delete` moves "trashed"
    /// paths there in test builds, never into the real Trash (0.46.3).
    _trash: EnvVar,
    _lock: EnvLock,
}

impl TempHome {
    /// A temp home under the system temp dir.
    pub fn new() -> Self {
        // The path never follows another test's TMPDIR (vault::spawn's short
        // `/tmp/k2t-…` socket dir): that left this HOME outside the temp dir
        // once TMPDIR was restored (and inside a folder that test deletes),
        // and folder_trust then judged the home "not isolated" and skipped
        // the gate (soak 2026-10-27, k2-daemon shuffled seed 5).
        let lock = lock();
        Self::at_locked(lock, unique_temp_path("test-home"))
    }

    /// A temp home with a SHORT path (`/tmp/k2t-<8 hex>`), for tests that
    /// bind Unix sockets under `$HOME`: a socket path must fit `SUN_LEN`
    /// (~104 bytes on macOS, 108 on Linux), and the macOS per-user temp dir
    /// alone is ~50 bytes.
    #[cfg(unix)]
    pub fn short() -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Self::at(PathBuf::from(format!("/tmp/k2t-{}", &id[..8])))
    }

    /// Windows: no Unix-socket path limit applies; a normal temp home.
    #[cfg(not(unix))]
    pub fn short() -> Self {
        Self::new()
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    fn at(home: PathBuf) -> Self {
        Self::at_locked(lock(), home)
    }

    fn at_locked(lock: EnvLock, home: PathBuf) -> Self {
        std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        crate::test_isolation::assert_isolated_from_prod();
        let runtime = EnvVar::set(crate::runtime_dir::RUNTIME_DIR_ENV, home.join("run"));
        let trash = EnvVar::set(crate::safe_delete::TEST_TRASH_DIR_ENV, home.join(".Trash"));
        Self { prev, home, _runtime: runtime, _trash: trash, _lock: lock }
    }

    /// The temp `$HOME`.
    pub fn path(&self) -> &Path {
        &self.home
    }
}

impl Default for TempHome {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Write an executable script (mode 0755) that a test will exec.
///
/// Never `fs::write` + `chmod` + exec from a multi-threaded test binary on
/// Linux: while this thread holds the file open for writing, another test
/// thread may `fork`, and the child keeps a copy of that write fd until it
/// `exec`s. An `exec` of the script inside that window fails with ETXTBSY
/// ("Text file busy"), which surfaced as a 3/200 flake. The bytes are
/// written by a short-lived `/bin/sh` child instead, so the write fd only
/// ever exists in that child, never in this process.
///
/// Windows has no `#!`: the script is written at `path` (LF line endings)
/// and a copy of the `k2-test-sh-shim` program at `path.exe` (`x.sh` →
/// `x.exe`), which runs it in Git Bash. An extensionless `path` handed to
/// `Command` or to K2 resolves to that `.exe`.
pub fn write_executable(path: &Path, contents: &str) {
    #[cfg(windows)]
    {
        std::fs::write(path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        let exe = if path.extension().is_some_and(|e| e == "sh") {
            path.with_extension("exe")
        } else {
            let mut s = path.as_os_str().to_os_string();
            s.push(".exe");
            PathBuf::from(s)
        };
        let shim = sh_shim_exe();
        std::fs::copy(&shim, &exe).unwrap_or_else(|e| panic!("copy {} to {}: {e}", shim.display(), exe.display()));
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::write(path, contents)
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }
    #[cfg(unix)]
    write_executable_unix(path, contents);
}

/// The built `k2-test-sh-shim.exe` (Windows): `K2_TEST_SH_SHIM`, else next
/// to this test binary's target dir (`target/<profile>/`). Missing is a
/// loud failure: build it first (`cargo build -p k2-test-sh-shim`), as the
/// ptyd suites need `cargo build -p k2-ptyd --bins`.
#[cfg(windows)]
pub fn sh_shim_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("K2_TEST_SH_SHIM").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().expect("current_exe");
    for dir in exe.ancestors().skip(1).take(3) {
        let cand = dir.join("k2-test-sh-shim.exe");
        if cand.is_file() {
            return cand;
        }
    }
    panic!(
        "k2-test-sh-shim.exe not found near {} — build it first: cargo build -p k2-test-sh-shim \
         (or set K2_TEST_SH_SHIM)",
        exe.display()
    );
}

/// A program that exits 0 at once, whatever its arguments (`/usr/bin/true`
/// on macOS, `/bin/true` on Linux, a written `exit 0` script on Windows,
/// kept in a per-process temp dir).
pub fn true_bin() -> PathBuf {
    #[cfg(unix)]
    {
        for cand in ["/usr/bin/true", "/bin/true"] {
            let p = PathBuf::from(cand);
            if p.exists() {
                return p;
            }
        }
        panic!("no `true` binary found at /usr/bin/true or /bin/true");
    }
    #[cfg(not(unix))]
    {
        static TRUE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        TRUE.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("k2-true-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("true dir");
            let p = dir.join("true");
            write_executable(&p, "#!/bin/sh\nexit 0\n");
            p
        })
        .clone()
    }
}

#[cfg(unix)]
fn write_executable_unix(path: &Path, contents: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn /bin/sh to write {}: {e}", path.display()));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(contents.as_bytes())
        .unwrap_or_else(|e| panic!("pipe script for {}: {e}", path.display()));
    let status = child.wait().expect("wait for script writer");
    assert!(status.success(), "writing {} failed: {status}", path.display());
}

/// RAII agent-CLI shims: a temp dir holding a `#!/bin/sh\nexec cat`
/// executable for every [`crate::terminal::agent_spawn_guard::AGENT_CLIS`]
/// name, registered as `K2_TEST_AGENT_SHIM_DIR` (prepended to any existing
/// list) while the guard lives. Holds [`lock`]. Any test that makes the
/// daemon spawn an agent (an explicit `claude`, or a workspace default
/// agent) needs one: test builds refuse real agent CLIs.
///
/// On Windows each shim is an sh script plus its `.exe` runner
/// ([`write_executable`]), so the same bodies work there.
#[must_use = "the shims are unregistered when the guard drops"]
pub struct AgentShim {
    dir: PathBuf,
    _var: EnvVar,
    /// Taken before the shim dir's path is made from `$TMPDIR` (see
    /// [`TempHome::new`]); dropped last.
    _lock: EnvLock,
}

impl AgentShim {
    /// Install the default `exec cat` shims.
    pub fn install() -> Self {
        Self::with_body("exec cat")
    }

    /// Install shims whose script body (after `#!/bin/sh`) is `body`.
    pub fn with_body(body: &str) -> Self {
        use crate::terminal::agent_spawn_guard::{AGENT_CLIS, SHIM_DIR_ENV};
        let lock = lock();
        let dir = unique_temp_path("agent-shim");
        std::fs::create_dir_all(&dir).expect("create agent shim dir");
        for name in AGENT_CLIS {
            write_executable(&dir.join(name), &format!("#!/bin/sh\n{body}\n"));
        }
        let list = match std::env::var_os(SHIM_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(prev) => {
                let mut dirs = vec![dir.clone()];
                dirs.extend(std::env::split_paths(&prev));
                std::env::join_paths(dirs).expect("join shim dirs")
            }
            None => dir.clone().into_os_string(),
        };
        let var = EnvVar::set(SHIM_DIR_ENV, list);
        Self { dir, _var: var, _lock: lock }
    }

    /// The shim dir (holds one executable per agent CLI name).
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for AgentShim {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Spawn a test PTY running a plain program (`cat`, `sh`, `sleep`), holding
/// [`lock`] while the spawn guard reads the environment.
///
/// While a test has `K2_TEST_AGENT_SHIM_DIR` set, the guard resolves every
/// program in those dirs only, and a plain program isn't there: a spawn with
/// no lock, in that window, fails with "agent shim \"cat\" not found" (soak
/// 10-29 `apply_resize_now_same_dims_is_noop`, 10-30
/// `setactive_claim_cannot_resize_a_pinned_session`). The guard stays strict;
/// the test waits its turn.
pub fn spawn_plain_pty(
    cfg: crate::terminal::DaemonPtyConfig,
) -> std::io::Result<std::sync::Arc<crate::terminal::DaemonPtySession>> {
    let _env = lock();
    crate::terminal::DaemonPtySession::spawn(cfg)
}

/// A loopback HTTP server that answers every request with one fixed
/// error status, for tests that need an "unreachable" upstream (control
/// plane, broker, peer). Use it instead of a well-known dead port such as
/// `127.0.0.1:9` or `:1`: on a host whose firewall drops loopback RSTs
/// (leftover K2 sandbox nft tables did, on the build box) a connect to a
/// closed port HANGS instead of being refused. Binds `127.0.0.1:0`; stops
/// on drop.
pub struct ErrorHttpServer {
    addr: std::net::SocketAddr,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ErrorHttpServer {
    /// Answer every request with `status` (e.g. 503) and an empty body.
    pub fn start(status: u16) -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local_addr");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_t = stop.clone();
        let response = format!(
            "HTTP/1.1 {status} Test Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_t.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                // Read until the end of the request headers (bodies are
                // ignored; the connection closes after the answer).
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { addr, stop, thread: Some(thread) }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The bound address.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }
}

impl Drop for ErrorHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // Unblock `accept` so the thread sees the flag.
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ── Source-walk ratchet (quiet-gate PRD §5.3) ────────────────────────

/// Env vars that only this module may set or remove (test code uses
/// [`TempHome`] / [`EnvVar`]). Production call sites are listed per crate.
pub const PROTECTED_VARS: &[&str] = &["HOME", "PATH", "SHELL"];

/// One file's raw env-mutation call counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvMutationCount {
    /// Every `set_var(` / `remove_var(` call.
    pub all: usize,
    /// Calls whose first argument is a literal in [`PROTECTED_VARS`].
    pub protected: usize,
}

/// Count raw `set_var(` / `remove_var(` calls in Rust source text (comment
/// lines and `fn set_var` definitions excluded; a call split over lines
/// still counts).
pub fn count_env_mutations(src: &str) -> EnvMutationCount {
    // Blank out `//` comment lines so prose about set_var never counts.
    let code: String = src
        .lines()
        .map(|l| if l.trim_start().starts_with("//") { "" } else { l })
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = code.as_bytes();
    let mut out = EnvMutationCount::default();
    for needle in ["set_var(", "remove_var("] {
        let mut from = 0;
        while let Some(i) = code[from..].find(needle) {
            let at = from + i;
            from = at + needle.len();
            let prev = if at == 0 { b' ' } else { bytes[at - 1] };
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                continue; // e.g. `my_set_var(`
            }
            if code[..at].trim_end().ends_with("fn") {
                continue; // a definition, not a call
            }
            out.all += 1;
            let rest = code[from..].trim_start();
            if PROTECTED_VARS
                .iter()
                .any(|v| rest.starts_with(&format!("\"{v}\"")))
            {
                out.protected += 1;
            }
        }
    }
    out
}

/// Walk `src_root` and compare every `.rs` file's env-mutation counts with
/// `baseline` (path relative to `src_root`, `/`-separated, then
/// `(max all, max protected)`; files not listed allow 0). Returns one line
/// per file over its limit; empty = pass. Lower counts always pass, so the
/// baseline only ever needs lowering (never raising) when sites are fixed.
pub fn env_mutation_violations(
    src_root: &Path,
    baseline: &[(&str, usize, usize)],
) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![src_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(src_root)
            .expect("under src root")
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let c = count_env_mutations(&src);
        let (max_all, max_protected) = baseline
            .iter()
            .find(|(f, _, _)| *f == rel)
            .map(|(_, a, p)| (*a, *p))
            .unwrap_or((0, 0));
        if c.protected > max_protected {
            out.push(format!(
                "{rel}: {} set_var/remove_var of HOME/PATH/SHELL (allowed {max_protected}): \
                 use k2_core::test_env::TempHome / EnvVar",
                c.protected
            ));
        }
        if c.all > max_all {
            out.push(format!(
                "{rel}: {} raw set_var/remove_var calls (allowed {max_all}): tests change env \
                 only through k2_core::test_env::EnvVar (holds the one env lock, restores on drop)",
                c.all
            ));
        }
    }
    out
}

/// Lines under `src_root` (non-comment) that use `thread::current().id()`.
/// Temp paths named from a `ThreadId` (or pid + nanos) repeat across the
/// test binaries of one run and across runs (the 0.45.0 gate leaked
/// `k2-update-test-boot-<pid>-ThreadId(N)` homes); use
/// [`unique_temp_path`]. Zero-tolerance for any use that can feed a name;
/// the one exception is a plain identity comparison
/// (`h.thread().id() == std::thread::current().id()`, k2-ptyd's "never join
/// ourselves"), which names nothing ([`thread_id_line_names_something`]).
pub fn thread_id_name_violations(src_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![src_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (n, line) in src.lines().enumerate() {
                // concat! so this line never matches itself.
                if !line.trim_start().starts_with("//") && thread_id_line_names_something(line) {
                    out.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    out.sort();
    out
}

/// Does this line use the current `ThreadId` for anything but an `==` /
/// `!=` comparison with another `ThreadId`? Every such use counts (a
/// `format!`, a path, a `{:?}`, a binding that may become one later); only
/// a line whose every `current().id()` is one side of an identity
/// comparison with another `.id()` is allowed.
pub fn thread_id_line_names_something(line: &str) -> bool {
    // concat! so this function never matches its own source line.
    let needle = concat!("current()", ".id()");
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let mut rest = compact.as_str();
    while let Some(i) = rest.find(needle) {
        let before = &rest[..i];
        let after = &rest[i + needle.len()..];
        let lhs_is_id = |s: &str| s.ends_with(".id()==") || s.ends_with(".id()!=");
        // `X.id() == std::thread::current().id()`: strip the path prefix.
        let before_cmp = before.trim_end_matches("std::thread::").trim_end_matches("thread::");
        let rhs_is_id = (after.starts_with("==") || after.starts_with("!=")) && after[2..].contains(".id()");
        if !(lhs_is_id(before_cmp) || rhs_is_id) {
            return true;
        }
        rest = after;
    }
    false
}

/// A loopback TCP port that is RESERVED but not listening: connects to it
/// are refused at once, and no other test can take it while the guard
/// lives. Use it for "nothing listens there" tests instead of binding a
/// listener and dropping it, which frees the port for any parallel test to
/// re-bind (`local_port_reachable_false_for_closed_port` saw a closed port
/// answer under load). [`ClosedPort::new`] reserves `127.0.0.1:<port>`;
/// [`ClosedPort::dual`] also reserves `[::1]:<port>` when the host has IPv6
/// loopback, for tests of code that tries both.
///
/// How it is held: the port is the LOCAL end of a loopback connection the
/// guard owns (a socket bound to it and connected to a private listener),
/// not a bound-but-idle socket. An idle bound socket is a wildcard match
/// for an incoming SYN, and older macOS kernels (the Mac minis, 2026-10)
/// silently DROP a SYN that matches a socket in the CLOSED state, so the
/// connect hangs until its timeout instead of being refused (newer macOS
/// and Linux send a reset). A connected socket only matches its own peer,
/// so a SYN from anyone else finds no socket at all and every kernel
/// answers with a reset: refused at once, deterministically.
///
/// Reserved: the port comes from [`RESERVED_PORTS`], below every default
/// ephemeral range (macOS 49152+, Linux 32768+), so no `bind(…:0)` in any
/// test is ever handed it, and the held socket refuses a plain bind
/// (`EADDRINUSE`). Linux also refuses a `SO_REUSEADDR` bind (Rust's
/// `TcpListener::bind`) while the connection lives; BSD lets a
/// `SO_REUSEADDR` bind of that exact port through, which only a test that
/// names a port in that range on purpose could do.
#[cfg(unix)]
pub struct ClosedPort {
    port: u16,
    _fds: Vec<std::os::fd::OwnedFd>,
    _peers: Vec<std::net::TcpStream>,
}

#[cfg(unix)]
impl ClosedPort {
    /// Reserve a closed `127.0.0.1` port.
    pub fn new() -> Self {
        let (fd, peer, port) = reserve(false, 0).unwrap_or_else(|e| panic!("reserve 127.0.0.1:0: {e}"));
        Self { port, _fds: vec![fd], _peers: vec![peer] }
    }

    /// Reserve the same closed port on `127.0.0.1` and (when IPv6
    /// loopback exists) `::1`.
    pub fn dual() -> Self {
        for _ in 0..64 {
            let (v4, p4, port) = reserve(false, 0).unwrap_or_else(|e| panic!("reserve 127.0.0.1:0: {e}"));
            match reserve(true, port) {
                Ok((v6, p6, _)) => return Self { port, _fds: vec![v4, v6], _peers: vec![p4, p6] },
                // No IPv6 loopback on this host: nothing can listen there.
                Err(e) if e.raw_os_error() == Some(libc::EAFNOSUPPORT)
                    || e.raw_os_error() == Some(libc::EADDRNOTAVAIL) =>
                {
                    return Self { port, _fds: vec![v4], _peers: vec![p4] }
                }
                // Someone holds [::1]:port; pick another port.
                Err(_) => continue,
            }
        }
        panic!("could not reserve one closed port on both loopbacks");
    }

    /// The reserved port.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Where [`ClosedPort`] takes its ports: a range no K2 product or test port
/// uses (those are 5432, 8443, 8470, 8730, 384xx, 38600-38999), below
/// every default ephemeral range.
pub const RESERVED_PORTS: std::ops::Range<u16> = 21000..32000;

/// Windows [`ClosedPort`]: a socket bound to the port and never listening.
/// Windows answers a SYN to a bound, non-listening port with a reset (the
/// connect fails `ConnectionRefused`), and a second plain bind of it fails
/// (`WSAEADDRINUSE`; Rust's `TcpListener::bind` sets no `SO_REUSEADDR`
/// there). Hyper-V/WSL excluded port ranges answer `WSAEACCES`: skipped.
#[cfg(windows)]
pub struct ClosedPort {
    port: u16,
    _socks: Vec<socket2::Socket>,
}

#[cfg(windows)]
impl ClosedPort {
    /// Reserve a closed `127.0.0.1` port.
    pub fn new() -> Self {
        let (s, port) = reserve_win(false, 0).unwrap_or_else(|e| panic!("reserve 127.0.0.1:0: {e}"));
        Self { port, _socks: vec![s] }
    }

    /// Reserve the same closed port on `127.0.0.1` and (when IPv6
    /// loopback exists) `::1`.
    pub fn dual() -> Self {
        for _ in 0..64 {
            let (v4, port) = reserve_win(false, 0).unwrap_or_else(|e| panic!("reserve 127.0.0.1:0: {e}"));
            match reserve_win(true, port) {
                Ok((v6, _)) => return Self { port, _socks: vec![v4, v6] },
                Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => return Self { port, _socks: vec![v4] },
                Err(_) => continue,
            }
        }
        panic!("could not reserve one closed port on both loopbacks");
    }

    /// The reserved port.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// A loopback socket bound to `port` (0 = the next free one in
/// [`RESERVED_PORTS`]), not listening.
#[cfg(windows)]
fn reserve_win(v6: bool, port: u16) -> std::io::Result<(socket2::Socket, u16)> {
    let bind_at = |p: u16| -> std::io::Result<socket2::Socket> {
        let (domain, addr) = if v6 {
            (socket2::Domain::IPV6, std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, p)))
        } else {
            (socket2::Domain::IPV4, std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, p)))
        };
        let s = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
        s.bind(&addr.into())?;
        Ok(s)
    };
    if port != 0 {
        return bind_at(port).map(|s| (s, port));
    }
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let span = (RESERVED_PORTS.end - RESERVED_PORTS.start) as u32;
    let base = std::process::id().wrapping_mul(2654435761) % span;
    let mut last = None;
    for _ in 0..512 {
        let p = RESERVED_PORTS.start + ((base + NEXT.fetch_add(1, Ordering::Relaxed)) % span) as u16;
        match bind_at(p) {
            Ok(s) => return Ok((s, p)),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free port in the reserved range")))
}

/// A TCP socket bound to loopback `port` (0 = the next free one in
/// [`RESERVED_PORTS`]) and connected to a private loopback listener, plus
/// that connection's accepted end; see [`ClosedPort`] for why.
#[cfg(unix)]
fn reserve(v6: bool, port: u16) -> std::io::Result<(std::os::fd::OwnedFd, std::net::TcpStream, u16)> {
    if port != 0 {
        return reserve_at(v6, port);
    }
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let span = (RESERVED_PORTS.end - RESERVED_PORTS.start) as u32;
    // Spread parallel test binaries over the range (pid), then walk it.
    let base = std::process::id().wrapping_mul(2654435761) % span;
    let mut last = None;
    for _ in 0..512 {
        let p = RESERVED_PORTS.start + ((base + NEXT.fetch_add(1, Ordering::Relaxed)) % span) as u16;
        match reserve_at(v6, p) {
            Ok(r) => return Ok(r),
            Err(e) if e.raw_os_error() == Some(libc::EADDRINUSE) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free port in the reserved range")))
}

/// [`reserve`] at exactly `port`.
#[cfg(unix)]
fn reserve_at(v6: bool, port: u16) -> std::io::Result<(std::os::fd::OwnedFd, std::net::TcpStream, u16)> {
    use std::os::fd::AsRawFd;
    // The private peer: bound first so its own port is never `port`.
    let peer_listener = if v6 {
        std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0))?
    } else {
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?
    };
    let peer_port = peer_listener.local_addr()?.port();
    let (fd, mut storage, len) = bind_plain(v6, port)?;
    // SAFETY: `storage` is the sockaddr `bind_plain` filled for this family.
    unsafe {
        // Connect to the private peer (same family, loopback): the socket
        // then matches only that peer, never a SYN from anyone else.
        if v6 {
            let a = &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in6);
            a.sin6_port = peer_port.to_be();
        } else {
            let a = &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in);
            a.sin_port = peer_port.to_be();
        }
        if libc::connect(fd.as_raw_fd(), &storage as *const _ as *const libc::sockaddr, len) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let (peer, _) = peer_listener.accept()?;
    Ok((fd, peer, port))
}

/// A loopback socket bound to exactly `port` with no options (so a bind
/// conflict is `EADDRINUSE`). Also the test's "is it still taken" probe.
#[cfg(unix)]
fn bind_plain(v6: bool, port: u16) -> std::io::Result<(std::os::fd::OwnedFd, libc::sockaddr_storage, libc::socklen_t)> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let family = if v6 { libc::AF_INET6 } else { libc::AF_INET };
    // SAFETY: plain socket syscalls on a fresh fd we own; every pointer
    // passed is to a correctly sized, zero-initialized sockaddr.
    unsafe {
        let raw = libc::socket(family, libc::SOCK_STREAM, 0);
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(raw);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        let mut storage: libc::sockaddr_storage = std::mem::zeroed();
        let len = if v6 {
            let a = &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in6);
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                a.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
            }
            a.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            a.sin6_port = port.to_be();
            a.sin6_addr.s6_addr = std::net::Ipv6Addr::LOCALHOST.octets();
            std::mem::size_of::<libc::sockaddr_in6>()
        } else {
            let a = &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in);
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                a.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            }
            a.sin_family = libc::AF_INET as libc::sa_family_t;
            a.sin_port = port.to_be();
            a.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
            std::mem::size_of::<libc::sockaddr_in>()
        } as libc::socklen_t;
        if libc::bind(fd.as_raw_fd(), &storage as *const _ as *const libc::sockaddr, len) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((fd, storage, len))
    }
}

/// Per-test-daemon runtime isolation (2026-10-14 incident): a spawned
/// `k2-daemon` with a temp `$HOME` but the inherited `TMPDIR` /
/// `XDG_RUNTIME_DIR` resolves the REAL private runtime dir, so its tunnel
/// paths overwrite/delete the production `frpc.toml` and its stop path
/// `pkill`s the production frpc by config path.
///
/// `DaemonRuntime::new(root)` creates `<root>/tmp` and `<root>/run`
/// (0700); pass [`DaemonRuntime::envs`] to the daemon `Command`. On drop it
/// kills any frpc (real or fake) still running with a config under `root`
/// — declare it BEFORE the daemon child so the child is killed first.
#[must_use = "the frpc reap runs when the guard drops"]
pub struct DaemonRuntime {
    root: PathBuf,
}

impl DaemonRuntime {
    pub fn new(root: &Path) -> Self {
        for d in ["tmp", "run"] {
            let p = root.join(d);
            crate::private_fs::create_private_dir_all(&p).unwrap_or_else(|e| panic!("create {}: {e}", p.display()));
        }
        Self { root: root.to_path_buf() }
    }

    /// `TMPDIR`, `XDG_RUNTIME_DIR` under the root, and an empty
    /// `K2_RUNTIME_DIR` and `RUNTIME_DIRECTORY` (so an override, or the
    /// daemon unit's `/run/k2` an agent inherits, cannot win).
    pub fn envs(&self) -> Vec<(&'static str, OsString)> {
        #[allow(unused_mut)]
        let mut v = vec![
            ("TMPDIR", self.root.join("tmp").into_os_string()),
            ("XDG_RUNTIME_DIR", self.root.join("run").into_os_string()),
            (crate::runtime_dir::RUNTIME_DIR_ENV, OsString::new()),
            (crate::runtime_dir::SYSTEMD_RUNTIME_DIRECTORY_ENV, OsString::new()),
        ];
        // Windows' per-user temp dir (std::env::temp_dir) comes from TMP,
        // then TEMP: both point under the root so the runtime and secrets
        // dirs are the test daemon's own.
        #[cfg(windows)]
        {
            v.push(("TMP", self.root.join("tmp").into_os_string()));
            v.push(("TEMP", self.root.join("tmp").into_os_string()));
        }
        v
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for DaemonRuntime {
    fn drop(&mut self) {
        reap_frpc_under(&self.root);
    }
}

/// Windows: stop every process whose command line runs frpc with a config
/// under `root` (a test daemon's frpc, or a fake one), via PowerShell's
/// CIM process list.
#[cfg(windows)]
pub fn reap_frpc_under(root: &Path) {
    let r = root.display().to_string().replace('\'', "''");
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*frpc*-c*' -and $_.CommandLine -like '*{r}*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// SIGKILL every process launched as `… frpc -c <root>/…` (a test daemon's
/// supervised frpc, or a fake one). Matches only configs under `root`.
#[cfg(unix)]
pub fn reap_frpc_under(root: &Path) {
    let mut roots = vec![root.to_path_buf()];
    if let Ok(c) = dunce::canonicalize(&root) {
        if c != root {
            roots.push(c);
        }
    }
    for r in roots {
        let _ = std::process::Command::new("pkill")
            .arg("-9")
            .arg("-f")
            .arg(format!("frpc -c {}/", r.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Make a symlink in a test, on any OS. On Windows the link is a directory
/// link when `target` (resolved against the link's folder when relative)
/// is a directory, else a file link; creating one needs the symlink
/// privilege (an administrator, or Developer Mode), and without it this
/// fails loudly rather than letting the test skip.
pub fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        let (target, link) = (target.as_ref(), link.as_ref());
        // A relative target keeps its '/' verbatim in the reparse data, and
        // Windows resolves only '\' there.
        let target_buf = if target.is_relative() {
            PathBuf::from(target.to_string_lossy().replace('/', "\\"))
        } else {
            target.to_path_buf()
        };
        let target = target_buf.as_path();
        let resolved = match link.parent() {
            Some(dir) if target.is_relative() => dir.join(target),
            _ => target.to_path_buf(),
        };
        let r = if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        r.map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("symlink {} -> {}: {e} (Windows needs an administrator or Developer Mode)", link.display(), target.display()),
            )
        })
    }
}

/// The Python 3 command a test runs: `python3` on Unix; `python` on
/// Windows, where `python3` is usually the Microsoft Store stub.
pub const PYTHON: &str = if cfg!(windows) { "python" } else { "python3" };

/// A system program that runs about `secs` seconds and exits 0, as
/// `(program, args)`: `sleep N` on Unix; on Windows System32's `PING.EXE
/// -n N+1 127.0.0.1` (one echo a second; Windows has no `sleep`), which the
/// temp-HOME spawn belt allows as it allows /bin/sleep.
pub fn sleep_command(secs: u32) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let ping = root.join("System32").join("PING.EXE");
        (ping.display().to_string(), vec!["-n".into(), (secs + 1).to_string(), "127.0.0.1".into()])
    }
    #[cfg(not(windows))]
    {
        ("sleep".into(), vec![secs.to_string()])
    }
}

/// Run `f` with a fresh temp `$HOME` (see [`TempHome`]).
pub fn with_temp_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let home = TempHome::new();
    f(home.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_guard_restores_previous_value_lifo() {
        const KEY: &str = "K2_TEST_ENV_GUARD_PROBE";
        let _l = lock();
        assert_eq!(std::env::var_os(KEY), None);
        {
            let _a = EnvVar::set(KEY, "a");
            assert_eq!(std::env::var(KEY).unwrap(), "a");
            {
                let _b = EnvVar::set(KEY, "b");
                assert_eq!(std::env::var(KEY).unwrap(), "b");
                {
                    let _c = EnvVar::remove(KEY);
                    assert_eq!(std::env::var_os(KEY), None);
                }
                assert_eq!(std::env::var(KEY).unwrap(), "b");
            }
            assert_eq!(std::env::var(KEY).unwrap(), "a");
        }
        assert_eq!(std::env::var_os(KEY), None);
    }

    /// Soak 2026-10-27 (k2-daemon seed 5): a TempHome made while another
    /// test held the env lock with its own TMPDIR took that TMPDIR, so its
    /// HOME ended up outside the temp dir once TMPDIR was restored. The
    /// path is made under the lock now.
    #[test]
    fn temp_home_never_takes_another_tests_tmpdir() {
        let held = lock();
        let other = unique_temp_path("other-tmpdir");
        std::fs::create_dir_all(&other).expect("mkdir");
        let tmpdir = EnvVar::set("TMPDIR", &other);
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            let h = TempHome::new();
            tx.send((h.path().to_path_buf(), std::env::temp_dir())).expect("send");
        });
        // Let the other thread reach TempHome::new while TMPDIR is ours.
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(tmpdir);
        drop(held);
        let (home, tmp_then) = rx.recv().expect("the other thread's home");
        t.join().expect("join");
        // Not remove_dir_all: while TMPDIR pointed here, a test reading
        // temp_dir() without the lock may have put its files in this folder.
        // They aren't ours to delete; an empty folder is removed.
        let _ = std::fs::remove_dir(&other);
        assert!(!home.starts_with(&other), "took another test's TMPDIR: {}", home.display());
        assert!(home.starts_with(&tmp_then), "{} is outside the temp dir {}", home.display(), tmp_then.display());
    }

    /// A plain PTY spawn waits out another test's shim dir instead of being
    /// refused by it.
    #[cfg(unix)]
    #[test]
    fn a_plain_pty_spawn_waits_out_another_tests_shim_dir() {
        use crate::terminal::agent_spawn_guard::SHIM_DIR_ENV;
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _shim = EnvVar::set(SHIM_DIR_ENV, "/nonexistent/k2-no-shims-here");
            held_tx.send(()).expect("send");
            std::thread::sleep(std::time::Duration::from_millis(300));
        });
        held_rx.recv().expect("the holder has the shim dir set");
        let cfg = crate::terminal::DaemonPtyConfig {
            program: Some("cat".to_string()),
            cwd: Some(stable_temp_dir()),
            ..Default::default()
        };
        let s = spawn_plain_pty(cfg).unwrap_or_else(|e| panic!("refused by another test's shim dir: {e}"));
        s.kill();
        holder.join().expect("holder");
    }

    /// Soak 2026-10-30 (k2-core seed 5): a scratch dir made while another
    /// test had its own TMPDIR landed in that test's folder and vanished
    /// with it. The stable dir never follows TMPDIR, and a thread without
    /// the lock gets it without waiting for the holder.
    #[test]
    fn stable_temp_dir_never_takes_another_tests_tmpdir() {
        let _held = lock();
        let real = std::env::temp_dir();
        let other = unique_temp_path("other-tmpdir");
        std::fs::create_dir_all(&other).expect("mkdir");
        let (got, unique) = {
            let _tmpdir = EnvVar::set("TMPDIR", &other);
            std::thread::spawn(|| (stable_temp_dir(), unique_temp_path("x")))
                .join()
                .expect("a thread without the env lock")
        };
        let _ = std::fs::remove_dir(&other);
        assert_eq!(got, real);
        assert!(unique.starts_with(&real), "{}", unique.display());
        assert!(!unique.starts_with(&other), "{}", unique.display());
    }

    #[test]
    fn temp_home_is_unique_restored_and_removed() {
        let _l = lock();
        let before = std::env::var_os("HOME");
        let dir;
        {
            let a = TempHome::new();
            dir = a.path().to_path_buf();
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(dir.as_os_str()));
            assert!(dir.join(".k2").is_dir());
            // Re-entrant: a nested guard on the same thread does not deadlock.
            let b = TempHome::new();
            assert_ne!(a.path(), b.path());
            drop(b);
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(dir.as_os_str()));
        }
        assert_eq!(std::env::var_os("HOME"), before);
        assert!(!dir.exists(), "temp home removed on drop");
    }

    #[test]
    fn lock_survives_a_panicking_holder() {
        let t = std::thread::spawn(|| {
            let _g = lock();
            panic!("holder panics");
        });
        assert!(t.join().is_err());
        // Not poisoned: the next taker proceeds.
        let _g = lock();
    }

    /// A holder starts from a fresh process's switches whatever an earlier
    /// test left, a nested take changes nothing, and the release puts back
    /// what the outermost take found (soak 2026-10-25: a settings test
    /// left Garden access on for every later test).
    #[test]
    fn the_outermost_lock_owns_the_process_switches() {
        // What a settings test used to leave behind.
        let leaked = Switches {
            garden_access: true,
            permission_cards: true,
            apps: true,
            apps_engine_heal: false,
            ..Switches::PRISTINE
        };
        let outer = lock();
        let found = outer.found.expect("the outermost take records what it found");
        assert_eq!(Switches::read(), Switches::PRISTINE, "a holder starts from a fresh process's switches");
        leaked.write();
        {
            let nested = lock();
            assert!(nested.found.is_none(), "a nested take owns nothing");
            assert_eq!(Switches::read(), leaked, "a nested take resets nothing");
        }
        assert_eq!(Switches::read(), leaked, "a nested release restores nothing");
        // The next holder (on another thread, waiting for this one) finds
        // what this take found, not the leak, and starts pristine itself.
        let next = std::thread::spawn(|| {
            let g = lock();
            (g.found.expect("outermost on its thread"), Switches::read())
        });
        drop(outer);
        let (next_found, next_seen) = next.join().expect("next holder");
        assert_eq!(next_found, found, "the outermost release put back what its take found");
        assert_eq!(next_seen, Switches::PRISTINE);
    }

    #[cfg(unix)]
    #[test]
    fn agent_shim_registers_and_unregisters() {
        use crate::terminal::agent_spawn_guard::{resolve_program, GuardEnv, SHIM_DIR_ENV};
        let _l = lock();
        let before = std::env::var_os(SHIM_DIR_ENV);
        let dir;
        {
            let shim = AgentShim::install();
            dir = shim.dir().to_path_buf();
            let resolved = resolve_program("claude", "/usr/bin", &GuardEnv::from_process())
                .expect("claude resolves to the shim");
            assert_eq!(resolved, dir.join("claude").to_string_lossy());
            let body = std::fs::read_to_string(dir.join("codex")).unwrap();
            assert_eq!(body, "#!/bin/sh\nexec cat\n");
        }
        assert_eq!(std::env::var_os(SHIM_DIR_ENV), before);
        assert!(!dir.exists());
    }

    #[cfg(windows)]
    #[test]
    fn closed_port_refuses_and_stays_reserved_on_windows() {
        for closed in [ClosedPort::new(), ClosedPort::dual()] {
            for _ in 0..3 {
                let err = std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], closed.port())),
                    std::time::Duration::from_secs(5),
                )
                .expect_err("nothing listens on a reserved port");
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused, "{err:?}");
            }
            assert!(std::net::TcpListener::bind(("127.0.0.1", closed.port())).is_err(), "port {} is held", closed.port());
            assert!(RESERVED_PORTS.contains(&closed.port()), "{}", closed.port());
        }
    }

    #[cfg(unix)]
    #[test]
    fn closed_port_refuses_at_once_and_stays_reserved() {
        // Refused, not timed out: the kernel answered with a reset (a
        // dropped SYN would end as TimedOut after the 5 s cap, and fail
        // the kind check). No timing assertion: the kind is the proof.
        for closed in [ClosedPort::new(), ClosedPort::dual()] {
            for _ in 0..3 {
                let err = std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], closed.port())),
                    std::time::Duration::from_secs(5),
                )
                .expect_err("nothing listens on a reserved port");
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused, "{err:?}");
            }
            // Reserved: a plain bind is refused while the guard lives …
            let taken = bind_plain(false, closed.port()).map(|_| ());
            assert_eq!(taken.as_ref().map_err(|e| e.raw_os_error()), Err(Some(libc::EADDRINUSE)), "port {}", closed.port());
            // … on Linux even a SO_REUSEADDR one (Rust's TcpListener) …
            #[cfg(target_os = "linux")]
            assert!(std::net::TcpListener::bind(("127.0.0.1", closed.port())).is_err());
            // … and no `bind(…:0)` is ever handed it: it sits below the
            // ephemeral range.
            assert!(RESERVED_PORTS.contains(&closed.port()), "{}", closed.port());
            assert!(RESERVED_PORTS.end <= ephemeral_first(), "the reserved range overlaps this host's ephemeral ports");
        }
        // IPv6 loopback too, where the host has it.
        let closed = ClosedPort::dual();
        if let Ok(probe) = std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)) {
            drop(probe);
            let err = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, closed.port())),
                std::time::Duration::from_secs(5),
            )
            .expect_err("nothing listens on a reserved port");
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused, "{err:?}");
            let taken = bind_plain(true, closed.port()).map(|_| ());
            assert_eq!(taken.as_ref().map_err(|e| e.raw_os_error()), Err(Some(libc::EADDRINUSE)));
        }
    }

    /// The first port this host hands out for `bind(…:0)`.
    #[cfg(unix)]
    fn ephemeral_first() -> u16 {
        #[cfg(target_os = "linux")]
        {
            let t = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range").expect("ip_local_port_range");
            t.split_whitespace().next().expect("low end").parse().expect("a port")
        }
        #[cfg(not(target_os = "linux"))]
        {
            let o = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "net.inet.ip.portrange.first"]).output().expect("sysctl");
            assert!(o.status.success(), "{o:?}");
            String::from_utf8_lossy(&o.stdout).trim().parse().expect("a port")
        }
    }

    #[test]
    fn error_http_server_answers_its_status_fast() {
        use std::io::{Read, Write};
        let srv = ErrorHttpServer::start(503);
        assert!(srv.url().starts_with("http://127.0.0.1:"));
        let t = std::time::Instant::now();
        let mut c = std::net::TcpStream::connect(srv.addr()).expect("connect");
        c.write_all(b"POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n").unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 503 "), "{out:?}");
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn env_mutation_counter_sees_calls_not_prose() {
        let src = "// std::env::set_var(\"HOME\", x) in a comment\n\
                   fn set_var(k: &str) {}\n\
                   std::env::set_var(\"HOME\", h);\n\
                   std::env::remove_var(\n    \"PATH\");\n\
                   env::set_var(\"K2_X\", \"1\");\n\
                   my_set_var(\"HOME\");\n";
        assert_eq!(count_env_mutations(src), EnvMutationCount { all: 3, protected: 2 });
    }

    /// RATCHET: no new raw env mutation in k2-core. HOME/PATH/SHELL change
    /// only through this module; other vars only in the files (and counts)
    /// listed in `K2_CORE_ENV_BASELINE`. Lower a count when you convert
    /// sites; never raise one.
    #[test]
    fn thread_id_lint_allows_only_identity_comparisons() {
        let cur = concat!("std::thread::current()", ".id()");
        assert!(!thread_id_line_names_something(&format!("if h.thread().id() == {cur} {{")));
        assert!(!thread_id_line_names_something(&format!("if {cur} != other.id() {{")));
        assert!(thread_id_line_names_something(&format!("let p = dir.join(format!(\"t-{{:?}}\", {cur}));")));
        assert!(thread_id_line_names_something(&format!("let id = {cur};")));
        assert!(thread_id_line_names_something(&format!("let n = format!(\"{{:?}}\", {cur}) == x;")));
        assert!(!thread_id_line_names_something("let p = unique_temp_path(\"x\");"));
    }

    #[test]
    fn no_thread_id_temp_names_in_k2_core() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let v = thread_id_name_violations(&src);
        assert!(v.is_empty(), "use test_env::unique_temp_path, not a ThreadId:\n{}", v.join("\n"));
    }

    #[test]
    fn no_new_raw_env_mutation_in_k2_core() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let violations = env_mutation_violations(&src, K2_CORE_ENV_BASELINE);
        assert!(
            violations.is_empty(),
            "raw env mutation over the ratchet:\n{}",
            violations.join("\n")
        );
    }
}

/// `(file, max raw calls, max HOME/PATH/SHELL calls)` for k2-core `src/`.
/// `test_env.rs` is the sanctioned home of the raw calls (its guards, plus
/// the counter test's sample text); `lib.rs` is production
/// (`enrich_path_from_login_shell` adopts the login-shell PATH).
#[cfg(test)]
const K2_CORE_ENV_BASELINE: &[(&str, usize, usize)] = &[
    ("lib.rs", 1, 1),
    ("test_env.rs", 13, 3),
];

// ── Loopback ports for tests that can't keep their listener (0.46.3) ──

/// First port of the test range. Below every OS's ephemeral range (Linux
/// 32768–60999, macOS/BSD and Windows 49152–65535), so no `bind(":0")` and
/// no outgoing connection anywhere in the process can be handed a port from
/// it while a test is between "pick" and "the child binds".
pub const TEST_PORT_FIRST: u16 = 20_000;
/// One past the last port of the test range.
pub const TEST_PORT_END: u16 = 32_000;

/// A loopback TCP port for a test that must name a port and let something
/// else (a child process, a service it starts) bind it later.
///
/// The old pattern — `bind("127.0.0.1:0")`, read the port, drop the
/// listener, bind it again later — loses that port under parallel load:
/// the OS hands the just-freed ephemeral port to another test's `bind(0)`
/// or to an outgoing connection, and the later bind (or a "port must be
/// free" check) fails ("bind clash: … already accepting connections").
///
/// This hands out each port at most once per process, from
/// [`TEST_PORT_FIRST`]..[`TEST_PORT_END`] (outside the ephemeral range),
/// starting at a random offset so separate test processes (nextest, two
/// test binaries) rarely meet, and checks the port is free right now.
/// Prefer keeping a listener bound and handing it over when the code under
/// test takes a listener; use this only when it takes a port number.
pub fn loopback_test_port() -> u16 {
    use std::sync::atomic::AtomicU32;
    use std::sync::OnceLock;
    static START: OnceLock<u32> = OnceLock::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let start = *START.get_or_init(|| (uuid::Uuid::new_v4().as_u128() % TEST_PORT_SPAN as u128) as u32);
    pick_test_port(start, &NEXT, test_port_free_now)
}

const TEST_PORT_SPAN: u32 = (TEST_PORT_END - TEST_PORT_FIRST) as u32;

/// Free on the IPv4 loopback and the wildcard address right now (one at a
/// time: on Linux the two binds conflict with each other).
fn test_port_free_now(port: u16) -> bool {
    let v4_free = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
    v4_free && std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

/// The allocator behind [`loopback_test_port`]: the next port from
/// `start` (via the shared counter `next`, so no port is handed out twice)
/// that `free` says is bindable at hand-out time.
fn pick_test_port(start: u32, next: &std::sync::atomic::AtomicU32, free: impl Fn(u16) -> bool) -> u16 {
    use std::sync::atomic::Ordering;
    for _ in 0..TEST_PORT_SPAN {
        let n = next.fetch_add(1, Ordering::SeqCst);
        let port = TEST_PORT_FIRST + ((start + n) % TEST_PORT_SPAN) as u16;
        if free(port) {
            return port;
        }
    }
    panic!("loopback_test_port: no free port in {TEST_PORT_FIRST}..{TEST_PORT_END}");
}

#[cfg(test)]
mod loopback_test_port_tests {
    use super::*;

    /// Every port is handed out once and in range. (Whether a port is still
    /// free some time after hand-out depends on every other process on the
    /// machine, so that is not asserted here; the hand-out check is, below.)
    #[test]
    fn ports_are_unique_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let p = loopback_test_port();
            assert!((TEST_PORT_FIRST..TEST_PORT_END).contains(&p), "{p} out of range");
            assert!(seen.insert(p), "port {p} handed out twice");
        }
    }

    /// The allocator skips a port that is taken (one this test holds) and
    /// hands out the next one only after the real bind check passed for it
    /// at hand-out time.
    #[test]
    fn a_taken_port_is_skipped_and_the_one_handed_out_was_bindable() {
        use std::sync::atomic::AtomicU32;
        // Hold a port in range ourselves (the first one free now).
        let (held_port, _held) = (TEST_PORT_FIRST..TEST_PORT_END)
            .find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok().map(|l| (p, l)))
            .expect("some port in the test range is free");
        let start = u32::from(held_port - TEST_PORT_FIRST);
        let checked = std::sync::Mutex::new(Vec::new());
        let next = AtomicU32::new(0);
        let got = pick_test_port(start, &next, |p| {
            let ok = test_port_free_now(p);
            checked.lock().unwrap().push((p, ok));
            ok
        });
        let checked = checked.into_inner().unwrap();
        assert_eq!(checked[0], (held_port, false), "the held port is checked first and found taken: {checked:?}");
        assert_ne!(got, held_port, "a taken port is never handed out");
        assert_eq!(checked.last(), Some(&(got, true)), "the handed-out port passed the bind check at hand-out: {checked:?}");
        assert!(checked[..checked.len() - 1].iter().all(|(_, ok)| !ok), "every port before it was taken: {checked:?}");
        // A fixed checker: deterministic order and skipping.
        let next = AtomicU32::new(0);
        assert_eq!(pick_test_port(0, &next, |p| p >= TEST_PORT_FIRST + 3), TEST_PORT_FIRST + 3);
        assert_eq!(pick_test_port(0, &next, |_| true), TEST_PORT_FIRST + 4, "the counter never hands a port out twice");
    }
}
