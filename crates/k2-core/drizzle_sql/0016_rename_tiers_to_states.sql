-- Rename workspace_tiers table to workspace_states
ALTER TABLE workspace_tiers RENAME TO workspace_states;

--> statement-breakpoint

-- Update default state IDs from tier- prefix to state- prefix
UPDATE workspace_states SET id = REPLACE(id, 'tier-', 'state-') WHERE id LIKE 'tier-%';

--> statement-breakpoint

-- Update projects referencing old tier- IDs
UPDATE projects SET tier_id = REPLACE(tier_id, 'tier-', 'state-') WHERE tier_id LIKE 'tier-%';
