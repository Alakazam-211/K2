-- 0315: when each Claude token's sign-in reaches its time limit, and who
-- has seen the warning about it.
--
-- `llm_accounts.sign_in_ends_at`: Claude's `refreshTokenExpiresAt` (unix
-- seconds) from the token's own sign-in, or the live file's for the token
-- on record as live. NULL = the file doesn't say (older Claude Code).
--
-- `llm_sign_in_warnings`: an owner login (`user_key`: `owner-token` or
-- `login:<name>`) put away the warning for one token at one step
-- (`7d`, `1d`, `2h`, `ended`) of one sign-in (`epoch`: its end time, or
-- for an end with no time, when the token was last signed in). A new
-- sign-in has a new epoch, so its warnings start over. Store:
-- `crate::llm_accounts::warnings`.
ALTER TABLE llm_accounts ADD COLUMN sign_in_ends_at INTEGER;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS llm_sign_in_warnings (
  account_id   TEXT NOT NULL,
  threshold    TEXT NOT NULL,
  epoch        INTEGER NOT NULL,
  user_key     TEXT NOT NULL,
  dismissed_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, threshold, epoch, user_key)
);
