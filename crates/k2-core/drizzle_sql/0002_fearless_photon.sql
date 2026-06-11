CREATE TABLE `focus_groups` (
	`id` text PRIMARY KEY NOT NULL,
	`name` text NOT NULL,
	`color` text,
	`tab_order` integer DEFAULT 0 NOT NULL,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL
);
--> statement-breakpoint
ALTER TABLE `projects` ADD `focus_group_id` text REFERENCES focus_groups(id) ON DELETE SET NULL;