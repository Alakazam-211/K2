# Writes crates/k2-daemon/src/zen_lib_bundled.rs from src/shared/zen-lib.json
# (Gardens on the phone, Q5): run after scripts/vendor-zen-lib.sh changes a
# bundled library.   python3 scripts/zen-lib/gen-daemon-libs.py
import os, json
os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
m = json.load(open('src/shared/zen-lib.json'))
rows = []
for l in m['libs']:
    if l['source'] != 'bundled':
        continue
    key = f"{l['id']}@{l['version']}"
    for f in l['files']:
        rows.append(f'    ("{key}", "{f["name"]}", include_bytes!("../../../src/renderer/zen-lib/{key}/{f["name"]}")),')
head = '''//! The bundled widget libraries' bytes, served by the daemon (Gardens on
//! the phone, Rosson Q5, 0.46.6): K2 Companion carries no copy of K2's
//! widget library, so the remote Garden door serves every library, bundled
//! ones included, from these bytes (`zen_remote_routes`, `remote/lib`).
//!
//! The source is K2's own release: the files the desktop app ships in
//! `src/renderer/zen-lib/` (checked in, written by `scripts/vendor-zen-lib.sh`
//! from the pinned npm tarballs, or K2's own kit). Most of them are not an
//! upstream file byte for byte (prebundled to one classic script, rewritten
//! for the sealed frame, or K2's own code), so no public URL serves these
//! exact bytes; they ride in the daemon instead, and every file is checked
//! against its sha256 in `src/shared/zen-lib.json` before it is served.
//!
//! **Generated:** one line per file of every `bundled` row of
//! `src/shared/zen-lib.json`; a test fails when the table and the manifest
//! differ. After a vendor run, regenerate it with
//! `python3 scripts/zen-lib/gen-daemon-libs.py`.

/// `(key, file name, bytes)`, in manifest order.
pub static FILES: &[(&str, &str, &[u8])] = &[
'''
tail = '''];

/// The bytes of `file` in the bundled library `key` (`<id>@<version>`).
pub fn file(key: &str, name: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(k, n, _)| *k == key && *n == name).map(|(_, _, b)| *b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// The table is the manifest's bundled files, byte for byte (sha256 and
    /// size), and nothing else.
    #[test]
    fn the_table_is_every_bundled_file_of_the_manifest() {
        let m = k2_core::zen::stdlib::zen_lib_manifest();
        let mut want = Vec::new();
        for e in m.libs.iter().filter(|e| e.source == k2_core::zen::stdlib::LibSource::Bundled) {
            for f in &e.files {
                want.push((e.key(), f.name.clone()));
                let bytes = file(&e.key(), &f.name).unwrap_or_else(|| panic!("{}/{} is missing from the table; regenerate it", e.key(), f.name));
                assert_eq!(bytes.len() as u64, f.bytes, "{}/{} size", e.key(), f.name);
                let sum: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(sum, f.sha256, "{}/{} sha256", e.key(), f.name);
            }
        }
        let have: Vec<(String, String)> = FILES.iter().map(|(k, n, _)| (k.to_string(), n.to_string())).collect();
        assert_eq!(have, want, "the table and src/shared/zen-lib.json differ; regenerate the table");
    }
}
'''
open('crates/k2-daemon/src/zen_lib_bundled.rs', 'w').write(head + "\n".join(rows) + "\n" + tail)
print(len(rows))
