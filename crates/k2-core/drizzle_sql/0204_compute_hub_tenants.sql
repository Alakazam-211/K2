-- 0204: the compute hub's tenants (prd-compute-shared-queue-v1 S1, §6.4;
-- 0.46.3, Rosson 2026-10-20). New tables only, plus the `local` tenant row.
--
-- compute_tenants        who may submit to this server's queue. `local` =
--                        this server's own workspaces (today's passport path,
--                        unchanged). `server` = another K2 server, bound to
--                        its federation fingerprint (`peer_fp`, paired with
--                        pairing v2 and granted the peer capability
--                        `compute`). `apikey` is reserved for S8 devices.
--                        `trust` `team` is refused until the runtime wall
--                        lands (S6): only `own` tenants for now (Q3).
--                        `epoch` bumps on revoke; every signed hub request
--                        carries it (V25). `weight`/`top_class`/quotas are
--                        read by the fair queue (S3). `blob_mb` caps the
--                        tenant's blob store (NULL = the default).
-- compute_tenant_grants  per tenant, per node: which nodes a server tenant
--                        may use and with what limits (compute_node_grants
--                        stays for `local`, V30).
-- compute_blobs          the per-tenant blob store index (S2): uploaded
--                        bundles and --dirty blobs, content-addressed, under
--                        ~/.k2/compute/tenants/<id>/blobs/<sha256>.
CREATE TABLE IF NOT EXISTS compute_tenants (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('local','server','apikey')),
  label TEXT NOT NULL,
  peer_fp TEXT UNIQUE,
  trust TEXT NOT NULL DEFAULT 'own' CHECK (trust IN ('own','team')),
  weight INTEGER NOT NULL DEFAULT 1,
  top_class TEXT NOT NULL DEFAULT 'interactive' CHECK (top_class IN ('gate','interactive','batch')),
  max_running INTEGER,
  max_queued INTEGER,
  cpu_hours_day INTEGER,
  log_gb INTEGER,
  artifact_days INTEGER,
  blob_mb INTEGER,
  state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active','suspended','revoked')),
  epoch INTEGER NOT NULL DEFAULT 1,
  created_by TEXT,
  created_at INTEGER
);
--> statement-breakpoint
INSERT OR IGNORE INTO compute_tenants (id, kind, label, trust, weight, top_class, state, epoch, created_by, created_at)
VALUES ('local', 'local', 'local', 'own', 1, 'interactive', 'active', 1, 'migration', CAST(strftime('%s','now') AS INTEGER));
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_tenant_grants (
  tenant_id TEXT NOT NULL REFERENCES compute_tenants(id) ON DELETE CASCADE,
  node_id TEXT NOT NULL REFERENCES compute_nodes(id) ON DELETE CASCADE,
  max_parallel INTEGER NOT NULL DEFAULT 1,
  max_job_secs INTEGER NOT NULL DEFAULT 7200,
  max_disk_gb INTEGER NOT NULL DEFAULT 60,
  max_queued INTEGER NOT NULL DEFAULT 10,
  granted_by TEXT NOT NULL,
  granted_at INTEGER NOT NULL,
  PRIMARY KEY (tenant_id, node_id)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS compute_blobs (
  tenant_id TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('bundle','dirty')),
  bytes INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  last_used INTEGER NOT NULL,
  PRIMARY KEY (tenant_id, sha256)
);
