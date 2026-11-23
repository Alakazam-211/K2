//! Uploading a Simulator `.app` to a node (review IR-3): the rules both
//! ends apply to every archive entry.
//!
//! The server packs the bundle as a tar that **never follows links**
//! (`inside_workspace` checks only the top path, so a link inside
//! `Foo.app` pointing at `~/.k2/…` would otherwise ship that file). The
//! node unpacks it with the same rules before writing anything:
//!
//! - one top folder ending in `.app`; relative paths of normal components
//!   only (no absolute path, `.`, `..`);
//! - only folders, files and symlinks whose target stays inside the bundle
//!   (frameworks use `Versions/Current -> A`); no hardlinks, devices,
//!   FIFOs or sockets;
//! - nothing written through a link declared earlier;
//! - at most [`MAX_BYTES`] and [`MAX_ENTRIES`].

use std::path::{Component, Path, PathBuf};

/// Total file bytes in one upload (§3.6).
pub const MAX_BYTES: u64 = 1 << 30;
pub const MAX_ENTRIES: u64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    /// A hardlink, device, FIFO, socket or anything else.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackError(pub String);

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err(m: impl Into<String>) -> PackError {
    PackError(m.into())
}

/// A relative path of normal components only.
pub fn clean_rel(p: &Path) -> bool {
    !p.as_os_str().is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Does a symlink at `entry` (relative to the archive root) with `target`
/// resolve inside the bundle folder (the entry's first component)?
/// Absolute targets never do.
pub fn link_stays_inside(entry: &Path, target: &Path) -> bool {
    if target.is_absolute() || target.as_os_str().is_empty() {
        return false;
    }
    let mut stack: Vec<&std::ffi::OsStr> = entry.parent().map(|p| p.components().filter_map(|c| if let Component::Normal(s) = c { Some(s) } else { None }).collect()).unwrap_or_default();
    for c in target.components() {
        match c {
            Component::Normal(s) => stack.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    // Still inside the top folder (the bundle), not the archive root.
    !stack.is_empty() && entry.components().next().map(|c| c.as_os_str()) == stack.first().copied()
}

/// Checks entries in archive order.
#[derive(Debug, Default)]
pub struct Guard {
    top: Option<PathBuf>,
    links: Vec<PathBuf>,
    bytes: u64,
    entries: u64,
}

impl Guard {
    pub fn new() -> Self {
        Self::default()
    }

    /// One entry. Returns its clean relative path, or why the whole upload
    /// is refused.
    pub fn entry(&mut self, path: &Path, kind: EntryKind, size: u64, link_target: Option<&Path>) -> Result<PathBuf, PackError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(err(format!("the app has more than {MAX_ENTRIES} entries")));
        }
        // tar writes folders with a trailing slash; Path ignores it.
        if !clean_rel(path) {
            return Err(err(format!("{} isn't a plain relative path inside the app", path.display())));
        }
        let top = path.components().next().map(|c| PathBuf::from(c.as_os_str())).unwrap_or_default();
        match &self.top {
            None => {
                if top.extension().and_then(|e| e.to_str()) != Some("app") {
                    return Err(err("an upload holds one folder ending in .app"));
                }
                self.top = Some(top);
            }
            Some(t) if *t != top => return Err(err("an upload holds one .app folder only")),
            Some(_) => {}
        }
        if self.links.iter().any(|l| path.starts_with(l) && path != l) {
            return Err(err(format!("{} goes through a link", path.display())));
        }
        match kind {
            EntryKind::Dir => {}
            EntryKind::File => {
                self.bytes = self.bytes.saturating_add(size);
                if self.bytes > MAX_BYTES {
                    return Err(err(format!("the app is over {} MB", MAX_BYTES >> 20)));
                }
            }
            EntryKind::Symlink => {
                let t = link_target.ok_or_else(|| err(format!("{} is a link with no target", path.display())))?;
                if !link_stays_inside(path, t) {
                    return Err(err(format!("{} links outside the app ({})", path.display(), t.display())));
                }
                self.links.push(path.to_path_buf());
            }
            EntryKind::Other => {
                return Err(err(format!("{} is a hardlink or special file; an app upload holds only folders, files and links inside the app", path.display())));
            }
        }
        Ok(path.to_path_buf())
    }

    /// The bundle folder's name (`Foo.app`) once an entry was seen.
    pub fn top(&self) -> Option<&Path> {
        self.top.as_deref()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Pack the `.app` folder at `app` as a tar that never follows links. Every
/// entry passes the [`Guard`] first, so the server refuses an outward link,
/// a hardlink or a special file before a byte leaves the box.
#[cfg(unix)]
pub fn pack_app(app: &Path) -> Result<Vec<u8>, PackError> {
    use std::os::unix::fs::MetadataExt;
    let name = app.file_name().map(PathBuf::from).ok_or_else(|| err("the app has no folder name"))?;
    let top = std::fs::symlink_metadata(app).map_err(|e| err(format!("{}: {e}", app.display())))?;
    if !top.is_dir() {
        return Err(err(format!("{} isn't a folder", app.display())));
    }
    let mut guard = Guard::new();
    let mut b = tar::Builder::new(Vec::new());
    b.follow_symlinks(false);
    b.mode(tar::HeaderMode::Deterministic);
    let mut stack = vec![(app.to_path_buf(), name.clone())];
    while let Some((full, rel)) = stack.pop() {
        let m = std::fs::symlink_metadata(&full).map_err(|e| err(format!("{}: {e}", rel.display())))?;
        let ft = m.file_type();
        let (kind, target) = if ft.is_symlink() {
            (EntryKind::Symlink, Some(std::fs::read_link(&full).map_err(|e| err(format!("{}: {e}", rel.display())))?))
        } else if ft.is_dir() {
            (EntryKind::Dir, None)
        } else if ft.is_file() && m.nlink() <= 1 {
            (EntryKind::File, None)
        } else {
            (EntryKind::Other, None)
        };
        guard.entry(&rel, kind, m.len(), target.as_deref())?;
        b.append_path_with_name(&full, &rel).map_err(|e| err(format!("{}: {e}", rel.display())))?;
        if kind == EntryKind::Dir {
            let mut kids: Vec<_> = std::fs::read_dir(&full).map_err(|e| err(format!("{}: {e}", rel.display())))?.flatten().map(|e| e.file_name()).collect();
            kids.sort();
            for k in kids.into_iter().rev() {
                stack.push((full.join(&k), rel.join(&k)));
            }
        }
    }
    b.into_inner().map_err(|e| err(format!("pack: {e}")))
}

/// Unpack an uploaded app under `dest` (a fresh folder the node owns),
/// checking every entry before writing it. Returns `dest/<Name>.app`.
#[cfg(unix)]
pub fn unpack_app(bytes: &[u8], dest: &Path) -> Result<PathBuf, PackError> {
    use std::io::Read;
    std::fs::create_dir_all(dest).map_err(|e| err(format!("{}: {e}", dest.display())))?;
    let mut guard = Guard::new();
    let mut a = tar::Archive::new(bytes);
    for e in a.entries().map_err(|e| err(format!("the upload isn't a tar: {e}")))? {
        let mut e = e.map_err(|e| err(format!("the upload is damaged: {e}")))?;
        let path = e.path().map_err(|e| err(format!("bad entry path: {e}")))?.into_owned();
        let ty = e.header().entry_type();
        let kind = if ty.is_dir() {
            EntryKind::Dir
        } else if ty.is_file() {
            EntryKind::File
        } else if ty.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Other
        };
        let target = e.link_name().map_err(|e| err(format!("bad link: {e}")))?.map(|c| c.into_owned());
        let size = e.header().size().unwrap_or(u64::MAX);
        let rel = guard.entry(&path, kind, size, target.as_deref())?;
        let out = dest.join(&rel);
        match kind {
            EntryKind::Dir => std::fs::create_dir_all(&out).map_err(|e| err(format!("{}: {e}", rel.display())))?,
            EntryKind::File => {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| err(format!("{}: {e}", rel.display())))?;
                }
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&out).map_err(|e| err(format!("{}: {e}", rel.display())))?;
                let n = std::io::copy(&mut (&mut e).take(size), &mut f).map_err(|e| err(format!("{}: {e}", rel.display())))?;
                if n != size {
                    return Err(err(format!("{} is short", rel.display())));
                }
            }
            EntryKind::Symlink => {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| err(format!("{}: {e}", rel.display())))?;
                }
                let t = target.ok_or_else(|| err("a link with no target"))?;
                std::os::unix::fs::symlink(&t, &out).map_err(|e| err(format!("{}: {e}", rel.display())))?;
            }
            EntryKind::Other => unreachable!("the guard refuses it"),
        }
    }
    let top = guard.top().ok_or_else(|| err("the upload is empty"))?;
    Ok(dest.join(top))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn a_plain_bundle_with_an_inner_link_passes() {
        let mut g = Guard::new();
        g.entry(p("A.app"), EntryKind::Dir, 0, None).unwrap();
        g.entry(p("A.app/Info.plist"), EntryKind::File, 10, None).unwrap();
        g.entry(p("A.app/Frameworks/F.framework/Versions/A/F"), EntryKind::File, 10, None).unwrap();
        g.entry(p("A.app/Frameworks/F.framework/Versions/Current"), EntryKind::Symlink, 0, Some(p("A"))).unwrap();
        g.entry(p("A.app/Frameworks/F.framework/F"), EntryKind::Symlink, 0, Some(p("Versions/Current/F"))).unwrap();
        assert_eq!(g.top(), Some(p("A.app")));
        assert_eq!(g.bytes(), 20);
    }

    #[test]
    fn escapes_and_special_files_are_refused() {
        let cases: Vec<(&str, EntryKind, Option<&str>)> = vec![
            ("/etc/passwd", EntryKind::File, None),
            ("A.app/../../x", EntryKind::File, None),
            ("./A.app/x", EntryKind::File, None),
            ("A.app/k2", EntryKind::Symlink, Some("/Users/me/.k2/state.db")),
            ("A.app/k2", EntryKind::Symlink, Some("../../.k2")),
            ("A.app/up", EntryKind::Symlink, Some("..")),
            ("A.app/hard", EntryKind::Other, None),
        ];
        for (path, kind, target) in cases {
            let mut g = Guard::new();
            g.entry(p("A.app"), EntryKind::Dir, 0, None).unwrap();
            assert!(g.entry(p(path), kind, 0, target.map(Path::new)).is_err(), "{path} -> {target:?}");
        }
        // Not a .app, or two tops.
        assert!(Guard::new().entry(p("x/y"), EntryKind::File, 1, None).is_err());
        let mut g = Guard::new();
        g.entry(p("A.app/x"), EntryKind::File, 1, None).unwrap();
        assert!(g.entry(p("B.app/x"), EntryKind::File, 1, None).is_err());
    }

    #[test]
    fn nothing_is_written_through_a_link() {
        let mut g = Guard::new();
        g.entry(p("A.app/l"), EntryKind::Symlink, 0, Some(p("sub"))).unwrap();
        assert!(g.entry(p("A.app/l/evil"), EntryKind::File, 1, None).is_err());
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-pack-{tag}-{}-{}", std::process::id(), crate::crypto::random_id().unwrap()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn app(root: &Path) -> PathBuf {
        let a = root.join("K2Fixture.app");
        std::fs::create_dir_all(a.join("Frameworks/F.framework/Versions/A")).unwrap();
        std::fs::write(a.join("Info.plist"), "<plist/>").unwrap();
        std::fs::write(a.join("K2Fixture"), b"\xcf\xfa\xed\xfe").unwrap();
        std::fs::write(a.join("Frameworks/F.framework/Versions/A/F"), b"f").unwrap();
        std::os::unix::fs::symlink("A", a.join("Frameworks/F.framework/Versions/Current")).unwrap();
        a
    }

    #[test]
    fn pack_and_unpack_round_trip_keeping_inner_links() {
        let src = tmp("src");
        let a = app(&src);
        let bytes = pack_app(&a).unwrap();
        let dest = tmp("dest");
        let out = unpack_app(&bytes, &dest).unwrap();
        assert_eq!(out, dest.join("K2Fixture.app"));
        assert_eq!(std::fs::read(out.join("Info.plist")).unwrap(), b"<plist/>");
        assert_eq!(std::fs::read_link(out.join("Frameworks/F.framework/Versions/Current")).unwrap(), PathBuf::from("A"));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// IR-3: a link inside the bundle pointing at `~/.k2/…` must never ship
    /// the file it points at.
    #[test]
    fn the_server_refuses_a_bundle_with_an_outward_link_or_a_hardlink() {
        let src = tmp("out");
        let secret = src.join("state.db");
        std::fs::write(&secret, b"k2-canary-not-for-the-node").unwrap();
        let a = app(&src);
        std::os::unix::fs::symlink(&secret, a.join("db")).unwrap();
        let e = pack_app(&a).unwrap_err();
        assert!(e.0.contains("links outside the app"), "{e}");
        std::fs::remove_file(a.join("db")).unwrap();
        std::os::unix::fs::symlink("../state.db", a.join("db")).unwrap();
        assert!(pack_app(&a).is_err(), "a relative link out is refused too");
        std::fs::remove_file(a.join("db")).unwrap();
        std::fs::hard_link(&secret, a.join("hard")).unwrap();
        assert!(pack_app(&a).unwrap_err().0.contains("hardlink"));
        std::fs::remove_file(a.join("hard")).unwrap();
        assert!(pack_app(&a).is_ok());
        let _ = std::fs::remove_dir_all(&src);
    }

    /// A tar built by hand (no checks), as a hostile server could send.
    fn raw_tar(entries: &[(&str, tar::EntryType, &str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, ty, link, data) in entries {
            let mut h = tar::Header::new_old();
            {
                let old = h.as_old_mut();
                old.name[..name.len()].copy_from_slice(name.as_bytes());
                old.linkname[..link.len()].copy_from_slice(link.as_bytes());
            }
            h.set_entry_type(*ty);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn the_node_refuses_hostile_archives_before_writing_them() {
        use tar::EntryType as T;
        type Entry<'e> = (&'e str, T, &'e str, &'e [u8]);
        let cases: Vec<Vec<Entry<'_>>> = vec![
            vec![("A.app/../../escape", T::Regular, "", b"x")],
            vec![("/etc/evil", T::Regular, "", b"x")],
            vec![("A.app/l", T::Symlink, "/var/k2node", b"")],
            vec![("A.app/l", T::Symlink, "../..", b"")],
            vec![("A.app/h", T::Link, "A.app/Info.plist", b"")],
            vec![("A.app/c", T::Char, "", b"")],
            vec![("A.app/l", T::Symlink, "sub", b""), ("A.app/l/through", T::Regular, "", b"x")],
        ];
        for entries in cases {
            let dest = tmp("hostile");
            let r = unpack_app(&raw_tar(&entries), &dest);
            assert!(r.is_err(), "{:?} must be refused", entries.iter().map(|e| e.0).collect::<Vec<_>>());
            assert!(!dest.parent().unwrap().join("escape").exists());
            let _ = std::fs::remove_dir_all(&dest);
        }
    }

    #[test]
    fn size_is_capped() {
        let mut g = Guard::new();
        g.entry(p("A.app/a"), EntryKind::File, MAX_BYTES, None).unwrap();
        assert!(g.entry(p("A.app/b"), EntryKind::File, 1, None).is_err());
    }
}
