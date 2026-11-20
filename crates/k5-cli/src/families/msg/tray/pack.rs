//! `_msg_inbox_pack_tar_gz`: a flat `.tar.gz` of the files, as the
//! shell's Python `tarfile.open(out, "w:gz")` writes it (POSIX pax), each
//! under its base name (a repeat gets `-2`, `-3`, … before its last
//! extension), and base64 as Python's `base64.b64encode`.
//!
//! What a reader sees is the same: the names, the types (a symlink stays
//! a symlink, as `tarfile.add` takes it from `lstat`), the modes and the
//! bytes. The archive's own bytes are not (the owner names Python looks
//! up, the gzip header's time), so the parity test compares contents.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One archive name per file, as the shell's loop picks them.
pub fn names(paths: &[String]) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    for p in paths {
        let base = Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty() && !p.ends_with('/'))
            .unwrap_or_else(|| "attachment".into());
        let mut name = base.clone();
        let mut n = 2;
        while used.contains(&name) {
            name = match base.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{n}.{ext}"),
                _ => format!("{base}-{n}"),
            };
            n += 1;
        }
        used.push(name);
    }
    used
}

/// Packs `paths` into a temp archive (`mktemp`, as the shell does): its
/// path, or the shell's line and `Err`.
pub fn bundle(paths: &[String]) -> Result<PathBuf, ()> {
    let Some(out) = super::mktemp(&format!(
        "{}/k2-msg-inbox-bundle.XXXXXX.tar.gz",
        super::tmpdir()
    )) else {
        crate::out::stderr_line("Error: cannot create temp archive for multi-file inbox");
        return Err(());
    };
    if write_archive(&out, paths).is_err() {
        let _ = std::fs::remove_file(&out);
        crate::out::stderr_line("Error: failed to pack multi-file inbox bundle");
        return Err(());
    }
    Ok(out)
}

fn write_archive(out: &Path, paths: &[String]) -> std::io::Result<()> {
    let file = std::fs::File::create(out)?;
    let mut gz = GzEncoder::new(file, Compression::default());
    for (p, name) in paths.iter().zip(names(paths)) {
        entry(&mut gz, Path::new(p), &name)?;
    }
    // The end: two zero blocks.
    gz.write_all(&[0u8; 1024])?;
    gz.finish()?.sync_all()
}

fn octal(field: &mut [u8], v: u64) {
    let width = field.len() - 1;
    let s = format!("{v:0width$o}");
    let b = s.as_bytes();
    let b = &b[b.len().saturating_sub(width)..];
    field[..b.len()].copy_from_slice(b);
    field[width] = 0;
}

fn header(name: &[u8], mode: u32, uid: u64, gid: u64, size: u64, mtime: u64, kind: u8, link: &[u8]) -> [u8; 512] {
    let mut h = [0u8; 512];
    let n = name.len().min(100);
    h[..n].copy_from_slice(&name[..n]);
    octal(&mut h[100..108], u64::from(mode));
    octal(&mut h[108..116], uid.min(0o7777777));
    octal(&mut h[116..124], gid.min(0o7777777));
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime);
    h[156] = kind;
    let l = link.len().min(100);
    h[157..157 + l].copy_from_slice(&link[..l]);
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    h
}

/// One pax record, its length counting itself.
fn record(key: &str, value: &[u8]) -> Vec<u8> {
    let body_len = key.len() + value.len() + 3;
    let mut len = body_len + 1;
    while format!("{len}").len() + body_len != len {
        len = format!("{len}").len() + body_len;
    }
    let mut r = format!("{len} {key}=").into_bytes();
    r.extend_from_slice(value);
    r.push(b'\n');
    r
}

fn pad(w: &mut impl Write, n: u64) -> std::io::Result<()> {
    let rem = (n % 512) as usize;
    if rem != 0 {
        w.write_all(&vec![0u8; 512 - rem])?;
    }
    Ok(())
}

fn entry(w: &mut impl Write, path: &Path, name: &str) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    let (kind, link, size) = if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        (b'2', target.into_os_string().into_encoded_bytes(), 0)
    } else {
        (b'0', Vec::new(), meta.len())
    };
    let name_b = name.as_bytes();
    let mut pax = Vec::new();
    if name_b.len() > 100 || !name.is_ascii() {
        pax.extend(record("path", name_b));
    }
    if link.len() > 100 || !link.is_ascii() {
        pax.extend(record("linkpath", &link));
    }
    let mtime = u64::try_from(meta.mtime()).unwrap_or(0);
    if !pax.is_empty() {
        let h = header(b"././@PaxHeader", 0o644, 0, 0, pax.len() as u64, mtime, b'x', b"");
        w.write_all(&h)?;
        w.write_all(&pax)?;
        pad(w, pax.len() as u64)?;
    }
    let h = header(
        name_b,
        meta.mode() & 0o7777,
        u64::from(meta.uid()),
        u64::from(meta.gid()),
        size,
        mtime,
        kind,
        &link,
    );
    w.write_all(&h)?;
    if kind == b'0' {
        let mut f = std::fs::File::open(path)?;
        let copied = std::io::copy(&mut f, w)?;
        if copied != size {
            return Err(std::io::Error::other("the file changed while it was packed"));
        }
        pad(w, size)?;
    }
    Ok(())
}

/// `base64.b64encode(data).decode("ascii")`.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_pythons() {
        for (raw, want) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"\xff\x00\xfe\x01", "/wD+AQ=="),
        ] {
            assert_eq!(base64(raw), want);
        }
    }

    #[test]
    fn repeats_get_a_number_before_the_last_extension() {
        let paths: Vec<String> = ["/a/r.md", "/b/r.md", "/c/r.md", "/a/x", "/b/x", "/c/.env", "/d/.env"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            names(&paths),
            ["r.md", "r-2.md", "r-3.md", "x", "x-2", ".env", ".env-2"]
        );
    }

    #[test]
    fn a_pax_record_counts_its_own_length() {
        let r = record("path", b"abc");
        assert_eq!(r, b"12 path=abc\n");
        let long = vec![b'x'; 95];
        let r = record("path", &long);
        assert_eq!(r.len(), format!("{}", r.len()).len() + 1 + 5 + 95 + 1);
        assert!(r.starts_with(format!("{} path=", r.len()).as_bytes()));
    }
}
