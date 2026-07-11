-- K2 Mail S9 — external assistant inboxes (prd-email-server-v1 §17.5).
-- The user's OWN external email accounts (Gmail app-password, Fastmail,
-- company IMAP), each bound to exactly ONE workspace at add time: agents
-- in that workspace READ the inbox and save reply DRAFTS into the
-- account's real Drafts folder; sending from an external account has no
-- code path at all in V1. Any other workspace sees the masked
-- `not_found` (the S3 ownership rule applies unchanged).
--
-- `email_address` is stored ONLY normalized (lowercase local part +
-- lowercase punycode A-label domain — `mail::addresses::
-- normalize_address` at every boundary, pre-mortem #14) and is UNIQUE:
-- one row per account, and the address is the §17.5 seam key
-- (`backend_for_address` routes it to the IMAP backend).
--
-- `kind` is 'imap' in V1; 'jmap' / 'gmail-api' (OAuth2) exist in the
-- CHECK so V2 provider kinds are a row-value away — nothing may assume
-- kind == 'imap'. V1 auth is password/app-password ONLY; the secret
-- lives in the daemon vault under the DETERMINISTIC key
-- `ext-inbox-<id>` — NEVER a column here, never in any response.
--
-- `drafts_folder` NULL = autodetect (LIST SPECIAL-USE \Drafts, then the
-- common names); a value overrides detection. `owner_project_id` =
-- projects.id, NOT a FK (0064 idiom).
CREATE TABLE IF NOT EXISTS mail_external_inboxes (
    id               TEXT PRIMARY KEY NOT NULL,
    owner_project_id TEXT NOT NULL,
    email_address    TEXT NOT NULL UNIQUE,
    display_name     TEXT,
    kind             TEXT NOT NULL DEFAULT 'imap'
                     CHECK (kind IN ('imap','jmap','gmail-api')),
    host             TEXT NOT NULL,
    port             INTEGER NOT NULL,
    tls              TEXT NOT NULL DEFAULT 'implicit-tls'
                     CHECK (tls IN ('implicit-tls','starttls')),
    username         TEXT NOT NULL,
    drafts_folder    TEXT,
    status           TEXT NOT NULL DEFAULT 'connected'
                     CHECK (status IN ('connected','error')),
    last_checked_at  INTEGER,
    last_error       TEXT,
    created_at       INTEGER NOT NULL
);
--> statement-breakpoint
-- The read-path hot lookup: this workspace's bound inboxes.
CREATE INDEX IF NOT EXISTS idx_mail_external_inboxes_owner
    ON mail_external_inboxes (owner_project_id);
