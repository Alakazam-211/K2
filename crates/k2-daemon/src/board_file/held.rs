//! Held edits (prd-board-tab-v1 BT-24): a person's group that conflicted, kept
//! until they choose. It survives a restart, so an edit a save couldn't apply is
//! never lost by closing K2.
//!
//! `~/.k2/board-held/<sha256(person)>/<lineage>/<heldId>.json`, 0600 in a 0700
//! folder. `heldId` is a hash of the client's group id, and the folder a hash of
//! a login name, so neither can walk a path. **0600 isn't a wall against agents:**
//! they run as the same user.
//!
//! A row shows only on the file whose path it names. Lineage isn't unique per
//! file (a copy keeps it), so it's a folder and never the match. Agents' conflicts
//! aren't kept: an agent has its command. Rows don't follow a box move or a clone.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256 as ShaHasher};

/// The most groups one person may have held.
pub const MAX_GROUPS: usize = 256;
/// The most bytes one person may have held.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// A held edit nobody resolved goes after this.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// How often a board route looks for what's past its age.
const SWEEP_EVERY_SECS: i64 = 24 * 3600;

/// Why a row wasn't kept or changed.
#[derive(Debug)]
pub enum HeldError {
    /// Over a cap: refused, typed, and nothing is dropped.
    Full,
    /// No such row.
    NotFound,
    /// The id isn't one we mint.
    BadId,
    /// The disk.
    Io(std::io::Error),
}

impl From<std::io::Error> for HeldError {
    fn from(e: std::io::Error) -> HeldError {
        HeldError::Io(e)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The id of the group `group_id`: a retry of one group is one row.
pub fn held_id(group_id: &str) -> String {
    hex(&ShaHasher::digest(group_id.as_bytes())[..16])
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn root() -> PathBuf {
    k2_core::paths::home_dir().unwrap_or_else(std::env::temp_dir).join(".k2").join("board-held")
}

fn person_dir(pid: &str) -> PathBuf {
    root().join(hex(&ShaHasher::digest(pid.as_bytes())))
}

fn rows_of(pid: &str) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(lineages) = fs::read_dir(person_dir(pid)) else { return out };
    for l in lineages.flatten() {
        let Ok(files) = fs::read_dir(l.path()) else { continue };
        for f in files.flatten() {
            if f.path().extension().and_then(|x| x.to_str()) == Some("json") {
                out.push((f.path(), f.metadata().map_or(0, |m| m.len())));
            }
        }
    }
    out
}

fn find(pid: &str, id: &str) -> Option<PathBuf> {
    let name = format!("{id}.json");
    rows_of(pid).into_iter().map(|(p, _)| p).find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name.as_str()))
}

fn read_row(p: &Path) -> Option<Map<String, Value>> {
    serde_json::from_slice::<Value>(&fs::read(p).ok()?).ok()?.as_object().cloned()
}

fn fits(pid: &str, replacing: Option<&Path>, more: u64) -> bool {
    let rows = rows_of(pid);
    let others: Vec<&(PathBuf, u64)> = rows.iter().filter(|(p, _)| Some(p.as_path()) != replacing).collect();
    others.len() < MAX_GROUPS && others.iter().map(|(_, n)| *n).sum::<u64>() + more <= MAX_BYTES
}

/// Keeps a person's held group. A retry of the same group is the same row.
pub fn store(pid: &str, lineage: &str, path: &Path, guard: &str, group: &Value, objects: &Value) -> Result<String, HeldError> {
    let group_id = group.get("id").and_then(Value::as_str).unwrap_or("");
    let id = held_id(group_id);
    let target = person_dir(pid).join(lineage).join(format!("{id}.json"));
    let row = json!({
        "heldId": id, "groupId": group_id, "canonicalPath": path.to_string_lossy(), "guard": guard,
        "byAll": group.get("byAll").and_then(Value::as_bool).unwrap_or(false),
        "ops": group.get("ops").cloned().unwrap_or_else(|| json!([])), "objects": objects,
    });
    let bytes = serde_json::to_vec(&row).map_err(|e| HeldError::Io(std::io::Error::other(e)))?;
    if !fits(pid, Some(&target), bytes.len() as u64) {
        return Err(HeldError::Full);
    }
    k2_core::fs_atomic::create_private_dir_all(target.parent().unwrap_or(&target))?;
    k2_core::fs_atomic::atomic_write_private(&target, &bytes)?;
    Ok(id)
}

/// This person's rows on this file, oldest first.
pub fn for_file(pid: &str, path: &Path) -> Vec<Map<String, Value>> {
    let want = path.to_string_lossy();
    let mut rows: Vec<(SystemTime, Map<String, Value>)> = rows_of(pid)
        .into_iter()
        .filter_map(|(p, _)| {
            let m = read_row(&p)?;
            if m.get("canonicalPath").and_then(Value::as_str) != Some(want.as_ref()) {
                return None;
            }
            Some((fs::metadata(&p).and_then(|x| x.modified()).unwrap_or(SystemTime::UNIX_EPOCH), m))
        })
        .collect();
    rows.sort_by_key(|(t, _)| *t);
    rows.into_iter().map(|(_, m)| m).collect()
}

/// Clears one. An unknown id is fine, so a retry is safe.
pub fn resolve(pid: &str, id: &str) -> Result<(), HeldError> {
    if !valid_id(id) {
        return Err(HeldError::BadId);
    }
    if let Some(p) = find(pid, id) {
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Keeps the person's whole object beside a row, for **Put it back** on a
/// `deleted` one after a restart: neither the ops nor the file have it.
pub fn set_mine(pid: &str, id: &str, mine: &Value) -> Result<(), HeldError> {
    if !valid_id(id) {
        return Err(HeldError::BadId);
    }
    let p = find(pid, id).ok_or(HeldError::NotFound)?;
    let mut row = read_row(&p).ok_or(HeldError::NotFound)?;
    row.insert("mine".into(), mine.clone());
    let bytes = serde_json::to_vec(&Value::Object(row)).map_err(|e| HeldError::Io(std::io::Error::other(e)))?;
    if !fits(pid, Some(&p), bytes.len() as u64) {
        return Err(HeldError::Full);
    }
    k2_core::fs_atomic::atomic_write_private(&p, &bytes)?;
    Ok(())
}

/// A K2 rename or move of the file itself: its rows go with it, for every person.
pub fn retarget(from: &Path, to: &Path) {
    let (from, to) = (from.to_string_lossy(), to.to_string_lossy());
    let Ok(people) = fs::read_dir(root()) else { return };
    for person in people.flatten() {
        let Ok(lineages) = fs::read_dir(person.path()) else { continue };
        for l in lineages.flatten() {
            let Ok(files) = fs::read_dir(l.path()) else { continue };
            for f in files.flatten() {
                let Some(mut row) = read_row(&f.path()) else { continue };
                if row.get("canonicalPath").and_then(Value::as_str) == Some(from.as_ref()) {
                    row.insert("canonicalPath".into(), json!(to));
                    if let Ok(bytes) = serde_json::to_vec(&Value::Object(row)) {
                        let _ = k2_core::fs_atomic::atomic_write_private(&f.path(), &bytes);
                    }
                }
            }
        }
    }
}

/// Removes what's past its age.
pub fn sweep() {
    let Ok(people) = fs::read_dir(root()) else { return };
    for person in people.flatten() {
        let Ok(lineages) = fs::read_dir(person.path()) else { continue };
        for l in lineages.flatten() {
            let Ok(files) = fs::read_dir(l.path()) else { continue };
            for f in files.flatten() {
                let old = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|a| a > MAX_AGE);
                if old {
                    let _ = fs::remove_file(f.path());
                }
            }
        }
    }
}

static LAST_SWEEP: AtomicI64 = AtomicI64::new(0);

/// Daemon boot, and at most daily from a board route after that.
pub fn sweep_if_due() {
    let now = chrono::Utc::now().timestamp();
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    if now - last >= SWEEP_EVERY_SECS && LAST_SWEEP.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        sweep();
        super::stage::sweep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: &str = "00112233445566778899aabbccddeeff";

    fn group(id: &str) -> Value {
        json!({ "id": id, "byAll": false, "ops": [{ "op": "set", "id": "s-1", "rev": "sha256-old", "f": { "text": "mine" } }] })
    }

    #[test]
    fn a_held_edit_is_kept_for_its_person_and_its_file_only() {
        crate::test_support::with_temp_home(|| {
            let p = Path::new("/w/a.k5board");
            let id = store("person:ann", L, p, "sha256-g", &group("g1"), &json!([{ "id": "s-1", "why": "changed" }])).expect("store");
            assert_eq!(id.len(), 32);
            let rows = for_file("person:ann", p);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["heldId"], json!(id));
            assert_eq!(rows[0]["ops"][0]["f"]["text"], "mine");
            assert!(for_file("person:bob", p).is_empty(), "another person doesn't see it");
            assert!(for_file("person:ann", Path::new("/w/copy.k5board")).is_empty(), "a copy keeps the lineage, and not the edit");
        });
    }

    #[test]
    fn a_retry_is_one_row_and_resolving_twice_is_safe() {
        crate::test_support::with_temp_home(|| {
            let p = Path::new("/w/a.k5board");
            let a = store("person:ann", L, p, "g", &group("g1"), &json!([])).expect("store");
            let b = store("person:ann", L, p, "g", &group("g1"), &json!([])).expect("store");
            assert_eq!(a, b);
            assert_eq!(for_file("person:ann", p).len(), 1);
            resolve("person:ann", &a).expect("resolve");
            resolve("person:ann", &a).expect("again");
            assert!(for_file("person:ann", p).is_empty());
        });
    }

    #[test]
    fn a_held_id_cant_walk_a_path() {
        crate::test_support::with_temp_home(|| {
            assert!(matches!(resolve("person:ann", "../../../etc/passwd"), Err(HeldError::BadId)));
            assert!(matches!(set_mine("person:ann", "..", &json!({})), Err(HeldError::BadId)));
            let id = held_id("../../evil");
            assert!(valid_id(&id));
        });
    }

    #[test]
    fn the_caps_refuse_and_drop_nothing() {
        crate::test_support::with_temp_home(|| {
            let p = Path::new("/w/a.k5board");
            for i in 0..MAX_GROUPS {
                store("person:ann", L, p, "g", &group(&format!("g{i}")), &json!([])).expect("under the cap");
            }
            assert!(matches!(store("person:ann", L, p, "g", &group("one more"), &json!([])), Err(HeldError::Full)));
            assert_eq!(for_file("person:ann", p).len(), MAX_GROUPS);
            store("person:ann", L, p, "g", &group("g0"), &json!(["updated"])).expect("the same row can still change");
        });
    }

    #[test]
    fn the_size_cap_refuses_too() {
        crate::test_support::with_temp_home(|| {
            let p = Path::new("/w/a.k5board");
            let big = json!({ "id": "g-big", "ops": ["x".repeat(MAX_BYTES as usize + 1)] });
            assert!(matches!(store("person:ann", L, p, "g", &big, &json!([])), Err(HeldError::Full)));
        });
    }

    #[test]
    fn mine_is_kept_beside_the_row() {
        crate::test_support::with_temp_home(|| {
            let p = Path::new("/w/a.k5board");
            let id = store("person:ann", L, p, "g", &group("g1"), &json!([])).expect("store");
            set_mine("person:ann", &id, &json!({ "s-1": { "text": "whole" } })).expect("mine");
            assert_eq!(for_file("person:ann", p)[0]["mine"]["s-1"]["text"], "whole");
            assert!(matches!(set_mine("person:ann", &held_id("other"), &json!({})), Err(HeldError::NotFound)));
        });
    }

    #[test]
    fn a_rename_takes_the_rows_along_and_a_month_sweeps_them() {
        crate::test_support::with_temp_home(|| {
            let (a, b) = (Path::new("/w/a.k5board"), Path::new("/w/b.k5board"));
            let id = store("person:ann", L, a, "g", &group("g1"), &json!([])).expect("store");
            retarget(a, b);
            assert!(for_file("person:ann", a).is_empty());
            assert_eq!(for_file("person:ann", b).len(), 1);
            sweep();
            assert_eq!(for_file("person:ann", b).len(), 1, "too young to sweep");
            let f = fs::File::options().write(true).open(find("person:ann", &id).expect("row")).expect("open");
            f.set_modified(SystemTime::now() - Duration::from_secs(31 * 24 * 3600)).expect("age");
            sweep();
            assert!(for_file("person:ann", b).is_empty());
        });
    }

    #[test]
    fn a_row_is_private_on_disk() {
        crate::test_support::with_temp_home(|| {
            let id = store("person:ann", L, Path::new("/w/a.k5board"), "g", &group("g1"), &json!([])).expect("store");
            let p = find("person:ann", &id).expect("row");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o600);
                assert_eq!(fs::metadata(p.parent().expect("dir")).expect("meta").permissions().mode() & 0o777, 0o700);
            }
            assert!(!p.to_string_lossy().contains("person:ann"), "a login name isn't a path");
        });
    }
}
