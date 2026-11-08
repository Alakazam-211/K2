-- 0276: GPU jobs on K2 Compute (0.46.11, prd-compute-gpu-jobs-v1 S1, GH#79).
-- `allow_gpu`: this workspace may send `--gpu` jobs to this node. Set only by
-- the owner or an Admin (`k2 compute grant <node> --workspace WS --gpu`); an
-- agent asks with a `compute.gpu` card. Off for every grant (a new one and
-- every existing one); re-granting limits keeps the stored value (COALESCE,
-- like `allow_ci`). The node's own owner decides at the machine whether the
-- node runs GPU jobs for this server at all (`sudo k2-node admin gpu on`).
-- Renumber freely on a slip (and edit the migration-order tests). The store
-- is `crate::compute::store`.
ALTER TABLE compute_node_grants ADD COLUMN allow_gpu INTEGER NOT NULL DEFAULT 0;
