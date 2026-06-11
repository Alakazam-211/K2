CREATE TABLE `agent_presets` (
	`id` text PRIMARY KEY NOT NULL,
	`label` text NOT NULL,
	`command` text NOT NULL,
	`icon` text,
	`enabled` integer DEFAULT 1 NOT NULL,
	`sort_order` integer DEFAULT 0 NOT NULL,
	`is_built_in` integer DEFAULT 0 NOT NULL,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL
);
--> statement-breakpoint
CREATE TABLE `projects` (
	`id` text PRIMARY KEY NOT NULL,
	`name` text NOT NULL,
	`path` text NOT NULL,
	`color` text DEFAULT '#3b82f6' NOT NULL,
	`tab_order` integer DEFAULT 0 NOT NULL,
	`last_opened_at` integer,
	`worktree_mode` integer DEFAULT 0 NOT NULL,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `projects_path_unique` ON `projects` (`path`);--> statement-breakpoint
CREATE TABLE `terminal_panes` (
	`id` text PRIMARY KEY NOT NULL,
	`tab_id` text NOT NULL,
	`split_direction` text,
	`split_ratio` real DEFAULT 0.5,
	`pane_order` integer DEFAULT 0 NOT NULL,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL,
	FOREIGN KEY (`tab_id`) REFERENCES `terminal_tabs`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE TABLE `terminal_tabs` (
	`id` text PRIMARY KEY NOT NULL,
	`workspace_id` text NOT NULL,
	`title` text DEFAULT 'Terminal' NOT NULL,
	`tab_order` integer DEFAULT 0 NOT NULL,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL,
	FOREIGN KEY (`workspace_id`) REFERENCES `workspaces`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE TABLE `workspaces` (
	`id` text PRIMARY KEY NOT NULL,
	`project_id` text NOT NULL,
	`type` text DEFAULT 'branch' NOT NULL,
	`branch` text,
	`name` text NOT NULL,
	`tab_order` integer DEFAULT 0 NOT NULL,
	`worktree_path` text,
	`created_at` integer DEFAULT (unixepoch()) NOT NULL,
	FOREIGN KEY (`project_id`) REFERENCES `projects`(`id`) ON UPDATE no action ON DELETE cascade
);
