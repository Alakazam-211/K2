-- 0240: compute queue "N of M" (0.46.6, prd-compute-job-tiers-v2 L6, T25).
-- Provisional number: the integrator takes the next free one from
-- `Ops - Migration numbers`.
--
-- compute_jobs:
-- - `node_of`: how many jobs wait in the shared node's queue while this one
--   waits there at `node_pos` (`QueuePos.of`, sent by k2-node 0.46.6). NULL
--   from older nodes and for every job not in a node's queue; cleared with
--   `node_pos`.
ALTER TABLE compute_jobs ADD COLUMN node_of INTEGER;
