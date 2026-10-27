// Custom Garden widgets: the wire shapes between the daemon, the renderer
// and the sealed frame (prd-zen-user-widgets-v2 UW15, UW35, UW38, UWB9,
// UWB21–UWB23; no permissions since Rosson's 2026-10-08 smoke).
//
// Day-0 interface (Zen v2, 2026-10-08). Owner: B4 (renderer). The Rust side
// is `crates/k2-core/src/zen/widget_access.rs` (WidgetOrigin, Pause) and
// `crates/k2-core/src/zen/garden_catalog.rs` (TemplateInfo), owned by B2.
// Change a shape only with the integrator, on both sides in one commit.

import type { CatalogWireError } from '../contract/catalog-types'
import type { UserWidgetCap } from '../k2-caps.generated'
import type { ZenLibRef } from './zen-lib-loader'

// ── Where a widget came from, and the runaway pause ───────────────────

/**
 * Where a widget came from (Rust `WidgetOrigin`). `local` = in your own
 * Garden on this computer (a widget folder or a built-in `k2:`): it runs
 * with every Garden-safe cap it asks for, no review (Rosson 2026-10-08:
 * "If the widget exists, it should be able to interact with agents").
 * `other` = anything else (v4's widgets imported from other people, not
 * built; or an origin this client can't read): no caps until a review
 * exists, and `zenWidgetMayRun` is where that review will go.
 */
export type ZenWidgetOrigin = 'local' | 'other'

/** The runaway guard's pause (Rust `Pause`): posting is paused until the
 *  person clicks Resume. The widget keeps running. `writes` = Garden
 *  data's guard (the same key and value written over and over): its data
 *  writes are refused (`paused`) until Resume. */
export interface ZenWidgetPause {
  at: string
  reason: 'runaway' | 'writes'
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
  /** The caps it runs with: every Garden-safe cap it asks for (no grant). */
  caps: UserWidgetCap[]
  /** The manifest's `caps`. */
  requested: UserWidgetCap[]
  source: 'user'
  name: string
  description: string | null
  reasons: Partial<Record<UserWidgetCap, string>>
  /** `requires.libs` from the manifest, in inline order (UWB13). */
  libs: ZenLibRef[]
  /** 0.46.5 WL7: what the daemon's resolver says to load, in order: exact
   *  `<id>@<version>` keys (each library's needs first, every key once) and
   *  CDN refs as written. The frame loads exactly these; absent from an
   *  older daemon (then `libs`, through the same resolver here). */
  libKeys?: ZenLibRef[]
  hash: string
  state: 'ok' | 'errors' | 'broken'
  errors: ZenWidgetFinding[]
  warnings: ZenWidgetFinding[]
  origin: ZenWidgetOrigin
  /** Set while the runaway guard has its posting paused. */
  paused: ZenWidgetPause | null
  /** Garden v3: the manifest's `sources` (`"config"` or fixed names), or
   *  null / absent (an older daemon). */
  sources?: 'config' | string[] | null
  /** Garden v3: the source addresses this placement binds (`not_bound`
   *  for anything else). Absent from an older daemon. */
  sourceAddresses?: string[]
  /** 0.46.3: `garden` (the frame follows the Garden's light/dark, never
   *  the computer's) unless the manifest says `"colorScheme": "own"`.
   *  Absent from an older daemon: `garden`. */
  colorScheme?: 'garden' | 'own'
  /** prd-garden-access-v1 G26a (switch on only): does the live version pass
   *  the signer gate in this Garden? False: the ⋯ menu reads "History · not
   *  signed". Absent with Garden access off. */
  signed?: boolean
  /** 0.46.5 WS12: the running version's code size and the warning line
   *  (the ⋯ menu's "Large widget" note reads it; never the canvas). Absent
   *  from an older daemon. */
  size?: ZenWidgetSize | null
}

/** 0.46.5 WS12: a widget version's code size in bytes, the size K2 warns
 *  above (512 KB) and its limit (1 MB). */
export interface ZenWidgetSize {
  code: number
  codeWarnAt: number
  codeMax: number
}

/** The v4 seam: only a widget from your own Garden runs today. One that
 *  came from someone else will need a review here first (not built). */
export function zenWidgetMayRun(w: Pick<ZenCustomWidgetPayload, 'origin'>): boolean {
  return w.origin === 'local'
}

// ── Routes (UW35, UWB6, UWB15, UWB22, UWB23) ────────────────────────────

/** GET /cli/zen/widget/bundle?widget=<name>&garden=<id> */
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

/** POST /cli/zen/widget/pause: the runaway guard tripped (anything Zen accepts). */
export interface ZenWidgetPauseRequest {
  garden: string
  placement: string
  reason: 'runaway'
}

/** POST /cli/zen/widget/resume: the person's one click (owner token only). */
export interface ZenWidgetResumeRequest {
  garden: string
  placement: string
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
  newUsers: boolean
}

/** `k2.agents.show` (catalog v20, 0.46.3 r10): at most this many addresses
 *  (the custom layer refuses more with `bad_args`; `zen-data` holds them). */
export const ZEN_AGENTS_SHOW_MAX = 50

/** The fallback when the daemon has no templates route (older daemon). */
export const ZEN_TEMPLATES_FALLBACK: readonly ZenTemplateInfo[] = [
  {
    id: 'k2.texting@1',
    short: 'texting',
    // Rosson 2026-10-15: offered by name, next to the Diary (Rust
    // `GARDEN_ONE_LABEL` / `GARDEN_ONE_DESCRIPTION`).
    label: 'Garden 1',
    description: 'K2’s default: your agents beside a conversation, pinned agents on top, live activity above the box.',
    section: 'start',
    newUsers: true,
  },
  {
    id: 'k2.blank@1',
    short: 'blank',
    label: 'Start empty and ask my agent',
    description: 'An empty page. Your agent builds it with you.',
    section: 'start',
    newUsers: true,
  },
]

/** POST /cli/zen/garden/new. A catalog Garden's widgets work at once: there
 *  is nothing to allow. */
export interface ZenGardenNewRequest {
  name: string
  template?: string
  seedHome?: string
  at?: number
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
  /** `bytes`: a call that answers bytes (`blobs.get`) moves one
   *  ArrayBuffer to the frame (the host's transfer list). */
  | { id: number; ok: true; value: unknown; bytes?: ArrayBuffer }
  | { id: number; ok: false; error: CatalogWireError }
  | { sub: number; value: unknown }
  /** The subscription was refused (cap, budget, bad args, unknown verb):
   *  sent once, and the subscription is over. */
  | { sub: number; error: CatalogWireError }
  | { ping: number }
