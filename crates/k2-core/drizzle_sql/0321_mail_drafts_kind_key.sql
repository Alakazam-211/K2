-- 0321: what K2 keeps about the drafts an agent saves (0.46.17).
--
-- `kind`: 'reply' (a draft answering a message, the 0292 rows) or 'compose'
-- (a brand-new draft on a linked inbox, `k2 mail draft --to --subject`).
-- Compose drafts are now recorded too, so the agent that saved one can find
-- and delete it (`k2 mail draft delete <d_id>`). A compose row's `reply_of`
-- is ''. The reader's thread view keeps showing reply drafts only.
--
-- `client_key`: the idempotency key the CLI sends with a save. A save that
-- answers late (the CLI stopped waiting, the daemon kept going) is asked
-- again with the same key and answers the draft it already saved instead of
-- saving a second one. Unique per workspace (`project_id`). No mail text,
-- no recipients, as in 0292. Written by `k2-daemon` `mail::drafts`.
-- 0320 is push_device_voip's; 0321 is K2's.
ALTER TABLE mail_drafts ADD COLUMN kind TEXT NOT NULL DEFAULT 'reply' CHECK (kind IN ('reply', 'compose'));
--> statement-breakpoint
ALTER TABLE mail_drafts ADD COLUMN client_key TEXT;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS mail_drafts_by_client_key ON mail_drafts (project_id, client_key) WHERE client_key IS NOT NULL;
