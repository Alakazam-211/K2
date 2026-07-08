-- K2 Mail S1 (prd-email-server-v1 §4.1/§5.2): the enable flow is an
-- idempotent, RESUMABLE state machine — each step (download, verify,
-- extract, unit, start, bootstrap sub-steps…) records completion here
-- so a crashed/interrupted enable picks up where it left off instead
-- of re-downloading or re-minting. Polled by GET /cli/mail/status
-- (the house long-operation pattern: persisted step status + poll).
ALTER TABLE mail_server ADD COLUMN enable_progress_json TEXT;
--> statement-breakpoint
-- The most recent supervisor error (enable step failure, health-check
-- degradation detail). Surfaced verbatim in `k2 mail status` +
-- Settings→Email so each step "surfaces its real error" (PRD §5.2).
ALTER TABLE mail_server ADD COLUMN last_error TEXT;
