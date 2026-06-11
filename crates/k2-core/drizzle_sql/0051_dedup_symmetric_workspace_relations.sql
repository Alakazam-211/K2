-- 0051_dedup_symmetric_workspace_relations.sql
--
-- Phase 2.5b workspace==agent insight (0.39.0): a connection between
-- two workspaces now implies BIDIRECTIONAL awareness regardless of
-- which side initiated the relationship. Historical workspaces that
-- explicitly created BOTH A→B and B→A rows are now redundant — the
-- user-facing surfaces ([`crate::connections::list_peers`]) already
-- dedupe at the display layer (UNION outgoing + incoming), but the
-- storage layer should match so the count of `workspace_relations`
-- rows reflects actual distinct connections, not directional
-- bookkeeping.
--
-- Strategy: for each (source, target) pair where the reverse
-- (target, source) row ALSO exists, keep the earlier row (smaller
-- `created_at`, with `id` as deterministic tiebreaker) and DELETE
-- the later. If the two rows carry different `relation_type` values
-- (e.g. one is "peer" and the reverse is "collaborator"), merge the
-- distinct labels into the surviving row as semicolon-separated
-- values ("peer;collaborator") so no metadata is lost.
--
-- A future-side guard in `connections::connections("add", ...)` makes
-- the inverse no-op so this dedup work isn't undone by redundant
-- subsequent adds.
--
-- The UPDATE runs FIRST (still needs both rows present to merge the
-- types), then the DELETE collapses to a single row per pair. The
-- `id` column is TEXT (uuid), so ordering uses `created_at` with `id`
-- as a stable tiebreaker for rows created in the same second.

UPDATE workspace_relations AS keeper
SET relation_type = (
    SELECT GROUP_CONCAT(rt, ';')
    FROM (
        SELECT DISTINCT rt FROM (
            SELECT keeper.relation_type AS rt
            UNION
            SELECT later.relation_type AS rt
            FROM workspace_relations later
            WHERE later.source_project_id = keeper.target_project_id
              AND later.target_project_id = keeper.source_project_id
              AND (later.created_at, later.id) > (keeper.created_at, keeper.id)
        )
    )
)
WHERE EXISTS (
    SELECT 1 FROM workspace_relations later
    WHERE later.source_project_id = keeper.target_project_id
      AND later.target_project_id = keeper.source_project_id
      AND (later.created_at, later.id) > (keeper.created_at, keeper.id)
);

--> statement-breakpoint

DELETE FROM workspace_relations
WHERE id IN (
    SELECT later.id
    FROM workspace_relations later
    INNER JOIN workspace_relations keeper
      ON later.source_project_id = keeper.target_project_id
     AND later.target_project_id = keeper.source_project_id
     AND (later.created_at, later.id) > (keeper.created_at, keeper.id)
);
