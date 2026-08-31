-- Workspace agent LOGIN secret for sql_grants (0.40.128).
-- One cluster password per ws_<id>_agent: this column is the grantee-only
-- vault slot when that workspace owns no default-named database.
-- Same dbsec_* FileSecretStore as sql_databases.agent_secret_ref.
-- Nullable: pre-128 grants upgrade on first dsn/store/re-grant.
ALTER TABLE sql_grants ADD COLUMN agent_secret_ref TEXT;
