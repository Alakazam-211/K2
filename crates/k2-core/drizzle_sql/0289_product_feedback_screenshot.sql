-- 0289: a screenshot a person attached to Send feedback to K2 (0.46.11,
-- prd-feedback-screenshot-v1 §5.4). 0289 is reserved by K2 (0288 is the
-- OCmail reader's); renumber only with K2 (and edit the migration-order
-- tests). The store is `crate::product_feedback::store`.
--
-- One picture per outbox row. The PNG (at most 2 MiB, chunks cut to the
-- allow-list) sits in its own table so the outbox reads (`list`, `due`)
-- stay light. It is deleted in the same transaction as the row's move to
-- `sent`, `refused` or `cancelled`; a `failed` row keeps it for Retry; the
-- prune also deletes any picture whose row is gone or final.
-- `screenshot_meta` stays for the Sent tab:
-- {"bytes","width","height","source","capture"?,"sha256"}.
ALTER TABLE product_feedback_outbox ADD COLUMN screenshot_meta TEXT;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS product_feedback_screenshots (
  feedback_id TEXT PRIMARY KEY NOT NULL,
  png BLOB NOT NULL CHECK (length(png) <= 2097152),
  sha256 TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
