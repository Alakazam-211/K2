-- 0052 (0.39.39, #676 + #677.3): daemon-canonical tab titles +
-- workspace-layout tab-order revision.
--
-- #676 — `tab_titles`: today `setTabTitle` persists to per-renderer
-- layout state only, so a rename never reaches other windows / the
-- mobile companion. This table makes tab titles daemon-canonical,
-- keyed by (project_id, tab_id) — a tab is addressed by its id, not
-- by a workspace_id, so this is a dedicated table rather than a column
-- on workspace_layouts. The `POST /cli/workspace/set-tab-title` route
-- upserts here and broadcasts `TabTitleChanged`.
--
-- #677.3 — add a monotonic `revision` to `workspace_layouts` so
-- concurrent tab-order writes from multiple clients resolve
-- last-write-wins DETERMINISTICALLY. unixepoch() (`updated_at`) is
-- second-granular and collides under burst writes; an explicit
-- integer the daemon increments on every save is monotonic and
-- collision-free. Renderers carry the base revision and drop a write
-- whose base is behind the stored value.

CREATE TABLE IF NOT EXISTS `tab_titles` (
	`project_id` text NOT NULL,
	`tab_id` text NOT NULL,
	`title` text NOT NULL,
	`updated_at` integer DEFAULT (unixepoch()) NOT NULL,
	PRIMARY KEY (`project_id`, `tab_id`),
	FOREIGN KEY (`project_id`) REFERENCES `projects`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
ALTER TABLE `workspace_layouts` ADD `revision` integer DEFAULT 0 NOT NULL;
