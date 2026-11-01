//! `POST /cli/zen/data/board` (`k2 board`, Rosson 2026-11-01): agents work
//! on the Board Garden's boards through Board's own rules, not raw
//! `k2 store` writes. One route, one `cmd` per verb; the body is
//! `{cmd, board?, …}`.
//!
//! Every call goes through the Garden data door's gates (credential kind,
//! POST-only, body cap; `zen_data_routes`), then here: the Board core
//! (`core_off` while it's off: `agent_routes::data_space`), the caller's
//! grants on `data:k2:board` re-read on every call (an agent reaches only
//! the boards it was granted: `k2 zen data grant k2:board --agent <h>
//! --level write --prefix board/<b>/`), the agent budget, then the pure
//! rules in `k2_core::board` (schema checks with typed refusals, the lock
//! rule, the delete cascade, `apply`'s id mapping) and one all-or-nothing
//! store write with each key's `ifRev`. A conflict (someone wrote a key
//! meanwhile) re-reads and runs the command again, at most three times.
//! Authors are stable keys: `agent:<handle>`, or `person:owner` for the
//! owner token.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Map, Value};

use k2_core::board::export::{envelope, BlobBytes};
use k2_core::board::ops::{self as bops, Board, Ctx, Op, Stored, Write};
use k2_core::board::{self, schema, templates, BoardError};
use k2_core::garden_data::backend::{Cursor, Op as StoreOp};
use k2_core::garden_data::grants::Access;
use k2_core::garden_data::{DataError, Space, StoreBackend, MAX_LIST_LIMIT};

use crate::garden_data::agent_routes::{access, data_space, ByNames};
use crate::garden_data::audit::{self, Counted};
use crate::garden_data::budget::{self, BudgetKey, Spend};
use crate::garden_data::{notify_changed, store};
use crate::zen_data_routes::DataWho;

/// The route.
pub const ROUTE: &str = "/cli/zen/data/board";

/// Most ops one command writes (a template is ~60; a delete cascade of a
/// big mind map can be more).
pub const MAX_OPS: usize = 2000;

/// What a command answers: JSON, or a refusal.
enum Fail {
    Board(BoardError),
    Data(DataError),
}

impl From<BoardError> for Fail {
    fn from(e: BoardError) -> Fail {
        Fail::Board(e)
    }
}
impl From<DataError> for Fail {
    fn from(e: DataError) -> Fail {
        Fail::Data(e)
    }
}

fn status_of(code: &str) -> u16 {
    match code {
        "no_object" | "no_board" | "no_template" | "no_project" | "blob_missing" => 404,
        "locked" | "exists" | "voting_off" | "no_votes_left" | "board_trashed" | "conflict" => 409,
        _ => 400,
    }
}

/// Run one `k2 board` call. Errors keep the Garden data shape
/// (`{ok: false, error: {code, hint, …}}`).
pub fn handle(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    match run(who, body) {
        Ok(v) => Ok(v),
        Err(Fail::Data(e)) => Err(e),
        Err(Fail::Board(e)) => Err(DataError::Refused { code: e.code, status: status_of(e.code), hint: e.message }),
    }
}

fn body_of(body: &[u8]) -> Result<Map<String, Value>, Fail> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(Fail::Board(BoardError::new("bad_request", "k2 board sends {cmd, board?, …}"))),
    }
}

fn s<'a>(b: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    b.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn need<'a>(b: &'a Map<String, Value>, k: &str, cmd: &str) -> Result<&'a str, Fail> {
    s(b, k).ok_or_else(|| Fail::Board(BoardError::new("bad_request", format!("{cmd} needs {k}"))))
}

fn fields_of(b: &Map<String, Value>) -> Map<String, Value> {
    b.get("fields").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// The caller as a person key.
fn person_key(who: &DataWho) -> String {
    match who.project_id() {
        Some(p) => match crate::compute_routes::ws_by_id(p) {
            Some(ws) => format!("agent:{}", ws.handle.to_lowercase()),
            None => format!("agent:{}", p.to_lowercase()),
        },
        None => board::PERSON_OWNER.to_string(),
    }
}

fn spend(who: &DataWho, space: &Space, what: Spend) -> Result<(), DataError> {
    match who.project_id() {
        Some(p) => budget::spend(&BudgetKey::Agent { project_id: p.to_string(), space: space.as_str().to_string() }, what),
        None => Ok(()),
    }
}

/// Every readable key under `prefix`.
fn read_all(space: &Space, acc: &Access, prefix: &str) -> Result<Vec<Stored>, DataError> {
    let filter = acc.read_prefixes();
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..1000 {
        let page = store().list(space, prefix, after.as_deref(), MAX_LIST_LIMIT, filter.as_deref())?;
        out.extend(page.items.into_iter().map(|e| Stored { key: e.key, rev: e.rev, value: e.value }));
        match page.next {
            Some(n) => after = Some(n),
            None => return Ok(out),
        }
    }
    Ok(out)
}

fn is_live(row: &Value) -> bool {
    row.get("trashedAt").is_none_or(Value::is_null)
}

/// The boards this caller can see: `boards/<b>` rows it can read (live
/// ones), plus boards its grants name (`board/<b>/…`) when it can't read
/// the list. `(id, name, project)`.
fn visible_boards(space: &Space, acc: &Access) -> Result<Vec<(String, String, Option<String>, bool)>, DataError> {
    let mut out: Vec<(String, String, Option<String>, bool)> = Vec::new();
    for e in read_all(space, acc, "boards/")? {
        let Some(id) = e.key.strip_prefix("boards/") else { continue };
        let name = e.value.get("name").and_then(Value::as_str).unwrap_or("Board").to_string();
        let project = e.value.get("project").and_then(Value::as_str).map(str::to_string);
        out.push((id.to_string(), name, project, is_live(&e.value)));
    }
    if let Some(prefixes) = acc.read_prefixes() {
        for p in prefixes {
            if let Some(rest) = p.strip_prefix("board/") {
                let id = rest.split('/').next().unwrap_or("");
                if board::valid_board_id(id) && !out.iter().any(|b| b.0 == id) {
                    let name = store()
                        .get(space, &board::meta_key(id))?
                        .and_then(|e| e.value.get("title").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| id.to_string());
                    out.push((id.to_string(), name, None, true));
                }
            }
        }
    }
    Ok(out)
}

/// The board a call names (`board`: an id or a name), or the only one
/// the caller can see.
fn resolve(space: &Space, acc: &Access, arg: Option<&str>) -> Result<String, Fail> {
    let boards = visible_boards(space, acc)?;
    let live: Vec<&(String, String, Option<String>, bool)> = boards.iter().filter(|b| b.3).collect();
    let listing = || live.iter().map(|b| format!("{} ({})", b.0, b.1)).collect::<Vec<_>>().join(", ");
    let found = match arg {
        Some(a) => {
            let by_id = boards.iter().find(|b| b.0 == a);
            let by_name: Vec<_> = boards.iter().filter(|b| b.1.eq_ignore_ascii_case(a)).collect();
            match (by_id, by_name.len()) {
                (Some(b), _) => b.clone(),
                (None, 1) => by_name[0].clone(),
                (None, 0) => {
                    // A board the caller's grants reach but whose list row it can't read.
                    if board::valid_board_id(a) && acc.can_read(&board::meta_key(a)) && store().get(space, &board::meta_key(a))?.is_some() {
                        (a.to_string(), a.to_string(), None, true)
                    } else {
                        return Err(Fail::Board(BoardError::new(
                            "no_board",
                            format!("no board {a} you can read (k2 board boards; boards: {})", if live.is_empty() { "none".into() } else { listing() }),
                        )));
                    }
                }
                _ => return Err(Fail::Board(BoardError::new("board_required", format!("more than one board is named {a}: use its id ({})", listing())))),
            }
        }
        None => match live.len() {
            1 => live[0].clone(),
            0 => return Err(Fail::Board(BoardError::new("no_board", "you can't read any board yet: ask your human to grant one (the board's For agents panel shows the line)"))),
            _ => return Err(Fail::Board(BoardError::new("board_required", format!("say which board: --board <id> ({})", listing())))),
        },
    };
    if !found.3 {
        return Err(Fail::Board(BoardError::new("board_trashed", format!("board {} is in the Trash: agents can't use it until the person restores it", found.0))));
    }
    if !acc.can_read(&board::meta_key(&found.0)) {
        return Err(Fail::Data(DataError::NotGranted { key: Some(board::board_prefix(&found.0)) }));
    }
    Ok(found.0)
}

fn load(space: &Space, acc: &Access, id: &str) -> Result<Board, DataError> {
    Ok(Board::from_entries(id, read_all(space, acc, &board::board_prefix(id))?))
}

/// Write `ws` as one all-or-nothing commit with each key's `ifRev`.
fn commit(who: &DataWho, space: &Space, acc: &Access, ws: &[Write]) -> Result<i64, DataError> {
    if ws.is_empty() {
        return Ok(0);
    }
    if ws.len() > MAX_OPS {
        return Err(DataError::TooLarge(format!("one k2 board command writes at most {MAX_OPS} objects")));
    }
    for w in ws {
        if !acc.can_write(&w.key) {
            return Err(DataError::NotGranted { key: Some(w.key.clone()) });
        }
    }
    let ops: Vec<StoreOp> = ws
        .iter()
        .map(|w| match &w.value {
            Some(v) => StoreOp::Put { key: w.key.clone(), value: v.clone(), if_rev: Some(w.if_rev) },
            None => StoreOp::Delete { key: w.key.clone(), if_rev: Some(w.if_rev) },
        })
        .collect();
    let bytes: usize = ops.iter().map(StoreOp::value_bytes).sum();
    spend(who, space, Spend::Commit { ops: ops.len(), bytes })?;
    let actor = who.actor().ok_or(DataError::NotGranted { key: None })?;
    let rev = store().apply(space, &ops, &actor)?;
    notify_changed(space.as_str(), rev);
    audit::count(space.as_str(), &who.audit_label(), None, Counted::Write { ops: ops.len(), bytes });
    Ok(rev)
}

/// Read the board, plan, write; a conflict runs it again on fresh data
/// (three tries).
fn write_cmd(
    who: &DataWho,
    space: &Space,
    acc: &Access,
    id: &str,
    mut f: impl FnMut(&Board, &Ctx) -> Result<(Vec<Write>, Value), Fail>,
) -> Result<Value, Fail> {
    let by = person_key(who);
    for attempt in 0.. {
        let b = load(space, acc, id)?;
        let ctx = Ctx { by: by.clone(), at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true), batch: format!("cli-{}", schema::hex(10)) };
        let (ws, mut out) = f(&b, &ctx)?;
        match commit(who, space, acc, &ws) {
            Ok(rev) => {
                if let Some(o) = out.as_object_mut() {
                    o.insert("ok".into(), json!(true));
                    o.insert("board".into(), json!(id));
                    o.insert("rev".into(), json!(rev));
                }
                return Ok(out);
            }
            Err(DataError::Conflict { .. }) if attempt < 2 => continue,
            Err(DataError::Conflict { key, .. }) => {
                return Err(Fail::Board(BoardError::new("conflict", format!("the board kept changing while this ran ({key}): try again"))))
            }
            Err(e) => return Err(Fail::Data(e)),
        }
    }
    unreachable!("the loop returns")
}

fn written(ws: &[Write], b: &Board, id: &str) -> Value {
    let key = board::obj_key(&b.id, id);
    ws.iter().find(|w| w.key == key).and_then(|w| w.value.clone()).map(|v| bops::view(&v, false)).unwrap_or(Value::Null)
}

fn run(who: &DataWho, body: &[u8]) -> Result<Value, Fail> {
    let b = body_of(body)?;
    let cmd = need(&b, "cmd", "k2 board")?.to_string();
    // Static answers: no data read.
    match cmd.as_str() {
        "fields" => return Ok(json!({ "ok": true, "types": schema::describe(s(&b, "type"))? })),
        "templates" => {
            let t: Vec<Value> = templates::TEMPLATES.iter().map(|(n, l)| json!({ "name": n, "label": l })).collect();
            return Ok(json!({ "ok": true, "templates": t }));
        }
        _ => {}
    }
    let space = data_space(board::WIDGET)?;
    let acc = access(who, &space)?;
    if who.project_id().is_some() && acc == Access::None {
        return Err(Fail::Data(DataError::NotGranted { key: None }));
    }
    match cmd.as_str() {
        "boards" => {
            spend(who, &space, Spend::Read)?;
            let all = b.get("all").and_then(Value::as_bool).unwrap_or(false);
            let rows: Vec<Value> = visible_boards(&space, &acc)?
                .into_iter()
                .filter(|r| all || r.3)
                .map(|(id, name, project, live)| json!({ "id": id, "name": name, "project": project, "trashed": !live }))
                .collect();
            Ok(json!({ "ok": true, "boards": rows }))
        }
        "board-new" => board_new(who, &space, &acc, &b),
        "board-rename" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let name = need(&b, "name", "board-rename")?.to_string();
            check_name(&name)?;
            let row = store().get(&space, &board::row_key(&id))?;
            let (rev, mut v) = match row {
                Some(e) => (e.rev, e.value.as_object().cloned().unwrap_or_default()),
                None => return Err(Fail::Board(BoardError::new("no_board", format!("board {id} has no list row you can read")))),
            };
            v.insert("name".into(), json!(name));
            let id2 = id.clone();
            let name2 = name.clone();
            write_cmd(who, &space, &acc, &id, move |bd, ctx| {
                let mut ws = bops::plan(bd, &[Op::Meta(Map::from_iter([("title".to_string(), json!(name2))]))], None, ctx)?;
                ws.push(Write { key: board::row_key(&id2), value: Some(Value::Object(v.clone())), if_rev: rev });
                Ok((ws, json!({ "name": name2 })))
            })
        }
        "list" | "read" | "search" | "meta-get" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            spend(who, &space, Spend::Read)?;
            let bd = load(&space, &acc, &id)?;
            audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
            let full = b.get("full").and_then(Value::as_bool).unwrap_or(false);
            match cmd.as_str() {
                "read" => {
                    let oid = need(&b, "id", "read")?;
                    let o = bd.objects.get(oid).ok_or_else(|| BoardError::new("no_object", format!("no object {oid} on board {id}")))?;
                    Ok(json!({ "ok": true, "board": id, "object": bops::view(&o.value, full) }))
                }
                "search" => {
                    let q = need(&b, "text", "search")?.to_lowercase();
                    let hits: Vec<Value> = bd
                        .objects
                        .values()
                        .filter(|o| bops::text_of(&o.value).to_lowercase().contains(&q))
                        .map(|o| bops::view(&o.value, false))
                        .collect();
                    Ok(json!({ "ok": true, "board": id, "objects": hits }))
                }
                "meta-get" => Ok(json!({ "ok": true, "board": id, "meta": bd.meta_value() })),
                _ => {
                    let ty = s(&b, "type");
                    let objs: Vec<Value> = k2_core::board::export::ordered(&bd)
                        .into_iter()
                        .filter(|o| ty.is_none_or(|t| o.get("type").and_then(Value::as_str) == Some(t)))
                        .map(|o| bops::view(&o, full))
                        .collect();
                    Ok(json!({ "ok": true, "board": id, "title": bd.meta_value().get("title").cloned().unwrap_or(Value::Null), "count": objs.len(), "objects": objs }))
                }
            }
        }
        "log" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            spend(who, &space, Spend::Read)?;
            let limit = b.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 1000) as usize;
            Ok(json!({ "ok": true, "board": id, "log": log(&space, &acc, &id, limit)? }))
        }
        "add" | "frame" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let ty = if cmd == "frame" { "frame".to_string() } else { s(&b, "type").unwrap_or("sticky").to_string() };
            let mut obj = fields_of(&b);
            obj.remove("id");
            obj.insert("type".into(), json!(ty));
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                let mut o = obj.clone();
                o.insert("id".into(), json!(schema::new_id(&ty)));
                let new_id = o["id"].as_str().unwrap_or_default().to_string();
                let ws = bops::plan(bd, &[Op::Add(o)], None, ctx)?;
                let out = written(&ws, bd, &new_id);
                Ok((ws, json!({ "object": out })))
            })
        }
        "connect" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let from = need(&b, "from", "connect")?.to_string();
            let to = need(&b, "to", "connect")?.to_string();
            let mut obj = fields_of(&b);
            let fa = obj.remove("fromAnchor").unwrap_or(json!("auto"));
            let ta = obj.remove("toAnchor").unwrap_or(json!("auto"));
            obj.insert("type".into(), json!("connector"));
            obj.insert("from".into(), json!({ "id": from, "anchor": fa }));
            obj.insert("to".into(), json!({ "id": to, "anchor": ta }));
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                for e in [&from, &to] {
                    if bd.object(e).is_none() {
                        return Err(Fail::Board(BoardError::new("no_object", format!("no object {e} on this board"))));
                    }
                }
                let mut o = obj.clone();
                let new_id = schema::new_id("connector");
                o.insert("id".into(), json!(new_id));
                let ws = bops::plan(bd, &[Op::Add(o)], None, ctx)?;
                let out = written(&ws, bd, &new_id);
                Ok((ws, json!({ "object": out })))
            })
        }
        "update" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let oid = need(&b, "id", "update")?.to_string();
            let f = fields_of(&b);
            if f.is_empty() {
                return Err(Fail::Board(BoardError::new("bad_request", "update needs at least one --field value")));
            }
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                let ws = bops::plan(bd, &[Op::Set { id: oid.clone(), f: f.clone() }], None, ctx)?;
                let out = written(&ws, bd, &oid);
                Ok((ws, json!({ "object": out })))
            })
        }
        "delete" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let oid = need(&b, "id", "delete")?.to_string();
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                if bd.object(&oid).is_none() {
                    return Err(Fail::Board(BoardError::new("no_object", format!("no object {oid} on this board"))));
                }
                let gone = bops::cascade(bd, &oid);
                let named: HashSet<String> = HashSet::from([oid.clone()]);
                let ops: Vec<Op> = gone.iter().map(|g| Op::Del { id: g.clone() }).collect();
                let ws = bops::plan(bd, &ops, Some(&named), ctx)?;
                Ok((ws, json!({ "deleted": gone })))
            })
        }
        "template" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let name = need(&b, "name", "template")?.to_string();
            let x = b.get("x").and_then(num).unwrap_or(0.0);
            let y = b.get("y").and_then(num).unwrap_or(0.0);
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                let objs = templates::build(&name, x, y).ok_or_else(|| {
                    BoardError::new(
                        "no_template",
                        format!("no template {name} (one of {})", templates::TEMPLATES.iter().map(|t| t.0).collect::<Vec<_>>().join(", ")),
                    )
                })?;
                let ids: Vec<Value> = objs.iter().map(|o| o["id"].clone()).collect();
                let ops: Vec<Op> = objs.into_iter().filter_map(|o| o.as_object().cloned()).map(Op::Add).collect();
                let ws = bops::plan(bd, &ops, None, ctx)?;
                Ok((ws, json!({ "template": name, "ids": ids })))
            })
        }
        "react" | "vote" | "comment" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let oid = need(&b, "id", &cmd)?.to_string();
            let me = person_key(who);
            let emoji = s(&b, "emoji").map(str::to_string);
            let text = s(&b, "text").map(str::to_string);
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                let o = bd.object(&oid).ok_or_else(|| BoardError::new("no_object", format!("no object {oid} on this board")))?.clone();
                let (op, new_id) = match cmd.as_str() {
                    "react" => {
                        let e = emoji.clone().ok_or_else(|| BoardError::new("bad_request", "react needs an emoji"))?;
                        (Op::Set { id: oid.clone(), f: Map::from_iter([("reactions".to_string(), bops::toggle_reaction(&o, &me, &e)?)]) }, oid.clone())
                    }
                    "vote" => (Op::Set { id: oid.clone(), f: Map::from_iter([("votes".to_string(), bops::add_vote(bd, &oid, &me)?)]) }, oid.clone()),
                    _ => {
                        let t = text.clone().ok_or_else(|| BoardError::new("bad_request", "comment needs text"))?;
                        let x = o.get("x").and_then(Value::as_f64).unwrap_or(0.0) + o.get("w").and_then(Value::as_f64).unwrap_or(0.0) - 12.0;
                        let y = o.get("y").and_then(Value::as_f64).unwrap_or(0.0) - 20.0;
                        let nid = schema::new_id("comment");
                        (Op::Add(Map::from_iter([
                            ("id".to_string(), json!(nid)),
                            ("type".to_string(), json!("comment")),
                            ("x".to_string(), json!(x)),
                            ("y".to_string(), json!(y)),
                            ("text".to_string(), json!(t)),
                            ("target".to_string(), json!(oid)),
                        ])), nid)
                    }
                };
                let ws = bops::plan(bd, &[op], None, ctx)?;
                let out = written(&ws, bd, &new_id);
                Ok((ws, json!({ "object": out })))
            })
        }
        "meta" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let f = fields_of(&b);
            if f.is_empty() {
                spend(who, &space, Spend::Read)?;
                let bd = load(&space, &acc, &id)?;
                return Ok(json!({ "ok": true, "board": id, "meta": bd.meta_value() }));
            }
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                let ws = bops::plan(bd, &[Op::Meta(f.clone())], None, ctx)?;
                let meta = ws.iter().find(|w| w.key == board::meta_key(&bd.id)).and_then(|w| w.value.clone()).unwrap_or(Value::Null);
                Ok((ws, json!({ "meta": meta })))
            })
        }
        "apply" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            let raw = b.get("ops").cloned().ok_or_else(|| BoardError::new("bad_ops", "apply needs {\"ops\":[...]}"))?;
            let parsed = bops::parse_ops(&raw)?;
            if parsed.len() > MAX_OPS {
                return Err(Fail::Board(BoardError::too_many("ops", MAX_OPS)));
            }
            write_cmd(who, &space, &acc, &id, |bd, ctx| {
                let mut ops = parsed.clone();
                let map = bops::map_ids(&mut ops);
                let ws = bops::plan(bd, &ops, None, ctx)?;
                Ok((ws, json!({ "ids": map, "count": ops.len() })))
            })
        }
        "export" => {
            let id = resolve(&space, &acc, s(&b, "board"))?;
            spend(who, &space, Spend::Read)?;
            let bd = load(&space, &acc, &id)?;
            let title = bd
                .meta_value()
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| store().get(&space, &board::row_key(&id)).ok().flatten().and_then(|e| e.value.get("name").and_then(Value::as_str).map(str::to_string)))
                .unwrap_or_else(|| "Board".to_string());
            let names = |k: &str| -> (String, &'static str) {
                if k == board::PERSON_OWNER {
                    let people = k2_core::notify_prefs::People::load().unwrap_or_default();
                    (k2_core::person_name::effective(k2_core::notify_prefs::OWNER, &people).display_name, "person")
                } else if let Some(h) = k.strip_prefix("agent:") {
                    (h.to_string(), "agent")
                } else {
                    (k.to_string(), "person")
                }
            };
            let mut fetch = |hex: &str| -> Result<BlobBytes, BoardError> {
                let (row, bytes, _) = store()
                    .blob_read(&space, hex, 0, k2_core::garden_data::blobs::MAX_BLOB_BYTES)
                    .map_err(|e| BoardError::new("blob_missing", format!("image sha256-{hex}: {}", e.code())))?;
                Ok(BlobBytes { mime: row.mime, name: row.name, bytes })
            };
            let env = envelope(&bd, &title, &names, &mut fetch)?;
            audit::count(space.as_str(), &who.audit_label(), None, Counted::Read);
            Ok(json!({ "ok": true, "board": id, "export": env }))
        }
        other => Err(Fail::Board(BoardError::new("bad_request", format!("unknown k2 board command {other} (k2 board --help)")))),
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn check_name(name: &str) -> Result<(), Fail> {
    if name.chars().count() > 200 {
        return Err(Fail::Board(BoardError::too_long("name", 200)));
    }
    Ok(())
}

/// `board-new {name, project?}`: the list row and the board's meta, both
/// new keys (`ifRev 0`).
fn board_new(who: &DataWho, space: &Space, acc: &Access, b: &Map<String, Value>) -> Result<Value, Fail> {
    let name = need(b, "name", "board-new")?.to_string();
    check_name(&name)?;
    let project = match s(b, "project") {
        None | Some("none") => None,
        Some(p) => {
            let rows = read_all(space, acc, "projects/")?;
            let hit = rows.iter().find(|e| {
                let v = &e.value;
                is_live(v) && (e.key == format!("projects/{}", board::seg(p)) || v.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(p)))
            });
            match hit {
                Some(e) => Some(e.key.trim_start_matches("projects/").to_string()),
                None => return Err(Fail::Board(BoardError::new("no_project", format!("no Board project {p} you can read")))),
            }
        }
    };
    let by = person_key(who);
    for attempt in 0..3 {
        let id = board::new_board_id();
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let ws = vec![
            Write {
                key: board::row_key(&id),
                value: Some(json!({ "id": id, "name": name, "project": project, "format": board::FORMAT, "createdBy": by, "createdAt": at })),
                if_rev: 0,
            },
            Write { key: board::meta_key(&id), value: Some(json!({ "format": board::FORMAT, "title": name })), if_rev: 0 },
        ];
        match commit(who, space, acc, &ws) {
            Ok(rev) => return Ok(json!({ "ok": true, "board": id, "name": name, "project": project, "rev": rev })),
            Err(DataError::Conflict { .. }) if attempt < 2 => continue,
            Err(e) => return Err(Fail::Data(e)),
        }
    }
    unreachable!("the loop returns")
}

/// Who changed what on a board, newest last: one row per rev (a command,
/// or one save of the widget's), from the store's change feed.
fn log(space: &Space, acc: &Access, id: &str, limit: usize) -> Result<Vec<Value>, DataError> {
    let prefix = board::board_prefix(id);
    let filter = acc.read_prefixes();
    let mut names = ByNames::new();
    let mut rows: BTreeMap<i64, (i64, String, Vec<String>, usize)> = BTreeMap::new();
    let mut from = Cursor { since: 0, after: None };
    for _ in 0..200 {
        let page = store().changes(space, &from, &prefix, filter.as_deref())?;
        for c in page.items {
            let e = rows.entry(c.rev).or_insert_with(|| (c.at, names.wire(&c.by), Vec::new(), 0));
            let what = c.key.rsplit('/').next().unwrap_or(&c.key).to_string();
            if c.deleted {
                e.3 += 1;
            }
            if e.2.len() < 20 {
                e.2.push(what);
            }
        }
        match page.next {
            Some(n) => from = n,
            None => break,
        }
    }
    let all: Vec<Value> = rows
        .into_iter()
        .map(|(rev, (at, by, ids, deleted))| json!({ "rev": rev, "at": at, "by": by, "changed": ids, "deleted": deleted }))
        .collect();
    let skip = all.len().saturating_sub(limit);
    Ok(all.into_iter().skip(skip).collect())
}

#[cfg(test)]
#[path = "board_routes_tests.rs"]
mod tests;
