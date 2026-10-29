-- 0251_sidecar_worktrees — Workbench W1, worktree sidecars
-- (research-t3code-ui-as-garden-v1-vs-live §2 E1/E5/E6/E9, slices B1 + B3;
-- Rosson 2026-10-29: "build this as a garden experimentally first"; 0.46.7).
--
-- Why: `k2 sidecar new --worktree` runs a sidecar in its own git worktree
-- (`.worktrees/k2-<name>`, branch `k2/<name>`), and a person Lands its
-- commits on the workspace's main branch (a cherry-pick) and then Retires
-- it (stop, worktree to the Trash, branch deleted only when landed or
-- discarded). The daemon has to remember, per sidecar tab row:
--   * which worktree it runs in: the `workspaces` row K2 registered for it
--     (`worktree_ws_id`; NULL again once retired, the row is dropped), and
--     the branch and the folder relative to the workspace root (kept after
--     retire so the inbox can still say what it was);
--   * what it was cut from (`base_ref`: the branch Land puts it back on);
--   * when it landed and the main-branch commit it left (`landed_at`,
--     `landed_head`), and when and how it was retired (`retired_at`,
--     `retired_how`: 'landed' or 'discarded').
-- From these and git, the daemon derives the inbox state (new, working,
-- review, landed, discarded) it serves in `GET /cli/sidecar/list`.
--
-- Every column is NULL on a plain sidecar and on every row that existed
-- before. ADD COLUMN only, no backfill: renumber freely on a slip (and edit
-- the migration-order tests). The store is `crate::worktree_task::store`.
ALTER TABLE workspace_tab_sessions ADD COLUMN worktree_ws_id TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN worktree_branch TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN worktree_dir TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN base_ref TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN landed_at INTEGER;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN landed_head TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN retired_at INTEGER;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN retired_how TEXT;
