-- W5 (0.40.30) — provider metadata for the API-key BYO LLM credential, so a
-- `/v1` principal's stored key stages the RIGHT env var(s) for a workspace
-- whose configured agent is not claude (codex/gemini/grok/…).
--
-- `provider` — canonical lowercase provider name of the credential stored in
--   `anthropic_api_key` (that column name is historical; it holds THE LLM
--   credential, whatever the provider). Canonical values written by the
--   create route: 'anthropic' | 'openai' | 'google' | 'xai'.
--   NULL = anthropic (the pre-0071 behavior, byte-identical staging:
--   ANTHROPIC_API_KEY only) — existing rows backfill to NULL and keep
--   working unchanged. An UNRECOGNIZED value fails CLOSED at staging time:
--   nothing is staged (a structured warning is logged, never the credential).
--
-- `base_url` — optional provider endpoint override associated with this
--   credential (NOT a secret). Staged as OPENAI_BASE_URL when provider is
--   'openai' (the OpenAI-compatible-endpoint convention); ignored for other
--   providers. NULL = no override.
ALTER TABLE api_keys ADD COLUMN provider TEXT;
--> statement-breakpoint
ALTER TABLE api_keys ADD COLUMN base_url TEXT;
