//! The `model` key of a Claude Code user settings file, and putting it back
//! after K2 switches one session's model (0.46.13, Rosson).
//!
//! Claude Code's `/model <id>` switches the running session AND saves the
//! pick as the default for every new session: it writes `"model"` into the
//! user settings file (`$CLAUDE_CONFIG_DIR/settings.json`, else
//! `~/.claude/settings.json`; Claude Code 2.1.153+). When K2 switches an
//! agent's model from the Thread (`/cli/sessions/model`) it types that same
//! `/model <id>` into the session (no restart, its subagents keep running),
//! then waits for Claude's write and puts the key back with
//! [`wait_and_restore`]:
//!
//! - only the top-level `model` key changes: every other byte of the file
//!   is kept ([`with_model`] edits the text, then checks that the parsed
//!   file differs in `model` alone);
//! - the write is atomic (a temp file in the same folder, then a rename);
//!   a `settings.json` that is a symlink (a wallet login's home links the
//!   shared one) is written through, so the link stays;
//! - it is K2's change that is undone, nobody else's: the file is restored
//!   only after its `model` became exactly the id K2 sent, and it is read
//!   again right before the write. A file that changed to anything else
//!   (a person's own `/model` in their own terminal) is left alone.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The user settings file inside a Claude home.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join("settings.json")
}

/// What the file held at one moment: its text (`None` = no file) and its
/// top-level `model` (`None` = absent, not a string, or not JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub raw: Option<String>,
    pub model: Option<String>,
}

/// Read the file now.
pub fn snapshot(path: &Path) -> Snapshot {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let model = model_of(&raw);
            Snapshot { raw: Some(raw), model }
        }
        Err(_) => Snapshot { raw: None, model: None },
    }
}

/// The top-level `model` string of a settings file's text.
pub fn model_of(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get("model")?.as_str().map(str::to_string)
}

// ── A text edit of one top-level key ─────────────────────────────────

struct Member {
    key: String,
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

struct TopObject {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// `i` at an opening quote; the index just past the closing one.
fn skip_string(b: &[u8], mut i: usize) -> Result<usize, String> {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("unterminated string".into())
}

/// `i` at the first byte of a value; the index just past it.
fn skip_value(b: &[u8], mut i: usize) -> Result<usize, String> {
    match b.get(i) {
        Some(b'"') => skip_string(b, i),
        Some(b'{') | Some(b'[') => {
            let mut depth = 0usize;
            while i < b.len() {
                match b[i] {
                    b'"' => {
                        i = skip_string(b, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Err("unterminated object or array".into())
        }
        Some(_) => {
            while i < b.len() && !matches!(b[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                i += 1;
            }
            Ok(i)
        }
        None => Err("missing value".into()),
    }
}

/// Where the top-level object and each of its members sit in `raw`.
/// `raw` must already parse as a JSON object.
fn top_object(raw: &str) -> Result<TopObject, String> {
    let b = raw.as_bytes();
    let open = skip_ws(b, 0);
    if b.get(open) != Some(&b'{') {
        return Err("settings file is not a JSON object".into());
    }
    let mut members = Vec::new();
    let mut i = skip_ws(b, open + 1);
    if b.get(i) == Some(&b'}') {
        return Ok(TopObject { open, close: i, members });
    }
    loop {
        if b.get(i) != Some(&b'"') {
            return Err("expected a key".into());
        }
        let key_start = i;
        let key_end = skip_string(b, i)?;
        let key: String = serde_json::from_str(&raw[key_start..key_end]).map_err(|e| e.to_string())?;
        i = skip_ws(b, key_end);
        if b.get(i) != Some(&b':') {
            return Err("expected ':'".into());
        }
        let value_start = skip_ws(b, i + 1);
        let value_end = skip_value(b, value_start)?;
        members.push(Member { key, key_start, value_start, value_end });
        i = skip_ws(b, value_end);
        match b.get(i) {
            Some(b',') => i = skip_ws(b, i + 1),
            Some(b'}') => return Ok(TopObject { open, close: i, members }),
            _ => return Err("expected ',' or '}'".into()),
        }
    }
}

fn without_model(v: &serde_json::Value) -> serde_json::Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("model");
    }
    v
}

/// `raw` with its top-level `model` set to `model` (`None` removes the
/// key), every other byte kept. An empty file reads as `{}`. Refuses a
/// file that isn't a JSON object, and checks its own result: the parsed
/// file must differ in `model` alone.
pub fn with_model(raw: &str, model: Option<&str>) -> Result<String, String> {
    let src = if raw.trim().is_empty() { "{}" } else { raw };
    let before: serde_json::Value =
        serde_json::from_str(src).map_err(|e| format!("settings file is not JSON: {e}"))?;
    if !before.is_object() {
        return Err("settings file is not a JSON object".into());
    }
    let obj = top_object(src)?;
    let hits: Vec<usize> = obj.members.iter().enumerate().filter(|(_, m)| m.key == "model").map(|(i, _)| i).collect();
    let mut out = src.to_string();
    match model {
        Some(id) => {
            let lit = serde_json::to_string(id).map_err(|e| e.to_string())?;
            if hits.is_empty() {
                if let Some(first) = obj.members.first() {
                    // Before the first member, in its indentation.
                    let lead = &src[obj.open + 1..first.key_start];
                    out = format!("{}{lead}\"model\": {lit},{}", &src[..obj.open + 1], &src[obj.open + 1..]);
                } else {
                    out = format!("{}{{\n  \"model\": {lit}\n}}{}", &src[..obj.open], &src[obj.close + 1..]);
                }
            } else {
                // Right to left, so earlier offsets stay good.
                for &h in hits.iter().rev() {
                    let m = &obj.members[h];
                    out.replace_range(m.value_start..m.value_end, &lit);
                }
            }
        }
        None => {
            for &h in hits.iter().rev() {
                let n = obj.members.len();
                let (from, to) = if h + 1 < n {
                    (obj.members[h].key_start, obj.members[h + 1].key_start)
                } else if h > 0 {
                    (obj.members[h - 1].value_end, obj.members[h].value_end)
                } else {
                    (obj.open + 1, obj.close)
                };
                out.replace_range(from..to, "");
            }
            // Two removals that touched would have overlapped: only one
            // `model` key is expected; the check below catches the rest.
        }
    }
    let after: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| format!("edit produced invalid JSON: {e}"))?;
    if without_model(&after) != without_model(&before) {
        return Err("edit changed more than the model key".into());
    }
    if after.get("model").and_then(|v| v.as_str()) != model {
        return Err("edit did not set the model key".into());
    }
    Ok(out)
}

/// Replace `path` with `text` atomically (temp file in the same folder,
/// then rename), keeping its permission bits. A symlink is written
/// through: its target is replaced, the link stays.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::canonicalize(path).map_err(|e| format!("resolve {}: {e}", path.display()))?
        }
        _ => path.to_path_buf(),
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&target).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644)
    };
    #[cfg(not(unix))]
    let mode = 0o644;
    crate::fs_atomic::atomic_write_mode(&target, text.as_bytes(), mode)
        .map_err(|e| format!("write {}: {e}", target.display()))
}

/// What [`wait_and_restore`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// The CLI wrote K2's pick; the key is back to `to` (`None` = removed,
    /// it wasn't there before and the server has no default).
    Restored { to: Option<String> },
    /// The CLI wrote K2's pick and that is also the value to keep (the
    /// server's default is the pick): nothing to write.
    Kept { model: String },
    /// The file already said the pick before K2's switch, so the CLI's
    /// write changed nothing there: left alone.
    AlreadyPick,
    /// The file changed, but not to K2's pick (someone else's change), or
    /// changed again before the restore: left alone.
    ChangedElsewhere { model: Option<String> },
    /// The CLI didn't write the pick within the wait: nothing written.
    NotWritten,
}

/// After K2 typed `/model <pick>` into a session: wait (up to `wait`,
/// checking every `poll`) for the CLI to write `pick` into `path`, then
/// put the key back to `default` (the server's default for the harness),
/// else to what `before` held (removed if it was absent). See the module
/// doc for what is never touched.
pub fn wait_and_restore(
    path: &Path,
    before: &Snapshot,
    pick: &str,
    default: Option<&str>,
    wait: Duration,
    poll: Duration,
) -> Result<Restore, String> {
    if before.model.as_deref() == Some(pick) {
        return Ok(Restore::AlreadyPick);
    }
    let deadline = Instant::now() + wait;
    loop {
        let now = snapshot(path);
        if now.raw != before.raw && now.model.as_deref() == Some(pick) {
            break;
        }
        if Instant::now() >= deadline {
            return Ok(if now.raw != before.raw {
                Restore::ChangedElsewhere { model: now.model }
            } else {
                Restore::NotWritten
            });
        }
        std::thread::sleep(poll);
    }
    let keep = default.map(str::to_string).or_else(|| before.model.clone());
    if keep.as_deref() == Some(pick) {
        return Ok(Restore::Kept { model: pick.to_string() });
    }
    // Read once more right before the write: only K2's pick is undone.
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if model_of(&raw).as_deref() != Some(pick) {
        return Ok(Restore::ChangedElsewhere { model: model_of(&raw) });
    }
    let text = with_model(&raw, keep.as_deref())?;
    // There was no file before and the CLI's holds nothing else: no file.
    let empty = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.as_object().map(|o| o.is_empty()));
    if before.raw.is_none() && empty == Some(true) {
        std::fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    } else {
        write_atomic(path, &text)?;
    }
    Ok(Restore::Restored { to: keep })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-cms-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).expect("temp home");
        d
    }

    const CLAUDE_FILE: &str = "{\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  },\n  \"model\": \"sonnet\",\n  \"theme\": \"dark\"\n}\n";

    #[test]
    fn with_model_changes_only_the_model_value() {
        let out = with_model(CLAUDE_FILE, Some("opus")).expect("set");
        assert_eq!(out, CLAUDE_FILE.replace("\"sonnet\"", "\"opus\""));
        let removed = with_model(CLAUDE_FILE, None).expect("remove");
        assert_eq!(
            removed,
            "{\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  },\n  \"theme\": \"dark\"\n}\n"
        );
        let last = "{\n  \"a\": 1,\n  \"model\": \"x\"\n}";
        assert_eq!(with_model(last, None).expect("remove last"), "{\n  \"a\": 1\n}");
        assert_eq!(with_model("{\"model\":\"x\"}", None).expect("remove only"), "{}");
    }

    #[test]
    fn with_model_adds_the_key_in_the_files_own_indentation() {
        let src = "{\n    \"theme\": \"dark\"\n}\n";
        assert_eq!(with_model(src, Some("opus")).expect("add"), "{\n    \"model\": \"opus\",\n    \"theme\": \"dark\"\n}\n");
        assert_eq!(with_model("{}", Some("opus")).expect("empty object"), "{\n  \"model\": \"opus\"\n}");
        assert_eq!(with_model("", Some("opus")).expect("empty file"), "{\n  \"model\": \"opus\"\n}");
        // A nested "model" is someone else's key: not the top-level one.
        let nested = "{\"env\": {\"model\": \"keep\"}}";
        let out = with_model(nested, Some("opus")).expect("nested");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!((v["model"].as_str(), v["env"]["model"].as_str()), (Some("opus"), Some("keep")));
    }

    #[test]
    fn with_model_refuses_what_it_cant_edit_safely() {
        assert!(with_model("[1, 2]", Some("opus")).is_err());
        assert!(with_model("{ not json", Some("opus")).is_err());
        assert!(with_model("\"model\"", None).is_err());
    }

    /// The CLI writes K2's pick (and one other key of its own): the restore
    /// puts back the server default and keeps every other key, the file's
    /// formatting, and a key the CLI added.
    #[test]
    fn restore_after_the_cli_writes_the_pick() {
        let home = tmp_home("restore");
        let path = settings_path(&home);
        std::fs::write(&path, CLAUDE_FILE).unwrap();
        let before = snapshot(&path);
        let p2 = path.clone();
        let cli = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let raw = std::fs::read_to_string(&p2).unwrap();
            std::fs::write(&p2, raw.replace("\"sonnet\"", "\"haiku\"")).unwrap();
        });
        let r = wait_and_restore(&path, &before, "haiku", Some("opus"), Duration::from_secs(5), Duration::from_millis(20)).expect("restore");
        cli.join().unwrap();
        assert_eq!(r, Restore::Restored { to: Some("opus".into()) });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CLAUDE_FILE.replace("\"sonnet\"", "\"opus\""));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// No server default: the key goes back to what it was, or away when
    /// it wasn't there.
    #[test]
    fn restore_without_a_default_puts_back_the_old_value_or_removes_it() {
        let home = tmp_home("old");
        let path = settings_path(&home);
        let src = "{\n  \"theme\": \"dark\"\n}\n";
        std::fs::write(&path, src).unwrap();
        let before = snapshot(&path);
        std::fs::write(&path, with_model(src, Some("haiku")).unwrap()).unwrap();
        let r = wait_and_restore(&path, &before, "haiku", None, Duration::from_secs(2), Duration::from_millis(10)).unwrap();
        assert_eq!(r, Restore::Restored { to: None });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), src);

        std::fs::write(&path, CLAUDE_FILE).unwrap();
        let before = snapshot(&path);
        std::fs::write(&path, CLAUDE_FILE.replace("\"sonnet\"", "\"haiku\"")).unwrap();
        let r = wait_and_restore(&path, &before, "haiku", None, Duration::from_secs(2), Duration::from_millis(10)).unwrap();
        assert_eq!(r, Restore::Restored { to: Some("sonnet".into()) });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CLAUDE_FILE);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The file changed underneath to something that isn't K2's pick (a
    /// person's own `/model`): it is left exactly as they wrote it.
    #[test]
    fn no_restore_when_the_file_changed_to_something_else() {
        let home = tmp_home("else");
        let path = settings_path(&home);
        std::fs::write(&path, CLAUDE_FILE).unwrap();
        let before = snapshot(&path);
        let theirs = CLAUDE_FILE.replace("\"sonnet\"", "\"fable\"");
        std::fs::write(&path, &theirs).unwrap();
        let r = wait_and_restore(&path, &before, "haiku", Some("opus"), Duration::from_millis(300), Duration::from_millis(20)).unwrap();
        assert_eq!(r, Restore::ChangedElsewhere { model: Some("fable".into()) });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_written_when_the_cli_never_writes_or_already_said_the_pick() {
        let home = tmp_home("none");
        let path = settings_path(&home);
        std::fs::write(&path, CLAUDE_FILE).unwrap();
        let before = snapshot(&path);
        let r = wait_and_restore(&path, &before, "haiku", Some("opus"), Duration::from_millis(200), Duration::from_millis(20)).unwrap();
        assert_eq!(r, Restore::NotWritten);
        let r = wait_and_restore(&path, &before, "sonnet", Some("opus"), Duration::from_millis(200), Duration::from_millis(20)).unwrap();
        assert_eq!(r, Restore::AlreadyPick);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CLAUDE_FILE);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A wallet login's home links the shared settings file: the restore
    /// writes through the link and the link stays a link.
    #[cfg(unix)]
    #[test]
    fn restore_writes_through_a_symlinked_settings_file() {
        let shared = tmp_home("shared");
        let slot = tmp_home("slot");
        let real = settings_path(&shared);
        std::fs::write(&real, CLAUDE_FILE).unwrap();
        let link = settings_path(&slot);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let before = snapshot(&link);
        std::fs::write(&real, CLAUDE_FILE.replace("\"sonnet\"", "\"haiku\"")).unwrap();
        let r = wait_and_restore(&link, &before, "haiku", None, Duration::from_secs(2), Duration::from_millis(10)).unwrap();
        assert_eq!(r, Restore::Restored { to: Some("sonnet".into()) });
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), CLAUDE_FILE);
        let _ = std::fs::remove_dir_all(&shared);
        let _ = std::fs::remove_dir_all(&slot);
    }
}
