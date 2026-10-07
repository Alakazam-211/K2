-- 0133: who may hold hosted-mail credentials (agents send only through K2).
--
-- Agents send mail through K2 (`k2 mail send`), where the owner's rules
-- apply: agentSend, always-BCC, the recipient cap and the outbox. From
-- 0.45.0 an agent WITHOUT mail-manage never gets an IMAP/SMTP secret. An
-- IT agent (its workspace has mail-manage) is as capable as the owner,
-- except it can't get a secret for its OWN workspace's mailboxes (that
-- would loosen the rules on its own sends). Before 0.45.0 any agent saw
-- its mailbox password at mint and a mail-manage agent could add app
-- passwords anywhere; Stalwart doesn't record who made a secret, so K2
-- keeps this list. `k2 hostmail doctor` flags a live secret with no row
-- here, or one an agent made for its own mailbox. It never revokes.
--
-- One row per credential:
--   kind = 'mailbox'       credential_id = ''   (the mailbox password)
--   kind = 'app_password'  credential_id = the Stalwart AppPassword id
-- origin: minted | rotated | kept (reviewed) | withheld (an agent minted
--   the mailbox and nobody was shown the password)
-- creator_project_id: NULL = the owner (owner token or an owner/admin
--   login); otherwise the projects.id of the agent's workspace.
--
-- No secrets here — ids only. `address_id` = mail_addresses.id (plain
-- column, like the rest of the mail family).
CREATE TABLE IF NOT EXISTS mail_credential_marks (
    address_id         TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('mailbox','app_password')),
    credential_id      TEXT NOT NULL DEFAULT '',
    origin             TEXT NOT NULL
                       CHECK (origin IN ('minted','rotated','kept','withheld')),
    creator_project_id TEXT,
    created_at         INTEGER NOT NULL,
    PRIMARY KEY (address_id, kind, credential_id)
);
--> statement-breakpoint
-- A PERSON's mailbox (1) vs an agent's own mailbox (0, the default). A
-- person's mailbox bound to an IT agent's workspace is not that agent's
-- "own" mailbox: the IT agent may mint, rotate and see its secrets. Set at
-- create (`k2 hostmail create --person`), with `k2 hostmail person <addr>`,
-- or for a whole workspace at once with `k2 hostmail person --all
-- --workspace <ws> on` (never its agent's send identities). The default
-- stays 0 for existing rows: nothing recorded tells an agent's own sending
-- mailbox apart from an imported or provisioned one (one create route, no
-- import record, no configured From), so 0 is the safe guess. An IT agent
-- can't turn it on in its own workspace (owner only).
ALTER TABLE mail_addresses ADD COLUMN person_mailbox INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
-- The cutoff for the bulk review (`k2 hostmail password keep
-- --all-existing` / `app-password keep --all-existing`): the unix time this
-- migration ran. Only secrets created BEFORE it can be bulk-kept; anything
-- newer is reviewed one by one. One row, written once, never a secret.
CREATE TABLE IF NOT EXISTS mail_credential_baseline (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    migrated_at INTEGER NOT NULL
);
--> statement-breakpoint
INSERT OR IGNORE INTO mail_credential_baseline (id, migrated_at)
VALUES (1, CAST(strftime('%s', 'now') AS INTEGER));
