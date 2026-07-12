-- K2 Mail — OAuth device-flow / loopback linked inboxes
-- (prd-email-oauth-providers-v1, slice O1). Lets a user connect a Gmail
-- or Microsoft 365 account to Email Link over OAuth 2.0 (no password),
-- read + save reply drafts exactly like an app-password IMAP inbox.
--
-- Three additive columns on `mail_external_inboxes`:
--   * auth_kind        — 'password' (today's app-password path) or
--                        'oauth' (device-code / loopback token path).
--                        Default 'password' so every EXISTING row keeps
--                        its meaning; CHECK bounds it.
--   * provider         — 'gmail' | 'microsoft' | NULL(generic IMAP).
--                        Which OAuth vendor drives refresh/endpoints.
--   * token_expires_at — unix secs; the ONLY non-secret token bit that
--                        lives in the DB, so refresh can be decided
--                        WITHOUT unvaulting. NULL for password auth.
-- Tokens themselves (access + refresh) are VAULTED under the
-- deterministic key `ext-inbox-<id>-oauth` (JSON) — never a column,
-- never a log line (mirrors the app-password `ext-inbox-<id>` key).
--
-- This migration ALSO widens the `kind` CHECK to admit 'graph' (the
-- Phase-2 Microsoft Graph REST backend, prd §12) alongside the existing
-- 'imap' / 'jmap' / 'gmail-api'. SQLite cannot ALTER a CHECK in place,
-- so the safe path is a TABLE REBUILD: create the table with the final
-- (widened CHECK + new columns) shape, copy every existing row, drop the
-- old table, rename the new one into place, and recreate the index. No
-- foreign key references this table (grants key it by inbox_id in code —
-- the 0064 idiom), so the drop is safe. The whole migration runs inside
-- ONE transaction (run_single_migration wraps it in BEGIN/COMMIT) and is
-- recorded once by NAME, so it applies exactly once.
--
-- The rebuild is chosen over `ADD COLUMN` + a separate CHECK dance
-- because it does BOTH the widen and the three adds atomically in a
-- single, easy-to-audit table definition. Existing rows land as
-- auth_kind='password', provider=NULL, token_expires_at=NULL — the
-- app-password path, unchanged.
CREATE TABLE IF NOT EXISTS mail_external_inboxes_new (
    id                 TEXT PRIMARY KEY NOT NULL,
    owner_project_id   TEXT NOT NULL,
    email_address      TEXT NOT NULL UNIQUE,
    display_name       TEXT,
    kind               TEXT NOT NULL DEFAULT 'imap'
                       CHECK (kind IN ('imap','jmap','gmail-api','graph')),
    host               TEXT NOT NULL,
    port               INTEGER NOT NULL,
    tls                TEXT NOT NULL DEFAULT 'implicit-tls'
                       CHECK (tls IN ('implicit-tls','starttls')),
    username           TEXT NOT NULL,
    drafts_folder      TEXT,
    status             TEXT NOT NULL DEFAULT 'connected'
                       CHECK (status IN ('connected','error')),
    last_checked_at    INTEGER,
    last_error         TEXT,
    created_at         INTEGER NOT NULL,
    -- 0079: the primary workspace's own access ceiling (linked default
    -- 'draft'). Carried through verbatim so the rebuild is lossless.
    primary_level      TEXT NOT NULL DEFAULT 'draft',
    smtp_host          TEXT,
    smtp_port          INTEGER,
    smtp_tls           TEXT,
    primary_can_manage INTEGER NOT NULL DEFAULT 0,
    primary_can_delete INTEGER NOT NULL DEFAULT 0,
    auth_kind          TEXT NOT NULL DEFAULT 'password'
                       CHECK (auth_kind IN ('password','oauth')),
    provider           TEXT,
    token_expires_at   INTEGER
);
--> statement-breakpoint
-- Copy every existing row + EVERY existing column (0077 base + 0079
-- primary_level + 0080 smtp_* + 0081 primary_can_*); the three new O1
-- columns take their DEFAULT / NULL. The column list is EXPLICIT (never
-- SELECT *) so the copy is lossless and stable.
INSERT INTO mail_external_inboxes_new (
    id, owner_project_id, email_address, display_name, kind, host, port,
    tls, username, drafts_folder, status, last_checked_at, last_error,
    created_at, primary_level, smtp_host, smtp_port, smtp_tls,
    primary_can_manage, primary_can_delete
)
SELECT
    id, owner_project_id, email_address, display_name, kind, host, port,
    tls, username, drafts_folder, status, last_checked_at, last_error,
    created_at, primary_level, smtp_host, smtp_port, smtp_tls,
    primary_can_manage, primary_can_delete
FROM mail_external_inboxes;
--> statement-breakpoint
DROP TABLE mail_external_inboxes;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes_new RENAME TO mail_external_inboxes;
--> statement-breakpoint
-- The read-path hot lookup: this workspace's bound inboxes (recreated
-- on the rebuilt table).
CREATE INDEX IF NOT EXISTS idx_mail_external_inboxes_owner
    ON mail_external_inboxes (owner_project_id);
