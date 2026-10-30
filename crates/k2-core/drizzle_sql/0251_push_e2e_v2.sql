-- 0251 — bigger sealed push buckets (push-e2e-v2, 0.46.7). Additive only.
--
-- `push_device_keys.v`: the sealed format the phone said it can open when
-- it registered its key (`e2e.v`): 1 = today's buckets (1536-byte alert,
-- 256-byte nudge), 2 = also the bigger ones (2880 / 1024). Every key
-- registered before this is 1.
ALTER TABLE push_device_keys ADD COLUMN v INTEGER NOT NULL DEFAULT 1;
--> statement-breakpoint
-- `push_relay_caps.s2`: the relay's last-seen `X-K2-Push-Caps` listed `s2`
-- (it accepts format-2 blobs), beside `s1`. Written only when it changes.
ALTER TABLE push_relay_caps ADD COLUMN s2 INTEGER NOT NULL DEFAULT 0;
