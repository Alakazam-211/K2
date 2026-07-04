-- Presence/Multiplayer S7a (prd-presence-multiplayer-v1 §5.5) — pin-to-size.
--
-- A terminal session can be PINNED to fixed cols×rows daemon-side
-- (education/presentation: everyone sees the same grid; small windows
-- letterbox client-side). While pinned, EVERY resize path — grid-WS
-- Resize, typing input-claim snap, SetActive claim, detach-promotion —
-- is clamped to the pinned dims at the single `request_resize`
-- chokepoint on `DaemonPtySession`.
--
-- These columns make the pin CANONICAL + persistent: on session
-- (re)spawn, `v2_session_map::register` reads the row back and
-- re-applies the pin onto the fresh `DaemonPtySession`, so a pin
-- survives daemon restart. All three NULL = unpinned (the backfill
-- value for every existing row — non-retroactive by construction).
--
-- - `pinned_cols` / `pinned_rows`: the frozen grid geometry. Validated
--   at the write route (`POST /cli/terminal/pin-size`: 20..=500 cols,
--   5..=200 rows); readers treat any NULL/0 as unpinned.
-- - `pinned_set_by`: attribution — "owner" or the connect-user's
--   daemon-resolved username (never client-supplied; same D3 rule as
--   send-message `from`). Display-only in V1.
ALTER TABLE workspace_tab_sessions ADD COLUMN pinned_cols INTEGER;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN pinned_rows INTEGER;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN pinned_set_by TEXT;
