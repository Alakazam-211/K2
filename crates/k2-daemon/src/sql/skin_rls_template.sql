-- Optional dump template for skin row isolation via GUC
-- (prd-skin-chunk-db-rls-v1). Copy into the workspace
-- `.k2/db/migrations` only if you want this policy in the dump.
-- `k2 db create` does NOT apply this file and does not SET ROLE.
--
-- K2 stamps `k2.skin_principal` on session store and dump DML:
--   SELECT set_config('k2.skin_principal', '<uuid>', true)
-- LOGIN is the workspace agent (`ws_*_agent`). Guests never hold a DSN.
-- Do not mint a per-dentist Postgres role. Do not FORCE RLS.
--
-- subject / principal id = the K2 skin principal UUID from SQLite
-- (not Hydra, not Connect). Hydra `sub` (when the OIDC sidecar is on)
-- is that same id.
--
-- create_database GRANTs to ws_*_agent stay unchanged.

CREATE SCHEMA IF NOT EXISTS k2;
GRANT USAGE ON SCHEMA k2 TO PUBLIC;
CREATE OR REPLACE FUNCTION k2.skin_uid() RETURNS uuid
LANGUAGE sql
STABLE
AS $$
  SELECT nullif(current_setting('k2.skin_principal', true), '')::uuid
$$;
GRANT EXECUTE ON FUNCTION k2.skin_uid() TO PUBLIC;

-- Example POLICY (dentist A and dentist B see different rows because
-- their session GUC differs — not because they hold different roles):
--
-- ALTER TABLE public.example ENABLE ROW LEVEL SECURITY;
--
-- CREATE POLICY skin_own_rows ON public.example
--   FOR SELECT
--   USING (principal_id = k2.skin_uid());
--
-- CREATE POLICY skin_own_rows_write ON public.example
--   FOR ALL
--   USING (principal_id = k2.skin_uid())
--   WITH CHECK (principal_id = k2.skin_uid());
--
-- Do not FORCE RLS on public.example (K2 migrate refuses FORCE).
