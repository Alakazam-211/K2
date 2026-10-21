-- 0211 — end-to-end encrypted phone pushes (prd-e2e-push-v1 rev 2 §8,
-- 0.46.3). Additive only, no backfill: until a phone registers a key,
-- every device keeps today's pushes byte for byte.
--
-- `push_device_keys`: at most one key per registered device (a device id
-- is per (install, saved server entry), so this is per (device, server)).
-- - `public_key`: the phone's P-256 public key, 65-byte SEC1 uncompressed.
-- - `psk`: the 32-byte HPKE pre-shared key the phone made (PSK mode). It
--   and the key arrive only over the E2E path, never through the relay.
--   Never echoed by any route; only `key_id` is.
-- - `key_id`: hex of the first 8 bytes of SHA-256(public_key).
-- - `next_ctr`: the replay counter. A push takes
--   `ctr = max(next_ctr, unix_ms)` and stores `ctr + 1` in one
--   UPDATE … RETURNING, so a restored backup can't rewind it.
-- - `expires_at`: receive time + 60 days; after that the device is
--   treated as unkeyed.
-- Every delete path on push_devices deletes the key row explicitly; the
-- foreign key is the second guard.
CREATE TABLE IF NOT EXISTS push_device_keys (
    device_id  TEXT PRIMARY KEY NOT NULL
               REFERENCES push_devices(device_id) ON DELETE CASCADE,
    suite      TEXT NOT NULL,
    public_key BLOB NOT NULL,
    psk        BLOB NOT NULL,
    key_id     TEXT NOT NULL,
    next_ctr   INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
--> statement-breakpoint
-- The owner's per-device test override: NULL (follow the server's
-- `pushE2E`) | 'off' | 'sealed'.
ALTER TABLE push_devices ADD COLUMN e2e_override TEXT;
--> statement-breakpoint
-- The relay's last-seen sealed capability (`X-K2-Push-Caps: s1`) per
-- gateway URL, so the first push after a restart is sealed too. Written
-- only when the value changes. Not a secret.
CREATE TABLE IF NOT EXISTS push_relay_caps (
    gateway_url TEXT PRIMARY KEY NOT NULL,
    s1          INTEGER NOT NULL,
    seen_at     INTEGER NOT NULL
);
