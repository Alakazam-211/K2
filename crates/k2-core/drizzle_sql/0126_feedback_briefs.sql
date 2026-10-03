-- Ticket HTML brief (prd-ticket-html-brief-v1.md H6/H8/H9/H10).
--
-- One brief per ticket, written in the same transaction as its
-- `feedback` row and never edited (no update route). A NEW child table,
-- not a column on `feedback`: list reads stay small, and `feedback` is
-- never rebuilt (H10: a copy/DROP/RENAME rebuild under foreign_keys=ON
-- cascades into every child table, which is how 0090/0098 wiped
-- `feedback_comments`).
--
-- `html`       the CLEANED brief (daemon allowlist), never the raw input
-- `text`       plain-text extract (<= 16 KiB) for the CLI, the phone, and
--              text-only readers
-- `bytes`      byte length of `html`
-- `sha256`     hex SHA-256 of `html` (client cache key)
-- `sanitizer`  allowlist version tag, e.g. `k2-brief-v1`; stored rows are
--              never re-cleaned when the tag changes
CREATE TABLE IF NOT EXISTS feedback_briefs (
    feedback_id TEXT PRIMARY KEY REFERENCES feedback(id) ON DELETE CASCADE,
    html TEXT NOT NULL,
    text TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    sanitizer TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
