-- 0181: agents run Gardens end to end (0.46.0, Rosson 2026-10-16).
--
-- `projects.agents_can_manage_gardens` — the per-agent "Manage Gardens and
-- data sources" switch (`gardens_manage`). Default 0 (off). With it on, the
-- workspace's agent may approve data sources (its own proposals and others'
-- on this server), bind a secret it may already use, pause/resume, pin/unpin,
-- approve and enable triggers, and edit Gardens and widgets, with no person
-- confirm; every such act is audited. Written only by
-- POST /cli/agent-access/set {toggle: "gardens_manage"} (Owner/Admin), never
-- by workspace/set, so an agent can never grant it to itself.
ALTER TABLE projects ADD COLUMN agents_can_manage_gardens INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint

-- `sources.secret_workspace` — the vault scope of the bound secret. NULL is
-- the server scope (every source before 0181, and every person's bind); a
-- workspace id is that workspace's own secret, bound by its agent.
ALTER TABLE sources ADD COLUMN secret_workspace TEXT CHECK (secret_workspace IS NULL OR length(secret_workspace) <= 128);
--> statement-breakpoint

-- Garden and widget changes an agent without the switch proposes; a person
-- (or an agent with it) approves or rejects. `kind` 'garden' writes
-- gardens/<target>.toml; 'widget' writes widgets/<target>/<file>.
-- `base_hash` is the sha256 of the file when it was proposed (empty: the
-- file didn't exist); approving a proposal whose file changed since is 409.
CREATE TABLE IF NOT EXISTS zen_proposals (
  id               TEXT PRIMARY KEY,
  kind             TEXT NOT NULL CHECK (kind IN ('garden', 'widget')),
  target           TEXT NOT NULL CHECK (length(target) BETWEEN 1 AND 64),
  file             TEXT CHECK (file IS NULL OR length(file) BETWEEN 1 AND 64),
  content          TEXT NOT NULL,
  base_hash        TEXT NOT NULL,
  note             TEXT CHECK (note IS NULL OR length(note) <= 500),
  status           TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected', 'withdrawn')),
  made_by          TEXT NOT NULL,
  made_by_project  TEXT NOT NULL,
  made_by_session  TEXT,
  created_at       INTEGER NOT NULL,
  decided_by       TEXT,
  decided_at       INTEGER,
  CHECK ((kind = 'widget') = (file IS NOT NULL)),
  CHECK ((status = 'pending') = (decided_at IS NULL))
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS zen_proposals_status ON zen_proposals(status, created_at);
