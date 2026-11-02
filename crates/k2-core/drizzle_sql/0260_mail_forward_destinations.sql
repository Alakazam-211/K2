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
--   failed  it wasn't; the forward runs anyway, `forward list` shows it and
--           K2 tries again hourly
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
-- New index and table + three ADD COLUMNs: renumber freely on a slip (and edit the
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
    kind     TEXT NOT NULL CHECK (kind IN ('change', 'stop')),
    line     TEXT NOT NULL,
    rolled   INTEGER NOT NULL DEFAULT 0 CHECK (rolled IN (0, 1)),
    told_at  INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS mail_forward_owner_notes_at ON mail_forward_owner_notes (at);
