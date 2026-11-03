-- 0260_mail_forward_destinations — several destinations per hosted mailbox,
-- and opt-out outside forwards (Rosson 2026-11-02; 0.46.8).
--
-- One live row per (mailbox, destination), not one per mailbox. A row's
-- state, stop link and pause stay its own. No existing row changes here:
-- what a live row did before this migration it still does after it.
--
-- `notice_state` says what K2 told an outside address about its forward:
--   NULL    a same-box forward, or an outside one from before opt-out (its
--           destination confirmed through a link)
--   sent    the "you've been added" email was accepted by the mail server
--   failed  it wasn't; `forward list` shows it and K2 keeps trying (1 to 6
--           hours apart). After a day the forward pauses until the email goes
--   due     a `pending_verify` row waiting for its turn: K2 emails the address,
--           then turns the row on, a few a minute. Either one from before
--           opt-out, recent enough to go active, or a new one added while K2
--           was already pacing its notices
--   stale   a `pending_verify` row too old to go active: it stays off, sends
--           nothing, and is listed until someone sets it again
-- `mail_server.forward_new_per_day`: how many new outside addresses this box
-- may add a day (NULL: the default). The owner's alone.
-- `mail_forward_owner_notes`: what K2 has told the owner about outside
-- forwards this hour. The first few go out one by one; the rest wait
-- (`told_at` NULL) for one hourly summary.
-- `mail_forwards.paused_by`: who paused it (agents resume only their own).
-- `held_by`: someone paused a row K2 already holds and lifts by itself; when K2
-- lets go it stays paused for them instead of going back on.
-- `agent_ws`: the workspace whose agent set this outside forward on its own
-- mailbox. It follows that workspace's agent sending until a person takes it.
-- `notice_failed_at`, `notice_next_at`: since when K2 hasn't managed to email
-- this address, and when it tries next (a backoff, not a row per try).
-- `mail_server.forward_reload_owed`: limits saved, reload not confirmed, across a
-- restart. `forward_webhook_error`: why delivery outcomes aren't reaching K2.
-- New index and table + ADD COLUMNs: renumber freely on a slip (and edit the
-- migration-order tests). The store is `crate::mail_forward`.
DROP INDEX IF EXISTS mail_forwards_live;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS mail_forwards_live_dest ON mail_forwards (address_id, dest) WHERE state <> 'removed';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forwards_mailbox ON mail_forwards (address_id, state);
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN notice_state TEXT CHECK (notice_state IN ('sent', 'failed', 'due', 'stale'));
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN notice_at INTEGER;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN forward_new_per_day INTEGER;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS mail_forward_owner_notes (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    at       INTEGER NOT NULL,
    kind     TEXT NOT NULL CHECK (kind IN ('add', 'change', 'stop')),
    line     TEXT NOT NULL,
    rolled   INTEGER NOT NULL DEFAULT 0 CHECK (rolled IN (0, 1)),
    told_at  INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_owner_notes_at ON mail_forward_owner_notes (at);
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN paused_by TEXT;
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN notice_failed_at INTEGER;
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN notice_next_at INTEGER;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN forward_reload_owed INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN forward_webhook_error TEXT;
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN held_by TEXT;
--> statement-breakpoint
ALTER TABLE mail_forwards ADD COLUMN agent_ws TEXT;
