-- 0311: Postal handles and friends (prd-postal-in-k2-v1 S3, §5.6, 0.46.16).
--
-- postal_handles: a workspace's Postal address (one per workspace in v1,
-- Rosson's 11-08 answer 3). `status` is `local` until the Postal network
-- confirms the claim (`claimed`), then `released` when given up. The
-- identity key lives in the vault (`key_name`), never here.
--
-- postal_friends: one row per (handle, peer). Every request, incoming or
-- outgoing, waits for a person's card (Rosson ruling 5): `pending_in` /
-- `pending_out`, then `accepted`, `rejected`, `revoked` (removed),
-- `blocked`, or `rekey_pending` when a friend's key changes. `pair_id` is
-- the Postal network's request id (NULL until it has one). No message
-- bodies here: those live where `k2 msg` bodies live.
CREATE TABLE IF NOT EXISTS postal_handles (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    address TEXT NOT NULL UNIQUE CHECK (length(address) BETWEEN 3 AND 200),
    key_name TEXT,
    route TEXT,
    status TEXT NOT NULL DEFAULT 'local' CHECK (status IN ('local', 'claimed', 'released')),
    claimed_by TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS postal_handles_workspace ON postal_handles (workspace_id) WHERE status != 'released';
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS postal_friends (
    id TEXT PRIMARY KEY,
    handle_id TEXT NOT NULL REFERENCES postal_handles (id) ON DELETE CASCADE,
    peer_address TEXT NOT NULL CHECK (length(peer_address) BETWEEN 3 AND 200),
    peer_typ TEXT,
    peer_fingerprint TEXT,
    peer_key_pem TEXT,
    pinned_at INTEGER,
    sas_version INTEGER,
    pair_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending_in', 'pending_out', 'accepted', 'rejected', 'revoked', 'blocked', 'rekey_pending')),
    route TEXT,
    owner_name TEXT,
    note TEXT,
    asked_by TEXT,
    decided_by TEXT,
    decided_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (handle_id, peer_address)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS postal_friends_status ON postal_friends (handle_id, status);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS postal_friends_pair ON postal_friends (pair_id) WHERE pair_id IS NOT NULL;
