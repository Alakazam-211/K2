//! `k2.download` consent (prd-garden-board-needs-v1; slice-1 security
//! review #1): the widgets a person let save files to Downloads from a
//! Garden without K2's card asking again.
//!
//! - **Who decides.** The first time a widget in a Garden saves a file, K2
//!   (the renderer, outside the sealed frame) shows its own card: Don't
//!   save / Save once / Always let <widget> save files from <Garden>. Only
//!   "Always" lands here; "Save once" stays in that client's memory. The
//!   routes take this computer's owner token only, for reading, adding and
//!   revoking: a widget can't reach them and an agent can't read or change
//!   them.
//! - **One truth.** Kept by the daemon, so every client (each window, the
//!   web client) honours the same answer, and clearing a browser's storage
//!   never skips or loses it.
//! - **Key.** (Garden id, widget folder name). A new version of the widget
//!   keeps the answer, as a site does for `k2.open`; revoking it brings the
//!   card back.
//! - **Budget.** Each (Garden, widget) saves at most [`HOURLY_FILES`] files
//!   and [`HOURLY_BYTES`] bytes in any hour ([`spend`], at the moment the
//!   app authorises a save), whichever client asks; kept in the same file,
//!   so a second window, the web client or a restart doesn't reset it.
//! - **Where.** `~/.k2/zen/downloads.json`, daemon-written (atomic), per
//!   computer: never synced, never in a Garden's TOML.
//!
//! Honest bar (as `links`): a same-user process can edit the file; this is a
//! guardrail against a widget writing to Downloads unasked, not a wall.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::store::{valid_garden_id, ZenError};

/// The file, in the Zen folder.
pub const DOWNLOADS_FILE: &str = "downloads.json";
pub const DOWNLOADS_VERSION: u32 = 1;
/// Widgets remembered per Garden.
pub const MAX_WIDGETS_PER_GARDEN: usize = 200;
/// Files one widget saves per hour, from any client.
pub const HOURLY_FILES: usize = 30;
/// Bytes one widget saves per hour, from any client.
pub const HOURLY_BYTES: u64 = 200 * 1024 * 1024;
/// The largest single save (`downloads.save`'s cap).
pub const MAX_SAVE_BYTES: u64 = 50 * 1024 * 1024;
const HOUR_SECS: i64 = 3_600;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadsFile {
    #[serde(default)]
    pub version: u32,
    /// Garden id → the widgets that save files without asking, sorted.
    #[serde(default)]
    pub gardens: BTreeMap<String, Vec<String>>,
    /// `<garden>/<widget>` → the saves of the last hour: `(seconds, bytes)`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spent: BTreeMap<String, Vec<(i64, u64)>>,
}

fn check(garden: &str, widget: &str) -> Result<(), ZenError> {
    if !valid_garden_id(garden) {
        return Err(ZenError::BadRequest(format!("'{garden}' isn't a Garden id")));
    }
    if !super::valid_theme_name(widget) {
        return Err(ZenError::BadRequest(format!("'{widget}' isn't a widget name")));
    }
    Ok(())
}

/// The file as it is (empty when missing; a broken file reads as empty, so
/// the person is simply asked again).
pub fn read(zen_dir: &Path) -> DownloadsFile {
    std::fs::read_to_string(zen_dir.join(DOWNLOADS_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write(zen_dir: &Path, f: &DownloadsFile) -> Result<(), ZenError> {
    let path = zen_dir.join(DOWNLOADS_FILE);
    let mut f = f.clone();
    f.version = DOWNLOADS_VERSION;
    f.gardens.retain(|_, w| !w.is_empty());
    f.spent.retain(|_, s| !s.is_empty());
    let body = serde_json::to_string_pretty(&f).map_err(|e| ZenError::Io(e.to_string()))?;
    std::fs::create_dir_all(zen_dir).map_err(|e| ZenError::Io(format!("{}: {e}", zen_dir.display())))?;
    crate::fs_atomic::atomic_write_str(&path, &(body + "\n")).map_err(|e| ZenError::Io(format!("{}: {e}", path.display())))
}

/// The widgets `garden` lets save files without asking.
pub fn widgets(zen_dir: &Path, garden: &str) -> Result<Vec<String>, ZenError> {
    if !valid_garden_id(garden) {
        return Err(ZenError::BadRequest(format!("'{garden}' isn't a Garden id")));
    }
    Ok(read(zen_dir).gardens.get(garden).cloned().unwrap_or_default())
}

/// "Always let <widget> save files from this Garden". False when already so.
pub fn allow(zen_dir: &Path, garden: &str, widget: &str) -> Result<bool, ZenError> {
    check(garden, widget)?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = read(zen_dir);
    let list = f.gardens.entry(garden.to_string()).or_default();
    if list.iter().any(|w| w == widget) {
        return Ok(false);
    }
    if list.len() >= MAX_WIDGETS_PER_GARDEN {
        return Err(ZenError::BadRequest(format!("a Garden remembers at most {MAX_WIDGETS_PER_GARDEN} widgets")));
    }
    list.push(widget.to_string());
    list.sort();
    write(zen_dir, &f)?;
    Ok(true)
}

/// Revoke: K2's card asks again. False when it wasn't there.
pub fn forget(zen_dir: &Path, garden: &str, widget: &str) -> Result<bool, ZenError> {
    check(garden, widget)?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = read(zen_dir);
    let Some(list) = f.gardens.get_mut(garden) else { return Ok(false) };
    let before = list.len();
    list.retain(|w| w != widget);
    if list.len() == before {
        return Ok(false);
    }
    write(zen_dir, &f)?;
    Ok(true)
}

/// Spend one save of `bytes` for (`garden`, `widget`) at `now` (seconds):
/// `Ok(None)` when it fits the hour's budget (and is counted), `Ok(Some(why))`
/// when it doesn't (nothing counted).
pub fn spend(zen_dir: &Path, garden: &str, widget: &str, bytes: u64, now: i64) -> Result<Option<String>, ZenError> {
    check(garden, widget)?;
    if bytes > MAX_SAVE_BYTES {
        return Err(ZenError::BadRequest(format!("a save is at most {} MiB", MAX_SAVE_BYTES / (1024 * 1024))));
    }
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = read(zen_dir);
    // Only the last hour is kept, for every widget.
    for list in f.spent.values_mut() {
        list.retain(|(at, _)| now - *at < HOUR_SECS && *at <= now);
    }
    let key = format!("{garden}/{widget}");
    let list = f.spent.entry(key).or_default();
    let used: u64 = list.iter().map(|(_, b)| *b).sum();
    if list.len() >= HOURLY_FILES {
        return Ok(Some(format!("at most {HOURLY_FILES} downloads an hour from one widget")));
    }
    if used + bytes > HOURLY_BYTES {
        return Ok(Some(format!("at most {} MiB of downloads an hour from one widget", HOURLY_BYTES / (1024 * 1024))));
    }
    list.push((now, bytes));
    write(zen_dir, &f)?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_forget_per_garden_and_widget() {
        let d = crate::test_env::unique_temp_path("zen-downloads");
        std::fs::create_dir_all(&d).expect("mkdir");
        assert!(widgets(&d, "g-1").expect("list").is_empty());
        assert!(allow(&d, "g-1", "board").expect("allow"));
        assert!(!allow(&d, "g-1", "board").expect("again"));
        assert_eq!(widgets(&d, "g-1").expect("list"), vec!["board"]);
        assert!(widgets(&d, "g-2").expect("list").is_empty(), "per Garden");
        assert!(forget(&d, "g-1", "board").expect("forget"));
        assert!(!forget(&d, "g-1", "board").expect("forget again"));
        assert!(widgets(&d, "g-1").expect("list").is_empty());
        for (g, w) in [("../x", "board"), ("g-1", "../board"), ("g-1", "Board")] {
            assert!(matches!(allow(&d, g, w), Err(ZenError::BadRequest(_))), "{g} {w}");
        }
        // The hourly budget: files, bytes, per widget, and the hour rolls on.
        let t = 1_760_000_000;
        for i in 0..HOURLY_FILES {
            assert_eq!(spend(&d, "g-1", "board", 1, t + i as i64).expect("spend"), None, "save {i}");
        }
        assert!(spend(&d, "g-1", "board", 1, t + 100).expect("spend").is_some(), "the 31st in the hour");
        assert_eq!(spend(&d, "g-1", "notes", 1, t + 100).expect("spend"), None, "per widget");
        assert_eq!(spend(&d, "g-1", "board", 1, t + HOUR_SECS + 30).expect("spend"), None, "an hour on");
        assert!(spend(&d, "g-2", "big", MAX_SAVE_BYTES, t).expect("spend").is_none());
        assert!(spend(&d, "g-2", "big", MAX_SAVE_BYTES, t).expect("spend").is_none());
        assert!(spend(&d, "g-2", "big", MAX_SAVE_BYTES, t).expect("spend").is_none());
        assert!(spend(&d, "g-2", "big", MAX_SAVE_BYTES, t).expect("spend").is_none());
        assert!(spend(&d, "g-2", "big", 1, t).expect("spend").is_some(), "200 MiB in the hour");
        assert!(matches!(spend(&d, "g-2", "big", MAX_SAVE_BYTES + 1, t), Err(ZenError::BadRequest(_))));
        // It's in the file: a restarted daemon reads the same budget.
        assert!(read(&d).spent.contains_key("g-2/big"));
        std::fs::write(d.join(DOWNLOADS_FILE), "{not json").expect("write");
        assert!(widgets(&d, "g-1").expect("list").is_empty(), "a broken file reads as empty (asks again)");
        let _ = std::fs::remove_dir_all(&d);
    }
}
