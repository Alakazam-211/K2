//! One request → the statements to run and how to read their answer.

use serde_json::{Map, Value};

use crate::build::{cond_sql, limit_sql, order_sql, rpc_args_sql, wrap_rows, Builder, Rel};
use crate::error::ApiError;
use crate::parse::{parse_query, Query, SelectItem};
use crate::schema::{Function, SchemaCache, Table};
use crate::sql::{lit, qi};

/// One request, as the door read it.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// Upper case.
    pub method: String,
    /// After `/rest/v1`, percent-decoded: `/notes`, `/rpc/fn`.
    pub path: String,
    /// Decoded query pairs.
    pub query: Vec<(String, String)>,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn headers_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers.iter().filter(move |(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// The App's settings the grammar needs.
#[derive(Debug, Clone)]
pub struct Options {
    /// Exposed schemas; the first is the default.
    pub schemas: Vec<String>,
    /// Rows in one answer (`db-max-rows`); `None`: no cap.
    pub max_rows: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Exact,
    Planned,
    Estimated,
}

/// What the statements are for, so [`crate::respond`] can shape the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Read,
    /// A function returning one row (an object, or null).
    RpcObject,
    /// A function returning a scalar (body is its JSON value) or a set of
    /// them (an array).
    RpcScalar,
    RpcVoid,
    /// `upsert` (merge-duplicates): the status says whether any row was new (201) or every one
    /// merged into an existing row (200), as PostgREST answers.
    Insert { representation: bool, upsert: bool },
    Update { representation: bool },
    Delete { representation: bool },
}

/// A statement and whether it answers rows (else the affected count is
/// what matters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Rows(String),
    Exec(String),
}

impl Stmt {
    pub fn sql(&self) -> &str {
        match self {
            Stmt::Rows(s) | Stmt::Exec(s) => s,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub stmts: Vec<Stmt>,
    pub kind: Kind,
    /// Run the transaction READ ONLY (GET and HEAD, as PostgREST does).
    pub read_only: bool,
    /// The schema the request is about (first on the search path).
    pub schema: String,
    pub head: bool,
    /// `Accept: application/vnd.pgrst.object+json`.
    pub singular: bool,
    pub count: Option<Count>,
    /// `stmts[1]` is the planner's estimate (`EXPLAIN (FORMAT JSON)`).
    pub planned_stmt: bool,
    pub offset: u64,
    pub max_rows: Option<u64>,
    /// `Preference-Applied`.
    pub applied: Vec<String>,
}

#[derive(Debug, Default)]
struct Prefer {
    representation: bool,
    minimal: bool,
    count: Option<Count>,
    merge: bool,
    ignore: bool,
    missing_default: bool,
}

fn prefer(req: &Request) -> Result<(Prefer, Vec<String>), ApiError> {
    let mut p = Prefer::default();
    let mut applied = Vec::new();
    for h in req.headers_all("prefer") {
        for item in h.split(',') {
            let item = item.trim();
            match item {
                "return=representation" => {
                    p.representation = true;
                    applied.push(item.to_string());
                }
                "return=minimal" => {
                    p.minimal = true;
                    applied.push(item.to_string());
                }
                "return=headers-only" => return Err(ApiError::unsupported("Prefer: return=headers-only")),
                "count=exact" => p.count = Some(Count::Exact),
                "count=planned" => p.count = Some(Count::Planned),
                "count=estimated" => p.count = Some(Count::Estimated),
                "resolution=merge-duplicates" => {
                    p.merge = true;
                    applied.push(item.to_string());
                }
                "resolution=ignore-duplicates" => {
                    p.ignore = true;
                    applied.push(item.to_string());
                }
                "missing=default" => {
                    p.missing_default = true;
                    applied.push(item.to_string());
                }
                "params=single-object" => return Err(ApiError::unsupported("Prefer: params=single-object")),
                "tx=rollback" => return Err(ApiError::unsupported("Prefer: tx=rollback")),
                _ => {}
            }
        }
    }
    if let Some(c) = p.count {
        applied.push(
            match c {
                Count::Exact => "count=exact",
                Count::Planned => "count=planned",
                Count::Estimated => "count=estimated",
            }
            .to_string(),
        );
    }
    Ok((p, applied))
}

fn singular(req: &Request) -> Result<bool, ApiError> {
    let Some(a) = req.header("accept") else { return Ok(false) };
    let a = a.to_ascii_lowercase();
    if a.contains("application/vnd.pgrst.object") {
        return Ok(true);
    }
    if a.contains("text/csv") {
        return Err(ApiError::unsupported("CSV answers (csv())"));
    }
    if a.contains("vnd.pgrst.plan") {
        return Err(ApiError::unsupported("explain()"));
    }
    if a.contains("geo+json") {
        return Err(ApiError::unsupported("GeoJSON answers"));
    }
    Ok(false)
}

fn pick_schema(req: &Request, opts: &Options) -> Result<String, ApiError> {
    let read = matches!(req.method.as_str(), "GET" | "HEAD");
    let want = if read { req.header("accept-profile") } else { req.header("content-profile") };
    match want {
        None => Ok(opts.schemas.first().cloned().unwrap_or_else(|| "public".into())),
        Some(s) if opts.schemas.iter().any(|x| x == s) => Ok(s.to_string()),
        Some(_) => Err(ApiError::new(406, "PGRST106", format!("The schema must be one of the following: {}", opts.schemas.join(", ")))),
    }
}

/// `Range: a-b` (or `a-`) → (offset, limit).
fn range(req: &Request) -> Result<(Option<u64>, Option<u64>), ApiError> {
    let Some(r) = req.header("range") else { return Ok((None, None)) };
    let r = r.trim().strip_prefix("items=").unwrap_or(r.trim());
    let bad = || ApiError::new(416, "PGRST103", "Requested range not satisfiable").details(format!("bad Range header: {r}"));
    let (a, b) = r.split_once('-').ok_or_else(bad)?;
    let a: u64 = a.trim().parse().map_err(|_| bad())?;
    if b.trim().is_empty() {
        return Ok((Some(a), None));
    }
    let b: u64 = b.trim().parse().map_err(|_| bad())?;
    if b < a {
        return Err(bad());
    }
    Ok((Some(a), Some(b - a + 1)))
}

fn json_body(req: &Request) -> Result<Value, ApiError> {
    if let Some(ct) = req.header("content-type") {
        let ct = ct.to_ascii_lowercase();
        if !(ct.starts_with("application/json") || ct.starts_with("application/vnd.pgrst")) {
            return Err(ApiError::unsupported(&format!("a {ct} request body (send application/json)")));
        }
    }
    if req.body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(&req.body).map_err(|e| ApiError::new(400, "PGRST102", "Empty or invalid json").details(e.to_string()))
}

/// A table the cache doesn't have: PostgREST 12's answer (Postgres's own
/// `42P01`), which is what Supabase's PostgREST gives today.
fn table_missing(schema: &str, name: &str) -> ApiError {
    ApiError::new(404, "42P01", format!("relation \"{schema}.{name}\" does not exist"))
}

/// Plan one `/rest/v1` request.
pub fn plan(req: &Request, cache: &SchemaCache, opts: &Options) -> Result<Plan, ApiError> {
    let schema = pick_schema(req, opts)?;
    let (pref, applied) = prefer(req)?;
    let singular = singular(req)?;
    let mut q = parse_query(&req.query)?;
    let path = req.path.trim_start_matches('/');
    if path.is_empty() {
        return Err(ApiError::unsupported("the OpenAPI root (GET /rest/v1/)"));
    }
    let base = Plan {
        stmts: Vec::new(),
        kind: Kind::Read,
        read_only: matches!(req.method.as_str(), "GET" | "HEAD"),
        schema: schema.clone(),
        head: req.method == "HEAD",
        singular,
        count: pref.count,
        planned_stmt: false,
        offset: 0,
        max_rows: opts.max_rows,
        applied,
    };
    if let Some(name) = path.strip_prefix("rpc/") {
        if name.is_empty() || name.contains('/') {
            return Err(ApiError::new(404, "PGRST202", format!("Could not find the function {schema}.{name} in the schema cache")));
        }
        return plan_rpc(req, cache, &schema, name, q, base);
    }
    if path.contains('/') {
        return Err(table_missing(&schema, path));
    }
    let table = cache.table(&schema, path).ok_or_else(|| table_missing(&schema, path))?;
    q.take_filters()?;
    match req.method.as_str() {
        "GET" | "HEAD" => plan_read(req, cache, &schema, table, &q, base),
        "POST" => plan_insert(req, cache, &schema, table, &q, &pref, base),
        "PATCH" => plan_update(req, cache, &schema, table, &q, &pref, base),
        "DELETE" => plan_delete(cache, &schema, table, &q, &pref, base),
        "PUT" => Err(ApiError::unsupported("PUT (single-row upsert by its key); use POST with resolution=merge-duplicates")),
        m => Err(ApiError::new(405, "PGRST117", format!("Unsupported HTTP method: {m}"))),
    }
}

fn select_of(q: &Query) -> Vec<SelectItem> {
    q.select.clone().unwrap_or_else(|| vec![SelectItem::Star])
}

fn total_sql(count: Option<Count>, from_where: &str, max_rows: Option<u64>) -> String {
    match count {
        Some(Count::Exact) => format!("(SELECT count(*) FROM {from_where})::text"),
        // Exact up to max_rows + 1; past that the planner's estimate
        // (stmts[1]) stands in.
        Some(Count::Estimated) => match max_rows {
            Some(m) => format!("(SELECT count(*) FROM (SELECT 1 FROM {from_where} LIMIT {}) _c)::text", m + 1),
            None => format!("(SELECT count(*) FROM {from_where})::text"),
        },
        _ => "NULL::text".into(),
    }
}

fn plan_read(req: &Request, cache: &SchemaCache, schema: &str, table: &Table, q: &Query, mut p: Plan) -> Result<Plan, ApiError> {
    let items = select_of(q);
    let mut b = Builder::new(cache, schema, q);
    let root = Rel { table: Some(table), rref: table.qualified() };
    let cols = b.select_list(&root, &items, &[])?;
    let conds = b.where_conds(&root, &items, &[])?;
    let wh = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    let order = q.order_at(&[]).map(|o| format!(" ORDER BY {}", order_sql(&root.rref, o))).unwrap_or_default();
    let (r_off, r_lim) = range(req)?;
    let offset = q.offset_at(&[]).or(r_off);
    let mut limit = q.limit_at(&[]).or(r_lim);
    if let Some(m) = p.max_rows {
        limit = Some(limit.map_or(m, |l| l.min(m)));
    }
    p.offset = offset.unwrap_or(0);
    let from_where = format!("{}{wh}", table.qualified());
    let cte = format!("SELECT {} FROM {from_where}{order}{}", cols.join(", "), limit_sql(limit, offset));
    p.stmts.push(Stmt::Rows(wrap_rows(&cte, &total_sql(p.count, &from_where, p.max_rows))));
    if matches!(p.count, Some(Count::Planned) | Some(Count::Estimated)) {
        p.stmts.push(Stmt::Rows(format!("EXPLAIN (FORMAT JSON) SELECT 1 FROM {from_where}")));
        p.planned_stmt = true;
    }
    p.kind = Kind::Read;
    Ok(p)
}

fn column_missing(table: &Table, col: &str) -> ApiError {
    ApiError::new(400, "PGRST204", format!("Could not find the '{col}' column of '{}' in the schema cache", table.name))
}

/// The representation wrapper for a write: `RETURNING` into the shaped CTE.
fn returning(cache: &SchemaCache, schema: &str, table: &Table, q: &Query, write: &str) -> Result<String, ApiError> {
    let items = select_of(q);
    let mut b = Builder::new(cache, schema, q);
    let src = Rel { table: Some(table), rref: "\"pgrst_written\"".into() };
    let cols = b.select_list(&src, &items, &[])?;
    let shaped = format!("SELECT {} FROM pgrst_written", cols.join(", "));
    let t = table.qualified();
    // Each written row, and whether it was new (`xmax = 0`: an upsert that
    // merged into an existing row has the updating transaction's xmax).
    let cols = table.columns.iter().map(|c| qi(&c.name)).collect::<Vec<_>>().join(", ");
    Ok(format!(
        "WITH pgrst_ins AS ({write} RETURNING {t}.*, ({t}.xmax = 0) AS pgrst_new), pgrst_written AS (SELECT {cols} FROM pgrst_ins), pgrst_source AS ({shaped}) \
         SELECT (SELECT count(*) FROM pgrst_written)::text AS total, (SELECT count(*) FROM pgrst_source)::text AS page_total, \
         coalesce((SELECT json_agg(_postgrest_t) FROM pgrst_source AS _postgrest_t), '[]'::json)::text AS body, \
         (SELECT count(*) FROM pgrst_ins WHERE pgrst_new)::text AS inserted"
    ))
}

fn plan_insert(req: &Request, cache: &SchemaCache, schema: &str, table: &Table, q: &Query, pref: &Prefer, mut p: Plan) -> Result<Plan, ApiError> {
    let body = json_body(req)?;
    let rows: Vec<Map<String, Value>> = match body {
        Value::Object(o) => vec![o],
        Value::Array(a) => a
            .into_iter()
            .map(|v| match v {
                Value::Object(o) => Ok(o),
                _ => Err(ApiError::new(400, "PGRST102", "All object keys must match").details("every element of the array must be an object")),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(ApiError::new(400, "PGRST102", "Empty or invalid json").details("send an object or an array of objects")),
    };
    let cols: Vec<String> = match &q.columns {
        Some(c) => c.clone(),
        None => {
            let first: Vec<String> = rows.first().map(|r| r.keys().cloned().collect()).unwrap_or_default();
            for r in &rows {
                if r.len() != first.len() || !first.iter().all(|k| r.contains_key(k)) {
                    return Err(ApiError::new(400, "PGRST102", "All object keys must match"));
                }
            }
            first
        }
    };
    for c in &cols {
        if table.column(c).is_none() {
            return Err(column_missing(table, c));
        }
    }
    let t = table.qualified();
    let arr = Value::Array(rows.into_iter().map(Value::Object).collect()).to_string();
    crate::sql::check_value(&arr)?;
    let qcols = cols.iter().map(|c| qi(c)).collect::<Vec<_>>().join(", ");
    let mut write = if cols.is_empty() {
        // `{}`: one row of defaults (several `{}` are refused).
        if arr != "[{}]" {
            return Err(ApiError::unsupported("inserting several rows with no columns"));
        }
        format!("INSERT INTO {t} DEFAULT VALUES")
    } else if pref.missing_default {
        let vals = cols
            .iter()
            .map(|c| {
                let dflt = table.column(c).and_then(|x| x.default.clone()).unwrap_or_else(|| "NULL".into());
                format!("CASE WHEN _j.obj::jsonb ? {} THEN (json_populate_record(NULL::{t}, _j.obj)).{} ELSE {dflt} END", lit(c), qi(c))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("INSERT INTO {t} ({qcols}) SELECT {vals} FROM json_array_elements({}::json) WITH ORDINALITY AS _j(obj, ord) ORDER BY _j.ord", lit(&arr))
    } else {
        format!("INSERT INTO {t} ({qcols}) SELECT {qcols} FROM json_populate_recordset(NULL::{t}, {}::json) AS _", lit(&arr))
    };
    if pref.merge || pref.ignore {
        let target = q.on_conflict.clone().unwrap_or_else(|| table.pk.clone());
        if target.is_empty() {
            return Err(ApiError::new(400, "42P10", "there is no unique or exclusion constraint matching the ON CONFLICT specification")
                .hint("name the key with on_conflict=<columns>"));
        }
        let tgt = target.iter().map(|c| qi(c)).collect::<Vec<_>>().join(", ");
        if pref.ignore || cols.is_empty() {
            write.push_str(&format!(" ON CONFLICT ({tgt}) DO NOTHING"));
        } else {
            let set = cols.iter().map(|c| format!("{} = EXCLUDED.{}", qi(c), qi(c))).collect::<Vec<_>>().join(", ");
            write.push_str(&format!(" ON CONFLICT ({tgt}) DO UPDATE SET {set}"));
        }
    }
    let representation = pref.representation;
    p.stmts.push(if representation { Stmt::Rows(returning(cache, schema, table, q, &write)?) } else { Stmt::Exec(write) });
    p.kind = Kind::Insert { representation, upsert: pref.merge };
    Ok(p)
}

fn root_where(cache: &SchemaCache, schema: &str, table: &Table, q: &Query, verb: &str) -> Result<String, ApiError> {
    if q.limit_at(&[]).is_some() || q.offset_at(&[]).is_some() || q.order_at(&[]).is_some() {
        return Err(ApiError::unsupported(&format!("limit, offset or order on {verb}")));
    }
    let _ = (cache, schema);
    let rref = table.qualified();
    let conds = q.filters_at(&[]).map(|c| cond_sql(&rref, c)).collect::<Result<Vec<_>, _>>()?;
    if conds.is_empty() {
        return Err(ApiError::new(400, "21000", format!("{verb} requires a WHERE clause")));
    }
    Ok(conds.join(" AND "))
}

fn plan_update(req: &Request, cache: &SchemaCache, schema: &str, table: &Table, q: &Query, pref: &Prefer, mut p: Plan) -> Result<Plan, ApiError> {
    let body = match json_body(req)? {
        Value::Object(o) => o,
        _ => return Err(ApiError::new(400, "PGRST102", "Empty or invalid json").details("PATCH takes one object")),
    };
    let wh = root_where(cache, schema, table, q, "UPDATE")?;
    for c in body.keys() {
        if table.column(c).is_none() {
            return Err(column_missing(table, c));
        }
    }
    let t = table.qualified();
    let write = if body.is_empty() {
        // Nothing to set: PostgREST answers the empty set.
        format!("UPDATE {t} SET {} = {t}.{} WHERE false", qi(&table.columns[0].name), qi(&table.columns[0].name))
    } else {
        let obj = Value::Object(body.clone()).to_string();
        crate::sql::check_value(&obj)?;
        let set = body.keys().map(|c| format!("{} = _b.{}", qi(c), qi(c))).collect::<Vec<_>>().join(", ");
        format!("UPDATE {t} SET {set} FROM (SELECT * FROM json_populate_record(NULL::{t}, {}::json)) AS _b WHERE {wh}", lit(&obj))
    };
    let representation = pref.representation;
    p.stmts.push(if representation { Stmt::Rows(returning(cache, schema, table, q, &write)?) } else { Stmt::Exec(write) });
    p.kind = Kind::Update { representation };
    Ok(p)
}

fn plan_delete(cache: &SchemaCache, schema: &str, table: &Table, q: &Query, pref: &Prefer, mut p: Plan) -> Result<Plan, ApiError> {
    let wh = root_where(cache, schema, table, q, "DELETE")?;
    let write = format!("DELETE FROM {} WHERE {wh}", table.qualified());
    let representation = pref.representation;
    p.stmts.push(if representation { Stmt::Rows(returning(cache, schema, table, q, &write)?) } else { Stmt::Exec(write) });
    p.kind = Kind::Delete { representation };
    Ok(p)
}

// ── rpc ──────────────────────────────────────────────────────────────

fn plan_rpc(req: &Request, cache: &SchemaCache, schema: &str, name: &str, mut q: Query, mut p: Plan) -> Result<Plan, ApiError> {
    let get = matches!(req.method.as_str(), "GET" | "HEAD");
    if !get && req.method != "POST" {
        return Err(ApiError::new(405, "PGRST101", "Only GET, HEAD and POST are allowed for functions"));
    }
    let cands: Vec<&Function> = cache.functions_named(schema, name).collect();
    // The arguments: the body's keys (POST), else the root query pairs
    // that name an argument of some overload (GET).
    let mut args = Map::new();
    let mut whole_body: Option<Value> = None;
    if get {
        let arg_names: Vec<&str> = cands.iter().flat_map(|f| f.inputs().map(|a| a.name.as_str())).collect();
        let raw = std::mem::take(&mut q.raw);
        for (path, k, v) in raw {
            if path.is_empty() && arg_names.contains(&k.as_str()) {
                args.insert(k, Value::String(v));
            } else {
                q.raw.push((path, k, v));
            }
        }
    } else {
        match json_body(req)? {
            Value::Object(o) => {
                // One unnamed json/jsonb argument takes the whole body.
                if cands.iter().any(|f| f.inputs().count() == 1 && f.inputs().all(|a| a.name.is_empty() && matches!(a.ty.as_str(), "json" | "jsonb"))) {
                    whole_body = Some(Value::Object(o));
                } else {
                    args = o;
                }
            }
            other => whole_body = Some(other),
        }
    }
    q.take_filters()?;
    let keys: Vec<String> = args.keys().cloned().collect();
    let fits: Vec<&Function> = cands
        .iter()
        .copied()
        .filter(|f| {
            if whole_body.is_some() {
                return f.inputs().count() == 1 && f.inputs().all(|a| a.name.is_empty() && matches!(a.ty.as_str(), "json" | "jsonb"));
            }
            let names: Vec<&str> = f.inputs().map(|a| a.name.as_str()).collect();
            keys.iter().all(|k| names.contains(&k.as_str())) && f.inputs().all(|a| a.has_default || keys.contains(&a.name))
        })
        .collect();
    let f = match fits.len() {
        1 => fits[0],
        0 => {
            let mut sorted = keys.clone();
            sorted.sort();
            return Err(ApiError::new(404, "PGRST202", format!("Could not find the function {schema}.{name}({}) in the schema cache", sorted.join(", ")))
                .hint("Perhaps the arguments are named differently, or the schema cache is stale"));
        }
        _ => {
            return Err(ApiError::new(300, "PGRST203", format!("Could not choose the best candidate function between: {}", fits.iter().map(|f| format!("{}.{}", f.schema, f.name)).collect::<Vec<_>>().join(", "))))
        }
    };
    let fname = format!("{}.{}", qi(&f.schema), qi(&f.name));
    let (args_cte, call_args, from_args) = match &whole_body {
        Some(b) => {
            let s = b.to_string();
            crate::sql::check_value(&s)?;
            (String::new(), format!("{}::json", lit(&s)), String::new())
        }
        None => {
            let (cte, call) = rpc_args_sql(f, &args)?;
            let from = if cte.is_empty() { String::new() } else { " FROM pgrst_args".to_string() };
            (cte, call, from)
        }
    };
    let call = format!("{fname}({call_args})");
    if f.returns_void() {
        p.stmts.push(Stmt::Rows(format!("WITH {args_cte}pgrst_call AS MATERIALIZED (SELECT {call}{from_args}) SELECT NULL::text, '0', 'null' FROM pgrst_call")));
        p.kind = Kind::RpcVoid;
        return Ok(p);
    }
    if !f.returns_rows() {
        let body = if f.returns_set { "coalesce((SELECT json_agg(pgrst_scalar) FROM pgrst_call), '[]'::json)::text" } else { "(SELECT to_json(pgrst_scalar) FROM pgrst_call)::text" };
        p.stmts.push(Stmt::Rows(format!(
            "WITH {args_cte}pgrst_call AS MATERIALIZED (SELECT {call} AS pgrst_scalar{from_args}) SELECT NULL::text AS total, (SELECT count(*) FROM pgrst_call)::text AS page_total, {body} AS body"
        )));
        p.kind = Kind::RpcScalar;
        return Ok(p);
    }
    // Rows: shaped like a table read, from the materialized call.
    let ret_table = f.ret_rel.as_ref().and_then(|(s, n)| cache.table(s, n));
    let items = select_of(&q);
    let mut b = Builder::new(cache, schema, &q);
    let root = Rel { table: ret_table, rref: "\"pgrst_call\"".into() };
    let cols = b.select_list(&root, &items, &[])?;
    let conds = b.where_conds(&root, &items, &[])?;
    let wh = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    let order = q.order_at(&[]).map(|o| format!(" ORDER BY {}", order_sql(&root.rref, o))).unwrap_or_default();
    let (r_off, r_lim) = range(req)?;
    let offset = q.offset_at(&[]).or(r_off);
    let mut limit = q.limit_at(&[]).or(r_lim);
    if let (Some(m), true) = (p.max_rows, f.returns_set) {
        limit = Some(limit.map_or(m, |l| l.min(m)));
    }
    p.offset = offset.unwrap_or(0);
    let from_where = format!("pgrst_call{wh}");
    let total = match p.count {
        Some(_) => format!("(SELECT count(*) FROM {from_where})::text"),
        None => "NULL::text".into(),
    };
    let src_from = if from_args.is_empty() { format!("SELECT _f.* FROM {call} AS _f") } else { format!("SELECT _f.* FROM pgrst_args, LATERAL {call} AS _f") };
    p.stmts.push(Stmt::Rows(format!(
        "WITH {args_cte}pgrst_call AS MATERIALIZED ({src_from}), pgrst_source AS (SELECT {} FROM {from_where}{order}{}) \
         SELECT {total} AS total, (SELECT count(*) FROM pgrst_source)::text AS page_total, \
         coalesce((SELECT json_agg(_postgrest_t) FROM pgrst_source AS _postgrest_t), '[]'::json)::text AS body",
        cols.join(", "),
        limit_sql(limit, offset)
    )));
    if p.count == Some(Count::Planned) || p.count == Some(Count::Estimated) {
        // A function's estimate means running it again: count exactly.
        p.count = Some(Count::Exact);
    }
    p.kind = if f.returns_set { Kind::Read } else { Kind::RpcObject };
    Ok(p)
}
