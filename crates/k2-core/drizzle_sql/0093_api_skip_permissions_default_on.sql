-- Host sessions: product default for api_skip_permissions flips to ON.
--
-- Context: /v1 host-sessions is headless. Default OFF stripped auto-approve
-- flags and stalled agents on the first tool HITL (no human to click).
-- Scout + owner: default ON; explicit 0 remains owner opt-out.
--
-- 0069 added a nullable INTEGER with no DEFAULT; NULL historically read as
-- OFF in get_api_skip_permissions. Backfill NULL → 1 so DB matches the new
-- default. Explicit 0 is preserved.
--
-- Read path also treats residual NULL as ON (see get_api_skip_permissions).

UPDATE projects
SET api_skip_permissions = 1
WHERE api_skip_permissions IS NULL;
