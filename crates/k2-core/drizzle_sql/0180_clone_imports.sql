-- 0180: a clone carries its Threads (prd-clone-carries-threads-and-vault-v1
-- §5.2, slice B; vs-live CL5/CL6/CL24). 0171–0176 are held for the 0.46.1
-- runtime lanes, 0177 = publish probe, 0178 = Changes review notes, 0179
-- unused. Additive only: the integrator renumbers freely on slips.
--
-- overlay_imports  = one row per Thread conversation that arrived with a
--                    clone (or `k2 thread import`). `local_conversation_id`
--                    is the pin id, a sidecar key, or `import:<uuid>` while
--                    the clone has no pinned Chat yet (`pending_canonical`,
--                    settled onto the pin the first time one is known) or
--                    when the sidecar's transcript did not travel
--                    (`detached`, read-only history). The doc-level dedupe
--                    index lives in redb (`import_ids`), not here.
-- clone_proposals  = agent-proposed clones held for a person (slice C,
--                    pattern of 0168 `app_widenings`). Names only, never a
--                    secret value.
CREATE TABLE IF NOT EXISTS overlay_imports (
  id                      TEXT PRIMARY KEY,
  project_id              TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  local_conversation_id   TEXT NOT NULL,
  role                    TEXT NOT NULL CHECK (role IN ('canonical','pending_canonical','sidecar','detached')),
  clone_id                TEXT NOT NULL,
  source_host             TEXT NOT NULL,
  source_handle           TEXT NOT NULL,
  source_conversation_id  TEXT NOT NULL,
  source_session_id       TEXT,
  harness                 TEXT,
  name                    TEXT,
  doc_count               INTEGER NOT NULL DEFAULT 0,
  skipped_count           INTEGER NOT NULL DEFAULT 0,
  imported_at             INTEGER NOT NULL DEFAULT (unixepoch()),
  settled_at              INTEGER,
  UNIQUE (project_id, clone_id, source_conversation_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_overlay_imports_pending ON overlay_imports(project_id) WHERE role = 'pending_canonical';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_overlay_imports_conv ON overlay_imports(local_conversation_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS clone_proposals (
  id            TEXT PRIMARY KEY,
  project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  proposed_by   TEXT NOT NULL,
  target        TEXT NOT NULL,
  options_json  TEXT NOT NULL,
  why           TEXT,
  status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','approved','denied','withdrawn','expired','done','failed')),
  decided_by    TEXT,
  decided_at    INTEGER,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  expires_at    INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_clone_proposals_open ON clone_proposals(project_id, status);
