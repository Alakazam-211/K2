//! Numeric limits and rate-limit defaults (C16 and the numbers elsewhere in C1–C18).
//!
//! Durations are milliseconds. Each constant cites its contract.

// ---- Messages (C5, C16) ----

/// Largest message body in UTF-8 bytes (C16: 16 KiB; more → `413 too_large`).
pub const MESSAGE_BODY_MAX_BYTES: usize = 16_384;
/// Attachments per message (C16).
pub const ATTACHMENTS_PER_MESSAGE_MAX: usize = 10;
/// Distinct reactions per message (C16).
pub const DISTINCT_REACTIONS_PER_MESSAGE_MAX: usize = 50;
/// Link previews per message (C16).
pub const LINK_PREVIEWS_PER_MESSAGE_MAX: usize = 3;
/// Longest client `nonce` in Unicode scalar values (C5).
pub const NONCE_MAX_CHARS: usize = 64;
/// Largest forward or quote comment in UTF-8 bytes (same cap as a message body).
pub const FORWARD_BODY_MAX_BYTES: usize = MESSAGE_BODY_MAX_BYTES;
/// Longest channel topic in characters (S1 §6.9).
pub const TOPIC_MAX_CHARS: usize = 250;
/// Longest channel purpose in characters (S1 §6.9).
pub const PURPOSE_MAX_CHARS: usize = 250;

// ---- Messaging (S13 §17, gaps G18, G19) ----

/// Distinct member mention targets per message.
pub const MENTION_MEMBERS_MAX: usize = 50;
/// Role plus user-group mention targets per message.
pub const MENTION_ROLES_GROUPS_MAX: usize = 10;
/// Distinct reactions one member may add to one message.
pub const REACTIONS_PER_MEMBER_PER_MESSAGE_MAX: usize = 20;
/// Pinned messages per channel.
pub const PINS_PER_CHANNEL_MAX: usize = 250;
/// Versions (edits) per message.
pub const REVISIONS_PER_MESSAGE_MAX: u32 = 1_000;
/// Mention fan-out recipients per transaction.
pub const FANOUT_BATCH: usize = 1_000;
/// Replies deleted per transaction in a whole-thread delete.
pub const THREAD_DELETE_BATCH: usize = 1_000;
/// Edits, deletes, reactions and pins per member per channel per window (S13 gap G18).
pub const MESSAGE_OPS_RATE_COUNT: u32 = 30;
/// Window for [`MESSAGE_OPS_RATE_COUNT`].
pub const MESSAGE_OPS_RATE_WINDOW_MS: u64 = 10_000;
/// Request body cap on message routes, so `413 too_large` comes from validation (S13 G23).
pub const MESSAGE_REQUEST_BODY_MAX_BYTES: usize = 262_144;

// ---- Rates (C5, C6, C7, C16) ----

/// Messages per member per channel per window (C16: 10 per 10 s).
pub const MESSAGE_RATE_COUNT: u32 = 10;
/// Window for [`MESSAGE_RATE_COUNT`].
pub const MESSAGE_RATE_WINDOW_MS: u64 = 10_000;
/// HTTP requests per second per session (C16).
pub const HTTP_RATE_PER_S: u32 = 50;
/// HTTP burst per session (C16).
pub const HTTP_BURST: u32 = 100;
/// One IDENTIFY per token per this many ms (C7, C16).
pub const IDENTIFY_PER_TOKEN_INTERVAL_MS: u64 = 5_000;
/// IDENTIFYs per second per install (C7, C16).
pub const IDENTIFY_PER_INSTALL_PER_S: u32 = 100;
/// Failed standalone logins per IP per window (C6 said 5 attempts; contracts rev 9 counts only
/// failures, 20 per 300 s, so a shared office NAT is not locked out).
pub const LOGIN_FAILURES_PER_IP: u32 = 20;
/// Window for [`LOGIN_FAILURES_PER_IP`] (C6).
pub const LOGIN_RATE_WINDOW_MS: u64 = 300_000;
/// Minimum gap between JWKS refetches on an unknown `kid` (C6).
pub const JWKS_REFETCH_MIN_INTERVAL_MS: u64 = 30_000;

// ---- Uploads and signed URLs (C5, C11, C16) ----

/// Uploads in flight per member (C16).
pub const UPLOADS_IN_FLIGHT_PER_MEMBER: u32 = 5;
/// Signed download URL lifetime (C5: 1 h).
pub const SIGNED_DOWNLOAD_URL_TTL_MS: u64 = 3_600_000;
/// Signed upload URL lifetime (C5: 1 h).
pub const SIGNED_UPLOAD_URL_TTL_MS: u64 = 3_600_000;
/// Lifetime of an `upload_pending` blob link (C11: 24 h).
pub const PENDING_UPLOAD_TTL_MS: u64 = 86_400_000;
/// Grace before the sweep deletes a blob with no live link (C11: 24 h).
pub const BLOB_SWEEP_GRACE_MS: u64 = 86_400_000;

// ---- Files (S15 §17, contracts rev 25 G13) ----

/// Largest upload file name in UTF-8 bytes (after the server keeps the last path segment).
pub const FILENAME_MAX_BYTES: usize = 255;
/// Longest declared upload content type in bytes.
pub const UPLOAD_CONTENT_TYPE_MAX_BYTES: usize = 255;
/// Plaintext bytes per upload chunk (S7, fixed in v1): data requests end on a multiple of it.
pub const UPLOAD_CHUNK_SIZE: u32 = 1_048_576;
/// Largest data request of an upload (16 MiB, under K2's 25 MiB `--sew` gateway cap).
pub const UPLOAD_REQUEST_MAX_BYTES: u64 = 16_777_216;
/// An upload stops counting as in flight after this long without a data request.
pub const UPLOAD_IN_FLIGHT_IDLE_MS: u64 = 600_000;
/// Upload creates per member per window (the `uploads` bucket).
pub const UPLOAD_RATE_COUNT: u32 = 60;
/// Window for [`UPLOAD_RATE_COUNT`].
pub const UPLOAD_RATE_WINDOW_MS: u64 = 60_000;
/// Signed URLs keep one string per window, so renders inside it are identical (S15 §11.1).
pub const SIGNED_URL_WINDOW_MS: u64 = 300_000;
/// A data upload or byte download with no byte for this long is cut (S15 G12).
pub const FILE_IDLE_TIMEOUT_MS: u64 = 30_000;
/// Largest total of a cross-space forward's file copies (S15 §14).
pub const FORWARD_COPY_MAX_BYTES: u64 = 2_147_483_648;

// ---- Structure (C4, C16, gap G3) ----

/// Spaces per install (C16, soft).
pub const SPACES_PER_INSTALL_MAX: u32 = 1_000;
/// Channels per space (C16, soft).
pub const CHANNELS_PER_SPACE_MAX: u32 = 5_000;
/// People in a group DM, including the opener (C16).
pub const GROUP_DM_MAX_PEOPLE: usize = 9;
/// Other people a DM request may name (group DM size minus the opener).
pub const GROUP_DM_MAX_OTHERS: usize = GROUP_DM_MAX_PEOPLE - 1;
/// Longest slug in bytes (C4).
pub const SLUG_MAX_LEN: usize = 80;
/// Longest space, category or channel name in characters.
pub const NAME_MAX_CHARS: usize = 100;
/// Most default channels of a space (Hearth HC6, prd-hearth-01 §5.2 GH20).
pub const SPACE_DEFAULT_CHANNELS_MAX: usize = 10;
/// Longest space description in characters (Hearth HC4, prd-hearth-01 §5.2 GH4).
pub const SPACE_DESCRIPTION_MAX_CHARS: usize = 300;
/// Shortest handle (gap G3).
pub const HANDLE_MIN_LEN: usize = 2;
/// Longest handle (gap G3).
pub const HANDLE_MAX_LEN: usize = 64;
/// Longest external id in bytes (S1 §6.4).
pub const EXTERNAL_ID_MAX_BYTES: usize = 128;
/// Longest standalone password in bytes (S1 §6.9).
pub const PASSWORD_MAX_BYTES: usize = 1_024;
/// Longest display name in characters (C9).
pub const DISPLAY_NAME_MAX_CHARS: usize = 100;
/// Longest profile title in characters (C9).
pub const TITLE_MAX_CHARS: usize = 100;
/// Longest pronouns field in characters (C9).
pub const PRONOUNS_MAX_CHARS: usize = 40;
/// Longest email address in bytes (RFC 5321 path limit).
pub const EMAIL_MAX_BYTES: usize = 254;
/// Longest K2 login session id (`sid`, `k2_session`) in bytes (gap G31, contracts rev 15).
pub const K2_SESSION_ID_MAX_LEN: usize = 128;
/// Longest gateway client name or version in characters (C7).
pub const CLIENT_INFO_MAX_CHARS: usize = 64;

// ---- Pagination (C5, C8, gap G28) ----

/// Default page size (C5).
pub const PAGE_LIMIT_DEFAULT: u32 = 50;
/// Largest page size (C5).
pub const PAGE_LIMIT_MAX: u32 = 100;
/// Largest `ids=` list (C8).
pub const IDS_PER_QUERY_MAX: usize = 100;
/// Default change catch-up page (gap G28).
pub const CHANGES_LIMIT_DEFAULT: u32 = 500;
/// Largest change catch-up page (gap G28).
pub const CHANGES_LIMIT_MAX: u32 = 500;

// ---- Idempotency (C5) ----

/// Shortest `Idempotency-Key`.
pub const IDEMPOTENCY_KEY_MIN_LEN: usize = 1;
/// Longest `Idempotency-Key`.
pub const IDEMPOTENCY_KEY_MAX_LEN: usize = 64;
/// How long stored idempotent responses are kept (C5: 24 h).
pub const IDEMPOTENCY_TTL_MS: u64 = 86_400_000;

// ---- Sessions (C6) ----

/// K2-provided session lifetime (1 h).
pub const K2_SESSION_TTL_MS: u64 = 3_600_000;
/// How long the old token stays valid after a renew (60 s).
pub const RENEW_GRACE_MS: u64 = 60_000;
/// Standalone session lifetime (30 days).
pub const STANDALONE_SESSION_TTL_MS: u64 = 2_592_000_000;
/// Standalone session idle timeout (7 days).
pub const STANDALONE_SESSION_IDLE_MS: u64 = 604_800_000;
/// Gateway ticket lifetime (30 s).
pub const GATEWAY_TICKET_TTL_MS: u64 = 30_000;
/// Longest allowed K2 pass life, `exp - iat` (120 s).
pub const PASS_MAX_LIFE_MS: u64 = 120_000;
/// Clock skew allowed on pass checks (30 s).
pub const PASS_CLOCK_SKEW_MS: u64 = 30_000;
/// Live sessions kept per member; creating one more revokes the oldest (S8 gap S8-G17).
pub const SESSIONS_PER_MEMBER_MAX: u64 = 100;
/// Standalone one-time password setup link lifetime (24 h, S8 gap S8-G7).
pub const SETUP_TOKEN_TTL_MS: u64 = 86_400_000;
/// Shortest standalone password, in characters (S8 §8.4).
pub const PASSWORD_MIN_CHARS: usize = 12;

// ---- Gateway (C7, gaps G11, G13) ----

/// Heartbeat interval sent in HELLO.
pub const HEARTBEAT_INTERVAL_MS: u64 = 30_000;
/// The server closes `4009` after this long without a heartbeat (1.5 × the interval).
pub const HEARTBEAT_TIMEOUT_MS: u64 = 45_000;
/// IDENTIFY or RESUME must arrive within this long after HELLO, else `4003`.
pub const IDENTIFY_TIMEOUT_MS: u64 = 10_000;
/// At most one TYPING per channel per this interval.
pub const TYPING_INTERVAL_MS: u64 = 8_000;
/// Typing channels per subscription.
pub const SUBSCRIBE_TYPING_MAX: usize = 100;
/// Presence ids per subscription.
pub const SUBSCRIBE_PRESENCE_MAX: usize = 500;
/// Member-list ranges per subscription.
pub const MEMBER_LIST_RANGES_MAX: usize = 5;
/// Rows per member-list range (`hi - lo < 100`).
pub const MEMBER_LIST_RANGE_LEN_MAX: u32 = 100;
/// Outbound queue per socket, in frames.
pub const OUTBOUND_QUEUE_FRAMES: usize = 1_024;
/// Outbound queue per socket, in bytes (8 MiB).
pub const OUTBOUND_QUEUE_BYTES: usize = 8 * 1024 * 1024;
/// Past this fill percentage, lossy events are dropped for a socket.
pub const LOSSY_DROP_PERCENT: u32 = 50;
/// Replay buffer size in dispatches.
pub const REPLAY_MAX_FRAMES: usize = 2_000;
/// Replay buffer age.
pub const REPLAY_MAX_AGE_MS: u64 = 300_000;
/// Replay buffer size in bytes (4 MiB).
pub const REPLAY_MAX_BYTES: usize = 4 * 1024 * 1024;
/// A session stays resumable this long after a disconnect.
pub const RESUME_WINDOW_MS: u64 = 300_000;
/// Upper bound (exclusive) of the RECONNECT `after_ms` jitter on shutdown.
pub const RECONNECT_JITTER_MAX_MS: u64 = 15_000;
/// Client reconnect backoff floor.
pub const CLIENT_BACKOFF_MIN_MS: u64 = 1_000;
/// Client reconnect backoff ceiling.
pub const CLIENT_BACKOFF_MAX_MS: u64 = 60_000;

// ---- Change log (C8) ----

/// Channel change entries are kept at least this long (30 days).
pub const CHANNEL_CHANGES_KEEP_MS: u64 = 2_592_000_000;
/// Channel change entries kept at least this many per channel.
pub const CHANNEL_CHANGES_KEEP_ENTRIES: u64 = 10_000;
/// Member change entries are kept this long (30 days).
pub const MEMBER_CHANGES_KEEP_MS: u64 = 2_592_000_000;

// ---- Mgmt (C9) ----

/// Longest agent deliveries long-poll, and its default.
pub const DELIVERIES_LONG_POLL_MAX_MS: u64 = 25_000;

// ---- Addresses (C4) ----

/// Longest chat address in bytes.
pub const ADDR_MAX_BYTES: usize = 256;

// ---- Roles, overwrites, moderation (S12 §5.6) ----

/// Roles per space, `@everyone` included.
pub const ROLES_PER_SPACE: u32 = 250;
/// Longest role name in characters.
pub const ROLE_NAME_MAX: usize = 100;
/// Role ids in one assign or unassign request.
pub const ROLE_IDS_PER_REQUEST: usize = 25;
/// Overwrites per channel or category.
pub const OVERWRITES_PER_SCOPE: u32 = 100;
/// Shortest timeout (1 min).
pub const TIMEOUT_MIN_MS: u64 = 60_000;
/// Longest timeout (28 days, S5 G6).
pub const TIMEOUT_MAX_MS: u64 = 2_419_200_000;
/// Longest slow mode (6 h).
pub const SLOW_MODE_MAX_S: u32 = 21_600;
/// Affected members up to which `perms_changed` is computed inline (server config
/// `[perms] inline_diff_max`, default); above it a `perms_fanout` job sends it.
pub const INLINE_PERMS_DIFF_MAX: u32 = 2_000;
/// Members per `perms_fanout` transaction.
pub const PERMS_FANOUT_BATCH: u32 = 1_000;

// ---- Emoji, GIFs, link previews (S16 §14) ----

/// Custom emoji aliases per emoji.
pub const EMOJI_ALIASES_MAX: usize = 5;
/// Shortest custom emoji name or alias.
pub const EMOJI_NAME_MIN_CHARS: usize = 2;
/// Longest custom emoji name or alias.
pub const EMOJI_NAME_MAX_CHARS: usize = 32;
/// Largest custom emoji upload (default of `[emoji] max_upload_bytes`).
pub const EMOJI_UPLOAD_MAX_BYTES: u64 = 262_144;
/// Largest source side of a custom emoji image in pixels.
pub const EMOJI_SOURCE_MAX_SIDE: u32 = 1_024;
/// Largest stored custom emoji side in pixels.
pub const EMOJI_SIDE: u32 = 128;
/// Most frames in an animated custom emoji.
pub const EMOJI_FRAMES_MAX: usize = 300;
/// Largest stored custom emoji image.
pub const EMOJI_OUTPUT_MAX_BYTES: u64 = 524_288;
/// Distinct custom emoji tokens per message body.
pub const EMOJI_TOKENS_PER_BODY_MAX: usize = 50;
/// Largest space icon upload (default of `[media] space_icon_max_bytes`).
pub const SPACE_ICON_MAX_BYTES: u64 = 5_242_880;
/// Largest space icon source side in pixels.
pub const SPACE_ICON_SOURCE_MAX_SIDE: u32 = 4_096;
/// Longest URL a link preview considers, in bytes.
pub const PREVIEW_URL_MAX_BYTES: usize = 2_048;
/// Longest preview title in characters.
pub const PREVIEW_TITLE_MAX_CHARS: usize = 256;
/// Longest preview description in characters.
pub const PREVIEW_DESCRIPTION_MAX_CHARS: usize = 512;
/// Longest preview site name in characters.
pub const PREVIEW_SITE_NAME_MAX_CHARS: usize = 100;
/// Largest stored preview image side in pixels.
pub const PREVIEW_IMAGE_SIDE: u32 = 800;
/// Largest GIF fetched for search media or attach (default of `[gifs] max_bytes`).
pub const GIF_MAX_BYTES: u64 = 8_388_608;
/// Custom emoji writes per member per space per window.
pub const EMOJI_WRITE_RATE_COUNT: u32 = 20;
/// Window for [`EMOJI_WRITE_RATE_COUNT`].
pub const EMOJI_WRITE_RATE_WINDOW_MS: u64 = 600_000;
/// Space icon writes per space per window.
pub const SPACE_ICON_RATE_COUNT: u32 = 5;
/// Window for [`SPACE_ICON_RATE_COUNT`].
pub const SPACE_ICON_RATE_WINDOW_MS: u64 = 600_000;
/// GIF search, trending and attach calls per member per window.
pub const GIFS_RATE_COUNT: u32 = 30;
/// Window for [`GIFS_RATE_COUNT`].
pub const GIFS_RATE_WINDOW_MS: u64 = 60_000;
/// Provider calls (cache misses) per install per second.
pub const GIFS_INSTALL_PER_S: u32 = 10;
/// Secret writes per member (or the mgmt service) per window.
pub const SECRETS_RATE_COUNT: u32 = 10;
/// Window for [`SECRETS_RATE_COUNT`].
pub const SECRETS_RATE_WINDOW_MS: u64 = 600_000;

// ---- S17: drafts, saved, scheduled, reminders, polls, bookmarks (prd-sew-17 §16) ----

/// Largest draft or scheduled-message body in bytes (as a message body).
pub const DRAFT_BODY_MAX_BYTES: usize = 16_384;
/// Uploads per draft or scheduled message.
pub const DRAFT_UPLOADS_MAX: usize = 10;
/// Drafts per member.
pub const DRAFTS_PER_MEMBER_MAX: usize = 500;
/// Saved items per member (every state).
pub const SAVED_PER_MEMBER_MAX: usize = 10_000;
/// Largest saved-item or reminder note in bytes.
pub const NOTE_MAX_BYTES: usize = 2_048;
/// Scheduled messages in state `pending` or `failed` per member.
pub const SCHEDULED_PER_MEMBER_MAX: usize = 100;
/// Earliest scheduled send: now + this.
pub const SCHEDULED_MIN_AHEAD_MS: u64 = 10_000;
/// Latest scheduled send: now + this (120 days).
pub const SCHEDULED_MAX_AHEAD_MS: u64 = 120 * 86_400_000;
/// A scheduled message later than this after `send_at` fails `overdue`.
pub const SCHEDULED_GRACE_MS: u64 = 3_600_000;
/// Sent scheduled rows are kept this long after `sent_at`.
pub const SCHEDULED_SENT_KEEP_MS: u64 = 7 * 86_400_000;
/// Reminders in state `pending` or `fired` per member.
pub const REMINDERS_PER_MEMBER_MAX: usize = 500;
/// Earliest reminder: now + this.
pub const REMINDER_MIN_AHEAD_MS: u64 = 10_000;
/// Latest reminder: now + this (366 days).
pub const REMINDER_MAX_AHEAD_MS: u64 = 366 * 86_400_000;
/// Done reminders are kept this long after `done_at`.
pub const REMINDER_DONE_KEEP_MS: u64 = 30 * 86_400_000;
/// Longest poll question in characters.
pub const POLL_QUESTION_MAX_CHARS: usize = 300;
/// Fewest poll options.
pub const POLL_OPTIONS_MIN: usize = 2;
/// Most poll options.
pub const POLL_OPTIONS_MAX: usize = 10;
/// Longest poll option in characters.
pub const POLL_OPTION_MAX_CHARS: usize = 100;
/// Shortest timed poll in seconds.
pub const POLL_DURATION_MIN_S: u32 = 60;
/// Longest timed poll in seconds (32 days).
pub const POLL_DURATION_MAX_S: u32 = 2_764_800;
/// Channel-wide `poll_updated` events per poll per window (plus one trailing event).
pub const POLL_EVENT_WINDOW_MS: u64 = 1_000;
/// Bookmarks per channel.
pub const BOOKMARKS_PER_CHANNEL_MAX: usize = 100;
/// Longest bookmark title in characters.
pub const BOOKMARK_TITLE_MAX_CHARS: usize = 100;
/// Longest bookmark URL in bytes.
pub const BOOKMARK_URL_MAX_BYTES: usize = 2_048;
/// Draft saves and deletes per member per window (bucket `drafts`).
pub const DRAFTS_RATE_COUNT: u32 = 120;
/// Window for [`DRAFTS_RATE_COUNT`].
pub const DRAFTS_RATE_WINDOW_MS: u64 = 60_000;
/// Saved, scheduled, reminder and bookmark writes per member per window (bucket `s17_ops`).
pub const S17_OPS_RATE_COUNT: u32 = 30;
/// Window for [`S17_OPS_RATE_COUNT`].
pub const S17_OPS_RATE_WINDOW_MS: u64 = 10_000;
// ---- Sync: read state, unread, threads inbox (S14 §15, §17) ----

/// Channel items per bulk ack.
pub const BULK_ACK_CHANNELS_MAX: usize = 500;
/// Explicit thread items per bulk ack.
pub const BULK_ACK_THREADS_MAX: usize = 500;
/// Explicit plus implied threads one bulk ack processes (server config `[sync]
/// bulk_max_threads`, default and ceiling).
pub const BULK_MAX_THREADS: u32 = 5_000;
/// Acks, mark-unreads and bulk acks per member per window (bucket `read_acks`).
pub const READ_ACKS_RATE_COUNT: u32 = 100;
/// Window for [`READ_ACKS_RATE_COUNT`].
pub const READ_ACKS_RATE_WINDOW_MS: u64 = 10_000;
/// Mention inbox rows are kept at least this many days after they are read (`[sync]
/// mention_keep_days` default).
pub const MENTION_KEEP_DAYS: u32 = 30;
/// Mark unread works on messages up to this many days old (`[sync] mark_unread_window_days`
/// default; never more than the keep window).
pub const MARK_UNREAD_WINDOW_DAYS: u32 = 30;
/// Longest `wait_ms` long-poll on message lists (contracts rev 31, S30 G3).
pub const MESSAGES_WAIT_MAX_MS: u32 = 25_000;

// ---- Search (S20 §17) ----

/// Longest search query `q` in bytes after trimming (`query_too_long`).
pub const SEARCH_Q_MAX_BYTES: usize = 1_000;
/// Text clauses per query (`too_many_terms`).
pub const SEARCH_TEXT_CLAUSES_MAX: usize = 32;
/// Filter clauses per query (`too_many_filters`).
pub const SEARCH_FILTERS_MAX: usize = 20;
/// Values of one filter name per query (`too_many_filters`).
pub const SEARCH_FILTER_VALUES_MAX: usize = 10;
/// Shortest `word*` prefix in characters (`prefix_too_short`).
pub const SEARCH_PREFIX_MIN_CHARS: usize = 2;
/// Most distinct terms a prefix may expand to (`prefix_too_broad`).
pub const SEARCH_PREFIX_TERMS_MAX: usize = 1_000;
/// Default search page size (a C5 exception, contracts rev 27).
pub const SEARCH_PAGE_LIMIT_DEFAULT: u32 = 20;
/// Largest search page size.
pub const SEARCH_PAGE_LIMIT_MAX: u32 = 100;
/// Candidate rounds per search page before a short page with `has_more`.
pub const SEARCH_CANDIDATE_ROUNDS: u32 = 3;
/// A search cursor lives this long (`cursor_expired`).
pub const SEARCH_CURSOR_TTL_MS: u64 = 1_800_000;
/// Query deadline (`503 search_timeout`).
pub const SEARCH_QUERY_TIMEOUT_MS: u64 = 5_000;
/// Concurrent queries per process (`503 search_busy`).
pub const SEARCH_CONCURRENT_QUERIES: u32 = 8;
/// How long a query waits for a slot before `search_busy`.
pub const SEARCH_QUEUE_WAIT_MS: u64 = 1_000;
/// Ids in one `search_delete {message_ids}` job.
pub const SEARCH_DELETE_IDS_MAX: usize = 150;
/// Searches per member per window (bucket `search`).
pub const SEARCH_RATE_COUNT: u32 = 30;
/// The `search` bucket's window.
pub const SEARCH_RATE_WINDOW_MS: u64 = 60_000;
/// Longest `reason` of a mgmt search rebuild request, in bytes.
pub const SEARCH_REBUILD_REASON_MAX_BYTES: usize = 200;
