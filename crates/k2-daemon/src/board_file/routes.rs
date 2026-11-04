//! `/cli/board/file/*` (prd-board-tab-v1 §5.2; the wire is
//! `.k2/prds/board-tab-b3-interface.md`). Keyed by `(workspace, path)`, with the
//! jail, deny list and rights of `docs::target` and `docs::host`, so a person's
//! tab and an agent's `k2 board --file` pass the same doors. The docs runtime
//! never starts for a board.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use base64::Engine as _;
use k2_mpc_board::{json as fj, package_guard_of, Blob, Board, Id, Limits, Package, Person, Refusal, RefusalCode, Sha256, MIN_READER};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256 as ShaHasher};

use super::{held, map, save, stage};
use crate::cli_response::CliResponse;
use crate::docs::caller::{refusal, DocCaller};
use crate::docs::host::DocRights;
use crate::docs::target::{self, Target};
use save::{Me, SaveFail, SaveInput};

pub const PREFIX: &str = "/cli/board/file/";
pub const INFO: &str = "/cli/board/file/info";
pub const READ: &str = "/cli/board/file/read";
pub const CREATE: &str = "/cli/board/file/create";
pub const START: &str = "/cli/board/file/start";
pub const SAVE: &str = "/cli/board/file/save";
pub const BLOB: &str = "/cli/board/file/blob";
pub const BLOB_PUT: &str = "/cli/board/file/blob/put";
pub const COPY: &str = "/cli/board/file/copy";
pub const COPY_RESCUE: &str = "/cli/board/file/copy/rescue";
pub const FIELDS: &str = "/cli/board/file/fields";
pub const HELD_RESOLVE: &str = "/cli/board/file/held/resolve";
pub const HELD_MINE: &str = "/cli/board/file/held/mine";
pub const LIST: &str = "/cli/board/file/list";

/// Every route, for the dispatcher and the tests.
pub const ROUTES: &[&str] = &[INFO, READ, CREATE, START, SAVE, BLOB, BLOB_PUT, COPY, COPY_RESCUE, FIELDS, HELD_RESOLVE, HELD_MINE, LIST];

/// The most a `save` may carry (as `k2 doc write`), and a `blob/put` chunk in base64.
const MAX_BODY: usize = 12 * 1024 * 1024;
/// A `copy/rescue` carries the whole board, in the widget's shape.
const MAX_RESCUE_BODY: usize = 300 * 1024 * 1024;
/// The largest `board.json` a `read` opens without `allowLarge`.
const TAB_BOARD_BYTES: u64 = 32 * 1024 * 1024;
/// How many bytes of boards may be open at once, so a hostile file or a saving
/// agent can't starve the daemon. One file over this still runs, alone.
const OPEN_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

/// The body cap for one request.
pub fn max_body(path: &str) -> usize {
    if path.split('?').next() == Some(COPY_RESCUE) {
        MAX_RESCUE_BODY
    } else {
        MAX_BODY
    }
}

// ---------- answers ----------

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn with_status(status: &'static str, v: Value) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: v.to_string() }
}

fn bad(hint: impl Into<String>) -> CliResponse {
    refusal("400 Bad Request", "bad_request", hint)
}

/// A format refusal: its code, where, and what a client needs to say so.
fn refused(r: &Refusal) -> CliResponse {
    let mut e = Map::new();
    e.insert("code".into(), json!(r.code.as_str()));
    e.insert("hint".into(), json!(r.detail));
    if let Some(at) = &r.at {
        e.insert("at".into(), json!(at));
    }
    if r.code == RefusalCode::MinReaderTooHigh {
        e.insert("needsReader".into(), json!(r.needs_reader));
        e.insert("ours".into(), json!(MIN_READER));
    }
    if r.code == RefusalCode::IdInvalid {
        e.insert("badIds".into(), json!(r.bad_ids));
        e.insert("badIdsTotal".into(), json!(r.bad_ids_total));
        e.insert("badIdsTruncated".into(), json!(r.bad_ids_truncated));
    }
    let status = if r.code == RefusalCode::IoError { "503 Service Unavailable" } else { "422 Unprocessable Entity" };
    with_status(status, json!({ "ok": false, "error": e }))
}

fn failed(f: &SaveFail) -> CliResponse {
    match f {
        SaveFail::NotFound => refusal("404 Not Found", "not_found", "this file was moved or deleted"),
        SaveFail::Refused(r) => refused(r),
        SaveFail::Bad(m) => bad(m.clone()),
        SaveFail::Io { code, message } => refusal("503 Service Unavailable", code, message.clone()),
        SaveFail::Busy => refusal("409 Conflict", "busy", "the file kept changing; try again, your changes are kept"),
        SaveFail::WriteFailed(m) => refusal("500 Internal Server Error", "write_failed", m.clone()),
    }
}

// ---------- the budget ----------

struct Budget {
    used: Mutex<u64>,
    freed: Condvar,
}

struct Spent(u64);

fn budget() -> &'static Budget {
    static B: OnceLock<Budget> = OnceLock::new();
    B.get_or_init(|| Budget { used: Mutex::new(0), freed: Condvar::new() })
}

impl Spent {
    fn take(bytes: u64) -> Spent {
        let want = bytes.min(OPEN_BUDGET_BYTES);
        let b = budget();
        let mut used = b.used.lock().unwrap_or_else(|p| p.into_inner());
        while *used + want > OPEN_BUDGET_BYTES {
            used = b.freed.wait(used).unwrap_or_else(|p| p.into_inner());
        }
        *used += want;
        Spent(want)
    }
}

impl Drop for Spent {
    fn drop(&mut self) {
        let b = budget();
        *b.used.lock().unwrap_or_else(|p| p.into_inner()) -= self.0;
        b.freed.notify_all();
    }
}

// ---------- the caller ----------

/// Rights on a target, with the teaching refusals. A share lives in the docs
/// store: with every file core off the runtime doesn't run, so one on disk isn't
/// seen and the refusal says why. This never starts the runtime.
fn rights(caller: &DocCaller, t: &Target, workspace: Option<&str>) -> Result<DocRights, CliResponse> {
    let rt = crate::docs::runtime().ok();
    let store = rt.as_ref().and_then(|r| r.store());
    crate::docs::routes::rights_or_refusal(caller, t, workspace, store.as_deref())
}

fn need_read(r: DocRights) -> Result<(), CliResponse> {
    if r.read {
        Ok(())
    } else {
        Err(refusal("403 Forbidden", "forbidden", "you can't read this file"))
    }
}

fn need_edit(r: DocRights) -> Result<(), CliResponse> {
    if r.edit {
        Ok(())
    } else {
        Err(refusal("403 Forbidden", "forbidden", "you can't change this file"))
    }
}

/// Who the caller is in a file: K2 Cores' C9. Matched by name and kind in
/// `people`; a token from a hash of the lineage, kind and name otherwise, stable
/// across reads with nothing stored. Display data, not access. On a file whose
/// names were stripped there's nothing to match, and the new token has no name,
/// so saving doesn't put one back.
fn me_of(caller: &DocCaller, board: &Board, lineage_hex: &str) -> Result<Me, Refusal> {
    let name = caller.participant(None).name;
    let kind = if caller.is_person() { "person" } else { "agent" };
    for (token, p) in &board.people {
        if p.name.as_deref() == Some(name.as_str()) && p.kind.as_deref() == Some(kind) {
            return Ok(Me { token: token.clone(), person: p.clone() });
        }
    }
    let stripped = !board.people.is_empty() && board.people.values().all(|p| p.name.is_none());
    let d = ShaHasher::digest(format!("{lineage_hex}\0{kind}\0{name}").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    Ok(Me {
        token: k2_mpc_board::mint_person_token(b),
        person: Person { name: (!stripped).then_some(name), kind: Some(kind.to_string()), extra: Default::default() },
    })
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn random16() -> [u8; 16] {
    let mut h = ShaHasher::new();
    h.update(uuid::Uuid::new_v4().as_bytes());
    h.update(uuid::Uuid::new_v4().as_bytes());
    let d = h.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

fn emit_changed(t: &Target, hash: &str, caller: &DocCaller) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::BoardFileChanged {
        workspace: t.root.to_string_lossy().to_string(),
        rel: t.rel.clone(),
        hash: hash.to_string(),
        by: caller.pid(),
    });
    crate::session_events::emit_fs_changed_for_paths([t.abs.to_string_lossy().to_string()]);
}

// ---------- the routes ----------

struct Input<'a> {
    body: Value,
    params: &'a HashMap<String, String>,
}

impl Input<'_> {
    fn str(&self, k: &str) -> Option<String> {
        match self.body.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => self.params.get(k).cloned(),
        }
    }

    fn flag(&self, k: &str) -> bool {
        match self.body.get(k) {
            Some(Value::Bool(b)) => *b,
            _ => self.str(k).is_some_and(|v| v == "1" || v == "true"),
        }
    }

    fn num(&self, k: &str) -> Option<u64> {
        match self.body.get(k) {
            Some(Value::Number(n)) => n.as_u64(),
            _ => self.params.get(k).and_then(|v| v.parse().ok()),
        }
    }

    fn target(&self, caller: &DocCaller) -> Result<Target, CliResponse> {
        target::resolve_board(self.str("workspace").as_deref(), &self.str("path").unwrap_or_default(), caller.default_workspace()).map_err(|r| match r.code {
            "file_not_found" => refusal("404 Not Found", "not_found", r.message),
            _ => refusal(r.status, r.code, r.message),
        })
    }
}

/// The entry for every transport. `caller` is resolved.
pub fn handle(method: &str, path: &str, params: &HashMap<String, String>, body: &[u8], caller: &DocCaller) -> CliResponse {
    if !ROUTES.contains(&path) {
        return CliResponse::not_found();
    }
    if !k2_core::board_preview::enabled() {
        return refusal("404 Not Found", "not_enabled", "Board files are a preview: the owner turns them on with K2_BOARD_FILE=1 or Settings' board_file_preview");
    }
    let is_post = method.eq_ignore_ascii_case("POST");
    // Mutating routes are POST only (feedback_post_only_route_guards).
    if matches!(path, CREATE | START | SAVE | BLOB_PUT | COPY | COPY_RESCUE | HELD_RESOLVE | HELD_MINE) && !is_post {
        return CliResponse { status: "405 Method Not Allowed", content_type: "application/json", body: r#"{"error":"POST only"}"#.to_string() };
    }
    if !is_post && !method.eq_ignore_ascii_case("GET") {
        return CliResponse { status: "405 Method Not Allowed", content_type: "application/json", body: r#"{"error":"GET or POST only"}"#.to_string() };
    }
    let parsed = if body.is_empty() {
        Value::Object(Map::new())
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => return bad("body must be a JSON object"),
            Err(e) => return bad(format!("body is not JSON: {e}")),
        }
    };
    let input = Input { body: parsed, params };
    held::sweep_if_due();
    let r = match path {
        INFO => info(&input, caller),
        READ => read(&input, caller),
        CREATE => create(&input, caller),
        START => start(&input, caller),
        SAVE => save_route(&input, caller),
        BLOB => blob(&input, caller),
        BLOB_PUT => blob_put(&input, caller),
        COPY => copy(&input, caller),
        COPY_RESCUE => rescue(&input, caller),
        FIELDS => fields(&input),
        HELD_RESOLVE => held_resolve(&input, caller),
        HELD_MINE => held_mine(&input, caller),
        LIST => list(caller),
        _ => return CliResponse::not_found(),
    };
    match r {
        Ok(resp) | Err(resp) => resp,
    }
}

/// The format's own tables: what a file's objects may hold.
fn fields(input: &Input) -> Result<CliResponse, CliResponse> {
    let schema: Value = serde_json::from_str(&k2_mpc_board::schema_json()).map_err(|e| refusal("500 Internal Server Error", "bad_schema", e.to_string()))?;
    let common = schema["common"].clone();
    let types = &schema["types"];
    match input.str("type").filter(|t| !t.is_empty()) {
        Some(t) => match types.get(&t) {
            Some(own) => Ok(ok(json!({ "type": t, "fields": own, "common": common }))),
            None => Err(refusal("404 Not Found", "not_found", format!("no type {t}: {}", types.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()))),
        },
        None => Ok(ok(json!({ "types": types, "common": common, "settings": schema["settings"], "kinds": schema["kinds"] }))),
    }
}

fn can_write(path: &Path) -> bool {
    let writable = |p: &Path| fs::metadata(p).map(|m| !m.permissions().readonly()).unwrap_or(false);
    writable(path) && path.parent().is_some_and(writable)
}

fn mtime_ms(m: &fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

fn io_refusal(e: &std::io::Error) -> CliResponse {
    match save::io_code(e) {
        "not_found" => refusal("404 Not Found", "not_found", "this file was moved or deleted"),
        code => refusal("503 Service Unavailable", code, e.to_string()),
    }
}

fn info(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let t = input.target(caller)?;
    let r = rights(caller, &t, input.str("workspace").as_deref())?;
    need_read(r)?;
    let meta = fs::metadata(&t.abs).map_err(|e| io_refusal(&e))?;
    let write = r.edit && can_write(&t.abs);
    if meta.len() == 0 {
        return Ok(ok(json!({ "empty": true, "bytes": 0, "mtimeMs": mtime_ms(&meta), "canWrite": write })));
    }
    let f = File::open(&t.abs).map_err(|e| io_refusal(&e))?;
    let guard = package_guard_of(BufReader::new(f), &Limits::V1).map_err(|e| refused(&e))?;
    Ok(ok(json!({ "hash": guard.to_string(), "bytes": meta.len(), "mtimeMs": mtime_ms(&meta), "canWrite": write })))
}

fn fj_json(v: &fj::Value) -> Value {
    fj::to_canonical(v).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

fn read(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let t = input.target(caller)?;
    let r = rights(caller, &t, input.str("workspace").as_deref())?;
    need_read(r)?;
    let meta = fs::metadata(&t.abs).map_err(|e| io_refusal(&e))?;
    if meta.len() == 0 {
        return Ok(ok(json!({ "empty": true })));
    }
    if let Some(have) = input.str("ifHash").filter(|h| !h.is_empty()) {
        let f = File::open(&t.abs).map_err(|e| io_refusal(&e))?;
        let guard = package_guard_of(BufReader::new(f), &Limits::V1).map_err(|e| refused(&e))?;
        if guard.to_string() == have {
            return Ok(ok(json!({ "same": true, "hash": have })));
        }
    }
    sweep_once(&t.abs);
    let _spent = Spent::take(meta.len());
    let limits = if input.flag("allowLarge") { Limits::V1 } else { Limits { board_json_bytes: TAB_BOARD_BYTES, ..Limits::V1 } };
    let f = File::open(&t.abs).map_err(|e| io_refusal(&e))?;
    let pkg = match Package::open(BufReader::new(f), &limits) {
        Ok(p) => p,
        // Decided from the zip's directory, before any parse.
        Err(e) if !input.flag("allowLarge") && e.code == RefusalCode::LimitPartBytes && e.at.as_deref() == Some("board.json") => {
            return Ok(ok(json!({ "large": true, "bytes": meta.len() })));
        }
        Err(e) => return Err(refused(&e)),
    };
    let board = pkg.board();
    let m = pkg.manifest();
    let me = me_of(caller, board, &m.lineage.to_hex()).map_err(|e| refused(&e))?;
    let settings = fj_json(&fj::Value::Obj(board.settings.clone()));
    let settings_rev = board.settings_rev().map_err(|e| refused(&e))?.to_string();
    let people: Map<String, Value> = board.people.iter().map(|(k, p)| (k.as_str().to_string(), fj_json(&p.to_value()))).collect();
    let missing: Vec<String> = pkg.missing_blobs().iter().map(Sha256::to_string).collect();
    let mut base = json!({
        "hash": pkg.package_guard().to_string(), "version": m.version, "minReader": m.min_reader,
        "settingsRev": settings_rev, "people": people, "me": me.token.as_str(), "missing": missing,
        "canWrite": r.edit && can_write(&t.abs),
    });
    let mut revs = Map::new();
    if input.str("shape").as_deref() == Some("file") {
        // An agent's view: the file's own objects, each with its rev.
        let mut objects = Vec::with_capacity(board.objects.len());
        for o in &board.objects {
            let rev = o.rev().map_err(|e| refused(&e))?.to_string();
            objects.push(json!({ "rev": rev, "object": fj_json(&o.to_value()) }));
        }
        base["objects"] = Value::Array(objects);
        base["settings"] = settings;
        base["frames"] = json!(board.frames.iter().map(Id::as_str).collect::<Vec<_>>());
        base["references"] = Value::Array(
            board.references().iter().map(|x| json!({ "from": x.from.as_str(), "field": x.field, "to": x.target.as_str(), "state": format!("{:?}", x.state).to_lowercase() })).collect(),
        );
        return Ok(ok(base));
    }
    let order: HashMap<&str, f64> = board.frames.iter().enumerate().map(|(i, f)| (f.as_str(), (i + 1) as f64)).collect();
    let mut objects = Vec::with_capacity(board.objects.len());
    for (i, o) in board.objects.iter().enumerate() {
        revs.insert(o.id.as_str().to_string(), json!(o.rev().map_err(|e| refused(&e))?.to_string()));
        objects.push(map::object_to_widget(&fj_json(&o.to_value()), (i + 1) as f64, order.get(o.id.as_str()).copied()));
    }
    if keeps_held(caller) {
        base["held"] = Value::Array(held_rows(&caller.pid(), &t.abs, board));
    }
    base["shape"] = json!(map::SHAPE);
    base["objects"] = Value::Array(objects);
    base["revs"] = Value::Object(revs);
    base["meta"] = map::settings_to_meta(&settings);
    Ok(ok(base))
}

/// The first `read` of a file in this daemon's run sweeps its stray temps.
fn sweep_once(path: &Path) {
    static SEEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let fresh = SEEN.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap_or_else(|p| p.into_inner()).insert(path.to_path_buf());
    if fresh {
        save::sweep_temps(path);
    }
}

fn create(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let dir = input.str("dir").filter(|d| !d.trim().is_empty()).ok_or_else(|| bad("dir is required: the folder the board goes in"))?;
    let t = target::board_folder(input.str("workspace").as_deref(), &dir, caller.default_workspace()).map_err(|r| refusal(r.status, r.code, r.message))?;
    need_edit(rights(caller, &t, input.str("workspace").as_deref())?)?;
    let folder = t.abs.parent().ok_or_else(|| bad("no folder"))?.to_path_buf();
    let bytes = save::new_board_bytes(random16()).map_err(|f| failed(&f))?;
    // O_CREAT|O_EXCL: it never lands on a file that's there. A crash between the
    // create and the write leaves a zero-byte file, which opens to "Start a board here".
    let name = input.str("name").filter(|n| !n.trim().is_empty());
    if let Some(n) = &name {
        if !target::is_board_file(n) || n.contains(['/', '\\']) {
            return Err(bad("name is a file name ending in .k5board"));
        }
    }
    if let Some(n) = &name {
        // A name an agent chose: the same no-replace, and a whole file from the first byte.
        let (p, hash) = save::create_named(&folder.join(n), random16()).map_err(|f| failed(&f))?;
        crate::session_events::emit_fs_changed_for_paths([p.to_string_lossy().to_string()]);
        let shown = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        return Ok(ok(json!({ "path": p.to_string_lossy(), "name": shown, "hash": hash })));
    }
    let created = k2_core::fs_commands::create_untitled(&folder.to_string_lossy(), Some("k5board"), None, &[], &bytes).map_err(|e| {
        if e == "Permission denied" {
            refusal("503 Service Unavailable", "read_only", format!("{} can't be written to", folder.display()))
        } else {
            refusal("400 Bad Request", "cant_create", e)
        }
    })?;
    let name = created.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let f = File::open(&created).map_err(|e| io_refusal(&e))?;
    let hash = package_guard_of(BufReader::new(f), &Limits::V1).map_err(|e| refused(&e))?.to_string();
    crate::session_events::emit_fs_changed_for_paths([created.to_string_lossy().to_string()]);
    Ok(ok(json!({ "path": created.to_string_lossy(), "name": name, "hash": hash })))
}

fn start(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let t = input.target(caller)?;
    need_edit(rights(caller, &t, input.str("workspace").as_deref())?)?;
    match save::start(&t.abs, random16()) {
        Ok(Some(hash)) => {
            emit_changed(&t, &hash, caller);
            Ok(ok(json!({ "hash": hash })))
        }
        Ok(None) => Err(refusal("409 Conflict", "not_empty", "this file isn't empty any more")),
        Err(f) => Err(failed(&f)),
    }
}

/// Who's saving, from a look at the file: the lock is taken inside.
fn me_for_write(path: &Path, caller: &DocCaller) -> Result<Me, CliResponse> {
    let f = File::open(path).map_err(|e| io_refusal(&e))?;
    let pkg = Package::open(BufReader::new(f), &Limits::V1).map_err(|e| refused(&e))?;
    me_of(caller, pkg.board(), &pkg.manifest().lineage.to_hex()).map_err(|e| refused(&e))
}

fn groups_of(input: &Input) -> Result<Vec<Value>, CliResponse> {
    match input.body.get("groups") {
        Some(Value::Array(g)) => Ok(g.clone()),
        None => Ok(Vec::new()),
        Some(_) => Err(bad("groups must be a list")),
    }
}

fn save_route(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let t = input.target(caller)?;
    need_edit(rights(caller, &t, input.str("workspace").as_deref())?)?;
    let groups = groups_of(input)?;
    let ops: usize = groups.iter().map(|g| g.get("ops").and_then(Value::as_array).map_or(0, Vec::len)).sum();
    if ops > crate::board_routes::MAX_OPS {
        return Err(refusal("400 Bad Request", "too_many", format!("one save is at most {} ops", crate::board_routes::MAX_OPS)));
    }
    let bytes = fs::metadata(&t.abs).map_err(|e| io_refusal(&e))?.len();
    let _spent = Spent::take(bytes);
    let me = me_for_write(&t.abs, caller)?;
    let owner = stage::owner_of(&caller.pid());
    let staged = |h: &Sha256| stage::get(&owner, h);
    let at = now();
    let base = input.str("baseHash");
    let a = save::save(&t.abs, &SaveInput { base_hash: base.as_deref(), groups: &groups, me: &me, at: &at, staged: &staged }).map_err(|f| failed(&f))?;
    if a.wrote {
        emit_changed(&t, &a.hash, caller);
    }
    let mut a = a;
    settle_held(&mut a, &groups, caller, &t.abs);
    let mut body = a.to_json(&me.token);
    if a.all_held(groups.len()) {
        body["error"] = json!({ "code": "board_conflict", "hint": "someone changed this since you read it; your edit wasn't saved" });
        return Ok(with_status("409 Conflict", body));
    }
    Ok(ok(body))
}

/// How deep the listing looks in a workspace, and how many it names.
const LIST_DEPTH: usize = 6;
const LIST_MAX: usize = 2000;
/// Folders the listing never walks into: they're big, or not a person's.
const LIST_SKIP: &[&str] = &[".git", "node_modules", "target", ".worktrees", "dist", "build", ".next", ".venv", "venv", "__pycache__"];

fn walk_boards(root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > LIST_DEPTH || out.len() >= LIST_MAX {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= LIST_MAX {
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let rel = p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            // `.k2/` only where a doc may live there: wiki, prds, docs.
            let k2_ok = rel == ".k2" || [".k2/wiki", ".k2/prds", ".k2/docs"].iter().any(|k| rel == *k || rel.starts_with(&format!("{k}/")));
            if LIST_SKIP.contains(&name.as_str()) || (rel.starts_with(".k2") && !k2_ok) {
                continue;
            }
            walk_boards(root, &p, depth + 1, out);
        } else if ft.is_file() && target::is_board_file(&name) {
            out.push(p);
        }
    }
}

/// `GET list`: every `.k5board` the caller may read, across the workspaces on
/// this server (Rosson 11-03), by name only. Each file is checked with the same
/// rights as `read`, so the listing never shows what a read would refuse: an
/// agent sees its own workspace's boards and those another workspace connected
/// and shared with it, nothing more.
fn list(caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let projects = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::list(&conn).map_err(|e| refusal("500 Internal Server Error", "db_error", e.to_string()))?
    };
    let mut boards = Vec::new();
    let mut truncated = false;
    for p in projects {
        let Ok(root) = Path::new(&p.path).canonicalize() else { continue };
        let mut found = Vec::new();
        walk_boards(&root, &root, 0, &mut found);
        truncated |= found.len() >= LIST_MAX;
        for f in found {
            let Ok(t) = target::resolve_board(None, &f.to_string_lossy(), caller.default_workspace()) else { continue };
            if t.project_id != p.id {
                continue;
            }
            match rights(caller, &t, None) {
                Ok(r) if r.read => {}
                _ => continue,
            }
            let meta = fs::metadata(&t.abs).ok();
            boards.push(json!({
                "path": t.abs.to_string_lossy(), "rel": t.rel, "workspace": t.root.to_string_lossy(),
                "workspaceName": if p.handle.trim().is_empty() { p.name.clone() } else { p.handle.clone() },
                "name": t.abs.file_stem().and_then(|n| n.to_str()).unwrap_or(""),
                "bytes": meta.as_ref().map_or(0, |m| m.len()), "mtimeMs": meta.as_ref().map_or(0, mtime_ms),
            }));
        }
    }
    Ok(ok(json!({ "boards": boards, "truncated": truncated })))
}

/// A person in a tab keeps a held edit. An agent or a K2 terminal has its command.
fn keeps_held(caller: &DocCaller) -> bool {
    matches!(caller, DocCaller::Person { via, .. } if *via != crate::docs::caller::VIA_TERMINAL)
}

/// `read`'s rows, with `theirs` and `why` from the file as it is now.
fn held_rows(pid: &str, path: &Path, board: &Board) -> Vec<Value> {
    held::for_file(pid, path)
        .into_iter()
        .map(|row| {
            let mut ids: Vec<String> = Vec::new();
            for op in row.get("ops").and_then(Value::as_array).into_iter().flatten() {
                if let Some(id) = op.get("id").and_then(Value::as_str) {
                    if !ids.iter().any(|x| x == id) {
                        ids.push(id.to_string());
                    }
                }
            }
            let objects: Vec<Value> = ids
                .iter()
                .map(|id| match board.objects.iter().find(|o| o.id.as_str() == id) {
                    Some(o) => json!({ "id": id, "why": "changed", "theirs": map::object_to_widget(&fj_json(&o.to_value()), 1.0, None) }),
                    None => json!({ "id": id, "why": "deleted" }),
                })
                .collect();
            let mut out = json!({ "heldId": row.get("heldId"), "groupId": row.get("groupId"), "byAll": row.get("byAll"), "ops": row.get("ops"), "objects": objects });
            if let Some(m) = row.get("mine") {
                out["mine"] = m.clone();
            }
            out
        })
        .collect()
}

/// After a save: each held group gets its row before the answer goes out, and a
/// **Use mine** that passed clears the one it came from. One that conflicts again
/// keeps its row. A row that can't be kept says so: the client then treats the
/// edit as held in memory only, and asks before it's lost.
fn settle_held(a: &mut save::SaveAnswer, groups: &[Value], caller: &DocCaller, path: &Path) {
    let pid = caller.pid();
    let keeps = keeps_held(caller);
    let group_of = |id: &str| groups.iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id));
    let lineage = if keeps && !a.held.is_empty() {
        File::open(path).ok().and_then(|f| Package::open(BufReader::new(f), &Limits::V1).ok()).map(|p| p.manifest().lineage.to_hex())
    } else {
        None
    };
    let hash = a.hash.clone();
    for entry in a.held.iter_mut() {
        let Some(gid) = entry.get("groupId").and_then(Value::as_str).map(str::to_string) else { continue };
        let Some(group) = group_of(&gid) else { continue };
        if let Some(existing) = group.get("heldId").and_then(Value::as_str) {
            entry["heldId"] = json!(existing);
            continue;
        }
        if !keeps {
            continue;
        }
        let objects = entry.get("objects").cloned().unwrap_or_else(|| json!([]));
        let kept = match &lineage {
            Some(l) => held::store(&pid, l, path, &hash, group, &objects),
            None => Err(held::HeldError::NotFound),
        };
        match kept {
            Ok(id) => entry["heldId"] = json!(id),
            Err(held::HeldError::Full) => entry["heldError"] = json!("held_full"),
            Err(e) => {
                k2_core::log_debug!("[board-file] a held edit wasn't kept: {e:?}");
                entry["heldError"] = json!("held_not_kept");
            }
        }
    }
    let still_held: Vec<String> = a.held.iter().filter_map(|h| h.get("groupId").and_then(Value::as_str).map(str::to_string)).collect();
    let refused: Vec<String> = a.refused.iter().filter_map(|h| h.get("groupId").and_then(Value::as_str).map(str::to_string)).collect();
    for g in groups {
        let (Some(gid), Some(id)) = (g.get("id").and_then(Value::as_str), g.get("heldId").and_then(Value::as_str)) else { continue };
        if !still_held.iter().any(|x| x == gid) && !refused.iter().any(|x| x == gid) {
            let _ = held::resolve(&pid, id);
        }
    }
}

fn held_error(e: held::HeldError) -> CliResponse {
    match e {
        held::HeldError::Full => refusal("413 Payload Too Large", "held_full", "too many edits are held: resolve some first"),
        held::HeldError::NotFound => refusal("404 Not Found", "not_found", "that held edit is gone"),
        held::HeldError::BadId => bad("heldId isn't one of K2's"),
        held::HeldError::Io(e) => io_refusal(&e),
    }
}

fn held_resolve(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let id = input.str("heldId").ok_or_else(|| bad("heldId is required"))?;
    held::resolve(&caller.pid(), &id).map_err(held_error)?;
    Ok(ok(json!({ "ok": true })))
}

fn held_mine(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let id = input.str("heldId").ok_or_else(|| bad("heldId is required"))?;
    let mine = input.body.get("mine").filter(|m| m.is_object()).ok_or_else(|| bad("mine is {id: object}"))?;
    held::set_mine(&caller.pid(), &id, mine).map_err(held_error)?;
    Ok(ok(json!({ "ok": true })))
}

/// The verified blob last read, kept between chunks: `read_blob` re-reads and re-hashes.
type BlobCache = Mutex<Option<(PathBuf, Sha256, Sha256, Arc<Blob>)>>;

fn blob_cache() -> &'static BlobCache {
    static C: OnceLock<BlobCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// PNG, JPEG, WebP, GIF, from the bytes: a manifest can lie.
pub fn sniff_image(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

fn blob(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let t = input.target(caller)?;
    need_read(rights(caller, &t, input.str("workspace").as_deref())?)?;
    let text = input.str("hash").ok_or_else(|| bad("hash is required"))?;
    let hash = Sha256::parse(&text).map_err(|_| bad(format!("{text} isn't a blob's hash")))?;
    let f = File::open(&t.abs).map_err(|e| io_refusal(&e))?;
    let guard = package_guard_of(BufReader::new(f), &Limits::V1).map_err(|e| refused(&e))?;
    let cached = blob_cache().lock().unwrap_or_else(|p| p.into_inner()).as_ref().filter(|c| c.0 == t.abs && c.1 == guard && c.2 == hash).map(|c| c.3.clone());
    let b = match cached {
        Some(b) => b,
        None => {
            let meta = fs::metadata(&t.abs).map_err(|e| io_refusal(&e))?;
            let _spent = Spent::take(meta.len());
            let f = File::open(&t.abs).map_err(|e| io_refusal(&e))?;
            let mut pkg = Package::open(BufReader::new(f), &Limits::V1).map_err(|e| refused(&e))?;
            let b = Arc::new(pkg.read_blob(&hash).map_err(|e| refused(&e))?);
            *blob_cache().lock().unwrap_or_else(|p| p.into_inner()) = Some((t.abs.clone(), guard, hash, b.clone()));
            b
        }
    };
    let total = b.bytes.len() as u64;
    let offset = input.num("offset").unwrap_or(0).min(total);
    let len = input.num("len").unwrap_or(k2_core::garden_data::blobs::BLOB_CHUNK_BYTES as u64).min(k2_core::garden_data::blobs::BLOB_CHUNK_BYTES as u64).min(total - offset);
    let chunk = &b.bytes[offset as usize..(offset + len) as usize];
    let sniffed = sniff_image(&b.bytes);
    // The file's media type is only ever a string in JSON: never a content type here.
    Ok(ok(json!({
        "hash": text, "mime": sniffed.unwrap_or(b.media_type.as_str()), "display": sniffed.is_some(),
        "total": total, "offset": offset, "data": base64::engine::general_purpose::STANDARD.encode(chunk),
    })))
}

fn blob_put(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let text = input.str("hash").ok_or_else(|| bad("hash is required"))?;
    let hash = Sha256::parse(&text).map_err(|_| bad(format!("{text} isn't a blob's hash")))?;
    let offset = input.num("offset").unwrap_or(0);
    let total = input.num("total").ok_or_else(|| bad("total is required"))?;
    let mime = input.str("mime").unwrap_or_else(|| "application/octet-stream".to_string());
    let data = input.str("data").ok_or_else(|| bad("data is required, in base64"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).map_err(|_| bad("data isn't base64"))?;
    let owner = stage::owner_of(&caller.pid());
    match stage::put(&owner, &hash, offset, total, &mime, &bytes) {
        Ok((received, done)) => Ok(ok(json!({ "received": received, "done": done }))),
        Err(stage::StageError::Refused(status, code, hint)) => Err(refusal(status, code, hint)),
        Err(stage::StageError::Io(e)) => Err(io_refusal(&e)),
    }
}

/// Where a copy may land, as a `Target` for the rights check. A person who owns
/// the box may save anywhere on it, since the point can be to leave a full disk
/// or a gone share; everyone else stays in a workspace they can edit.
fn copy_folder(input: &Input, caller: &DocCaller, to: &Path) -> Result<(), CliResponse> {
    if !to.is_absolute() {
        return Err(bad("to must be a full path"));
    }
    if !target::is_board_file(&to.to_string_lossy()) {
        return Err(bad("a copy's name ends in .k5board"));
    }
    let dir = to.parent().filter(|d| d.is_dir()).ok_or_else(|| refusal("404 Not Found", "folder_not_found", format!("no folder for {}", to.display())))?;
    if let DocCaller::Person { key, via } = caller {
        if *via != crate::docs::caller::VIA_TERMINAL && crate::docs::host::person_rights(key).owner {
            return Ok(());
        }
    }
    let t = target::board_folder(input.str("workspace").as_deref(), &dir.to_string_lossy(), caller.default_workspace()).map_err(|r| refusal(r.status, r.code, r.message))?;
    need_edit(rights(caller, &t, input.str("workspace").as_deref())?)
}

fn copied(p: &Path, a: &save::Copied) -> CliResponse {
    crate::session_events::emit_fs_changed_for_paths([p.to_string_lossy().to_string()]);
    ok(json!({
        "path": p.to_string_lossy(), "name": p.file_name().and_then(|n| n.to_str()).unwrap_or(""), "hash": a.hash,
        "missing": a.dropped.iter().map(Sha256::to_string).collect::<Vec<_>>(),
    }))
}

fn missing_answer(list: &[Sha256]) -> CliResponse {
    let n = list.len();
    with_status(
        "409 Conflict",
        json!({ "ok": false, "error": { "code": "missing", "hint": format!("{n} picture{} can't be read; save without {}?", if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "them" }) },
                "missing": list.iter().map(Sha256::to_string).collect::<Vec<_>>() }),
    )
}

fn copy(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let t = input.target(caller)?;
    need_read(rights(caller, &t, input.str("workspace").as_deref())?)?;
    let to = PathBuf::from(input.str("to").ok_or_else(|| bad("to is required"))?);
    copy_folder(input, caller, &to)?;
    let groups = groups_of(input)?;
    let me = me_for_write(&t.abs, caller)?;
    let owner = stage::owner_of(&caller.pid());
    let staged = |h: &Sha256| stage::get(&owner, h);
    let at = now();
    let inp = SaveInput { base_hash: None, groups: &groups, me: &me, at: &at, staged: &staged };
    match save::copy_to(&t.abs, &to, &inp, input.flag("dropMissing")) {
        Ok((p, a)) => Ok(copied(&p, &a)),
        Err(save::CopyFail::Missing(list)) => Err(missing_answer(&list)),
        Err(save::CopyFail::Fail(f)) => Err(failed(&f)),
    }
}

fn rescue(input: &Input, caller: &DocCaller) -> Result<CliResponse, CliResponse> {
    let to = PathBuf::from(input.str("to").ok_or_else(|| bad("to is required"))?);
    copy_folder(input, caller, &to)?;
    let owner = stage::owner_of(&caller.pid());
    let staged = |h: &Sha256| stage::get(&owner, h);
    match save::rescue_to(&to, &input.body, &staged, input.flag("dropMissing"), random16()) {
        Ok((p, a)) => Ok(copied(&p, &a)),
        Err(save::CopyFail::Missing(list)) => Err(missing_answer(&list)),
        Err(save::CopyFail::Fail(f)) => Err(failed(&f)),
    }
}
