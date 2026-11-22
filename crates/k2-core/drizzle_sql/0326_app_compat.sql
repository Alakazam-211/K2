-- 0326: the Supabase-compatible data layer, one App at a time
-- (prd-app-supabase-compat-v1 §5, §6; lane A). Off by default: a row here
-- means `k2 app compat on <app> --db <db>` turned it on.
--
-- app_compat: the database the doors bind to, the door login
-- (`<db>_authenticator`) and the db secret-store refs of its password and of
-- the database's claims key (`k2.set_claims`), the anon lane, the server
-- key on the public host, origins, exposed schemas, limits (JSON), and the
-- third-party token rules (`sub` = the issuer's id or the user's; an email
-- that is no App user refused or run as authenticated). Lane C reads it
-- through `apps::compat::door_config::for_app`.
--
-- app_identity_issuers: the third-party issuers an App trusts (§6), each
-- pinned to one audience and one key-set URL. `issuer_key` names the claim
-- that keeps the issuer's own id under `sub user` (`<issuer_key>_uid`).
CREATE TABLE IF NOT EXISTS app_compat (
    app_id                   TEXT PRIMARY KEY NOT NULL,
    database_id              TEXT NOT NULL,
    database_name            TEXT NOT NULL,
    authenticator_role       TEXT NOT NULL,
    authenticator_secret_ref TEXT NOT NULL,
    claims_key_ref           TEXT NOT NULL,
    anon                     INTEGER NOT NULL DEFAULT 0,
    service_key_public       INTEGER NOT NULL DEFAULT 0,
    origins                  TEXT NOT NULL DEFAULT '[]',
    schemas                  TEXT NOT NULL DEFAULT '["public"]',
    limits                   TEXT NOT NULL DEFAULT '{}',
    sub_mode                 TEXT NOT NULL DEFAULT 'token' CHECK (sub_mode IN ('token', 'user')),
    unknown_email            TEXT NOT NULL DEFAULT 'refuse' CHECK (unknown_email IN ('refuse', 'authenticated')),
    users_synced_at          INTEGER,
    created_by               TEXT NOT NULL,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_app_compat_database ON app_compat (database_name);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_identity_issuers (
    app_id     TEXT NOT NULL,
    issuer_key TEXT NOT NULL,
    issuer     TEXT NOT NULL,
    audience   TEXT NOT NULL,
    jwks_url   TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (app_id, issuer)
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_app_identity_issuers_key ON app_identity_issuers (app_id, issuer_key);
