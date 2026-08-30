-- Skin OIDC issuer (prd-skin-oidc-hydra-v1 leftover 123) — K2-side
-- singleton for the Hydra sidecar. Template: 0108_sql.sql (id CHECK 1).
--
-- Enable skins ≠ start Hydra. Absence of the row is "off". Hydra stores
-- no users/passwords; subject = skin principal id (SQLite).
CREATE TABLE IF NOT EXISTS skin_hydra (
    id         INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    enabled    INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL
);
