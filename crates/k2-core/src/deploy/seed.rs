//! The Seed store (§7.1): `~/.k2/seeds/<hash>/`, read-only trees shared by
//! every App (and, later, frozen Garden widgets).
//!
//! - A Seed is written once: copied into `~/.k2/seeds/.tmp-<random>/`,
//!   hashed, made read-only (files 0444, or 0555 when executable; dirs
//!   0555), then renamed to its hash. The same tree always gets the same
//!   hash, so a rebuild of an unchanged folder reuses the stored Seed.
//! - The hash covers sorted relative paths, the kind of each entry, the
//!   executable bit, file contents and symlink targets. Symlinks must stay
//!   inside the tree; sockets, FIFOs and devices are refused.
//! - Every read for a deploy re-hashes the tree: a mismatch refuses the
//!   Seed (`seed_corrupt`), so a tampered Seed never starts.
//! - A `.k2appignore` at the folder's root (gitignore syntax, 0.46.4) keeps
//!   matching paths out when a folder is packed ([`store_folder`]): local
//!   state (`var/`), `.env*`, build caches (`node_modules/.cache`). The file
//!   itself is packed, so a wall build's output honours it too.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::codes;

/// Most entries one Seed may hold.
pub const MAX_ENTRIES: usize = 200_000;
/// Largest Seed (sum of file sizes).
pub const MAX_BYTES: u64 = 4 << 30;
/// Always left out of a build, at the top of the folder.
pub const ALWAYS_EXCLUDED: &[&str] = &[".git", ".k2", ".DS_Store"];
/// Paths a folder keeps out of its Seed (gitignore syntax, at its root).
pub const IGNORE_FILE: &str = ".k2appignore";
/// How many of the largest paths a pack reports ([`measure`]).
pub const LARGEST_SHOWN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedError {
    pub code: &'static str,
    pub message: String,
}

impl SeedError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// What a tree hashes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeHash {
    pub hash: String,
    pub size_bytes: u64,
    pub entries: usize,
}

/// A valid Seed hash: 64 lower-case hex.
pub fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Where Seed `hash` lives.
pub fn seed_path(hash: &str) -> PathBuf {
    super::seeds_dir().join(hash)
}

fn rel_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn sorted_children(dir: &Path) -> Result<Vec<fs::DirEntry>, SeedError> {
    let mut v: Vec<fs::DirEntry> = fs::read_dir(dir)
        .map_err(|e| SeedError::new(codes::SEED_INVALID, format!("read {}: {e}", dir.display())))?
        .collect::<Result<_, _>>()
        .map_err(|e| SeedError::new(codes::SEED_INVALID, format!("read {}: {e}", dir.display())))?;
    v.sort_by_key(|e| e.file_name());
    Ok(v)
}

#[cfg(unix)]
fn is_exec(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_meta: &fs::Metadata) -> bool {
    false
}

/// A symlink target stays inside the tree (lexically, from the link's
/// own folder). Absolute targets never do.
fn link_stays_inside(rel_link: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = rel_link.parent().map(|p| p.components().count() as i64).unwrap_or(0);
    for c in target.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

fn sha_file(path: &Path) -> Result<(String, u64), SeedError> {
    let mut f = fs::File::open(path).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("open {}: {e}", path.display())))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        n_total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((hex(&h.finalize()), n_total))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hash a tree (§7.1). `exclude` names top-level entries to skip (build
/// time only; a stored Seed is hashed whole).
pub fn hash_tree(root: &Path, exclude: &BTreeSet<String>) -> Result<TreeHash, SeedError> {
    let mut h = Sha256::new();
    let mut size = 0u64;
    let mut entries = 0usize;
    walk(root, root, exclude, &mut |rel, meta, path| {
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err(SeedError::new(codes::SEED_INVALID, format!("more than {MAX_ENTRIES} entries")));
        }
        let r = rel_string(rel);
        let ft = meta.file_type();
        if ft.is_dir() {
            h.update(format!("d {r}\n").as_bytes());
        } else if ft.is_symlink() {
            let t = fs::read_link(path).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("readlink {r}: {e}")))?;
            if !link_stays_inside(rel, &t) {
                return Err(SeedError::new(codes::SEED_INVALID, format!("symlink {r} points outside the Seed ({})", t.display())));
            }
            h.update(format!("l {r}\0{}\n", t.to_string_lossy()).as_bytes());
        } else if ft.is_file() {
            let (sha, n) = sha_file(path)?;
            size += n;
            if size > MAX_BYTES {
                return Err(SeedError::new(codes::SEED_INVALID, format!("larger than {} GiB", MAX_BYTES >> 30)));
            }
            let x = if is_exec(meta) { "x" } else { "-" };
            h.update(format!("f {x} {r}\0{sha}\n").as_bytes());
        } else {
            return Err(SeedError::new(codes::SEED_INVALID, format!("{r} is not a file, folder or symlink")));
        }
        Ok(())
    })?;
    Ok(TreeHash { hash: hex(&h.finalize()), size_bytes: size, entries })
}

type Visit<'a> = dyn FnMut(&Path, &fs::Metadata, &Path) -> Result<(), SeedError> + 'a;

fn walk(root: &Path, dir: &Path, exclude: &BTreeSet<String>, f: &mut Visit<'_>) -> Result<(), SeedError> {
    walk_ignoring(root, dir, exclude, None, f)
}

/// [`walk`], skipping what `ignore` (a `.k2appignore`) matches: an ignored
/// folder is never entered.
fn walk_ignoring(
    root: &Path,
    dir: &Path,
    exclude: &BTreeSet<String>,
    ignore: Option<&ignore::gitignore::Gitignore>,
    f: &mut Visit<'_>,
) -> Result<(), SeedError> {
    for e in sorted_children(dir)? {
        let path = e.path();
        let rel = path.strip_prefix(root).expect("child of root").to_path_buf();
        if dir == root {
            let top = e.file_name().to_string_lossy().to_string();
            if exclude.contains(&top) {
                continue;
            }
        }
        let meta = fs::symlink_metadata(&path).map_err(|er| SeedError::new(codes::SEED_INVALID, format!("stat {}: {er}", path.display())))?;
        if let Some(gi) = ignore {
            if gi.matched(&rel, meta.file_type().is_dir()).is_ignore() {
                continue;
            }
        }
        f(&rel, &meta, &path)?;
        if meta.file_type().is_dir() {
            walk_ignoring(root, &path, exclude, ignore, f)?;
        }
    }
    Ok(())
}

/// The folder's `.k2appignore` as a matcher (`None` when it has none). A
/// line the gitignore syntax can't read refuses the build (`seed_invalid`,
/// naming it), never silently packs what it meant to leave out.
pub fn folder_ignore(folder: &Path) -> Result<Option<ignore::gitignore::Gitignore>, SeedError> {
    let file = folder.join(IGNORE_FILE);
    match fs::symlink_metadata(&file) {
        Ok(m) if m.file_type().is_file() => {}
        Ok(_) => return Err(SeedError::new(codes::SEED_INVALID, format!("{IGNORE_FILE} must be a plain file"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SeedError::new(codes::SEED_INVALID, format!("{IGNORE_FILE}: {e}"))),
    }
    let mut b = ignore::gitignore::GitignoreBuilder::new(folder);
    if let Some(e) = b.add(&file) {
        return Err(SeedError::new(codes::SEED_INVALID, format!("{IGNORE_FILE}: {e}")));
    }
    b.build().map(Some).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("{IGNORE_FILE}: {e}")))
}

/// The top-level names a build leaves out: [`ALWAYS_EXCLUDED`] plus the
/// manifest's `exclude` (top-level names only).
pub fn build_excludes(manifest_exclude: &[String]) -> BTreeSet<String> {
    let mut s: BTreeSet<String> = ALWAYS_EXCLUDED.iter().map(|x| x.to_string()).collect();
    for e in manifest_exclude {
        let t = e.trim().trim_start_matches("./").trim_end_matches('/');
        if !t.is_empty() && !t.contains('/') {
            s.insert(t.to_string());
        }
    }
    s
}

fn copy_tree(src: &Path, dst: &Path, exclude: &BTreeSet<String>, ignore: Option<&ignore::gitignore::Gitignore>) -> Result<(), SeedError> {
    fs::create_dir_all(dst).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("mkdir {}: {e}", dst.display())))?;
    walk_ignoring(src, src, exclude, ignore, &mut |rel, meta, path| {
        let to = dst.join(rel);
        let ft = meta.file_type();
        let err = |e: std::io::Error| SeedError::new(codes::SEED_INVALID, format!("copy {}: {e}", rel.display()));
        if ft.is_dir() {
            fs::create_dir_all(&to).map_err(err)?;
        } else if ft.is_symlink() {
            let t = fs::read_link(path).map_err(err)?;
            if !link_stays_inside(rel, &t) {
                return Err(SeedError::new(codes::SEED_INVALID, format!("symlink {} points outside the Seed", rel.display())));
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&t, &to).map_err(err)?;
            #[cfg(not(unix))]
            return Err(SeedError::new(codes::SEED_INVALID, "symlinks in Seeds need a Unix host"));
        } else if ft.is_file() {
            fs::copy(path, &to).map_err(err)?;
            set_mode(&to, if is_exec(meta) { 0o755 } else { 0o644 })?;
        } else {
            return Err(SeedError::new(codes::SEED_INVALID, format!("{} is not a file, folder or symlink", rel.display())));
        }
        Ok(())
    })
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) -> Result<(), SeedError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
        .map_err(|e| SeedError::new(codes::SEED_INVALID, format!("chmod {}: {e}", p.display())))
}

#[cfg(not(unix))]
fn set_mode(_p: &Path, _mode: u32) -> Result<(), SeedError> {
    Ok(())
}

/// Make a stored tree read-only (files 0444/0555, dirs 0555), deepest
/// first so the walk can still list each folder.
fn seal(root: &Path) -> Result<(), SeedError> {
    let mut dirs = vec![root.to_path_buf()];
    walk(root, root, &BTreeSet::new(), &mut |_rel, meta, path| {
        let ft = meta.file_type();
        if ft.is_dir() {
            dirs.push(path.to_path_buf());
        } else if ft.is_file() {
            set_mode(path, if is_exec(meta) { 0o555 } else { 0o444 })?;
        }
        Ok(())
    })?;
    for d in dirs.iter().rev() {
        set_mode(d, 0o555)?;
    }
    Ok(())
}

/// Undo [`seal`] so a tree can be removed (sweep, a failed build).
fn unseal(root: &Path) {
    if fs::symlink_metadata(root).is_err() {
        return;
    }
    let _ = set_mode(root, 0o755);
    let _ = walk(root, root, &BTreeSet::new(), &mut |_rel, meta, path| {
        if meta.file_type().is_dir() {
            let _ = set_mode(path, 0o755);
        }
        Ok(())
    });
}

/// Remove a stored Seed tree (the DB row is the caller's).
pub fn remove_tree(hash: &str) -> Result<(), SeedError> {
    if !valid_hash(hash) {
        return Err(SeedError::new(codes::SEED_NOT_FOUND, format!("'{hash}' is not a Seed hash")));
    }
    let p = seed_path(hash);
    unseal(&p);
    match fs::remove_dir_all(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SeedError::new(codes::SEED_INVALID, format!("remove {}: {e}", p.display()))),
    }
}

/// Name prefix of a Seed tree the sweep moved aside (a `.tmp-` name, so an
/// interrupted removal is cleaned by [`clean_tmp`] at the next boot).
pub const DETACHED_PREFIX: &str = ".tmp-sweep-";

/// Move a stored Seed tree out of the store's name (one rename; fast
/// enough under the DB lock). Returns where it went; the caller removes it
/// with [`remove_detached`] outside the lock.
pub fn detach_tree(hash: &str) -> Result<PathBuf, SeedError> {
    if !valid_hash(hash) {
        return Err(SeedError::new(codes::SEED_NOT_FOUND, format!("'{hash}' is not a Seed hash")));
    }
    let p = seed_path(hash);
    let aside = super::seeds_dir().join(format!("{DETACHED_PREFIX}{hash}-{}", uuid::Uuid::new_v4().simple()));
    // A sealed tree's top folder is 0555: renaming within the same parent
    // needs only the parent's write bit, but open it anyway so the move
    // works on every platform.
    let _ = set_mode(&p, 0o755);
    match fs::rename(&p, &aside) {
        Ok(()) => Ok(aside),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(aside),
        Err(e) => {
            let _ = set_mode(&p, 0o555);
            Err(SeedError::new(codes::SEED_INVALID, format!("move {} aside: {e}", p.display())))
        }
    }
}

/// Remove a tree [`detach_tree`] moved aside. Only paths in the Seed
/// store with the detached prefix (never anything else).
pub fn remove_detached(p: &Path) {
    let ok = p.parent() == Some(super::seeds_dir().as_path())
        && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(DETACHED_PREFIX));
    if !ok {
        crate::log_debug!("[seeds] refused to remove {} (not a detached Seed)", p.display());
        return;
    }
    unseal(p);
    if let Err(e) = fs::remove_dir_all(p) {
        if e.kind() != std::io::ErrorKind::NotFound {
            crate::log_debug!("[seeds] remove {}: {e} (cleaned at the next boot)", p.display());
        }
    }
}

/// The result of storing a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub hash: String,
    pub size_bytes: u64,
    pub entries: usize,
    /// The same tree was already stored (nothing new written).
    pub reused: bool,
}

/// Copy `folder` (minus `exclude` and what its `.k2appignore` matches)
/// into the store and return its hash. Idempotent: an unchanged folder gets
/// the hash it had before.
pub fn store_folder(folder: &Path, exclude: &BTreeSet<String>) -> Result<Stored, SeedError> {
    let ignore = folder_ignore(folder)?;
    let root = super::seeds_dir();
    fs::create_dir_all(&root).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("mkdir {}: {e}", root.display())))?;
    let tmp = root.join(format!(".tmp-{}", uuid::Uuid::new_v4().simple()));
    let res = (|| {
        copy_tree(folder, &tmp, exclude, ignore.as_ref())?;
        let th = hash_tree(&tmp, &BTreeSet::new())?;
        let dest = root.join(&th.hash);
        if dest.is_dir() {
            // Same content already stored: check it really is.
            verify(&th.hash)?;
            unseal(&tmp);
            let _ = fs::remove_dir_all(&tmp);
            return Ok(Stored { hash: th.hash, size_bytes: th.size_bytes, entries: th.entries, reused: true });
        }
        seal(&tmp)?;
        fs::rename(&tmp, &dest).map_err(|e| SeedError::new(codes::SEED_INVALID, format!("rename into the store: {e}")))?;
        Ok(Stored { hash: th.hash, size_bytes: th.size_bytes, entries: th.entries, reused: false })
    })();
    if res.is_err() {
        unseal(&tmp);
        let _ = fs::remove_dir_all(&tmp);
    }
    res
}

/// What a stored Seed weighs, for the build and deploy answers (NSI
/// 0.46.4 #3: a pack with `var/` and `.next/cache` in it was 10× the code).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedSize {
    pub bytes: u64,
    pub files: usize,
    /// The heaviest folders and files at depth 1 and 2 (`node_modules`,
    /// `.next/cache`), largest first, at most [`LARGEST_SHOWN`].
    pub largest: Vec<PathSize>,
    /// The folder had a `.k2appignore` when it was packed (the file is in
    /// the Seed).
    pub ignore_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PathSize {
    pub path: String,
    pub bytes: u64,
}

/// Measure a stored Seed (sizes only; nothing is hashed).
pub fn measure(hash: &str) -> Result<SeedSize, SeedError> {
    if !valid_hash(hash) {
        return Err(SeedError::new(codes::SEED_NOT_FOUND, format!("'{hash}' is not a Seed hash")));
    }
    let root = seed_path(hash);
    let mut sizes: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let (mut bytes, mut files) = (0u64, 0usize);
    walk(&root, &root, &BTreeSet::new(), &mut |rel, meta, _path| {
        if !meta.file_type().is_file() {
            return Ok(());
        }
        files += 1;
        bytes += meta.len();
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        for depth in 1..=parts.len().min(2) {
            *sizes.entry(parts[..depth].join("/")).or_default() += meta.len();
        }
        Ok(())
    })?;
    Ok(SeedSize { bytes, files, largest: largest_of(sizes), ignore_file: root.join(IGNORE_FILE).is_file() })
}

/// The [`LARGEST_SHOWN`] heaviest paths; a folder whose weight is almost
/// all one child is shown as that child (`.next/cache`, not `.next`).
fn largest_of(sizes: std::collections::BTreeMap<String, u64>) -> Vec<PathSize> {
    let mut all: Vec<PathSize> = sizes.into_iter().map(|(path, bytes)| PathSize { path, bytes }).collect();
    all.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    let mut out: Vec<PathSize> = Vec::new();
    // Every path ever shown, a folder its child replaced included.
    let mut shown: Vec<String> = Vec::new();
    for p in all {
        if out.len() >= LARGEST_SHOWN {
            break;
        }
        let under = |o: &str| p.path.starts_with(&format!("{o}/"));
        // A child carrying at least 90% of a folder still shown replaces it.
        if let Some(i) = out.iter().position(|o| under(&o.path) && p.bytes * 10 >= o.bytes * 9) {
            shown.push(p.path.clone());
            out[i] = p;
            continue;
        }
        // Any other child of a shown folder (or a parent of a shown child)
        // adds nothing.
        if shown.iter().any(|o| under(o) || o.starts_with(&format!("{}/", p.path))) {
            continue;
        }
        shown.push(p.path.clone());
        out.push(p);
    }
    out
}

/// Re-hash a stored Seed. `seed_corrupt` on any mismatch (§7.1).
pub fn verify(hash: &str) -> Result<TreeHash, SeedError> {
    if !valid_hash(hash) {
        return Err(SeedError::new(codes::SEED_NOT_FOUND, format!("'{hash}' is not a Seed hash")));
    }
    let p = seed_path(hash);
    if !p.is_dir() {
        return Err(SeedError::new(codes::SEED_NOT_FOUND, format!("Seed {hash} is not in the store")));
    }
    let th = hash_tree(&p, &BTreeSet::new()).map_err(|e| SeedError::new(codes::SEED_CORRUPT, e.message))?;
    if th.hash != hash {
        return Err(SeedError::new(
            codes::SEED_CORRUPT,
            format!("Seed {hash} was changed on disk (it now hashes to {}); refusing it", th.hash),
        ));
    }
    Ok(th)
}

/// Remove `.tmp-*` leftovers of builds interrupted by a daemon restart.
pub fn clean_tmp() {
    let Ok(rd) = fs::read_dir(super::seeds_dir()) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(".tmp-") {
            unseal(&e.path());
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::TempHome;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    fn folder(home: &Path) -> PathBuf {
        let f = home.join("src-app");
        write(&f.join("manifest.json"), "{}");
        write(&f.join("server.js"), "console.log(1)");
        write(&f.join("lib/a.js"), "a");
        write(&f.join(".git/HEAD"), "ref");
        f
    }

    #[test]
    fn a_folder_builds_to_a_stable_hash_and_reuses_the_store() {
        let home = TempHome::new();
        let f = folder(home.path());
        let ex = build_excludes(&[]);
        let a = store_folder(&f, &ex).expect("store");
        assert!(!a.reused);
        assert!(valid_hash(&a.hash));
        let b = store_folder(&f, &ex).expect("store again");
        assert_eq!(a.hash, b.hash, "same folder, same hash");
        assert!(b.reused);
        assert!(!seed_path(&a.hash).join(".git").exists(), ".git is never in a Seed");
        verify(&a.hash).expect("verifies");
        // A content change is a new hash.
        write(&f.join("lib/a.js"), "b");
        let c = store_folder(&f, &ex).expect("store changed");
        assert_ne!(a.hash, c.hash);
        // Excluding a changed top-level entry brings the old hash back.
        write(&f.join("tmp-cache/x"), "junk");
        write(&f.join("lib/a.js"), "a");
        let d = store_folder(&f, &build_excludes(&["tmp-cache/".into()])).expect("exclude");
        assert_eq!(d.hash, a.hash);
    }

    #[cfg(unix)]
    #[test]
    fn a_tampered_seed_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let home = TempHome::new();
        let f = folder(home.path());
        let s = store_folder(&f, &build_excludes(&[])).unwrap();
        let p = seed_path(&s.hash).join("server.js");
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444, "stored files are read-only");
        // The owner can still force a write (same uid): the re-hash catches it.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&p, "evil()").unwrap();
        let e = verify(&s.hash).unwrap_err();
        assert_eq!(e.code, codes::SEED_CORRUPT, "{e}");
        remove_tree(&s.hash).expect("remove works on a sealed tree");
        assert!(!seed_path(&s.hash).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_must_stay_inside() {
        let home = TempHome::new();
        let f = folder(home.path());
        std::os::unix::fs::symlink("../lib/a.js", f.join("lib/inner-ok")).unwrap();
        store_folder(&f, &build_excludes(&[])).expect("inner link is fine");
        std::os::unix::fs::symlink("/etc/passwd", f.join("escape")).unwrap();
        let e = store_folder(&f, &build_excludes(&[])).unwrap_err();
        assert_eq!(e.code, codes::SEED_INVALID);
        assert!(e.message.contains("outside"), "{e}");
        let leftovers: Vec<_> = fs::read_dir(super::super::seeds_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "a failed build leaves no temp tree");
    }

    /// 0.46.4 #3: a `.k2appignore` (gitignore syntax) keeps local state,
    /// env files and caches out of the Seed; a bad line refuses the build.
    #[test]
    fn k2appignore_keeps_matching_paths_out_of_the_seed() {
        let home = TempHome::new();
        let f = folder(home.path());
        write(&f.join("var/answers.json"), "{}");
        write(&f.join(".env.local"), "SECRET=1");
        write(&f.join(".env"), "SECRET=2");
        write(&f.join("node_modules/.cache/big"), &"x".repeat(8192));
        write(&f.join("node_modules/next/index.js"), &"n".repeat(2048));
        write(&f.join("lib/keep.env.js"), "k");
        let plain = store_folder(&f, &build_excludes(&[])).expect("no ignore file");
        assert!(seed_path(&plain.hash).join("var/answers.json").exists(), "without the file everything is packed");
        write(&f.join(IGNORE_FILE), "# local state\nvar/\n.env*\nnode_modules/.cache\n!.env.example\n");
        write(&f.join(".env.example"), "SECRET=");
        let s = store_folder(&f, &build_excludes(&[])).expect("store");
        let p = seed_path(&s.hash);
        for gone in ["var", ".env", ".env.local", "node_modules/.cache"] {
            assert!(!p.join(gone).exists(), "{gone} is ignored");
        }
        for kept in ["server.js", "lib/a.js", "lib/keep.env.js", "node_modules/next/index.js", ".env.example", IGNORE_FILE] {
            assert!(p.join(kept).exists(), "{kept} is packed");
        }
        assert_ne!(s.hash, plain.hash);
        verify(&s.hash).expect("verifies");
        // Stable: the same folder and file, the same Seed.
        assert_eq!(store_folder(&f, &build_excludes(&[])).unwrap().hash, s.hash);
        // The size report: what's in it, heaviest first.
        let m = measure(&s.hash).unwrap();
        assert!(m.ignore_file);
        assert_eq!(m.files, s.entries - count_dirs(&p), "files only");
        assert_eq!(m.bytes, s.size_bytes);
        assert_eq!(m.largest[0].path, "node_modules/next", "{:?}", m.largest);
        let big = measure(&plain.hash).unwrap();
        assert_eq!(big.largest[0].path, "node_modules/.cache", "the cache dominates node_modules: {:?}", big.largest);
        assert_eq!(big.largest[0].bytes, 8192);
        assert!(!big.ignore_file);
        assert!(big.largest.len() <= LARGEST_SHOWN);
        // A line the syntax can't read refuses the build.
        write(&f.join(IGNORE_FILE), "var/\n[unclosed\n");
        let e = store_folder(&f, &build_excludes(&[])).unwrap_err();
        assert_eq!(e.code, codes::SEED_INVALID, "{e}");
        assert!(e.message.contains(IGNORE_FILE), "{e}");
    }

    fn count_dirs(root: &Path) -> usize {
        let mut n = 0;
        walk(root, root, &BTreeSet::new(), &mut |_r, m, _p| {
            if m.file_type().is_dir() {
                n += 1;
            }
            Ok(())
        })
        .unwrap();
        n
    }

    #[test]
    fn largest_paths_prefer_the_child_that_carries_its_folder() {
        let m: std::collections::BTreeMap<String, u64> = [
            (".next", 100u64),
            (".next/cache", 95),
            (".next/server", 5),
            ("node_modules", 60),
            ("node_modules/a", 30),
            ("node_modules/b", 30),
            ("server.js", 1),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let got: Vec<String> = largest_of(m).into_iter().map(|p| p.path).collect();
        assert_eq!(got, vec![".next/cache", "node_modules", "server.js"]);
    }

    #[test]
    fn link_escape_rules() {
        assert!(link_stays_inside(Path::new("a/b"), Path::new("c")));
        assert!(link_stays_inside(Path::new("a/b"), Path::new("../c")));
        assert!(!link_stays_inside(Path::new("a/b"), Path::new("../../c")));
        assert!(!link_stays_inside(Path::new("b"), Path::new("/abs")));
    }
}
