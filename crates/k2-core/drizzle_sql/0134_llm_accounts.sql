-- 0134: LLM login wallet (`k2 llm accounts`, Settings → LLMs).
--
-- One ACTIVE login per tool per server. The active login lives where the
-- tool always keeps it (Claude's keychain item or ~/.claude/.credentials.json,
-- ~/.codex/auth.json, ~/.grok/auth.json), so every session on the server
-- uses it and conversations never split. Every other login is a wallet
-- slot under ~/.k2/llm-accounts/<tool>/<id>/ (dir 0700, files 0600).
-- Switching saves the outgoing live login back to its slot, then writes
-- the chosen slot into the live store.
--
-- METADATA ONLY in these tables: never a token, never a refresh token,
-- never a hash of one. The writer refuses token-shaped values.
CREATE TABLE IF NOT EXISTS llm_accounts (
    id               TEXT PRIMARY KEY,
    tool             TEXT NOT NULL,
    label            TEXT NOT NULL,
    -- 'subscription' (a CLI login swapped into the live store) now;
    -- 'api_key' later (injected per session via env, never swapped).
    kind             TEXT NOT NULL DEFAULT 'subscription',
    -- order of the tool's pool; "switch to next login" walks it
    position         INTEGER NOT NULL DEFAULT 0,
    created_by       TEXT,
    created_at       INTEGER NOT NULL,
    -- the last time this login became the active one
    last_used_at     INTEGER,
    -- not_set_up | signing_in | signed_in | needs_login | unknown
    state            TEXT NOT NULL DEFAULT 'not_set_up',
    detail           TEXT,
    email            TEXT,
    org              TEXT,
    plan             TEXT,
    -- access-token expiry as the tool recorded it (unix seconds); metadata
    expires_at       INTEGER,
    -- last successful refresh of this login while it was idle in the wallet
    refreshed_at     INTEGER,
    -- last usage snapshot (same shape as /cli/usage/subscriptions rows)
    usage_json       TEXT,
    usage_checked_at INTEGER,
    removed_at       INTEGER
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS llm_accounts_live_label
    ON llm_accounts (tool, label) WHERE removed_at IS NULL;
--> statement-breakpoint
-- Which wallet entry is in the tool's live store right now. One row per
-- tool at most. Switching affects every session on this server. Kept as
-- its own pointer table so a later per-workspace pin table
-- (llm_account_pins(workspace_id, tool, account_id)) or a parallel
-- "more than one active" mode can be added beside it without touching
-- the accounts rows.
CREATE TABLE IF NOT EXISTS llm_active (
    tool        TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL,
    switched_at INTEGER NOT NULL,
    switched_by TEXT,
    -- When the pool's active login is an API key (env-injected), the
    -- subscription login still sitting in the tool's live store; NULL
    -- when account_id itself is in the live store.
    live_account_id TEXT
);
--> statement-breakpoint
-- Pins: a workspace (its canonical agent, sidecars, heartbeats, tabs)
-- or one session (v2 session key) runs on a specific login instead of
-- the pool's active one. Session pin > workspace pin > pool. A
-- subscription login that is pinned anywhere is never the pool's active
-- login (and the reverse): one login is never live in two places, since
-- refresh tokens rotate.
CREATE TABLE IF NOT EXISTS llm_account_pins (
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','session')),
    scope_id   TEXT NOT NULL,
    tool       TEXT NOT NULL,
    account_id TEXT NOT NULL,
    created_by TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (scope_kind, scope_id, tool)
);
--> statement-breakpoint
-- The login a session/conversation started under, so a resume uses the
-- same home even after pins change. conversation_id '' = the latest
-- fresh spawn on that session key (ids a CLI mints itself are adopted
-- later). account_id NULL = the tool's live home (the pool).
CREATE TABLE IF NOT EXISTS llm_session_logins (
    session_key     TEXT NOT NULL,
    tool            TEXT NOT NULL,
    conversation_id TEXT NOT NULL DEFAULT '',
    account_id      TEXT,
    home            TEXT,
    recorded_at     INTEGER NOT NULL,
    PRIMARY KEY (session_key, tool, conversation_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS llm_session_logins_conv
    ON llm_session_logins (tool, conversation_id);
