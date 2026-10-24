-- 0221: App runtime update continuity (prd-app-runtime-update-continuity-v1
-- rev 3.1 §7). 0.46.4. Lands AFTER 0220 (`k2 job`, branch k2/vault-jobs);
-- if 0220 slips, the integrator renumbers (never below a live number).
--
-- Additive only. Written once for both slices so no box records a partial
-- 0221:
-- * S1 (engine refresh and boot) reads and writes `deployments.error_code`:
--   the structured failure code (A4), so K2's own faults
--   (`engine_not_answering`, `engine_protocol_mismatch`,
--   `engine_caller_refused`, `helper_needed`) are told apart from the App's
--   without parsing the `error` text, and never feed the crash breaker.
-- * S2 (re-host) and S3 (daemon restarts) own the rest: the per-App
--   re-host policy and window (R14), the warm instance and its deadline
--   (R20), the runtime fingerprint (R10) and the last readiness answer
--   (R32). Nothing reads them until those slices land; `apps` rows read
--   through `APP_COLS` don't carry them yet (S2 adds them there).
--
-- `kind = 'rehost'` and `state = 'waiting_engine'` need no change: neither
-- column has a CHECK (0165).

ALTER TABLE apps ADD COLUMN rehost_policy TEXT NOT NULL DEFAULT 'auto'
  CHECK (rehost_policy IN ('auto','handover','window','manual','off'));
ALTER TABLE apps ADD COLUMN rehost_window TEXT;
-- Rosson 2026-10-24 (Q4 + Q16): when an update can't run two copies (no
-- room for both, extra ports, the process exception): `pause` = stop, then
-- start, with a short pause (default); `defer` = skip it and the owner
-- updates it (the owner card). The App panel's per-App toggle (S2).
ALTER TABLE apps ADD COLUMN rehost_when_no_overlap TEXT NOT NULL DEFAULT 'pause'
  CHECK (rehost_when_no_overlap IN ('pause','defer'));
ALTER TABLE apps ADD COLUMN warm_deployment TEXT;
ALTER TABLE apps ADD COLUMN warm_until INTEGER;
ALTER TABLE deployments ADD COLUMN runtime_json TEXT;
ALTER TABLE deployments ADD COLUMN ready_json TEXT;
ALTER TABLE deployments ADD COLUMN error_code TEXT;
