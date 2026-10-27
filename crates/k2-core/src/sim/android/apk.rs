//! Read an APK's package name and version from its binary
//! `AndroidManifest.xml` (Android's compiled XML), so `k2 sim install` can
//! say what it installed without `aapt` (platform-tools doesn't ship it).

use std::io::Read;
use std::path::Path;

const RES_STRING_POOL: u16 = 0x0001;
const RES_XML_START_ELEMENT: u16 = 0x0102;
const UTF8_FLAG: u32 = 0x100;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkInfo {
    pub package: String,
    pub version_name: Option<String>,
    pub version_code: Option<u32>,
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, String> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| format!("manifest truncated at {i}"))
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, String> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("manifest truncated at {i}"))
}

fn strings(b: &[u8], at: usize) -> Result<Vec<String>, String> {
    let count = u32_at(b, at + 8)? as usize;
    let flags = u32_at(b, at + 16)?;
    let strings_start = u32_at(b, at + 20)? as usize;
    if count > 200_000 {
        return Err("string pool too large".into());
    }
    let utf8 = flags & UTF8_FLAG != 0;
    let header = u16_at(b, at + 2)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = u32_at(b, at + header + i * 4)? as usize;
        let mut p = at + strings_start + off;
        if utf8 {
            // utf16 length (1–2 bytes), then utf8 length (1–2 bytes).
            let l16 = *b.get(p).ok_or("bad string")?;
            p += if l16 & 0x80 != 0 { 2 } else { 1 };
            let mut len = *b.get(p).ok_or("bad string")? as usize;
            if len & 0x80 != 0 {
                len = ((len & 0x7f) << 8) | *b.get(p + 1).ok_or("bad string")? as usize;
                p += 2;
            } else {
                p += 1;
            }
            out.push(String::from_utf8_lossy(b.get(p..p + len).ok_or("bad string")?).to_string());
        } else {
            let mut len = u16_at(b, p)? as usize;
            if len & 0x8000 != 0 {
                len = ((len & 0x7fff) << 16) | u16_at(b, p + 2)? as usize;
                p += 4;
            } else {
                p += 2;
            }
            let units: Vec<u16> = (0..len).map(|k| u16_at(b, p + k * 2)).collect::<Result<_, _>>()?;
            out.push(String::from_utf16_lossy(&units));
        }
    }
    Ok(out)
}

/// Parse a compiled manifest.
pub fn parse_manifest(b: &[u8]) -> Result<ApkInfo, String> {
    if u16_at(b, 0)? != 0x0003 {
        return Err("not a compiled Android manifest".into());
    }
    let mut i = u16_at(b, 2)? as usize;
    let mut pool: Vec<String> = Vec::new();
    while i + 8 <= b.len() {
        let ty = u16_at(b, i)?;
        let header = u16_at(b, i + 2)? as usize;
        let size = u32_at(b, i + 4)? as usize;
        if size < 8 || i + size > b.len() {
            return Err(format!("bad chunk at {i}"));
        }
        if ty == RES_STRING_POOL {
            pool = strings(b, i)?;
        } else if ty == RES_XML_START_ELEMENT {
            let name = u32_at(b, i + 20)? as usize;
            if pool.get(name).map(String::as_str) == Some("manifest") {
                let attr_start = u16_at(b, i + 24)? as usize;
                let attr_size = u16_at(b, i + 26)? as usize;
                let count = u16_at(b, i + 28)? as usize;
                let mut info = ApkInfo { package: String::new(), version_name: None, version_code: None };
                for k in 0..count {
                    let a = i + header + attr_start + k * attr_size;
                    let an = pool.get(u32_at(b, a + 4)? as usize).cloned().unwrap_or_default();
                    let raw = u32_at(b, a + 8)?;
                    let dtype = *b.get(a + 15).ok_or("bad attribute")?;
                    let data = u32_at(b, a + 16)?;
                    let s = if raw != u32::MAX { pool.get(raw as usize).cloned() } else if dtype == TYPE_STRING { pool.get(data as usize).cloned() } else { None };
                    match an.as_str() {
                        "package" => info.package = s.unwrap_or_default(),
                        "versionName" => info.version_name = s,
                        "versionCode" if dtype == TYPE_INT_DEC => info.version_code = Some(data),
                        _ => {}
                    }
                }
                if info.package.is_empty() {
                    return Err("the manifest has no package name".into());
                }
                return Ok(info);
            }
        }
        i += size;
    }
    Err("no <manifest> element".into())
}

/// Read `AndroidManifest.xml` out of an APK.
pub fn read(apk: &Path) -> Result<ApkInfo, String> {
    let f = std::fs::File::open(apk).map_err(|e| format!("open {}: {e}", apk.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{} is not an APK (zip): {e}", apk.display()))?;
    let mut m = z.by_name("AndroidManifest.xml").map_err(|_| format!("{} has no AndroidManifest.xml", apk.display()))?;
    if m.size() > 8 * 1024 * 1024 {
        return Err("AndroidManifest.xml is over 8 MB".into());
    }
    let mut b = Vec::new();
    m.read_to_end(&mut b).map_err(|e| e.to_string())?;
    parse_manifest(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal compiled manifest: string pool (UTF-16) + one
    /// `<manifest package=… versionName=… versionCode=7>` start element.
    pub(crate) fn manifest_bytes(package: &str) -> Vec<u8> {
        let strs = ["manifest", "package", "versionName", "versionCode", package, "1.2.3"];
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for s in strs {
            offsets.push(data.len() as u32);
            let u: Vec<u16> = s.encode_utf16().collect();
            data.extend_from_slice(&(u.len() as u16).to_le_bytes());
            for c in u {
                data.extend_from_slice(&c.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let pool_header = 28usize;
        let strings_start = pool_header + offsets.len() * 4;
        let mut pool = Vec::new();
        pool.extend_from_slice(&RES_STRING_POOL.to_le_bytes());
        pool.extend_from_slice(&(pool_header as u16).to_le_bytes());
        pool.extend_from_slice(&((strings_start + data.len()) as u32).to_le_bytes());
        pool.extend_from_slice(&(strs.len() as u32).to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        pool.extend_from_slice(&(strings_start as u32).to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        for o in offsets {
            pool.extend_from_slice(&o.to_le_bytes());
        }
        pool.extend_from_slice(&data);
        let attrs: [(u32, u32, u8, u32); 3] = [(1, 4, TYPE_STRING, 4), (2, 5, TYPE_STRING, 5), (3, u32::MAX, TYPE_INT_DEC, 7)];
        let mut el = Vec::new();
        el.extend_from_slice(&RES_XML_START_ELEMENT.to_le_bytes());
        el.extend_from_slice(&16u16.to_le_bytes());
        el.extend_from_slice(&((16 + 20 + attrs.len() * 20) as u32).to_le_bytes());
        el.extend_from_slice(&1u32.to_le_bytes()); // line
        el.extend_from_slice(&u32::MAX.to_le_bytes()); // comment
        el.extend_from_slice(&u32::MAX.to_le_bytes()); // ns
        el.extend_from_slice(&0u32.to_le_bytes()); // name = "manifest"
        el.extend_from_slice(&20u16.to_le_bytes()); // attributeStart
        el.extend_from_slice(&20u16.to_le_bytes()); // attributeSize
        el.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
        el.extend_from_slice(&[0u8; 6]); // id, class, style
        for (name, raw, ty, data) in attrs {
            el.extend_from_slice(&u32::MAX.to_le_bytes());
            el.extend_from_slice(&name.to_le_bytes());
            el.extend_from_slice(&raw.to_le_bytes());
            el.extend_from_slice(&8u16.to_le_bytes());
            el.push(0);
            el.push(ty);
            el.extend_from_slice(&data.to_le_bytes());
        }
        let mut doc = Vec::new();
        doc.extend_from_slice(&0x0003u16.to_le_bytes());
        doc.extend_from_slice(&8u16.to_le_bytes());
        doc.extend_from_slice(&((8 + pool.len() + el.len()) as u32).to_le_bytes());
        doc.extend_from_slice(&pool);
        doc.extend_from_slice(&el);
        doc
    }

    #[test]
    fn reads_package_and_version() {
        let info = parse_manifest(&manifest_bytes("com.acme.app")).expect("parses");
        assert_eq!(info, ApkInfo { package: "com.acme.app".into(), version_name: Some("1.2.3".into()), version_code: Some(7) });
        assert!(parse_manifest(b"PK\x03\x04junk").is_err());
        assert!(parse_manifest(&[3, 0, 8, 0, 8, 0, 0, 0]).is_err(), "no manifest element");
    }
}
