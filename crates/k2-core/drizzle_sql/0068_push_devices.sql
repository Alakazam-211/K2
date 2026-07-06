-- Companion C4 (prd-companion-v2 §4, feedback PRD §8.4 Option B) —
-- the daemon-held push-device registry.
--
-- A row is one mobile device that asked THIS daemon to push to it:
-- the companion re-registers on every app launch over the already-
-- authenticated K2 Connect transport (`/cli/push/register-device`,
-- upsert keyed by a stable device_id — which also absorbs APNs/FCM
-- token rotation for free). Tokens live HERE, in the user's own
-- SQLite, next to everything else they own; the relay gateway stays
-- stateless (§8.4 Option B — the locked design). Timestamps are unix
-- SECONDS (house convention, cf. 0064/0066).
--
-- Shape notes:
-- - `device_id` = a stable app-install identity minted by the
--   companion; the upsert key. NOT the vendor token (which rotates).
-- - `platform` ∈ 'apns' | 'fcm' — route-validated; core refuses
--   anything else loudly.
-- - `token` = the vendor routing handle (APNs device token / FCM
--   registration token). Indexed: gateway dead-token feedback
--   (`410 Unregistered` → dead:true) prunes BY TOKEN.
-- - `username` = the daemon-resolved authed user who registered the
--   device ('owner' | a connect-user name) — resolved from the
--   session token, NEVER the request body (the D3 discipline).
--   V1 dispatch fans out to ALL devices; per-user subscriptions
--   build on this column later.
-- - `last_seen_at` bumps on every (re-)register — the future
--   list/revoke UI's staleness signal.
CREATE TABLE IF NOT EXISTS push_devices (
    device_id    TEXT PRIMARY KEY NOT NULL,
    platform     TEXT NOT NULL,
    token        TEXT NOT NULL,
    username     TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
);
--> statement-breakpoint
-- The prune hot path: the gateway reports dead TOKENS, not device ids.
CREATE INDEX IF NOT EXISTS idx_push_devices_token ON push_devices (token);
