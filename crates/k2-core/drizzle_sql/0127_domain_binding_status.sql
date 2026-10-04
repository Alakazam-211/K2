-- 0127: custom-domain attach A8.1 — zone status on the local binding.
-- k2.dev `POST /api/dns/zones/bind` now answers
-- `{zoneId, status, nameservers, dnsWrite, created?}`. A zone k2.dev
-- just auto-added starts `pending_ns` (dnsWrite=false) until its
-- registrar nameservers point at k2.dev; `k2 domain refresh` re-binds
-- and flips it to `active`. Columns only — never rebuild
-- `domain_bindings`.
--
-- `status`        `active` | `pending_ns` from the control plane. NULL for
--                 BYO rows (not owned / unpaired) and rows bound before A8.1.
-- `nameservers`   JSON array of the nameservers k2.dev wants at the
--                 registrar (`["ns1.k2.dev","ns2.k2.dev"]`). NULL = none.
-- `auto_created`  1 when k2.dev created the zone in this account on bind
--                 (`created:true`).
ALTER TABLE domain_bindings ADD COLUMN status TEXT;
--> statement-breakpoint
ALTER TABLE domain_bindings ADD COLUMN nameservers TEXT;
--> statement-breakpoint
ALTER TABLE domain_bindings ADD COLUMN auto_created INTEGER NOT NULL DEFAULT 0;
