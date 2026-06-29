-- P3a (sandbox / K2-as-a-server) — first-class API-key auth tier for the
-- external `/v1/*` surface.
--
-- An API key is a high-entropy random credential (`k2sk_<base62>`) the OWNER
-- mints so an external caller can authenticate to the public sandbox API. It
-- authorizes `/v1/*` and mints a host-side Principal (P3b's policy-resolver
-- input); it can NEVER manage keys (mint/list/revoke are owner-token-only).
--
-- HASH-AT-REST (key_hash): we store SHA-256(raw key) hex, NOT argon2. argon2
-- is for LOW-entropy human passwords (slow-by-design to blunt brute force). An
-- API key is 256+ bits of CSPRNG output — there is no dictionary to grind, so
-- a fast cryptographic hash is the CORRECT primitive (mirrors how connect_users
-- stores SHA-256 *session-token* digests, also high-entropy, vs argon2 for
-- passwords). The raw key is returned to the owner exactly ONCE at create and
-- is never recoverable from this table.
--
-- anthropic_api_key (nullable): the BYO LLM credential associated with THIS
-- key — staged (B3a-style) as ANTHROPIC_API_KEY into the ephemeral session a
-- `/v1/*` call spawns under this principal. PLAINTEXT at rest like the B3a
-- per-workspace column (0056): the box DB is root-only; at-rest encryption is
-- a follow-up. NEVER logged/echoed; list never returns it (only a boolean).
--
-- scope (TEXT, default 'owner'): the principal's authorization scope. Only
-- 'owner' exists today (own-use); a column NOW so per-tenant scopes drop in
-- for P4 untrusted tenancy without a migration.
--
-- revoked_at (nullable INTEGER): revocation is IMMEDIATE + durable — once set,
-- resolve_api_key skips the row (a non-NULL revoked_at row never authorizes).
-- Rows are retained (audit) rather than deleted.
CREATE TABLE IF NOT EXISTS api_keys (
    id                TEXT PRIMARY KEY NOT NULL,
    key_hash          TEXT NOT NULL,
    label             TEXT,
    anthropic_api_key TEXT,
    scope             TEXT NOT NULL DEFAULT 'owner',
    created_at        INTEGER NOT NULL,
    revoked_at        INTEGER
);
--> statement-breakpoint
-- resolve_api_key looks up a presented key by its SHA-256 digest; index it so
-- the hot auth path is an index probe, not a table scan.
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys (key_hash);
