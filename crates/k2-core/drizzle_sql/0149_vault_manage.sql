-- 0149: K2 vault (prd-k2-vault-v1 §17, KV16).
--
-- `projects.agents_can_manage_vault` — the per-agent "Allow managing
-- server-wide secrets" switch (`vault_manage`). Default 0 (off). With it
-- on, the workspace's agent may create server-wide secrets and share them
-- with OTHER workspaces (never its own). Written only by
-- POST /cli/agent-access/set (Admin), never by workspace/set.
ALTER TABLE projects ADD COLUMN agents_can_manage_vault INTEGER NOT NULL DEFAULT 0;
