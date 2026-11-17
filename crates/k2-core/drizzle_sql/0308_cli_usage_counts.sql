-- 0308: how the k5 CLI is used, counted by the daemon (R1 slice 10). The
-- store is `crate::cli_usage`.
--
-- One row per (kind, key):
--   fallthrough      key = the CLI noun the native binary handed to the
--                    shell CLI (`X-K5-Cli-Noun`, validated; else `other`)
--   would_refuse     key = the workspace id an owner-token call came from
--                    inside an agent session (`X-K5-Would-Refuse: 1`)
--   owner_from_disk  key = the workspace id of an owner-token call that read
--                    the token from disk (`X-K5-Owner-Source: disk`)
-- A header is a client's claim: counted only after the request's own auth,
-- and it never grants or denies anything. No arguments, paths, pids or tool
-- names are stored. `first_at` and `last_at` are unix seconds.
CREATE TABLE IF NOT EXISTS cli_usage_counts (
  kind     TEXT    NOT NULL CHECK (kind IN ('fallthrough', 'would_refuse', 'owner_from_disk')),
  key      TEXT    NOT NULL CHECK (length(key) BETWEEN 1 AND 64),
  count    INTEGER NOT NULL CHECK (count >= 0),
  first_at INTEGER NOT NULL,
  last_at  INTEGER NOT NULL,
  PRIMARY KEY (kind, key)
);
