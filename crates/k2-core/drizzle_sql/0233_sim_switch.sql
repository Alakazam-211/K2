-- 0233_sim_switch — mobile development (prd-mobile-dev-v1 §3.1, §8;
-- vs-live M-A11; 0.46.6). Additive.
--
-- 1. The per-workspace agent switch "Allow mobile devices"
--    (`projects.agents_can_use_sim`, DEFAULT 0 = off; Rosson 2026-10-27).
--    Written only by `POST /cli/agent-access/set {toggle: "sim"}` (Admin).
-- 2. `sim_sessions`: one row per workspace device (one device per
--    workspace in v1). The daemon owns the row; clients render it.
-- 3. `sim_audit`: one row per boot, stop, erase, install, take-over,
--    licence acceptance, idle shutdown and (later) node lease.
-- Times: INTEGER seconds since the Unix epoch (UTC).

ALTER TABLE projects ADD COLUMN agents_can_use_sim INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS sim_sessions (
  -- `s<id>` on the wire.
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id    TEXT NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
  lane          TEXT NOT NULL CHECK (lane IN ('android','ios','ios-node')),
  -- The lane's opaque handle (AVD name + port, UDID). NULL until booted.
  device_id     TEXT,
  -- DeviceProfile JSON (id, name, size, scale, os).
  profile_json  TEXT,
  -- Lane-private re-attach state (ports, token FILE path; never a secret).
  lane_state    TEXT NOT NULL DEFAULT '{}',
  state         TEXT NOT NULL CHECK (state IN ('installing','booting','running','stopping','stopped','failed')),
  reason        TEXT,
  pid           INTEGER,
  booted_at     INTEGER,
  -- Last agent verb or viewer input (the idle reaper's clock).
  last_used_at  INTEGER NOT NULL,
  -- Last time the device ran (data older than the limit is deleted).
  last_run_at   INTEGER,
  created_at    INTEGER NOT NULL,
  created_by    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS sim_sessions_state ON sim_sessions(state);

CREATE TABLE IF NOT EXISTS sim_audit (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  at          INTEGER NOT NULL,
  -- `owner`, a login, or `agent:<workspace>`.
  actor       TEXT NOT NULL,
  project_id  TEXT,
  lane        TEXT,
  -- boot | stop | erase | install | take_over | hand_back | idle_stop |
  -- licence_accept | setup | data_expired | adopt_lost | boot_failed
  action      TEXT NOT NULL,
  -- Small JSON: app id, profile, reason. Never a secret or typed text.
  detail      TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS sim_audit_project ON sim_audit(project_id, at);
