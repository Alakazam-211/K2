-- Projects V1 §6.7.7 — project-group icon + color (the collapsed-rail
-- identity, mirroring workspace icons).
--
-- A project group has NO folder on disk to detect an icon from (unlike
-- `projects.icon_url`, which falls back to a filesystem scan), so the
-- icon lives entirely IN the row: a `data:image/...` dataUrl string,
-- NULL = unset (the renderer falls back to initials + a stable derived
-- color). dataUrls run ~10-50KB, so the icon is deliberately EXCLUDED
-- from the list/show wire payloads and served by the dedicated
-- GET /cli/project-group/icon route instead (get-icon idiom).
--
-- `color` is a tiny `#rrggbb` accent (NULL = renderer derives a stable
-- fallback from the group id) and DOES ride the group wire shape.
ALTER TABLE project_groups ADD COLUMN icon TEXT;
--> statement-breakpoint
ALTER TABLE project_groups ADD COLUMN color TEXT;
