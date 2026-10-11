//! Root-side data operations for `k2 hostmail upgrade` (CAL14), ported
//! unchanged from the protocol-2 `k2-mail-helper` (`k2-daemon::mail::
//! helper::data_ops`): the snapshot, the restore, `usage`, and the atomic
//! binary install. Every function takes a [`DataLayout`] so tests run them
//! over a temp tree; the helper passes [`DataLayout::under`] its root.
//!
//! Rules: never follow a symlink (a symlink where a dir is expected is a
//! refusal), refuse sockets/FIFOs/devices, keep owner + mode + times,
//! build into a temp sibling and rename only when complete, and refuse
//! unless Stalwart is stopped (systemd `inactive`/`failed` AND nobody
//! holds the RocksDB lock) — checked again after the copy.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use k2_root_proto::paths::{
    STALWART_CONFIG_DIR, STALWART_DATA_DIR, STALWART_SNAPSHOT_DIR, STALWART_SNAPSHOT_MARKER,
    STALWART_STORE_LOCK,
};
use k2_root_proto::PROTOCOL;

/// Free space a snapshot needs beyond the bytes it copies (CAL14).
pub const SNAPSHOT_FREE_MARGIN: u64 = 1 << 30;

/// The fixed paths `snapshot-data`, `restore-data` and `usage` act on.
/// Nothing on argv can choose a path. Tests build one over a temp tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    pub data: PathBuf,
    pub config: PathBuf,
    pub snapshot: PathBuf,
    pub store_lock: PathBuf,
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

impl DataLayout {
    /// The compiled-in layout beneath `root` (`/` in the binary).
    pub fn under(root: &Path) -> Self {
        let j = |abs: &str| root.join(abs.trim_start_matches('/'));
        Self {
            data: j(STALWART_DATA_DIR),
            config: j(STALWART_CONFIG_DIR),
            snapshot: j(STALWART_SNAPSHOT_DIR),
            store_lock: j(STALWART_STORE_LOCK),
        }
    }
    /// The snapshot is built here and renamed onto `snapshot` when done.
    pub fn snapshot_tmp(&self) -> PathBuf {
        with_suffix(&self.snapshot, ".tmp")
    }
    pub fn snapshot_data(&self) -> PathBuf {
        self.snapshot.join("data")
    }
    pub fn snapshot_config(&self) -> PathBuf {
        self.snapshot.join("config")
    }
    pub fn snapshot_marker(&self) -> PathBuf {
        self.snapshot.join(STALWART_SNAPSHOT_MARKER)
    }
    pub fn data_restore_tmp(&self) -> PathBuf {
        with_suffix(&self.data, ".k2-restore.tmp")
    }
    pub fn data_aside(&self) -> PathBuf {
        with_suffix(&self.data, ".k2-failed")
    }
    pub fn config_restore_tmp(&self) -> PathBuf {
        with_suffix(&self.config, ".k2-restore.tmp")
    }
    pub fn config_aside(&self) -> PathBuf {
        with_suffix(&self.config, ".k2-failed")
    }
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> String {
    format!("{what} {}: {e}", path.display())
}

/// Stalwart must be stopped: systemd says `inactive` or `failed`, and
/// no process holds the store's RocksDB lock (a Stalwart started by
/// hand outside systemd still has it).
pub fn require_stopped(unit_state: &str, layout: &DataLayout) -> Result<(), String> {
    match unit_state.trim() {
        "inactive" | "failed" => {}
        "" => {
            return Err(
                "refused: could not read the stalwart unit state — Stalwart must be \
                 stopped first"
                    .into(),
            )
        }
        other => {
            return Err(format!(
                "refused: Stalwart is not stopped (systemd: {other}) — the data dir is \
                 only copied while Stalwart is stopped"
            ))
        }
    }
    if store_lock_held(&layout.store_lock)? {
        return Err(format!(
            "refused: a process still holds the Stalwart store lock {} — Stalwart is \
             running outside systemd; stop it first",
            layout.store_lock.display()
        ));
    }
    Ok(())
}

/// POSIX `F_GETLK` on RocksDB's LOCK file (RocksDB takes an `F_SETLK`
/// write lock on it while the store is open). Missing file = no store
/// open. Any other open failure fails closed.
pub fn store_lock_held(path: &Path) -> Result<bool, String> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io_err("refused: cannot check the store lock", path, e)),
    };
    // SAFETY: zeroed `flock` is a valid all-fields-zero C struct; the
    // fields set below are the ones F_GETLK reads.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_WRLCK as _;
    fl.l_whence = libc::SEEK_SET as _;
    fl.l_start = 0;
    fl.l_len = 0;
    // SAFETY: `file` keeps the fd open for the call; `fl` is a valid
    // `flock` the kernel fills in.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut fl) };
    if rc != 0 {
        return Err(format!(
            "refused: cannot check the store lock {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(i64::from(fl.l_type) != i64::from(libc::F_UNLCK))
}

/// Apparent bytes of every regular file under `path` (symlinks count 0,
/// never followed). Missing = 0.
pub fn tree_bytes(path: &Path) -> Result<u64, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io_err("stat", path, e)),
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        return Ok(0);
    }
    if ft.is_file() {
        return Ok(meta.len());
    }
    if !ft.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    for ent in fs::read_dir(path).map_err(|e| io_err("read dir", path, e))? {
        let ent = ent.map_err(|e| io_err("read dir", path, e))?;
        total = total.saturating_add(tree_bytes(&ent.path())?);
    }
    Ok(total)
}

/// Bytes an unprivileged writer may still use on `path`'s filesystem.
pub fn free_bytes(path: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "statvfs: bad path".to_string())?;
    // SAFETY: zeroed statvfs is a valid out-param; `c` is NUL-terminated.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(format!(
            "statvfs {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    #[allow(clippy::unnecessary_cast)]
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

fn parent_of(path: &Path) -> Result<&Path, String> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("no parent directory for {}", path.display()))
}

/// A real directory (not a symlink to one), or a refusal naming `what`.
fn require_real_dir(path: &Path, what: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            Err(format!("refused: {what} {} is a symlink", path.display()))
        }
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "refused: {what} {} is not a directory",
            path.display()
        )),
        Err(e) => Err(io_err(&format!("refused: {what}"), path, e)),
    }
}

/// Remove a directory tree we own (a temp/aside sibling). Missing is
/// fine; a symlink or a plain file at that path is a refusal (never
/// delete through a link, never guess).
pub fn remove_real_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err("stat", path, e)),
        Ok(m) if m.file_type().is_symlink() => {
            Err(format!("refused: {} is a symlink", path.display()))
        }
        Ok(m) if m.is_dir() => fs::remove_dir_all(path).map_err(|e| io_err("remove", path, e)),
        Ok(_) => Err(format!("refused: {} is not a directory", path.display())),
    }
}

fn fsync_dir(path: &Path) -> Result<(), String> {
    let dir = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| io_err("open dir", path, e))?;
    dir.sync_all().map_err(|e| io_err("fsync dir", path, e))
}

fn rename(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| format!("rename {} -> {}: {e}", from.display(), to.display()))
}

fn path_cstring(path: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("bad path {}", path.display()))
}

fn lchown(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    let c = path_cstring(path)?;
    // SAFETY: NUL-terminated path; lchown never follows a link.
    let rc = unsafe { libc::lchown(c.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) };
    if rc != 0 {
        return Err(format!(
            "chown {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn times_of(meta: &fs::Metadata) -> [libc::timespec; 2] {
    // SAFETY: zeroed timespec is valid; fields set right after.
    let mut ts: [libc::timespec; 2] = unsafe { std::mem::zeroed() };
    ts[0].tv_sec = meta.atime() as _;
    ts[0].tv_nsec = meta.atime_nsec() as _;
    ts[1].tv_sec = meta.mtime() as _;
    ts[1].tv_nsec = meta.mtime_nsec() as _;
    ts
}

fn set_times_nofollow(path: &Path, meta: &fs::Metadata) -> Result<(), String> {
    let c = path_cstring(path)?;
    let ts = times_of(meta);
    // SAFETY: NUL-terminated path, two valid timespecs.
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            ts.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc != 0 {
        return Err(format!(
            "set times {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Copy `src` to `dst` (which must NOT exist), keeping uid/gid, mode
/// (incl. setgid/sticky bits) and atime/mtime. Directories recurse;
/// symlinks are re-created as symlinks (never followed); regular files
/// are copied and fsync'd; anything else is a refusal. Hard links are
/// copied as separate files; xattrs/ACLs are not copied.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| io_err("stat", src, e))?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        let target = fs::read_link(src).map_err(|e| io_err("readlink", src, e))?;
        std::os::unix::fs::symlink(&target, dst).map_err(|e| io_err("symlink", dst, e))?;
        return lchown(dst, meta.uid(), meta.gid());
    }
    if ft.is_dir() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(dst)
            .map_err(|e| io_err("mkdir", dst, e))?;
        for ent in fs::read_dir(src).map_err(|e| io_err("read dir", src, e))? {
            let ent = ent.map_err(|e| io_err("read dir", src, e))?;
            copy_tree(&ent.path(), &dst.join(ent.file_name()))?;
        }
        // Owner first (chown clears set-id bits), then mode, then times
        // (after the children, whose creation bumped mtime).
        lchown(dst, meta.uid(), meta.gid())?;
        fs::set_permissions(dst, fs::Permissions::from_mode(meta.mode() & 0o7777))
            .map_err(|e| io_err("chmod", dst, e))?;
        return set_times_nofollow(dst, &meta);
    }
    if ft.is_file() {
        return copy_file(src, dst, &meta);
    }
    Err(format!(
        "refused: {} is not a regular file, directory or symlink",
        src.display()
    ))
}

fn copy_file(src: &Path, dst: &Path, meta: &fs::Metadata) -> Result<(), String> {
    let mut from = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(src)
        .map_err(|e| io_err("open", src, e))?;
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dst)
        .map_err(|e| io_err("create", dst, e))?;
    let copied = std::io::copy(&mut from, &mut to).map_err(|e| io_err("copy", src, e))?;
    if copied != meta.len() {
        return Err(format!(
            "copy {}: {copied} of {} bytes (file changed during the copy)",
            src.display(),
            meta.len()
        ));
    }
    to.sync_all().map_err(|e| io_err("fsync", dst, e))?;
    let fd = to.as_raw_fd();
    // SAFETY: `fd` belongs to `to`, open for this whole block.
    if unsafe { libc::fchown(fd, meta.uid() as libc::uid_t, meta.gid() as libc::gid_t) } != 0 {
        return Err(format!(
            "chown {}: {}",
            dst.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: as above.
    if unsafe { libc::fchmod(fd, (meta.mode() & 0o7777) as libc::mode_t) } != 0 {
        return Err(format!(
            "chmod {}: {}",
            dst.display(),
            std::io::Error::last_os_error()
        ));
    }
    let ts = times_of(meta);
    // SAFETY: as above; two valid timespecs.
    if unsafe { libc::futimens(fd, ts.as_ptr()) } != 0 {
        return Err(format!(
            "set times {}: {}",
            dst.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// `usage`: what a snapshot would copy, the snapshot already there,
/// and free space where the snapshot lives. Read-only.
pub fn usage_with(
    layout: &DataLayout,
    free_of: &dyn Fn(&Path) -> Result<u64, String>,
) -> Result<serde_json::Value, String> {
    let complete = snapshot_complete(layout);
    Ok(serde_json::json!({
        "dataDir": layout.data.display().to_string(),
        "configDir": layout.config.display().to_string(),
        "snapshotDir": layout.snapshot.display().to_string(),
        "dataBytes": tree_bytes(&layout.data)?,
        "configBytes": tree_bytes(&layout.config)?,
        "snapshotBytes": tree_bytes(&layout.snapshot)?,
        "snapshotComplete": complete,
        "freeBytes": free_of(parent_of(&layout.snapshot)?)?,
    }))
}

/// A snapshot is complete only with its marker AND both copies.
pub fn snapshot_complete(layout: &DataLayout) -> bool {
    let real_dir =
        |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    real_dir(&layout.snapshot)
        && fs::symlink_metadata(layout.snapshot_marker()).is_ok_and(|m| m.file_type().is_file())
        && real_dir(&layout.snapshot_data())
        && real_dir(&layout.snapshot_config())
}

/// `snapshot-data`: copy data + config into `snapshot.tmp`, write the
/// marker last, rename onto `snapshot`. The previous snapshot (if any)
/// is removed first — counted as free space — so one snapshot exists
/// at a time. On any failure the temp copy is removed and the live
/// dirs are untouched (this verb only ever reads them).
pub fn snapshot_with(
    layout: &DataLayout,
    unit_state: &dyn Fn() -> String,
    free_of: &dyn Fn(&Path) -> Result<u64, String>,
    margin: u64,
) -> Result<serde_json::Value, String> {
    require_stopped(&unit_state(), layout)?;
    require_real_dir(&layout.data, "Stalwart data dir")?;
    require_real_dir(&layout.config, "Stalwart config dir")?;
    let tmp = layout.snapshot_tmp();
    remove_real_dir(&tmp)?;

    let data_bytes = tree_bytes(&layout.data)?;
    let config_bytes = tree_bytes(&layout.config)?;
    let old_snapshot = tree_bytes(&layout.snapshot)?;
    let parent = parent_of(&layout.snapshot)?;
    let free = free_of(parent)?;
    let need = data_bytes
        .saturating_add(config_bytes)
        .saturating_add(margin);
    let have = free.saturating_add(old_snapshot);
    if have < need {
        return Err(format!(
            "refused: not enough free space for the snapshot on {} — need {need} bytes \
             (data {data_bytes} + config {config_bytes} + {margin} margin), have {have} \
             (free {free} + old snapshot {old_snapshot})",
            parent.display()
        ));
    }
    remove_real_dir(&layout.snapshot)?;

    let result = (|| -> Result<serde_json::Value, String> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&tmp)
            .map_err(|e| io_err("mkdir", &tmp, e))?;
        copy_tree(&layout.data, &tmp.join("data"))?;
        copy_tree(&layout.config, &tmp.join("config"))?;
        // Stalwart must not have started while we copied (a TLS-reload
        // restart, a human): that copy would not be a consistent store.
        require_stopped(&unit_state(), layout)
            .map_err(|e| format!("Stalwart started during the snapshot copy — {e}"))?;
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let marker = serde_json::json!({
            "createdAt": created_at,
            "dataBytes": data_bytes,
            "configBytes": config_bytes,
            "helperProtocol": PROTOCOL,
            "dataDir": layout.data.display().to_string(),
            "configDir": layout.config.display().to_string(),
        });
        let marker_path = tmp.join(STALWART_SNAPSHOT_MARKER);
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&marker_path)
            .map_err(|e| io_err("create", &marker_path, e))?;
        f.write_all(marker.to_string().as_bytes())
            .map_err(|e| io_err("write", &marker_path, e))?;
        f.sync_all().map_err(|e| io_err("fsync", &marker_path, e))?;
        fsync_dir(&tmp)?;
        rename(&tmp, &layout.snapshot)?;
        fsync_dir(parent)?;
        let mut out = marker;
        out["snapshotDir"] = serde_json::json!(layout.snapshot.display().to_string());
        Ok(out)
    })();
    if result.is_err() {
        let _ = remove_real_dir(&tmp);
    }
    result
}

/// `restore-data`: replace the live data + config dirs with the
/// complete snapshot. The snapshot itself is only read (it survives,
/// for a second attempt). With room for a full copy beside the live
/// data, the copy is made first and swapped in; without room the
/// (failed-upgrade) live data dir is moved aside and deleted first —
/// the snapshot is the copy that matters.
pub fn restore_with(
    layout: &DataLayout,
    unit_state: &dyn Fn() -> String,
    free_of: &dyn Fn(&Path) -> Result<u64, String>,
    margin: u64,
) -> Result<serde_json::Value, String> {
    require_stopped(&unit_state(), layout)?;
    if !snapshot_complete(layout) {
        return Err(format!(
            "refused: no complete snapshot at {} (marker {} missing or a copy missing) — \
             nothing was restored",
            layout.snapshot.display(),
            layout.snapshot_marker().display()
        ));
    }
    let data_tmp = layout.data_restore_tmp();
    let data_aside = layout.data_aside();
    let config_tmp = layout.config_restore_tmp();
    let config_aside = layout.config_aside();
    for p in [&data_tmp, &data_aside, &config_tmp, &config_aside] {
        remove_real_dir(p)?;
    }
    // A live dir that is a symlink is never moved or deleted.
    for (p, what) in [
        (&layout.data, "Stalwart data dir"),
        (&layout.config, "Stalwart config dir"),
    ] {
        if fs::symlink_metadata(p).is_ok() {
            require_real_dir(p, what)?;
        }
    }

    // Config first (small): staged copy, nothing live touched yet.
    if let Err(e) = copy_tree(&layout.snapshot_config(), &config_tmp) {
        let _ = remove_real_dir(&config_tmp);
        return Err(format!("restore: nothing changed — {e}"));
    }

    let mut leftovers: Vec<String> = Vec::new();
    let snap_data_bytes = tree_bytes(&layout.snapshot_data())?;
    let data_parent = parent_of(&layout.data)?;
    let free = free_of(data_parent)?;
    let copy_first = free >= snap_data_bytes.saturating_add(margin);
    let lost = |e: String| {
        format!(
            "restore FAILED after the live data dir was removed — {e}. The snapshot at {} \
             is intact: copy {} to {} by hand (cp -a) with Stalwart stopped",
            layout.snapshot.display(),
            layout.snapshot_data().display(),
            layout.data.display()
        )
    };
    if copy_first {
        if let Err(e) = copy_tree(&layout.snapshot_data(), &data_tmp) {
            let _ = remove_real_dir(&data_tmp);
            let _ = remove_real_dir(&config_tmp);
            return Err(format!("restore: nothing changed — {e}"));
        }
        if fs::symlink_metadata(&layout.data).is_ok() {
            rename(&layout.data, &data_aside).map_err(|e| {
                let _ = remove_real_dir(&data_tmp);
                let _ = remove_real_dir(&config_tmp);
                format!("restore: nothing changed — {e}")
            })?;
        }
        rename(&data_tmp, &layout.data).map_err(lost)?;
        if let Err(e) = remove_real_dir(&data_aside) {
            leftovers.push(format!("{} ({e})", data_aside.display()));
        }
    } else {
        if fs::symlink_metadata(&layout.data).is_ok() {
            rename(&layout.data, &data_aside).map_err(|e| {
                let _ = remove_real_dir(&config_tmp);
                format!("restore: nothing changed — {e}")
            })?;
        }
        remove_real_dir(&data_aside).map_err(lost)?;
        if let Err(e) = copy_tree(&layout.snapshot_data(), &data_tmp) {
            let _ = remove_real_dir(&data_tmp);
            return Err(lost(e));
        }
        rename(&data_tmp, &layout.data).map_err(lost)?;
    }
    fsync_dir(data_parent)?;

    if fs::symlink_metadata(&layout.config).is_ok() {
        rename(&layout.config, &config_aside)?;
    }
    rename(&config_tmp, &layout.config)?;
    if let Err(e) = remove_real_dir(&config_aside) {
        leftovers.push(format!("{} ({e})", config_aside.display()));
    }
    fsync_dir(parent_of(&layout.config)?)?;
    Ok(serde_json::json!({
        "restoredFrom": layout.snapshot.display().to_string(),
        "dataBytes": snap_data_bytes,
        "copyFirst": copy_first,
        // Old (failed-upgrade) dirs that could not be deleted after the
        // swap. The restore itself succeeded; remove these by hand.
        "leftovers": leftovers,
    }))
}

/// Write `bytes` to `staging` (fresh, never through a link), set
/// owner + mode, fsync, then rename onto `dest`. A crash leaves the old
/// `dest` or the new one, never a half-written binary. `owner` is
/// `Some((0, 0))` in the helper; tests pass `None`.
pub fn install_file_atomic(
    staging: &Path,
    dest: &Path,
    bytes: &[u8],
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<(), String> {
    match fs::symlink_metadata(staging) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err("install-bin: stat", staging, e)),
        Ok(m) if m.file_type().is_file() => {
            fs::remove_file(staging).map_err(|e| io_err("install-bin: remove", staging, e))?
        }
        Ok(_) => {
            return Err(format!(
                "install-bin refused: staging path {} is not a regular file",
                staging.display()
            ))
        }
    }
    let result = (|| -> Result<(), String> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(staging)
            .map_err(|e| io_err("install-bin: create", staging, e))?;
        f.write_all(bytes)
            .map_err(|e| io_err("install-bin: write", staging, e))?;
        if let Some((uid, gid)) = owner {
            // SAFETY: the fd belongs to `f`, open for this call.
            if unsafe { libc::fchown(f.as_raw_fd(), uid as libc::uid_t, gid as libc::gid_t) } != 0 {
                return Err(format!(
                    "install-bin: chown {}: {}",
                    staging.display(),
                    std::io::Error::last_os_error()
                ));
            }
        }
        f.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|e| io_err("install-bin: chmod", staging, e))?;
        f.sync_all()
            .map_err(|e| io_err("install-bin: fsync", staging, e))?;
        rename(staging, dest)?;
        fsync_dir(parent_of(dest)?)
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    const MARGIN: u64 = 1024;

    struct Tree {
        root: PathBuf,
        layout: DataLayout,
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // Make anything chmod'ed 000 removable again.
            let _ = std::process::Command::new("chmod")
                .args(["-R", "u+rwx"])
                .arg(&self.root)
                .status();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn tree(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!(
            "k2-mail-upgrade-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data = root.join("var/lib/stalwart");
        let config = root.join("etc/stalwart");
        fs::create_dir_all(data.join("data")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(data.join("data/000123.sst"), b"rocksdb table bytes").unwrap();
        fs::write(data.join("data/CURRENT"), b"MANIFEST-000001\n").unwrap();
        fs::write(data.join("data/LOCK"), b"").unwrap();
        fs::set_permissions(
            data.join("data/000123.sst"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        fs::write(config.join("config.json"), br#"{"@type":"RocksDb"}"#).unwrap();
        fs::set_permissions(
            config.join("config.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        std::os::unix::fs::symlink("data/CURRENT", data.join("current-link")).unwrap();
        let layout = DataLayout {
            data: data.clone(),
            config,
            snapshot: root.join("var/lib/stalwart.k2-snap"),
            store_lock: data.join("data/LOCK"),
        };
        Tree { root, layout }
    }

    fn stopped() -> String {
        "inactive".into()
    }
    fn plenty(_: &Path) -> Result<u64, String> {
        Ok(u64::MAX / 4)
    }

    #[test]
    fn snapshot_copies_data_and_config_with_modes_times_and_links() {
        let t = tree("snap");
        let src_meta = fs::metadata(t.layout.data.join("data/000123.sst")).unwrap();
        let out = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("snapshot");
        assert_eq!(out["helperProtocol"], PROTOCOL);
        assert!(snapshot_complete(&t.layout));
        let snap = &t.layout.snapshot;
        assert_eq!(
            fs::read(snap.join("data/data/000123.sst")).unwrap(),
            b"rocksdb table bytes"
        );
        assert_eq!(
            fs::read(snap.join("config/config.json")).unwrap(),
            br#"{"@type":"RocksDb"}"#
        );
        let copy_meta = fs::metadata(snap.join("data/data/000123.sst")).unwrap();
        assert_eq!(copy_meta.mode() & 0o7777, 0o640);
        assert_eq!(copy_meta.mtime(), src_meta.mtime());
        assert_eq!(copy_meta.uid(), src_meta.uid());
        assert_eq!(
            fs::metadata(snap.join("config/config.json"))
                .unwrap()
                .mode()
                & 0o7777,
            0o600
        );
        let link = snap.join("data/current-link");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("data/CURRENT"));
        assert!(!t.layout.snapshot_tmp().exists(), "tmp renamed away");
        assert_eq!(
            fs::metadata(snap).unwrap().mode() & 0o777,
            0o700,
            "snapshot root is private"
        );
        // usage sees it.
        let u = usage_with(&t.layout, &plenty).expect("usage");
        assert_eq!(u["snapshotComplete"], true);
        assert!(u["dataBytes"].as_u64().unwrap() > 0);
        assert!(u["snapshotBytes"].as_u64().unwrap() >= u["dataBytes"].as_u64().unwrap());
        // A second snapshot replaces the first (one snapshot at a time).
        fs::write(t.layout.data.join("data/000124.sst"), b"newer").unwrap();
        snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("second snapshot");
        assert_eq!(
            fs::read(snap.join("data/data/000124.sst")).unwrap(),
            b"newer"
        );
    }

    #[test]
    fn snapshot_and_restore_refuse_unless_stalwart_is_stopped() {
        let t = tree("running");
        for state in ["active", "activating", "deactivating", "reloading", ""] {
            let st = move || state.to_string();
            let err = snapshot_with(&t.layout, &st, &plenty, MARGIN).expect_err(state);
            assert!(err.starts_with("refused"), "{state}: {err}");
            assert!(!t.layout.snapshot.exists(), "{state}");
            assert!(!t.layout.snapshot_tmp().exists(), "{state}");
            let err = restore_with(&t.layout, &st, &plenty, MARGIN).expect_err(state);
            assert!(err.starts_with("refused"), "{state}: {err}");
        }
        // `failed` is a stopped unit.
        let failed = || "failed".to_string();
        snapshot_with(&t.layout, &failed, &plenty, MARGIN).expect("failed unit = stopped");
    }

    #[test]
    fn snapshot_fails_closed_if_stalwart_starts_during_the_copy() {
        let t = tree("started");
        let calls = std::cell::Cell::new(0);
        let st = || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                "inactive"
            } else {
                "active"
            }
            .to_string()
        };
        let err = snapshot_with(&t.layout, &st, &plenty, MARGIN).expect_err("started");
        assert!(err.contains("started during the snapshot copy"), "{err}");
        assert!(!t.layout.snapshot.exists());
        assert!(!t.layout.snapshot_tmp().exists(), "partial copy removed");
        assert!(
            t.layout.data.join("data/000123.sst").exists(),
            "live data untouched"
        );
    }

    #[test]
    fn snapshot_refuses_symlinks_special_files_and_low_space() {
        // Live data dir is a symlink → refused.
        let t = tree("symlink");
        let real = t.root.join("elsewhere");
        fs::rename(&t.layout.data, &real).unwrap();
        std::os::unix::fs::symlink(&real, &t.layout.data).unwrap();
        let err = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("symlink");
        assert!(err.contains("symlink"), "{err}");

        // A FIFO inside the store → refused, tmp cleaned up.
        let t = tree("fifo");
        let fifo = t.layout.data.join("data/pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: NUL-terminated path in a temp dir we own.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let err = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("fifo");
        assert!(err.contains("not a regular file"), "{err}");
        assert!(!t.layout.snapshot_tmp().exists());
        assert!(!t.layout.snapshot.exists());

        // Space: data + config + margin must fit in free + old snapshot.
        let t = tree("space");
        let tight = |_: &Path| -> Result<u64, String> { Ok(10) };
        let err = snapshot_with(&t.layout, &stopped, &tight, MARGIN).expect_err("space");
        assert!(err.contains("not enough free space"), "{err}");
        assert!(!t.layout.snapshot_tmp().exists());
    }

    #[test]
    fn snapshot_cleans_up_a_partial_copy_on_a_read_error() {
        // RH16: no silent skip. Root reads mode-000 files, so this case
        // must run unprivileged — fail loudly if it doesn't.
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "run the helper unit tests as a non-root user"
        );
        let t = tree("unreadable");
        let f = t.layout.data.join("data/000123.sst");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o000)).unwrap();
        let err = snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("unreadable");
        assert!(err.contains("open"), "{err}");
        assert!(!t.layout.snapshot_tmp().exists(), "partial copy removed");
        assert!(!snapshot_complete(&t.layout));
        fs::set_permissions(&f, fs::Permissions::from_mode(0o640)).unwrap();
    }

    #[test]
    fn restore_puts_the_snapshot_back_on_both_space_paths() {
        for (tag, free) in [("copy-first", u64::MAX / 4), ("delete-first", 0u64)] {
            let t = tree(tag);
            snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("snapshot");
            // The failed upgrade rewrote the store and the config.
            fs::write(
                t.layout.data.join("data/000123.sst"),
                b"migrated by 0.16.20",
            )
            .unwrap();
            fs::write(t.layout.data.join("data/999999.sst"), b"new file").unwrap();
            fs::write(t.layout.config.join("config.json"), b"{}").unwrap();
            let free_of = move |_: &Path| -> Result<u64, String> { Ok(free) };
            let out = restore_with(&t.layout, &stopped, &free_of, MARGIN).expect(tag);
            assert_eq!(out["copyFirst"], tag == "copy-first", "{tag}");
            assert!(
                out["leftovers"].as_array().unwrap().is_empty(),
                "{tag}: {out}"
            );
            assert_eq!(
                fs::read(t.layout.data.join("data/000123.sst")).unwrap(),
                b"rocksdb table bytes",
                "{tag}"
            );
            assert!(!t.layout.data.join("data/999999.sst").exists(), "{tag}");
            assert_eq!(
                fs::read(t.layout.config.join("config.json")).unwrap(),
                br#"{"@type":"RocksDb"}"#,
                "{tag}"
            );
            assert_eq!(
                fs::metadata(t.layout.data.join("data/000123.sst"))
                    .unwrap()
                    .mode()
                    & 0o7777,
                0o640,
                "{tag}"
            );
            for p in [
                t.layout.data_restore_tmp(),
                t.layout.data_aside(),
                t.layout.config_restore_tmp(),
                t.layout.config_aside(),
            ] {
                assert!(!p.exists(), "{tag}: {} left behind", p.display());
            }
            // The snapshot survives a restore (a second attempt can use it).
            assert!(snapshot_complete(&t.layout), "{tag}");
        }
    }

    #[test]
    fn restore_refuses_an_incomplete_snapshot_and_changes_nothing() {
        let t = tree("incomplete");
        let err = restore_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("none");
        assert!(err.contains("no complete snapshot"), "{err}");
        snapshot_with(&t.layout, &stopped, &plenty, MARGIN).expect("snapshot");
        fs::remove_file(t.layout.snapshot_marker()).unwrap();
        fs::write(t.layout.data.join("data/000123.sst"), b"live").unwrap();
        let err = restore_with(&t.layout, &stopped, &plenty, MARGIN).expect_err("no marker");
        assert!(err.contains("no complete snapshot"), "{err}");
        assert_eq!(
            fs::read(t.layout.data.join("data/000123.sst")).unwrap(),
            b"live",
            "live data untouched"
        );
    }

    #[test]
    fn install_file_atomic_replaces_and_refuses_a_staging_link() {
        let t = tree("install");
        let bin_dir = t.root.join("usr/local/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let dest = bin_dir.join("stalwart");
        let staging = bin_dir.join(".stalwart.k2-new");
        fs::write(&dest, b"old binary").unwrap();
        fs::write(&staging, b"stale staging").unwrap();
        install_file_atomic(&staging, &dest, b"new binary", 0o755, None).expect("install");
        assert_eq!(fs::read(&dest).unwrap(), b"new binary");
        assert_eq!(fs::metadata(&dest).unwrap().mode() & 0o7777, 0o755);
        assert!(!staging.exists());
        std::os::unix::fs::symlink(t.root.join("evil"), &staging).unwrap();
        let err = install_file_atomic(&staging, &dest, b"x", 0o755, None).expect_err("link");
        assert!(err.contains("not a regular file"), "{err}");
        assert_eq!(fs::read(&dest).unwrap(), b"new binary", "dest untouched");
        assert!(
            !t.root.join("evil").exists(),
            "never written through the link"
        );
    }

    #[test]
    fn store_lock_is_free_when_missing_or_unlocked() {
        let t = tree("lock");
        assert!(!store_lock_held(&t.layout.store_lock).expect("unlocked"));
        assert!(!store_lock_held(&t.root.join("nope/LOCK")).expect("missing"));
    }

    /// A store some OTHER process holds open (RocksDB's `F_SETLK` write
    /// lock on LOCK — e.g. a Stalwart started by hand) refuses the data
    /// verbs even when systemd says `inactive`.
    #[test]
    fn a_lock_held_by_another_process_refuses_snapshot_and_restore() {
        let t = tree("held");
        let path = std::ffi::CString::new(t.layout.store_lock.to_str().unwrap()).unwrap();
        let mut fds = [0i32; 2];
        // SAFETY: plain pipe/fork; the child only makes async-signal-safe
        // calls (open, fcntl, write, sleep, _exit) and never returns.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork");
        if pid == 0 {
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_RDWR);
                let mut fl: libc::flock = std::mem::zeroed();
                fl.l_type = libc::F_WRLCK as _;
                fl.l_whence = libc::SEEK_SET as _;
                let ok = fd >= 0 && libc::fcntl(fd, libc::F_SETLK, &fl) == 0;
                let byte: u8 = if ok { b'1' } else { b'0' };
                libc::write(fds[1], (&byte as *const u8).cast(), 1);
                libc::sleep(30);
                libc::_exit(0);
            }
        }
        let mut got = 0u8;
        // SAFETY: reading one byte from our pipe end.
        let n = unsafe { libc::read(fds[0], (&mut got as *mut u8).cast(), 1) };
        assert_eq!(n, 1, "child reported");
        assert_eq!(got, b'1', "child took the lock");
        let held = store_lock_held(&t.layout.store_lock);
        let snap = snapshot_with(&t.layout, &stopped, &plenty, MARGIN);
        let restore = restore_with(&t.layout, &stopped, &plenty, MARGIN);
        // SAFETY: our own child.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        assert!(held.expect("lock check"), "another process holds the lock");
        let err = snap.expect_err("held lock refuses the snapshot");
        assert!(err.contains("store lock"), "{err}");
        assert!(!t.layout.snapshot.exists());
        let err = restore.expect_err("held lock refuses the restore");
        assert!(err.contains("store lock"), "{err}");
        assert!(
            !store_lock_held(&t.layout.store_lock).expect("released"),
            "the lock dies with the process"
        );
    }
}
