//! Handing a verb to the shell CLI.
//!
//! A verb the front doesn't handle runs in the shell CLI, with the
//! arguments exactly as they came (never re-encoded or rewritten) and three
//! variables added ([`CalledAs::fallthrough_env`]); nothing else changes,
//! and the shell finds its passport and token itself.
//!
//! Which copy of the script runs, first that applies:
//!
//! 1. The bundled copy, when the binary sits in a packaged macOS app:
//!    `cli_install`'s layout, `<X>.app/Contents/Resources/_up_/cli/k2` for
//!    an executable in `<X>.app/Contents/MacOS/`. It counts only when its
//!    BLAKE3 is this build's. Nowhere else does the layout name a copy,
//!    so elsewhere there is none.
//! 2. The cache, `<K2 home>/run/cli/k2-<first 12 of the BLAKE3>.sh`
//!    (folder 0700, file 0755). It is hashed again before every use and
//!    replaced when it doesn't match.
//! 3. Otherwise the copy packed into the binary is written to the cache
//!    (to a temporary name in the same folder, then renamed) and runs
//!    from there.
//!
//! The check is BLAKE3 ([`embedded::SCRIPT_BLAKE3`], from `build.rs`):
//! the whole file, every run. Nothing outside the binary reads that
//! digest or the cache file's name.
//!
//! A copy that doesn't match is never run. The copy is opened once, and
//! the open file is both what is hashed and what bash reads: on Unix bash
//! runs `/dev/fd/N`, a descriptor of that open file, so a file swapped in
//! at the path after the check is not what runs. On Windows (no
//! `/dev/fd`) bash still runs the path, a moment after the check.
//!
//! The bash: `/bin/bash` on macOS; elsewhere on Unix the first of
//! `/bin/bash` and `/usr/bin/bash` that is there. On Windows, Git Bash, by
//! the search of `k2.cmd` (`k2_client::win_cli::find_git_bash`).
//!
//! On Unix the front becomes bash (`exec`): the process id, the terminal,
//! signals and the process ancestry the passport check walks are bash's
//! own, and so is the exit code. bash's `$0` is `/dev/fd/N`, which says
//! nothing about the name or the folder, so the front passes both: the
//! name it was called by in `K5_CLI_CALLED_AS`, and in `K5_CLI_SCRIPT_DIR`
//! the folder where an install keeps the files shipped beside the CLI
//! ([`script_dir_for`]). The shell closes N first thing, so the commands
//! it starts don't inherit it; bash reads the script through its own
//! descriptor. On Windows bash runs as a child, `$0` is the path, and the
//! front exits with bash's code.

use crate::called_as::{CalledAs, SCRIPT_DIR_VAR};
use crate::embedded;
use crate::exit::Exit;
use crate::home::HomeError;
use crate::trace;
use flate2::read::GzDecoder;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A packed copy of the shell CLI: gzip bytes, and the BLAKE3 (lowercase
/// hex) of what they unpack to.
#[derive(Debug, Clone, Copy)]
pub struct Packed<'a> {
    pub gz: &'a [u8],
    pub blake3: &'a str,
}

/// The copy this binary carries.
pub fn embedded() -> Packed<'static> {
    Packed { gz: embedded::SCRIPT_GZ, blake3: embedded::SCRIPT_BLAKE3 }
}

impl Packed<'_> {
    /// The cache file's name: `k2-<first 12 of the BLAKE3>.sh`.
    pub fn cache_name(&self) -> String {
        format!("k2-{}.sh", &self.blake3[..12])
    }

    /// The script, unpacked, when it matches its BLAKE3.
    pub fn unpack(&self) -> Result<Vec<u8>, FrontError> {
        let mut out = Vec::new();
        GzDecoder::new(self.gz).read_to_end(&mut out).map_err(|e| FrontError::Packed(e.to_string()))?;
        let got = blake3_hex(&out);
        if got != self.blake3 {
            return Err(FrontError::Packed(format!("it unpacks to BLAKE3 {got}, not {}", self.blake3)));
        }
        Ok(out)
    }
}

/// The BLAKE3 of `bytes`, in lowercase hex.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The copy of the shell CLI that runs: its path, and the file opened at
/// that path whose bytes were hashed, wound back to the start. The open
/// file is what bash reads on Unix ([`inheritable_copy`]).
#[derive(Debug)]
pub struct Script {
    pub path: PathBuf,
    pub file: fs::File,
}

/// `path` opened, when it is a file whose BLAKE3 is `want`, wound back to
/// the start. With `follow_links` false a link is no match (on Unix it is
/// not even opened: `O_NOFOLLOW`). What is hashed is the open file, so it
/// is the same bytes whatever is put at `path` afterwards.
fn open_matching(path: &Path, want: &str, follow_links: bool) -> Option<fs::File> {
    // Looked at before opening, so a folder or a pipe is never opened.
    let meta = if follow_links { fs::metadata(path) } else { fs::symlink_metadata(path) };
    if !meta.ok()?.is_file() {
        return None;
    }
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if !follow_links {
            opts.custom_flags(libc::O_NOFOLLOW);
        }
    }
    let mut file = opts.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    trace::mark("open copy");
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file).ok()?;
    let matches = hasher.finalize().to_hex().as_str() == want;
    trace::mark("hash copy");
    if !matches {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    Some(file)
}

/// The front's own failures. Each is one line on stderr and exit 1;
/// Windows without Git Bash is exit 9009, as `k2.cmd` does.
#[derive(Debug)]
pub enum FrontError {
    /// The K2 home can't be found, so the cache has nowhere to go.
    Home(HomeError),
    /// The cache folder or file can't be made.
    Cache { path: PathBuf, err: io::Error },
    /// The cache file was written but doesn't read back as this build's.
    CacheMismatch { path: PathBuf },
    /// The packed copy doesn't unpack to its BLAKE3: a broken build.
    Packed(String),
    /// No bash in any of the places looked in.
    NoBash { looked: Vec<PathBuf> },
    /// Windows: no Git Bash. Carries the verb.
    NoGitBash { noun: String },
    /// bash could not be started.
    Run { bash: PathBuf, err: io::Error },
}

impl FrontError {
    /// The exit.
    pub fn exit(&self) -> Exit {
        match self {
            FrontError::NoGitBash { .. } => Exit::NoGitBash,
            _ => Exit::Error,
        }
    }

    /// The exit code.
    pub fn exit_code(&self) -> i32 {
        self.exit().code()
    }

    /// The line printed on stderr, for a front called as `who`.
    pub fn line(&self, who: CalledAs) -> String {
        match self {
            FrontError::NoGitBash { .. } => self.to_string(),
            _ => format!("{}: {self}", who.name()),
        }
    }
}

impl fmt::Display for FrontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrontError::Home(e) => write!(f, "can't find where to keep the shell CLI: {e}"),
            FrontError::Cache { path, err } => write!(f, "can't write the shell CLI to {}: {err}", path.display()),
            FrontError::CacheMismatch { path } => {
                write!(f, "the shell CLI written to {} doesn't match this build", path.display())
            }
            FrontError::Packed(why) => write!(f, "this build's copy of the shell CLI is broken: {why}"),
            FrontError::NoBash { looked } => {
                let looked: Vec<String> = looked.iter().map(|p| p.display().to_string()).collect();
                write!(f, "no bash to run the shell CLI (looked for {})", looked.join(", "))
            }
            FrontError::NoGitBash { noun } => write!(
                f,
                "k5 {noun} is not built into k5 on Windows yet and still needs Git Bash (Git for Windows): {}",
                k2_client::win_cli::GIT_FOR_WINDOWS_URL
            ),
            FrontError::Run { bash, err } => write!(f, "can't run {}: {err}", bash.display()),
        }
    }
}

impl std::error::Error for FrontError {}

/// The verb as the shell reads it: the first argument, `help` when there
/// is none or it is empty (`${1:-help}`).
pub fn noun(args: &[OsString]) -> String {
    match args.first() {
        Some(a) if !a.is_empty() => a.to_string_lossy().into_owned(),
        _ => "help".to_string(),
    }
}

/// The cache folder under the K2 home: `run/cli`.
pub fn cache_dir(k2_home: &Path) -> PathBuf {
    k2_home.join("run").join("cli")
}

/// The script to run, open: the bundled copy when it is this build's
/// (links followed), else the cache under the K2 home ([`cached`]). The
/// home is looked up only when the bundled copy doesn't do.
pub fn script(
    packed: &Packed<'_>,
    bundled: Option<&Path>,
    k2_home: impl FnOnce() -> Result<PathBuf, HomeError>,
) -> Result<Script, FrontError> {
    if let Some(copy) = bundled {
        if let Some(file) = open_matching(copy, packed.blake3, true) {
            return Ok(Script { path: copy.to_path_buf(), file });
        }
    }
    let home = k2_home().map_err(FrontError::Home)?;
    trace::mark("home");
    cached(packed, &cache_dir(&home))
}

/// The cache copy in `dir`, made private first, open. A plain file whose
/// BLAKE3 is this build's is kept; anything else (missing, changed, a
/// link, a folder) is replaced from the packed copy, which is checked
/// before it is written and read back after.
pub fn cached(packed: &Packed<'_>, dir: &Path) -> Result<Script, FrontError> {
    private_dir(dir)?;
    trace::mark("cache folder");
    let path = dir.join(packed.cache_name());
    if let Some(file) = open_matching(&path, packed.blake3, false) {
        return Ok(Script { path, file });
    }
    let bytes = packed.unpack()?;
    write_replacing(dir, &path, &bytes).map_err(|err| FrontError::Cache { path: path.clone(), err })?;
    trace::mark("unpack and write the cache");
    match open_matching(&path, packed.blake3, false) {
        Some(file) => Ok(Script { path, file }),
        None => Err(FrontError::CacheMismatch { path }),
    }
}

/// Makes `dir` (and what is missing above it) and leaves it a folder only
/// its owner can open (0700 on Unix).
fn private_dir(dir: &Path) -> Result<(), FrontError> {
    let fail = |err: io::Error| FrontError::Cache { path: dir.to_path_buf(), err };
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(fail)?;
        let meta = fs::symlink_metadata(dir).map_err(fail)?;
        if !meta.is_dir() {
            return Err(fail(io::Error::other("not a folder")));
        }
        if meta.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(fail)?;
        }
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir).map_err(fail)?;
    Ok(())
}

/// Writes `bytes` to a new temporary file in `dir` (0755 on Unix) and
/// renames it over `path`, so no one ever runs a half-written script.
fn write_replacing(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let result = write_new(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        // Best effort: the error that matters is the one returned.
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Writes `bytes` to `path`, which must not exist yet (0755 on Unix).
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o755);
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    {
        // The umask may have taken bits off the mode above.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    file.sync_all()
}

/// Where the bundled copy is for an executable at `exe`, by cli_install's
/// layout (a packaged macOS app), when it is there. `None` for a build in
/// `target/` or anything not in an app.
#[cfg(unix)]
pub fn bundled_place_for(exe: &Path) -> Option<PathBuf> {
    k2_client::cli_install::packaged_cli_for_exe(exe, k2_client::cli_install::CLI_NAME)
}

/// This process's executable, links resolved.
pub fn resolved_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(fs::canonicalize(&exe).unwrap_or(exe))
}

/// [`bundled_place_for`] `exe`; never on Windows.
pub fn bundled_place(exe: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        bundled_place_for(exe)
    }
    #[cfg(not(unix))]
    {
        let _ = exe;
        None
    }
}

/// The folder [`SCRIPT_DIR_VAR`] names for an executable at `exe` (links
/// already resolved): where an install keeps the files shipped beside the
/// CLI, `k2-approve` and `k2-widget-shot.mjs`.
///
/// In a packaged macOS app that is the bundled script's folder
/// (`Contents/Resources/_up_/cli`, where the app's resources put
/// `k2-widget-shot.mjs` beside `k2`), not `Contents/MacOS`. Anywhere else
/// it is the executable's own folder, links resolved: cli_install links
/// `k2` in a bin folder (`~/.local/bin`, `/usr/local/bin`) to the CLI and
/// puts nothing beside the link, and the shell has always looked beside
/// its own file with links followed. So a link's folder is never it.
pub fn script_dir_for(exe: &Path) -> Option<PathBuf> {
    match bundled_place(exe) {
        Some(bundled) => bundled.parent().map(Path::to_path_buf),
        None => exe.parent().map(Path::to_path_buf),
    }
}

/// The places bash is looked for on a Unix, in order.
pub fn unix_bash_places(macos: bool) -> &'static [&'static str] {
    if macos {
        &["/bin/bash"]
    } else {
        &["/bin/bash", "/usr/bin/bash"]
    }
}

/// The first of [`unix_bash_places`] that `is_file` accepts.
pub fn find_unix_bash(macos: bool, is_file: impl Fn(&Path) -> bool) -> Result<PathBuf, FrontError> {
    let places: Vec<PathBuf> = unix_bash_places(macos).iter().map(PathBuf::from).collect();
    match places.iter().find(|p| is_file(p)) {
        Some(p) => Ok(p.clone()),
        None => Err(FrontError::NoBash { looked: places }),
    }
}

/// The bash for this system.
pub fn interpreter(args: &[OsString]) -> Result<PathBuf, FrontError> {
    #[cfg(windows)]
    {
        k2_client::win_cli::find_git_bash(|name| std::env::var_os(name), |p| p.is_file())
            .ok_or_else(|| FrontError::NoGitBash { noun: noun(args) })
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        find_unix_bash(cfg!(target_os = "macos"), |p| p.is_file())
    }
}

/// The command that runs `script` (`/dev/fd/N` on Unix) in `bash` with
/// `args` as they came, and the front's variables for the shell: the three
/// of [`CalledAs::fallthrough_env`], and [`SCRIPT_DIR_VAR`] set to
/// `script_dir`, or taken away when there is none (so a value left by
/// another call is never read as this one's). The rest of the environment
/// is this process's own.
pub fn command(bash: &Path, script: &OsStr, args: &[OsString], who: CalledAs, script_dir: Option<&Path>) -> Command {
    let mut cmd = Command::new(bash);
    cmd.arg(script).args(args).envs(who.fallthrough_env());
    match script_dir {
        Some(dir) => cmd.env(SCRIPT_DIR_VAR, dir),
        None => cmd.env_remove(SCRIPT_DIR_VAR),
    };
    cmd
}

/// A copy of `file`'s descriptor that bash inherits: numbered 3 or above
/// (never stdin, stdout or stderr) and without close-on-exec, which
/// `F_DUPFD` clears on the copy. Every other descriptor the front opens,
/// `file` included, keeps close-on-exec, so this is the one bash gets.
/// The copy shares `file`'s place in the file, at the start.
#[cfg(unix)]
pub fn inheritable_copy(file: &fs::File) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: F_DUPFD on a descriptor `file` holds open; it takes and
    // returns plain integers and touches no memory.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just made by F_DUPFD, is open, and nothing else
    // owns it.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

/// The path bash opens for the descriptor `fd`: `/dev/fd/<fd>`. Linux and
/// macOS both have it, and bash 3.2 runs a script from it.
#[cfg(unix)]
pub fn dev_fd(fd: &std::os::fd::OwnedFd) -> OsString {
    use std::os::fd::AsRawFd;
    OsString::from(format!("/dev/fd/{}", fd.as_raw_fd()))
}

/// Hands `args` to the shell CLI. On Unix this process becomes bash, so
/// this returns only when that fails; on Windows bash runs as a child and
/// this process exits with its code. What comes back is the front's own
/// error.
pub fn run(who: CalledAs, args: &[OsString]) -> FrontError {
    let bash = match interpreter(args) {
        Ok(b) => b,
        Err(e) => return e,
    };
    trace::mark("find bash");
    let exe = resolved_exe();
    let bundled = exe.as_deref().and_then(bundled_place);
    trace::mark("exe and bundled place");
    let script = match script(&embedded(), bundled.as_deref(), crate::home::k2_home) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let dir = exe.as_deref().and_then(script_dir_for);
    trace::mark("script dir");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let fd = match inheritable_copy(&script.file) {
            Ok(fd) => fd,
            Err(err) => return FrontError::Run { bash, err },
        };
        let mut cmd = command(&bash, &dev_fd(&fd), args, who, dir.as_deref());
        trace::finish("descriptor and command");
        let err = cmd.exec();
        FrontError::Run { bash, err }
    }
    #[cfg(not(unix))]
    {
        // No /dev/fd: bash runs the path. The open file stays open until
        // bash is done.
        trace::finish("command");
        match command(&bash, script.path.as_os_str(), args, who, dir.as_deref()).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => FrontError::Run { bash, err },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    /// A fresh folder under the system temp folder. Never the K2 home.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k5-fallthrough-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if d.exists() {
            fs::remove_dir_all(&d).unwrap();
        }
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(bytes).unwrap();
        gz.finish().unwrap()
    }

    const SCRIPT: &[u8] = b"#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\necho real\n";

    struct Fixture {
        gz: Vec<u8>,
        blake: String,
    }

    impl Fixture {
        fn new(bytes: &[u8]) -> Fixture {
            Fixture { gz: gzip(bytes), blake: blake3_hex(bytes) }
        }
        fn packed(&self) -> Packed<'_> {
            Packed { gz: &self.gz, blake3: &self.blake }
        }
    }

    fn read_all(mut file: fs::File) -> Vec<u8> {
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        out
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_cache_name_is_the_hash_s_first_12() {
        let f = Fixture::new(SCRIPT);
        assert_eq!(f.packed().cache_name(), format!("k2-{}.sh", &f.blake[..12]));
        assert_eq!(cache_dir(Path::new("/h/.k2")), Path::new("/h/.k2/run/cli"));
    }

    /// The binary's own copy is this checkout's `cli/k2`, byte for byte.
    #[test]
    fn the_embedded_copy_is_cli_k2() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        let want = fs::read(&repo).unwrap_or_else(|e| panic!("read {}: {e}", repo.display()));
        let got = embedded().unpack().expect("the embedded copy unpacks");
        assert!(got == want, "the embedded copy differs from {}", repo.display());
        assert_eq!(embedded().blake3, blake3_hex(&want));
    }

    /// The hex is BLAKE3's own: the reference vectors for "" and "abc".
    #[test]
    fn the_digest_is_blake3() {
        assert_eq!(blake3_hex(b""), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
        assert_eq!(blake3_hex(b"abc"), "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
    }

    /// The digest built into the binary is the BLAKE3 of this checkout's
    /// `cli/k2`, worked out here; and a copy one byte off, at the start, in
    /// the middle or at the end, is never what runs: not from the cache
    /// (it is replaced with the real one) and not from a bundled copy.
    #[test]
    fn the_built_in_blake3_is_cli_k2_s_and_a_one_byte_change_is_refused() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        let real = fs::read(&repo).unwrap_or_else(|e| panic!("read {}: {e}", repo.display()));
        assert_eq!(embedded::SCRIPT_BLAKE3, blake3::hash(&real).to_hex().as_str());
        let packed = embedded();
        for at in [0, real.len() / 2, real.len() - 1] {
            let mut off = real.clone();
            off[at] ^= 0x01;
            assert_ne!(blake3_hex(&off), packed.blake3, "byte {at}");
            let dir = scratch("onebyte");
            let path = dir.join(packed.cache_name());
            fs::write(&path, &off).unwrap();
            let got = cached(&packed, &dir).unwrap();
            assert_eq!(got.path, path);
            assert!(read_all(got.file) == real, "byte {at}: the changed cache file was run");
            assert!(fs::read(&path).unwrap() == real, "byte {at}: the changed cache file was not replaced");
            let bundled = dir.join("bundled-k2");
            fs::write(&bundled, &off).unwrap();
            let home = dir.join("home");
            let got = script(&packed, Some(&bundled), || Ok(home.clone())).unwrap();
            assert_eq!(got.path, cache_dir(&home).join(packed.cache_name()), "byte {at}: the changed bundled copy was used");
            assert!(read_all(got.file) == real);
            assert!(fs::read(&bundled).unwrap() == off, "byte {at}: the bundled copy was touched");
            fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn a_first_run_writes_the_cache_private_and_runnable() {
        let home = scratch("first");
        let f = Fixture::new(SCRIPT);
        let got = script(&f.packed(), None, || Ok(home.clone())).expect("cached");
        let path = got.path.clone();
        assert_eq!(path, home.join("run/cli").join(f.packed().cache_name()));
        assert_eq!(fs::read(&path).unwrap(), SCRIPT);
        assert_eq!(read_all(got.file), SCRIPT, "the open file reads from the start");
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o755);
            assert_eq!(mode(&home.join("run/cli")), 0o700);
        }
        // Nothing else is left in the folder (no temporary file).
        let names: Vec<_> = fs::read_dir(home.join("run/cli")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![OsString::from(f.packed().cache_name())]);
        fs::remove_dir_all(&home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_matching_cache_is_kept_as_it_is() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch("keep");
        let f = Fixture::new(SCRIPT);
        let path = cached(&f.packed(), &dir).unwrap().path;
        let before = fs::metadata(&path).unwrap().ino();
        let again = cached(&f.packed(), &dir).unwrap();
        assert_eq!(again.path, path);
        assert_eq!(read_all(again.file), SCRIPT, "a kept cache file is read from the start");
        assert_eq!(fs::metadata(&path).unwrap().ino(), before, "a matching cache file is not rewritten");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_changed_cache_is_replaced_and_never_run() {
        let dir = scratch("tampered");
        let f = Fixture::new(SCRIPT);
        let path = dir.join(f.packed().cache_name());
        fs::write(&path, b"#!/bin/bash\necho PWNED\n").unwrap();
        let got = cached(&f.packed(), &dir).unwrap();
        assert_eq!(got.path, path);
        assert_eq!(read_all(got.file), SCRIPT);
        assert_eq!(fs::read(&path).unwrap(), SCRIPT);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_cache_is_replaced_and_its_target_left_alone() {
        let dir = scratch("link");
        let f = Fixture::new(SCRIPT);
        // Even a link to a matching copy: the cache holds plain files only.
        let elsewhere = dir.join("elsewhere.sh");
        fs::write(&elsewhere, SCRIPT).unwrap();
        let cache = dir.join("cli");
        fs::create_dir(&cache).unwrap();
        let path = cache.join(f.packed().cache_name());
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert_eq!(cached(&f.packed(), &cache).unwrap().path, path);
        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        assert_eq!(fs::read(&elsewhere).unwrap(), SCRIPT);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_open_cache_folder_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("open");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        cached(&Fixture::new(SCRIPT).packed(), &dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_where_the_file_goes_is_an_error() {
        let dir = scratch("folder");
        let f = Fixture::new(SCRIPT);
        fs::create_dir(dir.join(f.packed().cache_name())).unwrap();
        let err = cached(&f.packed(), &dir).expect_err("can't replace a folder");
        assert!(matches!(err, FrontError::Cache { .. }), "{err:?}");
        assert_eq!(err.exit_code(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A packed copy that doesn't unpack to its hash is never written,
    /// so it can't be run.
    #[test]
    fn a_packed_copy_that_does_not_match_is_never_written() {
        let dir = scratch("badpack");
        let f = Fixture::new(SCRIPT);
        let wrong = blake3_hex(b"something else");
        let bad = Packed { gz: &f.gz, blake3: &wrong };
        let err = cached(&bad, &dir).expect_err("a mismatch is an error");
        assert!(matches!(err, FrontError::Packed(_)), "{err:?}");
        assert!(!dir.join(bad.cache_name()).exists());
        let not_gzip = Packed { gz: b"not gzip", blake3: &f.blake };
        assert!(matches!(not_gzip.unpack(), Err(FrontError::Packed(_))));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bundled_copy_counts_only_when_it_is_this_build_s() {
        let dir = scratch("bundled");
        let f = Fixture::new(SCRIPT);
        let bundled = dir.join("k2");
        fs::write(&bundled, SCRIPT).unwrap();
        let home = dir.join("home");
        // Matching: used as it is, and the home is never looked up.
        let got = script(&f.packed(), Some(&bundled), || panic!("the home is not needed")).unwrap();
        assert_eq!(got.path, bundled);
        assert_eq!(read_all(got.file), SCRIPT);
        assert!(!home.exists());
        // Changed: the cache instead, and the bundled file is left alone.
        fs::write(&bundled, b"#!/bin/bash\necho old\n").unwrap();
        let got = script(&f.packed(), Some(&bundled), || Ok(home.clone())).unwrap().path;
        assert_eq!(got, cache_dir(&home).join(f.packed().cache_name()));
        assert_eq!(fs::read(&bundled).unwrap(), b"#!/bin/bash\necho old\n");
        // Missing: the cache.
        fs::remove_file(&bundled).unwrap();
        assert_eq!(script(&f.packed(), Some(&bundled), || Ok(home.clone())).unwrap().path, got);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_home_is_an_error_and_writes_nothing() {
        let f = Fixture::new(SCRIPT);
        let err = script(&f.packed(), None, || Err(HomeError::NoUserHome)).expect_err("no home");
        assert!(matches!(err, FrontError::Home(HomeError::NoUserHome)), "{err:?}");
        assert_eq!(
            err.line(CalledAs::K5Name),
            "k5: can't find where to keep the shell CLI: no home folder: set HOME"
        );
        assert_eq!(err.exit_code(), 1);
    }

    /// cli_install's layout: an executable in an app's `Contents/MacOS`
    /// finds `Contents/Resources/_up_/cli/k2`; a build in `target/` none.
    #[cfg(unix)]
    #[test]
    fn the_bundled_place_is_cli_install_s_layout() {
        let dir = scratch("app");
        let contents = dir.join("K2.app/Contents");
        fs::create_dir_all(contents.join("MacOS")).unwrap();
        let exe = contents.join("MacOS/k5-cli");
        fs::write(&exe, b"").unwrap();
        assert_eq!(bundled_place_for(&exe), None, "no script in the app yet");
        let cli = contents.join("Resources/_up_/cli");
        fs::create_dir_all(&cli).unwrap();
        fs::write(cli.join("k2"), SCRIPT).unwrap();
        assert_eq!(bundled_place_for(&exe), Some(cli.join("k2")));
        let target = dir.join("target/release");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("k2"), SCRIPT).unwrap();
        assert_eq!(bundled_place_for(&target.join("k5-cli")), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bash_is_looked_for_in_the_system_s_places() {
        let only = |have: &'static [&'static str]| move |p: &Path| have.iter().any(|h| Path::new(h) == p);
        assert_eq!(find_unix_bash(true, only(&["/bin/bash"])).unwrap(), Path::new("/bin/bash"));
        assert!(matches!(find_unix_bash(true, only(&["/usr/bin/bash"])), Err(FrontError::NoBash { .. })));
        assert_eq!(find_unix_bash(false, only(&["/bin/bash", "/usr/bin/bash"])).unwrap(), Path::new("/bin/bash"));
        assert_eq!(find_unix_bash(false, only(&["/usr/bin/bash"])).unwrap(), Path::new("/usr/bin/bash"));
        // Never one from PATH.
        let err = find_unix_bash(false, only(&["/usr/local/bin/bash"])).expect_err("no bash");
        assert_eq!(err.line(CalledAs::K2Name), "k2: no bash to run the shell CLI (looked for /bin/bash, /usr/bin/bash)");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn without_git_bash_windows_says_so_and_exits_9009() {
        let err = FrontError::NoGitBash { noun: noun(&[OsString::from("msg"), OsString::from("x")]) };
        let line = "k5 msg is not built into k5 on Windows yet and still needs Git Bash (Git for Windows): https://git-scm.com/download/win";
        assert_eq!(err.line(CalledAs::K2Name), line);
        assert_eq!(err.line(CalledAs::K5Name), line);
        assert_eq!(err.exit_code(), 9009);
        assert_eq!(noun(&[]), "help");
        assert_eq!(noun(&[OsString::new()]), "help");
    }

    #[test]
    fn the_command_passes_the_arguments_as_they_came() {
        let mut args = vec![OsString::from("msg"), OsString::from("a b"), OsString::from(""), OsString::from("--json")];
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            args.push(OsString::from_vec(b"caf\xe9 \xff\n".to_vec()));
        }
        let cmd = command(Path::new("/bin/bash"), OsStr::new("/dev/fd/4"), &args, CalledAs::K5Name, Some(Path::new("/opt/k 2")));
        assert_eq!(cmd.get_program(), "/bin/bash");
        let got: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        let mut want: Vec<&std::ffi::OsStr> = vec![std::ffi::OsStr::new("/dev/fd/4")];
        want.extend(args.iter().map(OsString::as_os_str));
        assert_eq!(got, want);
        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned())))
            .collect();
        let mut want: Vec<(String, Option<String>)> = CalledAs::K5Name
            .fallthrough_env()
            .iter()
            .map(|(k, v)| (k.to_string(), Some(v.to_string())))
            .collect();
        want.push((SCRIPT_DIR_VAR.to_string(), Some("/opt/k 2".to_string())));
        want.sort();
        let mut envs = envs;
        envs.sort();
        assert_eq!(envs, want, "only the four variables are added, nothing removed");
        // No folder: the variable is taken away, so an inherited one is
        // never read as this call's.
        let cmd = command(Path::new("/bin/bash"), OsStr::new("/dev/fd/4"), &[], CalledAs::K2Name, None);
        let dir: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == SCRIPT_DIR_VAR).collect();
        assert_eq!(dir, vec![(OsStr::new(SCRIPT_DIR_VAR), None)]);
    }

    /// The folder for the files beside the CLI: the bundled script's in a
    /// packaged app, else the executable's own (links already resolved
    /// by the caller).
    #[cfg(unix)]
    #[test]
    fn the_script_dir_is_where_an_install_keeps_the_files_beside_the_cli() {
        let dir = scratch("scriptdir");
        let contents = dir.join("K2.app/Contents");
        fs::create_dir_all(contents.join("MacOS")).unwrap();
        let exe = contents.join("MacOS/k5-cli");
        fs::write(&exe, b"").unwrap();
        // No bundled script: the executable's folder.
        assert_eq!(script_dir_for(&exe), Some(contents.join("MacOS")));
        let cli = contents.join("Resources/_up_/cli");
        fs::create_dir_all(&cli).unwrap();
        fs::write(cli.join("k2"), SCRIPT).unwrap();
        assert_eq!(script_dir_for(&exe), Some(cli));
        // A build or an install elsewhere: its own folder.
        assert_eq!(script_dir_for(&dir.join("opt/k5/k5-cli")), Some(dir.join("opt/k5")));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// What bash reads through `/dev/fd/N` is the file that was hashed,
    /// even when another file is renamed over the path afterwards; the
    /// copy is 3 or above and survives exec, the front's own descriptor
    /// does not.
    #[cfg(unix)]
    #[test]
    fn bash_reads_the_file_that_was_hashed() {
        use std::os::fd::AsRawFd;
        let dir = scratch("fd");
        let f = Fixture::new(SCRIPT);
        let got = cached(&f.packed(), &dir).unwrap();
        // Swapped after the check, as an attacker or a racing writer would.
        let other = dir.join("other");
        fs::write(&other, b"#!/bin/bash\necho PWNED\n").unwrap();
        fs::rename(&other, &got.path).unwrap();
        let fd = inheritable_copy(&got.file).unwrap();
        assert!(fd.as_raw_fd() >= 3);
        let cloexec = |fd: i32| {
            // SAFETY: F_GETFD on an open descriptor; integers only.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0, "F_GETFD {fd}: {}", io::Error::last_os_error());
            flags & libc::FD_CLOEXEC != 0
        };
        assert!(!cloexec(fd.as_raw_fd()), "the copy bash gets survives exec");
        assert!(cloexec(got.file.as_raw_fd()), "the front's own descriptor closes at exec");
        let path = dev_fd(&fd);
        assert_eq!(path, OsString::from(format!("/dev/fd/{}", fd.as_raw_fd())));
        assert_eq!(fs::read(&path).unwrap(), SCRIPT, "through {path:?}");
        assert_eq!(fs::read(&got.path).unwrap(), b"#!/bin/bash\necho PWNED\n");
        // On macOS opening /dev/fd/N shares the place in the file, which
        // the read above left at the end; the front hands bash a file at
        // its start, so wind it back as the front leaves it.
        (&got.file).seek(SeekFrom::Start(0)).unwrap();
        // And bash itself runs it so, $0 included.
        let out = Command::new(find_unix_bash(cfg!(target_os = "macos"), |p| p.is_file()).unwrap())
            .arg(&path)
            .arg("x y")
            .env_clear()
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "real\n");
        let probe = dir.join("probe.sh");
        fs::write(&probe, b"printf '%s|%s\\n' \"$0\" \"$1\"\n").unwrap();
        let file = fs::File::open(&probe).unwrap();
        let fd = inheritable_copy(&file).unwrap();
        let out = Command::new(find_unix_bash(cfg!(target_os = "macos"), |p| p.is_file()).unwrap())
            .arg(dev_fd(&fd))
            .arg("x y")
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("/dev/fd/{}|x y\n", fd.as_raw_fd()), "{out:?}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
