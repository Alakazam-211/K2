-- 0292: drafted replies K2 keeps track of (0.46.11, Rosson 2026-11-10: an
-- agent's draft reply shows at the top of its thread in the mail reader,
-- plainly unsent, and the person edits, sends or discards it).
--
-- The draft itself is a real draft in the mailbox's Drafts folder; this row
-- only says where it is, which message it answers, who wrote it, who last
-- edited it and what became of it. No message text, no recipient lists.
-- `in_reply_to` / `refs` are the Message-ID chain the reply carries (so an
-- edit keeps the thread). Written by `k2-daemon` `mail::drafts`. 0291 may be
-- the App database door's; 0292 is K2's.
CREATE TABLE IF NOT EXISTS mail_drafts (
    -- 'd_' + 32 hex: the id the reader and `k2 mail drafts` use
    id TEXT PRIMARY KEY,
    -- 'hosted' | 'linked'
    inbox_source TEXT NOT NULL CHECK (inbox_source IN ('hosted', 'linked')),
    inbox_id TEXT NOT NULL,
    address TEXT NOT NULL,
    -- the workspace the inbox belongs to (projects.id)
    project_id TEXT NOT NULL,
    -- the opaque message id (m_...) of the message this draft answers
    reply_of TEXT NOT NULL,
    -- the backend's thread id of that message, when it has one
    thread_ref TEXT,
    in_reply_to TEXT,
    refs TEXT,
    -- where the draft sits in the mailbox: the backend email id (JMAP id,
    -- IMAP uid token). NULL when the backend didn't say (then the reader
    -- shows the draft but can't edit it)
    backend_ref TEXT,
    -- 'agent' (author = projects.id) | 'person' (author = the actor)
    author_kind TEXT NOT NULL CHECK (author_kind IN ('agent', 'person')),
    author TEXT NOT NULL,
    author_label TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    edited_at INTEGER NOT NULL,
    -- the person who last saved it, when that isn't the author
    edited_by TEXT,
    -- 'open' | 'sent' | 'held' (waiting for approval) | 'discarded'
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'sent', 'held', 'discarded')),
    state_at INTEGER,
    state_by TEXT,
    -- the outbox row a sent draft became
    outbound_id TEXT,
    -- when the drafting agent was last told what happened
    agent_told_at INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_drafts_by_address ON mail_drafts (address, state);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_drafts_by_author ON mail_drafts (author_kind, author, created_at);
