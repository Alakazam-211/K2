-- Context hamburger: default system layers (AGENT / PROJECT / Tooling) become
-- toggleable. Default ON so new/existing workspaces keep current behavior.
-- Additive; fail-closed readers treat missing columns as 1 via COALESCE.

ALTER TABLE projects ADD COLUMN context_include_agent INTEGER NOT NULL DEFAULT 1;
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN context_include_project INTEGER NOT NULL DEFAULT 1;
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN context_include_tooling INTEGER NOT NULL DEFAULT 1;
