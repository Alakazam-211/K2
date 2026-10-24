//! `vault.db` schema (own file, own `PRAGMA user_version`, §6).
//!
//! Every value is ciphertext. Every row carries a MAC under a VDK subkey;
//! the header MAC is keyed from the KEK (VS7). The audit table is an
//! append-only MAC chain (VS8).

use rusqlite::Connection;

use crate::error::Result;

pub const SCHEMA_VERSION: i64 = 1;

const V1: &str = r#"
CREATE TABLE IF NOT EXISTS header (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    format TEXT NOT NULL,
    vault_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    gen INTEGER NOT NULL,
    box_wrap BLOB NOT NULL,
    wraps TEXT NOT NULL,
    recovery_state TEXT NOT NULL,
    recovery_confirmed_at TEXT,
    confirm_hash TEXT,
    carry_recipient TEXT,
    audit_seq INTEGER NOT NULL,
    audit_mac BLOB NOT NULL,
    header_mac BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS secret (
    id TEXT PRIMARY KEY,
    namespace TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    provider TEXT NOT NULL,
    external_ref TEXT NOT NULL,
    allowed_hosts TEXT NOT NULL,
    inject TEXT NOT NULL,
    default_delivery TEXT NOT NULL,
    delivery_ceiling TEXT NOT NULL,
    origin TEXT NOT NULL,
    created_by TEXT NOT NULL,
    human_given INTEGER NOT NULL,
    latest_version INTEGER NOT NULL,
    status TEXT NOT NULL,
    rotate_every_days INTEGER,
    description TEXT NOT NULL,
    expires_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    row_mac BLOB NOT NULL,
    UNIQUE (namespace, scope_kind, scope_id, name)
);

CREATE TABLE IF NOT EXISTS secret_version (
    secret_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL,
    state TEXT NOT NULL,
    revoked_at TEXT,
    row_mac BLOB NOT NULL,
    PRIMARY KEY (secret_id, version)
);

CREATE TABLE IF NOT EXISTS grants (
    id TEXT PRIMARY KEY,
    secret_id TEXT NOT NULL,
    version_selector TEXT NOT NULL,
    consumer_kind TEXT NOT NULL,
    consumer_id TEXT NOT NULL,
    delivery TEXT NOT NULL,
    env_name TEXT,
    file_name TEXT,
    hosts_narrowed TEXT,
    granted_by TEXT NOT NULL,
    granted_at TEXT NOT NULL,
    expires_at TEXT,
    row_mac BLOB NOT NULL,
    UNIQUE (secret_id, consumer_kind, consumer_id, delivery)
);
CREATE INDEX IF NOT EXISTS grants_consumer ON grants (consumer_kind, consumer_id);

CREATE TABLE IF NOT EXISTS secret_request (
    card_id TEXT PRIMARY KEY,
    conversation TEXT NOT NULL,
    session_id TEXT,
    workspace_id TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    proposed_scope TEXT NOT NULL,
    proposed_hosts TEXT NOT NULL,
    proposed_delivery TEXT NOT NULL,
    prompt TEXT NOT NULL,
    ttl_secs INTEGER,
    status TEXT NOT NULL,
    answered_by TEXT,
    secret_id TEXT,
    created_at TEXT NOT NULL,
    answered_at TEXT,
    row_mac BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS placeholder (
    token_hash TEXT PRIMARY KEY,
    secret_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE INDEX IF NOT EXISTS placeholder_session ON placeholder (session_id);

CREATE TABLE IF NOT EXISTS audit (
    seq INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    secret_id TEXT,
    version INTEGER,
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    consumer TEXT,
    target_host TEXT,
    outcome TEXT NOT NULL,
    count INTEGER NOT NULL,
    prev_mac BLOB NOT NULL,
    row_mac BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_secret ON audit (secret_id, seq);
"#;

/// Agent-to-agent secret handoff (prd-agent-secret-handoff-v1 §12):
/// four additive tables, created on every open. `user_version` stays 1
/// (H22), so a 0.46.0 daemon still opens a vault that has them (it never
/// reads them). Every row carries a MAC under the row key.
///
/// Deviation from the PRD sketch: `handoff` is keyed by (delivery id,
/// direction), so a same-server send keeps its `out` row and its `in` row
/// in one vault; `card` holds the Thread card the pending copy posted.
pub const HANDOFF_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS handoff (
    delivery_id TEXT NOT NULL,
    direction TEXT NOT NULL,
    status TEXT NOT NULL,
    local_workspace_id TEXT NOT NULL,
    peer_addr TEXT NOT NULL,
    peer_fp TEXT,
    sender_actor TEXT NOT NULL,
    source_vault_id TEXT,
    name TEXT NOT NULL,
    landed_name TEXT,
    kind TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    reply_to TEXT,
    item_json TEXT NOT NULL,
    secret_id TEXT,
    version INTEGER,
    reason TEXT,
    card TEXT,
    created_at TEXT NOT NULL,
    decided_at TEXT,
    decided_by TEXT,
    expires_at TEXT,
    row_mac BLOB NOT NULL,
    PRIMARY KEY (delivery_id, direction)
);
CREATE INDEX IF NOT EXISTS handoff_ws ON handoff (local_workspace_id, status);

CREATE TABLE IF NOT EXISTS handoff_value (
    delivery_id TEXT PRIMARY KEY,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    row_mac BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS handoff_accept (
    workspace_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    added_by TEXT NOT NULL,
    added_at TEXT NOT NULL,
    row_mac BLOB NOT NULL,
    PRIMARY KEY (workspace_id, sender)
);

CREATE TABLE IF NOT EXISTS peer_vault (
    peer_fp TEXT PRIMARY KEY,
    vault_id TEXT NOT NULL,
    recipient TEXT NOT NULL,
    features TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    row_mac BLOB NOT NULL
);
"#;

/// Where a secret's value has been placed outside the vault
/// (`prd-vault-gaps-v1` item 4, 0.46.4): `systemd_cred` rows K2 wrote
/// through the root helper (`k2 vault systemd-cred`), `manual` rows a person
/// or agent noted by hand (`k2 vault placement add`). Names and places only,
/// never a value. Additive (H22): `user_version` stays 1, and an older
/// daemon never reads it. Rows carry a MAC under the row key.
pub const PLACEMENT_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS placement (
    id TEXT PRIMARY KEY,
    secret_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    target TEXT NOT NULL,
    unit TEXT,
    cred_name TEXT,
    where_text TEXT,
    note TEXT NOT NULL DEFAULT '',
    restart INTEGER NOT NULL DEFAULT 0,
    placed_by TEXT NOT NULL,
    placed_at TEXT NOT NULL,
    written_at TEXT,
    row_mac BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS placement_secret ON placement (secret_id);
"#;

/// `agent` delivery (`prd-vault-gaps-v1` item 3). Additive (H22):
/// `user_version` stays 1, and an older daemon never reads them. Rows carry
/// a MAC under the row key.
/// - `agent_policy` (R4): per-secret rate limits and forwarded signing. No
///   row = the defaults (burst 60/s, 240/min, forwarded refused).
///   `audit_seq` binds a row to the audit entry that wrote it (freshness).
/// - `agent_socket`: the daemon's record of each live agent socket; the
///   only thing a restarted daemon re-serves or removes a socket from.
pub const AGENT_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS agent_policy (
    secret_id TEXT PRIMARY KEY,
    burst INTEGER NOT NULL,
    per_minute INTEGER NOT NULL,
    allow_forwarded INTEGER NOT NULL DEFAULT 0,
    updated_by TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    audit_seq INTEGER NOT NULL,
    row_mac BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_socket (
    tag TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    secrets_dir TEXT NOT NULL,
    sock_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    row_mac BLOB NOT NULL
);
"#;

pub fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "secure_delete", "ON")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(crate::error::VaultError::new(
            "vault_too_new",
            500,
            format!("vault.db is schema {v}; this K2 understands up to {SCHEMA_VERSION}. Update K2."),
        ));
    }
    if v < 1 {
        conn.execute_batch(V1)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    // Additive tables (H22): no version bump.
    conn.execute_batch(HANDOFF_TABLES)?;
    conn.execute_batch(PLACEMENT_TABLES)?;
    conn.execute_batch(AGENT_TABLES)?;
    Ok(())
}
