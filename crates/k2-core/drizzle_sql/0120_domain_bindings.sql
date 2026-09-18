-- 0120: host-wide custom-domain inventory (prd-custom-domains-and-certs-v1 A1).
-- Binding an apex to THIS daemon is not NS transfer. dns_write=1 only when
-- the control plane bind succeeded for a K2-hosted zone. Hostnames are
-- FQDNs under an apex (or the apex itself). No FK: dangling apex on a
-- name row is treated as missing at read time (same posture as 0074).
CREATE TABLE IF NOT EXISTS domain_bindings (
    apex TEXT PRIMARY KEY NOT NULL,
    zone_id TEXT,
    dns_write INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS domain_names (
    hostname TEXT PRIMARY KEY NOT NULL,
    apex TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'other'
);
