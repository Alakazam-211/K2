//! Name-only changes: an image drawn once serves every address that draws
//! the same thing.
//!
//! A scene's render digest leaves object and camera ids out
//! (`k2_scenerender::digest`), so renaming an object gives its container a
//! new address (`@obj:Cube` → `@obj:Box`) and the same render hash. The
//! cache is keyed by address, so without this the renamed object would be
//! drawn again. Instead, when a container's exact image isn't cached but the
//! doc has one at the same render hash, width and look under another
//! address, that image is copied under the new address (its metadata saying
//! `reused_from`, the address it came from) and `preview_ready` is pushed at
//! once: no worker runs. An image's id image comes along with it (without
//! it the container is drawn), and its id table is named as the listing is
//! now ([`service::with_current_ids`]): the pixels hold object indexes, so
//! they stay right; only the ids in the table change. The cached table has
//! no keys; the push and the route add each object's live key from the
//! listing now ([`service::with_served_ids`]).
//!
//! The render hash names the bytes drawn (the contract: a digest covers
//! everything a drawing reads), so two addresses with one hash draw the same
//! pixels, whatever the type.

use std::fs;
use std::sync::Arc;

use k2_core::log_debug;
use k2_mpc_base::blob::BlobStore;
use k2_mpc_base::store::DocKey;

use super::cache::{self, Entry, Got};
use super::service::{self, Keyed, Snapshot};
use super::{Look, Shape, Width};
use crate::docs::host;
use crate::docs::host_events::{self, HostEvent};

/// The newest cached image of `key`'s doc at exactly (`hash`, `width`,
/// `build`) under an address other than `except`.
pub fn same_hash_entry(blobs: &BlobStore, key: &DocKey, except: &str, hash: &str, width: Width, build: &str) -> Option<Entry> {
    let own = cache::container_id(except);
    let suffix = format!("-{}-{}-{}.{}", cache::clean(hash), width.as_str(), cache::clean(build), cache::EXT);
    let mut found: Vec<(String, std::time::SystemTime)> = fs::read_dir(cache::doc_dir(key))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = name.len() == 16 + suffix.len()
                && name.ends_with(&suffix)
                && name[..16].bytes().all(|b| b.is_ascii_hexdigit())
                && !name.starts_with(&own);
            ours.then(|| (name, e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)))
        })
        .collect();
    found.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, _) in found {
        match cache::open(blobs, key, &name) {
            Got::Hit(e) => return Some(e),
            Got::KeyDestroyed => return None,
            Got::Miss => {}
        }
    }
    None
}

/// Answer `k` at `width`, `look` and `shape` from another address's image,
/// if the doc has one at its render hash and its own isn't cached: cache it
/// under `k`'s address with `reused_from`, push `preview_ready`, and return
/// the address it came from. Two addresses at one render hash draw the same
/// view (a renamed object), so both are shaped or neither.
pub fn try_reuse(blobs: &Arc<BlobStore>, snap: &Snapshot, k: &Keyed, width: Width, look: Look, shape: Shape) -> Option<String> {
    let build = snap.cache_build(&k.c.key, look, shape);
    if cache::entry_exists(&snap.key, &k.c.key, &k.hash, width, &build) {
        return None;
    }
    let entry = same_hash_entry(blobs, &snap.key, &k.c.key, &k.hash, width, &build)?;
    let from = entry.meta.get("container")?.as_str()?.to_string();
    let ids = match entry.meta.get("ids") {
        Some(_) => match cache::get(blobs, &snap.key, &from, &k.hash, width, &cache::ids_build(&build)) {
            Got::Hit(e) => Some(e),
            Got::Miss | Got::KeyDestroyed => return None,
        },
        None => None,
    };
    let now = host::now_ms();
    let follow = |mut meta: serde_json::Value| {
        meta["container"] = serde_json::json!(k.c.key);
        meta["rev"] = serde_json::json!(snap.rev);
        meta["render_ms"] = serde_json::json!(0);
        meta["rendered_at_ms"] = serde_json::json!(now);
        meta["reused_from"] = serde_json::json!(from);
        meta
    };
    let mut meta = follow(entry.meta);
    service::with_current_ids(snap, k, &mut meta);
    // Cached named as now but without keys (they are in no digest); pushed with them.
    let mut served = meta.clone();
    service::with_served_ids(snap, k, &mut served);
    let put = || -> Result<(), String> {
        if let Some(ids) = ids {
            cache::put(blobs, &snap.key, &k.c.key, &k.hash, width, &cache::ids_build(&build), &follow(ids.meta), &ids.image)?;
        }
        cache::put(blobs, &snap.key, &k.c.key, &k.hash, width, &build, &meta, &entry.image)
    };
    if let Err(e) = put() {
        log_debug!("[docs/preview] WARN: reusing a preview of {}: {e}; drawing it", host::key_label(&snap.key));
        return None;
    }
    host_events::publish(HostEvent::PreviewReady {
        key: snap.key.clone(),
        container: k.c.key.clone(),
        render_hash: k.hash.clone(),
        width: width.as_str(),
        state: "ready",
        failed: None,
        rev: snap.rev,
        rendered_at_ms: now,
        extras: service::ready_fields(&served),
    });
    Some(from)
}

/// The widths to draw `k` at when the doc's head moved: those already
/// looked at for its address; for a new address (a rename), those looked
/// at for the address in `prev` that had its render hash, so the renamed
/// container's `ready` comes at once (from [`try_reuse`]).
pub fn widths_to_redraw(prev: &Snapshot, k: &Keyed, build: &str) -> Vec<Width> {
    let own = cache::widths_cached(&prev.key, &k.c.key, build);
    if !own.is_empty() || prev.container(&k.c.key).is_some() {
        return own;
    }
    let mut ws: Vec<Width> = prev
        .containers
        .iter()
        .filter(|p| p.hash == k.hash && p.c.key != k.c.key)
        .flat_map(|p| cache::widths_cached(&prev.key, &p.c.key, build))
        .collect();
    ws.sort();
    ws.dedup();
    ws
}
