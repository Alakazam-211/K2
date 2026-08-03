-- Soft-disable for emergency kill without permanent revoke (Settings → K2 API Tokens).
-- NULL = enabled; non-NULL = disabled (resolve_api_key rejects; re-enable clears).
-- Revoked keys stay revoked_at; disabled is independent (cannot enable a revoked key).
ALTER TABLE api_keys ADD COLUMN disabled_at INTEGER;
