-- 0310: content-bound delivery records (prd-postal-in-k2-v1 S2, 0.46.16).
--
-- `msg_deliveries` (0161) is rebuilt, every row and column kept, so that:
--   * body_sha256: the SHA-256 (hex) of the text K2 delivered. `k2 msg
--     verify <code> --text-file f` says whether a text is that one, so a
--     real id copied into another body no longer "verifies".
--   * badge: the Postal SDK's `p_` + 20 hex code (native Postal
--     deliveries; NULL for K2's own `m_` ids).
--   * postal_msg_id: the Postal message id; at most one row each, so a
--     re-sent or re-pulled message is delivered once (P5, P18).
--   * sender_address / sender_fingerprint: who sent it, outside this
--     server (a Postal address, or a federated agent::host and its pinned
--     key's fingerprint).
--   * landed: where it landed (JSON), for Postal routing.
--   * via also takes `postal` and `federation` (P28): federated messages
--     now get a record and a badge too.
DROP TABLE IF EXISTS msg_deliveries_0310;
--> statement-breakpoint
CREATE TABLE msg_deliveries_0310 (
    id TEXT PRIMARY KEY,
    sender_workspace_id TEXT,
    sender_label TEXT,
    via TEXT NOT NULL CHECK (via IN ('passport', 'owner', 'api-key', 'postal', 'federation')),
    actor TEXT,
    on_behalf_of TEXT,
    target_workspace_id TEXT,
    target TEXT NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0,
    at INTEGER NOT NULL DEFAULT (unixepoch()),
    body_sha256 TEXT CHECK (body_sha256 IS NULL OR length(body_sha256) = 64),
    badge TEXT CHECK (badge IS NULL OR length(badge) = 22),
    postal_msg_id TEXT,
    sender_address TEXT,
    sender_fingerprint TEXT,
    landed TEXT
);
--> statement-breakpoint
INSERT INTO msg_deliveries_0310 (id, sender_workspace_id, sender_label, via, actor, on_behalf_of,
                                 target_workspace_id, target, delivered, at)
  SELECT id, sender_workspace_id, sender_label, via, actor, on_behalf_of,
         target_workspace_id, target, delivered, at
  FROM msg_deliveries;
--> statement-breakpoint
DROP TABLE msg_deliveries;
--> statement-breakpoint
ALTER TABLE msg_deliveries_0310 RENAME TO msg_deliveries;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS msg_deliveries_target ON msg_deliveries (target_workspace_id, at);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS msg_deliveries_postal_msg_id ON msg_deliveries (postal_msg_id) WHERE postal_msg_id IS NOT NULL;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS msg_deliveries_badge ON msg_deliveries (badge) WHERE badge IS NOT NULL;
