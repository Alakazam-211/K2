-- GH#22/#23/#24: heal junk per-project heartbeat schedules written by
-- pre-0.40.41 CLIs.
--
-- The k2 CLI misparsed `--help` (and bare subcommand words like "add",
-- "list", "remove") on heartbeat subcommands as a schedule frequency
-- and POSTed them to the legacy /cli/heartbeat/schedule route, which
-- wrote them verbatim into projects.heartbeat_mode /
-- heartbeat_schedule / heartbeat_enabled with success:true. The mode
-- was usually a valid 'scheduled' with garbage in the schedule JSON's
-- $.frequency; some shapes poisoned the mode itself.
--
-- Reset every poisoned row to a clean disabled state:
--   * heartbeat_mode outside off/hourly/scheduled
--   * heartbeat_mode = 'scheduled' whose schedule JSON is missing,
--     malformed, or whose $.frequency is not one of
--     daily/weekly/monthly/yearly
--
-- json_valid() guards json_extract() (SQLite AND short-circuits
-- left-to-right) so a malformed heartbeat_schedule can't abort the
-- migration; IFNULL folds a missing $.frequency into the junk bucket
-- (a bare NULL IN (...) would evaluate to NULL and silently skip the
-- row). The route now rejects these writes (misc_routes.rs
-- validate_heartbeat_schedule_write), so healed rows stay healed.
UPDATE projects
SET heartbeat_mode = 'off',
    heartbeat_schedule = NULL,
    heartbeat_enabled = 0
WHERE heartbeat_mode NOT IN ('off', 'hourly', 'scheduled')
   OR (
        heartbeat_mode = 'scheduled'
        AND NOT (
            heartbeat_schedule IS NOT NULL
            AND json_valid(heartbeat_schedule)
            AND IFNULL(json_extract(heartbeat_schedule, '$.frequency'), '')
                IN ('daily', 'weekly', 'monthly', 'yearly')
        )
   );
