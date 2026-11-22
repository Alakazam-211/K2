//! The statements' answers → PostgREST's answer: status, `Content-Range`,
//! `Preference-Applied`, body.

use serde_json::Value;

use crate::error::ApiError;
use crate::plan::{Count, Kind, Plan};

/// One statement's answer, as text columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Rows(Vec<Vec<Option<String>>>),
    Affected(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

const JSON: &str = "application/json; charset=utf-8";
const OBJECT: &str = "application/vnd.pgrst.object+json; charset=utf-8";

fn first_row(o: Option<&Out>) -> Result<(Option<u64>, u64, String), ApiError> {
    let bad = || ApiError::new(500, "K2REST500", "the statement's answer had an unexpected shape");
    let Some(Out::Rows(rows)) = o else { return Err(bad()) };
    let r = rows.first().ok_or_else(bad)?;
    let total = r.first().cloned().flatten().and_then(|t| t.parse::<u64>().ok());
    let page = r.get(1).cloned().flatten().and_then(|t| t.parse::<u64>().ok()).ok_or_else(bad)?;
    let body = r.get(2).cloned().flatten().unwrap_or_else(|| "null".into());
    Ok((total, page, body))
}

/// `EXPLAIN (FORMAT JSON)`'s row estimate.
fn planned_rows(o: Option<&Out>) -> Option<u64> {
    let Some(Out::Rows(rows)) = o else { return None };
    let text = rows.first()?.first()?.clone()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get(0)?.get("Plan")?.get("Plan Rows")?.as_f64().map(|f| f.max(0.0) as u64)
}

fn singular_error(n: u64) -> ApiError {
    ApiError::new(406, "PGRST116", "JSON object requested, multiple (or no) rows returned").details(format!("The result contains {n} rows"))
}

fn content_range(offset: u64, n: u64, total: Option<u64>) -> String {
    let t = total.map_or("*".to_string(), |t| t.to_string());
    if n == 0 {
        format!("*/{t}")
    } else {
        format!("{offset}-{}/{t}", offset + n - 1)
    }
}

/// The first element of a JSON array body (singular answers).
fn only(body: &str) -> Result<String, ApiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| ApiError::new(500, "K2REST500", format!("answer body: {e}")))?;
    Ok(v.get(0).cloned().unwrap_or(Value::Null).to_string())
}

/// Shape the answer. `Err` means the transaction must roll back (a write
/// whose rows break `Accept: …object+json`) or the read can't be answered
/// (a range past the end).
pub fn respond(plan: &Plan, outs: &[Out]) -> Result<Response, ApiError> {
    let mut headers: Vec<(String, String)> = Vec::new();
    if !plan.applied.is_empty() {
        headers.push(("Preference-Applied".into(), plan.applied.join(", ")));
    }
    let ct = if plan.singular { OBJECT } else { JSON };
    match &plan.kind {
        Kind::Read | Kind::RpcObject => {
            let (exact, n, body) = first_row(outs.first())?;
            let total = match plan.count {
                Some(Count::Exact) => exact,
                Some(Count::Planned) => planned_rows(outs.get(1)),
                Some(Count::Estimated) => match (exact, plan.max_rows) {
                    (Some(e), Some(m)) if e > m => planned_rows(outs.get(1)).map(|p| p.max(e)),
                    (e, _) => e,
                },
                None => None,
            };
            if plan.kind == Kind::RpcObject {
                headers.push(("Content-Range".into(), content_range(0, n, None)));
                headers.push(("Content-Type".into(), ct.into()));
                return Ok(Response { status: 200, headers, body: if plan.head { Vec::new() } else { only(&body)?.into_bytes() } });
            }
            if plan.singular && n != 1 {
                return Err(singular_error(n));
            }
            let mut status = 200;
            if let Some(t) = total {
                if plan.offset > t {
                    return Err(ApiError::new(416, "PGRST103", "Requested range not satisfiable")
                        .details(format!("An offset of {} was requested, but there are only {t} rows.", plan.offset)));
                }
                if n < t {
                    status = 206;
                }
            }
            headers.push(("Content-Range".into(), content_range(plan.offset, n, total)));
            headers.push(("Content-Type".into(), ct.into()));
            let body = if plan.singular { only(&body)? } else { body };
            Ok(Response { status, headers, body: if plan.head { Vec::new() } else { body.into_bytes() } })
        }
        Kind::RpcScalar => {
            let (_, n, body) = first_row(outs.first())?;
            headers.push(("Content-Range".into(), content_range(0, n, None)));
            headers.push(("Content-Type".into(), JSON.into()));
            Ok(Response { status: 200, headers, body: if plan.head { Vec::new() } else { body.into_bytes() } })
        }
        Kind::RpcVoid => Ok(Response { status: 204, headers, body: Vec::new() }),
        Kind::Insert { representation, .. } | Kind::Update { representation } | Kind::Delete { representation } => {
            let insert = matches!(plan.kind, Kind::Insert { .. });
            // An upsert whose every row merged into an existing one is 200.
            let merged_only = matches!(plan.kind, Kind::Insert { upsert: true, .. })
                && *representation
                && matches!(outs.first(), Some(Out::Rows(r)) if r.first().and_then(|row| row.get(3).cloned().flatten()).as_deref() == Some("0"));
            let (n, body) = if *representation {
                let (_, n, body) = first_row(outs.first())?;
                (n, Some(body))
            } else {
                match outs.first() {
                    Some(Out::Affected(n)) => (*n, None),
                    _ => return Err(ApiError::new(500, "K2REST500", "the write's answer had an unexpected shape")),
                }
            };
            if plan.singular && n != 1 {
                return Err(singular_error(n));
            }
            let total = plan.count.map(|_| n);
            headers.push(("Content-Range".into(), if insert { format!("*/{}", total.map_or("*".into(), |t| t.to_string())) } else { content_range(0, n, total) }));
            match body {
                Some(b) => {
                    headers.push(("Content-Type".into(), ct.into()));
                    let b = if plan.singular { only(&b)? } else { b };
                    Ok(Response { status: if insert && !merged_only { 201 } else { 200 }, headers, body: b.into_bytes() })
                }
                None => Ok(Response { status: if insert { 201 } else { 204 }, headers, body: Vec::new() }),
            }
        }
    }
}
