-- Fix Maintenance state: Issues should be gated, Crashes and Security should be auto.
UPDATE `workspace_states` SET cap_crashes = 'auto', cap_security = 'auto' WHERE id = 'state-maintenance';
-- Also update description to match
UPDATE `workspace_states` SET description = 'Agents fix bugs and security issues automatically. Submitted issues and audits require approval. No new features.' WHERE id = 'state-maintenance';
