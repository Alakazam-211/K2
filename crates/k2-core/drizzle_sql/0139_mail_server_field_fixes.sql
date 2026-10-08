-- 0139: hosted-mail field fixes (K2 0.45.1, prd-hostmail-field-fixes-0451-v1).
--
-- public_ipv4 / public_ipv4_at: the box's public IPv4 (the egress address
--   checkip/ipify report) and when it was read (unix seconds). Written by
--   enable, `domain add`, `domain check` and the doctor; never under
--   K2_AIRGAP. The DNS record table's "A for the mail host" row reads it,
--   so building that table never touches the network. NULL = never read.
-- outbound_json: ONLY what K2 itself changed in Stalwart's registry, each
--   entry with `from` and `at` — never a copy of live state (status reads
--   Stalwart). {"mxRoute":{id,from,to,at}, "dane":[{id,name,from,at}],
--   "spamUrl":{from,to,at}, "spamRepair":[{id,name,at}],
--   "acmeRetry":{domainId,taskId,at}, "pendingReload":{at,error}}.
--   A recorded change is never made again, and a value a person set is
--   never undone (HF6/HF7). NULL = K2 never changed anything.
-- Additive ALTERs; one per statement so a re-run skips a duplicate
-- column without skipping the rest.
ALTER TABLE mail_server ADD COLUMN public_ipv4 TEXT;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN public_ipv4_at INTEGER;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN outbound_json TEXT;
