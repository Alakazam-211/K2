-- 0262_mail_forward_volume — the owner's numbers for how much one mailbox may
-- forward (Rosson 2026-11-03; 0.46.8). NULL means the default.
--
-- `forward_copies_per_day`: copies per mailbox per trailing 24 hours, where K2
-- pauses its outside forwards (K2's own pass, `mail::forward_outcomes`).
-- `forward_copies_per_hour`: copies per mailbox per hour, K2 writes it as the
-- mail server's per-sender throttle (`mail::defaults::forward_throttles_at`).
-- ADD COLUMN only: renumber freely on a slip (and edit the migration-order
-- tests). The store is `crate::mail_forward`.
ALTER TABLE mail_server ADD COLUMN forward_copies_per_day INTEGER;
--> statement-breakpoint
ALTER TABLE mail_server ADD COLUMN forward_copies_per_hour INTEGER;
