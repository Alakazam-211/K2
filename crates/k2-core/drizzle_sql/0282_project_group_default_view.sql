-- 0282: the view a project's agent tiles open in (prd-projects-threads-v1 §4.1,
-- 0.46.11). 'terminal' | 'thread' | 'chatter'; checked in k2-core
-- (`project_groups::set_group_default_view`), not SQL, the same as `color`
-- (0067). SQLite's constant DEFAULT fills every existing row, so every project,
-- old and new, starts on 'thread' with no backfill. 0282 is reserved for this
-- (0278–0281 are other branches'); renumber only with K2 (and edit the
-- migration-order tests). The store is `crate::project_groups`.
ALTER TABLE project_groups ADD COLUMN default_view TEXT NOT NULL DEFAULT 'thread';
