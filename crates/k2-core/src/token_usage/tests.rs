use super::*;
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tmp() -> Tmp {
    let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("k2-token-usage-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("tmpdir");
    Tmp(fs::canonicalize(&p).expect("canon tmp"))
}

fn line(v: &Value) -> String {
    let mut s = serde_json::to_string(v).expect("json line");
    s.push('\n');
    s
}

fn write_jsonl(path: &Path, lines: &[Value]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    let body: String = lines.iter().map(line).collect();
    fs::write(path, body).expect("write jsonl");
}

fn append(path: &Path, text: &str) {
    let mut f = fs::OpenOptions::new().append(true).open(path).expect("append open");
    f.write_all(text.as_bytes()).expect("append");
}

#[derive(Debug)]
struct Stored {
    turn_key: String,
    workspace_path: String,
    harness: String,
    model: String,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    recorded_at: String,
    account_id: String,
    session_id: String,
    cost: Option<f64>,
}

fn stored_rows(ledger: &Path) -> Vec<Stored> {
    let conn = Connection::open(ledger).expect("reopen ledger");
    let mut stmt = conn
        .prepare(
            "SELECT turn_key, workspace_path, harness, model, input_tokens, output_tokens, cache_read_tokens,
                    cache_write_tokens, reasoning_tokens, recorded_at, account_id, session_id, provider_cost_usd
             FROM turns ORDER BY turn_key",
        )
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| {
            Ok(Stored {
                turn_key: r.get(0)?,
                workspace_path: r.get(1)?,
                harness: r.get(2)?,
                model: r.get(3)?,
                input: r.get(4)?,
                output: r.get(5)?,
                cache_read: r.get(6)?,
                cache_write: r.get(7)?,
                reasoning: r.get(8)?,
                recorded_at: r.get(9)?,
                account_id: r.get(10)?,
                session_id: r.get(11)?,
                cost: r.get(12)?,
            })
        })
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

fn claude_line(id: &str, model: &str, cwd: &str, ts: &str, input: i64, output: i64, cr: i64, cw: i64) -> Value {
    json!({
        "type": "assistant",
        "cwd": cwd,
        "sessionId": "sess-claude",
        "timestamp": ts,
        "message": {
            "id": id,
            "model": model,
            "role": "assistant",
            "content": [{"type": "text", "text": "not tokens"}],
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": cr,
                "cache_creation_input_tokens": cw
            }
        }
    })
}

fn ws_dir(root: &Path, name: &str) -> String {
    let p = root.join(name);
    fs::create_dir_all(&p).expect("ws dir");
    canonical_workspace(&p.to_string_lossy())
}

fn claude_root(dir: &Path) -> Root {
    Root { source: Source::Claude, dir: dir.to_path_buf(), home: Home::Default }
}

/// Insert finished rows directly (query tests).
fn insert(ledger: &Path, rows: &[(&str, &str, &str, &str, Option<i64>, i64, i64, i64, i64)]) {
    let conn = open_writer(ledger).expect("open ledger");
    for (key, harness, model, ws, ms, input, output, cr, cw) in rows {
        conn.execute(
            "INSERT INTO turns (turn_key, workspace_path, harness, model, input_tokens, output_tokens,
                cache_read_tokens, cache_write_tokens, recorded_at, recorded_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![key, ws, harness, model, input, output, cr, cw, ms.map(|m| m.to_string()).unwrap_or_default(), ms],
        )
        .expect("insert");
    }
}

const T0: i64 = 1_789_735_446_000; // 2026-09-18T12:44:06Z

// ── Ledger file ─────────────────────────────────────────────────────

#[test]
fn ledger_path_is_usage_tokens_sqlite() {
    let p = ledger_path();
    assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("tokens.sqlite"));
    assert_eq!(p.parent().and_then(|s| s.file_name()).and_then(|s| s.to_str()), Some("usage"));
    assert!(!p.to_string_lossy().ends_with("k2.db"));
}

#[test]
fn v1_ledger_is_wiped_once_logged_in_meta_and_never_again() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    {
        // The 0.40.150 schema, no user_version.
        let conn = Connection::open(&ledger).expect("v1");
        conn.execute_batch(
            "CREATE TABLE turns (turn_key TEXT PRIMARY KEY, workspace_path TEXT NOT NULL, harness TEXT NOT NULL,
               model TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
               cache_write_tokens INTEGER, recorded_at TEXT NOT NULL);
             CREATE TABLE scanned_files (path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime_secs INTEGER NOT NULL, mtime_nanos INTEGER NOT NULL);
             INSERT INTO turns VALUES ('old-1', '/x', 'codex', 'm', 30, 3, 11, 0, '2026-09-01T00:00:00Z');
             INSERT INTO turns VALUES ('old-2', '/x', 'codex', 'm', 5, 1, 1, 0, '2026-09-01T00:00:00Z');
             INSERT INTO scanned_files VALUES ('/x/rollout.jsonl', 10, 1, 1);",
        )
        .expect("v1 schema");
    }
    let conn = open_writer(&ledger).expect("migrate");
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).expect("version");
    assert_eq!(version, LEDGER_VERSION);
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0)).expect("count");
    assert_eq!(rows, 0, "v1 rows are wiped (counted under the old rules)");
    let files: i64 = conn.query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0)).expect("files");
    assert_eq!(files, 0, "scan stamps wiped so the rescan reads every file");
    drop(conn);
    let meta = ledger_meta(&ledger).expect("meta");
    assert_eq!(meta.get("rebuilt_from").map(String::as_str), Some("1"));
    assert_eq!(meta.get("rebuilt_rows").map(String::as_str), Some("2"));
    let first = meta.get("rebuilt_at").cloned().expect("rebuilt_at");
    // A v2 row survives every later open: the wipe runs once.
    insert(&ledger, &[("new", "claude", "m", "/x", Some(T0), 1, 1, 0, 0)]);
    drop(open_writer(&ledger).expect("reopen"));
    drop(open_reader(&ledger).expect("reader"));
    assert_eq!(stored_rows(&ledger).len(), 1);
    assert_eq!(ledger_meta(&ledger).expect("meta").get("rebuilt_at"), Some(&first));
}

#[test]
fn fresh_ledger_records_no_rebuild() {
    let dir = tmp();
    let ledger = dir.0.join("u").join("tokens.sqlite");
    drop(open_reader(&ledger).expect("create"));
    assert!(ledger_meta(&ledger).expect("meta").is_empty());
}

#[test]
fn ledger_has_no_planted_token_and_is_mode_0600() {
    let dir = tmp();
    let cwd = ws_dir(&dir.0, "ws");
    let token = "sk-provider-access-TOKEN-do-not-store";
    let projects = dir.0.join("claude");
    let mut l = claude_line("msg_tok", "claude-a", &cwd, "2026-09-18T00:00:00Z", 4, 5, 6, 7);
    l["message"]["content"][0]["text"] = json!(token);
    l["access_token"] = json!(token);
    write_jsonl(&projects.join("s.jsonl"), &[l]);
    let ledger = dir.0.join("usage").join("tokens.sqlite");
    scan(&ledger, &[claude_root(&projects)], &NoAccounts).expect("scan");
    let mut hay = fs::read(&ledger).expect("db");
    for extra in [sibling(&ledger, "-wal"), sibling(&ledger, "-shm")] {
        if extra.is_file() {
            hay.extend(fs::read(&extra).expect("sidecar"));
        }
    }
    assert!(!hay.windows(token.len()).any(|w| w == token.as_bytes()), "ledger holds the planted token");
    assert!(!hay.windows(10).any(|w| w == b"not tokens"), "ledger holds message content");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&ledger).expect("meta").permissions().mode() & 0o777, 0o600);
    }
}

// ── Claude (P6) ─────────────────────────────────────────────────────

#[test]
fn claude_copies_are_one_row_first_place_kept_counts_maxed() {
    let dir = tmp();
    let a = ws_dir(&dir.0, "a");
    let b = ws_dir(&dir.0, "b");
    let projects = dir.0.join("claude");
    // Placeholder first, full numbers later in the same file.
    write_jsonl(
        &projects.join("p1").join("s1.jsonl"),
        &[
            claude_line("msg_grow", "claude-a", &a, "2026-09-18T01:00:00Z", 1, 1, 5, 0),
            claude_line("msg_grow", "claude-a", &a, "2026-09-18T01:00:01Z", 80, 40, 900, 12),
        ],
    );
    // A resumed copy in another workspace's folder, later timestamp, a
    // stale placeholder: it must not move or shrink the row.
    write_jsonl(
        &projects.join("p2").join("fork.jsonl"),
        &[claude_line("msg_grow", "claude-a", &b, "2026-10-01T00:00:00Z", 2, 2, 2, 2)],
    );
    write_jsonl(
        &projects.join("history.jsonl"),
        &[claude_line("msg_history_only", "claude-a", &a, "2026-09-18T00:00:00Z", 999, 999, 999, 999)],
    );
    let ledger = dir.0.join("tokens.sqlite");
    scan(&ledger, &[claude_root(&projects)], &NoAccounts).expect("scan");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(r.turn_key, "msg_grow");
    assert_eq!((r.input, r.output, r.cache_read, r.cache_write), (80, 40, 900, 12), "max of each count");
    assert_eq!(r.workspace_path, a, "first workspace kept");
    assert_eq!(r.recorded_at, "2026-09-18T01:00:00Z", "first time kept");
    assert_eq!(r.session_id, "sess-claude");
    assert_eq!(r.harness, "claude");
    // Claude input is already uncached: never minus cache.
    assert_ne!(r.input, 80 - 900);
}

#[test]
fn claude_session_falls_back_to_the_file_stem() {
    let dir = tmp();
    let a = ws_dir(&dir.0, "a");
    let projects = dir.0.join("claude");
    let mut l = claude_line("m1", "claude-a", &a, "2026-09-18T01:00:00Z", 1, 1, 0, 0);
    l.as_object_mut().expect("obj").remove("sessionId");
    write_jsonl(&projects.join("conv-123.jsonl"), &[l]);
    let ledger = dir.0.join("tokens.sqlite");
    scan(&ledger, &[claude_root(&projects)], &NoAccounts).expect("scan");
    assert_eq!(stored_rows(&ledger)[0].session_id, "conv-123");
}

// ── Codex (P2, P4) ──────────────────────────────────────────────────

fn token_count(ts: &str, total: Option<(i64, i64, i64, i64)>, last: (i64, i64, i64, i64)) -> Value {
    let mut info = json!({"last_token_usage": {"input_tokens": last.0, "cached_input_tokens": last.1, "output_tokens": last.2, "reasoning_output_tokens": last.3}});
    if let Some(t) = total {
        info["total_token_usage"] = json!({"input_tokens": t.0, "cached_input_tokens": t.1, "output_tokens": t.2, "reasoning_output_tokens": t.3, "total_tokens": t.0 + t.2});
    }
    json!({"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count", "info": info}})
}

#[test]
fn codex_counts_every_request_uncached_and_dedupes_repeats() {
    let dir = tmp();
    let cwd = ws_dir(&dir.0, "ws");
    let sessions = dir.0.join("codex");
    let rollout = sessions.join("2026").join("09").join("26").join("rollout-a.jsonl");
    write_jsonl(
        &rollout,
        &[
            json!({"type": "session_meta", "payload": {"id": "sess-1", "cwd": cwd}}),
            json!({"type": "turn_context", "payload": {"turn_id": "turn-1", "model": "gpt-first", "cwd": cwd}}),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": null}}),
            token_count("2026-09-26T00:00:01Z", Some((10, 4, 1, 0)), (10, 4, 1, 0)),
            // Codex re-emits an unchanged token_count: same request.
            token_count("2026-09-26T00:00:01Z", Some((10, 4, 1, 0)), (10, 4, 1, 0)),
            token_count("2026-09-26T00:00:02Z", Some((30, 12, 3, 1)), (20, 8, 2, 1)),
            token_count("2026-09-26T00:00:03Z", Some((60, 23, 6, 1)), (30, 11, 3, 0)),
            json!({"type": "turn_context", "payload": {"turn_id": "turn-2", "model": "gpt-second", "cwd": cwd}}),
            token_count("2026-09-26T00:01:00Z", Some((65, 24, 7, 1)), (5, 1, 1, 0)),
        ],
    );
    // A forked copy of the same history in another rollout: no new rows.
    write_jsonl(
        &sessions.join("2026").join("09").join("27").join("rollout-fork.jsonl"),
        &[
            json!({"type": "session_meta", "payload": {"id": "sess-fork", "cwd": cwd}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-first", "cwd": cwd}}),
            token_count("2026-09-26T00:00:01Z", Some((10, 4, 1, 0)), (10, 4, 1, 0)),
            token_count("2026-09-26T00:00:02Z", Some((30, 12, 3, 1)), (20, 8, 2, 1)),
        ],
    );
    let ledger = dir.0.join("tokens.sqlite");
    let root = Root { source: Source::Codex, dir: sessions.clone(), home: Home::Default };
    scan(&ledger, &[root], &NoAccounts).expect("scan");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.len(), 4, "one row per request: {rows:?}");
    let first_model: Vec<&Stored> = rows.iter().filter(|r| r.model == "gpt-first").collect();
    assert_eq!(first_model.len(), 3);
    let input: i64 = first_model.iter().map(|r| r.input).sum();
    let cached: i64 = first_model.iter().map(|r| r.cache_read).sum();
    let output: i64 = first_model.iter().map(|r| r.output).sum();
    // Uncached: (10-4) + (20-8) + (30-11).
    assert_eq!(input, 6 + 12 + 19);
    assert_eq!(cached, 4 + 8 + 11);
    assert_eq!(output, 1 + 2 + 3);
    assert_ne!(input, 30, "not just the last request (B2)");
    let second = rows.iter().find(|r| r.model == "gpt-second").expect("turn 2");
    assert_eq!((second.input, second.cache_read, second.output), (4, 1, 1));
    assert!(rows.iter().all(|r| r.session_id == "sess-1" && r.workspace_path == cwd && r.harness == "codex"));
    assert!(rows.iter().all(|r| r.reasoning <= r.output));
}

#[test]
fn codex_without_totals_counts_each_request_by_offset() {
    let dir = tmp();
    let sessions = dir.0.join("codex");
    write_jsonl(
        &sessions.join("old.jsonl"),
        &[
            json!({"type": "turn_context", "payload": {"model": "gpt-old"}}),
            token_count("2026-09-26T00:00:01Z", None, (10, 0, 1, 0)),
            token_count("2026-09-26T00:00:02Z", None, (20, 0, 2, 0)),
        ],
    );
    let ledger = dir.0.join("tokens.sqlite");
    scan(&ledger, &[Root { source: Source::Codex, dir: sessions, home: Home::Default }], &NoAccounts).expect("scan");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.len(), 2, "no session id no longer collapses a session into one row");
    assert_eq!(rows.iter().map(|r| r.input).sum::<i64>(), 30);
}

#[test]
fn codex_rate_limits_surface_newest_per_login_with_pinned_conversations() {
    struct Pins;
    impl AccountResolver for Pins {
        fn live_owner(&self, _tool: &str) -> String {
            "acc-live".into()
        }
        fn conversation_login(&self, _tool: &str, conv: &str) -> Option<String> {
            (conv == "sess-pinned").then(|| "acc-pinned".to_string())
        }
    }
    let dir = tmp();
    let sessions = dir.0.join("codex");
    let limits = |pct: f64| json!({"primary": {"used_percent": pct, "window_minutes": 300, "resets_at": 1_800_000_000}, "secondary": {"used_percent": 3.0, "window_minutes": 10080}});
    let tc = |ts: &str, pct: f64, n: i64| {
        let mut v = token_count(ts, Some((n, 0, 1, 0)), (n, 0, 1, 0));
        v["payload"]["rate_limits"] = limits(pct);
        v
    };
    write_jsonl(
        &sessions.join("a.jsonl"),
        &[json!({"type": "session_meta", "payload": {"id": "sess-live"}}), tc("2026-09-26T00:00:01Z", 10.0, 7), tc("2026-09-26T00:00:05Z", 12.0, 9)],
    );
    let mut spark = token_count("2026-09-26T00:00:09Z", Some((99, 0, 1, 0)), (99, 0, 1, 0));
    spark["payload"]["rate_limits"] = json!({"limit_id": "spark", "primary": {"used_percent": 99.0}});
    write_jsonl(
        &sessions.join("b.jsonl"),
        &[json!({"type": "session_meta", "payload": {"id": "sess-pinned"}}), tc("2026-09-26T00:00:03Z", 40.0, 11), spark],
    );
    let ledger = dir.0.join("tokens.sqlite");
    let stats = scan(&ledger, &[Root { source: Source::Codex, dir: sessions, home: Home::Default }], &Pins).expect("scan");
    assert_eq!(stats.codex_limits.len(), 2, "{:?}", stats.codex_limits);
    let live = stats.codex_limits.iter().find(|l| l.account_id == "acc-live").expect("live");
    assert_eq!(live.snapshot["primary"]["used_percent"], json!(12.0), "newest snapshot wins");
    let pinned = stats.codex_limits.iter().find(|l| l.account_id == "acc-pinned").expect("pinned");
    assert_eq!(pinned.snapshot["primary"]["used_percent"], json!(40.0), "a model-specific snapshot is ignored");
    let rows = stored_rows(&ledger);
    assert!(rows.iter().filter(|r| r.session_id == "sess-pinned").all(|r| r.account_id == "acc-pinned"));
    assert!(rows.iter().filter(|r| r.session_id == "sess-live").all(|r| r.account_id == "acc-live"));
}

// ── Grok (P4) ───────────────────────────────────────────────────────

fn grok_session(dir: &Path, cwd: &str, lines: &[Value]) -> PathBuf {
    let session = dir.join("grok").join("encoded").join("sess-g");
    fs::create_dir_all(&session).expect("session");
    fs::write(session.join("summary.json"), json!({"current_model_id": "end-model", "info": {"cwd": cwd}}).to_string()).expect("summary");
    fs::write(session.join("chat_history.jsonl"), "{\"usage\":{\"inputTokens\":999}}\n").expect("history");
    write_jsonl(&session.join("updates.jsonl"), lines);
    dir.join("grok")
}

#[test]
fn grok_reasoning_is_inside_output_input_is_uncached_ticks_are_cost() {
    let dir = tmp();
    let cwd = ws_dir(&dir.0, "ws");
    let usage = json!({"inputTokens": 100, "outputTokens": 40, "reasoningTokens": 25, "cachedReadTokens": 30, "cacheCreationTokens": 10, "costUsdTicks": 12_500_000_000_i64});
    let root = grok_session(
        &dir.0,
        &cwd,
        &[
            json!({"method": "session/update", "params": {"update": {"sessionUpdate": "user_message_chunk", "content": "hello"}}}),
            json!({"method": "_x.ai/session/update", "timestamp": 1_789_735_446, "params": {"sessionId": "g-sess", "update": {"sessionUpdate": "turn_completed", "prompt_id": "p9", "model": "grok-test", "usage": usage}}}),
            json!({"method": "session/update", "timestamp": 1_789_735_447, "params": {"sessionId": "g-sess", "update": {"sessionUpdate": "turn_completed", "prompt_id": "p9", "model": "grok-test", "usage": usage}}}),
        ],
    );
    let ledger = dir.0.join("tokens.sqlite");
    scan(&ledger, &[Root { source: Source::Grok, dir: root, home: Home::Default }], &NoAccounts).expect("scan");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.turn_key, "p9\tgrok-test");
    assert_eq!(r.model, "grok-test");
    assert_eq!(r.input, 100 - 30 - 10, "uncached input");
    assert_eq!(r.output, 40, "reasoning is never added to output (B7b)");
    assert_eq!(r.reasoning, 25);
    assert_eq!((r.cache_read, r.cache_write), (30, 10));
    assert!((r.cost.expect("provider cost") - 1.25).abs() < 1e-12, "10^10 ticks = 1 USD");
    assert_eq!(r.session_id, "g-sess");
    assert_eq!(r.workspace_path, cwd);
}

#[test]
fn grok_model_usage_splits_and_shares_the_top_level_cost() {
    let dir = tmp();
    let cwd = ws_dir(&dir.0, "ws");
    let root = grok_session(
        &dir.0,
        &cwd,
        &[json!({"method": "_x.ai/session/update", "timestamp": 1_789_735_446, "params": {"update": {"prompt_id": "split", "usage": {
            "inputTokens": 100, "outputTokens": 10, "costUsdTicks": 40_000_000_000_i64,
            "modelUsage": {
                "m-a": {"inputTokens": 40, "outputTokens": 3, "reasoningTokens": 1, "cachedReadTokens": 5, "costUsdTicks": 10_000_000_000_i64},
                "m-b": {"inputTokens": 30, "outputTokens": 7, "cachedReadTokens": 0},
                "m-c": {"inputTokens": 20, "outputTokens": 3}
            }}}}})],
    );
    let ledger = dir.0.join("tokens.sqlite");
    scan(&ledger, &[Root { source: Source::Grok, dir: root, home: Home::Default }], &NoAccounts).expect("scan");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.len(), 3);
    let get = |m: &str| rows.iter().find(|r| r.model == m).expect(m);
    assert_eq!(get("m-a").input, 35);
    assert!((get("m-a").cost.expect("a") - 1.0).abs() < 1e-9, "own ticks kept");
    // Remaining $3 shared by size among the unticked: b = 37, c = 23.
    assert!((get("m-b").cost.expect("b") - 3.0 * 37.0 / 60.0).abs() < 1e-9);
    assert!((get("m-c").cost.expect("c") - 3.0 * 23.0 / 60.0).abs() < 1e-9);
    assert_eq!(rows.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(), vec!["sess-g"; 3], "folder name");
}

// ── Incremental ingest (P10) ────────────────────────────────────────

#[test]
fn incremental_ingest_resumes_at_the_offset_and_waits_for_whole_lines() {
    let dir = tmp();
    let a = ws_dir(&dir.0, "a");
    let projects = dir.0.join("claude");
    let file = projects.join("s.jsonl");
    write_jsonl(
        &file,
        &[
            claude_line("m1", "claude-a", &a, "2026-09-18T01:00:00Z", 1, 1, 0, 0),
            claude_line("m2", "claude-a", &a, "2026-09-18T01:00:01Z", 2, 2, 0, 0),
        ],
    );
    let ledger = dir.0.join("tokens.sqlite");
    let roots = [claude_root(&projects)];
    let s1 = scan(&ledger, &roots, &NoAccounts).expect("scan 1");
    assert_eq!((s1.files_read, s1.rows), (1, 2));
    let s2 = scan(&ledger, &roots, &NoAccounts).expect("scan 2");
    assert_eq!((s2.files_read, s2.files_skipped, s2.rows), (0, 1, 0), "unchanged file skipped");

    // One whole line plus a half-written one.
    let third = line(&claude_line("m3", "claude-a", &a, "2026-09-18T01:00:02Z", 3, 3, 0, 0));
    let fourth = line(&claude_line("m4", "claude-a", &a, "2026-09-18T01:00:03Z", 4, 4, 0, 0));
    let (head, tail) = fourth.split_at(fourth.len() / 2);
    append(&file, &third);
    append(&file, head);
    let s3 = scan(&ledger, &roots, &NoAccounts).expect("scan 3");
    assert_eq!(s3.rows, 1, "only the new whole line is read, not the file again");
    assert_eq!(stored_rows(&ledger).len(), 3);

    append(&file, tail);
    let s4 = scan(&ledger, &roots, &NoAccounts).expect("scan 4");
    assert_eq!(s4.rows, 1, "the finished line is read once");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.iter().map(|r| r.input).collect::<Vec<_>>(), vec![1, 2, 3, 4]);

    // Replaced file (new inode): read from the top, no double count.
    let body = fs::read_to_string(&file).expect("body");
    fs::remove_file(&file).expect("rm");
    fs::write(&file, body).expect("rewrite");
    scan(&ledger, &roots, &NoAccounts).expect("scan 5");
    assert_eq!(stored_rows(&ledger).iter().map(|r| r.input).sum::<i64>(), 10);
}

#[test]
fn incremental_ingest_keeps_codex_context_across_reads() {
    let dir = tmp();
    let cwd = ws_dir(&dir.0, "ws");
    let sessions = dir.0.join("codex");
    let file = sessions.join("r.jsonl");
    write_jsonl(
        &file,
        &[
            json!({"type": "session_meta", "payload": {"id": "sess-x", "cwd": cwd}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-x", "cwd": cwd}}),
            token_count("2026-09-26T00:00:01Z", Some((10, 0, 1, 0)), (10, 0, 1, 0)),
        ],
    );
    let ledger = dir.0.join("tokens.sqlite");
    let roots = [Root { source: Source::Codex, dir: sessions, home: Home::Default }];
    scan(&ledger, &roots, &NoAccounts).expect("scan 1");
    append(&file, &line(&token_count("2026-09-26T00:00:02Z", Some((30, 0, 2, 0)), (20, 0, 1, 0))));
    let s = scan(&ledger, &roots, &NoAccounts).expect("scan 2");
    assert_eq!(s.rows, 1);
    let rows = stored_rows(&ledger);
    let late = rows.iter().find(|r| r.input == 20).expect("appended request");
    assert_eq!((late.model.as_str(), late.session_id.as_str(), late.workspace_path.as_str()), ("gpt-x", "sess-x", cwd.as_str()));
}

#[test]
fn a_report_does_not_wait_for_a_running_scan() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    insert(&ledger, &[("k", "claude", "m", "/x", Some(T0), 1, 1, 0, 0)]);
    let (tx, rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _g = writer_lock();
        tx.send(()).expect("locked");
        std::thread::sleep(Duration::from_millis(1500));
    });
    rx.recv().expect("lock held");
    let started = std::time::Instant::now();
    let r = report(&ledger, &Filter::default(), &ReportOpts::default(), &[]).expect("report");
    assert_eq!(r.total.requests, 1);
    assert!(started.elapsed() < Duration::from_millis(1000), "reader blocked on the scan lock: {:?}", started.elapsed());
    holder.join().expect("join");
}

// ── Roots and logins (P3) ───────────────────────────────────────────

#[test]
fn slot_homes_are_their_login_and_default_homes_the_live_login() {
    struct Live;
    impl AccountResolver for Live {
        fn live_owner(&self, tool: &str) -> String {
            format!("live-{tool}")
        }
        fn conversation_login(&self, _: &str, _: &str) -> Option<String> {
            None
        }
    }
    let dir = tmp();
    let a = ws_dir(&dir.0, "a");
    let default_projects = dir.0.join("claude").join("projects");
    let slot_projects = dir.0.join("k2").join("llm-accounts").join("claude").join("acc-slot").join("projects");
    write_jsonl(&default_projects.join("x").join("d.jsonl"), &[claude_line("m-default", "claude-a", &a, "2026-09-18T01:00:00Z", 1, 1, 0, 0)]);
    write_jsonl(&slot_projects.join("x").join("s.jsonl"), &[claude_line("m-slot", "claude-a", &a, "2026-09-18T01:00:00Z", 7, 1, 0, 0)]);
    let ledger = dir.0.join("tokens.sqlite");
    let roots = [
        claude_root(&default_projects),
        Root { source: Source::Claude, dir: slot_projects, home: Home::Slot("acc-slot".into()) },
    ];
    scan(&ledger, &roots, &Live).expect("scan");
    let rows = stored_rows(&ledger);
    assert_eq!(rows.iter().find(|r| r.turn_key == "m-slot").expect("slot").account_id, "acc-slot");
    assert_eq!(rows.iter().find(|r| r.turn_key == "m-default").expect("default").account_id, "live-claude");
    let opts = ReportOpts { group_by: Some(GroupBy::Login), cost: false, ..Default::default() };
    let r = report(&ledger, &Filter::default(), &opts, &[a.clone()]).expect("report");
    let groups = r.groups.expect("groups");
    assert_eq!(groups[0].key, "acc-slot", "largest first");
    assert_eq!(groups[0].totals.input_tokens, 7);
    let only = report(&ledger, &Filter { account: Some("acc-slot".into()), ..Default::default() }, &opts, &[]).expect("one login");
    assert_eq!(only.total.input_tokens, 7);
    assert_eq!(only.logins.len(), 1);
}

#[test]
fn push_root_dedupes_and_prefers_the_slot() {
    let dir = tmp();
    let d = dir.0.join("p");
    fs::create_dir_all(&d).expect("d");
    let mut out = Vec::new();
    push_root(&mut out, Root { source: Source::Claude, dir: d.clone(), home: Home::Default });
    push_root(&mut out, Root { source: Source::Claude, dir: d.join("."), home: Home::Slot("s".into()) });
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].home, Home::Slot("s".into()));
}

// ── Reports (P5, P12, Phase 2, cost) ────────────────────────────────

#[test]
fn days_bucket_in_the_callers_zone() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    // 2026-10-16T01:30Z is the evening of 10-15 in Denver (UTC-6).
    let evening = chrono::DateTime::parse_from_rfc3339("2026-10-16T01:30:00Z").expect("t").timestamp_millis();
    let noon = chrono::DateTime::parse_from_rfc3339("2026-10-15T18:00:00Z").expect("t").timestamp_millis();
    insert(&ledger, &[("e", "claude", "m", "/x", Some(evening), 5, 0, 0, 0), ("n", "claude", "m", "/x", Some(noon), 3, 0, 0, 0)]);
    let utc = report(&ledger, &Filter::default(), &ReportOpts { cost: false, ..Default::default() }, &[]).expect("utc");
    assert_eq!(utc.tz, "UTC");
    assert_eq!(utc.days.iter().map(|d| (d.day.as_str(), d.totals.input_tokens)).collect::<Vec<_>>(), vec![("2026-10-15", 3), ("2026-10-16", 5)]);
    let tz: chrono_tz::Tz = "America/Denver".parse().expect("zone");
    let local = report(&ledger, &Filter::default(), &ReportOpts { tz, cost: false, ..Default::default() }, &[]).expect("denver");
    assert_eq!(local.tz, "America/Denver");
    assert_eq!(local.days.len(), 1, "both are the 15th in Denver: {:?}", local.days);
    assert_eq!(local.days[0].day, "2026-10-15");
    assert_eq!(local.days[0].totals.input_tokens, 8);
    // Kathmandu is UTC+5:45: the quarter-hour buckets still land right.
    let ktm: chrono_tz::Tz = "Asia/Kathmandu".parse().expect("zone");
    let late = chrono::DateTime::parse_from_rfc3339("2026-10-15T18:14:00Z").expect("t").timestamp_millis();
    assert_eq!(query::local_day(late, ktm).as_deref(), Some("2026-10-15"));
    assert_eq!(query::local_day(late + 60_000, ktm).as_deref(), Some("2026-10-16"));
}

#[test]
fn undatable_rows_count_in_total_but_not_days() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    insert(&ledger, &[("fixture", "grok", "m", "/x", None, 42, 7, 9, 11), ("real", "grok", "m", "/x", Some(T0), 8, 1, 2, 3)]);
    let r = report(&ledger, &Filter::default(), &ReportOpts { cost: false, ..Default::default() }, &[]).expect("report");
    assert_eq!(r.days.len(), 1);
    assert_eq!(r.days[0].day, "2026-09-18");
    assert_eq!(r.days[0].totals.input_tokens, 8);
    assert_eq!(r.total.input_tokens, 50);
    assert_eq!((r.total.requests, r.total.turns), (2, 2));
    assert_eq!(epoch_ms("10"), None);
}

#[test]
fn since_until_bound_totals_days_and_parse_relative_times() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    let day = 86_400_000;
    insert(
        &ledger,
        &[
            ("old", "claude", "m", "/x", Some(T0 - 10 * day), 100, 0, 0, 0),
            ("mid", "claude", "m", "/x", Some(T0 - 2 * day), 10, 0, 0, 0),
            ("new", "claude", "m", "/x", Some(T0), 1, 0, 0, 0),
        ],
    );
    let f = Filter { since_ms: Some(parse_time_bound("7d", T0).expect("7d")), ..Default::default() };
    let r = report(&ledger, &f, &ReportOpts { cost: false, ..Default::default() }, &[]).expect("7d");
    assert_eq!(r.total.input_tokens, 11);
    assert_eq!(r.days.len(), 2);
    let f = Filter { since_ms: Some(T0 - 3 * day), until_ms: Some(T0), ..Default::default() };
    let r = report(&ledger, &f, &ReportOpts { cost: false, ..Default::default() }, &[]).expect("window");
    assert_eq!(r.total.input_tokens, 10, "until is exclusive");
    assert_eq!(parse_time_bound("24h", T0).expect("24h"), T0 - day);
    assert_eq!(parse_time_bound("90m", T0).expect("90m"), T0 - 90 * 60_000);
    assert_eq!(parse_time_bound("2026-09-18T12:44:06Z", 0).expect("rfc"), T0);
    assert_eq!(parse_time_bound("1789735446000", 0).expect("ms"), T0);
    assert!(parse_time_bound("soon", T0).is_err());
    assert!(parse_time_bound("7x", T0).is_err());
}

#[test]
fn prefix_attribution_counts_subdirs_and_keeps_nested_worktrees_apart() {
    let dir = tmp();
    let root = ws_dir(&dir.0, "repo");
    let sub = ws_dir(&dir.0, "repo/app");
    let wt = ws_dir(&dir.0, "repo/.claude/worktrees/w1");
    let lookalike = ws_dir(&dir.0, "repo-other");
    let ledger = dir.0.join("tokens.sqlite");
    insert(
        &ledger,
        &[
            ("r", "claude", "m", &root, Some(T0), 10, 0, 0, 0),
            ("s", "claude", "m", &sub, Some(T0), 20, 0, 0, 0),
            ("w", "claude", "m", &wt, Some(T0), 300, 0, 0, 0),
            ("l", "claude", "m", &lookalike, Some(T0), 4000, 0, 0, 0),
            ("o", "claude", "m", "_orphan", Some(T0), 50000, 0, 0, 0),
        ],
    );
    let known = vec![root.clone(), wt.clone()];
    let opts = ReportOpts { cost: false, ..Default::default() };
    let all = report(&ledger, &Filter::default(), &opts, &known).expect("all");
    let ws = |p: &str| all.workspaces.iter().find(|w| w.path == p).map(|w| w.totals.input_tokens);
    assert_eq!(ws(&root), Some(30), "a subdir counts for its workspace (B9)");
    assert_eq!(ws(&wt), Some(300), "the longest known prefix wins");
    assert_eq!(all.outside.input_tokens, 54000, "repo-other is not inside repo");
    assert_eq!(all.total.input_tokens, 54330);
    let only = report(&ledger, &Filter { workspace: Some(root.clone()), ..Default::default() }, &opts, &known).expect("root");
    assert_eq!((only.scope.as_str(), only.total.input_tokens), ("workspace", 30));
    assert_eq!(only.workspaces.len(), 1);
    let out = report(&ledger, &Filter { outside_only: true, ..Default::default() }, &opts, &known).expect("outside");
    assert_eq!(out.total.input_tokens, 54000);
    assert!(out.workspaces.is_empty());
}

#[test]
fn group_by_model_and_harness_and_filters_combine() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    insert(
        &ledger,
        &[
            ("a", "claude", "m-big", "/x", Some(T0), 100, 0, 0, 0),
            ("b", "claude", "m-small", "/x", Some(T0), 1, 0, 0, 0),
            ("c", "codex", "g", "/x", Some(T0), 50, 0, 0, 0),
        ],
    );
    let opts = |g| ReportOpts { group_by: Some(g), cost: false, ..Default::default() };
    let m = report(&ledger, &Filter::default(), &opts(GroupBy::Model), &[]).expect("model");
    let keys: Vec<String> = m.groups.expect("groups").iter().map(|g| g.key.clone()).collect();
    assert_eq!(keys, vec!["claude/m-big", "codex/g", "claude/m-small"]);
    let h = report(&ledger, &Filter { harness: Some("claude".into()), ..Default::default() }, &opts(GroupBy::Harness), &[]).expect("harness");
    assert_eq!(h.total.input_tokens, 101);
    assert_eq!(h.groups.expect("groups").len(), 1);
    let one = report(&ledger, &Filter { model: Some("g".into()), ..Default::default() }, &opts(GroupBy::Model), &[]).expect("model filter");
    assert_eq!(one.total.input_tokens, 50);
    assert_eq!(GroupBy::parse("login"), Some(GroupBy::Login));
    assert_eq!(GroupBy::parse("nope"), None);
}

#[test]
fn cost_prefers_provider_then_table_and_tags_unpriced() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    insert(
        &ledger,
        &[
            ("priced", "claude", "claude-opus-5-5", "/x", Some(T0), 1_000_000, 0, 0, 0),
            ("unknown", "claude", "made-up-model", "/x", Some(T0), 5, 0, 0, 0),
        ],
    );
    {
        let conn = open_writer(&ledger).expect("open");
        conn.execute(
            "INSERT INTO turns (turn_key, harness, model, input_tokens, recorded_ms, provider_cost_usd) VALUES ('grok', 'grok', 'grok-x', 10, ?1, 0.5)",
            params![T0],
        )
        .expect("grok row");
    }
    let table = crate::usage_prices::table();
    let opus = table.lookup("claude-opus-5-5").expect("opus priced");
    let r = report(&ledger, &Filter::default(), &ReportOpts::default(), &[]).expect("report");
    let cost = r.total.cost.clone().expect("cost");
    assert_eq!((cost.provider_requests, cost.priced_requests, cost.unpriced_requests), (1, 2, 1));
    assert_eq!(cost.source, "mixed");
    assert!((cost.usd - (0.5 + 1_000_000.0 * opus.input)).abs() < 1e-9, "{}", cost.usd);
    let grok = r.harnesses.iter().find(|h| h.harness == "grok").expect("grok");
    assert_eq!(grok.totals.cost.as_ref().expect("c").source, "provider");
    let unknown = r.models.iter().find(|m| m.model == "made-up-model").expect("unknown");
    let uc = unknown.totals.cost.as_ref().expect("c");
    assert_eq!((uc.source.as_str(), uc.usd), ("unpriced", 0.0));
    assert!(r.prices.is_some());
    let page = turns(&ledger, &Filter::default(), &[], None, None, 10).expect("turns");
    let src = |k: &str| page.rows.iter().find(|r| r.model == k).map(|r| r.cost_source.clone());
    assert_eq!(src("grok-x").as_deref(), Some("provider"));
    assert_eq!(src("claude-opus-5-5").as_deref(), Some("model"));
    assert_eq!(src("made-up-model").as_deref(), Some("unpriced"));
    let none = report(&ledger, &Filter { harness: Some("codex".into()), ..Default::default() }, &ReportOpts::default(), &[]).expect("empty");
    assert_eq!(none.total.cost.expect("c").source, "none");
}

// ── Turn log (P11) ──────────────────────────────────────────────────

#[test]
fn turns_are_newest_by_time_not_ingest_order_and_page_both_ways() {
    let dir = tmp();
    let ledger = dir.0.join("tokens.sqlite");
    let s = 1000;
    // Ingest order is NOT time order: an old file scanned last.
    insert(
        &ledger,
        &[
            ("t3", "claude", "m", "/x", Some(T0 + 3 * s), 3, 0, 0, 0),
            ("t5", "claude", "m", "/x", Some(T0 + 5 * s), 5, 0, 0, 0),
            ("nil", "claude", "m", "/x", None, 9, 0, 0, 0),
            ("t1", "claude", "m", "/x", Some(T0 + s), 1, 0, 0, 0),
            ("t4", "claude", "m", "/x", Some(T0 + 4 * s), 4, 0, 0, 0),
            ("t2", "claude", "m", "/x", Some(T0 + 2 * s), 2, 0, 0, 0),
        ],
    );
    let ins = |p: &TurnPage| p.rows.iter().map(|r| r.input_tokens).collect::<Vec<_>>();
    let p1 = turns(&ledger, &Filter::default(), &[], None, None, 2).expect("p1");
    assert_eq!(ins(&p1), vec![5, 4], "newest by recorded time (B15)");
    assert!(p1.has_more);
    let c1 = TurnCursor::parse(p1.next_cursor.as_deref().expect("cursor")).expect("parse");
    let p2 = turns(&ledger, &Filter::default(), &[], Some(c1), None, 2).expect("p2");
    assert_eq!(ins(&p2), vec![3, 2]);
    let c2 = TurnCursor::parse(p2.next_cursor.as_deref().expect("cursor")).expect("parse");
    let p3 = turns(&ledger, &Filter::default(), &[], Some(c2), None, 2).expect("p3");
    assert_eq!(ins(&p3), vec![1, 9], "undated rows last");
    assert!(!p3.has_more && p3.next_cursor.is_none());

    // "Since head": a newer row lands; `after` the old head finds it.
    let head = TurnCursor::parse(&p1.rows[0].cursor).expect("head");
    insert(&ledger, &[("t6", "claude", "m", "/x", Some(T0 + 6 * s), 6, 0, 0, 0), ("t7", "claude", "m", "/x", Some(T0 + 7 * s), 7, 0, 0, 0)]);
    let fresh = turns(&ledger, &Filter::default(), &[], None, Some(head), 1).expect("after");
    assert_eq!(ins(&fresh), vec![7]);
    assert!(fresh.has_more, "a newer row sits between the page and the old head (B16)");
    let gap = TurnCursor::parse(&fresh.rows[0].cursor).expect("gap");
    let rest = turns(&ledger, &Filter::default(), &[], Some(gap), Some(head), 10).expect("fill");
    assert_eq!(ins(&rest), vec![6]);
    assert!(!rest.has_more);

    // A v1 integer cursor still works as a rowid bound.
    assert_eq!(TurnCursor::parse("12"), Some(TurnCursor::LegacyRowid(12)));
    assert_eq!(TurnCursor::parse("n:4"), Some(TurnCursor::At { ms: None, rowid: 4 }));
    assert_eq!(TurnCursor::parse("x:y"), None);
}
