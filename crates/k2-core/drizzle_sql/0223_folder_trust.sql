-- 0223: folder-trust decisions (prd-security-sweep-0464-v1 rev 2, SS9; 0.46.4).
--
-- Before K2 tells Claude, Codex, Grok, Gemini or Cursor that a folder is
-- trusted, it scans the folder for settings that run commands or send
-- logins elsewhere (SS8). A clean folder is trusted as before. A risky one
-- needs a person's decision, kept here. The store is
-- `crate::cli_folder_trust::gate`.
--
-- * `scope` — the canonical work tree of the repo's git common dir (all
--   worktrees of a repo share one decision), or the canonical folder
--   outside git.
-- * `decision` — NULL (undecided) | `once` (this `fingerprint` only) |
--   `always` (any fingerprint) | `deny` (K2 starts no AI tool here).
-- * `fingerprint` — the scan fingerprint a `once` decision covers.
-- * `decided_by` / `decided_at` — `owner`, a login name, or `upgrade`.
-- * `k2_granted` — 1 once K2 has written trust grants for this scope, so
--   "Don't trust" removes exactly K2's entries; `granted_keys` (JSON:
--   `{"claude":[…],"codex":[…],"grok":[…],"gemini":[…],"cursor":[…]}`) is
--   which entries, per store. An entry the person had before K2 wrote is
--   never listed, so it is never removed (it is reported instead).
-- * `pending_card` — JSON of the open ask (`{card, conversation, project,
--   fingerprint, ticket, at}`), NULL when nothing is waiting.
-- * `upgrade` — 1 on rows this migration made from 0.46.3's registered
--   folders; the first 0.46.4 boot re-keys them by scope, records the
--   grants K2 wrote, marks each risky one `once`/`upgrade` and files one
--   summary for the owner (A4, Q6). 0 afterwards.
--
-- Additive only. 0.46.3 trusted every registered folder silently, so every
-- registered path starts `k2_granted = 1`.
CREATE TABLE IF NOT EXISTS folder_trust (
  scope         TEXT PRIMARY KEY CHECK (length(scope) >= 1),
  decision      TEXT CHECK (decision IS NULL OR decision IN ('once', 'always', 'deny')),
  fingerprint   TEXT,
  decided_by    TEXT,
  decided_at    INTEGER,
  k2_granted    INTEGER NOT NULL DEFAULT 0,
  granted_keys  TEXT NOT NULL DEFAULT '{}',
  pending_card  TEXT,
  upgrade       INTEGER NOT NULL DEFAULT 0,
  updated_at    INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO folder_trust (scope, k2_granted, upgrade, updated_at)
SELECT path, 1, 1, CAST(strftime('%s', 'now') AS INTEGER)
FROM projects
WHERE path IS NOT NULL AND length(path) >= 1;
