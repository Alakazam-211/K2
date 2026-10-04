//! Home avatar cache core (prd-home-picker-and-remote-avatars-v1 S3,
//! tests T3.1–T3.4).
//!
//! Drives `k2_core::home_avatars` directly on temp folders: encoding, put
//! (write, hash no-op, type swap, miss), refusals with exact codes, prune
//! with its grace, and the entry cap. k2-core tests run through this daemon
//! test binary (never `cargo test -p k2-core`). Nothing here touches the
//! real `~/.k2`.
//!
//! Fail loudly: every assertion names what it saw.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use k2_core::home_avatars::{
    self as ha, AvatarCache, AvatarError, MAX_ENTRIES, MAX_IMAGE_BYTES, PRUNE_GRACE_MS,
};

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_cache(tag: &str) -> (TempRoot, AvatarCache) {
    let dir = std::env::temp_dir().join(format!(
        "k2-home-avatars-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp cache root");
    let cache = AvatarCache::at(dir.join("agent-avatars"));
    (TempRoot(dir), cache)
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn png(seed: u8, len: usize) -> Vec<u8> {
    let mut b = PNG_MAGIC.to_vec();
    b.extend(std::iter::repeat(seed).take(len.saturating_sub(8)));
    b
}

fn jpeg(seed: u8) -> Vec<u8> {
    let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0];
    b.extend(std::iter::repeat(seed).take(60));
    b
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p)
        .unwrap_or_else(|e| panic!("stat {}: {e}", p.display()))
        .modified()
        .expect("mtime")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

// ── T3.1 encoding ───────────────────────────────────────────────────────

#[test]
fn t3_1_encoding_round_trips_one_to_one_and_refuses_bad_components() {
    let cases = [
        ("local", "local"),
        ("dtl.k2.dev", "dtl.k2.dev"),
        ("192.168.1.20:38471", "192.168.1.20_3a38471"),
        ("[fe80::1]:38471", "_5bfe80_3a_3a1_5d_3a38471"),
        ("scout_v3", "scout_5fv3"),
    ];
    let mut seen = std::collections::HashSet::new();
    for (raw, want) in cases {
        let enc = ha::encode_component(raw).unwrap_or_else(|e| panic!("encode {raw}: {e}"));
        assert_eq!(enc, want, "encoding of {raw}");
        for bad in ['/', '\\', ':'] {
            assert!(!enc.contains(bad), "{raw} encodes to {enc}, which holds {bad:?}");
        }
        assert_eq!(ha::decode_component(&enc).as_deref(), Some(raw), "{enc} must decode back to {raw}");
        assert!(seen.insert(enc.clone()), "two inputs encoded to {enc}");
    }
    // `_` encodes, so a literal `_3a` can't collide with an encoded `:`.
    assert_ne!(ha::encode_component("a_3a").expect("a_3a"), ha::encode_component("a:").expect("a:"));
    // A decode the encoder could never have produced is refused.
    assert_eq!(ha::decode_component("_61"), None, "'a' is never escaped");
    assert_eq!(ha::decode_component("A"), None, "uppercase is never raw");
    assert_eq!(ha::decode_component("_3A"), None, "hex is lowercase");

    for (raw, why) in [("", "empty"), (".", "dot"), ("..", "dot-dot")] {
        match ha::encode_component(raw) {
            Err(AvatarError::BadAddress(_)) => {}
            other => panic!("{why} component must be refused, got {other:?}"),
        }
    }
    let long = "a".repeat(201);
    assert!(matches!(ha::encode_component(&long), Err(AvatarError::BadAddress(_))), "201 bytes refused");
    assert!(ha::encode_component(&"a".repeat(200)).is_ok(), "200 bytes allowed");
    // 67 `:` encode to 201 bytes.
    assert!(matches!(ha::encode_component(&":".repeat(67)), Err(AvatarError::BadAddress(_))));

    // Addresses split on the FIRST `::`; a bracketed IPv6 host is fine.
    assert_eq!(
        ha::parse_address("Scout::[FE80::1]:38471").expect("ipv6 address"),
        ("scout".to_string(), "[fe80::1]:38471".to_string())
    );
    for bad in ["scout", "::host", "scout::", "a::b::c"] {
        assert!(ha::parse_address(bad).is_err(), "{bad} must be refused");
    }
}

// ── T3.2 put ────────────────────────────────────────────────────────────

#[test]
fn t3_2_put_writes_then_same_hash_is_a_no_op_then_type_swap_then_miss() {
    let (_t, cache) = temp_cache("put");
    let addr = "scout::dtl.k2.dev";
    let img = png(7, 300);

    let out = cache.put_at(addr, Some(&data_url("image/png", &img)), 1_000).expect("first put");
    assert!(out.changed, "first put changes: {out:?}");
    let png_path = cache.image_path(addr, "png").expect("png path");
    let meta_path = cache.meta_path(addr).expect("meta path");
    assert_eq!(png_path, cache.root().join("dtl.k2.dev").join("scout.png"));
    assert_eq!(std::fs::read(&png_path).expect("png on disk"), img, "image bytes on disk");
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).expect("meta on disk")).expect("meta json");
    assert_eq!(meta["v"], 1, "{meta}");
    assert_eq!(meta["ext"], "png", "{meta}");
    assert_eq!(meta["bytes"], 300, "{meta}");
    assert_eq!(meta["fetchedAt"], 1_000, "{meta}");
    assert_eq!(meta["missing"], false, "{meta}");
    let sha = meta["sha256"].as_str().unwrap_or_else(|| panic!("meta has no sha256: {meta}")).to_string();
    assert_eq!(sha.len(), 64, "{sha}");
    assert_eq!(out.sha256.as_deref(), Some(sha.as_str()));

    let got = cache.get(addr).expect("get").expect("entry after put");
    assert_eq!(got.data_url.as_deref(), Some(data_url("image/png", &img).as_str()), "read returns the same data URL");
    assert_eq!(got.fetched_at, 1_000);

    // Same bytes: no image write, fetchedAt moves.
    let before = mtime(&png_path);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out = cache.put_at(addr, Some(&data_url("image/png", &img)), 5_000).expect("same put");
    assert!(!out.changed, "same bytes must be changed:false: {out:?}");
    assert_eq!(mtime(&png_path), before, "the image file must not be rewritten for the same hash");
    assert_eq!(cache.get(addr).expect("get").expect("entry").fetched_at, 5_000, "fetchedAt moved");

    // A JPEG replaces it and removes scout.png.
    let j = jpeg(3);
    let out = cache.put_at(addr, Some(&data_url("image/jpeg", &j)), 6_000).expect("jpeg put");
    assert!(out.changed, "{out:?}");
    assert!(!png_path.exists(), "the old .png must go when the type changes");
    assert_eq!(names(&cache.root().join("dtl.k2.dev")), vec!["scout.jpg", "scout.json"]);
    let got = cache.get(addr).expect("get").expect("entry");
    assert_eq!(got.data_url.as_deref(), Some(data_url("image/jpeg", &j).as_str()));

    // null → missing, no image file.
    let out = cache.put_at(addr, None, 7_000).expect("null put");
    assert!(out.changed && out.missing, "{out:?}");
    assert_eq!(names(&cache.root().join("dtl.k2.dev")), vec!["scout.json"], "a miss keeps only the meta");
    let got = cache.get(addr).expect("get").expect("a miss is an entry");
    assert!(got.missing, "{got:?}");
    assert_eq!(got.data_url, None);
    assert_eq!(got.fetched_at, 7_000, "a miss counts as fetched");
    let out = cache.put_at(addr, None, 8_000).expect("null again");
    assert!(!out.changed, "a second miss is not a change: {out:?}");

    // Addresses are normalized: mixed case reads the same entry.
    assert!(cache.get("SCOUT::DTL.K2.DEV").expect("get").is_some(), "lowercased lookup");
    assert_eq!(cache.get("nobody::dtl.k2.dev").expect("get"), None, "no entry → None");
}

#[test]
fn t3_2_svg_and_other_types_round_trip_with_canonical_mime() {
    let (_t, cache) = temp_cache("types");
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#;
    cache.put_at("a::local", Some(&data_url("image/svg+xml", svg)), 1).expect("svg put");
    let got = cache.get("a::local").expect("get").expect("svg entry");
    assert_eq!(got.data_url.as_deref(), Some(data_url("image/svg+xml", svg).as_str()));

    let ico = [0u8, 0, 1, 0, 1, 0, 16, 16];
    cache.put_at("b::local", Some(&data_url("image/vnd.microsoft.icon", &ico)), 1).expect("ico put");
    let got = cache.get("b::local").expect("get").expect("ico entry");
    assert_eq!(got.data_url.as_deref(), Some(data_url("image/x-icon", &ico).as_str()), "ico reads back as image/x-icon");

    let mut webp = b"RIFF\x10\x00\x00\x00WEBPVP8 ".to_vec();
    webp.extend([0u8; 8]);
    cache.put_at("c::local", Some(&data_url("image/webp", &webp)), 1).expect("webp put");
    cache.put_at("d::local", Some(&data_url("image/gif", b"GIF89a\x01\x00\x01\x00")), 1).expect("gif put");
}

// ── T3.3 refusals ───────────────────────────────────────────────────────

#[test]
fn t3_3_refusals_carry_exact_codes_and_write_nothing() {
    let (_t, cache) = temp_cache("refuse");
    let addr = "scout::dtl.k2.dev";
    let cases: Vec<(String, &str)> = vec![
        ("https://x/y.png".to_string(), "not_data_url"),
        ("data:image/png,rawbytes".to_string(), "not_data_url"),
        (data_url("image/png", &jpeg(1)), "type_mismatch"),
        (data_url("image/png", &png(1, 129 * 1024)), "too_large"),
        (
            data_url(
                "image/svg+xml",
                br#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY x "y">]><svg>&x;</svg>"#,
            ),
            "svg_refused",
        ),
        (data_url("image/svg+xml", b"<html>no svg here</html>"), "svg_refused"),
        (data_url("image/svg+xml", &[0xff, 0xfe, 0x00]), "svg_refused"),
        (data_url("text/html", b"<svg/>"), "type_refused"),
        (data_url("image/tiff", b"II*\0"), "type_refused"),
    ];
    for (url, code) in cases {
        let shown: String = url.chars().take(60).collect();
        match cache.put_at(addr, Some(&url), 1) {
            Err(e) => assert_eq!(e.code(), code, "{shown}: got {e}"),
            Ok(o) => panic!("{shown} must be refused with {code}, got {o:?}"),
        }
    }
    // Exactly at the cap is accepted.
    cache
        .put_at(addr, Some(&data_url("image/png", &png(2, MAX_IMAGE_BYTES))), 1)
        .expect("a 128 KiB image fits");
    assert!(matches!(cache.put_at("bad", None, 1), Err(AvatarError::BadAddress(_))));
    // `../x` encodes its `/`, so it stays a file inside the server folder.
    cache.put_at("../x::local", None, 1).expect("a `/` in a handle is encoded, not followed");
    assert!(cache.root().join("local").join(".._2fx.json").is_file(), "{:?}", walk(cache.root()));
    // `..` as a whole component is refused.
    assert!(matches!(cache.put_at("..::local", None, 1), Err(AvatarError::BadAddress(_))));
    assert!(matches!(cache.put_at("a::..", None, 1), Err(AvatarError::BadAddress(_))));
    let everything: Vec<PathBuf> = walk(cache.root());
    assert!(
        everything.iter().all(|p| p.starts_with(cache.root())),
        "every file stays under the cache root: {everything:?}"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

// ── T3.4 prune and the cap ──────────────────────────────────────────────

#[test]
fn t3_4_prune_keeps_listed_and_recent_and_removes_empty_servers() {
    let (_t, cache) = temp_cache("prune");
    let now = 10_000_000u64;
    let old = now - PRUNE_GRACE_MS - 1;
    cache.put_at("keep::dtl.k2.dev", Some(&data_url("image/png", &png(1, 64))), old).expect("keep");
    cache.put_at("gone::dtl.k2.dev", Some(&data_url("image/png", &png(2, 64))), old).expect("gone");
    cache.put_at("fresh::dtl.k2.dev", Some(&data_url("image/png", &png(3, 64))), now - 10_000).expect("fresh");
    cache.put_at("solo::192.168.1.20:38471", None, old).expect("solo miss");

    let out = cache
        .prune_at(&["KEEP::dtl.k2.dev".to_string(), "not an address".to_string()], now)
        .expect("prune");
    assert_eq!(out.removed, 2, "gone + solo are removed: {out:?}");
    assert_eq!(out.kept, 2, "keep + fresh (10 s old) stay: {out:?}");
    assert_eq!(out.removed_servers, 1, "the LAN server folder is left empty and removed: {out:?}");
    assert!(cache.get("keep::dtl.k2.dev").expect("get").is_some(), "listed entry kept");
    assert!(cache.get("fresh::dtl.k2.dev").expect("get").is_some(), "an unlisted entry written 10 s ago is kept");
    assert_eq!(cache.get("gone::dtl.k2.dev").expect("get"), None, "unlisted old entry removed");
    assert_eq!(
        names(&cache.root().join("dtl.k2.dev")),
        vec!["fresh.json", "fresh.png", "keep.json", "keep.png"],
        "gone.png and gone.json are deleted"
    );
    assert!(!cache.root().join("192.168.1.20_3a38471").exists(), "empty server folder removed");

    // Later, nothing listed and everything old → the cache empties.
    let out = cache.prune_at(&[], now + 10 * PRUNE_GRACE_MS).expect("prune all");
    assert_eq!((out.removed, out.kept, out.removed_servers), (2, 0, 1), "{out:?}");
    assert_eq!(names(cache.root()), Vec::<String>::new(), "no server folders left");
}

#[test]
fn t3_4_entry_cap_evicts_the_oldest() {
    let (_t, cache) = temp_cache("cap");
    let servers = ["a.k2.dev", "b.k2.dev", "c.k2.dev", "d.k2.dev"];
    let total = MAX_ENTRIES + 1;
    let tiny = data_url("image/png", &png(9, 16));
    for i in 0..total {
        let addr = format!("w{i}::{}", servers[i % servers.len()]);
        // Every tenth entry carries an image; the rest are misses (still entries).
        let url = (i % 10 == 0).then_some(tiny.as_str());
        let out = cache.put_at(&addr, url, 1_000 + i as u64).unwrap_or_else(|e| panic!("put {addr}: {e}"));
        let want_evicted = usize::from(i == MAX_ENTRIES);
        assert_eq!(out.evicted, want_evicted, "put #{i} evicted");
    }
    assert_eq!(cache.entry_count(), MAX_ENTRIES, "{total} puts leave {MAX_ENTRIES} entries");
    assert_eq!(cache.get("w0::a.k2.dev").expect("get"), None, "the oldest entry is gone");
    assert!(!cache.root().join("a.k2.dev").join("w0.png").exists(), "its image is gone too");
    assert!(cache.get("w1::b.k2.dev").expect("get").is_some(), "the second-oldest stays");
    let last = format!("w{}::{}", MAX_ENTRIES, servers[MAX_ENTRIES % servers.len()]);
    assert!(cache.get(&last).expect("get").is_some(), "the newest is there");
    // Re-putting an existing address never evicts.
    let out = cache.put_at("w1::b.k2.dev", None, 99_999).expect("re-put");
    assert_eq!(out.evicted, 0, "{out:?}");
    assert_eq!(cache.entry_count(), MAX_ENTRIES);
}
