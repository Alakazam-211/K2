-- 0283: the Org Chart (0.46.11, prd-org-chart-v1 + Rosson's answers of
-- 2026-11-10). Orgs of agents, the head of each, the solid lines between
-- cards, the canvas positions a person placed, each org's 5-line bulletin,
-- who read the server bulletin on which day, the night runs, members'
-- updates and heads' asks, and "connect me to that head" asks.
--
-- Only adds things. The store and every rule are `crate::org_chart` (k2-core);
-- nothing here is read while the `orgChart` setting is off. 0283 is reserved
-- for this (wiki Ops - Migration numbers); renumber only with K2 and edit the
-- migration-order tests.
--
-- Rosson 2026-11-10: no frozen daily edition (the bulletin is live and an
-- agent reads it once per day), no role field (roles live in AGENTS.md), and
-- the purpose is a short paragraph (at most 400 characters, checked in code).

-- One row per org. A soft delete keeps the row so an id never comes back.
CREATE TABLE IF NOT EXISTS org_chart_orgs (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    name_key        TEXT NOT NULL,
    purpose         TEXT NOT NULL DEFAULT '',
    head_project_id TEXT,
    created_by      TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    deleted_at      INTEGER
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_org_chart_orgs_name ON org_chart_orgs (name_key) WHERE deleted_at IS NULL;
--> statement-breakpoint

-- Many to many on purpose: the one-org rule is code (MAX_ORGS_PER_AGENT).
CREATE TABLE IF NOT EXISTS org_chart_members (
    org_id     TEXT NOT NULL REFERENCES org_chart_orgs(id),
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    added_by   TEXT NOT NULL,
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (org_id, project_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_org_chart_members_project ON org_chart_members (project_id);
--> statement-breakpoint

-- Solid lines. 'below': a sits above b (top to bottom). 'beside': side by
-- side, stored with a < b so a pair has one row. An end is an agent or an org.
CREATE TABLE IF NOT EXISTS org_chart_lines (
    id         TEXT PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('below', 'beside')),
    a_kind     TEXT NOT NULL CHECK (a_kind IN ('agent', 'org')),
    a_id       TEXT NOT NULL,
    b_kind     TEXT NOT NULL CHECK (b_kind IN ('agent', 'org')),
    b_id       TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (a_kind, a_id, b_kind, b_id)
);
--> statement-breakpoint

-- Positions a person placed. A member's x/y is relative to its bucket; an
-- org's w/h is a person's resize (a minimum, never smaller than its cards).
-- Everything else is laid out by the daemon on every read.
CREATE TABLE IF NOT EXISTS org_chart_nodes (
    node_kind  TEXT NOT NULL CHECK (node_kind IN ('agent', 'org')),
    node_id    TEXT NOT NULL,
    x          REAL NOT NULL,
    y          REAL NOT NULL,
    w          REAL,
    h          REAL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (node_kind, node_id)
);
--> statement-breakpoint

-- 'revision': bumped by every change, so a stale canvas drag is refused.
CREATE TABLE IF NOT EXISTS org_chart_meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
--> statement-breakpoint
INSERT OR IGNORE INTO org_chart_meta (key, value) VALUES ('revision', '0');
--> statement-breakpoint

-- The org bulletin as it is now (live: an edit shows at once).
CREATE TABLE IF NOT EXISTS org_chart_bulletins (
    org_id     TEXT PRIMARY KEY NOT NULL REFERENCES org_chart_orgs(id),
    body       TEXT NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
--> statement-breakpoint

-- What members sent their head (`k2 organization update`). `night` is the
-- night it counted for (NULL: sent during the day, stored, no wake).
CREATE TABLE IF NOT EXISTS org_chart_updates (
    id              TEXT PRIMARY KEY NOT NULL,
    org_id          TEXT NOT NULL,
    from_project_id TEXT NOT NULL,
    body            TEXT NOT NULL,
    night           TEXT,
    woke            INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_org_chart_updates_org ON org_chart_updates (org_id, created_at);
--> statement-breakpoint

-- What a head asked its members (`k2 organization ask`), one row per member.
CREATE TABLE IF NOT EXISTS org_chart_asks (
    id              TEXT PRIMARY KEY NOT NULL,
    org_id          TEXT NOT NULL,
    from_project_id TEXT NOT NULL,
    to_project_id   TEXT NOT NULL,
    body            TEXT NOT NULL,
    woke            INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_org_chart_asks_from ON org_chart_asks (from_project_id, created_at);
--> statement-breakpoint

-- One row per org per night (the owner-timezone date the night starts on):
-- the ledger that makes each night wake happen once.
CREATE TABLE IF NOT EXISTS org_chart_nights (
    org_id           TEXT NOT NULL,
    night            TEXT NOT NULL,
    run_at           INTEGER,
    run_result       TEXT,
    last_call_at     INTEGER,
    last_call_result TEXT,
    PRIMARY KEY (org_id, night)
);
--> statement-breakpoint

-- Every org wake that reached an agent, for the hourly wake budget
-- (`crate::wake_budget::wakes_since`). Pruned after two days.
CREATE TABLE IF NOT EXISTS org_chart_wakes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('night_run', 'last_call', 'update', 'ask', 'reach')),
    at         INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_org_chart_wakes_project ON org_chart_wakes (project_id, at);
--> statement-breakpoint

-- Who read the server bulletin on which day. agent_address = the canonical
-- agent's project id, or a sidecar's handle (its own row).
CREATE TABLE IF NOT EXISTS org_chart_reads (
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    agent_address TEXT NOT NULL,
    day           TEXT NOT NULL,
    read_at       INTEGER NOT NULL,
    PRIMARY KEY (project_id, agent_address, day)
);
--> statement-breakpoint

-- "I need that head and I'm not connected": the ask and how it was filed.
-- `notified_at`: the asking agent was told the outcome (connected/declined).
CREATE TABLE IF NOT EXISTS org_chart_reach (
    id                    TEXT PRIMARY KEY NOT NULL,
    from_project_id       TEXT NOT NULL,
    to_project_id         TEXT NOT NULL,
    org_id                TEXT NOT NULL,
    why                   TEXT NOT NULL,
    via                   TEXT NOT NULL CHECK (via IN ('card', 'ticket')),
    permission_request_id TEXT,
    feedback_id           TEXT,
    state                 TEXT NOT NULL CHECK (state IN ('open', 'connected', 'declined', 'cancelled')),
    created_at            INTEGER NOT NULL,
    resolved_at           INTEGER,
    notified_at           INTEGER
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_org_chart_reach_open ON org_chart_reach (from_project_id, to_project_id) WHERE state = 'open';
--> statement-breakpoint

-- A connection between the pair, added by any path (a card, Settings, the
-- CLI, team links, a raw insert), closes the ask. The daemon tells the
-- agent and resolves the ticket on its next pass.
CREATE TRIGGER IF NOT EXISTS org_chart_reach_on_connection
AFTER INSERT ON workspace_relations
BEGIN
    UPDATE org_chart_reach
       SET state = 'connected', resolved_at = CAST(strftime('%s', 'now') AS INTEGER)
     WHERE state = 'open'
       AND ((from_project_id = NEW.source_project_id AND to_project_id = NEW.target_project_id)
         OR (from_project_id = NEW.target_project_id AND to_project_id = NEW.source_project_id));
END;
--> statement-breakpoint

-- A workspace removed by any path: its lines, positions and head seats go.
-- Members and reads go by their foreign keys.
CREATE TRIGGER IF NOT EXISTS org_chart_project_deleted
AFTER DELETE ON projects
BEGIN
    DELETE FROM org_chart_members WHERE project_id = OLD.id;
    DELETE FROM org_chart_lines
     WHERE (a_kind = 'agent' AND a_id = OLD.id) OR (b_kind = 'agent' AND b_id = OLD.id);
    DELETE FROM org_chart_nodes WHERE node_kind = 'agent' AND node_id = OLD.id;
    DELETE FROM org_chart_reads WHERE project_id = OLD.id;
    UPDATE org_chart_orgs SET head_project_id = NULL WHERE head_project_id = OLD.id;
    UPDATE org_chart_reach
       SET state = 'cancelled', resolved_at = CAST(strftime('%s', 'now') AS INTEGER)
     WHERE state = 'open' AND (from_project_id = OLD.id OR to_project_id = OLD.id);
    UPDATE org_chart_meta SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) WHERE key = 'revision';
END;
--> statement-breakpoint

-- The per-agent switch "Manage the org chart". Off by default; the Owner
-- turns it on (`organization.manage`).
ALTER TABLE projects ADD COLUMN agents_can_manage_org_chart INTEGER NOT NULL DEFAULT 0;
