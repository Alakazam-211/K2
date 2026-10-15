-- 0166: Sew on K2 — the supervisor's singleton row and the per-agent
-- "Manage Sew" switch (Sew S29 work items K11/K12; plan-sew-s27-s29 §1,
-- plan-sew-s30-s32 K19). 0165 is the App runtime core on this base, so
-- Sew takes the next number; the integrator renumbers on slips (never
-- below a number already live).
--
-- `sew_server` — one row (id = 1) once `k2 hostchat enable` has started.
-- No row = not installed. `status` is what K2 last recorded; GET
-- /cli/sew/status overlays systemd (never `running` unless both Sew units
-- are active). Master keys live in the K2 vault (system namespace, scope
-- engine:sew); this row only names their refs. `app_url` is folded in
-- now for S32 (the plan's ask: no extra migration later).
CREATE TABLE IF NOT EXISTS sew_server (
  id                     INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  status                 TEXT NOT NULL CHECK (status IN
                           ('installing','running','degraded','stopped','disabled',
                            'error','upgrading','removed')),
  pinned_version         TEXT NOT NULL,
  installed_version      TEXT,
  previous_version       TEXT,
  install_id             TEXT,
  master_key_ref         TEXT,
  master_key_created_at  INTEGER,
  master_next_ref        TEXT,
  master_prev_ref        TEXT,
  recovery_shown_at      INTEGER,
  host                   TEXT,
  host_mode              TEXT CHECK (host_mode IN ('direct','connect')),
  app_url                TEXT,
  mirror_kind            TEXT NOT NULL DEFAULT 'none' CHECK (mirror_kind IN ('none','dir')),
  mirror_path            TEXT,
  ship_kind              TEXT NOT NULL DEFAULT 'none' CHECK (ship_kind IN ('none','dir')),
  ship_path              TEXT,
  last_shipped_id        TEXT,
  last_shipped_at        INTEGER,
  last_ship_error        TEXT,
  enable_progress_json   TEXT,
  upgrade_progress_json  TEXT,
  rotate_progress_json   TEXT,
  restore_progress_json  TEXT,
  doctor_json            TEXT,
  doctor_at              INTEGER,
  last_error             TEXT,
  installed_at           INTEGER,
  updated_at             INTEGER NOT NULL
);

-- "Manage Sew" (`chat_manage`): OFF by default (Rosson 2026-10-15). On
-- lets this workspace's agent run `k2 hostchat` (the server on this box)
-- and the community-management verbs. Written only by POST
-- /cli/agent-access/set {toggle: "chat_manage"} (Owner/Admin) or the hire
-- API's agent settings; never by workspace/set, never by the agent itself.
ALTER TABLE projects ADD COLUMN agents_can_manage_chat INTEGER NOT NULL DEFAULT 0;
