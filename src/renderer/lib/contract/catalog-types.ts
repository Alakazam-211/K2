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
export type CatalogExposure = 'widget' | 'app'

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
  http?: { method: 'GET' | 'POST'; route: string; params?: Record<string, string> }
  /** Sockets are read-only. */
  socket?: { path: string; frame: string; refetch?: string }
}

export interface CatalogVerbRow {
  /** `surface.action` */
  verb: string
  cap: string | null
  /** Absent or empty = none (K2 built-ins only). */
  exposure?: CatalogExposure[]
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
  'failed',
] as const

export type CatalogErrorCode = (typeof CATALOG_ERROR_CODES)[number]

export interface CatalogErrorRow {
  code: CatalogErrorCode
  /** Extra fields the error carries: `cap`, `room`, `feature`. */
  fields?: Array<'cap' | 'room' | 'feature'>
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
}
