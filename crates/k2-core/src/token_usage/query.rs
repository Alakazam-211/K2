//! Ledger reports (Usage v2): filters, `since`/`until`, day buckets in the
//! caller's IANA zone (B4) bucketed in SQL (B17), longest-prefix workspace
//! attribution (B9), per-login totals, `group_by`, and the estimated cost
//! (provider cost first, then the price table).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use chrono::TimeZone;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::usage_prices::{PriceInfo, PriceTable};

use super::{canonical_workspace, is_sentinel_workspace, ledger_path, open_reader};

/// Clamp bounds for a turn-log page.
pub const MAX_TURNS_LIMIT: i64 = 200;
pub const DEFAULT_TURNS_LIMIT: i64 = 50;

/// Day buckets are built from 15-minute SQL buckets: every zone offset in
/// use today is a whole number of quarter hours.
const BUCKET_MS: i64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// One workspace (prefix attribution). Not with `outside_only`.
    pub workspace: Option<String>,
    pub outside_only: bool,
    pub harness: Option<String>,
    pub account: Option<String>,
    pub model: Option<String>,
    /// Inclusive lower bound, unix ms.
    pub since_ms: Option<i64>,
    /// Exclusive upper bound, unix ms.
    pub until_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Workspace,
    Model,
    Login,
    Harness,
}

impl GroupBy {
    pub fn parse(s: &str) -> Option<GroupBy> {
        match s.trim() {
            "workspace" => Some(GroupBy::Workspace),
            "model" => Some(GroupBy::Model),
            "login" | "account" => Some(GroupBy::Login),
            "harness" => Some(GroupBy::Harness),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReportOpts {
    pub tz: chrono_tz::Tz,
    pub group_by: Option<GroupBy>,
    /// Compute the estimated cost.
    pub cost: bool,
}

/// An IANA zone name (`America/Denver`, `UTC`) for [`ReportOpts::tz`].
pub fn parse_tz(name: &str) -> Result<chrono_tz::Tz, String> {
    let n = name.trim();
    if n.is_empty() {
        return Ok(chrono_tz::UTC);
    }
    n.parse::<chrono_tz::Tz>().map_err(|_| format!("unknown time zone {n:?} (an IANA name like America/Denver)"))
}

impl Default for ReportOpts {
    fn default() -> Self {
        ReportOpts { tz: chrono_tz::UTC, group_by: None, cost: true }
    }
}

/// Estimated API-equivalent cost of a set of rows. Not the user's bill.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct CostSummary {
    pub usd: f64,
    /// `provider` · `model` · `mixed` · `unpriced` · `none` (no rows).
    pub source: String,
    pub priced_requests: i64,
    pub provider_requests: i64,
    pub unpriced_requests: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct TokenTotals {
    /// Uncached input for every harness.
    pub input_tokens: i64,
    /// Includes reasoning.
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// A subset of `output_tokens`.
    pub reasoning_tokens: i64,
    /// Model requests (one ledger row each).
    pub requests: i64,
    /// Alias of `requests` for the v1 page.
    pub turns: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HarnessTotals {
    pub harness: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelTotals {
    pub harness: String,
    pub model: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkspaceTotals {
    pub path: String,
    pub outside: bool,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoginTotals {
    pub account_id: String,
    pub tool: String,
    pub label: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GroupTotals {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

/// One calendar day in the report's zone.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DayTotals {
    /// `YYYY-MM-DD` in the report's `tz`.
    pub day: String,
    #[serde(flatten)]
    pub totals: TokenTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UsageReport {
    pub scope: String,
    pub workspace: Option<String>,
    pub tz: String,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub total: TokenTotals,
    pub harnesses: Vec<HarnessTotals>,
    pub models: Vec<ModelTotals>,
    pub workspaces: Vec<WorkspaceTotals>,
    pub logins: Vec<LoginTotals>,
    pub outside: TokenTotals,
    /// Ascending days with rows. Rows without a usable time count in
    /// `total` only.
    pub days: Vec<DayTotals>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<GroupTotals>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prices: Option<PriceInfo>,
}

// ── Accumulation ────────────────────────────────────────────────────

/// Sums for one SQL group, split so the cost can be priced linearly:
/// rows with a provider cost vs rows the table must price.
#[derive(Debug, Clone, Default)]
struct Sums {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    requests: i64,
    np_input: i64,
    np_output: i64,
    np_cache_read: i64,
    np_cache_write: i64,
    np_requests: i64,
    provider_usd: f64,
    provider_requests: i64,
}

const SUM_COLUMNS: &str = "
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0),
    COALESCE(SUM(reasoning_tokens), 0), COUNT(*),
    COALESCE(SUM(CASE WHEN provider_cost_usd IS NULL THEN input_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN provider_cost_usd IS NULL THEN output_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN provider_cost_usd IS NULL THEN cache_read_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN provider_cost_usd IS NULL THEN cache_write_tokens ELSE 0 END), 0),
    COALESCE(SUM(CASE WHEN provider_cost_usd IS NULL THEN 1 ELSE 0 END), 0),
    COALESCE(SUM(provider_cost_usd), 0.0), COUNT(provider_cost_usd)";

fn read_sums(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Sums> {
    Ok(Sums {
        input: r.get(at)?,
        output: r.get(at + 1)?,
        cache_read: r.get(at + 2)?,
        cache_write: r.get(at + 3)?,
        reasoning: r.get(at + 4)?,
        requests: r.get(at + 5)?,
        np_input: r.get(at + 6)?,
        np_output: r.get(at + 7)?,
        np_cache_read: r.get(at + 8)?,
        np_cache_write: r.get(at + 9)?,
        np_requests: r.get(at + 10)?,
        provider_usd: r.get(at + 11)?,
        provider_requests: r.get(at + 12)?,
    })
}

#[derive(Debug, Clone, Default)]
struct Acc {
    t: TokenTotals,
    provider_usd: f64,
    provider_requests: i64,
    model_usd: f64,
    model_requests: i64,
    unpriced_requests: i64,
}

impl Acc {
    fn add(&mut self, model: &str, s: &Sums, prices: Option<&PriceTable>) {
        self.t.input_tokens += s.input;
        self.t.output_tokens += s.output;
        self.t.cache_read_tokens += s.cache_read;
        self.t.cache_write_tokens += s.cache_write;
        self.t.reasoning_tokens += s.reasoning;
        self.t.requests += s.requests;
        self.t.turns += s.requests;
        let Some(prices) = prices else { return };
        self.provider_usd += s.provider_usd;
        self.provider_requests += s.provider_requests;
        if s.np_requests > 0 {
            match prices.cost(model, s.np_input, s.np_output, s.np_cache_read, s.np_cache_write) {
                Some(c) => {
                    self.model_usd += c;
                    self.model_requests += s.np_requests;
                }
                None => self.unpriced_requests += s.np_requests,
            }
        }
    }

    fn finish(&self, cost: bool) -> TokenTotals {
        let mut t = self.t.clone();
        if cost {
            let priced = self.provider_requests + self.model_requests;
            let source = if self.t.requests == 0 {
                "none"
            } else if priced == 0 {
                "unpriced"
            } else if self.model_requests == 0 {
                "provider"
            } else if self.provider_requests == 0 {
                "model"
            } else {
                "mixed"
            };
            t.cost = Some(CostSummary {
                usd: self.provider_usd + self.model_usd,
                source: source.into(),
                priced_requests: priced,
                provider_requests: self.provider_requests,
                unpriced_requests: self.unpriced_requests,
            });
        }
        t
    }
}

// ── Attribution ─────────────────────────────────────────────────────

/// Known workspaces, canonical, longest first.
pub(crate) fn known_sorted(known: &[String]) -> Vec<String> {
    let mut out: Vec<String> = known
        .iter()
        .filter(|p| !is_sentinel_workspace(p))
        .map(|p| canonical_workspace(p))
        .filter(|p| !p.is_empty())
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

/// The longest known workspace that contains `raw` (P12, B9).
pub(crate) fn attribute(raw: &str, known: &[String]) -> Option<String> {
    if raw.is_empty() || is_sentinel_workspace(raw) {
        return None;
    }
    known.iter().find(|k| super::within(raw, k)).cloned()
}

fn distinct_paths(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare("SELECT DISTINCT workspace_path FROM turns").map_err(|e| format!("ledger paths: {e}"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("ledger paths: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("ledger path: {e}"))?);
    }
    Ok(out)
}

/// The raw paths a workspace / outside filter keeps, as a JSON array for
/// `json_each` (`None` = no path filter).
fn path_set(conn: &Connection, filter: &Filter, known: &[String]) -> Result<Option<String>, String> {
    let target = filter.workspace.as_deref().map(canonical_workspace).filter(|s| !s.is_empty());
    if target.is_none() && !filter.outside_only {
        return Ok(None);
    }
    let keep: Vec<String> = distinct_paths(conn)?
        .into_iter()
        .filter(|raw| {
            let attr = attribute(raw, known);
            match &target {
                Some(t) => attr.as_deref() == Some(t.as_str()),
                None => attr.is_none(),
            }
        })
        .collect();
    Ok(Some(serde_json::to_string(&keep).map_err(|e| e.to_string())?))
}

const WHERE: &str = "
    WHERE (?1 IS NULL OR workspace_path IN (SELECT value FROM json_each(?1)))
      AND (?2 IS NULL OR harness = ?2)
      AND (?3 IS NULL OR account_id = ?3)
      AND (?4 IS NULL OR model = ?4)
      AND (?5 IS NULL OR recorded_ms >= ?5)
      AND (?6 IS NULL OR recorded_ms < ?6)";

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

// ── Labels ──────────────────────────────────────────────────────────

/// `(tool, label)` of a wallet login, from the shared DB when present.
fn login_label(account_id: &str) -> Option<(String, String)> {
    if account_id.is_empty() {
        return None;
    }
    let db = crate::db::try_shared()?;
    let conn = db.lock();
    let e = crate::llm_accounts::get(&conn, account_id).ok()??;
    let label = e.email.clone().filter(|s| !s.is_empty() && e.label == "Default").unwrap_or(e.label.clone());
    Some((e.tool, label))
}

// ── Report ──────────────────────────────────────────────────────────

/// The report for `filter`. `known` = this daemon's workspace paths.
pub fn report(ledger: &Path, filter: &Filter, opts: &ReportOpts, known: &[String]) -> Result<UsageReport, String> {
    let known = known_sorted(known);
    let table = if opts.cost { Some(crate::usage_prices::table()) } else { None };
    let prices = table.as_deref();
    let conn = open_reader(ledger)?;
    let paths = path_set(&conn, filter, &known)?;

    // Groups: (workspace, harness, model, login). Few rows: one per
    // distinct combination, not per request.
    let sql = format!("SELECT workspace_path, harness, model, account_id, {SUM_COLUMNS} FROM turns {WHERE} GROUP BY 1, 2, 3, 4");
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("ledger query: {e}"))?;
    let rows = stmt
        .query_map(
            params![
                paths,
                nonempty(&filter.harness),
                nonempty(&filter.account),
                nonempty(&filter.model),
                filter.since_ms,
                filter.until_ms
            ],
            |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, read_sums(r, 4)?))
        },
        )
        .map_err(|e| format!("ledger query: {e}"))?;
    let target = filter.workspace.as_deref().map(canonical_workspace).filter(|s| !s.is_empty());
    let mut total = Acc::default();
    let mut outside = Acc::default();
    let mut by_ws: BTreeMap<String, Acc> = BTreeMap::new();
    let mut by_h: BTreeMap<String, Acc> = BTreeMap::new();
    let mut by_m: BTreeMap<(String, String), Acc> = BTreeMap::new();
    let mut by_login: BTreeMap<(String, String), Acc> = BTreeMap::new();
    let mut attr_memo: HashMap<String, Option<String>> = HashMap::new();
    for row in rows {
        let (raw, harness, model, account, s) = row.map_err(|e| format!("ledger row: {e}"))?;
        let attr = attr_memo.entry(raw.clone()).or_insert_with(|| attribute(&raw, &known)).clone();
        total.add(&model, &s, prices);
        by_h.entry(harness.clone()).or_default().add(&model, &s, prices);
        by_m.entry((harness.clone(), model.clone())).or_default().add(&model, &s, prices);
        by_login.entry((account.clone(), harness.clone())).or_default().add(&model, &s, prices);
        match attr {
            Some(ws) => by_ws.entry(ws).or_default().add(&model, &s, prices),
            None => outside.add(&model, &s, prices),
        }
    }
    drop(stmt);
    if let Some(t) = &target {
        by_ws.entry(t.clone()).or_default();
    }

    // Days: 15-minute buckets from SQL, folded into local days in Rust.
    let days = day_buckets(&conn, filter, paths_param(&paths), opts, prices)?;

    let cost = opts.cost;
    let mut workspaces: Vec<WorkspaceTotals> =
        by_ws.iter().map(|(path, a)| WorkspaceTotals { path: path.clone(), outside: false, totals: a.finish(cost) }).collect();
    if target.is_none() && filter.outside_only {
        workspaces.clear();
    }
    let harnesses = by_h.iter().map(|(h, a)| HarnessTotals { harness: h.clone(), totals: a.finish(cost) }).collect();
    let models = by_m
        .iter()
        .map(|((h, m), a)| ModelTotals { harness: h.clone(), model: m.clone(), totals: a.finish(cost) })
        .collect();
    let logins: Vec<LoginTotals> = by_login
        .iter()
        .map(|((id, harness), a)| {
            let (tool, label) = login_label(id).unwrap_or_else(|| (harness.clone(), if id.is_empty() { "Unknown login".into() } else { id.clone() }));
            LoginTotals { account_id: id.clone(), tool, label, totals: a.finish(cost) }
        })
        .collect();
    let groups = opts.group_by.map(|g| {
        let mut out: Vec<GroupTotals> = match g {
            GroupBy::Workspace => {
                let mut v: Vec<GroupTotals> = by_ws
                    .iter()
                    .map(|(p, a)| GroupTotals { key: p.clone(), label: p.clone(), totals: a.finish(cost) })
                    .collect();
                if outside.t.requests > 0 {
                    v.push(GroupTotals { key: String::new(), label: "Outside workspaces".into(), totals: outside.finish(cost) });
                }
                v
            }
            GroupBy::Model => by_m
                .iter()
                .map(|((h, m), a)| GroupTotals { key: format!("{h}/{m}"), label: m.clone(), totals: a.finish(cost) })
                .collect(),
            GroupBy::Harness => by_h
                .iter()
                .map(|(h, a)| GroupTotals { key: h.clone(), label: h.clone(), totals: a.finish(cost) })
                .collect(),
            GroupBy::Login => logins
                .iter()
                .map(|l| GroupTotals { key: l.account_id.clone(), label: l.label.clone(), totals: l.totals.clone() })
                .collect(),
        };
        out.sort_by(|a, b| {
            let size = |t: &TokenTotals| t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;
            size(&b.totals).cmp(&size(&a.totals)).then_with(|| a.key.cmp(&b.key))
        });
        out
    });
    Ok(UsageReport {
        scope: if target.is_some() { "workspace".into() } else { "machine".into() },
        workspace: target,
        tz: opts.tz.name().to_string(),
        since_ms: filter.since_ms,
        until_ms: filter.until_ms,
        total: total.finish(cost),
        harnesses,
        models,
        workspaces,
        logins,
        outside: outside.finish(cost),
        days,
        groups,
        prices: table.as_ref().map(|t| t.info()),
    })
}

fn paths_param(p: &Option<String>) -> Option<&str> {
    p.as_deref()
}

fn day_buckets(
    conn: &Connection,
    filter: &Filter,
    paths: Option<&str>,
    opts: &ReportOpts,
    prices: Option<&PriceTable>,
) -> Result<Vec<DayTotals>, String> {
    let sql = format!(
        "SELECT recorded_ms / {BUCKET_MS}, model, {SUM_COLUMNS} FROM turns {WHERE} AND recorded_ms IS NOT NULL GROUP BY 1, 2"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("ledger days: {e}"))?;
    let rows = stmt
        .query_map(
            params![
                paths,
                nonempty(&filter.harness),
                nonempty(&filter.account),
                nonempty(&filter.model),
                filter.since_ms,
                filter.until_ms
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, read_sums(r, 2)?)),
        )
        .map_err(|e| format!("ledger days: {e}"))?;
    let mut by_day: BTreeMap<String, Acc> = BTreeMap::new();
    let mut day_memo: HashMap<i64, Option<String>> = HashMap::new();
    for row in rows {
        let (bucket, model, s) = row.map_err(|e| format!("ledger day row: {e}"))?;
        let day = day_memo
            .entry(bucket)
            .or_insert_with(|| local_day(bucket.saturating_mul(BUCKET_MS), opts.tz))
            .clone();
        let Some(day) = day else { continue };
        by_day.entry(day).or_default().add(&model, &s, prices);
    }
    Ok(by_day.into_iter().map(|(day, a)| DayTotals { day, totals: a.finish(opts.cost) }).collect())
}

/// `YYYY-MM-DD` of an instant in `tz`.
pub fn local_day(ms: i64, tz: chrono_tz::Tz) -> Option<String> {
    let dt = tz.timestamp_millis_opt(ms).single()?;
    Some(dt.format("%Y-%m-%d").to_string())
}

pub fn report_host(filter: &Filter, opts: &ReportOpts, known: &[String]) -> Result<UsageReport, String> {
    report(&ledger_path(), filter, opts, known)
}

// ── Turn log ────────────────────────────────────────────────────────

/// One ledger row (one request) for the live log.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TurnRow {
    pub rowid: i64,
    /// Pass as `before` / `after` (`<recorded_ms|n>:<rowid>`).
    pub cursor: String,
    /// The raw transcript cwd.
    pub workspace_path: String,
    /// The known workspace it counts for (longest prefix), if any.
    pub workspace: Option<String>,
    /// Not in any known workspace (or a sentinel).
    pub outside: bool,
    pub harness: String,
    pub model: String,
    pub account_id: String,
    pub session_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub recorded_at: String,
    pub recorded_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    /// `provider` · `model` · `unpriced`.
    pub cost_source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TurnPage {
    pub rows: Vec<TurnRow>,
    /// Continue older with `before=<next_cursor>`; `None` after a short page.
    pub next_cursor: Option<String>,
    /// More rows match than this page holds (newer ones, for `after`).
    pub has_more: bool,
}

/// A position in the log's order (`recorded_ms` desc, then rowid desc;
/// rows without a time last).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCursor {
    At { ms: Option<i64>, rowid: i64 },
    /// A v1 integer cursor: a bare rowid bound.
    LegacyRowid(i64),
}

impl TurnCursor {
    pub fn parse(s: &str) -> Option<TurnCursor> {
        let s = s.trim();
        if let Some((ms, rowid)) = s.split_once(':') {
            let rowid: i64 = rowid.parse().ok()?;
            let ms = if ms == "n" { None } else { Some(ms.parse::<i64>().ok()?) };
            return Some(TurnCursor::At { ms, rowid });
        }
        s.parse::<i64>().ok().map(TurnCursor::LegacyRowid)
    }

    pub fn of(ms: Option<i64>, rowid: i64) -> String {
        match ms {
            Some(ms) => format!("{ms}:{rowid}"),
            None => format!("n:{rowid}"),
        }
    }
}

/// SQL for "strictly older than" / "strictly newer than" a cursor (`?7`,
/// `?8` = ms, rowid).
fn cursor_clause(c: Option<TurnCursor>, newer: bool) -> &'static str {
    match (c, newer) {
        (None, _) => "",
        (Some(TurnCursor::LegacyRowid(_)), false) => " AND rowid < ?8",
        (Some(TurnCursor::LegacyRowid(_)), true) => " AND rowid > ?8",
        (Some(TurnCursor::At { ms: Some(_), .. }), false) => {
            " AND ((recorded_ms IS NOT NULL AND (recorded_ms < ?7 OR (recorded_ms = ?7 AND rowid < ?8))) OR recorded_ms IS NULL)"
        }
        (Some(TurnCursor::At { ms: None, .. }), false) => " AND recorded_ms IS NULL AND rowid < ?8",
        (Some(TurnCursor::At { ms: Some(_), .. }), true) => {
            " AND recorded_ms IS NOT NULL AND (recorded_ms > ?7 OR (recorded_ms = ?7 AND rowid > ?8))"
        }
        (Some(TurnCursor::At { ms: None, .. }), true) => " AND (recorded_ms IS NOT NULL OR rowid > ?8)",
    }
}

fn cursor_values(c: Option<TurnCursor>) -> (Option<i64>, Option<i64>) {
    match c {
        None => (None, None),
        Some(TurnCursor::LegacyRowid(r)) => (None, Some(r)),
        Some(TurnCursor::At { ms, rowid }) => (ms, Some(rowid)),
    }
}

/// Newest-first rows (B15). `before` pages older; `after` returns the
/// newest `limit` rows above a cursor (`has_more` = more sit between).
pub fn turns(
    ledger: &Path,
    filter: &Filter,
    known: &[String],
    before: Option<TurnCursor>,
    after: Option<TurnCursor>,
    limit: i64,
) -> Result<TurnPage, String> {
    let known = known_sorted(known);
    let limit = limit.clamp(1, MAX_TURNS_LIMIT);
    let prices = crate::usage_prices::table();
    let conn = open_reader(ledger)?;
    let paths = path_set(&conn, filter, &known)?;
    let (b_ms, b_row) = cursor_values(before);
    let (a_ms, a_row) = cursor_values(after);
    let sql = format!(
        "SELECT rowid, workspace_path, harness, model, account_id, session_id,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                recorded_at, recorded_ms, provider_cost_usd
         FROM turns {WHERE}{}{}
         ORDER BY (recorded_ms IS NULL), recorded_ms DESC, rowid DESC
         LIMIT ?11",
        cursor_clause(before, false),
        cursor_clause(after, true).replace("?7", "?9").replace("?8", "?10"),
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("ledger turns: {e}"))?;
    let mapped = stmt
        .query_map(
            params![
                paths,
                nonempty(&filter.harness),
                nonempty(&filter.account),
                nonempty(&filter.model),
                filter.since_ms,
                filter.until_ms,
                b_ms,
                b_row,
                a_ms,
                a_row,
                limit + 1
            ],
            |r| {
                let rowid: i64 = r.get(0)?;
                let raw: String = r.get(1)?;
                let model: String = r.get(3)?;
                let (input, output, cr, cw): (i64, i64, i64, i64) = (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?);
                let recorded_ms: Option<i64> = r.get(12)?;
                let provider: Option<f64> = r.get(13)?;
                let (cost_usd, cost_source) = match provider {
                    Some(c) => (Some(c), "provider"),
                    None => match prices.cost(&model, input, output, cr, cw) {
                        Some(c) => (Some(c), "model"),
                        None => (None, "unpriced"),
                    },
                };
                let workspace = attribute(&raw, &known);
                Ok(TurnRow {
                    rowid,
                    cursor: TurnCursor::of(recorded_ms, rowid),
                    outside: workspace.is_none(),
                    workspace,
                    workspace_path: raw,
                    harness: r.get(2)?,
                    model,
                    account_id: r.get(4)?,
                    session_id: r.get(5)?,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cr,
                    cache_write_tokens: cw,
                    reasoning_tokens: r.get(10)?,
                    recorded_at: r.get(11)?,
                    recorded_ms,
                    cost_usd,
                    cost_source: cost_source.into(),
                })
            },
        )
        .map_err(|e| format!("ledger turns: {e}"))?;
    let mut rows = Vec::new();
    for row in mapped {
        rows.push(row.map_err(|e| format!("ledger turn row: {e}"))?);
    }
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more { rows.last().map(|r| r.cursor.clone()) } else { None };
    Ok(TurnPage { rows, next_cursor, has_more })
}

pub fn turns_host(
    filter: &Filter,
    known: &[String],
    before: Option<TurnCursor>,
    after: Option<TurnCursor>,
    limit: i64,
) -> Result<TurnPage, String> {
    turns(&ledger_path(), filter, known, before, after, limit)
}
