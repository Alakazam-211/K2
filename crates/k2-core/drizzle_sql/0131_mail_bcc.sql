-- 0131: blind copies on agent mail (Mara / Countertop Visualizer ask).
--
-- `projects.mail_always_bcc`      JSON array of addresses the OWNER wants
--                                 blind-copied on every send from this
--                                 workspace's agent (the "always BCC"
--                                 oversight policy). NULL = no policy.
--                                 Written only by the owner/admin config
--                                 path — NOT on the workspace/set
--                                 allowlist, so an agent can never set or
--                                 clear it.
-- `mail_outbound.bcc_json`        JSON array of BCCs the AGENT added
--                                 (`k2 mail send --bcc`). NULL = none.
-- `mail_outbound.policy_bcc_json` JSON array of BCCs the owner policy
--                                 added at send time. NULL = none.
--
-- BCCs ride the SMTP envelope only — never a Bcc header on the sent
-- message. ALTER only; never rebuild projects or mail_outbound.
ALTER TABLE projects ADD COLUMN mail_always_bcc TEXT;
--> statement-breakpoint
ALTER TABLE mail_outbound ADD COLUMN bcc_json TEXT;
--> statement-breakpoint
ALTER TABLE mail_outbound ADD COLUMN policy_bcc_json TEXT;
