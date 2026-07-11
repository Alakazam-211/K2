-- K2 Mail S11 — ONE inbox access layer over BOTH provisioning sources
-- (prd-email-server-v1 §17.5, generalizing S10's linked-only grants to
-- cover hosted addresses too).
--
-- The two provisioning tables stay: `mail_addresses` (hosted — K2 mints
-- the mailbox on a verified domain) and `mail_external_inboxes` (linked
-- — the user's OWN external IMAP account). Their `owner_project_id`
-- column KEEPS its name but is now the **PRIMARY** workspace: the one
-- that manages the inbox (grant/revoke access, transfer, remove). A new
-- `primary_level` column records how much the primary itself may do —
-- hosted defaults 'send', linked defaults 'draft' (linked can NEVER
-- send: there is no send code path against an IMAP account).
--
-- `mail_inbox_grants` is the SINGLE access table across both sources.
-- `source` discriminates which provisioning table `inbox_id` points at
-- ('hosted' → mail_addresses.id, 'linked' → mail_external_inboxes.id) —
-- neither is a FK (the 0064 idiom; the ops layer cascades on remove).
-- `level` ∈ read < draft < send; 'send' is only ever stored for
-- 'hosted' rows (the ops layer refuses granting 'send' on a linked
-- inbox). A workspace with neither the PRIMARY binding nor a grant row
-- sees the SAME masked `not_found` as before (the S3 ownership rule,
-- unchanged — no existence leak).
CREATE TABLE IF NOT EXISTS mail_inbox_grants (
    source     TEXT NOT NULL CHECK(source IN ('hosted','linked')),
    inbox_id   TEXT NOT NULL,
    project_id TEXT NOT NULL,
    level      TEXT NOT NULL CHECK(level IN ('read','draft','send')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (source, inbox_id, project_id)
);
--> statement-breakpoint
-- The list/manage lookup: every grant on one inbox.
CREATE INDEX IF NOT EXISTS idx_mail_inbox_grants_source_inbox
    ON mail_inbox_grants (source, inbox_id);
--> statement-breakpoint
-- The read-path hot lookup: every inbox a workspace has been granted.
CREATE INDEX IF NOT EXISTS idx_mail_inbox_grants_project
    ON mail_inbox_grants (project_id);
--> statement-breakpoint
-- Data-migrate S10's linked-only grants into the unified table (they
-- are all 'linked' by construction — the only source S10 knew).
INSERT INTO mail_inbox_grants (source, inbox_id, project_id, level, created_at)
    SELECT 'linked', inbox_id, project_id, level, created_at
    FROM mail_external_inbox_grants;
--> statement-breakpoint
-- The S10 table is superseded — drop it (its rows are now above).
DROP TABLE mail_external_inbox_grants;
--> statement-breakpoint
-- The PRIMARY workspace's own access ceiling on each provisioning row.
-- Hosted primaries default to 'send' (they own the mailbox); linked
-- primaries default to 'draft' (no send path exists from an external
-- account). Runs once by migration name, so the ADD COLUMN never
-- double-applies.
ALTER TABLE mail_addresses ADD COLUMN primary_level TEXT NOT NULL DEFAULT 'send';
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN primary_level TEXT NOT NULL DEFAULT 'draft';
