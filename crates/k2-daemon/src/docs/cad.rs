//! The CAD core's reads: `.dxf` and `.dwg` drawings, and the AV reading of a
//! set of them. Read only ([`super::cad_read_only`]); every read is of the
//! doc's current version, with no join and no presence (a read here never
//! shows anyone in the doc).
//!
//! | Route | Method | Answer |
//! |---|---|---|
//! | `/cli/docs/cad/draw` | GET | the drawing-model read **as the whole body**, the pack's JSON as it gives it (no `{ok}` around it): `what=header` is `draw header` (`cad.drawing.header/1`), `what=page&space=S[&cursor=C]` is `draw page` (`cad.drawing.page/1`, `next` the pack's next call). The viewer's route |
//! | `/cli/docs/cad/read` | GET | one of the pack's read verbs, `argv` a JSON array (`["layer","list"]`): `{ok, path, version, verb, shape, stdout, data, next}` |
//! | `/cli/docs/av/read` | GET | the AV reading of a set: `config` (a workspace file of `key = value` lines) and `paths` (a JSON array of drawings); `what=read` (`av.set/1`, every row) or `what=summary` (counts and finding codes only, never a value) |
//!
//! **Paths, rights and walls** are the docs routes' own (`routes::handle`):
//! `path` absolute or workspace-relative, `workspace` optional, the read
//! right needed (a cross-workspace agent needs a share). A file that isn't a
//! drawing is `400 not_a_drawing`; while the CAD core is off every route
//! answers `403 core_off` for it (exit 3), as every docs verb does.
//!
//! **The version.** Each read is of the doc's current version: an outside
//! save is taken in first (`check_disk`), and that version is the pack's
//! `--version` (in every answer, and in a page cursor, `v<version>:...`). A
//! cursor from another version is the pack's `stale_cursor` (`409`): the
//! viewer reads the header again. While a viewer pages, the drawing is kept
//! open per version ([`Cache`]): the DXF bytes (each page reads them again,
//! the pack keeps nothing between calls) or the DWG reader's opened drawing
//! (as `DwgDrawing` asks a host to keep it).
//!
//! **Which reads.** A `.dxf` takes the DXF pack's twelve read verbs
//! (`k2_mpc_dxf::verbs::NAMES`). A `.dwg` takes `draw header` and `draw
//! page` (the DWG reader's), and the reads the drawing model shares across
//! formats over the model it fills (`drawing info`, `layer list`, `block
//! list`, `entity find`, `text find`, `attrib find`, `block show`, `entity
//! props`); `header get` and `check` are DXF's own.
//!
//! **Errors.** A pack refusal answers with its name as `error.code`, its id
//! (`CAD-E…`, `DXF-E…`, `DWG-E…`, `AV-E…`) as `error.id` and its exit:
//! `stale_cursor` 409, an address that names nothing 404, another usage
//! refusal 400, a drawing that doesn't read 422.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use k2_core::{docs_cores, product_name};
use k2_mpc_base::blob::BlobId;
use k2_mpc_base::session::SessionError;
use k2_mpc_base::store::{DocKey, VersionId};
use k2_mpc_cad_model::draw::{DrawSource, NoXrefs};
use k2_mpc_cad_model::reads::{self, ReadOut};
use k2_mpc_cad_model::CadError;
use k2_mpc_dwg::drawing::DwgDrawing;

use super::blob_packs::{kind_by_name, BlobKind};
use super::caller::{refusal, DocCaller};
use super::routes::{self, Call, Input};
use super::target::{self, Target};
use crate::cli_response::CliResponse;

/// `GET /cli/docs/cad/draw`: the viewer's drawing-model read.
pub const DRAW: &str = "/cli/docs/cad/draw";
/// `GET /cli/docs/cad/read`: one read verb (`k2 doc cad`).
pub const READ: &str = "/cli/docs/cad/read";
/// `GET /cli/docs/av/read`: the AV reading of a set (`k2 doc av`).
pub const AV: &str = "/cli/docs/av/read";

/// Most drawings in one AV reading.
pub const AV_MAX_SHEETS: usize = 200;
/// Largest AV configuration file.
pub const AV_CONFIG_MAX: u64 = 64 << 10;
/// Most opened drawings kept while viewers page ([`Cache`]).
const CACHE_ENTRIES: usize = 4;
/// How long an opened drawing is kept after its last read.
const CACHE_IDLE: Duration = Duration::from_secs(10 * 60);

/// The DWG reader's own verbs; the rest of a `.dwg`'s reads are the model's.
const DWG_DRAW_VERBS: [&str; 2] = ["draw header", "draw page"];
/// The reads the drawing model shares across formats (`k2_mpc_cad_model::reads`).
const MODEL_VERBS: [&str; 8] =
    ["drawing info", "layer list", "block list", "entity find", "text find", "attrib find", "block show", "entity props"];

/// Whether `path` is one of the routes on a single drawing.
pub fn on_a_drawing(path: &str) -> bool {
    path == DRAW || path == READ
}

/// Before the generic core gate, for the routes on one drawing: a file that
/// isn't a drawing is `400 not_a_drawing`; a drawing while the CAD core is
/// off is `403 core_off` for the CAD core (never the any-file core it is
/// served by then).
pub fn precheck(t: &Target) -> Option<CliResponse> {
    if !super::cad_read_only::is_drawing(&t.rel) {
        return Some(not_a_drawing(&t.rel));
    }
    (!super::cores::is_on(docs_cores::CAD)).then(|| super::cores::core_off(docs_cores::CAD, &t.rel))
}

fn not_a_drawing(rel: &str) -> CliResponse {
    routes::with_exit(
        refusal("400 Bad Request", "not_a_drawing", format!("{rel} isn't a drawing: the CAD reads take .dxf and .dwg files")),
        k2_mpc_base::tools::exit::USAGE,
    )
}

/// A pack's refusal: its name, id and exit (module docs for the status).
pub fn cad_refusal(e: &CadError) -> CliResponse {
    let status = match (e.code.name, e.code.exit) {
        ("stale_cursor", _) => "409 Conflict",
        (_, 4) => "404 Not Found",
        ("usage" | "name_ambiguous" | "not_a_boundary" | "bad_config", _) => "400 Bad Request",
        _ => "422 Unprocessable Entity",
    };
    let hint = if e.next.is_empty() { e.why.clone() } else { format!("{}: {}", e.why, e.next) };
    let mut r = routes::with_exit(refusal(status, e.code.name, hint), e.code.exit);
    if let Ok(mut v) = serde_json::from_str::<Value>(&r.body) {
        if let Some(o) = v.get_mut("error").and_then(Value::as_object_mut) {
            o.insert("id".into(), Value::from(e.code.id));
            if let Some(at) = e.offset {
                o.insert("offset".into(), Value::from(at));
            }
            r.body = v.to_string();
        }
    }
    r
}

// ── the drawing, kept open per version ─────────────────────────────────

/// One opened version of a drawing.
#[derive(Clone)]
pub enum Opened {
    /// A DXF file's bytes: each read parses them (the pack keeps nothing).
    Dxf(Arc<Vec<u8>>),
    /// A DWG file, opened by its reader.
    Dwg(Arc<DwgDrawing>),
}

struct Entry {
    key: DocKey,
    version: VersionId,
    blob: BlobId,
    opened: Opened,
    used: Instant,
}

/// The drawings viewers are paging through: at most [`CACHE_ENTRIES`], each
/// kept [`CACHE_IDLE`] after its last read. Keyed by doc, version and the
/// version's content id, so a restore or an outside save is a new entry.
struct Cache(Mutex<VecDeque<Entry>>);

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Cache(Mutex::new(VecDeque::new())))
}

impl Cache {
    fn get(&self, key: &DocKey, version: VersionId, blob: &BlobId) -> Option<Opened> {
        let mut q = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        q.retain(|e| now.duration_since(e.used) < CACHE_IDLE);
        let e = q.iter_mut().find(|e| &e.key == key && e.version == version && &e.blob == blob)?;
        e.used = now;
        Some(e.opened.clone())
    }

    fn put(&self, key: DocKey, version: VersionId, blob: BlobId, opened: Opened) {
        let mut q = self.0.lock().unwrap_or_else(|p| p.into_inner());
        q.retain(|e| e.key != key);
        while q.len() >= CACHE_ENTRIES {
            let oldest = q.iter().enumerate().min_by_key(|(_, e)| e.used).map(|(i, _)| i).unwrap_or(0);
            q.remove(oldest);
        }
        q.push_back(Entry { key, version, blob, opened, used: Instant::now() });
    }
}

/// A drawing's current version, opened: the doc is loaded (its file taken
/// in), an outside save checked for, and its content read from the docs
/// store by the version's content id. No join: nobody is shown in the doc.
pub fn head(rt: &Arc<super::Runtime>, t: &Target) -> Result<(VersionId, Opened), CliResponse> {
    let kind = kind_by_name(&t.rel);
    let s = super::tier0::open(rt, t)?;
    s.check_disk().map_err(|e| routes::session_error(&e))?;
    let st = s.status().map_err(|e| routes::session_error(&e))?;
    let key = t.key();
    if let Some(o) = cache().get(&key, st.version, &st.current.blob) {
        return Ok((st.version, o));
    }
    let unavailable = || refusal("503 Service Unavailable", "docs_unavailable", "the docs store isn't open");
    let store = rt.store().ok_or_else(unavailable)?;
    let blobs = rt.blobs().ok_or_else(unavailable)?;
    let id = store
        .find(&key)
        .map_err(|e| routes::session_error(&SessionError::Store(e)))?
        .ok_or_else(|| refusal("404 Not Found", "doc_not_found", format!("{} isn't a doc yet", t.rel)))?;
    let bytes = blobs
        .read_to_vec_of(&key.space, id, &st.current.blob, super::blob_max_bytes())
        .map_err(|e| routes::session_error(&SessionError::Blob(e)))?;
    let opened = match kind {
        BlobKind::Dwg => Opened::Dwg(Arc::new(DwgDrawing::open(&bytes, &k2_mpc_dwg::Limits::default()).map_err(|e| cad_refusal(&e))?)),
        _ => Opened::Dxf(Arc::new(bytes)),
    };
    cache().put(key, st.version, st.current.blob, opened.clone());
    Ok((st.version, opened))
}

/// Run read verb `verb` (`args` after it) on an opened drawing at `version`.
/// The draw verbs take the version K2 read (`--version`), which the caller
/// can't set.
pub fn run(opened: &Opened, version: VersionId, verb: &str, args: &[String]) -> Result<ReadOut, CadError> {
    let v = version.to_string();
    let mut a: Vec<&str> = Vec::with_capacity(args.len() + 2);
    if verb.starts_with("draw ") {
        a.extend(["--version", v.as_str()]);
    }
    a.extend(args.iter().map(String::as_str));
    match opened {
        Opened::Dxf(bytes) => k2_mpc_dxf::verbs::run(bytes, verb, &a),
        Opened::Dwg(d) => {
            let d: &DwgDrawing = d;
            match verb {
                "draw header" => k2_mpc_cad_model::draw::header(d, &a, &NoXrefs),
                "draw page" => k2_mpc_cad_model::draw::page(d, &a),
                "drawing info" => Ok(reads::info(d.doc(), Vec::new())),
                "layer list" => reads::layer_list(d.doc(), &a),
                "block list" => reads::block_list(d.doc(), &a),
                "entity find" => reads::entity_find(d.doc(), &a),
                "text find" => reads::text_find(d.doc(), &a),
                "attrib find" => reads::attrib_find(d.doc(), &a),
                "block show" => reads::block_show(d.doc(), &a),
                "entity props" => reads::entity_props(d.doc(), &a),
                _ => Err(CadError::new(
                    k2_mpc_cad_model::error::USAGE,
                    format!("no DWG read verb {verb:?}"),
                    format!("the DWG reads: {}, {}", DWG_DRAW_VERBS.join(", "), MODEL_VERBS.join(", ")),
                )),
            }
        }
    }
}

/// `argv`'s verb (one word or two, as the pack names them) and the rest.
fn split_verb(argv: &[String], kind: BlobKind) -> Result<(String, Vec<String>), CliResponse> {
    let names: Vec<&str> = match kind {
        BlobKind::Dwg => DWG_DRAW_VERBS.iter().chain(MODEL_VERBS.iter()).copied().collect(),
        _ => k2_mpc_dxf::verbs::NAMES.to_vec(),
    };
    let two = argv.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(" ");
    let (verb, n) = if names.contains(&two.as_str()) {
        (two, 2)
    } else if let Some(one) = argv.first().filter(|w| names.contains(&w.as_str())) {
        (one.clone(), 1)
    } else {
        let what = if kind == BlobKind::Dwg { "a .dwg's reads" } else { "a .dxf's reads" };
        return Err(routes::bad(format!("argv must start with a read verb ({what}: {})", names.join(", "))));
    };
    let rest = argv[n..].to_vec();
    if rest.iter().any(|a| a == "--version" || a.starts_with("--version=")) {
        return Err(routes::bad(concat!("--version is the doc's current version: ", product_name!(), " sets it")));
    }
    Ok((verb, rest))
}

/// The pack's typed data as JSON.
fn data_json(out: &ReadOut) -> Value {
    serde_json::from_str(&out.data.to_json()).unwrap_or(Value::Null)
}

/// `/cli/docs/cad/draw` and `/cli/docs/cad/read`, after the docs family
/// resolved the caller, the target, its core and the read right.
pub(crate) fn handle(path: &str, rt: &Arc<super::Runtime>, c: &Call) -> CliResponse {
    let t = c.target;
    if path == DRAW {
        let (verb, args) = match c.input.str("what").as_deref().map(str::trim) {
            Some("header") => ("draw header", Vec::new()),
            Some("page") => {
                let Some(space) = c.input.str("space").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
                    return routes::bad("what=page needs space: model, or a block's or a layout's @h:<handle>");
                };
                let mut a = vec!["--space".to_string(), space];
                if let Some(cur) = c.input.str("cursor").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                    a.extend(["--cursor".to_string(), cur]);
                }
                ("draw page", a)
            }
            Some(w) => return routes::bad(format!("what must be header or page (got {w})")),
            None => return routes::bad("what is required: header or page"),
        };
        let (version, opened) = match head(rt, t) {
            Ok(h) => h,
            Err(r) => return r,
        };
        return match run(&opened, version, verb, &args) {
            // The pack's object is the whole body, as it prints it.
            Ok(out) => CliResponse::ok_json(format!("{}\n", out.data.to_json_pretty())),
            Err(e) => cad_refusal(&e),
        };
    }
    let argv: Vec<String> = match c.input.str("argv").map(|s| serde_json::from_str::<Vec<String>>(&s)) {
        Some(Ok(a)) if !a.is_empty() => a,
        _ => match c.input.body.get("argv").and_then(Value::as_array) {
            Some(a) if !a.is_empty() && a.iter().all(Value::is_string) => {
                a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()
            }
            _ => return routes::bad("argv is required: a JSON array of the read verb and its arguments, [\"layer\",\"list\"]"),
        },
    };
    let (verb, rest) = match split_verb(&argv, kind_by_name(&t.rel)) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (version, opened) = match head(rt, t) {
        Ok(h) => h,
        Err(r) => return r,
    };
    match run(&opened, version, &verb, &rest) {
        Ok(out) => routes::ok(json!({
            "path": t.rel,
            "version": version,
            "verb": verb,
            "shape": format!("{}/{}", out.spec.0, out.spec.1),
            "stdout": out.stdout,
            "data": data_json(&out),
            "next": out.next,
        })),
        Err(e) => cad_refusal(&e),
    }
}

// ── the AV reading of a set ─────────────────────────────────────────────

/// A string list param: a JSON array in the query, or an array in the body.
fn list_param(input: &Input<'_>, k: &str) -> Option<Vec<String>> {
    if let Some(Value::Array(a)) = input.body.get(k) {
        return a.iter().map(|v| v.as_str().map(str::to_string)).collect();
    }
    input.str(k).and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
}

/// One sheet of a set: resolved, its core and the read right checked as
/// for every docs route, then read at its current version.
fn sheet(rt: &Arc<super::Runtime>, caller: &DocCaller, ws: Option<&str>, path: &str) -> Result<(Target, VersionId, Opened), CliResponse> {
    let t = target::resolve(ws, path, caller.default_workspace()).map_err(|r| refusal(r.status, r.code, r.message))?;
    if let Some(r) = precheck(&t) {
        return Err(r);
    }
    super::cores::gate(&t)?;
    let store = rt.store();
    routes::rights_or_refusal(caller, &t, ws, store.as_deref())?;
    drop(store);
    let (v, o) = head(rt, &t)?;
    Ok((t, v, o))
}

/// `GET /cli/docs/av/read` (module docs). Every drawing and the
/// configuration file need the read right; the drawings need the CAD core.
pub(super) fn av_route(caller: &DocCaller, input: &Input<'_>) -> CliResponse {
    let summary = match input.str("what").as_deref().map(str::trim) {
        None | Some("read") => false,
        Some("summary") => true,
        Some(w) => return routes::bad(format!("what must be read or summary (got {w})")),
    };
    let Some(paths) = list_param(input, "paths").filter(|p| !p.is_empty()) else {
        return routes::bad("paths is required: a JSON array of the set's drawings (.dxf, .dwg)");
    };
    if paths.len() > AV_MAX_SHEETS {
        return routes::bad(format!("at most {AV_MAX_SHEETS} drawings in one reading (got {})", paths.len()));
    }
    let Some(config_path) = input.str("config").filter(|p| !p.trim().is_empty()) else {
        return routes::bad("config is required: the set's configuration file (one key = value per line)");
    };
    if !super::cores::is_on(docs_cores::CAD) {
        return super::cores::core_off(docs_cores::CAD, &paths[0]);
    }
    let rt = match super::runtime() {
        Ok(rt) => rt,
        Err(u) => return refusal("503 Service Unavailable", "docs_unavailable", u.message()),
    };
    let ws = input.str("workspace");
    // The configuration: a workspace file the caller may read, read as it
    // is on disk (it isn't opened as a doc).
    let config = {
        let t = match target::resolve(ws.as_deref(), &config_path, caller.default_workspace()) {
            Ok(t) => t,
            Err(r) => return refusal(r.status, r.code, r.message),
        };
        let store = rt.store();
        if let Err(r) = routes::rights_or_refusal(caller, &t, ws.as_deref(), store.as_deref()) {
            return r;
        }
        drop(store);
        match std::fs::metadata(&t.abs) {
            Ok(m) if m.len() > AV_CONFIG_MAX => {
                return routes::bad(format!("{}: a configuration file is at most {AV_CONFIG_MAX} bytes", t.rel));
            }
            Ok(_) => {}
            Err(e) => return refusal("404 Not Found", "config_not_found", format!("{}: {e}", t.rel)),
        }
        match std::fs::read_to_string(&t.abs) {
            Ok(s) => s,
            Err(e) => return routes::bad(format!("{}: the configuration isn't readable text: {e}", t.rel)),
        }
    };
    let cfg = match k2_mpc_av::Config::parse(&config) {
        Ok(c) => c,
        Err(e) => return cad_refusal(&e),
    };
    let mut sheets = Vec::with_capacity(paths.len());
    let mut read = Vec::with_capacity(paths.len());
    for p in &paths {
        let (t, version, opened) = match sheet(&rt, caller, ws.as_deref(), p) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let doc = match &opened {
            Opened::Dxf(bytes) => match k2_mpc_dxf::read_doc(bytes, &k2_mpc_dxf::Limits::default()) {
                Ok(d) => d,
                Err(e) => {
                    return cad_refusal(&CadError::new(
                        k2_mpc_av::SHEET_UNREADABLE,
                        format!("{}: {}", t.rel, e.summary()),
                        "`k2 doc cad check` on that drawing names what doesn't read",
                    ))
                }
            },
            Opened::Dwg(d) => d.doc().clone(),
        };
        read.push(json!({ "path": t.rel, "version": version }));
        sheets.push(k2_mpc_av::Sheet { name: t.rel.clone(), doc });
    }
    let out = k2_mpc_av::read_set(&cfg, &sheets);
    if summary {
        // Counts and finding codes only: no name, identifier or label.
        return routes::ok(json!({ "what": "summary", "drawings": sheets.len(), "summary": k2_mpc_av::set::summary(&out.data) }));
    }
    routes::ok(json!({
        "what": "read",
        "sheets_read": read,
        "shape": format!("{}/{}", out.spec.0, out.spec.1),
        "stdout": out.stdout,
        "data": data_json(&out),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The verb is the pack's own name, one word or two; the version is
    /// K2's; a `.dwg` takes the draw verbs and the model's shared reads.
    #[test]
    fn a_read_names_one_of_the_packs_verbs() {
        let ok = |argv: &[&str], kind| split_verb(&s(argv), kind).unwrap_or_else(|r| panic!("{argv:?}: {}", r.body));
        assert_eq!(ok(&["layer", "list", "--unused"], BlobKind::Dxf), ("layer list".into(), s(&["--unused"])));
        assert_eq!(ok(&["check", "--writeback"], BlobKind::Dxf), ("check".into(), s(&["--writeback"])));
        assert_eq!(ok(&["draw", "page", "--space", "model"], BlobKind::Dwg), ("draw page".into(), s(&["--space", "model"])));
        assert_eq!(ok(&["entity", "find", "--layer", "0"], BlobKind::Dwg).0, "entity find");
        for (argv, kind) in [(&["check"][..], BlobKind::Dwg), (&["header", "get", "$ACADVER"], BlobKind::Dwg), (&["layer"], BlobKind::Dxf), (&["entity", "list"], BlobKind::Dxf)] {
            let r = split_verb(&s(argv), kind).expect_err("not a read verb");
            assert!(r.status.starts_with("400"), "{argv:?}: {}", r.body);
        }
        let r = split_verb(&s(&["draw", "header", "--version", "3"]), BlobKind::Dxf).expect_err("K2 sets the version");
        assert!(r.body.contains("K2 sets it"), "{}", r.body);
    }

    /// A pack refusal keeps its name, id and exit; the status follows it.
    #[test]
    fn a_pack_refusal_keeps_its_name_id_and_exit() {
        let stale = CadError::new(k2_mpc_cad_model::error::STALE_CURSOR, "from another version", "read the header again");
        let r = cad_refusal(&stale);
        assert_eq!(r.status, "409 Conflict");
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!((&v["error"]["code"], &v["error"]["id"], &v["error"]["exit"]), (&json!("stale_cursor"), &json!("CAD-E013"), &json!(2)), "{v}");
        assert_eq!(cad_refusal(&CadError::new(k2_mpc_cad_model::error::NOT_FOUND, "no", "")).status, "404 Not Found");
        assert_eq!(cad_refusal(&CadError::new(k2_mpc_cad_model::error::USAGE, "no", "")).status, "400 Bad Request");
        let unreadable = k2_mpc_dxf::read_doc(b"not a drawing", &k2_mpc_dxf::Limits::default()).expect_err("not DXF");
        let r = cad_refusal(&unreadable);
        assert_eq!(r.status, "422 Unprocessable Entity", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert!(v["error"]["id"].as_str().is_some_and(|id| id.starts_with("DXF-E")), "{v}");
    }

    /// The pack's sampler: the draw verbs at a version give the pack's own
    /// goldens byte for byte (the route's body), and a cursor from another
    /// version is stale.
    #[test]
    fn the_draw_verbs_answer_the_packs_goldens() {
        let b = k2_mpc_dxf::testgen::draw_sampler(k2_mpc_dxf::testgen::Ver::R2018).build();
        let o = Opened::Dxf(Arc::new(b.bytes));
        let golden = |n: &str| {
            std::fs::read_to_string(format!("{}/../../vendor/k2-mpc-cad/crates/dxf/tests/golden/draw-sampler-2018-{n}.json", env!("CARGO_MANIFEST_DIR")))
                .unwrap_or_else(|e| panic!("{n}: {e}"))
        };
        let h = run(&o, 7, "draw header", &[]).expect("header");
        assert!(format!("{}\n", h.data.to_json_pretty()) == golden("header"), "the header differs from the pack's golden");
        let p = run(&o, 7, "draw page", &s(&["--space", "model"])).expect("page");
        assert!(format!("{}\n", p.data.to_json_pretty()) == golden("page-model"), "the model page differs from the pack's golden");
        if let Some(next) = p.next {
            let at = next.iter().position(|a| a == "--cursor").expect("a cursor") + 1;
            let e = run(&o, 8, "draw page", &s(&["--space", "model", "--cursor", &next[at]])).expect_err("another version");
            assert_eq!(e.code.name, "stale_cursor");
        }
    }
}
