-- K2 Mail — agent-scheduled outbound delivery.
-- Extends mail_outbound with send_after + status 'scheduled'.
-- TABLE REBUILD (SQLite cannot ALTER CHECK) — same pattern as 0082.
CREATE TABLE IF NOT EXISTS mail_outbound_new (
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
                                       'sent','failed','scheduled')),
    decided_by       TEXT,
    note             TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    decided_at       INTEGER,
    sent_at          INTEGER,
    send_after       INTEGER
);
--> statement-breakpoint
INSERT INTO mail_outbound_new (
    id, owner_project_id, agent_name, from_address, to_json, cc_json,
    subject, body_ref, attachments_ref, status, decided_by, note,
    created_at, updated_at, decided_at, sent_at
)
SELECT
    id, owner_project_id, agent_name, from_address, to_json, cc_json,
    subject, body_ref, attachments_ref, status, decided_by, note,
    created_at, updated_at, decided_at, sent_at
FROM mail_outbound;
--> statement-breakpoint
DROP TABLE mail_outbound;
--> statement-breakpoint
ALTER TABLE mail_outbound_new RENAME TO mail_outbound;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_mail_outbound_owner_status_created
    ON mail_outbound (owner_project_id, status, created_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_mail_outbound_status_created
    ON mail_outbound (status, created_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_mail_outbound_status_send_after
    ON mail_outbound (status, send_after);
