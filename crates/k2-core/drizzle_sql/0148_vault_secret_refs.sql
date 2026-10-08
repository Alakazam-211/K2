-- 0148: K2 vault (prd-k2-vault-v1 §10.4, §17, KV22, VS9).
--
-- The LLM keys people configured before the vault move into it. The main
-- DB keeps only a `k2v://` reference; the daemon moves existing values at
-- first boot (verified), then nulls the plaintext and rewrites the DB file
-- (secure_delete + VACUUM). The plaintext columns are dropped one release
-- later under a new number. Preset env values with sensitive names become
-- `k2v://` references in place (no column change).
ALTER TABLE projects ADD COLUMN anthropic_secret_ref TEXT;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN anthropic_secret_ref TEXT;
