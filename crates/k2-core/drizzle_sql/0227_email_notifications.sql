-- 0227: Email Notifications (prd-email-notifications-core-v1 §5.1, E6,
-- E25; 0.46.5).
--
-- K2's one-way notification mail (password resets, invites, an App's own
-- `email.send`) goes from SENDERS, never from mailboxes. A sender is a
-- Stalwart account K2 creates with no usable credential (E3), or an address
-- on an owner-configured SMTP relay. Senders live ONLY here, never in
-- `mail_addresses`, so every Hosted Mail verb (read, grant, app password,
-- rotate, forward, alias, import, quota…) answers `not_found` for them by
-- construction (E1). The store is `crate::email_notify`.
--
-- * `email_senders` — one row per address (one owner per address, E20).
--   `owner_kind` NULL = not yet given to anyone; `system` = the box's one
--   system sender (partial unique index); `app` = one App's
--   (`app_service_id`). `state`: `ready` | `needs_dmarc` (hosted: no DMARC
--   record yet, E10) | `needs_test` (relay: no passing test yet) | `off` |
--   `retired` (removed: the address stays reserved 30 days, E26).
--   `bounce_hints` counts DSNs the sweep found in the sender's mailbox (E5).
-- * `email_relays` — owner-configured SMTP relays (E12). The password lives
--   in the vault (system namespace, scope `engine:email-notify`, name
--   `relay-<id>`); no column holds it.
-- * `email_app_state` — per App: on/off, the granted sender, the recipient
--   scope (`users` | `any`, Q3).
-- * `email_notify_log` — metadata only (E7, E22): never a body. `queued`
--   is the row written before the hand-off (no row, no send); it becomes
--   `submitted` or `failed`. Kept 30 days by default (owner 7–90).
-- * `email_idempotency` — (App, key) → the log row, 24 h (E19).
-- * `email_notify_settings` — one row: log retention and the owner's own
--   address (the only place `test` commands send to).
CREATE TABLE IF NOT EXISTS email_senders (
  id                  TEXT PRIMARY KEY,
  address             TEXT NOT NULL UNIQUE,
  domain              TEXT NOT NULL,
  path                TEXT NOT NULL CHECK (path IN ('hosted', 'relay')),
  stalwart_account_id TEXT,
  relay_id            TEXT,
  owner_kind          TEXT CHECK (owner_kind IS NULL OR owner_kind IN ('system', 'app')),
  app_service_id      TEXT,
  from_name           TEXT NOT NULL,
  from_name_locked    INTEGER NOT NULL DEFAULT 0,
  reply_to_json       TEXT,
  state               TEXT NOT NULL CHECK (state IN ('ready', 'needs_dmarc', 'needs_test', 'off', 'retired')),
  bounce_hints        INTEGER NOT NULL DEFAULT 0,
  last_sweep_at       INTEGER,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  retired_at          INTEGER,
  CHECK ((owner_kind = 'app') = (app_service_id IS NOT NULL)),
  CHECK ((path = 'hosted') OR (relay_id IS NOT NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS email_senders_one_system
  ON email_senders (owner_kind) WHERE owner_kind = 'system' AND state <> 'retired';
CREATE INDEX IF NOT EXISTS email_senders_app ON email_senders (app_service_id);
CREATE INDEX IF NOT EXISTS email_senders_domain ON email_senders (domain);

CREATE TABLE IF NOT EXISTS email_relays (
  id            TEXT PRIMARY KEY,
  host          TEXT NOT NULL,
  port          INTEGER NOT NULL CHECK (port > 0 AND port < 65536),
  tls           TEXT NOT NULL CHECK (tls IN ('implicit', 'starttls')),
  username      TEXT NOT NULL,
  from_domain   TEXT NOT NULL,
  last_test_at  INTEGER,
  last_test_ok  INTEGER,
  created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS email_app_state (
  app_service_id  TEXT PRIMARY KEY,
  enabled         INTEGER NOT NULL DEFAULT 0,
  sender_id       TEXT,
  recipient_scope TEXT NOT NULL DEFAULT 'users' CHECK (recipient_scope IN ('users', 'any')),
  turned_on_at    INTEGER,
  turned_on_by    TEXT,
  off_reason      TEXT,
  updated_at      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS email_notify_log (
  id              TEXT PRIMARY KEY,
  sender_id       TEXT NOT NULL,
  origin          TEXT NOT NULL CHECK (origin IN ('system', 'app', 'agent')),
  app_service_id  TEXT,
  purpose         TEXT,
  to_json         TEXT NOT NULL,
  recipient_count INTEGER NOT NULL,
  subject         TEXT NOT NULL DEFAULT '',
  tags_json       TEXT NOT NULL DEFAULT '[]',
  status          TEXT NOT NULL CHECK (status IN ('queued', 'scheduled', 'held', 'submitted', 'failed', 'refused')),
  error_code      TEXT,
  message_id      TEXT,
  send_after      INTEGER,
  outbound_id     TEXT,
  scope_any       INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS email_notify_log_sender ON email_notify_log (sender_id, created_at);
CREATE INDEX IF NOT EXISTS email_notify_log_app ON email_notify_log (app_service_id, created_at);
CREATE INDEX IF NOT EXISTS email_notify_log_due ON email_notify_log (status, send_after);
CREATE INDEX IF NOT EXISTS email_notify_log_created ON email_notify_log (created_at);

CREATE TABLE IF NOT EXISTS email_idempotency (
  app_service_id  TEXT NOT NULL,
  key             TEXT NOT NULL,
  request_sha256  TEXT NOT NULL,
  log_id          TEXT NOT NULL,
  created_at      INTEGER NOT NULL,
  PRIMARY KEY (app_service_id, key)
);

CREATE TABLE IF NOT EXISTS email_notify_settings (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),
  log_retention_days  INTEGER NOT NULL DEFAULT 30 CHECK (log_retention_days BETWEEN 7 AND 90),
  owner_email         TEXT,
  updated_at          INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO email_notify_settings (id, log_retention_days, updated_at) VALUES (1, 30, 0);
