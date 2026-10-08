// Custom Garden widgets: the wire shapes between the daemon, the renderer
// and the sealed frame (prd-zen-user-widgets-v2 UW15, UW35, UW38, UWB4,
// UWB6–UWB9, UWB21–UWB23).
//
// Day-0 interface (Zen v2, 2026-10-08). Owner: B4 (renderer). The Rust side
// is `crates/k2-core/src/zen/grants.rs` (GrantView, Scope, GrantEntry) and
// `crates/k2-core/src/zen/garden_catalog.rs` (TemplateInfo), owned by B2.
// Change a shape only with the integrator, on both sides in one commit.

import type { CatalogWireError } from '../contract/catalog-types'
import type { UserWidgetCap } from '../k2-caps.generated'
import type { ZenLibRef } from './zen-lib-loader'

// ── Scope and grant (UWB4, UWB7) ─────────────────────────────────────────

/**
 * What a widget may see; exactly one key. The renderer resolves it to rows
 * at Allow and at every mount (Homes are device-local). Bound rows are the
 * union of the scope's rows now (QA1); a missing Home gives fewer rows,
 * never the window's Home or a loose view.
 */
export type ZenScope =
  | { agent: string } // `handle::host`
  | { home: string } // Home id
  | { homes: string[] } // Home ids, 1+, no repeats
  | { allHomes: true }
  | { server: string } // host key
  | { allServers: true } // local + every saved Connect host; ≤ 8 live per widget (R7)

/** One bound row at Allow (UWA8): `server` = host of `handle::host`, `room` = handle. */
export interface ZenGrantEntry {
  server: string
  room: string
}

/**
 * none: no live grant · invalid: signature, key or widget doesn't check out
 * · review: granted for a different home/agent ask · partial: manifest asks
 * for more, granted caps keep working · granted.
 */
export type ZenGrantState = 'none' | 'invalid' | 'review' | 'partial' | 'granted'

export interface ZenGrantPause {
  at: string
  reason: 'runaway'
}

/** `grant` on a custom widget in `GET /cli/zen/get` (Rust `GrantView`). */
export interface ZenGrantView {
  state: ZenGrantState
  /** Effective: requested ∩ granted ∩ USER_WIDGET_CAPS. */
  caps: UserWidgetCap[]
  granted: UserWidgetCap[]
  scope: ZenScope | null
  entries: ZenGrantEntry[]
  sending: boolean
  paused: ZenGrantPause | null
  grantedAt: string | null
  widgetHash: string | null
}

/** True when K2 draws its review card instead of the frame. */
export function zenGrantNeedsReview(grant: ZenGrantView | null, requested: readonly string[]): boolean {
  if (requested.length === 0) return false
  if (!grant) return true
  return grant.state === 'none' || grant.state === 'invalid' || grant.state === 'review'
}

// ── The custom widget in a resolved page (UW38) ─────────────────────────

/** A finding, the Z13 shape. */
export interface ZenWidgetFinding {
  file: string
  line: number
  col: number
  message: string
}

export interface ZenCustomWidgetPayload {
  id: string
  kind: 'custom'
  /** A folder name, or a built-in `k2:<name>@<n>` (UWB21). */
  widget: string
  column: number
  props: { home?: string; agent?: string; config: Record<string, string | number | boolean> }
  /** Effective caps (the daemon's step 1, UW26). */
  caps: UserWidgetCap[]
  /** The manifest's `caps`. */
  requested: UserWidgetCap[]
  source: 'user'
  name: string
  description: string | null
  reasons: Partial<Record<UserWidgetCap, string>>
  /** `requires.libs` from the manifest, in inline order (UWB13). */
  libs: ZenLibRef[]
  hash: string
  state: 'ok' | 'errors' | 'broken'
  errors: ZenWidgetFinding[]
  warnings: ZenWidgetFinding[]
  grant: ZenGrantView | null
}

// ── Routes (UW35, UWB6, UWB15, UWB22, UWB23) ────────────────────────────

/** GET /cli/zen/widget/bundle?widget=<name> */
export interface ZenWidgetBundleResponse {
  ok: true
  widget: string
  /** sha256 of the bundle without nonces: stable across responses. */
  hash: string
  /** 16 random bytes, fresh per response; every script carries it. */
  nonce: string
  html: string
  bytes: number
  /** The widget's state (B2): `ok`, `errors` (last good served) or `broken`. */
  state?: 'ok' | 'errors' | 'broken'
}

/**
 * POST /cli/zen/widget/grant (owner token only; passports, Connect logins
 * and app passes get 403 `owner_only`). The daemon reads the placement's
 * `home`/`agent` ask from the Garden file itself; 409 `widget_changed` when
 * `hash` isn't the current bundle.
 */
export interface ZenWidgetGrantRequest {
  garden: string
  placement: string
  widget: string
  caps: UserWidgetCap[]
  scope: ZenScope
  entries: ZenGrantEntry[]
  /** On by default after Allow (R6). */
  sending: boolean
  /** The hash the dialog showed. */
  hash: string
}

/** POST /cli/zen/widget/revoke: one placement, or every placement of a widget. */
export type ZenWidgetRevokeRequest = { garden: string; placement: string } | { widget: string }

/**
 * POST /cli/zen/widget/sending: off is allowed like revoke (the runaway
 * guard sends `reason: 'runaway'`, which also sets the pause); on is a grant
 * (owner only).
 */
export interface ZenWidgetSendingRequest {
  garden: string
  placement: string
  on: boolean
  reason?: 'runaway' | 'user'
}

/** POST /cli/zen/widget/resume: clears a runaway pause (owner only). */
export interface ZenWidgetResumeRequest {
  garden: string
  placement: string
}

/** GET /cli/zen/widget/grants: the Settings list (UWB3c). */
export interface ZenWidgetGrantRow {
  garden: string
  placement: string
  widget: string
  state: ZenGrantState
  caps: UserWidgetCap[]
  scope: ZenScope
  entries: ZenGrantEntry[]
  sending: boolean
  paused: ZenGrantPause | null
  grantedAt: string
  /** The Garden's name (B2; absent from an older daemon). */
  gardenName?: string
}

/** A catalog Garden's grant (Rust `CatalogGrant`). */
export interface ZenCatalogGrant {
  widget: string
  caps: UserWidgetCap[]
  /** `'local'` = this computer's agents only; null = the person picks. */
  scope: 'local' | null
  consent: string | null
}

/** One row of GET /cli/zen/templates (UWB23; Rust `TemplateInfo`). */
export interface ZenTemplateInfo {
  /** `k2.texting@1`, `k2.blank@1`, `k2.diary@1`, … */
  id: string
  /** What `garden/new {template}` and `gardens.useTemplate` take. */
  short: string
  label: string
  description: string
  /** start = the two New Garden starts; catalog = ready-made Gardens (R5). */
  section: 'start' | 'catalog'
  /** Creating it also grants this (UWB22), in the same click. `scope:
   *  'local'` fixes the scope to this computer's agents (`{server:
   *  'local'}`; the Diary, Rosson 2026-10-08); `consent` is the one plain
   *  sentence New Garden shows beside Create. */
  needsGrant: ZenCatalogGrant | null
  newUsers: boolean
}

/** The fallback when the daemon has no templates route (older daemon). */
export const ZEN_TEMPLATES_FALLBACK: readonly ZenTemplateInfo[] = [
  {
    id: 'k2.texting@1',
    short: 'texting',
    label: 'Start with the default',
    description: 'Garden 1’s layout: your agents beside a conversation.',
    section: 'start',
    needsGrant: null,
    newUsers: true,
  },
  {
    id: 'k2.blank@1',
    short: 'blank',
    label: 'Start empty and ask my agent',
    description: 'An empty page. Your agent builds it with you.',
    section: 'start',
    needsGrant: null,
    newUsers: true,
  },
]

/** POST /cli/zen/garden/new, with the grant a catalog Garden needs (UWB22; owner only when `grant` is set). */
export interface ZenGardenNewRequest {
  name: string
  template?: string
  seedHome?: string
  at?: number
  /** B2: `entries` (the bound rows at Allow, UWA8) may be empty. `scope`
   *  may be left out when the catalog fixes it (the daemon refuses any
   *  other scope then). */
  grant?: { scope?: ZenScope; sending?: boolean; entries?: ZenGrantEntry[] }
}

// ── The frame protocol (UW15, §7) ───────────────────────────────────────

/** Host → frame, once, with the port. */
export interface ZenFrameHello {
  k2: 'hello'
  v: 1
  caps: UserWidgetCap[]
  /** The local daemon's `/boot-status` features. */
  features: string[]
  widget: { id: string; name: string; garden: string }
  config: Record<string, string | number | boolean>
  motion: { reduced: boolean }
}

/** Frame → host over the port. */
export type ZenFrameRequest =
  | { id: number; verb: string; args: unknown[] }
  | { sub: number; verb: string; args: unknown[] }
  | { unsub: number }
  | { ready: true }
  | { pong: number }
  | { error: { message: string; stack?: string } }
  | { chord: string }

/** Host → frame over the port. */
export type ZenFrameReply =
  | { id: number; ok: true; value: unknown }
  | { id: number; ok: false; error: CatalogWireError }
  | { sub: number; value: unknown }
  /** The subscription was refused (cap, budget, bad args, unknown verb):
   *  sent once, and the subscription is over. */
  | { sub: number; error: CatalogWireError }
  | { ping: number }
