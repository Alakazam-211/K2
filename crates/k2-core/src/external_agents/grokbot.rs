//! Grok Bot ("Sand", a SpaceXAI/Cursor product) adapter.
//!
//! Everything here is OBSERVED, not a published API (samples from the
//! grokbot box, 2026-10-15: `.k2/prds/research-local-grokbots-samples-2026-10-15.md`).
//! Every assumption about Grok Bot's files and endpoints lives in
//! [`schema`] so it can be corrected in one place.
//!
//! What K2 reads (daemon user's home only) — and NOTHING else in
//! `~/sand-data` (it also holds `box-secrets.json`, `host-secrets.json`,
//! `connector-secrets/`, cookie seeds, …):
//!
//! ```text
//! ~/sand-data/gateway.json                  scheme/host/port + bearer (kept in memory)
//! ~/sand-data/agents/<uuid>/profile.json    name, title, colour (offline roster)
//! ~/sand-data/agents/<uuid>/store.db        transcript_entries = the human chat (read-only)
//! ```
//!
//! Every file open goes through [`GrokBot::open_allowed`], which refuses
//! anything outside that list (pinned by a test).
//!
//! Token rules: the bearer is only sent to a LOOPBACK gateway URL, with
//! redirects and proxies off. It is never logged, never returned, and
//! never part of an error message ([`Gateway`]'s `Debug` redacts it).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value};

use super::collect::LiveSummary;
use super::{
    valid_agent_id, AgentRef, Author, Card, CardOption, CardState, Detection, ExtError,
    ExternalAgent, ExternalAgentProvider, ProviderStatus, SendReceipt, ThreadMessage, ThreadPage,
};

pub const KIND: &str = "grokbot";
pub const LABEL: &str = "Grok Bot";

/// Longest message K2 will send in one prompt.
pub const MAX_SEND_BYTES: usize = 32 * 1024;
/// Most lines one thread page returns.
pub const MAX_PAGE: usize = 500;

/// Every assumption K2 makes about Grok Bot's internals. Correct these
/// when Grok Bot's internals move; nothing else in this file names a Grok
/// Bot file, key or endpoint.
pub mod schema {
    /// Data folder under the daemon user's home.
    pub const DATA_DIR: &str = "sand-data";
    /// Install marker: `{port, pid, startedAt, scheme, host, token}`.
    pub const GATEWAY_FILE: &str = "gateway.json";
    pub const GATEWAY_SCHEME_KEY: &str = "scheme";
    pub const GATEWAY_HOST_KEY: &str = "host";
    pub const GATEWAY_PORT_KEY: &str = "port";
    pub const GATEWAY_TOKEN_KEY: &str = "token";
    /// Used only when `gateway.json` names no scheme / host.
    pub const DEFAULT_SCHEME: &str = "http";
    pub const DEFAULT_HOST: &str = "127.0.0.1";

    /// `GET`, no auth: `{ok, isBusy, activeAgentId, lastBusyAtMs, …}` —
    /// box-wide.
    pub const HEALTH_PATH: &str = "/health";
    /// `POST` + bearer, body `{}`: a JSON ARRAY of LIVE agents with
    /// per-agent status. The roster. Disk folders can be empty shells.
    pub const LIST_AGENTS_PATH: &str = "/api/listAgents";
    /// `POST` + bearer: `{prompt, agentId, clientNonce}`. Runs a billed
    /// turn. UNTESTED by Grok Bot: the response body and what happens when
    /// the agent is busy. K2 treats any 2xx as accepted and shows the reply
    /// when it lands in `store.db` (a stale agent: collected from the roster).
    pub const SEND_PROMPT_PATH: &str = "/api/sendPrompt";
    /// Whether K2 lets the human send while the agent is running a turn.
    /// Unknown on Grok Bot's side (queued / refused / interrupted), so K2
    /// allows it and the client shows a gentle "it's busy" note.
    pub const SEND_WHILE_BUSY_ALLOWED: bool = true;

    /// `listAgents` item keys.
    pub const AGENT_ID: &str = "id";
    pub const AGENT_NAME: &str = "name";
    pub const AGENT_TITLE: &str = "title";
    pub const AGENT_AVATAR: &str = "avatarDataUrl";
    pub const AGENT_COLOR: &str = "avatarColor";
    /// Working: either is true.
    pub const AGENT_WORKING_KEYS: &[&str] = &["isRunningTurn", "isComposingMessage"];
    /// Needs you: non-null.
    pub const AGENT_AWAITING: &str = "awaitingUserResponse";
    pub const AGENT_UNREAD: &str = "unreadCount";
    /// Hidden in Grok Bot's own sidebar: K2 hides it too.
    pub const AGENT_HIDDEN: &str = "isHiddenFromSidebar";
    /// Chat change marker: K2 re-reads `store.db` only when these move.
    pub const AGENT_REV_KEYS: &[&str] = &["snapshotEpoch", "snapshotSeq", "newestEntryId"];
    /// Latest activity anywhere vs on this computer (ms since epoch). An
    /// agent whose live conversation runs elsewhere (another box, the
    /// cloud) has `lastActivityAt` ahead of `localActivityAt` (seen on a
    /// real box 2026-10-15; the gateway has no transcript endpoint).
    pub const AGENT_LAST_ACTIVITY: &str = "lastActivityAt";
    pub const AGENT_LOCAL_ACTIVITY: &str = "localActivityAt";
    /// The newest transcript entry's id (`transcript_entries.id`).
    pub const AGENT_NEWEST_ENTRY: &str = "newestEntryId";
    /// One-line preview of the latest message, current even when stale.
    pub const AGENT_LAST_PREVIEW: &str = "lastMessagePreview";
    /// The latest message's id: the collected line's entry id (falls back to
    /// [`AGENT_NEWEST_ENTRY`]).
    pub const AGENT_LAST_MESSAGE_ID: &str = "lastMessageId";
    /// `{kind, text}` of the latest entry. `text` is the collected line
    /// (as much as the gateway gives; unverified whether it is ever cut).
    pub const AGENT_LAST_ENTRY: &str = "lastEntry";
    pub const LAST_ENTRY_KIND: &str = "kind";
    pub const LAST_ENTRY_TEXT: &str = "text";
    /// Who wrote the latest message (null in every sample so far). Equal to
    /// the agent's own id → the agent.
    pub const AGENT_LAST_AUTHOR: &str = "lastMessageAuthorId";
    /// `lastEntry.kind` values that are the agent writing (anything else
    /// that is a chat kind reads as the human).
    pub const AGENT_ENTRY_KINDS: &[&str] = &["send-message", "assistant", "agent"];
    pub const HUMAN_ENTRY_KINDS: &[&str] = &["message", "user", "human"];
    /// `lastEntry.kind` values that are not chat lines: their `text` is not
    /// shown; the preview is used instead.
    pub const NON_CHAT_ENTRY_KINDS: &[&str] = &["tool-call", "event", "spend-initiation", "feedback"];
    /// Snapshot counter (gap detection within one epoch).
    pub const AGENT_SNAPSHOT_EPOCH: &str = "snapshotEpoch";
    pub const AGENT_SNAPSHOT_SEQ: &str = "snapshotSeq";
    /// `lastActivityAt` may lead `localActivityAt` by this much before the
    /// agent counts as stale (clock skew, a write in flight).
    pub const STALE_TOLERANCE_MS: i64 = 60_000;
    /// `transcript_entries.id`, for the newest-entry lookup.
    pub const ID_COLUMN: &str = "id";

    /// Per-agent folder and the only two files K2 opens in it.
    pub const AGENTS_DIR: &str = "agents";
    pub const PROFILE_FILE: &str = "profile.json";
    pub const STORE_FILE: &str = "store.db";
    /// SQLite's own sidecars of `store.db` (stat only, for change checks).
    pub const STORE_WAL_FILE: &str = "store.db-wal";

    /// `transcript_entries(seq INTEGER PRIMARY KEY, id TEXT UNIQUE,
    /// entry TEXT /* JSON */) STRICT`. `seq` is K2's paging cursor.
    pub const TRANSCRIPT_TABLE: &str = "transcript_entries";
    pub const SEQ_COLUMN: &str = "seq";
    pub const ENTRY_COLUMN: &str = "entry";
    /// Rows the app's own chat index skips (`idx_transcript_window`).
    pub const SKIP_WHERE: &str = "COALESCE(json_extract(entry, '$.kind'), '') != 'tool-call' \
         AND COALESCE(json_extract(entry, '$.branched'), 0) != 1";

    /// `entry.kind` of a chat line: `message` (role user/assistant, maybe
    /// with `fromAgent` / `toAgent`) or `send-message` (the agent writing
    /// to the human). Everything else (`event`, `spend-initiation`,
    /// `feedback`, …) is not shown.
    pub const KIND_MESSAGE: &str = "message";
    pub const KIND_SEND_MESSAGE: &str = "send-message";
    pub const ROLE_USER: &str = "user";
    pub const ROLE_ASSISTANT: &str = "assistant";
    /// `send-message.message.type` K2 can show as text. Other types
    /// (`widget`, `connector`, `secret-request`, `bot-template-share`) show
    /// as "Open in Grok Bot".
    pub const SEND_MESSAGE_TEXT_TYPE: &str = "text";
    /// Non-text `send-message` types K2 draws as read-only cards
    /// (shapes 2026-10-15). Answered in Grok Bot only.
    pub const SEND_MESSAGE_WIDGET_TYPE: &str = "widget";
    pub const SEND_MESSAGE_CONNECTOR_TYPE: &str = "connector";
    pub const SEND_MESSAGE_SECRET_TYPE: &str = "secret-request";
    pub const SEND_MESSAGE_TEMPLATE_TYPE: &str = "bot-template-share";
    /// Entry-level (outside `message`) response flags.
    pub const WIDGET_RESPONDED: &str = "respondedValue";
    pub const WIDGET_SKIPPED: &str = "widgetSkipped";
    pub const WIDGET_DISMISSED: &str = "widgetDismissed";
    pub const SECRET_PROVIDED: &str = "secretProvided";
    pub const TO_AGENT: &str = "toAgent";
    pub const FROM_AGENT: &str = "fromAgent";
    pub const TIMESTAMP: &str = "timestampMs";
    pub const CLIENT_NONCE: &str = "clientNonce";
    /// Text prefix of host-injected prompts that are not human-visible.
    pub const HIDDEN_PREFIX: &str = "[SAND_HIDDEN_PROMPT]";

    /// K2's mark on the `clientNonce` it sends: still a valid UUID
    /// (Grok Bot's own clients send UUIDs), whose first group starts with
    /// `6b32` (ASCII "k2"). Rows carrying it are shown as "sent from K2".
    pub const K2_NONCE_MARK: &str = "6b32";
}

/// Gateway location + bearer. `Debug` never prints the token.
#[derive(Clone)]
pub struct Gateway {
    base: String,
    token: Option<String>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("base", &self.base)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Gateway {
    pub fn base(&self) -> &str {
        &self.base
    }
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }
}

/// The Grok Bot provider for one data folder (the daemon user's
/// `~/sand-data`).
#[derive(Debug, Clone)]
pub struct GrokBot {
    root: PathBuf,
    http_timeout: Duration,
    send_timeout: Duration,
}

impl GrokBot {
    /// The daemon OS user's own Grok Bot. Never another user's home.
    pub fn for_current_user() -> Self {
        let root = dirs::home_dir().unwrap_or_default().join(schema::DATA_DIR);
        Self {
            root,
            http_timeout: Duration::from_secs(3),
            send_timeout: Duration::from_secs(20),
        }
    }

    pub fn data_path(&self) -> &Path {
        &self.root
    }

    fn agents_dir(&self) -> PathBuf {
        self.root.join(schema::AGENTS_DIR)
    }

    fn agent_dir(&self, id: &str) -> Option<PathBuf> {
        valid_agent_id(id).then(|| self.agents_dir().join(id))
    }

    pub fn installed(&self) -> bool {
        self.root.join(schema::GATEWAY_FILE).is_file()
    }

    /// The ONE door for opening files under `sand-data`: `gateway.json`,
    /// `agents/<id>/profile.json` and `agents/<id>/store.db`. Anything else
    /// is refused, so a future edit can't wander into Grok Bot's secrets.
    fn open_allowed(&self, path: &Path) -> std::io::Result<PathBuf> {
        let rel = path.strip_prefix(&self.root).map_err(|_| denied())?;
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        let ok = match parts.as_slice() {
            [f] => *f == schema::GATEWAY_FILE,
            [a, id, f] => {
                *a == schema::AGENTS_DIR
                    && valid_agent_id(id)
                    && (*f == schema::PROFILE_FILE || *f == schema::STORE_FILE)
            }
            _ => false,
        };
        if !ok || parts.len() != rel.iter().count() {
            return Err(denied());
        }
        #[cfg(any(test, feature = "test-util"))]
        fixture::record_open(path);
        Ok(path.to_path_buf())
    }

    fn read_json(&self, path: &Path) -> Option<Map<String, Value>> {
        let p = self.open_allowed(path).ok()?;
        let bytes = std::fs::read(p).ok()?;
        match serde_json::from_slice::<Value>(&bytes).ok()? {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Parse `gateway.json`. `None` when missing, unreadable, or when the
    /// URL is not loopback (the token never leaves this computer).
    pub fn gateway(&self) -> Option<Gateway> {
        let obj = self.read_json(&self.root.join(schema::GATEWAY_FILE))?;
        let scheme = str_of(&obj, schema::GATEWAY_SCHEME_KEY).unwrap_or(schema::DEFAULT_SCHEME);
        let host = str_of(&obj, schema::GATEWAY_HOST_KEY).unwrap_or(schema::DEFAULT_HOST);
        // A gateway listening on every address (`0.0.0.0` / `::`, seen on a
        // real box 2026-10-15) is dialled on this computer's loopback.
        let host = match host {
            "0.0.0.0" => "127.0.0.1",
            "::" | "[::]" => "::1",
            h => h,
        };
        let port = obj.get(schema::GATEWAY_PORT_KEY).and_then(Value::as_u64)?;
        if port == 0 || port > 65_535 {
            return None;
        }
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let base = format!("{scheme}://{host}:{port}");
        if !is_loopback_http(&base) {
            return None;
        }
        let token = str_of(&obj, schema::GATEWAY_TOKEN_KEY).map(str::to_string);
        Some(Gateway { base, token })
    }

    fn client(&self, timeout: Duration) -> Result<reqwest::blocking::Client, ExtError> {
        reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_millis(800))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ExtError::Unreachable("http client".into()))
    }

    /// The disk roster from `agents/<uuid>/profile.json`, sorted by name.
    /// Folders without a profile are empty shells and are skipped.
    pub fn disk_agents(&self) -> Vec<ExternalAgent> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.agents_dir()) else {
            return out;
        };
        for ent in rd.flatten() {
            let Some(id) = ent.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !valid_agent_id(&id) || !ent.path().is_dir() {
                continue;
            }
            let Some(profile) = self.read_json(&ent.path().join(schema::PROFILE_FILE)) else {
                continue;
            };
            out.push(ExternalAgent {
                kind: KIND,
                name: str_of(&profile, schema::AGENT_NAME)
                    .map(str::to_string)
                    .unwrap_or_else(|| fallback_name_for(&id)),
                title: str_of(&profile, schema::AGENT_TITLE).map(str::to_string),
                avatar: None,
                color: str_of(&profile, schema::AGENT_COLOR).map(str::to_string),
                source: "disk",
                working: false,
                needs_you: false,
                unread_count: 0,
                stale: false,
                last_message_preview: None,
                last_activity_at: None,
                local_activity_at: None,
                rev: String::new(),
                live: None,
                id,
            });
        }
        sort_agents(&mut out);
        out
    }

    /// The live roster from `listAgents`.
    pub fn gateway_agents(&self, gw: &Gateway) -> Result<Vec<ExternalAgent>, ExtError> {
        let token = gw
            .token
            .as_deref()
            .ok_or_else(|| ExtError::Unreachable("gateway.json has no token".into()))?;
        let resp = self
            .client(self.http_timeout)?
            .post(format!("{}{}", gw.base, schema::LIST_AGENTS_PATH))
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{}")
            .send()
            .map_err(|e| ExtError::Unreachable(redact_err(&e)))?;
        if !resp.status().is_success() {
            return Err(ExtError::Unreachable(format!(
                "listAgents HTTP {}",
                resp.status().as_u16()
            )));
        }
        let v: Value = resp
            .bytes()
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| ExtError::Protocol("listAgents body".into()))?;
        let items = v
            .as_array()
            .ok_or_else(|| ExtError::Protocol("listAgents shape".into()))?;
        let mut out = Vec::new();
        for it in items {
            let Some(o) = it.as_object() else { continue };
            let Some(id) = str_of(o, schema::AGENT_ID).map(str::to_string) else {
                continue;
            };
            if !valid_agent_id(&id)
                || o.get(schema::AGENT_HIDDEN).and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            let rev = schema::AGENT_REV_KEYS
                .iter()
                .map(|k| o.get(*k).map(|v| v.to_string()).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("|");
            let last_activity_at = o.get(schema::AGENT_LAST_ACTIVITY).and_then(epoch_ms);
            let local_activity_at = o.get(schema::AGENT_LOCAL_ACTIVITY).and_then(epoch_ms);
            let newest = str_of(o, schema::AGENT_NEWEST_ENTRY);
            let stale = activity_is_elsewhere(last_activity_at, local_activity_at)
                || newest.is_some_and(|e| {
                    newest_entry_settled(last_activity_at, now_ms())
                        && self.store_has_entry(&id, e) == Some(false)
                });
            let live = live_summary(&id, o, last_activity_at);
            out.push(ExternalAgent {
                kind: KIND,
                name: str_of(o, schema::AGENT_NAME)
                    .map(str::to_string)
                    .unwrap_or_else(|| fallback_name_for(&id)),
                title: str_of(o, schema::AGENT_TITLE).map(str::to_string),
                avatar: str_of(o, schema::AGENT_AVATAR)
                    .filter(|a| is_inline_image(a))
                    .map(str::to_string),
                color: str_of(o, schema::AGENT_COLOR).map(str::to_string),
                source: "gateway",
                working: schema::AGENT_WORKING_KEYS
                    .iter()
                    .any(|k| o.get(*k).and_then(Value::as_bool) == Some(true)),
                needs_you: o.get(schema::AGENT_AWAITING).is_some_and(|v| !v.is_null()),
                unread_count: o
                    .get(schema::AGENT_UNREAD)
                    .and_then(Value::as_u64)
                    .map(|n| n.min(u32::MAX as u64) as u32)
                    .unwrap_or(0),
                stale,
                last_message_preview: str_of(o, schema::AGENT_LAST_PREVIEW).and_then(clean_preview),
                last_activity_at,
                local_activity_at,
                rev,
                live: Some(live),
                id,
            });
        }
        sort_agents(&mut out);
        Ok(out)
    }

    fn open_store(&self, agent_id: &str) -> Result<Option<Connection>, ExtError> {
        let dir = self
            .agent_dir(agent_id)
            .ok_or_else(|| ExtError::BadInput("invalid agent id".into()))?;
        if !dir.is_dir() {
            return Err(ExtError::UnknownAgent(agent_id.to_string()));
        }
        let db = dir.join(schema::STORE_FILE);
        if !db.is_file() {
            return Ok(None);
        }
        let db = self
            .open_allowed(&db)
            .map_err(|_| ExtError::Protocol("store path refused".into()))?;
        // Read-only (SQLite `mode=ro`): K2 never takes a write lock on Grok
        // Bot's store, so the host's writer is never blocked.
        let conn = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| ExtError::Protocol("open store".into()))?;
        let _ = conn.busy_timeout(Duration::from_millis(250));
        let _ = conn.pragma_update(None, "query_only", true);
        Ok(Some(conn))
    }

    /// Whether `entry_id` is in the agent's local `store.db`. None when
    /// K2 can't tell (no store, unknown layout, unreadable). A hit is
    /// remembered (an entry never leaves), so a current agent costs one
    /// lookup per new entry, not one per poll.
    pub fn store_has_entry(&self, agent_id: &str, entry_id: &str) -> Option<bool> {
        type Key = (PathBuf, String, String);
        static FOUND: std::sync::Mutex<Option<std::collections::HashSet<Key>>> =
            std::sync::Mutex::new(None);
        let key = (self.root.clone(), agent_id.to_string(), entry_id.to_string());
        if FOUND
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|f| f.contains(&key))
        {
            return Some(true);
        }
        let conn = self.open_store(agent_id).ok()??;
        if !transcript_layout_known(&conn) {
            return None;
        }
        let found = match conn.query_row(
            &format!(
                "SELECT 1 FROM {} WHERE {} = ?1 LIMIT 1",
                schema::TRANSCRIPT_TABLE,
                schema::ID_COLUMN
            ),
            [entry_id],
            |_| Ok(()),
        ) {
            Ok(()) => true,
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(_) => return None,
        };
        if found {
            FOUND
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(Default::default)
                .insert(key);
        }
        Some(found)
    }

    /// The highest `store.db` cursor right now (the collection start mark:
    /// K2 collects only rows after it).
    pub fn store_max_seq(&self, agent_id: &str) -> Result<StoreRead<i64>, ExtError> {
        let Some(conn) = self.open_store(agent_id)? else {
            return Ok(StoreRead::NoStore);
        };
        if !transcript_layout_known(&conn) {
            return Ok(StoreRead::Unsupported(UNKNOWN_LAYOUT.into()));
        }
        match conn.query_row(
            &format!(
                "SELECT COALESCE(MAX({}), 0) FROM {}",
                schema::SEQ_COLUMN,
                schema::TRANSCRIPT_TABLE
            ),
            [],
            |r| r.get::<_, i64>(0),
        ) {
            Ok(n) => Ok(StoreRead::Ok(n)),
            Err(_) => Ok(StoreRead::Unsupported(UNREADABLE.into())),
        }
    }

    /// Human-visible-candidate rows after cursor `after`, ascending.
    /// `newest = Some(n)`: only the newest `n` of them (the window K2
    /// re-reads for lines Grok Bot updates in place); else the oldest
    /// `limit`.
    pub fn store_lines(
        &self,
        agent_id: &str,
        after: i64,
        newest: Option<usize>,
        limit: usize,
    ) -> Result<StoreRead<Vec<StoreLine>>, ExtError> {
        let Some(conn) = self.open_store(agent_id)? else {
            return Ok(StoreRead::NoStore);
        };
        if !transcript_layout_known(&conn) {
            return Ok(StoreRead::Unsupported(UNKNOWN_LAYOUT.into()));
        }
        let has_id = table_columns(&conn).iter().any(|c| c == schema::ID_COLUMN);
        let id_sel = if has_id {
            format!("CAST({} AS TEXT)", schema::ID_COLUMN)
        } else {
            "NULL".to_string()
        };
        let (order, n) = match newest {
            Some(n) => ("DESC", n),
            None => ("ASC", limit),
        };
        let sql = format!(
            "SELECT {seq}, {id_sel}, CAST({entry} AS TEXT) FROM {t} WHERE {skip} AND {seq} > ?1 \
             ORDER BY {seq} {order} LIMIT ?2",
            seq = schema::SEQ_COLUMN,
            entry = schema::ENTRY_COLUMN,
            t = schema::TRANSCRIPT_TABLE,
            skip = schema::SKIP_WHERE,
        );
        let rows: rusqlite::Result<Vec<(i64, Option<String>, String)>> = conn
            .prepare(&sql)
            .and_then(|mut stmt| {
                stmt.query_map(rusqlite::params![after, n.max(1) as i64], |r| {
                    Ok((
                        r.get(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })?
                .collect()
            });
        let Ok(mut rows) = rows else {
            return Ok(StoreRead::Unsupported(UNREADABLE.into()));
        };
        if newest.is_some() {
            rows.reverse();
        }
        Ok(StoreRead::Ok(
            rows.into_iter()
                .map(|(seq, id, entry)| {
                    let client_nonce = serde_json::from_str::<Value>(&entry)
                        .ok()
                        .and_then(|v| {
                            v.get(schema::CLIENT_NONCE)
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        });
                    StoreLine {
                        seq,
                        entry_id: id
                            .filter(|i| !i.is_empty())
                            .unwrap_or_else(|| format!("seq-{seq}")),
                        client_nonce,
                        message: entry_to_message(seq, &entry),
                    }
                })
                .collect(),
        ))
    }

    /// Cheap stat-based change marker for an agent's chat (size + mtime of
    /// `store.db` and its WAL). The secondary change path while the gateway
    /// is down; opens no file.
    pub fn store_fingerprint(&self, agent_id: &str) -> Option<String> {
        let mut parts = Vec::new();
        for p in self.watch_paths(agent_id) {
            if let Ok(m) = std::fs::metadata(&p) {
                let mt = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                parts.push(format!("{}:{mt}", m.len()));
            }
        }
        (!parts.is_empty()).then(|| parts.join("|"))
    }
}

impl ExternalAgentProvider for GrokBot {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn label(&self) -> &'static str {
        LABEL
    }

    fn detect(&self) -> Detection {
        let installed = self.installed();
        Detection {
            installed,
            data_path: self.root.display().to_string(),
            agents: if installed {
                self.disk_agents()
            } else {
                Vec::new()
            },
        }
    }

    fn list_agents(&self) -> Result<Vec<ExternalAgent>, ExtError> {
        if !self.installed() {
            return Err(ExtError::NotInstalled);
        }
        match self.gateway().map(|gw| self.gateway_agents(&gw)) {
            Some(Ok(list)) => Ok(list),
            _ => Ok(self.disk_agents()),
        }
    }

    fn status(&self) -> ProviderStatus {
        let Some(gw) = self.gateway() else {
            return ProviderStatus::default();
        };
        let Ok(client) = self.client(self.http_timeout) else {
            return ProviderStatus::default();
        };
        let Ok(resp) = client
            .get(format!("{}{}", gw.base, schema::HEALTH_PATH))
            .send()
        else {
            return ProviderStatus::default();
        };
        if !resp.status().is_success() {
            return ProviderStatus::default();
        }
        let Some(v) = resp
            .bytes()
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            return ProviderStatus {
                reachable: true,
                ..Default::default()
            };
        };
        let busy = v.get("isBusy").and_then(Value::as_bool).unwrap_or(false);
        let active = v
            .get("activeAgentId")
            .and_then(Value::as_str)
            .filter(|s| valid_agent_id(s))
            .map(str::to_string);
        ProviderStatus {
            reachable: v.get("ok").and_then(Value::as_bool).unwrap_or(true),
            busy,
            active_agent_id: if busy { active } else { None },
            last_busy_at_ms: v.get("lastBusyAtMs").and_then(Value::as_i64),
        }
    }

    fn read_thread(
        &self,
        agent_id: &str,
        since: Option<i64>,
        limit: usize,
    ) -> Result<ThreadPage, ExtError> {
        let limit = limit.clamp(1, MAX_PAGE);
        let since_v = since.unwrap_or(0);
        let Some(conn) = self.open_store(agent_id)? else {
            return Ok(ThreadPage {
                supported: true,
                reason: None,
                messages: Vec::new(),
                next_since: since_v,
                has_more_before: false,
            });
        };
        if !transcript_layout_known(&conn) {
            return Ok(ThreadPage::unsupported(
                "Grok Bot's chat store has a layout K2 doesn't know yet",
                since_v,
            ));
        }
        let rows = match fetch_rows(&conn, since, limit) {
            Ok(r) => r,
            Err(_) => {
                return Ok(ThreadPage::unsupported(
                    "Grok Bot's chat store could not be read",
                    since_v,
                ))
            }
        };
        let (rows, has_more_before) = if since.is_none() && rows.len() > limit {
            (rows[rows.len() - limit..].to_vec(), true)
        } else {
            (rows, false)
        };
        let next_since = rows
            .iter()
            .map(|r| r.0)
            .max()
            .unwrap_or(since_v)
            .max(since_v);
        let messages: Vec<ThreadMessage> = rows
            .into_iter()
            .filter_map(|(seq, entry)| entry_to_message(seq, &entry))
            .collect();
        Ok(ThreadPage {
            supported: true,
            reason: None,
            messages,
            next_since,
            has_more_before,
        })
    }

    fn thread_cursor(&self, agent_id: &str) -> Option<i64> {
        let conn = self.open_store(agent_id).ok()??;
        conn.query_row(
            &format!(
                "SELECT COALESCE(MAX({}), 0) FROM {}",
                schema::SEQ_COLUMN,
                schema::TRANSCRIPT_TABLE
            ),
            [],
            |r| r.get::<_, i64>(0),
        )
        .ok()
    }

    fn watch_paths(&self, agent_id: &str) -> Vec<PathBuf> {
        match self.agent_dir(agent_id) {
            Some(d) => vec![d.join(schema::STORE_FILE), d.join(schema::STORE_WAL_FILE)],
            None => Vec::new(),
        }
    }

    fn send(&self, agent_id: &str, text: &str) -> Result<SendReceipt, ExtError> {
        if !valid_agent_id(agent_id) {
            return Err(ExtError::BadInput("invalid agent id".into()));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(ExtError::BadInput("message is empty".into()));
        }
        if text.len() > MAX_SEND_BYTES {
            return Err(ExtError::BadInput(format!(
                "message is over {} KB",
                MAX_SEND_BYTES / 1024
            )));
        }
        if !self.installed() {
            return Err(ExtError::NotInstalled);
        }
        let gw = self
            .gateway()
            .ok_or_else(|| ExtError::Unreachable("Grok Bot's gateway is not configured".into()))?;
        let roster = self.gateway_agents(&gw)?;
        let Some(agent) = roster.iter().find(|a| a.id == agent_id) else {
            return Err(ExtError::UnknownAgent(agent_id.to_string()));
        };
        // A stale agent (live conversation elsewhere) still gets the send:
        // the gateway delivers it to the live conversation wherever it runs
        // (seen on a real box 2026-10-15: `lastActivityAt` moved 3 s after a
        // K2 send while this computer's store did not). The caller records
        // it in the bot's Thread (see `collect`).
        let agent_stale = agent.stale;
        if agent.working && !schema::SEND_WHILE_BUSY_ALLOWED {
            return Err(ExtError::BadInput("the agent is busy".into()));
        }
        let token = gw
            .token
            .as_deref()
            .ok_or_else(|| ExtError::Unreachable("gateway.json has no token".into()))?;
        let nonce = k2_nonce();
        // `text` arrives already stamped `[from <person>]` by the daemon's
        // route (who typed it); no agent envelope. The nonce makes the send
        // attributable to K2.
        let resp = self
            .client(self.send_timeout)?
            .post(format!("{}{}", gw.base, schema::SEND_PROMPT_PATH))
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(
                serde_json::json!({
                    "prompt": text,
                    "agentId": agent_id,
                    "clientNonce": nonce,
                })
                .to_string(),
            )
            .send()
            .map_err(|e| ExtError::Unreachable(redact_err(&e)))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(ExtError::Unreachable("Grok Bot refused K2's token".into()));
        }
        if !status.is_success() {
            return Err(ExtError::Protocol(format!(
                "sendPrompt HTTP {}",
                status.as_u16()
            )));
        }
        // Any 2xx is accepted (the body is untested on Grok Bot's side).
        Ok(SendReceipt {
            client_nonce: nonce,
            agent_stale,
        })
    }
}

/// A fresh `clientNonce` carrying K2's mark (see [`schema::K2_NONCE_MARK`]).
pub fn k2_nonce() -> String {
    let u = uuid::Uuid::new_v4().to_string();
    format!(
        "{}{}",
        schema::K2_NONCE_MARK,
        &u[schema::K2_NONCE_MARK.len()..]
    )
}

// ── live summary (stale agents) ─────────────────────────────────────────

/// What `listAgents` says about an agent's latest entry, for collection
/// ([`super::collect`]). Only the LATEST entry is ever visible this way.
fn live_summary(
    agent_id: &str,
    o: &Map<String, Value>,
    last_activity_at: Option<i64>,
) -> LiveSummary {
    let last = o.get(schema::AGENT_LAST_ENTRY).and_then(Value::as_object);
    let kind = last.and_then(|l| str_of(l, schema::LAST_ENTRY_KIND));
    let chat_kind = !kind.is_some_and(|k| schema::NON_CHAT_ENTRY_KINDS.contains(&k));
    let entry_text = last
        .filter(|_| chat_kind)
        .and_then(|l| str_of(l, schema::LAST_ENTRY_TEXT))
        .and_then(clean_entry_text);
    let (text, preview_only) = match entry_text {
        Some(t) => (Some(t), false),
        None => (
            str_of(o, schema::AGENT_LAST_PREVIEW).and_then(clean_preview),
            true,
        ),
    };
    let author_id = str_of(o, schema::AGENT_LAST_AUTHOR);
    let author = if author_id == Some(agent_id) {
        Author::Agent
    } else if text
        .as_deref()
        .is_some_and(|t| t.starts_with("[from "))
    {
        Author::Human
    } else {
        match kind {
            Some(k) if schema::AGENT_ENTRY_KINDS.contains(&k) => Author::Agent,
            Some(k) if schema::HUMAN_ENTRY_KINDS.contains(&k) => Author::Human,
            _ => Author::Agent,
        }
    };
    LiveSummary {
        entry_id: str_of(o, schema::AGENT_LAST_MESSAGE_ID)
            .or_else(|| str_of(o, schema::AGENT_NEWEST_ENTRY))
            .map(str::to_string),
        at_ms: last_activity_at,
        author: Some(author),
        text,
        preview_only,
        snapshot_epoch: o
            .get(schema::AGENT_SNAPSHOT_EPOCH)
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())),
        snapshot_seq: o.get(schema::AGENT_SNAPSHOT_SEQ).and_then(Value::as_i64),
    }
}

/// `lastEntry.text` as a chat line: control characters other than line
/// breaks and tabs dropped, trimmed. Hidden host prompts are dropped.
fn clean_entry_text(raw: &str) -> Option<String> {
    let t: String = raw
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let t = t.trim();
    (!t.is_empty() && !t.starts_with(schema::HIDDEN_PREFIX)).then(|| t.to_string())
}

// ── transcript reading ──────────────────────────────────────────────────

const UNKNOWN_LAYOUT: &str = "Grok Bot's chat store has a layout K2 doesn't know yet";
const UNREADABLE: &str = "Grok Bot's chat store could not be read";

/// What a read of an agent's `store.db` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreRead<T> {
    /// No `store.db` (yet) for this agent.
    NoStore,
    /// A layout K2 doesn't know, or unreadable (the reason, for people).
    Unsupported(String),
    Ok(T),
}

/// One `store.db` row after the collection mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLine {
    pub seq: i64,
    /// `transcript_entries.id` (the id `listAgents` names too), else
    /// `seq-<n>`.
    pub entry_id: String,
    /// The row's `clientNonce` (a K2 send carries K2's mark).
    pub client_nonce: Option<String>,
    /// The chat line, or None when the row is not human-visible.
    pub message: Option<ThreadMessage>,
}

fn table_columns(conn: &Connection) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({})", schema::TRANSCRIPT_TABLE))
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn transcript_layout_known(conn: &Connection) -> bool {
    let names = table_columns(conn);
    names.iter().any(|n| n == schema::SEQ_COLUMN) && names.iter().any(|n| n == schema::ENTRY_COLUMN)
}

fn fetch_rows(
    conn: &Connection,
    since: Option<i64>,
    limit: usize,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let select = format!(
        "SELECT {seq}, CAST({entry} AS TEXT) FROM {t} WHERE {skip}",
        seq = schema::SEQ_COLUMN,
        entry = schema::ENTRY_COLUMN,
        t = schema::TRANSCRIPT_TABLE,
        skip = schema::SKIP_WHERE,
    );
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(i64, String)> {
        Ok((
            r.get(0)?,
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    };
    match since {
        Some(s) => {
            let mut stmt = conn.prepare(&format!(
                "{select} AND {seq} > ?1 ORDER BY {seq} ASC LIMIT ?2",
                seq = schema::SEQ_COLUMN
            ))?;
            let rows = stmt.query_map(rusqlite::params![s, limit as i64], map)?;
            rows.collect()
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "{select} ORDER BY {seq} DESC LIMIT ?1",
                seq = schema::SEQ_COLUMN
            ))?;
            let rows = stmt.query_map(rusqlite::params![(limit + 1) as i64], map)?;
            let mut v: Vec<(i64, String)> = rows.collect::<rusqlite::Result<_>>()?;
            v.reverse();
            Ok(v)
        }
    }
}

/// Map one `entry` JSON to a chat line, or `None` when it isn't one.
fn entry_to_message(seq: i64, entry: &str) -> Option<ThreadMessage> {
    let o = match serde_json::from_str::<Value>(entry).ok()? {
        Value::Object(o) => o,
        _ => return None,
    };
    let kind = str_of(&o, "kind")?;
    let at_ms = o
        .get(schema::TIMESTAMP)
        .and_then(Value::as_i64)
        .filter(|t| *t > 0);
    let via_k2 =
        str_of(&o, schema::CLIENT_NONCE).is_some_and(|n| n.starts_with(schema::K2_NONCE_MARK));
    let mut msg = ThreadMessage {
        seq,
        author: Author::Agent,
        text: String::new(),
        at_ms,
        to_agent: o.get(schema::TO_AGENT).and_then(agent_ref),
        from_agent: o.get(schema::FROM_AGENT).and_then(agent_ref),
        via_k2,
        open_in_app: None,
        card: None,
    };
    if kind == schema::KIND_MESSAGE {
        let role = str_of(&o, "role")?;
        msg.author = if msg.from_agent.is_some() {
            Author::Peer
        } else if role == schema::ROLE_USER {
            Author::Human
        } else if role == schema::ROLE_ASSISTANT {
            Author::Agent
        } else {
            return None;
        };
        match content_text(o.get("content")) {
            Some(t) => msg.text = t,
            None if o.contains_key("richText") => msg.open_in_app = Some("rich text".into()),
            None => return None,
        }
    } else if kind == schema::KIND_SEND_MESSAGE {
        let m = o.get("message").and_then(Value::as_object)?;
        let ty = str_of(m, "type").unwrap_or("unknown");
        if ty == schema::SEND_MESSAGE_TEXT_TYPE {
            msg.text = content_text(m.get("content"))?;
        } else {
            msg.open_in_app = Some(ty.replace('-', " "));
            msg.card = card_of(ty, m, &o);
        }
    } else {
        return None;
    }
    if msg.text.starts_with(schema::HIDDEN_PREFIX) {
        return None;
    }
    Some(msg)
}

/// Longest string K2 copies into a card field.
const MAX_CARD_TEXT: usize = 2_000;

fn card_str(o: &Map<String, Value>, key: &str) -> Option<String> {
    str_of(o, key).map(|s| s.chars().take(MAX_CARD_TEXT).collect())
}

/// A read-only card for a non-text `send-message` (shapes:
/// `research-grokbot-payload-shapes-2026-10-15.md`). `m` is `message`,
/// `entry` the whole row (the human's response flags live there). Never
/// copies a secret value (the transcript holds none; K2 reads only
/// `secretProvided`).
fn card_of(ty: &str, m: &Map<String, Value>, entry: &Map<String, Value>) -> Option<Card> {
    match ty {
        schema::SEND_MESSAGE_WIDGET_TYPE => {
            let w = m.get("widget").and_then(Value::as_object)?;
            let prompt = card_str(w, "prompt")?;
            let responded = str_of(entry, schema::WIDGET_RESPONDED).map(str::to_string);
            let mut matched = false;
            let mut options = Vec::new();
            for op in w
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(12)
            {
                let Some(op) = op.as_object() else { continue };
                let Some(label) = card_str(op, "label") else {
                    continue;
                };
                let chosen = responded
                    .as_deref()
                    .is_some_and(|r| str_of(op, "value") == Some(r));
                matched |= chosen;
                options.push(CardOption {
                    label,
                    description: card_str(op, "description"),
                    chosen,
                });
            }
            let flag = |k: &str| entry.get(k).and_then(Value::as_bool) == Some(true);
            let state = if responded.is_some() {
                CardState::Answered
            } else if flag(schema::WIDGET_SKIPPED) {
                CardState::Skipped
            } else if flag(schema::WIDGET_DISMISSED) {
                CardState::Dismissed
            } else {
                CardState::Open
            };
            let custom_answer = if matched {
                None
            } else {
                responded.map(|r| r.chars().take(MAX_CARD_TEXT).collect())
            };
            Some(Card::Choice {
                prompt,
                help_text: card_str(w, "helpText"),
                options,
                state,
                custom_answer,
            })
        }
        schema::SEND_MESSAGE_CONNECTOR_TYPE => Some(Card::Connector {
            connector: card_str(m, "connector")?,
            variant: card_str(m, "variant"),
        }),
        schema::SEND_MESSAGE_SECRET_TYPE => {
            let r = m.get("secretRequest").and_then(Value::as_object)?;
            Some(Card::SecretRequest {
                label: card_str(r, "label")?,
                description: card_str(r, "description"),
                platform: r
                    .get("target")
                    .and_then(Value::as_object)
                    .and_then(|t| card_str(t, "platform")),
                provided: entry.get(schema::SECRET_PROVIDED).and_then(Value::as_bool) == Some(true),
            })
        }
        schema::SEND_MESSAGE_TEMPLATE_TYPE => Some(Card::TemplateShare {
            name: card_str(m, "name")?,
            body: card_str(m, "body"),
            share_url: str_of(m, "shareUrl")
                .filter(|u| u.starts_with("https://") && u.len() <= 2_048)
                .map(str::to_string),
        }),
        _ => None,
    }
}

fn content_text(v: Option<&Value>) -> Option<String> {
    let t = match v? {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| match p {
                Value::String(s) => Some(s.as_str()),
                Value::Object(po) => po.get("text").and_then(Value::as_str),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn agent_ref(v: &Value) -> Option<AgentRef> {
    let o = v.as_object()?;
    let id = o.get("id").and_then(Value::as_str)?.to_string();
    let name = o
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fallback_name_for(&id));
    Some(AgentRef { id, name })
}

// ── small helpers ───────────────────────────────────────────────────────

fn denied() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "path outside K2's Grok Bot allowlist",
    )
}

/// A timestamp as ms since epoch: a number (ms, or seconds when small) or
/// an RFC 3339 string. 0, negative or unparseable is None.
fn epoch_ms(v: &Value) -> Option<i64> {
    let ms = match v {
        Value::Number(n) => {
            let n = n.as_f64()?;
            if n < 100_000_000_000.0 {
                (n * 1000.0) as i64
            } else {
                n as i64
            }
        }
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
            .ok()?
            .timestamp_millis(),
        _ => return None,
    };
    (ms > 0).then_some(ms)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Stale rule 1: the agent was active more recently than this computer's
/// copy of its chat goes (beyond [`schema::STALE_TOLERANCE_MS`]).
pub fn activity_is_elsewhere(last: Option<i64>, local: Option<i64>) -> bool {
    matches!((last, local), (Some(l), Some(loc)) if l - loc > schema::STALE_TOLERANCE_MS)
}

/// Stale rule 2 (newest entry missing from `store.db`) only applies once
/// that entry has had time to land: no activity in the last
/// [`schema::STALE_TOLERANCE_MS`]. A turn being written right now never
/// reads as stale.
pub fn newest_entry_settled(last: Option<i64>, now: i64) -> bool {
    match last {
        Some(l) => now - l > schema::STALE_TOLERANCE_MS,
        None => true,
    }
}

/// Longest preview carried (chars).
pub const MAX_PREVIEW_CHARS: usize = 300;

/// A preview as plain one-line text: control characters become spaces,
/// runs of whitespace collapse, capped at [`MAX_PREVIEW_CHARS`]. Hidden
/// host prompts are dropped.
pub fn clean_preview(raw: &str) -> Option<String> {
    if raw.trim_start().starts_with(schema::HIDDEN_PREFIX) {
        return None;
    }
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') { ' ' } else { c })
        .collect();
    let one_line = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = one_line.chars().take(MAX_PREVIEW_CHARS).collect();
    let capped = capped.trim_end();
    (!capped.is_empty()).then(|| capped.to_string())
}

fn str_of<'a>(o: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    o.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn sort_agents(v: &mut [ExternalAgent]) {
    v.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
}

pub fn fallback_name_for(id: &str) -> String {
    format!("Grok Bot {}", id.chars().take(8).collect::<String>())
}

/// Largest inline avatar passed through.
const MAX_AVATAR_CHARS: usize = 512 * 1024;

fn is_inline_image(a: &str) -> bool {
    a.starts_with("data:image/") && a.len() <= MAX_AVATAR_CHARS && !a.starts_with("data:image/svg")
}

fn is_loopback_http(base: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(base) else {
        return false;
    };
    if u.scheme() != "http" && u.scheme() != "https" {
        return false;
    }
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    match u.host_str() {
        Some(h) => {
            h.eq_ignore_ascii_case("localhost")
                || h.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

/// A transport error without the request URL or any header.
fn redact_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "not running".into()
    } else {
        "request failed".into()
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod fixture;

#[cfg(test)]
mod tests;
