-- W2 (0.40.30) — agent-preset METADATA: what the daemon needs to treat a
-- preset as more than a bare command string.
--
-- `danger_flags` — JSON array of strings: THIS preset's own dangerous
--   auto-approve flags (e.g. `["--dangerously-skip-permissions"]`). The
--   `/v1` host-session policy resolver strips the UNION of these and its
--   legacy hardcoded floor, so a custom agent's own auto-approve flag
--   (e.g. `--auto-yes`) fails CLOSED on API spawn instead of open.
-- `env` — JSON object of environment variables the preset's agent wants
--   in its child env at spawn. Merged UNDER explicit AGENT.md launch-block
--   env and K2-internal env (K2_HOOK_TOKEN etc.), OVER inherited shell env.
-- `readiness` — TEXT readiness class for the wake/injection path, the same
--   vocabulary as `provider_resume::InjectionProfile`:
--   'bracketed-paste' (the ?2004h flip is trustworthy) or 'settle:<ms>'
--   (?2004h lies; wait the settle floor).
--
-- All three NULLABLE, NO DEFAULT: NULL = legacy/unknown metadata — existing
-- rows backfill to NULL and consumers fail closed (danger-flag strip keeps
-- its hardcoded floor; readiness falls back to the default profile).
-- Built-in seeds get truthful values via `seed_agent_presets`' label-keyed
-- COALESCE backfill (never clobbers a non-NULL row).
ALTER TABLE agent_presets ADD COLUMN danger_flags TEXT;
--> statement-breakpoint
ALTER TABLE agent_presets ADD COLUMN env TEXT;
--> statement-breakpoint
ALTER TABLE agent_presets ADD COLUMN readiness TEXT;
