-- 0244_email_verified_addresses — Email Notifications part 2, slice V
-- (prd-email-notifications-part2-v1 §4 V, vs-live EN-A26; 0.46.6).
--
-- An address K2 may mail on someone's behalf only after its owner clicked
-- a confirm link. `purpose` says what the address is for:
--   * `person_notice` — a person on this box gets agent notices there
--     (N10); `subject_ref` is the person key ('owner' | a login).
--   * `forward`       — a hosted mailbox forwards to it (the forwarding
--     PRD); `subject_ref` is the mailbox address.
-- Tokens are kept as SHA-256 only (like k2inv_): `token_sha256` confirms
-- (POST only, once). Every mail to the address carries its own "stop these
-- emails" link, kept as SHA-256 in `email_verified_stops`. At most one live (pending or verified) row per
-- (purpose, subject_ref, address). New table only: renumber freely on a
-- slip (and edit the migration-order tests).
CREATE TABLE IF NOT EXISTS email_verified_addresses (
    id            TEXT PRIMARY KEY,
    address       TEXT NOT NULL,
    purpose       TEXT NOT NULL CHECK (purpose IN ('person_notice', 'forward')),
    subject_ref   TEXT NOT NULL,
    requested_by  TEXT NOT NULL,
    token_sha256  TEXT,
    state         TEXT NOT NULL CHECK (state IN ('pending', 'verified', 'revoked', 'expired')),
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    verified_at   INTEGER,
    revoked_at    INTEGER,
    revoked_by    TEXT,
    CHECK ((state = 'verified') = (verified_at IS NOT NULL)),
    CHECK ((state = 'revoked') = (revoked_at IS NOT NULL))
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS email_verified_addresses_live
    ON email_verified_addresses (purpose, subject_ref, address) WHERE state IN ('pending', 'verified');
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS email_verified_addresses_token
    ON email_verified_addresses (token_sha256) WHERE token_sha256 IS NOT NULL;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS email_verified_stops (
    stop_sha256  TEXT PRIMARY KEY,
    address_id   TEXT NOT NULL REFERENCES email_verified_addresses(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_verified_stops_address ON email_verified_stops (address_id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_verified_addresses_subject
    ON email_verified_addresses (purpose, subject_ref, state);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS email_verified_addresses_created
    ON email_verified_addresses (created_at);
