-- 0314: LLM subscription auto-rotate ("auto Next"), one row per tool.
-- A row means it is ON: the server default moves to the next signed-in
-- subscription by itself once the live one reaches 99% of its binding
-- usage window, the same switch as the wallet's Next. Off by default (no
-- row). A person turns it on, in person (a vault confirm), never an agent:
-- `person` is that person's wallet key (`owner-token` or `login:<name>`),
-- and the loop stops acting when that person can no longer change the
-- server default. The loop's own bookkeeping rides along: the last
-- automatic switch (its cooldown), the last line it wrote, and when it
-- last found every subscription at its limit (one notice per episode).
-- Checked in k2-core (`crate::llm_accounts::auto_next`). Safe to run
-- again (IF NOT EXISTS).
CREATE TABLE IF NOT EXISTS llm_auto_next (
  tool            TEXT    PRIMARY KEY NOT NULL CHECK (tool IN ('claude', 'codex', 'grok')),
  person          TEXT    NOT NULL CHECK (length(person) BETWEEN 1 AND 200),
  enabled_at      INTEGER NOT NULL,
  last_switch_at  INTEGER,
  last_line       TEXT    CHECK (last_line IS NULL OR length(last_line) <= 500),
  last_line_at    INTEGER,
  exhausted_at    INTEGER
);
