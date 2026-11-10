-- 0284: the mailbox access log (0.46.11, prd-ocmail-admin-in-k2-v1 §7.2/§7.3,
-- slice OA0). One row per inbox a login or the owner token reaches through a
-- named workspace, written BEFORE the mail is touched (no row, no access). The
-- admin reader (OA1/OA2) writes here too. Never message content: `message_ref`
-- is the opaque message id the call named, nothing more. Written by
-- `k2-daemon` `mail::mailbox_guard`. 0284 is K2's (0283 is the Org Chart's).
CREATE TABLE IF NOT EXISTS mail_reader_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL,
    -- 'owner-token' | 'user:<login>'
    actor TEXT NOT NULL,
    -- 'owner' | 'admin' | 'member' | 'voice_chat'
    actor_role TEXT NOT NULL,
    -- 'hosted' | 'linked'
    inbox_source TEXT NOT NULL,
    inbox_id TEXT NOT NULL,
    address TEXT NOT NULL,
    -- 'agent' | 'person'
    inbox_kind TEXT NOT NULL,
    -- 1 when the viewer is the mailbox's own person
    own INTEGER NOT NULL DEFAULT 0,
    -- the workspace the call named (projects.id)
    workspace_id TEXT,
    -- the route, e.g. 'read', 'messages', 'move', 'reply'
    action TEXT NOT NULL,
    message_ref TEXT,
    reason TEXT,
    -- where the call came in: 'loopback' | 'lan' | 'tunnel' | 'app'
    ip_hint TEXT,
    -- 'ok' | 'refused'
    outcome TEXT NOT NULL,
    -- the HTTP status the call answered (filled in after the handler)
    status INTEGER
);
CREATE INDEX IF NOT EXISTS idx_mail_reader_audit_inbox ON mail_reader_audit (inbox_source, inbox_id, at);
CREATE INDEX IF NOT EXISTS idx_mail_reader_audit_actor ON mail_reader_audit (actor, at);
