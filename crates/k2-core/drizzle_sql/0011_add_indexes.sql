CREATE INDEX IF NOT EXISTS `workspaces_project_id` ON `workspaces` (`project_id`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `terminal_tabs_workspace_id` ON `terminal_tabs` (`workspace_id`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `terminal_panes_tab_id` ON `terminal_panes` (`tab_id`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `time_entries_project_id` ON `time_entries` (`project_id`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `workspace_sections_project_id` ON `workspace_sections` (`project_id`);
