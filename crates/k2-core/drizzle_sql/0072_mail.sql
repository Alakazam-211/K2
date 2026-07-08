-- K2 Mail foundation (prd-email-server-v1 §12) — the K2-side state for
-- the Stalwart-sidecar email server. Template: 0064_feedback.sql
-- (unix-SECONDS timestamps, CHECK-constrained enums, project_id TEXT
-- and NOT a FK).
--
-- Boundary rule (PRD §4): everything K2 needs to know about mail —
-- agent ownership, approvals, caps, doctor history — lives HERE, in
-- K2's DB. Stalwart holds only what a mail server holds (domains,
-- accounts, messages, DKIM keys); it is a separate process driven over
-- its JMAP management API, never linked or vendored.
--
-- `mail_server` — the SINGLETON install record (id is CHECKed to 1).
-- "not-installed" is represented by the ABSENCE of the row, so the
-- status enum only covers installed lifecycles. `pinned_version` is
-- stamped from the daemon's STALWART_PINNED_VERSION const at install;
-- `installed_version` is what's actually on disk (they diverge only
-- mid-upgrade). `*_secret_ref` columns hold REFERENCES into the
-- daemon's secret storage — never the secrets themselves (PRD §10).
CREATE TABLE IF NOT EXISTS mail_server (
    id                INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    status            TEXT NOT NULL
                      CHECK (status IN ('installing','running','degraded',
                                        'stopped','disabled','error')),
    pinned_version    TEXT NOT NULL,
    installed_version TEXT,
    hostname          TEXT,
    -- TLS/port plan chosen by the detect-443-and-adapt preflight
    -- (PRD §5.3): A = :443 free (TLS-ALPN), B = DNS-01, C = HTTP-01
    -- behind the user's proxy.
    port_plan         TEXT
                      CHECK (port_plan IN ('tls-alpn','dns-01','http-01')),
    -- Stalwart's mgmt/JMAP base URL (localhost-only in plans B/C).
    -- The API path under it is DISCOVERED via /.well-known/jmap at
    -- runtime — never stored hardcoded (PRD §4.1).
    api_url           TEXT,
    admin_secret_ref  TEXT,
    api_key_ref       TEXT,
    installed_at      INTEGER,
    updated_at        INTEGER NOT NULL
);
--> statement-breakpoint
-- `mail_domains` — one row per hosted domain. `domain` is stored ONLY
-- in normalized form (lowercase punycode A-label, no trailing dot —
-- k2_core::mail_domain::normalize_mail_domain at every boundary, PRD
-- pre-mortem #14); display-decoding happens at the edge.
-- `dns_status_json` is the per-record verification state
-- (Valid | Missing | Wrong + live values, PRD §6.3). `relay_config_id`
-- is a PLAIN column (not a FK): a relay config can be replaced/removed
-- independently and the send path validates it at use time.
CREATE TABLE IF NOT EXISTS mail_domains (
    id                 TEXT PRIMARY KEY NOT NULL,
    domain             TEXT NOT NULL UNIQUE,
    stalwart_domain_id TEXT,
    send_mode          TEXT NOT NULL DEFAULT 'receive-only'
                       CHECK (send_mode IN ('direct','relay','receive-only')),
    relay_config_id    TEXT,
    status             TEXT NOT NULL DEFAULT 'pending'
                       CHECK (status IN ('pending','verified','error')),
    dns_status_json    TEXT,
    verified_at        INTEGER,
    last_checked_at    INTEGER,
    created_at         INTEGER NOT NULL
);
--> statement-breakpoint
-- `mail_relay_configs` — smart-host outbound credentials (PRD §8.3).
-- `kind` anticipates the V2 provider one-clicks; V1 implements ONLY
-- 'smtp' but nothing may assume kind == smtp. `config_json` is the
-- provider-shaped blob for non-smtp kinds. `secret_ref` references the
-- daemon's secret storage (the relay password is never stored here).
CREATE TABLE IF NOT EXISTS mail_relay_configs (
    id          TEXT PRIMARY KEY NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'smtp'
                CHECK (kind IN ('smtp','mailgun','ses','resend')),
    host        TEXT,
    port        INTEGER,
    username    TEXT,
    secret_ref  TEXT,
    tls_kind    TEXT
                CHECK (tls_kind IN ('implicit','starttls')),
    -- The SPF include string for relay/dual mode, entered/confirmed by
    -- the owner from the provider's setup screen (never hardcoded —
    -- they drift; PRD §8.3).
    spf_include TEXT,
    config_json TEXT,
    created_at  INTEGER NOT NULL
);
--> statement-breakpoint
-- `mail_addresses` — the K2-side binding of a Stalwart account to its
-- OWNING workspace/agent (PRD §7.1). `owner_project_id` = projects.id,
-- NOT a FK (0064 idiom: project rows can be removed independently and
-- the address record must survive as auditable history until purged).
-- `client_id` powers idempotent minting (`k2 mail create --id`): the
-- same (owner, client_id) returns the existing address (PRD §7.2).
-- `domain_id` is a PLAIN column: domain removal RETIRES addresses via
-- the route flow (with retention), never a silent cascade (PRD §6.6).
CREATE TABLE IF NOT EXISTS mail_addresses (
    id                  TEXT PRIMARY KEY NOT NULL,
    address             TEXT NOT NULL UNIQUE,
    domain_id           TEXT NOT NULL,
    stalwart_account_id TEXT,
    owner_project_id    TEXT NOT NULL,
    client_id           TEXT,
    status              TEXT NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active','retired')),
    created_at          INTEGER NOT NULL,
    retired_at          INTEGER
);
--> statement-breakpoint
-- Idempotent-minting lookup: one live client_id per owning workspace.
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_addresses_owner_client
    ON mail_addresses (owner_project_id, client_id)
    WHERE client_id IS NOT NULL;
--> statement-breakpoint
-- The cap-check + list hot path: this workspace's addresses by status.
CREATE INDEX IF NOT EXISTS idx_mail_addresses_owner_status
    ON mail_addresses (owner_project_id, status);
--> statement-breakpoint
-- `mail_outbound` — the approval queue AND the send audit log in one
-- table (PRD §8.4): EVERY outbound message gets a row BEFORE it is
-- handed to Stalwart, regardless of gating mode (pre-mortem #11:
-- no row, no send — fail-closed). `body_ref`/`attachments_ref` point
-- at daemon-managed storage, never inline blobs (never log bodies).
-- `decided_by`/`note` carry the human's approve/deny back to the agent.
CREATE TABLE IF NOT EXISTS mail_outbound (
    id               TEXT PRIMARY KEY NOT NULL,
    owner_project_id TEXT NOT NULL,
    agent_name       TEXT NOT NULL,
    from_address     TEXT NOT NULL,
    to_json          TEXT NOT NULL,
    cc_json          TEXT,
    subject          TEXT NOT NULL,
    body_ref         TEXT,
    attachments_ref  TEXT,
    status           TEXT NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending','approved','denied',
                                       'sent','failed')),
    decided_by       TEXT,
    note             TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    decided_at       INTEGER,
    sent_at          INTEGER
);
--> statement-breakpoint
-- The agent's `k2 mail outbox` read path: my outbound, newest first.
CREATE INDEX IF NOT EXISTS idx_mail_outbound_owner_status_created
    ON mail_outbound (owner_project_id, status, created_at);
--> statement-breakpoint
-- The owner's Approvals queue read path: pending across all workspaces.
CREATE INDEX IF NOT EXISTS idx_mail_outbound_status_created
    ON mail_outbound (status, created_at);
--> statement-breakpoint
-- `mail_doctor_runs` — deliverability-doctor history (PRD §9).
-- `domain_id` nullable: server-level runs (network/PTR/blocklists)
-- have no domain. `results_json` = the full per-check ✓/✖/? table.
CREATE TABLE IF NOT EXISTS mail_doctor_runs (
    id           TEXT PRIMARY KEY NOT NULL,
    domain_id    TEXT,
    results_json TEXT NOT NULL,
    grade        TEXT NOT NULL
                 CHECK (grade IN ('pass','warn','fail')),
    ran_at       INTEGER NOT NULL
);
--> statement-breakpoint
-- The "latest doctor result per domain" read path.
CREATE INDEX IF NOT EXISTS idx_mail_doctor_runs_domain_ran
    ON mail_doctor_runs (domain_id, ran_at);
--> statement-breakpoint
-- Per-workspace SEND-GATING override (PRD §12 / D4): 'off'|'approval'|
-- 'on', write-validated in `update_project_setting`. NULL (the
-- backfill) = inherit the GLOBAL default (`AppSettings.mail_agent_send`,
-- which itself defaults 'off' — send is OFF by default, fail-closed).
ALTER TABLE projects ADD COLUMN mail_agent_send TEXT;
--> statement-breakpoint
-- Per-workspace ADDRESS-CAP override (PRD §12 / D6): non-negative int,
-- 0 = unlimited, write-validated in `update_project_setting`. NULL =
-- inherit the GLOBAL default (`AppSettings.mail_address_cap`, 5).
ALTER TABLE projects ADD COLUMN mail_address_cap INTEGER;
