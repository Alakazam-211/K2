// K2's verb catalog: the row shape (prd-zen-user-widgets-v2 UWA1, UWA9,
// UWA10, UWB29).
//
// Day-0 interface (Zen v2, 2026-10-08). Owner: B1. Hand-written mirror of
// `crates/k2-core/src/contract/mod.rs`; change both in one commit, with the
// integrator. The data is `crates/k2-core/src/contract/catalog.json`; the
// generated files (`lib/zen/zen-verbs.generated.ts`,
// `lib/k2-caps.generated.ts`, `sdk/generated/*`) import these types and are
// never edited by hand.

/** Who outside K2's own code may call a verb or hold a cap. Empty = none. */
export type CatalogExposure = 'widget' | 'app' | 'garden-local'

/** Whether the same call and shape work in an App (UWA4). */
export type CatalogReach = 'portable' | 'local'

/** A `write` row's http binding is POST (UWA3). */
export type CatalogEffect = 'read' | 'write'

/** `call` → Promise; `subscribe` → unsubscribe fn; `event` → host-pushed. */
export type CatalogVerbKind = 'call' | 'subscribe' | 'event'

/** builtin = `zen-bridge.ts` BUILTIN_VERBS; registered = `registerZenVerb`;
 *  frame = custom-widget frame host only (never in ZEN_VERBS). */
export type CatalogRendererImpl = 'builtin' | 'registered' | 'frame'

export interface CatalogBindings {
  renderer?: { impl: CatalogRendererImpl }
  /** `route` is a `ROUTES` row (UWA2); rows carry no auth (UWB29). */
  http?: {
    method: 'GET' | 'POST'
    route: string
    params?: Record<string, string>
    /** `app` rows: catalog response field → live wire field, at any depth (RS7). */
    responseNames?: Record<string, string>
    /** `app` POST rows: arguments sent in the query string. */
    query?: string[]
    /** `app` rows: the one door that forwards it (absent = both). */
    door?: 'helper' | 'link'
    /** `app` GET rows: false = no HEAD twin. */
    head?: boolean
  }
  /** Sockets are read-only. `param` (app rows): the query parameter it needs. */
  socket?: { path: string; frame: string; refetch?: string; param?: string }
}

export interface CatalogVerbRow {
  /** `surface.action` */
  verb: string
  cap: string | null
  /** More caps the verb also needs, checked like `cap` (0.46.5). Absent =
   *  `cap` alone. */
  alsoCaps?: string[]
  /** Its place on `k2 study live` (0.46.5): a live surface. */
  live?: {
    topic: string
    alsoTopics?: string[]
    role: 'read' | 'subscribe' | 'ready'
    pairs?: string[]
    counts?: Record<string, string>
    uses?: string[]
    never?: string
    ready?: string
  }
  /** Absent or empty = none (K2 built-ins only). */
  exposure?: CatalogExposure[]
  /** Where the row runs for a Garden on a paired phone (0.46.6,
   *  prd-gardens-on-phone-v1 §4.3): on every `widget` / `garden-local` row. */
  phone?: 'host' | 'door' | 'later' | 'never'
  reach: CatalogReach
  effect: CatalogEffect
  kind: CatalogVerbKind
  /** `/boot-status` feature key. */
  feature: string
  bindings: CatalogBindings
  /** JSON Schema per argument. */
  request: unknown[]
  /** JSON Schema of the answer or of each pushed value; closed on exposed rows. */
  response: unknown
  errors: CatalogErrorCode[]
  doc: string
  example: string
}

export interface CatalogCapRow {
  name: string
  exposure?: CatalogExposure[]
  /** Settings → Apps words (today's SKIN_CAP_LABELS for app caps). */
  label: string
  /** Garden review-dialog sentence with a `{where}` slot; set on widget caps. */
  sentence?: string
}

/**
 * The shared error list (UWA10, UWB9, UWB10), in catalog order. Pinned by
 * `crates/k2-core/src/contract/mod.rs` tests; `not_open` is gone (UWB10)
 * and `sending_off` is new (UWB9).
 */
export const CATALOG_ERROR_CODES = [
  'cap_not_granted',
  'not_bound',
  'rate_limited',
  'too_large',
  'unknown_verb',
  'verb_unavailable',
  'verb_local',
  'not_exposed',
  'sending_off',
  // Garden v3 data sources (contract-sources-v1 §6).
  'server_not_saved',
  'unreachable',
  'removed',
  'no_history',
  'failed',
  // Garden data (contract-garden-data-v1 §7.2).
  'conflict',
  'space_full',
  'value_too_large',
  'bad_key',
  'not_granted',
  'resync',
  'author_unknown',
  'stale_widget',
  'store_full',
  'bad_tx',
  'tx_expired',
  'busy',
  'paused',
  // 0.46.5 (prd-garden-board-needs-v1 GB3): a value names an unheld blob.
  'blob_missing',
  // Widget SDK guards (0.46.1, catalog 11).
  'bad_args',
  'bad_shape',
  // The App transport (runtime S23): gateway and App-route refusals, plus
  // the server SDK's own two.
  'pass_required',
  'pass_invalid',
  'pass_wrong_link',
  'not_on_link',
  'json_required',
  'skin_room',
  'usage',
  'not_found',
  'bad_login',
  'locked',
  'card_not_pending',
  'guest_required',
  'no_app_pass',
  'no_app_key',
  // Sew's chat pass (S28 §9) through the App transport.
  'no_chat_grant',
  // Sew for Apps (0.46.6): the caps ceiling, the app claim, guest chat, the identity map.
  'no_chat_caps',
  'app_claim_invalid',
  'not_app_granted',
  'chat_off_for_person',
  'shared_person',
  'has_assistants',
  'chat_limit',
  'username_unusable',
  'chat_not_app_scoped',
  'no_chat_member',
  // Sew for Apps A9 (0.46.6): chat.createChannel.
  'sew_feature_missing',
  // App-governed Spaces S4+S5 (catalog v42): chat.channels, chat.setChannels.
  'chat_key_required',
  'sew_unreachable',
  'sew_refused',
  'space_unbound',
  'app_mismatch',
  // Sew 0.6.0: chat.createChannel on a slug of another type.
  'channel_type_mismatch',
  'chat_unavailable',
  // RS-A15 guest rooms (App route + Garden door).
  'app_pass_required',
  'grant_required',
  'outside_app_rooms',
  'above_role',
  'has_role',
  'confirm_required',
  'declined',
  'refresh_invalid',
  'refresh_reused',
  // App users' personal wallets (0.46.3).
  'person_pass_required',
  'wallet_off',
  // A custom App's push notifications (0.46.4, prd-custom-app-push-v1).
  'push_off',
  'not_for_host',
  'channel_unavailable',
  'push_devices_full',
  'push_not_configured',
  'too_many_recipients',
  'push_queue_full',
  'airgap',
  'vault_closed',
  'token_in_use',
  // App phone pairing (0.46.5, catalog v24).
  'pairing_off',
  'pairing_from_device',
  'reauth_required',
  'device_signin_off',
  'host_not_paired',
  'no_public_host',
  'pairing_not_served',
  'pair_code_invalid',
  'pairing_not_from_browser',
  'duplicate_label',
  // Garden access lists (prd-garden-access-v1 G22): the signer gate.
  'needs_editor',
  // 0.46.4: a retired verb (k2.sources.ask until 0.47.0).
  'retired',
  // 0.46.4 (catalog v22): only a person answers a card.
  'card_person_only',
  // App user profile (0.46.5, catalog v25): a person's own edits and pictures.
  'edit_off',
  'type_refused',
  'type_mismatch',
  'bad_image',
  'agent_login',
  // Email Notifications (0.46.5, catalog v26).
  'email_off',
  'no_sender',
  'sender_not_granted',
  'system_sender',
  'recipient_out_of_scope',
  'header_not_allowed',
  'message_too_large',
  'domain_not_ready',
  'dmarc_missing',
  'relay_not_ready',
  'idempotency_conflict',
  'no_mail_path',
  // Catalog v35 (email notifications part 2, N6).
  'email_paused',
  'all_suppressed',
  // Catalog v38 (must-change at next sign-in, 0.46.6).
  'password_change_required',
  'password_unchanged',
  // Catalog v40 (0.46.7 Workbench): git reads on a folder that isn't a repo.
  'not_a_repo',
] as const

export type CatalogErrorCode = (typeof CATALOG_ERROR_CODES)[number]

export interface CatalogErrorRow {
  code: CatalogErrorCode
  /** Extra fields the error carries: `cap`, `room`, `feature`. */
  fields?: Array<'cap' | 'room' | 'feature'>
  /** Worth trying again unchanged (catalog 11). */
  retryable?: boolean
  doc: string
}

export interface Catalog {
  catalogVersion: number
  caps: CatalogCapRow[]
  errors: CatalogErrorRow[]
  bannedFields: string[]
  verbs: CatalogVerbRow[]
}

/** The wire error a refused call carries (frame protocol, §7). */
export interface CatalogWireError {
  code: CatalogErrorCode
  message: string
  cap?: string
  room?: string
  feature?: string
  /** Garden v3: `unreachable` from a sources verb carries the last view K2
   *  has, with phase `stale` (contract-sources-v1 §6). */
  last?: unknown
}
