-- 0279: agent mail notifications (0.46.11,
-- prd-routines-reminders-and-agent-mail-v1 §4.6). An agent hears about mail
-- on the inboxes someone chose, without blocking on `k2 mail wait`.
--
-- Nothing wakes an agent until someone chooses (Rosson 11-09). A row exists
-- here only for an inbox a person or the agent picked; no row = off.
--
-- agent_mail_watch: one per agent. `vip_session_line` = also write the VIP
-- line into the agent's session (W35: off keeps it to the Thread card).
CREATE TABLE IF NOT EXISTS agent_mail_watch (
    project_id          TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    enabled             INTEGER NOT NULL DEFAULT 0,
    vip_outside_window  INTEGER NOT NULL DEFAULT 1,
    snippets            INTEGER NOT NULL DEFAULT 1,
    vip_session_line    INTEGER NOT NULL DEFAULT 1,
    paused_until        TEXT,
    updated_at          TEXT,
    updated_by          TEXT NOT NULL DEFAULT ''
);
--> statement-breakpoint

-- One row per inbox someone chose for this agent. `wake`: `vip` = only VIP
-- senders wake it; `all` = VIPs at once, everyone else in the digest.
CREATE TABLE IF NOT EXISTS agent_mail_watch_inboxes (
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source      TEXT NOT NULL CHECK (source IN ('hosted', 'linked')),
    inbox_id    TEXT NOT NULL,
    wake        TEXT NOT NULL DEFAULT 'vip' CHECK (wake IN ('vip', 'all')),
    added_by    TEXT NOT NULL DEFAULT '',
    added_at    TEXT NOT NULL,
    PRIMARY KEY (project_id, source, inbox_id)
);
--> statement-breakpoint

CREATE TABLE IF NOT EXISTS agent_mail_vips (
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('address', 'domain', 'person')),
    value       TEXT NOT NULL,
    added_by    TEXT NOT NULL DEFAULT '',
    added_at    TEXT NOT NULL,
    PRIMARY KEY (project_id, kind, value)
);
--> statement-breakpoint

-- Per inbox, not per agent: an inbox is polled once however many agents hear
-- about it. `needs_relink` (W30): the credential failed; polling stopped and
-- waits for the credential to change (`relink_cred` is what it was).
CREATE TABLE IF NOT EXISTS mail_watch_cursors (
    source            TEXT NOT NULL CHECK (source IN ('hosted', 'linked')),
    inbox_id          TEXT NOT NULL,
    uidvalidity       INTEGER,
    last_uid          INTEGER,
    jmap_state        TEXT,
    last_received_at  TEXT,
    last_poll_at      TEXT,
    last_error        TEXT,
    needs_relink      INTEGER NOT NULL DEFAULT 0,
    relink_cred       TEXT,
    next_poll_at      INTEGER NOT NULL DEFAULT 0,
    consecutive_errs  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source, inbox_id)
);
--> statement-breakpoint

-- One row per message per agent. K2.db keeps who and when, never the subject
-- or body (those are peeked at delivery; a VIP card keeps them with the
-- Thread, W5). Kept 30 days.
CREATE TABLE IF NOT EXISTS agent_mail_notices (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source          TEXT NOT NULL CHECK (source IN ('hosted', 'linked')),
    inbox_id        TEXT NOT NULL,
    inbox_address   TEXT NOT NULL,
    message_token   TEXT NOT NULL,
    rfc_message_id  TEXT NOT NULL,
    from_addr       TEXT NOT NULL,
    received_at     TEXT NOT NULL,
    class           TEXT NOT NULL CHECK (class IN ('vip', 'digest', 'ignored')),
    reason          TEXT NOT NULL DEFAULT '',
    state           TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'digested', 'suppressed', 'dropped')),
    delivery_ref    TEXT,
    woke            INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    delivered_at    INTEGER,
    UNIQUE (project_id, rfc_message_id)
);
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS idx_agent_mail_notices_pending
    ON agent_mail_notices (project_id, state, class);
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS idx_agent_mail_notices_created
    ON agent_mail_notices (created_at);
