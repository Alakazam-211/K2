-- 0138: Zen Gardens v2 custom-widget grants (prd-zen-user-widgets-v2
-- UWB4, UWB5, §15.2).
--
-- One LIVE row per (garden, placement): the caps the owner allowed with a
-- click in the K2 app, the scope the widget may see, the bound rows at
-- Allow, the placement's home/agent ask, and the Sending switch. Each row
-- is signed by the daemon (HMAC-SHA256 over the `k2-zen-grant/v2`
-- canonical form) with the 32-byte key in ~/.k2/zen-grant.key; `key_id` is
-- the first 8 hex of sha256(key). A row edited by hand, or signed with
-- another key, resolves `invalid` and the widget goes back to review.
--
-- Revoking keeps the row (revoked_at set) for the audit trail; only the
-- grant routes write this table. `widget_hash`, `paused_json` and
-- `granted_by_kind` are row data, not signed: code changes keep a grant
-- (Q2), and a hand-cleared pause can't turn sending back on because
-- `sending` is signed.
CREATE TABLE IF NOT EXISTS zen_widget_grants (
    id               INTEGER PRIMARY KEY,
    garden           TEXT NOT NULL,
    placement        TEXT NOT NULL,
    -- a widget folder name, or a built-in k2:<name>@<n>
    widget           TEXT NOT NULL,
    -- JSON list of granted caps
    caps_json        TEXT NOT NULL,
    -- JSON scope, one key: agent | home | homes | allHomes | server | allServers
    scope_json       TEXT NOT NULL,
    -- JSON list of {server, room} bound at Allow (UWA8)
    entries_json     TEXT NOT NULL,
    -- JSON {home?, agent?}: the placement's ask when granted (§15.2)
    ask_json         TEXT NOT NULL,
    sending          INTEGER NOT NULL DEFAULT 1,
    -- RFC 3339 UTC
    granted_at       TEXT NOT NULL,
    -- the bundle hash the review dialog showed
    widget_hash      TEXT NOT NULL,
    key_id           TEXT NOT NULL,
    sig              TEXT NOT NULL,
    -- JSON {at, reason} while the runaway guard has sending paused
    paused_json      TEXT,
    -- 'owner_token' today (audit)
    granted_by_kind  TEXT NOT NULL,
    revoked_at       TEXT,
    -- 'owner_token' | 'pruned' | 'replaced'
    revoked_by_kind  TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS zen_widget_grants_live
    ON zen_widget_grants (garden, placement) WHERE revoked_at IS NULL;

CREATE INDEX IF NOT EXISTS zen_widget_grants_widget
    ON zen_widget_grants (widget) WHERE revoked_at IS NULL;
