-- 0188: OAuth v2 for linked inboxes (prd-app-mail-calendar-surfaces-v1
-- §3.2 / §6, slice MCS1). The PRD's placeholder number was 0187; 0187 is
-- Sew S30's (vs-live MCS-A2), so this takes 0188.
--
-- ALTER only, never a rebuild. Every column is non-secret: tokens stay in
-- the daemon vault (`ext-inbox-<id>-oauth`), never a column.
--
-- `features`        what the link was consented for: 'mail', 'calendar' or
--                   'mail,calendar'. Existing rows were mail-only links.
-- `granted_scopes`  the scope string the provider granted (mirrored from the
--                   token response; backfilled on the next refresh for rows
--                   linked before 0188).
-- `oauth_client`    which client minted the row: 'baked' (K2's built-in app)
--                   or 'byo' (the owner's own app). Refresh uses that client,
--                   not whatever the box resolves now. NULL = linked before
--                   0188 (refresh keeps the old resolution).
-- `oauth_client_hash` sha256 prefix of the minting client id, so a BYO app
--                   that was swapped since is caught instead of silently used.
-- `account_tenant`  Microsoft `tid` / Google Workspace `hd`.
-- `account_kind`    'personal' | 'work' (Microsoft), 'consumer' | 'workspace'
--                   (Google). Free/busy reach and admin-consent hints use it.
-- `account_checked_at` when the signed-in account was checked against the
--                   row's address. NULL = never checked (rows from before
--                   0188 until 'k2 mail link upgrade').
ALTER TABLE mail_external_inboxes ADD COLUMN features TEXT NOT NULL DEFAULT 'mail';
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN granted_scopes TEXT;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN oauth_client TEXT CHECK (oauth_client IS NULL OR oauth_client IN ('baked', 'byo'));
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN oauth_client_hash TEXT;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN account_tenant TEXT;
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN account_kind TEXT CHECK (account_kind IS NULL OR account_kind IN ('personal', 'work', 'consumer', 'workspace'));
--> statement-breakpoint
ALTER TABLE mail_external_inboxes ADD COLUMN account_checked_at INTEGER;
