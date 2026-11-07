//! K2's own Linux daemon byte checks, for project `k2`'s compute-built
//! daemons (a port of `scripts/lib/check-daemon-asset.py`, which k2sign
//! runs on the same asset):
//! - an ELF64 little-endian file for the arch;
//! - exactly one `K2_DAEMON_BUILD v=<version> os=linux arch=<arch>;` marker;
//! - no test-keys or test-pins flag from any K2 crate;
//! - k2-ptyd embedded: an ELF for the same arch inside the daemon whose
//!   sha256 hex the daemon also carries.

use crate::{refused, R};

const TEST_FLAGS: &[&[u8]] = &[
    b"K2_ROOT_HELPER_TEST_KEYS_ENABLED",
    b"K2_NODE_TEST_KEYS_ENABLED",
    b"K2_APP_ENGINE_TEST_KEYS_ENABLED",
    b"K2_SEW_TEST_PINS_ENABLED",
];

/// The arch of a K2 Linux daemon asset name, if it is one.
pub fn daemon_arch(asset: &str) -> Option<&'static str> {
    match asset {
        "k2-daemon-linux-x86_64" => Some("x86_64"),
        "k2-daemon-linux-aarch64" => Some("aarch64"),
        _ => None,
    }
}

fn machine_of(arch: &str) -> Option<u16> {
    match arch {
        "x86_64" => Some(62),
        "aarch64" => Some(183),
        _ => None,
    }
}

fn elf_machine(b: &[u8], at: usize) -> Option<u16> {
    if b.len() < at + 64 || &b[at..at + 4] != b"\x7fELF" || b[at + 4] != 2 || b[at + 5] != 1 {
        return None;
    }
    Some(u16::from_le_bytes([b[at + 18], b[at + 19]]))
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn markers(b: &[u8]) -> Vec<(String, String, String)> {
    let pre = b"K2_DAEMON_BUILD v=";
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = find(b, pre, i) {
        i = at + 1;
        let rest = &b[at + pre.len()..];
        let take = |s: &[u8], ok: fn(u8) -> bool| -> usize { s.iter().take_while(|c| ok(**c)).count() };
        let n = take(rest, |c| c.is_ascii_digit() || c == b'.');
        if n == 0 {
            continue;
        }
        let v = &rest[..n];
        let Some(r) = rest[n..].strip_prefix(b" os=") else { continue };
        let n2 = take(r, |c| c.is_ascii_lowercase());
        if n2 == 0 {
            continue;
        }
        let os = &r[..n2];
        let Some(r2) = r[n2..].strip_prefix(b" arch=") else { continue };
        let n3 = take(r2, |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
        if n3 == 0 || r2.get(n3) != Some(&b';') {
            continue;
        }
        let arch = &r2[..n3];
        let s = |x: &[u8]| String::from_utf8_lossy(x).into_owned();
        out.push((s(v), s(os), s(arch)));
    }
    out
}

/// Every problem with a daemon asset (empty = ok).
pub fn problems(b: &[u8], arch: &str, version: &str) -> Vec<String> {
    let Some(machine) = machine_of(arch) else {
        return vec![format!("unknown arch {arch:?}")];
    };
    let mut p = Vec::new();
    match elf_machine(b, 0) {
        None => p.push("not an ELF64 little-endian file".to_string()),
        Some(m) if m != machine => p.push(format!("ELF machine {m}, expected {machine} ({arch})")),
        _ => {}
    }
    let ms = markers(b);
    if ms.len() != 1 {
        p.push(format!("expected exactly one K2_DAEMON_BUILD marker, found {}", ms.len()));
    } else {
        let (v, os, a) = &ms[0];
        if v != version {
            p.push(format!("built as v{v}, release is v{version}"));
        }
        if os != "linux" || a != arch {
            p.push(format!("marker says os={os} arch={a}, expected linux {arch}"));
        }
    }
    for f in TEST_FLAGS {
        if find(b, f, 0).is_some() {
            p.push(format!("carries {} (a test build) — refusing to sign", String::from_utf8_lossy(f)));
        }
    }
    let mut ptyd = false;
    let mut i = 1;
    while let Some(at) = find(b, b"\x7fELF", i) {
        i = at + 1;
        if elf_machine(b, at) != Some(machine) {
            continue;
        }
        let shoff = u64::from_le_bytes(b[at + 0x28..at + 0x30].try_into().unwrap_or([0; 8]));
        let shentsize = u16::from_le_bytes([b[at + 0x3A], b[at + 0x3B]]) as u64;
        let shnum = u16::from_le_bytes([b[at + 0x3C], b[at + 0x3D]]) as u64;
        let size = shoff.saturating_add(shentsize * shnum);
        if size > 4096 && size <= (b.len() - at) as u64 {
            let sha = crate::ctx::sha256_hex(&b[at..at + size as usize]);
            if find(b, sha.as_bytes(), 0).is_some() {
                ptyd = true;
                break;
            }
        }
    }
    if !ptyd {
        p.push(format!("does not embed k2-ptyd (no embedded {arch} ELF whose sha256 the daemon carries; P84)"));
    }
    p
}

/// Refuse a daemon asset with any problem.
pub fn check(asset: &str, b: &[u8], version: &str) -> R<()> {
    let Some(arch) = daemon_arch(asset) else { return Ok(()) };
    let ps = problems(b, arch, version);
    if !ps.is_empty() {
        return refused(format!("{asset}: {}", ps.join("; ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(machine: u16, size: usize) -> Vec<u8> {
        let mut h = vec![0u8; 64];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[5] = 1;
        h[6] = 1;
        h[18..20].copy_from_slice(&machine.to_le_bytes());
        h[0x28..0x30].copy_from_slice(&((size - 128) as u64).to_le_bytes());
        h[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
        h[0x3C..0x3E].copy_from_slice(&2u16.to_le_bytes());
        h.resize(size, 0xA5);
        h
    }

    /// k2sign-s3b.sh's `mkset` daemon shape.
    fn daemon(version: &str) -> Vec<u8> {
        let ptyd = elf(62, 8192);
        let mut b = elf(62, 4096);
        b.extend_from_slice(&ptyd);
        b.extend_from_slice(crate::ctx::sha256_hex(&ptyd).as_bytes());
        b.extend_from_slice(format!("\0K2_DAEMON_BUILD v={version} os=linux arch=x86_64;").as_bytes());
        b
    }

    #[test]
    fn a_real_shaped_daemon_passes_and_wrong_ones_fail() {
        let d = daemon("0.47.1");
        assert!(problems(&d, "x86_64", "0.47.1").is_empty(), "{:?}", problems(&d, "x86_64", "0.47.1"));
        assert!(problems(&d, "x86_64", "0.47.2").iter().any(|p| p.contains("built as v0.47.1")));
        assert!(problems(&d, "aarch64", "0.47.1").iter().any(|p| p.contains("ELF machine 62")));
        let mut t = d.clone();
        t.extend_from_slice(b"K2_NODE_TEST_KEYS_ENABLED");
        assert!(problems(&t, "x86_64", "0.47.1").iter().any(|p| p.contains("test build")));
        let mut two = d.clone();
        two.extend_from_slice(b"\0K2_DAEMON_BUILD v=0.47.1 os=linux arch=x86_64;");
        assert!(problems(&two, "x86_64", "0.47.1").iter().any(|p| p.contains("found 2")));
        let no_ptyd = {
            let mut b = elf(62, 4096);
            b.extend_from_slice(b"\0K2_DAEMON_BUILD v=0.47.1 os=linux arch=x86_64;");
            b
        };
        assert!(problems(&no_ptyd, "x86_64", "0.47.1").iter().any(|p| p.contains("k2-ptyd")));
        assert!(check("k2-daemon-linux-x86_64", &d, "0.47.1").is_ok());
        assert!(check("acme", b"anything", "1.0.0").is_ok());
    }
}
