-- Team links + sender check (0.45.6, prd-agent-hire-templates-v1 S6/S7).
--
-- team_links: a K2-side record that one team serves (or helps) a
-- workspace, keyed on ids (a rename never touches it). Creating a link
-- writes workspace_relations rows tagged origin_link_id = the link id;
-- removing it deletes exactly those rows, so hand-made connections
-- survive, and `connections remove` / `relations/delete` refuse a
-- link-made row (409 managed_by_link). Internal team edges written at hire
-- carry origin_link_id = 'inst:<instance id>'.
CREATE TABLE IF NOT EXISTS team_links (
    id TEXT PRIMARY KEY,
    instance_id TEXT,
    team_group_id TEXT,
    team_lead_workspace_id TEXT NOT NULL,
    consumer_workspace_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('serves', 'helper_of')),
    topology TEXT NOT NULL DEFAULT 'lead' CHECK (topology IN ('lead', 'mesh')),
    scope_json TEXT,
    created_by_actor TEXT NOT NULL,
    api_key_id TEXT,
    on_behalf_of TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'orphaned', 'removed')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    removed_at INTEGER
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS team_links_consumer ON team_links (consumer_workspace_id, status);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS team_links_lead ON team_links (team_lead_workspace_id, status);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS team_links_instance ON team_links (instance_id);
--> statement-breakpoint
ALTER TABLE workspace_relations ADD COLUMN origin_link_id TEXT;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS workspace_relations_origin ON workspace_relations (origin_link_id);
--> statement-breakpoint
-- msg_deliveries: one row per live `k2 msg` / `/v1 … /message` delivery
-- (HT15). The recipient sees `[from <sender> · <id>]`; `k2 msg who <id>`
-- returns the credential-derived sender, so a body can't fake one.
--   via = passport (an agent's own session token), owner (the owner token
--   or a Connect login), api-key (a K2 v1 API key).
CREATE TABLE IF NOT EXISTS msg_deliveries (
    id TEXT PRIMARY KEY,
    sender_workspace_id TEXT,
    sender_label TEXT,
    via TEXT NOT NULL CHECK (via IN ('passport', 'owner', 'api-key')),
    actor TEXT,
    on_behalf_of TEXT,
    target_workspace_id TEXT,
    target TEXT NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0,
    at INTEGER NOT NULL DEFAULT (unixepoch())
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS msg_deliveries_target ON msg_deliveries (target_workspace_id, at);
