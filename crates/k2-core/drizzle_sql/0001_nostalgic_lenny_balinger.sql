CREATE TABLE `workspace_sections` (
	`id` text PRIMARY KEY NOT NULL,
	`project_id` text NOT NULL,
	`name` text NOT NULL,
	`color` text,
	`is_collapsed` integer DEFAULT 0 NOT NULL,
	`tab_order` integer DEFAULT 0 NOT NULL,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL,
	FOREIGN KEY (`project_id`) REFERENCES `projects`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
ALTER TABLE `workspaces` ADD `section_id` text REFERENCES workspace_sections(id) ON DELETE SET NULL;