-- Workspace status tiers: configurable capability bundles.
-- Each capability has three states: 'auto', 'gated', 'off'.
-- 'auto' = agents act and merge without asking
-- 'gated' = agents act but wait for human approval before merge
-- 'off' = agents don't touch this type of work

CREATE TABLE `workspace_tiers` (
  `id` text PRIMARY KEY NOT NULL,
  `name` text NOT NULL,
  `description` text,
  `is_built_in` integer NOT NULL DEFAULT 0,
  `cap_features` text NOT NULL DEFAULT 'off',
  `cap_issues` text NOT NULL DEFAULT 'off',
  `cap_crashes` text NOT NULL DEFAULT 'off',
  `cap_security` text NOT NULL DEFAULT 'off',
  `cap_audits` text NOT NULL DEFAULT 'off',
  `heartbeat` integer NOT NULL DEFAULT 1,
  `sort_order` integer NOT NULL DEFAULT 0,
  `created_at` integer DEFAULT (unixepoch()) NOT NULL
);

-- Link projects to their active tier
ALTER TABLE `projects` ADD COLUMN `tier_id` text REFERENCES `workspace_tiers`(`id`);

-- Seed the 4 default tiers
INSERT INTO `workspace_tiers` (`id`, `name`, `description`, `is_built_in`, `cap_features`, `cap_issues`, `cap_crashes`, `cap_security`, `cap_audits`, `heartbeat`, `sort_order`)
VALUES
  ('tier-build', 'Build', 'Full autonomy. Agents build, merge, and ship everything automatically.', 1, 'auto', 'auto', 'auto', 'auto', 'auto', 1, 0),
  ('tier-managed', 'Managed Service', 'Agents build everything. Features and audits require approval before merge. Crashes and security auto-ship.', 1, 'gated', 'auto', 'auto', 'auto', 'gated', 1, 1),
  ('tier-maintenance', 'Maintenance', 'Agents fix bugs and security issues. All work requires approval before merge. No new features.', 1, 'off', 'gated', 'gated', 'gated', 'gated', 1, 2),
  ('tier-locked', 'Locked', 'No agent activity. Fully manual or dormant.', 1, 'off', 'off', 'off', 'off', 'off', 0, 3);
