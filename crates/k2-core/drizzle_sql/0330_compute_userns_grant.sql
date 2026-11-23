-- 0330: user namespaces in compute jobs (0.46.18). `allow_userns`: this
-- workspace may send `k2 compute run --userns` jobs to this node (by default
-- a job can't make a user namespace). Set only by the owner or an Admin
-- (`k2 compute grant <node> --workspace WS --userns`) and only on a node this
-- server owns (never on a node where this server is a tenant). Off for every
-- grant (a new one and every existing one); re-granting limits keeps the
-- stored value (COALESCE, like `allow_gpu`). `granted_by` / `granted_at`
-- record who set it and when. Renumber freely on a slip (and edit the
-- migration-order tests). The store is `crate::compute::store`.
ALTER TABLE compute_node_grants ADD COLUMN allow_userns INTEGER NOT NULL DEFAULT 0;
