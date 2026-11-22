//! Which update keys a build compiles in, from the brand file's `[update]`
//! table (`brand/<id>/brand.toml`). `build.rs` uses this module (`#[path]`),
//! and so do this crate's tests.
//!
//! - Brands `k2` and `k5` ship K2's own keys: the list the update-key
//!   ceremony writes between the `UPDATER_KEYS` markers in `lib.rs`. Their
//!   `[update]` must equal that list exactly (role, key id, key, in order);
//!   the build then compiles the ceremony's list as it is.
//! - Any other brand ships its own keys, compiled from its `[update]`. It
//!   may not carry one of K2's keys, nor the retired U1, by key id or by key.
//!
//! Every refusal names the file and the field.

use base64::Engine as _;

/// The brands that ship K2's own update keys.
pub const OWN_KEY_BRANDS: &[&str] = &["k2", "k5"];

/// One update key, as the brand file or the ceremony's list spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub role: String,
    pub key_id: String,
    pub pubkey: String,
}

/// What a build compiles in.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    /// K2's own keys: the ceremony's list in `lib.rs`, unchanged.
    Own,
    /// The brand's own keys, from its brand file.
    Brand(Vec<Key>),
}

/// minisign's key id of a public key line: "Ed", the 8 id bytes as a
/// little-endian u64 (16 upper hex), the 32 key bytes.
pub fn key_id_of(line: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(line)
        .ok()?;
    if raw.len() != 42 || &raw[0..2] != b"Ed" {
        return None;
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw[2..10]);
    Some(format!("{:016X}", u64::from_le_bytes(id)))
}

/// The value of `name: "..."` at or after `from` in `s`, and where it ends.
fn quoted_after(s: &str, from: usize, name: &str) -> Option<(String, usize)> {
    let at = from + s[from..].find(&format!("{name}: \""))? + name.len() + 3;
    let end = at + s[at..].find('"')?;
    Some((s[at..end].to_string(), end))
}

/// K2's own keys (the ceremony's `UPDATER_KEYS` block in `lib.rs`) and the
/// retired key U1 (`LEGACY_UPDATER_KEY_ID` / `_B64`).
pub fn own_keys(lib_rs: &str) -> Result<(Vec<Key>, Key), String> {
    let a = lib_rs
        .find("// BEGIN UPDATER_KEYS")
        .ok_or("lib.rs: no // BEGIN UPDATER_KEYS")?;
    let b = lib_rs
        .find("// END UPDATER_KEYS")
        .ok_or("lib.rs: no // END UPDATER_KEYS")?;
    let block = &lib_rs[a..b];
    let mut keys = Vec::new();
    let mut pos = 0;
    while let Some((role, end)) = quoted_after(block, pos, "role") {
        let (key_id, end) =
            quoted_after(block, end, "key_id").ok_or("lib.rs: a key with no key_id")?;
        let (pubkey, end) = quoted_after(block, end, "public_key_b64")
            .ok_or("lib.rs: a key with no public_key_b64")?;
        keys.push(Key {
            role,
            key_id,
            pubkey,
        });
        pos = end;
    }
    let konst = |name: &str| -> Result<String, String> {
        let p = format!("pub const {name}: &str = \"");
        let at = lib_rs
            .find(&p)
            .ok_or_else(|| format!("lib.rs: no {name}"))?
            + p.len();
        let end = at
            + lib_rs[at..]
                .find('"')
                .ok_or_else(|| format!("lib.rs: {name} is not closed"))?;
        Ok(lib_rs[at..end].to_string())
    };
    let retired = Key {
        role: "retired".into(),
        key_id: konst("LEGACY_UPDATER_KEY_ID")?,
        pubkey: konst("LEGACY_UPDATER_KEY_B64")?,
    };
    Ok((keys, retired))
}

/// The brand id and the `[update]` keys of a brand file, checked: each key
/// a minisign key line whose id is its `key_id`, roles primary or backup,
/// a primary first, no key twice.
pub fn brand_keys(text: &str) -> Result<(String, Vec<Key>), String> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| format!("not valid TOML: {e}"))?;
    let id = match doc.get("id") {
        Some(toml::Value::String(s)) if !s.is_empty() => s.clone(),
        _ => return Err("id is missing or not a string".into()),
    };
    let update = match doc.get("update") {
        Some(toml::Value::Table(t)) => t,
        Some(_) => return Err("[update] must be a table".into()),
        None => return Err("[update] is missing; a build needs its own update keys".into()),
    };
    if let Some(k) = update.keys().find(|k| k.as_str() != "keys") {
        return Err(format!(
            "[update] holds a key it does not know: {k} (it knows keys)"
        ));
    }
    let items = match update.get("keys") {
        Some(toml::Value::Array(a)) if !a.is_empty() => a,
        _ => {
            return Err(
                "update.keys must be a list of { role, key_id, pubkey } tables, at least one"
                    .into(),
            )
        }
    };
    let mut keys: Vec<Key> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let f = format!("update.keys[{i}]");
        let toml::Value::Table(t) = item else {
            return Err(format!("{f} must be a table"));
        };
        if let Some(k) = t
            .keys()
            .find(|k| !["role", "key_id", "pubkey"].contains(&k.as_str()))
        {
            return Err(format!(
                "{f} holds a key it does not know: {k} (it knows role, key_id, pubkey)"
            ));
        }
        let field = |name: &str| match t.get(name) {
            Some(toml::Value::String(s)) if !s.is_empty() => Ok(s.clone()),
            _ => Err(format!("{f}.{name} is missing or not a string")),
        };
        let key = Key {
            role: field("role")?,
            key_id: field("key_id")?,
            pubkey: field("pubkey")?,
        };
        if key.role != "primary" && key.role != "backup" {
            return Err(format!("{f}.role {:?} must be primary or backup", key.role));
        }
        match key_id_of(&key.pubkey) {
            None => return Err(format!("{f}.pubkey is not a minisign public key line")),
            Some(id) if id != key.key_id => {
                return Err(format!(
                    "{f}.key_id {:?} is not the pubkey's key id ({id})",
                    key.key_id
                ))
            }
            Some(_) => {}
        }
        if keys.iter().any(|k| k.key_id == key.key_id) {
            return Err(format!("update.keys lists {} twice", key.key_id));
        }
        keys.push(key);
    }
    if keys[0].role != "primary" {
        return Err("update.keys must start with a primary key".into());
    }
    Ok((id, keys))
}

/// What this brand's build compiles in, or why it may not be built.
pub fn choose(id: &str, keys: Vec<Key>, own: &[Key], retired: &Key) -> Result<Choice, String> {
    if OWN_KEY_BRANDS.contains(&id) {
        if keys.as_slice() == own {
            return Ok(Choice::Own);
        }
        return Err(format!(
            "brand {id} must carry exactly the ceremony's update keys (UPDATER_KEYS in crates/k2-release-keys/src/lib.rs, in order)"
        ));
    }
    for k in &keys {
        for o in own.iter().chain(std::iter::once(retired)) {
            if k.key_id == o.key_id || k.pubkey == o.pubkey {
                return Err(format!(
                    "brand {id} carries the ceremony's update key {}; only {} may ship it",
                    o.key_id,
                    OWN_KEY_BRANDS.join(" and ")
                ));
            }
        }
    }
    Ok(Choice::Brand(keys))
}

/// The Rust `include!`d for a brand's own keys.
pub fn render(id: &str, from: &str, keys: &[Key]) -> String {
    let mut s = format!(
        "// Generated by crates/k2-release-keys/build.rs from {from}. Do not edit.\n\
         /// The update keys every verifier in this build trusts: brand `{id}`'s\n\
         /// `[update]` keys.\n\
         pub const UPDATER_KEYS: &[TrustedKey] = &[\n"
    );
    for k in keys {
        s.push_str(&format!(
            "    TrustedKey {{\n        role: {:?},\n        key_id: {:?},\n        public_key_b64: {:?},\n    }},\n",
            k.role, k.key_id, k.pubkey
        ));
    }
    s.push_str("];\n");
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;

    fn repo(rel: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    fn own() -> (Vec<Key>, Key) {
        own_keys(&repo("crates/k2-release-keys/src/lib.rs")).expect("the ceremony's block")
    }

    /// brand/xy's file with its `[update]` table swapped for one key.
    fn xy_with(key: &Key) -> String {
        let xy = repo("brand/xy/brand.toml");
        let at = xy
            .find("\n[update]\n")
            .expect("brand/xy has an [update] table")
            + 1;
        let rest = &xy[at..];
        let end = rest
            .find("\n]\n")
            .expect("brand/xy's keys list ends with ]")
            + 3;
        format!(
            "{}[update]\nkeys = [\n    {{ role = \"primary\", key_id = \"{}\", pubkey = \"{}\" }},\n]\n{}",
            &xy[..at],
            key.key_id,
            key.pubkey,
            &rest[end..]
        )
    }

    /// The brand file this build compiled its keys from (build.rs names it).
    pub(crate) fn this_brand() -> String {
        let p = env!("K2_UPDATE_KEYS_BRAND_FILE");
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {p}: {e}"))
    }

    #[test]
    fn the_compiled_keys_are_the_brand_files() {
        let (id, mine) = brand_keys(&this_brand()).expect("this build's brand file");
        let compiled: Vec<Key> = crate::UPDATER_KEYS
            .iter()
            .map(|k| Key {
                role: k.role.into(),
                key_id: k.key_id.into(),
                pubkey: k.public_key_b64.into(),
            })
            .collect();
        assert_eq!(compiled, mine, "brand {id}");
        let (_, retired) = own();
        assert_eq!(retired.key_id, crate::LEGACY_UPDATER_KEY_ID);
        assert_eq!(retired.pubkey, crate::LEGACY_UPDATER_KEY_B64);
        assert!(!compiled
            .iter()
            .any(|k| k.key_id == retired.key_id || k.pubkey == retired.pubkey));
    }

    #[test]
    fn brand_k2_and_k5_carry_exactly_the_ceremony_keys() {
        let (keys, retired) = own();
        assert!(!keys.is_empty());
        for b in ["brand/k2/brand.toml", "brand/k5/brand.toml"] {
            let (id, mine) = brand_keys(&repo(b)).unwrap_or_else(|e| panic!("{b}: {e}"));
            assert_eq!(mine, keys, "{b}");
            assert_eq!(choose(&id, mine, &keys, &retired), Ok(Choice::Own), "{b}");
        }
    }

    #[test]
    fn brand_xy_builds_with_its_own_placeholder_key() {
        let (keys, retired) = own();
        let (id, mine) = brand_keys(&repo("brand/xy/brand.toml")).expect("brand/xy");
        assert_eq!(id, "xy");
        let Ok(Choice::Brand(got)) = choose(&id, mine.clone(), &keys, &retired) else {
            panic!("brand/xy must build with its own keys");
        };
        assert_eq!(got, mine);
        let text = render(&id, "brand/xy/brand.toml", &got);
        assert!(
            text.contains(&format!("key_id: {:?}", got[0].key_id)),
            "{text}"
        );
        assert!(!keys.iter().any(|k| text.contains(&k.pubkey)), "{text}");
    }

    #[test]
    fn brand_xy_given_our_key_on_purpose_is_refused() {
        let (keys, retired) = own();
        for ours in keys.iter().chain(std::iter::once(&retired)) {
            let (id, mine) = brand_keys(&xy_with(ours)).expect("still a well-formed brand file");
            let e = choose(&id, mine, &keys, &retired).expect_err("our key in brand xy");
            assert!(
                e.contains(&format!(
                    "carries the ceremony's update key {}",
                    ours.key_id
                )),
                "{e}"
            );
        }
    }

    #[test]
    fn a_k2_brand_with_another_key_is_refused() {
        let (keys, retired) = own();
        let (_, xy) = brand_keys(&repo("brand/xy/brand.toml")).expect("brand/xy");
        let e = choose("k2", xy, &keys, &retired).expect_err("k2 with xy's key");
        assert!(
            e.contains("brand k2 must carry exactly the ceremony's update keys"),
            "{e}"
        );
        let mut fewer = keys.clone();
        fewer.pop();
        assert!(
            choose("k5", fewer, &keys, &retired).is_err(),
            "k5 without the backup key"
        );
    }

    #[test]
    fn a_bad_update_table_is_refused_with_its_field() {
        let xy = repo("brand/xy/brand.toml");
        let (_, mine) = brand_keys(&xy).expect("brand/xy");
        let k = &mine[0];
        for (text, want) in [
            (
                xy.replace(&k.pubkey, "RWQnotakey"),
                "update.keys[0].pubkey is not a minisign public key line",
            ),
            (
                xy.replace(&k.key_id, "0000000000000000"),
                "is not the pubkey's key id",
            ),
            (
                xy.replace("role = \"primary\"", "role = \"spare\""),
                "must be primary or backup",
            ),
            (
                xy[..xy.find("\n[update]\n").expect("table") + 1].to_string(),
                "[update] is missing",
            ),
        ] {
            let e = brand_keys(&text).expect_err(want);
            assert!(e.contains(want), "{e}");
        }
    }
}
