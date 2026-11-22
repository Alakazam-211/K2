//! `POST /cli/skin/roles/paths` (`k2 app role paths`): one room line's
//! folders, download entries and hidden paths on a role. Split out of
//! `skin_routes.rs` (0.46.17, the size ratchet) when the hide list came.

use crate::cli_response::CliResponse;
use crate::skin_routes::{actor_is_agent, app_users_off_response, json_body, person_only_refused, str_field};
use k2_core::skin;

/// One folder list from a body key: an array of strings (absent = empty).
fn folder_list(v: &serde_json::Value, key: &str) -> Result<Vec<String>, CliResponse> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| CliResponse::bad_request(format!("{key} must be an array of folder strings")))
            })
            .collect(),
        Some(_) => Err(CliResponse::bad_request(format!("{key} must be an array of folder strings"))),
    }
}

/// `POST /cli/skin/roles/paths` (`k2 app role paths`, room-scope R3,
/// RS-A1..A3): set one room line's folders on a role. Body `{name, handle,
/// read?: [..], write?: [..], upload?: [..], uploadDir?: ".."}` replaces
/// the line's folders; `{name, handle, clear: true}` (or nothing listed)
/// puts the line back to the whole room. The room must already be on the
/// role. `upload` (0.46.4, AC8) is the line's drop folders: narrower than
/// `write`, so an agent with "manage Apps" may set them like any folder.
///
/// `download` (0.46.4, AC15/AC18a) is the line's download entries (folders
/// or single files `files:download` may save without `files:read`):
/// - an omitted `download` keeps the stored entries (`download: []`
///   clears them), so editing read folders never drops what a person set;
/// - a body that names only `download` changes only the entries, never
///   the line's read/write/upload folders (so it can't unwall a line);
/// - `clear: true` still clears everything (narrowing).
///
/// `hide` (0.46.17) is the line's hidden paths (names or room-relative
/// paths its logins never see or read, even inside a permitted folder;
/// `skin::paths::hides`): `hide: [..]` replaces the list (`[]` empties it),
/// `hideAdd` / `hideRemove` add or take off entries. Like `download`, an
/// omitted hide list is kept, a body that names only hide fields changes
/// only that list, and `clear: true` keeps it (clearing the folders must
/// never reveal what was hidden). The right is the one that sets folders.
pub fn handle_roles_paths(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["name", "role", "id"]) else {
        return CliResponse::bad_request("missing role name");
    };
    let Some(handle) = str_field(&v, &["handle", "room", "workspace"]) else {
        return CliResponse::bad_request("missing handle (the room)");
    };
    let ids = match skin::resolve_room_tokens(&[handle.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        Ok(_) => return CliResponse::bad_request(format!("unknown workspace handle {handle:?}")),
        Err(e) => return CliResponse::bad_request(e),
    };
    let clear = v.get("clear").and_then(|x| x.as_bool()).unwrap_or(false);
    let hide_edit = match HideEdit::from_body(&v) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let req: Option<PathsBody> = if clear {
        None
    } else {
        let read = match folder_list(&v, "read") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let write = match folder_list(&v, "write") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let upload = match folder_list(&v, "upload") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let upload_dir = match v.get("uploadDir").or_else(|| v.get("upload_dir")) {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => return CliResponse::bad_request("uploadDir must be a folder string"),
        };
        let download_sent = !matches!(v.get("download"), None | Some(serde_json::Value::Null));
        let download = match folder_list(&v, "download") {
            Ok(l) => l,
            Err(r) => return r,
        };
        let folders_sent = !read.is_empty() || !write.is_empty() || !upload.is_empty() || upload_dir.is_some();
        if !folders_sent && !download_sent && !hide_edit.sent() {
            return CliResponse::bad_request(
                "nothing to set: send read, write, upload, uploadDir, download or hide (hideAdd, hideRemove), or clear:true for the whole room",
            );
        }
        let mut sent_download: Vec<String> = Vec::new();
        for e in &download {
            match skin::paths::normalize_folder(e) {
                Ok(f) if !sent_download.contains(&f) => sent_download.push(f),
                Ok(_) => {}
                Err(e) => return CliResponse::bad_request(format!("download: {e}")),
            }
        }
        Some(PathsBody { download_sent, sent_download, folders_sent, read, write, upload_dir, upload })
    };
    // Adding or widening a download entry: the agent that owns the App, with the
    // App-users switch on (Rosson 2026-11-03). Any other agent only narrows.
    let role_app = skin::find_role(id_or_name).ok().flatten().and_then(|r| r.app_id);
    let may_widen = !actor_is_agent(actor)
        || role_app
            .as_deref()
            .and_then(skin::ties::app_workspace)
            .is_some_and(|w| k2_core::workspace::settings::agents_can_manage_app_users(&w));
    // Review A5: the stored line is read, the widening checked and the new
    // line written in ONE transaction (`update_role_room_paths`).
    let written = skin::paths::update_role_room_paths(id_or_name, &ids[0], |stored| {
        let hide = hide_edit.apply(&stored.hide).map_err(skin::paths::PathsUpdateError::Bad)?;
        let Some(b) = &req else { return Ok(skin::RoomPathScope { hide, ..Default::default() }) };
        // The stored line: kept download entries, the widening check, and a
        // download-only body's other folders.
        let download = if b.download_sent {
            if !may_widen {
                if let Some(entry) = skin::paths::download_widening(&stored.download, &b.sent_download) {
                    return Err(skin::paths::PathsUpdateError::Refused(entry.to_string()));
                }
            }
            b.sent_download.clone()
        } else {
            stored.download.clone()
        };
        Ok(if b.folders_sent {
            skin::RoomPathScope {
                read: b.read.clone(),
                write: b.write.clone(),
                upload_dir: b.upload_dir.clone(),
                upload: b.upload.clone(),
                download,
                hide,
            }
        } else {
            skin::RoomPathScope { download, hide, ..stored.clone() }
        })
    });
    match written {
        Err(skin::paths::PathsUpdateError::Refused(_)) if role_app.is_some() => app_users_off_response(),
        Err(skin::paths::PathsUpdateError::Refused(_)) => person_only_refused(skin::CAP_FILES_DOWNLOAD),
        Err(skin::paths::PathsUpdateError::Bad(e)) => CliResponse::bad_request(e),
        Ok(role) => {
            let scope = role.room_paths.get(&ids[0]).cloned().unwrap_or_default();
            k2_core::log_debug!(
                "[skin] actor={actor} role paths {} handle={handle} clear={clear} read={:?} write={:?} upload={:?} uploadDir={:?} download={:?} hide={:?}",
                role.name,
                scope.read,
                scope.write,
                scope.upload,
                scope.upload_dir,
                scope.download,
                scope.hide
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
    }
}

/// The hide-list edit a `POST /cli/skin/roles/paths` body asks for
/// (0.46.17): a full replacement (`hide`), or entries to add and take off.
struct HideEdit {
    replace: Option<Vec<String>>,
    add: Vec<String>,
    remove: Vec<String>,
}

impl HideEdit {
    fn from_body(v: &serde_json::Value) -> Result<Self, CliResponse> {
        let replace = match v.get("hide") {
            None | Some(serde_json::Value::Null) => None,
            Some(_) => Some(folder_list(v, "hide")?),
        };
        let add = folder_list(v, "hideAdd")?;
        let remove = folder_list(v, "hideRemove")?;
        if replace.is_some() && (!add.is_empty() || !remove.is_empty()) {
            return Err(CliResponse::bad_request("send hide (the whole list) or hideAdd/hideRemove, not both"));
        }
        Ok(Self { replace, add, remove })
    }

    fn sent(&self) -> bool {
        self.replace.is_some() || !self.add.is_empty() || !self.remove.is_empty()
    }

    /// The new hide list from the stored one. Taking off an entry that
    /// isn't on the line is an error (a typo would otherwise look done).
    fn apply(&self, stored: &[String]) -> Result<Vec<String>, String> {
        if let Some(all) = &self.replace {
            return Ok(all.clone());
        }
        let mut out: Vec<String> = stored.to_vec();
        for r in &self.remove {
            let n = skin::paths::normalize_hide(r)?;
            let before = out.len();
            out.retain(|h| h != &n);
            if out.len() == before {
                return Err(format!(
                    "{n:?} is not hidden on this room line (hidden: {})",
                    if stored.is_empty() { "nothing".to_string() } else { stored.join(", ") }
                ));
            }
        }
        out.extend(self.add.iter().cloned());
        Ok(out)
    }
}

/// What a `POST /cli/skin/roles/paths` body asks for (`download`
/// normalized here for the widening check; the store normalizes the rest).
struct PathsBody {
    download_sent: bool,
    sent_download: Vec<String>,
    folders_sent: bool,
    read: Vec<String>,
    write: Vec<String>,
    upload_dir: Option<String>,
    upload: Vec<String>,
}
