-- 0186: the remote switch covers agents on other servers only
-- (Rosson 2026-10-16).
--
-- Before: the roster served to paired peers listed a configured agent when
-- the app master `allowRemoteInstruct` was on, OR its own
-- `allow_remote_instruct` was on, OR its "Allow agents to create
-- connections" (`agents_can_create_connections`) was on. After: only the
-- master or `allow_remote_instruct` lists it.
--
-- So nothing goes hidden on upgrade (Rosson: upgrade keeps everyone's
-- current effective setting), every workspace that had connections on and
-- the switch off gets the switch on. It stays listed in every master state,
-- exactly as before, and peers never mark their links to it "gone".
-- Federated live text to such an agent is now delivered instead of
-- declined (that is what the switch means now). Runs once (tracked in
-- `_migrations`); a later owner choice is never overwritten.
--
-- Number: 0.46.1 lanes hold 0182–0185; 0186 was the integrator spare
-- (wiki `Ops - Migration numbers`). The integrator renumbers on slips.
UPDATE projects
   SET allow_remote_instruct = 1
 WHERE agents_can_create_connections = 1
   AND allow_remote_instruct = 0;
