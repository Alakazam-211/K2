-- 0114: published_services.kind + skin_root (prd-publish-skin-gateway-v1).
-- kind=cmd is a user shell; kind=skin is the official Dannon gateway helper
-- child (never a wiki-serve in-process bind). Existing rows stay cmd via
-- DEFAULT. skin_root empty = bundled UI. UNIQUE (project_id, name) unchanged.
ALTER TABLE published_services ADD COLUMN kind TEXT NOT NULL DEFAULT 'cmd'
    CHECK (kind IN ('cmd', 'skin'));
ALTER TABLE published_services ADD COLUMN skin_root TEXT NOT NULL DEFAULT '';
