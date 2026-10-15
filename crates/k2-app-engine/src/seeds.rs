//! Seeds on the engine side: `<k2app home>/seeds/<hash>/`, unpacked from a
//! tar the daemon streams (the engine can't read the daemon user's home,
//! and shouldn't). The daemon verified the Seed against its hash; the
//! engine trusts the daemon process but still unpacks defensively:
//!
//! - into a fresh temp dir, renamed into place only when complete;
//! - regular files, directories and symlinks only (no hard links, devices,
//!   fifos); no absolute or `..` member paths (the tar crate also refuses
//!   to write through a symlink outside the destination);
//! - no ownership, no xattrs, no setuid/setgid/sticky: directories become
//!   0755, files 0644 or 0755 (any execute bit kept), so a container whose
//!   ids aren't `k2app`'s can still read its Seed (it is mounted
//!   read-only).

use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub fn seed_path(home_seeds: &Path, hash: &str) -> PathBuf {
    home_seeds.join(hash)
}

fn member_ok(p: &Path) -> bool {
    !p.as_os_str().is_empty()
        && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Unpack exactly `len` bytes of tar from `r` as Seed `hash` under
/// `seeds`. `Ok(false)` when the Seed was already there (the body is
/// still read to the end so the connection stays in step).
pub fn put(seeds: &Path, hash: &str, len: u64, r: &mut dyn Read) -> Result<bool, String> {
    let dst = seed_path(seeds, hash);
    let mut limited = r.take(len);
    if dst.is_dir() {
        std::io::copy(&mut limited, &mut std::io::sink()).map_err(|e| format!("read body: {e}"))?;
        return Ok(false);
    }
    let tmp = seeds.join(format!(".tmp-{hash}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir(&tmp).map_err(|e| format!("mkdir {}: {e}", tmp.display()))?;
    let res = unpack_into(&tmp, &mut limited).and_then(|_| {
        // The rest of the body (tar padding past the end marker).
        std::io::copy(&mut limited, &mut std::io::sink()).map_err(|e| format!("read body: {e}"))?;
        normalize(&tmp)
    });
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    match std::fs::rename(&tmp, &dst) {
        Ok(()) => Ok(true),
        Err(_) if dst.is_dir() => {
            // Another put of the same Seed won the race.
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(false)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(format!("place Seed: {e}"))
        }
    }
}

fn unpack_into(dst: &Path, r: &mut dyn Read) -> Result<(), String> {
    let mut a = tar::Archive::new(r);
    a.set_preserve_permissions(false);
    a.set_preserve_ownerships(false);
    a.set_unpack_xattrs(false);
    a.set_overwrite(false);
    for e in a.entries().map_err(|e| format!("tar: {e}"))? {
        let mut e = e.map_err(|e| format!("tar: {e}"))?;
        let path = e.path().map_err(|e| format!("tar path: {e}"))?.into_owned();
        if !member_ok(&path) {
            return Err(format!("Seed member path refused: {}", path.display()));
        }
        match e.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Directory | tar::EntryType::Symlink => {}
            tar::EntryType::GNULongName | tar::EntryType::GNULongLink | tar::EntryType::XHeader | tar::EntryType::XGlobalHeader => {}
            other => return Err(format!("Seed member type refused ({other:?}): {}", path.display())),
        }
        if !e.unpack_in(dst).map_err(|err| format!("unpack {}: {err}", path.display()))? {
            return Err(format!("Seed member escapes the Seed: {}", path.display()));
        }
    }
    Ok(())
}

/// Directories 0755; files 0644, or 0755 when any execute bit was set.
/// Never follows a symlink.
fn normalize(root: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("chmod {}: {e}", d.display()))?;
        for ent in std::fs::read_dir(&d).map_err(|e| format!("read {}: {e}", d.display()))? {
            let ent = ent.map_err(|e| e.to_string())?;
            let ft = ent.file_type().map_err(|e| e.to_string())?;
            let p = ent.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                let mode = ent.metadata().map_err(|e| e.to_string())?.permissions().mode();
                let want = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(want)).map_err(|e| format!("chmod {}: {e}", p.display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tar_of(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        build(&mut b);
        b.into_inner().unwrap()
    }

    fn file(b: &mut tar::Builder<Vec<u8>>, path: &str, body: &[u8], mode: u32) {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(mode);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, path, body).unwrap();
    }

    #[test]
    fn unpacks_normalizes_and_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let t = tar_of(|b| {
            file(b, "server.mjs", b"console.log(1)", 0o4777);
            file(b, "node_modules/x/bin.js", b"#!", 0o700);
            file(b, "node_modules/x/index.js", b"x", 0o600);
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "node_modules/.bin/x", "../x/bin.js").unwrap();
        });
        let h = "a".repeat(64);
        assert!(put(d.path(), &h, t.len() as u64, &mut &t[..]).unwrap());
        let s = d.path().join(&h);
        let mode = |p: &str| std::fs::metadata(s.join(p)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode("server.mjs"), 0o755, "setuid dropped, exec kept");
        assert_eq!(mode("node_modules/x/bin.js"), 0o755);
        assert_eq!(mode("node_modules/x/index.js"), 0o644);
        assert!(std::fs::symlink_metadata(s.join("node_modules/.bin/x")).unwrap().file_type().is_symlink());
        // Again: already there, body consumed, nothing changes.
        let mut cur = std::io::Cursor::new(t.clone());
        assert!(!put(d.path(), &h, t.len() as u64, &mut cur).unwrap());
        assert_eq!(cur.position(), t.len() as u64);
    }

    #[test]
    fn refuses_escapes_and_odd_members() {
        let d = tempfile::tempdir().unwrap();
        // A link out, then a file written through it.
        let t = tar_of(|b| {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "out", d.path().join("victim").to_str().unwrap()).unwrap();
            file(b, "out/pwn", b"x", 0o644);
        });
        std::fs::create_dir(d.path().join("victim")).unwrap();
        let seeds = d.path().join("seeds");
        std::fs::create_dir(&seeds).unwrap();
        assert!(put(&seeds, &"b".repeat(64), t.len() as u64, &mut &t[..]).is_err());
        assert!(!d.path().join("victim/pwn").exists());
        assert!(!seeds.join("b".repeat(64)).exists(), "nothing placed");
        // A hard link and a fifo are refused.
        let t = tar_of(|b| {
            file(b, "a", b"x", 0o644);
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            b.append_link(&mut h, "b", "a").unwrap();
        });
        assert!(put(&seeds, &"c".repeat(64), t.len() as u64, &mut &t[..]).is_err());
        let t = tar_of(|b| {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Fifo);
            h.set_size(0);
            h.set_path("f").unwrap();
            h.set_cksum();
            b.append(&h, &[][..]).unwrap();
        });
        assert!(put(&seeds, &"d".repeat(64), t.len() as u64, &mut &t[..]).is_err());
        assert!(std::fs::read_dir(&seeds).unwrap().next().is_none(), "temp dirs cleaned");
    }
}
