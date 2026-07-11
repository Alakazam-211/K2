-- K2 Mail S10 — multi-workspace per-role access for external inboxes
-- (prd-email-server-v1 §17.5, generalizing S9's single-owner model).
--
-- `mail_external_inboxes.owner_project_id` STAYS the OWNER workspace:
-- full read+draft AND the only one who can manage the inbox (grant /
-- revoke access, change credentials, remove). Additional workspaces
-- get an ACCESS GRANT row here — `level` = 'read' (messages/read/wait)
-- or 'draft' (read + save reply drafts). The owner is NEVER a grant
-- row (its access is the `owner_project_id` binding itself).
--
-- `inbox_id` = `mail_external_inboxes.id`, NOT a FK (the 0064 idiom);
-- `external remove` cascades these rows in code. `project_id` =
-- projects.id, likewise not a FK. A workspace with neither the owner
-- binding nor a grant row sees the SAME masked `not_found` as before
-- (no existence leak — the S3 ownership rule, unchanged).
CREATE TABLE IF NOT EXISTS mail_external_inbox_grants (
    inbox_id   TEXT NOT NULL,
    project_id TEXT NOT NULL,
    level      TEXT NOT NULL CHECK(level IN ('read','draft')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (inbox_id, project_id)
);
--> statement-breakpoint
-- The list/manage lookup: every grant on one inbox.
CREATE INDEX IF NOT EXISTS idx_mail_external_inbox_grants_inbox
    ON mail_external_inbox_grants (inbox_id);
--> statement-breakpoint
-- The read-path hot lookup: every inbox a workspace has been granted.
CREATE INDEX IF NOT EXISTS idx_mail_external_inbox_grants_project
    ON mail_external_inbox_grants (project_id);
