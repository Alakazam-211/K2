//! A13: unpack a publisher's Seed tarball into a fresh private folder, and
//! check its inner `SHA256SUMS` (R26).
//!
//! Refused: symlinks, hardlinks, devices, FIFOs and every other entry kind
//! but files and folders; absolute paths and `..`; a path twice; more than
//! the Seed limits in entries or bytes; a decompressed stream larger than
//! those bytes plus header room (a gzip bomb stops there, mid-read). The
//! destination must not exist; on any failure it is removed. No code runs
//! and nothing is followed: files are created with `create_new`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use crate::product_name;

/// What one unpack may hold.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_entries: usize,
    pub max_bytes: u64,
}

impl Limits {
    /// The Seed store's own limits (`deploy::seed`).
    pub fn seed() -> Self {
        Limits { max_entries: crate::deploy::seed::MAX_ENTRIES, max_bytes: crate::deploy::seed::MAX_BYTES }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    pub bytes: u64,
}

/// A reader that refuses to go past `cap` bytes (the decompressed stream).
struct Capped<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // One more byte would be past the cap: is there one?
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(concat!("the archive unpacks to more than ", product_name!(), " accepts for a Seed"))),
            };
        }
        let n = buf.len().min(self.left.min(usize::MAX as u64) as usize);
        let got = self.inner.read(&mut buf[..n])?;
        self.left -= got as u64;
        Ok(got)
    }
}

/// The entry's path inside the Seed, or why it is refused. `None` = the
/// root itself (`./`), skipped.
fn safe_rel(raw: &Path) -> Result<Option<PathBuf>, String> {
    let mut out = PathBuf::new();
    for c in raw.components() {
        match c {
            Component::Normal(p) => {
                let s = p.to_str().ok_or_else(|| format!("{}: not a UTF-8 path", raw.display()))?;
                if s.is_empty() || s.contains('\\') {
                    return Err(format!("{}: not a plain path", raw.display()));
                }
                out.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(format!("{}: a path with '..'", raw.display())),
            Component::RootDir | Component::Prefix(_) => return Err(format!("{}: an absolute path", raw.display())),
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

#[cfg(unix)]
fn mkdir_private(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(p)
}

#[cfg(not(unix))]
fn mkdir_private(p: &Path) -> io::Result<()> {
    fs::create_dir(p)
}

fn write_file(p: &Path, exec: bool) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(if exec { 0o700 } else { 0o600 });
    }
    #[cfg(not(unix))]
    let _ = exec;
    o.open(p)
}

/// Unpack `tarball` (gzip'd tar) into `dest`, which must not exist yet
/// (it is made 0700). `Err` = refused; `dest` is gone then.
pub fn unpack_tar_gz(tarball: &Path, dest: &Path, limits: Limits) -> Result<Unpacked, String> {
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    mkdir_private(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let r = unpack_into(tarball, dest, limits);
    if r.is_err() {
        let _ = make_writable(dest);
        let _ = fs::remove_dir_all(dest);
    }
    r
}

fn make_writable(dest: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for e in walk(dest) {
            let _ = fs::set_permissions(&e, fs::Permissions::from_mode(0o700));
        }
    }
    let _ = dest;
    Ok(())
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut i = 0;
    while i < out.len() {
        let p = out[i].clone();
        i += 1;
        if let Ok(rd) = fs::read_dir(&p) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    out.push(e.path());
                }
            }
        }
    }
    out
}

fn unpack_into(tarball: &Path, dest: &Path, limits: Limits) -> Result<Unpacked, String> {
    let f = fs::File::open(tarball).map_err(|e| format!("{}: {e}", tarball.display()))?;
    let gz = flate2::read::GzDecoder::new(io::BufReader::new(f));
    // Header room: 512 bytes per header plus long names, generously.
    let room = (limits.max_entries as u64).saturating_mul(2048).saturating_add(1 << 20);
    let capped = Capped { inner: gz, left: limits.max_bytes.saturating_add(room) };
    let mut ar = tar::Archive::new(capped);
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let (mut files, mut bytes, mut n) = (0usize, 0u64, 0usize);
    for e in ar.entries().map_err(|e| format!("not a tar archive: {e}"))? {
        let mut e = e.map_err(|e| format!("a damaged archive: {e}"))?;
        n += 1;
        if n > limits.max_entries {
            return Err(format!("more than {} entries", limits.max_entries));
        }
        let kind = e.header().entry_type();
        let raw = e.path().map_err(|e| format!("an entry path: {e}"))?.into_owned();
        let Some(rel) = safe_rel(&raw)? else { continue };
        if !seen.insert(rel.clone()) {
            return Err(format!("{}: listed twice", rel.display()));
        }
        let out = dest.join(&rel);
        match kind {
            tar::EntryType::Directory => {
                fs::create_dir_all(&out).map_err(|e| format!("mkdir {}: {e}", rel.display()))?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if kind == tar::EntryType::Continuous {
                    return Err(format!("{}: a contiguous-file entry", rel.display()));
                }
                let size = e.header().size().map_err(|e| format!("{}: size: {e}", rel.display()))?;
                bytes = bytes.saturating_add(size);
                if bytes > limits.max_bytes {
                    return Err(format!("more than {} bytes of files", limits.max_bytes));
                }
                if let Some(p) = out.parent() {
                    fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))?;
                }
                let exec = e.header().mode().map(|m| m & 0o111 != 0).unwrap_or(false);
                let mut w = write_file(&out, exec).map_err(|e| format!("{}: {e}", rel.display()))?;
                let copied = io::copy(&mut (&mut e).take(size), &mut w).map_err(|e| format!("{}: {e}", rel.display()))?;
                if copied != size {
                    return Err(format!("{}: truncated ({copied} of {size} bytes)", rel.display()));
                }
                w.flush().map_err(|e| format!("{}: {e}", rel.display()))?;
                files += 1;
            }
            tar::EntryType::Symlink => return Err(format!("{}: a symlink", rel.display())),
            tar::EntryType::Link => return Err(format!("{}: a hardlink", rel.display())),
            tar::EntryType::Char | tar::EntryType::Block => return Err(format!("{}: a device file", rel.display())),
            tar::EntryType::Fifo => return Err(format!("{}: a FIFO", rel.display())),
            other => return Err(format!("{}: an entry of kind {other:?}", rel.display())),
        }
    }
    Ok(Unpacked { files, bytes })
}

fn sha256_of(p: &Path) -> io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The sha256 of a file (lower-case hex).
pub fn sha256_file(p: &Path) -> Result<String, String> {
    sha256_of(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Every regular file under `root` (relative, `/`-separated).
fn files_under(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for d in walk(root) {
        for e in fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let t = e.file_type().map_err(|err| format!("{}: {err}", e.path().display()))?;
            if t.is_file() {
                let rel = e.path().strip_prefix(root).map_err(|e| e.to_string())?.to_path_buf();
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The Seed's own `SHA256SUMS` (R26): it lists every file but itself,
/// once, and every hash matches. `Err((file, detail))`.
pub fn verify_sums(root: &Path) -> Result<usize, (String, String)> {
    let p = root.join("SHA256SUMS");
    let meta = fs::metadata(&p).map_err(|_| ("SHA256SUMS".to_string(), "the Seed has no SHA256SUMS".to_string()))?;
    if meta.len() > 4 << 20 {
        return Err(("SHA256SUMS".into(), "larger than 4 MiB".into()));
    }
    let text = fs::read_to_string(&p).map_err(|e| ("SHA256SUMS".to_string(), e.to_string()))?;
    let mut listed: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (h, name) = line.split_once("  ").ok_or_else(|| ("SHA256SUMS".to_string(), format!("not a sums line: {line:.80}")))?;
        let name = name.strip_prefix("./").unwrap_or(name).to_string();
        if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(("SHA256SUMS".into(), format!("not a sha256: {h:.80}")));
        }
        if listed.insert(name.clone(), h.to_ascii_lowercase()).is_some() {
            return Err((name, "listed twice in SHA256SUMS".into()));
        }
    }
    let present = files_under(root).map_err(|e| ("(tree)".to_string(), e))?;
    for f in present.iter().filter(|f| f.as_str() != "SHA256SUMS") {
        let want = listed.remove(f).ok_or_else(|| (f.clone(), "a file SHA256SUMS doesn't list".to_string()))?;
        let got = sha256_of(&root.join(f)).map_err(|e| (f.clone(), e.to_string()))?;
        if got != want {
            return Err((f.clone(), format!("sha256 {got}, SHA256SUMS says {want}")));
        }
    }
    if let Some((f, _)) = listed.into_iter().next() {
        return Err((f, "listed in SHA256SUMS but not in the Seed".into()));
    }
    Ok(present.len().saturating_sub(1))
}

#[cfg(test)]
mod capped_tests {
    use super::*;

    /// A13: the decompressed stream stops at the cap, mid-read (a gzip bomb
    /// never reaches the disk past it).
    #[test]
    fn a_stream_past_the_cap_is_refused_mid_read() {
        let mut c = Capped { inner: io::repeat(0).take(10_000), left: 100 };
        let mut out = Vec::new();
        let e = c.read_to_end(&mut out).expect_err("past the cap");
        assert!(e.to_string().contains("more than K2 accepts"), "{e}");
        assert_eq!(out.len(), 100, "read up to the cap, not past it");
        let mut ok = Capped { inner: io::repeat(0).take(100), left: 100 };
        let mut out = Vec::new();
        assert_eq!(ok.read_to_end(&mut out).unwrap(), 100, "exactly the cap is fine");
    }
}
