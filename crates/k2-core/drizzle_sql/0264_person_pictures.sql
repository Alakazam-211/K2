-- 0264: profile pictures for K2's own people (0.46.8, Rosson 2026-11-04):
-- the owner and every Connect login, per server (never synced from K2
-- Connect or other servers). `person_key` is `owner` or `connect:<id>` (the
-- login's never-reused account id). `image` is always K2's own JPEG
-- re-encode (agent_look::normalize_profile_image, at most 64 KiB); the
-- upload's bytes are never stored. Written only by POST /cli/people/picture
-- and POST /cli/people/picture/clear; a removed login's row goes with it.
-- The store is `crate::person_picture`.
CREATE TABLE IF NOT EXISTS person_pictures (
  person_key TEXT PRIMARY KEY NOT NULL,
  image BLOB NOT NULL,
  image_type TEXT NOT NULL CHECK (image_type = 'image/jpeg'),
  image_hash TEXT NOT NULL,
  set_at INTEGER NOT NULL,
  set_by TEXT NOT NULL
);
