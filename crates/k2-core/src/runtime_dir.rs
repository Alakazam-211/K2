//! A private, memory-backed runtime directory for files that briefly hold
//! secrets (the rendered frpc config, K2 vault §10.4). Never `/tmp` or the
//! home folder. Order:
//!
//! 1. [`RUNTIME_DIR_ENV`] (`K2_RUNTIME_DIR`) names the directory outright
//!    (in-process tests point it under their temp `$HOME`).
//! 2. Linux under systemd: the unit's own `RuntimeDirectory=k2`, which
//!    systemd hands over as `$RUNTIME_DIRECTORY` (`/run/k2` for a system
//!    unit). Never taken by a test process (an agent inherits it from the
//!    daemon unit it runs in).
//! 3. Linux fallback chain ([`linux_fallback_chain`]): `$XDG_RUNTIME_DIR/k2`,
//!    else `/run/user/<uid>/k2`, else `/dev/shm/k2-<uid>`. macOS: the
//!    per-user `$TMPDIR/k2-runtime` (private, 0700).
//!
//! `None` when none is usable.
//!
//! **Why the unit's own dir (akzm, 2026-10-24).** A system unit running as
//! a normal user is not a logind session. With no linger and logind's
//! default `RemoveIPC=yes`, systemd-logind deletes `/run/user/<uid>` and
//! every `/dev/shm` file the user owns when that user's last login session
//! (SSH, su) ends: frpc.toml and a live agent's `$K2_SECRETS_DIR` vanished
//! under a running daemon. `RuntimeDirectory=` belongs to the unit, and
//! `RuntimeDirectoryPreserve=yes` keeps it across restarts (frpc and
//! k2-ptyd sessions outlive the daemon process). [`logind_risk`] is the
//! doctor / boot check for a box still on the fallback chain.
//!
//! **Test belt (2026-10-14 incident).** k2-core is linked WITHOUT
//! `cfg(test)` by integration tests and by every test daemon they spawn.
//! A test daemon with a temp `$HOME` but the inherited `TMPDIR` /
//! `XDG_RUNTIME_DIR` resolved the REAL user runtime dir, so its tunnel
//! release deleted the production `frpc.toml` (and its stop path
//! `pkill`ed the production frpc by config path); the production
//! supervisor then respawned frpc against a missing file for two hours.
//! A process that looks like a test ([`looks_like_test_process`]) is now
//! refused the real user runtime dir, loudly.

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Explicit runtime-dir override (absolute path; created 0700 if its
/// parent exists).
pub const RUNTIME_DIR_ENV: &str = "K2_RUNTIME_DIR";

/// The env marker every test daemon harness sets (the agent-spawn guard's
/// shim dir; see `terminal::agent_spawn_guard::SHIM_DIR_ENV`).
const TEST_MARKER_ENV: &str = "K2_TEST_AGENT_SHIM_DIR";

#[cfg(unix)]
fn ensure_private(dir: &Path) -> bool {
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || m.uid() != me {
                return false;
            }
            if m.permissions().mode() & 0o077 != 0 {
                return std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok();
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new().mode(0o700).create(dir).is_ok()
        }
        Err(_) => false,
    }
}

/// Windows: a real directory we own whose DACL is owner-only (the 0700 of
/// `k2_winsec`); a missing leaf is created that way.
#[cfg(windows)]
fn ensure_private(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || !k2_winsec::owned_by_current_user(dir).unwrap_or(false) {
                return false;
            }
            if !k2_winsec::is_owner_only(dir).unwrap_or(false) {
                return k2_winsec::set_owner_only(dir).is_ok();
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(dir).is_ok() && k2_winsec::set_owner_only(dir).is_ok()
        }
        Err(_) => false,
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|x| !x.is_empty()).map(PathBuf::from)
}

/// systemd's `$RUNTIME_DIRECTORY`: the unit's `RuntimeDirectory=` paths,
/// colon-separated.
pub const SYSTEMD_RUNTIME_DIRECTORY_ENV: &str = "RUNTIME_DIRECTORY";

/// The `RuntimeDirectory=` name the daemon units carry (`/run/k2` for a
/// system unit, `$XDG_RUNTIME_DIR/k2` for a user unit).
pub const UNIT_RUNTIME_DIRECTORY: &str = "k2";

/// K2's entry in a `$RUNTIME_DIRECTORY` value: the absolute path named
/// [`UNIT_RUNTIME_DIRECTORY`] when the unit lists several, else the first.
pub fn parse_systemd_runtime_directory(raw: &str) -> Option<PathBuf> {
    let paths: Vec<PathBuf> = raw.split(':').filter(|p| p.starts_with('/')).map(PathBuf::from).collect();
    let named = paths.iter().find(|p| p.file_name().is_some_and(|n| n == UNIT_RUNTIME_DIRECTORY));
    named.or(paths.first()).cloned()
}

/// The unit's runtime dir from the environment, if systemd set one.
pub fn systemd_runtime_directory() -> Option<PathBuf> {
    std::env::var(SYSTEMD_RUNTIME_DIRECTORY_ENV).ok().and_then(|r| parse_systemd_runtime_directory(&r))
}

/// The pre-0.46.4 Linux chain, still the fallback for dev runs and units
/// without `RuntimeDirectory=`: `$XDG_RUNTIME_DIR/k2`,
/// `/run/user/<uid>/k2`, `/dev/shm/k2-<uid>`.
pub fn linux_fallback_chain(xdg_runtime_dir: Option<&Path>, uid: u32) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(x) = xdg_runtime_dir {
        v.push(x.join("k2"));
    }
    v.push(PathBuf::from(format!("/run/user/{uid}/k2")));
    v.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    v
}

/// Pure Linux candidate order (see the module doc): the override alone,
/// else the unit's runtime dir (never for a test process), then the
/// fallback chain.
pub fn linux_candidates(
    override_dir: Option<&Path>,
    systemd_dir: Option<&Path>,
    xdg_runtime_dir: Option<&Path>,
    uid: u32,
    is_test: bool,
) -> Vec<PathBuf> {
    if let Some(d) = override_dir {
        return vec![d.to_path_buf()];
    }
    let mut v = Vec::new();
    if let Some(s) = systemd_dir.filter(|_| !is_test) {
        v.push(s.to_path_buf());
    }
    v.extend(linux_fallback_chain(xdg_runtime_dir, uid));
    v
}

fn candidates(is_test: bool) -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        linux_candidates(
            env_path(RUNTIME_DIR_ENV).as_deref(),
            systemd_runtime_directory().as_deref(),
            env_path("XDG_RUNTIME_DIR").as_deref(),
            uid,
            is_test,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = is_test;
        if let Some(d) = env_path(RUNTIME_DIR_ENV) {
            return vec![d];
        }
        let mut candidates: Vec<PathBuf> = vec![];
        #[cfg(target_os = "macos")]
        {
            if let Some(t) = env_path("TMPDIR") {
                candidates.push(t.join("k2-runtime"));
            }
        }
        // Windows: the per-user temp dir (TMP / TEMP, under the profile's
        // AppData\Local), owner-only like macOS's $TMPDIR/k2-runtime.
        #[cfg(windows)]
        candidates.push(std::env::temp_dir().join("k2-runtime"));
        candidates
    }
}

/// Earlier homes of the runtime dir that `current` supersedes: the Linux
/// fallback chain minus `current` (a box moving onto the unit's `/run/k2`,
/// or one that fell from `/run/user/<uid>/k2` to `/dev/shm`). Empty under
/// the [`RUNTIME_DIR_ENV`] override, for a test process, and off Linux:
/// those never touch the real user dirs.
pub fn superseded_runtime_dirs(current: &Path) -> Vec<PathBuf> {
    if !cfg!(target_os = "linux") || env_path(RUNTIME_DIR_ENV).is_some() || looks_like_test_process() {
        return Vec::new();
    }
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        superseded_in(current, &linux_fallback_chain(env_path("XDG_RUNTIME_DIR").as_deref(), uid))
    }
    #[cfg(not(unix))]
    {
        let _ = current;
        Vec::new()
    }
}

/// Pure: `chain` minus `current`, de-duplicated, in order.
pub fn superseded_in(current: &Path, chain: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for c in chain {
        if c != current && !out.contains(c) {
            out.push(c.clone());
        }
    }
    out
}

/// Remove K2's own files from a superseded runtime dir, after their owner
/// (the tunnel) reaped whatever ran from it: exactly the names in `files`
/// plus their `<name>.tmp.<pid>` staging leftovers, regular files only (a
/// symlink is never followed or removed), and only when `dir` itself is a
/// real directory owned by us. The dir goes too when that leaves it empty
/// (`rmdir`, never recursive: a `secrets/` subdir keeps it). Returns what
/// was removed.
pub fn clean_superseded_dir(dir: &Path, files: &[&str]) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() && owned_by_me(dir, &m) => {}
        _ => return removed,
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return removed };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !files.iter().any(|f| name == *f || is_staging_leftover(&name, f)) {
            continue;
        }
        let p = e.path();
        let regular = std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_file() && owned_by_me(&p, &m));
        if regular && std::fs::remove_file(&p).is_ok() {
            removed.push(p);
        }
    }
    if std::fs::remove_dir(dir).is_ok() {
        removed.push(dir.to_path_buf());
    }
    removed
}

/// `m` (the metadata of `p`) belongs to this process's user.
#[cfg(unix)]
fn owned_by_me(_p: &Path, m: &std::fs::Metadata) -> bool {
    // SAFETY: geteuid has no preconditions.
    m.uid() == unsafe { libc::geteuid() }
}

#[cfg(windows)]
fn owned_by_me(p: &Path, _m: &std::fs::Metadata) -> bool {
    k2_winsec::owned_by_current_user(p).unwrap_or(false)
}

/// `frpc.toml.tmp.123` for `frpc.toml` (and `frpc.pid.tmp.123`).
fn is_staging_leftover(name: &str, file: &str) -> bool {
    name.strip_prefix(file)
        .and_then(|rest| rest.strip_prefix(".tmp."))
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
}

/// The private runtime dir, or `None` when no candidate is usable.
///
/// Panics (loudly, on purpose) when this process looks like a test and the
/// dir would be the real user runtime dir — see the module doc. A
/// production daemon never carries the test markers.
pub fn private_runtime_dir() -> Option<PathBuf> {
    match resolve_private_runtime_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            panic!("{e}");
        }
    }
}

/// [`private_runtime_dir`] without the panic: `Err` = refused (a test
/// process resolved the real user runtime dir).
pub fn resolve_private_runtime_dir() -> Result<Option<PathBuf>, String> {
    let is_test = looks_like_test_process();
    let real = if is_test { real_runtime_dirs() } else { Vec::new() };
    for c in candidates(is_test) {
        if !c.parent().is_some_and(|p| p.is_dir()) {
            continue;
        }
        // Refuse BEFORE `ensure_private`, which would create or chmod the
        // real dir: a refused test must not touch it at all.
        check_not_real_under_test(&c, is_test, &real)?;
        if ensure_private(&c) {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// Pure refusal rule: a test process must never use one of `real` (the
/// user's live runtime dirs).
pub fn check_not_real_under_test(dir: &Path, is_test: bool, real: &[PathBuf]) -> Result<(), String> {
    if !is_test {
        return Ok(());
    }
    // Canonical parent + leaf name: the dir itself may not exist yet, and
    // macOS spells the temp dir both /var/folders and /private/var/folders.
    let canon = |p: &Path| match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => dunce::canonicalize(&parent)
            .map(|c| c.join(name))
            .unwrap_or_else(|_| p.to_path_buf()),
        _ => p.to_path_buf(),
    };
    let d = canon(dir);
    if real.iter().any(|r| canon(r) == d) {
        return Err(format!(
            "REFUSED: this looks like a test process ({TEST_MARKER_ENV} set, or $HOME is a temp dir) \
             but the K2 private runtime dir resolved to the REAL user runtime dir {}. A test daemon \
             there overwrites and deletes the production frpc.toml and kills the production frpc. \
             Give every test daemon its own TMPDIR and XDG_RUNTIME_DIR under its temp root \
             (k2_core::test_env::DaemonRuntime), or set {RUNTIME_DIR_ENV} for in-process tests \
             (k2_core::test_env::TempHome does).",
            dir.display()
        ));
    }
    Ok(())
}

/// The user's REAL runtime dirs (what the production daemon resolves).
pub fn real_runtime_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        v.push(PathBuf::from(format!("/run/user/{uid}/k2")));
        v.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
        // The system unit's RuntimeDirectory=k2, and the unit this process
        // inherited $RUNTIME_DIRECTORY from (an agent's test run).
        v.push(PathBuf::from("/run").join(UNIT_RUNTIME_DIRECTORY));
        if let Some(d) = systemd_runtime_directory() {
            if !v.contains(&d) {
                v.push(d);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = darwin_user_temp_dir() {
            v.push(t.join("k2-runtime"));
        }
    }
    #[cfg(windows)]
    {
        if let Some(t) = windows_user_temp_dir() {
            v.push(t.join("k2-runtime"));
        }
    }
    v
}

/// Windows per-user temp dir from the profile's Local AppData known folder
/// (`…\AppData\Local\Temp`) — the real one, whatever `TMP`/`TEMP` say.
#[cfg(windows)]
pub fn windows_user_temp_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("Temp"))
}

/// macOS per-user temp dir from `confstr(_CS_DARWIN_USER_TEMP_DIR)` — the
/// real one, whatever `$TMPDIR` says.
#[cfg(target_os = "macos")]
pub fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    let mut buf = vec![0 as libc::c_char; 1024];
    // SAFETY: buf is valid for buf.len() bytes; confstr NUL-terminates.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), buf.len()) };
    if n == 0 || n > buf.len() {
        return None;
    }
    // SAFETY: confstr wrote a NUL-terminated string into buf.
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

// ── logind check (akzm, 2026-10-24) ──────────────────────────────────

/// K2 dirs systemd-logind may delete under a running daemon, found by
/// [`logind_risk`] (the vault doctor and the boot WARN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogindRisk {
    pub user: String,
    pub uid: u32,
    /// The at-risk dirs, in the order given.
    pub dirs: Vec<PathBuf>,
    /// This daemon runs as a systemd unit (`$INVOCATION_ID`).
    pub under_systemd: bool,
}

impl LogindRisk {
    /// One paragraph: what goes, why, and the one-line fix.
    pub fn message(&self) -> String {
        let dirs: Vec<String> = self.dirs.iter().map(|d| d.display().to_string()).collect();
        let user = &self.user;
        let shm = self.dirs.iter().any(|d| d.starts_with("/dev/shm"));
        let only_shm = self.dirs.iter().all(|d| d.starts_with("/dev/shm"));
        let mut what = Vec::new();
        if !only_shm {
            what.push(format!("/run/user/{}", self.uid));
        }
        if shm {
            what.push(format!("{user}'s /dev/shm files (RemoveIPC isn't off)"));
        }
        let mut m = format!(
            "K2's runtime dir {} can vanish under the running daemon: {user} has no linger, so systemd-logind \
             deletes {} when {user}'s last login session (SSH, su) ends, taking frpc.toml and agents' secret \
             files with it. Fix: `sudo loginctl enable-linger {user}`",
            dirs.join(" and "),
            what.join(" and "),
        );
        if only_shm {
            m.push_str(", or a drop-in /etc/systemd/logind.conf.d/k2.conf with `[Login]` `RemoveIPC=no`");
        }
        m.push('.');
        if self.under_systemd {
            m.push_str(
                " Restarting the daemon's systemd unit also moves it to the unit's own /run/k2 once the unit \
                 carries RuntimeDirectory=k2 (K2 0.46.4).",
            );
        }
        m
    }
}

/// systemd's compiled-in `SYSTEM_UID_MAX`: logind's `RemoveIPC=` skips
/// root and system users.
const SYSTEM_UID_MAX: u32 = 999;

/// logind's config files under `root` (`/` live), in apply order: the main
/// file (first found of /etc, /run, /usr/local/lib, /usr/lib), then the
/// `logind.conf.d/*.conf` drop-ins sorted by file name, a name in an
/// earlier dir hiding the same name in a later one.
fn logind_conf_files(root: &Path) -> Vec<PathBuf> {
    let bases = ["etc/systemd", "run/systemd", "usr/local/lib/systemd", "usr/lib/systemd"];
    let mut out = Vec::new();
    if let Some(main) = bases.iter().map(|b| root.join(b).join("logind.conf")).find(|p| p.is_file()) {
        out.push(main);
    }
    let mut dropins: std::collections::BTreeMap<std::ffi::OsString, PathBuf> = Default::default();
    for b in bases {
        let Ok(rd) = std::fs::read_dir(root.join(b).join("logind.conf.d")) else { continue };
        for e in rd.flatten() {
            let name = e.file_name();
            if Path::new(&name).extension().is_some_and(|x| x == "conf") {
                dropins.entry(name).or_insert_with(|| e.path());
            }
        }
    }
    out.extend(dropins.into_values());
    out
}

/// logind's effective `RemoveIPC=` under `root`: the last `[Login]`
/// assignment across [`logind_conf_files`]; `yes` (systemd's default)
/// when none, or when the last one is empty or not a boolean.
pub fn logind_remove_ipc(root: &Path) -> bool {
    let mut on = true;
    for f in logind_conf_files(root) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = s.trim().to_string();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            if section != "Login" || k.trim() != "RemoveIPC" {
                continue;
            }
            on = match v.trim().to_ascii_lowercase().as_str() {
                "no" | "false" | "off" | "0" | "n" | "f" => false,
                _ => true,
            };
        }
    }
    on
}

/// Pure rule, with every input named (`root` holds the fake
/// `var/lib/systemd/linger/<user>` and logind config in tests):
/// - under `/run/user/<uid>`: at risk without linger (logind removes it
///   whatever `RemoveIPC=` says);
/// - under `/dev/shm`: at risk without linger when `RemoveIPC=` isn't
///   `no` and `uid` is not a system user;
/// - anything else (the unit's `/run/k2`, macOS): never.
pub fn logind_risk_at(root: &Path, dirs: &[PathBuf], uid: u32, user: &str, under_systemd: bool) -> Option<LogindRisk> {
    if root.join("var/lib/systemd/linger").join(user).exists() {
        return None;
    }
    let run_user = PathBuf::from(format!("/run/user/{uid}"));
    let mut remove_ipc: Option<bool> = None;
    let mut at_risk: Vec<PathBuf> = Vec::new();
    for d in dirs {
        let risky = if d.starts_with(&run_user) {
            true
        } else if d.starts_with("/dev/shm") {
            uid > SYSTEM_UID_MAX && *remove_ipc.get_or_insert_with(|| logind_remove_ipc(root))
        } else {
            false
        };
        if risky && !at_risk.contains(d) {
            at_risk.push(d.clone());
        }
    }
    (!at_risk.is_empty()).then(|| LogindRisk { user: user.to_string(), uid, dirs: at_risk, under_systemd })
}

/// Live check (Linux; `None` elsewhere): this daemon's runtime dir plus
/// `extra` (the vault's secrets root), against the real linger dir and
/// logind config. Resolves the runtime dir the way the tunnel does (so it
/// may create it); a test process, refused the real dir, checks only
/// `extra`.
pub fn logind_risk(extra: &[PathBuf]) -> Option<LogindRisk> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    #[cfg(unix)]
    {
        let mut dirs: Vec<PathBuf> = resolve_private_runtime_dir().ok().flatten().into_iter().collect();
        dirs.extend(extra.iter().cloned());
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let user = passwd_entry(uid).map(|(name, _)| name).filter(|n| !n.is_empty()).unwrap_or_else(|| uid.to_string());
        logind_risk_at(Path::new("/"), &dirs, uid, &user, std::env::var_os("INVOCATION_ID").is_some())
    }
    #[cfg(not(unix))]
    {
        let _ = extra;
        None
    }
}

/// Windows: the profile folder the shell reports (never `HOME`).
#[cfg(windows)]
fn passwd_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// The home the password database lists for this uid (never `$HOME`).
#[cfg(unix)]
fn passwd_home() -> Option<PathBuf> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    passwd_entry(uid).and_then(|(_, dir)| (!dir.as_os_str().is_empty()).then_some(dir))
}

/// `(name, home)` from the password database for `uid`.
#[cfg(unix)]
fn passwd_entry(uid: libc::uid_t) -> Option<(String, PathBuf)> {
    use std::ffi::CStr;
    let mut size = 4096usize;
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        // SAFETY: zeroed passwd is a valid out-param for getpwuid_r.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: all pointers are valid for the call.
        let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() || pwd.pw_name.is_null() {
            return None;
        }
        // SAFETY: pw_name and pw_dir are NUL-terminated strings inside buf.
        let name = unsafe { CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned();
        // SAFETY: as above.
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_string_lossy().into_owned();
        return Some((name, PathBuf::from(dir)));
    }
}

/// Temp roots a test `$HOME` lives under (never a production home).
fn temp_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        ["/tmp", "/private/tmp", "/var/tmp", "/dev/shm", "/var/folders", "/private/var/folders"]
            .iter()
            .map(PathBuf::from)
            .collect();
    if let Some(t) = env_path("TMPDIR") {
        v.push(t);
    }
    #[cfg(target_os = "macos")]
    if let Some(t) = darwin_user_temp_dir() {
        v.push(t);
    }
    #[cfg(windows)]
    {
        for k in ["TMP", "TEMP"] {
            if let Some(t) = env_path(k) {
                v.push(t);
            }
        }
        v.push(std::env::temp_dir());
        if let Some(t) = windows_user_temp_dir() {
            v.push(t);
        }
    }
    v
}

/// Pure test-process rule: the harness marker is set, or `$HOME` sits
/// under a temp root and is not the account's own (password-database)
/// home.
pub fn is_test_process(marker_set: bool, home: Option<&Path>, passwd: Option<&Path>, roots: &[PathBuf]) -> bool {
    if marker_set {
        return true;
    }
    let Some(home) = home else { return false };
    let canon = |p: &Path| dunce::canonicalize(&p).unwrap_or_else(|_| p.to_path_buf());
    let h = canon(home);
    if passwd.is_some_and(|p| canon(p) == h) {
        return false;
    }
    roots.iter().any(|r| h.starts_with(canon(r)) || h.starts_with(r))
}

/// Whether this process looks like a test (see [`is_test_process`]).
pub fn looks_like_test_process() -> bool {
    is_test_process(
        env_path(TEST_MARKER_ENV).is_some(),
        env_path("HOME").as_deref(),
        passwd_home().as_deref(),
        &temp_roots(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_process_is_refused_the_real_runtime_dir() {
        let root = crate::test_env::unique_temp_path("rt-belt");
        let real = root.join("real").join("k2-runtime");
        let mine = root.join("mine").join("k2-runtime");
        std::fs::create_dir_all(&real).expect("mkdir real");
        std::fs::create_dir_all(&mine).expect("mkdir mine");
        let reals = vec![real.clone()];
        let err = check_not_real_under_test(&real, true, &reals).expect_err("must refuse");
        assert!(err.contains("REFUSED"), "{err}");
        assert!(err.contains(&real.display().to_string()), "{err}");
        check_not_real_under_test(&mine, true, &reals).expect("own dir is fine");
        check_not_real_under_test(&real, false, &reals).expect("production uses the real dir");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn linux_order_override_then_unit_dir_then_the_old_chain() {
        let xdg = p("/run/user/1001");
        let unit = p("/run/k2");
        let chain = vec![p("/run/user/1001/k2"), p("/run/user/1001/k2"), p("/dev/shm/k2-1001")];
        // Override wins alone.
        assert_eq!(
            linux_candidates(Some(Path::new("/x/run")), Some(&unit), Some(&xdg), 1001, false),
            vec![p("/x/run")]
        );
        assert_eq!(linux_candidates(Some(Path::new("/x/run")), Some(&unit), None, 1001, true), vec![p("/x/run")]);
        // The unit's RuntimeDirectory= first, then the old chain.
        let mut want = vec![unit.clone()];
        want.extend(chain.clone());
        assert_eq!(linux_candidates(None, Some(&unit), Some(&xdg), 1001, false), want);
        // A test process never takes the unit's dir.
        assert_eq!(linux_candidates(None, Some(&unit), Some(&xdg), 1001, true), chain);
        // No unit dir, no XDG: the pre-0.46.4 fallbacks.
        assert_eq!(
            linux_candidates(None, None, None, 1001, false),
            vec![p("/run/user/1001/k2"), p("/dev/shm/k2-1001")]
        );
    }

    #[test]
    fn runtime_directory_env_names_k2s_dir() {
        assert_eq!(parse_systemd_runtime_directory("/run/k2"), Some(p("/run/k2")));
        assert_eq!(parse_systemd_runtime_directory("/run/other:/run/k2"), Some(p("/run/k2")));
        assert_eq!(parse_systemd_runtime_directory("/run/a:/run/b"), Some(p("/run/a")));
        assert_eq!(parse_systemd_runtime_directory("/run/user/1001/k2"), Some(p("/run/user/1001/k2")));
        assert_eq!(parse_systemd_runtime_directory(""), None);
        assert_eq!(parse_systemd_runtime_directory("k2"), None, "relative is never taken");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_candidates_follow_the_env() {
        let unit = crate::test_env::unique_temp_path("unit-rtd");
        let _o = crate::test_env::EnvVar::remove(RUNTIME_DIR_ENV);
        let _x = crate::test_env::EnvVar::set("XDG_RUNTIME_DIR", "/run/user/4242");
        let uid = unsafe { libc::geteuid() };
        let chain = linux_fallback_chain(Some(Path::new("/run/user/4242")), uid);
        {
            let _r = crate::test_env::EnvVar::remove(SYSTEMD_RUNTIME_DIRECTORY_ENV);
            assert_eq!(candidates(false), chain, "unset: the old chain");
        }
        {
            let _r = crate::test_env::EnvVar::set(SYSTEMD_RUNTIME_DIRECTORY_ENV, "");
            assert_eq!(candidates(false), chain, "empty = unset");
        }
        let _r = crate::test_env::EnvVar::set(SYSTEMD_RUNTIME_DIRECTORY_ENV, &unit);
        let mut want = vec![unit.clone()];
        want.extend(chain.clone());
        assert_eq!(candidates(false), want, "set: the unit's dir first");
        assert_eq!(candidates(true), chain, "a test process skips it");
        assert!(real_runtime_dirs().contains(&unit), "an inherited unit dir counts as real for the belt");
        assert!(real_runtime_dirs().contains(&p("/run/k2")));
        let _o2 = crate::test_env::EnvVar::set(RUNTIME_DIR_ENV, "/x/run");
        assert_eq!(candidates(false), vec![p("/x/run")], "override wins");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ignores_runtime_directory() {
        let _o = crate::test_env::EnvVar::remove(RUNTIME_DIR_ENV);
        let _r = crate::test_env::EnvVar::set(SYSTEMD_RUNTIME_DIRECTORY_ENV, "/run/k2");
        let t = env_path("TMPDIR").expect("macOS sets TMPDIR");
        assert_eq!(candidates(false), vec![t.join("k2-runtime")]);
    }

    #[test]
    fn superseded_dirs_are_the_chain_minus_the_current_one() {
        let chain = linux_fallback_chain(Some(Path::new("/run/user/1001")), 1001);
        assert_eq!(superseded_in(Path::new("/run/k2"), &chain), vec![p("/run/user/1001/k2"), p("/dev/shm/k2-1001")]);
        assert_eq!(superseded_in(Path::new("/run/user/1001/k2"), &chain), vec![p("/dev/shm/k2-1001")]);
        assert_eq!(superseded_in(Path::new("/dev/shm/k2-1001"), &chain), vec![p("/run/user/1001/k2")]);
    }

    #[test]
    fn superseded_cleanup_removes_only_k2_files_and_never_follows_links() {
        let root = crate::test_env::unique_temp_path("rt-clean");
        let old = root.join("old");
        std::fs::create_dir_all(old.join("secrets").join("sess-1")).expect("mkdir");
        let outside = root.join("outside.txt");
        std::fs::write(&outside, b"keep").expect("write");
        std::fs::write(old.join("frpc.toml"), b"token").expect("write");
        std::fs::write(old.join("frpc.toml.tmp.42"), b"token").expect("write");
        std::fs::write(old.join("frpc.toml.tmp.x"), b"not ours").expect("write");
        std::fs::write(old.join("notes.txt"), b"not ours").expect("write");
        crate::test_env::symlink(&outside, old.join("frpc.pid")).expect("symlink");
        let removed = clean_superseded_dir(&old, &["frpc.toml", "frpc.pid"]);
        assert_eq!(removed.len(), 2, "{removed:?}");
        assert!(!old.join("frpc.toml").exists() && !old.join("frpc.toml.tmp.42").exists());
        assert!(old.join("frpc.toml.tmp.x").exists() && old.join("notes.txt").exists());
        assert!(std::fs::symlink_metadata(old.join("frpc.pid")).expect("link").file_type().is_symlink());
        assert_eq!(std::fs::read(&outside).expect("outside"), b"keep", "the link's target is untouched");
        assert!(old.join("secrets").join("sess-1").is_dir(), "secrets are not this cleanup's");
        assert!(old.is_dir(), "a non-empty dir stays");

        // Only K2's files: the dir goes with them.
        let lone = root.join("lone");
        std::fs::create_dir_all(&lone).expect("mkdir");
        std::fs::write(lone.join("frpc.toml"), b"token").expect("write");
        assert_eq!(clean_superseded_dir(&lone, &["frpc.toml"]), vec![lone.join("frpc.toml"), lone.clone()]);
        assert!(!lone.exists());

        // A symlinked dir is never entered.
        let real = root.join("real");
        std::fs::create_dir_all(&real).expect("mkdir");
        std::fs::write(real.join("frpc.toml"), b"token").expect("write");
        let link = root.join("link");
        crate::test_env::symlink(&real, &link).expect("symlink");
        assert!(clean_superseded_dir(&link, &["frpc.toml"]).is_empty());
        assert!(real.join("frpc.toml").exists());
        assert!(clean_superseded_dir(&root.join("missing"), &["frpc.toml"]).is_empty());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    /// A fake `/` for the logind check: `linger` users and logind config.
    fn fake_root(linger: &[&str], files: &[(&str, &str)]) -> PathBuf {
        let root = crate::test_env::unique_temp_path("logind");
        std::fs::create_dir_all(root.join("var/lib/systemd/linger")).expect("mkdir");
        for u in linger {
            std::fs::write(root.join("var/lib/systemd/linger").join(u), b"").expect("linger");
        }
        for (rel, body) in files {
            let f = root.join(rel);
            std::fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
            std::fs::write(&f, body).expect("write");
        }
        root
    }

    #[test]
    fn logind_remove_ipc_reads_main_file_then_dropins_in_order() {
        let r = fake_root(&[], &[]);
        assert!(logind_remove_ipc(&r), "systemd's default is yes");
        let r = fake_root(&[], &[("etc/systemd/logind.conf", "[Login]\n#RemoveIPC=no\n")]);
        assert!(logind_remove_ipc(&r), "a comment is not a setting");
        let r = fake_root(&[], &[("etc/systemd/logind.conf", "[Login]\nRemoveIPC=no\n")]);
        assert!(!logind_remove_ipc(&r));
        let r = fake_root(&[], &[("etc/systemd/logind.conf", "[Other]\nRemoveIPC=no\n")]);
        assert!(logind_remove_ipc(&r), "only [Login] counts");
        let r = fake_root(&[], &[("usr/lib/systemd/logind.conf.d/50-k2.conf", "[Login]\nRemoveIPC = false\n")]);
        assert!(!logind_remove_ipc(&r), "a vendor drop-in");
        // Drop-ins apply after the main file, by file name; /etc hides a
        // same-named /usr/lib one.
        let r = fake_root(
            &[],
            &[
                ("etc/systemd/logind.conf", "[Login]\nRemoveIPC=no\n"),
                ("etc/systemd/logind.conf.d/90-late.conf", "[Login]\nRemoveIPC=yes\n"),
                ("run/systemd/logind.conf.d/10-early.conf", "[Login]\nRemoveIPC=no\n"),
            ],
        );
        assert!(logind_remove_ipc(&r), "90-late wins");
        let r = fake_root(
            &[],
            &[
                ("etc/systemd/logind.conf.d/k2.conf", "[Login]\nRemoveIPC=no\n"),
                ("usr/lib/systemd/logind.conf.d/k2.conf", "[Login]\nRemoveIPC=yes\n"),
            ],
        );
        assert!(!logind_remove_ipc(&r), "/etc hides /usr/lib");
        let r = fake_root(&[], &[("etc/systemd/logind.conf.d/k2.txt", "[Login]\nRemoveIPC=no\n")]);
        assert!(logind_remove_ipc(&r), "only *.conf drop-ins");
    }

    #[test]
    fn logind_risk_needs_no_linger_and_a_logind_owned_dir() {
        let dirs = |v: &[&str]| v.iter().map(|s| p(s)).collect::<Vec<_>>();
        let r = fake_root(&[], &[]);
        // The akzm case: both dirs, no linger, default RemoveIPC.
        let risk = logind_risk_at(&r, &dirs(&["/run/user/1001/k2", "/dev/shm/k2-1001/secrets"]), 1001, "k2", true)
            .expect("at risk");
        assert_eq!(risk.dirs, dirs(&["/run/user/1001/k2", "/dev/shm/k2-1001/secrets"]));
        let m = risk.message();
        assert!(m.contains("`sudo loginctl enable-linger k2`"), "{m}");
        assert!(m.contains("/run/user/1001 and k2's /dev/shm files"), "{m}");
        assert!(!m.contains("RemoveIPC=no"), "RemoveIPC=no would not save /run/user: {m}");
        assert!(m.contains("RuntimeDirectory=k2"), "{m}");
        // /dev/shm only: RemoveIPC=no is a fix too.
        let m = logind_risk_at(&r, &dirs(&["/dev/shm/k2-1001"]), 1001, "k2", false).expect("at risk").message();
        assert!(m.contains("/etc/systemd/logind.conf.d/k2.conf") && m.contains("`RemoveIPC=no`"), "{m}");
        assert!(!m.contains("RuntimeDirectory"), "not a systemd unit: {m}");
        // The unit's own /run/k2: never.
        assert_eq!(logind_risk_at(&r, &dirs(&["/run/k2", "/run/k2/secrets"]), 1001, "k2", true), None);
        // Another uid's /run/user is not this user's.
        assert_eq!(logind_risk_at(&r, &dirs(&["/run/user/1002/k2"]), 1001, "k2", true), None);
        // Linger on: nothing to fix.
        let lingered = fake_root(&["k2"], &[]);
        assert_eq!(logind_risk_at(&lingered, &dirs(&["/run/user/1001/k2", "/dev/shm/k2-1001"]), 1001, "k2", true), None);
        // RemoveIPC=no: /dev/shm is safe, /run/user still goes.
        let ipc_off = fake_root(&[], &[("etc/systemd/logind.conf.d/k2.conf", "[Login]\nRemoveIPC=no\n")]);
        assert_eq!(logind_risk_at(&ipc_off, &dirs(&["/dev/shm/k2-1001"]), 1001, "k2", true), None);
        let risk = logind_risk_at(&ipc_off, &dirs(&["/run/user/1001/k2", "/dev/shm/k2-1001"]), 1001, "k2", true)
            .expect("/run/user still at risk");
        assert_eq!(risk.dirs, dirs(&["/run/user/1001/k2"]));
        // A system user: RemoveIPC skips its /dev/shm.
        assert_eq!(logind_risk_at(&r, &dirs(&["/dev/shm/k2-998"]), 998, "k2", true), None);
    }

    #[test]
    fn test_process_detection() {
        let roots = vec![PathBuf::from("/tmp"), PathBuf::from("/var/folders")];
        let passwd = Path::new("/Users/someone");
        assert!(is_test_process(true, Some(passwd), Some(passwd), &roots), "marker wins");
        assert!(is_test_process(false, Some(Path::new("/tmp/k2t-abc")), Some(passwd), &roots));
        assert!(is_test_process(
            false,
            Some(Path::new("/var/folders/xx/T/k2-test-home-1")),
            Some(passwd),
            &roots
        ));
        assert!(!is_test_process(false, Some(passwd), Some(passwd), &roots), "the real home");
        assert!(!is_test_process(false, Some(Path::new("/home/k2")), Some(passwd), &roots));
        // A box whose real home IS under /tmp is not a test.
        assert!(!is_test_process(false, Some(Path::new("/tmp/h")), Some(Path::new("/tmp/h")), &roots));
        assert!(!is_test_process(false, None, Some(passwd), &roots));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_macos_runtime_dir_comes_from_confstr() {
        let t = darwin_user_temp_dir().expect("confstr user temp dir");
        assert!(t.is_dir(), "{}", t.display());
        assert_eq!(real_runtime_dirs(), vec![t.join("k2-runtime")]);
    }
}
